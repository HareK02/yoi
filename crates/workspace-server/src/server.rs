use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::hosts::{HostSummary, LocalRuntimeBridge, RuntimeDiagnostic, WorkerSummary};
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
    pub workspace_root: PathBuf,
    pub static_assets_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    pub max_records: usize,
    pub local_runtime_data_dir: Option<PathBuf>,
}

impl ServerConfig {
    pub fn local_dev(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let display = workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace");
        Self {
            workspace_id: format!("local:{display}"),
            workspace_root,
            static_assets_dir: None,
            auth: AuthConfig::LocalDevToken {
                token_configured: false,
            },
            max_records: 200,
            local_runtime_data_dir: manifest::paths::data_dir(),
        }
    }
}

#[derive(Clone)]
pub struct WorkspaceApi {
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    records: LocalProjectRecordReader,
}

impl WorkspaceApi {
    pub async fn new(config: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Result<Self> {
        let display_name = config
            .workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: config.workspace_id.clone(),
                display_name,
                state: "active".to_string(),
                created_at: "1970-01-01T00:00:00Z".to_string(),
                updated_at: "1970-01-01T00:00:00Z".to_string(),
            })
            .await?;
        Ok(Self {
            records: LocalProjectRecordReader::new(config.workspace_root.clone()),
            config,
            store,
        })
    }

    pub fn workspace_id(&self) -> &str {
        self.config.workspace_id.as_str()
    }

    fn local_runtime_bridge(&self) -> LocalRuntimeBridge {
        LocalRuntimeBridge::new(
            self.config.workspace_id.clone(),
            self.config.workspace_root.clone(),
            self.config.local_runtime_data_dir.clone(),
        )
    }

    fn local_repository_reader(&self) -> LocalRepositoryReader {
        LocalRepositoryReader::new(self.config.workspace_root.clone())
    }

    fn workspace_display_name(&self) -> String {
        self.config
            .workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string()
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
        .route("/api/workers", get(list_workers))
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

async fn get_workspace(State(api): State<WorkspaceApi>) -> ApiResult<Json<WorkspaceResponse>> {
    let schema_version = api.store.schema_version().await?;
    let stored = api.store.get_workspace(api.workspace_id()).await?;
    let display_name = stored
        .as_ref()
        .map(|record| record.display_name.clone())
        .or_else(|| {
            api.config
                .workspace_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "workspace".to_string());
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
                note: "No browser-to-Pod socket path is exposed in this bootstrap; any future stream must be a Workspace server proxy that resolves Worker identity and enforces method allow/block boundaries.".to_string(),
            },
            host_worker_bridge: ExtensionPointState {
                status: "read_only_local".to_string(),
                note: "Local Hosts and Workers are exposed as a read-only bridge over existing Pod metadata; no direct Pod socket, scheduling, or lifecycle control is implemented.".to_string(),
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
    let items = reader.list(&api.workspace_display_name());
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
    ensure_local_repository(&repository_id)?;
    let reader = api.local_repository_reader();
    Ok(Json(RepositoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        item: reader.summary(&api.workspace_display_name()),
        source: "local_workspace_root".to_string(),
    }))
}

async fn repository_log(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<RepositoryLogResponse>> {
    ensure_local_repository(&repository_id)?;
    let RepositoryLogRead {
        limit,
        items,
        diagnostics,
    } = api.local_repository_reader().recent_log(query.limit);
    Ok(Json(RepositoryLogResponse {
        workspace_id: api.config.workspace_id,
        repository_id,
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
    ensure_local_repository(&repository_id)?;
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_tickets(limit)?;
    Ok(Json(RepositoryTicketsResponse {
        workspace_id: api.config.workspace_id,
        repository_id,
        limit,
        columns: ticket_kanban_columns(items),
        invalid_records,
        record_authority,
        source: "workspace_local_ticket_fallback".to_string(),
        diagnostics: vec![RuntimeDiagnostic {
            code: "repository_ticket_target_metadata_absent".to_string(),
            severity: "info".to_string(),
            message: "Ticket target Repository metadata is not available yet; Kanban groups all workspace-local Tickets by state as a read-only fallback.".to_string(),
        }],
    }))
}

async fn list_hosts(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<HostSummary>>> {
    let limit = api.config.max_records.min(200);
    let bridge = api.local_runtime_bridge();
    let (items, diagnostics) = bridge.list_hosts(limit);
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        source: "local_pod_metadata".to_string(),
        diagnostics,
    }))
}

async fn list_workers(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    workers_response(api).map(Json)
}

async fn list_host_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(host_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    let bridge = api.local_runtime_bridge();
    if host_id != bridge.host_id() {
        return Err(Error::UnknownHost(host_id).into());
    }
    workers_response(api).map(Json)
}

fn workers_response(api: WorkspaceApi) -> ApiResult<RuntimeListResponse<WorkerSummary>> {
    let limit = api.config.max_records.min(200);
    let bridge = api.local_runtime_bridge();
    let (items, diagnostics) = bridge.list_workers(limit);
    Ok(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        source: "local_pod_metadata".to_string(),
        diagnostics,
    })
}

fn ensure_local_repository(repository_id: &str) -> Result<()> {
    if LocalRepositoryReader::is_local_repository_id(repository_id) {
        Ok(())
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
            Error::InvalidRecordId(_)
            | Error::MissingFrontmatter(_)
            | Error::UnknownHost(_)
            | Error::UnknownRepository(_) => StatusCode::NOT_FOUND,
            Error::Ticket(_) => StatusCode::NOT_FOUND,
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
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::store::SqliteWorkspaceStore;

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
        let mut config = ServerConfig::local_dev(dir.path());
        config.workspace_id = "local:test".to_string();
        config.static_assets_dir = Some(static_dir);
        config.local_runtime_data_dir = Some(dir.path().join("data"));
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        assert_eq!(workspace["workspace_id"], "local:test");
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
        assert_eq!(repositories["items"][0]["id"], "local");
        assert_eq!(repositories["items"][0]["kind"], "local");

        let repository_detail = get_json(app.clone(), "/api/repositories/local").await;
        assert_eq!(repository_detail["item"]["id"], "local");

        let repository_log = get_json(app.clone(), "/api/repositories/local/log?limit=3").await;
        assert_eq!(repository_log["repository_id"], "local");
        assert_eq!(repository_log["limit"], 3);

        let repository_tickets = get_json(app.clone(), "/api/repositories/local/tickets").await;
        assert_eq!(repository_tickets["repository_id"], "local");
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
        assert_eq!(hosts["items"][0]["host_id"], "local-local-test");
        assert_eq!(hosts["items"][0]["kind"], "local_host");
        assert_eq!(
            hosts["items"][0]["capabilities"]["local_pod_inspection"],
            "unavailable"
        );

        let workers = get_json(app.clone(), "/api/workers").await;
        assert!(workers["items"].as_array().unwrap().is_empty());
        assert_eq!(
            workers["diagnostics"][0]["code"],
            "local_pod_metadata_root_missing"
        );

        let host_workers = get_json(app.clone(), "/api/hosts/local-local-test/workers").await;
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

    async fn get_json(app: Router, uri: &str) -> Value {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
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
