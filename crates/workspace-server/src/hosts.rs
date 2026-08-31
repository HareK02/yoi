use crate::Error;
use crate::resource_broker::{BackendResourceBroker, BackendResourceTarget};
use chrono::Utc;
use protocol::Segment;
use reqwest::blocking::{Client as BlockingHttpClient, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client as AsyncHttpClient, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, RwLock},
    time::Duration,
};
use workdir::{
    Workdir, WorkdirError, WorkdirSessionHandle,
    http::{OpenWorkdirSessionRequest, RemoteWorkdirSession, WorkdirHttpAuthorization},
};
use worker_runtime::RuntimeWorkspaceScope;
use worker_runtime::auth::{CapabilityTokenSigner, capability_claims};
use worker_runtime::catalog::{
    ConfigBundleRef, CreateWorkerRequest, ProfileSelector, ProfileSourceArchiveHttpRef,
    ProfileSourceArchiveSource, WorkerDetail as EmbeddedWorkerDetail,
    WorkerStatus as EmbeddedWorkerStatus, WorkingDirectoryClaim,
    WorkingDirectoryRepositoryAccessRequest, WorkingDirectoryRequest, WorkingDirectoryStatus,
    WorkingDirectorySummary, WorkspaceApiRef,
};
use worker_runtime::config_bundle::{ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary};
#[cfg(test)]
use worker_runtime::config_bundle::{
    ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::error::RuntimeError as EmbeddedRuntimeError;
#[cfg(test)]
use worker_runtime::execution::WorkerExecutionRunState;
use worker_runtime::fs_store::FsRuntimeStoreOptions;
use worker_runtime::http_server::{
    RuntimeHttpConfigBundleAvailabilityResponse, RuntimeHttpConfigBundleSyncRequest,
    RuntimeHttpErrorResponse, RuntimeHttpRepositoryAccessResponse, RuntimeHttpSummaryResponse,
    RuntimeHttpWorkerCompletionsRequest, RuntimeHttpWorkerCompletionsResponse,
    RuntimeHttpWorkerDeleteResponse, RuntimeHttpWorkerInputResponse,
    RuntimeHttpWorkerLifecycleRequest, RuntimeHttpWorkerLifecycleResponse,
    RuntimeHttpWorkerResponse, RuntimeHttpWorkerWorkspaceApiRequest, RuntimeHttpWorkersResponse,
    RuntimeHttpWorkingDirectoriesResponse, RuntimeHttpWorkingDirectoryResponse,
    RuntimeHttpWorkspacePromptProjectionRequest, RuntimeHttpWorkspacePromptProjectionResponse,
};
use worker_runtime::identity::{
    RuntimeWorkerRef, WorkerId as EmbeddedWorkerId, WorkerRef as EmbeddedWorkerRef,
};
use worker_runtime::interaction::{
    WorkerInput as EmbeddedWorkerInput, WorkerInputKind as EmbeddedWorkerInputKind,
};
use worker_runtime::management::{RuntimeOptions as EmbeddedRuntimeOptions, RuntimeStatus};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};
use worker_runtime::retention::{
    WorkerRetentionExecutionRequest, WorkerRetentionExecutionResult, WorkerRetentionInventory,
};

pub(crate) const EMBEDDED_RUNTIME_ID: &str = "embedded-worker-runtime";
const EMBEDDED_HOST_KIND: &str = "embedded-worker-runtime-host";
const REMOTE_HOST_KIND: &str = "remote-worker-runtime-host";
const MAX_DIAGNOSTICS: usize = 16;
const MAX_HOST_SCAN: usize = 256;
const MAX_IDENTIFIER_LEN: usize = 120;
const ID_DIGEST_HEX_LEN: usize = 16;

fn run_blocking_http<T, F>(operation: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(operation)
        }
        Ok(_) => std::thread::spawn(operation)
            .join()
            .expect("blocking HTTP thread panicked"),
        Err(_) => operation(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

impl RuntimeDiagnostic {
    pub fn new(code: impl Into<String>, severity: &str, message: impl Into<String>) -> Self {
        let severity = match severity {
            "error" => DiagnosticSeverity::Error,
            "warning" => DiagnosticSeverity::Warning,
            _ => DiagnosticSeverity::Info,
        };
        diagnostic(code, severity, message)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceKind {
    EmbeddedWorkerRuntime,
    RemoteHttp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIdentityAuthority {
    /// Public Runtime/Host/Worker ids are registry projections, never raw
    /// socket addresses, session ids, credentials, or paths.
    RuntimeRegistryProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSourceSummary {
    pub kind: RuntimeSourceKind,
    pub status: RuntimeSourceStatus,
    pub identity_authority: RuntimeIdentityAuthority,
    pub note: String,
}

impl RuntimeSourceSummary {
    pub fn embedded_worker_runtime() -> Self {
        Self {
            kind: RuntimeSourceKind::EmbeddedWorkerRuntime,
            status: RuntimeSourceStatus::Active,
            identity_authority: RuntimeIdentityAuthority::RuntimeRegistryProjection,
            note: "backend-internal embedded worker-runtime Runtime exposed only through runtime_id plus worker_id projections".to_string(),
        }
    }

    pub fn embedded_worker_runtime_reserved() -> Self {
        Self {
            kind: RuntimeSourceKind::EmbeddedWorkerRuntime,
            status: RuntimeSourceStatus::Reserved,
            identity_authority: RuntimeIdentityAuthority::RuntimeRegistryProjection,
            note: "reserved boundary for an embedded worker-runtime adapter; not connected by this fixture source".to_string(),
        }
    }

    pub fn remote_http() -> Self {
        Self {
            kind: RuntimeSourceKind::RemoteHttp,
            status: RuntimeSourceStatus::Active,
            identity_authority: RuntimeIdentityAuthority::RuntimeRegistryProjection,
            note: "backend-owned remote worker-runtime REST/WS client; endpoints and credentials remain backend-private".to_string(),
        }
    }

    pub fn remote_http_reserved() -> Self {
        Self {
            kind: RuntimeSourceKind::RemoteHttp,
            status: RuntimeSourceStatus::Reserved,
            identity_authority: RuntimeIdentityAuthority::RuntimeRegistryProjection,
            note: "reserved boundary for a future remote Runtime adapter; no HTTP client or REST server is implemented here".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub source: RuntimeSourceSummary,
    pub host_ids: Vec<String>,
    pub worker_creation_available: bool,
    pub os: String,
    pub arch: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSummary {
    pub runtime_id: String,
    pub host_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub observed_at: String,
    pub last_seen_at: Option<String>,
    pub os: String,
    pub arch: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerImplementationSummary {
    pub kind: String,
    pub display_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCapabilitySummary {
    pub can_stop: bool,
    pub can_spawn_followup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSummary {
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub host_id: String,
    /// Human-readable display name. This is not identity and may be duplicated.
    pub display_name: String,
    /// Backward-compatible display label. New UI should prefer `display_name`.
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    pub state: String,
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectorySummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

impl From<RuntimeDiagnostic> for workspace_api::Diagnostic {
    fn from(diagnostic: RuntimeDiagnostic) -> Self {
        let severity = match diagnostic.severity {
            DiagnosticSeverity::Info => workspace_api::DiagnosticSeverity::Info,
            DiagnosticSeverity::Warning => workspace_api::DiagnosticSeverity::Warning,
            DiagnosticSeverity::Error => workspace_api::DiagnosticSeverity::Error,
        };
        Self {
            code: diagnostic.code,
            severity,
            message: diagnostic.message,
        }
    }
}

impl From<RuntimeSourceSummary> for workspace_api::RuntimeSourceSummary {
    fn from(source: RuntimeSourceSummary) -> Self {
        let kind = match source.kind {
            RuntimeSourceKind::EmbeddedWorkerRuntime => {
                workspace_api::RuntimeSourceKind::EmbeddedWorkerRuntime
            }
            RuntimeSourceKind::RemoteHttp => workspace_api::RuntimeSourceKind::RemoteHttp,
        };
        let status = match source.status {
            RuntimeSourceStatus::Active => workspace_api::RuntimeSourceStatus::Active,
            RuntimeSourceStatus::Reserved => workspace_api::RuntimeSourceStatus::Reserved,
        };
        let identity_authority = match source.identity_authority {
            RuntimeIdentityAuthority::RuntimeRegistryProjection => {
                workspace_api::RuntimeIdentityAuthority::RuntimeRegistryProjection
            }
        };
        Self {
            kind,
            status,
            identity_authority,
            note: source.note,
        }
    }
}

impl From<RuntimeSummary> for workspace_api::RuntimeSummary {
    fn from(runtime: RuntimeSummary) -> Self {
        Self {
            runtime_id: runtime.runtime_id,
            label: runtime.label,
            kind: runtime.kind,
            status: runtime.status,
            source: runtime.source.into(),
            host_ids: runtime.host_ids,
            worker_creation_available: runtime.worker_creation_available,
            os: runtime.os,
            arch: runtime.arch,
            diagnostics: runtime.diagnostics.into_iter().map(Into::into).collect(),
        }
    }
}

pub(crate) fn workspace_worker_summary(
    summary: WorkerSummary,
    resource_key: String,
) -> workspace_api::WorkerSummary {
    workspace_api::WorkerSummary {
        runtime_id: summary.worker.runtime_id,
        worker_id: summary.worker.worker_id,
        resource_key,
        host_id: summary.host_id,
        display_name: summary.display_name,
        label: summary.label,
        profile: summary.profile,
        singleton_key: summary.singleton_key,
        tags: summary.tags,
        workspace: workspace_api::WorkerWorkspaceSummary {
            visibility: summary.workspace.visibility,
            identity: summary.workspace.identity,
            workspace_id: summary.workspace.workspace_id,
        },
        state: summary.state,
        last_seen_at: summary.last_seen_at,
        pinned: summary.pinned,
        retention_state: summary.retention_state,
        implementation: workspace_api::WorkerImplementationSummary {
            kind: summary.implementation.kind,
            display_hint: summary.implementation.display_hint,
        },
        capabilities: workspace_api::WorkerCapabilitySummary {
            can_stop: summary.capabilities.can_stop,
            can_spawn_followup: summary.capabilities.can_spawn_followup,
        },
        working_directory: summary.working_directory.map(crate::workdir_api::summary),
        diagnostics: summary.diagnostics.into_iter().map(Into::into).collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRestoreResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkspaceApiResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeList<T> {
    pub items: Vec<T>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

impl<T> RuntimeList<T> {
    fn new(items: Vec<T>, diagnostics: Vec<RuntimeDiagnostic>) -> Self {
        Self { items, diagnostics }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerLookupResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeWorkingDirectoryResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryStatus>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

/// Browser-safe worker spawn request shape.
///
/// The request carries Browser-facing launch semantics only: workspace intent,
/// optional display identity, acceptance policy, optional profile selector,
/// optional initial input, and optional configured Repository selector for
/// working-directory materialization. Runtime execution authority is resolved by the host
/// into a synced ConfigBundle before the canonical Runtime create request is
/// built. Raw workspace roots, child cwd, executable paths, tool scope,
/// credentials, raw config stores, sockets, sessions, and storage paths are not
/// accepted from Workspace API callers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerSpawnWorkingDirectoryRequest {
    /// Safe configured Repository id. The host resolves this id to repository
    /// authority from server-side config; browser callers cannot provide raw
    /// source paths or runtime-internal storage paths.
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerTicketAssignmentRequest {
    pub ticket_id: String,
    pub operation_id: String,
}

pub(crate) fn worker_spawn_create_fingerprint(
    request: &WorkerSpawnRequest,
) -> Result<String, String> {
    let encoded = serde_json::to_vec(request)
        .map_err(|error| format!("serialize Worker create input: {error}"))?;
    Ok(format!("sha256:{}", digest_hex(&encoded, 64)))
}

pub(crate) fn worker_spawn_idempotency(
    request: &WorkerSpawnRequest,
) -> Result<Option<(String, String)>, String> {
    if let Some(operation) = request.resolved_control_operation.as_ref() {
        return Ok(Some((
            operation.operation_id.clone(),
            operation.input_fingerprint.clone(),
        )));
    }
    let Some(assignment) = request.ticket_assignment.as_ref() else {
        return Ok(None);
    };
    let encoded = serde_json::to_vec(request)
        .map_err(|error| format!("serialize Worker spawn idempotency input: {error}"))?;
    Ok(Some((
        assignment.operation_id.clone(),
        format!("sha256:{}", digest_hex(&encoded, 64)),
    )))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerControlOperation {
    pub operation_id: String,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCreateBinding {
    pub worker_id: EmbeddedWorkerId,
    pub create_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerSpawnRequest {
    pub intent: WorkerSpawnIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_worker_name: Option<String>,
    pub acceptance: WorkerSpawnAcceptanceRequirement,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_assignment: Option<WorkerTicketAssignmentRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_submit: Vec<Segment>,
    /// Optional safe working-directory creation request. The Workspace server resolves
    /// this into a runtime-internal `WorkingDirectoryRequest` from configured
    /// repositories before calling a host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_request: Option<WorkerSpawnWorkingDirectoryRequest>,
    #[serde(skip, default)]
    pub resolved_working_directory_request: Option<WorkingDirectoryRequest>,
    #[serde(skip, default)]
    pub resolved_working_directory: Option<WorkingDirectoryClaim>,
    #[serde(skip, default)]
    pub resolved_config_bundle: Option<ConfigBundle>,
    #[serde(skip, default)]
    pub resolved_workspace_api: Option<WorkspaceApiRef>,
    /// Backend-authored immutable Workspace Memory settings snapshot.
    #[serde(skip, default)]
    pub resolved_memory_settings: Option<manifest::WorkspaceMemorySettingsSnapshot>,
    /// Backend-owned feature enablement; client input cannot set it.
    #[serde(skip, default)]
    pub resolved_worker_observation_enabled: bool,
    /// Backend-authored peer-session grants. Browser/model input cannot set this field.
    #[serde(skip, default)]
    pub resolved_worker_observation_grants: Vec<worker_runtime::identity::RuntimeWorkerRef>,
    /// Trusted Backend operation identity used to make Worker-owned spawns replay-safe.
    #[serde(skip, default)]
    pub resolved_control_operation: Option<WorkerControlOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerSpawnIntent {
    WorkspaceCompanion,
    WorkspaceOrchestrator,
    WorkspaceCoding,
    TicketRole {
        ticket_id: String,
        role: TicketWorkerRole,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TicketWorkerRole {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerSpawnAcceptanceRequirement {
    SocketReady,
    RunAccepted { expected_segments: usize },
}

fn initial_worker_input(segments: &[Segment]) -> Option<EmbeddedWorkerInput> {
    if segments.is_empty() {
        return None;
    }
    Some(EmbeddedWorkerInput {
        kind: EmbeddedWorkerInputKind::User,
        content: Segment::flatten_to_text(segments),
        submission_id: None,
        segments: Some(segments.to_vec()),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSpawnResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub acceptance_evidence: Vec<WorkerSpawnAcceptanceEvidence>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigBundleSyncResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ConfigBundleAvailability>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigBundleCheckResult {
    pub state: WorkerOperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ConfigBundleAvailability>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigBundleListResult {
    pub bundles: Vec<ConfigBundleSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

fn required_worker_workspace_api(
    request: &WorkerSpawnRequest,
) -> Result<WorkspaceApiRef, RuntimeDiagnostic> {
    request.resolved_workspace_api.clone().ok_or_else(|| {
        diagnostic(
            "worker_workspace_api_missing",
            DiagnosticSeverity::Error,
            "Workspace-bound Worker spawn requires a resolved Workspace API binding",
        )
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationState {
    Accepted,
    Unsupported,
    Rejected,
}

impl From<WorkerOperationState> for workspace_api::WorkerOperationState {
    fn from(state: WorkerOperationState) -> Self {
        match state {
            WorkerOperationState::Accepted => Self::Accepted,
            WorkerOperationState::Unsupported => Self::Unsupported,
            WorkerOperationState::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSpawnAcceptanceEvidence {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStopRequest {
    pub worker_id: String,
    pub mode: WorkerStopMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStopMode {
    Graceful,
    Force,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStopResult {
    pub state: WorkerOperationState,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerLifecycleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_assignment: Option<WorkerTicketAssignmentRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerLifecycleResult {
    pub state: WorkerOperationState,
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    Notify,
    Compact,
    ListRewindTargets,
    RegisterPeer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerDeleteResult {
    pub state: WorkerOperationState,
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub deleted: bool,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInputRequest {
    #[serde(default = "default_worker_input_kind")]
    pub kind: WorkerInputKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<protocol::Segment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCompletionsRequest {
    pub kind: protocol::CompletionKind,
    #[serde(default)]
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCompletionsResult {
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub kind: protocol::CompletionKind,
    pub prefix: String,
    pub entries: Vec<protocol::CompletionEntry>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInputResult {
    pub state: WorkerOperationState,
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerProxyConnectPoint {
    pub kind: String,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRegistryError {
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    UnknownRuntime(String),
    UnknownHost(String),
    UnknownWorker {
        worker: RuntimeWorkerRef,
    },
    RuntimeOperationFailed {
        runtime_id: String,
        code: String,
        message: String,
    },
}

impl RuntimeRegistryError {
    pub fn message(&self) -> String {
        match self {
            Self::InvalidIdentifier { kind, value } => {
                format!("invalid {kind} identifier `{value}`")
            }
            Self::UnknownRuntime(runtime_id) => format!("unknown runtime `{runtime_id}`"),
            Self::UnknownHost(host_id) => format!("unknown host `{host_id}`"),
            Self::UnknownWorker { worker } => format!(
                "unknown worker `{}` in runtime `{}`",
                worker.worker_id, worker.runtime_id
            ),
            Self::RuntimeOperationFailed { message, .. } => message.clone(),
        }
    }

    pub fn into_error(self) -> Error {
        match self {
            Self::InvalidIdentifier { kind, value } => Error::InvalidRuntimeIdentifier {
                kind: kind.to_string(),
                value,
            },
            Self::UnknownRuntime(runtime_id) => Error::UnknownRuntime(runtime_id),
            Self::UnknownHost(host_id) => Error::UnknownHost(host_id),
            Self::UnknownWorker { worker } => Error::UnknownWorker { worker },
            Self::RuntimeOperationFailed {
                runtime_id,
                code,
                message,
            } => Error::RuntimeOperationFailed {
                runtime_id,
                code,
                message,
            },
        }
    }
}

fn default_worker_input_kind() -> WorkerInputKind {
    WorkerInputKind::User
}

pub trait WorkspaceWorkerRuntime: Send + Sync {
    fn runtime_id(&self) -> &str;

    fn runtime_summary(&self, limit: usize) -> RuntimeSummary;

    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary>;

    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary>;

    fn list_stopped_workers(&self, _limit: usize) -> RuntimeList<WorkerSummary> {
        RuntimeList::new(Vec::new(), Vec::new())
    }

    fn worker(&self, worker_id: &str) -> WorkerLookupResult;

    fn restore_worker(&self, worker_id: &str) -> WorkerRestoreResult {
        WorkerRestoreResult {
            state: WorkerOperationState::Unsupported,
            worker: None,
            diagnostics: vec![diagnostic(
                "worker_restore_unsupported",
                DiagnosticSeverity::Info,
                format!("runtime does not implement worker restore for `{worker_id}`"),
            )],
        }
    }

    fn replace_worker_workspace_api(
        &self,
        worker_id: &str,
        _workspace_api: WorkspaceApiRef,
    ) -> WorkerWorkspaceApiResult {
        WorkerWorkspaceApiResult {
            state: WorkerOperationState::Unsupported,
            worker: None,
            diagnostics: vec![diagnostic(
                "worker_workspace_api_replace_unsupported",
                DiagnosticSeverity::Info,
                format!(
                    "runtime does not support replacing the Workspace API for worker `{worker_id}`"
                ),
            )],
        }
    }

    fn create_working_directory(
        &self,
        _request: WorkingDirectoryRequest,
    ) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Unsupported,
            working_directory: None,
            diagnostics: vec![diagnostic(
                "runtime_working_directory_create_unsupported",
                DiagnosticSeverity::Info,
                "runtime does not implement working directory creation".to_string(),
            )],
        }
    }

    fn authorize_working_directory_repository_access(
        &self,
        _request: WorkingDirectoryRepositoryAccessRequest,
    ) -> std::result::Result<(), Error> {
        Err(Error::InvalidInput(
            "Runtime does not support working directory Repository access authorization"
                .to_string(),
        ))
    }

    fn list_working_directories(&self) -> RuntimeList<WorkingDirectoryStatus> {
        RuntimeList::new(Vec::new(), Vec::new())
    }

    fn working_directory(&self, working_directory_id: &str) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Unsupported,
            working_directory: None,
            diagnostics: vec![diagnostic(
                "runtime_working_directory_lookup_unsupported",
                DiagnosticSeverity::Info,
                format!(
                    "runtime does not implement working directory lookup for `{working_directory_id}`"
                ),
            )],
        }
    }

    fn open_workdir_session<'a>(
        &'a self,
        _working_directory_id: &'a str,
        _owner_worker_id: Option<&'a str>,
    ) -> Pin<
        Box<dyn Future<Output = Result<workdir::WorkdirSessionHandle, WorkdirError>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(WorkdirError::Unavailable(
                "Runtime does not expose Workdir operation sessions".to_string(),
            ))
        })
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Unsupported,
            working_directory: None,
            diagnostics: vec![diagnostic(
                "runtime_working_directory_cleanup_unsupported",
                DiagnosticSeverity::Info,
                format!(
                    "runtime does not implement working directory cleanup for `{working_directory_id}`"
                ),
            )],
        }
    }

    fn spawn_worker(
        &self,
        _binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        WorkerSpawnResult {
            state: WorkerOperationState::Unsupported,
            worker: None,
            acceptance_evidence: Vec::new(),
            diagnostics: vec![diagnostic(
                "worker_spawn_resolver_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker spawn intent '{}' was accepted as a typed request shape, but launch resolution is not implemented by this registry surface",
                    worker_spawn_intent_label(&request.intent)
                ),
            )],
        }
    }

    fn observe_workspace_prompt_projection(
        &self,
        _projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        Ok(())
    }

    fn sync_config_bundle(&self, _bundle: ConfigBundle) -> ConfigBundleSyncResult {
        ConfigBundleSyncResult {
            state: WorkerOperationState::Unsupported,
            availability: None,
            diagnostics: vec![diagnostic(
                "config_bundle_sync_unsupported",
                DiagnosticSeverity::Info,
                "runtime does not implement config bundle sync".to_string(),
            )],
        }
    }

    fn check_config_bundle(&self, _reference: ConfigBundleRef) -> ConfigBundleCheckResult {
        ConfigBundleCheckResult {
            state: WorkerOperationState::Unsupported,
            availability: None,
            diagnostics: vec![diagnostic(
                "config_bundle_check_unsupported",
                DiagnosticSeverity::Info,
                "runtime does not implement config bundle availability checks".to_string(),
            )],
        }
    }

    fn list_config_bundles(&self) -> ConfigBundleListResult {
        ConfigBundleListResult {
            bundles: Vec::new(),
            diagnostics: vec![diagnostic(
                "config_bundle_list_unsupported",
                DiagnosticSeverity::Info,
                "runtime does not implement config bundle listing".to_string(),
            )],
        }
    }

    fn send_protocol_method(
        &self,
        _worker_id: &str,
        _method: protocol::Method,
    ) -> Result<Vec<protocol::Event>, RuntimeRegistryError> {
        Err(RuntimeRegistryError::RuntimeOperationFailed {
            runtime_id: self.runtime_id().to_string(),
            code: "worker_protocol_method_unsupported".to_string(),
            message: "runtime does not support Worker protocol command transport".to_string(),
        })
    }

    fn stop_worker(
        &self,
        worker_id: &str,
        _request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        WorkerLifecycleResult {
            state: WorkerOperationState::Unsupported,
            worker: RuntimeWorkerRef::new(self.runtime_id().to_string(), worker_id.to_string()),
            diagnostics: vec![diagnostic(
                "worker_stop_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker stop for '{worker_id}' is reserved for the runtime service boundary and is not implemented by this registry surface"
                ),
            )],
        }
    }

    fn cancel_worker(
        &self,
        worker_id: &str,
        _request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        WorkerLifecycleResult {
            state: WorkerOperationState::Unsupported,
            worker: RuntimeWorkerRef::new(self.runtime_id().to_string(), worker_id.to_string()),
            diagnostics: vec![diagnostic(
                "worker_cancel_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker cancel for '{worker_id}' is reserved for the runtime service boundary and is not implemented by this registry surface"
                ),
            )],
        }
    }

    fn delete_worker(&self, worker_id: &str) -> WorkerDeleteResult {
        WorkerDeleteResult {
            state: WorkerOperationState::Unsupported,
            worker: RuntimeWorkerRef::new(self.runtime_id().to_string(), worker_id.to_string()),
            deleted: false,
            diagnostics: vec![diagnostic(
                "worker_delete_unsupported",
                DiagnosticSeverity::Info,
                format!("runtime does not implement worker deletion for '{worker_id}'"),
            )],
        }
    }

    fn worker_retention_inventory(
        &self,
        worker_id: &str,
    ) -> Result<WorkerRetentionInventory, String> {
        Err(format!(
            "runtime does not implement retention inventory for '{worker_id}'"
        ))
    }

    fn execute_worker_retention(
        &self,
        request: WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, String> {
        Err(format!(
            "runtime does not implement retention execution for '{}'",
            request.worker_id
        ))
    }

    fn observation_source(
        &self,
        _worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSource> {
        None
    }

    fn send_input(&self, worker_id: &str, _request: WorkerInputRequest) -> WorkerInputResult {
        WorkerInputResult {
            state: WorkerOperationState::Unsupported,
            worker: RuntimeWorkerRef::new(self.runtime_id().to_string(), worker_id.to_string()),
            diagnostics: vec![diagnostic(
                "worker_input_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker input for '{worker_id}' is reserved for the runtime service boundary and is not implemented by this registry source"
                ),
            )],
        }
    }

    fn worker_completions(
        &self,
        worker_id: &str,
        request: WorkerCompletionsRequest,
    ) -> WorkerCompletionsResult {
        WorkerCompletionsResult {
            worker: RuntimeWorkerRef::new(self.runtime_id().to_string(), worker_id.to_string()),
            kind: request.kind,
            prefix: request.prefix,
            entries: Vec::new(),
            diagnostics: vec![diagnostic(
                "worker_completions_unsupported",
                DiagnosticSeverity::Info,
                format!("runtime does not implement completions for worker '{worker_id}'"),
            )],
        }
    }

    fn proxy_connect_points(&self, worker_id: &str) -> Vec<WorkerProxyConnectPoint> {
        vec![WorkerProxyConnectPoint {
            kind: "stream_proxy".to_string(),
            status: "not_implemented".to_string(),
            diagnostics: vec![diagnostic(
                "worker_proxy_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker proxy connect points for '{}' are not implemented by this overview-only registry surface",
                    worker_id
                ),
            )],
        }]
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeRegistryUnregisterResult {
    Removed,
    NotFound,
    BlockedByWorkers {
        worker_count: usize,
        diagnostics: Vec<RuntimeDiagnostic>,
    },
}

#[derive(Clone)]
pub struct RuntimeRegistry {
    runtimes: Arc<RwLock<Vec<Arc<dyn WorkspaceWorkerRuntime>>>>,
}

impl RuntimeRegistry {
    pub fn new(runtimes: Vec<Arc<dyn WorkspaceWorkerRuntime>>) -> Self {
        Self {
            runtimes: Arc::new(RwLock::new(runtimes)),
        }
    }

    pub fn for_workspace(embedded_runtime: EmbeddedWorkerRuntime) -> Self {
        Self::new(vec![Arc::new(embedded_runtime)])
    }

    pub fn register<R>(&self, runtime: R)
    where
        R: WorkspaceWorkerRuntime + 'static,
    {
        self.runtimes
            .write()
            .expect("runtime registry lock poisoned")
            .push(Arc::new(runtime));
    }

    pub fn register_or_replace<R>(&self, runtime: R)
    where
        R: WorkspaceWorkerRuntime + 'static,
    {
        let runtime = Arc::new(runtime);
        let runtime_id = runtime.runtime_id().to_string();
        let mut runtimes = self
            .runtimes
            .write()
            .expect("runtime registry lock poisoned");
        runtimes.retain(|existing| existing.runtime_id() != runtime_id);
        runtimes.push(runtime);
    }

    pub fn unregister_if_idle(
        &self,
        runtime_id: &str,
        worker_scan_limit: usize,
    ) -> Result<RuntimeRegistryUnregisterResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self
            .runtimes_snapshot()
            .into_iter()
            .find(|runtime| runtime.runtime_id() == runtime_id);
        if let Some(runtime) = runtime {
            let worker_list = runtime.list_workers(worker_scan_limit);
            if !worker_list.items.is_empty() {
                return Ok(RuntimeRegistryUnregisterResult::BlockedByWorkers {
                    worker_count: worker_list.items.len(),
                    diagnostics: worker_list.diagnostics,
                });
            }
        } else {
            return Ok(RuntimeRegistryUnregisterResult::NotFound);
        }

        let mut runtimes = self
            .runtimes
            .write()
            .expect("runtime registry lock poisoned");
        let before = runtimes.len();
        runtimes.retain(|runtime| runtime.runtime_id() != runtime_id);
        if runtimes.len() == before {
            Ok(RuntimeRegistryUnregisterResult::NotFound)
        } else {
            Ok(RuntimeRegistryUnregisterResult::Removed)
        }
    }

    pub fn list_runtimes(&self, limit: usize) -> RuntimeList<RuntimeSummary> {
        let mut diagnostics = Vec::new();
        let mut items = Vec::new();
        for runtime in self.runtimes_snapshot().iter().take(limit) {
            let summary = runtime.runtime_summary(limit);
            diagnostics.extend(summary.diagnostics.iter().cloned());
            items.push(summary);
        }
        diagnostics.truncate(MAX_DIAGNOSTICS);
        RuntimeList::new(items, diagnostics)
    }

    pub fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary> {
        let mut items = Vec::new();
        let mut diagnostics = Vec::new();
        for runtime in self.runtimes_snapshot() {
            if items.len() >= limit {
                break;
            }
            let mut list = runtime.list_hosts(limit.saturating_sub(items.len()));
            diagnostics.append(&mut list.diagnostics);
            items.append(&mut list.items);
        }
        diagnostics.truncate(MAX_DIAGNOSTICS);
        RuntimeList::new(items, diagnostics)
    }

    pub fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        let mut items = Vec::new();
        let mut diagnostics = Vec::new();
        for runtime in self.runtimes_snapshot() {
            if items.len() >= limit {
                break;
            }
            let mut list = runtime.list_workers(limit.saturating_sub(items.len()));
            diagnostics.append(&mut list.diagnostics);
            items.extend(
                list.items
                    .into_iter()
                    .take(limit.saturating_sub(items.len())),
            );
        }
        diagnostics.truncate(MAX_DIAGNOSTICS);
        RuntimeList::new(items, diagnostics)
    }

    pub fn list_workers_for_runtime(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> Result<RuntimeList<WorkerSummary>, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        let worker_list = runtime.list_workers(limit);
        let mut items: Vec<_> = worker_list.items.into_iter().take(limit).collect();
        items.truncate(limit);
        let mut diagnostics = worker_list.diagnostics;
        diagnostics.truncate(MAX_DIAGNOSTICS);
        Ok(RuntimeList::new(items, diagnostics))
    }

    pub fn list_stopped_workers_for_runtime(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> Result<RuntimeList<WorkerSummary>, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        let worker_list = runtime.list_stopped_workers(limit);
        let mut items: Vec<_> = worker_list.items.into_iter().take(limit).collect();
        items.truncate(limit);
        let mut diagnostics = worker_list.diagnostics;
        diagnostics.truncate(MAX_DIAGNOSTICS);
        Ok(RuntimeList::new(items, diagnostics))
    }

    pub fn list_workers_for_host(
        &self,
        host_id: &str,
        limit: usize,
    ) -> Result<RuntimeList<WorkerSummary>, RuntimeRegistryError> {
        validate_backend_identifier("host_id", host_id)?;

        let mut host_found = false;
        let mut diagnostics = Vec::new();
        let mut items = Vec::new();
        for runtime in self.runtimes_snapshot() {
            let host_list = runtime.list_hosts(MAX_HOST_SCAN);
            diagnostics.extend(host_list.diagnostics);
            if !host_list.items.iter().any(|host| host.host_id == host_id) {
                continue;
            }
            host_found = true;
            let worker_list = runtime.list_workers(limit);
            diagnostics.extend(worker_list.diagnostics);
            items.extend(
                worker_list
                    .items
                    .into_iter()
                    .filter(|worker| worker.host_id == host_id)
                    .take(limit.saturating_sub(items.len())),
            );
            if items.len() >= limit {
                break;
            }
        }
        diagnostics.truncate(MAX_DIAGNOSTICS);
        if host_found {
            Ok(RuntimeList::new(items, diagnostics))
        } else {
            Err(RuntimeRegistryError::UnknownHost(host_id.to_string()))
        }
    }

    pub fn worker(&self, worker: &RuntimeWorkerRef) -> Result<WorkerSummary, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        let worker = lookup.worker.ok_or_else(|| {
            operation_failed_or_unknown_worker(runtime_id, worker_id, lookup.diagnostics)
        })?;
        Ok(worker)
    }

    pub fn restore_worker(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> Result<WorkerRestoreResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.restore_worker(worker_id))
    }

    pub fn replace_worker_workspace_api(
        &self,
        worker: &RuntimeWorkerRef,
        workspace_api: WorkspaceApiRef,
    ) -> Result<WorkerWorkspaceApiResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.replace_worker_workspace_api(worker_id, workspace_api))
    }

    pub fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Vec<RuntimeDiagnostic> {
        let runtimes = self
            .runtimes
            .read()
            .map(|runtimes| runtimes.clone())
            .unwrap_or_default();
        runtimes
            .into_iter()
            .filter_map(|runtime| {
                runtime
                    .observe_workspace_prompt_projection(projection.clone())
                    .err()
                    .map(|message| {
                        diagnostic(
                            "workspace_prompt_projection_notification_failed",
                            DiagnosticSeverity::Warning,
                            format!(
                                "runtime '{}' rejected Workspace Prompt projection revision {}: {message}",
                                runtime.runtime_id(), projection.config_revision
                            ),
                        )
                    })
            })
            .take(MAX_DIAGNOSTICS)
            .collect()
    }

    pub fn spawn_worker(
        &self,
        runtime_id: &str,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> Result<WorkerSpawnResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        match request.acceptance {
            WorkerSpawnAcceptanceRequirement::RunAccepted { expected_segments }
                if expected_segments != request.initial_submit.len() =>
            {
                return Err(RuntimeRegistryError::RuntimeOperationFailed {
                    runtime_id: runtime_id.to_string(),
                    code: "worker_initial_segment_count_mismatch".to_string(),
                    message: format!(
                        "spawn acceptance expects {expected_segments} initial segment(s), request carries {}",
                        request.initial_submit.len()
                    ),
                });
            }
            WorkerSpawnAcceptanceRequirement::SocketReady if !request.initial_submit.is_empty() => {
                return Err(RuntimeRegistryError::RuntimeOperationFailed {
                    runtime_id: runtime_id.to_string(),
                    code: "worker_initial_submit_require_run_acceptance".to_string(),
                    message:
                        "spawn requests with initial segments must require RunAccepted acceptance"
                            .to_string(),
                });
            }
            _ => {}
        }
        let runtime = self.runtime(runtime_id)?;
        if let Some(bundle) = request.resolved_config_bundle.clone() {
            let sync = runtime.sync_config_bundle(bundle);
            if sync.state != WorkerOperationState::Accepted {
                let message = sync
                    .diagnostics
                    .first()
                    .map(|diagnostic| diagnostic.message.clone())
                    .unwrap_or_else(|| "Runtime rejected the resolved config bundle".to_string());
                return Err(RuntimeRegistryError::RuntimeOperationFailed {
                    runtime_id: runtime_id.to_string(),
                    code: "worker_config_bundle_sync_rejected".to_string(),
                    message,
                });
            }
        }
        Ok(runtime.spawn_worker(binding, request))
    }

    pub fn create_working_directory(
        &self,
        runtime_id: &str,
        request: WorkingDirectoryRequest,
    ) -> Result<RuntimeWorkingDirectoryResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.create_working_directory(request))
    }

    pub fn authorize_working_directory_repository_access(
        &self,
        runtime_id: &str,
        request: WorkingDirectoryRepositoryAccessRequest,
    ) -> Result<(), RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("working_directory_id", &request.working_directory_id)?;
        let runtime = self.runtime(runtime_id)?;
        runtime
            .authorize_working_directory_repository_access(request)
            .map_err(|error| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "working_directory_repository_access_failed".to_string(),
                message: error.to_string(),
            })
    }

    pub fn list_working_directories(
        &self,
        runtime_id: &str,
    ) -> Result<RuntimeList<WorkingDirectoryStatus>, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.list_working_directories())
    }

    pub fn working_directory(
        &self,
        runtime_id: &str,
        working_directory_id: &str,
    ) -> Result<RuntimeWorkingDirectoryResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("working_directory_id", working_directory_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.working_directory(working_directory_id))
    }

    pub async fn open_workdir_session(
        &self,
        runtime_id: &str,
        working_directory_id: &str,
        owner_worker_id: Option<&str>,
    ) -> Result<WorkdirSessionHandle, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("working_directory_id", working_directory_id)?;
        if let Some(owner_worker_id) = owner_worker_id {
            validate_backend_identifier("owner_worker_id", owner_worker_id)?;
        }
        let runtime = self.runtime(runtime_id)?;
        runtime
            .open_workdir_session(working_directory_id, owner_worker_id)
            .await
            .map_err(|error| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workdir_session_open_failed".to_string(),
                message: error.to_string(),
            })
    }

    pub fn cleanup_working_directory(
        &self,
        runtime_id: &str,
        working_directory_id: &str,
    ) -> Result<RuntimeWorkingDirectoryResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("working_directory_id", working_directory_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.cleanup_working_directory(working_directory_id))
    }

    pub fn sync_config_bundle(
        &self,
        runtime_id: &str,
        bundle: ConfigBundle,
    ) -> Result<ConfigBundleSyncResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.sync_config_bundle(bundle))
    }

    pub fn check_config_bundle(
        &self,
        runtime_id: &str,
        reference: ConfigBundleRef,
    ) -> Result<ConfigBundleCheckResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.check_config_bundle(reference))
    }

    pub fn list_config_bundles(
        &self,
        runtime_id: &str,
    ) -> Result<ConfigBundleListResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.list_config_bundles())
    }

    pub fn send_protocol_method(
        &self,
        worker: &RuntimeWorkerRef,
        method: protocol::Method,
    ) -> Result<Vec<protocol::Event>, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        runtime.send_protocol_method(worker_id, method)
    }

    pub fn send_input(
        &self,
        worker: &RuntimeWorkerRef,
        request: WorkerInputRequest,
    ) -> Result<WorkerInputResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        Ok(runtime.send_input(worker_id, request))
    }

    pub fn worker_completions(
        &self,
        worker: &RuntimeWorkerRef,
        request: WorkerCompletionsRequest,
    ) -> Result<WorkerCompletionsResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        Ok(runtime.worker_completions(worker_id, request))
    }

    pub fn stop_worker(
        &self,
        worker: &RuntimeWorkerRef,
        request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        Ok(runtime.stop_worker(worker_id, request))
    }

    pub fn cancel_worker(
        &self,
        worker: &RuntimeWorkerRef,
        request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        Ok(runtime.cancel_worker(worker_id, request))
    }

    pub fn delete_worker(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> Result<WorkerDeleteResult, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        if lookup.worker.is_none() {
            return Err(operation_failed_or_unknown_worker(
                runtime_id,
                worker_id,
                lookup.diagnostics,
            ));
        }
        Ok(runtime.delete_worker(worker_id))
    }

    pub fn worker_retention_inventory(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> Result<WorkerRetentionInventory, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", &worker.runtime_id)?;
        validate_backend_identifier("worker_id", &worker.worker_id)?;
        self.runtime(&worker.runtime_id)?
            .worker_retention_inventory(&worker.worker_id)
            .map_err(|message| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: "worker_retention_inventory_failed".to_string(),
                message,
            })
    }

    pub fn execute_worker_retention(
        &self,
        worker: &RuntimeWorkerRef,
        request: WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", &worker.runtime_id)?;
        validate_backend_identifier("worker_id", &worker.worker_id)?;
        if request.worker_id.to_string() != worker.worker_id {
            return Err(RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: "worker_id_mismatch".to_string(),
                message: "retention request worker_id does not match target".to_string(),
            });
        }
        self.runtime(&worker.runtime_id)?
            .execute_worker_retention(request)
            .map_err(|message| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: "worker_retention_execution_failed".to_string(),
                message,
            })
    }

    pub fn observation_source(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> Result<crate::observation::RuntimeObservationSource, RuntimeRegistryError> {
        let runtime_id = worker.runtime_id.as_str();
        let worker_id = worker.worker_id.as_str();
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        runtime
            .observation_source(worker_id)
            .ok_or_else(|| RuntimeRegistryError::UnknownWorker {
                worker: worker.clone(),
            })
    }

    fn runtimes_snapshot(&self) -> Vec<Arc<dyn WorkspaceWorkerRuntime>> {
        self.runtimes
            .read()
            .expect("runtime registry lock poisoned")
            .clone()
    }

    fn runtime(
        &self,
        runtime_id: &str,
    ) -> Result<Arc<dyn WorkspaceWorkerRuntime>, RuntimeRegistryError> {
        self.runtimes
            .read()
            .expect("runtime registry lock poisoned")
            .iter()
            .find(|runtime| runtime.runtime_id() == runtime_id)
            .cloned()
            .ok_or_else(|| RuntimeRegistryError::UnknownRuntime(runtime_id.to_string()))
    }
}

#[derive(Clone)]
pub struct EmbeddedWorkerRuntime {
    workspace_id: String,
    runtime_id: String,
    host_id: String,
    runtime: worker_runtime::Runtime,
    execution_enabled: bool,
    resource_broker: BackendResourceBroker,
}

fn embedded_runtime_options() -> EmbeddedRuntimeOptions {
    EmbeddedRuntimeOptions {
        display_name: Some("embedded".to_string()),
        ..EmbeddedRuntimeOptions::default()
    }
}

impl EmbeddedWorkerRuntime {
    pub fn new_memory(workspace_id: impl AsRef<str>) -> Self {
        let runtime = worker_runtime::Runtime::with_options(embedded_runtime_options());
        Self::from_runtime(workspace_id, runtime)
    }

    pub fn new_memory_with_execution_backend(
        workspace_id: impl AsRef<str>,
        backend: std::sync::Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
    ) -> Result<Self, worker_runtime::error::RuntimeError> {
        let runtime =
            worker_runtime::Runtime::with_execution_backend(embedded_runtime_options(), backend)?;
        let mut embedded = Self::from_runtime(workspace_id, runtime);
        embedded.execution_enabled = true;
        Ok(embedded)
    }

    pub fn new_fs_store_with_execution_backend(
        workspace_id: impl AsRef<str>,
        store_root: impl Into<PathBuf>,
        backend: std::sync::Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
    ) -> Result<Self, worker_runtime::error::RuntimeError> {
        let runtime = worker_runtime::Runtime::with_fs_store_and_execution_backend(
            FsRuntimeStoreOptions {
                root: store_root.into(),
                runtime_id: EMBEDDED_RUNTIME_ID.to_string(),
                display_name: Some("embedded".to_string()),
            },
            backend,
        )?;
        let mut embedded = Self::from_runtime(workspace_id, runtime);
        embedded.execution_enabled = true;
        Ok(embedded)
    }

    pub fn with_resource_broker(mut self, resource_broker: BackendResourceBroker) -> Self {
        self.resource_broker = resource_broker;
        self
    }

    pub(crate) fn subscription_runtime(&self) -> worker_runtime::Runtime {
        self.runtime.clone()
    }

    pub fn from_runtime(workspace_id: impl AsRef<str>, runtime: worker_runtime::Runtime) -> Self {
        let workspace_id = workspace_id.as_ref().to_string();
        runtime
            .bind_runtime_identity(EMBEDDED_RUNTIME_ID)
            .expect("fresh embedded Runtime must accept its Backend-owned identity");
        Self {
            host_id: host_id_for_embedded_workspace(&workspace_id),
            workspace_id,
            runtime_id: EMBEDDED_RUNTIME_ID.to_string(),
            runtime,
            execution_enabled: false,
            resource_broker: BackendResourceBroker::default(),
        }
    }

    fn worker_ref(&self, worker_id: &str) -> Option<EmbeddedWorkerRef> {
        Some(EmbeddedWorkerRef::new(EmbeddedWorkerId::parse(worker_id)?))
    }

    fn can_stop_embedded_worker(&self, status: EmbeddedWorkerStatus) -> bool {
        runtime_worker_can_stop(self.execution_enabled, status)
    }

    fn map_worker_summary(&self, summary: worker_runtime::catalog::WorkerSummary) -> WorkerSummary {
        let worker_id = summary.worker_ref.worker_id.to_string();
        let profile = embedded_profile_label(&summary.profile);
        let display = worker_display_metadata(
            &worker_id,
            profile.as_deref(),
            summary.display_name.as_deref(),
            true,
        );
        WorkerSummary {
            worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id.clone()),
            host_id: self.host_id.clone(),
            display_name: display.display_name.clone(),
            label: display.display_name,
            profile,
            singleton_key: display.singleton_key,
            tags: display.tags,
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
                workspace_id: summary.workspace_id.clone(),
            },
            state: embedded_worker_status_label(summary.status).to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: self.can_stop_embedded_worker(summary.status),
                can_spawn_followup: false,
            },
            working_directory: summary.working_directory.map(|status| status.summary),
            diagnostics: embedded_worker_projection_diagnostics(),
        }
    }

    fn map_worker_detail(&self, detail: EmbeddedWorkerDetail) -> WorkerSummary {
        let worker_id = detail.worker_id.to_string();
        let profile = embedded_profile_label(&detail.profile);
        let display = worker_display_metadata(
            &worker_id,
            profile.as_deref(),
            detail.display_name.as_deref(),
            true,
        );
        WorkerSummary {
            worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id.clone()),
            host_id: self.host_id.clone(),
            display_name: display.display_name.clone(),
            label: display.display_name,
            profile,
            singleton_key: display.singleton_key,
            tags: display.tags,
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
                workspace_id: detail.workspace_id.clone(),
            },
            state: embedded_worker_status_label(detail.status).to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: self.can_stop_embedded_worker(detail.status),
                can_spawn_followup: false,
            },
            working_directory: detail.working_directory.map(|status| status.summary),
            diagnostics: embedded_worker_projection_diagnostics(),
        }
    }
}

impl WorkspaceWorkerRuntime for EmbeddedWorkerRuntime {
    fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    fn runtime_summary(&self, limit: usize) -> RuntimeSummary {
        let mut diagnostics = Vec::new();
        let summary = match self.runtime.summary() {
            Ok(summary) => summary,
            Err(err) => {
                diagnostics.push(embedded_runtime_diagnostic(&err));
                return RuntimeSummary {
                    runtime_id: self.runtime_id.clone(),
                    label: "Embedded backend Runtime".to_string(),
                    kind: "embedded_worker_runtime".to_string(),
                    status: "unavailable".to_string(),
                    source: RuntimeSourceSummary::embedded_worker_runtime(),
                    host_ids: Vec::new(),
                    worker_creation_available: false,
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                    diagnostics,
                };
            }
        };

        RuntimeSummary {
            runtime_id: self.runtime_id.clone(),
            label: summary
                .display_name
                .clone()
                .unwrap_or_else(|| "Embedded backend Runtime".to_string()),
            kind: "embedded_worker_runtime".to_string(),
            status: embedded_runtime_status_label(summary.status).to_string(),
            source: RuntimeSourceSummary::embedded_worker_runtime(),
            host_ids: if limit == 0 {
                Vec::new()
            } else {
                vec![self.host_id.clone()]
            },
            worker_creation_available: true,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            diagnostics,
        }
    }

    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        RuntimeList::new(
            vec![HostSummary {
                runtime_id: self.runtime_id.clone(),
                host_id: self.host_id.clone(),
                label: "embedded".to_string(),
                kind: EMBEDDED_HOST_KIND.to_string(),
                status: "available".to_string(),
                observed_at: Utc::now().to_rfc3339(),
                last_seen_at: None,
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                diagnostics: vec![diagnostic(
                    "embedded_runtime_host_boundary",
                    DiagnosticSeverity::Info,
                    "Backend-internal host exposes only bounded runtime and worker projections"
                        .to_string(),
                )],
            }],
            Vec::new(),
        )
    }

    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        match self.runtime.list_workers() {
            Ok(workers) => RuntimeList::new(
                workers
                    .into_iter()
                    .take(limit)
                    .map(|worker| self.map_worker_summary(worker))
                    .collect(),
                Vec::new(),
            ),
            Err(err) => RuntimeList::new(Vec::new(), vec![embedded_runtime_diagnostic(&err)]),
        }
    }

    fn list_stopped_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        match self.runtime.list_stopped_workers() {
            Ok(workers) => RuntimeList::new(
                workers
                    .into_iter()
                    .take(limit)
                    .map(|worker| self.map_worker_summary(worker))
                    .collect(),
                Vec::new(),
            ),
            Err(err) => RuntimeList::new(Vec::new(), vec![embedded_runtime_diagnostic(&err)]),
        }
    }

    fn worker(&self, worker_id: &str) -> WorkerLookupResult {
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return WorkerLookupResult {
                worker: None,
                diagnostics: vec![diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                )],
            };
        };
        match self.runtime.worker_detail(&worker_ref) {
            Ok(detail) => WorkerLookupResult {
                worker: Some(self.map_worker_detail(detail)),
                diagnostics: Vec::new(),
            },
            Err(EmbeddedRuntimeError::WorkerNotFound { .. }) => WorkerLookupResult {
                worker: None,
                diagnostics: Vec::new(),
            },
            Err(err) => WorkerLookupResult {
                worker: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn restore_worker(&self, worker_id: &str) -> WorkerRestoreResult {
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return WorkerRestoreResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be restored".to_string(),
                )],
            };
        };
        match self.runtime.restore_worker(&worker_ref) {
            Ok(detail) => WorkerRestoreResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(detail)),
                diagnostics: Vec::new(),
            },
            Err(err) => WorkerRestoreResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn replace_worker_workspace_api(
        &self,
        worker_id: &str,
        workspace_api: WorkspaceApiRef,
    ) -> WorkerWorkspaceApiResult {
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return WorkerWorkspaceApiResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot receive Workspace access".to_string(),
                )],
            };
        };
        match self
            .runtime
            .replace_worker_workspace_api(&worker_ref, workspace_api)
        {
            Ok(detail) => WorkerWorkspaceApiResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(detail)),
                diagnostics: Vec::new(),
            },
            Err(err) => WorkerWorkspaceApiResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn create_working_directory(
        &self,
        _request: WorkingDirectoryRequest,
    ) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Rejected,
            working_directory: None,
            diagnostics: vec![embedded_workdir_unsupported_diagnostic()],
        }
    }

    fn list_working_directories(&self) -> RuntimeList<WorkingDirectoryStatus> {
        RuntimeList::new(Vec::new(), Vec::new())
    }

    fn working_directory(&self, _working_directory_id: &str) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Rejected,
            working_directory: None,
            diagnostics: vec![embedded_workdir_unsupported_diagnostic()],
        }
    }

    fn cleanup_working_directory(
        &self,
        _working_directory_id: &str,
    ) -> RuntimeWorkingDirectoryResult {
        RuntimeWorkingDirectoryResult {
            state: WorkerOperationState::Rejected,
            working_directory: None,
            diagnostics: vec![embedded_workdir_unsupported_diagnostic()],
        }
    }

    fn spawn_worker(
        &self,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        let mut diagnostics = Vec::new();
        if request.resolved_working_directory_request.is_some()
            || request.resolved_working_directory.is_some()
        {
            diagnostics.push(embedded_workdir_unsupported_diagnostic());
            return WorkerSpawnResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                acceptance_evidence: Vec::new(),
                diagnostics,
            };
        }
        if matches!(
            request.acceptance,
            WorkerSpawnAcceptanceRequirement::SocketReady
        ) {
            diagnostics.push(diagnostic(
                "embedded_runtime_no_socket",
                DiagnosticSeverity::Warning,
                "Embedded backend Runtime is transportless; use run_accepted/create acceptance for backend-internal Workers".to_string(),
            ));
            return WorkerSpawnResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                acceptance_evidence: Vec::new(),
                diagnostics,
            };
        }
        if request.requested_worker_name.is_some() {
            diagnostics.push(diagnostic(
                "embedded_worker_name_display_only",
                DiagnosticSeverity::Info,
                "requested_worker_name is used only as display_name; Worker identity is allocated by Workspace authority".to_string(),
            ));
        }
        if matches!(request.acceptance, WorkerSpawnAcceptanceRequirement::RunAccepted { expected_segments } if expected_segments > 0)
        {
            diagnostics.push(diagnostic(
                "embedded_runtime_acceptance_projection",
                DiagnosticSeverity::Info,
                "Embedded Runtime accepts creation through a runtime execution backend; provider segment counts are observed after execution, not faked at create time".to_string(),
            ));
        }

        let profile = request.profile.clone();
        let profile_source = match profile_source_archive_source(&request, &profile) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(diagnostic(
                    "embedded_profile_source_archive_invalid",
                    DiagnosticSeverity::Error,
                    error,
                ));
                return WorkerSpawnResult {
                    state: WorkerOperationState::Rejected,
                    worker: None,
                    acceptance_evidence: Vec::new(),
                    diagnostics,
                };
            }
        };
        let workspace_api = match required_worker_workspace_api(&request) {
            Ok(workspace_api) => workspace_api,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return WorkerSpawnResult {
                    state: WorkerOperationState::Rejected,
                    worker: None,
                    acceptance_evidence: Vec::new(),
                    diagnostics,
                };
            }
        };
        let workspace_id = workspace_api.workspace_id.clone();
        let config_bundle = spawn_config_bundle_ref(&request);
        let create_request = CreateWorkerRequest {
            worker_id: binding.worker_id,
            create_fingerprint: binding.create_fingerprint,
            profile,
            display_name: request.requested_worker_name.clone(),
            config_bundle,
            profile_source,
            initial_input: initial_worker_input(&request.initial_submit),
            working_directory_request: request.resolved_working_directory_request.clone(),
            working_directory: request.resolved_working_directory.clone(),
            worker_observation_enabled: request.resolved_worker_observation_enabled,
            worker_observation_grants: request.resolved_worker_observation_grants.clone(),
            workspace_api: Some(workspace_api),
            memory_settings: request.resolved_memory_settings.clone(),
        };
        let workspace_scope = RuntimeWorkspaceScope::new(workspace_id, "embedded-backend");
        match self
            .runtime
            .create_worker_scoped(&workspace_scope, create_request)
        {
            Ok(detail) => WorkerSpawnResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(detail)),
                acceptance_evidence: vec![
                    WorkerSpawnAcceptanceEvidence {
                        kind: "embedded_runtime_worker_created".to_string(),
                        detail:
                            "worker-runtime catalog accepted a backend-internal tools-less Worker"
                                .to_string(),
                    },
                    WorkerSpawnAcceptanceEvidence {
                        kind: "embedded_runtime_backend_internal_projection".to_string(),
                        detail: "only runtime_id plus worker_id backend projections were exposed"
                            .to_string(),
                    },
                ],
                diagnostics,
            },
            Err(err) => {
                diagnostics.push(embedded_runtime_diagnostic(&err));
                WorkerSpawnResult {
                    state: WorkerOperationState::Rejected,
                    worker: None,
                    acceptance_evidence: Vec::new(),
                    diagnostics,
                }
            }
        }
    }

    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        self.runtime
            .observe_workspace_prompt_projection(projection)
            .map_err(|error| error.to_string())
    }

    fn sync_config_bundle(&self, bundle: ConfigBundle) -> ConfigBundleSyncResult {
        match self.runtime.store_config_bundle(bundle) {
            Ok(availability) => ConfigBundleSyncResult {
                state: WorkerOperationState::Accepted,
                availability: Some(availability),
                diagnostics: Vec::new(),
            },
            Err(error) => ConfigBundleSyncResult {
                state: WorkerOperationState::Rejected,
                availability: None,
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
    }

    fn check_config_bundle(&self, reference: ConfigBundleRef) -> ConfigBundleCheckResult {
        match self.runtime.check_config_bundle(&reference) {
            Ok(availability) => ConfigBundleCheckResult {
                state: WorkerOperationState::Accepted,
                availability: Some(availability),
                diagnostics: Vec::new(),
            },
            Err(error) => ConfigBundleCheckResult {
                state: WorkerOperationState::Rejected,
                availability: None,
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
    }

    fn list_config_bundles(&self) -> ConfigBundleListResult {
        match self.runtime.list_config_bundles() {
            Ok(bundles) => ConfigBundleListResult {
                bundles,
                diagnostics: Vec::new(),
            },
            Err(error) => ConfigBundleListResult {
                bundles: Vec::new(),
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
    }

    fn stop_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        if !self.execution_enabled {
            return embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_execution_unavailable",
                    DiagnosticSeverity::Info,
                    format!("worker stop for '{worker_id}' requires an embedded execution backend"),
                ),
            );
        }
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                ),
            );
        };
        match self.runtime.stop_worker(&worker_ref, request.reason) {
            Ok(_) => WorkerLifecycleResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                diagnostics: Vec::new(),
            },
            Err(error) => embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                embedded_runtime_diagnostic(&error),
            ),
        }
    }

    fn cancel_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        if !self.execution_enabled {
            return embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_execution_unavailable",
                    DiagnosticSeverity::Info,
                    format!(
                        "worker cancel for '{worker_id}' requires an embedded execution backend"
                    ),
                ),
            );
        }
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                ),
            );
        };
        match self.runtime.cancel_worker(&worker_ref, request.reason) {
            Ok(_) => WorkerLifecycleResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                diagnostics: Vec::new(),
            },
            Err(error) => embedded_lifecycle_rejected(
                &self.runtime_id,
                worker_id,
                embedded_runtime_diagnostic(&error),
            ),
        }
    }

    fn delete_worker(&self, worker_id: &str) -> WorkerDeleteResult {
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return WorkerDeleteResult {
                state: WorkerOperationState::Rejected,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                deleted: false,
                diagnostics: vec![diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                )],
            };
        };
        match self.runtime.delete_worker(&worker_ref) {
            Ok(result) => WorkerDeleteResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(
                    self.runtime_id.clone(),
                    result.worker_id.to_string(),
                ),
                deleted: result.deleted,
                diagnostics: Vec::new(),
            },
            Err(error) => WorkerDeleteResult {
                state: WorkerOperationState::Rejected,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                deleted: false,
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
    }

    fn worker_retention_inventory(
        &self,
        worker_id: &str,
    ) -> Result<WorkerRetentionInventory, String> {
        let worker_ref = self
            .worker_ref(worker_id)
            .ok_or_else(|| format!("invalid embedded Worker id '{worker_id}'"))?;
        self.runtime
            .worker_retention_inventory(&self.workspace_id, &worker_ref)
            .map_err(|error| error.to_string())
    }

    fn execute_worker_retention(
        &self,
        request: WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, String> {
        if request.workspace_id != self.workspace_id {
            return Err("retention request Workspace does not match embedded Runtime".to_string());
        }
        self.runtime
            .execute_worker_retention(&request)
            .map_err(|error| error.to_string())
    }

    fn observation_source(
        &self,
        worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSource> {
        let worker_ref = self.worker_ref(worker_id)?;
        if self.runtime.worker_detail(&worker_ref).is_err() {
            return None;
        }
        Some(crate::observation::RuntimeObservationSource::embedded(
            crate::observation::EmbeddedRuntimeObservationSource {
                worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id),
                runtime: self.runtime.clone(),
                worker_ref,
            },
        ))
    }

    fn send_protocol_method(
        &self,
        worker_id: &str,
        method: protocol::Method,
    ) -> Result<Vec<protocol::Event>, RuntimeRegistryError> {
        if !self.execution_enabled {
            return Err(RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: self.runtime_id.clone(),
                code: "embedded_worker_execution_unavailable".to_string(),
                message: format!(
                    "worker protocol command for '{worker_id}' requires an embedded execution backend"
                ),
            });
        }
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return Err(RuntimeRegistryError::UnknownWorker {
                worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id),
            });
        };
        self.runtime
            .send_protocol_method(&worker_ref, method)
            .map_err(|error| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: self.runtime_id.clone(),
                code: "embedded_worker_protocol_command_failed".to_string(),
                message: error.to_string(),
            })
    }

    fn send_input(&self, worker_id: &str, request: WorkerInputRequest) -> WorkerInputResult {
        if !self.execution_enabled {
            return embedded_input_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_execution_unavailable",
                    DiagnosticSeverity::Info,
                    format!(
                        "worker input for '{worker_id}' requires an embedded execution backend"
                    ),
                ),
            );
        }
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return embedded_input_rejected(
                &self.runtime_id,
                worker_id,
                diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                ),
            );
        };
        let input = EmbeddedWorkerInput {
            kind: match request.kind {
                WorkerInputKind::User => EmbeddedWorkerInputKind::User,
                WorkerInputKind::Notify => EmbeddedWorkerInputKind::Notify,
                WorkerInputKind::Compact => EmbeddedWorkerInputKind::Compact,
                WorkerInputKind::ListRewindTargets => EmbeddedWorkerInputKind::ListRewindTargets,
                WorkerInputKind::RegisterPeer => EmbeddedWorkerInputKind::RegisterPeer,
            },
            content: request.content,
            submission_id: None,
            segments: request.segments,
        };
        match self.runtime.send_input(&worker_ref, input) {
            Ok(_) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                diagnostics: Vec::new(),
            },
            Err(error) => embedded_input_rejected(
                &self.runtime_id,
                worker_id,
                embedded_runtime_diagnostic(&error),
            ),
        }
    }

    fn worker_completions(
        &self,
        worker_id: &str,
        request: WorkerCompletionsRequest,
    ) -> WorkerCompletionsResult {
        if !self.execution_enabled {
            return WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: request.kind,
                prefix: request.prefix,
                entries: Vec::new(),
                diagnostics: vec![diagnostic(
                    "embedded_worker_execution_unavailable",
                    DiagnosticSeverity::Info,
                    format!(
                        "worker completions for '{worker_id}' require an embedded execution backend"
                    ),
                )],
            };
        }
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: request.kind,
                prefix: request.prefix,
                entries: Vec::new(),
                diagnostics: vec![diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                )],
            };
        };
        match self
            .runtime
            .worker_completions(&worker_ref, request.kind, &request.prefix)
        {
            Ok(entries) => WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: request.kind,
                prefix: request.prefix,
                entries,
                diagnostics: Vec::new(),
            },
            Err(error) => WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: request.kind,
                prefix: request.prefix,
                entries: Vec::new(),
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
    }
}

#[derive(Clone)]
pub struct RemoteRuntimeConfig {
    pub runtime_id: String,
    /// Explicit Workspace assignment granted by Server authority.
    pub workspace_id: Option<String>,
    pub display_name: String,
    pub base_url: String,
    pub bearer_token: Option<String>,
    pub auth: Option<RemoteRuntimeAuthConfig>,
    pub cached_worker_creation_available: bool,
    pub cached_os: String,
    pub cached_arch: String,
    pub cached_status: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRuntimeAuthConfig {
    pub server_id: String,
    pub server_private_key: String,
}

impl std::fmt::Debug for RemoteRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteRuntimeConfig")
            .field("runtime_id", &self.runtime_id)
            .field("display_name", &self.display_name)
            .field("base_url", &"<backend-private>")
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("auth", &self.auth.as_ref().map(|_| "<capability-signer>"))
            .field(
                "cached_worker_creation_available",
                &self.cached_worker_creation_available,
            )
            .field("cached_os", &self.cached_os)
            .field("cached_arch", &self.cached_arch)
            .field("cached_status", &self.cached_status)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl RemoteRuntimeConfig {
    pub fn new(
        runtime_id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: impl Into<String>,
        bearer_token: Option<String>,
    ) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            workspace_id: None,
            display_name: display_name.into(),
            base_url: base_url.into(),
            bearer_token,
            auth: None,
            cached_worker_creation_available: false,
            cached_os: "unknown".to_string(),
            cached_arch: "unknown".to_string(),
            cached_status: "configured".to_string(),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }

    pub fn with_auth(mut self, auth: RemoteRuntimeAuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn with_cached_status(mut self, status: impl Into<String>) -> Self {
        self.cached_status = status.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Clone)]
struct RemoteWorkdirAuthorization {
    runtime_id: String,
    workspace_id: String,
    auth: Option<RemoteRuntimeAuthConfig>,
    fallback_bearer_token: Option<String>,
}

impl std::fmt::Debug for RemoteWorkdirAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteWorkdirAuthorization")
            .field("runtime_id", &self.runtime_id)
            .field("workspace_id", &self.workspace_id)
            .field("auth", &self.auth.as_ref().map(|_| "capability_token"))
            .field(
                "fallback_bearer_token",
                &self.fallback_bearer_token.as_ref().map(|_| "configured"),
            )
            .finish()
    }
}

impl WorkdirHttpAuthorization for RemoteWorkdirAuthorization {
    fn bearer_token(&self) -> Result<String, WorkdirError> {
        if let Some(auth) = self.auth.as_ref() {
            let claims = capability_claims(
                &auth.server_id,
                &self.runtime_id,
                &self.workspace_id,
                all_remote_runtime_permissions(),
                300,
            )
            .map_err(|error| WorkdirError::Unavailable(error.to_string()))?;
            return CapabilityTokenSigner::new(&auth.server_id, &auth.server_private_key)
                .sign(&claims)
                .map_err(|error| WorkdirError::Unavailable(error.to_string()));
        }
        self.fallback_bearer_token.clone().ok_or_else(|| {
            WorkdirError::Unavailable(
                "remote Runtime does not have bearer authorization configured".to_string(),
            )
        })
    }
}

#[derive(Clone)]
pub struct RemoteWorkerRuntime {
    runtime_id: String,
    display_name: String,
    base_url: String,
    backend_base_url: String,
    workspace_id: String,
    bearer_token: Option<String>,
    auth: Option<RemoteRuntimeAuthConfig>,
    cached_worker_creation_available: bool,
    cached_os: String,
    cached_arch: String,
    cached_status: String,
    host_id: String,
    resource_broker: BackendResourceBroker,
    http: BlockingHttpClient,
    async_http: AsyncHttpClient,
}

fn all_remote_runtime_permissions() -> Vec<String> {
    [
        "workers:list",
        "workers:create",
        "workers:read",
        "workers:delete",
        "workers:input",
        "workers:stop",
        "workers:protocol",
        "workdirs:operate",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl RemoteWorkerRuntime {
    pub fn new(
        config: RemoteRuntimeConfig,
        workspace_id: String,
        backend_base_url: String,
    ) -> Result<Self, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", &config.runtime_id)?;
        let base_url = config.base_url.trim_end_matches('/').to_string();
        let timeout = config.timeout;
        let http =
            run_blocking_http(move || BlockingHttpClient::builder().timeout(timeout).build())
                .map_err(|err| RuntimeRegistryError::RuntimeOperationFailed {
                    runtime_id: config.runtime_id.clone(),
                    code: "remote_runtime_client_build_failed".to_string(),
                    message: err.to_string(),
                })?;
        // Workdir command-output waits are bounded to 20 seconds by Runtime;
        // leave transport margin while retaining a finite client timeout.
        let workdir_timeout = timeout.max(Duration::from_secs(30));
        let async_http = AsyncHttpClient::builder()
            .timeout(workdir_timeout)
            .build()
            .map_err(|err| RuntimeRegistryError::RuntimeOperationFailed {
                runtime_id: config.runtime_id.clone(),
                code: "remote_runtime_async_client_build_failed".to_string(),
                message: err.to_string(),
            })?;
        Ok(Self {
            host_id: host_id_for_remote_runtime(&config.runtime_id),
            runtime_id: config.runtime_id,
            display_name: config.display_name,
            base_url,
            backend_base_url: backend_base_url.trim_end_matches('/').to_string(),
            workspace_id,
            bearer_token: config.bearer_token,
            auth: config.auth,
            cached_worker_creation_available: config.cached_worker_creation_available,
            cached_os: config.cached_os,
            cached_arch: config.cached_arch,
            cached_status: config.cached_status,
            resource_broker: BackendResourceBroker::default(),
            http,
            async_http,
        })
    }

    pub fn with_resource_broker(mut self, resource_broker: BackendResourceBroker) -> Self {
        self.resource_broker = resource_broker;
        self
    }

    pub async fn open_workdir_session(
        &self,
        working_directory_id: &str,
        owner_worker_id: Option<&str>,
    ) -> Result<RemoteWorkdirSession, WorkdirError> {
        let base_url = Url::parse(&self.base_url)
            .map_err(|error| WorkdirError::InvalidArgument(error.to_string()))?;
        let workdir_id = Workdir::new(working_directory_id).id().clone();
        let authorization: Arc<dyn WorkdirHttpAuthorization> =
            Arc::new(RemoteWorkdirAuthorization {
                runtime_id: self.runtime_id.clone(),
                workspace_id: self.workspace_id.clone(),
                auth: self.auth.clone(),
                fallback_bearer_token: self.bearer_token.clone(),
            });
        RemoteWorkdirSession::open_with_authorization(
            self.async_http.clone(),
            base_url,
            authorization,
            workdir_id,
            OpenWorkdirSessionRequest {
                owner_worker_id: owner_worker_id.map(str::to_string),
            },
        )
        .await
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn bundle_availability_path(reference: &ConfigBundleRef) -> String {
        format!(
            "/v1/config-bundles/{}/availability?digest={}",
            url_path_segment_encode(&reference.id),
            url_query_value_encode(&reference.digest)
        )
    }

    fn ws_endpoint(&self, worker_id: &str) -> String {
        let mut base = self.base_url.clone();
        if let Some(rest) = base.strip_prefix("https://") {
            base = format!("wss://{rest}");
        } else if let Some(rest) = base.strip_prefix("http://") {
            base = format!("ws://{rest}");
        }
        format!("{base}/v1/workers/{worker_id}/protocol/ws")
    }

    fn get_json<T>(&self, path: &str) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(path, self.http.get(self.endpoint(path)))
    }

    fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T, RuntimeDiagnostic>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(path, self.http.post(self.endpoint(path)).json(body))
    }

    fn delete_json<T>(&self, path: &str) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(path, self.http.delete(self.endpoint(path)))
    }

    fn runtime_capability_token(&self, path: &str) -> Option<String> {
        let auth = self.auth.as_ref()?;
        let signer = CapabilityTokenSigner::new(&auth.server_id, &auth.server_private_key);
        let claims = capability_claims(
            &auth.server_id,
            &self.runtime_id,
            &self.workspace_id,
            all_remote_runtime_permissions(),
            300,
        )
        .map_err(|error| {
            eprintln!(
                "failed to build Runtime capability claims for {} {}: {error}",
                self.runtime_id, path
            );
            error
        })
        .ok()?;
        signer
            .sign(&claims)
            .map_err(|error| {
                eprintln!(
                    "failed to sign Runtime capability token for {} {}: {error}",
                    self.runtime_id, path
                );
                error
            })
            .ok()
    }

    fn send_json<T>(&self, path: &str, request: RequestBuilder) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let runtime_id = self.runtime_id.clone();
        let bearer_token = self.bearer_token.clone();
        let capability_token = self.runtime_capability_token(path);
        run_blocking_http(move || {
            let request = request.header(CONTENT_TYPE, "application/json");
            let request =
                if let Some(token) = capability_token.as_deref().or(bearer_token.as_deref()) {
                    request.header(AUTHORIZATION, format!("Bearer {token}"))
                } else {
                    request
                };
            let response = request
                .send()
                .map_err(|err| remote_reqwest_diagnostic(&runtime_id, err))?;
            let status = response.status();
            if status.is_success() {
                response.json::<T>().map_err(|err| {
                    diagnostic(
                        "remote_runtime_malformed_response",
                        DiagnosticSeverity::Error,
                        format!(
                            "Remote Runtime returned malformed JSON for '{}': {err}",
                            runtime_id
                        ),
                    )
                })
            } else {
                Err(remote_http_status_diagnostic(&runtime_id, status, response))
            }
        })
    }

    fn map_worker_summary(&self, summary: worker_runtime::catalog::WorkerSummary) -> WorkerSummary {
        let worker_id = summary.worker_ref.worker_id.to_string();
        let profile = embedded_profile_label(&summary.profile);
        let display = worker_display_metadata(
            &worker_id,
            profile.as_deref(),
            summary.display_name.as_deref(),
            false,
        );
        WorkerSummary {
            worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id.clone()),
            host_id: self.host_id.clone(),
            display_name: display.display_name.clone(),
            label: display.display_name,
            profile,
            singleton_key: display.singleton_key,
            tags: display.tags,
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
                workspace_id: summary.workspace_id.clone(),
            },
            state: embedded_worker_status_label(summary.status).to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: runtime_worker_can_stop(true, summary.status),
                can_spawn_followup: false,
            },
            working_directory: summary.working_directory.map(|status| status.summary),
            diagnostics: vec![diagnostic(
                "remote_runtime_projection",
                DiagnosticSeverity::Info,
                "Remote Worker identity is projected only as runtime_id plus worker_id; endpoint and credentials remain backend-private".to_string(),
            )],
        }
    }

    fn map_worker_detail(&self, detail: EmbeddedWorkerDetail) -> WorkerSummary {
        let worker_id = detail.worker_id.to_string();
        let profile = embedded_profile_label(&detail.profile);
        let display = worker_display_metadata(
            &worker_id,
            profile.as_deref(),
            detail.display_name.as_deref(),
            false,
        );
        WorkerSummary {
            worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id.clone()),
            host_id: self.host_id.clone(),
            display_name: display.display_name.clone(),
            label: display.display_name,
            profile,
            singleton_key: display.singleton_key,
            tags: display.tags,
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
                workspace_id: detail.workspace_id.clone(),
            },
            state: embedded_worker_status_label(detail.status).to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: runtime_worker_can_stop(true, detail.status),
                can_spawn_followup: false,
            },
            working_directory: detail.working_directory.map(|status| status.summary),
            diagnostics: vec![diagnostic(
                "remote_runtime_projection",
                DiagnosticSeverity::Info,
                "Remote Worker identity is projected only as runtime_id plus worker_id; endpoint and credentials remain backend-private".to_string(),
            )],
        }
    }

    fn lifecycle_result_from_response(
        &self,
        worker_id: &str,
        response: RuntimeHttpWorkerLifecycleResponse,
    ) -> WorkerLifecycleResult {
        WorkerLifecycleResult {
            state: WorkerOperationState::Accepted,
            worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
            diagnostics: vec![diagnostic(
                "remote_runtime_lifecycle_accepted",
                DiagnosticSeverity::Info,
                format!(
                    "Remote Runtime acknowledged lifecycle operation for '{worker_id}' with status {}",
                    embedded_worker_status_label(response.ack.status)
                ),
            )],
        }
    }
}

impl WorkspaceWorkerRuntime for RemoteWorkerRuntime {
    fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    fn runtime_summary(&self, limit: usize) -> RuntimeSummary {
        match self.get_json::<RuntimeHttpSummaryResponse>("/v1/runtime") {
            Ok(response) => RuntimeSummary {
                runtime_id: self.runtime_id.clone(),
                label: response
                    .runtime
                    .display_name
                    .unwrap_or_else(|| self.display_name.clone()),
                kind: "remote_worker_runtime".to_string(),
                status: embedded_runtime_status_label(response.runtime.status).to_string(),
                source: RuntimeSourceSummary::remote_http(),
                host_ids: if limit == 0 {
                    Vec::new()
                } else {
                    vec![self.host_id.clone()]
                },
                worker_creation_available: response.runtime.worker_creation_available,
                os: response.runtime.os,
                arch: response.runtime.arch,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => RuntimeSummary {
                runtime_id: self.runtime_id.clone(),
                label: self.display_name.clone(),
                kind: "remote_worker_runtime".to_string(),
                status: self.cached_status.clone(),
                source: RuntimeSourceSummary::remote_http(),
                host_ids: if limit == 0 {
                    Vec::new()
                } else {
                    vec![self.host_id.clone()]
                },
                worker_creation_available: self.cached_worker_creation_available,
                os: self.cached_os.clone(),
                arch: self.cached_arch.clone(),
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        RuntimeList::new(
            vec![HostSummary {
                runtime_id: self.runtime_id.clone(),
                host_id: self.host_id.clone(),
                label: self.display_name.clone(),
                kind: REMOTE_HOST_KIND.to_string(),
                status: "configured".to_string(),
                observed_at: Utc::now().to_rfc3339(),
                last_seen_at: None,
                os: self.cached_os.clone(),
                arch: self.cached_arch.clone(),
                diagnostics: Vec::new(),
            }],
            Vec::new(),
        )
    }

    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        match self.get_json::<RuntimeHttpWorkersResponse>("/v1/workers") {
            Ok(response) => RuntimeList::new(
                response
                    .workers
                    .into_iter()
                    .take(limit)
                    .map(|worker| self.map_worker_summary(worker))
                    .collect(),
                Vec::new(),
            ),
            Err(diagnostic) => RuntimeList::new(Vec::new(), vec![diagnostic]),
        }
    }

    fn list_stopped_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }
        match self.get_json::<RuntimeHttpWorkersResponse>("/v1/workers?status=stopped") {
            Ok(response) => RuntimeList::new(
                response
                    .workers
                    .into_iter()
                    .take(limit)
                    .map(|worker| self.map_worker_summary(worker))
                    .collect(),
                Vec::new(),
            ),
            Err(diagnostic) => RuntimeList::new(Vec::new(), vec![diagnostic]),
        }
    }

    fn worker(&self, worker_id: &str) -> WorkerLookupResult {
        match self.get_json::<RuntimeHttpWorkerResponse>(&format!("/v1/workers/{worker_id}")) {
            Ok(response) => WorkerLookupResult {
                worker: Some(self.map_worker_detail(response.worker)),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) if diagnostic.code == "worker_not_found" => WorkerLookupResult {
                worker: None,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerLookupResult {
                worker: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn restore_worker(&self, worker_id: &str) -> WorkerRestoreResult {
        match self.post_json::<_, RuntimeHttpWorkerResponse>(
            &format!("/v1/workers/{worker_id}/restore"),
            &serde_json::json!({}),
        ) {
            Ok(response) => WorkerRestoreResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(response.worker)),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerRestoreResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn replace_worker_workspace_api(
        &self,
        worker_id: &str,
        workspace_api: WorkspaceApiRef,
    ) -> WorkerWorkspaceApiResult {
        match self.post_json::<_, RuntimeHttpWorkerResponse>(
            &format!("/v1/workers/{worker_id}/workspace-api"),
            &RuntimeHttpWorkerWorkspaceApiRequest { workspace_api },
        ) {
            Ok(response) => WorkerWorkspaceApiResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(response.worker)),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerWorkspaceApiResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn create_working_directory(
        &self,
        request: WorkingDirectoryRequest,
    ) -> RuntimeWorkingDirectoryResult {
        match self.post_json::<_, RuntimeHttpWorkingDirectoryResponse>(
            "/v1/working-directories",
            &request,
        ) {
            Ok(response) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(response.working_directory),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn authorize_working_directory_repository_access(
        &self,
        request: WorkingDirectoryRepositoryAccessRequest,
    ) -> std::result::Result<(), Error> {
        self.post_json::<_, RuntimeHttpRepositoryAccessResponse>(
            "/v1/working-directories/repository-access",
            &request,
        )
        .map(|_| ())
        .map_err(|diagnostic| Error::RegistryInconsistency(diagnostic.message))
    }

    fn list_working_directories(&self) -> RuntimeList<WorkingDirectoryStatus> {
        match self.get_json::<RuntimeHttpWorkingDirectoriesResponse>("/v1/working-directories") {
            Ok(response) => RuntimeList::new(response.working_directories, Vec::new()),
            Err(diagnostic) => RuntimeList::new(Vec::new(), vec![diagnostic]),
        }
    }

    fn working_directory(&self, working_directory_id: &str) -> RuntimeWorkingDirectoryResult {
        match self.get_json::<RuntimeHttpWorkingDirectoryResponse>(&format!(
            "/v1/working-directories/{working_directory_id}"
        )) {
            Ok(response) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(response.working_directory),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn open_workdir_session<'a>(
        &'a self,
        working_directory_id: &'a str,
        owner_worker_id: Option<&'a str>,
    ) -> Pin<
        Box<dyn Future<Output = Result<workdir::WorkdirSessionHandle, WorkdirError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let session = RemoteWorkerRuntime::open_workdir_session(
                self,
                working_directory_id,
                owner_worker_id,
            )
            .await?;
            Ok(Arc::new(session) as workdir::WorkdirSessionHandle)
        })
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> RuntimeWorkingDirectoryResult {
        match self.delete_json::<RuntimeHttpWorkingDirectoryResponse>(&format!(
            "/v1/working-directories/{working_directory_id}"
        )) {
            Ok(response) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(response.working_directory),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn spawn_worker(
        &self,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        if matches!(
            request.acceptance,
            WorkerSpawnAcceptanceRequirement::SocketReady
        ) {
            return WorkerSpawnResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                acceptance_evidence: Vec::new(),
                diagnostics: vec![diagnostic(
                    "remote_runtime_no_socket_ready_acceptance",
                    DiagnosticSeverity::Warning,
                    "Remote Runtime v0 exposes backend-proxied REST/WS control, not direct socket readiness".to_string(),
                )],
            };
        }
        let profile = request.profile.clone();
        let profile_source = match profile_source_archive_http_source(
            &request,
            &profile,
            &self.workspace_id,
            Some(self.runtime_id.as_str()),
            &self.resource_broker,
            &self.backend_base_url,
        ) {
            Ok(source) => source,
            Err(error) => {
                return WorkerSpawnResult {
                    state: WorkerOperationState::Rejected,
                    worker: None,
                    acceptance_evidence: Vec::new(),
                    diagnostics: vec![diagnostic(
                        "remote_profile_source_archive_invalid",
                        DiagnosticSeverity::Error,
                        error,
                    )],
                };
            }
        };
        let workspace_api = match required_worker_workspace_api(&request) {
            Ok(workspace_api) => workspace_api,
            Err(diagnostic) => {
                return WorkerSpawnResult {
                    state: WorkerOperationState::Rejected,
                    worker: None,
                    acceptance_evidence: Vec::new(),
                    diagnostics: vec![diagnostic],
                };
            }
        };
        let config_bundle = spawn_config_bundle_ref(&request);
        let create = CreateWorkerRequest {
            worker_id: binding.worker_id,
            create_fingerprint: binding.create_fingerprint,
            profile,
            display_name: request.requested_worker_name.clone(),
            config_bundle,
            profile_source,
            initial_input: initial_worker_input(&request.initial_submit),
            working_directory_request: request.resolved_working_directory_request.clone(),
            working_directory: request.resolved_working_directory.clone(),
            worker_observation_enabled: request.resolved_worker_observation_enabled,
            worker_observation_grants: request.resolved_worker_observation_grants.clone(),
            workspace_api: Some(workspace_api),
            memory_settings: request.resolved_memory_settings.clone(),
        };
        match self.post_json::<_, RuntimeHttpWorkerResponse>("/v1/workers", &create) {
            Ok(response) => WorkerSpawnResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(response.worker)),
                acceptance_evidence: vec![WorkerSpawnAcceptanceEvidence {
                    kind: "remote_runtime_worker_created".to_string(),
                    detail: "worker-runtime REST create endpoint accepted the Worker".to_string(),
                }],
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerSpawnResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                acceptance_evidence: Vec::new(),
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        self.post_json::<_, RuntimeHttpWorkspacePromptProjectionResponse>(
            "/v1/workspace-prompt-projections",
            &RuntimeHttpWorkspacePromptProjectionRequest { projection },
        )
        .map(|_| ())
        .map_err(|error| error.message)
    }

    fn sync_config_bundle(&self, bundle: ConfigBundle) -> ConfigBundleSyncResult {
        let request = RuntimeHttpConfigBundleSyncRequest { bundle };
        match self.post_json::<_, RuntimeHttpConfigBundleAvailabilityResponse>(
            "/v1/config-bundles",
            &request,
        ) {
            Ok(response) => ConfigBundleSyncResult {
                state: WorkerOperationState::Accepted,
                availability: Some(response.availability),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => ConfigBundleSyncResult {
                state: WorkerOperationState::Rejected,
                availability: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn check_config_bundle(&self, reference: ConfigBundleRef) -> ConfigBundleCheckResult {
        let path = Self::bundle_availability_path(&reference);
        match self.get_json::<RuntimeHttpConfigBundleAvailabilityResponse>(&path) {
            Ok(response) => ConfigBundleCheckResult {
                state: WorkerOperationState::Accepted,
                availability: Some(response.availability),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => ConfigBundleCheckResult {
                state: WorkerOperationState::Rejected,
                availability: None,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn stop_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        let body = RuntimeHttpWorkerLifecycleRequest {
            reason: request.reason,
        };
        match self.post_json::<_, RuntimeHttpWorkerLifecycleResponse>(
            &format!("/v1/workers/{worker_id}/stop"),
            &body,
        ) {
            Ok(response) => self.lifecycle_result_from_response(worker_id, response),
            Err(diagnostic) => remote_lifecycle_rejected(&self.runtime_id, worker_id, diagnostic),
        }
    }

    fn cancel_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        let body = RuntimeHttpWorkerLifecycleRequest {
            reason: request.reason,
        };
        match self.post_json::<_, RuntimeHttpWorkerLifecycleResponse>(
            &format!("/v1/workers/{worker_id}/cancel"),
            &body,
        ) {
            Ok(response) => self.lifecycle_result_from_response(worker_id, response),
            Err(diagnostic) => remote_lifecycle_rejected(&self.runtime_id, worker_id, diagnostic),
        }
    }

    fn delete_worker(&self, worker_id: &str) -> WorkerDeleteResult {
        match self
            .delete_json::<RuntimeHttpWorkerDeleteResponse>(&format!("/v1/workers/{worker_id}"))
        {
            Ok(response) => WorkerDeleteResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(
                    self.runtime_id.clone(),
                    response.worker.worker_id.to_string(),
                ),
                deleted: response.worker.deleted,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerDeleteResult {
                state: WorkerOperationState::Rejected,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                deleted: false,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn worker_retention_inventory(
        &self,
        worker_id: &str,
    ) -> Result<WorkerRetentionInventory, String> {
        self.get_json::<WorkerRetentionInventory>(&format!(
            "/v1/workers/{worker_id}/retention/inventory"
        ))
        .map_err(|diagnostic| diagnostic.message)
    }

    fn execute_worker_retention(
        &self,
        request: WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, String> {
        let worker_id = request.worker_id.to_string();
        self.post_json::<_, WorkerRetentionExecutionResult>(
            &format!("/v1/workers/{worker_id}/retention/execute"),
            &request,
        )
        .map_err(|diagnostic| diagnostic.message)
    }

    fn observation_source(
        &self,
        worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSource> {
        Some(crate::observation::RuntimeObservationSource::remote_ws(
            crate::observation::RuntimeObservationSourceConfig {
                worker: RuntimeWorkerRef::new(&self.runtime_id, worker_id),
                endpoint: self.ws_endpoint(worker_id),
                bearer_token: self
                    .runtime_capability_token(&format!("/v1/workers/{worker_id}/protocol"))
                    .or_else(|| self.bearer_token.clone()),
            },
        ))
    }

    fn send_input(&self, worker_id: &str, request: WorkerInputRequest) -> WorkerInputResult {
        let input = EmbeddedWorkerInput {
            kind: match request.kind {
                WorkerInputKind::User => EmbeddedWorkerInputKind::User,
                WorkerInputKind::Notify => EmbeddedWorkerInputKind::Notify,
                WorkerInputKind::Compact => EmbeddedWorkerInputKind::Compact,
                WorkerInputKind::ListRewindTargets => EmbeddedWorkerInputKind::ListRewindTargets,
                WorkerInputKind::RegisterPeer => EmbeddedWorkerInputKind::RegisterPeer,
            },
            content: request.content,
            submission_id: None,
            segments: request.segments,
        };
        match self.post_json::<_, RuntimeHttpWorkerInputResponse>(
            &format!("/v1/workers/{worker_id}/input"),
            &input,
        ) {
            Ok(_) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => remote_input_rejected(&self.runtime_id, worker_id, diagnostic),
        }
    }

    fn worker_completions(
        &self,
        worker_id: &str,
        request: WorkerCompletionsRequest,
    ) -> WorkerCompletionsResult {
        let request = RuntimeHttpWorkerCompletionsRequest {
            kind: request.kind,
            prefix: request.prefix,
        };
        match self.post_json::<_, RuntimeHttpWorkerCompletionsResponse>(
            &format!("/v1/workers/{worker_id}/completions"),
            &request,
        ) {
            Ok(response) => WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: response.kind,
                prefix: response.prefix,
                entries: response.entries,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerCompletionsResult {
                worker: RuntimeWorkerRef::new(self.runtime_id.clone(), worker_id.to_string()),
                kind: request.kind,
                prefix: request.prefix,
                entries: Vec::new(),
                diagnostics: vec![diagnostic],
            },
        }
    }
}

fn embedded_runtime_status_label(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Running => "running",
        RuntimeStatus::Stopped => "stopped",
    }
}

fn runtime_worker_can_stop(execution_enabled: bool, status: EmbeddedWorkerStatus) -> bool {
    execution_enabled && status.is_active()
}

fn embedded_worker_status_label(status: EmbeddedWorkerStatus) -> &'static str {
    match status {
        EmbeddedWorkerStatus::Idle => "idle",
        EmbeddedWorkerStatus::Running => "running",
        EmbeddedWorkerStatus::Paused => "paused",
        EmbeddedWorkerStatus::Stopped => "stopped",
        EmbeddedWorkerStatus::Cancelled => "cancelled",
    }
}

fn embedded_worker_projection_diagnostics() -> Vec<RuntimeDiagnostic> {
    vec![diagnostic(
        "embedded_runtime_projection",
        DiagnosticSeverity::Info,
        "Worker identity is projected only as runtime_id plus worker_id; embedded runtime internals remain backend-private".to_string(),
    )]
}

fn spawn_config_bundle_ref(request: &WorkerSpawnRequest) -> Option<ConfigBundleRef> {
    request
        .resolved_config_bundle
        .as_ref()
        .map(|bundle| ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        })
}

fn profile_source_archive_for_request(
    request: &WorkerSpawnRequest,
    profile: &ProfileSelector,
) -> Result<ProfileSourceArchive, String> {
    if let Some(archive) = request
        .resolved_config_bundle
        .as_ref()
        .and_then(|bundle| bundle.profile_source_archive.clone())
    {
        return Ok(archive);
    }
    builtin_profile_source_archive(profile)
}

fn profile_source_archive_source(
    request: &WorkerSpawnRequest,
    profile: &ProfileSelector,
) -> Result<ProfileSourceArchiveSource, String> {
    Ok(ProfileSourceArchiveSource::Embedded {
        archive: profile_source_archive_for_request(request, profile)?,
    })
}

fn profile_source_archive_http_source(
    request: &WorkerSpawnRequest,
    profile: &ProfileSelector,
    workspace_id: &str,
    runtime_id: Option<&str>,
    resource_broker: &BackendResourceBroker,
    backend_base_url: &str,
) -> Result<ProfileSourceArchiveSource, String> {
    let archive = profile_source_archive_for_request(request, profile)?;
    let target = runtime_id
        .map(BackendResourceTarget::Runtime)
        .unwrap_or(BackendResourceTarget::Workspace);
    let _handle = resource_broker.issue_profile_source_archive_handle(
        workspace_id.to_string(),
        target,
        archive.clone(),
    );
    let etag = format!("\"profile-source:{}\"", archive.reference.digest);
    let url = format!(
        "{}/api/w/{}/profile-source-archives/{}",
        backend_base_url.trim_end_matches('/'),
        workspace_id,
        archive.reference.digest
    );
    Ok(ProfileSourceArchiveSource::Http {
        location: ProfileSourceArchiveHttpRef {
            url,
            etag: Some(etag),
            archive: archive.reference.clone(),
        },
    })
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileSourceArchiveTransport {
    Inline,
    BackendResourceHandle,
}

#[cfg(test)]
fn builtin_profile_config_bundle(
    profile: &ProfileSelector,
    workspace_id: &str,
    runtime_id: Option<&str>,
    resource_broker: &BackendResourceBroker,
    archive_transport: ProfileSourceArchiveTransport,
) -> Result<ConfigBundle, String> {
    let id = format!(
        "workspace-runtime-{}",
        embedded_profile_label(profile)
            .unwrap_or_else(|| "default".to_string())
            .replace([':', '/', ' '], "-")
    );
    let archive = builtin_profile_source_archive(profile)?;
    let (profile_source_archive, profile_source_archive_handle) = match archive_transport {
        ProfileSourceArchiveTransport::Inline => (Some(archive), None),
        ProfileSourceArchiveTransport::BackendResourceHandle => {
            let target = runtime_id
                .map(BackendResourceTarget::Runtime)
                .unwrap_or(BackendResourceTarget::Workspace);
            let handle = resource_broker.issue_profile_source_archive_handle(
                workspace_id.to_string(),
                target,
                archive,
            );
            (None, Some(handle))
        }
    };
    Ok(ConfigBundle {
        metadata: ConfigBundleMetadata {
            id,
            digest: String::new(),
            revision: "workspace-runtime-v0".to_string(),
            workspace_id: workspace_id.to_string(),
            created_at: "runtime-generated".to_string(),
            provenance: ConfigBundleProvenance {
                source: "workspace-server".to_string(),
                detail: Some("backend-resolved launch bundle".to_string()),
            },
        },
        profiles: vec![ConfigProfileDescriptor {
            selector: profile.clone(),
            label: embedded_profile_label(profile),
        }],
        declarations: Vec::new(),
        prompt_catalog: None,
        profile_source_archive,
        profile_source_archive_handle,
    }
    .with_computed_digest())
}

fn embedded_profile_label(profile: &ProfileSelector) -> Option<String> {
    Some(match profile {
        ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => {
            let builtin_name = name.strip_prefix("builtin:").unwrap_or(name);
            if builtin_name == MEMORY_CONSOLIDATION_PROFILE {
                MEMORY_CONSOLIDATION_PROFILE.to_string()
            } else if builtin_name == WORKSPACE_ORCHESTRATOR_PROFILE {
                WORKSPACE_ORCHESTRATOR_PROFILE.to_string()
            } else {
                safe_display_hint(name)
            }
        }
    })
}

fn builtin_profile_source_archive(
    profile: &ProfileSelector,
) -> Result<ProfileSourceArchive, String> {
    let selected_profile = match profile {
        ProfileSelector::Builtin(name) => {
            if name.starts_with("builtin:") {
                name.clone()
            } else {
                format!("builtin:{name}")
            }
        }
        ProfileSelector::Named(name) => {
            return Err(format!(
                "embedded runtime does not provide named Profile `{name}`"
            ));
        }
    };
    let catalog = manifest::builtin_profile_catalog_snapshot();
    if !catalog.entrypoints.contains_key(&selected_profile) {
        return Err(format!(
            "embedded runtime does not provide Profile `{selected_profile}`"
        ));
    }

    ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: catalog.id.to_owned(),
        sources: catalog.sources,
        entrypoints: catalog.entrypoints,
        imports: catalog.imports,
    })
    .map_err(|error| format!("failed to build built-in Profile source archive: {error}"))
}

const MEMORY_CONSOLIDATION_PROFILE: &str = "memory-consolidation";
const MEMORY_CONSOLIDATION_SINGLETON_KEY: &str = "workspace-memory-consolidation";
const WORKSPACE_ORCHESTRATOR_PROFILE: &str = "orchestrator";
pub(crate) const WORKSPACE_ORCHESTRATOR_SINGLETON_KEY: &str = "workspace-orchestrator";

struct WorkerDisplayMetadata {
    display_name: String,
    singleton_key: Option<String>,
    tags: Vec<String>,
}

fn worker_display_metadata(
    worker_id: &str,
    profile_label: Option<&str>,
    requested_display_name: Option<&str>,
    internal: bool,
) -> WorkerDisplayMetadata {
    if profile_label == Some(MEMORY_CONSOLIDATION_PROFILE) {
        let mut tags = vec![
            "memory".to_string(),
            "consolidation".to_string(),
            "singleton".to_string(),
        ];
        if internal {
            tags.insert(0, "internal".to_string());
        }
        return WorkerDisplayMetadata {
            display_name: "Memory Consolidation".to_string(),
            singleton_key: Some(MEMORY_CONSOLIDATION_SINGLETON_KEY.to_string()),
            tags,
        };
    }
    if profile_label == Some(WORKSPACE_ORCHESTRATOR_PROFILE)
        && requested_display_name == Some(WORKSPACE_ORCHESTRATOR_SINGLETON_KEY)
    {
        let mut tags = vec!["orchestrator".to_string(), "singleton".to_string()];
        if internal {
            tags.insert(0, "internal".to_string());
        }
        return WorkerDisplayMetadata {
            display_name: "Workspace Orchestrator".to_string(),
            singleton_key: Some(WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.to_string()),
            tags,
        };
    }
    let display_name = requested_display_name
        .filter(|value| !value.trim().is_empty())
        .map(safe_display_hint)
        .or_else(|| profile_label.map(profile_display_name))
        .unwrap_or_else(|| format!("Worker {worker_id}"));
    let mut tags = Vec::new();
    if internal {
        tags.push("internal".to_string());
    }
    if let Some(profile_label) = profile_label {
        tags.push(format!("profile:{profile_label}"));
    }
    WorkerDisplayMetadata {
        display_name,
        singleton_key: None,
        tags,
    }
}

fn profile_display_name(profile_label: &str) -> String {
    profile_label
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn embedded_input_rejected(
    runtime_id: &str,
    worker_id: &str,
    diagnostic: RuntimeDiagnostic,
) -> WorkerInputResult {
    WorkerInputResult {
        state: WorkerOperationState::Rejected,
        worker: RuntimeWorkerRef::new(runtime_id.to_string(), worker_id.to_string()),
        diagnostics: vec![diagnostic],
    }
}

fn remote_input_rejected(
    runtime_id: &str,
    worker_id: &str,
    diagnostic: RuntimeDiagnostic,
) -> WorkerInputResult {
    WorkerInputResult {
        state: WorkerOperationState::Rejected,
        worker: RuntimeWorkerRef::new(runtime_id.to_string(), worker_id.to_string()),
        diagnostics: vec![diagnostic],
    }
}

fn embedded_lifecycle_rejected(
    runtime_id: &str,
    worker_id: &str,
    diagnostic: RuntimeDiagnostic,
) -> WorkerLifecycleResult {
    WorkerLifecycleResult {
        state: WorkerOperationState::Rejected,
        worker: RuntimeWorkerRef::new(runtime_id.to_string(), worker_id.to_string()),
        diagnostics: vec![diagnostic],
    }
}

fn remote_lifecycle_rejected(
    runtime_id: &str,
    worker_id: &str,
    diagnostic: RuntimeDiagnostic,
) -> WorkerLifecycleResult {
    WorkerLifecycleResult {
        state: WorkerOperationState::Rejected,
        worker: RuntimeWorkerRef::new(runtime_id.to_string(), worker_id.to_string()),
        diagnostics: vec![diagnostic],
    }
}

fn embedded_workdir_unsupported_diagnostic() -> RuntimeDiagnostic {
    diagnostic(
        "embedded_worker_workdir_unsupported",
        DiagnosticSeverity::Error,
        "Embedded Runtime is no-workdir only; choose a non-embedded Runtime for workspace-file Workers".to_string(),
    )
}

fn sanitize_embedded_execution_message(
    message: &str,
    operation: &impl std::fmt::Debug,
    outcome: &impl std::fmt::Debug,
) -> String {
    let summary =
        format!("Embedded Worker execution backend rejected {operation:?} with {outcome:?}");
    let mut redact_next = false;
    let detail = message
        .split_whitespace()
        .map(|part| {
            if redact_next {
                redact_next = false;
                return "[redacted]";
            }
            let lowercase = part.to_ascii_lowercase();
            let label =
                lowercase.trim_matches(|character: char| !character.is_ascii_alphanumeric());
            if matches!(
                label,
                "bearer" | "credential" | "key" | "password" | "secret" | "session" | "token"
            ) {
                redact_next = true;
            }
            if part.contains('/')
                || part.contains('\\')
                || lowercase.contains("credential=")
                || lowercase.contains("key=")
                || lowercase.contains("password=")
                || lowercase.contains("secret=")
                || lowercase.contains("session=")
                || lowercase.contains("session_id=")
                || lowercase.contains("token=")
            {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let detail = detail.trim();
    if detail.is_empty() {
        return summary;
    }
    let truncated = detail.chars().count() > 512;
    let mut detail = detail.chars().take(512).collect::<String>();
    if truncated {
        detail.push('…');
    }
    format!("{summary}: {detail}")
}

fn embedded_runtime_diagnostic(error: &EmbeddedRuntimeError) -> RuntimeDiagnostic {
    match error {
        EmbeddedRuntimeError::RuntimeStopped => diagnostic(
            "embedded_runtime_stopped",
            DiagnosticSeverity::Warning,
            "Embedded Runtime is stopped".to_string(),
        ),
        EmbeddedRuntimeError::WorkerNotFound { .. } => diagnostic(
            "embedded_worker_not_found",
            DiagnosticSeverity::Warning,
            "Embedded Runtime worker was not found".to_string(),
        ),
        EmbeddedRuntimeError::WorkerExecutionUnavailable { .. }
        | EmbeddedRuntimeError::ExecutionBackendUnavailable { .. } => diagnostic(
            "embedded_worker_execution_unavailable",
            DiagnosticSeverity::Warning,
            "Embedded Worker has no execution backend attached".to_string(),
        ),
        EmbeddedRuntimeError::WorkerExecutionRejected {
            operation,
            outcome,
            message,
            ..
        } => diagnostic(
            "embedded_worker_execution_rejected",
            DiagnosticSeverity::Warning,
            sanitize_embedded_execution_message(message, operation, outcome),
        ),
        EmbeddedRuntimeError::LimitTooLarge { requested, max } => diagnostic(
            "embedded_runtime_limit_too_large",
            DiagnosticSeverity::Warning,
            format!("Requested limit {requested} exceeds embedded Runtime maximum {max}"),
        ),
        EmbeddedRuntimeError::InvalidInitialInputKind { .. } => diagnostic(
            "embedded_worker_initial_input_kind_invalid",
            DiagnosticSeverity::Warning,
            error.to_string(),
        ),
        EmbeddedRuntimeError::WorkingDirectory(workdir_diagnostic) => diagnostic(
            workdir_diagnostic.code.clone(),
            DiagnosticSeverity::Warning,
            workdir_diagnostic.message.clone(),
        ),
        EmbeddedRuntimeError::InvalidRequest(_)
        | EmbeddedRuntimeError::WorkspaceOwnerMismatch { .. }
        | EmbeddedRuntimeError::ConfigBundleMissing { .. }
        | EmbeddedRuntimeError::ConfigBundleDigestMismatch { .. }
        | EmbeddedRuntimeError::InvalidProfileSelector { .. }
        | EmbeddedRuntimeError::UnsupportedConfigDeclaration { .. } => diagnostic(
            "embedded_runtime_invalid_request",
            DiagnosticSeverity::Warning,
            "Embedded Runtime rejected the request".to_string(),
        ),
        EmbeddedRuntimeError::StoreIo { .. }
        | EmbeddedRuntimeError::StoreMissing { .. }
        | EmbeddedRuntimeError::StoreCorrupt { .. } => diagnostic(
            "embedded_runtime_store_error",
            DiagnosticSeverity::Error,
            "Embedded Runtime storage operation failed; internal paths are not exposed".to_string(),
        ),
        EmbeddedRuntimeError::StatePoisoned => diagnostic(
            "embedded_runtime_state_unavailable",
            DiagnosticSeverity::Error,
            "Embedded Runtime state is unavailable".to_string(),
        ),
    }
}

fn host_id_for_embedded_workspace(workspace_id: &str) -> String {
    bounded_backend_identifier("embedded-", workspace_id)
}

fn host_id_for_remote_runtime(runtime_id: &str) -> String {
    bounded_backend_identifier("remote-", runtime_id)
}

fn url_path_segment_encode(input: &str) -> String {
    percent_encode(input, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':')
    })
}

fn url_query_value_encode(input: &str) -> String {
    percent_encode(input, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
    })
}

fn percent_encode(input: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        if keep(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn remote_reqwest_diagnostic(runtime_id: &str, err: reqwest::Error) -> RuntimeDiagnostic {
    if err.is_timeout() {
        diagnostic(
            "remote_runtime_timeout",
            DiagnosticSeverity::Error,
            format!("Timed out while contacting remote Runtime '{runtime_id}'"),
        )
    } else if err.is_connect() || err.is_request() {
        diagnostic(
            "remote_runtime_network_error",
            DiagnosticSeverity::Error,
            format!("Failed to contact remote Runtime '{runtime_id}'"),
        )
    } else {
        diagnostic(
            "remote_runtime_client_error",
            DiagnosticSeverity::Error,
            format!("Remote Runtime client error for '{runtime_id}'"),
        )
    }
}

fn sanitize_remote_runtime_message(code: &str, message: &str) -> String {
    if message.contains('/') || message.contains('\\') {
        format!("remote Runtime returned {code}; backend-private details were omitted")
    } else {
        message.to_string()
    }
}

fn remote_http_status_diagnostic(
    runtime_id: &str,
    status: StatusCode,
    response: reqwest::blocking::Response,
) -> RuntimeDiagnostic {
    let error = response.json::<RuntimeHttpErrorResponse>().ok();
    let remote_code = error
        .as_ref()
        .map(|error| error.error.code.as_str())
        .unwrap_or("remote_http_error");
    let remote_message = error
        .as_ref()
        .map(|error| sanitize_remote_runtime_message(remote_code, &error.error.message))
        .unwrap_or_else(|| "remote Runtime did not return a typed error body".to_string());
    let (code, severity) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ("remote_runtime_auth_failed", DiagnosticSeverity::Error)
        }
        _ if error.is_some() => (remote_code, DiagnosticSeverity::Warning),
        StatusCode::NOT_FOUND => ("remote_runtime_not_found", DiagnosticSeverity::Warning),
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => {
            ("remote_runtime_unsupported", DiagnosticSeverity::Warning)
        }
        _ if status.is_server_error() => ("remote_runtime_http_error", DiagnosticSeverity::Error),
        _ => (remote_code, DiagnosticSeverity::Warning),
    };
    diagnostic(
        code,
        severity,
        format!("Remote Runtime '{runtime_id}' rejected request (HTTP {status}): {remote_message}"),
    )
}

fn diagnostic(
    code: impl Into<String>,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.into(),
        severity,
        message: message.into(),
    }
}

fn operation_failed_or_unknown_worker(
    runtime_id: &str,
    worker_id: &str,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> RuntimeRegistryError {
    diagnostics
        .into_iter()
        .find(|diagnostic| matches!(diagnostic.severity, DiagnosticSeverity::Error))
        .map(|diagnostic| RuntimeRegistryError::RuntimeOperationFailed {
            runtime_id: runtime_id.to_string(),
            code: diagnostic.code,
            message: diagnostic.message,
        })
        .unwrap_or_else(|| RuntimeRegistryError::UnknownWorker {
            worker: RuntimeWorkerRef::new(runtime_id, worker_id),
        })
}

fn bounded_backend_identifier(prefix: &str, value: &str) -> String {
    let digest = digest_hex(value.as_bytes(), ID_DIGEST_HEX_LEN);
    let mut body = sanitize_identifier_body(value);
    if body.is_empty() {
        body = "id".to_string();
    }

    let suffix_len = 1 + ID_DIGEST_HEX_LEN;
    let body_budget = MAX_IDENTIFIER_LEN
        .saturating_sub(prefix.len())
        .saturating_sub(suffix_len)
        .max(1);
    if body.len() > body_budget {
        body.truncate(body_budget);
        body = body.trim_matches('-').to_string();
        if body.is_empty() {
            body = "id".to_string();
        }
    }

    let mut id = format!("{prefix}{body}-{digest}");
    if id.len() > MAX_IDENTIFIER_LEN {
        let digest_suffix = format!("-{digest}");
        let prefix_budget = MAX_IDENTIFIER_LEN.saturating_sub(digest_suffix.len());
        id = format!(
            "{}{}",
            prefix.chars().take(prefix_budget).collect::<String>(),
            digest_suffix
        );
    }
    id
}

fn sanitize_identifier_body(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn digest_hex(bytes: &[u8], hex_len: usize) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(hex_len);
    for byte in digest {
        if out.len() >= hex_len {
            break;
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out.truncate(hex_len);
    out
}

fn validate_backend_identifier(
    kind: &'static str,
    value: &str,
) -> Result<(), RuntimeRegistryError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_LEN
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ':'))
    {
        return Err(RuntimeRegistryError::InvalidIdentifier {
            kind,
            value: value.chars().take(MAX_IDENTIFIER_LEN).collect(),
        });
    }
    Ok(())
}

fn safe_display_hint(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() && *ch != '/' && *ch != '\\')
        .take(80)
        .collect()
}

fn worker_spawn_intent_label(intent: &WorkerSpawnIntent) -> &'static str {
    match intent {
        WorkerSpawnIntent::WorkspaceCompanion => "workspace_companion",
        WorkerSpawnIntent::WorkspaceOrchestrator => "workspace_orchestrator",
        WorkerSpawnIntent::WorkspaceCoding => "workspace_coding",
        WorkerSpawnIntent::TicketRole { role, .. } => match role {
            TicketWorkerRole::Intake => "ticket_intake",
            TicketWorkerRole::Orchestrator => "ticket_orchestrator",
            TicketWorkerRole::Coder => "ticket_coder",
            TicketWorkerRole::Reviewer => "ticket_reviewer",
        },
    }
}

pub fn placeholder_worker(host_id: impl Into<String>) -> WorkerSummary {
    let host_id = host_id.into();
    WorkerSummary {
        worker: RuntimeWorkerRef::new("placeholder", "worker-placeholder"),
        host_id,
        display_name: "Worker runtime actions are not implemented".to_string(),
        label: "Worker runtime actions are not implemented".to_string(),
        profile: None,
        singleton_key: None,
        tags: Vec::new(),
        workspace: WorkerWorkspaceSummary {
            visibility: "none".to_string(),
            identity: "unsupported".to_string(),
            workspace_id: None,
        },
        state: "unsupported".to_string(),
        last_seen_at: None,
        pinned: false,
        retention_state: "transient".to_string(),
        implementation: WorkerImplementationSummary {
            kind: "placeholder".to_string(),
            display_hint: "unsupported".to_string(),
        },
        capabilities: WorkerCapabilitySummary {
            can_stop: false,
            can_spawn_followup: false,
        },
        working_directory: None,
        diagnostics: vec![diagnostic(
            "runtime_capability_unsupported",
            DiagnosticSeverity::Info,
            "worker control is outside this overview-only registry surface".to_string(),
        )],
    }
}

pub fn placeholder_spawn_response(host_id: impl Into<String>) -> WorkerSpawnResult {
    WorkerSpawnResult {
        state: WorkerOperationState::Unsupported,
        worker: Some(placeholder_worker(host_id)),
        acceptance_evidence: Vec::new(),
        diagnostics: vec![diagnostic(
            "worker_spawn_unsupported",
            DiagnosticSeverity::Info,
            "Workspace worker runtime control is not implemented yet".to_string(),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn test_create_binding() -> WorkerCreateBinding {
        WorkerCreateBinding {
            worker_id: EmbeddedWorkerId::now_v7(),
            create_fingerprint: "sha256:test-create".to_string(),
        }
    }

    fn test_workspace_api() -> WorkspaceApiRef {
        WorkspaceApiRef {
            workspace_id: "workspace-test".to_string(),
            base_url: "http://127.0.0.1:8787".to_string(),
        }
    }

    fn test_memory_settings() -> manifest::WorkspaceMemorySettingsSnapshot {
        manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-test".to_string(),
            settings_revision: 1,
            language: "English".to_string(),
        }
    }

    #[test]
    fn worker_summary_keeps_flat_wire_identity_while_using_structured_internal_identity() {
        let summary = placeholder_worker("placeholder");
        assert_eq!(
            summary.worker,
            RuntimeWorkerRef::new("placeholder", "worker-placeholder")
        );
        let value = serde_json::to_value(summary).unwrap();
        assert_eq!(value["runtime_id"], "placeholder");
        assert_eq!(value["worker_id"], "worker-placeholder");
        assert!(value.get("worker").is_none());

        let lifecycle = WorkerLifecycleResult {
            state: WorkerOperationState::Accepted,
            worker: RuntimeWorkerRef::new("arcadia", "30"),
            diagnostics: Vec::new(),
        };
        let value = serde_json::to_value(lifecycle).unwrap();
        assert_eq!(value["runtime_id"], "arcadia");
        assert_eq!(value["worker_id"], "30");
        assert!(value.get("worker").is_none());
    }

    #[test]
    fn embedded_builtin_decodal_profiles_resolve_through_archive() {
        let root = tempfile::tempdir().unwrap();
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let bundle = builtin_profile_config_bundle(
            &ProfileSelector::Builtin("builtin:companion".to_string()),
            "workspace-test",
            Some(runtime_id),
            &broker,
            ProfileSourceArchiveTransport::BackendResourceHandle,
        )
        .unwrap();
        let handle = bundle.profile_source_archive_handle.as_ref().unwrap();
        assert!(bundle.profile_source_archive.is_none());
        let response = broker
            .fetch_resource(worker_runtime::resource::BackendResourceFetchRequest {
                handle: handle.clone(),
                runtime_id: runtime_id.to_string(),
                worker_id: None,
                audit_correlation_id: handle.audit_correlation_id.clone(),
            })
            .unwrap();
        let archive =
            worker_runtime::resource::profile_source_archive_from_response(handle, response)
                .unwrap()
                .verify()
                .unwrap();
        assert!(!archive.reference().source_graph.entrypoints.is_empty());
        for selector_key in archive.reference().source_graph.entrypoints.keys() {
            let manifest = archive
                .resolve_profile(selector_key, root.path(), "embedded-test-worker")
                .unwrap();
            assert_eq!(manifest.worker.name, "embedded-test-worker");
        }
        let companion = archive
            .resolve_profile("builtin:companion", root.path(), "embedded-test-companion")
            .unwrap();
        assert!(companion.feature.manage_workdir.enabled);
        assert!(companion.feature.sub_worker.enabled);
        assert!(companion.feature.worker.enabled);
        assert!(!companion.feature.worker.direct_spawn);
        assert!(companion.feature.workspace_worker_discovery.enabled);
        let default_from_archive = archive
            .resolve_profile("builtin:default", root.path(), "embedded-test-default")
            .unwrap();
        let default_from_native = manifest::ProfileResolver::new()
            .with_workspace_base(root.path())
            .resolve(
                &manifest::ProfileSelector::source_named(
                    manifest::ProfileRegistrySource::Builtin,
                    "default",
                ),
                manifest::ProfileResolveOptions::with_worker_name("embedded-test-default"),
            )
            .unwrap()
            .manifest;
        let mut archive_value = serde_json::to_value(&default_from_archive).unwrap();
        let mut native_value = serde_json::to_value(&default_from_native).unwrap();
        let archive_profile = archive_value
            .as_object_mut()
            .and_then(|value| value.remove("profile"))
            .expect("archive resolution records Profile provenance");
        let native_profile = native_value
            .as_object_mut()
            .and_then(|value| value.remove("profile"))
            .expect("native resolution records Profile provenance");
        assert_eq!(archive_value, native_value);
        assert_eq!(archive_profile["source"]["kind"], "archive");
        assert_eq!(native_profile["source"]["kind"], "registry");
        assert!(default_from_archive.feature.sub_worker.enabled);
        assert!(!default_from_archive.feature.ticket.enabled);
        assert!(!default_from_archive.feature.objective.enabled);

        let coder = archive
            .resolve_profile("builtin:coder", root.path(), "embedded-test-coder")
            .unwrap();
        assert!(coder.feature.sub_worker.enabled);
        assert!(!coder.feature.worker.enabled);
    }

    #[test]
    fn remote_default_bundle_inlines_profile_archive_for_standalone_runtime() {
        let root = tempfile::tempdir().unwrap();
        let broker = BackendResourceBroker::default();
        let runtime_id = "remote:test";
        let bundle = builtin_profile_config_bundle(
            &ProfileSelector::Builtin("builtin:coder".to_string()),
            "workspace-test",
            Some(runtime_id),
            &broker,
            ProfileSourceArchiveTransport::Inline,
        )
        .unwrap();
        assert!(bundle.profile_source_archive_handle.is_none());
        let archive = bundle
            .profile_source_archive
            .as_ref()
            .expect("remote built-in bundle carries inline profile archive")
            .verify()
            .unwrap();
        let manifest = archive
            .resolve_profile("builtin:coder", root.path(), "remote-test-worker")
            .unwrap();
        assert_eq!(manifest.worker.name, "remote-test-worker");
        assert!(manifest.feature.sub_worker.enabled);
        assert!(!manifest.feature.worker.enabled);
    }

    #[test]
    fn resolved_project_profile_archive_is_used_for_runtime_delivery() {
        let broker = BackendResourceBroker::default();
        let builtin_selector = ProfileSelector::Builtin("builtin:coder".to_string());
        let archive = builtin_profile_source_archive(&builtin_selector)
            .expect("build stand-in project profile archive");
        let mut bundle = builtin_profile_config_bundle(
            &builtin_selector,
            "workspace-test",
            Some("runtime-test"),
            &broker,
            ProfileSourceArchiveTransport::Inline,
        )
        .expect("build project profile bundle");
        bundle.profile_source_archive = Some(archive.clone());
        let mut request = embedded_spawn_request();
        request.profile = ProfileSelector::Named("project:custom".to_string());
        request.resolved_config_bundle = Some(bundle);

        let delivered = profile_source_archive_for_request(&request, &request.profile)
            .expect("resolve project profile archive");
        assert_eq!(delivered.reference, archive.reference);
        assert_eq!(delivered.content, archive.content);
    }

    #[test]
    fn remote_profile_source_archive_url_uses_workspace_id_not_host_id() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "remote:test";
        let request = embedded_spawn_request();
        let source = profile_source_archive_http_source(
            &request,
            &ProfileSelector::Builtin("builtin:coder".to_string()),
            "workspace-actual",
            Some(runtime_id),
            &broker,
            "http://127.0.0.1:8787/",
        )
        .unwrap();
        let ProfileSourceArchiveSource::Http { location } = source else {
            panic!("remote profile source should be HTTP fetched");
        };
        assert!(
            location.url.starts_with(
                "http://127.0.0.1:8787/api/w/workspace-actual/profile-source-archives/"
            ),
            "{}",
            location.url
        );
        assert!(!location.url.contains("remote-runtime"), "{}", location.url);
    }

    #[test]
    fn embedded_archive_rejects_unknown_selectors() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        assert!(
            builtin_profile_config_bundle(
                &ProfileSelector::Builtin("builtin:missing".to_string()),
                "workspace-test",
                Some(runtime_id),
                &broker,
                ProfileSourceArchiveTransport::BackendResourceHandle,
            )
            .is_err()
        );
        assert!(
            builtin_profile_config_bundle(
                &ProfileSelector::Named("custom".to_string()),
                "workspace-test",
                Some(runtime_id),
                &broker,
                ProfileSourceArchiveTransport::BackendResourceHandle,
            )
            .is_err()
        );
    }

    fn test_config_bundle() -> ConfigBundle {
        ConfigBundle {
            metadata: worker_runtime::config_bundle::ConfigBundleMetadata {
                id: "bundle-1".to_string(),
                digest: String::new(),
                revision: "rev-1".to_string(),
                workspace_id: "local:test".to_string(),
                created_at: "2026-06-26T00:00:00Z".to_string(),
                provenance: worker_runtime::config_bundle::ConfigBundleProvenance {
                    source: "workspace-server-test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![worker_runtime::config_bundle::ConfigProfileDescriptor {
                selector: ProfileSelector::Builtin("builtin:coder".to_string()),
                label: Some("Coder".to_string()),
            }],
            declarations: vec![worker_runtime::config_bundle::ConfigDeclaration {
                kind: worker_runtime::config_bundle::ConfigDeclarationKind::CapabilityGrant,
                name: "read".to_string(),
                reference: "capability:read".to_string(),
            }],
            prompt_catalog: None,
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    struct FailingSpawnBackend;

    impl worker_runtime::execution::WorkerExecutionBackend for FailingSpawnBackend {
        fn backend_id(&self) -> &str {
            "workspace-server-failing-spawn-backend"
        }

        fn spawn_worker(
            &self,
            _request: worker_runtime::execution::WorkerExecutionSpawnRequest,
        ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
            worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
                worker_runtime::execution::WorkerExecutionResult::errored(
                    worker_runtime::execution::WorkerExecutionOperation::Spawn,
                    "provider setup failed at /tmp/secret-provider-config token=secret-value session_id=session-42",
                ),
            )
        }

        fn dispatch_input(
            &self,
            _handle: &worker_runtime::execution::WorkerExecutionHandle,
            _input: EmbeddedWorkerInput,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            worker_runtime::execution::WorkerExecutionResult::rejected(
                worker_runtime::execution::WorkerExecutionOperation::Input,
                "spawn failed before input could be dispatched",
            )
        }
    }

    #[derive(Default)]
    struct AcceptingExecutionBackend {
        contexts:
            Mutex<HashMap<EmbeddedWorkerRef, worker_runtime::execution::WorkerExecutionContext>>,
    }

    impl worker_runtime::execution::WorkerExecutionBackend for AcceptingExecutionBackend {
        fn backend_id(&self) -> &str {
            "workspace-server-test-backend"
        }

        fn spawn_worker(
            &self,
            request: worker_runtime::execution::WorkerExecutionSpawnRequest,
        ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
            self.contexts
                .lock()
                .unwrap()
                .insert(request.worker_ref.clone(), request.context);
            worker_runtime::execution::WorkerExecutionSpawnResult::Connected {
                handle: worker_runtime::execution::WorkerExecutionHandle::new(
                    request.worker_ref,
                    self.backend_id(),
                ),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request
                    .working_directory
                    .as_ref()
                    .map(|binding| binding.status()),
            }
        }

        fn dispatch_input(
            &self,
            handle: &worker_runtime::execution::WorkerExecutionHandle,
            input: EmbeddedWorkerInput,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            let context = self
                .contexts
                .lock()
                .unwrap()
                .get(handle.worker_ref())
                .cloned();
            let Some(context) = context else {
                return worker_runtime::execution::WorkerExecutionResult::rejected(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    "missing test context",
                );
            };
            let submission_id = input.submission_id.clone();
            let content = input.content;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let _ = context.publish_protocol_event(protocol::Event::Status {
                    status: protocol::WorkerStatus::Running,
                });
                let _ = context.publish_protocol_event(protocol::Event::TextDone {
                    text: format!("echo: {content}"),
                });
                let _ = context.publish_protocol_event(protocol::Event::RunEnd {
                    result: protocol::RunResult::Finished,
                });
                let _ = context.publish_protocol_event(protocol::Event::Status {
                    status: protocol::WorkerStatus::Idle,
                });
            });
            if let Some(submission_id) = submission_id {
                worker_runtime::execution::WorkerExecutionResult::accepted_input_committed(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    WorkerExecutionRunState::Busy,
                    submission_id,
                )
            } else {
                worker_runtime::execution::WorkerExecutionResult::accepted(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    WorkerExecutionRunState::Busy,
                )
            }
        }
    }

    #[derive(Clone)]
    struct FixtureRuntime {
        runtime_id: String,
        host_id: String,
        workers: Vec<WorkerSummary>,
        observed_prompt_revisions: Arc<Mutex<Vec<u64>>>,
    }

    impl FixtureRuntime {
        fn with_worker(runtime_id: &str, host_id: &str, worker_id: &str, label: &str) -> Self {
            Self {
                runtime_id: runtime_id.to_string(),
                host_id: host_id.to_string(),
                workers: vec![WorkerSummary {
                    worker: RuntimeWorkerRef::new(runtime_id, worker_id),
                    host_id: host_id.to_string(),
                    display_name: label.to_string(),
                    label: label.to_string(),
                    profile: None,
                    singleton_key: None,
                    tags: Vec::new(),
                    workspace: WorkerWorkspaceSummary {
                        visibility: "opaque".to_string(),
                        identity: host_id.to_string(),
                        workspace_id: None,
                    },
                    state: "available".to_string(),
                    last_seen_at: None,
                    pinned: false,
                    retention_state: "transient".to_string(),
                    implementation: WorkerImplementationSummary {
                        kind: "fixture".to_string(),
                        display_hint: "test fixture".to_string(),
                    },
                    capabilities: WorkerCapabilitySummary {
                        can_stop: false,
                        can_spawn_followup: false,
                    },
                    working_directory: None,
                    diagnostics: Vec::new(),
                }],
                observed_prompt_revisions: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl WorkspaceWorkerRuntime for FixtureRuntime {
        fn runtime_id(&self) -> &str {
            &self.runtime_id
        }

        fn observe_workspace_prompt_projection(
            &self,
            projection: worker::WorkspacePromptProjection,
        ) -> Result<(), String> {
            self.observed_prompt_revisions
                .lock()
                .map_err(|_| "prompt projection observations poisoned".to_string())?
                .push(projection.config_revision);
            Ok(())
        }

        fn runtime_summary(&self, _limit: usize) -> RuntimeSummary {
            RuntimeSummary {
                runtime_id: self.runtime_id.clone(),
                label: self.runtime_id.clone(),
                kind: "fixture".to_string(),
                status: "available".to_string(),
                source: RuntimeSourceSummary::embedded_worker_runtime_reserved(),
                host_ids: vec![self.host_id.clone()],
                worker_creation_available: false,
                os: "test".to_string(),
                arch: "test".to_string(),
                diagnostics: Vec::new(),
            }
        }

        fn list_hosts(&self, _limit: usize) -> RuntimeList<HostSummary> {
            RuntimeList::new(
                vec![HostSummary {
                    runtime_id: self.runtime_id.clone(),
                    host_id: self.host_id.clone(),
                    label: "fixture host".to_string(),
                    kind: "fixture".to_string(),
                    status: "available".to_string(),
                    observed_at: "unknown".to_string(),
                    last_seen_at: None,
                    os: "test".to_string(),
                    arch: "test".to_string(),
                    diagnostics: Vec::new(),
                }],
                Vec::new(),
            )
        }

        fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
            RuntimeList::new(
                self.workers.iter().take(limit).cloned().collect(),
                Vec::new(),
            )
        }

        fn worker(&self, worker_id: &str) -> WorkerLookupResult {
            WorkerLookupResult {
                worker: self
                    .workers
                    .iter()
                    .find(|worker| worker.worker.worker_id == worker_id)
                    .cloned(),
                diagnostics: Vec::new(),
            }
        }
    }

    #[test]
    fn registry_worker_lookup_is_scoped_by_runtime_id() {
        let registry = RuntimeRegistry::new(vec![
            Arc::new(FixtureRuntime::with_worker(
                "runtime-a",
                "host-a",
                "shared-worker",
                "worker from runtime a",
            )),
            Arc::new(FixtureRuntime::with_worker(
                "runtime-b",
                "host-b",
                "shared-worker",
                "worker from runtime b",
            )),
        ]);

        let from_runtime_b = registry
            .worker(&RuntimeWorkerRef::new("runtime-b", "shared-worker"))
            .unwrap();
        assert_eq!(from_runtime_b.worker.runtime_id, "runtime-b");
        assert_eq!(from_runtime_b.host_id, "host-b");
        assert_eq!(from_runtime_b.label, "worker from runtime b");

        let from_runtime_a = registry
            .worker(&RuntimeWorkerRef::new("runtime-a", "shared-worker"))
            .unwrap();
        assert_eq!(from_runtime_a.worker.runtime_id, "runtime-a");
        assert_eq!(from_runtime_a.host_id, "host-a");
        assert_eq!(from_runtime_a.label, "worker from runtime a");
    }

    #[test]
    fn registry_broadcasts_workspace_prompt_projection_revisions() {
        let runtime =
            FixtureRuntime::with_worker("runtime-a", "host-a", "worker-a", "worker from runtime a");
        let observed = runtime.observed_prompt_revisions.clone();
        let registry = RuntimeRegistry::new(vec![Arc::new(runtime)]);
        let catalog = worker::EffectivePromptCatalog::new(
            std::collections::BTreeMap::from([(
                "default".to_string(),
                "workspace prompt".to_string(),
            )]),
            12,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection = worker::WorkspacePromptProjection::new(
            "workspace-a",
            "source-12",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();

        let diagnostics = registry.observe_workspace_prompt_projection(projection);

        assert!(diagnostics.is_empty());
        assert_eq!(*observed.lock().unwrap(), vec![12]);
    }

    #[test]
    fn registry_worker_list_can_be_scoped_by_runtime_id() {
        let registry = RuntimeRegistry::new(vec![
            Arc::new(FixtureRuntime::with_worker(
                "runtime-a",
                "host-a",
                "shared-worker",
                "worker from runtime a",
            )),
            Arc::new(FixtureRuntime::with_worker(
                "runtime-b",
                "host-b",
                "shared-worker",
                "worker from runtime b",
            )),
        ]);

        let listed = registry.list_workers_for_runtime("runtime-b", 10).unwrap();
        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].worker.runtime_id, "runtime-b");
        assert_eq!(listed.items[0].host_id, "host-b");
        assert_eq!(listed.items[0].label, "worker from runtime b");
    }

    #[test]
    fn registry_keeps_companion_profile_workers_visible() {
        let mut runtime =
            FixtureRuntime::with_worker("runtime-a", "host-a", "worker-a", "Companion Worker");
        runtime.workers[0].profile = Some("builtin:companion".to_string());
        let registry = RuntimeRegistry::new(vec![Arc::new(runtime)]);

        let listed = registry.list_workers_for_runtime("runtime-a", 10).unwrap();
        assert_eq!(listed.items.len(), 1);
        assert_eq!(
            listed.items[0].profile.as_deref(),
            Some("builtin:companion")
        );

        let worker = registry
            .worker(&RuntimeWorkerRef::new("runtime-a", "worker-a"))
            .unwrap();
        assert_eq!(worker.profile.as_deref(), Some("builtin:companion"));
    }

    #[test]
    fn registry_worker_lookup_reports_unknown_runtime_and_worker_separately() {
        let registry = RuntimeRegistry::new(vec![Arc::new(FixtureRuntime::with_worker(
            "runtime-a",
            "host-a",
            "worker-a",
            "worker from runtime a",
        ))]);

        let unknown_runtime = registry
            .worker(&RuntimeWorkerRef::new("runtime-missing", "worker-a"))
            .unwrap_err();
        assert_eq!(
            unknown_runtime,
            RuntimeRegistryError::UnknownRuntime("runtime-missing".to_string())
        );
        assert!(matches!(
            unknown_runtime.into_error(),
            Error::UnknownRuntime(runtime_id) if runtime_id == "runtime-missing"
        ));

        let unknown_worker = registry
            .worker(&RuntimeWorkerRef::new("runtime-a", "999"))
            .unwrap_err();
        assert_eq!(
            unknown_worker,
            RuntimeRegistryError::UnknownWorker {
                worker: RuntimeWorkerRef::new("runtime-a", "999"),
            }
        );
        assert!(matches!(
            unknown_worker.into_error(),
            Error::UnknownWorker { worker }
                if worker == RuntimeWorkerRef::new("runtime-a", "999")
        ));
    }

    fn embedded_spawn_request() -> WorkerSpawnRequest {
        WorkerSpawnRequest {
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: "00001KVZSGT0Q".to_string(),
                role: TicketWorkerRole::Coder,
            },
            requested_worker_name: None,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: Some(test_workspace_api()),
            resolved_memory_settings: Some(test_memory_settings()),
        }
    }

    #[test]
    fn trusted_control_operation_is_runtime_spawn_idempotency_authority() {
        let mut request = embedded_spawn_request();
        request.resolved_control_operation = Some(WorkerControlOperation {
            operation_id: "control-op-1".to_string(),
            input_fingerprint: "sha256:control-input".to_string(),
        });

        assert_eq!(
            worker_spawn_idempotency(&request).unwrap(),
            Some((
                "control-op-1".to_string(),
                "sha256:control-input".to_string(),
            ))
        );
    }

    #[test]
    fn spawn_config_bundle_ref_preserves_bundle_identity() {
        let mut request = embedded_spawn_request();
        let bundle = test_config_bundle();
        let expected_id = bundle.metadata.id.clone();
        let expected_digest = bundle.metadata.digest.clone();
        request.resolved_config_bundle = Some(bundle);

        let bundle_ref = spawn_config_bundle_ref(&request).expect("bundle reference");
        assert_eq!(bundle_ref.id, expected_id);
        assert_eq!(bundle_ref.digest, expected_digest);
    }

    #[test]
    fn registry_syncs_bundle_before_embedded_spawn() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(AcceptingExecutionBackend::default()),
        )
        .expect("test backend should connect");
        let registry = RuntimeRegistry::for_workspace(runtime);
        let mut request = embedded_spawn_request();
        let bundle = test_config_bundle();
        let bundle_ref = ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        };
        request.resolved_config_bundle = Some(bundle);
        let binding = test_create_binding();

        let result = registry
            .spawn_worker("embedded-worker-runtime", binding.clone(), request)
            .expect("spawn request");
        assert_eq!(result.state, WorkerOperationState::Accepted);
        assert_eq!(
            result.worker.as_ref().unwrap().worker.worker_id,
            binding.worker_id.to_string()
        );
        let check = registry
            .check_config_bundle("embedded-worker-runtime", bundle_ref)
            .expect("bundle check");
        assert_eq!(check.state, WorkerOperationState::Accepted);
        assert!(check.availability.is_some());
    }

    #[test]
    fn embedded_runtime_rejects_missing_workspace_api_binding() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(AcceptingExecutionBackend::default()),
        )
        .expect("test backend should connect");
        let mut request = embedded_spawn_request();
        request.resolved_workspace_api = None;

        let spawned = runtime.spawn_worker(test_create_binding(), request);

        assert_eq!(spawned.state, WorkerOperationState::Rejected);
        assert!(
            spawned
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "worker_workspace_api_missing" })
        );
    }

    #[test]
    fn embedded_runtime_spawn_execution_failure_is_rejected_and_not_input_capable() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(FailingSpawnBackend),
        )
        .expect("test backend should connect");
        let spawned = runtime.spawn_worker(test_create_binding(), embedded_spawn_request());
        assert_eq!(spawned.state, WorkerOperationState::Rejected);
        assert!(spawned.acceptance_evidence.is_empty());
        assert!(spawned.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "embedded_worker_execution_rejected"
                && diagnostic
                    .message
                    .contains("provider setup failed at [redacted]")
                && !diagnostic.message.contains("/tmp/secret-provider-config")
                && !diagnostic.message.contains("secret-value")
                && !diagnostic.message.contains("session-42")
        }));
        assert!(spawned.worker.is_none());
    }

    #[test]
    fn worker_spawn_idempotency_fingerprint_covers_canonical_initial_submit() {
        let mut request = embedded_spawn_request();
        request.ticket_assignment = Some(WorkerTicketAssignmentRequest {
            ticket_id: "00001KVZSGT0Q".to_string(),
            operation_id: "operation-1".to_string(),
        });
        request.initial_submit = vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::text("Implement Ticket 00001KVZSGT0Q"),
        ];
        request.acceptance = WorkerSpawnAcceptanceRequirement::RunAccepted {
            expected_segments: request.initial_submit.len(),
        };

        let first = worker_spawn_idempotency(&request).unwrap().unwrap();
        let repeated = worker_spawn_idempotency(&request).unwrap().unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.0, "operation-1");

        let mut changed = request.clone();
        changed.initial_submit[1] = Segment::text("Different instruction");
        let changed = worker_spawn_idempotency(&changed).unwrap().unwrap();
        assert_ne!(first.1, changed.1);
    }

    #[test]
    fn shared_spawn_projects_typed_initial_submit_to_runtime_user_input() {
        let segments = vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::text("Implement Ticket 00001"),
        ];

        let input = initial_worker_input(&segments).expect("typed initial input");

        assert_eq!(input.kind, EmbeddedWorkerInputKind::User);
        assert_eq!(input.content, Segment::flatten_to_text(&segments));
        assert_eq!(input.segments, Some(segments));
        assert!(initial_worker_input(&[]).is_none());
    }

    #[test]
    fn embedded_runtime_with_execution_backend_routes_input_and_updates_status() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(AcceptingExecutionBackend::default()),
        )
        .expect("test backend should connect");
        let spawned = runtime.spawn_worker(test_create_binding(), embedded_spawn_request());
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker = spawned.worker.expect("created embedded worker");
        assert!(worker.capabilities.can_stop);

        let input = runtime.send_input(
            &worker.worker.worker_id,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: "hello".to_string(),
                segments: None,
            },
        );
        assert_eq!(input.state, WorkerOperationState::Accepted);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let detail = runtime
                .worker(&worker.worker.worker_id)
                .worker
                .expect("worker detail");
            if detail.state == "idle" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for embedded execution projection"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn embedded_runtime_registers_routes_input_without_internal_leaks() {
        let registry = RuntimeRegistry::for_workspace(
            EmbeddedWorkerRuntime::new_memory_with_execution_backend(
                "local:test",
                Arc::new(AcceptingExecutionBackend::default()),
            )
            .expect("test backend should connect"),
        );

        let runtimes = registry.list_runtimes(10);
        let embedded_summary = runtimes
            .items
            .iter()
            .find(|runtime| runtime.runtime_id == EMBEDDED_RUNTIME_ID)
            .expect("embedded runtime summary");
        assert_eq!(
            embedded_summary.source.kind,
            RuntimeSourceKind::EmbeddedWorkerRuntime
        );
        assert_eq!(embedded_summary.source.status, RuntimeSourceStatus::Active);
        assert!(embedded_summary.worker_creation_available);

        let spawned = registry
            .spawn_worker(
                EMBEDDED_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: Some("friendly-name-is-not-authority".to_string()),
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_control_operation: None,
                    resolved_workspace_api: Some(test_workspace_api()),
                    resolved_memory_settings: Some(test_memory_settings()),
                },
            )
            .unwrap();
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        assert!(
            spawned
                .acceptance_evidence
                .iter()
                .any(|evidence| evidence.kind == "embedded_runtime_backend_internal_projection")
        );
        let worker = spawned.worker.expect("created embedded worker");
        assert_eq!(worker.worker.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(worker.workspace.visibility, "backend_internal");
        assert_eq!(worker.workspace.identity, "runtime_registry_worker");
        assert_eq!(worker.implementation.kind, "embedded_worker_runtime");
        assert_eq!(worker.profile.as_deref(), Some("builtin:coder"));
        let input = registry
            .send_input(
                &worker.worker,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello embedded runtime".to_string(),
                    segments: None,
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);
        assert_eq!(input.worker.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(input.worker.worker_id, worker.worker.worker_id);

        let detail = registry.worker(&worker.worker).unwrap();

        let json = serde_json::to_string(&(embedded_summary, worker, input, detail)).unwrap();
        for forbidden in [
            "/workspace/project",
            "metadata.json",
            "session",
            "socket",
            "token",
            "credential",
            "provider",
            "transcript",
            "can_stream_events",
            "can_read_bounded_transcript",
        ] {
            assert!(
                !json.contains(forbidden),
                "embedded runtime projection leaked forbidden term: {forbidden}: {json}"
            );
        }
    }

    #[test]
    fn embedded_backend_syncs_config_bundle_and_spawns_with_bundle_ref() {
        let registry = RuntimeRegistry::new(vec![Arc::new(
            EmbeddedWorkerRuntime::new_memory_with_execution_backend(
                "local:test",
                Arc::new(AcceptingExecutionBackend::default()),
            )
            .unwrap(),
        )]);
        let bundle = test_config_bundle();
        let sync = registry
            .sync_config_bundle(EMBEDDED_RUNTIME_ID, bundle.clone())
            .unwrap();
        assert_eq!(sync.state, WorkerOperationState::Accepted);
        let reference = sync.availability.expect("bundle availability").reference;
        assert_eq!(reference.id, bundle.metadata.id);
        assert_eq!(reference.digest, bundle.metadata.digest);

        let check = registry
            .check_config_bundle(EMBEDDED_RUNTIME_ID, reference.clone())
            .unwrap();
        assert_eq!(check.state, WorkerOperationState::Accepted);

        let spawned = registry
            .spawn_worker(
                EMBEDDED_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_control_operation: None,
                    resolved_workspace_api: Some(test_workspace_api()),
                    resolved_memory_settings: Some(test_memory_settings()),
                },
            )
            .unwrap();
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        assert_eq!(
            spawned.worker.unwrap().profile.as_deref(),
            Some("builtin:coder")
        );
    }

    #[test]
    fn embedded_runtime_rejects_socket_ready_acceptance_without_socket_identity() {
        let registry = RuntimeRegistry::new(vec![Arc::new(
            EmbeddedWorkerRuntime::new_memory_with_execution_backend(
                "local:test",
                Arc::new(AcceptingExecutionBackend::default()),
            )
            .unwrap(),
        )]);
        let result = registry
            .spawn_worker(
                EMBEDDED_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::WorkspaceCompanion,
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::SocketReady,
                    profile: ProfileSelector::Builtin("builtin:companion".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_control_operation: None,
                    resolved_workspace_api: Some(test_workspace_api()),
                    resolved_memory_settings: Some(test_memory_settings()),
                },
            )
            .unwrap();
        assert_eq!(result.state, WorkerOperationState::Rejected);
        assert!(result.worker.is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| diag.code == "embedded_runtime_no_socket")
        );
    }

    #[tokio::test]
    async fn remote_runtime_client_can_initialize_inside_tokio_context() {
        let runtime = RemoteWorkerRuntime::new(
            RemoteRuntimeConfig::new(
                "remote:async-init",
                "Remote Async Init",
                "http://127.0.0.1:9",
                None,
            ),
            "workspace-test".to_string(),
            "http://127.0.0.1:8787".to_string(),
        )
        .unwrap();

        assert_eq!(runtime.runtime_id(), "remote:async-init");
    }

    #[test]
    fn remote_runtime_registry_routes_commands_without_browser_secret_leaks() {
        let worker_id = EmbeddedWorkerId::from_legacy_u64(1).to_string();
        let worker_json = worker_json("remote:primary", &worker_id);
        let (base_url, server) = serve_mock_http(vec![
            mock_response(
                "GET",
                "/v1/workers",
                true,
                200,
                json!({ "workers": [worker_json.clone()] }).to_string(),
            ),
            mock_response(
                "GET",
                format!("/v1/workers/{worker_id}"),
                true,
                200,
                json!({ "worker": worker_json.clone() }).to_string(),
            ),
            mock_response(
                "POST",
                format!("/v1/workers/{worker_id}/input"),
                true,
                200,
                json!({
                    "ack": {
                        "worker_ref": { "runtime_id": "remote:primary", "worker_id": worker_id.clone() },
                        "status": "running"
                    }
                })
                .to_string(),
            ),
        ]);
        let secret = "secret-token-do-not-leak".to_string();
        let registry = RuntimeRegistry::new(Vec::new());
        registry.register(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig::new(
                    "remote:primary",
                    "Remote Primary",
                    base_url.clone(),
                    Some(secret.clone()),
                ),
                "workspace-test".to_string(),
                "http://127.0.0.1:8787".to_string(),
            )
            .unwrap(),
        );

        let observation = registry
            .observation_source(&RuntimeWorkerRef::new("remote:primary", &worker_id))
            .expect("remote runtime exposes backend-owned WS observation source");
        let crate::observation::RuntimeObservationSource::RemoteWs(observation) = observation
        else {
            panic!("remote runtime should expose a remote WS observation source");
        };
        assert!(observation.endpoint.starts_with("ws://127.0.0.1:"));
        assert!(
            observation
                .endpoint
                .ends_with(&format!("/v1/workers/{worker_id}/protocol/ws"))
        );
        assert_eq!(observation.bearer_token.as_deref(), Some(secret.as_str()));

        let workers = registry.list_workers(10);
        assert_eq!(workers.items.len(), 1);
        assert_eq!(workers.items[0].worker.runtime_id, "remote:primary");
        assert_eq!(workers.items[0].worker.worker_id, worker_id.as_str());
        assert_eq!(
            workers.items[0].implementation.kind,
            "remote_worker_runtime"
        );
        assert_eq!(
            workers.items[0].workspace.identity,
            "runtime_registry_worker"
        );
        assert!(workers.items[0].capabilities.can_stop);

        let input = registry
            .send_input(
                &RuntimeWorkerRef::new("remote:primary", &worker_id),
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello remote".to_string(),
                    segments: None,
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);

        server.join().expect("mock remote server finished");
        let browser_payload = serde_json::to_string(&(workers, input)).unwrap();
        assert!(
            !browser_payload.contains(&base_url),
            "leaked base URL: {browser_payload}"
        );
        assert!(
            !browser_payload.contains(&secret),
            "leaked token: {browser_payload}"
        );
        assert!(browser_payload.contains("runtime_id"));
        assert!(browser_payload.contains("worker_id"));
    }

    #[test]
    fn remote_runtime_projection_uses_canonical_worker_status_for_stop_capability() {
        let worker_ids = (1..=4)
            .map(|value| EmbeddedWorkerId::from_legacy_u64(value).to_string())
            .collect::<Vec<_>>();
        let worker_id = worker_ids[0].clone();
        let (base_url, server) = serve_mock_http(vec![
            mock_response(
                "GET",
                "/v1/workers",
                true,
                200,
                json!({
                    "workers": [
                        worker_json_with_status("remote:primary", &worker_ids[0], "stopped"),
                        worker_json_with_status("remote:primary", &worker_ids[1], "cancelled"),
                        worker_json_with_status("remote:primary", &worker_ids[2], "paused"),
                        worker_json_with_status("remote:primary", &worker_ids[3], "idle")
                    ]
                })
                .to_string(),
            ),
            mock_response(
                "GET",
                format!("/v1/workers/{worker_id}"),
                true,
                200,
                json!({
                    "worker": worker_json_with_status(
                        "remote:primary",
                        &worker_ids[0],
                        "stopped"
                    )
                })
                .to_string(),
            ),
        ]);
        let registry = RuntimeRegistry::new(vec![Arc::new(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig::new(
                    "remote:primary",
                    "Remote Primary",
                    base_url,
                    Some("secret-token-do-not-leak".to_string()),
                ),
                "workspace-test".to_string(),
                "http://127.0.0.1:8787".to_string(),
            )
            .unwrap(),
        )]);

        let workers = registry.list_workers(10);
        assert_eq!(workers.items.len(), 4);
        assert!(!workers.items[0].capabilities.can_stop);
        assert!(!workers.items[1].capabilities.can_stop);
        assert!(workers.items[2].capabilities.can_stop);
        assert!(workers.items[3].capabilities.can_stop);
        assert_eq!(workers.items[0].state, "stopped");
        assert_eq!(workers.items[1].state, "cancelled");
        assert_eq!(workers.items[2].state, "paused");
        assert_eq!(workers.items[3].state, "idle");

        let stopped_detail = registry
            .worker(&RuntimeWorkerRef::new("remote:primary", &worker_id))
            .unwrap();
        assert!(!stopped_detail.capabilities.can_stop);
        assert_eq!(stopped_detail.state, "stopped");

        server.join().expect("mock remote server finished");
    }

    #[test]
    fn remote_config_bundle_sync_and_check_diagnostics_are_sanitized_and_path_safe() {
        let leaked_store_path = "/var/lib/yoi/runtime/bundles/bundle-1.json";
        let leaked_session_path = ".yoi/sessions/session.jsonl";
        let digest = "0".repeat(64);
        let (base_url, server) = serve_mock_http(vec![
            mock_response(
                "POST",
                "/v1/config-bundles",
                true,
                500,
                json!({
                    "error": {
                        "code": "store_io",
                        "message": format!("failed to write {leaked_store_path}")
                    }
                })
                .to_string(),
            ),
            mock_response(
                "GET",
                "/v1/config-bundles/bundle%2F1%3Fx/availability?digest=0000000000000000000000000000000000000000000000000000000000000000",
                true,
                400,
                json!({
                    "error": {
                        "code": "invalid_request",
                        "message": format!("invalid path {leaked_session_path}")
                    }
                })
                .to_string(),
            ),
        ]);
        let registry = RuntimeRegistry::new(Vec::new());
        registry.register(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig::new(
                    "remote:primary",
                    "Remote Primary",
                    base_url,
                    Some("secret-token".to_string()),
                ),
                "workspace-test".to_string(),
                "http://127.0.0.1:8787".to_string(),
            )
            .unwrap(),
        );

        let sync = registry
            .sync_config_bundle("remote:primary", test_config_bundle())
            .unwrap();
        assert_eq!(sync.state, WorkerOperationState::Rejected);
        let sync_payload = serde_json::to_string(&sync).unwrap();
        assert!(!sync_payload.contains(leaked_store_path), "{sync_payload}");

        let check = registry
            .check_config_bundle(
                "remote:primary",
                ConfigBundleRef {
                    id: "bundle/1?x".to_string(),
                    digest,
                },
            )
            .unwrap();
        assert_eq!(check.state, WorkerOperationState::Rejected);
        let check_payload = serde_json::to_string(&check).unwrap();
        assert!(
            !check_payload.contains(leaked_session_path),
            "{check_payload}"
        );
        assert!(!check_payload.contains(".yoi/sessions"), "{check_payload}");
        server.join().expect("mock remote server finished");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_workdir_session_uses_authenticated_http_operations_and_closes() {
        use workdir::{EntryKind, StatRequest, StatResult, WorkdirPath};

        let opened = workdir::http::OpenWorkdirSessionResponse {
            session_id: workdir::http::WorkdirSessionId::new("session-1").unwrap(),
            workdir_id: Workdir::new("wd-1").id().clone(),
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
        };
        let stat = workdir::http::WorkdirSessionOperationResult::Stat(StatResult {
            path: WorkdirPath::new("hello.txt").unwrap(),
            kind: EntryKind::File,
            size: 5,
        });
        let (base_url, server) = serve_mock_http(vec![
            mock_response(
                "POST",
                "/v1/working-directories/wd-1/sessions",
                true,
                200,
                serde_json::to_string(&opened).unwrap(),
            ),
            mock_response(
                "POST",
                "/v1/workdir-sessions/session-1/operations",
                true,
                200,
                serde_json::to_string(&stat).unwrap(),
            ),
            mock_response(
                "DELETE",
                "/v1/workdir-sessions/session-1",
                true,
                204,
                String::new(),
            ),
        ]);
        let runtime = RemoteWorkerRuntime::new(
            RemoteRuntimeConfig::new(
                "runtime-a",
                "Runtime A",
                base_url,
                Some("secret-token".to_string()),
            ),
            "workspace-a".to_string(),
            "http://backend.invalid".to_string(),
        )
        .unwrap();

        let runtime: Arc<dyn WorkspaceWorkerRuntime> = Arc::new(runtime);
        let session = runtime
            .open_workdir_session("wd-1", Some("1"))
            .await
            .expect("open remote Workdir session");
        let result = session
            .stat(StatRequest {
                path: WorkdirPath::new("hello.txt").unwrap(),
            })
            .await
            .expect("remote stat");
        assert_eq!(result.size, 5);
        session.close().await.expect("close remote session");
        session.close().await.expect("idempotent close");
        let error = session
            .stat(StatRequest {
                path: WorkdirPath::new("hello.txt").unwrap(),
            })
            .await
            .expect_err("closed session must reject local operation without another request");
        assert!(matches!(error, WorkdirError::Unavailable(_)));
        server.join().expect("mock remote server finished");
    }

    #[test]
    fn remote_runtime_auth_errors_map_to_typed_backend_error() {
        let (base_url, server) = serve_mock_http(vec![mock_response(
            "GET",
            "/v1/workers/999",
            true,
            401,
            json!({ "error": { "code": "unauthorized", "message": "bad token" } }).to_string(),
        )]);
        let registry = RuntimeRegistry::new(Vec::new());
        registry.register(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig::new(
                    "remote:primary",
                    "Remote Primary",
                    base_url,
                    Some("secret-token".to_string()),
                ),
                "workspace-test".to_string(),
                "http://127.0.0.1:8787".to_string(),
            )
            .unwrap(),
        );

        let error = registry
            .worker(&RuntimeWorkerRef::new("remote:primary", "999"))
            .expect_err("auth failure is a backend operation error");
        assert!(matches!(
            error,
            RuntimeRegistryError::RuntimeOperationFailed { runtime_id, code, .. }
                if runtime_id == "remote:primary" && code == "remote_runtime_auth_failed"
        ));
        server.join().expect("mock remote server finished");
    }

    #[derive(Clone)]
    struct MockResponse {
        method: &'static str,
        path: String,
        require_auth: bool,
        status: u16,
        body: String,
    }

    fn mock_response(
        method: &'static str,
        path: impl Into<String>,
        require_auth: bool,
        status: u16,
        body: String,
    ) -> MockResponse {
        MockResponse {
            method,
            path: path.into(),
            require_auth,
            status,
            body,
        }
    }

    fn serve_mock_http(responses: Vec<MockResponse>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for expected in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                let first_line = request.lines().next().unwrap_or_default();
                let expected_line = format!("{} {} ", expected.method, expected.path);
                assert!(
                    first_line.starts_with(&expected_line),
                    "unexpected request line: {first_line}, expected prefix {expected_line}"
                );
                if expected.require_auth {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer secret-token"),
                        "authorization header missing from request: {request}"
                    );
                }
                let status_text = match expected.status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    _ => "Mock",
                };
                let response = format!(
                    "HTTP/1.1 {} {status_text}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    expected.status,
                    expected.body.len(),
                    expected.body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (base_url, handle)
    }

    fn worker_json(runtime_id: &str, worker_id: &str) -> serde_json::Value {
        worker_json_with_status(runtime_id, worker_id, "idle")
    }

    fn worker_json_with_status(
        runtime_id: &str,
        worker_id: &str,
        status: &str,
    ) -> serde_json::Value {
        json!({
            "worker_ref": { "runtime_id": runtime_id, "worker_id": worker_id },
            "runtime_id": runtime_id,
            "worker_id": worker_id,
            "status": status,
            "intent": { "kind": "role", "role": "coder", "purpose": "remote test" },
            "profile": { "kind": "builtin", "value": "coder" },
            "profile_source": {
                "id": "remote-profile-source",
                "digest": "remote-profile-digest",
                "size_bytes": 0,
                "source_graph": { "source_count": 0, "total_source_bytes": 0, "entrypoints": {}, "import_count": 0 }
            },
            "config_bundle": { "id": "remote-bundle", "digest": "remote-digest" }
        })
    }
}
