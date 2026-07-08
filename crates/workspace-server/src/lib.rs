//! Local workspace web control plane backend bootstrap.
//!
//! This crate deliberately provides backend building blocks and an HTTP router;
//! it is not the product CLI facade. Existing `.yoi` Ticket and Objective files
//! remain the canonical project records and are read through bounded bridge APIs.

pub mod companion;
pub mod config;
pub mod hosts;
pub mod identity;
pub mod observation;
pub mod records;
pub mod repositories;
pub mod resource_broker;
pub mod server;
pub mod store;

pub use config::{
    ConfigDiff, ResolvedWorkspaceBackendConfig, WORKSPACE_BACKEND_CONFIG_RELATIVE_PATH,
    WORKSPACE_BACKEND_CONFIG_TEMPLATE, WorkspaceBackendConfigFile,
};
pub use identity::{WORKSPACE_IDENTITY_RELATIVE_PATH, WorkspaceIdentity};
pub use records::{
    LocalProjectRecordReader, ObjectiveDetail, ObjectiveSummary, TicketDetail, TicketSummary,
};
pub use repositories::{
    ConfiguredRepository, GitCommitSummary, GitRemoteSummary, GitRepositorySummary,
    RepositoryLogRead, RepositoryRegistryReader, RepositorySummary,
};
pub use server::{AuthConfig, ServerConfig, WorkspaceApi, build_router, serve};
pub use store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};

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
    #[error("unknown worker `{worker_id}` in runtime `{runtime_id}`")]
    UnknownWorker {
        runtime_id: String,
        worker_id: String,
    },
    #[error("invalid runtime {kind} `{value}`")]
    InvalidRuntimeIdentifier { kind: String, value: String },
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
    #[error("workspace identity error: {0}")]
    WorkspaceIdentity(String),
    #[error("store error: {0}")]
    Store(String),
}
