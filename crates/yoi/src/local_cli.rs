//! Explicit Standalone socket commands; launch and transport remain Host-owned.

use std::path::PathBuf;

use super::{ParseError, required_option_value};

pub(crate) const SERVE_HELP: &str = r#"yoi --local serve

Usage:
  yoi --local serve --socket <PATH> [--profile <REF>] [--workspace <PATH>]
  yoi --local serve --socket <PATH> --worker-id <UUID>

Run a foreground Standalone Host without a TUI. New Workers use the same Profile
resolution and saved Worker store as `yoi --local`. --worker-id restores a saved
Standalone Worker with its saved Profile and working directory; it is not a Runtime id.

Options:
      --socket <PATH>      Host-owned Unix endpoint (required)
      --profile <REF>      Profile for a new Worker; conflicts with --worker-id
      --workspace <PATH>   Working directory for a new Worker (defaults to cwd)
      --worker-id <UUID>   Restore a saved Standalone Worker
  -h, --help               Print help

Connect with `yoi --local attach --socket <PATH>`. The endpoint parent must already
be a private directory owned by this Unix user (mode 0700). Existing endpoints are
never replaced or auto-removed as stale; only same-uid clients are admitted.
Client EOF/disconnect leaves this Host running. Send protocol Method::Shutdown to
explicitly stop the Host, or use SIGINT/SIGTERM for confirmed Host shutdown.
"#;

pub(crate) const ATTACH_HELP: &str = r#"yoi --local attach

Usage:
  yoi --local attach --socket <PATH>

Connect stdin/stdout to an existing Standalone Host: one protocol Method JSON
object per stdin line, one protocol Event JSON object per stdout line (JSONL).
EOF disconnects this client without stopping the Host. Method::Shutdown explicitly
stops the Host. No TUI, Worker spawn, or Backend/Runtime selection is performed.

Options:
      --socket <PATH>      Host-owned Unix endpoint (required)
  -h, --help               Print help
"#;

#[derive(Debug)]
pub(crate) enum LocalCli {
    Help(&'static str),
    Serve(ServeOptions),
    Attach(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ServeOptions {
    pub endpoint: PathBuf,
    pub workspace: Option<PathBuf>,
    pub profile: Option<String>,
    pub restore: Option<protocol::WorkerId>,
}

pub(crate) fn parse(command: &str, args: &[String]) -> Result<LocalCli, ParseError> {
    let serve = command == "serve";
    let mut endpoint = None;
    let mut workspace = None;
    let mut profile = None;
    let mut restore = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if matches!(arg.as_str(), "--help" | "-h") {
            return Ok(LocalCli::Help(if serve { SERVE_HELP } else { ATTACH_HELP }));
        }
        let (option, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(key, value)| (key, Some(value)));
        if !matches!(option, "--socket")
            && !(serve && matches!(option, "--workspace" | "--profile" | "--worker-id"))
        {
            return Err(ParseError(format!("unknown yoi {command} option `{arg}`")));
        }
        let value = if let Some(value) = inline {
            if value.is_empty() {
                return Err(ParseError(format!("{option} requires a value")));
            }
            i += 1;
            value
        } else {
            let value = required_option_value(args, i, option)?;
            i += 2;
            value
        };
        match option {
            "--socket" => set_once(&mut endpoint, PathBuf::from(value), option)?,
            "--workspace" => set_once(&mut workspace, PathBuf::from(value), option)?,
            "--profile" => set_once(&mut profile, value.to_owned(), option)?,
            "--worker-id" => {
                let id = value.parse().map_err(|_| {
                    ParseError("--worker-id requires a Standalone Worker UUID".to_owned())
                })?;
                set_once(&mut restore, id, option)?;
            }
            _ => unreachable!("validated option"),
        }
    }
    let endpoint = endpoint
        .ok_or_else(|| ParseError(format!("yoi --local {command} requires --socket <PATH>")))?;
    if !serve {
        return Ok(LocalCli::Attach(endpoint));
    }
    if restore.is_some() && (profile.is_some() || workspace.is_some()) {
        return Err(ParseError(
            "--worker-id restores the saved Profile and cwd; it cannot be combined with --profile or --workspace"
                .to_owned(),
        ));
    }
    Ok(LocalCli::Serve(ServeOptions {
        endpoint,
        workspace,
        profile,
        restore,
    }))
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), ParseError> {
    if slot.is_some() {
        return Err(ParseError(format!("{option} may only be provided once")));
    }
    *slot = Some(value);
    Ok(())
}

pub(crate) async fn run_serve(
    options: ServeOptions,
    state_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let launch = if let Some(worker_id) = options.restore {
        standalone::HeadlessLaunch::Restore {
            state_dir,
            worker_id,
        }
    } else {
        let workspace = options.workspace.map_or_else(std::env::current_dir, Ok)?;
        let profile = options
            .profile
            .map_or(manifest::ProfileSelector::Default, |value| {
                manifest::ProfileSelector::parse_cli(&value)
            });
        let launch =
            standalone::StandaloneLaunchConfig::new(workspace, state_dir, profile, "local")
                .resolve()?;
        standalone::HeadlessLaunch::Fresh(launch)
    };
    standalone::run_headless(launch, options.endpoint).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strings(command: &str, args: &[&str]) -> Result<LocalCli, ParseError> {
        parse(
            command,
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn serve_parses_fresh_launch_and_saved_worker_restore() {
        let LocalCli::Serve(options) = parse_strings(
            "serve",
            &[
                "--socket",
                "/tmp/host.sock",
                "--profile=builtin:standalone",
                "--workspace",
                "/work",
            ],
        )
        .unwrap() else {
            panic!("expected serve")
        };
        assert_eq!(options.endpoint, PathBuf::from("/tmp/host.sock"));
        assert_eq!(options.workspace, Some(PathBuf::from("/work")));
        assert_eq!(options.profile.as_deref(), Some("builtin:standalone"));
        assert_eq!(options.restore, None);

        let id = "01900000-0000-7000-8000-000000000001";
        let LocalCli::Serve(options) =
            parse_strings("serve", &["--socket=/tmp/host.sock", "--worker-id", id]).unwrap()
        else {
            panic!("expected serve")
        };
        assert_eq!(options.restore.unwrap().to_string(), id);
    }

    #[test]
    fn serve_rejects_invalid_restore_or_launch_overrides() {
        for args in [
            vec!["--worker-id", "not-a-uuid"],
            vec![
                "--worker-id",
                "01900000-0000-7000-8000-000000000001",
                "--profile",
                "standalone",
            ],
            vec![
                "--worker-id",
                "01900000-0000-7000-8000-000000000001",
                "--workspace",
                "/work",
            ],
            vec!["--runtime-id", "runtime"],
            vec!["--backend=http://backend.example"],
            vec!["--subject", "subject"],
        ] {
            let input = [vec!["--socket", "/tmp/host.sock"], args].concat();
            assert!(parse_strings("serve", &input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn attach_accepts_only_endpoint_options() {
        for args in [
            vec!["--socket", "/tmp/host.sock"],
            vec!["--socket=/tmp/host.sock"],
        ] {
            let LocalCli::Attach(endpoint) = parse_strings("attach", &args).unwrap() else {
                panic!("expected attach")
            };
            assert_eq!(endpoint, PathBuf::from("/tmp/host.sock"));
        }
        for args in [
            vec!["--profile", "standalone"],
            vec!["--workspace", "/work"],
            vec!["--worker-id", "01900000-0000-7000-8000-000000000001"],
            vec!["--backend", "http://backend.example"],
        ] {
            let input = [vec!["--socket", "/tmp/host.sock"], args].concat();
            assert!(parse_strings("attach", &input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn socket_is_required_nonempty_and_unique() {
        for command in ["serve", "attach"] {
            for args in [
                vec![],
                vec!["--socket"],
                vec!["--socket="],
                vec!["--socket", ""],
                vec!["--socket", "--profile"],
                vec!["--socket=a", "--socket", "b"],
            ] {
                assert!(parse_strings(command, &args).is_err(), "{command} {args:?}");
            }
        }
    }

    #[test]
    fn help_explains_protocol_disconnect_and_explicit_shutdown() {
        for (command, expected) in [("serve", SERVE_HELP), ("attach", ATTACH_HELP)] {
            let LocalCli::Help(help) = parse_strings(command, &["--help"]).unwrap() else {
                panic!("expected help")
            };
            assert_eq!(help, expected);
            assert!(help.contains("Method::Shutdown"));
            assert!(help.contains("--socket <PATH>"));
        }
        assert!(ATTACH_HELP.contains("JSONL"));
        assert!(ATTACH_HELP.contains("EOF disconnects"));
    }
}
