//! Embedded Runtime domain API for Worker management.
//!
//! `worker-runtime` owns the Runtime authority surface and, for the standalone
//! process, wires that Runtime to the real Worker host. Filesystem persistence is
//! available only through the optional `fs-store` feature, and the REST/WebSocket
//! server is available only through the optional `http-server` / `ws-server`
//! features.

pub mod auth;
pub mod catalog;
pub mod config_bundle;
pub mod diagnostics;
pub mod error;
pub mod execution;
#[cfg(feature = "fs-store")]
pub mod fs_store;
#[cfg(feature = "http-server")]
pub mod http_server;
pub mod identity;
pub mod interaction;
pub mod management;
pub mod observation;
pub mod profile_archive;
pub mod resource;
#[cfg(feature = "fs-store")]
pub mod retention;
mod runtime;
pub mod worker_backend;
pub mod worker_source;
pub mod working_directory;

#[cfg(feature = "fs-store")]
pub use fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
pub use management::RuntimeOptions;
pub use runtime::{Runtime, RuntimeWorkspaceScope};
pub use session_store::UploadedFileUploadContext;
