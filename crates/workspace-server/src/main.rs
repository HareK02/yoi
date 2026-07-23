use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tokio::net::TcpListener;
use yoi_workspace_server::store::RepositoryRecord;
use yoi_workspace_server::{
    BackendRuntimesConfigFile, ControlPlaneStore, ServerConfig, SqliteWorkspaceStore,
    WORKSPACE_BACKEND_CONFIG_TEMPLATE, WorkspaceBackendConfigFile, WorkspaceIdentity,
    WorkspaceRecord, serve,
};

#[derive(Debug)]
enum Command {
    Serve(ServeOptions),
    Init(InitOptions),
    ConfigDefault,
    ConfigDiff(WorkspacePathOptions),
    Skills(SkillsCommand),
    Help,
}

#[derive(Debug)]
struct ServeOptions {
    listen: Option<SocketAddr>,
}

#[derive(Debug)]
struct InitOptions {
    workspace: PathBuf,
}

#[derive(Debug)]
struct WorkspacePathOptions {
    workspace: PathBuf,
}

#[derive(Debug)]
enum SkillsCommand {
    List(WorkspacePathOptions),
    Lint(WorkspacePathOptions),
    Show { workspace: PathBuf, name: String },
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
            eprintln!("yoi-workspace-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match parse_command(&args)? {
        Command::Serve(options) => run_serve(options).await,
        Command::Init(options) => run_init(options).await,
        Command::ConfigDefault => run_config_default(),
        Command::ConfigDiff(options) => run_config_diff(options),
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
        "init" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                print_init_help();
                return Ok(Command::Help);
            }
            Ok(Command::Init(parse_init_options(rest)?))
        }
        "config" => parse_config_command(rest),
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
            "unknown command `{other}`; expected `init`, `config`, `skills`, or `serve`"
        ))),
    }
}

async fn run_init(options: InitOptions) -> Result<(), Box<dyn std::error::Error>> {
    run_init_with_database_path(options, ServerConfig::default_server_database_path()).await
}

async fn run_init_with_database_path(
    options: InitOptions,
    database_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = WorkspaceIdentity::load_or_init(&options.workspace)?;
    WorkspaceBackendConfigFile::ensure_local_config_for_workspace(&options.workspace)?;

    if let Some(parent) = database_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let store = SqliteWorkspaceStore::open(&database_path)?;
    store
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: identity.workspace_id.clone(),
            owner_account_id: None,
            display_name: identity.display_name.clone(),
            state: "active".to_string(),
            created_at: identity.created_at.clone(),
            updated_at: identity.created_at.clone(),
        })
        .await?;
    store.upsert_repository(&RepositoryRecord {
        workspace_id: identity.workspace_id.clone(),
        repository_id: "main".to_string(),
        name: "Main repository".to_string(),
        kind: "git".to_string(),
        provider: Some("git".to_string()),
        uri: options.workspace.display().to_string(),
        default_ref: Some("HEAD".to_string()),
        auth_ref_kind: None,
        auth_ref_key: None,
        created_at: identity.created_at.clone(),
        updated_at: identity.created_at.clone(),
    })?;

    eprintln!(
        "yoi-workspace-server: initialized workspace `{}` ({}) in server DB `{}`",
        options.workspace.display(),
        identity.workspace_id,
        database_path.display()
    );
    Ok(())
}

fn run_config_default() -> Result<(), Box<dyn std::error::Error>> {
    print!("{WORKSPACE_BACKEND_CONFIG_TEMPLATE}");
    Ok(())
}

fn run_config_diff(options: WorkspacePathOptions) -> Result<(), Box<dyn std::error::Error>> {
    let diff = WorkspaceBackendConfigFile::local_config_diff_for_workspace(&options.workspace)?;
    print!("{}", diff.text);
    Ok(())
}

fn run_skills(command: SkillsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SkillsCommand::List(options) => {
            let catalog = yoi_workspace_server::skills::catalog(&options.workspace);
            println!("{}", serde_json::to_string_pretty(&catalog)?);
        }
        SkillsCommand::Lint(options) => {
            let catalog = yoi_workspace_server::skills::lint(&options.workspace);
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
        SkillsCommand::Show { workspace, name } => {
            let detail = yoi_workspace_server::skills::detail(&workspace, &name)?;
            println!("{}", serde_json::to_string_pretty(&detail)?);
        }
    }
    Ok(())
}

async fn run_serve(options: ServeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let database_path = ServerConfig::default_server_database_path();
    if let Some(parent) = database_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let store = Arc::new(SqliteWorkspaceStore::open(&database_path)?);
    let workspace = select_serve_workspace(store.as_ref())?;
    let workspace_root = infer_workspace_root_from_repositories(store.as_ref(), &workspace)?;
    let identity = WorkspaceIdentity {
        workspace_id: workspace.workspace_id.clone(),
        created_at: workspace.created_at.clone(),
        display_name: workspace.display_name.clone(),
    };
    let runtime_config = BackendRuntimesConfigFile::load_default()?;
    let mut resolved = WorkspaceBackendConfigFile::default().resolve_with_runtime_config(
        &workspace_root,
        identity,
        &runtime_config,
    )?;
    resolved.database_path = database_path.clone();
    resolved.server.database_path = database_path.clone();
    if let Some(listen) = options.listen {
        resolved = resolved.with_listen(listen);
    }

    let listener = TcpListener::bind(resolved.listen).await?;
    eprintln!(
        "yoi-workspace-server: serving workspace `{}` from server DB `{}` on http://{}",
        workspace.workspace_id,
        database_path.display(),
        listener.local_addr()?
    );
    serve(resolved.server, store, listener).await?;
    Ok(())
}

fn select_serve_workspace(store: &SqliteWorkspaceStore) -> Result<WorkspaceRecord, CliError> {
    let workspaces = store
        .list_workspaces()
        .map_err(|error| CliError(format!("failed to list workspaces from server DB: {error}")))?;
    match workspaces.as_slice() {
        [] => Err(CliError(
            "server DB has no workspace records; run `yoi-workspace-server init --workspace <PATH>`".to_string(),
        )),
        [workspace] => Ok(workspace.clone()),
        _ => Err(CliError(format!(
            "server DB contains {} workspaces; serve workspace selection is not implemented yet",
            workspaces.len()
        ))),
    }
}

fn infer_workspace_root_from_repositories(
    store: &SqliteWorkspaceStore,
    workspace: &WorkspaceRecord,
) -> Result<PathBuf, CliError> {
    let repositories = store
        .list_repositories(&workspace.workspace_id)
        .map_err(|error| {
            CliError(format!(
                "failed to list repositories from server DB: {error}"
            ))
        })?;
    let Some(repository) = repositories
        .iter()
        .find(|repository| repository.repository_id == "main")
        .or_else(|| repositories.first())
    else {
        return Err(CliError(format!(
            "workspace `{}` has no repository records; cannot derive a workspace root",
            workspace.workspace_id
        )));
    };

    let repository_path = PathBuf::from(&repository.uri);
    if !repository_path.is_absolute() {
        return Err(CliError(format!(
            "repository `{}` has relative URI `{}`; repository records used by serve must be absolute paths",
            repository.repository_id, repository.uri
        )));
    }
    Ok(repository_path)
}

fn parse_config_command(args: &[String]) -> Result<Command, CliError> {
    let Some((subcommand, rest)) = args.split_first() else {
        print_config_help();
        return Ok(Command::Help);
    };
    match subcommand.as_str() {
        "default" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                print_config_help();
                return Ok(Command::Help);
            }
            if !rest.is_empty() {
                return Err(CliError(
                    "config default does not accept options".to_string(),
                ));
            }
            Ok(Command::ConfigDefault)
        }
        "diff" => {
            if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
                print_config_help();
                return Ok(Command::Help);
            }
            Ok(Command::ConfigDiff(parse_workspace_path_options(rest)?))
        }
        "--help" | "-h" => {
            print_config_help();
            Ok(Command::Help)
        }
        other => Err(CliError(format!(
            "unknown config subcommand `{other}`; expected `default` or `diff`"
        ))),
    }
}

fn parse_skills_command(args: &[String]) -> Result<Command, CliError> {
    let Some((subcommand, rest)) = args.split_first() else {
        print_skills_help();
        return Ok(Command::Help);
    };
    match subcommand.as_str() {
        "list" => Ok(Command::Skills(SkillsCommand::List(
            parse_workspace_path_options(rest)?,
        ))),
        "lint" => Ok(Command::Skills(SkillsCommand::Lint(
            parse_workspace_path_options(rest)?,
        ))),
        "show" => {
            let Some((name, rest)) = rest.split_first() else {
                return Err(CliError("skills show requires a Skill name".to_string()));
            };
            Ok(Command::Skills(SkillsCommand::Show {
                workspace: parse_workspace_path_options(rest)?.workspace,
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

fn parse_workspace_path_options(args: &[String]) -> Result<WorkspacePathOptions, CliError> {
    let mut workspace = std::env::current_dir()
        .map_err(|error| CliError(format!("failed to read current dir: {error}")))?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError("--workspace requires a path".to_string()))?;
                workspace = PathBuf::from(value);
            }
            value if value.starts_with("--workspace=") => {
                workspace = PathBuf::from(value_after_equals(arg, "--workspace")?);
            }
            other => return Err(CliError(format!("unknown workspace option `{other}`"))),
        }
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| CliError(format!("failed to canonicalize workspace: {error}")))?;
    Ok(WorkspacePathOptions { workspace })
}

fn parse_init_options(args: &[String]) -> Result<InitOptions, CliError> {
    let mut workspace = std::env::current_dir()
        .map_err(|error| CliError(format!("failed to read current dir: {error}")))?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError("--workspace requires a path".to_string()))?;
                workspace = PathBuf::from(value);
            }
            value if value.starts_with("--workspace=") => {
                workspace = PathBuf::from(value_after_equals(arg, "--workspace")?);
            }
            other => return Err(CliError(format!("unknown init option `{other}`"))),
        }
    }

    let workspace = workspace
        .canonicalize()
        .map_err(|error| CliError(format!("failed to canonicalize workspace: {error}")))?;
    Ok(InitOptions { workspace })
}

fn parse_serve_options(args: &[String]) -> Result<ServeOptions, CliError> {
    let mut listen = None;

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

    Ok(ServeOptions { listen })
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
        "yoi-workspace-server\n\nUsage:\n  yoi-workspace-server init [OPTIONS]\n  yoi-workspace-server config <COMMAND> [OPTIONS]\n  yoi-workspace-server skills <COMMAND> [OPTIONS]\n  yoi-workspace-server serve [OPTIONS]\n\nOptions:\n  -h, --help    Print help"
    );
}

fn print_init_help() {
    println!(
        "yoi-workspace-server init\n\nUsage:\n  yoi-workspace-server init [OPTIONS]\n\nDescription:\n  Initializes a Workspace identity, copies the packaged Backend config template to .yoi/workspace-backend.local.toml, and registers the Workspace in the Yoi server DB.\n\nOptions:\n      --workspace <PATH>  Workspace root to initialize (defaults to cwd)\n  -h, --help              Print help"
    );
}

fn print_config_help() {
    println!(
        "yoi-workspace-server config\n\nUsage:\n  yoi-workspace-server config default\n  yoi-workspace-server config diff [OPTIONS]\n\nDescription:\n  Prints the packaged Workspace Backend config template or compares it with the workspace-local config.\n\nOptions for diff:\n      --workspace <PATH>  Workspace root (defaults to cwd)\n  -h, --help              Print help"
    );
}

fn print_skills_help() {
    println!(
        "yoi-workspace-server skills\n\nUsage:\n  yoi-workspace-server skills list [OPTIONS]\n  yoi-workspace-server skills lint [OPTIONS]\n  yoi-workspace-server skills show <NAME> [OPTIONS]\n\nDescription:\n  Uses the Workspace backend Skill catalog/lint/detail authority. Catalog output is lightweight and omits full SKILL.md bodies; detail output includes the body. allowed-tools and scripts are diagnostics only.\n\nOptions:\n      --workspace <PATH>  Workspace root (defaults to cwd)\n  -h, --help              Print help"
    );
}

fn print_serve_help() {
    println!(
        "yoi-workspace-server serve\n\nUsage:\n  yoi-workspace-server serve [OPTIONS]\n\nDescription:\n  Serves the Workspace recorded in the Yoi server DB. Workspace records are stored in the XDG/Yoi data directory, and runtime sources are loaded from XDG runtimes.toml.\n\nOptions:\n      --listen <ADDR>     Listen address (default 127.0.0.1:8787)\n  -h, --help              Print help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoi_workspace_server::{
        WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH, WORKSPACE_BACKEND_CONFIG_TEMPLATE,
        WORKSPACE_IDENTITY_RELATIVE_PATH,
    };

    #[test]
    fn parse_init_defaults_workspace_to_cwd_or_flag() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec!["--workspace".to_string(), temp.path().display().to_string()];
        let options = parse_init_options(&args).unwrap();
        assert_eq!(options.workspace, temp.path().canonicalize().unwrap());
    }

    #[test]
    fn parse_serve_accepts_listen_only() {
        let args = vec!["--listen".to_string(), "127.0.0.1:0".to_string()];
        let options = parse_serve_options(&args).unwrap();
        assert_eq!(options.listen.unwrap(), "127.0.0.1:0".parse().unwrap());
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

    #[tokio::test]
    async fn init_creates_identity_local_config_and_server_records() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("data").join("server").join("server.db");
        run_init_with_database_path(
            InitOptions {
                workspace: temp.path().canonicalize().unwrap(),
            },
            database_path.clone(),
        )
        .await
        .unwrap();

        assert!(temp.path().join(WORKSPACE_IDENTITY_RELATIVE_PATH).exists());
        let local_config_path = temp.path().join(WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH);
        assert!(local_config_path.exists());
        assert_eq!(
            std::fs::read_to_string(local_config_path).unwrap(),
            WORKSPACE_BACKEND_CONFIG_TEMPLATE
        );
        assert!(
            !temp
                .path()
                .join(".yoi/workspace-backend.default.toml")
                .exists()
        );
        assert!(!temp.path().join(".yoi/workspace.db").exists());
        assert!(!temp.path().join(".yoi/embedded-runtime").exists());
        assert!(database_path.exists());

        let store = SqliteWorkspaceStore::open(&database_path).unwrap();
        let workspaces = store.list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        let repositories = store
            .list_repositories(&workspaces[0].workspace_id)
            .unwrap();
        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0].repository_id, "main");
        assert_eq!(
            repositories[0].uri,
            temp.path().canonicalize().unwrap().display().to_string()
        );
    }
}
