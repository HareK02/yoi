use crate::transport::websocket::{Socket as WebSocket, SocketError as WebSocketError};
use crate::{BackendApiClient, BackendApiClientError, Client};
use reqwest::Method as HttpMethod;
use std::fmt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
pub use workspace_api::{
    Diagnostic as BackendDiagnostic, DiagnosticSeverity as BackendDiagnosticSeverity,
    ListResponse as BackendRuntimeListResponse, RuntimeSummary as BackendRuntimeSummary,
    WorkerCapabilitySummary as BackendWorkerCapabilitySummary,
    WorkerImplementationSummary as BackendWorkerImplementationSummary,
    WorkerRestoreResponse as BackendWorkerRestoreResponse,
    WorkerRestoreResult as BackendWorkerRestoreResult, WorkerSummary as BackendWorkerSummary,
    WorkerWorkspaceSummary as BackendWorkerWorkspaceSummary,
    WorkingDirectoryCreateRequest as BackendWorkingDirectoryCreateRequest,
    WorkingDirectoryCreateResponse as BackendWorkingDirectoryCreateResponse,
    WorkingDirectoryDetailResponse as BackendWorkingDirectoryDetailResponse,
    WorkingDirectoryListResponse as BackendWorkingDirectoryListResponse,
    WorkingDirectorySummary as BackendWorkingDirectorySummary,
};

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

#[derive(Debug)]
pub enum BackendRuntimeClientError {
    InvalidTarget(String),
    Api(BackendApiClientError),
    Http(reqwest::Error),
    Protocol(String),
}

impl fmt::Display for BackendRuntimeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Api(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BackendRuntimeClientError {}

impl From<BackendApiClientError> for BackendRuntimeClientError {
    fn from(error: BackendApiClientError) -> Self {
        Self::Api(error)
    }
}

impl From<reqwest::Error> for BackendRuntimeClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

pub async fn list_backend_workers(
    target: &BackendRuntimeListTarget,
) -> Result<BackendRuntimeListResponse<BackendWorkerSummary>, BackendRuntimeClientError> {
    validate_list_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    if let Some(runtime_id) = target.runtime_id.as_deref() {
        let path = backend_runtime_workers_path(
            target
                .workspace_id
                .as_deref()
                .expect("validated Backend Workspace scope"),
            runtime_id,
        );
        let response = api.request(HttpMethod::GET, &path)?.send().await?;
        api.check_status(response.status())?;
        return Ok(response
            .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
            .await?);
    }

    let runtime_path = backend_runtimes_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
    );
    let response = api.request(HttpMethod::GET, &runtime_path)?.send().await?;
    api.check_status(response.status())?;
    let runtimes = response
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
        let response = match api.request(HttpMethod::GET, &path)?.send().await {
            Ok(response) => response,
            Err(error) => {
                diagnostics.push(BackendDiagnostic {
                    code: "runtime_worker_list_failed".to_string(),
                    severity: BackendDiagnosticSeverity::Error,
                    message: format!(
                        "failed to list workers for runtime {}: {error}",
                        runtime.runtime_id
                    ),
                });
                continue;
            }
        };
        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            api.check_status(response.status())?;
        }
        if !response.status().is_success() {
            diagnostics.push(BackendDiagnostic {
                code: "runtime_worker_list_failed".to_string(),
                severity: BackendDiagnosticSeverity::Error,
                message: format!(
                    "failed to list workers for runtime {}: Backend returned HTTP {}",
                    runtime.runtime_id,
                    response.status().as_u16()
                ),
            });
            continue;
        }
        let response = response
            .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
            .await?;
        diagnostics.extend(response.diagnostics);
        items.extend(response.items);
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
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    let path = backend_runtime_workers_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
        runtime_id,
    );
    let response = api
        .request(HttpMethod::GET, &format!("{path}?status=stopped"))?
        .send()
        .await?;
    api.check_status(response.status())?;
    Ok(response
        .json::<BackendRuntimeListResponse<BackendWorkerSummary>>()
        .await?)
}

pub async fn restore_backend_worker(
    target: &BackendRuntimeTarget,
) -> Result<BackendWorkerRestoreResponse, BackendRuntimeClientError> {
    validate_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    let path = backend_runtime_worker_restore_path(
        &target.workspace_id,
        &target.runtime_id,
        &target.worker_id,
    );
    let response = api
        .request(HttpMethod::POST, &path)?
        .json(&serde_json::json!({}))
        .send()
        .await?;
    let response = api.require_success(response).await?;
    Ok(response.json::<BackendWorkerRestoreResponse>().await?)
}

pub async fn connect_backend_runtime(
    target: BackendRuntimeTarget,
) -> Result<Client<WebSocket>, BackendRuntimeClientError> {
    validate_target(&target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    let request = protocol_ws_request(&target, &api).map_err(|error| {
        BackendRuntimeClientError::Protocol(format!(
            "Backend protocol request could not be constructed for {}: {error}",
            target.display_label()
        ))
    })?;
    match WebSocket::connect(request).await {
        Ok(socket) => Ok(Client::new(socket)),
        Err(WebSocketError::WebSocket(error)) => Err(BackendRuntimeClientError::Protocol(
            protocol_connect_error_message(&target, &api, &error),
        )),
    }
}

fn protocol_connect_error_message(
    target: &BackendRuntimeTarget,
    api: &BackendApiClient,
    error: &tokio_tungstenite::tungstenite::Error,
) -> String {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        if let Ok(status) = reqwest::StatusCode::from_u16(response.status().as_u16()) {
            if matches!(
                status,
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            ) {
                if let Err(error) = api.check_status(status) {
                    return error.to_string();
                }
            }
        }
    }
    format!(
        "Backend protocol WebSocket connect failed for {}: {error}",
        target.display_label()
    )
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

fn protocol_ws_request(
    target: &BackendRuntimeTarget,
    api: &BackendApiClient,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    let mut request = protocol_ws_url(target)
        .into_client_request()
        .map_err(|error| error.to_string())?;
    let value = HeaderValue::from_str(&api.authorization_header_value())
        .map_err(|_| "saved Backend token is not a valid Authorization header".to_string())?;
    request.headers_mut().insert(AUTHORIZATION, value);
    Ok(request)
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
    fn protocol_request_attaches_saved_bearer_authorization() {
        let target = BackendRuntimeTarget::new(
            "http://127.0.0.1:8787/",
            "workspace alpha",
            "runtime/one",
            "worker one",
        );
        let api = BackendApiClient::from_access_token_for_test(
            "http://127.0.0.1:8787",
            "websocket-secret",
        )
        .unwrap();
        let request = protocol_ws_request(&target, &api).unwrap();
        assert_eq!(
            request.headers().get(AUTHORIZATION).unwrap(),
            "Bearer websocket-secret"
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
        assert_eq!(occupied_by.runtime_id, "arcadia");
        assert_eq!(occupied_by.worker_id, "worker-opaque-64");

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
