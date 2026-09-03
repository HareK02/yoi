// Worker-backed Runtime REST process wrapper.
//
// This binary starts a Runtime command API with a real worker execution backend.
// A REST Runtime process that cannot spawn Workers is not a valid Runtime for the
// Workspace Browser.

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use worker_runtime::auth::{
    RuntimeHttpAuthConfig, RuntimeIdentityMaterial, TrustedServerKey, decode_public_key,
};
use worker_runtime::error::RuntimeError;
use worker_runtime::fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
use worker_runtime::http_server::{
    RuntimeHttpServerConfig, RuntimeHttpServerError, RuntimeHttpStoreSelection,
};
use worker_runtime::worker_backend::{ProfileRuntimeWorkerFactory, WorkerRuntimeExecutionBackend};
use worker_runtime::working_directory::RuntimeGitCacheMaterializer;
use worker_runtime::{Runtime, RuntimeOptions};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) == Some("__repository-ssh") {
        arguments.remove(0);
        return match worker_runtime::working_directory::run_repository_ssh_client(&arguments) {
            Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("yoi-runtime: {error}");
            if let ProcessError::Usage(_) = error {
                eprintln!();
                eprintln!("{}", usage());
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run() -> Result<(), ProcessError> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if matches!(args.first().map(String::as_str), Some("migrate")) {
        return run_migration_command(args);
    }
    if matches!(
        args.first().map(String::as_str),
        Some("identity" | "trust-server")
    ) {
        return run_auth_command(args);
    }
    let Some(mut config) = parse_args(args)? else {
        println!("{}", usage());
        return Ok(());
    };
    config.http.auth = load_runtime_http_auth(&config)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.http.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        let worker_runtime = build_runtime(&config)?;
        eprintln!(
            "yoi-runtime listening on {local_addr}; intended client is a trusted backend/proxy, not a browser"
        );
        worker_runtime::http_server::serve_runtime_http_with_auth(
            worker_runtime,
            listener,
            config.http.local_token,
            config.http.auth,
        )
        .await
        .map_err(ProcessError::from)
    })?;
    Ok(())
}

fn run_migration_command(mut args: Vec<String>) -> Result<(), ProcessError> {
    args.remove(0);
    let dry_run_index = args
        .iter()
        .position(|argument| argument == "--dry-run")
        .ok_or_else(|| ProcessError::usage("migrate currently requires --dry-run".to_string()))?;
    args.remove(dry_run_index);
    let explicit_runtime_id =
        if let Some(index) = args.iter().position(|argument| argument == "--runtime-id") {
            if index + 1 >= args.len() {
                return Err(ProcessError::usage(
                    "--runtime-id requires a value".to_string(),
                ));
            }
            let runtime_id = args.remove(index + 1);
            args.remove(index);
            Some(runtime_id)
        } else {
            None
        };
    let config = parse_args(args)?.ok_or_else(|| {
        ProcessError::usage("migrate requires a Runtime store configuration".to_string())
    })?;
    let root = match &config.http.store {
        RuntimeHttpStoreSelection::Fs { root } => root.clone(),
        RuntimeHttpStoreSelection::Memory => {
            return Err(ProcessError::usage(
                "migration dry-run requires the fs Runtime store".to_string(),
            ));
        }
        _ => {
            return Err(ProcessError::usage(
                "unsupported Runtime catalog store selection".to_string(),
            ));
        }
    };
    let persisted_runtime_id = read_runtime_auth_file(&runtime_auth_path(&config))?
        .identity
        .map(|identity| identity.identity_id);
    let runtime_id = match (persisted_runtime_id, explicit_runtime_id) {
        (Some(persisted), Some(explicit)) if persisted != explicit => {
            return Err(ProcessError::usage(format!(
                "--runtime-id {explicit} does not match persisted Runtime identity {persisted}"
            )));
        }
        (Some(persisted), _) => persisted,
        (None, Some(explicit)) if !explicit.is_empty() => explicit,
        (None, Some(_)) => {
            return Err(ProcessError::usage(
                "--runtime-id must not be empty".to_string(),
            ));
        }
        (None, None) => {
            return Err(ProcessError::usage(
                "migration dry-run requires a persisted Runtime identity or explicit --runtime-id"
                    .to_string(),
            ));
        }
    };
    let mut options = FsRuntimeStoreOptions::new(root).with_runtime_id(runtime_id);
    options.display_name = config.http.display_name.clone();
    let plan = FsRuntimeStore::migration_plan(&options).map_err(ProcessError::Runtime)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&plan)
            .map_err(|error| ProcessError::Auth(format!("encode migration plan: {error}")))?
    );
    Ok(())
}

fn build_runtime(config: &ProcessConfig) -> Result<Runtime, ProcessError> {
    let fs_paths = config.resolved_fs_paths();
    let runtime_store_dir = match &config.http.store {
        RuntimeHttpStoreSelection::Memory => fs_paths.runtime_dir.clone(),
        RuntimeHttpStoreSelection::Fs { root } => root.clone(),
        _ => fs_paths.runtime_dir.clone(),
    };
    let mut factory = ProfileRuntimeWorkerFactory::new(fs_paths.worker_dir.join("worker-root"))
        .with_runtime_store_dir(runtime_store_dir);
    let runtime_auth = read_runtime_auth_file(&runtime_auth_path(config))?;
    if let Some(identity) = runtime_auth.identity.clone() {
        if let [trusted_server] = runtime_auth.trusted_servers.as_slice() {
            factory =
                factory.with_runtime_request_identity(identity, trusted_server.server_id.clone());
        } else {
            factory = factory.with_remote_worker_mutation_identity(identity);
        }
    }
    let mut backend_resource_client: Option<
        Arc<dyn worker_runtime::resource::BackendResourceClient>,
    > = None;
    if let Some(endpoint) = config.backend_resource_endpoint.clone() {
        let identity = runtime_auth.identity.as_ref().ok_or_else(|| {
            ProcessError::Auth(
                "--backend-resource-endpoint requires a configured Runtime identity".to_owned(),
            )
        })?;
        let [trusted_server] = runtime_auth.trusted_servers.as_slice() else {
            return Err(ProcessError::Auth(
                "--backend-resource-endpoint requires exactly one trusted Server identity"
                    .to_owned(),
            ));
        };
        let client = Arc::new(
            worker_runtime::resource::HttpBackendResourceClient::new(
                endpoint,
                config.backend_resource_token.clone(),
            )
            .with_runtime_request_source(identity, trusted_server.server_id.clone()),
        );
        factory = factory.with_resource_client(client.clone());
        backend_resource_client = Some(client);
    }
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(factory)
            .map_err(ProcessError::WorkerAdapter)?
            .with_working_directory_materializer(RuntimeGitCacheMaterializer::new(
                fs_paths.workdir_target.clone(),
            )),
    );

    let runtime = match &config.http.store {
        RuntimeHttpStoreSelection::Memory => {
            Runtime::with_execution_backend(runtime_options_from_http(&config.http), backend)
                .map_err(ProcessError::Runtime)?
        }
        RuntimeHttpStoreSelection::Fs { root } => {
            let mut options = FsRuntimeStoreOptions::new(root.clone()).with_runtime_id(
                config
                    .http
                    .auth
                    .as_ref()
                    .map(|auth| auth.runtime_id.as_str())
                    .unwrap_or("local"),
            );
            options.display_name = config.http.display_name.clone();
            Runtime::with_fs_store_and_execution_backend(options, backend)
                .map_err(ProcessError::Runtime)?
        }
        _ => {
            return Err(ProcessError::usage(
                "unsupported Runtime catalog store selection".to_string(),
            ));
        }
    };
    if let Some(client) = backend_resource_client {
        runtime
            .install_backend_resource_client(client)
            .map_err(ProcessError::Runtime)?;
    }
    Ok(runtime)
}

fn runtime_options_from_http(config: &RuntimeHttpServerConfig) -> RuntimeOptions {
    RuntimeOptions {
        display_name: config.display_name.clone(),
    }
}

fn default_fs_root() -> PathBuf {
    manifest::paths::data_dir().unwrap_or_else(|| env::temp_dir().join("yoi"))
}

fn parse_args<I, S>(args: I) -> Result<Option<ProcessConfig>, ProcessError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut config = ProcessConfig::default()?;
    let mut args = args.into_iter().map(Into::into).collect::<VecDeque<_>>();

    while let Some(arg) = args.pop_front() {
        let (flag, inline_value) = split_flag_value(arg)?;
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--bind" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                config.http.bind_addr = value.parse::<SocketAddr>().map_err(|error| {
                    ProcessError::usage(format!("invalid --bind socket address `{value}`: {error}"))
                })?;
            }
            "--display-name" => {
                config.http.display_name = Some(take_value(&flag, inline_value, &mut args)?);
            }
            "--fs-worker-dir" => {
                config.fs_worker_dir =
                    Some(PathBuf::from(take_value(&flag, inline_value, &mut args)?));
            }
            "--fs-runtime-dir" => {
                config.fs_runtime_dir =
                    Some(PathBuf::from(take_value(&flag, inline_value, &mut args)?));
            }
            "--workdir-target" => {
                config.workdir_target =
                    Some(PathBuf::from(take_value(&flag, inline_value, &mut args)?));
            }
            "--backend-resource-endpoint" => {
                config.backend_resource_endpoint =
                    Some(take_value(&flag, inline_value, &mut args)?);
            }
            "--backend-resource-token" => {
                config.backend_resource_token = Some(take_value(&flag, inline_value, &mut args)?);
            }
            "--store" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                if !matches!(value.as_str(), "fs" | "fs-store") {
                    return Err(ProcessError::usage(format!(
                        "unsupported --store `{value}`; only `fs` is accepted (use --no-store for ephemeral mode)"
                    )));
                }
            }
            "--no-store" => {
                ensure_no_inline_value(&flag, inline_value.as_deref())?;
                config.no_store = true;
            }
            "--fs-root" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                config.fs_root = Some(PathBuf::from(value));
            }
            "--local-token" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                if value.is_empty() {
                    return Err(ProcessError::usage(
                        "--local-token must not be empty when provided".to_string(),
                    ));
                }
                config.http.local_token = Some(value);
            }
            "--local-token-env" => {
                let name = take_value(&flag, inline_value, &mut args)?;
                let value = env::var(&name).map_err(|error| {
                    ProcessError::usage(format!(
                        "failed to read --local-token-env `{name}`: {error}"
                    ))
                })?;
                if value.is_empty() {
                    return Err(ProcessError::usage(format!(
                        "--local-token-env `{name}` resolved to an empty value"
                    )));
                }
                config.http.local_token = Some(value);
            }
            _ => {
                return Err(ProcessError::usage(format!("unknown argument `{flag}`")));
            }
        }
    }

    apply_store_selection(&mut config);
    Ok(Some(config))
}

fn split_flag_value(arg: String) -> Result<(String, Option<String>), ProcessError> {
    if !arg.starts_with('-') {
        return Err(ProcessError::usage(format!(
            "unexpected positional argument `{arg}`"
        )));
    }
    if let Some((flag, value)) = arg.split_once('=') {
        Ok((flag.to_string(), Some(value.to_string())))
    } else {
        Ok((arg, None))
    }
}

fn take_value(
    flag: &str,
    inline_value: Option<String>,
    args: &mut VecDeque<String>,
) -> Result<String, ProcessError> {
    if let Some(value) = inline_value {
        return Ok(value);
    }
    args.pop_front()
        .ok_or_else(|| ProcessError::usage(format!("{flag} requires a value")))
}

fn ensure_no_inline_value(flag: &str, inline_value: Option<&str>) -> Result<(), ProcessError> {
    if inline_value.is_some() {
        return Err(ProcessError::usage(format!(
            "{flag} does not accept a value"
        )));
    }
    Ok(())
}

fn apply_store_selection(config: &mut ProcessConfig) {
    if config.no_store {
        config.http.store = RuntimeHttpStoreSelection::Memory;
    } else {
        let runtime_dir = config.resolved_fs_paths().runtime_dir;
        config.http.store = RuntimeHttpStoreSelection::Fs { root: runtime_dir };
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessConfig {
    http: RuntimeHttpServerConfig,
    fs_root: Option<PathBuf>,
    fs_worker_dir: Option<PathBuf>,
    fs_runtime_dir: Option<PathBuf>,
    workdir_target: Option<PathBuf>,
    no_store: bool,
    backend_resource_endpoint: Option<String>,
    backend_resource_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FsPathConfig {
    root: PathBuf,
    worker_dir: PathBuf,
    runtime_dir: PathBuf,
    workdir_target: PathBuf,
}

impl ProcessConfig {
    fn default() -> Result<Self, ProcessError> {
        Ok(Self {
            http: RuntimeHttpServerConfig::default(),
            fs_root: None,
            fs_worker_dir: None,
            fs_runtime_dir: None,
            workdir_target: None,
            no_store: false,
            backend_resource_endpoint: None,
            backend_resource_token: None,
        })
    }

    fn resolved_fs_paths(&self) -> FsPathConfig {
        let root = self.fs_root.clone().unwrap_or_else(default_fs_root);
        FsPathConfig {
            worker_dir: self
                .fs_worker_dir
                .clone()
                .unwrap_or_else(|| root.join("worker")),
            runtime_dir: self
                .fs_runtime_dir
                .clone()
                .unwrap_or_else(|| root.join("runtime")),
            workdir_target: self
                .workdir_target
                .clone()
                .unwrap_or_else(|| root.join("workdirs")),
            root,
        }
    }
}

#[derive(Debug)]
enum ProcessError {
    Usage(String),
    Server(RuntimeHttpServerError),
    Runtime(RuntimeError),
    WorkerAdapter(String),
    Auth(String),
    Io(std::io::Error),
}

impl ProcessError {
    fn usage(message: String) -> Self {
        Self::Usage(message)
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => message.fmt(f),
            Self::Server(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::WorkerAdapter(error) => error.fmt(f),
            Self::Auth(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl Error for ProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Usage(_) | Self::WorkerAdapter(_) | Self::Auth(_) => None,
            Self::Server(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<RuntimeHttpServerError> for ProcessError {
    fn from(error: RuntimeHttpServerError) -> Self {
        Self::Server(error)
    }
}

impl From<std::io::Error> for ProcessError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeAuthFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<RuntimeIdentityMaterial>,
    #[serde(default)]
    trusted_servers: Vec<TrustedServerKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimePublicIdentityView {
    identity_id: String,
    public_key: String,
}

fn runtime_public_identity_view(identity: &RuntimeIdentityMaterial) -> RuntimePublicIdentityView {
    RuntimePublicIdentityView {
        identity_id: identity.identity_id.clone(),
        public_key: identity.public_key.clone(),
    }
}

fn runtime_auth_path(config: &ProcessConfig) -> PathBuf {
    config.resolved_fs_paths().runtime_dir.join("auth.toml")
}

fn read_runtime_auth_file(path: &Path) -> Result<RuntimeAuthFile, ProcessError> {
    if !path.exists() {
        return Ok(RuntimeAuthFile::default());
    }
    let contents = std::fs::read_to_string(path)?;
    toml::from_str(&contents)
        .map_err(|error| ProcessError::Auth(format!("failed to parse {}: {error}", path.display())))
}

fn write_runtime_auth_file(path: &Path, auth: &RuntimeAuthFile) -> Result<(), ProcessError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(auth).map_err(|error| {
        ProcessError::Auth(format!("failed to serialize {}: {error}", path.display()))
    })?;
    write_secret_file(path, contents.as_bytes())
}

fn write_secret_file(path: &Path, contents: &[u8]) -> Result<(), ProcessError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).write(true).truncate(true).mode(0o600);
        std::io::Write::write_all(&mut options.open(path)?, contents)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

fn load_runtime_http_auth(
    config: &ProcessConfig,
) -> Result<Option<RuntimeHttpAuthConfig>, ProcessError> {
    let path = runtime_auth_path(config);
    let auth = read_runtime_auth_file(&path)?;
    let Some(identity) = auth.identity else {
        return Ok(None);
    };
    if auth.trusted_servers.is_empty() {
        return Ok(None);
    }
    Ok(Some(RuntimeHttpAuthConfig {
        runtime_id: identity.identity_id,
        trusted_servers: auth.trusted_servers,
    }))
}

fn parse_auth_storage_flags(args: &mut VecDeque<String>) -> Result<ProcessConfig, ProcessError> {
    let mut config = ProcessConfig::default()?;
    while let Some(arg) = args.pop_front() {
        let (flag, inline_value) = split_flag_value(arg)?;
        match flag.as_str() {
            "--fs-root" => {
                config.fs_root = Some(PathBuf::from(take_value(&flag, inline_value, args)?));
            }
            "--fs-runtime-dir" => {
                config.fs_runtime_dir = Some(PathBuf::from(take_value(&flag, inline_value, args)?));
            }
            _ => {
                return Err(ProcessError::usage(format!(
                    "unknown auth command argument `{flag}`"
                )));
            }
        }
    }
    Ok(config)
}

fn run_auth_command(args: Vec<String>) -> Result<(), ProcessError> {
    let mut args = VecDeque::from(args);
    let command = args.pop_front().unwrap_or_default();
    match command.as_str() {
        "identity" => run_identity_command(args),
        "trust-server" => run_trust_server_command(args),
        _ => Err(ProcessError::usage(format!(
            "unknown auth command `{command}`"
        ))),
    }
}

fn run_identity_command(mut args: VecDeque<String>) -> Result<(), ProcessError> {
    let subcommand = args.pop_front().ok_or_else(|| {
        ProcessError::usage("identity requires subcommand `init` or `show`".to_string())
    })?;
    match subcommand.as_str() {
        "init" => {
            let mut runtime_id = None;
            let mut replace = false;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--runtime-id" => {
                        runtime_id = Some(take_value(&flag, inline_value, &mut args)?)
                    }
                    "--replace" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        replace = true;
                    }
                    "--fs-root" | "--fs-runtime-dir" => {
                        rest.push_back(flag);
                        if let Some(value) = inline_value {
                            rest.push_back(value);
                        } else {
                            rest.push_back(args.pop_front().ok_or_else(|| {
                                ProcessError::usage(format!(
                                    "{} requires a value",
                                    rest.back().unwrap()
                                ))
                            })?);
                        }
                    }
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unknown identity init argument `{flag}`"
                        )));
                    }
                }
            }
            let config = parse_auth_storage_flags(&mut rest)?;
            let path = runtime_auth_path(&config);
            let mut auth = read_runtime_auth_file(&path)?;
            if auth.identity.is_some() && !replace {
                return Err(ProcessError::usage(format!(
                    "runtime identity already exists at {}; pass --replace to rotate it",
                    path.display()
                )));
            }
            let runtime_id = runtime_id.ok_or_else(|| {
                ProcessError::usage("identity init requires --runtime-id".to_string())
            })?;
            auth.identity = Some(
                RuntimeIdentityMaterial::generate(runtime_id)
                    .map_err(|error| ProcessError::Auth(error.to_string()))?,
            );
            write_runtime_auth_file(&path, &auth)?;
            let identity = auth.identity.as_ref().unwrap();
            println!("runtime_id={}", identity.identity_id);
            println!("public_key={}", identity.public_key);
            println!("auth_file={}", path.display());
            Ok(())
        }
        "show" => {
            let mut json = false;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--json" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        json = true;
                    }
                    "--fs-root" | "--fs-runtime-dir" => {
                        rest.push_back(flag);
                        if let Some(value) = inline_value {
                            rest.push_back(value);
                        } else {
                            rest.push_back(args.pop_front().ok_or_else(|| {
                                ProcessError::usage(format!(
                                    "{} requires a value",
                                    rest.back().unwrap()
                                ))
                            })?);
                        }
                    }
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unknown identity show argument `{flag}`"
                        )));
                    }
                }
            }
            let config = parse_auth_storage_flags(&mut rest)?;
            let path = runtime_auth_path(&config);
            let auth = read_runtime_auth_file(&path)?;
            let Some(identity) = auth.identity else {
                return Err(ProcessError::usage(format!(
                    "runtime identity is not initialized at {}",
                    path.display()
                )));
            };
            let view = runtime_public_identity_view(&identity);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&view)
                        .map_err(|error| ProcessError::Auth(error.to_string()))?
                );
            } else {
                println!("runtime_id={}", view.identity_id);
                println!("public_key={}", view.public_key);
                println!("auth_file={}", path.display());
            }
            Ok(())
        }
        _ => Err(ProcessError::usage(format!(
            "unknown identity subcommand `{subcommand}`"
        ))),
    }
}

fn run_trust_server_command(mut args: VecDeque<String>) -> Result<(), ProcessError> {
    let subcommand = args.pop_front().ok_or_else(|| {
        ProcessError::usage(
            "trust-server requires subcommand `add`, `list`, or `revoke`".to_string(),
        )
    })?;
    match subcommand.as_str() {
        "add" => {
            let mut server_id = None;
            let mut public_key = None;
            let mut display_name = None;
            let mut replace = false;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--server-id" => server_id = Some(take_value(&flag, inline_value, &mut args)?),
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
                    "--fs-root" | "--fs-runtime-dir" => {
                        rest.push_back(flag);
                        if let Some(value) = inline_value {
                            rest.push_back(value);
                        } else {
                            rest.push_back(args.pop_front().ok_or_else(|| {
                                ProcessError::usage(format!(
                                    "{} requires a value",
                                    rest.back().unwrap()
                                ))
                            })?);
                        }
                    }
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unknown trust-server add argument `{flag}`"
                        )));
                    }
                }
            }
            let config = parse_auth_storage_flags(&mut rest)?;
            let path = runtime_auth_path(&config);
            let mut auth = read_runtime_auth_file(&path)?;
            let server_id = server_id.ok_or_else(|| {
                ProcessError::usage("trust-server add requires --server-id".to_string())
            })?;
            let public_key = public_key.ok_or_else(|| {
                ProcessError::usage("trust-server add requires --public-key".to_string())
            })?;
            decode_public_key(&public_key)
                .map_err(|error| ProcessError::usage(error.to_string()))?;
            if auth
                .trusted_servers
                .iter()
                .any(|server| server.server_id == server_id)
                && !replace
            {
                return Err(ProcessError::usage(format!(
                    "trusted server `{server_id}` already exists; pass --replace to update it"
                )));
            }
            auth.trusted_servers
                .retain(|server| server.server_id != server_id);
            auth.trusted_servers.push(TrustedServerKey {
                server_id: server_id.clone(),
                public_key,
                display_name,
            });
            write_runtime_auth_file(&path, &auth)?;
            println!("trusted_server_id={server_id}");
            println!("auth_file={}", path.display());
            Ok(())
        }
        "list" => {
            let mut json = false;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--json" => {
                        ensure_no_inline_value(&flag, inline_value.as_deref())?;
                        json = true;
                    }
                    "--fs-root" | "--fs-runtime-dir" => {
                        rest.push_back(flag);
                        if let Some(value) = inline_value {
                            rest.push_back(value);
                        } else {
                            rest.push_back(args.pop_front().ok_or_else(|| {
                                ProcessError::usage(format!(
                                    "{} requires a value",
                                    rest.back().unwrap()
                                ))
                            })?);
                        }
                    }
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unknown trust-server list argument `{flag}`"
                        )));
                    }
                }
            }
            let config = parse_auth_storage_flags(&mut rest)?;
            let auth = read_runtime_auth_file(&runtime_auth_path(&config))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&auth.trusted_servers)
                        .map_err(|error| ProcessError::Auth(error.to_string()))?
                );
            } else {
                for server in auth.trusted_servers {
                    println!(
                        "server_id={} public_key={} display_name={}",
                        server.server_id,
                        server.public_key,
                        server.display_name.unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        "revoke" => {
            let mut server_id = None;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                let (flag, inline_value) = split_flag_value(arg)?;
                match flag.as_str() {
                    "--server-id" => server_id = Some(take_value(&flag, inline_value, &mut args)?),
                    "--fs-root" | "--fs-runtime-dir" => {
                        rest.push_back(flag);
                        if let Some(value) = inline_value {
                            rest.push_back(value);
                        } else {
                            rest.push_back(args.pop_front().ok_or_else(|| {
                                ProcessError::usage(format!(
                                    "{} requires a value",
                                    rest.back().unwrap()
                                ))
                            })?);
                        }
                    }
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unknown trust-server revoke argument `{flag}`"
                        )));
                    }
                }
            }
            let config = parse_auth_storage_flags(&mut rest)?;
            let path = runtime_auth_path(&config);
            let mut auth = read_runtime_auth_file(&path)?;
            let server_id = server_id.ok_or_else(|| {
                ProcessError::usage("trust-server revoke requires --server-id".to_string())
            })?;
            let before = auth.trusted_servers.len();
            auth.trusted_servers
                .retain(|server| server.server_id != server_id);
            if auth.trusted_servers.len() == before {
                return Err(ProcessError::usage(format!(
                    "trusted server `{server_id}` is not registered"
                )));
            }
            write_runtime_auth_file(&path, &auth)?;
            println!("revoked_server_id={server_id}");
            Ok(())
        }
        _ => Err(ProcessError::usage(format!(
            "unknown trust-server subcommand `{subcommand}`"
        ))),
    }
}

fn usage() -> &'static str {
    r#"Usage: yoi-runtime [OPTIONS]
       yoi-runtime migrate --dry-run [--runtime-id <ID>] [OPTIONS]

Starts a worker-backed Runtime REST command API for a trusted backend/proxy.
Browsers must not connect to this Runtime process directly.

Options:
  --bind <ADDR>                         Bind socket address (default: 127.0.0.1:38800)
  --display-name <NAME>                 Runtime display name
  --fs-root <PATH>                      Filesystem-backed storage root (default: user data dir)
  --fs-worker-dir <PATH>                Worker storage directory (default: <fs-root>/worker)
  --fs-runtime-dir <PATH>               Runtime catalog directory (default: <fs-root>/runtime)
  --workdir-target <PATH>               Workdir materialization target (default: <fs-root>/workdirs)
  --backend-resource-endpoint <URL>     Internal Backend resource fetch endpoint for resource handles
  --backend-resource-token <TOKEN>      Optional bearer token for the Backend resource fetch endpoint
  --store <fs>                          Runtime catalog store kind (default: fs)
  --no-store                            Disable Runtime catalog persistence for ephemeral runs
  --local-token <TOKEN>                 Minimal local bearer token placeholder
  --local-token-env <ENV>               Read local bearer token placeholder from env
  -h, --help                            Show this help

Auth commands:
  identity init --runtime-id ID [--replace] [--fs-root PATH] [--fs-runtime-dir PATH]
  identity show [--json] [--fs-root PATH] [--fs-runtime-dir PATH]
  trust-server add --server-id ID --public-key KEY [--display-name NAME] [--replace] [--fs-root PATH] [--fs-runtime-dir PATH]
  trust-server list [--json] [--fs-root PATH] [--fs-runtime-dir PATH]
  trust-server revoke --server-id ID [--fs-root PATH] [--fs-runtime-dir PATH]"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_does_not_start_server() {
        assert!(parse_args(["--help"]).unwrap().is_none());
    }

    #[test]
    fn default_bind_addr_is_stable_for_local_dev() {
        let config = parse_args([] as [&str; 0]).unwrap().unwrap();

        assert_eq!(
            config.http.bind_addr,
            "127.0.0.1:38800".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn rejects_removed_workspace_cwd_and_worker_path_options() {
        assert!(parse_args(["--workspace", "/tmp/workspace"]).is_err());
        assert!(parse_args(["--cwd", "/tmp/workspace"]).is_err());
        assert!(parse_args(["--worker-store-dir", "/tmp/sessions"]).is_err());
        assert!(parse_args(["--worker-metadata-dir", "/tmp/metadata"]).is_err());
        assert!(parse_args(["--worker-runtime-base-dir", "/tmp/runtime"]).is_err());
        assert!(parse_args(["--profile", "builtin:coder"]).is_err());
    }

    #[test]
    fn rejects_store_memory_in_favor_of_no_store() {
        assert!(parse_args(["--store", "memory"]).is_err());
    }

    #[test]
    fn parses_runtime_process_config() {
        let config = parse_args([
            "--bind",
            "127.0.0.1:0",
            "--display-name",
            "Runtime Alpha",
            "--local-token-env",
            "TEST_RUNTIME_TOKEN",
        ]);
        assert!(config.is_err());

        unsafe {
            env::set_var("TEST_RUNTIME_TOKEN", "secret-token");
        }
        let config = parse_args([
            "--bind",
            "127.0.0.1:0",
            "--display-name",
            "Runtime Alpha",
            "--local-token-env",
            "TEST_RUNTIME_TOKEN",
        ])
        .unwrap()
        .unwrap();
        unsafe {
            env::remove_var("TEST_RUNTIME_TOKEN");
        }

        assert_eq!(config.http.bind_addr, "127.0.0.1:0".parse().unwrap());
        assert_eq!(config.http.display_name.as_deref(), Some("Runtime Alpha"));
        assert_eq!(config.http.local_token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn parses_fs_store_runtime_process_config() {
        let config = parse_args([
            "--store",
            "fs",
            "--fs-root",
            "/tmp/yoi",
            "--fs-worker-dir",
            "/tmp/yoi-worker",
            "--fs-runtime-dir",
            "/tmp/yoi-runtime",
            "--workdir-target",
            "/tmp/yoi-workdirs",
        ])
        .unwrap()
        .unwrap();
        assert!(matches!(
            config.http.store,
            RuntimeHttpStoreSelection::Fs { ref root } if root == &PathBuf::from("/tmp/yoi-runtime")
        ));
        let paths = config.resolved_fs_paths();
        assert_eq!(paths.worker_dir, PathBuf::from("/tmp/yoi-worker"));
        assert_eq!(paths.workdir_target, PathBuf::from("/tmp/yoi-workdirs"));
    }

    #[test]
    fn migration_dry_run_accepts_previous_schema_document_without_workers_field() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        std::fs::create_dir_all(root.join("workers")).unwrap();
        std::fs::write(
            root.join("runtime.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 3,
                "display_name": "local",
                "backend": "fs_store",
                "status": "running",
                "next_diagnostic_id": 1,
                "config_bundles": {},
                "workspace_owners": {},
                "assignments": [],
                "execution": [],
                "diagnostics": []
            }))
            .unwrap(),
        )
        .unwrap();
        let before = std::fs::read(root.join("runtime.json")).unwrap();
        run_migration_command(vec![
            "migrate".to_string(),
            "--dry-run".to_string(),
            "--runtime-id".to_string(),
            "local".to_string(),
            "--store".to_string(),
            "fs".to_string(),
            "--fs-root".to_string(),
            temp.path().display().to_string(),
            "--fs-runtime-dir".to_string(),
            root.display().to_string(),
        ])
        .unwrap();
        assert_eq!(std::fs::read(root.join("runtime.json")).unwrap(), before);
    }

    #[test]
    fn migration_dry_run_rejects_previous_schema_document_that_cannot_decode_as_v4() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        std::fs::create_dir_all(root.join("workers")).unwrap();
        std::fs::write(
            root.join("runtime.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 3,
                "display_name": "local",
                "backend": "fs_store",
                "status": 3,
                "next_diagnostic_id": 1,
                "config_bundles": {},
                "workspace_owners": {},
                "diagnostics": []
            }))
            .unwrap(),
        )
        .unwrap();
        let before = std::fs::read(root.join("runtime.json")).unwrap();
        let error = run_migration_command(vec![
            "migrate".to_string(),
            "--dry-run".to_string(),
            "--runtime-id".to_string(),
            "local".to_string(),
            "--store".to_string(),
            "fs".to_string(),
            "--fs-root".to_string(),
            temp.path().display().to_string(),
            "--fs-runtime-dir".to_string(),
            root.display().to_string(),
        ])
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("decode migrated Runtime snapshot")
        );
        assert_eq!(std::fs::read(root.join("runtime.json")).unwrap(), before);
    }

    #[test]
    fn no_store_disables_runtime_catalog_persistence() {
        let config = parse_args(["--no-store"]).unwrap().unwrap();
        assert!(matches!(
            config.http.store,
            RuntimeHttpStoreSelection::Memory
        ));
    }
}
