use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pod_store::{PodMetadata, validate_pod_name};
use serde::{Deserialize, Serialize};

const MAX_DIAGNOSTICS: usize = 20;
const MAX_LABEL_LEN: usize = 120;
const MAX_PATH_LEN: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSummary {
    pub host_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub observed_at: String,
    pub last_seen_at: String,
    pub capabilities: HostCapabilitySummary,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostCapabilitySummary {
    pub local_pod_inspection: String,
    pub workspace_root: String,
    pub os: String,
    pub arch: String,
    pub max_workers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSummary {
    pub worker_id: String,
    pub host_id: String,
    pub label: String,
    pub pod_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    pub state: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    pub implementation: WorkerImplementation,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerImplementation {
    pub kind: String,
    pub pod_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeList<T> {
    pub items: Vec<T>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
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
/// profile selectors are resolved by the host/runtime service and never accepted
/// from Workspace API callers.
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

pub trait WorkspaceWorkerRuntime: Send + Sync {
    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary>;
    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary>;
    fn worker(&self, worker_id: &str) -> WorkerLookupResult;
    fn spawn_worker(&self, request: WorkerSpawnRequest) -> WorkerSpawnResult;
    fn stop_worker(&self, request: WorkerStopRequest) -> WorkerStopResult;
    fn proxy_connect_points(&self, worker_id: &str) -> Vec<WorkerProxyConnectPoint>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRuntimeBridge {
    workspace_id: String,
    workspace_root: PathBuf,
    data_dir: Option<PathBuf>,
}

impl LocalRuntimeBridge {
    pub fn new(
        workspace_id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        data_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            workspace_root: workspace_root.into(),
            data_dir,
        }
    }

    pub fn host_id(&self) -> String {
        stable_local_host_id(&self.workspace_id)
    }

    pub fn list_hosts(&self, limit: usize) -> (Vec<HostSummary>, Vec<RuntimeDiagnostic>) {
        if limit == 0 {
            return (Vec::new(), Vec::new());
        }

        let observed_at = unix_timestamp(SystemTime::now());
        let mut diagnostics = pod_root_diagnostics(self.pod_root().as_deref());
        let local_pod_inspection = if diagnostics.is_empty() {
            "available"
        } else {
            "unavailable"
        }
        .to_string();
        let status = if local_pod_inspection == "available" {
            "available"
        } else {
            "degraded"
        }
        .to_string();
        truncate_diagnostics(&mut diagnostics);

        let host = HostSummary {
            host_id: self.host_id(),
            label: format!(
                "Local host ({})",
                self.workspace_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("workspace root must have a final path component")
            ),
            kind: "local_host".to_string(),
            status,
            observed_at: observed_at.clone(),
            last_seen_at: observed_at,
            capabilities: HostCapabilitySummary {
                local_pod_inspection,
                workspace_root: bounded_path(&self.workspace_root),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                max_workers: limit.min(200),
            },
            diagnostics: diagnostics.clone(),
        };

        (vec![host], diagnostics)
    }

    pub fn list_workers(&self, limit: usize) -> (Vec<WorkerSummary>, Vec<RuntimeDiagnostic>) {
        let limit = limit.min(200);
        let Some(pod_root) = self.pod_root() else {
            return (
                Vec::new(),
                vec![RuntimeDiagnostic::new(
                    "local_yoi_data_dir_unavailable",
                    "warning",
                    "local Yoi data directory is not configured; local Pod workers cannot be inspected",
                )],
            );
        };

        let mut diagnostics = Vec::new();
        if !pod_root.exists() {
            diagnostics.push(RuntimeDiagnostic::new(
                "local_pod_metadata_root_missing",
                "info",
                "local Pod metadata directory is absent; no local workers were discovered",
            ));
            return (Vec::new(), diagnostics);
        }

        let entries = match fs::read_dir(&pod_root) {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(RuntimeDiagnostic::new(
                    "local_pod_metadata_root_unreadable",
                    "warning",
                    format!("local Pod metadata directory cannot be read: {error}"),
                ));
                return (Vec::new(), diagnostics);
            }
        };

        let mut workers = Vec::new();
        let mut candidate_names = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    push_diagnostic(
                        &mut diagnostics,
                        RuntimeDiagnostic::new(
                            "local_pod_metadata_entry_unreadable",
                            "warning",
                            format!("one local Pod metadata entry cannot be read: {error}"),
                        ),
                    );
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    push_diagnostic(
                        &mut diagnostics,
                        RuntimeDiagnostic::new(
                            "local_pod_metadata_entry_type_unreadable",
                            "warning",
                            format!("one local Pod metadata entry type cannot be read: {error}"),
                        ),
                    );
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                push_diagnostic(
                    &mut diagnostics,
                    RuntimeDiagnostic::new(
                        "local_pod_name_non_utf8",
                        "warning",
                        "one local Pod metadata directory has a non-UTF-8 name and was skipped",
                    ),
                );
                continue;
            };
            if validate_pod_name(&name).is_err() || name.len() > MAX_LABEL_LEN {
                push_diagnostic(
                    &mut diagnostics,
                    RuntimeDiagnostic::new(
                        "local_pod_name_invalid",
                        "warning",
                        "one local Pod metadata directory has an invalid or oversized name and was skipped",
                    ),
                );
                continue;
            }
            if entry.path().join("metadata.json").exists() {
                candidate_names.push(name);
            }
        }

        candidate_names.sort();
        candidate_names.truncate(limit);
        for pod_name in candidate_names {
            match read_worker(&pod_root, &pod_name, &self.host_id()) {
                Ok(worker) => workers.push(worker),
                Err(diagnostic) => push_diagnostic(&mut diagnostics, diagnostic),
            }
        }
        truncate_diagnostics(&mut diagnostics);
        (workers, diagnostics)
    }

    fn pod_root(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|data_dir| data_dir.join("pods"))
    }
}

impl WorkspaceWorkerRuntime for LocalRuntimeBridge {
    fn list_hosts(&self, limit: usize) -> RuntimeList<HostSummary> {
        let (items, diagnostics) = LocalRuntimeBridge::list_hosts(self, limit);
        RuntimeList { items, diagnostics }
    }

    fn list_workers(&self, limit: usize) -> RuntimeList<WorkerSummary> {
        let (items, diagnostics) = LocalRuntimeBridge::list_workers(self, limit);
        RuntimeList { items, diagnostics }
    }

    fn worker(&self, worker_id: &str) -> WorkerLookupResult {
        let RuntimeList {
            items,
            mut diagnostics,
        } = WorkspaceWorkerRuntime::list_workers(self, 200);
        let worker = items
            .into_iter()
            .find(|worker| worker.worker_id == worker_id);
        if worker.is_none() {
            diagnostics.push(RuntimeDiagnostic::new(
                "worker_not_found",
                "info",
                format!("worker '{worker_id}' was not found on the local runtime"),
            ));
        }
        truncate_diagnostics(&mut diagnostics);
        WorkerLookupResult {
            worker,
            diagnostics,
        }
    }

    fn spawn_worker(&self, request: WorkerSpawnRequest) -> WorkerSpawnResult {
        let diagnostic = RuntimeDiagnostic::new(
            "worker_spawn_resolver_pending",
            "info",
            format!(
                "worker spawn intent '{}' was accepted as a typed request shape, but local launch resolution is not implemented by this ticket",
                worker_spawn_intent_label(&request.intent)
            ),
        );
        WorkerSpawnResult {
            state: WorkerOperationState::Unsupported,
            worker: None,
            acceptance_evidence: Vec::new(),
            diagnostics: vec![diagnostic],
        }
    }

    fn stop_worker(&self, request: WorkerStopRequest) -> WorkerStopResult {
        WorkerStopResult {
            state: WorkerOperationState::Unsupported,
            diagnostics: vec![RuntimeDiagnostic::new(
                "worker_stop_pending",
                "info",
                format!(
                    "worker stop for '{}' is reserved for the runtime service boundary and is not implemented by this ticket",
                    request.worker_id
                ),
            )],
        }
    }

    fn proxy_connect_points(&self, worker_id: &str) -> Vec<WorkerProxyConnectPoint> {
        vec![WorkerProxyConnectPoint {
            kind: "stream_proxy".to_string(),
            status: "not_implemented".to_string(),
            diagnostics: vec![RuntimeDiagnostic::new(
                "worker_stream_proxy_pending",
                "info",
                format!(
                    "future stream/proxy connection point for '{worker_id}' is reserved without opening a protocol surface"
                ),
            )],
        }]
    }
}

impl RuntimeDiagnostic {
    pub fn new(
        code: impl Into<String>,
        severity: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: truncate_string(&code.into(), MAX_LABEL_LEN),
            severity: truncate_string(&severity.into(), MAX_LABEL_LEN),
            message: truncate_string(&message.into(), 240),
        }
    }
}

fn read_worker(
    pod_root: &Path,
    pod_name: &str,
    host_id: &str,
) -> Result<WorkerSummary, RuntimeDiagnostic> {
    let metadata_path = pod_root.join(pod_name).join("metadata.json");
    let last_seen_at = metadata_path
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(unix_timestamp);
    let raw = fs::read_to_string(&metadata_path).map_err(|error| {
        RuntimeDiagnostic::new(
            "local_pod_metadata_unreadable",
            "warning",
            format!("local Pod metadata for `{pod_name}` cannot be read: {error}"),
        )
    })?;
    let metadata: PodMetadata = serde_json::from_str(&raw).map_err(|error| {
        RuntimeDiagnostic::new(
            "local_pod_metadata_invalid",
            "warning",
            format!("local Pod metadata for `{pod_name}` is invalid: {error}"),
        )
    })?;

    let mut worker_diagnostics = Vec::new();
    if metadata.pod_name != pod_name {
        worker_diagnostics.push(RuntimeDiagnostic::new(
            "local_pod_metadata_name_mismatch",
            "warning",
            "metadata pod_name differed from its directory name; the directory name was used",
        ));
    }

    let state = if metadata.active.is_some() {
        "active"
    } else {
        "inactive"
    }
    .to_string();
    let status = match metadata.active.as_ref() {
        Some(active) if active.segment_id.is_some() => "active_segment_known",
        Some(_) => "active_session_pending_segment",
        None => "metadata_only",
    }
    .to_string();
    let (role, profile) = extract_safe_role_profile(metadata.resolved_manifest_snapshot.as_ref());

    Ok(WorkerSummary {
        worker_id: format!("local-pod-{}", sanitize_identifier(pod_name, MAX_LABEL_LEN)),
        host_id: host_id.to_string(),
        label: truncate_string(pod_name, MAX_LABEL_LEN),
        pod_name: truncate_string(pod_name, MAX_LABEL_LEN),
        role,
        profile,
        workspace_root: metadata
            .workspace_root
            .as_ref()
            .map(|path| bounded_path(path)),
        state,
        status,
        last_seen_at,
        implementation: WorkerImplementation {
            kind: "local_pod".to_string(),
            pod_name: truncate_string(pod_name, MAX_LABEL_LEN),
        },
        diagnostics: worker_diagnostics,
    })
}

fn pod_root_diagnostics(pod_root: Option<&Path>) -> Vec<RuntimeDiagnostic> {
    let Some(pod_root) = pod_root else {
        return vec![RuntimeDiagnostic::new(
            "local_yoi_data_dir_unavailable",
            "warning",
            "local Yoi data directory is not configured; local Pod inspection is unavailable",
        )];
    };
    if !pod_root.exists() {
        return vec![RuntimeDiagnostic::new(
            "local_pod_metadata_root_missing",
            "info",
            "local Pod metadata directory is absent; local Pod inspection found no workers",
        )];
    }
    match fs::read_dir(pod_root) {
        Ok(_) => Vec::new(),
        Err(error) => vec![RuntimeDiagnostic::new(
            "local_pod_metadata_root_unreadable",
            "warning",
            format!("local Pod metadata directory cannot be read: {error}"),
        )],
    }
}

fn extract_safe_role_profile(
    snapshot: Option<&serde_json::Value>,
) -> (Option<String>, Option<String>) {
    let Some(snapshot) = snapshot else {
        return (None, None);
    };
    let role = snapshot
        .get("role")
        .and_then(|value| value.as_str())
        .and_then(safe_metadata_label);
    let profile = snapshot
        .get("profile")
        .and_then(|profile| {
            profile
                .get("name")
                .or_else(|| profile.get("selector"))
                .or_else(|| profile.get("id"))
        })
        .and_then(|value| value.as_str())
        .and_then(safe_metadata_label);
    (role, profile)
}

fn safe_metadata_label(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_LABEL_LEN
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
        || value.chars().any(|ch| ch.is_control())
    {
        return None;
    }
    Some(value.to_string())
}

fn worker_spawn_intent_label(intent: &WorkerSpawnIntent) -> &'static str {
    match intent {
        WorkerSpawnIntent::WorkspaceCompanion => "workspace_companion",
        WorkerSpawnIntent::WorkspaceOrchestrator => "workspace_orchestrator",
        WorkerSpawnIntent::TicketRole { .. } => "ticket_role",
    }
}

fn stable_local_host_id(workspace_id: &str) -> String {
    format!("local-{}", sanitize_identifier(workspace_id, 96))
}

fn sanitize_identifier(value: &str, max_len: usize) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if output.len() >= max_len {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if !output.ends_with('-') {
            output.push('-');
        }
    }
    let output = output.trim_matches('-');
    if output.is_empty() {
        "local".to_string()
    } else {
        output.to_string()
    }
}

fn bounded_path(path: &Path) -> String {
    truncate_string(&path.to_string_lossy(), MAX_PATH_LEN)
}

fn truncate_string(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut end = max_len;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn push_diagnostic(diagnostics: &mut Vec<RuntimeDiagnostic>, diagnostic: RuntimeDiagnostic) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(diagnostic);
    }
}

fn truncate_diagnostics(diagnostics: &mut Vec<RuntimeDiagnostic>) {
    diagnostics.truncate(MAX_DIAGNOSTICS);
}

fn unix_timestamp(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("unix:{}", duration.as_secs()),
        Err(_) => "unix:0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_workers_from_local_pod_metadata_without_exposing_snapshot_contents() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let worker_dir = data_dir.join("pods/coder");
        fs::create_dir_all(&worker_dir).unwrap();
        fs::write(
            worker_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&json!({
                "pod_name": "coder",
                "active": {
                    "session_id": "018f4b8e-7c8a-7b41-8d66-111111111111",
                    "segment_id": "018f4b8e-7c8a-7b41-8d66-222222222222"
                },
                "workspace_root": "/workspace/project",
                "resolved_manifest_snapshot": {
                    "role": "coder",
                    "profile": { "name": "builtin-coder" },
                    "secret_token": "do-not-return",
                    "system_prompt": "do-not-return"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let bridge = LocalRuntimeBridge::new("local:test", "/workspace/project", Some(data_dir));
        let (workers, diagnostics) = bridge.list_workers(20);
        assert!(diagnostics.is_empty());
        assert_eq!(workers.len(), 1);
        let worker = &workers[0];
        assert_eq!(worker.worker_id, "local-pod-coder");
        assert_eq!(worker.host_id, "local-local-test");
        assert_eq!(worker.pod_name, "coder");
        assert_eq!(worker.role.as_deref(), Some("coder"));
        assert_eq!(worker.profile.as_deref(), Some("builtin-coder"));
        assert_eq!(worker.workspace_root.as_deref(), Some("/workspace/project"));
        assert_eq!(worker.state, "active");
        assert_eq!(worker.status, "active_segment_known");
        assert_eq!(worker.implementation.kind, "local_pod");
        assert_eq!(worker.implementation.pod_name, "coder");

        let response_json = serde_json::to_string(&workers).unwrap();
        assert!(!response_json.contains("do-not-return"));
        assert!(!response_json.contains("system_prompt"));
        assert!(!response_json.contains("session_id"));
        assert!(!response_json.contains("segment_id"));
    }

    #[test]
    fn missing_local_pod_data_dir_degrades_to_empty_workers_and_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let bridge = LocalRuntimeBridge::new(
            "local:test",
            temp.path(),
            Some(temp.path().join("missing-data")),
        );
        let (workers, diagnostics) = bridge.list_workers(20);
        assert!(workers.is_empty());
        assert_eq!(diagnostics[0].code, "local_pod_metadata_root_missing");

        let (hosts, host_diagnostics) = bridge.list_hosts(20);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].status, "degraded");
        assert_eq!(hosts[0].capabilities.local_pod_inspection, "unavailable");
        assert_eq!(host_diagnostics[0].code, "local_pod_metadata_root_missing");
    }

    #[test]
    fn worker_list_and_diagnostics_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let pod_root = data_dir.join("pods");
        fs::create_dir_all(&pod_root).unwrap();
        for index in 0..250 {
            let worker_dir = pod_root.join(format!("worker-{index:03}"));
            fs::create_dir_all(&worker_dir).unwrap();
            fs::write(
                worker_dir.join("metadata.json"),
                serde_json::to_vec_pretty(&json!({
                    "pod_name": format!("worker-{index:03}"),
                    "workspace_root": "/workspace/project"
                }))
                .unwrap(),
            )
            .unwrap();
        }

        let bridge = LocalRuntimeBridge::new("local:test", "/workspace/project", Some(data_dir));
        let (workers, diagnostics) = bridge.list_workers(5);
        assert_eq!(workers.len(), 5);
        assert!(diagnostics.len() <= MAX_DIAGNOSTICS);
        assert_eq!(workers[0].pod_name, "worker-000");
        assert_eq!(workers[4].pod_name, "worker-004");
    }
}
