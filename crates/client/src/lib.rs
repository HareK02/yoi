//! Backend Workspace/Runtime と既存 Worker protocol へ接続するクライアント。
//!
//! Standalone execution is owned by the `standalone` crate and does not spawn
//! a Worker subprocess through this crate.

pub mod backend_api;
mod backend_auth;
pub mod backend_runtime;
pub mod backend_workspace;
mod client;
pub mod target;
pub mod transport;
mod workspace_product;

pub use backend_api::{
    BackendApiClient, BackendApiClientError, BackendOrigin, backend_token_file_path,
    save_backend_token,
};
pub use backend_auth::{
    BackendAuthClientError, BackendAuthTarget, DeviceLoginPollResponse, DeviceLoginStartResponse,
    poll_device_login, start_device_login, wait_for_device_login,
};
pub use backend_runtime::{
    BackendDiagnostic, BackendDiagnosticSeverity, BackendRuntimeClientError,
    BackendRuntimeListResponse, BackendRuntimeListTarget, BackendRuntimeSummary,
    BackendRuntimeTarget, BackendWorkerCapabilitySummary, BackendWorkerImplementationSummary,
    BackendWorkerRestoreResponse, BackendWorkerRestoreResult, BackendWorkerSummary,
    BackendWorkerWorkspaceSummary, BackendWorkingDirectorySummary, connect_backend_runtime,
    list_backend_stopped_workers, list_backend_workers, restore_backend_worker,
};
pub use backend_workspace::{
    BackendWorkspace, BackendWorkspaceCatalogTarget, BackendWorkspaceClientError,
    CreateBackendWorkspaceRepository, CreateBackendWorkspaceRequest,
    CreateBackendWorkspaceResponse, create_backend_workspace, list_backend_workspaces,
};
pub use client::{Client, ClientError};
pub use target::{
    BackendTarget, Dashboard, ResolvedTarget, StandaloneSessionListIntent,
    StandaloneSessionResumeIntent, StandaloneTarget, Target, TargetError, TargetKind,
    WorkerConnection, WorkerConnectionSelector, WorkerList, WorkerListRequest, WorkerSpawn,
};
pub use workspace_api::{ObjectiveDetail, ObjectiveSummary};
pub use workspace_product::BackendWorkspaceProductClient;
