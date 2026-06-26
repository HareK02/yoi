use crate::Error;
use chrono::Utc;
use pod_store::WorkerMetadata;
use reqwest::StatusCode;
use reqwest::blocking::{Client as BlockingHttpClient, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use worker_runtime::catalog::{
    CreateWorkerRequest, ProfileSelector, WorkerDetail as EmbeddedWorkerDetail, WorkerIntent,
    WorkerStatus as EmbeddedWorkerStatus,
};
use worker_runtime::error::RuntimeError as EmbeddedRuntimeError;
use worker_runtime::http_server::{
    RuntimeHttpErrorResponse, RuntimeHttpSummaryResponse, RuntimeHttpTranscriptResponse,
    RuntimeHttpWorkerInputResponse, RuntimeHttpWorkerLifecycleRequest,
    RuntimeHttpWorkerLifecycleResponse, RuntimeHttpWorkerResponse, RuntimeHttpWorkersResponse,
};
use worker_runtime::identity::{
    RuntimeId as EmbeddedRuntimeId, WorkerId as EmbeddedWorkerId, WorkerRef as EmbeddedWorkerRef,
};
use worker_runtime::interaction::{
    WorkerInput as EmbeddedWorkerInput, WorkerInputKind as EmbeddedWorkerInputKind,
};
use worker_runtime::management::{RuntimeOptions as EmbeddedRuntimeOptions, RuntimeStatus};
use worker_runtime::observation::{
    TranscriptProjection as EmbeddedTranscriptProjection, TranscriptQuery, TranscriptRole,
};

const LOCAL_RUNTIME_ID: &str = "local-worker-runtime";
const EMBEDDED_RUNTIME_ID: &str = "embedded-worker-runtime";
const LOCAL_HOST_KIND: &str = "local-worker-host";
const EMBEDDED_HOST_KIND: &str = "embedded-worker-runtime-host";
const REMOTE_HOST_KIND: &str = "remote-worker-runtime-host";
const MAX_DIAGNOSTICS: usize = 16;
const MAX_HOST_SCAN: usize = 256;
const MAX_IDENTIFIER_LEN: usize = 120;
const ID_DIGEST_HEX_LEN: usize = 16;

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
    /// Compatibility projection over the existing local Pod metadata store.
    LocalCompatibility,
    /// Reserved boundary for the future in-process worker-runtime Runtime adapter.
    EmbeddedWorkerRuntime,
    /// Reserved boundary for a future remote Workspace Runtime adapter.
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
    /// compatibility-store names, socket addresses, session ids, or paths.
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
    pub fn local_compatibility() -> Self {
        Self {
            kind: RuntimeSourceKind::LocalCompatibility,
            status: RuntimeSourceStatus::Active,
            identity_authority: RuntimeIdentityAuthority::RuntimeRegistryProjection,
            note: "read-only compatibility projection over local Worker metadata; no socket, session, or path authority is exposed".to_string(),
        }
    }

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
    pub can_accept_input: bool,
    pub can_stream_events: bool,
    pub can_read_bounded_transcript: bool,
    pub has_workspace_fs: bool,
    pub has_shell: bool,
    pub has_git: bool,
    pub supports_worktrees: bool,
    pub supports_backend_internal_tools: bool,
    pub local_pod_inspection: String,
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
    pub can_accept_input: bool,
    pub can_stream_events: bool,
    pub can_stop: bool,
    pub can_spawn_followup: bool,
    pub can_read_bounded_transcript: bool,
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
    pub status: String,
    pub last_seen_at: Option<String>,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
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

/// Browser-safe worker spawn request shape.
///
/// The request intentionally carries only workspace policy intents and stable
/// worker identifiers. Raw workspace roots, child cwd, executable path, and raw
/// profile selectors are resolved by the runtime service and never accepted from
/// Workspace API callers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSpawnRequest {
    pub intent: WorkerSpawnIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_worker_name: Option<String>,
    pub acceptance: WorkerSpawnAcceptanceRequirement,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerSpawnIntent {
    WorkspaceCompanion,
    WorkspaceOrchestrator,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInputRequest {
    #[serde(default = "default_worker_input_kind")]
    pub kind: WorkerInputKind,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInputResult {
    pub state: WorkerOperationState,
    pub runtime_id: String,
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<u64>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerTranscriptItem {
    pub sequence: u64,
    pub role: String,
    pub content: String,
    pub event_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerTranscriptProjection {
    pub state: WorkerOperationState,
    pub runtime_id: String,
    pub worker_id: String,
    pub start: usize,
    pub limit: usize,
    pub total_items: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start: Option<usize>,
    pub items: Vec<WorkerTranscriptItem>,
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

    fn observation_source(
        &self,
        _worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSourceConfig> {
        None
    }

    fn send_input(&self, worker_id: &str, _request: WorkerInputRequest) -> WorkerInputResult {
        WorkerInputResult {
            state: WorkerOperationState::Unsupported,
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            transcript_sequence: None,
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

    fn transcript(
        &self,
        worker_id: &str,
        start: usize,
        limit: usize,
    ) -> WorkerTranscriptProjection {
        WorkerTranscriptProjection {
            state: WorkerOperationState::Unsupported,
            runtime_id: self.runtime_id().to_string(),
            worker_id: worker_id.to_string(),
            start,
            limit,
            total_items: 0,
            next_start: None,
            items: Vec::new(),
            diagnostics: vec![diagnostic(
                "worker_transcript_pending",
                DiagnosticSeverity::Info,
                format!(
                    "bounded transcript for '{worker_id}' is not implemented by this registry source"
                ),
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

#[derive(Clone)]
pub struct RuntimeRegistry {
    runtimes: Vec<Arc<dyn WorkspaceWorkerRuntime>>,
}

impl RuntimeRegistry {
    pub fn new(runtimes: Vec<Arc<dyn WorkspaceWorkerRuntime>>) -> Self {
        Self { runtimes }
    }

    pub fn for_local_pods(runtime: LocalWorkerRuntime) -> Self {
        Self::new(vec![Arc::new(runtime)])
    }

    pub fn for_workspace(
        local_runtime: LocalWorkerRuntime,
        embedded_runtime: EmbeddedWorkerRuntime,
    ) -> Self {
        Self::new(vec![Arc::new(local_runtime), Arc::new(embedded_runtime)])
    }

    pub fn register<R>(&mut self, runtime: R)
    where
        R: WorkspaceWorkerRuntime + 'static,
    {
        self.runtimes.push(Arc::new(runtime));
    }

    pub fn list_runtimes(&self, limit: usize) -> RuntimeList<RuntimeSummary> {
        let mut diagnostics = Vec::new();
        let mut items = Vec::new();
        for runtime in self.runtimes.iter().take(limit) {
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
        for runtime in &self.runtimes {
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
        for runtime in &self.runtimes {
            if items.len() >= limit {
                break;
            }
            let mut list = runtime.list_workers(limit.saturating_sub(items.len()));
            diagnostics.append(&mut list.diagnostics);
            items.append(&mut list.items);
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
        for runtime in &self.runtimes {
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

    pub fn worker(
        &self,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<WorkerSummary, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", runtime_id)?;
        validate_backend_identifier("worker_id", worker_id)?;
        let runtime = self.runtime(runtime_id)?;
        let lookup = runtime.worker(worker_id);
        lookup.worker.ok_or_else(|| {
            operation_failed_or_unknown_worker(runtime_id, worker_id, lookup.diagnostics)
        })
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

    pub fn transcript(
        &self,
        runtime_id: &str,
        worker_id: &str,
        start: usize,
        limit: usize,
    ) -> Result<WorkerTranscriptProjection, RuntimeRegistryError> {
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
        Ok(runtime.transcript(worker_id, start, limit))
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

    pub fn observation_source(
        &self,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<crate::observation::RuntimeObservationSourceConfig, RuntimeRegistryError> {
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

    fn runtime(
        &self,
        runtime_id: &str,
    ) -> Result<&Arc<dyn WorkspaceWorkerRuntime>, RuntimeRegistryError> {
        self.runtimes
            .iter()
            .find(|runtime| runtime.runtime_id() == runtime_id)
            .ok_or_else(|| RuntimeRegistryError::UnknownRuntime(runtime_id.to_string()))
    }
}

#[derive(Clone)]
pub struct EmbeddedWorkerRuntime {
    runtime_id: String,
    host_id: String,
    runtime: worker_runtime::Runtime,
}

impl EmbeddedWorkerRuntime {
    pub fn new_memory(workspace_id: impl AsRef<str>) -> Self {
        let runtime_id = EmbeddedRuntimeId::new(EMBEDDED_RUNTIME_ID)
            .expect("embedded runtime id is a non-empty literal");
        let runtime = worker_runtime::Runtime::with_options(EmbeddedRuntimeOptions {
            runtime_id: Some(runtime_id),
            display_name: Some("Workspace backend embedded Runtime".to_string()),
            ..EmbeddedRuntimeOptions::default()
        });
        Self::from_runtime(workspace_id, runtime)
    }

    pub fn from_runtime(workspace_id: impl AsRef<str>, runtime: worker_runtime::Runtime) -> Self {
        let runtime_id = runtime
            .runtime_id()
            .ok()
            .map(|id| id.as_str().to_string())
            .unwrap_or_else(|| EMBEDDED_RUNTIME_ID.to_string());
        Self {
            runtime_id,
            host_id: host_id_for_embedded_workspace(workspace_id.as_ref()),
            runtime,
        }
    }

    fn worker_ref(&self, worker_id: &str) -> Option<EmbeddedWorkerRef> {
        Some(EmbeddedWorkerRef::new(
            EmbeddedRuntimeId::new(self.runtime_id.clone())?,
            EmbeddedWorkerId::new(worker_id.to_string())?,
        ))
    }

    fn map_worker_summary(&self, summary: worker_runtime::catalog::WorkerSummary) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: summary.worker_ref.worker_id.as_str().to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(summary.worker_ref.worker_id.as_str()),
            role: embedded_intent_label(&summary.intent),
            profile: embedded_profile_label(&summary.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_status_label(summary.status).to_string(),
            status: embedded_worker_status_label(summary.status).to_string(),
            last_seen_at: None,
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_accept_input: true,
                can_stream_events: false,
                can_stop: false,
                can_spawn_followup: false,
                can_read_bounded_transcript: true,
            },
            diagnostics: vec![diagnostic(
                "embedded_runtime_projection",
                DiagnosticSeverity::Info,
                "Worker identity is projected only as runtime_id plus worker_id; embedded runtime internals remain backend-private".to_string(),
            )],
        }
    }

    fn map_worker_detail(&self, detail: EmbeddedWorkerDetail) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: detail.worker_id.as_str().to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(detail.worker_id.as_str()),
            role: embedded_intent_label(&detail.intent),
            profile: embedded_profile_label(&detail.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "backend_internal".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_status_label(detail.status).to_string(),
            status: embedded_worker_status_label(detail.status).to_string(),
            last_seen_at: None,
            implementation: WorkerImplementationSummary {
                kind: "embedded_worker_runtime".to_string(),
                display_hint: "backend-internal worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_accept_input: true,
                can_stream_events: false,
                can_stop: false,
                can_spawn_followup: false,
                can_read_bounded_transcript: true,
            },
            diagnostics: vec![diagnostic(
                "embedded_runtime_projection",
                DiagnosticSeverity::Info,
                "Worker identity is projected only as runtime_id plus worker_id; embedded runtime internals remain backend-private".to_string(),
            )],
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
                    capabilities: embedded_runtime_capabilities(limit, false),
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
            capabilities: embedded_runtime_capabilities(limit, true),
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
                label: "Workspace backend embedded Runtime".to_string(),
                kind: EMBEDDED_HOST_KIND.to_string(),
                status: "available".to_string(),
                observed_at: Utc::now().to_rfc3339(),
                last_seen_at: None,
                capabilities: embedded_runtime_capabilities(limit, true),
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
                "embedded_runtime_tools_less",
                DiagnosticSeverity::Info,
                "Embedded Runtime v0 creates a tools-less catalog Worker and does not spawn provider segments".to_string(),
            ));
        }

        let create_request = CreateWorkerRequest::tools_less(
            embedded_create_intent(&request.intent),
            embedded_profile_selector(&request.intent),
        );
        match self.runtime.create_worker(create_request) {
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

    fn send_input(&self, worker_id: &str, request: WorkerInputRequest) -> WorkerInputResult {
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
            },
            content: request.content,
        };
        match self.runtime.send_input(&worker_ref, input) {
            Ok(ack) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                transcript_sequence: Some(ack.transcript_sequence),
                event_id: Some(ack.event_id),
                diagnostics: Vec::new(),
            },
            Err(err) => embedded_input_rejected(
                &self.runtime_id,
                worker_id,
                embedded_runtime_diagnostic(&err),
            ),
        }
    }

    fn transcript(
        &self,
        worker_id: &str,
        start: usize,
        limit: usize,
    ) -> WorkerTranscriptProjection {
        let Some(worker_ref) = self.worker_ref(worker_id) else {
            return embedded_transcript_rejected(
                &self.runtime_id,
                worker_id,
                start,
                limit,
                diagnostic(
                    "embedded_worker_id_invalid",
                    DiagnosticSeverity::Warning,
                    "Worker id was empty and cannot be resolved".to_string(),
                ),
            );
        };
        match self
            .runtime
            .transcript_projection(&worker_ref, TranscriptQuery::new(start, limit))
        {
            Ok(projection) => {
                embedded_transcript_projection(&self.runtime_id, worker_id, projection)
            }
            Err(err) => embedded_transcript_rejected(
                &self.runtime_id,
                worker_id,
                start,
                limit,
                embedded_runtime_diagnostic(&err),
            ),
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
            cached_capabilities: remote_runtime_capabilities(200, false),
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
    bearer_token: Option<String>,
    cached_capabilities: RuntimeCapabilitySummary,
    cached_status: String,
    host_id: String,
    http: BlockingHttpClient,
}

impl RemoteWorkerRuntime {
    pub fn new(config: RemoteRuntimeConfig) -> Result<Self, RuntimeRegistryError> {
        validate_backend_identifier("runtime_id", &config.runtime_id)?;
        let base_url = config.base_url.trim_end_matches('/').to_string();
        let http = BlockingHttpClient::builder()
            .timeout(config.timeout)
            .build()
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
            bearer_token: config.bearer_token,
            cached_capabilities: config.cached_capabilities,
            cached_status: config.cached_status,
            http,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
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

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        let request = request.header(CONTENT_TYPE, "application/json");
        if let Some(token) = self.bearer_token.as_deref() {
            request.header(AUTHORIZATION, format!("Bearer {token}"))
        } else {
            request
        }
    }

    fn get_json<T>(&self, path: &str) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned,
    {
        self.send_json(self.http.get(self.endpoint(path)))
    }

    fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T, RuntimeDiagnostic>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.send_json(self.http.post(self.endpoint(path)).json(body))
    }

    fn send_json<T>(&self, request: RequestBuilder) -> Result<T, RuntimeDiagnostic>
    where
        T: DeserializeOwned,
    {
        let response = self
            .authorize(request)
            .send()
            .map_err(|err| remote_reqwest_diagnostic(&self.runtime_id, err))?;
        let status = response.status();
        if status.is_success() {
            response.json::<T>().map_err(|err| {
                diagnostic(
                    "remote_runtime_malformed_response",
                    DiagnosticSeverity::Error,
                    format!(
                        "Remote Runtime returned malformed JSON for '{}': {err}",
                        self.runtime_id
                    ),
                )
            })
        } else {
            Err(remote_http_status_diagnostic(
                &self.runtime_id,
                status,
                response,
            ))
        }
    }

    fn map_worker_summary(&self, summary: worker_runtime::catalog::WorkerSummary) -> WorkerSummary {
        WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id: summary.worker_ref.worker_id.as_str().to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(summary.worker_ref.worker_id.as_str()),
            role: embedded_intent_label(&summary.intent),
            profile: embedded_profile_label(&summary.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_status_label(summary.status).to_string(),
            status: embedded_worker_status_label(summary.status).to_string(),
            last_seen_at: None,
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_accept_input: true,
                can_stream_events: true,
                can_stop: true,
                can_spawn_followup: false,
                can_read_bounded_transcript: true,
            },
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
            worker_id: detail.worker_id.as_str().to_string(),
            host_id: self.host_id.clone(),
            label: safe_display_hint(detail.worker_id.as_str()),
            role: embedded_intent_label(&detail.intent),
            profile: embedded_profile_label(&detail.profile),
            workspace: WorkerWorkspaceSummary {
                visibility: "remote_runtime".to_string(),
                identity: "runtime_registry_worker".to_string(),
            },
            state: embedded_worker_status_label(detail.status).to_string(),
            status: embedded_worker_status_label(detail.status).to_string(),
            last_seen_at: None,
            implementation: WorkerImplementationSummary {
                kind: "remote_worker_runtime".to_string(),
                display_hint: "Backend-proxied remote worker-runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_accept_input: true,
                can_stream_events: true,
                can_stop: true,
                can_spawn_followup: false,
                can_read_bounded_transcript: true,
            },
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
                capabilities: remote_runtime_capabilities(limit, true),
                diagnostics: vec![diagnostic(
                    "remote_runtime_backend_proxy",
                    DiagnosticSeverity::Info,
                    "Remote Runtime is accessed only by backend-owned REST/WS clients".to_string(),
                )],
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
                capabilities: remote_runtime_capabilities(limit, true),
                diagnostics: vec![diagnostic(
                    "remote_runtime_backend_proxy",
                    DiagnosticSeverity::Info,
                    "Remote host endpoint and credentials are backend-private".to_string(),
                )],
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
            Err(diagnostic) if diagnostic.code == "remote_worker_not_found" => WorkerLookupResult {
                worker: None,
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => WorkerLookupResult {
                worker: None,
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
        let create = CreateWorkerRequest::tools_less(
            embedded_create_intent(&request.intent),
            embedded_profile_selector(&request.intent),
        );
        match self.post_json::<_, RuntimeHttpWorkerResponse>("/v1/workers", &create) {
            Ok(response) => WorkerSpawnResult {
                state: WorkerOperationState::Accepted,
                worker: Some(self.map_worker_detail(response.worker)),
                acceptance_evidence: vec![WorkerSpawnAcceptanceEvidence {
                    kind: "remote_runtime_worker_created".to_string(),
                    detail: "worker-runtime REST create endpoint accepted the Worker".to_string(),
                }],
                diagnostics: vec![diagnostic(
                    "remote_runtime_backend_proxy",
                    DiagnosticSeverity::Info,
                    "Remote create used a backend-owned REST client; browser-facing payload exposes only runtime_id plus worker_id".to_string(),
                )],
            },
            Err(diagnostic) => WorkerSpawnResult {
                state: WorkerOperationState::Rejected,
                worker: None,
                acceptance_evidence: Vec::new(),
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

    fn observation_source(
        &self,
        worker_id: &str,
    ) -> Option<crate::observation::RuntimeObservationSourceConfig> {
        Some(crate::observation::RuntimeObservationSourceConfig {
            runtime_id: self.runtime_id.clone(),
            worker_id: worker_id.to_string(),
            endpoint: self.ws_endpoint(worker_id),
            bearer_token: self.bearer_token.clone(),
        })
    }

    fn send_input(&self, worker_id: &str, request: WorkerInputRequest) -> WorkerInputResult {
        let input = EmbeddedWorkerInput {
            kind: match request.kind {
                WorkerInputKind::User => EmbeddedWorkerInputKind::User,
                WorkerInputKind::System => EmbeddedWorkerInputKind::System,
            },
            content: request.content,
        };
        match self.post_json::<_, RuntimeHttpWorkerInputResponse>(
            &format!("/v1/workers/{worker_id}/input"),
            &input,
        ) {
            Ok(response) => WorkerInputResult {
                state: WorkerOperationState::Accepted,
                runtime_id: self.runtime_id.clone(),
                worker_id: worker_id.to_string(),
                transcript_sequence: Some(response.ack.transcript_sequence),
                event_id: Some(response.ack.event_id),
                diagnostics: Vec::new(),
            },
            Err(diagnostic) => remote_input_rejected(&self.runtime_id, worker_id, diagnostic),
        }
    }

    fn transcript(
        &self,
        worker_id: &str,
        start: usize,
        limit: usize,
    ) -> WorkerTranscriptProjection {
        match self.get_json::<RuntimeHttpTranscriptResponse>(&format!(
            "/v1/workers/{worker_id}/transcript?start={start}&limit={limit}"
        )) {
            Ok(response) => {
                embedded_transcript_projection(&self.runtime_id, worker_id, response.transcript)
            }
            Err(diagnostic) => {
                embedded_transcript_rejected(&self.runtime_id, worker_id, start, limit, diagnostic)
            }
        }
    }
}

#[derive(Clone)]
pub struct LocalWorkerRuntime {
    runtime_id: String,
    host_id: String,
    workspace_root: PathBuf,
    data_dir: Option<PathBuf>,
}

pub type LocalRuntimeBridge = LocalWorkerRuntime;

impl LocalWorkerRuntime {
    pub fn new(
        workspace_id: impl AsRef<str>,
        workspace_root: impl Into<PathBuf>,
        data_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            runtime_id: LOCAL_RUNTIME_ID.to_string(),
            host_id: host_id_for_workspace(workspace_id.as_ref()),
            workspace_root: workspace_root.into(),
            data_dir,
        }
    }

    fn pod_root(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|dir| dir.join("pods"))
    }

    fn worker_names(&self, pod_root: &Path) -> Result<BTreeSet<String>, RuntimeDiagnostic> {
        let entries = fs::read_dir(pod_root).map_err(|err| {
            diagnostic(
                "local_pod_registry_unreadable",
                DiagnosticSeverity::Warning,
                format!("local Worker registry could not be read: {err}"),
            )
        })?;

        let mut worker_names = BTreeSet::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(|name| name.to_string()) else {
                continue;
            };
            worker_names.insert(name);
        }
        Ok(worker_names)
    }

    fn read_worker(&self, pod_root: &Path, worker_name: &str) -> WorkerReadOutcome {
        let metadata_path = pod_root.join(worker_name).join("metadata.json");
        let data = match fs::read_to_string(metadata_path) {
            Ok(data) => data,
            Err(err) => {
                return WorkerReadOutcome::Diagnostic(diagnostic(
                    "local_pod_metadata_unreadable",
                    DiagnosticSeverity::Warning,
                    format!("local Worker metadata could not be read: {err}"),
                ));
            }
        };
        let metadata: WorkerMetadata = match serde_json::from_str(&data) {
            Ok(metadata) => metadata,
            Err(err) => {
                return WorkerReadOutcome::Diagnostic(diagnostic(
                    "local_pod_metadata_invalid",
                    DiagnosticSeverity::Warning,
                    format!("local Worker metadata could not be parsed: {err}"),
                ));
            }
        };

        let Some(metadata_workspace_root) = metadata.workspace_root.as_ref() else {
            return WorkerReadOutcome::Diagnostic(diagnostic(
                "local_pod_workspace_root_missing",
                DiagnosticSeverity::Warning,
                "local Worker metadata did not include a workspace identity and was not included"
                    .to_string(),
            ));
        };
        if !same_workspace_root(metadata_workspace_root, &self.workspace_root) {
            return WorkerReadOutcome::Diagnostic(diagnostic(
                "local_pod_workspace_not_visible",
                DiagnosticSeverity::Info,
                "local Worker metadata belongs to another workspace and was not included"
                    .to_string(),
            ));
        }

        let label = safe_display_hint(&metadata.worker_name);
        let role = manifest_hint_string(&metadata.resolved_manifest_snapshot, "role");
        let profile =
            manifest_hint_string(&metadata.resolved_manifest_snapshot, "profile_selector");
        let worker_id = worker_id_for_pod(worker_name);
        let status = match metadata
            .active
            .as_ref()
            .and_then(|active| active.segment_id.as_ref())
        {
            Some(_) => "active:segment".to_string(),
            None if metadata.active.is_some() => "active:pending_segment".to_string(),
            None => "metadata_only".to_string(),
        };
        let state = if metadata.active.is_some() {
            "active".to_string()
        } else {
            "metadata_only".to_string()
        };
        let last_seen_at = None;
        WorkerReadOutcome::Worker(WorkerSummary {
            runtime_id: self.runtime_id.clone(),
            worker_id,
            host_id: self.host_id.clone(),
            label,
            role,
            profile,
            workspace: WorkerWorkspaceSummary {
                visibility: "current_workspace".to_string(),
                identity: "runtime_workspace".to_string(),
            },
            state,
            status,
            last_seen_at,
            implementation: WorkerImplementationSummary {
                kind: "local_pod".to_string(),
                display_hint: safe_display_hint(worker_name),
            },
            capabilities: WorkerCapabilitySummary {
                can_accept_input: true,
                can_stream_events: false,
                can_stop: false,
                can_spawn_followup: false,
                can_read_bounded_transcript: false,
            },
            diagnostics: vec![diagnostic(
                "worker_actions_not_implemented",
                DiagnosticSeverity::Info,
                "runtime overview is available; worker control and stream/proxy operations are not yet implemented"
                    .to_string(),
            )],
        })
    }
}

impl WorkspaceWorkerRuntime for LocalWorkerRuntime {
    fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    fn runtime_summary(&self, limit: usize) -> RuntimeSummary {
        let host_list = self.list_hosts(1);
        RuntimeSummary {
            runtime_id: self.runtime_id.clone(),
            label: "Local Worker runtime".to_string(),
            kind: "local_pod".to_string(),
            status: "available".to_string(),
            source: RuntimeSourceSummary::local_compatibility(),
            host_ids: host_list
                .items
                .iter()
                .take(limit)
                .map(|host| host.host_id.clone())
                .collect(),
            capabilities: local_runtime_capabilities(limit, self.pod_root().is_some()),
            diagnostics: host_list.diagnostics,
        }
    }

    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary> {
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }

        let mut diagnostics = Vec::new();
        let inspection_available = self.pod_root().is_some();
        if !inspection_available {
            diagnostics.push(diagnostic(
                "local_pod_registry_unavailable",
                DiagnosticSeverity::Warning,
                "local Worker data directory is not configured; worker discovery is unavailable"
                    .to_string(),
            ));
        }

        let item = HostSummary {
            runtime_id: self.runtime_id.clone(),
            host_id: self.host_id.clone(),
            label: label_for_workspace(&self.workspace_root),
            kind: LOCAL_HOST_KIND.to_string(),
            status: if inspection_available {
                "available"
            } else {
                "degraded"
            }
            .to_string(),
            observed_at: Utc::now().to_rfc3339(),
            last_seen_at: None,
            capabilities: local_runtime_capabilities(limit, inspection_available),
            diagnostics: diagnostics.clone(),
        };
        RuntimeList::new(vec![item], diagnostics)
    }

    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        let Some(pod_root) = self.pod_root() else {
            return RuntimeList::new(
                Vec::new(),
                vec![diagnostic(
                    "local_pod_registry_unavailable",
                    DiagnosticSeverity::Warning,
                    "local Worker data directory is not configured; worker discovery is unavailable"
                        .to_string(),
                )],
            );
        };
        if limit == 0 {
            return RuntimeList::new(Vec::new(), Vec::new());
        }

        let mut workers = Vec::new();
        let mut diagnostics = Vec::new();
        let worker_names = match self.worker_names(&pod_root) {
            Ok(worker_names) => worker_names,
            Err(diag) => return RuntimeList::new(Vec::new(), vec![diag]),
        };

        for worker_name in worker_names {
            if workers.len() >= limit {
                break;
            }
            match self.read_worker(&pod_root, &worker_name) {
                WorkerReadOutcome::Worker(worker) => workers.push(worker),
                WorkerReadOutcome::Diagnostic(diag) => {
                    if diagnostics.len() < MAX_DIAGNOSTICS {
                        diagnostics.push(diag);
                    }
                }
            }
        }
        RuntimeList::new(workers, diagnostics)
    }

    fn worker(&self, worker_id: &str) -> WorkerLookupResult {
        let Some(pod_root) = self.pod_root() else {
            return WorkerLookupResult {
                worker: None,
                diagnostics: vec![diagnostic(
                    "local_pod_registry_unavailable",
                    DiagnosticSeverity::Warning,
                    "local Worker data directory is not configured; worker discovery is unavailable"
                        .to_string(),
                )],
            };
        };
        let worker_names = match self.worker_names(&pod_root) {
            Ok(worker_names) => worker_names,
            Err(diag) => {
                return WorkerLookupResult {
                    worker: None,
                    diagnostics: vec![diag],
                };
            }
        };
        for worker_name in worker_names {
            if worker_id_for_pod(&worker_name) != worker_id {
                continue;
            }
            return match self.read_worker(&pod_root, &worker_name) {
                WorkerReadOutcome::Worker(worker) => WorkerLookupResult {
                    worker: Some(worker),
                    diagnostics: Vec::new(),
                },
                WorkerReadOutcome::Diagnostic(diag) => WorkerLookupResult {
                    worker: None,
                    diagnostics: vec![diag],
                },
            };
        }
        WorkerLookupResult {
            worker: None,
            diagnostics: Vec::new(),
        }
    }
}

enum WorkerReadOutcome {
    Worker(WorkerSummary),
    Diagnostic(RuntimeDiagnostic),
}

fn embedded_runtime_capabilities(limit: usize, available: bool) -> RuntimeCapabilitySummary {
    RuntimeCapabilitySummary {
        can_list_hosts: true,
        can_list_workers: available,
        can_get_worker: available,
        can_spawn_worker: available,
        can_stop_worker: false,
        can_accept_input: available,
        can_stream_events: false,
        can_read_bounded_transcript: available,
        has_workspace_fs: false,
        has_shell: false,
        has_git: false,
        supports_worktrees: false,
        supports_backend_internal_tools: true,
        local_pod_inspection: "not_applicable".to_string(),
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

fn embedded_worker_status_label(status: EmbeddedWorkerStatus) -> &'static str {
    match status {
        EmbeddedWorkerStatus::Running => "running",
        EmbeddedWorkerStatus::Stopped => "stopped",
        EmbeddedWorkerStatus::Cancelled => "cancelled",
    }
}

fn embedded_create_intent(intent: &WorkerSpawnIntent) -> WorkerIntent {
    match intent {
        WorkerSpawnIntent::WorkspaceCompanion => WorkerIntent::Role {
            role: "workspace_companion".to_string(),
            purpose: Some("workspace backend internal companion".to_string()),
        },
        WorkerSpawnIntent::WorkspaceOrchestrator => WorkerIntent::Role {
            role: "workspace_orchestrator".to_string(),
            purpose: Some("workspace backend internal orchestration".to_string()),
        },
        WorkerSpawnIntent::TicketRole { ticket_id, role } => WorkerIntent::Role {
            role: ticket_role_profile_slug(role).to_string(),
            purpose: Some(format!("ticket {ticket_id}")),
        },
    }
}

fn embedded_profile_selector(intent: &WorkerSpawnIntent) -> ProfileSelector {
    match intent {
        WorkerSpawnIntent::TicketRole { role, .. } => {
            ProfileSelector::Builtin(format!("builtin:{}", ticket_role_profile_slug(role)))
        }
        WorkerSpawnIntent::WorkspaceCompanion | WorkerSpawnIntent::WorkspaceOrchestrator => {
            ProfileSelector::RuntimeDefault
        }
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

fn embedded_intent_label(intent: &WorkerIntent) -> Option<String> {
    match intent {
        WorkerIntent::Assistant { purpose } => {
            purpose.clone().or_else(|| Some("assistant".to_string()))
        }
        WorkerIntent::Task { objective } => Some(safe_display_hint(objective)),
        WorkerIntent::Role { role, .. } => Some(safe_display_hint(role)),
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
        transcript_sequence: None,
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
        transcript_sequence: None,
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

fn embedded_transcript_projection(
    runtime_id: &str,
    worker_id: &str,
    projection: EmbeddedTranscriptProjection,
) -> WorkerTranscriptProjection {
    WorkerTranscriptProjection {
        state: WorkerOperationState::Accepted,
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        start: projection.start,
        limit: projection.limit,
        total_items: projection.total_items,
        next_start: projection.next_start,
        items: projection
            .items
            .into_iter()
            .map(|item| WorkerTranscriptItem {
                sequence: item.sequence,
                role: embedded_transcript_role_label(item.role).to_string(),
                content: item.content,
                event_id: item.event_id,
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

fn embedded_transcript_rejected(
    runtime_id: &str,
    worker_id: &str,
    start: usize,
    limit: usize,
    diagnostic: RuntimeDiagnostic,
) -> WorkerTranscriptProjection {
    WorkerTranscriptProjection {
        state: WorkerOperationState::Rejected,
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.to_string(),
        start,
        limit,
        total_items: 0,
        next_start: None,
        items: Vec::new(),
        diagnostics: vec![diagnostic],
    }
}

fn embedded_transcript_role_label(role: TranscriptRole) -> &'static str {
    match role {
        TranscriptRole::User => "user",
        TranscriptRole::System => "system",
    }
}

fn embedded_runtime_diagnostic(error: &EmbeddedRuntimeError) -> RuntimeDiagnostic {
    match error {
        EmbeddedRuntimeError::RuntimeStopped { .. } => diagnostic(
            "embedded_runtime_stopped",
            DiagnosticSeverity::Warning,
            "Embedded Runtime is stopped".to_string(),
        ),
        EmbeddedRuntimeError::WrongRuntime { .. }
        | EmbeddedRuntimeError::WrongRuntimeCursor { .. } => diagnostic(
            "embedded_runtime_wrong_identity",
            DiagnosticSeverity::Warning,
            "Embedded Runtime rejected a worker/runtime identity mismatch".to_string(),
        ),
        EmbeddedRuntimeError::WorkerNotFound { .. } => diagnostic(
            "embedded_worker_not_found",
            DiagnosticSeverity::Warning,
            "Embedded Runtime worker was not found".to_string(),
        ),
        EmbeddedRuntimeError::LimitTooLarge { requested, max } => diagnostic(
            "embedded_runtime_limit_too_large",
            DiagnosticSeverity::Warning,
            format!("Requested limit {requested} exceeds embedded Runtime maximum {max}"),
        ),
        EmbeddedRuntimeError::InvalidRequest(_) => diagnostic(
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

fn remote_runtime_capabilities(limit: usize, available: bool) -> RuntimeCapabilitySummary {
    RuntimeCapabilitySummary {
        can_list_hosts: true,
        can_list_workers: available,
        can_get_worker: available,
        can_spawn_worker: available,
        can_stop_worker: available,
        can_accept_input: available,
        can_stream_events: available,
        can_read_bounded_transcript: available,
        has_workspace_fs: false,
        has_shell: false,
        has_git: false,
        supports_worktrees: false,
        supports_backend_internal_tools: false,
        local_pod_inspection: "unavailable".to_string(),
        workspace_scope: "remote_runtime_backend_private".to_string(),
        max_workers: limit,
        os: "remote".to_string(),
        arch: "remote".to_string(),
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
        .map(|error| error.error.message.clone())
        .unwrap_or_else(|| format!("remote Runtime returned HTTP {status}"));
    let (code, severity) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ("remote_runtime_auth_failed", DiagnosticSeverity::Error)
        }
        StatusCode::NOT_FOUND => ("remote_worker_not_found", DiagnosticSeverity::Warning),
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => {
            ("remote_runtime_unsupported", DiagnosticSeverity::Warning)
        }
        _ if status.is_server_error() => ("remote_runtime_http_error", DiagnosticSeverity::Error),
        _ => ("remote_runtime_http_rejected", DiagnosticSeverity::Warning),
    };
    diagnostic(
        code,
        severity,
        format!("Remote Runtime '{runtime_id}' rejected request ({remote_code}): {remote_message}"),
    )
}

fn local_runtime_capabilities(
    limit: usize,
    inspection_available: bool,
) -> RuntimeCapabilitySummary {
    RuntimeCapabilitySummary {
        can_list_hosts: true,
        can_list_workers: inspection_available,
        can_get_worker: inspection_available,
        can_spawn_worker: false,
        can_stop_worker: false,
        can_accept_input: false,
        can_stream_events: false,
        can_read_bounded_transcript: false,
        has_workspace_fs: false,
        has_shell: false,
        has_git: false,
        supports_worktrees: false,
        supports_backend_internal_tools: false,
        local_pod_inspection: if inspection_available {
            "available"
        } else {
            "unavailable"
        }
        .to_string(),
        workspace_scope: "current_workspace".to_string(),
        max_workers: limit,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
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

fn host_id_for_workspace(workspace_id: &str) -> String {
    bounded_backend_identifier("local-", workspace_id)
}

fn worker_id_for_pod(worker_name: &str) -> String {
    bounded_backend_identifier("local-worker-", worker_name)
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

fn label_for_workspace(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_string()
}

fn same_workspace_root(left: &Path, right: &Path) -> bool {
    lexical_path_string(left) == lexical_path_string(right)
}

fn lexical_path_string(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    while value.len() > 1 && value.ends_with('/') {
        value.pop();
    }
    value
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
        WorkerSpawnIntent::TicketRole { role, .. } => match role {
            TicketWorkerRole::Intake => "ticket_intake",
            TicketWorkerRole::Orchestrator => "ticket_orchestrator",
            TicketWorkerRole::Coder => "ticket_coder",
            TicketWorkerRole::Reviewer => "ticket_reviewer",
        },
    }
}

fn manifest_hint_string(snapshot: &Option<Value>, key: &str) -> Option<String> {
    let value = snapshot.as_ref()?;
    let candidate = match key {
        "role" => value
            .get("role")
            .or_else(|| value.get("role_claim").and_then(|role| role.get("role"))),
        "profile_selector" => value
            .get("profile_selector")
            .or_else(|| value.get("profile")),
        _ => value.get(key),
    }?;
    let text = candidate.as_str().map(ToOwned::to_owned).or_else(|| {
        candidate
            .get("name")
            .and_then(|name| name.as_str())
            .map(ToOwned::to_owned)
    })?;
    let safe = safe_display_hint(&text);
    (!safe.is_empty()).then_some(safe)
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
        status: "Worker runtime control is not wired yet".to_string(),
        last_seen_at: None,
        implementation: WorkerImplementationSummary {
            kind: "placeholder".to_string(),
            display_hint: "unsupported".to_string(),
        },
        capabilities: WorkerCapabilitySummary {
            can_accept_input: false,
            can_stream_events: false,
            can_stop: false,
            can_spawn_followup: false,
            can_read_bounded_transcript: false,
        },
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
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use tempfile::TempDir;

    fn write_metadata(dir: &Path, worker_name: &str, metadata: &WorkerMetadata) {
        let pod_dir = dir.join("pods").join(worker_name);
        fs::create_dir_all(&pod_dir).unwrap();
        fs::write(
            pod_dir.join("metadata.json"),
            serde_json::to_vec(metadata).unwrap(),
        )
        .unwrap();
    }

    fn metadata(workspace_root: Option<&str>) -> WorkerMetadata {
        let mut metadata = WorkerMetadata::new("coder", None);
        metadata.workspace_root = workspace_root.map(PathBuf::from);
        metadata.resolved_manifest_snapshot = Some(json!({
            "role": "coder",
            "profile_selector": "builtin:coder"
        }));
        metadata
    }

    fn assert_valid_generated_id(id: &str) {
        assert!(id.len() <= MAX_IDENTIFIER_LEN, "id too long: {id}");
        validate_backend_identifier("test_id", id).unwrap();
    }

    fn host_id() -> String {
        host_id_for_workspace("local:test")
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
                    state: "running".to_string(),
                    status: "available".to_string(),
                    last_seen_at: None,
                    implementation: WorkerImplementationSummary {
                        kind: "fixture".to_string(),
                        display_hint: "test fixture".to_string(),
                    },
                    capabilities: WorkerCapabilitySummary {
                        can_accept_input: false,
                        can_stream_events: false,
                        can_stop: false,
                        can_spawn_followup: false,
                        can_read_bounded_transcript: false,
                    },
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
                    can_accept_input: false,
                    can_stream_events: false,
                    can_read_bounded_transcript: false,
                    has_workspace_fs: false,
                    has_shell: false,
                    has_git: false,
                    supports_worktrees: false,
                    supports_backend_internal_tools: false,
                    local_pod_inspection: "none".to_string(),
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

        let unknown_worker = registry.worker("runtime-a", "worker-missing").unwrap_err();
        assert_eq!(
            unknown_worker,
            RuntimeRegistryError::UnknownWorker {
                runtime_id: "runtime-a".to_string(),
                worker_id: "worker-missing".to_string(),
            }
        );
        assert!(matches!(
            unknown_worker.into_error(),
            Error::UnknownWorker { runtime_id, worker_id }
                if runtime_id == "runtime-a" && worker_id == "worker-missing"
        ));
    }

    #[test]
    fn local_runtime_reports_host_without_private_paths() {
        let bridge = LocalWorkerRuntime::new("local:test", "/workspace/project", None);
        let hosts = bridge.list_hosts(10);
        assert_eq!(hosts.items.len(), 1);
        let host = &hosts.items[0];
        assert_eq!(host.runtime_id, LOCAL_RUNTIME_ID);
        assert_eq!(host.host_id, host_id());
        assert_valid_generated_id(&host.host_id);
        assert_eq!(host.capabilities.local_pod_inspection, "unavailable");
        assert_eq!(host.capabilities.workspace_scope, "current_workspace");
        let json = serde_json::to_string(host).unwrap();
        assert!(!json.contains("/workspace/project"));
        assert!(!json.contains("metadata.json"));
    }

    #[test]
    fn registry_lists_runtimes_hosts_and_workers() {
        let temp = TempDir::new().unwrap();
        write_metadata(temp.path(), "coder", &metadata(Some("/workspace/project")));
        let registry = RuntimeRegistry::for_local_pods(LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        ));

        let runtimes = registry.list_runtimes(10);
        assert_eq!(runtimes.items[0].runtime_id, LOCAL_RUNTIME_ID);
        assert_eq!(
            runtimes.items[0].source.kind,
            RuntimeSourceKind::LocalCompatibility
        );
        assert_eq!(
            runtimes.items[0].source.identity_authority,
            RuntimeIdentityAuthority::RuntimeRegistryProjection
        );
        assert_eq!(runtimes.items[0].host_ids, vec![host_id()]);

        let hosts = registry.list_hosts(10);
        assert_eq!(hosts.items[0].host_id, host_id());
        assert_valid_generated_id(&hosts.items[0].host_id);

        let workers = registry.list_workers(10);
        assert_eq!(workers.items.len(), 1);
        let worker = &workers.items[0];
        assert_eq!(worker.runtime_id, LOCAL_RUNTIME_ID);
        assert_eq!(worker.worker_id, worker_id_for_pod("coder"));
        assert_valid_generated_id(&worker.worker_id);
        assert_eq!(worker.host_id, host_id());
        assert_eq!(worker.workspace.visibility, "current_workspace");
        assert_eq!(worker.implementation.display_hint, "coder");
        let json = serde_json::to_string(worker).unwrap();
        assert!(!json.contains("/workspace/project"));
        assert!(!json.contains("metadata.json"));
    }

    #[test]
    fn registry_resolves_backend_validated_host_ids() {
        let temp = TempDir::new().unwrap();
        write_metadata(temp.path(), "coder", &metadata(Some("/workspace/project")));
        let registry = RuntimeRegistry::for_local_pods(LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        ));

        let workers = registry.list_workers_for_host(&host_id(), 10).unwrap();
        assert_eq!(workers.items.len(), 1);
        assert!(matches!(
            registry.list_workers_for_host("../secret", 10),
            Err(RuntimeRegistryError::InvalidIdentifier { .. })
        ));
        assert!(matches!(
            registry.list_workers_for_host("local-missing", 10),
            Err(RuntimeRegistryError::UnknownHost(_))
        ));
    }

    #[test]
    fn local_runtime_excludes_other_workspace_metadata() {
        let temp = TempDir::new().unwrap();
        write_metadata(temp.path(), "other", &metadata(Some("/workspace/other")));
        write_metadata(temp.path(), "missing", &metadata(None));
        let bridge = LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        );
        let workers = bridge.list_workers(10);
        assert!(workers.items.is_empty());
        assert!(
            workers
                .diagnostics
                .iter()
                .any(|diag| diag.code == "local_pod_workspace_not_visible")
        );
        assert!(
            workers
                .diagnostics
                .iter()
                .any(|diag| diag.code == "local_pod_workspace_root_missing")
        );
    }

    #[test]
    fn local_runtime_worker_detail_is_safe_and_bounded() {
        let temp = TempDir::new().unwrap();
        write_metadata(temp.path(), "coder", &metadata(Some("/workspace/project")));
        let bridge = LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        );
        let worker_id = worker_id_for_pod("coder");
        let worker = bridge.worker(&worker_id).worker.unwrap();
        assert_eq!(worker.label, "coder");
        assert_eq!(worker.workspace.identity, "runtime_workspace");
        assert!(
            bridge
                .worker(&worker_id_for_pod("missing"))
                .worker
                .is_none()
        );
    }

    #[test]
    fn embedded_runtime_registers_routes_input_and_transcript_without_internal_leaks() {
        let temp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::for_local_pods(LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        ));
        registry.register(EmbeddedWorkerRuntime::new_memory("local:test"));

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
        assert!(embedded_summary.capabilities.can_accept_input);
        assert!(embedded_summary.capabilities.can_read_bounded_transcript);

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
        assert!(worker.capabilities.can_accept_input);
        assert!(worker.capabilities.can_read_bounded_transcript);

        let input = registry
            .send_input(
                EMBEDDED_RUNTIME_ID,
                &worker.worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello embedded runtime".to_string(),
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);
        assert_eq!(input.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(input.worker_id, worker.worker_id);
        assert_eq!(input.transcript_sequence, Some(1));

        let transcript = registry
            .transcript(EMBEDDED_RUNTIME_ID, &worker.worker_id, 0, 10)
            .unwrap();
        assert_eq!(transcript.state, WorkerOperationState::Accepted);
        assert_eq!(transcript.items.len(), 1);
        assert_eq!(transcript.items[0].role, "user");
        assert_eq!(transcript.items[0].content, "hello embedded runtime");

        assert!(matches!(
            registry.send_input(
                LOCAL_RUNTIME_ID,
                &worker.worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "wrong runtime".to_string(),
                },
            ),
            Err(RuntimeRegistryError::UnknownWorker { runtime_id, worker_id })
                if runtime_id == LOCAL_RUNTIME_ID && worker_id == worker.worker_id
        ));

        let json = serde_json::to_string(&(embedded_summary, worker, transcript)).unwrap();
        for forbidden in [
            "/workspace/project",
            "metadata.json",
            "session",
            "socket",
            "token",
            "credential",
            "provider",
        ] {
            assert!(
                !json.contains(forbidden),
                "embedded runtime projection leaked forbidden term: {forbidden}: {json}"
            );
        }
    }

    #[test]
    fn embedded_runtime_rejects_socket_ready_acceptance_without_socket_identity() {
        let registry = RuntimeRegistry::new(vec![Arc::new(EmbeddedWorkerRuntime::new_memory(
            "local:test",
        ))]);
        let result = registry
            .spawn_worker(
                EMBEDDED_RUNTIME_ID,
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::WorkspaceCompanion,
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::SocketReady,
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

    #[test]
    fn remote_runtime_registry_routes_commands_without_browser_secret_leaks() {
        let worker_json = worker_json("remote:primary", "worker-remote-1");
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
                "/v1/workers/worker-remote-1",
                true,
                200,
                json!({ "worker": worker_json.clone() }).to_string(),
            ),
            mock_response(
                "POST",
                "/v1/workers/worker-remote-1/input",
                true,
                200,
                json!({
                    "ack": {
                        "worker_ref": { "runtime_id": "remote:primary", "worker_id": "worker-remote-1" },
                        "status": "running",
                        "transcript_sequence": 7,
                        "event_id": 8
                    }
                })
                .to_string(),
            ),
        ]);
        let secret = "secret-token-do-not-leak".to_string();
        let mut registry = RuntimeRegistry::new(Vec::new());
        registry.register(
            RemoteWorkerRuntime::new(RemoteRuntimeConfig::new(
                "remote:primary",
                "Remote Primary",
                base_url.clone(),
                Some(secret.clone()),
            ))
            .unwrap(),
        );

        let observation = registry
            .observation_source("remote:primary", "worker-remote-1")
            .expect("remote runtime exposes backend-owned WS observation source");
        assert!(observation.endpoint.starts_with("ws://127.0.0.1:"));
        assert!(
            observation
                .endpoint
                .ends_with("/v1/workers/worker-remote-1/events/ws")
        );
        assert_eq!(observation.bearer_token.as_deref(), Some(secret.as_str()));

        let workers = registry.list_workers(10);
        assert_eq!(workers.items.len(), 1);
        assert_eq!(workers.items[0].runtime_id, "remote:primary");
        assert_eq!(workers.items[0].worker_id, "worker-remote-1");
        assert_eq!(
            workers.items[0].implementation.kind,
            "remote_worker_runtime"
        );
        assert_eq!(
            workers.items[0].workspace.identity,
            "runtime_registry_worker"
        );

        let input = registry
            .send_input(
                "remote:primary",
                "worker-remote-1",
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "hello remote".to_string(),
                },
            )
            .unwrap();
        assert_eq!(input.state, WorkerOperationState::Accepted);
        assert_eq!(input.transcript_sequence, Some(7));
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
    fn remote_runtime_auth_errors_map_to_typed_backend_error() {
        let (base_url, server) = serve_mock_http(vec![mock_response(
            "GET",
            "/v1/workers/worker-missing",
            true,
            401,
            json!({ "error": { "code": "unauthorized", "message": "bad token" } }).to_string(),
        )]);
        let mut registry = RuntimeRegistry::new(Vec::new());
        registry.register(
            RemoteWorkerRuntime::new(RemoteRuntimeConfig::new(
                "remote:primary",
                "Remote Primary",
                base_url,
                Some("secret-token".to_string()),
            ))
            .unwrap(),
        );

        let error = registry
            .worker("remote:primary", "worker-missing")
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
        json!({
            "worker_ref": { "runtime_id": runtime_id, "worker_id": worker_id },
            "runtime_id": runtime_id,
            "worker_id": worker_id,
            "status": "running",
            "intent": { "kind": "role", "role": "coder", "purpose": "remote test" },
            "profile": { "kind": "builtin", "value": "coder" },
            "config_bundle": null,
            "requested_capabilities": [],
            "workspace_refs": [],
            "mount_refs": [],
            "requested_capability_count": 0,
            "has_config_bundle": false,
            "transcript_len": 0,
            "last_event_id": 0
        })
    }

    #[test]
    fn generated_worker_ids_are_opaque_bounded_unique_and_resolvable() {
        let temp = TempDir::new().unwrap();
        let long_a = format!("{}-A", "TicketWorkerWithVeryLongName".repeat(8));
        let long_b = format!("{}-B", "TicketWorkerWithVeryLongName".repeat(8));
        let worker_names = vec![
            "00001KVWECEQG-Coder.Worker".to_string(),
            "foo.bar".to_string(),
            "foo-bar".to_string(),
            "Ticket#Worker@Reviewer".to_string(),
            long_a,
            long_b,
        ];
        for worker_name in &worker_names {
            write_metadata(
                temp.path(),
                worker_name,
                &metadata(Some("/workspace/project")),
            );
        }
        let bridge = LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        );
        let registry = RuntimeRegistry::for_local_pods(bridge.clone());

        let listed = registry.list_workers(100);
        assert_eq!(listed.items.len(), worker_names.len());
        let mut ids = BTreeSet::new();
        for worker in listed.items {
            assert_valid_generated_id(&worker.worker_id);
            assert!(
                ids.insert(worker.worker_id.clone()),
                "duplicate id: {}",
                worker.worker_id
            );
            assert!(!worker.worker_id.contains('.'));
            assert!(!worker.worker_id.contains('@'));
            assert!(!worker.worker_id.contains('#'));

            let from_registry = registry
                .worker(&worker.runtime_id, &worker.worker_id)
                .unwrap();
            assert_eq!(from_registry.worker_id, worker.worker_id);
            let from_runtime = bridge.worker(&worker.worker_id).worker.unwrap();
            assert_eq!(from_runtime.worker_id, worker.worker_id);
        }

        let dotted = worker_id_for_pod("foo.bar");
        let dashed = worker_id_for_pod("foo-bar");
        assert_ne!(dotted, dashed);
        assert!(ids.contains(&dotted));
        assert!(ids.contains(&dashed));
        assert!(ids.iter().any(|id| id.len() == MAX_IDENTIFIER_LEN));
    }
}
