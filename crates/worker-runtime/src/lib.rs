//! Embedded Runtime domain API for Worker management.
//!
//! `worker-runtime` keeps its core independent from HTTP/WebSocket servers,
//! provider execution, and the existing Worker host.  Filesystem persistence is
//! available only through the optional `fs-store` feature, and the minimal REST
//! process adapter is available only through the optional `http-server` feature.
//! The crate defines the in-process Runtime authority surface that higher layers
//! can later adapt into registries or backend APIs.

pub mod catalog;
pub mod config_bundle;
pub mod diagnostics;
pub mod error;
#[cfg(feature = "fs-store")]
pub mod fs_store;
#[cfg(feature = "http-server")]
pub mod http_server;
pub mod identity;
pub mod interaction;
pub mod management;
pub mod observation;
mod runtime;

#[cfg(feature = "fs-store")]
pub use fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
pub use runtime::Runtime;
