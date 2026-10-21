#![doc = include_str!("../README.md")]

pub mod host;
mod job_store;
pub mod jobs;
pub mod launch;
pub mod store;
pub mod subjektiv;
mod subjektiv_sessions;
pub mod unix;

pub use unix::{UnixEndpoint, relay_unix_client, run_unix_client};

/// Restore reads the saved cwd and Profile, not a newly resolved launch recipe.
pub enum HeadlessLaunch {
    Fresh(ResolvedStandaloneLaunch),
    Restore {
        state_dir: std::path::PathBuf,
        worker_id: WorkerId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    #[error(transparent)]
    Transport(#[from] std::io::Error),
    #[error(transparent)]
    Startup(#[from] StandaloneStartupError),
    #[error(transparent)]
    Shutdown(#[from] StandaloneShutdownError),
}

/// Reserve the optional public transport before creating or restoring execution state.
pub async fn run_headless(
    launch: HeadlessLaunch,
    endpoint: std::path::PathBuf,
) -> Result<(), HeadlessError> {
    let endpoint = UnixEndpoint::bind(endpoint)?;
    let host = match launch {
        HeadlessLaunch::Fresh(launch) => StandaloneHost::start(launch).await?,
        HeadlessLaunch::Restore {
            state_dir,
            worker_id,
        } => StandaloneHost::restore(state_dir, worker_id).await?,
    };
    host.serve_unix(endpoint).await
}

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
