#![doc = include_str!("../README.md")]

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
#[cfg(feature = "restore-checkpoints")]
pub mod restore_checkpoints;
#[cfg(test)]
#[path = "../../../tests/support/restore_environment.rs"]
mod restore_test_environment;
#[cfg(feature = "fs-store")]
pub mod retention;
mod runtime;
#[cfg(feature = "fs-store")]
mod session_material;
#[cfg(feature = "fs-store")]
mod session_public;
mod session_retention;
pub mod ssh_host_key_probe;
pub mod worker_backend;
pub mod worker_source;
pub mod working_directory;
pub mod workspace_issuer;
#[cfg(feature = "http-server")]
pub mod workspace_request;

#[cfg(feature = "test-support")]
pub use fs_store::test_support;
#[cfg(feature = "fs-store")]
pub use fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
pub use management::RuntimeOptions;
pub use runtime::{
    Runtime, RuntimeEventSelectorSubscription, RuntimeSubscriptionRecvError,
    RuntimeSubscriptionUpdate, RuntimeWorkspaceScope,
};
pub use session_store::UploadedFileUploadContext;
