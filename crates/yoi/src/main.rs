mod cli_connection;
mod mcp_cli;
mod memory_lint;
mod objective_cli;
mod plugin_cli;
mod session_cli;
mod ticket_cli;
mod worker_cleanup_cli;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use cli_connection::{
    CliCommand, CliConnectionResolver, ClientConfigCliConnectionResolver, ClientDefaultConnection,
    backend_target_option_error_for_local_command, is_backend_target_option,
    resolve_backend_cli_connection, resolve_connection_aware_cli_connection,
    resolve_local_cli_connection,
};
use client::{BackendAuthTarget, Target, TargetKind, start_device_login, wait_for_device_login};
use memory_lint::{LintCliOptions, LintStatus};
use serde::{Deserialize, Serialize};
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
    Login {
        backend_url: String,
        no_wait: bool,
    },
    WorkerRuntime(Vec<String>),
    Keys,
    SetupModel,
    Tui {
        target: Box<dyn Target>,
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
        Mode::Login {
            backend_url,
            no_wait,
        } => match run_login(&backend_url, no_wait).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("yoi login: {e}");
                ExitCode::FAILURE
            }
        },
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
            target,
            mode,
            workspace_root,
        } => {
            tui::launch(LaunchOptions {
                target,
                mode,
                workspace_root,
            })
            .await
        }
    }
}

#[derive(Debug, Default, Clone)]
struct TargetSelection {
    explicit_local: bool,
    backend_url: Option<String>,
    workspace_id: Option<String>,
}

impl TargetSelection {
    fn explicit_backend(&self) -> bool {
        self.backend_url.is_some()
    }
}

fn resolve_tui_target<R: CliConnectionResolver + ?Sized>(
    connection_resolver: &R,
    command: CliCommand,
    selection: &TargetSelection,
    workspace_root: &Path,
) -> Result<Box<dyn Target>, ParseError> {
    let workspace_id = if selection.explicit_backend() && selection.workspace_id.is_none() {
        resolve_workspace_id_from_root(workspace_root)?
    } else {
        selection.workspace_id.clone()
    };
    resolve_connection_aware_cli_connection(
        connection_resolver,
        command,
        selection.explicit_local,
        selection.backend_url.clone(),
        workspace_id.as_deref(),
    )
}

fn parse_top_level_target_selection(
    args: &[String],
) -> Result<(TargetSelection, &[String]), ParseError> {
    let mut selection = TargetSelection::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--local" => {
                if selection.backend_url.is_some() {
                    return Err(ParseError(
                        "--local and --backend are mutually exclusive".to_string(),
                    ));
                }
                selection.explicit_local = true;
                i += 1;
            }
            "--backend" => {
                if selection.explicit_local {
                    return Err(ParseError(
                        "--local and --backend are mutually exclusive".to_string(),
                    ));
                }
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--backend requires a URL".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--backend requires a URL".to_string()));
                }
                selection.backend_url = Some(value.clone());
                i += 2;
            }
            "--workspace-id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace-id requires a value".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--workspace-id requires a value".to_string()));
                }
                selection.workspace_id = Some(value.clone());
                i += 2;
            }
            arg if arg.starts_with("--backend=") => {
                if selection.explicit_local {
                    return Err(ParseError(
                        "--local and --backend are mutually exclusive".to_string(),
                    ));
                }
                let value = arg.trim_start_matches("--backend=");
                if value.is_empty() {
                    return Err(ParseError("--backend requires a URL".to_string()));
                }
                selection.backend_url = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--workspace-id=") => {
                let value = arg.trim_start_matches("--workspace-id=");
                if value.is_empty() {
                    return Err(ParseError("--workspace-id requires a value".to_string()));
                }
                selection.workspace_id = Some(value.to_string());
                i += 1;
            }
            _ => break,
        }
    }
    Ok((selection, &args[i..]))
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
    let resolver = ClientConfigCliConnectionResolver;
    parse_args_slice_with_connection_resolver(args, &resolver)
}

fn parse_args_slice_with_connection_resolver<R: CliConnectionResolver + ?Sized>(
    args: &[String],
    connection_resolver: &R,
) -> Result<Mode, ParseError> {
    let (target_selection, args) = parse_top_level_target_selection(args)?;
    if args.is_empty() {
        let workspace_root = current_dir()?;
        let target = resolve_tui_target(
            connection_resolver,
            CliCommand::DefaultTui,
            &target_selection,
            &workspace_root,
        )?;
        let mode = if target.kind() == client::TargetKind::Backend {
            LaunchMode::Workers {
                runtime_id: None,
                include_stopped: false,
                all: false,
            }
        } else {
            LaunchMode::Spawn {
                worker_name: None,
                profile: None,
            }
        };
        return Ok(Mode::Tui {
            target,
            mode,
            workspace_root,
        });
    }

    match args[0].as_str() {
        "--help" | "-h" => return Ok(Mode::Help),
        "resume" => return parse_resume_args(&args[1..], &target_selection, connection_resolver),
        "workers" => return parse_workers_args(&args[1..], &target_selection, connection_resolver),
        "worker" => {
            if let Some(cli) = worker_cleanup_cli::parse_worker_management_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?
            {
                let _target =
                    resolve_local_cli_connection(connection_resolver, CliCommand::WorkerCleanup)?;
                return Ok(Mode::WorkerCleanup(cli));
            }
            let _target =
                resolve_local_cli_connection(connection_resolver, CliCommand::WorkerRuntime)?;
            return Ok(Mode::WorkerRuntime(args[1..].to_vec()));
        }
        "objective" => {
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Objective)?;
            let objective_cli = objective_cli::parse_objective_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Objective(objective_cli));
        }
        "session" => {
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Session)?;
            let session_cli = session_cli::parse_session_args(&args[1..])
                .map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Session(session_cli));
        }
        "ticket" => {
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Ticket)?;
            let ticket_cli =
                ticket_cli::parse_ticket_args(&args[1..]).map_err(|e| ParseError(e.to_string()))?;
            return Ok(Mode::Ticket(ticket_cli));
        }
        "plugin" => {
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Plugin)?;
            let plugin_cli = parse_plugin_args(&args[1..])?;
            return Ok(Mode::Plugin(plugin_cli));
        }
        "login" => {
            return parse_login_args(&args[1..], connection_resolver);
        }
        "mcp" => {
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Mcp)?;
            let mcp_cli = parse_mcp_args(&args[1..])?;
            return Ok(Mode::Mcp(mcp_cli));
        }
        "panel" => {
            let panel_options = parse_panel_args(&args[1..])?;
            let target = resolve_tui_target(
                connection_resolver,
                CliCommand::Panel,
                &target_selection,
                &panel_options.workspace_root,
            )?;
            if panel_options.include_stopped && target.kind() == TargetKind::Backend {
                return Err(ParseError(
                    "yoi panel -r is only supported for local targets; Backend panel restore UI is not implemented"
                        .to_string(),
                ));
            }
            return Ok(Mode::Tui {
                target,
                mode: LaunchMode::Panel {
                    include_stopped: panel_options.include_stopped,
                },
                workspace_root: panel_options.workspace_root,
            });
        }
        "keys" => {
            if args.len() != 1 {
                if let Some(arg) = args[1..].iter().find(|arg| is_backend_target_option(arg)) {
                    return Err(backend_target_option_error_for_local_command(
                        CliCommand::Keys,
                        arg,
                    ));
                }
                return Err(ParseError("yoi keys does not accept arguments".into()));
            }
            let _target = resolve_local_cli_connection(connection_resolver, CliCommand::Keys)?;
            return Ok(Mode::Keys);
        }
        "setup-model" => {
            if args.len() != 1 {
                if let Some(arg) = args[1..].iter().find(|arg| is_backend_target_option(arg)) {
                    return Err(backend_target_option_error_for_local_command(
                        CliCommand::SetupModel,
                        arg,
                    ));
                }
                return Err(ParseError(
                    "yoi setup-model does not accept arguments".into(),
                ));
            }
            let _target =
                resolve_local_cli_connection(connection_resolver, CliCommand::SetupModel)?;
            return Ok(Mode::SetupModel);
        }
        "memory" if args.get(1).map(String::as_str) == Some("lint") => {
            let _target =
                resolve_local_cli_connection(connection_resolver, CliCommand::MemoryLint)?;
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

    parse_console_options(args, &target_selection, connection_resolver)
}

fn parse_console_options<R: CliConnectionResolver + ?Sized>(
    args: &[String],
    target_selection: &TargetSelection,
    connection_resolver: &R,
) -> Result<Mode, ParseError> {
    let mut workspace_root = current_dir()?;
    let mut worker_name = None;
    let mut session = None;
    let mut profile = None;
    let mut socket_override = None;
    let mut runtime_id = None;
    let mut worker_id = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--worker" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--worker requires a name".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--worker requires a name".to_string()));
                }
                worker_name = Some(value.clone());
                i += 2;
            }
            "--workspace" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace requires a path".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--workspace requires a path".to_string()));
                }
                workspace_root = PathBuf::from(value);
                i += 2;
            }
            "--session" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--session requires a path".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--session requires a path".to_string()));
                }
                session = Some(PathBuf::from(value));
                i += 2;
            }
            "--socket" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--socket requires a path".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--socket requires a path".to_string()));
                }
                socket_override = Some(PathBuf::from(value));
                i += 2;
            }
            "--profile" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--profile requires a name".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--profile requires a name".to_string()));
                }
                profile = Some(value.clone());
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
            arg if arg.starts_with("--worker=") => {
                let value = arg.trim_start_matches("--worker=");
                if value.is_empty() {
                    return Err(ParseError("--worker requires a name".to_string()));
                }
                worker_name = Some(value.to_string());
                i += 1;
            }
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a path".to_string()));
                }
                workspace_root = PathBuf::from(value);
                i += 1;
            }
            arg if arg.starts_with("--session=") => {
                let value = arg.trim_start_matches("--session=");
                if value.is_empty() {
                    return Err(ParseError("--session requires a path".to_string()));
                }
                session = Some(PathBuf::from(value));
                i += 1;
            }
            arg if arg.starts_with("--socket=") => {
                let value = arg.trim_start_matches("--socket=");
                if value.is_empty() {
                    return Err(ParseError("--socket requires a path".to_string()));
                }
                socket_override = Some(PathBuf::from(value));
                i += 1;
            }
            arg if arg.starts_with("--profile=") => {
                let value = arg.trim_start_matches("--profile=");
                if value.is_empty() {
                    return Err(ParseError("--profile requires a name".to_string()));
                }
                profile = Some(value.to_string());
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
            arg if arg.starts_with('-') => {
                return Err(ParseError(format!("unknown argument: {arg}")));
            }
            value => {
                return Err(ParseError(format!(
                    "yoi does not accept positional argument `{value}` before a subcommand"
                )));
            }
        }
    }

    if worker_id.is_some() && runtime_id.is_none() {
        return Err(ParseError(
            "--worker-id requires --runtime-id for Runtime API attach".to_string(),
        ));
    }
    if (runtime_id.is_some() || worker_id.is_some())
        && (session.is_some()
            || worker_name.is_some()
            || socket_override.is_some()
            || profile.is_some())
    {
        return Err(ParseError(
            "Runtime API target cannot be combined with --worker, --socket, --session, or --profile".to_string(),
        ));
    }
    if profile.is_some() && (session.is_some() || socket_override.is_some()) {
        return Err(ParseError(
            "--profile can only be used for fresh spawn".to_string(),
        ));
    }
    if session.is_some() && socket_override.is_some() {
        return Err(ParseError(
            "--session cannot be combined with --socket".to_string(),
        ));
    }
    if socket_override.is_some() && worker_name.is_none() {
        return Err(ParseError("--socket requires --worker".to_string()));
    }

    let target = resolve_tui_target(
        connection_resolver,
        CliCommand::DefaultTui,
        target_selection,
        &workspace_root,
    )?;

    if let (Some(runtime_id), Some(worker_id)) = (runtime_id.clone(), worker_id) {
        return Ok(Mode::Tui {
            target,
            mode: LaunchMode::OpenWorker {
                runtime_id,
                worker_id,
            },
            workspace_root,
        });
    }

    if runtime_id.is_some() {
        return Ok(Mode::Tui {
            target,
            mode: LaunchMode::Workers {
                runtime_id,
                include_stopped: false,
                all: false,
            },
            workspace_root,
        });
    }

    let mode = if let Some(profile) = profile {
        LaunchMode::Spawn {
            worker_name,
            profile: Some(profile),
        }
    } else if let Some(session) = session {
        LaunchMode::ResumeWithSession {
            id: parse_session_id(&session.to_string_lossy())?,
            worker_name,
        }
    } else if let Some(worker_name) = worker_name {
        LaunchMode::WorkerName {
            worker_name,
            socket_override,
        }
    } else {
        LaunchMode::Spawn {
            worker_name: None,
            profile: None,
        }
    };

    Ok(Mode::Tui {
        target,
        mode,
        workspace_root,
    })
}

fn parse_workers_args<R: CliConnectionResolver + ?Sized>(
    args: &[String],
    target_selection: &TargetSelection,
    connection_resolver: &R,
) -> Result<Mode, ParseError> {
    let mut workspace_root = current_dir()?;
    let mut runtime_id = None;
    let mut include_stopped = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" | "-h" => {
                return Err(ParseError(
                    "usage: yoi [--local|--backend <URL>] [--workspace-id <ID>] workers [-r|--stopped] [--workspace <PATH>] [--runtime-id <ID>]".to_string(),
                ));
            }
            "-r" | "--restoreable" | "--stopped" => {
                include_stopped = true;
                i += 1;
            }
            "--workspace" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace requires a value".to_string()))?;
                if value.starts_with('-') || value.is_empty() {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
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
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a value".to_string()));
                }
                workspace_root = PathBuf::from(value);
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
            arg if arg.starts_with('-') => {
                return Err(ParseError(format!("unknown yoi workers option `{arg}`")));
            }
            value => {
                return Err(ParseError(format!(
                    "yoi workers does not accept positional argument `{value}`"
                )));
            }
        }
    }
    let target = resolve_tui_target(
        connection_resolver,
        CliCommand::Workers,
        target_selection,
        &workspace_root,
    )?;
    Ok(Mode::Tui {
        target,
        mode: LaunchMode::Workers {
            runtime_id,
            include_stopped,
            all: false,
        },
        workspace_root,
    })
}

fn parse_resume_args<R: CliConnectionResolver + ?Sized>(
    args: &[String],
    target_selection: &TargetSelection,
    connection_resolver: &R,
) -> Result<Mode, ParseError> {
    let mut workspace_root = current_dir()?;
    let mut workspace_set = false;
    let mut all = false;
    let mut runtime_id = None;

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

    let target = resolve_tui_target(
        connection_resolver,
        CliCommand::Resume,
        target_selection,
        &workspace_root,
    )?;

    Ok(Mode::Tui {
        target,
        mode: LaunchMode::Workers {
            runtime_id,
            include_stopped: true,
            all,
        },
        workspace_root,
    })
}

fn current_dir() -> Result<PathBuf, ParseError> {
    std::env::current_dir()
        .map_err(|e| ParseError(format!("failed to resolve current directory: {e}")))
}

#[derive(Debug, Deserialize)]
struct WorkspaceIdentityFile {
    #[serde(alias = "workspace_id")]
    id: String,
}

#[derive(Debug, Default)]
struct ClientConfigFile {
    default_backend: Option<String>,
    default_connection: ClientDefaultConnection,
    backends: BTreeMap<String, ClientBackendConfig>,
    workspaces: BTreeMap<String, ClientWorkspaceConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct ClientConfigOverlay {
    default_backend: Option<String>,
    default_connection: Option<ClientDefaultConnection>,
    #[serde(default)]
    backends: BTreeMap<String, ClientBackendConfigOverlay>,
    #[serde(default)]
    workspaces: BTreeMap<String, ClientWorkspaceConfigOverlay>,
}

#[derive(Debug, Default)]
struct ClientBackendConfig {
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ClientBackendConfigOverlay {
    url: Option<String>,
}

#[derive(Debug, Default)]
struct ClientWorkspaceConfig {
    backend: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ClientWorkspaceConfigOverlay {
    backend: Option<String>,
}

impl ClientConfigFile {
    fn apply_overlay(&mut self, overlay: ClientConfigOverlay) {
        if let Some(default_backend) = overlay.default_backend {
            self.default_backend = Some(default_backend);
        }
        if let Some(default_connection) = overlay.default_connection {
            self.default_connection = default_connection;
        }
        for (name, backend) in overlay.backends {
            let entry = self.backends.entry(name).or_default();
            if let Some(url) = backend.url {
                entry.url = Some(url);
            }
        }
        for (id, workspace) in overlay.workspaces {
            let entry = self.workspaces.entry(id).or_default();
            if let Some(backend) = workspace.backend {
                entry.backend = Some(backend);
            }
        }
    }
}

fn resolve_workspace_id_from_root(workspace_root: &Path) -> Result<Option<String>, ParseError> {
    let mut current = if workspace_root.is_absolute() {
        workspace_root.to_path_buf()
    } else {
        current_dir()?.join(workspace_root)
    };
    loop {
        let path = current.join(".yoi").join("workspace.toml");
        if path.is_file() {
            let contents = fs::read_to_string(&path)
                .map_err(|e| ParseError(format!("failed to read {}: {e}", path.display())))?;
            let identity: WorkspaceIdentityFile = toml::from_str(&contents)
                .map_err(|e| ParseError(format!("failed to parse {}: {e}", path.display())))?;
            let id = identity.id.trim();
            if id.is_empty() {
                return Err(ParseError(format!(
                    "{} must contain a non-empty workspace id",
                    path.display()
                )));
            }
            return Ok(Some(id.to_string()));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

fn resolve_backend_url(
    explicit_backend_url: Option<String>,
    workspace_id: Option<&str>,
) -> Result<String, ParseError> {
    if let Some(url) = explicit_backend_url {
        return Ok(url);
    }
    let Some(config) = read_client_config()? else {
        return Err(ParseError(client_config_missing_message(workspace_id)));
    };
    let backend_name = workspace_id
        .and_then(|id| {
            config
                .workspaces
                .get(id)
                .and_then(|workspace| workspace.backend.as_deref())
        })
        .or(config.default_backend.as_deref())
        .ok_or_else(|| ParseError(client_config_missing_message(workspace_id)))?;
    let backend = config.backends.get(backend_name).ok_or_else(|| {
        ParseError(format!(
            "client config references backend `{backend_name}`, but [backends.{backend_name}] is not defined"
        ))
    })?;
    let Some(url) = backend.url.as_deref().map(str::trim) else {
        return Err(ParseError(format!(
            "client config backend `{backend_name}` must contain a url"
        )));
    };
    if url.is_empty() {
        return Err(ParseError(format!(
            "client config backend `{backend_name}` must contain a non-empty url"
        )));
    }
    Ok(url.to_string())
}

fn read_client_default_connection() -> Result<ClientDefaultConnection, ParseError> {
    Ok(read_client_config()?
        .map(|config| config.default_connection)
        .unwrap_or_default())
}

fn read_client_config() -> Result<Option<ClientConfigFile>, ParseError> {
    let mut config = ClientConfigFile::default();
    let mut found = false;

    if let Some(path) = client_global_config_path() {
        if let Some(overlay) = read_client_config_overlay(&path)? {
            config.apply_overlay(overlay);
            found = true;
        }
    }

    let cwd_path = client_cwd_config_path()?;
    if let Some(overlay) = read_client_config_overlay(&cwd_path)? {
        config.apply_overlay(overlay);
        found = true;
    }

    Ok(found.then_some(config))
}

fn read_client_config_overlay(path: &Path) -> Result<Option<ClientConfigOverlay>, ParseError> {
    if !path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)
        .map_err(|e| ParseError(format!("failed to read {}: {e}", path.display())))?;
    toml::from_str::<ClientConfigOverlay>(&contents)
        .map(Some)
        .map_err(|e| ParseError(format!("failed to parse {}: {e}", path.display())))
}

fn client_global_config_path() -> Option<PathBuf> {
    manifest::paths::data_dir().map(|dir| dir.join("client").join("config.toml"))
}

fn client_cwd_config_path() -> Result<PathBuf, ParseError> {
    Ok(current_dir()?.join(".yoi").join("client.config.toml"))
}

fn client_config_location_message() -> String {
    match client_global_config_path() {
        Some(path) => format!("{} or <cwd>/.yoi/client.config.toml", path.display()),
        None => "<data_dir>/client/config.toml or <cwd>/.yoi/client.config.toml".to_string(),
    }
}

fn client_config_missing_message(workspace_id: Option<&str>) -> String {
    let locations = client_config_location_message();
    match workspace_id {
        Some(workspace_id) => format!(
            "Backend URL is required. Pass --backend <URL> or configure {locations} with [workspaces.{workspace_id}] backend = <name> and [backends.<name>].url"
        ),
        None => format!(
            "Backend URL is required. Pass --backend <URL> or configure default_backend in {locations}"
        ),
    }
}

fn parse_login_args<R: CliConnectionResolver + ?Sized>(
    args: &[String],
    connection_resolver: &R,
) -> Result<Mode, ParseError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Err(ParseError(
            "yoi login usage: yoi login [--backend <URL>] [--no-wait]".to_string(),
        ));
    }
    let mut backend_url = None;
    let mut no_wait = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
            arg if arg.starts_with("--backend=") => {
                let value = arg.trim_start_matches("--backend=");
                if value.is_empty() {
                    return Err(ParseError("--backend requires a URL".to_string()));
                }
                backend_url = Some(value.to_string());
                i += 1;
            }
            "--no-wait" => {
                no_wait = true;
                i += 1;
            }
            other => {
                return Err(ParseError(format!(
                    "unknown yoi login argument '{other}' (try 'yoi login --help')"
                )));
            }
        }
    }
    let _target = resolve_backend_cli_connection(
        connection_resolver,
        CliCommand::Login,
        backend_url.clone(),
        None,
    )?;
    let backend_url = resolve_backend_url(backend_url, None)?;
    Ok(Mode::Login {
        backend_url,
        no_wait,
    })
}

async fn run_login(backend_url: &str, no_wait: bool) -> Result<(), ParseError> {
    let target = BackendAuthTarget::new(backend_url.to_string());
    let start = start_device_login(&target, Some("yoi cli"))
        .await
        .map_err(|error| ParseError(error.to_string()))?;
    println!("Open this URL in your browser to approve Yoi CLI login:");
    println!("  {}", start.verification_uri_complete);
    println!();
    println!("User code: {}", start.user_code);
    println!("Expires in: {} seconds", start.expires_in);
    if no_wait {
        println!("Device code: {}", start.device_code);
        println!(
            "Run without --no-wait after approving, or poll the Backend device endpoint manually."
        );
        return Ok(());
    }
    let token = wait_for_device_login(
        &target,
        &start.device_code,
        Duration::from_secs(start.interval.max(1)),
        Duration::from_secs(start.expires_in.max(1)),
    )
    .await
    .map_err(|error| ParseError(error.to_string()))?;
    save_backend_token(backend_url, &token)?;
    println!("Saved Backend API token for {backend_url}");
    Ok(())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BackendTokenFile {
    #[serde(default)]
    tokens: BTreeMap<String, BackendTokenEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackendTokenEntry {
    token_type: String,
    access_token: String,
}

fn save_backend_token(backend_url: &str, access_token: &str) -> Result<(), ParseError> {
    let path = backend_token_path().ok_or_else(|| {
        ParseError("HOME or XDG_CONFIG_HOME is required to save Backend token".to_string())
    })?;
    let mut file = if path.is_file() {
        let contents = fs::read_to_string(&path)
            .map_err(|error| ParseError(format!("failed to read {}: {error}", path.display())))?;
        serde_json::from_str::<BackendTokenFile>(&contents)
            .map_err(|error| ParseError(format!("failed to parse {}: {error}", path.display())))?
    } else {
        BackendTokenFile::default()
    };
    file.tokens.insert(
        backend_url.trim_end_matches('/').to_string(),
        BackendTokenEntry {
            token_type: "Bearer".to_string(),
            access_token: access_token.to_string(),
        },
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ParseError(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let serialized = serde_json::to_string_pretty(&file)
        .map_err(|error| ParseError(format!("failed to serialize Backend token file: {error}")))?;
    fs::write(&path, format!("{serialized}\n"))
        .map_err(|error| ParseError(format!("failed to write {}: {error}", path.display())))?;
    Ok(())
}

fn backend_token_path() -> Option<PathBuf> {
    yoi_config_dir().map(|dir| dir.join("backend-tokens.json"))
}

fn yoi_config_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(home).join("yoi"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("yoi"))
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelCliOptions {
    workspace_root: PathBuf,
    include_stopped: bool,
}

fn parse_panel_args(args: &[String]) -> Result<PanelCliOptions, ParseError> {
    let mut workspace_root: Option<PathBuf> = None;
    let mut include_stopped = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| ParseError("--workspace requires a path".to_string()))?;
                workspace_root = Some(PathBuf::from(value));
                i += 2;
            }
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(ParseError("--workspace requires a path".to_string()));
                }
                workspace_root = Some(PathBuf::from(value));
                i += 1;
            }
            "-r" | "--stopped" | "--restoreable" => {
                include_stopped = true;
                i += 1;
            }
            other => {
                return Err(ParseError(format!(
                    "unknown panel option `{other}`; usage: yoi [TARGET] panel [-r|--stopped] [--workspace <PATH>]"
                )));
            }
        }
    }
    Ok(PanelCliOptions {
        workspace_root: workspace_root.unwrap_or(current_dir()?),
        include_stopped,
    })
}

fn parse_session_id(value: &str) -> Result<SegmentId, ParseError> {
    value
        .parse()
        .map_err(|_| ParseError(format!("invalid --session UUID: {value}")))
}

const TOP_LEVEL_HELP: &str = r#"yoi

Usage:
  yoi [TARGET] [CONSOLE_OPTIONS]
  yoi [TARGET] workers [-r|--stopped] [--workspace <PATH>] [--runtime-id <ID>]
  yoi [TARGET] resume [--workspace <PATH>|--all] [--runtime-id <ID>]
  yoi [TARGET] panel [-r|--stopped] [--workspace <PATH>]
  yoi [--backend <URL>] login [--no-wait]
  yoi <LOCAL_COMMAND> [OPTIONS]

Target selection:
  Target options are top-level options and must appear before the command.

      --local              Use the local Worker runtime explicitly
      --backend <URL>      Use a Workspace Backend explicitly
      --workspace-id <ID>  Scope Backend routes to a Workspace id

  If no target is explicit, connection-aware commands use the merged client config:
      <data_dir>/client/config.toml
      <cwd>/.yoi/client.config.toml

  Supported client config keys:
      default_connection = "local" | "backend"
      default_backend = "<name>"
      [backends.<name>] url = "https://backend.example"
      [workspaces.<workspace_id>] backend = "<name>"

Connection-aware commands:
  yoi                         Local: open a new Console. Backend: open Backend Workers.
  yoi workers                 List/select Workers for the selected target.
  yoi workers -r              Include stopped Workers. --restoreable is accepted as a legacy alias.
  yoi resume                  Open the Worker picker with stopped Workers included.
  yoi panel                   Open the dashboard/panel TUI for the selected target.
  yoi panel -r                Local only: include stopped/restorable Worker rows.

Console options:
      --workspace <PATH>   Local workspace root for local Console/Worker lists (defaults to cwd)
      --worker <NAME>      Open/create a named local Worker Console
      --socket <PATH>      Attach to a local Worker socket; requires --worker
      --session <UUID>     Resume a local session segment
      --profile <REF>      Select a reusable Profile recipe for a fresh local Worker
      --runtime-id <ID>    Backend Runtime id for Backend Worker list/attach
      --worker-id <ID>     Backend Worker id to attach; requires --runtime-id

Local commands:
  keys                         Manage local model/API keys
  setup-model                  Configure a local model provider
  worker [WORKER_OPTIONS]      Run the local Worker runtime CLI
  worker delete <NAME>         Delete local Worker records
  worker prune                 Prune old local Worker records
  ticket <COMMAND>             Manage Tickets through the local workspace authority
  objective <COMMAND>          Manage Objectives through the local workspace authority
  plugin <COMMAND>             Build/check/list/show plugins
  mcp <COMMAND>                Inspect configured MCP servers
  memory lint                  Lint local memory files
  session <COMMAND>            Inspect/prune local session logs

Backend-only commands:
  login                        Run Backend device login and save the API token

Standalone binaries:
  yoi-workspace-server         Workspace Backend server/admin CLI
  worker-runtime-rest-server   Worker Runtime REST server

Options:
  -h, --help                   Print help
"#;

fn print_help() {
    println!("{TOP_LEVEL_HELP}");
}

fn print_resume_help() {
    println!(
        "yoi resume\n\nUsage:\n  yoi [TARGET] resume [--workspace <PATH>|--all] [--runtime-id <ID>]\n\nTarget options:\n      --local              Use local Worker records explicitly\n      --backend <URL>      Use Backend Worker records explicitly\n      --workspace-id <ID>  Scope Backend routes to a Workspace id\n\nOptions:\n      --workspace <PATH>   Open the Worker picker scoped to this local workspace (defaults to cwd)\n      --all                Open the Worker picker across this host/data dir\n      --runtime-id <ID>    Restrict Backend picker to a Runtime id\n  -h, --help               Print help\n"
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
    use crate::cli_connection::CliConnectionInput;
    use client::{BackendTarget, LocalTarget, TargetKind, WorkerListRequest};

    struct FixedCliConnectionResolver {
        backend_url: &'static str,
    }

    impl CliConnectionResolver for FixedCliConnectionResolver {
        fn resolve_connection(
            &self,
            _command: CliCommand,
            input: CliConnectionInput<'_>,
        ) -> Result<Box<dyn Target>, ParseError> {
            match input {
                CliConnectionInput::DefaultTarget { .. } | CliConnectionInput::LocalTarget => {
                    Ok(Box::new(LocalTarget::new()))
                }
                CliConnectionInput::BackendTarget { workspace_id, .. } => Ok(Box::new(
                    BackendTarget::new(self.backend_url, workspace_id.map(str::to_string)),
                )),
            }
        }
    }

    #[test]
    fn parser_uses_local_target_for_workers_without_backend_option() {
        let resolver = FixedCliConnectionResolver {
            backend_url: "http://fake-backend.example",
        };
        let args = vec!["workers".to_string()];
        let mode = parse_args_slice_with_connection_resolver(&args, &resolver).unwrap();

        match mode {
            Mode::Tui {
                target,
                mode: LaunchMode::Workers { runtime_id, .. },
                ..
            } => {
                assert_eq!(runtime_id, None);
                assert_eq!(target.kind(), TargetKind::Local);
                let workers = target.list_workers(WorkerListRequest::new(None)).unwrap();
                assert!(workers.local_runtime_command.is_some());
                assert!(workers.backend_target.is_none());
            }
            other => panic!("expected Workers mode, got {other:?}"),
        }
    }

    #[test]
    fn client_config_default_connection_defaults_to_local() {
        let config = ClientConfigFile::default();
        assert_eq!(config.default_connection, ClientDefaultConnection::Local);
    }

    #[test]
    fn client_config_overlay_merges_property_wise() {
        let mut config = ClientConfigFile::default();
        config.apply_overlay(
            toml::from_str(
                r#"
default_backend = "global"
default_connection = "backend"

[backends.global]
url = "http://global.example"

[backends.shared]
url = "http://shared-global.example"

[workspaces.workspace-a]
backend = "global"
"#,
            )
            .unwrap(),
        );
        config.apply_overlay(
            toml::from_str(
                r#"
default_backend = "shared"

[backends.shared]
url = "http://shared-cwd.example"

[workspaces.workspace-b]
backend = "shared"
"#,
            )
            .unwrap(),
        );

        assert_eq!(config.default_connection, ClientDefaultConnection::Backend);
        assert_eq!(config.default_backend.as_deref(), Some("shared"));
        assert_eq!(
            config
                .backends
                .get("global")
                .and_then(|backend| backend.url.as_deref()),
            Some("http://global.example")
        );
        assert_eq!(
            config
                .backends
                .get("shared")
                .and_then(|backend| backend.url.as_deref()),
            Some("http://shared-cwd.example")
        );
        assert_eq!(
            config
                .workspaces
                .get("workspace-a")
                .and_then(|workspace| workspace.backend.as_deref()),
            Some("global")
        );
        assert_eq!(
            config
                .workspaces
                .get("workspace-b")
                .and_then(|workspace| workspace.backend.as_deref()),
            Some("shared")
        );
    }

    #[test]
    fn top_level_help_matches_current_target_surface() {
        assert!(TOP_LEVEL_HELP.contains("Target options are top-level options"));
        assert!(TOP_LEVEL_HELP.contains("--local"));
        assert!(TOP_LEVEL_HELP.contains("--backend <URL>"));
        assert!(TOP_LEVEL_HELP.contains("<data_dir>/client/config.toml"));
        assert!(TOP_LEVEL_HELP.contains("<cwd>/.yoi/client.config.toml"));
        assert!(TOP_LEVEL_HELP.contains("yoi-workspace-server"));
        assert!(TOP_LEVEL_HELP.contains("worker-runtime-rest-server"));
        assert!(!TOP_LEVEL_HELP.contains("yoi workspace"));
        assert!(!TOP_LEVEL_HELP.contains("yoi server"));
        assert!(!TOP_LEVEL_HELP.contains("TARGET_OPTIONS"));
    }

    #[test]
    fn parse_local_only_commands_reject_backend_target_options() {
        let err = parse_args_from(["keys", "--workspace-id=workspace-a"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "yoi keys uses a local connection target and cannot accept Backend target option `--workspace-id=workspace-a`"
        );
    }

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
                target,
                mode:
                    LaunchMode::OpenWorker {
                        runtime_id,
                        worker_id,
                    },
                ..
            } => {
                assert_eq!(target.kind(), TargetKind::Backend);
                let connection = target
                    .connect_worker(client::WorkerConnectionSelector::new(
                        runtime_id.clone(),
                        worker_id.clone(),
                    ))
                    .unwrap();
                assert_eq!(connection.target.base_url, "http://127.0.0.1:8787");
                assert_eq!(runtime_id, "runtime-a");
                assert_eq!(worker_id, "worker-b");
            }
            _ => panic!("expected OpenWorker mode"),
        }
    }

    #[test]
    fn parse_backend_runtime_target_requires_runtime_for_worker_identity() {
        let err = parse_args_from(["--backend", "http://127.0.0.1:8787", "--worker-id", "w"])
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--worker-id requires --runtime-id for Runtime API attach"
        );
    }

    #[test]
    fn parse_backend_runtime_picker_target_mode() {
        match parse_args_from(["--backend", "http://127.0.0.1:8787", "--runtime-id", "r"]).unwrap()
        {
            Mode::Tui {
                target,
                mode: LaunchMode::Workers { runtime_id, .. },
                ..
            } => {
                assert_eq!(target.kind(), TargetKind::Backend);
                let workers = target
                    .list_workers(WorkerListRequest::new(runtime_id.clone()))
                    .unwrap();
                assert_eq!(
                    workers.backend_target.as_ref().unwrap().base_url,
                    "http://127.0.0.1:8787"
                );
                assert_eq!(
                    workers
                        .backend_target
                        .as_ref()
                        .unwrap()
                        .runtime_id
                        .as_deref(),
                    Some("r")
                );
                assert_eq!(runtime_id.as_deref(), Some("r"));
            }
            _ => panic!("expected Workers mode"),
        }
    }

    #[test]
    fn parse_workers_subcommand_uses_backend_runtime_picker() {
        match parse_args_from([
            "--backend",
            "http://127.0.0.1:8787",
            "--workspace-id",
            "workspace-a",
            "workers",
        ])
        .unwrap()
        {
            Mode::Tui {
                target,
                mode: LaunchMode::Workers { runtime_id, .. },
                ..
            } => {
                assert_eq!(target.kind(), TargetKind::Backend);
                let workers = target
                    .list_workers(WorkerListRequest::new(runtime_id.clone()))
                    .unwrap();
                assert_eq!(
                    workers.backend_target.as_ref().unwrap().base_url,
                    "http://127.0.0.1:8787"
                );
                assert_eq!(
                    workers
                        .backend_target
                        .as_ref()
                        .unwrap()
                        .workspace_id
                        .as_deref(),
                    Some("workspace-a")
                );
                assert_eq!(runtime_id, None);
            }
            _ => panic!("expected Workers mode"),
        }
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
            "Runtime API target cannot be combined with --worker, --socket, --session, or --profile"
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
                mode:
                    LaunchMode::Workers {
                        include_stopped,
                        all,
                        ..
                    },
                ..
            } => {
                assert!(include_stopped);
                assert!(!all);
            }
            _ => panic!("expected Workers mode"),
        }
    }

    #[test]
    fn parse_resume_workspace_scope() {
        match parse_args_from(["resume", "--workspace", "/tmp/resume-workspace"]).unwrap() {
            Mode::Tui {
                mode:
                    LaunchMode::Workers {
                        include_stopped,
                        all,
                        ..
                    },
                workspace_root,
                ..
            } => {
                assert!(include_stopped);
                assert!(!all);
                assert_eq!(workspace_root, PathBuf::from("/tmp/resume-workspace"));
            }
            _ => panic!("expected Workers mode"),
        }
    }

    #[test]
    fn parse_resume_all_scope() {
        match parse_args_from(["resume", "--all"]).unwrap() {
            Mode::Tui {
                mode:
                    LaunchMode::Workers {
                        include_stopped,
                        all,
                        ..
                    },
                ..
            } => {
                assert!(include_stopped);
                assert!(all);
            }
            _ => panic!("expected Workers mode"),
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
    fn parse_workspace_command_is_removed_from_yoi_surface() {
        let err = parse_args_from(["workspace", "serve", "--listen", "127.0.0.1:0"]).unwrap_err();
        assert_eq!(err.to_string(), "unknown command `workspace`");
    }

    #[test]
    fn parse_server_command_is_removed_from_yoi_surface() {
        let err = parse_args_from(["server", "identity", "show"]).unwrap_err();
        assert_eq!(err.to_string(), "unknown command `server`");
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
                ..
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
                    "p.toml".to_string(),
                    "--session".to_string(),
                    segment_id,
                ],
                "--profile can only be used for fresh spawn",
            ),
            (
                vec![
                    "--profile".to_string(),
                    "p.toml".to_string(),
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
                mode:
                    LaunchMode::Panel {
                        include_stopped: false,
                    },
                workspace_root,
                ..
            } => assert_eq!(workspace_root, PathBuf::from("/tmp/other-workspace")),
            _ => panic!("expected Panel mode"),
        }
    }

    #[test]
    fn parse_panel_stopped_mode() {
        for flag in ["-r", "--stopped", "--restoreable"] {
            match parse_args_from(["panel", flag, "--workspace", "/tmp/other-workspace"]).unwrap() {
                Mode::Tui {
                    mode:
                        LaunchMode::Panel {
                            include_stopped: true,
                        },
                    workspace_root,
                    ..
                } => assert_eq!(workspace_root, PathBuf::from("/tmp/other-workspace")),
                _ => panic!("expected Panel stopped mode for {flag}"),
            }
        }
    }

    #[test]
    fn parse_backend_panel_stopped_is_not_supported() {
        let err =
            parse_args_from(["--backend", "http://127.0.0.1:8787", "panel", "-r"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "yoi panel -r is only supported for local targets; Backend panel restore UI is not implemented"
        );
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
