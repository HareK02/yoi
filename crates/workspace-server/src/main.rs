use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use chrono::Utc;
use tokio::net::TcpListener;
use yoi_workspace_server::hosts::{EMBEDDED_RUNTIME_ID, RemoteRuntimeConfig};
use yoi_workspace_server::store::{
    SqliteWorkspaceStore, WorkspaceRuntimeAuthenticationMode, WorkspaceRuntimeBinding,
    WorkspaceRuntimeBindingState,
};
use yoi_workspace_server::{
    ControlPlaneStore, ResolvedWorkspaceBackendConfig, ServerConfig, ServerHostConfigFile,
    WorkspaceIdentity, WorkspaceRecord, serve_workspace_catalog,
};

#[derive(Debug)]
enum Command {
    Serve(ServeOptions),
    Migrate(MigrateOptions),
    Skills(SkillsCommand),
    Help,
}

#[derive(Debug)]
struct ServeOptions {
    listen: Option<SocketAddr>,
    config: Option<PathBuf>,
}

#[derive(Debug)]
struct MigrateOptions {
    database: Option<PathBuf>,
    dry_run: bool,
    help: bool,
}

#[derive(Debug)]
struct SkillWorkspaceOptions {
    workspace_id: String,
}

#[derive(Debug)]
enum SkillsCommand {
    List(SkillWorkspaceOptions),
    Lint(SkillWorkspaceOptions),
    Show { workspace_id: String, name: String },
}

#[derive(Debug)]
struct CliError(String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("yoi-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match parse_command(&args)? {
        Command::Serve(options) => run_serve(options).await,
        Command::Migrate(options) => run_migrate(options),
        Command::Skills(command) => run_skills(command),
        Command::Help => Ok(()),
    }
}

fn parse_command(args: &[String]) -> Result<Command, CliError> {
    let Some((command, rest)) = args.split_first() else {
        print_help();
        return Ok(Command::Help);
    };

    match command.as_str() {
        "migrate" => parse_migrate_options(rest).map(Command::Migrate),
        "skills" => parse_skills_command(rest),
        "serve" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                print_serve_help();
                return Ok(Command::Help);
            }
            Ok(Command::Serve(parse_serve_options(rest)?))
        }
        "--help" | "-h" => {
            print_help();
            Ok(Command::Help)
        }
        other => Err(CliError(format!(
            "unknown command `{other}`; expected `migrate`, `skills`, or `serve`"
        ))),
    }
}

fn run_skills(command: SkillsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SkillsCommand::List(options) => {
            let state = load_skill_workspace_config(&options.workspace_id)?;
            let catalog = yoi_workspace_server::skills::catalog(&state)?;
            println!("{}", serde_json::to_string_pretty(&catalog)?);
        }
        SkillsCommand::Lint(options) => {
            let state = load_skill_workspace_config(&options.workspace_id)?;
            let catalog = yoi_workspace_server::skills::lint(&state)?;
            println!("{}", serde_json::to_string_pretty(&catalog)?);
            if catalog
                .diagnostics
                .iter()
                .chain(
                    catalog
                        .entries
                        .iter()
                        .flat_map(|entry| entry.diagnostics.iter()),
                )
                .any(|diagnostic| {
                    diagnostic.severity == worker::skill::SkillDiagnosticSeverity::Error
                })
            {
                return Err(Box::new(CliError("Skill lint found errors".to_string())));
            }
        }
        SkillsCommand::Show { workspace_id, name } => {
            let state = load_skill_workspace_config(&workspace_id)?;
            let detail = yoi_workspace_server::skills::detail(&state, &name)?;
            println!("{}", serde_json::to_string_pretty(&detail)?);
        }
    }
    Ok(())
}

fn load_skill_workspace_config(
    workspace_id: &str,
) -> Result<yoi_workspace_server::config_source::WorkspaceConfigState, Box<dyn std::error::Error>> {
    let store = SqliteWorkspaceStore::open(ServerConfig::default_server_database_path())?;
    store.load_workspace_config(workspace_id)?.ok_or_else(|| {
        Box::new(CliError(format!(
            "Workspace `{workspace_id}` has no active config revision"
        ))) as Box<dyn std::error::Error>
    })
}

fn remote_runtime_config_from_binding(
    binding: WorkspaceRuntimeBinding,
) -> Result<Option<RemoteRuntimeConfig>, CliError> {
    if binding.runtime_id == EMBEDDED_RUNTIME_ID {
        return Ok(None);
    }
    if binding.authentication_mode != WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity {
        return Err(CliError(format!(
            "Runtime binding '{}:{}' still uses removed legacy Server-issued authentication",
            binding.workspace_id, binding.runtime_id
        )));
    }
    Ok(Some(
        RemoteRuntimeConfig::new(
            binding.runtime_id,
            binding.display_name,
            binding.base_url,
            None,
        )
        .with_workspace_id(binding.workspace_id)
        .with_strict_public_egress(true),
    ))
}

fn run_migrate(options: MigrateOptions) -> Result<(), Box<dyn std::error::Error>> {
    if options.help {
        print_migrate_help();
        return Ok(());
    }
    let database_path = options
        .database
        .unwrap_or_else(ServerConfig::default_server_database_path);
    let plan = if options.dry_run {
        SqliteWorkspaceStore::migration_plan(&database_path)?
    } else {
        SqliteWorkspaceStore::migrate_database(&database_path)?
    };

    println!("server_db={}", database_path.display());
    println!("current_schema_version={}", plan.current_schema_version);
    println!("target_schema_version={}", plan.target_schema_version);
    println!("migration_required={}", plan.migration_required());
    for migration in &plan.migrations {
        println!("migration={} {}", migration.version, migration.name);
    }
    println!(
        "result={}",
        if options.dry_run {
            "dry-run-validated"
        } else if plan.migration_required() {
            "migrated"
        } else {
            "unchanged"
        }
    );
    Ok(())
}

fn init_serve_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stdout)
        .with_ansi(false)
        .json()
        .flatten_event(true)
        .try_init();
}

async fn run_serve(options: ServeOptions) -> Result<(), Box<dyn std::error::Error>> {
    init_serve_tracing();
    let database_path = ServerConfig::default_server_database_path();
    if let Some(parent) = database_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let store = Arc::new(SqliteWorkspaceStore::open(&database_path)?);
    let workspaces = store.list_workspaces()?;
    let (identity, workspace_root) = if let Some(workspace) = workspaces.first() {
        (
            WorkspaceIdentity {
                workspace_id: workspace.workspace_id.clone(),
                created_at: workspace.created_at.clone(),
                display_name: workspace.display_name.clone(),
            },
            workspace_root_from_server_data(workspace)?,
        )
    } else {
        (
            WorkspaceIdentity {
                workspace_id: "00000000-0000-0000-0000-000000000000".to_string(),
                created_at: Utc::now().to_rfc3339(),
                display_name: "Server bootstrap".to_string(),
            },
            database_path
                .parent()
                .ok_or_else(|| CliError("server database path has no parent".to_string()))?
                .to_path_buf(),
        )
    };
    let host_config = match options.config.as_ref() {
        Some(path) => ServerHostConfigFile::load_from_path(path)?,
        None => ServerHostConfigFile::load_default()?,
    };
    let mut resolved =
        ResolvedWorkspaceBackendConfig::local_dev(&workspace_root, identity, &host_config)?;
    resolved.database_path = database_path.clone();
    resolved.server.database_path = database_path.clone();
    append_workspace_runtime_sources(store.as_ref(), &mut resolved.server.remote_runtime_sources)?;
    if let Some(listen) = options.listen {
        resolved = resolved.with_listen(listen);
    }

    let listener = TcpListener::bind(resolved.listen).await?;
    let local_addr = listener.local_addr()?;
    if resolved.server.backend_base_url.is_none() {
        resolved = resolved.with_backend_base_url(format!("http://{local_addr}"));
    }
    eprintln!(
        "yoi-server: serving {} workspace(s) from server DB `{}` on http://{}",
        workspaces.len(),
        database_path.display(),
        local_addr
    );
    serve_workspace_catalog(resolved.server, store, listener).await?;
    Ok(())
}

fn append_workspace_runtime_sources(
    store: &SqliteWorkspaceStore,
    remote_runtime_sources: &mut Vec<RemoteRuntimeConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspaces = store.list_workspaces()?;
    let bindings = workspaces
        .iter()
        .map(|workspace| {
            store
                .list_workspace_runtime_bindings(&workspace.workspace_id, false)
                .map(|bindings| {
                    bindings
                        .into_iter()
                        .filter(|binding| {
                            binding.runtime_id != yoi_workspace_server::hosts::EMBEDDED_RUNTIME_ID
                                && binding.state == WorkspaceRuntimeBindingState::Verified
                        })
                        .collect::<Vec<_>>()
                })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    for binding in bindings {
        let Some(remote) = remote_runtime_config_from_binding(binding)? else {
            continue;
        };
        remote_runtime_sources.retain(|existing| {
            existing.workspace_id.as_deref() != remote.workspace_id.as_deref()
                || existing.runtime_id != remote.runtime_id
        });
        remote_runtime_sources.push(remote);
    }
    Ok(())
}

fn workspace_root_from_server_data(workspace: &WorkspaceRecord) -> Result<PathBuf, CliError> {
    Ok(ServerConfig::default_workspace_backend_data_root(
        &workspace.workspace_id,
    ))
}

fn parse_skills_command(args: &[String]) -> Result<Command, CliError> {
    let Some((subcommand, rest)) = args.split_first() else {
        print_skills_help();
        return Ok(Command::Help);
    };
    match subcommand.as_str() {
        "list" => Ok(Command::Skills(SkillsCommand::List(
            parse_skill_workspace_options(rest)?,
        ))),
        "lint" => Ok(Command::Skills(SkillsCommand::Lint(
            parse_skill_workspace_options(rest)?,
        ))),
        "show" => {
            let Some((name, rest)) = rest.split_first() else {
                return Err(CliError("skills show requires a Skill name".to_string()));
            };
            Ok(Command::Skills(SkillsCommand::Show {
                workspace_id: parse_skill_workspace_options(rest)?.workspace_id,
                name: name.to_string(),
            }))
        }
        "--help" | "-h" => {
            print_skills_help();
            Ok(Command::Help)
        }
        other => Err(CliError(format!(
            "unknown skills subcommand `{other}`; expected `list`, `lint`, or `show`"
        ))),
    }
}

fn parse_skill_workspace_options(args: &[String]) -> Result<SkillWorkspaceOptions, CliError> {
    let mut workspace_id = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError("--workspace requires a Workspace id".to_string()))?;
                workspace_id = Some(value.clone());
            }
            value if value.starts_with("--workspace=") => {
                workspace_id = Some(value_after_equals(arg, "--workspace")?.to_string());
            }
            other => return Err(CliError(format!("unknown skills option `{other}`"))),
        }
    }
    let workspace_id = workspace_id.ok_or_else(|| {
        CliError("skills commands require --workspace <workspace-id>".to_string())
    })?;
    if workspace_id.trim().is_empty() {
        return Err(CliError("--workspace must not be empty".to_string()));
    }
    Ok(SkillWorkspaceOptions { workspace_id })
}

fn parse_migrate_options(args: &[String]) -> Result<MigrateOptions, CliError> {
    let mut database = None;
    let mut dry_run = false;
    let mut help = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--database" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--database requires a path".to_string()))?;
                database = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--database=") => {
                database = Some(PathBuf::from(value_after_equals(arg, "--database")?));
            }
            "--dry-run" => dry_run = true,
            "--help" | "-h" => help = true,
            _ if arg.starts_with('-') => {
                return Err(CliError(format!("unknown migrate option `{arg}`")));
            }
            _ => {
                return Err(CliError(format!(
                    "unexpected positional argument `{arg}`; use --database <PATH>"
                )));
            }
        }
        index += 1;
    }
    Ok(MigrateOptions {
        database,
        dry_run,
        help,
    })
}

fn parse_serve_options(args: &[String]) -> Result<ServeOptions, CliError> {
    let mut listen = None;
    let mut config = None;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--listen" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--listen requires a value".to_string()))?;
                listen = Some(parse_listen(value)?);
            }
            _ if arg.starts_with("--listen=") => {
                listen = Some(parse_listen(value_after_equals(arg, "--listen")?)?);
            }
            "--config" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| CliError("--config requires a path".to_string()))?;
                config = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--config=") => {
                config = Some(PathBuf::from(value_after_equals(arg, "--config")?));
            }
            _ if arg.starts_with('-') => {
                return Err(CliError(format!("unknown serve option `{arg}`")));
            }
            _ => {
                return Err(CliError(format!(
                    "unexpected positional argument `{arg}`; serve reads the workspace from the server DB"
                )));
            }
        }
        index += 1;
    }

    Ok(ServeOptions { listen, config })
}

fn value_after_equals<'a>(arg: &'a str, flag: &str) -> Result<&'a str, CliError> {
    let value = arg
        .strip_prefix(flag)
        .and_then(|rest| rest.strip_prefix('='))
        .unwrap_or_default();
    if value.is_empty() {
        return Err(CliError(format!("{flag} requires a value")));
    }
    Ok(value)
}

fn parse_listen(value: &str) -> Result<SocketAddr, CliError> {
    value
        .parse()
        .map_err(|_| CliError(format!("invalid --listen address `{value}`")))
}

fn print_help() {
    println!(
        "yoi-server\n\nUsage:\n  yoi-server migrate [--dry-run] [--database <PATH>]\n  yoi-server skills <COMMAND> [OPTIONS]\n  yoi-server serve [OPTIONS]\n\nOptions:\n  -h, --help    Print help"
    );
}

fn print_migrate_help() {
    println!(
        "yoi-server migrate\n\nUsage:\n  yoi-server migrate [OPTIONS]\n\nDescription:\n  Validates and applies every retained Server DB schema migration in order. --dry-run copies the database into memory and runs the same migration path without changing the source database. Stop yoi-server before applying migrations.\n\nOptions:\n      --database <PATH>  Server DB path (default: canonical Yoi server DB)\n      --dry-run          Validate the complete migration without changing the source DB\n  -h, --help             Print help"
    );
}

fn print_skills_help() {
    println!(
        "yoi-server skills\n\nUsage:\n  yoi-server skills list --workspace <WORKSPACE_ID>\n  yoi-server skills lint --workspace <WORKSPACE_ID>\n  yoi-server skills show <NAME> --workspace <WORKSPACE_ID>\n\nDescription:\n  Reads the active Server DB virtual-config revision. Catalog output is lightweight and omits imported Markdown content; detail output includes that content. allowed-tools and scripts are diagnostics only.\n\nOptions:\n      --workspace <WORKSPACE_ID>  Workspace id in the Server DB (required)\n  -h, --help                      Print help"
    );
}

fn print_serve_help() {
    println!(
        "yoi-server serve\n\nUsage:\n  yoi-server serve [OPTIONS]\n\nDescription:\n  Serves Workspaces recorded in the Yoi server DB. Host-level deployment settings are loaded from the explicit --config path or the canonical XDG yoi/server.toml path, and Runtime bindings are loaded from the Server DB.\n\nOptions:\n      --listen <ADDR>     Listen address (default 127.0.0.1:8787)\n      --config <PATH>     Host-level Server config path\n  -h, --help              Print help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_repository_local_commands_are_rejected() {
        for command in ["init", "config"] {
            let error = parse_command(&[command.to_string()]).unwrap_err();
            assert!(
                error.to_string().contains("unknown command"),
                "unexpected error for {command}: {error}"
            );
        }
    }

    #[test]
    fn parse_migrate_uses_the_shared_schema_path() {
        let command = parse_command(&[
            "migrate".to_string(),
            "--dry-run".to_string(),
            "--database=/tmp/server.db".to_string(),
        ])
        .unwrap();
        let Command::Migrate(options) = command else {
            panic!("expected migrate command");
        };
        assert!(options.dry_run);
        assert!(!options.help);
        assert_eq!(options.database, Some(PathBuf::from("/tmp/server.db")));
    }

    #[test]
    fn parse_migrate_rejects_unknown_options() {
        let error = parse_command(&["migrate".to_string(), "--apply-all".to_string()]).unwrap_err();
        assert_eq!(error.to_string(), "unknown migrate option `--apply-all`");
    }

    #[test]
    fn parse_skills_requires_server_workspace_id() {
        let error = parse_skills_command(&["list".to_string()]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "skills commands require --workspace <workspace-id>"
        );
        let command = parse_skills_command(&[
            "show".to_string(),
            "debug-rust".to_string(),
            "--workspace=workspace-a".to_string(),
        ])
        .unwrap();
        let Command::Skills(SkillsCommand::Show { workspace_id, name }) = command else {
            panic!("expected skills show command");
        };
        assert_eq!(workspace_id, "workspace-a");
        assert_eq!(name, "debug-rust");
    }

    #[test]
    fn parse_serve_accepts_listen_and_host_config() {
        let args = vec![
            "--listen".to_string(),
            "127.0.0.1:0".to_string(),
            "--config=/etc/yoi/server.toml".to_string(),
        ];
        let options = parse_serve_options(&args).unwrap();
        assert_eq!(options.listen.unwrap(), "127.0.0.1:0".parse().unwrap());
        assert_eq!(
            options.config.unwrap(),
            PathBuf::from("/etc/yoi/server.toml")
        );
    }

    #[test]
    fn parse_serve_rejects_legacy_workspace_flag() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec!["--workspace".to_string(), temp.path().display().to_string()];
        let error = parse_serve_options(&args).unwrap_err();
        assert_eq!(error.to_string(), "unknown serve option `--workspace`");
    }

    #[test]
    fn parse_serve_rejects_legacy_db_and_frontend_flags() {
        let error = parse_serve_options(&["--db=/tmp/yoi.db".to_string()]).unwrap_err();
        assert_eq!(error.to_string(), "unknown serve option `--db=/tmp/yoi.db`");
        let error = parse_serve_options(&["--frontend=/tmp/web".to_string()]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "unknown serve option `--frontend=/tmp/web`"
        );
    }
    #[test]
    fn runtime_startup_rejects_legacy_server_issuer_bindings() {
        let binding = WorkspaceRuntimeBinding {
            workspace_id: "workspace-a".to_owned(),
            runtime_id: "runtime-a".to_owned(),
            display_name: "Runtime A".to_owned(),
            base_url: "https://runtime.example.test".to_owned(),
            public_key: "unused".to_owned(),
            public_key_fingerprint: "unused".to_owned(),
            binding_revision: 1,
            state: WorkspaceRuntimeBindingState::Verified,
            authentication_mode: WorkspaceRuntimeAuthenticationMode::LegacyServerIssuer,
            workspace_key_id: None,
            workspace_key_generation: None,
            created_at: "2026-09-01T00:00:00Z".to_owned(),
            updated_at: "2026-09-01T00:00:00Z".to_owned(),
            revoked_at: None,
        };
        let error = remote_runtime_config_from_binding(binding)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "Runtime binding 'workspace-a:runtime-a' still uses removed legacy Server-issued authentication"
        );
    }

    #[test]
    fn parse_cli_rejects_removed_server_global_runtime_trust_commands() {
        for command in ["identity", "trust-runtime"] {
            let error = parse_command(&[command.to_owned()]).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("unknown command `{command}`; expected `migrate`, `skills`, or `serve`")
            );
        }
    }
}
