//! Embedded Runtime domain API for Worker management.
//!
//! `worker-runtime` intentionally stays independent from HTTP/WebSocket servers,
//! provider execution, and the existing Worker host.  Filesystem persistence is
//! available only through the optional `fs-store` feature.  The crate defines the
//! in-process Runtime authority surface that higher layers can later adapt into
//! registries or web APIs.

pub mod catalog;
pub mod diagnostics;
pub mod error;
#[cfg(feature = "fs-store")]
pub mod fs_store;
pub mod identity;
pub mod interaction;
pub mod management;
pub mod observation;
mod runtime;

#[cfg(feature = "fs-store")]
pub use fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
pub use runtime::Runtime;
