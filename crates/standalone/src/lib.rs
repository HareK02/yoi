//! In-process standalone host for one top-level Yoi Worker.
//!
//! The crate composes existing `worker`, `manifest`, `session-store`, and
//! `workdir` contracts. It intentionally owns no TUI, Runtime, Workspace
//! Server, HTTP, WebSocket, subprocess Worker, or alternative execution path.

pub mod host;
pub mod launch;
pub mod store;

pub use host::{StandaloneHost, StandaloneShutdownError, StandaloneStartupError};
pub use launch::{ResolvedStandaloneLaunch, StandaloneLaunchConfig, StandaloneLaunchError};
pub use protocol::WorkerId;
pub use store::{
    StaleLeasePolicy, StandaloneCwdIdentity, StandaloneListScope, StandaloneShutdownReason,
    StandaloneStoreError, StandaloneWorkerRecord, StandaloneWorkerStatus, StandaloneWorkerStore,
};
