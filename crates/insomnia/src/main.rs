mod memory_lint;

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use client::PodRuntimeCommand;
use memory_lint::{LintCliOptions, LintStatus};
use session_store::SegmentId;
use tui::{LaunchMode, LaunchOptions};

#[derive(Debug)]
enum Mode {
    Help,
    MemoryLintHelp,
    MemoryLint(LintCliOptions),
    PodRuntime(Vec<String>),
    Keys,
    Tui(LaunchMode),
}

#[derive(Debug)]
struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[tokio::main]
async fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("insomnia: {e}");
            eprintln!("try `insomnia --help` for usage.");
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Help => {
            print_help();
            ExitCode::SUCCESS
        }
        Mode::MemoryLintHelp => {
            print_memory_lint_help();
            ExitCode::SUCCESS
        }
        Mode::MemoryLint(options) => match memory_lint::run(&options) {
            Ok(LintStatus::Clean) => ExitCode::SUCCESS,
            Ok(LintStatus::Failed) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("insomnia memory lint: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::PodRuntime(args) => pod::entrypoint::run_cli_from("insomnia pod", args).await,
        Mode::Keys => tui::keys::launch().await,
        Mode::Tui(mode) => {
            let runtime_command = match PodRuntimeCommand::resolve() {
                Ok(command) => command,
                Err(e) => {
                    eprintln!("insomnia: failed to resolve Pod runtime command: {e}");
                    return ExitCode::FAILURE;
                }
            };
            tui::launch(LaunchOptions {
                mode,
                runtime_command,
            })
            .await
        }
    }
}

fn parse_args() -> Result<Mode, ParseError> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Mode, ParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    parse_args_slice(&args)
}

fn parse_args_slice(args: &[String]) -> Result<Mode, ParseError> {
    if args.is_empty() {
        return Ok(Mode::Tui(LaunchMode::Spawn { profile: None }));
    }

    match args[0].as_str() {
        "--help" | "-h" => return Ok(Mode::Help),
        "pod" => return Ok(Mode::PodRuntime(args[1..].to_vec())),
        "keys" => {
            if args.len() != 1 {
                return Err(ParseError("insomnia keys does not accept arguments".into()));
            }
            return Ok(Mode::Keys);
        }
        "memory" if args.get(1).map(String::as_str) == Some("lint") => {
            let lint_args = &args[2..];
            if lint_args.iter().any(|arg| arg == "--help" || arg == "-h") {
                return Ok(Mode::MemoryLintHelp);
            }
            let options =
                memory_lint::parse_lint_args(lint_args).map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::MemoryLint(options));
        }
        "memory" => {
            return Ok(Mode::Tui(LaunchMode::PodName {
                pod_name: "memory".to_string(),
                socket_override: None,
            }));
        }
        _ => {}
    }

    let mut resume = false;
    let mut session = None;
    let mut pod_name = None;
    let mut socket_override = None;
    let mut profile = None;
    let mut multi = false;
    let mut positional = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--resume" | "-r" => {
                resume = true;
                i += 1;
            }
            "--multi" => {
                multi = true;
                i += 1;
            }
            "--session" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--session requires a value".to_string()))?;
                session = Some(parse_session_id(value)?);
                i += 2;
            }
            "--pod" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--pod requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--pod requires a value".to_string()));
                }
                pod_name = Some(value.clone());
                i += 2;
            }
            "--socket" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--socket requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--socket requires a value".to_string()));
                }
                socket_override = Some(PathBuf::from(value));
                i += 2;
            }
            "--profile" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--profile requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--profile requires a value".to_string()));
                }
                profile = Some(value.clone());
                i += 2;
            }
            arg if arg.starts_with("--session=") => {
                let value = arg.trim_start_matches("--session=");
                if value.is_empty() {
                    return Err(ParseError("--session requires a value".to_string()));
                }
                session = Some(parse_session_id(value)?);
                i += 1;
            }
            arg if arg.starts_with("--pod=") => {
                let value = arg.trim_start_matches("--pod=");
                if value.is_empty() {
                    return Err(ParseError("--pod requires a value".to_string()));
                }
                pod_name = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--socket=") => {
                let value = arg.trim_start_matches("--socket=");
                if value.is_empty() {
                    return Err(ParseError("--socket requires a value".to_string()));
                }
                socket_override = Some(PathBuf::from(value));
                i += 1;
            }
            arg if arg.starts_with("--profile=") => {
                let value = arg.trim_start_matches("--profile=");
                if value.is_empty() {
                    return Err(ParseError("--profile requires a value".to_string()));
                }
                profile = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with('-') => {
                return Err(ParseError(format!("unknown argument: {arg}")));
            }
            value => {
                if positional.replace(value.to_string()).is_some() {
                    return Err(ParseError(
                        "only one positional Pod name is supported".to_string(),
                    ));
                }
                i += 1;
            }
        }
    }

    if pod_name.is_some() && positional.is_some() {
        return Err(ParseError(
            "--pod and a positional Pod name are mutually exclusive".to_string(),
        ));
    }

    if profile.is_some()
        && (resume
            || session.is_some()
            || pod_name.is_some()
            || positional.is_some()
            || socket_override.is_some()
            || multi)
    {
        return Err(ParseError(
            "--profile can only be used for fresh spawn".to_string(),
        ));
    }
    if multi && resume {
        return Err(ParseError(
            "--multi and --resume are mutually exclusive".to_string(),
        ));
    }
    if multi && session.is_some() {
        return Err(ParseError(
            "--multi and --session are mutually exclusive".to_string(),
        ));
    }
    if multi && pod_name.is_some() {
        return Err(ParseError(
            "--multi and --pod are mutually exclusive".to_string(),
        ));
    }
    if multi && positional.is_some() {
        return Err(ParseError(
            "--multi cannot be used with a positional Pod name".to_string(),
        ));
    }
    if multi && socket_override.is_some() {
        return Err(ParseError(
            "--multi and --socket are mutually exclusive".to_string(),
        ));
    }
    if pod_name.is_some() && session.is_some() {
        return Err(ParseError(
            "--pod and --session are mutually exclusive".to_string(),
        ));
    }
    if pod_name.is_some() && resume {
        return Err(ParseError(
            "--pod and --resume are mutually exclusive".to_string(),
        ));
    }
    if positional.is_some() && resume {
        return Err(ParseError(
            "--resume cannot be used with a positional Pod name".to_string(),
        ));
    }
    if socket_override.is_some() && pod_name.is_none() && positional.is_none() {
        return Err(ParseError(
            "--socket requires --pod or a positional Pod name".to_string(),
        ));
    }
    if resume && session.is_some() {
        return Err(ParseError(
            "--resume and --session are mutually exclusive".to_string(),
        ));
    }

    if multi {
        return Ok(Mode::Tui(LaunchMode::Multi));
    }
    let pod_name = pod_name.or(positional);
    if let Some(pod_name) = pod_name {
        return Ok(Mode::Tui(LaunchMode::PodName {
            pod_name,
            socket_override,
        }));
    }
    if resume {
        return Ok(Mode::Tui(LaunchMode::Resume));
    }
    if let Some(id) = session {
        return Ok(Mode::Tui(LaunchMode::ResumeWithSession(id)));
    }

    Ok(Mode::Tui(LaunchMode::Spawn { profile }))
}

fn parse_session_id(value: &str) -> Result<SegmentId, ParseError> {
    value
        .parse()
        .map_err(|_| ParseError(format!("invalid --session UUID: {value}")))
}

fn print_help() {
    println!(
        "insomnia\n\nUsage:\n  insomnia [OPTIONS] [POD_NAME]\n  insomnia keys\n  insomnia pod [POD_OPTIONS]\n  insomnia memory lint [OPTIONS]\n\nOptions:\n  -r, --resume           Open the Pod picker and resume/attach a Pod\n      --multi            Open the multi-Pod dashboard\n      --pod <NAME>       Attach/restore/create a Pod by name\n      --socket <PATH>    Attach to a specific Pod socket with --pod\n      --session <UUID>   Resume a specific session segment\n      --profile <REF>    Start a fresh Pod from a profile\n  -h, --help             Print help\n"
    );
}

fn print_memory_lint_help() {
    println!(
        "insomnia memory lint\n\nUsage:\n  insomnia memory lint [OPTIONS]\n\nOptions:\n      --workspace <PATH>       Workspace root to lint (defaults to cwd)\n      --json                   Emit a JSON report\n      --warnings-as-errors     Return failure when warnings are present\n  -h, --help                   Print help\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pod_name_mode() {
        match parse_args_from(["--pod", "agent", "--socket", "/tmp/agent.sock"]).unwrap() {
            Mode::Tui(LaunchMode::PodName {
                pod_name,
                socket_override,
            }) => {
                assert_eq!(pod_name, "agent");
                assert_eq!(socket_override, Some(PathBuf::from("/tmp/agent.sock")));
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_positional_name_uses_pod_name_mode() {
        match parse_args_from(["agent"]).unwrap() {
            Mode::Tui(LaunchMode::PodName {
                pod_name,
                socket_override,
            }) => {
                assert_eq!(pod_name, "agent");
                assert_eq!(socket_override, None);
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_memory_alone_remains_positional_pod_name() {
        match parse_args_from(["memory"]).unwrap() {
            Mode::Tui(LaunchMode::PodName {
                pod_name,
                socket_override,
            }) => {
                assert_eq!(pod_name, "memory");
                assert_eq!(socket_override, None);
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_pod_subcommand_uses_runtime_mode() {
        match parse_args_from(["pod", "--pod", "agent", "--profile", "default"]).unwrap() {
            Mode::PodRuntime(args) => assert_eq!(args, ["--pod", "agent", "--profile", "default"]),
            _ => panic!("expected PodRuntime mode"),
        }
    }

    #[test]
    fn parse_keys_subcommand() {
        match parse_args_from(["keys"]).unwrap() {
            Mode::Keys => {}
            _ => panic!("expected Keys mode"),
        }
    }

    #[test]
    fn parse_literal_pod_name_still_available_with_flag() {
        match parse_args_from(["--pod", "pod"]).unwrap() {
            Mode::Tui(LaunchMode::PodName {
                pod_name,
                socket_override,
            }) => {
                assert_eq!(pod_name, "pod");
                assert_eq!(socket_override, None);
            }
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_memory_lint_mode() {
        match parse_args_from([
            "memory",
            "lint",
            "--workspace",
            "/tmp/ws",
            "--json",
            "--warnings-as-errors",
        ])
        .unwrap()
        {
            Mode::MemoryLint(options) => {
                assert_eq!(options.workspace, Some(PathBuf::from("/tmp/ws")));
                assert!(options.json);
                assert!(options.warnings_as_errors);
            }
            _ => panic!("expected MemoryLint mode"),
        }
    }

    #[test]
    fn parse_memory_lint_rejects_usage_errors() {
        let err = parse_args_from(["memory", "lint", "--workspace"]).unwrap_err();
        assert_eq!(err.to_string(), "--workspace requires a value");
    }

    #[test]
    fn parse_memory_lint_workspace_equals() {
        match parse_args_from(["memory", "lint", "--workspace=/tmp/ws"]).unwrap() {
            Mode::MemoryLint(options) => {
                assert_eq!(options.workspace, Some(PathBuf::from("/tmp/ws")));
                assert!(!options.json);
                assert!(!options.warnings_as_errors);
            }
            _ => panic!("expected MemoryLint mode"),
        }
    }

    #[test]
    fn memory_lint_with_other_second_word_remains_positional_pod_name() {
        match parse_args_from(["memory", "other"]).unwrap() {
            Mode::Tui(LaunchMode::PodName { pod_name, .. }) => assert_eq!(pod_name, "memory"),
            _ => panic!("expected PodName mode"),
        }
    }

    #[test]
    fn parse_rejects_pod_and_session() {
        let segment_id = session_store::new_segment_id().to_string();
        let err = parse_args_from(["--pod", "agent", "--session", &segment_id]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--pod and --session are mutually exclusive"
        );
    }

    #[test]
    fn parse_rejects_resume_and_pod_name_selection() {
        let cases = [
            (
                vec!["-r".to_string(), "--pod".to_string(), "agent".to_string()],
                "--pod and --resume are mutually exclusive",
            ),
            (
                vec!["--pod".to_string(), "agent".to_string(), "-r".to_string()],
                "--pod and --resume are mutually exclusive",
            ),
            (
                vec!["-r".to_string(), "agent".to_string()],
                "--resume cannot be used with a positional Pod name",
            ),
        ];

        for (args, message) in cases {
            let err = parse_args_from(args).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }

    #[test]
    fn parse_profile_spawn_mode() {
        match parse_args_from(["--profile", "/profiles/coder.lua"]).unwrap() {
            Mode::Tui(LaunchMode::Spawn { profile }) => {
                assert_eq!(profile, Some("/profiles/coder.lua".to_string()));
            }
            _ => panic!("expected Spawn mode"),
        }
    }

    #[test]
    fn parse_profile_rejects_resume_attach_modes() {
        let segment_id = session_store::new_segment_id().to_string();
        let cases = [
            (
                vec![
                    "--profile".to_string(),
                    "p.lua".to_string(),
                    "--resume".to_string(),
                ],
                "--profile can only be used for fresh spawn",
            ),
            (
                vec![
                    "--profile".to_string(),
                    "p.lua".to_string(),
                    "--session".to_string(),
                    segment_id,
                ],
                "--profile can only be used for fresh spawn",
            ),
            (
                vec![
                    "--profile".to_string(),
                    "p.lua".to_string(),
                    "--socket".to_string(),
                    "/tmp/insomnia/sock".to_string(),
                ],
                "--profile can only be used for fresh spawn",
            ),
            (
                vec![
                    "--profile".to_string(),
                    "p.lua".to_string(),
                    "agent".to_string(),
                ],
                "--profile can only be used for fresh spawn",
            ),
        ];

        for (args, message) in cases {
            let err = parse_args_from(args).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }

    #[test]
    fn parse_multi_mode() {
        match parse_args_from(["--multi"]).unwrap() {
            Mode::Tui(LaunchMode::Multi) => {}
            _ => panic!("expected Multi mode"),
        }
    }

    #[test]
    fn parse_top_level_help() {
        match parse_args_from(["--help"]).unwrap() {
            Mode::Help => {}
            _ => panic!("expected Help mode"),
        }
    }

    #[test]
    fn parse_memory_lint_help() {
        match parse_args_from(["memory", "lint", "--help"]).unwrap() {
            Mode::MemoryLintHelp => {}
            _ => panic!("expected MemoryLintHelp mode"),
        }
    }

    #[test]
    fn parse_multi_conflicts_are_clear() {
        let segment_id = session_store::new_segment_id().to_string();
        let cases = [
            (
                vec!["--multi".to_string(), "--resume".to_string()],
                "--multi and --resume are mutually exclusive",
            ),
            (
                vec!["--multi".to_string(), "--session".to_string(), segment_id],
                "--multi and --session are mutually exclusive",
            ),
            (
                vec![
                    "--multi".to_string(),
                    "--pod".to_string(),
                    "agent".to_string(),
                ],
                "--multi and --pod are mutually exclusive",
            ),
            (
                vec!["--multi".to_string(), "agent".to_string()],
                "--multi cannot be used with a positional Pod name",
            ),
            (
                vec![
                    "--multi".to_string(),
                    "--socket".to_string(),
                    "/tmp/a.sock".to_string(),
                ],
                "--multi and --socket are mutually exclusive",
            ),
        ];

        for (args, message) in cases {
            let err = parse_args_from(args).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }
}
