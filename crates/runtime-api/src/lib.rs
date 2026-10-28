#![doc = include_str!("../README.md")]

use std::collections::BTreeMap;

use api_macros::api;
pub use api_macros::axum as server_support;
pub use api_macros::reqwest as client_support;
pub use api_macros::{ApiContract, HttpMethod};
use protocol::{CompletionEntry, CompletionKind, Segment, WorkerId, WorkerStateSnapshot};
use serde::{Deserialize, Serialize};
use workdir::workspace::{MaterializerKind, RuntimeWorkerRef, RuntimeWorkingDirectorySummary};

pub const RUNTIME_API_VERSION: &str = "v1";
pub const RUNTIME_API_BASE_PATH: &str = "/v1";
/// Runtime transport classification for a materialized Repository source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySourceKind {
    LocalPath,
    File,
    Ssh,
    Https,
    Invalid,
}

impl RepositorySourceKind {
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Ssh | Self::Https)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local_path",
            Self::File => "file",
            Self::Ssh => "ssh",
            Self::Https => "https",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local_path" => Self::LocalPath,
            "file" => Self::File,
            "ssh" => Self::Ssh,
            "https" => Self::Https,
            "http" | "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

impl<'de> Deserialize<'de> for RepositorySourceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::unknown_variant(
                &value,
                &["local_path", "file", "ssh", "https", "invalid"],
            )
        })
    }
}

/// Runtime transport identity for one Repository source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySource {
    pub kind: RepositorySourceKind,
    pub uri: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessMode {
    ReadOnly,
    ReadWrite,
}

/// Runtime transport outcome for an explicit Worker restore operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRestoreState {
    Accepted,
    Rejected,
    RolledBack,
    /// A later Stop superseded this restore; neither rollback nor prior Accepted is asserted.
    Superseded,
    ReconciliationRequired,
}

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
    pub public_key_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySshMaterializationAccess {
    pub credential_candidates: Vec<RepositorySshCredentialCandidate>,
    pub host_trust_id: String,
    pub host_key_fingerprint: String,
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
    pub config_projection_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<RepositorySshMaterializationAccess>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryRequest {
    pub repository: WorkingDirectoryRepository,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub materializer: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_workdir_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<RepositoryMaterializationContext>,
}

/// Workspace-defined alias-to-Workdir mapping, independent of Runtime materialized bindings.
///
/// Logical aliases and session capabilities are stored as Workspace attachments; Runtime resolves
/// host paths and provider sessions. Workdir write scope alone does not grant command execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogicalWorkdirAttachment {
    /// Stable Worker-local routing key. This is not a Workdir id or display name.
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory_id: String,
    #[serde(default = "default_workdir_session_capabilities")]
    pub capabilities: workdir::WorkdirSessionCapabilities,
}

fn default_workdir_session_capabilities() -> workdir::WorkdirSessionCapabilities {
    // Capability-less wire and persisted records must not silently regain write
    // or command authority when read by a newer Runtime.
    workdir::WorkdirSessionCapabilities::READ_ONLY
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryAttachmentClaim {
    /// Stable Worker-local routing key. This is not a Workdir id or display name.
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_cwd: Option<String>,
    /// Backend-authoritative maximum capabilities for the attachment session.
    #[serde(default = "default_workdir_session_capabilities")]
    pub capabilities: workdir::WorkdirSessionCapabilities,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerWorkdirAttachmentsRequest {
    /// Exact current logical attachment set. Runtime-local bindings are not
    /// inferred from these identities.
    pub logical_workdir_attachments: Vec<LogicalWorkdirAttachment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryAttachmentRequest {
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory: WorkingDirectoryRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryAttachmentStatus {
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory: WorkingDirectoryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkingDirectoryStatus {
    pub summary: RuntimeWorkingDirectorySummary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    /// Agent input accepted only while Idle; never enters the human Submit queue.
    UserIfIdle,
    Notify,
    Compact,
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

/// Job/attempt identity fixed by the Host at creation or Restore.
///
/// Profile, instruction, and display name do not imply Job authority. The Backend accepts structured
/// results by matching the submitting Worker and this binding's current attempt and input digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendJobExecutionBinding {
    pub job_id: String,
    pub attempt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_digest: Option<String>,
    #[serde(default)]
    pub subjektiv_consolidation: bool,
    #[serde(default)]
    pub subjektiv_journal: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachment_requests: Vec<WorkingDirectoryAttachmentRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkingDirectoryAttachmentClaim>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worker_observation_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_observation_grants: Vec<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_api: Option<WorkspaceApiRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_settings: Option<manifest::WorkspaceMemorySettingsSnapshot>,
    /// Backend-attested subject or consolidation attachment. Profile policy alone
    /// never activates subjektiv for an ordinary Workspace Worker.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subjektiv_attached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_job: Option<BackendJobExecutionBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_observation_token: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
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

/// Restore intent with a fixed observation token and request ID.
///
/// Replay and recovery use the same pair; Runtime owns the durable operation and its settlement.
/// preparation is created by the Workspace Backend, not chosen as browser/Tool input.
/// Preparation side effects must not run before Runtime admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRestoreRequest {
    pub expected_observation_token: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<WorkerRestorePreparation>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRestorePreparation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_api: Option<WorkspaceApiRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir_attachments: Option<Vec<LogicalWorkdirAttachment>>,
    /// Retained in saved Restore intents; Restore does not apply Repository access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_access: Vec<WorkingDirectoryRepositoryAccessRequest>,
    /// Retained references from saved resource preparation, not Restore prerequisites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_access_workdirs: Vec<RepositoryAccessWorkdirReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessWorkdirReference {
    pub runtime_id: String,
    pub working_directory_id: String,
}

/// Request tying Restore admission and Workspace preparation to the same Runtime operation.
///
/// Queries without preparation only recover an existing owner/result; they admit no new intent.
/// Prepared requests verify and bind the observation token to the owner, retaining the original
/// preparation on replay.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRestoreCoordinationRequest {
    pub expected_observation_token: String,
    pub request_id: String,
    /// None means owner/result recovery only; it can never admit a new intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<WorkerRestorePreparation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRestoreCoordinationResponse {
    pub result: Option<WorkerRestoreResponse>,
    /// Original read-only preparation, only when the existing operation still needs it.
    pub preparation: Option<WorkerRestorePreparation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRepositoryAccessRequest {
    pub working_directory_id: String,
    pub materialization: RepositoryMaterializationContext,
}

/// Explicit result of a Restore attempt that could be reached.
///
/// Distinguishes preflight rejection from outcomes requiring state checks after side effects.
/// HTTP response loss does not imply rollback; check the Runtime-owned receipt/reconciliation
/// using the same Restore intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRestoreResponse {
    pub state: WorkerRestoreState,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<protocol::CompletionContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionResponse {
    pub kind: CompletionKind,
    pub prefix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<protocol::CompletionContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
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
    pub source_created_at: String,
    pub removed_at: String,
    pub effective_profile: Option<String>,
    pub retention_class: Option<String>,
    pub policy_id: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_active_segment_id: Option<String>,
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
    pub operation_id: String,
    pub input_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerRetentionExecutionResult {
    pub operation_id: String,
    pub input_fingerprint: String,
    pub worker_id: WorkerId,
    pub session_disposition: SessionDisposition,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub archive: Option<WorkerSessionArchiveManifest>,
    pub source_removed: bool,
    pub diagnostics_retained: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSessionRequest {
    pub workspace_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSessionHistoryRequest {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerSessionHistoryUnavailableReason {
    RetentionMissing,
    ActivePointerMissing,
    CorruptLog,
    MigrationRequired,
    RetentionExpired,
    StorageUnavailable,
    InvalidCursor,
    ResourceLimit,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum WorkerSessionHistoryAvailability {
    Page {
        page: protocol::SessionHistoryPage,
    },
    Unavailable {
        reason: WorkerSessionHistoryUnavailableReason,
        message: String,
    },
}

/// Backend-selected storage source. This type is never exposed as model tool
/// input; Runtime paths remain behind the authenticated Workspace capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum SessionPublicSource {
    Retained { worker_id: WorkerId },
    Archived { archive_id: String },
}

/// Canonical Session metadata, distinct from Memory's evidence/public-index projection.
/// `origin_json` is the exact serialized saved origin; it is not model-authored evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMaterialDescriptor {
    pub segment_id: String,
    pub entry_ref: String,
    pub canonical_order: u64,
    pub origin_json: String,
    pub sha256: String,
    pub total_bytes: u64,
}

/// Explicit Host erasure, distinct from ordinary retention policy or pin release.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMaterialEraseRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
}

/// Acknowledges the permanent fence and physical erasure of Session originals,
/// including archived diagnostics bound to the same removal operations. Historical
/// retention receipts remain immutable; their old retained flags are not current availability.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMaterialEraseResponse {
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMaterialPinRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    pub pin_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMaterialPinResponse {
    pub session_id: String,
    pub pin_id: String,
}

/// One body-bearing immutable slice. The byte budget includes slice metadata,
/// but not the enclosing availability enum or the caller's durable Job envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecordingCaptureRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    pub start: session_store::capture::RecordingPosition,
    pub max_entries: usize,
    pub max_serialized_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SessionRecordingCaptureAvailability {
    NotMaterialized {
        session_id: String,
    },
    Captured {
        slice: session_store::capture::RecordingCaptureSlice,
    },
    Unavailable {
        reason: SessionPublicUnavailableReason,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMaterialCatalogRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    pub expected_catalog_sha256: Option<String>,
    pub offset: u64,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMaterialCatalogPage {
    pub session_id: String,
    pub active_segment_id: String,
    pub catalog_sha256: String,
    pub descriptors: Vec<SessionMaterialDescriptor>,
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMaterialReadRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    /// Compared with a freshly resolved saved descriptor, never adopted as evidence.
    pub expected_descriptor: SessionMaterialDescriptor,
    pub byte_offset: u64,
    pub max_content_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMaterialReadPage {
    pub descriptor: SessionMaterialDescriptor,
    pub content: String,
    pub next_offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SessionMaterialCatalogAvailability {
    /// The saved active pointer is pending and storage proves no Session material
    /// has ever been published. Missing/corrupt materialized sources are not empty.
    NotMaterialized {
        session_id: String,
    },
    Page {
        page: SessionMaterialCatalogPage,
    },
    Unavailable {
        reason: SessionPublicUnavailableReason,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SessionMaterialReadAvailability {
    Page {
        page: SessionMaterialReadPage,
    },
    Unavailable {
        reason: SessionPublicUnavailableReason,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicEntryKind {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicToolPart {
    Input,
    Output,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicReadMode {
    Compact,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicLineageKind {
    Root,
    Fork,
    Compact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicLineage {
    pub kind: SessionPublicLineageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicSearchRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SessionPublicEntryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub tool_part: SessionPublicToolPart,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_cursor: Option<String>,
    pub limit: usize,
    pub max_scan_bytes: u64,
    pub max_segments: usize,
    pub max_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicReadRequest {
    pub workspace_id: String,
    pub source: SessionPublicSource,
    pub expected_session_id: String,
    pub expected_generation: Option<String>,
    pub segment_id: String,
    pub entry_ref: String,
    pub mode: SessionPublicReadMode,
    pub byte_offset: usize,
    pub max_content_bytes: usize,
    pub max_scan_bytes: u64,
    pub max_segments: usize,
    pub max_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicSearchItem {
    pub segment_id: String,
    pub entry_ref: String,
    pub kind: SessionPublicEntryKind,
    pub origin: protocol::SessionEntryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_part: Option<SessionPublicToolPart>,
    pub compact: String,
    pub compact_truncated: bool,
    pub lineage: SessionPublicLineage,
    /// Opaque Runtime continuation immediately before this result.
    pub scan_cursor: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicSearchPage {
    pub session_id: String,
    pub generation: String,
    pub items: Vec<SessionPublicSearchItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_scan_cursor: Option<String>,
    pub has_more: bool,
    pub scanned_bytes: u64,
    pub scanned_segments: usize,
    pub scanned_entries: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_manifest: Option<WorkerSessionArchiveManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicReadPage {
    pub session_id: String,
    pub generation: String,
    pub segment_id: String,
    pub entry_ref: String,
    pub kind: SessionPublicEntryKind,
    pub origin: protocol::SessionEntryProvenance,
    pub lineage: SessionPublicLineage,
    pub mode: SessionPublicReadMode,
    pub content: String,
    pub next_byte_offset: Option<usize>,
    pub has_more: bool,
    pub scanned_bytes: u64,
    pub scanned_segments: usize,
    pub scanned_entries: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_manifest: Option<WorkerSessionArchiveManifest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicUnavailableReason {
    NotFound,
    RetentionMissing,
    RetentionExpired,
    ArchiveIncomplete,
    CorruptLog,
    MigrationRequired,
    StorageUnavailable,
    InvalidCursor,
    ResourceLimit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SessionPublicSearchAvailability {
    Page {
        page: SessionPublicSearchPage,
    },
    Unavailable {
        reason: SessionPublicUnavailableReason,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SessionPublicReadAvailability {
    Page {
        page: SessionPublicReadPage,
    },
    Unavailable {
        reason: SessionPublicUnavailableReason,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedSessionIdentity {
    pub session_id: String,
    pub segment_id: String,
    pub entry_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerSessionUnavailableReason {
    RetentionMissing,
    ActivePointerMissing,
    CorruptLog,
    MigrationRequired,
    RetentionExpired,
    SnapshotTooLarge,
    StorageUnavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum WorkerSessionAvailability {
    LiveProtocol,
    RetainedSnapshot {
        identity: RetainedSessionIdentity,
        snapshot: protocol::SessionSnapshot,
    },
    Unavailable {
        reason: WorkerSessionUnavailableReason,
        message: String,
    },
}

/// Host-owned Subject release operation. A reservation has no timeout: uncertainty retains
/// occupancy and mutation exclusion until the same operation is explicitly reconciled.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectReleaseRequest {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub binding_id: String,
    pub operation_id: String,
    pub action: SubjectReleaseAction,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectReleaseAction {
    Reserve,
    Disconnect,
    Cancel,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectReleaseState {
    Reserved,
    Disconnected,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectReleaseResponse {
    pub operation_id: String,
    pub binding_id: String,
    pub state: SubjectReleaseState,
}

/// Irreversibly fence an unpublished failed Subject create. Exact retries must retain
/// all four fields; this is distinct from releasing an existing Subject Controller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectCreateRecoveryRequest {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub operation_id: String,
}

/// Durable admission-fence proof plus freshly confirmed execution absence.
/// An error (including a lost response) is never evidence of absence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectCreateRecoveryResponse {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub operation_id: String,
}

/// Host requests this only after durably fencing the Subject in Backend authority.
/// Permanently exclude the Worker from creation and restoration, and confirm actual
/// execution shutdown. This does not delete Worker or Session records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectDeleteWorkerRequest {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub operation_id: String,
}

/// Durable admission exclusion plus confirmed execution shutdown/absence. Errors,
/// timeouts and lost responses are unknown outcomes; retry the same full request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectDeleteWorkerResponse {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub operation_id: String,
    /// The canonical live Worker/Session/diagnostic aggregate is physically absent
    /// under the permanent creation fence. False is not a deletion receipt.
    pub originals_absent: bool,
}

/// Additional erasure of a Subject Job's newly reconciled removal copies.
/// Both identities are exact saved requests; this never changes the old receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectJobRemovalPurgeRequest {
    pub deletion: SubjectDeleteWorkerRequest,
    pub removal: WorkerRetentionExecutionRequest,
}

/// Confirms current physical erasure under the permanent deletion fence.
/// Operation identity belongs to `removal`, not the Subject deletion operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectJobRemovalPurgeResponse {
    pub workspace_id: String,
    pub worker_id: WorkerId,
    pub operation_id: String,
    pub input_fingerprint: String,
}

#[api(reqwest, axum)]
pub trait RuntimeApi {
    #[post("/v1/subject-job-removal-purge", status = 200, error_status = 400)]
    async fn purge_subject_job_removal(
        &self,
        #[body] request: SubjectJobRemovalPurgeRequest,
    ) -> Result<SubjectJobRemovalPurgeResponse, RuntimeApiError>;

    #[post("/v1/subject-delete-worker", status = 200, error_status = 400)]
    async fn subject_delete_worker(
        &self,
        #[body] request: SubjectDeleteWorkerRequest,
    ) -> Result<SubjectDeleteWorkerResponse, RuntimeApiError>;

    #[post("/v1/subject-create-recovery", status = 200, error_status = 400)]
    async fn recover_subject_create(
        &self,
        #[body] request: SubjectCreateRecoveryRequest,
    ) -> Result<SubjectCreateRecoveryResponse, RuntimeApiError>;

    #[post("/v1/subject-release", status = 200, error_status = 400)]
    async fn subject_release(
        &self,
        #[body] request: SubjectReleaseRequest,
    ) -> Result<SubjectReleaseResponse, RuntimeApiError>;

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

    #[get("/v1/workers/{worker_id}/session", status = 200, error_status = 400)]
    async fn worker_session(
        &self,
        #[path] worker_id: String,
        #[query] request: WorkerSessionRequest,
    ) -> Result<WorkerSessionAvailability, RuntimeApiError>;

    #[get(
        "/v1/workers/{worker_id}/session/history",
        status = 200,
        error_status = 400
    )]
    async fn worker_session_history(
        &self,
        #[path] worker_id: String,
        #[query] request: WorkerSessionHistoryRequest,
    ) -> Result<WorkerSessionHistoryAvailability, RuntimeApiError>;

    #[post(
        "/v1/session-public/material/retention/erase",
        status = 200,
        error_status = 400
    )]
    async fn session_material_erase(
        &self,
        #[body] request: SessionMaterialEraseRequest,
    ) -> Result<SessionMaterialEraseResponse, RuntimeApiError>;

    #[post(
        "/v1/session-public/material/retention/pin",
        status = 200,
        error_status = 400
    )]
    async fn session_material_pin(
        &self,
        #[body] request: SessionMaterialPinRequest,
    ) -> Result<SessionMaterialPinResponse, RuntimeApiError>;

    #[post(
        "/v1/session-public/material/retention/release",
        status = 200,
        error_status = 400
    )]
    async fn session_material_release(
        &self,
        #[body] request: SessionMaterialPinRequest,
    ) -> Result<SessionMaterialPinResponse, RuntimeApiError>;

    #[post(
        "/v1/session-public/recording-capture",
        status = 200,
        error_status = 400
    )]
    async fn session_recording_capture(
        &self,
        #[body] request: SessionRecordingCaptureRequest,
    ) -> Result<SessionRecordingCaptureAvailability, RuntimeApiError>;

    #[post(
        "/v1/session-public/material/catalog",
        status = 200,
        error_status = 400
    )]
    async fn session_material_catalog(
        &self,
        #[body] request: SessionMaterialCatalogRequest,
    ) -> Result<SessionMaterialCatalogAvailability, RuntimeApiError>;

    #[post("/v1/session-public/material/read", status = 200, error_status = 400)]
    async fn session_material_read(
        &self,
        #[body] request: SessionMaterialReadRequest,
    ) -> Result<SessionMaterialReadAvailability, RuntimeApiError>;

    #[post("/v1/session-public/search", status = 200, error_status = 400)]
    async fn session_public_search(
        &self,
        #[body] request: SessionPublicSearchRequest,
    ) -> Result<SessionPublicSearchAvailability, RuntimeApiError>;

    #[post("/v1/session-public/read", status = 200, error_status = 400)]
    async fn session_public_read(
        &self,
        #[body] request: SessionPublicReadRequest,
    ) -> Result<SessionPublicReadAvailability, RuntimeApiError>;

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

    #[post("/v1/workers/{worker_id}/restore/coordinate", status = 200, error_status = 400, additional_error_statuses = [409])]
    async fn coordinate_worker_restore(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerRestoreCoordinationRequest,
    ) -> Result<WorkerRestoreCoordinationResponse, RuntimeApiError>;

    #[post("/v1/workers/{worker_id}/restore", status = 200, error_status = 400, additional_error_statuses = [409])]
    async fn restore_worker(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerRestoreRequest,
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
        "/v1/workers/{worker_id}/workdir-attachments",
        status = 200,
        error_status = 400
    )]
    async fn replace_worker_workdir_attachments(
        &self,
        #[path] worker_id: String,
        #[body] request: WorkerWorkdirAttachmentsRequest,
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
pub const RUNTIME_ROUTE_WORKER_SESSION_ATTACHMENT: &str =
    "/v1/workers/{worker_id}/sessions/{session_id}/attachments/{attachment_id}";

/// Intentionally out-of-contract Runtime routes. This inventory keeps manual paths visible rather
/// than letting them be mistaken for management-contract omissions.
pub const REMAINING_RUNTIME_ROUTES: &[RemainingRuntimeRoute] = &[
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
    RemainingRuntimeRoute {
        method: "GET",
        path: RUNTIME_ROUTE_WORKER_SESSION_ATTACHMENT,
        reason: "Session image attachment transport",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_job_journal_grant_defaults_off_and_does_not_enable_memory_grant() {
        let legacy = serde_json::json!({"job_id":"job", "attempt_id":"attempt", "subjektiv_consolidation":true});
        let binding: BackendJobExecutionBinding = serde_json::from_value(legacy).unwrap();
        assert!(binding.subjektiv_consolidation);
        assert!(!binding.subjektiv_journal);
        let journal =
            serde_json::json!({"job_id":"job", "attempt_id":"attempt", "subjektiv_journal":true});
        let binding: BackendJobExecutionBinding = serde_json::from_value(journal.clone()).unwrap();
        assert!(binding.subjektiv_journal);
        assert!(!binding.subjektiv_consolidation);
        let roundtrip: BackendJobExecutionBinding =
            serde_json::from_value(serde_json::to_value(&binding).unwrap()).unwrap();
        assert_eq!(roundtrip, binding);
    }

    #[test]
    fn restore_request_is_mandatory_and_conflict_is_not_an_operation_state() {
        for invalid in [
            serde_json::json!({}),
            serde_json::json!({"request_id":"r"}),
            serde_json::json!({"expected_observation_token":"t"}),
        ] {
            assert!(serde_json::from_value::<WorkerRestoreRequest>(invalid).is_err());
        }
        let request: WorkerRestoreRequest = serde_json::from_value(
            serde_json::json!({"expected_observation_token":"t", "request_id":"r"}),
        )
        .unwrap();
        assert!(request.preparation.is_none());
        for (state, wire) in [
            (WorkerRestoreState::Accepted, "accepted"),
            (WorkerRestoreState::Rejected, "rejected"),
            (WorkerRestoreState::RolledBack, "rolled_back"),
            (WorkerRestoreState::Superseded, "superseded"),
            (
                WorkerRestoreState::ReconciliationRequired,
                "reconciliation_required",
            ),
        ] {
            assert_eq!(
                serde_json::from_value::<WorkerRestoreState>(serde_json::json!(wire)).unwrap(),
                state
            );
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(wire)
            );
        }
        assert!(
            serde_json::from_value::<WorkerRestoreState>(serde_json::json!(
                "restore_observation_conflict"
            ))
            .is_err()
        );
        let conflict = RuntimeApiError::new(
            409,
            "restore_observation_conflict",
            "refresh before a new intent",
        );
        assert_eq!(conflict.status(), 409);
        assert_eq!(
            serde_json::to_value(conflict).unwrap()["error"]["code"],
            "restore_observation_conflict"
        );
    }

    #[test]
    fn feature_argument_completion_wire_preserves_source_and_argument() {
        let json = serde_json::json!({"kind":"feature_argument", "prefix":"資料/", "context": {"invocation":"builtin:test/prepare", "argument":"path"}});
        let request: CompletionRequest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(request.kind, CompletionKind::FeatureArgument);
        assert_eq!(
            request.context.as_ref().unwrap().argument.as_deref(),
            Some("path")
        );
        assert_eq!(serde_json::to_value(request).unwrap(), json);
        let legacy: CompletionRequest =
            serde_json::from_value(serde_json::json!({"kind":"file"})).unwrap();
        assert!(legacy.context.is_none());
    }

    #[derive(Clone)]
    struct RoundTripService;

    fn test_error(status: u16) -> RuntimeApiError {
        RuntimeApiError::new(status, "test_error", "test error")
    }

    #[test]
    fn capability_less_attachment_claims_default_to_read_only() {
        let claim: WorkingDirectoryAttachmentClaim = serde_json::from_value(serde_json::json!({
            "alias": "docs",
            "working_directory_id": "workdir-docs"
        }))
        .unwrap();
        assert_eq!(
            claim.capabilities,
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );

        let logical: LogicalWorkdirAttachment = serde_json::from_value(serde_json::json!({
            "alias": "docs",
            "working_directory_id": "workdir-docs"
        }))
        .unwrap();
        assert_eq!(
            logical.capabilities,
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );
    }

    #[test]
    fn attachment_claim_serializes_backend_authored_capabilities() {
        let claim = WorkingDirectoryAttachmentClaim {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory_id: "workdir-main".to_string(),
            relative_cwd: None,
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
        };
        let value = serde_json::to_value(&claim).unwrap();
        assert_eq!(
            value.get("capabilities"),
            Some(&serde_json::to_value(workdir::WorkdirSessionCapabilities::ALL).unwrap())
        );
        assert_eq!(
            serde_json::from_value::<WorkingDirectoryAttachmentClaim>(value)
                .unwrap()
                .capabilities,
            workdir::WorkdirSessionCapabilities::ALL
        );
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

        async fn worker_session(
            &self,
            _worker_id: String,
            _request: WorkerSessionRequest,
        ) -> Result<WorkerSessionAvailability, RuntimeApiError> {
            Ok(WorkerSessionAvailability::LiveProtocol)
        }

        async fn worker_session_history(
            &self,
            _worker_id: String,
            _request: WorkerSessionHistoryRequest,
        ) -> Result<WorkerSessionHistoryAvailability, RuntimeApiError> {
            Ok(WorkerSessionHistoryAvailability::Unavailable {
                reason: WorkerSessionHistoryUnavailableReason::Unsupported,
                message: "unsupported".to_string(),
            })
        }

        async fn session_material_erase(
            &self,
            _request: SessionMaterialEraseRequest,
        ) -> Result<SessionMaterialEraseResponse, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_material_pin(
            &self,
            _request: SessionMaterialPinRequest,
        ) -> Result<SessionMaterialPinResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn purge_subject_job_removal(
            &self,
            request: SubjectJobRemovalPurgeRequest,
        ) -> Result<SubjectJobRemovalPurgeResponse, RuntimeApiError> {
            Ok(SubjectJobRemovalPurgeResponse {
                workspace_id: request.removal.workspace_id,
                worker_id: request.removal.worker_id,
                operation_id: request.removal.operation_id,
                input_fingerprint: request.removal.input_fingerprint,
            })
        }

        async fn subject_delete_worker(
            &self,
            request: SubjectDeleteWorkerRequest,
        ) -> Result<SubjectDeleteWorkerResponse, RuntimeApiError> {
            Ok(SubjectDeleteWorkerResponse {
                originals_absent: true,
                workspace_id: request.workspace_id,
                worker_id: request.worker_id,
                create_fingerprint: request.create_fingerprint,
                operation_id: request.operation_id,
            })
        }

        async fn recover_subject_create(
            &self,
            request: SubjectCreateRecoveryRequest,
        ) -> Result<SubjectCreateRecoveryResponse, RuntimeApiError> {
            Ok(SubjectCreateRecoveryResponse {
                workspace_id: request.workspace_id,
                worker_id: request.worker_id,
                create_fingerprint: request.create_fingerprint,
                operation_id: request.operation_id,
            })
        }

        async fn subject_release(
            &self,
            _request: SubjectReleaseRequest,
        ) -> Result<SubjectReleaseResponse, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_material_release(
            &self,
            _request: SessionMaterialPinRequest,
        ) -> Result<SessionMaterialPinResponse, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_recording_capture(
            &self,
            _request: SessionRecordingCaptureRequest,
        ) -> Result<SessionRecordingCaptureAvailability, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_material_catalog(
            &self,
            _request: SessionMaterialCatalogRequest,
        ) -> Result<SessionMaterialCatalogAvailability, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_material_read(
            &self,
            _request: SessionMaterialReadRequest,
        ) -> Result<SessionMaterialReadAvailability, RuntimeApiError> {
            Err(test_error(501))
        }

        async fn session_public_search(
            &self,
            _request: SessionPublicSearchRequest,
        ) -> Result<SessionPublicSearchAvailability, RuntimeApiError> {
            Ok(SessionPublicSearchAvailability::Unavailable {
                reason: SessionPublicUnavailableReason::StorageUnavailable,
                message: "unsupported".to_string(),
            })
        }

        async fn session_public_read(
            &self,
            _request: SessionPublicReadRequest,
        ) -> Result<SessionPublicReadAvailability, RuntimeApiError> {
            Ok(SessionPublicReadAvailability::Unavailable {
                reason: SessionPublicUnavailableReason::StorageUnavailable,
                message: "unsupported".to_string(),
            })
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
        async fn coordinate_worker_restore(
            &self,
            _worker_id: String,
            _request: WorkerRestoreCoordinationRequest,
        ) -> Result<WorkerRestoreCoordinationResponse, RuntimeApiError> {
            Err(test_error(501))
        }
        async fn restore_worker(
            &self,
            _worker_id: String,
            _request: WorkerRestoreRequest,
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
        async fn replace_worker_workdir_attachments(
            &self,
            _worker_id: String,
            _request: WorkerWorkdirAttachmentsRequest,
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

        let recovery = SubjectCreateRecoveryRequest {
            workspace_id: "workspace-a".into(),
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "failed-create-fingerprint".into(),
            operation_id: "recovery-1".into(),
        };
        let response = client
            .recover_subject_create(recovery.clone())
            .await
            .unwrap();
        assert_eq!(response.workspace_id, recovery.workspace_id);
        assert_eq!(response.worker_id, recovery.worker_id);
        assert_eq!(response.create_fingerprint, recovery.create_fingerprint);
        assert_eq!(response.operation_id, recovery.operation_id);

        let deletion = SubjectDeleteWorkerRequest {
            workspace_id: "workspace-a".into(),
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "deleted-create".into(),
            operation_id: "delete-1".into(),
        };
        let receipt = client
            .subject_delete_worker(deletion.clone())
            .await
            .unwrap();
        let mut expected = serde_json::to_value(&deletion).unwrap();
        expected["originals_absent"] = true.into();
        assert_eq!(serde_json::to_value(receipt).unwrap(), expected);
        let purge = SubjectJobRemovalPurgeRequest {
            removal: WorkerRetentionExecutionRequest {
                operation_id: "old-removal".into(),
                input_fingerprint: "old-removal-fingerprint".into(),
                archive_id: Some("old-archive".into()),
                workspace_id: deletion.workspace_id.clone(),
                source_runtime_id: "runtime-a".into(),
                worker_id: deletion.worker_id,
                source_created_at: "2026-01-01T00:00:00Z".into(),
                removed_at: "2026-01-02T00:00:00Z".into(),
                effective_profile: None,
                retention_class: None,
                policy_id: "old-policy".into(),
                session_disposition: SessionDisposition::Archive,
                diagnostics_disposition: DiagnosticsDisposition::Retain,
            },
            deletion,
        };
        let receipt = client
            .purge_subject_job_removal(purge.clone())
            .await
            .unwrap();
        assert_eq!(receipt.workspace_id, purge.removal.workspace_id);
        assert_eq!(receipt.worker_id, purge.removal.worker_id);
        assert_eq!(receipt.operation_id, purge.removal.operation_id);
        assert_eq!(receipt.input_fingerprint, purge.removal.input_fingerprint);
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
    fn worker_session_availability_has_closed_tagged_wire_shape() {
        let value = serde_json::to_value(WorkerSessionAvailability::Unavailable {
            reason: WorkerSessionUnavailableReason::SnapshotTooLarge,
            message: "snapshot exceeds limit".to_string(),
        })
        .unwrap();
        assert_eq!(value["availability"], "unavailable");
        assert_eq!(value["reason"], "snapshot_too_large");
        assert_eq!(value["message"], "snapshot exceeds limit");
    }

    #[test]
    fn worker_session_history_availability_has_closed_tagged_wire_shape() {
        let value = serde_json::to_value(WorkerSessionHistoryAvailability::Unavailable {
            reason: WorkerSessionHistoryUnavailableReason::InvalidCursor,
            message: "cursor is stale".to_string(),
        })
        .unwrap();
        assert_eq!(value["availability"], "unavailable");
        assert_eq!(value["reason"], "invalid_cursor");
        assert_eq!(value["message"], "cursor is stale");
    }

    #[test]
    fn contract_inventory_has_unique_routes_under_api_base_path() {
        let operations = RuntimeApiMetadata::OPERATIONS;
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
        assert_eq!(REMAINING_RUNTIME_ROUTES.len(), 19);
        for remaining in REMAINING_RUNTIME_ROUTES {
            assert!(!RuntimeApiMetadata::OPERATIONS.iter().any(|operation| {
                format!("{:?}", operation.method).eq_ignore_ascii_case(remaining.method)
                    && operation.path == remaining.path
            }));
        }
    }
}
