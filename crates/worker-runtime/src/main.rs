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
use worker_runtime::identity::RuntimeId;
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
    let mut factory = ProfileRuntimeWorkerFactory::new(config.workspace_root.clone())
        .with_cwd(config.cwd.clone());
    if let Some(store_dir) = config.worker_store_dir.clone() {
        factory = factory.with_store_dir(store_dir);
    }
    if let Some(metadata_dir) = config.worker_metadata_dir.clone() {
        factory = factory.with_worker_metadata_dir(metadata_dir);
    }
    if let Some(runtime_base_dir) = config.worker_runtime_base_dir.clone() {
        factory = factory.with_runtime_base_dir(runtime_base_dir);
    }
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
    let working_directory_root = working_directory_runtime_root(config);
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(factory)
            .map_err(ProcessError::WorkerAdapter)?
            .with_working_directory_materializer(LocalGitWorktreeMaterializer::new(
                working_directory_root,
            )),
    );

    match &config.http.store {
        RuntimeHttpStoreSelection::Memory => {
            Runtime::with_execution_backend(runtime_options_from_http(&config.http), backend)
                .map_err(ProcessError::Runtime)
        }
        RuntimeHttpStoreSelection::Fs { root } => {
            let mut options = FsRuntimeStoreOptions::new(root.clone());
            options.runtime_id = config.http.runtime_id.clone();
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

fn working_directory_runtime_root(config: &ProcessConfig) -> PathBuf {
    match &config.http.store {
        RuntimeHttpStoreSelection::Fs { root } => root.clone(),
        _ => config
            .worker_runtime_base_dir
            .clone()
            .unwrap_or_else(|| env::temp_dir().join("yoi-worker-runtime")),
    }
}

fn runtime_options_from_http(config: &RuntimeHttpServerConfig) -> RuntimeOptions {
    RuntimeOptions {
        runtime_id: config.runtime_id.clone(),
        display_name: config.display_name.clone(),
        limits: config.limits.clone(),
    }
}

fn parse_args<I, S>(args: I) -> Result<Option<ProcessConfig>, ProcessError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut config = ProcessConfig::default()?;
    let mut store = StoreArg::Memory;
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
            "--runtime-id" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                config.http.runtime_id = Some(RuntimeId::new(value).ok_or_else(|| {
                    ProcessError::usage("--runtime-id must not be empty".to_string())
                })?);
            }
            "--display-name" => {
                config.http.display_name = Some(take_value(&flag, inline_value, &mut args)?);
            }
            "--workspace" => {
                config.workspace_root = PathBuf::from(take_value(&flag, inline_value, &mut args)?);
            }
            "--cwd" => {
                config.cwd = PathBuf::from(take_value(&flag, inline_value, &mut args)?);
            }
            "--worker-store-dir" => {
                config.worker_store_dir =
                    Some(PathBuf::from(take_value(&flag, inline_value, &mut args)?));
            }
            "--worker-metadata-dir" => {
                config.worker_metadata_dir =
                    Some(PathBuf::from(take_value(&flag, inline_value, &mut args)?));
            }
            "--worker-runtime-base-dir" => {
                config.worker_runtime_base_dir =
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
                store = match value.as_str() {
                    "memory" => StoreArg::Memory,
                    "fs" | "fs-store" => StoreArg::Fs { root: None },
                    _ => {
                        return Err(ProcessError::usage(format!(
                            "unsupported --store `{value}`; expected `memory` or `fs`"
                        )));
                    }
                };
            }
            "--fs-root" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                store = StoreArg::Fs {
                    root: Some(PathBuf::from(value)),
                };
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
            "--max-transcript-projection-items" => {
                config.http.limits.max_transcript_projection_items =
                    parse_usize_flag(&flag, take_value(&flag, inline_value, &mut args)?)?;
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

    apply_store_selection(&mut config.http, store)?;
    config.workspace_root = normalize_workspace_path(config.workspace_root)?;
    if config.cwd.as_os_str().is_empty() {
        config.cwd = config.workspace_root.clone();
    } else {
        config.cwd = normalize_child_path(&config.workspace_root, config.cwd);
    }
    Ok(Some(config))
}

fn normalize_workspace_path(path: PathBuf) -> Result<PathBuf, ProcessError> {
    let absolute = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    Ok(absolute.canonicalize().unwrap_or(absolute))
}

fn normalize_child_path(base: &std::path::Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path.canonicalize().unwrap_or(path)
    } else {
        let absolute = base.join(path);
        absolute.canonicalize().unwrap_or(absolute)
    }
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

fn parse_usize_flag(flag: &str, value: String) -> Result<usize, ProcessError> {
    value
        .parse::<usize>()
        .map_err(|error| ProcessError::usage(format!("invalid {flag} value `{value}`: {error}")))
}

fn apply_store_selection(
    config: &mut RuntimeHttpServerConfig,
    store: StoreArg,
) -> Result<(), ProcessError> {
    match store {
        StoreArg::Memory => {
            config.store = RuntimeHttpStoreSelection::Memory;
            Ok(())
        }
        StoreArg::Fs { root } => {
            let root = root.ok_or_else(|| {
                ProcessError::usage("--store fs requires --fs-root <PATH>".to_string())
            })?;
            config.store = RuntimeHttpStoreSelection::Fs { root };
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessConfig {
    http: RuntimeHttpServerConfig,
    workspace_root: PathBuf,
    cwd: PathBuf,
    worker_store_dir: Option<PathBuf>,
    worker_metadata_dir: Option<PathBuf>,
    worker_runtime_base_dir: Option<PathBuf>,
    backend_resource_endpoint: Option<String>,
    backend_resource_token: Option<String>,
    profile: Option<String>,
}

impl ProcessConfig {
    fn default() -> Result<Self, ProcessError> {
        let workspace_root = env::current_dir()?;
        Ok(Self {
            http: RuntimeHttpServerConfig::default(),
            cwd: workspace_root.clone(),
            workspace_root,
            worker_store_dir: None,
            worker_metadata_dir: None,
            worker_runtime_base_dir: None,
            backend_resource_endpoint: None,
            backend_resource_token: None,
            profile: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StoreArg {
    Memory,
    Fs { root: Option<PathBuf> },
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
    "Usage: worker-runtime-rest-server [OPTIONS]\n\n\
Starts a worker-backed Runtime REST command API for a trusted backend/proxy.\n\
Browsers must not connect to this Runtime process directly.\n\n\
Options:\n\
  --bind <ADDR>                         Bind socket address (default: 127.0.0.1:38800)\n\
  --runtime-id <ID>                     Runtime authority id (default: generated)\n\
  --display-name <NAME>                 Runtime display name\n\
  --workspace <PATH>                    Workspace root used for spawned Workers (default: cwd)\n\
  --cwd <PATH>                          Process cwd used for spawned Workers (default: workspace)\n\
  --profile <SELECTOR>                  Force spawned Workers to use a Profile selector\n\
  --worker-store-dir <PATH>             Worker session store directory\n\
  --worker-metadata-dir <PATH>          Worker metadata directory\n\
  --worker-runtime-base-dir <PATH>      Worker controller runtime directory\n\
  --backend-resource-endpoint <URL>     Internal Backend resource fetch endpoint for resource handles\n\
  --backend-resource-token <TOKEN>      Optional bearer token for the Backend resource fetch endpoint\n\
  --store <memory|fs>                   Runtime catalog store selection (default: memory)\n\
  --fs-root <PATH>                      Runtime catalog filesystem store root\n\
  --local-token <TOKEN>                 Minimal local bearer token placeholder\n\
  --local-token-env <ENV>               Read local bearer token placeholder from env\n\
  --max-transcript-projection-items <N> Override transcript projection limit\n\
  --max-event-batch-items <N>           Override event batch limit\n\
  -h, --help                            Show this help"
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
    fn normalizes_relative_workspace_for_worker_spawn() {
        let current_dir = env::current_dir().unwrap().canonicalize().unwrap();
        let config = parse_args(["--workspace", ".", "--cwd", "."])
            .unwrap()
            .unwrap();

        assert_eq!(config.workspace_root, current_dir);
        assert_eq!(config.cwd, current_dir);
    }

    #[test]
    fn parses_memory_runtime_process_config() {
        let config = parse_args([
            "--bind",
            "127.0.0.1:0",
            "--runtime-id=runtime-alpha",
            "--display-name",
            "Runtime Alpha",
            "--workspace",
            "/tmp/workspace",
            "--cwd",
            "/tmp/workspace/subdir",
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
            "--runtime-id=runtime-alpha",
            "--display-name",
            "Runtime Alpha",
            "--workspace",
            "/tmp/workspace",
            "--cwd",
            "/tmp/workspace/subdir",
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
        assert_eq!(
            config.http.runtime_id.as_ref().map(ToString::to_string),
            Some("runtime-alpha".to_string())
        );
        assert_eq!(config.http.display_name.as_deref(), Some("Runtime Alpha"));
        assert_eq!(config.workspace_root, PathBuf::from("/tmp/workspace"));
        assert_eq!(config.cwd, PathBuf::from("/tmp/workspace/subdir"));
        assert_eq!(config.profile.as_deref(), Some("builtin:coder"));
        assert_eq!(config.http.local_token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn parses_fs_store_runtime_process_config() {
        let config = parse_args(["--store", "fs", "--fs-root", "/tmp/runtime-store"])
            .unwrap()
            .unwrap();
        assert!(matches!(
            config.http.store,
            RuntimeHttpStoreSelection::Fs { ref root } if root == &PathBuf::from("/tmp/runtime-store")
        ));
    }
}
