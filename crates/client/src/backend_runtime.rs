use crate::transport::websocket::{Socket as WebSocket, SocketError as WebSocketError};
use crate::{BackendApiClient, BackendApiClientError, Client};
use reqwest::Method as HttpMethod;
use serde::Deserialize;
pub use server_api::{
    BrowserCreateWorkerResponse as BackendCreateWorkerResponse,
    CreateWorkspaceWorkerRequest as BackendCreateWorkerRequest, Diagnostic as BackendDiagnostic,
    DiagnosticSeverity as BackendDiagnosticSeverity, ListResponse as BackendRuntimeListResponse,
    RuntimeSummary as BackendRuntimeSummary,
    WorkerCapabilitySummary as BackendWorkerCapabilitySummary,
    WorkerImplementationSummary as BackendWorkerImplementationSummary,
    WorkerLaunchOptionsResponse as BackendWorkerLaunchOptions,
    WorkerLaunchProfileCandidate as BackendWorkerLaunchProfileCandidate,
    WorkerLaunchRuntimeOption as BackendWorkerLaunchRuntimeOption,
    WorkerOperationState as BackendWorkerOperationState,
    WorkerRestoreResponse as BackendWorkerRestoreResponse,
    WorkerRestoreResult as BackendWorkerRestoreResult,
    WorkerRestoreState as BackendWorkerRestoreState, WorkerSummary as BackendWorkerSummary,
    WorkerWorkspaceSummary as BackendWorkerWorkspaceSummary,
    WorkingDirectoryCreateRequest as BackendWorkingDirectoryCreateRequest,
    WorkingDirectoryCreateResponse as BackendWorkingDirectoryCreateResponse,
    WorkingDirectoryDetailResponse as BackendWorkingDirectoryDetailResponse,
    WorkingDirectoryListResponse as BackendWorkingDirectoryListResponse,
    WorkingDirectorySource as BackendWorkingDirectorySource,
    WorkingDirectorySummary as BackendWorkingDirectorySummary,
};
use std::fmt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

#[derive(Debug, Clone, PartialEq)]
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
    pub initial_snapshot: Option<protocol::SessionSnapshot>,
    pub initial_notice: Option<String>,
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
            initial_snapshot: None,
            initial_notice: None,
        }
    }

    pub fn display_label(&self) -> String {
        format!("{}:{}", self.runtime_id, self.worker_id)
    }

    pub async fn upload_file(
        &self,
        file_name: &str,
        media_type: &str,
        content: Vec<u8>,
    ) -> Result<protocol::UploadedFileRef, BackendRuntimeClientError> {
        self.upload_file_with_id(
            &uuid::Uuid::now_v7().to_string(),
            file_name,
            media_type,
            content,
        )
        .await
    }

    pub async fn upload_file_with_id(
        &self,
        upload_id: &str,
        file_name: &str,
        media_type: &str,
        content: Vec<u8>,
    ) -> Result<protocol::UploadedFileRef, BackendRuntimeClientError> {
        let api = BackendApiClient::from_stored_token(&self.base_url)?;
        let client = runtime_server_api_client(&self.base_url, &api)?;
        let grant = client
            .runtime_worker_attachment_upload_grant(
                self.workspace_id.clone(),
                self.runtime_id.clone(),
                self.worker_id.clone(),
                server_api::WorkerFileUploadQuery {
                    file_name: file_name.to_string(),
                    media_type: media_type.to_string(),
                    upload_id: Some(upload_id.to_string()),
                },
            )
            .await?;
        let worker_path = format!(
            "/api/w/{}/runtimes/{}/workers/{}",
            path_segment_encode(&self.workspace_id),
            path_segment_encode(&self.runtime_id),
            path_segment_encode(&self.worker_id),
        );
        let upload_path = format!(
            "{worker_path}/attachment-uploads/{}",
            path_segment_encode(&grant.upload_id),
        );
        let response = api
            .request(HttpMethod::PUT, &upload_path)?
            .body(content)
            .send()
            .await
            .map_err(BackendRuntimeClientError::Http)?;
        api.check_status(response.status())?;
        response
            .json::<UploadedFileResponse>()
            .await
            .map(|response| response.file)
            .map_err(BackendRuntimeClientError::Http)
    }

    pub async fn cancel_file_upload(
        &self,
        upload_id: &str,
    ) -> Result<(), BackendRuntimeClientError> {
        let api = BackendApiClient::from_stored_token(&self.base_url)?;
        runtime_server_api_client(&self.base_url, &api)?
            .runtime_worker_attachment_upload_cancel(
                self.workspace_id.clone(),
                self.runtime_id.clone(),
                self.worker_id.clone(),
                upload_id.to_string(),
            )
            .await?;
        Ok(())
    }

    pub async fn delete_uploaded_file(
        &self,
        artifact_id: &str,
    ) -> Result<(), BackendRuntimeClientError> {
        let api = BackendApiClient::from_stored_token(&self.base_url)?;
        runtime_server_api_client(&self.base_url, &api)?
            .runtime_worker_attachment_delete(
                self.workspace_id.clone(),
                self.runtime_id.clone(),
                self.worker_id.clone(),
                artifact_id.to_string(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct UploadedFileResponse {
    file: protocol::UploadedFileRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendWorkerLaunchTarget {
    pub base_url: String,
    pub workspace_id: Option<String>,
}

impl BackendWorkerLaunchTarget {
    pub fn new(base_url: impl Into<String>, workspace_id: Option<String>) -> Self {
        Self {
            base_url: base_url.into(),
            workspace_id,
        }
    }

    pub fn select_workspace(&mut self, workspace_id: impl Into<String>) {
        self.workspace_id = Some(workspace_id.into());
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
                "workspace_id is required before creating a Backend worker".to_string(),
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
    SessionApi(server_api::ServerApiClientError),
    ContractApi(server_api::client_support::ClientError<server_api::RepositoryApiError>),
    Http(reqwest::Error),
    Protocol(String),
}

impl fmt::Display for BackendRuntimeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Api(error) => write!(f, "{error}"),
            Self::SessionApi(error) => write!(f, "{error}"),
            Self::ContractApi(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BackendRuntimeClientError {}

impl From<server_api::ServerApiClientError> for BackendRuntimeClientError {
    fn from(value: server_api::ServerApiClientError) -> Self {
        Self::SessionApi(value)
    }
}

impl From<server_api::client_support::ClientError<server_api::RepositoryApiError>>
    for BackendRuntimeClientError
{
    fn from(
        value: server_api::client_support::ClientError<server_api::RepositoryApiError>,
    ) -> Self {
        Self::ContractApi(value)
    }
}

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

const WORKER_SESSION_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

fn runtime_server_api_client(
    base_url: &str,
    backend: &BackendApiClient,
) -> Result<server_api::ServerApiClient<StoredBearerAuthorizer>, BackendRuntimeClientError> {
    server_api::ServerApiClient::builder(base_url)
        .map_err(|error| BackendRuntimeClientError::Protocol(error.to_string()))?
        .client(backend.asynchronous_client())
        .authorizer(StoredBearerAuthorizer {
            authorization: backend.authorization_header_value(),
        })
        .response_body_limit(WORKER_SESSION_RESPONSE_LIMIT)
        .build()
        .map_err(|error| BackendRuntimeClientError::Protocol(error.to_string()))
}

fn server_api_error_is_auth(
    error: &server_api::client_support::ClientError<server_api::RepositoryApiError>,
) -> bool {
    matches!(
        error,
        server_api::client_support::ClientError::Public { status, .. }
            if matches!(
                *status,
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            )
    )
}

#[derive(Clone, Debug)]
struct StoredBearerAuthorizer {
    authorization: String,
}

impl server_api::client_support::RequestAuthorizer for StoredBearerAuthorizer {
    fn authorize(
        &self,
        _request: server_api::client_support::AuthorizerRequest<'_>,
    ) -> Result<reqwest::header::HeaderMap, server_api::client_support::AuthorizationError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            self.authorization
                .parse()
                .map_err(|_| server_api::client_support::AuthorizationError::new())?,
        );
        Ok(headers)
    }
}

pub async fn observe_backend_worker_session(
    target: &BackendRuntimeTarget,
) -> Result<server_api::WorkspaceWorkerSessionResponse, BackendRuntimeClientError> {
    validate_target(target)?;
    let backend = BackendApiClient::from_stored_token(&target.base_url)?;
    observe_backend_worker_session_with_client(target, &backend).await
}

async fn observe_backend_worker_session_with_client(
    target: &BackendRuntimeTarget,
    backend: &BackendApiClient,
) -> Result<server_api::WorkspaceWorkerSessionResponse, BackendRuntimeClientError> {
    runtime_server_api_client(&target.base_url, backend)?
        .worker_session(
            target.workspace_id.clone(),
            target.runtime_id.clone(),
            target.worker_id.clone(),
        )
        .await
        .map_err(Into::into)
}

pub async fn get_backend_worker_launch_options(
    target: &BackendWorkerLaunchTarget,
) -> Result<BackendWorkerLaunchOptions, BackendRuntimeClientError> {
    validate_launch_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    get_backend_worker_launch_options_with_client(target, &api).await
}

async fn get_backend_worker_launch_options_with_client(
    target: &BackendWorkerLaunchTarget,
    api: &BackendApiClient,
) -> Result<BackendWorkerLaunchOptions, BackendRuntimeClientError> {
    let path = backend_workspace_workers_launch_options_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
    );
    let response = api.request(HttpMethod::GET, &path)?.send().await?;
    let response = api.require_success(response).await?;
    Ok(response.json::<BackendWorkerLaunchOptions>().await?)
}

pub async fn create_backend_worker(
    target: &BackendWorkerLaunchTarget,
    request: &BackendCreateWorkerRequest,
) -> Result<BackendCreateWorkerResponse, BackendRuntimeClientError> {
    validate_launch_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    create_backend_worker_with_client(target, request, &api).await
}

async fn create_backend_worker_with_client(
    target: &BackendWorkerLaunchTarget,
    request: &BackendCreateWorkerRequest,
    api: &BackendApiClient,
) -> Result<BackendCreateWorkerResponse, BackendRuntimeClientError> {
    let path = backend_workspace_workers_path(
        target
            .workspace_id
            .as_deref()
            .expect("validated Backend Workspace scope"),
    );
    let response = api
        .request(HttpMethod::POST, &path)?
        .json(request)
        .send()
        .await?;
    let response = api.require_success(response).await?;
    Ok(response.json::<BackendCreateWorkerResponse>().await?)
}

pub async fn list_backend_workers(
    target: &BackendRuntimeListTarget,
) -> Result<BackendRuntimeListResponse<BackendWorkerSummary>, BackendRuntimeClientError> {
    validate_list_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    let client = runtime_server_api_client(&target.base_url, &api)?;
    let workspace_id = target
        .workspace_id
        .as_deref()
        .expect("validated Backend Workspace scope");
    if let Some(runtime_id) = target.runtime_id.as_deref() {
        return client
            .runtime_worker_list(
                workspace_id.to_string(),
                runtime_id.to_string(),
                server_api::RuntimeWorkersQuery::default(),
            )
            .await
            .map(runtime_worker_list_response)
            .map_err(Into::into);
    }

    let runtimes = client.runtime_list(workspace_id.to_string()).await?;
    let mut items = Vec::new();
    let mut diagnostics = runtimes.diagnostics;
    for resource in runtimes.items {
        let runtime = resource.runtime;
        match client
            .runtime_worker_list(
                workspace_id.to_string(),
                runtime.runtime_id.clone(),
                server_api::RuntimeWorkersQuery::default(),
            )
            .await
        {
            Ok(response) => {
                diagnostics.extend(response.diagnostics);
                items.extend(response.items);
            }
            Err(error) if server_api_error_is_auth(&error) => return Err(error.into()),
            Err(error) => diagnostics.push(BackendDiagnostic {
                code: "runtime_worker_list_failed".to_string(),
                severity: BackendDiagnosticSeverity::Error,
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
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    runtime_server_api_client(&target.base_url, &api)?
        .runtime_worker_list(
            target
                .workspace_id
                .as_deref()
                .expect("validated Backend Workspace scope")
                .to_string(),
            runtime_id.to_string(),
            server_api::RuntimeWorkersQuery {
                status: Some(server_api::RuntimeWorkersStatusFilter::Stopped),
            },
        )
        .await
        .map(runtime_worker_list_response)
        .map_err(Into::into)
}

pub async fn restore_backend_worker(
    target: &BackendRuntimeTarget,
) -> Result<BackendWorkerRestoreResponse, BackendRuntimeClientError> {
    validate_target(target)?;
    let api = BackendApiClient::from_stored_token(&target.base_url)?;
    runtime_server_api_client(&target.base_url, &api)?
        .runtime_worker_restore(
            target.workspace_id.clone(),
            target.runtime_id.clone(),
            target.worker_id.clone(),
            server_api::RestoreTicketAssignmentQuery {
                ticket_id: None,
                assignment_operation_id: None,
            },
        )
        .await
        .map_err(Into::into)
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

fn runtime_worker_list_response(
    response: server_api::RuntimeWorkerListResponse,
) -> BackendRuntimeListResponse<BackendWorkerSummary> {
    BackendRuntimeListResponse {
        workspace_id: response.workspace_id,
        limit: response.limit,
        items: response.items,
        source: response.source,
        diagnostics: response.diagnostics,
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

fn validate_launch_target(
    target: &BackendWorkerLaunchTarget,
) -> Result<(), BackendRuntimeClientError> {
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
    match target.workspace_id.as_deref() {
        Some("") => Err(BackendRuntimeClientError::InvalidTarget(
            "workspace_id must not be empty".to_string(),
        )),
        None => Err(BackendRuntimeClientError::InvalidTarget(
            "workspace selection is required before creating a Backend worker".to_string(),
        )),
        Some(_) => Ok(()),
    }
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

fn backend_workspace_workers_path(workspace_id: &str) -> String {
    format!("/api/w/{}/workers", path_segment_encode(workspace_id))
}

fn backend_workspace_workers_launch_options_path(workspace_id: &str) -> String {
    format!(
        "{}/launch-options",
        backend_workspace_workers_path(workspace_id)
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_json_once(body: serde_json::Value) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut buffer = [0_u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client closed before sending HTTP headers");
                request.extend_from_slice(&buffer[..read]);
                if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let mut buffer = [0_u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client closed before sending HTTP body");
                request.extend_from_slice(&buffer[..read]);
            }

            let body = serde_json::to_vec(&body).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (base_url, task)
    }

    #[tokio::test]
    async fn worker_session_request_uses_bearer_and_sixteen_mib_response_limit() {
        let large_message = "x".repeat(2 * 1024 * 1024);
        let (base_url, server) = serve_json_once(serde_json::json!({
            "subject": {
                "kind": "runtime_worker",
                "runtime_id": "runtime-a",
                "worker_id": "worker-a"
            },
            "availability": "unavailable",
            "reason": "storage_unavailable",
            "message": large_message
        }))
        .await;
        let target = BackendRuntimeTarget::new(&base_url, "workspace-a", "runtime-a", "worker-a");
        let api =
            BackendApiClient::from_access_token_for_test(&base_url, "session-secret").unwrap();

        let response = observe_backend_worker_session_with_client(&target, &api)
            .await
            .unwrap();
        assert!(matches!(
            response.observation,
            runtime_api::WorkerSessionAvailability::Unavailable { ref message, .. }
                if message.len() == 2 * 1024 * 1024
        ));
        let request = server.await.unwrap();
        assert!(request.starts_with(
            "GET /api/w/workspace-a/runtimes/runtime-a/workers/worker-a/session HTTP/1.1\r\n"
        ));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer session-secret\r\n")
        );
    }

    #[tokio::test]
    async fn launch_options_request_uses_workspace_path_and_bearer_auth() {
        let (base_url, server) = serve_json_once(serde_json::json!({
            "workspace_id": "team main",
            "runtimes": [{
                "runtime_id": "embedded",
                "display_name": "Embedded",
                "built_in": true,
                "worker_creation_available": true,
                "working_directory_required": false,
                "status": "online",
                "diagnostics": []
            }],
            "default_profile": "builtin:default",
            "profiles": [{
                "id": "builtin:default",
                "label": "Default",
                "description": ""
            }],
            "repositories": [],
            "working_directories": [],
            "diagnostics": []
        }))
        .await;
        let target = BackendWorkerLaunchTarget::new(&base_url, Some("team main".to_string()));
        let api = BackendApiClient::from_access_token_for_test(&base_url, "launch-secret").unwrap();

        let response = get_backend_worker_launch_options_with_client(&target, &api)
            .await
            .unwrap();
        assert_eq!(response.runtimes[0].runtime_id, "embedded");
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /api/w/team%20main/workers/launch-options HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer launch-secret\r\n")
        );
    }

    #[tokio::test]
    async fn create_worker_posts_frontend_contract_to_workspace_path() {
        let (base_url, server) = serve_json_once(serde_json::json!({
            "workspace_id": "workspace-1",
            "runtime_id": "embedded",
            "worker_id": "worker-1",
            "console_href": "/w/workspace-1/workers/embedded/worker-1",
            "worker": {
                "runtime_id": "embedded",
                "worker_id": "worker-1",
                "host_id": "host-1",
                "display_name": "Coder one",
                "label": "Coder one",
                "profile": "builtin:coder",
                "singleton_key": null,
                "tags": [],
                "workspace": {
                    "visibility": "workspace",
                    "identity": "workspace",
                    "workspace_id": "workspace-1"
                },
                "state": "idle",
                "last_seen_at": null,
                "pinned": false,
                "retention_state": "resident",
                "implementation": {"kind": "embedded", "display_hint": "Embedded"},
                "capabilities": {"can_stop": true, "can_spawn_followup": false},
                "diagnostics": []
            },
            "diagnostics": []
        }))
        .await;
        let target = BackendWorkerLaunchTarget::new(&base_url, Some("workspace-1".to_string()));
        let api = BackendApiClient::from_access_token_for_test(&base_url, "create-secret").unwrap();
        let create = BackendCreateWorkerRequest {
            runtime_id: "embedded".to_string(),
            display_name: "Coder one".to_string(),
            profile: Some("builtin:coder".to_string()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            control_operation_id: None,
        };

        let response = create_backend_worker_with_client(&target, &create, &api)
            .await
            .unwrap();
        assert_eq!(response.worker_id, "worker-1");
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /api/w/workspace-1/workers HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer create-secret\r\n")
        );
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["runtime_id"], "embedded");
        assert_eq!(body["display_name"], "Coder one");
        assert_eq!(body["profile"], "builtin:coder");
        assert_eq!(body["initial_submit"], serde_json::json!([]));
        assert_eq!(body["workdir_attachments"], serde_json::json!([]));
    }

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
            "workdir_attachments": [{
                "alias": "checkout",
                "working_directory": {
                    "working_directory_id": "wd-1",
                    "source": {"kind": "repository", "repository_key": "main"},
                    "materializer_kind": "runtime_git_clone",
                    "status": "active",
                    "occupied_by": {
                        "runtime_id": "arcadia",
                        "worker_id": "worker-opaque-64",
                        "display_name": "Coder",
                        "linked_at": "2026-08-12T00:00:00Z"
                    }
                }
            }]
        });

        let worker: BackendWorkerSummary = serde_json::from_value(payload.clone()).unwrap();
        let attachment = worker.workdir_attachments.into_iter().next().unwrap();
        assert_eq!(attachment.alias, "checkout");
        let workdir = attachment.working_directory;
        assert_eq!(
            workdir.source,
            BackendWorkingDirectorySource::Repository {
                repository_key: "main".to_string(),
            }
        );
        let occupied_by = workdir.occupied_by.expect("occupied Workdir");
        assert_eq!(occupied_by.runtime_id, "arcadia");
        assert_eq!(occupied_by.worker_id, "worker-opaque-64");

        let mut stale = payload;
        stale["workdir_attachments"][0]["working_directory"]["occupied_by"]["runtime_worker_id"] =
            serde_json::json!(64);
        assert!(serde_json::from_value::<BackendWorkerSummary>(stale).is_err());
    }
}
