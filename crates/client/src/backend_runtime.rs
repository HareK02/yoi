use futures::{SinkExt, StreamExt};
use protocol::stream::{decode_event, encode_method};
use protocol::{ErrorCode, Event, Method};
use serde::Deserialize;
use std::collections::VecDeque;
use std::fmt;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
pub use workdir::workspace::WorkingDirectorySummary as BackendWorkingDirectorySummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRuntimeTarget {
    /// Workspace Backend API root URL, for example `http://127.0.0.1:8787`.
    /// This is intentionally the Backend endpoint, not a Runtime endpoint.
    pub base_url: String,
    /// Workspace identity used for every Worker lifecycle and protocol operation.
    pub workspace_id: String,
    /// Backend-owned Runtime identity used as path authority.
    pub runtime_id: String,
    /// Backend-owned Worker identity used as path authority.
    pub worker_id: String,
}

impl BackendRuntimeTarget {
    pub fn new(
        base_url: impl Into<String>,
        workspace_id: impl Into<String>,
        runtime_id: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            workspace_id: workspace_id.into(),
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
        }
    }

    pub fn display_label(&self) -> String {
        format!("{}:{}", self.runtime_id, self.worker_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRuntimeListTarget {
    pub base_url: String,
    pub workspace_id: Option<String>,
    pub runtime_id: Option<String>,
}

impl BackendRuntimeListTarget {
    pub fn new(
        base_url: impl Into<String>,
        workspace_id: Option<String>,
        runtime_id: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            workspace_id,
            runtime_id,
        }
    }

    pub fn select_workspace(&mut self, workspace_id: impl Into<String>) {
        self.workspace_id = Some(workspace_id.into());
    }

    pub fn clear_workspace(&mut self) {
        self.workspace_id = None;
    }

    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    pub fn runtime_target(
        &self,
        runtime_id: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Result<BackendRuntimeTarget, BackendRuntimeClientError> {
        let workspace_id = self.workspace_id.clone().ok_or_else(|| {
            BackendRuntimeClientError::InvalidTarget(
                "workspace_id is required before selecting a Backend worker".to_string(),
            )
        })?;
        Ok(BackendRuntimeTarget::new(
            self.base_url.clone(),
            workspace_id,
            runtime_id,
            worker_id,
        ))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendRuntimeListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    #[serde(default)]
    pub diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendRuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub host_ids: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerImplementationSummary {
    pub kind: String,
    pub display_hint: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerCapabilitySummary {
    pub can_stop: bool,
    pub can_spawn_followup: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub resource_key: String,
    pub host_id: String,
    #[serde(default)]
    pub display_name: String,
    pub label: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub singleton_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub workspace: BackendWorkerWorkspaceSummary,
    pub state: String,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub retention_state: String,
    pub implementation: BackendWorkerImplementationSummary,
    pub capabilities: BackendWorkerCapabilitySummary,
    #[serde(default)]
    pub working_directory: Option<BackendWorkingDirectorySummary>,
    #[serde(default)]
    pub diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerRestoreResult {
    pub state: String,
    #[serde(default)]
    pub worker: Option<BackendWorkerSummary>,
    #[serde(default)]
    pub diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkerRestoreResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub result: BackendWorkerRestoreResult,
}

#[derive(Debug)]
pub struct BackendRuntimeClient {
    target: BackendRuntimeTarget,
    command_tx: mpsc::UnboundedSender<Method>,
    events: mpsc::UnboundedReceiver<Event>,
    diagnostics: VecDeque<Event>,
    _protocol_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub enum BackendRuntimeClientError {
    InvalidTarget(String),
    Http(reqwest::Error),
}

impl fmt::Display for BackendRuntimeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BackendRuntimeClientError {}

impl From<reqwest::Error> for BackendRuntimeClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

pub async fn list_backend_workers(
    target: &BackendRuntimeListTarget,
) -> Result<BackendRuntimeListResponse<BackendWorkerSummary>, BackendRuntimeClientError> {
    validate_list_target(target)?;
    let http = reqwest::Client::new();
    if let Some(runtime_id) = target.runtime_id.as_deref() {
        let path = backend_runtime_workers_path(
            target
                .workspace_id
                .as_deref()
                .expect("validated Backend Workspace scope"),
            runtime_id,
        );
        let url = join_base_and_path(&target.base_url, &path);
        return Ok(http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
            .await?);
    }

    let runtime_path = backend_runtimes_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
    );
    let runtime_url = join_base_and_path(&target.base_url, &runtime_path);
    let runtimes = http
        .get(runtime_url)
        .send()
        .await?
        .error_for_status()?
        .json::<BackendRuntimeListResponse<BackendRuntimeSummary>>()
        .await?;

    let mut items = Vec::new();
    let mut diagnostics = runtimes.diagnostics;
    for runtime in runtimes.items {
        let path = backend_runtime_workers_path(
            target
                .workspace_id
                .as_deref()
                .expect("validated Backend Workspace scope"),
            &runtime.runtime_id,
        );
        let url = join_base_and_path(&target.base_url, &path);
        match http
            .get(url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => {
                let response = response
                    .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
                    .await?;
                diagnostics.extend(response.diagnostics);
                items.extend(response.items);
            }
            Err(error) => diagnostics.push(BackendDiagnostic {
                code: "runtime_worker_list_failed".to_string(),
                severity: Some("error".to_string()),
                message: format!(
                    "failed to list workers for runtime {}: {error}",
                    runtime.runtime_id
                ),
            }),
        }
    }

    Ok(BackendRuntimeListResponse {
        workspace_id: runtimes.workspace_id,
        limit: runtimes.limit,
        items,
        source: "backend_runtime_worker_summary".to_string(),
        diagnostics,
    })
}

pub async fn list_backend_stopped_workers(
    target: &BackendRuntimeListTarget,
) -> Result<BackendRuntimeListResponse<BackendWorkerSummary>, BackendRuntimeClientError> {
    validate_list_target(target)?;
    let Some(runtime_id) = target.runtime_id.as_deref() else {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "stopped worker listing requires a runtime id".to_string(),
        ));
    };
    let http = reqwest::Client::new();
    let path = backend_runtime_workers_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
        runtime_id,
    );
    let url = join_base_and_path(&target.base_url, &format!("{path}?status=stopped"));
    Ok(http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
        .await?)
}

pub async fn restore_backend_worker(
    target: &BackendRuntimeTarget,
) -> Result<BackendWorkerRestoreResponse, BackendRuntimeClientError> {
    validate_target(target)?;
    let http = reqwest::Client::new();
    let path = backend_runtime_worker_restore_path(
        &target.workspace_id,
        &target.runtime_id,
        &target.worker_id,
    );
    let url = join_base_and_path(&target.base_url, &path);
    Ok(http
        .post(url)
        .json(&serde_json::json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<BackendWorkerRestoreResponse>()
        .await?)
}

impl BackendRuntimeClient {
    pub async fn connect(target: BackendRuntimeTarget) -> Result<Self, BackendRuntimeClientError> {
        validate_target(&target)?;
        let (event_tx, rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let protocol_target = target.clone();
        let protocol_event_tx = event_tx.clone();
        let protocol_task = tokio::spawn(async move {
            run_worker_protocol_transport(protocol_target, command_rx, protocol_event_tx).await;
        });

        Ok(Self {
            target,
            command_tx,
            events: rx,
            diagnostics: VecDeque::new(),
            _protocol_task: protocol_task,
        })
    }

    pub fn try_next_event(&mut self) -> Option<Event> {
        if let Some(event) = self.diagnostics.pop_front() {
            return Some(event);
        }
        self.events.try_recv().ok()
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        if let Some(event) = self.diagnostics.pop_front() {
            return Some(event);
        }
        self.events.recv().await
    }

    pub async fn send(&mut self, method: &Method) -> Result<(), BackendRuntimeClientError> {
        self.command_tx.send(method.clone()).map_err(|_| {
            BackendRuntimeClientError::InvalidTarget(format!(
                "Backend protocol command stream is closed for {}",
                self.target.display_label()
            ))
        })?;
        Ok(())
    }
}

impl Drop for BackendRuntimeClient {
    fn drop(&mut self) {
        self._protocol_task.abort();
    }
}

async fn run_worker_protocol_transport(
    target: BackendRuntimeTarget,
    mut commands: mpsc::UnboundedReceiver<Method>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let url = protocol_ws_url(&target);
    match connect_async(&url).await {
        Ok((ws, _)) => {
            let (mut sink, mut stream) = ws.split();
            loop {
                tokio::select! {
                    maybe_method = commands.recv() => {
                        let Some(method) = maybe_method else {
                            break;
                        };
                        match encode_method(&method) {
                            Ok(text) => {
                                if let Err(error) = sink.send(TungsteniteMessage::Text(text.into())).await {
                                    let _ = tx.send(diagnostic_event(format!(
                                        "Backend protocol command send failed for {}: {error}",
                                        target.display_label()
                                    )));
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = tx.send(diagnostic_event(format!(
                                    "Backend protocol command could not serialize method for {}: {error}",
                                    target.display_label()
                                )));
                            }
                        }
                    }
                    frame = stream.next() => {
                        match frame {
                            Some(Ok(TungsteniteMessage::Text(text))) => {
                                match decode_event(&text) {
                                    Ok(event) => {
                                        let _ = tx.send(event);
                                    }
                                    Err(error) => {
                                        let _ = tx.send(diagnostic_event(format!(
                                            "Backend protocol response was not valid Event JSON for {}: {error}",
                                            target.display_label()
                                        )));
                                    }
                                }
                            }
                            Some(Ok(TungsteniteMessage::Close(_))) | None => {
                                let _ = tx.send(diagnostic_event(format!(
                                    "Backend protocol command stream closed for {}",
                                    target.display_label()
                                )));
                                break;
                            }
                            Some(Ok(TungsteniteMessage::Ping(_)))
                            | Some(Ok(TungsteniteMessage::Pong(_)))
                            | Some(Ok(TungsteniteMessage::Binary(_)))
                            | Some(Ok(TungsteniteMessage::Frame(_))) => {}
                            Some(Err(error)) => {
                                let _ = tx.send(diagnostic_event(format!(
                                    "Backend protocol WebSocket error for {}: {error}",
                                    target.display_label()
                                )));
                                break;
                            }
                        }
                    }
                }
            }
        }
        Err(error) => {
            let _ = tx.send(diagnostic_event(format!(
                "Backend protocol WebSocket connect failed for {}: {error}",
                target.display_label()
            )));
            while commands.recv().await.is_some() {
                let _ = tx.send(diagnostic_event(format!(
                    "Backend protocol command was not sent because command stream is unavailable for {}",
                    target.display_label()
                )));
            }
        }
    }
}

fn diagnostic_event(message: impl Into<String>) -> Event {
    Event::Error {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

fn validate_target(target: &BackendRuntimeTarget) -> Result<(), BackendRuntimeClientError> {
    if target.base_url.trim().is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL is required".to_string(),
        ));
    }
    if !(target.base_url.starts_with("http://") || target.base_url.starts_with("https://")) {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL must start with http:// or https://".to_string(),
        ));
    }
    if target.workspace_id.is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "workspace_id is required".to_string(),
        ));
    }
    if target.runtime_id.is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "runtime_id is required".to_string(),
        ));
    }
    if target.worker_id.is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "worker_id is required".to_string(),
        ));
    }
    Ok(())
}

fn validate_list_target(
    target: &BackendRuntimeListTarget,
) -> Result<(), BackendRuntimeClientError> {
    if target.base_url.trim().is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL is required".to_string(),
        ));
    }
    if !(target.base_url.starts_with("http://")) && !(target.base_url.starts_with("https://")) {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL must start with http:// or https://".to_string(),
        ));
    }
    match target.workspace_id.as_deref() {
        Some("") => {
            return Err(BackendRuntimeClientError::InvalidTarget(
                "workspace_id must not be empty".to_string(),
            ));
        }
        None => {
            return Err(BackendRuntimeClientError::InvalidTarget(
                "workspace selection is required before listing Backend workers".to_string(),
            ));
        }
        Some(_) => {}
    }
    if target.runtime_id.as_deref().is_some_and(str::is_empty) {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "runtime_id must not be empty when provided".to_string(),
        ));
    }
    Ok(())
}

fn backend_runtimes_path(workspace_id: &str) -> String {
    format!("/api/w/{}/runtimes", path_segment_encode(workspace_id))
}

fn backend_runtime_workers_path(workspace_id: &str, runtime_id: &str) -> String {
    format!(
        "/api/w/{}/runtimes/{}/workers",
        path_segment_encode(workspace_id),
        path_segment_encode(runtime_id)
    )
}

fn backend_runtime_worker_restore_path(
    workspace_id: &str,
    runtime_id: &str,
    worker_id: &str,
) -> String {
    format!(
        "/api/w/{}/runtimes/{}/workers/{}/restore",
        path_segment_encode(workspace_id),
        path_segment_encode(runtime_id),
        path_segment_encode(worker_id)
    )
}

fn protocol_ws_url(target: &BackendRuntimeTarget) -> String {
    let path = format!(
        "/api/w/{}/runtimes/{}/workers/{}/protocol/ws",
        path_segment_encode(&target.workspace_id),
        path_segment_encode(&target.runtime_id),
        path_segment_encode(&target.worker_id)
    );
    join_base_and_path(&http_base_to_ws(&target.base_url), &path)
}

fn http_base_to_ws(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

fn join_base_and_path(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn path_segment_encode(input: &str) -> String {
    percent_encode(input, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
    })
}

fn percent_encode(input: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        if keep(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendDiagnostic {
    pub code: String,
    #[serde(default)]
    pub severity: Option<String>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_url_uses_backend_runtime_worker_identity() {
        let target = BackendRuntimeTarget::new(
            "http://127.0.0.1:8787/",
            "workspace alpha",
            "runtime/one",
            "worker one",
        );
        assert_eq!(
            protocol_ws_url(&target),
            "ws://127.0.0.1:8787/api/w/workspace%20alpha/runtimes/runtime%2Fone/workers/worker%20one/protocol/ws"
        );
    }

    #[test]
    fn backend_worker_summary_decodes_current_occupied_workdir_contract() {
        let payload = serde_json::json!({
            "runtime_id": "arcadia",
            "worker_id": "worker-opaque-64",
            "resource_key": "W-64",
            "host_id": "host",
            "display_name": "Coder",
            "label": "Coder",
            "workspace": {"visibility": "workspace", "identity": "workspace"},
            "state": "idle",
            "implementation": {"kind": "worker", "display_hint": "Coder"},
            "capabilities": {"can_stop": true, "can_spawn_followup": false},
            "working_directory": {
                "working_directory_id": "wd-1",
                "repository_id": "main",
                "materializer_kind": "local_git_worktree",
                "status": "active",
                "occupied_by": {
                    "runtime_id": "arcadia",
                    "worker_id": "worker-opaque-64",
                    "display_name": "Coder",
                    "linked_at": "2026-08-12T00:00:00Z"
                }
            }
        });

        let worker: BackendWorkerSummary = serde_json::from_value(payload.clone()).unwrap();
        let occupied_by = worker
            .working_directory
            .unwrap()
            .occupied_by
            .expect("occupied Workdir");
        assert_eq!(occupied_by.worker.runtime_id, "arcadia");
        assert_eq!(occupied_by.worker.worker_id, "worker-opaque-64");

        let mut stale = payload;
        stale["working_directory"]["occupied_by"]["runtime_worker_id"] = serde_json::json!(64);
        assert!(serde_json::from_value::<BackendWorkerSummary>(stale).is_err());
    }

    #[test]
    fn workers_path_requires_workspace_scope_for_status_queries() {
        let path = backend_runtime_workers_path("team main", "runtime/one");
        assert_eq!(
            format!("{path}?status=stopped"),
            "/api/w/team%20main/runtimes/runtime%2Fone/workers?status=stopped"
        );
    }

    #[test]
    fn restore_worker_path_requires_workspace_scope() {
        assert_eq!(
            backend_runtime_worker_restore_path("team main", "runtime/one", "worker one"),
            "/api/w/team%20main/runtimes/runtime%2Fone/workers/worker%20one/restore"
        );
    }
}
