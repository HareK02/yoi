//! Typed Worker Runtime management HTTP contract.
//!
//! This crate owns the public REST DTOs and generated client/server adapters only. Runtime domain
//! state, authorization, body limits, and operation permission policy remain in their existing
//! authorities.

use std::collections::BTreeMap;

use api_macros::api;
pub use api_macros::axum as server_support;
pub use api_macros::reqwest as client_support;
pub use api_macros::{ApiContract, HttpMethod};
use protocol::{CompletionEntry, CompletionKind, Segment, WorkerId, WorkerStateSnapshot};
use serde::{Deserialize, Serialize};
use workdir::workspace::{MaterializerKind, RuntimeWorkerRef, RuntimeWorkingDirectorySummary};
use workspace_api::{RepositoryAccessMode, RepositorySource};

pub const RUNTIME_API_VERSION: &str = "v1";
pub const RUNTIME_API_BASE_PATH: &str = "/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeApiError {
    pub error: RuntimeApiErrorDetail,
    #[serde(skip)]
    status: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeApiErrorDetail {
    pub code: String,
    pub message: String,
}

impl RuntimeApiError {
    pub fn new(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: RuntimeApiErrorDetail {
                code: code.into(),
                message: message.into(),
            },
            status,
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }
}

impl api_macros::HttpError for RuntimeApiError {
    fn status_code(&self) -> u16 {
        self.status
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePingResponse {
    pub runtime_id: String,
    pub protocol_version: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendKind {
    Memory,
    FsStore,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Running,
    Stopped,
}

fn unknown_platform_component() -> String {
    "unknown".to_string()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSummary {
    pub display_name: Option<String>,
    pub backend: RuntimeBackendKind,
    pub status: RuntimeStatus,
    pub worker_count: usize,
    pub active_worker_count: usize,
    pub stopped_worker_count: usize,
    pub diagnostic_count: usize,
    #[serde(default = "unknown_platform_component")]
    pub os: String,
    #[serde(default = "unknown_platform_component")]
    pub arch: String,
    #[serde(default)]
    pub worker_creation_available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSummaryResponse {
    pub runtime: RuntimeSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct WorkerRef {
    pub worker_id: WorkerId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceApiRef {
    pub workspace_id: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerWorkspaceApiRequest {
    pub workspace_api: WorkspaceApiRef,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Idle,
    Running,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProfileSelector {
    Builtin(String),
    Named(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileSourceGraphSummary {
    pub source_count: usize,
    pub total_source_bytes: u64,
    pub entrypoints: BTreeMap<String, String>,
    pub import_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileSourceArchiveRef {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub source_graph: ProfileSourceGraphSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileSourceArchive {
    pub reference: ProfileSourceArchiveRef,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSourceArchiveSource {
    Embedded { archive: ProfileSourceArchive },
    WorkspaceConfig { archive: ProfileSourceArchiveRef },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfigBundleRef {
    pub id: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySelector(pub String);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryRepository {
    pub id: String,
    pub provider: String,
    pub source: RepositorySource,
    pub source_revision: u64,
    pub source_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<RepositorySelector>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceKind {
    ProfileSourceArchive,
    RepositorySshAccess,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceOperation {
    FetchArchive,
    FetchOnce,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRedactionPolicy {
    RuntimeInternalOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendResourceHandle {
    pub kind: BackendResourceKind,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub resource_id: String,
    pub digest: String,
    pub operation: BackendResourceOperation,
    pub expires_at_unix_seconds: i64,
    pub nonce: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub max_bytes: u64,
    pub content_type: String,
    pub redaction: ResourceRedactionPolicy,
    pub audit_correlation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_source_graph: Option<ProfileSourceGraphSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySshCredentialCandidate {
    pub credential_id: String,
    pub credential_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySshMaterializationAccess {
    pub credential_candidates: Vec<RepositorySshCredentialCandidate>,
    pub host_trust_id: String,
    pub host_trust_revision: u64,
    pub access: RepositoryAccessMode,
    pub expires_at_epoch_seconds: u64,
    pub repository_id: String,
    pub repository_source_fingerprint: String,
    pub repository_uri: String,
    pub secret_resource: BackendResourceHandle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryMaterializationContext {
    pub workspace_id: String,
    pub runtime_id: String,
    pub operation_id: String,
    pub config_revision: u64,
    pub config_projection_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<RepositorySshMaterializationAccess>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryRequest {
    pub repository: WorkingDirectoryRepository,
    #[serde(default)]
    pub materializer: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_workdir_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<RepositoryMaterializationContext>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryClaim {
    pub working_directory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_cwd: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryStatus {
    pub summary: RuntimeWorkingDirectorySummary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    Notify,
    Compact,
    ListRewindTargets,
    RegisterPeer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerInput {
    pub kind: WorkerInputKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateWorkerRequest {
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<WorkerInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_request: Option<WorkingDirectoryRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryClaim>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worker_observation_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_observation_grants: Vec<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_api: Option<WorkspaceApiRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_settings: Option<manifest::WorkspaceMemorySettingsSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerSummary {
    pub worker_ref: WorkerRef,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    pub execution_metadata_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_state: Option<WorkerStateSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryStatus>,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
}

pub type WorkerDetail = WorkerSummary;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkersResponse {
    pub workers: Vec<WorkerSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerResponse {
    pub worker: WorkerDetail,
}

/// Explicit restore outcome returned for every reachable Runtime restore attempt,
/// including preflight rejection and uncertain post-side-effect reconciliation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRestoreResponse {
    pub state: workspace_api::WorkerRestoreState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatusFilter {
    Stopped,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerListQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkerStatusFilter>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerDeleteResult {
    pub worker_id: WorkerId,
    pub deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerDeleteResponse {
    pub worker: WorkerDeleteResult,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerLifecycleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyObjectRequest {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerLifecycleAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_state: Option<WorkerStateSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerLifecycleResponse {
    pub ack: WorkerLifecycleAck,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerSubmissionAck {
    pub submission_request_id: String,
    pub submission_id: String,
    pub disposition: protocol::SubmissionDisposition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerInteractionAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission: Option<WorkerSubmissionAck>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerInputResponse {
    pub ack: WorkerInteractionAck,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionRequest {
    pub kind: CompletionKind,
    #[serde(default)]
    pub prefix: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionResponse {
    pub kind: CompletionKind,
    pub prefix: String,
    pub entries: Vec<CompletionEntry>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDisposition {
    Archive,
    Purge,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsDisposition {
    Purge,
    Retain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRetentionInventory {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: WorkerId,
    pub session_id: Option<String>,
    pub segment_ids: Vec<String>,
    pub session_bytes: u64,
    pub diagnostics_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRetentionExecutionRequest {
    pub operation_id: String,
    pub input_fingerprint: String,
    pub archive_id: Option<String>,
    pub workspace_id: String,
    pub source_runtime_id: String,
    pub worker_id: WorkerId,
    pub expected_worker_revision: String,
    pub source_created_at: String,
    pub removed_at: String,
    pub effective_profile: Option<String>,
    pub retention_class: Option<String>,
    pub policy_id: String,
    pub policy_revision: u64,
    pub session_disposition: SessionDisposition,
    pub diagnostics_disposition: DiagnosticsDisposition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerSessionArchiveManifest {
    pub schema_version: u32,
    pub archive_id: String,
    pub workspace_id: String,
    pub source_runtime_id: String,
    pub source_worker_id: WorkerId,
    pub source_session_id: String,
    pub segment_ids: Vec<String>,
    pub source_created_at: String,
    pub removed_at: String,
    pub archived_at_unix_seconds: u64,
    pub effective_profile: Option<String>,
    pub retention_class: Option<String>,
    pub content_checksum_sha256: String,
    pub content_bytes: u64,
    pub content_file_count: u64,
    pub policy_id: String,
    pub policy_revision: u64,
    pub operation_id: String,
    pub input_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRetentionExecutionResult {
    pub operation_id: String,
    pub input_fingerprint: String,
    pub expected_worker_revision: String,
    pub worker_id: WorkerId,
    pub session_disposition: SessionDisposition,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub archive: Option<WorkerSessionArchiveManifest>,
    pub source_removed: bool,
    pub diagnostics_retained: bool,
}

#[api(reqwest, axum)]
pub trait RuntimeApi {
    #[get("/v1/ping", status = 200, error_status = 400)]
    async fn ping(
        &self,
        #[header("x-yoi-workspace-id")] workspace_id: String,
    ) -> Result<RuntimePingResponse, RuntimeApiError>;

    #[get("/v1/runtime", status = 200, error_status = 400)]
    async fn runtime_summary(&self) -> Result<RuntimeSummaryResponse, RuntimeApiError>;

    #[get("/v1/workers", status = 200, error_status = 400)]
    async fn list_workers(
        &self,
        #[query] query: WorkerListQuery,
    ) -> Result<WorkersResponse, RuntimeApiError>;

    #[get("/v1/workers/{worker_id}", status = 200, error_status = 400)]
    async fn get_worker(
        &self,
        #[path] worker_id: String,
    ) -> Result<WorkerResponse, RuntimeApiError>;

    #[post("/v1/workers", status = 200, error_status = 400)]
    async fn create_worker(
        &self,
        #[body] request: CreateWorkerRequest,
    ) -> Result<WorkerResponse, RuntimeApiError>;

    #[delete("/v1/workers/{worker_id}", status = 200, error_status = 400)]
    async fn delete_worker(
        &self,
        #[path] worker_id: String,
    ) -> Result<WorkerDeleteResponse, RuntimeApiError>;

    #[post("/v1/workers/{worker_id}/input", status = 200, error_status = 400)]
    async fn send_worker_input(
        &self,
        #[path] worker_id: String,
        #[body] input: WorkerInput,
    ) -> Result<WorkerInputResponse, RuntimeApiError>;

    #[post("/v1/workers/{worker_id}/stop", status = 200, error_status = 400)]
    async fn stop_worker(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResponse, RuntimeApiError>;

    #[post("/v1/workers/{worker_id}/cancel", status = 200, error_status = 400)]
    async fn cancel_worker(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResponse, RuntimeApiError>;

    #[post("/v1/workers/{worker_id}/restore", status = 200, error_status = 400)]
    async fn restore_worker(
        &self,
        #[path] worker_id: String,
        #[body] request: EmptyObjectRequest,
    ) -> Result<WorkerRestoreResponse, RuntimeApiError>;

    #[post(
        "/v1/workers/{worker_id}/workspace-api",
        status = 200,
        error_status = 400
    )]
    async fn replace_worker_workspace_api(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerWorkspaceApiRequest,
    ) -> Result<WorkerResponse, RuntimeApiError>;

    #[post(
        "/v1/workers/{worker_id}/completions",
        status = 200,
        error_status = 400
    )]
    async fn complete_worker_arguments(
        &self,
        #[path] worker_id: String,
        #[body] request: CompletionRequest,
    ) -> Result<CompletionResponse, RuntimeApiError>;

    #[get(
        "/v1/workers/{worker_id}/retention/inventory",
        status = 200,
        error_status = 400
    )]
    async fn retention_inventory(
        &self,
        #[path] worker_id: String,
    ) -> Result<WorkerRetentionInventory, RuntimeApiError>;

    #[post(
        "/v1/workers/{worker_id}/retention/execute",
        status = 200,
        error_status = 400
    )]
    async fn execute_retention(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, RuntimeApiError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemainingRuntimeRoute {
    pub method: &'static str,
    pub path: &'static str,
    pub reason: &'static str,
}

pub const RUNTIME_ROUTE_VERIFICATION_CHALLENGE: &str =
    "/v1/workspace-runtime-verification/challenge";
pub const RUNTIME_ROUTE_VERIFICATION_ACK: &str =
    "/v1/workspace-runtime-verification/acknowledgement";
pub const RUNTIME_ROUTE_CONFIG_BUNDLES: &str = "/v1/config-bundles";
pub const RUNTIME_ROUTE_CONFIG_BUNDLE_AVAILABILITY: &str =
    "/v1/config-bundles/{bundle_id}/availability";
pub const RUNTIME_ROUTE_WORKSPACE_PROMPT_PROJECTIONS: &str = "/v1/workspace-prompt-projections";
pub const RUNTIME_ROUTE_WORKING_DIRECTORIES: &str = "/v1/working-directories";
pub const RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS: &str =
    "/v1/working-directories/repository-access";
pub const RUNTIME_ROUTE_SSH_PROBE: &str = "/v1/repositories/ssh/probe";
pub const RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE: &str = "/v1/repository-refs/observe";
pub const RUNTIME_ROUTE_WORKDIR_SESSIONS: &str =
    "/v1/working-directories/{working_directory_id}/sessions";
pub const RUNTIME_ROUTE_WORKDIR_SESSION_OPERATIONS: &str =
    "/v1/workdir-sessions/{session_id}/operations";
pub const RUNTIME_ROUTE_WORKDIR_SESSION: &str = "/v1/workdir-sessions/{session_id}";
pub const RUNTIME_ROUTE_WORKING_DIRECTORY: &str = "/v1/working-directories/{working_directory_id}";
pub const RUNTIME_ROUTE_PROTOCOL_WS: &str = "/v1/protocol/ws";
pub const RUNTIME_ROUTE_WORKER_PROTOCOL_WS: &str = "/v1/workers/{worker_id}/protocol/ws";
pub const RUNTIME_ROUTE_WORKER_ATTACHMENTS: &str = "/v1/workers/{worker_id}/attachments";
pub const RUNTIME_ROUTE_WORKER_ATTACHMENT: &str =
    "/v1/workers/{worker_id}/attachments/{artifact_id}";

/// Intentionally out-of-contract Runtime routes. This inventory keeps manual paths visible rather
/// than letting them be mistaken for management-contract omissions.
pub const REMAINING_RUNTIME_ROUTES: &[RemainingRuntimeRoute] = &[
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_VERIFICATION_CHALLENGE,
        reason: "Workspace verification handshake",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_VERIFICATION_ACK,
        reason: "Workspace verification handshake",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_CONFIG_BUNDLES,
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_CONFIG_BUNDLES,
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_CONFIG_BUNDLE_AVAILABILITY,
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKSPACE_PROMPT_PROJECTIONS,
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_WORKING_DIRECTORIES,
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKING_DIRECTORIES,
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS,
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_SSH_PROBE,
        reason: "Repository SSH transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE,
        reason: "Repository observation transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKDIR_SESSIONS,
        reason: "Workdir session transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKDIR_SESSION_OPERATIONS,
        reason: "Workdir session transport",
    },
    RemainingRuntimeRoute {
        method: "DELETE",
        path: RUNTIME_ROUTE_WORKDIR_SESSION,
        reason: "Workdir session transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_WORKING_DIRECTORY,
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "DELETE",
        path: RUNTIME_ROUTE_WORKING_DIRECTORY,
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_PROTOCOL_WS,
        reason: "WebSocket protocol transport (ws-server feature)",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_WORKER_PROTOCOL_WS,
        reason: "WebSocket protocol transport (ws-server feature)",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: RUNTIME_ROUTE_WORKER_ATTACHMENTS,
        reason: "Binary attachment transport",
    },
    RemainingRuntimeRoute {
        method: "DELETE",
        path: RUNTIME_ROUTE_WORKER_ATTACHMENT,
        reason: "Binary attachment transport",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct RoundTripService;

    fn test_error(status: u16) -> RuntimeApiError {
        RuntimeApiError::new(status, "test_error", "test error")
    }

    impl RuntimeApi for RoundTripService {
        async fn ping(
            &self,
            _workspace_id: String,
        ) -> Result<RuntimePingResponse, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn runtime_summary(&self) -> Result<RuntimeSummaryResponse, RuntimeApiError> {
            Ok(RuntimeSummaryResponse {
                runtime: RuntimeSummary {
                    display_name: Some("round-trip".to_string()),
                    backend: RuntimeBackendKind::Memory,
                    status: RuntimeStatus::Running,
                    worker_count: 0,
                    active_worker_count: 0,
                    stopped_worker_count: 0,
                    diagnostic_count: 0,
                    os: "test".to_string(),
                    arch: "test".to_string(),
                    worker_creation_available: true,
                },
            })
        }

        async fn list_workers(
            &self,
            query: WorkerListQuery,
        ) -> Result<WorkersResponse, RuntimeApiError> {
            assert_eq!(query.status, Some(WorkerStatusFilter::Stopped));
            Ok(WorkersResponse {
                workers: Vec::new(),
            })
        }

        async fn get_worker(&self, worker_id: String) -> Result<WorkerResponse, RuntimeApiError> {
            assert_eq!(worker_id, "missing worker");
            Err(test_error(404))
        }

        async fn create_worker(
            &self,
            _request: CreateWorkerRequest,
        ) -> Result<WorkerResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn delete_worker(
            &self,
            _worker_id: String,
        ) -> Result<WorkerDeleteResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn send_worker_input(
            &self,
            _worker_id: String,
            _input: WorkerInput,
        ) -> Result<WorkerInputResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn stop_worker(
            &self,
            _worker_id: String,
            _request: WorkerLifecycleRequest,
        ) -> Result<WorkerLifecycleResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn cancel_worker(
            &self,
            _worker_id: String,
            _request: WorkerLifecycleRequest,
        ) -> Result<WorkerLifecycleResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn restore_worker(
            &self,
            _worker_id: String,
            _request: EmptyObjectRequest,
        ) -> Result<WorkerRestoreResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn replace_worker_workspace_api(
            &self,
            _worker_id: String,
            _request: WorkerWorkspaceApiRequest,
        ) -> Result<WorkerResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn complete_worker_arguments(
            &self,
            _worker_id: String,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn retention_inventory(
            &self,
            _worker_id: String,
        ) -> Result<WorkerRetentionInventory, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn execute_retention(
            &self,
            _worker_id: String,
            _request: WorkerRetentionExecutionRequest,
        ) -> Result<WorkerRetentionExecutionResult, RuntimeApiError> {
            Err(test_error(501))
        }
    }

    #[tokio::test]
    async fn generated_client_and_router_round_trip_paths_queries_and_errors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            server_support::framework::serve(listener, RuntimeApiAxum::router(RoundTripService))
                .await
                .unwrap();
        });
        let client = RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .build()
            .unwrap();

        let summary = client.runtime_summary().await.unwrap();
        assert_eq!(summary.runtime.display_name.as_deref(), Some("round-trip"));
        let workers = client
            .list_workers(WorkerListQuery {
                status: Some(WorkerStatusFilter::Stopped),
            })
            .await
            .unwrap();
        assert!(workers.workers.is_empty());
        let error = client
            .get_worker("missing worker".to_string())
            .await
            .unwrap_err();
        assert!(
            matches!(error, client_support::ClientError::Public { status, .. } if status.as_u16() == 404)
        );
        server.abort();
    }

    #[test]
    fn contract_inventory_is_complete_and_unique() {
        let operations = RuntimeApiMetadata::OPERATIONS;
        assert_eq!(operations.len(), 14);
        let mut routes = operations
            .iter()
            .map(|operation| (format!("{:?}", operation.method), operation.path))
            .collect::<Vec<_>>();
        routes.sort_unstable();
        routes.dedup();
        assert_eq!(routes.len(), operations.len());
        assert!(
            operations
                .iter()
                .all(|operation| operation.path.starts_with(RUNTIME_API_BASE_PATH))
        );
    }

    #[test]
    fn remaining_route_inventory_does_not_overlap_contract() {
        assert_eq!(REMAINING_RUNTIME_ROUTES.len(), 20);
        for remaining in REMAINING_RUNTIME_ROUTES {
            assert!(!RuntimeApiMetadata::OPERATIONS.iter().any(|operation| {
                format!("{:?}", operation.method).eq_ignore_ascii_case(remaining.method)
                    && operation.path == remaining.path
            }));
        }
    }
}
