use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use worker::runtime_adapter::WorkerRuntimeExecutionBackend;

use crate::companion::{
    CompanionCancelRequest, CompanionConsole, CompanionMessageRequest, CompanionMessageResponse,
    CompanionStatusResponse, CompanionTranscriptProjection,
};
use crate::config::{RemoteRuntimeConfigFile, WorkspaceBackendConfigFile};
use crate::hosts::{
    ConfigBundleCheckResult, ConfigBundleSyncResult, DiagnosticSeverity, EmbeddedWorkerRuntime,
    HostSummary, RemoteRuntimeConfig, RemoteWorkerRuntime, RuntimeDiagnostic, RuntimeRegistry,
    RuntimeSummary, WorkerInputRequest, WorkerInputResult, WorkerLifecycleRequest,
    WorkerLifecycleResult, WorkerOperationState, WorkerSpawnAcceptanceRequirement,
    WorkerSpawnIntent, WorkerSpawnRequest, WorkerSpawnResult, WorkerSummary,
    WorkerTranscriptProjection,
};
use crate::identity::WorkspaceIdentity;
use crate::observation::{
    BackendObservationProxy, ClientWorkerEventWsFrame, ClientWorkerEventsWsQuery,
    ObservationProxyError, RuntimeObservationClient, RuntimeObservationSourceConfig,
};
use crate::records::{
    LocalProjectRecordReader, ObjectiveDetail, ProjectRecordList, TicketDetail, TicketSummary,
};
use crate::repositories::{LocalRepositoryReader, RepositoryLogRead, RepositorySummary};
use crate::store::{ControlPlaneStore, WorkspaceRecord};
use crate::{Error, Result};
use worker_runtime::catalog::{ConfigBundleRef, ProfileSelector};
use worker_runtime::config_bundle::ConfigBundle;
use worker_runtime::http_server::RuntimeHttpSummaryResponse;
use worker_runtime::interaction::{
    WorkerInput as EmbeddedWorkerInput, WorkerInputKind as EmbeddedWorkerInputKind,
};

const EMBEDDED_WORKER_RUNTIME_ID: &str = "embedded-worker-runtime";

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
    pub frontend_url: String,
    pub embedded_runtime_store_root: PathBuf,
    pub static_assets_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    pub max_records: usize,
    pub runtime_event_sources: Vec<RuntimeObservationSourceConfig>,
    pub remote_runtime_sources: Vec<RemoteRuntimeConfig>,
}

impl ServerConfig {
    pub fn local_dev(workspace_root: impl Into<PathBuf>, identity: WorkspaceIdentity) -> Self {
        let workspace_root = workspace_root.into();
        let workspace_id = identity.workspace_id;
        let embedded_runtime_store_root = Self::default_embedded_runtime_store_root(&workspace_id);
        Self {
            workspace_id,
            workspace_display_name: identity.display_name,
            workspace_created_at: identity.created_at,
            workspace_root,
            frontend_url: "http://127.0.0.1:5173".to_string(),
            embedded_runtime_store_root,
            static_assets_dir: None,
            auth: AuthConfig::LocalDevToken {
                token_configured: false,
            },
            max_records: 200,
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
        }
    }

    pub fn workspace_backend_data_root_for_data_dir(
        data_dir: impl Into<PathBuf>,
        workspace_id: impl AsRef<str>,
    ) -> PathBuf {
        data_dir
            .into()
            .join("workspace-server")
            .join(workspace_id.as_ref())
    }

    pub fn default_workspace_backend_data_root(workspace_id: impl AsRef<str>) -> PathBuf {
        match manifest::paths::data_dir() {
            Some(data_dir) => {
                Self::workspace_backend_data_root_for_data_dir(data_dir, workspace_id.as_ref())
            }
            None => std::env::temp_dir()
                .join("yoi")
                .join("workspace-server")
                .join(workspace_id.as_ref()),
        }
    }

    pub fn embedded_runtime_store_root_for_data_dir(
        data_dir: impl Into<PathBuf>,
        workspace_id: impl AsRef<str>,
    ) -> PathBuf {
        Self::workspace_backend_data_root_for_data_dir(data_dir, workspace_id)
            .join("embedded-runtime")
    }

    pub fn default_embedded_runtime_store_root(workspace_id: impl AsRef<str>) -> PathBuf {
        Self::default_workspace_backend_data_root(workspace_id).join("embedded-runtime")
    }

    pub fn with_embedded_runtime_store_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.embedded_runtime_store_root = root.into();
        self
    }
}

#[derive(Clone)]
pub struct WorkspaceApi {
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    records: LocalProjectRecordReader,
    runtime: Arc<RuntimeRegistry>,
    companion: Arc<CompanionConsole>,
    observation_proxy: BackendObservationProxy,
}

impl WorkspaceApi {
    pub async fn new(config: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Result<Self> {
        let execution_backend = WorkerRuntimeExecutionBackend::from_workspace(
            config.workspace_root.clone(),
        )
        .map_err(|err| {
            crate::Error::Store(format!(
                "failed to initialize embedded Worker backend: {err}"
            ))
        })?;
        Self::new_with_execution_backend(config, store, Arc::new(execution_backend)).await
    }

    async fn new_with_execution_backend(
        config: ServerConfig,
        store: Arc<dyn ControlPlaneStore>,
        execution_backend: Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
    ) -> Result<Self> {
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
            EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(
                config.workspace_id.clone(),
                config.embedded_runtime_store_root.clone(),
                execution_backend,
            )
            .map_err(|err| {
                crate::Error::Store(format!("invalid embedded Worker backend: {err}"))
            })?,
        );
        for remote_config in config.remote_runtime_sources.iter().cloned() {
            runtime
                .register(RemoteWorkerRuntime::new(remote_config).map_err(|err| err.into_error())?);
        }
        let runtime = Arc::new(runtime);
        let companion = Arc::new(CompanionConsole::new(runtime.clone()));
        let observation_proxy = BackendObservationProxy::new(config.runtime_event_sources.clone());
        Ok(Self {
            records: LocalProjectRecordReader::new(config.workspace_root.clone()),
            config,
            store,
            runtime,
            companion,
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
        .route(
            "/api/workers",
            get(list_workers).post(create_workspace_worker),
        )
        .route(
            "/api/workers/launch-options",
            get(get_worker_launch_options),
        )
        .route(
            "/api/settings/runtime-connections",
            get(get_runtime_connection_settings),
        )
        .route(
            "/api/settings/runtime-connections/remotes",
            post(add_remote_runtime_connection),
        )
        .route(
            "/api/settings/runtime-connections/remotes/{runtime_id}",
            delete(delete_remote_runtime_connection),
        )
        .route(
            "/api/settings/runtime-connections/remotes/{runtime_id}/test",
            post(test_remote_runtime_connection),
        )
        .route("/api/companion/status", get(get_companion_status))
        .route("/api/companion/transcript", get(get_companion_transcript))
        .route("/api/companion/messages", post(post_companion_message))
        .route("/api/companion/cancel", post(post_companion_cancel))
        .route(
            "/api/runtimes/{runtime_id}/workers",
            post(create_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/config-bundles",
            post(sync_runtime_config_bundle),
        )
        .route(
            "/api/runtimes/{runtime_id}/config-bundles/{bundle_id}/availability",
            get(check_runtime_config_bundle),
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
    pub companion_console: ExtensionPointState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionPointState {
    pub status: String,
    pub note: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
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
pub struct RuntimeConnectionSettingsResponse {
    pub workspace_id: String,
    pub embedded: RuntimeConnectionSummary,
    pub remotes: Vec<RemoteRuntimeConnectionSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConnectionSummary {
    pub runtime_id: String,
    pub display_name: String,
    pub kind: String,
    pub built_in: bool,
    pub config_managed: bool,
    pub active: bool,
    pub can_spawn_worker: bool,
    pub restart_required: bool,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteRuntimeConnectionSummary {
    #[serde(flatten)]
    pub summary: RuntimeConnectionSummary,
    pub endpoint_configured: bool,
    pub token_ref_configured: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConnectionMutationResponse {
    pub workspace_id: String,
    pub restart_required: bool,
    pub remotes: Vec<RemoteRuntimeConnectionSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Deserialize)]
pub struct AddRemoteRuntimeConnectionRequest {
    pub runtime_id: String,
    pub display_name: Option<String>,
    pub endpoint: String,
    pub token_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteRuntimeTestResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub checked_at: String,
    pub state: String,
    pub protocol_version: Option<String>,
    pub compatibility_basis: String,
    pub capabilities: Vec<String>,
    pub health_result: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerLaunchOptionsResponse {
    pub workspace_id: String,
    pub runtimes: Vec<WorkerLaunchRuntimeOption>,
    pub profiles: Vec<WorkerLaunchProfileCandidate>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerLaunchRuntimeOption {
    pub runtime_id: String,
    pub display_name: String,
    pub built_in: bool,
    pub can_spawn_worker: bool,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerLaunchProfileCandidate {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct BrowserCreateWorkerRequest {
    pub runtime_id: String,
    pub display_name: String,
    pub profile: String,
    pub initial_text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserCreateWorkerResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub console_href: String,
    pub worker: WorkerSummary,
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
    let companion_status = api.companion.status();
    let companion_console = companion_console_extension_point(&companion_status);
    Ok(Json(WorkspaceResponse {
        workspace_id: api.config.workspace_id.clone(),
        display_name,
        record_authority: "local_yoi_project_records".to_string(),
        schema_version,
        auth: api.config.auth.clone(),
        extension_points: ExtensionPoints {
            store: "sqlite".to_string(),
            event_stream: ExtensionPointState {
                status: "backend_proxy".to_string(),
                note: "Worker observation streams are exposed only through the Workspace server proxy keyed by runtime_id + worker_id; browser clients never receive raw Runtime endpoints or socket paths.".to_string(),
                diagnostics: Vec::new(),
            },
            host_worker_bridge: ExtensionPointState {
                status: "runtime_registry".to_string(),
                note: "Hosts and Workers are projected from the Workspace RuntimeRegistry; raw Runtime endpoints, sockets, and local metadata paths are not exposed.".to_string(),
                diagnostics: Vec::new(),
            },
            companion_console,
        },
    }))
}

fn companion_console_extension_point(status: &CompanionStatusResponse) -> ExtensionPointState {
    let completion = status.transport.completion.clone();
    let note = match completion.as_str() {
        "connected" => "Workspace Companion is input-capable and browser input is dispatched through the normal Worker runtime path.".to_string(),
        "not_input_capable" => {
            let diagnostic_codes = status
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if diagnostic_codes.is_empty() {
                "Workspace Companion is not input-capable; check provider, config, profile, secret, and authority diagnostics.".to_string()
            } else {
                format!(
                    "Workspace Companion is not input-capable; check typed diagnostics: {diagnostic_codes}."
                )
            }
        }
        other => format!(
            "Workspace Companion transport reports {other}; browser input follows the Companion Worker runtime capability state."
        ),
    };
    ExtensionPointState {
        status: completion,
        note,
        diagnostics: status.diagnostics.clone(),
    }
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

async fn get_runtime_connection_settings(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeConnectionSettingsResponse>> {
    let local_config = load_workspace_backend_config_for_settings(&api)?;
    Ok(Json(runtime_connection_settings_response(
        &api,
        &local_config,
    )))
}

async fn add_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    Json(request): Json<AddRemoteRuntimeConnectionRequest>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    validate_runtime_connection_request(&request)?;
    let mut local_config = load_workspace_backend_config_for_settings(&api)?;
    let id = request.runtime_id.trim().to_string();
    if id == EMBEDDED_WORKER_RUNTIME_ID {
        return Err(settings_bad_request(
            "embedded_runtime_not_config_managed",
            "the embedded Runtime is built in and cannot be managed from local remote Runtime config",
        ));
    }
    if request
        .token_ref
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(settings_bad_request(
            "remote_runtime_token_ref_unsupported",
            "remote Runtime token_ref persistence is not supported by this v0 browser settings surface",
        ));
    }
    if local_config
        .runtimes
        .remote
        .iter()
        .any(|remote| remote.id == id)
    {
        return Err(settings_bad_request(
            "remote_runtime_already_exists",
            "a remote Runtime connection with that id is already configured",
        ));
    }
    local_config.runtimes.remote.push(RemoteRuntimeConfigFile {
        id,
        endpoint: request.endpoint.trim().to_string(),
        display_name: request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        token_ref: None,
    });
    write_workspace_backend_config_for_settings(&api, &local_config)?;
    let mut response = runtime_connection_mutation_response(&api, &local_config);
    response.diagnostics.push(settings_diagnostic(
        "workspace_backend_config_rewritten",
        DiagnosticSeverity::Info,
        "Local Runtime connection config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
    ));
    Ok(Json(response))
}

async fn delete_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    if runtime_id == EMBEDDED_WORKER_RUNTIME_ID {
        return Err(settings_bad_request(
            "embedded_runtime_not_config_managed",
            "the embedded Runtime is built in and cannot be deleted from remote Runtime config",
        ));
    }
    let mut local_config = load_workspace_backend_config_for_settings(&api)?;
    let before = local_config.runtimes.remote.len();
    local_config
        .runtimes
        .remote
        .retain(|remote| remote.id != runtime_id);
    if before == local_config.runtimes.remote.len() {
        return Err(Error::UnknownRuntime(runtime_id).into());
    }
    write_workspace_backend_config_for_settings(&api, &local_config)?;
    let mut response = runtime_connection_mutation_response(&api, &local_config);
    response.diagnostics.push(settings_diagnostic(
        "workspace_backend_config_rewritten",
        DiagnosticSeverity::Info,
        "Local Runtime connection config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
    ));
    Ok(Json(response))
}

async fn test_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RemoteRuntimeTestResponse>> {
    let local_config = load_workspace_backend_config_for_settings(&api)?;
    let remote = local_config
        .runtimes
        .remote
        .iter()
        .find(|remote| remote.id == runtime_id)
        .ok_or_else(|| Error::UnknownRuntime(runtime_id.clone()))?;
    Ok(Json(test_remote_runtime_config(&api, remote).await))
}

async fn get_worker_launch_options(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<WorkerLaunchOptionsResponse>> {
    Ok(Json(worker_launch_options_response(&api)))
}

async fn create_workspace_worker(
    State(api): State<WorkspaceApi>,
    Json(request): Json<BrowserCreateWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    let profile_selector = profile_selector_for_candidate(&request.profile).ok_or_else(|| {
        settings_bad_request(
            "unsupported_worker_profile",
            "profile must be selected from Backend-published worker profile candidates",
        )
    })?;
    let display_name = sanitize_worker_display_name(&request.display_name).ok_or_else(|| {
        settings_bad_request(
            "invalid_worker_display_name",
            "display_name must contain at least one non-control character",
        )
    })?;
    let initial_text = request.initial_text.trim().to_string();
    let initial_input = if initial_text.is_empty() {
        None
    } else {
        Some(EmbeddedWorkerInput {
            kind: EmbeddedWorkerInputKind::User,
            content: initial_text,
        })
    };
    let result = api
        .runtime
        .spawn_worker(
            &request.runtime_id,
            WorkerSpawnRequest {
                requested_worker_name: Some(display_name),
                intent: WorkerSpawnIntent::WorkspaceCoding,
                acceptance: if initial_input.is_some() {
                    WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 1,
                    }
                } else {
                    WorkerSpawnAcceptanceRequirement::SocketReady
                },
                profile: Some(profile_selector),
                initial_input,
            },
        )
        .map_err(|err| err.into_error())?;
    if result.state != WorkerOperationState::Accepted {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: request.runtime_id.clone(),
            code: "workspace_worker_create_failed".to_string(),
            message: "Runtime did not complete worker creation".to_string(),
        }
        .into());
    }
    let worker = result.worker.ok_or_else(|| Error::RuntimeOperationFailed {
        runtime_id: request.runtime_id.clone(),
        code: "workspace_worker_create_missing_summary".to_string(),
        message: "Runtime completed worker creation without returning a Worker summary".to_string(),
    })?;
    let runtime_id = worker.runtime_id.clone();
    let worker_id = worker.worker_id.clone();
    let console_href = format!(
        "/runtimes/{}/workers/{}/console",
        encode_path_segment(&runtime_id),
        encode_path_segment(&worker_id)
    );
    Ok(Json(BrowserCreateWorkerResponse {
        workspace_id: api.config.workspace_id,
        runtime_id,
        worker_id,
        console_href,
        worker,
        diagnostics: result.diagnostics,
    }))
}

async fn get_companion_status(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<CompanionStatusResponse>> {
    Ok(Json(api.companion.status()))
}

async fn get_companion_transcript(
    State(api): State<WorkspaceApi>,
    Query(query): Query<TranscriptQuery>,
) -> ApiResult<Json<CompanionTranscriptProjection>> {
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let start = query.start.unwrap_or(0);
    Ok(Json(api.companion.transcript(start, limit)))
}

async fn post_companion_message(
    State(api): State<WorkspaceApi>,
    Json(request): Json<CompanionMessageRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    Ok(Json(api.companion.send_message(request)))
}

async fn post_companion_cancel(
    State(api): State<WorkspaceApi>,
    Json(request): Json<CompanionCancelRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    Ok(Json(api.companion.cancel(request)))
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

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConfigBundleSyncRequest {
    pub bundle: ConfigBundle,
}

#[derive(Debug, Deserialize)]
struct RuntimeConfigBundleAvailabilityQuery {
    digest: String,
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

async fn sync_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
    Json(request): Json<RuntimeConfigBundleSyncRequest>,
) -> ApiResult<Json<ConfigBundleSyncResult>> {
    let result = api
        .runtime
        .sync_config_bundle(&runtime_id, request.bundle)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn check_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, bundle_id)): AxumPath<(String, String)>,
    Query(query): Query<RuntimeConfigBundleAvailabilityQuery>,
) -> ApiResult<Json<ConfigBundleCheckResult>> {
    let result = api
        .runtime
        .check_config_bundle(
            &runtime_id,
            ConfigBundleRef {
                id: bundle_id,
                digest: query.digest,
            },
        )
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
    source: crate::observation::RuntimeObservationSource,
    query: ClientWorkerEventsWsQuery,
    mut socket: WebSocket,
) {
    let open = match proxy.open(
        source.runtime_id(),
        source.worker_id(),
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
        match RuntimeObservationClient::connect(&source, open.runtime_cursor.as_deref()).await {
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

fn load_workspace_backend_config_for_settings(
    api: &WorkspaceApi,
) -> ApiResult<WorkspaceBackendConfigFile> {
    WorkspaceBackendConfigFile::load_for_workspace(&api.config.workspace_root).map_err(|error| {
        Error::Config(format!(
            "failed to read workspace backend local config for Runtime connections: {}",
            sanitize_backend_error(&error.to_string())
        ))
        .into()
    })
}

fn write_workspace_backend_config_for_settings(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
) -> ApiResult<()> {
    local_config
        .write_for_workspace(&api.config.workspace_root)
        .map_err(|error| {
            Error::Config(format!(
                "failed to write workspace backend local config for Runtime connections: {}",
                sanitize_backend_error(&error.to_string())
            ))
            .into()
        })
}

fn runtime_connection_settings_response(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
) -> RuntimeConnectionSettingsResponse {
    RuntimeConnectionSettingsResponse {
        workspace_id: api.config.workspace_id.clone(),
        embedded: embedded_runtime_connection_summary(api),
        remotes: remote_runtime_connection_summaries(api, local_config, false),
        diagnostics: Vec::new(),
    }
}

fn runtime_connection_mutation_response(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
) -> RuntimeConnectionMutationResponse {
    RuntimeConnectionMutationResponse {
        workspace_id: api.config.workspace_id.clone(),
        restart_required: true,
        remotes: remote_runtime_connection_summaries(api, local_config, true),
        diagnostics: vec![settings_diagnostic(
            "runtime_registry_restart_required",
            DiagnosticSeverity::Warning,
            "Runtime connection config changed; restart the Workspace backend for the live Runtime registry to use the new config.",
        )],
    }
}

fn embedded_runtime_connection_summary(api: &WorkspaceApi) -> RuntimeConnectionSummary {
    let active = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items
        .into_iter()
        .find(|runtime| runtime.runtime_id == EMBEDDED_WORKER_RUNTIME_ID);
    match active {
        Some(runtime) => RuntimeConnectionSummary {
            runtime_id: runtime.runtime_id,
            display_name: runtime.label,
            kind: runtime.kind,
            built_in: true,
            config_managed: false,
            active: runtime.status == "active",
            can_spawn_worker: runtime.capabilities.can_spawn_worker,
            restart_required: false,
            status: runtime.status,
            diagnostics: runtime.diagnostics,
        },
        None => RuntimeConnectionSummary {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            display_name: "Embedded Runtime".to_string(),
            kind: "embedded_worker_runtime".to_string(),
            built_in: true,
            config_managed: false,
            active: false,
            can_spawn_worker: false,
            restart_required: false,
            status: "unavailable".to_string(),
            diagnostics: vec![settings_diagnostic(
                "embedded_runtime_unavailable",
                DiagnosticSeverity::Warning,
                "The built-in embedded Runtime is not active in the current Runtime registry projection.",
            )],
        },
    }
}

fn remote_runtime_connection_summaries(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
    restart_required: bool,
) -> Vec<RemoteRuntimeConnectionSummary> {
    let live_runtimes = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items;
    local_config
        .runtimes
        .remote
        .iter()
        .map(|remote| {
            let live = live_runtimes
                .iter()
                .find(|runtime| runtime.runtime_id == remote.id);
            let (display_name, kind, active, can_spawn_worker, status, diagnostics) = match live {
                Some(runtime) => (
                    runtime.label.clone(),
                    runtime.kind.clone(),
                    runtime.status == "active",
                    runtime.capabilities.can_spawn_worker,
                    runtime.status.clone(),
                    runtime.diagnostics.clone(),
                ),
                None => (
                    remote
                        .display_name
                        .clone()
                        .unwrap_or_else(|| remote.id.clone()),
                    "remote_http".to_string(),
                    false,
                    false,
                    "configured_restart_required".to_string(),
                    if restart_required {
                        vec![settings_diagnostic(
                            "runtime_registry_restart_required",
                            DiagnosticSeverity::Warning,
                            "This remote Runtime config is persisted but not active until the Workspace backend restarts.",
                        )]
                    } else {
                        Vec::new()
                    },
                ),
            };
            RemoteRuntimeConnectionSummary {
                summary: RuntimeConnectionSummary {
                    runtime_id: remote.id.clone(),
                    display_name,
                    kind,
                    built_in: false,
                    config_managed: true,
                    active,
                    can_spawn_worker,
                    restart_required,
                    status,
                    diagnostics,
                },
                endpoint_configured: !remote.endpoint.trim().is_empty(),
                token_ref_configured: remote
                    .token_ref
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
            }
        })
        .collect()
}

fn validate_runtime_connection_request(
    request: &AddRemoteRuntimeConnectionRequest,
) -> ApiResult<()> {
    validate_public_runtime_id(request.runtime_id.trim())?;
    let endpoint = request.endpoint.trim();
    if endpoint.is_empty() || !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
    {
        return Err(settings_bad_request(
            "invalid_remote_runtime_endpoint",
            "endpoint must be an absolute http or https URL",
        ));
    }
    if request
        .display_name
        .as_deref()
        .is_some_and(|value| value.chars().any(char::is_control))
    {
        return Err(settings_bad_request(
            "invalid_remote_runtime_display_name",
            "display_name cannot contain control characters",
        ));
    }
    Ok(())
}

fn validate_public_runtime_id(runtime_id: &str) -> ApiResult<()> {
    if runtime_id.is_empty() {
        return Err(settings_bad_request(
            "invalid_runtime_id",
            "runtime_id must not be empty",
        ));
    }
    if runtime_id.len() > 96
        || !runtime_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(settings_bad_request(
            "invalid_runtime_id",
            "runtime_id may contain only ASCII letters, digits, '-', '_' and '.' and must be at most 96 characters",
        ));
    }
    Ok(())
}

async fn test_remote_runtime_config(
    api: &WorkspaceApi,
    remote: &RemoteRuntimeConfigFile,
) -> RemoteRuntimeTestResponse {
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if remote
        .token_ref
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return RemoteRuntimeTestResponse {
            workspace_id: api.config.workspace_id.clone(),
            runtime_id: remote.id.clone(),
            checked_at,
            state: "rejected".to_string(),
            protocol_version: None,
            compatibility_basis: "not_checked_token_ref_unsupported".to_string(),
            capabilities: Vec::new(),
            health_result: "not_checked".to_string(),
            diagnostics: vec![settings_diagnostic(
                "remote_runtime_token_ref_unsupported",
                DiagnosticSeverity::Error,
                "Remote Runtime test cannot use token_ref in v0; no token or secret value was exposed to the Browser.",
            )],
        };
    }
    let url = format!("{}/v1/runtime", remote.endpoint.trim_end_matches('/'));
    let result = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .and_then(|client| client.get(url).build().map(|request| (client, request)));
    let Ok((client, request)) = result else {
        return remote_runtime_test_failed(
            api,
            remote,
            checked_at,
            "remote_runtime_invalid_request",
            "failed to build a remote Runtime test request",
        );
    };
    match client.execute(request).await {
        Ok(response) if response.status().is_success() => {
            match response.json::<RuntimeHttpSummaryResponse>().await {
                Ok(summary) => RemoteRuntimeTestResponse {
                    workspace_id: api.config.workspace_id.clone(),
                    runtime_id: remote.id.clone(),
                    checked_at,
                    state: "compatible".to_string(),
                    protocol_version: None,
                    compatibility_basis: "worker-runtime /v1/runtime summary endpoint responded successfully; no protocol_version field was advertised".to_string(),
                    capabilities: Vec::new(),
                    health_result: format!("status={:?}", summary.runtime.status),
                    diagnostics: Vec::new(),
                },
                Err(_) => remote_runtime_test_failed(
                    api,
                    remote,
                    checked_at,
                    "remote_runtime_malformed_summary",
                    "remote Runtime test endpoint responded, but the summary payload was not recognized",
                ),
            }
        }
        Ok(response) => remote_runtime_test_failed(
            api,
            remote,
            checked_at,
            "remote_runtime_unhealthy",
            format!(
                "remote Runtime test endpoint returned HTTP status {}",
                response.status().as_u16()
            ),
        ),
        Err(error) => remote_runtime_test_failed(
            api,
            remote,
            checked_at,
            "remote_runtime_test_failed",
            sanitize_backend_error(&error.to_string()),
        ),
    }
}

fn remote_runtime_test_failed(
    api: &WorkspaceApi,
    remote: &RemoteRuntimeConfigFile,
    checked_at: String,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RemoteRuntimeTestResponse {
    RemoteRuntimeTestResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: remote.id.clone(),
        checked_at,
        state: "failed".to_string(),
        protocol_version: None,
        compatibility_basis: "worker-runtime /v1/runtime summary endpoint test".to_string(),
        capabilities: Vec::new(),
        health_result: "failed".to_string(),
        diagnostics: vec![settings_diagnostic(
            code,
            DiagnosticSeverity::Error,
            message,
        )],
    }
}

fn worker_launch_options_response(api: &WorkspaceApi) -> WorkerLaunchOptionsResponse {
    let runtimes = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items
        .into_iter()
        .map(|runtime| {
            let built_in = runtime.runtime_id == EMBEDDED_WORKER_RUNTIME_ID;
            WorkerLaunchRuntimeOption {
                runtime_id: runtime.runtime_id,
                display_name: runtime.label,
                built_in,
                can_spawn_worker: runtime.capabilities.can_spawn_worker,
                status: runtime.status,
                diagnostics: runtime.diagnostics,
            }
        })
        .collect();
    WorkerLaunchOptionsResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtimes,
        profiles: worker_profile_candidates(),
        diagnostics: Vec::new(),
    }
}

fn worker_profile_candidates() -> Vec<WorkerLaunchProfileCandidate> {
    vec![
        WorkerLaunchProfileCandidate {
            id: "builtin:coder".to_string(),
            label: "Coding Worker".to_string(),
            description: "Built-in coding role profile for implementation work.".to_string(),
        },
        WorkerLaunchProfileCandidate {
            id: "runtime_default".to_string(),
            label: "Runtime default".to_string(),
            description: "Use the selected Runtime's default profile.".to_string(),
        },
    ]
}

fn profile_selector_for_candidate(profile: &str) -> Option<ProfileSelector> {
    match profile {
        "builtin:coder" => Some(ProfileSelector::Builtin("builtin:coder".to_string())),
        "runtime_default" => Some(ProfileSelector::RuntimeDefault),
        _ => None,
    }
}

fn sanitize_worker_display_name(value: &str) -> Option<String> {
    let display_name = value.trim();
    if display_name.is_empty() || display_name.chars().any(char::is_control) {
        None
    } else {
        Some(display_name.chars().take(80).collect())
    }
}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn settings_bad_request(code: &'static str, message: &'static str) -> ApiError {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
    .into()
}

fn settings_diagnostic(
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

fn sanitize_backend_error(message: &str) -> String {
    let workspace_paths = ["/home/", "/Users/", "\\\\", ":\\"];
    if workspace_paths
        .iter()
        .any(|needle| message.contains(needle))
    {
        "operation failed; backend-private path details were omitted".to_string()
    } else {
        message.to_string()
    }
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
            Error::RuntimeOperationFailed { code, .. }
                if code.starts_with("workspace_settings_")
                    || code.starts_with("invalid_")
                    || code.starts_with("unsupported_worker_profile")
                    || code.ends_with("_already_exists")
                    || code.ends_with("_not_config_managed")
                    || code.ends_with("_unsupported") =>
            {
                StatusCode::BAD_REQUEST
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
    use std::sync::Arc;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;

    use crate::hosts::{
        TicketWorkerRole, WorkerInputKind, WorkerOperationState, WorkerSpawnAcceptanceRequirement,
        WorkerSpawnIntent,
    };
    use crate::observation::ClientWorkerEventWsDiagnostic;
    use crate::store::SqliteWorkspaceStore;

    const TEST_WORKSPACE_ID: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_REPOSITORY_ID: &str = "local-0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_CREATED_AT: &str = "2026-06-23T06:43:28Z";

    #[test]
    fn worker_profile_candidates_are_backend_published_and_mapped() {
        let candidates = worker_profile_candidates();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.id == "builtin:coder")
        );
        assert!(matches!(
            profile_selector_for_candidate("builtin:coder"),
            Some(ProfileSelector::Builtin(value)) if value == "builtin:coder"
        ));
        assert!(profile_selector_for_candidate("free-text-profile").is_none());
    }

    #[test]
    fn runtime_connection_request_validation_bounds_browser_input() {
        let ok = AddRemoteRuntimeConnectionRequest {
            runtime_id: "team-runtime_1".to_string(),
            display_name: Some("Team Runtime".to_string()),
            endpoint: "https://runtime.example".to_string(),
            token_ref: None,
        };
        assert!(validate_runtime_connection_request(&ok).is_ok());

        let bad_endpoint = AddRemoteRuntimeConnectionRequest {
            endpoint: "/tmp/socket".to_string(),
            ..ok
        };
        assert!(validate_runtime_connection_request(&bad_endpoint).is_err());
    }

    #[test]
    fn sanitized_errors_omit_backend_private_paths() {
        let sanitized = sanitize_backend_error(
            "failed to open /home/example/.yoi/workspace-backend.local.toml",
        );
        assert!(!sanitized.contains("/home/example"));
    }

    #[derive(Default)]
    struct DeterministicExecutionBackend {
        contexts: std::sync::Mutex<
            std::collections::HashMap<
                worker_runtime::identity::WorkerRef,
                worker_runtime::execution::WorkerExecutionContext,
            >,
        >,
    }

    impl worker_runtime::execution::WorkerExecutionBackend for DeterministicExecutionBackend {
        fn backend_id(&self) -> &str {
            "deterministic-workspace-server-test"
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
                run_state: worker_runtime::execution::WorkerExecutionRunState::Idle,
            }
        }

        fn dispatch_input(
            &self,
            handle: &worker_runtime::execution::WorkerExecutionHandle,
            input: worker_runtime::interaction::WorkerInput,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            let context = self
                .contexts
                .lock()
                .unwrap()
                .get(handle.worker_ref())
                .cloned()
                .expect("execution context");
            let content = input.content.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(25));
                let _ = context.publish_protocol_event(protocol::Event::TextDone {
                    text: format!("server companion echoed: {content}"),
                });
            });
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::Input,
                worker_runtime::execution::WorkerExecutionRunState::Idle,
            )
        }
    }

    fn test_identity() -> WorkspaceIdentity {
        WorkspaceIdentity {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            display_name: "Test Workspace".to_string(),
            created_at: TEST_CREATED_AT.to_string(),
        }
    }

    fn test_server_config(workspace_root: impl Into<PathBuf>) -> ServerConfig {
        let workspace_root = workspace_root.into();
        let store_root = workspace_root.join(".test-embedded-runtime-store");
        ServerConfig::local_dev(workspace_root, test_identity())
            .with_embedded_runtime_store_root(store_root)
    }

    fn runtime_test_bundle() -> worker_runtime::config_bundle::ConfigBundle {
        worker_runtime::config_bundle::ConfigBundle {
            metadata: worker_runtime::config_bundle::ConfigBundleMetadata {
                id: "server-test-bundle".to_string(),
                digest: String::new(),
                revision: "test".to_string(),
                workspace_id: "test".to_string(),
                created_at: "test".to_string(),
                provenance: worker_runtime::config_bundle::ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![worker_runtime::config_bundle::ConfigProfileDescriptor {
                selector: worker_runtime::catalog::ProfileSelector::RuntimeDefault,
                label: Some("server-test".to_string()),
            }],
            declarations: Vec::new(),
        }
        .with_computed_digest()
    }

    fn runtime_create_request() -> worker_runtime::catalog::CreateWorkerRequest {
        let bundle = runtime_test_bundle();
        worker_runtime::catalog::CreateWorkerRequest {
            profile: worker_runtime::catalog::ProfileSelector::RuntimeDefault,
            config_bundle: worker_runtime::catalog::ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            },
            initial_input: None,
        }
    }

    fn runtime_with_worker() -> (worker_runtime::Runtime, worker_runtime::identity::WorkerRef) {
        let runtime = worker_runtime::Runtime::with_execution_backend(
            worker_runtime::RuntimeOptions::default(),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(runtime_test_bundle()).unwrap();
        let worker = runtime.create_worker(runtime_create_request()).unwrap();
        (runtime, worker.worker_ref)
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
        let mut config = test_server_config(dir.path());
        config.static_assets_dir = Some(static_dir);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        assert_eq!(workspace["workspace_id"], TEST_WORKSPACE_ID);
        assert_eq!(workspace["display_name"], "Test Workspace");
        assert_eq!(workspace["record_authority"], "local_yoi_project_records");
        assert_eq!(
            workspace["extension_points"]["host_worker_bridge"]["status"],
            "runtime_registry"
        );
        let workspace_companion = &workspace["extension_points"]["companion_console"];
        assert_ne!(workspace_companion["status"], "not_connected");
        assert!(
            !workspace_companion["note"]
                .as_str()
                .unwrap()
                .contains("browser input remains disabled"),
            "stale Companion Console note returned: {workspace_companion}"
        );
        if workspace_companion["status"] == "not_input_capable" {
            assert!(
                !workspace_companion["diagnostics"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "not_input_capable workspace companion_console lacks typed diagnostics: {workspace_companion}"
            );
        }

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
        assert_eq!(hosts["items"][0]["runtime_id"], "embedded-worker-runtime");
        let host_id = hosts["items"][0]["host_id"].as_str().unwrap().to_string();
        assert_eq!(hosts["items"][0]["kind"], "embedded-worker-runtime-host");
        assert_eq!(
            hosts["items"][0]["capabilities"]["workspace_scope"],
            "backend_internal"
        );
        assert!(!hosts.to_string().contains("metadata.json"));

        let runtimes = get_json(app.clone(), "/api/runtimes").await;
        assert_eq!(runtimes["source"], "worker_runtime_registry");
        assert_eq!(
            runtimes["items"][0]["runtime_id"],
            "embedded-worker-runtime"
        );
        assert_eq!(
            runtimes["items"][0]["source"]["kind"],
            "embedded_worker_runtime"
        );
        assert_eq!(
            runtimes["items"][0]["source"]["identity_authority"],
            "runtime_registry_projection"
        );
        assert!(!runtimes.to_string().contains("/workspace/demo"));
        assert_eq!(runtimes["items"][0]["host_ids"][0], host_id);

        let workers = get_json(app.clone(), "/api/workers").await;
        let worker_items = workers["items"].as_array().unwrap();
        let companion_worker = worker_items
            .iter()
            .find(|worker| worker["role"] == "builtin:companion")
            .expect("companion worker is visible through runtime worker API");
        assert_eq!(companion_worker["runtime_id"], "embedded-worker-runtime");
        assert!(companion_worker["capabilities"]["can_stop"].is_boolean());

        let companion_status = get_json(app.clone(), "/api/companion/status").await;
        assert!(matches!(
            companion_status["state"].as_str(),
            Some("ready") | Some("error")
        ));
        assert_eq!(companion_status["worker"]["role"], "builtin:companion");
        assert_eq!(
            companion_status["transport"]["kind"],
            "embedded_worker_runtime"
        );
        assert_ne!(companion_status["transport"]["completion"], "not_connected");
        assert!(!companion_status.to_string().contains("/workspace/demo"));

        let companion_message = post_json(
            app.clone(),
            "/api/companion/messages",
            json!({ "content": "hello companion" }),
        )
        .await;
        assert_eq!(companion_message["state"], "accepted");
        assert_eq!(companion_message["user_item"]["role"], "user");
        assert_eq!(companion_message["user_item"]["content"], "hello companion");
        assert!(
            !companion_message
                .to_string()
                .contains("companion_llm_not_connected"),
            "legacy non-execution diagnostic leaked: {companion_message}"
        );
        assert!(!companion_message.to_string().contains("providerless"));
        assert!(!companion_message.to_string().contains("/workspace/demo"));

        let companion_transcript = get_json(app.clone(), "/api/companion/transcript").await;
        assert!(companion_transcript["total_items"].as_u64().unwrap() >= 1);

        let host_workers = get_json(app.clone(), &format!("/api/hosts/{host_id}/workers")).await;
        assert!(
            host_workers["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|worker| worker["role"] == "builtin:companion")
        );

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
    async fn legacy_companion_messages_route_dispatches_through_worker_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_server_config(temp.path().join("workspace"));
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        let workspace_companion = &workspace["extension_points"]["companion_console"];
        assert_eq!(workspace_companion["status"], "connected");
        assert!(
            workspace_companion["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            workspace_companion["note"]
                .as_str()
                .unwrap()
                .contains("normal Worker runtime path")
        );

        let status = get_json(app.clone(), "/api/companion/status").await;
        assert_eq!(status["transport"]["completion"], "connected");
        let worker_id = status["worker"]["worker_id"].as_str().unwrap().to_string();
        assert_eq!(status["worker"]["profile"], "builtin:companion");

        let response = post_json(
            app.clone(),
            "/api/companion/messages",
            serde_json::json!({ "content": "from legacy route" }),
        )
        .await;
        assert_eq!(response["state"], "accepted");
        assert_eq!(response["user_item"]["content"], "from legacy route");
        assert!(!response.to_string().contains("companion_llm_not_connected"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let transcript = loop {
            let transcript = get_json(app.clone(), "/api/companion/transcript").await;
            let has_assistant = transcript["items"].as_array().unwrap().iter().any(|item| {
                item["role"] == "assistant"
                    && item["content"] == "server companion echoed: from legacy route"
                    && item["source"] == "worker_runtime"
            });
            if has_assistant {
                break transcript;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for server companion transcript: {transcript}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(
            transcript["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["role"] == "user" && item["content"] == "from legacy route" })
        );

        let worker_transcript = get_json(
            app,
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/transcript"),
        )
        .await;
        assert!(
            worker_transcript["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| {
                    item["role"] == "assistant"
                        && item["content"] == "server companion echoed: from legacy route"
                })
        );
    }

    #[tokio::test]
    async fn embedded_runtime_fs_store_restores_catalog_config_bundle_transcript_and_stale_execution()
     {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path().join("workspace"));
        let store_root = config.embedded_runtime_store_root.clone();
        let bundle = runtime_test_bundle();
        let bundle_id = bundle.metadata.id.clone();

        let api = WorkspaceApi::new_with_execution_backend(
            config.clone(),
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .expect("fs-backed api starts");
        let synced = api
            .runtime
            .sync_config_bundle("embedded-worker-runtime", bundle)
            .expect("sync config bundle");
        assert_eq!(synced.state, WorkerOperationState::Accepted);
        assert!(store_root.exists(), "fs-store root should be created");

        let spawned = api
            .runtime
            .spawn_worker(
                "embedded-worker-runtime",
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
                },
            )
            .expect("spawn worker");
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker_id = spawned.worker.expect("created worker").worker_id;
        let sent = api
            .runtime
            .send_input(
                "embedded-worker-runtime",
                &worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "persist me".to_string(),
                },
            )
            .expect("send input");
        assert_eq!(sent.state, WorkerOperationState::Accepted);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let transcript = api
                .runtime
                .transcript("embedded-worker-runtime", &worker_id, 0, 10)
                .expect("transcript");
            if transcript.items.iter().any(|item| {
                item.role == "assistant" && item.content == "server companion echoed: persist me"
            }) {
                assert!(
                    transcript
                        .items
                        .iter()
                        .any(|item| item.role == "user" && item.content == "persist me")
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for deterministic transcript"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        drop(api);

        let restored = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .expect("restored fs-backed api starts");
        let restored_worker = restored
            .runtime
            .worker("embedded-worker-runtime", &worker_id)
            .expect("restored worker");
        assert_eq!(restored_worker.status, "stale");
        assert!(!restored_worker.capabilities.can_accept_input);
        assert!(!restored_worker.capabilities.can_stop);
        assert!(
            restored_worker
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "embedded_worker_execution_stale")
        );

        let bundles = restored
            .runtime
            .list_config_bundles("embedded-worker-runtime")
            .expect("config bundle list");
        assert!(
            bundles
                .bundles
                .iter()
                .any(|summary| summary.id == bundle_id)
        );

        let restored_transcript = restored
            .runtime
            .transcript("embedded-worker-runtime", &worker_id, 0, 10)
            .expect("restored transcript");
        assert!(
            restored_transcript
                .items
                .iter()
                .any(|item| item.role == "user" && item.content == "persist me")
        );
        assert!(restored_transcript.items.iter().any(|item| {
            item.role == "assistant" && item.content == "server companion echoed: persist me"
        }));

        let rejected_input = restored
            .runtime
            .send_input(
                "embedded-worker-runtime",
                &worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "should not be routed to stale handle".to_string(),
                },
            )
            .expect("stale worker input is projected as an operation result");
        assert_eq!(rejected_input.state, WorkerOperationState::Rejected);
    }

    #[tokio::test]
    async fn embedded_runtime_store_root_is_isolated_and_not_exposed_by_browser_api() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("user-data");
        let workspace_root = dir.path().join("workspace");
        let default_root =
            ServerConfig::embedded_runtime_store_root_for_data_dir(&data_dir, TEST_WORKSPACE_ID);
        assert_eq!(
            default_root,
            data_dir
                .join("workspace-server")
                .join(TEST_WORKSPACE_ID)
                .join("embedded-runtime")
        );
        assert!(!default_root.starts_with(workspace_root.join(".yoi")));

        let config = ServerConfig::local_dev(workspace_root, test_identity())
            .with_embedded_runtime_store_root(default_root.clone());
        let app = build_router(
            WorkspaceApi::new_with_execution_backend(
                config,
                Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
                Arc::new(DeterministicExecutionBackend::default()),
            )
            .await
            .unwrap(),
        );
        let raw_store_root = default_root.to_string_lossy().to_string();
        for uri in [
            "/api/workspace",
            "/api/hosts",
            "/api/runtimes",
            "/api/workers",
        ] {
            let body = get_json(app.clone(), uri).await;
            let serialized = serde_json::to_string(&body).unwrap();
            assert!(
                !serialized.contains(&raw_store_root),
                "{uri} leaked embedded runtime store root: {serialized}"
            );
        }
    }

    #[tokio::test]
    async fn embedded_runtime_api_routes_by_runtime_and_worker_ids_without_leaking_internals() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let config = test_server_config(dir.path());
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
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
        let diagnostics = spawned["diagnostics"].as_array().unwrap();
        assert!(diagnostics.iter().all(|diagnostic| {
            !diagnostic["message"]
                .as_str()
                .unwrap_or_default()
                .contains("/workspace/demo")
        }));
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
        assert!(accepted["diagnostics"].as_array().unwrap().is_empty());

        let transcript = get_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/transcript?start=0&limit=10"),
        )
        .await;
        assert_eq!(transcript["state"], "accepted");
        assert!(transcript["items"].as_array().unwrap().iter().any(
            |item| item["role"] == "user" && item["content"] == "hello from browser-facing api"
        ));

        let wrong_runtime = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/runtimes/unknown-runtime/workers/{worker_id}/input"
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
        let (runtime, worker_ref) = runtime_with_worker();
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
        let mut config = test_server_config(dir.path());
        config
            .runtime_event_sources
            .push(RuntimeObservationSourceConfig {
                runtime_id: "runtime-a".into(),
                worker_id: "worker-a".into(),
                endpoint: format!(
                    "ws://{runtime_addr}/v1/workers/{}/events/ws",
                    worker_ref.worker_id
                ),
                bearer_token: None,
            });
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
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
                &worker_ref,
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
                &worker_ref,
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
        let (runtime, worker_ref) = runtime_with_worker();
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
            worker_ref.worker_id
        );
        (runtime, worker_ref, endpoint)
    }

    async fn spawn_workspace_proxy(
        source: RuntimeObservationSourceConfig,
    ) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = test_server_config(dir.path());
        let runtime_id = source.runtime_id.clone();
        let worker_id = source.worker_id.clone();
        config.runtime_event_sources.push(source);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
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
