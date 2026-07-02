mod mcp_cli;
mod memory_lint;
mod objective_cli;
mod plugin_cli;
mod session_cli;
mod ticket_cli;
mod worker_cleanup_cli;

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use client::{BackendRuntimeTarget, WorkerRuntimeCommand};
use memory_lint::{LintCliOptions, LintStatus};
use session_store::SegmentId;
use tui::{LaunchMode, LaunchOptions};

#[derive(Debug)]
enum Mode {
    Help,
    ResumeHelp,
    MemoryLintHelp,
    MemoryLint(LintCliOptions),
    Mcp(mcp_cli::McpCliCommand),
    Plugin(plugin_cli::PluginCliCommand),
    Objective(objective_cli::ObjectiveCli),
    Session(session_cli::SessionCli),
    WorkerCleanup(worker_cleanup_cli::WorkerCleanupCli),
    Ticket(ticket_cli::TicketCli),
    WorkspaceHelp,
    WorkspaceServer {
        subcommand: String,
        args: Vec<String>,
    },
    WorkerRuntime(Vec<String>),
    Keys,
    SetupModel,
    Tui {
        mode: LaunchMode,
        workspace_root: PathBuf,
    },
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
            eprintln!("yoi: {e}");
            eprintln!("try `yoi --help` for usage.");
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Help => {
            print_help();
            ExitCode::SUCCESS
        }
        Mode::ResumeHelp => {
            print_resume_help();
            ExitCode::SUCCESS
        }
        Mode::MemoryLintHelp => {
            print_memory_lint_help();
            ExitCode::SUCCESS
        }
        Mode::WorkspaceHelp => {
            print_workspace_help();
            ExitCode::SUCCESS
        }
        Mode::WorkspaceServer { subcommand, args } => run_workspace_server(&subcommand, args),
        Mode::MemoryLint(options) => match memory_lint::run(&options) {
            Ok(LintStatus::Clean) => ExitCode::SUCCESS,
            Ok(LintStatus::Failed) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("yoi memory lint: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::Mcp(command) => match mcp_cli::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("yoi mcp: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::Plugin(command) => match plugin_cli::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("yoi plugin: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::Objective(cli) => match objective_cli::run(cli) {
            Ok(output) => {
                print!("{}", output.stdout);
                match output.status {
                    objective_cli::ObjectiveCliStatus::Success => ExitCode::SUCCESS,
                    objective_cli::ObjectiveCliStatus::Failure => ExitCode::FAILURE,
                }
            }
            Err(e) => {
                eprintln!("yoi objective: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::Session(cli) => match session_cli::run(cli) {
            Ok(output) => {
                print!("{}", output.stdout);
                match output.status {
                    session_cli::SessionCliStatus::Success => ExitCode::SUCCESS,
                    session_cli::SessionCliStatus::Failure => ExitCode::FAILURE,
                }
            }
            Err(e) => {
                eprintln!("yoi session: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::WorkerCleanup(cli) => match worker_cleanup_cli::run(cli).await {
            Ok(output) => {
                print!("{}", output.stdout);
                match output.status {
                    worker_cleanup_cli::WorkerCleanupCliStatus::Success => ExitCode::SUCCESS,
                    worker_cleanup_cli::WorkerCleanupCliStatus::Failure => ExitCode::FAILURE,
                }
            }
            Err(e) => {
                eprintln!("yoi worker: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::Ticket(cli) => match ticket_cli::run(cli) {
            Ok(output) => {
                print!("{}", output.stdout);
                match output.status {
                    ticket_cli::TicketCliStatus::Success => ExitCode::SUCCESS,
                    ticket_cli::TicketCliStatus::Failure => ExitCode::FAILURE,
                }
            }
            Err(e) => {
                eprintln!("yoi ticket: {e}");
                ExitCode::FAILURE
            }
        },
        Mode::WorkerRuntime(args) => worker::entrypoint::run_cli_from("yoi worker", args).await,
        Mode::Keys => tui::keys::launch().await,
        Mode::SetupModel => tui::setup_model::launch().await,
        Mode::Tui {
            mode,
            workspace_root,
        } => {
            let runtime_command = match WorkerRuntimeCommand::resolve() {
                Ok(command) => command,
                Err(e) => {
                    eprintln!("yoi: failed to resolve Worker runtime command: {e}");
                    return ExitCode::FAILURE;
                }
            };
            tui::launch(LaunchOptions {
                mode,
                runtime_command,
                workspace_root,
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
        return Ok(Mode::Tui {
            mode: LaunchMode::Spawn {
                worker_name: None,
                profile: None,
            },
            workspace_root: current_dir()?,
        });
    }

    match args[0].as_str() {
        "--help" | "-h" => return Ok(Mode::Help),
        "resume" => return parse_resume_args(&args[1..]),
        "worker" => {
            if let Some(cli) = worker_cleanup_cli::parse_worker_management_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?
            {
                return Ok(Mode::WorkerCleanup(cli));
            }
            return Ok(Mode::WorkerRuntime(args[1..].to_vec()));
        }
        "objective" => {
            let objective_cli = objective_cli::parse_objective_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Objective(objective_cli));
        }
        "session" => {
            let session_cli = session_cli::parse_session_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Session(session_cli));
        }
        "ticket" => {
            let ticket_cli =
                ticket_cli::parse_ticket_args(&args[1..]).map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Ticket(ticket_cli));
        }
        "plugin" => {
            let plugin_cli = parse_plugin_args(&args[1..])?;
            return Ok(Mode::Plugin(plugin_cli));
        }
        "workspace" => {
            return parse_workspace_args(&args[1..]);
        }
        "mcp" => {
            let mcp_cli = parse_mcp_args(&args[1..])?;
            return Ok(Mode::Mcp(mcp_cli));
        }
        "panel" => {
            return Ok(Mode::Tui {
                mode: LaunchMode::Panel,
                workspace_root: parse_panel_workspace(&args[1..])?,
            });
        }
        "keys" => {
            if args.len() != 1 {
                return Err(ParseError("yoi keys does not accept arguments".into()));
            }
            return Ok(Mode::Keys);
        }
        "setup-model" => {
            if args.len() != 1 {
                return Err(ParseError(
                    "yoi setup-model does not accept arguments".into(),
                ));
            }
            return Ok(Mode::SetupModel);
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
            return Err(ParseError(
                "yoi memory requires the `lint` subcommand".to_string(),
            ));
        }
        other if !other.starts_with('-') => {
            return Err(ParseError(format!("unknown command `{other}`")));
        }
        _ => {}
    }

    parse_console_options(args)
}

fn parse_console_options(args: &[String]) -> Result<Mode, ParseError> {
    let mut workspace_root = current_dir()?;
    let mut session = None;
    let mut worker_name = None;
    let mut socket_override = None;
    let mut backend_url = None;
    let mut runtime_id = None;
    let mut worker_id = None;
    let mut profile = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--session" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--session requires a value".to_string()))?;
                session = Some(parse_session_id(value)?);
                i += 2;
            }
            "--worker" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--worker requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--worker requires a value".to_string()));
                }
                worker_name = Some(value.clone());
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
            "--workspace" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
                i += 2;
            }
            "--backend" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--backend requires a URL".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--backend requires a URL".to_string()));
                }
                backend_url = Some(value.clone());
                i += 2;
            }
            "--runtime-id" | "--runtime" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--runtime-id requires a value".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--runtime-id requires a value".to_string()));
                }
                runtime_id = Some(value.clone());
                i += 2;
            }
            "--worker-id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--worker-id requires a value".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--worker-id requires a value".to_string()));
                }
                worker_id = Some(value.clone());
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
            arg if arg.starts_with("--worker=") => {
                let value = arg.trim_start_matches("--worker=");
                if value.is_empty() {
                    return Err(ParseError("--worker requires a value".to_string()));
                }
                worker_name = Some(value.to_string());
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
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
                i += 1;
            }
            arg if arg.starts_with("--backend=") => {
                let value = arg.trim_start_matches("--backend=");
                if value.is_empty() {
                    return Err(ParseError("--backend requires a URL".to_string()));
                }
                backend_url = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--runtime-id=") => {
                let value = arg.trim_start_matches("--runtime-id=");
                if value.is_empty() {
                    return Err(ParseError("--runtime-id requires a value".to_string()));
                }
                runtime_id = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--runtime=") => {
                let value = arg.trim_start_matches("--runtime=");
                if value.is_empty() {
                    return Err(ParseError("--runtime-id requires a value".to_string()));
                }
                runtime_id = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--worker-id=") => {
                let value = arg.trim_start_matches("--worker-id=");
                if value.is_empty() {
                    return Err(ParseError("--worker-id requires a value".to_string()));
                }
                worker_id = Some(value.to_string());
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
                return Err(ParseError(format!(
                    "unknown command `{value}`; use --worker <NAME> to open a Worker by name"
                )));
            }
        }
    }

    let backend_target_present =
        backend_url.is_some() || runtime_id.is_some() || worker_id.is_some();
    if backend_target_present
        && (backend_url.is_none() || runtime_id.is_none() || worker_id.is_none())
    {
        return Err(ParseError(
            "--backend, --runtime-id, and --worker-id are required together".to_string(),
        ));
    }
    if backend_target_present
        && (session.is_some()
            || worker_name.is_some()
            || socket_override.is_some()
            || profile.is_some())
    {
        return Err(ParseError(
            "Backend Runtime API target cannot be combined with --worker, --socket, --session, or --profile".to_string(),
        ));
    }

    if profile.is_some() && (session.is_some() || socket_override.is_some()) {
        return Err(ParseError(
            "--profile can only be used for fresh spawn".to_string(),
        ));
    }
    if socket_override.is_some() && worker_name.is_none() {
        return Err(ParseError("--socket requires --worker".to_string()));
    }
    if socket_override.is_some() && session.is_some() {
        return Err(ParseError(
            "--socket can only be used with --worker attach mode".to_string(),
        ));
    }

    if backend_target_present {
        return Ok(Mode::Tui {
            mode: LaunchMode::BackendRuntime {
                target: BackendRuntimeTarget::new(
                    backend_url.expect("checked by backend_target_present"),
                    runtime_id.expect("checked by backend_target_present"),
                    worker_id.expect("checked by backend_target_present"),
                ),
            },
            workspace_root,
        });
    }

    if let Some(profile) = profile {
        return Ok(Mode::Tui {
            mode: LaunchMode::Spawn {
                worker_name,
                profile: Some(profile),
            },
            workspace_root,
        });
    }
    if let Some(id) = session {
        return Ok(Mode::Tui {
            mode: LaunchMode::ResumeWithSession { id, worker_name },
            workspace_root,
        });
    }
    if let Some(worker_name) = worker_name {
        return Ok(Mode::Tui {
            mode: LaunchMode::WorkerName {
                worker_name,
                socket_override,
            },
            workspace_root,
        });
    }
    Ok(Mode::Tui {
        mode: LaunchMode::Spawn {
            worker_name: None,
            profile: None,
        },
        workspace_root,
    })
}

fn parse_resume_args(args: &[String]) -> Result<Mode, ParseError> {
    let mut workspace_root = current_dir()?;
    let mut workspace_set = false;
    let mut all = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" | "-h" => {
                if args.len() == 1 {
                    return Ok(Mode::ResumeHelp);
                }
                return Err(ParseError(
                    "yoi resume --help does not accept other arguments".to_string(),
                ));
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "--workspace" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace requires a value".to_string()))?;
                if value.starts_with('-') {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
                workspace_set = true;
                i += 2;
            }
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
                workspace_set = true;
                i += 1;
            }
            arg if arg.starts_with('-') => {
                return Err(ParseError(format!("unknown yoi resume option `{arg}`")));
            }
            value => {
                return Err(ParseError(format!(
                    "yoi resume does not accept positional argument `{value}`"
                )));
            }
        }
    }

    if all && workspace_set {
        return Err(ParseError(
            "yoi resume --all and --workspace are mutually exclusive".to_string(),
        ));
    }

    Ok(Mode::Tui {
        mode: LaunchMode::Resume { all },
        workspace_root,
    })
}

fn current_dir() -> Result<PathBuf, ParseError> {
    std::env::current_dir()
        .map_err(|e| ParseError(format!("failed to resolve current directory: {e}")))
}

fn parse_workspace_args(args: &[String]) -> Result<Mode, ParseError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(ParseError(
            "yoi workspace requires `init` or `serve` (try `yoi workspace --help`)".to_string(),
        ));
    };
    match subcommand.as_str() {
        "init" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                return Ok(Mode::WorkspaceHelp);
            }
            Ok(Mode::WorkspaceServer {
                subcommand: "init".to_string(),
                args: rest.to_vec(),
            })
        }
        "serve" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                return Ok(Mode::WorkspaceHelp);
            }
            Ok(Mode::WorkspaceServer {
                subcommand: "serve".to_string(),
                args: rest.to_vec(),
            })
        }
        "--help" | "-h" => Ok(Mode::WorkspaceHelp),
        other => Err(ParseError(format!(
            "unknown yoi workspace subcommand `{other}`; expected `init` or `serve`"
        ))),
    }
}

fn run_workspace_server(subcommand: &str, args: Vec<String>) -> ExitCode {
    let command = match resolve_workspace_server_command() {
        Ok(command) => command,
        Err(error) => {
            eprintln!("yoi workspace: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut child = Command::new(&command);
    child.arg(subcommand);
    child.args(args);
    match child.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).min(255) as u8),
        Err(error) => {
            eprintln!(
                "yoi workspace: failed to launch `{}`: {error}",
                command.to_string_lossy()
            );
            ExitCode::FAILURE
        }
    }
}

fn resolve_workspace_server_command() -> Result<OsString, ParseError> {
    if let Some(value) = std::env::var_os("YOI_WORKSPACE_SERVER_COMMAND") {
        if !value.is_empty() {
            return Ok(value);
        }
    }
    let current = std::env::current_exe()
        .map_err(|error| ParseError(format!("failed to resolve current executable: {error}")))?;
    let sibling = current.with_file_name("yoi-workspace-server");
    Ok(sibling.into_os_string())
}

fn parse_plugin_args(args: &[String]) -> Result<plugin_cli::PluginCliCommand, ParseError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(ParseError(
            "yoi plugin requires `new`, `check`, `pack`, `list`, or `show <ref>`".to_string(),
        ));
    };
    match subcommand.as_str() {
        "new" => {
            let (plugin_args, positional) = parse_plugin_common_args(rest)?;
            match positional.as_slice() {
                [template, destination] => Ok(plugin_cli::PluginCliCommand::New {
                    template: template.clone(),
                    destination: PathBuf::from(destination),
                    args: plugin_args,
                }),
                [] | [_] => Err(ParseError(
                    "yoi plugin new requires a template and destination".to_string(),
                )),
                _ => Err(ParseError(
                    "yoi plugin new accepts exactly a template and destination".to_string(),
                )),
            }
        }
        "check" => {
            let (plugin_args, positional) = parse_plugin_common_args(rest)?;
            match positional.as_slice() {
                [input] => Ok(plugin_cli::PluginCliCommand::Check {
                    input: PathBuf::from(input),
                    args: plugin_args,
                }),
                [] => Err(ParseError(
                    "yoi plugin check requires a plugin directory or .yoi-plugin path".to_string(),
                )),
                _ => Err(ParseError(
                    "yoi plugin check accepts exactly one plugin directory or .yoi-plugin path"
                        .to_string(),
                )),
            }
        }
        "pack" => {
            let (plugin_args, positional, output) = parse_plugin_pack_args(rest)?;
            match positional.as_slice() {
                [input] => Ok(plugin_cli::PluginCliCommand::Pack {
                    input: PathBuf::from(input),
                    output,
                    args: plugin_args,
                }),
                [] => Err(ParseError(
                    "yoi plugin pack requires a plugin directory".to_string(),
                )),
                _ => Err(ParseError(
                    "yoi plugin pack accepts exactly one plugin directory".to_string(),
                )),
            }
        }
        "list" => {
            let (plugin_args, positional) = parse_plugin_common_args(rest)?;
            if !positional.is_empty() {
                return Err(ParseError(
                    "yoi plugin list does not accept positional arguments".to_string(),
                ));
            }
            Ok(plugin_cli::PluginCliCommand::List(plugin_args))
        }
        "show" => {
            let (plugin_args, positional) = parse_plugin_common_args(rest)?;
            match positional.as_slice() {
                [reference] => Ok(plugin_cli::PluginCliCommand::Show {
                    reference: reference.clone(),
                    args: plugin_args,
                }),
                [] => Err(ParseError(
                    "yoi plugin show requires a plugin ref".to_string(),
                )),
                _ => Err(ParseError(
                    "yoi plugin show accepts exactly one plugin ref".to_string(),
                )),
            }
        }
        "--help" | "-h" => Err(ParseError(plugin_usage().to_string())),
        other => Err(ParseError(format!(
            "unknown yoi plugin subcommand `{other}`"
        ))),
    }
}

fn parse_plugin_common_args(
    args: &[String],
) -> Result<(plugin_cli::PluginCliArgs, Vec<String>), ParseError> {
    let mut parsed = plugin_cli::PluginCliArgs::default();
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--workspace" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseError("--workspace requires a value".to_string()));
                };
                parsed.workspace = Some(PathBuf::from(value));
            }
            "--profile" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseError("--profile requires a value".to_string()));
                };
                parsed.profile = Some(value.clone());
            }
            "--help" | "-h" => return Err(ParseError(plugin_usage().to_string())),
            _ if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                parsed.workspace = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--profile=") => {
                let value = arg.trim_start_matches("--profile=");
                if value.is_empty() {
                    return Err(ParseError("--profile requires a value".to_string()));
                }
                parsed.profile = Some(value.to_string());
            }
            _ if arg.starts_with('-') => {
                return Err(ParseError(format!("unknown yoi plugin option `{arg}`")));
            }
            _ => positional.push(arg.clone()),
        }
        index += 1;
    }
    Ok((parsed, positional))
}

fn parse_plugin_pack_args(
    args: &[String],
) -> Result<(plugin_cli::PluginCliArgs, Vec<String>, Option<PathBuf>), ParseError> {
    let mut normalized = Vec::new();
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--output" {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(ParseError("--output requires a value".to_string()));
            };
            output = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--output=") {
            if value.is_empty() {
                return Err(ParseError("--output requires a value".to_string()));
            }
            output = Some(PathBuf::from(value));
        } else {
            normalized.push(arg.clone());
        }
        index += 1;
    }
    let (plugin_args, positional) = parse_plugin_common_args(&normalized)?;
    Ok((plugin_args, positional, output))
}

fn plugin_usage() -> &'static str {
    "usage: yoi plugin new <rust-component-tool|rust-component-service> <path-or-name> [--json]\n       yoi plugin check <path-or-package> [--json]\n       yoi plugin pack <path> [--output <file>] [--json]\n       yoi plugin list [--workspace PATH] [--profile REF] [--json]\n       yoi plugin show <ref> [--workspace PATH] [--profile REF] [--json]"
}

fn parse_mcp_args(args: &[String]) -> Result<mcp_cli::McpCliCommand, ParseError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(ParseError(
            "yoi mcp requires `list`, `show <server>`, `tools [server]`, `resources [server]`, or `prompts [server]`".to_string(),
        ));
    };
    match subcommand.as_str() {
        "list" => {
            let (mcp_args, positional) = parse_mcp_common_args(rest)?;
            if !positional.is_empty() {
                return Err(ParseError(
                    "yoi mcp list does not accept positional arguments".to_string(),
                ));
            }
            Ok(mcp_cli::McpCliCommand::List(mcp_args))
        }
        "show" => {
            let (mcp_args, positional) = parse_mcp_common_args(rest)?;
            match positional.as_slice() {
                [server] => Ok(mcp_cli::McpCliCommand::Show {
                    server: server.clone(),
                    args: mcp_args,
                }),
                [] => Err(ParseError(
                    "yoi mcp show requires a server name".to_string(),
                )),
                _ => Err(ParseError(
                    "yoi mcp show accepts exactly one server name".to_string(),
                )),
            }
        }
        "tools" => {
            let (mcp_args, positional) = parse_mcp_common_args(rest)?;
            match positional.as_slice() {
                [] => Ok(mcp_cli::McpCliCommand::Tools {
                    server: None,
                    args: mcp_args,
                }),
                [server] => Ok(mcp_cli::McpCliCommand::Tools {
                    server: Some(server.clone()),
                    args: mcp_args,
                }),
                _ => Err(ParseError(
                    "yoi mcp tools accepts at most one server name".to_string(),
                )),
            }
        }
        "resources" => {
            let (mcp_args, positional) = parse_mcp_common_args(rest)?;
            match positional.as_slice() {
                [] => Ok(mcp_cli::McpCliCommand::Resources {
                    server: None,
                    args: mcp_args,
                }),
                [server] => Ok(mcp_cli::McpCliCommand::Resources {
                    server: Some(server.clone()),
                    args: mcp_args,
                }),
                _ => Err(ParseError(
                    "yoi mcp resources accepts at most one server name".to_string(),
                )),
            }
        }
        "prompts" => {
            let (mcp_args, positional) = parse_mcp_common_args(rest)?;
            match positional.as_slice() {
                [] => Ok(mcp_cli::McpCliCommand::Prompts {
                    server: None,
                    args: mcp_args,
                }),
                [server] => Ok(mcp_cli::McpCliCommand::Prompts {
                    server: Some(server.clone()),
                    args: mcp_args,
                }),
                _ => Err(ParseError(
                    "yoi mcp prompts accepts at most one server name".to_string(),
                )),
            }
        }
        "--help" | "-h" => Err(ParseError(mcp_usage().to_string())),
        other => Err(ParseError(format!("unknown yoi mcp command: {other}"))),
    }
}

fn parse_mcp_common_args(
    args: &[String],
) -> Result<(mcp_cli::McpCliArgs, Vec<String>), ParseError> {
    let mut mcp_args = mcp_cli::McpCliArgs::default();
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--json" {
            mcp_args.json = true;
            index += 1;
        } else if arg == "--workspace" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| ParseError("--workspace requires a value".to_string()))?;
            if value.starts_with('-') {
                return Err(ParseError("--workspace requires a value".to_string()));
            }
            mcp_args.workspace = Some(PathBuf::from(value));
            index += 2;
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            if value.is_empty() {
                return Err(ParseError("--workspace requires a value".to_string()));
            }
            mcp_args.workspace = Some(PathBuf::from(value));
            index += 1;
        } else if arg == "--profile" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| ParseError("--profile requires a value".to_string()))?;
            if value.starts_with('-') {
                return Err(ParseError("--profile requires a value".to_string()));
            }
            mcp_args.profile = Some(value.clone());
            index += 2;
        } else if let Some(value) = arg.strip_prefix("--profile=") {
            if value.is_empty() {
                return Err(ParseError("--profile requires a value".to_string()));
            }
            mcp_args.profile = Some(value.to_string());
            index += 1;
        } else if arg == "--help" || arg == "-h" {
            return Err(ParseError(mcp_usage().to_string()));
        } else if arg.starts_with('-') {
            return Err(ParseError(format!("unknown yoi mcp argument: {arg}")));
        } else {
            positional.push(arg.clone());
            index += 1;
        }
    }
    Ok((mcp_args, positional))
}

fn mcp_usage() -> &'static str {
    "usage: yoi mcp list [--workspace PATH] [--profile REF] [--json]\n       yoi mcp show <server> [--workspace PATH] [--profile REF] [--json]\n       yoi mcp tools [server] [--workspace PATH] [--profile REF] [--json]\n       yoi mcp resources [server] [--workspace PATH] [--profile REF] [--json]\n       yoi mcp prompts [server] [--workspace PATH] [--profile REF] [--json]"
}

fn parse_panel_workspace(args: &[String]) -> Result<PathBuf, ParseError> {
    match args {
        [] => std::env::current_dir()
            .map_err(|e| ParseError(format!("failed to resolve current directory: {e}"))),
        [flag, value] if flag == "--workspace" => Ok(PathBuf::from(value)),
        [flag] if flag.starts_with("--workspace=") => {
            let value = flag.trim_start_matches("--workspace=");
            if value.is_empty() {
                Err(ParseError("--workspace requires a value".to_string()))
            } else {
                Ok(PathBuf::from(value))
            }
        }
        [flag] if flag == "--workspace" => {
            Err(ParseError("--workspace requires a value".to_string()))
        }
        _ => Err(ParseError(
            "yoi panel accepts only --workspace <PATH>".to_string(),
        )),
    }
}

fn parse_session_id(value: &str) -> Result<SegmentId, ParseError> {
    value
        .parse()
        .map_err(|_| ParseError(format!("invalid --session UUID: {value}")))
}

fn print_help() {
    println!(
        "yoi\n\nUsage:\n  yoi [OPTIONS]\n  yoi resume [--workspace <PATH>] [--all]\n  yoi panel [--workspace <PATH>]\n  yoi keys\n  yoi setup-model\n  yoi worker [WORKER_OPTIONS]\n  yoi worker delete <NAME> [--force] [--dry-run]\n  yoi worker prune --older-than <DURATION> [--force] [--dry-run]\n  yoi objective <COMMAND> [OPTIONS]\n  yoi session analyze <SESSION_JSONL_PATH> --json\n  yoi session prune --unreferenced [--older-than <DURATION>] [--force] [--dry-run]\n  yoi ticket <COMMAND> [OPTIONS]\n  yoi workspace init [OPTIONS]
  yoi workspace serve [OPTIONS]\n  yoi plugin new <rust-component-tool|rust-component-service> <PATH> [--json]\n  yoi plugin check <PATH_OR_PACKAGE> [--json]\n  yoi plugin pack <PATH> [--output <FILE>] [--json]\n  yoi plugin list [--workspace <PATH>] [--profile <REF>] [--json]\n  yoi plugin show <REF> [--workspace <PATH>] [--profile <REF>] [--json]\n  yoi mcp list [--workspace <PATH>] [--profile <REF>] [--json]\n  yoi mcp show <SERVER> [--workspace <PATH>] [--profile <REF>] [--json]\n  yoi mcp tools|resources|prompts [SERVER] [--workspace <PATH>] [--profile <REF>] [--json]\n  yoi memory lint [OPTIONS]\n\nSurfaces:\n  Console   Single-Worker chat/client surface (default, --worker, yoi resume, Backend Runtime target)\n  Dashboard Workspace cockpit/action surface (yoi panel)\n  TUI       Terminal UI implementation umbrella for Console and Dashboard\n\nOptions:\n      --workspace <PATH> Runtime workspace root for default Console/--worker (defaults to cwd)\n      --worker <NAME>       Open the Worker Console by name (attach/restore/create)\n      --socket <PATH>    Attach a Worker Console to a specific socket with --worker\n      --session <UUID>   Resume a specific session segment in the Worker Console\n      --profile <REF>    Select a reusable Profile recipe\n  -h, --help             Print help\n"
    );
}

fn print_workspace_help() {
    println!(
        "yoi workspace\n\nUsage:\n  yoi workspace init [OPTIONS]\n  yoi workspace serve [OPTIONS]\n\nDescription:\n  Launches the separate yoi-workspace-server executable. The yoi binary does not link the workspace server crate.\n\nSubcommands:\n  init   Initialize .yoi/workspace.toml and the default Backend config template\n  serve  Serve an already initialized Workspace\n\nOptions forwarded to init/serve:\n      --workspace <PATH>  Workspace root (defaults to cwd)\n\nLegacy dev options forwarded to serve:\n      --db <PATH>         SQLite database path override\n      --frontend <PATH>   Static SPA build directory to serve\n      --listen <ADDR>     Listen address override\n  -h, --help              Print help\n\nEnvironment:\n  YOI_WORKSPACE_SERVER_COMMAND  Path to yoi-workspace-server executable override\n"
    );
}

fn print_resume_help() {
    println!(
        "yoi resume\n\nUsage:\n  yoi resume [--workspace <PATH>] [--all]\n\nOptions:\n      --workspace <PATH> Open the Worker Console picker scoped to this workspace (defaults to cwd)\n      --all              Open the Worker Console picker across this host/data dir\n  -h, --help             Print help\n"
    );
}

fn print_memory_lint_help() {
    println!(
        "yoi memory lint\n\nUsage:\n  yoi memory lint [OPTIONS]\n\nOptions:\n      --workspace <PATH>       Workspace root to lint (defaults to cwd)\n      --json                   Emit a JSON report\n      --warnings-as-errors     Return failure when warnings are present\n  -h, --help                   Print help\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worker_name_mode() {
        match parse_args_from(["--worker", "agent", "--socket", "/tmp/agent.sock"]).unwrap() {
            Mode::Tui {
                mode:
                    LaunchMode::WorkerName {
                        worker_name,
                        socket_override,
                    },
                ..
            } => {
                assert_eq!(worker_name, "agent");
                assert_eq!(socket_override, Some(PathBuf::from("/tmp/agent.sock")));
            }
            _ => panic!("expected WorkerName mode"),
        }
    }

    #[test]
    fn parse_backend_runtime_target_mode() {
        match parse_args_from([
            "--backend",
            "http://127.0.0.1:8787",
            "--runtime-id",
            "runtime-a",
            "--worker-id",
            "worker-b",
        ])
        .unwrap()
        {
            Mode::Tui {
                mode: LaunchMode::BackendRuntime { target },
                ..
            } => {
                assert_eq!(target.base_url, "http://127.0.0.1:8787");
                assert_eq!(target.runtime_id, "runtime-a");
                assert_eq!(target.worker_id, "worker-b");
            }
            _ => panic!("expected BackendRuntime mode"),
        }
    }

    #[test]
    fn parse_backend_runtime_target_requires_complete_identity() {
        let err = parse_args_from(["--backend", "http://127.0.0.1:8787", "--worker-id", "w"])
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--backend, --runtime-id, and --worker-id are required together"
        );
    }

    #[test]
    fn parse_backend_runtime_target_rejects_legacy_socket_mix() {
        let err = parse_args_from([
            "--backend",
            "http://127.0.0.1:8787",
            "--runtime-id",
            "r",
            "--worker-id",
            "w",
            "--worker",
            "agent",
        ])
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Backend Runtime API target cannot be combined with --worker, --socket, --session, or --profile"
        );
    }

    #[test]
    fn parse_bare_word_is_unknown_command() {
        let err = parse_args_from(["agent"]).unwrap_err();
        assert_eq!(err.to_string(), "unknown command `agent`");
    }

    #[test]
    fn parse_memory_without_lint_is_usage_error() {
        let err = parse_args_from(["memory"]).unwrap_err();
        assert_eq!(err.to_string(), "yoi memory requires the `lint` subcommand");
    }

    #[test]
    fn parse_resume_subcommand_defaults_to_workspace_scope() {
        match parse_args_from(["resume"]).unwrap() {
            Mode::Tui {
                mode: LaunchMode::Resume { all },
                ..
            } => assert!(!all),
            _ => panic!("expected Resume mode"),
        }
    }

    #[test]
    fn parse_resume_workspace_scope() {
        match parse_args_from(["resume", "--workspace", "/tmp/resume-workspace"]).unwrap() {
            Mode::Tui {
                mode: LaunchMode::Resume { all },
                workspace_root,
            } => {
                assert!(!all);
                assert_eq!(workspace_root, PathBuf::from("/tmp/resume-workspace"));
            }
            _ => panic!("expected Resume mode"),
        }
    }

    #[test]
    fn parse_resume_all_scope() {
        match parse_args_from(["resume", "--all"]).unwrap() {
            Mode::Tui {
                mode: LaunchMode::Resume { all },
                ..
            } => assert!(all),
            _ => panic!("expected Resume mode"),
        }
    }

    #[test]
    fn parse_worker_subcommand_uses_runtime_mode() {
        match parse_args_from(["worker", "--worker", "agent", "--profile", "default"]).unwrap() {
            Mode::WorkerRuntime(args) => {
                assert_eq!(args, ["--worker", "agent", "--profile", "default"])
            }
            _ => panic!("expected WorkerRuntime mode"),
        }
    }

    #[test]
    fn parse_worker_delete_uses_cleanup_mode() {
        match parse_args_from(["worker", "delete", "agent", "--dry-run"]).unwrap() {
            Mode::WorkerCleanup(worker_cleanup_cli::WorkerCleanupCli::Delete(options)) => {
                assert_eq!(options.name, "agent");
                assert!(options.dry_run);
                assert!(!options.force);
            }
            _ => panic!("expected Worker cleanup delete mode"),
        }
    }

    #[test]
    fn parse_worker_prune_uses_cleanup_mode() {
        match parse_args_from(["worker", "prune", "--older-than", "30d"]).unwrap() {
            Mode::WorkerCleanup(worker_cleanup_cli::WorkerCleanupCli::Prune(options)) => {
                assert_eq!(
                    options.older_than,
                    std::time::Duration::from_secs(30 * 24 * 60 * 60)
                );
            }
            _ => panic!("expected Worker cleanup prune mode"),
        }
    }

    #[test]
    fn parse_ticket_subcommand_uses_ticket_mode() {
        match parse_args_from(["ticket", "doctor"]).unwrap() {
            Mode::Ticket(ticket_cli::TicketCli::Command(ticket_cli::TicketCommand::Doctor)) => {}
            _ => panic!("expected Ticket doctor mode"),
        }
    }

    #[test]
    fn parse_session_analyze_uses_session_mode() {
        match parse_args_from(["session", "analyze", "/tmp/session.jsonl", "--json"]).unwrap() {
            Mode::Session(session_cli::SessionCli::Analyze(options)) => {
                assert_eq!(options.path, PathBuf::from("/tmp/session.jsonl"));
                assert!(options.json);
            }
            _ => panic!("expected Session analyze mode"),
        }
    }

    #[test]
    fn parse_ticket_help_uses_ticket_mode() {
        match parse_args_from(["ticket", "--help"]).unwrap() {
            Mode::Ticket(ticket_cli::TicketCli::Help) => {}
            _ => panic!("expected Ticket help mode"),
        }
    }

    #[test]
    fn parse_workspace_serve_passthrough() {
        match parse_args_from(["workspace", "serve", "--listen", "127.0.0.1:0"]).unwrap() {
            Mode::WorkspaceServer { subcommand, args } => {
                assert_eq!(subcommand, "serve");
                assert_eq!(args, vec!["--listen", "127.0.0.1:0"]);
            }
            other => panic!("unexpected mode: {other:?}"),
        }
    }

    #[test]
    fn parse_workspace_init_passthrough() {
        match parse_args_from(["workspace", "init", "--workspace", "/tmp/ws"]).unwrap() {
            Mode::WorkspaceServer { subcommand, args } => {
                assert_eq!(subcommand, "init");
                assert_eq!(args, vec!["--workspace", "/tmp/ws"]);
            }
            other => panic!("unexpected mode: {other:?}"),
        }
    }

    #[test]
    fn parse_workspace_help() {
        assert!(matches!(
            parse_args_from(["workspace", "--help"]).unwrap(),
            Mode::WorkspaceHelp
        ));
    }

    #[test]
    fn parse_keys_subcommand() {
        match parse_args_from(["keys"]).unwrap() {
            Mode::Keys => {}
            _ => panic!("expected Keys mode"),
        }
    }

    #[test]
    fn parse_setup_model_subcommand() {
        match parse_args_from(["setup-model"]).unwrap() {
            Mode::SetupModel => {}
            _ => panic!("expected SetupModel mode"),
        }
    }

    #[test]
    fn parse_setup_model_rejects_arguments() {
        let err = parse_args_from(["setup-model", "extra"]).unwrap_err();
        assert_eq!(err.to_string(), "yoi setup-model does not accept arguments");
    }

    #[test]
    fn parse_literal_worker_name_still_available_with_flag() {
        match parse_args_from(["--worker", "worker"]).unwrap() {
            Mode::Tui {
                mode:
                    LaunchMode::WorkerName {
                        worker_name,
                        socket_override,
                    },
                ..
            } => {
                assert_eq!(worker_name, "worker");
                assert_eq!(socket_override, None);
            }
            _ => panic!("expected WorkerName mode"),
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
    fn parse_plugin_list_and_show() {
        match parse_args_from(["plugin", "list", "--workspace=/tmp/ws", "--json"]).unwrap() {
            Mode::Plugin(plugin_cli::PluginCliCommand::List(options)) => {
                assert_eq!(options.workspace, Some(PathBuf::from("/tmp/ws")));
                assert!(options.json);
            }
            _ => panic!("expected Plugin list mode"),
        }

        match parse_args_from([
            "plugin",
            "show",
            "project:echo",
            "--profile",
            "project:inspect",
        ])
        .unwrap()
        {
            Mode::Plugin(plugin_cli::PluginCliCommand::Show { reference, args }) => {
                assert_eq!(reference, "project:echo");
                assert_eq!(args.profile.as_deref(), Some("project:inspect"));
            }
            _ => panic!("expected Plugin show mode"),
        }
    }

    #[test]
    fn parse_mcp_commands() {
        match parse_args_from(["mcp", "list", "--workspace=/tmp/ws", "--json"]).unwrap() {
            Mode::Mcp(mcp_cli::McpCliCommand::List(options)) => {
                assert_eq!(options.workspace, Some(PathBuf::from("/tmp/ws")));
                assert!(options.json);
            }
            _ => panic!("expected MCP list mode"),
        }

        match parse_args_from(["mcp", "show", "filesystem", "--profile", "project:mcp"]).unwrap() {
            Mode::Mcp(mcp_cli::McpCliCommand::Show { server, args }) => {
                assert_eq!(server, "filesystem");
                assert_eq!(args.profile.as_deref(), Some("project:mcp"));
            }
            _ => panic!("expected MCP show mode"),
        }

        match parse_args_from(["mcp", "tools", "filesystem"]).unwrap() {
            Mode::Mcp(mcp_cli::McpCliCommand::Tools { server, .. }) => {
                assert_eq!(server.as_deref(), Some("filesystem"));
            }
            _ => panic!("expected MCP tools mode"),
        }

        match parse_args_from(["mcp", "resources"]).unwrap() {
            Mode::Mcp(mcp_cli::McpCliCommand::Resources { server, .. }) => {
                assert!(server.is_none());
            }
            _ => panic!("expected MCP resources mode"),
        }

        match parse_args_from(["mcp", "prompts", "filesystem"]).unwrap() {
            Mode::Mcp(mcp_cli::McpCliCommand::Prompts { server, .. }) => {
                assert_eq!(server.as_deref(), Some("filesystem"));
            }
            _ => panic!("expected MCP prompts mode"),
        }
    }

    #[test]
    fn parse_mcp_rejects_usage_errors() {
        let err = parse_args_from(["mcp", "show"]).unwrap_err();
        assert_eq!(err.to_string(), "yoi mcp show requires a server name");
        let err = parse_args_from(["mcp", "list", "extra"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "yoi mcp list does not accept positional arguments"
        );
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
    fn memory_lint_with_other_second_word_is_usage_error() {
        let err = parse_args_from(["memory", "other"]).unwrap_err();
        assert_eq!(err.to_string(), "yoi memory requires the `lint` subcommand");
    }

    #[test]
    fn parse_session_accepts_explicit_runtime_pod_identity() {
        let segment_id = session_store::new_segment_id();
        match parse_args_from([
            "--session",
            &segment_id.to_string(),
            "--worker",
            "explicit-name",
        ])
        .unwrap()
        {
            Mode::Tui {
                mode:
                    LaunchMode::ResumeWithSession {
                        id,
                        worker_name: Some(worker_name),
                    },
                ..
            } => {
                assert_eq!(id, segment_id);
                assert_eq!(worker_name, "explicit-name");
            }
            _ => panic!("expected ResumeWithSession mode with explicit worker name"),
        }
    }

    #[test]
    fn parse_rejects_legacy_resume_flags() {
        let cases = [
            (vec!["-r".to_string()], "unknown argument: -r"),
            (vec!["--resume".to_string()], "unknown argument: --resume"),
            (
                vec![
                    "--worker".to_string(),
                    "agent".to_string(),
                    "-r".to_string(),
                ],
                "unknown argument: -r",
            ),
        ];

        for (args, message) in cases {
            let err = parse_args_from(args).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }

    #[test]
    fn parse_resume_rejects_workspace_with_all() {
        let err = parse_args_from(["resume", "--workspace", "/tmp/ws", "--all"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "yoi resume --all and --workspace are mutually exclusive"
        );
    }

    #[test]
    fn parse_profile_spawn_mode() {
        match parse_args_from([
            "--workspace",
            "/tmp/other-workspace",
            "--profile",
            "project:companion",
            "--worker",
            "agent",
        ])
        .unwrap()
        {
            Mode::Tui {
                mode:
                    LaunchMode::Spawn {
                        worker_name,
                        profile,
                    },
                workspace_root,
            } => {
                assert_eq!(worker_name, Some("agent".to_string()));
                assert_eq!(profile, Some("project:companion".to_string()));
                assert_eq!(workspace_root, PathBuf::from("/tmp/other-workspace"));
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
                    "/tmp/yoi/sock".to_string(),
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
    fn parse_panel_mode() {
        match parse_args_from(["panel", "--workspace", "/tmp/other-workspace"]).unwrap() {
            Mode::Tui {
                mode: LaunchMode::Panel,
                workspace_root,
            } => assert_eq!(workspace_root, PathBuf::from("/tmp/other-workspace")),
            _ => panic!("expected Panel mode"),
        }
    }

    #[test]
    fn parse_dashboard_word_is_not_an_alias_or_worker_name() {
        let err = parse_args_from(["dashboard"]).unwrap_err();
        assert_eq!(err.to_string(), "unknown command `dashboard`");
    }

    #[test]
    fn parse_multi_flag_is_not_a_launch_alias() {
        let err = parse_args_from(["--multi"]).unwrap_err();
        assert_eq!(err.to_string(), "unknown argument: --multi");
    }

    #[test]
    fn parse_top_level_help() {
        match parse_args_from(["--help"]).unwrap() {
            Mode::Help => {}
            _ => panic!("expected Help mode"),
        }
    }

    #[test]
    fn parse_resume_help() {
        match parse_args_from(["resume", "--help"]).unwrap() {
            Mode::ResumeHelp => {}
            _ => panic!("expected ResumeHelp mode"),
        }
    }

    #[test]
    fn parse_memory_lint_help() {
        match parse_args_from(["memory", "lint", "--help"]).unwrap() {
            Mode::MemoryLintHelp => {}
            _ => panic!("expected MemoryLintHelp mode"),
        }
    }
}
