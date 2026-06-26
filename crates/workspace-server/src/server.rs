use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::hosts::{
    DiagnosticSeverity, EmbeddedWorkerRuntime, HostSummary, LocalWorkerRuntime,
    RemoteRuntimeConfig, RemoteWorkerRuntime, RuntimeDiagnostic, RuntimeRegistry, RuntimeSummary,
    WorkerInputRequest, WorkerInputResult, WorkerLifecycleRequest, WorkerLifecycleResult,
    WorkerSpawnRequest, WorkerSpawnResult, WorkerSummary, WorkerTranscriptProjection,
};
use crate::identity::WorkspaceIdentity;
use crate::observation::{
    BackendObservationProxy, ClientWorkerEventWsFrame, ClientWorkerEventsWsQuery,
    ObservationProxyError, RuntimeObservationSourceConfig, RuntimeWsObservationClient,
};
use crate::records::{
    LocalProjectRecordReader, ObjectiveDetail, ProjectRecordList, TicketDetail, TicketSummary,
};
use crate::repositories::{LocalRepositoryReader, RepositoryLogRead, RepositorySummary};
use crate::store::{ControlPlaneStore, WorkspaceRecord};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthConfig {
    /// Local/dev-only mode. If a token is configured by a future entrypoint, it
    /// is a development guard only and not a production SaaS auth model.
    LocalDevToken { token_configured: bool },
}

#[derive(Clone)]
pub struct ServerConfig {
    pub workspace_id: String,
    pub workspace_display_name: String,
    pub workspace_created_at: String,
    pub workspace_root: PathBuf,
    pub static_assets_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    pub max_records: usize,
    pub local_runtime_data_dir: Option<PathBuf>,
    pub runtime_event_sources: Vec<RuntimeObservationSourceConfig>,
    pub remote_runtime_sources: Vec<RemoteRuntimeConfig>,
}

impl ServerConfig {
    pub fn local_dev(workspace_root: impl Into<PathBuf>, identity: WorkspaceIdentity) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            workspace_id: identity.workspace_id,
            workspace_display_name: identity.display_name,
            workspace_created_at: identity.created_at,
            workspace_root,
            static_assets_dir: None,
            auth: AuthConfig::LocalDevToken {
                token_configured: false,
            },
            max_records: 200,
            local_runtime_data_dir: manifest::paths::data_dir(),
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct WorkspaceApi {
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    records: LocalProjectRecordReader,
    runtime: Arc<RuntimeRegistry>,
    observation_proxy: BackendObservationProxy,
}

impl WorkspaceApi {
    pub async fn new(config: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Result<Self> {
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: config.workspace_id.clone(),
                display_name: config.workspace_display_name.clone(),
                state: "active".to_string(),
                created_at: config.workspace_created_at.clone(),
                updated_at: config.workspace_created_at.clone(),
            })
            .await?;
        let mut runtime = RuntimeRegistry::for_workspace(
            LocalWorkerRuntime::new(
                config.workspace_id.clone(),
                config.workspace_root.clone(),
                config.local_runtime_data_dir.clone(),
            ),
            EmbeddedWorkerRuntime::new_memory(config.workspace_id.clone()),
        );
        for remote_config in config.remote_runtime_sources.iter().cloned() {
            runtime
                .register(RemoteWorkerRuntime::new(remote_config).map_err(|err| err.into_error())?);
        }
        let runtime = Arc::new(runtime);
        let observation_proxy = BackendObservationProxy::new(config.runtime_event_sources.clone());
        Ok(Self {
            records: LocalProjectRecordReader::new(config.workspace_root.clone()),
            config,
            store,
            runtime,
            observation_proxy,
        })
    }

    pub fn workspace_id(&self) -> &str {
        self.config.workspace_id.as_str()
    }

    fn local_repository_reader(&self) -> LocalRepositoryReader {
        LocalRepositoryReader::new(
            self.config.workspace_root.clone(),
            self.config.workspace_id.clone(),
        )
    }

    fn local_repository_id(&self) -> String {
        LocalRepositoryReader::repository_id_for_workspace(self.workspace_id())
    }

    fn workspace_display_name(&self) -> &str {
        self.config.workspace_display_name.as_str()
    }
}

pub fn build_router(api: WorkspaceApi) -> Router {
    Router::new()
        .route("/api/workspace", get(get_workspace))
        .route("/api/tickets", get(list_tickets))
        .route("/api/tickets/{id}", get(get_ticket))
        .route("/api/objectives", get(list_objectives))
        .route("/api/objectives/{id}", get(get_objective))
        .route("/api/repositories", get(list_repositories))
        .route("/api/repositories/{repository_id}", get(repository_detail))
        .route("/api/repositories/{repository_id}/log", get(repository_log))
        .route(
            "/api/repositories/{repository_id}/tickets",
            get(repository_tickets),
        )
        .route("/api/hosts", get(list_hosts))
        .route("/api/runtimes", get(list_runtimes))
        .route("/api/workers", get(list_workers))
        .route(
            "/api/runtimes/{runtime_id}/workers",
            post(create_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}",
            get(get_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/input",
            post(send_runtime_worker_input),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/stop",
            post(stop_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/cancel",
            post(cancel_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/transcript",
            get(get_runtime_worker_transcript),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/events/ws",
            get(worker_observation_ws),
        )
        .route("/api/hosts/{host_id}/workers", get(list_host_workers))
        .fallback(get(static_or_spa_fallback))
        .with_state(api)
}

pub async fn serve(
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    listener: TcpListener,
) -> Result<()> {
    let api = WorkspaceApi::new(config, store).await?;
    axum::serve(listener, build_router(api)).await?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub local_root: PathBuf,
    pub record_authority: String,
    pub schema_version: i64,
    pub auth: AuthConfig,
    pub extension_points: ExtensionPoints,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionPoints {
    pub store: String,
    pub event_stream: ExtensionPointState,
    pub host_worker_bridge: ExtensionPointState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionPointState {
    pub status: String,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub invalid_records: Vec<crate::records::InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryListResponse {
    pub workspace_id: String,
    pub items: Vec<RepositorySummary>,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryDetailResponse {
    pub workspace_id: String,
    pub item: RepositorySummary,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryLogResponse {
    pub workspace_id: String,
    pub repository_id: String,
    pub limit: usize,
    pub items: Vec<crate::repositories::GitCommitSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryTicketsResponse {
    pub workspace_id: String,
    pub repository_id: String,
    pub limit: usize,
    pub columns: Vec<TicketKanbanColumn>,
    pub invalid_records: Vec<crate::records::InvalidProjectRecord>,
    pub record_authority: String,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TicketKanbanColumn {
    pub state: String,
    pub items: Vec<TicketSummary>,
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TicketKanbanQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptQuery {
    start: Option<usize>,
    limit: Option<usize>,
}

async fn get_workspace(State(api): State<WorkspaceApi>) -> ApiResult<Json<WorkspaceResponse>> {
    let schema_version = api.store.schema_version().await?;
    let stored = api.store.get_workspace(api.workspace_id()).await?;
    let display_name = stored
        .as_ref()
        .map(|record| record.display_name.clone())
        .unwrap_or_else(|| api.config.workspace_display_name.clone());
    Ok(Json(WorkspaceResponse {
        workspace_id: api.config.workspace_id.clone(),
        display_name,
        local_root: api.config.workspace_root.clone(),
        record_authority: "local_yoi_project_records".to_string(),
        schema_version,
        auth: api.config.auth.clone(),
        extension_points: ExtensionPoints {
            store: "sqlite".to_string(),
            event_stream: ExtensionPointState {
                status: "reserved".to_string(),
                note: "No browser-to-Worker socket path is exposed in this bootstrap; any future stream must be a Workspace server proxy that resolves Worker identity and enforces method allow/block boundaries.".to_string(),
            },
            host_worker_bridge: ExtensionPointState {
                status: "read_only_local".to_string(),
                note: "Local Hosts and Workers are exposed as a read-only bridge over existing Worker metadata; no direct Worker socket, scheduling, or lifecycle control is implemented.".to_string(),
            },
        },
    }))
}

async fn list_tickets(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<ListResponse<crate::records::TicketSummary>>> {
    let limit = api.config.max_records.min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_tickets(limit)?;
    Ok(Json(ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        invalid_records,
        record_authority,
    }))
}

async fn get_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<TicketDetail>> {
    Ok(Json(api.records.ticket(&id)?))
}

async fn list_objectives(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<ListResponse<crate::records::ObjectiveSummary>>> {
    let limit = api.config.max_records.min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_objectives(limit)?;
    Ok(Json(ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        invalid_records,
        record_authority,
    }))
}

async fn get_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<ObjectiveDetail>> {
    Ok(Json(api.records.objective(&id)?))
}

async fn list_repositories(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RepositoryListResponse>> {
    let reader = api.local_repository_reader();
    let items = reader.list(api.workspace_display_name());
    Ok(Json(RepositoryListResponse {
        workspace_id: api.config.workspace_id,
        items,
        source: "local_workspace_root".to_string(),
        diagnostics: Vec::new(),
    }))
}

async fn repository_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
) -> ApiResult<Json<RepositoryDetailResponse>> {
    let _canonical_repository_id = ensure_local_repository(&api, &repository_id)?;
    let reader = api.local_repository_reader();
    Ok(Json(RepositoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        item: reader.summary(api.workspace_display_name()),
        source: "local_workspace_root".to_string(),
    }))
}

async fn repository_log(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<RepositoryLogResponse>> {
    let canonical_repository_id = ensure_local_repository(&api, &repository_id)?;
    let RepositoryLogRead {
        limit,
        items,
        diagnostics,
    } = api.local_repository_reader().recent_log(query.limit);
    Ok(Json(RepositoryLogResponse {
        workspace_id: api.config.workspace_id,
        repository_id: canonical_repository_id,
        limit,
        items,
        diagnostics,
    }))
}

async fn repository_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
    Query(query): Query<TicketKanbanQuery>,
) -> ApiResult<Json<RepositoryTicketsResponse>> {
    let canonical_repository_id = ensure_local_repository(&api, &repository_id)?;
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_tickets(limit)?;
    Ok(Json(RepositoryTicketsResponse {
        workspace_id: api.config.workspace_id,
        repository_id: canonical_repository_id,
        limit,
        columns: ticket_kanban_columns(items),
        invalid_records,
        record_authority,
        source: "workspace_local_ticket_fallback".to_string(),
        diagnostics: vec![RuntimeDiagnostic {
            code: "repository_ticket_target_metadata_absent".to_string(),
            severity: DiagnosticSeverity::Info,
            message: "Ticket target Repository metadata is not available yet; Kanban groups all workspace-local Tickets by state as a read-only fallback.".to_string(),
        }],
    }))
}

async fn list_hosts(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<HostSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtime_hosts = api.runtime.list_hosts(limit);
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtime_hosts.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_hosts.diagnostics,
    }))
}

async fn list_runtimes(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<RuntimeSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtimes = api.runtime.list_runtimes(limit);
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtimes.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtimes.diagnostics,
    }))
}

async fn list_workers(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    workers_response(api).map(Json)
}

async fn get_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<WorkerSummary>> {
    let worker = api
        .runtime
        .worker(&runtime_id, &worker_id)
        .map_err(|err| err.into_error())?;
    Ok(Json(worker))
}

async fn create_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
    Json(request): Json<WorkerSpawnRequest>,
) -> ApiResult<Json<WorkerSpawnResult>> {
    let result = api
        .runtime
        .spawn_worker(&runtime_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn send_runtime_worker_input(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerInputRequest>,
) -> ApiResult<Json<WorkerInputResult>> {
    let result = api
        .runtime
        .send_input(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn stop_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let result = api
        .runtime
        .stop_worker(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn cancel_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let result = api
        .runtime
        .cancel_worker(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn get_runtime_worker_transcript(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Query(query): Query<TranscriptQuery>,
) -> ApiResult<Json<WorkerTranscriptProjection>> {
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let start = query.start.unwrap_or(0);
    let result = api
        .runtime
        .transcript(&runtime_id, &worker_id, start, limit)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn worker_observation_ws(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Query(query): Query<ClientWorkerEventsWsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    match api.observation_proxy.source(&runtime_id, &worker_id) {
        Ok(source) => ws.on_upgrade(move |socket| {
            worker_observation_ws_session(api.observation_proxy, source, query, socket)
        }),
        Err(ObservationProxyError::WorkerNotFound(_)) => {
            match api.runtime.observation_source(&runtime_id, &worker_id) {
                Ok(source) => ws.on_upgrade(move |socket| {
                    worker_observation_ws_session(api.observation_proxy, source, query, socket)
                }),
                Err(error) => ApiError(error.into_error()).into_response(),
            }
        }
        Err(error) => {
            let status = StatusCode::BAD_REQUEST;
            (
                status,
                Json(serde_json::json!({
                    "error": error.code(),
                    "message": error.message(),
                })),
            )
                .into_response()
        }
    }
}

async fn worker_observation_ws_session(
    proxy: BackendObservationProxy,
    source: RuntimeObservationSourceConfig,
    query: ClientWorkerEventsWsQuery,
    mut socket: WebSocket,
) {
    let open = match proxy.open(
        &source.runtime_id,
        &source.worker_id,
        query.cursor.as_deref(),
    ) {
        Ok(open) => open,
        Err(error) => {
            let _ = send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::diagnostic(error))
                .await;
            return;
        }
    };

    let mut backend_cursor = open.backend_cursor;
    for envelope in open.replay {
        backend_cursor = crate::observation::BackendObservationCursor::decode(&envelope.cursor)
            .unwrap_or(backend_cursor);
        if !send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::event(envelope)).await {
            return;
        }
    }

    let mut upstream =
        match RuntimeWsObservationClient::connect(&source, open.runtime_cursor.as_deref()).await {
            Ok(client) => client,
            Err(error) => {
                let _ =
                    send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::diagnostic(error))
                        .await;
                return;
            }
        };

    loop {
        tokio::select! {
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(WsMessage::Close(_))) | None => return,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if socket.send(WsMessage::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(WsMessage::Pong(_))) => {}
                    Some(Ok(_)) => {
                        let _ = send_client_ws_frame(
                            &mut socket,
                            ClientWorkerEventWsFrame::diagnostic(ObservationProxyError::ObservationOnly),
                        ).await;
                        return;
                    }
                    Some(Err(error)) => {
                        let _ = send_client_ws_frame(
                            &mut socket,
                            ClientWorkerEventWsFrame::diagnostic(
                                ObservationProxyError::MalformedFrame(format!(
                                    "client WebSocket receive error: {error}"
                                )),
                            ),
                        ).await;
                        return;
                    }
                }
            }
            upstream_event = upstream.next_event() => {
                match upstream_event {
                    Ok(event) => match proxy.store(event) {
                        Ok(envelope) => {
                            backend_cursor = crate::observation::BackendObservationCursor::decode(&envelope.cursor)
                                .unwrap_or(backend_cursor);
                            if !send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::event(envelope)).await {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::diagnostic(error)).await;
                            return;
                        }
                    },
                    Err(error) => {
                        let _ = send_client_ws_frame(&mut socket, ClientWorkerEventWsFrame::diagnostic(error)).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn send_client_ws_frame(socket: &mut WebSocket, frame: ClientWorkerEventWsFrame) -> bool {
    match serde_json::to_string(&frame) {
        Ok(text) => socket.send(WsMessage::Text(text.into())).await.is_ok(),
        Err(error) => {
            let fallback =
                ClientWorkerEventWsFrame::diagnostic(ObservationProxyError::MalformedFrame(
                    format!("failed to serialize backend observation frame: {error}"),
                ));
            let Ok(text) = serde_json::to_string(&fallback) else {
                return false;
            };
            socket.send(WsMessage::Text(text.into())).await.is_ok()
        }
    }
}

async fn list_host_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(host_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api
        .runtime
        .list_workers_for_host(&host_id, limit)
        .map_err(|err| err.into_error())?;
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtime_workers.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_workers.diagnostics,
    }))
}

fn workers_response(api: WorkspaceApi) -> ApiResult<RuntimeListResponse<WorkerSummary>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api.runtime.list_workers(limit);
    Ok(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtime_workers.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_workers.diagnostics,
    })
}

fn ensure_local_repository(api: &WorkspaceApi, repository_id: &str) -> Result<String> {
    let canonical_repository_id = api.local_repository_id();
    if LocalRepositoryReader::is_local_repository_id(repository_id, api.workspace_id()) {
        Ok(canonical_repository_id)
    } else {
        Err(Error::UnknownRepository(repository_id.to_string()))
    }
}

fn ticket_kanban_columns(items: Vec<TicketSummary>) -> Vec<TicketKanbanColumn> {
    let mut columns = vec![
        TicketKanbanColumn {
            state: "planning".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "ready".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "queued".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "inprogress".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "done".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "closed".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "other".to_string(),
            items: Vec::new(),
        },
    ];
    for item in items {
        let index = match item.state.as_str() {
            "planning" => 0,
            "ready" => 1,
            "queued" => 2,
            "inprogress" => 3,
            "done" => 4,
            "closed" => 5,
            _ => 6,
        };
        columns[index].items.push(item);
    }
    columns
}

async fn static_or_spa_fallback(State(api): State<WorkspaceApi>, uri: Uri) -> Response {
    if uri.path().starts_with("/api/") || uri.path() == "/api" {
        return (
            StatusCode::NOT_FOUND,
            [(CONTENT_TYPE, "application/json")],
            Json(serde_json::json!({
                "error": "not_found",
                "message": "unknown api route"
            }))
            .to_string(),
        )
            .into_response();
    }

    let Some(static_root) = api.config.static_assets_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match read_static_or_index(static_root, uri.path()).await {
        Ok(StaticAsset {
            bytes,
            content_type,
        }) => (StatusCode::OK, [(CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(error) => {
            tracing::debug!(%error, path = %uri.path(), "failed to serve static asset");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

struct StaticAsset {
    bytes: Vec<u8>,
    content_type: &'static str,
}

async fn read_static_or_index(root: &Path, request_path: &str) -> Result<StaticAsset> {
    let candidate = safe_static_candidate(root, request_path)?;
    let file = if tokio::fs::metadata(&candidate)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        candidate
    } else {
        root.join("index.html")
    };
    let content_type = content_type_for(&file);
    let bytes = tokio::fs::read(file).await?;
    Ok(StaticAsset {
        bytes,
        content_type,
    })
}

fn safe_static_candidate(root: &Path, request_path: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    let clean = request_path.trim_start_matches('/');
    if clean.is_empty() {
        path.push("index.html");
        return Ok(path);
    }
    for component in Path::new(clean).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err(Error::Store("static path escape rejected".to_string())),
        }
    }
    Ok(path)
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "html" | "" => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

struct ApiError(Error);

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::InvalidRuntimeIdentifier { .. } => StatusCode::BAD_REQUEST,
            Error::InvalidRecordId(_)
            | Error::MissingFrontmatter(_)
            | Error::UnknownHost(_)
            | Error::UnknownRuntime(_)
            | Error::UnknownWorker { .. }
            | Error::UnknownRepository(_) => StatusCode::NOT_FOUND,
            Error::Ticket(_) => StatusCode::NOT_FOUND,
            Error::RuntimeCapabilityUnsupported { .. } => StatusCode::NOT_IMPLEMENTED,
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_auth_failed" => {
                StatusCode::UNAUTHORIZED
            }
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_timeout" => {
                StatusCode::GATEWAY_TIMEOUT
            }
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_unsupported" => {
                StatusCode::NOT_IMPLEMENTED
            }
            Error::RuntimeOperationFailed { .. } => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            [(CONTENT_TYPE, "application/json")],
            Json(serde_json::json!({
                "error": status.canonical_reason().unwrap_or("error"),
                "message": self.0.to_string(),
            }))
            .to_string(),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;

    use crate::observation::ClientWorkerEventWsDiagnostic;
    use crate::store::SqliteWorkspaceStore;

    const TEST_WORKSPACE_ID: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_REPOSITORY_ID: &str = "local-0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_CREATED_AT: &str = "2026-06-23T06:43:28Z";

    fn test_identity() -> WorkspaceIdentity {
        WorkspaceIdentity {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            display_name: "Test Workspace".to_string(),
            created_at: TEST_CREATED_AT.to_string(),
        }
    }

    #[tokio::test]
    async fn serves_bounded_read_apis_and_static_spa_separately() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "API Ticket", "ready");
        write_objective(dir.path(), "00000000001J3", "API Objective", "active");
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(static_dir.join("assets")).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>Yoi Workspace</main>").unwrap();
        std::fs::write(static_dir.join("assets/app.js"), "console.log('yoi');").unwrap();

        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = ServerConfig::local_dev(dir.path(), test_identity());
        config.static_assets_dir = Some(static_dir);
        config.local_runtime_data_dir = Some(dir.path().join("data"));
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        assert_eq!(workspace["workspace_id"], TEST_WORKSPACE_ID);
        assert_eq!(workspace["display_name"], "Test Workspace");
        assert_eq!(workspace["record_authority"], "local_yoi_project_records");
        assert_eq!(
            workspace["extension_points"]["host_worker_bridge"]["status"],
            "read_only_local"
        );

        let tickets = get_json(app.clone(), "/api/tickets").await;
        assert_eq!(tickets["items"][0]["id"], "00000000001J2");
        assert_eq!(tickets["items"][0]["state"], "ready");

        let objectives = get_json(app.clone(), "/api/objectives").await;
        assert_eq!(objectives["items"][0]["id"], "00000000001J3");
        assert_eq!(objectives["items"][0]["summary"], "Objective body.");

        let repositories = get_json(app.clone(), "/api/repositories").await;
        assert_eq!(repositories["items"][0]["id"], TEST_REPOSITORY_ID);
        assert_eq!(repositories["items"][0]["kind"], "local");

        let repository_detail = get_json(app.clone(), "/api/repositories/local").await;
        assert_eq!(repository_detail["item"]["id"], TEST_REPOSITORY_ID);

        let repository_log = get_json(app.clone(), "/api/repositories/local/log?limit=3").await;
        assert_eq!(repository_log["repository_id"], TEST_REPOSITORY_ID);
        assert_eq!(repository_log["limit"], 3);

        let repository_tickets = get_json(app.clone(), "/api/repositories/local/tickets").await;
        assert_eq!(repository_tickets["repository_id"], TEST_REPOSITORY_ID);
        let ready_column = repository_tickets["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|column| column["state"] == "ready")
            .unwrap();
        assert_eq!(ready_column["items"][0]["id"], "00000000001J2");
        assert_eq!(
            repository_tickets["diagnostics"][0]["code"],
            "repository_ticket_target_metadata_absent"
        );

        let unknown_repository_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/repositories/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown_repository_response.status(), StatusCode::NOT_FOUND);

        let hosts = get_json(app.clone(), "/api/hosts").await;
        assert_eq!(hosts["source"], "worker_runtime_registry");
        assert_eq!(hosts["items"][0]["runtime_id"], "local-worker-runtime");
        let host_id = hosts["items"][0]["host_id"].as_str().unwrap().to_string();
        assert!(host_id.starts_with("local-"));
        assert!(host_id.len() <= 120);
        assert_ne!(host_id, TEST_REPOSITORY_ID);
        assert_eq!(hosts["items"][0]["kind"], "local-worker-host");
        assert_eq!(
            hosts["items"][0]["capabilities"]["local_pod_inspection"],
            "available"
        );
        assert_eq!(
            hosts["items"][0]["capabilities"]["workspace_scope"],
            "current_workspace"
        );
        assert!(!hosts.to_string().contains("metadata.json"));

        let runtimes = get_json(app.clone(), "/api/runtimes").await;
        assert_eq!(runtimes["source"], "worker_runtime_registry");
        assert_eq!(runtimes["items"][0]["runtime_id"], "local-worker-runtime");
        assert_eq!(
            runtimes["items"][0]["source"]["kind"],
            "local_compatibility"
        );
        assert_eq!(
            runtimes["items"][0]["source"]["identity_authority"],
            "runtime_registry_projection"
        );
        assert!(!runtimes.to_string().contains("/workspace/demo"));
        assert_eq!(runtimes["items"][0]["host_ids"][0], host_id);

        let workers = get_json(app.clone(), "/api/workers").await;
        assert!(workers["items"].as_array().unwrap().is_empty());
        assert_eq!(
            workers["diagnostics"][0]["code"],
            "local_pod_registry_unreadable"
        );

        let host_workers = get_json(app.clone(), &format!("/api/hosts/{host_id}/workers")).await;
        assert!(host_workers["items"].as_array().unwrap().is_empty());

        let runs_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(runs_response.status(), StatusCode::NOT_FOUND);

        let runners_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runners")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(runners_response.status(), StatusCode::NOT_FOUND);

        let static_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::OK);
        assert_eq!(
            static_response.headers().get(CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );

        let spa_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tickets/00000000001J2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa_response.status(), StatusCode::OK);
        let bytes = to_bytes(spa_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("Yoi Workspace")
        );

        let api_miss = app
            .oneshot(
                Request::builder()
                    .uri("/api/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_miss.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(api_miss.into_body(), usize::MAX).await.unwrap();
        assert!(
            !String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("Yoi Workspace")
        );
    }

    #[tokio::test]
    async fn embedded_runtime_api_routes_by_runtime_and_worker_ids_without_leaking_internals() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = ServerConfig::local_dev(dir.path(), test_identity());
        config.local_runtime_data_dir = Some(dir.path().join("data"));
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let app = build_router(api);

        let runtimes = get_json(app.clone(), "/api/runtimes").await;
        let embedded_summary = runtimes["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|runtime| runtime["runtime_id"] == "embedded-worker-runtime")
            .expect("embedded runtime summary");
        assert_eq!(
            embedded_summary["source"]["kind"],
            "embedded_worker_runtime"
        );
        assert_eq!(embedded_summary["source"]["status"], "active");
        assert_eq!(
            embedded_summary["capabilities"]["workspace_scope"],
            "backend_internal"
        );
        assert_eq!(embedded_summary["capabilities"]["has_workspace_fs"], false);

        let spawned = post_json(
            app.clone(),
            "/api/runtimes/embedded-worker-runtime/workers",
            json!({
                "intent": {
                    "kind": "ticket_role",
                    "ticket_id": "00001KVZSGT0Q",
                    "role": "coder"
                },
                "requested_worker_name": "api-friendly-name",
                "acceptance": {
                    "kind": "run_accepted",
                    "expected_segments": 0
                }
            }),
        )
        .await;
        assert_eq!(spawned["state"], "accepted");
        let worker_id = spawned["worker"]["worker_id"].as_str().unwrap().to_string();
        assert_eq!(spawned["worker"]["runtime_id"], "embedded-worker-runtime");
        assert_eq!(
            spawned["worker"]["workspace"]["visibility"],
            "backend_internal"
        );
        assert_eq!(
            spawned["worker"]["implementation"]["kind"],
            "embedded_worker_runtime"
        );

        let worker = get_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}"),
        )
        .await;
        assert_eq!(worker["worker_id"], worker_id);
        assert_eq!(worker["runtime_id"], "embedded-worker-runtime");

        let accepted = post_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/input"),
            json!({
                "kind": "user",
                "content": "hello from browser-facing api"
            }),
        )
        .await;
        assert_eq!(accepted["state"], "accepted");
        assert_eq!(accepted["runtime_id"], "embedded-worker-runtime");
        assert_eq!(accepted["worker_id"], worker_id);
        assert_eq!(accepted["transcript_sequence"], 1);

        let transcript = get_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/transcript?start=0&limit=10"),
        )
        .await;
        assert_eq!(transcript["state"], "accepted");
        assert_eq!(transcript["items"][0]["role"], "user");
        assert_eq!(
            transcript["items"][0]["content"],
            "hello from browser-facing api"
        );

        let wrong_runtime = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/runtimes/local-worker-runtime/workers/{worker_id}/input"
                    ))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "kind": "user",
                            "content": "wrong runtime"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_runtime.status(), StatusCode::NOT_FOUND);

        let projected = format!(
            "{}{}{}{}{}",
            embedded_summary, spawned, worker, accepted, transcript
        );
        for forbidden in [
            dir.path().to_string_lossy().as_ref(),
            "metadata.json",
            "socket",
            "session",
            "token",
            "credential",
            "provider",
        ] {
            assert!(
                !projected.contains(forbidden),
                "embedded api projection leaked forbidden term: {forbidden}: {projected}"
            );
        }
    }

    #[tokio::test]
    async fn proxies_worker_observation_ws_with_backend_cursors_and_diagnostics() {
        let runtime = worker_runtime::Runtime::new_memory();
        let worker = runtime
            .create_worker(worker_runtime::catalog::CreateWorkerRequest::default())
            .unwrap();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                    .await
                    .unwrap()
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = ServerConfig::local_dev(dir.path(), test_identity());
        config.local_runtime_data_dir = Some(dir.path().join("data"));
        config
            .runtime_event_sources
            .push(RuntimeObservationSourceConfig {
                runtime_id: "runtime-a".into(),
                worker_id: "worker-a".into(),
                endpoint: format!(
                    "ws://{runtime_addr}/v1/workers/{}/events/ws",
                    worker.worker_ref.worker_id
                ),
                bearer_token: None,
            });
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let app_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app_addr = app_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(app_listener, build_router(api)).await.unwrap() });

        let url = format!("ws://{app_addr}/api/runtimes/runtime-a/workers/worker-a/events/ws");
        let (mut stream, _) = connect_async(&url).await.unwrap();
        let snapshot = next_client_frame(&mut stream).await;
        let ClientWorkerEventWsFrame::Event { envelope: snapshot } = snapshot else {
            panic!("expected snapshot event");
        };
        assert_eq!(snapshot.runtime_id, "runtime-a");
        assert_eq!(snapshot.worker_id, "worker-a");
        assert!(matches!(snapshot.payload, protocol::Event::Snapshot { .. }));

        runtime
            .observe_worker_event(
                &worker.worker_ref,
                protocol::Event::TextDelta {
                    text: "live".into(),
                },
            )
            .unwrap();
        let live = next_client_frame(&mut stream).await;
        let ClientWorkerEventWsFrame::Event { envelope: live } = live else {
            panic!("expected live event");
        };
        assert_eq!(live.runtime_id, "runtime-a");
        assert_eq!(live.worker_id, "worker-a");
        assert!(matches!(live.payload, protocol::Event::TextDelta { .. }));

        let (mut resumed, _) = connect_async(format!("{url}?cursor={}", live.cursor))
            .await
            .unwrap();
        let _snapshot = next_client_frame(&mut resumed).await;
        runtime
            .observe_worker_event(
                &worker.worker_ref,
                protocol::Event::TextDone {
                    text: "done".into(),
                },
            )
            .unwrap();
        let resumed_event = next_client_frame(&mut resumed).await;
        let ClientWorkerEventWsFrame::Event {
            envelope: resumed_event,
        } = resumed_event
        else {
            panic!("expected resumed live event");
        };
        assert_ne!(resumed_event.cursor, live.cursor);
        assert!(matches!(
            resumed_event.payload,
            protocol::Event::TextDone { .. }
        ));

        let (mut malformed, _) = connect_async(format!("{url}?cursor=bad")).await.unwrap();
        let diagnostic = next_client_frame(&mut malformed).await;
        let ClientWorkerEventWsFrame::Diagnostic { diagnostic } = diagnostic else {
            panic!("expected malformed cursor diagnostic");
        };
        assert_eq!(diagnostic.code, "backend.cursor_malformed");

        stream.send(Message::Text("{}".into())).await.unwrap();
        let mut saw_observation_only = false;
        for _ in 0..3 {
            if let ClientWorkerEventWsFrame::Diagnostic { diagnostic } =
                next_client_frame(&mut stream).await
            {
                assert_eq!(diagnostic.code, "backend.observation_only");
                saw_observation_only = true;
                break;
            }
        }
        assert!(saw_observation_only, "expected observation-only diagnostic");
    }

    #[tokio::test]
    async fn proxy_reports_unknown_backend_cursor_before_upstream_connect() {
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint: "ws://127.0.0.1:9/not-used".into(),
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(format!("{url}?cursor=bo_ffffffffffffffff"))
            .await
            .unwrap();
        let diagnostic = next_client_diagnostic(&mut stream).await;
        assert_eq!(diagnostic.code, "backend.cursor_unknown_or_expired");
    }

    #[tokio::test]
    async fn proxy_maps_runtime_cursor_diagnostic_to_typed_backend_diagnostic() {
        let (_runtime, _worker_ref, endpoint) = spawn_runtime_worker().await;
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint: format!("{endpoint}?cursor=wo_ffffffffffffffff"),
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(&url).await.unwrap();
        assert!(matches!(
            next_client_frame(&mut stream).await,
            ClientWorkerEventWsFrame::Event { envelope } if matches!(envelope.payload, protocol::Event::Snapshot { .. })
        ));
        let diagnostic = next_client_diagnostic(&mut stream).await;
        assert_eq!(diagnostic.code, "backend.cursor_unknown_or_expired");
    }

    #[tokio::test]
    async fn proxy_maps_runtime_worker_not_found_http_404_to_typed_backend_diagnostic() {
        let (_runtime, _worker_ref, endpoint) = spawn_runtime_worker().await;
        let endpoint = endpoint.replace("/events/ws", "/missing-worker/events/ws");
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint,
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(&url).await.unwrap();
        let diagnostic = next_client_diagnostic(&mut stream).await;
        assert_eq!(diagnostic.code, "backend.worker_not_found");
    }

    #[tokio::test]
    async fn proxy_reports_actual_upstream_disconnect_separately() {
        let endpoint = spawn_closing_runtime_ws().await;
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint,
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(&url).await.unwrap();
        let diagnostic = next_client_diagnostic(&mut stream).await;
        assert_eq!(diagnostic.code, "backend.upstream_disconnect");
    }

    async fn next_client_frame(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> ClientWorkerEventWsFrame {
        let message = stream.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn next_client_diagnostic(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> ClientWorkerEventWsDiagnostic {
        match next_client_frame(stream).await {
            ClientWorkerEventWsFrame::Diagnostic { diagnostic } => diagnostic,
            ClientWorkerEventWsFrame::Event { envelope } => {
                panic!("expected diagnostic, got event: {envelope:?}")
            }
        }
    }

    async fn spawn_runtime_worker() -> (
        worker_runtime::Runtime,
        worker_runtime::identity::WorkerRef,
        String,
    ) {
        let runtime = worker_runtime::Runtime::new_memory();
        let worker = runtime
            .create_worker(worker_runtime::catalog::CreateWorkerRequest::default())
            .unwrap();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                    .await
                    .unwrap()
            }
        });
        let endpoint = format!(
            "ws://{runtime_addr}/v1/workers/{}/events/ws",
            worker.worker_ref.worker_id
        );
        (runtime, worker.worker_ref, endpoint)
    }

    async fn spawn_workspace_proxy(
        source: RuntimeObservationSourceConfig,
    ) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = ServerConfig::local_dev(dir.path(), test_identity());
        config.local_runtime_data_dir = Some(dir.path().join("data"));
        let runtime_id = source.runtime_id.clone();
        let worker_id = source.worker_id.clone();
        config.runtime_event_sources.push(source);
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let app_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app_addr = app_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(app_listener, build_router(api)).await.unwrap() });
        (
            format!("ws://{app_addr}/api/runtimes/{runtime_id}/workers/{worker_id}/events/ws"),
            dir,
        )
    }

    async fn spawn_closing_runtime_ws() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = websocket.close(None).await;
        });
        format!("ws://{addr}/events/ws")
    }

    async fn get_json(app: Router, uri: &str) -> Value {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn post_json(app: Router, uri: &str, body: Value) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn write_ticket(root: &Path, id: &str, title: &str, state: &str) {
        let ticket_dir = root.join(".yoi/tickets").join(id);
        std::fs::create_dir_all(&ticket_dir).unwrap();
        std::fs::write(
            ticket_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
---

Ticket body.
"#,
            ),
        )
        .unwrap();
        std::fs::write(ticket_dir.join("thread.md"), "").unwrap();
    }

    fn write_objective(root: &Path, id: &str, title: &str, state: &str) {
        let objective_dir = root.join(".yoi/objectives").join(id);
        std::fs::create_dir_all(&objective_dir).unwrap();
        std::fs::write(
            objective_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
linked_tickets: ["00000000001J2"]
---

Objective body.
"#,
            ),
        )
        .unwrap();
    }
}
