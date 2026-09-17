//! Typed Worker Runtime management HTTP contract.
//!
//! This crate owns the public REST DTOs and generated client/server adapters only. Runtime domain
//! state, authorization, body limits, and operation permission policy remain in their existing
//! authorities.

use std::collections::BTreeMap;

use api_macros::api;
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
pub struct BackendResourceHandle {
    pub resource_id: String,
    pub resource_kind: String,
    pub resource_version: u64,
    pub resource_fingerprint: String,
    pub secret_handle: String,
    pub expires_at_epoch_seconds: Option<u64>,
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

/// Intentionally out-of-contract Runtime routes. This inventory keeps manual paths visible rather
/// than letting them be mistaken for management-contract omissions.
pub const REMAINING_RUNTIME_ROUTES: &[RemainingRuntimeRoute] = &[
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/bootstrap",
        reason: "bootstrap lifecycle",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/workers/{worker_id}/workspace-api",
        reason: "Workspace API rebinding",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: "/v1/protocol/ws",
        reason: "WebSocket protocol transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: "/v1/workers/{worker_id}/protocol/ws",
        reason: "WebSocket protocol transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/workers/{worker_id}/attachments",
        reason: "binary attachment transport",
    },
    RemainingRuntimeRoute {
        method: "DELETE",
        path: "/v1/workers/{worker_id}/attachments/{artifact_id}",
        reason: "binary attachment transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: "/v1/config-bundles",
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/config-bundles",
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: "/v1/config-bundles/{bundle_id}/availability",
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/workspace-prompt-projections",
        reason: "Workspace Config transport",
    },
    RemainingRuntimeRoute {
        method: "GET",
        path: "/v1/working-directories",
        reason: "Workdir transport",
    },
    RemainingRuntimeRoute {
        method: "POST",
        path: "/v1/working-directories",
        reason: "Workdir transport",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_inventory_is_complete_and_unique() {
        let operations = RuntimeApiMetadata::OPERATIONS;
        assert_eq!(operations.len(), 13);
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
        for remaining in REMAINING_RUNTIME_ROUTES {
            assert!(!RuntimeApiMetadata::OPERATIONS.iter().any(|operation| {
                format!("{:?}", operation.method).eq_ignore_ascii_case(remaining.method)
                    && operation.path == remaining.path
            }));
        }
    }
}
