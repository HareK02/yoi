use crate::Error;
use chrono::Utc;
use pod_store::WorkerMetadata;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const LOCAL_RUNTIME_ID: &str = "local-worker-runtime";
const LOCAL_HOST_KIND: &str = "local-worker-host";
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerProxyConnectPoint {
    pub kind: String,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRegistryError {
    InvalidIdentifier { kind: &'static str, value: String },
    UnknownHost(String),
    UnknownWorker(String),
}

impl RuntimeRegistryError {
    pub fn into_error(self) -> Error {
        match self {
            Self::InvalidIdentifier { kind, value } => Error::InvalidRuntimeIdentifier {
                kind: kind.to_string(),
                value,
            },
            Self::UnknownHost(host_id) => Error::UnknownHost(host_id),
            Self::UnknownWorker(worker_id) => Error::UnknownWorker(worker_id),
        }
    }
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

    fn stop_worker(&self, request: WorkerStopRequest) -> WorkerStopResult {
        WorkerStopResult {
            state: WorkerOperationState::Unsupported,
            diagnostics: vec![diagnostic(
                "worker_stop_pending",
                DiagnosticSeverity::Info,
                format!(
                    "worker stop for '{}' is reserved for the runtime service boundary and is not implemented by this registry surface",
                    request.worker_id
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
pub struct WorkerRuntimeRegistry {
    runtimes: Vec<Arc<dyn WorkspaceWorkerRuntime>>,
}

impl WorkerRuntimeRegistry {
    pub fn new(runtimes: Vec<Arc<dyn WorkspaceWorkerRuntime>>) -> Self {
        Self { runtimes }
    }

    pub fn for_local_pods(runtime: LocalWorkerRuntime) -> Self {
        Self::new(vec![Arc::new(runtime)])
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

    pub fn worker(&self, worker_id: &str) -> Result<WorkerSummary, RuntimeRegistryError> {
        validate_backend_identifier("worker_id", worker_id)?;
        for runtime in &self.runtimes {
            let lookup = runtime.worker(worker_id);
            if let Some(worker) = lookup.worker {
                return Ok(worker);
            }
        }
        Err(RuntimeRegistryError::UnknownWorker(worker_id.to_string()))
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
        let registry = WorkerRuntimeRegistry::for_local_pods(LocalWorkerRuntime::new(
            "local:test",
            "/workspace/project",
            Some(temp.path().to_path_buf()),
        ));

        let runtimes = registry.list_runtimes(10);
        assert_eq!(runtimes.items[0].runtime_id, LOCAL_RUNTIME_ID);
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
        let registry = WorkerRuntimeRegistry::for_local_pods(LocalWorkerRuntime::new(
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
        let registry = WorkerRuntimeRegistry::for_local_pods(bridge.clone());

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

            let from_registry = registry.worker(&worker.worker_id).unwrap();
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
