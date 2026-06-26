//! Minimal Runtime REST process wrapper.
//!
//! This binary is available only when the `http-server` feature is enabled. It
//! starts a Runtime-local command API intended for a trusted backend/proxy;
//! browsers must not connect to this Runtime process directly.

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use worker_runtime::http_server::{
    RuntimeHttpServer, RuntimeHttpServerConfig, RuntimeHttpServerError, RuntimeHttpStoreSelection,
};
use worker_runtime::identity::RuntimeId;

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
        .enable_io()
        .build()?;
    runtime.block_on(async move {
        let server = RuntimeHttpServer::bind(config).await?;
        let local_addr = server.local_addr()?;
        eprintln!(
            "worker-runtime REST server listening on {local_addr}; intended client is a trusted backend/proxy, not a browser"
        );
        server.serve().await
    })?;
    Ok(())
}

fn parse_args<I, S>(args: I) -> Result<Option<RuntimeHttpServerConfig>, ProcessError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut config = RuntimeHttpServerConfig::default();
    let mut store = StoreArg::Memory;
    let mut args = args.into_iter().map(Into::into).collect::<VecDeque<_>>();

    while let Some(arg) = args.pop_front() {
        let (flag, inline_value) = split_flag_value(arg)?;
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--bind" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                config.bind_addr = value.parse::<SocketAddr>().map_err(|error| {
                    ProcessError::usage(format!("invalid --bind socket address `{value}`: {error}"))
                })?;
            }
            "--runtime-id" => {
                let value = take_value(&flag, inline_value, &mut args)?;
                config.runtime_id = Some(RuntimeId::new(value).ok_or_else(|| {
                    ProcessError::usage("--runtime-id must not be empty".to_string())
                })?);
            }
            "--display-name" => {
                config.display_name = Some(take_value(&flag, inline_value, &mut args)?);
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
                config.local_token = Some(value);
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
                config.local_token = Some(value);
            }
            "--max-transcript-projection-items" => {
                config.limits.max_transcript_projection_items =
                    parse_usize_flag(&flag, take_value(&flag, inline_value, &mut args)?)?;
            }
            "--max-event-batch-items" => {
                config.limits.max_event_batch_items =
                    parse_usize_flag(&flag, take_value(&flag, inline_value, &mut args)?)?;
            }
            _ => {
                return Err(ProcessError::usage(format!("unknown argument `{flag}`")));
            }
        }
    }

    apply_store_selection(&mut config, store)?;
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
        StoreArg::Fs { root } => apply_fs_store_selection(config, root),
    }
}

#[cfg(feature = "fs-store")]
fn apply_fs_store_selection(
    config: &mut RuntimeHttpServerConfig,
    root: Option<PathBuf>,
) -> Result<(), ProcessError> {
    let root = root
        .ok_or_else(|| ProcessError::usage("--store fs requires --fs-root <PATH>".to_string()))?;
    config.store = RuntimeHttpStoreSelection::Fs { root };
    Ok(())
}

#[cfg(not(feature = "fs-store"))]
fn apply_fs_store_selection(
    _config: &mut RuntimeHttpServerConfig,
    root: Option<PathBuf>,
) -> Result<(), ProcessError> {
    let _ = root;
    Err(ProcessError::usage(
        "fs store selection requires building worker-runtime with features `http-server,fs-store`"
            .to_string(),
    ))
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
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl Error for ProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Usage(_) => None,
            Self::Server(error) => Some(error),
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
Starts the worker-runtime REST command API for a trusted backend/proxy.\n\
Browsers must not connect to this Runtime process directly.\n\n\
Options:\n\
  --bind <ADDR>                         Bind socket address (default: 127.0.0.1:0)\n\
  --runtime-id <ID>                     Runtime authority id (default: generated)\n\
  --display-name <NAME>                 Runtime display name\n\
  --store <memory|fs>                   Store selection (default: memory)\n\
  --fs-root <PATH>                      Filesystem store root; requires fs-store feature\n\
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
    fn parses_memory_runtime_process_config() {
        let config = parse_args([
            "--bind",
            "127.0.0.1:48181",
            "--runtime-id=runtime-review",
            "--display-name",
            "review runtime",
            "--store",
            "memory",
            "--local-token",
            "local-placeholder",
            "--max-transcript-projection-items",
            "32",
            "--max-event-batch-items=16",
        ])
        .unwrap()
        .unwrap();

        assert_eq!(
            config.bind_addr,
            "127.0.0.1:48181".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            config.runtime_id.as_ref().map(RuntimeId::as_str),
            Some("runtime-review")
        );
        assert_eq!(config.display_name.as_deref(), Some("review runtime"));
        assert!(matches!(config.store, RuntimeHttpStoreSelection::Memory));
        assert_eq!(config.local_token.as_deref(), Some("local-placeholder"));
        assert_eq!(config.limits.max_transcript_projection_items, 32);
        assert_eq!(config.limits.max_event_batch_items, 16);
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn parses_fs_store_runtime_process_config_when_feature_enabled() {
        let config = parse_args(["--fs-root", "/tmp/yoi-worker-runtime-store"])
            .unwrap()
            .unwrap();

        assert!(matches!(
            config.store,
            RuntimeHttpStoreSelection::Fs { ref root }
                if root == &PathBuf::from("/tmp/yoi-worker-runtime-store")
        ));
    }

    #[cfg(not(feature = "fs-store"))]
    #[test]
    fn rejects_fs_store_runtime_process_config_without_feature() {
        let error = parse_args(["--store", "fs", "--fs-root", "/tmp/store"]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("requires building worker-runtime with features")
        );
    }

    #[test]
    fn help_does_not_start_server() {
        assert!(parse_args(["--help"]).unwrap().is_none());
    }
}
