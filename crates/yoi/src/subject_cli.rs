use std::path::Path;

use standalone::subjektiv::StandaloneSubjects;

use super::ParseError;

pub(crate) const HELP: &str = "Local Subjects (operator-managed standalone state):
  yoi --local subject create <ROLE> [--behavior-md <TEXT>]
  yoi --local subject list
  yoi --local --subject <ID> [--profile builtin:standalone-subjektiv]

Create prints a Subject record as JSON; list prints a JSON array.
Selection is explicit and only applies to fresh local launches.
Resume restores the original Subject binding and does not accept --subject.
";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubjectCli {
    Help,
    Create {
        role: String,
        behavior_md: Option<String>,
    },
    List,
}

pub(crate) fn parse(args: &[String]) -> Result<SubjectCli, ParseError> {
    match args.first().map(String::as_str) {
        None => Ok(SubjectCli::Help),
        Some("--help" | "-h") if args.len() == 1 => Ok(SubjectCli::Help),
        Some("list") if args.len() == 1 => Ok(SubjectCli::List),
        Some("create") => {
            let role = args
                .get(1)
                .filter(|role| !role.starts_with('-') && !role.trim().is_empty())
                .ok_or_else(|| ParseError("subject create requires a nonempty ROLE".to_owned()))?;
            let mut behavior_md = None;
            let mut i = 2;
            while i < args.len() {
                if behavior_md.is_some() {
                    return Err(ParseError(
                        "subject create accepts --behavior-md only once".to_owned(),
                    ));
                }
                match args[i].as_str() {
                    "--behavior-md" => {
                        let text = args
                            .get(i + 1)
                            .filter(|text| !text.starts_with("--"))
                            .ok_or_else(|| ParseError("--behavior-md requires TEXT".to_owned()))?;
                        behavior_md = Some(text.clone());
                        i += 2;
                    }
                    arg if arg.starts_with("--behavior-md=") => {
                        behavior_md = Some(arg.trim_start_matches("--behavior-md=").to_owned());
                        i += 1;
                    }
                    arg => {
                        return Err(ParseError(format!(
                            "unknown subject create argument `{arg}`"
                        )));
                    }
                }
            }
            Ok(SubjectCli::Create {
                role: role.clone(),
                behavior_md,
            })
        }
        Some(arg) => Err(ParseError(format!(
            "invalid subject command or arguments `{arg}`; use `yoi --local subject --help`"
        ))),
    }
}

pub(crate) fn run(cli: SubjectCli, state_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if cli == SubjectCli::Help {
        return Ok(HELP.to_owned());
    }
    let subjects = StandaloneSubjects::open(state_dir)?;
    let json = match cli {
        SubjectCli::Create { role, behavior_md } => {
            serde_json::to_string_pretty(&subjects.create(&role, behavior_md.as_deref())?)?
        }
        SubjectCli::List => serde_json::to_string_pretty(&subjects.list()?)?,
        SubjectCli::Help => unreachable!(),
    };
    Ok(format!("{json}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn parse_explicit_create_and_list() {
        assert_eq!(
            parse(&args(&[
                "create",
                "assistant",
                "--behavior-md",
                "Remember decisions."
            ]))
            .unwrap(),
            SubjectCli::Create {
                role: "assistant".to_owned(),
                behavior_md: Some("Remember decisions.".to_owned())
            }
        );
        assert_eq!(parse(&args(&["list"])).unwrap(), SubjectCli::List);
    }

    #[test]
    fn reject_missing_empty_duplicate_and_unknown_arguments() {
        for input in [
            vec!["create"],
            vec!["create", " "],
            vec!["create", "role", "--behavior-md"],
            vec!["create", "role", "--behavior-md=x", "--behavior-md=y"],
            vec!["list", "--subject=id"],
            vec!["create", "role", "--backend=url"],
        ] {
            assert!(parse(&args(&input)).is_err(), "{input:?}");
        }
    }

    #[test]
    fn create_and_list_share_only_the_selected_state_root() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        // New state roots are created privately; temporary parent directories
        // need not meet the state-root permission contract.
        let state_dir = root.path().join("state");
        let other_state_dir = other.path().join("state");
        let created: serde_json::Value = serde_json::from_str(
            &run(
                SubjectCli::Create {
                    role: "assistant".to_owned(),
                    behavior_md: Some("Be precise.".to_owned()),
                },
                &state_dir,
            )
            .unwrap(),
        )
        .unwrap();
        let listed: serde_json::Value =
            serde_json::from_str(&run(SubjectCli::List, &state_dir).unwrap()).unwrap();
        assert_eq!(listed, serde_json::json!([created]));
        let elsewhere: serde_json::Value =
            serde_json::from_str(&run(SubjectCli::List, &other_state_dir).unwrap()).unwrap();
        assert_eq!(elsewhere, serde_json::json!([]));
    }

    #[cfg(unix)]
    #[test]
    fn nonprivate_existing_root_requires_explicit_operator_permission_change() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(run(SubjectCli::List, root.path()).is_err());
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!root.path().join("subjektiv").exists());
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let listed: serde_json::Value =
            serde_json::from_str(&run(SubjectCli::List, root.path()).unwrap()).unwrap();
        assert_eq!(listed, serde_json::json!([]));
    }

    #[test]
    fn help_does_not_open_state() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("absent");
        assert_eq!(run(SubjectCli::Help, &absent).unwrap(), HELP);
        assert!(!absent.exists());
    }
}
