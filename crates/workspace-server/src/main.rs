use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use worker_runtime::auth::{RuntimeIdentityMaterial, decode_public_key};
use yoi_workspace_server::hosts::{RemoteRuntimeAuthConfig, RemoteRuntimeConfig};
use yoi_workspace_server::store::{SqliteWorkspaceStore, TrustedRuntimeRecord};
use yoi_workspace_server::{
    BackendRuntimesConfigFile, ControlPlaneStore, ResolvedWorkspaceBackendConfig, ServerConfig,
    ServerHostConfigFile, WorkspaceIdentity, WorkspaceRecord, serve_workspace_catalog,
};

#[derive(Debug)]
enum Command {
    Serve(ServeOptions),
    Identity(Vec<String>),
    TrustRuntime(Vec<String>),
    Skills(SkillsCommand),
    Help,
}

#[derive(Debug)]
struct ServeOptions {
    listen: Option<SocketAddr>,
    config: Option<PathBuf>,
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
        Command::Identity(args) => run_identity_command(args),
        Command::TrustRuntime(args) => run_trust_runtime_command(args),
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
        "identity" => Ok(Command::Identity(rest.to_vec())),
        "trust-runtime" => Ok(Command::TrustRuntime(rest.to_vec())),
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
            "unknown command `{other}`; expected `identity`, `trust-runtime`, `migrate`, `skills`, or `serve`"
        ))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ServerIdentityFile {
    identity: RuntimeIdentityMaterial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PublicIdentityView {
    identity_id: String,
    public_key: String,
}

fn server_identity_path() -> PathBuf {
    ServerConfig::default_server_data_root().join("identity.toml")
}

fn read_server_identity_file(
    path: &Path,
) -> Result<Option<ServerIdentityFile>, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    Ok(Some(toml::from_str(&contents)?))
}

fn write_server_identity_file(
    path: &Path,
    identity: &ServerIdentityFile,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(identity)?;
    write_secret_file(path, contents.as_bytes())?;
    Ok(())
}

fn write_secret_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        use std::io::Write as _;
        file.write_all(contents)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

fn public_identity_view(identity: &RuntimeIdentityMaterial) -> PublicIdentityView {
    PublicIdentityView {
        identity_id: identity.identity_id.clone(),
        public_key: identity.public_key.clone(),
    }
}

fn run_identity_command(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = VecDeque::from(args);
    let subcommand = args
        .pop_front()
        .ok_or_else(|| CliError("identity requires `init` or `show`".to_string()))?;
    match subcommand.as_str() {
        "init" => {
            let mut server_id = None;
            let mut replace = false;
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--server-id" => server_id = Some(take_value(&flag, inline_value, &mut args)?),
                    "--replace" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        replace = true;
                    }
                    _ => {
                        return Err(Box::new(CliError(format!(
                            "unknown identity init argument `{flag}`"
                        ))));
                    }
                }
            }
            let server_id = server_id
                .ok_or_else(|| CliError("identity init requires --server-id".to_string()))?;
            let path = server_identity_path();
            if read_server_identity_file(&path)?.is_some() && !replace {
                return Err(Box::new(CliError(format!(
                    "server identity already exists at {}; pass --replace to rotate it",
                    path.display()
                ))));
            }
            let identity = RuntimeIdentityMaterial::generate(server_id)?;
            write_server_identity_file(
                &path,
                &ServerIdentityFile {
                    identity: identity.clone(),
                },
            )?;
            println!("server_id={}", identity.identity_id);
            println!("public_key={}", identity.public_key);
            println!("identity_file={}", path.display());
            Ok(())
        }
        "show" => {
            let mut json = false;
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--json" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        json = true;
                    }
                    _ => {
                        return Err(Box::new(CliError(format!(
                            "unknown identity show argument `{flag}`"
                        ))));
                    }
                }
            }
            let path = server_identity_path();
            let identity = read_server_identity_file(&path)?.ok_or_else(|| {
                CliError(format!(
                    "server identity is not initialized at {}",
                    path.display()
                ))
            })?;
            let view = public_identity_view(&identity.identity);
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                println!("server_id={}", view.identity_id);
                println!("public_key={}", view.public_key);
                println!("identity_file={}", path.display());
            }
            Ok(())
        }
        _ => Err(Box::new(CliError(format!(
            "unknown identity subcommand `{subcommand}`"
        )))),
    }
}

fn run_trust_runtime_command(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = VecDeque::from(args);
    let subcommand = args
        .pop_front()
        .ok_or_else(|| CliError("trust-runtime requires `add`, `list`, or `revoke`".to_string()))?;
    let database_path = ServerConfig::default_server_database_path();
    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = SqliteWorkspaceStore::open(&database_path)?;
    match subcommand.as_str() {
        "add" => {
            let mut runtime_id = None;
            let mut workspace_id = None;
            let mut base_url = None;
            let mut public_key = None;
            let mut display_name = None;
            let mut replace = false;
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--runtime-id" => {
                        runtime_id = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--workspace-id" => {
                        workspace_id = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--base-url" | "--endpoint" => {
                        base_url = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--public-key" => {
                        public_key = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--display-name" => {
                        display_name = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--replace" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        replace = true;
                    }
                    _ => {
                        return Err(Box::new(CliError(format!(
                            "unknown trust-runtime add argument `{flag}`"
                        ))));
                    }
                }
            }
            let runtime_id = runtime_id
                .ok_or_else(|| CliError("trust-runtime add requires --runtime-id".to_string()))?;
            let workspace_id = workspace_id
                .ok_or_else(|| CliError("trust-runtime add requires --workspace-id".to_string()))?;
            if !store
                .list_workspaces()?
                .iter()
                .any(|workspace| workspace.workspace_id == workspace_id)
            {
                return Err(Box::new(CliError(format!(
                    "Workspace `{workspace_id}` is not registered"
                ))));
            }
            let base_url = base_url
                .ok_or_else(|| CliError("trust-runtime add requires --base-url".to_string()))?;
            let public_key = public_key
                .ok_or_else(|| CliError("trust-runtime add requires --public-key".to_string()))?;
            decode_public_key(&public_key)?;
            ensure_trusted_runtime_replace_allowed(&store, &runtime_id, replace)?;
            if let Some(existing) = store
                .list_trusted_runtimes(true)?
                .into_iter()
                .find(|runtime| runtime.runtime_id == runtime_id)
            {
                if existing.workspace_id.as_deref() != Some(workspace_id.as_str()) {
                    return Err(Box::new(CliError(format!(
                        "runtime `{runtime_id}` is already assigned to Workspace `{}` and cannot be reparented",
                        existing.workspace_id.as_deref().unwrap_or("unassigned")
                    ))));
                }
            }
            let now = Utc::now().to_rfc3339();
            store.upsert_trusted_runtime(&TrustedRuntimeRecord {
                runtime_id: runtime_id.clone(),
                workspace_id: Some(workspace_id.clone()),
                display_name: display_name.unwrap_or_else(|| runtime_id.clone()),
                base_url,
                public_key,
                created_at: now.clone(),
                updated_at: now,
                revoked_at: None,
            })?;
            println!("trusted_runtime_id={runtime_id}");
            println!("server_db={}", database_path.display());
            Ok(())
        }
        "list" => {
            let mut json = false;
            let mut include_revoked = false;
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--json" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        json = true;
                    }
                    "--include-revoked" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        include_revoked = true;
                    }
                    _ => {
                        return Err(Box::new(CliError(format!(
                            "unknown trust-runtime list argument `{flag}`"
                        ))));
                    }
                }
            }
            let records = store.list_trusted_runtimes(include_revoked)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                for runtime in records {
                    println!(
                        "runtime_id={} workspace_id={} base_url={} public_key={} revoked_at={}",
                        runtime.runtime_id,
                        runtime.workspace_id.unwrap_or_default(),
                        runtime.base_url,
                        runtime.public_key,
                        runtime.revoked_at.unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        "revoke" => {
            let mut runtime_id = None;
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--runtime-id" => {
                        runtime_id = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    _ => {
                        return Err(Box::new(CliError(format!(
                            "unknown trust-runtime revoke argument `{flag}`"
                        ))));
                    }
                }
            }
            let runtime_id = runtime_id.ok_or_else(|| {
                CliError("trust-runtime revoke requires --runtime-id".to_string())
            })?;
            let now = Utc::now().to_rfc3339();
            if !store.revoke_trusted_runtime(&runtime_id, &now)? {
                return Err(Box::new(CliError(format!(
                    "trusted runtime `{runtime_id}` is not registered or is already revoked"
                ))));
            }
            println!("revoked_runtime_id={runtime_id}");
            Ok(())
        }
        _ => Err(Box::new(CliError(format!(
            "unknown trust-runtime subcommand `{subcommand}`"
        )))),
    }
}

fn ensure_trusted_runtime_replace_allowed(
    store: &SqliteWorkspaceStore,
    runtime_id: &str,
    replace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if store
        .list_trusted_runtimes(true)?
        .iter()
        .any(|runtime| runtime.runtime_id == runtime_id)
        && !replace
    {
        return Err(Box::new(CliError(format!(
            "trusted runtime `{runtime_id}` already exists; pass --replace to update it"
        ))));
    }
    Ok(())
}

fn split_flag_value(arg: String) -> Result<(String, Option<String>), CliError> {
    if let Some((flag, value)) = arg.split_once('=') {
        if flag.is_empty() {
            return Err(CliError("empty flag name".to_string()));
        }
        Ok((flag.to_string(), Some(value.to_string())))
    } else {
        Ok((arg, None))
    }
}

fn take_value(
    flag: &str,
    inline_value: Option<String>,
    args: &mut VecDeque<String>,
) -> Result<String, CliError> {
    if let Some(value) = inline_value {
        return Ok(value);
    }
    args.pop_front()
        .ok_or_else(|| CliError(format!("{flag} requires a value")))
}

fn ensure_no_inline_value(flag: &str, inline_value: Option<&str>) -> Result<(), CliError> {
    if inline_value.is_some() {
        return Err(CliError(format!("{flag} does not accept a value")));
    }
    Ok(())
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

async fn run_serve(options: ServeOptions) -> Result<(), Box<dyn std::error::Error>> {
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
    let runtime_config = BackendRuntimesConfigFile::load_default()?;
    let mut resolved = ResolvedWorkspaceBackendConfig::local_dev(
        &workspace_root,
        identity,
        &host_config,
        &runtime_config,
    )?;
    resolved.database_path = database_path.clone();
    resolved.server.database_path = database_path.clone();
    append_trusted_runtime_sources(store.as_ref(), &mut resolved.server.remote_runtime_sources)?;
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

fn append_trusted_runtime_sources(
    store: &SqliteWorkspaceStore,
    remote_runtime_sources: &mut Vec<RemoteRuntimeConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(server_identity) = read_server_identity_file(&server_identity_path())? else {
        if !store.list_trusted_runtimes(false)?.is_empty() {
            return Err(Box::new(CliError(
                "trusted runtimes are registered but server identity is not initialized; run `yoi-server identity init`".to_string(),
            )));
        }
        return Ok(());
    };
    for runtime in store.list_trusted_runtimes(false)? {
        let Some(workspace_id) = runtime.workspace_id.clone() else {
            continue;
        };
        let auth = RemoteRuntimeAuthConfig {
            server_id: server_identity.identity.identity_id.clone(),
            server_private_key: server_identity.identity.private_key.clone(),
        };
        let remote = RemoteRuntimeConfig::new(
            runtime.runtime_id.clone(),
            runtime.display_name,
            runtime.base_url,
            None,
        )
        .with_workspace_id(workspace_id)
        .with_auth(auth);
        remote_runtime_sources.retain(|existing| existing.runtime_id != runtime.runtime_id);
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
        "yoi-server\n\nUsage:\n  yoi-server identity init --server-id <SERVER_ID> [--replace]\n  yoi-server identity show [--json]\n  yoi-server trust-runtime add --runtime-id <RUNTIME_ID> --workspace-id <WORKSPACE_ID> --base-url <URL> --public-key <KEY> [--display-name <NAME>] [--replace]\n  yoi-server trust-runtime list [--json] [--include-revoked]\n  yoi-server trust-runtime revoke --runtime-id <RUNTIME_ID>\n  yoi-server skills <COMMAND> [OPTIONS]\n  yoi-server serve [OPTIONS]\n\nOptions:\n  -h, --help    Print help"
    );
}

fn print_skills_help() {
    println!(
        "yoi-server skills\n\nUsage:\n  yoi-server skills list --workspace <WORKSPACE_ID>\n  yoi-server skills lint --workspace <WORKSPACE_ID>\n  yoi-server skills show <NAME> --workspace <WORKSPACE_ID>\n\nDescription:\n  Reads the active Server DB virtual-config revision. Catalog output is lightweight and omits imported Markdown content; detail output includes that content. allowed-tools and scripts are diagnostics only.\n\nOptions:\n      --workspace <WORKSPACE_ID>  Workspace id in the Server DB (required)\n  -h, --help                      Print help"
    );
}

fn print_serve_help() {
    println!(
        "yoi-server serve\n\nUsage:\n  yoi-server serve [OPTIONS]\n\nDescription:\n  Serves Workspaces recorded in the Yoi server DB. Host-level deployment settings are loaded from the explicit --config path or the canonical XDG yoi/server.toml path, and runtime sources are loaded from XDG runtimes.toml.\n\nOptions:\n      --listen <ADDR>     Listen address (default 127.0.0.1:8787)\n      --config <PATH>     Host-level Server config path\n  -h, --help              Print help"
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
    fn server_identity_init_requires_explicit_server_id() {
        let error = run_identity_command(vec!["init".to_string()]).unwrap_err();
        assert_eq!(error.to_string(), "identity init requires --server-id");
    }

    #[test]
    fn trusted_runtime_add_requires_replace_for_existing_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(temp.path().join("server.db")).unwrap();
        let public_key = RuntimeIdentityMaterial::generate("runtime-a")
            .unwrap()
            .public_key;
        store
            .upsert_trusted_runtime(&TrustedRuntimeRecord {
                runtime_id: "runtime-a".to_string(),
                workspace_id: None,
                display_name: "Runtime A".to_string(),
                base_url: "http://127.0.0.1:18080".to_string(),
                public_key,
                created_at: "2026-07-26T00:00:00Z".to_string(),
                updated_at: "2026-07-26T00:00:00Z".to_string(),
                revoked_at: None,
            })
            .unwrap();

        let error = ensure_trusted_runtime_replace_allowed(&store, "runtime-a", false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "trusted runtime `runtime-a` already exists; pass --replace to update it"
        );
        ensure_trusted_runtime_replace_allowed(&store, "runtime-a", true).unwrap();
    }
}
