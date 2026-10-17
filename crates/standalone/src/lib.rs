#![doc = include_str!("../README.md")]

pub mod host;
mod job_store;
pub mod jobs;
pub mod launch;
pub mod store;
pub mod subjektiv;
mod subjektiv_sessions;

pub use host::{
    StandaloneHost, StandaloneShutdownError, StandaloneStartupError,
    StandaloneSubjectDisconnectError,
};
pub use launch::{ResolvedStandaloneLaunch, StandaloneLaunchConfig, StandaloneLaunchError};
pub use protocol::WorkerId;
pub use store::{
    StaleLeasePolicy, StandaloneCwdIdentity, StandaloneListScope, StandaloneShutdownReason,
    StandaloneStoreError, StandaloneWorkerRecord, StandaloneWorkerStatus, StandaloneWorkerStore,
};
