use crate::Error;
use crate::resource_broker::BackendResourceBroker;
use chrono::Utc;
use reqwest::StatusCode;
use reqwest::blocking::{Client as BlockingHttpClient, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};
use worker_runtime::catalog::{
    ConfigBundleRef, CreateWorkerRequest, ProfileSelector, ProfileSourceArchiveHttpRef,
    ProfileSourceArchiveSource, WorkerDetail as EmbeddedWorkerDetail,
    WorkerStatus as EmbeddedWorkerStatus, WorkingDirectoryClaim, WorkingDirectoryRequest,
    WorkingDirectoryStatus, WorkingDirectorySummary, WorkspaceApiRef,
};
use worker_runtime::config_bundle::{ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary};
#[cfg(test)]
use worker_runtime::config_bundle::{
    ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
};
use worker_runtime::error::RuntimeError as EmbeddedRuntimeError;
use worker_runtime::execution::{
    WorkerExecutionBackendKind, WorkerExecutionRunState, WorkerExecutionStatus,
};
use worker_runtime::fs_store::FsRuntimeStoreOptions;
use worker_runtime::http_server::{
    RuntimeHttpConfigBundleAvailabilityResponse, RuntimeHttpConfigBundleSyncRequest,
    RuntimeHttpErrorResponse, RuntimeHttpSummaryResponse, RuntimeHttpWorkerCompletionsRequest,
    RuntimeHttpWorkerCompletionsResponse, RuntimeHttpWorkerDeleteResponse,
    RuntimeHttpWorkerInputResponse, RuntimeHttpWorkerLifecycleRequest,
    RuntimeHttpWorkerLifecycleResponse, RuntimeHttpWorkerResponse, RuntimeHttpWorkersResponse,
    RuntimeHttpWorkingDirectoriesResponse, RuntimeHttpWorkingDirectoryResponse,
};
use worker_runtime::identity::{WorkerId as EmbeddedWorkerId, WorkerRef as EmbeddedWorkerRef};
use worker_runtime::interaction::{
    WorkerInput as EmbeddedWorkerInput, WorkerInputKind as EmbeddedWorkerInputKind,
};
use worker_runtime::management::{RuntimeOptions as EmbeddedRuntimeOptions, RuntimeStatus};
use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};

const EMBEDDED_RUNTIME_ID: &str = "embedded-worker-runtime";
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
pub struct RuntimeCapabilitySummary {
    pub can_list_hosts: bool,
    pub can_list_workers: bool,
    pub can_get_worker: bool,
    pub can_spawn_worker: bool,
    pub can_stop_worker: bool,
    pub has_workspace_fs: bool,
    pub has_shell: bool,
    pub has_git: bool,
    pub supports_worktrees: bool,
    pub supports_backend_internal_tools: bool,
    pub workspace_scope: String,
    pub max_workers: usize,
    pub os: String,
    pub arch: String,
}

pub type HostCapabilitySummary = RuntimeCapabilitySummary;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub source: RuntimeSourceSummary,
    pub host_ids: Vec<String>,
    pub capabilities: RuntimeCapabilitySummary,
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
    pub capabilities: HostCapabilitySummary,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
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
    pub runtime_id: String,
    pub worker_id: String,
    pub host_id: String,
    pub label: String,
    pub role: Option<String>,
    pub profile: Option<String>,
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

fn is_retired_companion_worker(worker: &WorkerSummary) -> bool {
    worker.role.as_deref() == Some("builtin:companion")
        || worker.profile.as_deref() == Some("builtin:companion")
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
pub struct WorkerSpawnRequest {
    pub intent: WorkerSpawnIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_worker_name: Option<String>,
    pub acceptance: WorkerSpawnAcceptanceRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<EmbeddedWorkerInput>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationState {
    Accepted,
    Unsupported,
    Rejected,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerLifecycleResult {
    pub state: WorkerOperationState,
    pub runtime_id: String,
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<u64>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    System,
    Compact,
    ListRewindTargets,
    RegisterPeer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerDeleteResult {
    pub state: WorkerOperationState,
    pub runtime_id: String,
    pub worker_id: String,
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
    pub runtime_id: String,
    pub worker_id: String,
    pub kind: protocol::CompletionKind,
    pub prefix: String,
    pub entries: Vec<protocol::CompletionEntry>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInputResult {
    pub state: WorkerOperationState,
    pub runtime_id: String,
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<u64>,
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
        runtime_id: String,
        worker_id: String,
    },
    RuntimeOperationFailed {
        runtime_id: String,
        code: String,
        message: String,
    },
}

impl RuntimeRegistryError {
    pub fn into_error(self) -> Error {
        match self {
            Self::InvalidIdentifier { kind, value } => Error::InvalidRuntimeIdentifier {
                kind: kind.to_string(),
                value,
            },
            Self::UnknownRuntime(runtime_id) => Error::UnknownRuntime(runtime_id),
            Self::UnknownHost(host_id) => Error::UnknownHost(host_id),
            Self::UnknownWorker {
                runtime_id,
                worker_id,
            } => Error::UnknownWorker {
                runtime_id,
                worker_id,
            },
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

    fn worker(&self, worker_id: &str) -> WorkerLookupResult;

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

    fn spawn_worker(&self, request: WorkerSpawnRequest) -> WorkerSpawnResult {
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

    fn stop_worker(
        &self,
        worker_id: &str,
        _request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        WorkerLifecycleResult {
            state: WorkerOperationState::Unsupported,
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            event_id: None,
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
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            event_id: None,
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
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            deleted: false,
            diagnostics: vec![diagnostic(
                "worker_delete_unsupported",
                DiagnosticSeverity::Info,
                format!("runtime does not implement worker deletion for '{worker_id}'"),
            )],
        }
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
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            event_id: None,
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
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
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
                    .filter(|worker| !is_retired_companion_worker(worker))
                    .take(limit.saturating_sub(items.len())),
            );
        }
        diagnostics.truncate(MAX_DIAGNOSTICS);
        RuntimeList::new(items, diagnostics)
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
                    .filter(|worker| !is_retired_companion_worker(worker))
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

    pub fn worker(
        &self,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<WorkerSummary, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        let worker = lookup.worker.ok_or_else(|| {
            operation_failed_or_unknown_worker(runtime_id, worker_id, lookup.diagnostics)
        })?;
        if is_retired_companion_worker(&worker) {
            return Err(RuntimeRegistryError::UnknownWorker {
                runtime_id: runtime_id.to_string(),
                worker_id: worker_id.to_string(),
            });
        }
        Ok(worker)
    }

    pub fn spawn_worker(
        &self,
        runtime_id: &str,
        request: WorkerSpawnRequest,
    ) -> Result<WorkerSpawnResult, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        let runtime = self.runtime(runtime_id)?;
        Ok(runtime.spawn_worker(request))
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

    pub fn send_input(
        &self,
        runtime_id: &str,
        worker_id: &str,
        request: WorkerInputRequest,
    ) -> Result<WorkerInputResult, RuntimeRegistryError> {
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
        runtime_id: &str,
        worker_id: &str,
        request: WorkerCompletionsRequest,
    ) -> Result<WorkerCompletionsResult, RuntimeRegistryError> {
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
        runtime_id: &str,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResult, RuntimeRegistryError> {
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
        runtime_id: &str,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> Result<WorkerLifecycleResult, RuntimeRegistryError> {
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
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<WorkerDeleteResult, RuntimeRegistryError> {
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

    pub fn observation_source(
        &self,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<crate::observation::RuntimeObservationSource, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        runtime
            .observation_source(worker_id)
            .ok_or_else(|| RuntimeRegistryError::UnknownWorker {
                runtime_id: runtime_id.to_string(),
                worker_id: worker_id.to_string(),
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
    runtime_id: String,
    host_id: String,
    workspace_id: String,
    backend_base_url: Option<String>,
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
                display_name: Some("embedded".to_string()),
                limits: EmbeddedRuntimeOptions::default().limits,
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

    pub fn with_backend_base_url(mut self, backend_base_url: impl Into<String>) -> Self {
        self.backend_base_url = Some(backend_base_url.into().trim_end_matches('/').to_string());
        self
    }

    pub fn from_runtime(workspace_id: impl AsRef<str>, runtime: worker_runtime::Runtime) -> Self {
        let workspace_id = workspace_id.as_ref().to_string();
        Self {
            runtime_id: EMBEDDED_RUNTIME_ID.to_string(),
            host_id: host_id_for_embedded_workspace(&workspace_id),
            workspace_id,
            backend_base_url: None,
            runtime,
            execution_enabled: false,
            resource_broker: BackendResourceBroker::default(),
        }
    }

    fn worker_ref(&self, worker_id: &str) -> Option<EmbeddedWorkerRef> {
        Some(EmbeddedWorkerRef::new(EmbeddedWorkerId::parse(worker_id)?))
    }

    fn can_stop_embedded_worker(
        &self,
        status: EmbeddedWorkerStatus,
        execution: &worker_runtime::execution::WorkerExecutionStatus,
    ) -> bool {
        runtime_worker_can_stop(self.execution_enabled, status, execution)
    }

    fn map_worker_summary(&self, summary: worker_runtime::catalog::WorkerSummary) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: summary.worker_ref.worker_id.to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(&summary.worker_ref.worker_id.to_string()),
            role: embedded_profile_label(&summary.profile),
            profile: embedded_profile_label(&summary.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_execution_status_label(summary.status, &summary.execution)
                .to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: self.can_stop_embedded_worker(summary.status, &summary.execution),
                can_spawn_followup: false,
            },
            working_directory: summary
                .execution
                .working_directory
                .clone()
                .map(|status| status.summary),
            diagnostics: embedded_worker_projection_diagnostics(&summary.execution),
        }
    }

    fn map_worker_detail(&self, detail: EmbeddedWorkerDetail) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: detail.worker_id.to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(&detail.worker_id.to_string()),
            role: embedded_profile_label(&detail.profile),
            profile: embedded_profile_label(&detail.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_execution_status_label(detail.status, &detail.execution)
                .to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: self.can_stop_embedded_worker(detail.status, &detail.execution),
                can_spawn_followup: false,
            },
            working_directory: detail
                .execution
                .working_directory
                .clone()
                .map(|status| status.summary),
            diagnostics: embedded_worker_projection_diagnostics(&detail.execution),
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
                    capabilities: embedded_runtime_capabilities(limit, false, false),
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
            capabilities: embedded_runtime_capabilities(limit, true, self.execution_enabled),
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
                capabilities: embedded_runtime_capabilities(limit, true, self.execution_enabled),
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

    fn create_working_directory(
        &self,
        request: WorkingDirectoryRequest,
    ) -> RuntimeWorkingDirectoryResult {
        match self.runtime.create_working_directory(request) {
            Ok(working_directory) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(working_directory),
                diagnostics: Vec::new(),
            },
            Err(err) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn list_working_directories(&self) -> RuntimeList<WorkingDirectoryStatus> {
        match self.runtime.list_working_directories() {
            Ok(items) => RuntimeList::new(items, Vec::new()),
            Err(err) => RuntimeList::new(Vec::new(), vec![embedded_runtime_diagnostic(&err)]),
        }
    }

    fn working_directory(&self, working_directory_id: &str) -> RuntimeWorkingDirectoryResult {
        match self.runtime.working_directory(working_directory_id) {
            Ok(working_directory) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(working_directory),
                diagnostics: Vec::new(),
            },
            Err(err) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> RuntimeWorkingDirectoryResult {
        match self.runtime.cleanup_working_directory(working_directory_id) {
            Ok(working_directory) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Accepted,
                working_directory: Some(working_directory),
                diagnostics: Vec::new(),
            },
            Err(err) => RuntimeWorkingDirectoryResult {
                state: WorkerOperationState::Rejected,
                working_directory: None,
                diagnostics: vec![embedded_runtime_diagnostic(&err)],
            },
        }
    }

    fn spawn_worker(&self, request: WorkerSpawnRequest) -> WorkerSpawnResult {
        let mut diagnostics = Vec::new();
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
                "embedded_worker_name_ignored",
                DiagnosticSeverity::Info,
                "Embedded Runtime v0 allocates opaque runtime-local worker ids; requested display names are not authority".to_string(),
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

        let profile = request
            .profile
            .clone()
            .unwrap_or_else(|| embedded_profile_selector(&request.intent));
        let profile_source = match default_profile_source_archive_source(&profile) {
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
        let create_request = CreateWorkerRequest {
            profile,
            config_bundle: None,
            profile_source,
            initial_input: request.initial_input.clone(),
            working_directory_request: request.resolved_working_directory_request.clone(),
            working_directory: request.resolved_working_directory.clone(),
            workspace_api: self
                .backend_base_url
                .as_ref()
                .map(|base_url| WorkspaceApiRef {
                    workspace_id: self.workspace_id.clone(),
                    base_url: base_url.clone(),
                }),
        };
        match self.runtime.create_worker(create_request) {
            Ok(detail) => {
                let execution_failure =
                    embedded_spawn_execution_failure_diagnostic(&detail.execution);
                if let Some(diagnostic) = execution_failure {
                    diagnostics.push(diagnostic);
                    WorkerSpawnResult {
                        state: WorkerOperationState::Rejected,
                        worker: Some(self.map_worker_detail(detail)),
                        acceptance_evidence: Vec::new(),
                        diagnostics,
                    }
                } else {
                    WorkerSpawnResult {
                        state: WorkerOperationState::Accepted,
                        worker: Some(self.map_worker_detail(detail)),
                        acceptance_evidence: vec![
                            WorkerSpawnAcceptanceEvidence {
                                kind: "embedded_runtime_worker_created".to_string(),
                                detail: "worker-runtime catalog accepted a backend-internal tools-less Worker"
                                    .to_string(),
                            },
                            WorkerSpawnAcceptanceEvidence {
                                kind: "embedded_runtime_backend_internal_projection".to_string(),
                                detail:
                                    "only runtime_id plus worker_id backend projections were exposed"
                                        .to_string(),
                            },
                        ],
                        diagnostics,
                    }
                }
            }
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
            Ok(ack) => WorkerLifecycleResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                event_id: Some(ack.event_id),
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
            Ok(ack) => WorkerLifecycleResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                event_id: Some(ack.event_id),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: result.worker_id.to_string(),
                deleted: result.deleted,
                diagnostics: Vec::new(),
            },
            Err(error) => WorkerDeleteResult {
                state: WorkerOperationState::Rejected,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                deleted: false,
                diagnostics: vec![embedded_runtime_diagnostic(&error)],
            },
        }
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                runtime: self.runtime.clone(),
                worker_ref,
            },
        ))
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
                WorkerInputKind::System => EmbeddedWorkerInputKind::System,
                WorkerInputKind::Compact => EmbeddedWorkerInputKind::Compact,
                WorkerInputKind::ListRewindTargets => EmbeddedWorkerInputKind::ListRewindTargets,
                WorkerInputKind::RegisterPeer => EmbeddedWorkerInputKind::RegisterPeer,
            },
            content: request.content,
            segments: request.segments,
        };
        match self.runtime.send_input(&worker_ref, input) {
            Ok(ack) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                event_id: Some(ack.event_id),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                kind: request.kind,
                prefix: request.prefix,
                entries,
                diagnostics: Vec::new(),
            },
            Err(error) => WorkerCompletionsResult {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
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
    pub display_name: String,
    pub base_url: String,
    pub bearer_token: Option<String>,
    pub cached_capabilities: RuntimeCapabilitySummary,
    pub cached_status: String,
    pub timeout: Duration,
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
            .field("cached_capabilities", &self.cached_capabilities)
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
            display_name: display_name.into(),
            base_url: base_url.into(),
            bearer_token,
            cached_capabilities: remote_runtime_capabilities(
                200, false, false, "unknown", "unknown",
            ),
            cached_status: "configured".to_string(),
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_cached_capabilities(mut self, capabilities: RuntimeCapabilitySummary) -> Self {
        self.cached_capabilities = capabilities;
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
pub struct RemoteWorkerRuntime {
    runtime_id: String,
    display_name: String,
    base_url: String,
    backend_base_url: String,
    workspace_id: String,
    bearer_token: Option<String>,
    cached_capabilities: RuntimeCapabilitySummary,
    cached_status: String,
    host_id: String,
    resource_broker: BackendResourceBroker,
    http: BlockingHttpClient,
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
        Ok(Self {
            host_id: host_id_for_remote_runtime(&config.runtime_id),
            runtime_id: config.runtime_id,
            display_name: config.display_name,
            base_url,
            backend_base_url: backend_base_url.trim_end_matches('/').to_string(),
            workspace_id,
            bearer_token: config.bearer_token,
            cached_capabilities: config.cached_capabilities,
            cached_status: config.cached_status,
            resource_broker: BackendResourceBroker::default(),
            http,
        })
    }

    pub fn with_resource_broker(mut self, resource_broker: BackendResourceBroker) -> Self {
        self.resource_broker = resource_broker;
        self
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
        format!("{base}/v1/workers/{worker_id}/events/ws")
    }

    fn get_json<T>(&self, path: &str) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(self.http.get(self.endpoint(path)))
    }

    fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T, RuntimeDiagnostic>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(self.http.post(self.endpoint(path)).json(body))
    }

    fn delete_json<T>(&self, path: &str) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        self.send_json(self.http.delete(self.endpoint(path)))
    }

    fn send_json<T>(&self, request: RequestBuilder) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let runtime_id = self.runtime_id.clone();
        let bearer_token = self.bearer_token.clone();
        run_blocking_http(move || {
            let request = request.header(CONTENT_TYPE, "application/json");
            let request = if let Some(token) = bearer_token.as_deref() {
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
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: summary.worker_ref.worker_id.to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(&summary.worker_ref.worker_id.to_string()),
            role: None,
            profile: embedded_profile_label(&summary.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_execution_status_label(summary.status, &summary.execution)
                .to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: runtime_worker_can_stop(true, summary.status, &summary.execution),
                can_spawn_followup: false,
            },
            working_directory: summary.execution.working_directory.clone().map(|status| status.summary),
            diagnostics: vec![diagnostic(
                "remote_runtime_projection",
                DiagnosticSeverity::Info,
                "Remote Worker identity is projected only as runtime_id plus worker_id; endpoint and credentials remain backend-private".to_string(),
            )],
        }
    }

    fn map_worker_detail(&self, detail: EmbeddedWorkerDetail) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: detail.worker_id.to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(&detail.worker_id.to_string()),
            role: None,
            profile: embedded_profile_label(&detail.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_execution_status_label(detail.status, &detail.execution)
                .to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "transient".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: runtime_worker_can_stop(true, detail.status, &detail.execution),
                can_spawn_followup: false,
            },
            working_directory: detail.execution.working_directory.clone().map(|status| status.summary),
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
            runtime_id: self.runtime_id.clone(),
            worker_id: worker_id.to_string(),
            event_id: Some(response.ack.event_id),
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
                capabilities: remote_runtime_capabilities(
                    limit,
                    true,
                    response.runtime.worker_creation_available,
                    response.runtime.os,
                    response.runtime.arch,
                ),
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
                capabilities: self.cached_capabilities.clone(),
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
                capabilities: remote_runtime_capabilities(limit, true, false, "unknown", "unknown"),
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

    fn spawn_worker(&self, request: WorkerSpawnRequest) -> WorkerSpawnResult {
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
        let profile = request
            .profile
            .clone()
            .unwrap_or_else(|| embedded_profile_selector(&request.intent));
        let profile_source = match default_profile_source_archive_http_source(
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
        let create = CreateWorkerRequest {
            profile,
            config_bundle: None,
            profile_source,
            initial_input: request.initial_input.clone(),
            working_directory_request: request.resolved_working_directory_request.clone(),
            working_directory: request.resolved_working_directory.clone(),
            workspace_api: Some(WorkspaceApiRef {
                workspace_id: self.workspace_id.clone(),
                base_url: self.backend_base_url.clone(),
            }),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: response.worker.worker_id.to_string(),
                deleted: response.worker.deleted,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerDeleteResult {
                state: WorkerOperationState::Rejected,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                deleted: false,
                diagnostics: vec![diagnostic],
            },
        }
    }

    fn observation_source(
        &self,
        worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSource> {
        Some(crate::observation::RuntimeObservationSource::remote_ws(
            crate::observation::RuntimeObservationSourceConfig {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                endpoint: self.ws_endpoint(worker_id),
                bearer_token: self.bearer_token.clone(),
            },
        ))
    }

    fn send_input(&self, worker_id: &str, request: WorkerInputRequest) -> WorkerInputResult {
        let input = EmbeddedWorkerInput {
            kind: match request.kind {
                WorkerInputKind::User => EmbeddedWorkerInputKind::User,
                WorkerInputKind::System => EmbeddedWorkerInputKind::System,
                WorkerInputKind::Compact => EmbeddedWorkerInputKind::Compact,
                WorkerInputKind::ListRewindTargets => EmbeddedWorkerInputKind::ListRewindTargets,
                WorkerInputKind::RegisterPeer => EmbeddedWorkerInputKind::RegisterPeer,
            },
            content: request.content,
            segments: request.segments,
        };
        match self.post_json::<_, RuntimeHttpWorkerInputResponse>(
            &format!("/v1/workers/{worker_id}/input"),
            &input,
        ) {
            Ok(response) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                event_id: Some(response.ack.event_id),
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
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                kind: response.kind,
                prefix: response.prefix,
                entries: response.entries,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerCompletionsResult {
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                kind: request.kind,
                prefix: request.prefix,
                entries: Vec::new(),
                diagnostics: vec![diagnostic],
            },
        }
    }
}

fn embedded_runtime_capabilities(
    limit: usize,
    available: bool,
    execution_enabled: bool,
) -> RuntimeCapabilitySummary {
    RuntimeCapabilitySummary {
        can_list_hosts: true,
        can_list_workers: available,
        can_get_worker: available,
        can_spawn_worker: available,
        can_stop_worker: available && execution_enabled,
        has_workspace_fs: false,
        has_shell: false,
        has_git: false,
        supports_worktrees: false,
        supports_backend_internal_tools: true,
        workspace_scope: "backend_internal".to_string(),
        max_workers: limit,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

fn embedded_runtime_status_label(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Running => "running",
        RuntimeStatus::Stopped => "stopped",
    }
}

fn embedded_spawn_execution_failure_diagnostic(
    execution: &worker_runtime::execution::WorkerExecutionStatus,
) -> Option<RuntimeDiagnostic> {
    let result = execution.last_result.as_ref()?;
    let severity = match result.outcome {
        worker_runtime::execution::WorkerExecutionOutcome::Accepted => return None,
        worker_runtime::execution::WorkerExecutionOutcome::Rejected
        | worker_runtime::execution::WorkerExecutionOutcome::Busy
        | worker_runtime::execution::WorkerExecutionOutcome::Unsupported => {
            DiagnosticSeverity::Warning
        }
        worker_runtime::execution::WorkerExecutionOutcome::Errored => DiagnosticSeverity::Error,
    };
    let status = match result.outcome {
        worker_runtime::execution::WorkerExecutionOutcome::Accepted => "accepted",
        worker_runtime::execution::WorkerExecutionOutcome::Rejected => "rejected",
        worker_runtime::execution::WorkerExecutionOutcome::Busy => "busy",
        worker_runtime::execution::WorkerExecutionOutcome::Unsupported => "unsupported",
        worker_runtime::execution::WorkerExecutionOutcome::Errored => "errored",
    };
    Some(diagnostic(
        format!("embedded_worker_execution_spawn_{status}"),
        severity,
        format!(
            "Embedded Worker execution spawn was {status} during setup; check runtime configuration"
        ),
    ))
}

fn runtime_worker_can_stop(
    execution_enabled: bool,
    status: EmbeddedWorkerStatus,
    execution: &WorkerExecutionStatus,
) -> bool {
    execution_enabled
        && status == EmbeddedWorkerStatus::Running
        && execution.backend == worker_runtime::execution::WorkerExecutionBackendKind::Connected
        && !matches!(
            execution.run_state,
            WorkerExecutionRunState::Stopped
                | WorkerExecutionRunState::Rejected
                | WorkerExecutionRunState::Errored
                | WorkerExecutionRunState::Unconnected
        )
        && !execution_last_result_blocks_control(execution)
}

fn execution_last_result_blocks_control(execution: &WorkerExecutionStatus) -> bool {
    execution.last_result.as_ref().is_some_and(|result| {
        matches!(
            result.outcome,
            worker_runtime::execution::WorkerExecutionOutcome::Rejected
                | worker_runtime::execution::WorkerExecutionOutcome::Errored
                | worker_runtime::execution::WorkerExecutionOutcome::Unsupported
        )
    })
}

fn embedded_worker_status_label(status: EmbeddedWorkerStatus) -> &'static str {
    match status {
        EmbeddedWorkerStatus::Running => "running",
        EmbeddedWorkerStatus::Stopped => "stopped",
        EmbeddedWorkerStatus::Cancelled => "cancelled",
    }
}

fn embedded_worker_projection_diagnostics(
    execution: &WorkerExecutionStatus,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = vec![diagnostic(
        "embedded_runtime_projection",
        DiagnosticSeverity::Info,
        "Worker identity is projected only as runtime_id plus worker_id; embedded runtime internals remain backend-private".to_string(),
    )];

    if execution.backend == WorkerExecutionBackendKind::Stale {
        diagnostics.push(diagnostic(
            "embedded_worker_execution_stale",
            DiagnosticSeverity::Warning,
            "Worker execution handle is not connected in this server process; persisted execution binding was marked stale".to_string(),
        ));
    } else if execution.backend == WorkerExecutionBackendKind::Unconnected
        || execution.run_state == WorkerExecutionRunState::Unconnected
    {
        diagnostics.push(diagnostic(
            "embedded_worker_execution_unconnected",
            DiagnosticSeverity::Warning,
            "Worker execution handle is not connected in this server process".to_string(),
        ));
    } else if execution.run_state == WorkerExecutionRunState::Rejected {
        diagnostics.push(diagnostic(
            "embedded_worker_execution_spawn_rejected",
            DiagnosticSeverity::Error,
            "Worker execution spawn was rejected; backend-private details are not exposed"
                .to_string(),
        ));
    }

    diagnostics
}

fn embedded_worker_execution_status_label(
    status: EmbeddedWorkerStatus,
    execution: &WorkerExecutionStatus,
) -> &'static str {
    match status {
        EmbeddedWorkerStatus::Stopped => "stopped",
        EmbeddedWorkerStatus::Cancelled => "cancelled",
        EmbeddedWorkerStatus::Running => {
            if execution.backend == worker_runtime::execution::WorkerExecutionBackendKind::Stale {
                return "stale";
            }
            match execution.run_state {
                WorkerExecutionRunState::Idle => "idle",
                WorkerExecutionRunState::Busy => "running",
                WorkerExecutionRunState::Stopped => "stopped",
                WorkerExecutionRunState::Rejected => "rejected",
                WorkerExecutionRunState::Errored => "errored",
                WorkerExecutionRunState::Unconnected => "unconnected",
            }
        }
    }
}

fn default_profile_source_archive_source(
    profile: &ProfileSelector,
) -> Result<ProfileSourceArchiveSource, String> {
    Ok(ProfileSourceArchiveSource::Embedded {
        archive: default_profile_source_archive(profile)?,
    })
}

fn default_profile_source_archive_http_source(
    profile: &ProfileSelector,
    workspace_id: &str,
    runtime_id: Option<&str>,
    resource_broker: &BackendResourceBroker,
    backend_base_url: &str,
) -> Result<ProfileSourceArchiveSource, String> {
    let archive = default_profile_source_archive(profile)?;
    let _handle = resource_broker.issue_profile_source_archive_handle(
        workspace_id.to_string(),
        runtime_id,
        None,
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
fn default_embedded_config_bundle(
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
    let archive = default_profile_source_archive(profile)?;
    let (profile_source_archive, profile_source_archive_handle) = match archive_transport {
        ProfileSourceArchiveTransport::Inline => (Some(archive), None),
        ProfileSourceArchiveTransport::BackendResourceHandle => {
            let handle = resource_broker.issue_profile_source_archive_handle(
                workspace_id.to_string(),
                runtime_id,
                None,
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
        profile_source_archive,
        profile_source_archive_handle,
    }
    .with_computed_digest())
}

fn default_profile_source_archive(
    profile: &ProfileSelector,
) -> Result<ProfileSourceArchive, String> {
    let selected = embedded_profile_label(profile).unwrap_or_else(|| "default".to_string());
    let selected_path = embedded_profile_path(profile)?;
    let mut entrypoints = BTreeMap::new();
    entrypoints.insert("default".to_string(), "profiles/default.dcdl".to_string());
    entrypoints.insert(
        "builtin:default".to_string(),
        "profiles/default.dcdl".to_string(),
    );
    for slug in ["companion", "intake", "orchestrator", "coder", "reviewer"] {
        entrypoints.insert(format!("builtin:{slug}"), format!("profiles/{slug}.dcdl"));
    }
    entrypoints.insert(selected, selected_path);

    let mut sources = BTreeMap::new();
    sources.insert(
        "profiles/default.dcdl".to_string(),
        include_str!("../../../resources/profiles/default.dcdl").to_string(),
    );
    sources.insert(
        "profiles/companion.dcdl".to_string(),
        include_str!("../../../resources/profiles/companion.dcdl").to_string(),
    );
    sources.insert(
        "profiles/intake.dcdl".to_string(),
        include_str!("../../../resources/profiles/intake.dcdl").to_string(),
    );
    sources.insert(
        "profiles/orchestrator.dcdl".to_string(),
        include_str!("../../../resources/profiles/orchestrator.dcdl").to_string(),
    );
    sources.insert(
        "profiles/coder.dcdl".to_string(),
        include_str!("../../../resources/profiles/coder.dcdl").to_string(),
    );
    sources.insert(
        "profiles/reviewer.dcdl".to_string(),
        include_str!("../../../resources/profiles/reviewer.dcdl").to_string(),
    );

    let mut imports = BTreeMap::new();
    for slug in ["companion", "intake", "orchestrator", "coder", "reviewer"] {
        imports.insert(
            format!("profiles/{slug}.dcdl\0./default.dcdl"),
            "profiles/default.dcdl".to_string(),
        );
    }

    ProfileSourceArchive::build(ProfileSourceArchiveInput {
        id: "builtin-decodal-profiles-v1".to_string(),
        entrypoints,
        imports,
        sources,
    })
    .map_err(|err| err.to_string())
}

fn embedded_profile_path(profile: &ProfileSelector) -> Result<String, String> {
    match profile {
        ProfileSelector::RuntimeDefault => Ok("profiles/default.dcdl".to_string()),
        ProfileSelector::Builtin(name) => match name.strip_prefix("builtin:").unwrap_or(name) {
            "default" => Ok("profiles/default.dcdl".to_string()),
            "companion" => Ok("profiles/companion.dcdl".to_string()),
            "intake" => Ok("profiles/intake.dcdl".to_string()),
            "orchestrator" => Ok("profiles/orchestrator.dcdl".to_string()),
            "coder" => Ok("profiles/coder.dcdl".to_string()),
            "reviewer" => Ok("profiles/reviewer.dcdl".to_string()),
            other => Err(format!("unknown builtin profile selector: builtin:{other}")),
        },
        ProfileSelector::Named(name) => Err(format!("unknown named profile selector: {name}")),
    }
}

fn embedded_profile_selector(intent: &WorkerSpawnIntent) -> ProfileSelector {
    match intent {
        WorkerSpawnIntent::TicketRole { role, .. } => {
            ProfileSelector::Builtin(format!("builtin:{}", ticket_role_profile_slug(role)))
        }
        WorkerSpawnIntent::WorkspaceCompanion => {
            ProfileSelector::Builtin("builtin:companion".to_string())
        }
        WorkerSpawnIntent::WorkspaceOrchestrator => ProfileSelector::RuntimeDefault,
        WorkerSpawnIntent::WorkspaceCoding => ProfileSelector::Builtin("builtin:coder".to_string()),
    }
}

fn ticket_role_profile_slug(role: &TicketWorkerRole) -> &'static str {
    match role {
        TicketWorkerRole::Intake => "intake",
        TicketWorkerRole::Orchestrator => "orchestrator",
        TicketWorkerRole::Coder => "coder",
        TicketWorkerRole::Reviewer => "reviewer",
    }
}

fn embedded_profile_label(profile: &ProfileSelector) -> Option<String> {
    Some(match profile {
        ProfileSelector::RuntimeDefault => "runtime_default".to_string(),
        ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => safe_display_hint(name),
    })
}

fn embedded_input_rejected(
    runtime_id: &str,
    worker_id: &str,
    diagnostic: RuntimeDiagnostic,
) -> WorkerInputResult {
    WorkerInputResult {
        state: WorkerOperationState::Rejected,
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        event_id: None,
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
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        event_id: None,
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
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        event_id: None,
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
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        event_id: None,
        diagnostics: vec![diagnostic],
    }
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
            operation, outcome, ..
        } => diagnostic(
            "embedded_worker_execution_rejected",
            DiagnosticSeverity::Warning,
            format!("Embedded Worker execution backend rejected {operation:?} with {outcome:?}"),
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

fn remote_runtime_capabilities(
    limit: usize,
    available: bool,
    worker_creation_available: bool,
    os: impl Into<String>,
    arch: impl Into<String>,
) -> RuntimeCapabilitySummary {
    RuntimeCapabilitySummary {
        can_list_hosts: true,
        can_list_workers: available,
        can_get_worker: available,
        can_spawn_worker: available && worker_creation_available,
        can_stop_worker: available,
        has_workspace_fs: false,
        has_shell: false,
        has_git: false,
        supports_worktrees: false,
        supports_backend_internal_tools: false,
        workspace_scope: "remote_runtime_backend_private".to_string(),
        max_workers: limit,
        os: os.into(),
        arch: arch.into(),
    }
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
            runtime_id: runtime_id.to_string(),
            worker_id: worker_id.to_string(),
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
        runtime_id: "placeholder".to_string(),
        worker_id: "worker-placeholder".to_string(),
        host_id,
        label: "Worker runtime actions are not implemented".to_string(),
        role: None,
        profile: None,
        workspace: WorkerWorkspaceSummary {
            visibility: "none".to_string(),
            identity: "unsupported".to_string(),
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

    #[test]
    fn embedded_builtin_decodal_profiles_resolve_through_archive() {
        let root = tempfile::tempdir().unwrap();
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        for selector in [
            ProfileSelector::RuntimeDefault,
            ProfileSelector::Builtin("builtin:companion".to_string()),
            ProfileSelector::Builtin("builtin:intake".to_string()),
            ProfileSelector::Builtin("builtin:orchestrator".to_string()),
            ProfileSelector::Builtin("builtin:coder".to_string()),
            ProfileSelector::Builtin("builtin:reviewer".to_string()),
        ] {
            let bundle = default_embedded_config_bundle(
                &selector,
                "workspace-test",
                Some(runtime_id),
                &broker,
                ProfileSourceArchiveTransport::BackendResourceHandle,
            )
            .unwrap();
            let handle = bundle.profile_source_archive_handle.as_ref().unwrap();
            assert!(bundle.profile_source_archive.is_none());
            let response = broker
                .fetch_profile_source_archive(
                    worker_runtime::resource::BackendResourceFetchRequest {
                        handle: handle.clone(),
                        runtime_id: runtime_id.to_string(),
                        worker_id: None,
                        audit_correlation_id: handle.audit_correlation_id.clone(),
                    },
                )
                .unwrap();
            let archive =
                worker_runtime::resource::profile_source_archive_from_response(handle, response)
                    .unwrap()
                    .verify()
                    .unwrap();
            let selector_key = match &selector {
                ProfileSelector::RuntimeDefault => "default".to_string(),
                ProfileSelector::Builtin(name) => name.clone(),
                ProfileSelector::Named(name) => name.clone(),
            };
            let manifest = archive
                .resolve_profile(&selector_key, root.path(), "embedded-test-worker")
                .unwrap();
            assert_eq!(manifest.worker.name, "embedded-test-worker");
            assert_eq!(manifest.model.ref_.as_deref(), Some("codex-oauth/gpt-5.5"));
        }
    }

    #[test]
    fn remote_default_bundle_inlines_profile_archive_for_standalone_runtime() {
        let root = tempfile::tempdir().unwrap();
        let broker = BackendResourceBroker::default();
        let runtime_id = "remote:test";
        let bundle = default_embedded_config_bundle(
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
            .expect("remote default bundle carries inline profile archive")
            .verify()
            .unwrap();
        let manifest = archive
            .resolve_profile("builtin:coder", root.path(), "remote-test-worker")
            .unwrap();
        assert_eq!(manifest.worker.name, "remote-test-worker");
    }

    #[test]
    fn remote_profile_source_archive_url_uses_workspace_id_not_host_id() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "remote:test";
        let source = default_profile_source_archive_http_source(
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
            default_embedded_config_bundle(
                &ProfileSelector::Builtin("builtin:missing".to_string()),
                "workspace-test",
                Some(runtime_id),
                &broker,
                ProfileSourceArchiveTransport::BackendResourceHandle,
            )
            .is_err()
        );
        assert!(
            default_embedded_config_bundle(
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
                    "provider setup failed at /tmp/secret-provider-config",
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
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::Input,
                WorkerExecutionRunState::Busy,
            )
        }
    }

    #[derive(Clone)]
    struct FixtureRuntime {
        runtime_id: String,
        host_id: String,
        workers: Vec<WorkerSummary>,
    }

    impl FixtureRuntime {
        fn with_worker(runtime_id: &str, host_id: &str, worker_id: &str, label: &str) -> Self {
            Self {
                runtime_id: runtime_id.to_string(),
                host_id: host_id.to_string(),
                workers: vec![WorkerSummary {
                    runtime_id: runtime_id.to_string(),
                    worker_id: worker_id.to_string(),
                    host_id: host_id.to_string(),
                    label: label.to_string(),
                    role: None,
                    profile: None,
                    workspace: WorkerWorkspaceSummary {
                        visibility: "opaque".to_string(),
                        identity: host_id.to_string(),
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
            }
        }
    }

    impl WorkspaceWorkerRuntime for FixtureRuntime {
        fn runtime_id(&self) -> &str {
            &self.runtime_id
        }

        fn runtime_summary(&self, _limit: usize) -> RuntimeSummary {
            RuntimeSummary {
                runtime_id: self.runtime_id.clone(),
                label: self.runtime_id.clone(),
                kind: "fixture".to_string(),
                status: "available".to_string(),
                source: RuntimeSourceSummary::embedded_worker_runtime_reserved(),
                host_ids: vec![self.host_id.clone()],
                capabilities: RuntimeCapabilitySummary {
                    can_list_hosts: true,
                    can_list_workers: true,
                    can_get_worker: true,
                    can_spawn_worker: false,
                    can_stop_worker: false,
                    has_workspace_fs: false,
                    has_shell: false,
                    has_git: false,
                    supports_worktrees: false,
                    supports_backend_internal_tools: false,
                    workspace_scope: "none".to_string(),
                    max_workers: self.workers.len(),
                    os: "test".to_string(),
                    arch: "test".to_string(),
                },
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
                    capabilities: self.runtime_summary(1).capabilities,
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
                    .find(|worker| worker.worker_id == worker_id)
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

        let from_runtime_b = registry.worker("runtime-b", "shared-worker").unwrap();
        assert_eq!(from_runtime_b.runtime_id, "runtime-b");
        assert_eq!(from_runtime_b.host_id, "host-b");
        assert_eq!(from_runtime_b.label, "worker from runtime b");

        let from_runtime_a = registry.worker("runtime-a", "shared-worker").unwrap();
        assert_eq!(from_runtime_a.runtime_id, "runtime-a");
        assert_eq!(from_runtime_a.host_id, "host-a");
        assert_eq!(from_runtime_a.label, "worker from runtime a");
    }

    #[test]
    fn registry_worker_lookup_reports_unknown_runtime_and_worker_separately() {
        let registry = RuntimeRegistry::new(vec![Arc::new(FixtureRuntime::with_worker(
            "runtime-a",
            "host-a",
            "worker-a",
            "worker from runtime a",
        ))]);

        let unknown_runtime = registry.worker("runtime-missing", "worker-a").unwrap_err();
        assert_eq!(
            unknown_runtime,
            RuntimeRegistryError::UnknownRuntime("runtime-missing".to_string())
        );
        assert!(matches!(
            unknown_runtime.into_error(),
            Error::UnknownRuntime(runtime_id) if runtime_id == "runtime-missing"
        ));

        let unknown_worker = registry.worker("runtime-a", "999").unwrap_err();
        assert_eq!(
            unknown_worker,
            RuntimeRegistryError::UnknownWorker {
                runtime_id: "runtime-a".to_string(),
                worker_id: "999".to_string(),
            }
        );
        assert!(matches!(
            unknown_worker.into_error(),
            Error::UnknownWorker { runtime_id, worker_id }
                if runtime_id == "runtime-a" && worker_id == "999"
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
            profile: None,
            initial_input: None,
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
        }
    }

    #[test]
    fn embedded_runtime_spawn_execution_failure_is_rejected_and_not_input_capable() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(FailingSpawnBackend),
        )
        .expect("test backend should connect");
        let spawned = runtime.spawn_worker(embedded_spawn_request());
        assert_eq!(spawned.state, WorkerOperationState::Rejected);
        assert!(spawned.acceptance_evidence.is_empty());
        assert!(spawned.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "embedded_worker_execution_rejected"
                && !diagnostic.message.contains("/tmp/secret-provider-config")
        }));
        assert!(spawned.worker.is_none());
    }

    #[test]
    fn embedded_runtime_rejects_system_initial_input_without_worker_projection() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(AcceptingExecutionBackend::default()),
        )
        .expect("test backend should connect");
        let mut request = embedded_spawn_request();
        request.initial_input = Some(EmbeddedWorkerInput {
            kind: EmbeddedWorkerInputKind::System,
            content: "system/role instruction belongs in profile".to_string(),
            segments: None,
        });

        let spawned = runtime.spawn_worker(request);
        assert_eq!(spawned.state, WorkerOperationState::Rejected);
        assert!(spawned.worker.is_none());
        assert!(spawned.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "embedded_worker_initial_input_kind_invalid"
                && diagnostic
                    .message
                    .contains("initial worker input must be user input")
        }));
        assert!(runtime.list_workers(10).items.is_empty());
    }

    #[test]
    fn embedded_runtime_with_execution_backend_routes_input_and_updates_status() {
        let runtime = EmbeddedWorkerRuntime::new_memory_with_execution_backend(
            "local:test",
            Arc::new(AcceptingExecutionBackend::default()),
        )
        .expect("test backend should connect");
        let spawned = runtime.spawn_worker(embedded_spawn_request());
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker = spawned.worker.expect("created embedded worker");
        assert!(worker.capabilities.can_stop);

        let input = runtime.send_input(
            &worker.worker_id,
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
                .worker(&worker.worker_id)
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
        assert!(embedded_summary.capabilities.can_spawn_worker);

        let spawned = registry
            .spawn_worker(
                EMBEDDED_RUNTIME_ID,
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: Some("friendly-name-is-not-authority".to_string()),
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: None,
                    initial_input: None,
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
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
        assert_eq!(worker.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(worker.workspace.visibility, "backend_internal");
        assert_eq!(worker.workspace.identity, "runtime_registry_worker");
        assert_eq!(worker.implementation.kind, "embedded_worker_runtime");
        assert_eq!(worker.profile.as_deref(), Some("builtin:coder"));
        let input = registry
            .send_input(
                EMBEDDED_RUNTIME_ID,
                &worker.worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello embedded runtime".to_string(),
                    segments: None,
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);
        assert_eq!(input.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(input.worker_id, worker.worker_id);

        let detail = registry
            .worker(EMBEDDED_RUNTIME_ID, &worker.worker_id)
            .unwrap();

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
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: Some(ProfileSelector::Builtin("builtin:coder".to_string())),
                    initial_input: None,
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
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
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::WorkspaceCompanion,
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::SocketReady,
                    profile: None,
                    initial_input: None,
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
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
        let worker_json = worker_json("remote:primary", "1");
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
                "/v1/workers/1",
                true,
                200,
                json!({ "worker": worker_json.clone() }).to_string(),
            ),
            mock_response(
                "POST",
                "/v1/workers/1/input",
                true,
                200,
                json!({
                    "ack": {
                        "worker_ref": { "runtime_id": "remote:primary", "worker_id": 1 },
                        "status": "running",
                        "event_id": 8
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
            .observation_source("remote:primary", "1")
            .expect("remote runtime exposes backend-owned WS observation source");
        let crate::observation::RuntimeObservationSource::RemoteWs(observation) = observation
        else {
            panic!("remote runtime should expose a remote WS observation source");
        };
        assert!(observation.endpoint.starts_with("ws://127.0.0.1:"));
        assert!(observation.endpoint.ends_with("/v1/workers/1/events/ws"));
        assert_eq!(observation.bearer_token.as_deref(), Some(secret.as_str()));

        let workers = registry.list_workers(10);
        assert_eq!(workers.items.len(), 1);
        assert_eq!(workers.items[0].runtime_id, "remote:primary");
        assert_eq!(workers.items[0].worker_id, "1");
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
                "remote:primary",
                "1",
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello remote".to_string(),
                    segments: None,
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);
        assert_eq!(input.event_id, Some(8));

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
    fn remote_runtime_projection_blocks_stale_and_unconnected_execution_input() {
        let (base_url, server) = serve_mock_http(vec![
            mock_response(
                "GET",
                "/v1/workers",
                true,
                200,
                json!({
                    "workers": [
                    worker_json_with_execution(
                        "remote:primary",
                        "1",
                        "stale",
                        "unconnected",
                        None,
                    ),
                    worker_json_with_execution(
                        "remote:primary",
                        "2",
                        "unconnected",
                        "unconnected",
                        None,
                    ),
                    worker_json_with_execution(
                        "remote:primary",
                        "3",
                        "connected",
                        "rejected",
                        Some("rejected"),
                    ),
                    worker_json_with_execution(
                        "remote:primary",
                        "4",
                        "connected",
                        "errored",
                        Some("errored"),
                    )
                    ]
                })
                .to_string(),
            ),
            mock_response(
                "GET",
                "/v1/workers/1",
                true,
                200,
                json!({
                    "worker": worker_json_with_execution(
                    "remote:primary",
                    "1",
                    "stale",
                    "unconnected",
                    None,
                )})
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
        for worker in &workers.items {
            assert!(
                !worker.capabilities.can_stop,
                "{} should not be stoppable",
                worker.worker_id
            );
        }
        assert_eq!(workers.items[0].state, "stale");
        assert_eq!(workers.items[1].state, "unconnected");
        assert_eq!(workers.items[2].state, "rejected");
        assert_eq!(workers.items[3].state, "errored");

        let stale_detail = registry.worker("remote:primary", "1").unwrap();
        assert!(!stale_detail.capabilities.can_stop);
        assert_eq!(stale_detail.state, "stale");

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
            .worker("remote:primary", "999")
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
        path: &'static str,
        require_auth: bool,
        status: u16,
        body: String,
    }

    fn mock_response(
        method: &'static str,
        path: &'static str,
        require_auth: bool,
        status: u16,
        body: String,
    ) -> MockResponse {
        MockResponse {
            method,
            path,
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
        worker_json_with_execution(runtime_id, worker_id, "connected", "idle", None)
    }

    fn worker_json_with_execution(
        runtime_id: &str,
        worker_id: &str,
        backend: &str,
        run_state: &str,
        last_outcome: Option<&str>,
    ) -> serde_json::Value {
        let last_result = last_outcome.map(|outcome| {
            json!({
                "operation": "input",
                "outcome": outcome,
                "run_state": run_state,
                "message": format!("{outcome} result")
            })
        });
        let worker_id = worker_id.parse::<u64>().unwrap();
        json!({
            "worker_ref": { "runtime_id": runtime_id, "worker_id": worker_id },
            "runtime_id": runtime_id,
            "worker_id": worker_id,
            "status": "running",
            "execution": { "backend": backend, "run_state": run_state, "last_result": last_result },
            "intent": { "kind": "role", "role": "coder", "purpose": "remote test" },
            "profile": { "kind": "builtin", "value": "coder" },
            "profile_source": {
                "id": "remote-profile-source",
                "digest": "remote-profile-digest",
                "size_bytes": 0,
                "source_graph": { "source_count": 0, "total_source_bytes": 0, "entrypoints": {}, "import_count": 0 }
            },
            "config_bundle": { "id": "remote-bundle", "digest": "remote-digest" },
            "last_event_id": 0
        })
    }
}
