//! Worker-backed Runtime REST process wrapper.
//!
//! This binary starts a Runtime command API with a real worker execution backend.
//! A REST Runtime process that cannot spawn Workers is not a valid Runtime for the
//! Workspace Browser.

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use worker_runtime::error::RuntimeError;
use worker_runtime::fs_store::FsRuntimeStoreOptions;
use worker_runtime::http_server::{
    RuntimeHttpServerConfig, RuntimeHttpServerError, RuntimeHttpStoreSelection,
};
use worker_runtime::worker_backend::{ProfileRuntimeWorkerFactory, WorkerRuntimeExecutionBackend};
use worker_runtime::working_directory::LocalGitWorktreeMaterializer;
use worker_runtime::{Runtime, RuntimeOptions};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("worker-runtime-rest-server: {error}");
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
    let Some(config) = parse_args(env::args().skip(1))? else {
        println!("{}", usage());
        return Ok(());
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.http.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        let worker_runtime = build_runtime(&config)?;
        eprintln!(
            "worker-runtime REST server listening on {local_addr}; intended client is a trusted backend/proxy, not a browser"
        );
        worker_runtime::http_server::serve_runtime_http(
            worker_runtime,
            listener,
            config.http.local_token,
        )
        .await
        .map_err(ProcessError::from)
    })?;
    Ok(())
}

fn build_runtime(config: &ProcessConfig) -> Result<Runtime, ProcessError> {
    let fs_paths = config.resolved_fs_paths();
    let mut factory = ProfileRuntimeWorkerFactory::new(fs_paths.worker_dir.join("worker-root"))
        .with_store_dir(fs_paths.worker_dir.join("sessions"))
        .with_worker_metadata_dir(fs_paths.worker_dir.join("metadata"))
        .with_runtime_base_dir(fs_paths.worker_dir.join("runtime"));
    if let Some(endpoint) = config.backend_resource_endpoint.clone() {
        factory = factory.with_resource_client(Arc::new(
            worker_runtime::resource::HttpBackendResourceClient::new(
                endpoint,
                config.backend_resource_token.clone(),
            ),
        ));
    }
    if let Some(profile) = config.profile.clone() {
        factory = factory.with_profile(profile);
    }
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(factory)
            .map_err(ProcessError::WorkerAdapter)?
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
                fs_paths.workdir_target.clone(),
            )),
    );

    match &config.http.store {
        RuntimeHttpStoreSelection::Memory => {
            Runtime::with_execution_backend(runtime_options_from_http(&config.http), backend)
                .map_err(ProcessError::Runtime)
        }
        RuntimeHttpStoreSelection::Fs { root } => {
            let mut options = FsRuntimeStoreOptions::new(root.clone());
            options.display_name = config.http.display_name.clone();
            options.limits = config.http.limits.clone();
            Runtime::with_fs_store_and_execution_backend(options, backend)
                .map_err(ProcessError::Runtime)
        }
        _ => Err(ProcessError::usage(
            "unsupported Runtime catalog store selection".to_string(),
        )),
    }
}

fn runtime_options_from_http(config: &RuntimeHttpServerConfig) -> RuntimeOptions {
    RuntimeOptions {
        display_name: config.display_name.clone(),
        limits: config.limits.clone(),
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
            "--profile" => {
                config.profile = Some(take_value(&flag, inline_value, &mut args)?);
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
            "--max-event-batch-items" => {
                config.http.limits.max_event_batch_items =
                    parse_usize_flag(&flag, take_value(&flag, inline_value, &mut args)?)?;
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

fn parse_usize_flag(flag: &str, value: String) -> Result<usize, ProcessError> {
    value
        .parse::<usize>()
        .map_err(|error| ProcessError::usage(format!("invalid {flag} value `{value}`: {error}")))
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
    profile: Option<String>,
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
            profile: None,
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
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl Error for ProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Usage(_) | Self::WorkerAdapter(_) => None,
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

fn usage() -> &'static str {
    r#"Usage: worker-runtime-rest-server [OPTIONS]

Starts a worker-backed Runtime REST command API for a trusted backend/proxy.
Browsers must not connect to this Runtime process directly.

Options:
  --bind <ADDR>                         Bind socket address (default: 127.0.0.1:38800)
  --display-name <NAME>                 Runtime display name
  --profile <SELECTOR>                  Force spawned Workers to use a Profile selector
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
  --max-event-batch-items <N>           Override event batch limit
  -h, --help                            Show this help"#
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
            "--profile",
            "builtin:coder",
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
            "--profile",
            "builtin:coder",
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
        assert_eq!(config.profile.as_deref(), Some("builtin:coder"));
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
    fn no_store_disables_runtime_catalog_persistence() {
        let config = parse_args(["--no-store"]).unwrap().unwrap();
        assert!(matches!(
            config.http.store,
            RuntimeHttpStoreSelection::Memory
        ));
    }
}
