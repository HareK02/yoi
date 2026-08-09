//! Local workspace web control plane backend bootstrap.
//!
//! This crate deliberately provides backend building blocks and an HTTP router;
//! it is not the product CLI facade. Tickets and Objectives are served through
//! Backend authority surfaces rather than Worker-local filesystem access.

pub mod auth;
pub mod authority;
pub mod companion;
pub mod config;
pub mod hosts;
pub mod identity;
pub mod memory_backend;
pub mod memory_staging;
pub mod observation;
pub mod profile_settings;
pub mod records;
#[cfg(feature = "typescript")]
pub use records::ticket_api_typescript;
pub mod repositories;
pub mod resource_broker;
pub mod runtime_subscription;
pub mod server;
pub mod skills;
pub mod store;
mod workspace_subscription;

pub use authority::{
    MemoryAuthority, MemoryDocument, MemoryStagingEntry, MemoryStagingResolution,
    ObjectiveAuthority, SqliteWorkspaceAuthority, TicketAuthority, WorkspaceAuthority,
};
pub use config::{
    BackendRuntimesConfigFile, ConfigDiff, ResolvedWorkspaceBackendConfig,
    WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH, WORKSPACE_BACKEND_CONFIG_TEMPLATE,
    WorkspaceBackendConfigFile,
};
pub use identity::{WORKSPACE_IDENTITY_RELATIVE_PATH, WorkspaceIdentity};
pub use records::{ObjectiveDetail, ObjectiveSummary, TicketDetail, TicketSummary};
pub use repositories::{
    ConfiguredRepository, GitCommitSummary, GitRemoteSummary, GitRepositorySummary,
    RepositoryLogRead, RepositoryRegistryReader, RepositorySummary,
};
pub use server::{AuthConfig, ServerConfig, WorkspaceApi, build_router, serve};
pub use store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};

use worker_runtime::identity::RuntimeWorkerRef;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("ticket error: {0}")]
    Ticket(#[from] ticket::TicketError),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid project record id `{0}`")]
    InvalidRecordId(String),
    #[error("workspace backend config error: {0}")]
    Config(String),
    #[error("record `{0}` is missing frontmatter")]
    MissingFrontmatter(String),
    #[error("unknown local host `{0}`")]
    UnknownHost(String),
    #[error("unknown runtime `{0}`")]
    UnknownRuntime(String),
    #[error("unknown worker `{}` in runtime `{}`", worker.worker_id, worker.runtime_id)]
    UnknownWorker { worker: RuntimeWorkerRef },
    #[error("invalid runtime {kind} `{value}`")]
    InvalidRuntimeIdentifier { kind: String, value: String },
    #[error("worker name is reserved for a dedicated Workspace service: {0}")]
    ReservedWorkerName(String),
    #[error("runtime `{runtime_id}` operation failed ({code}): {message}")]
    RuntimeOperationFailed {
        runtime_id: String,
        code: String,
        message: String,
    },
    #[error("runtime `{runtime_id}` does not support `{capability}`")]
    RuntimeCapabilityUnsupported {
        runtime_id: String,
        capability: String,
    },
    #[error("unknown local repository `{0}`")]
    UnknownRepository(String),
    #[error("workspace id does not match this Workspace backend")]
    WorkspaceIdMismatch,
    #[error("Ticket assignment conflict: {0}")]
    TicketAssignmentConflict(String),
    #[error("Workdir attachment conflict: {0}")]
    WorkdirAttachmentConflict(String),
    #[error("Registry inconsistency: {0}")]
    RegistryInconsistency(String),
    #[error("Worker source identity is invalid: {0}")]
    WorkerSourceIdentity(String),
    #[error("workspace identity error: {0}")]
    WorkspaceIdentity(String),
    #[error("store error: {0}")]
    Store(String),
}
