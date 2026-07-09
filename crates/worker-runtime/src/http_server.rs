//! Optional REST process adapter for the Runtime command API.
//!
//! This module is intentionally gated by the `http-server` feature so embedded
//! Runtime users do not pull HTTP dependencies.  The server is a process-local
//! command surface for a trusted backend/proxy. Browsers must not connect to the
//! Runtime process directly; a backend is expected to own any browser-facing
//! credentials, registration, and policy.

use crate::Runtime;
use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, WorkerDetail, WorkerLifecycleAck, WorkerSummary,
    WorkingDirectoryRequest, WorkingDirectoryStatus,
};
use crate::config_bundle::{ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary};
use crate::error::RuntimeError;
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInteractionAck};
use crate::management::{RuntimeLimits, RuntimeSummary};
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationCursor;
use crate::observation::{TranscriptProjection, TranscriptQuery};
use axum::body::{Body, Bytes};
use axum::extract::rejection::{JsonRejection, QueryRejection};
#[cfg(feature = "ws-server")]
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
#[cfg(feature = "ws-server")]
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;
#[cfg(feature = "fs-store")]
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

const DEFAULT_RUNTIME_HTTP_PORT: u16 = 38800;

fn default_runtime_http_bind_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], DEFAULT_RUNTIME_HTTP_PORT))
}

/// v0 Runtime REST server configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeHttpServerConfig {
    /// Address for the Runtime process to bind. Use a loopback address unless a
    /// trusted backend proxy explicitly owns network exposure.
    pub bind_addr: SocketAddr,
    /// Optional explicit Runtime authority id. If omitted, the Runtime library
    /// generates one.
    pub runtime_id: Option<RuntimeId>,
    /// Optional display label surfaced by `GET /v1/runtime`.
    pub display_name: Option<String>,
    /// Bounded Runtime API limits.
    pub limits: RuntimeLimits,
    /// v0 store selection for the Runtime process.
    pub store: RuntimeHttpStoreSelection,
    /// Minimal local bearer token placeholder for backend-to-Runtime calls.
    /// This is not a browser-facing credential model.
    pub local_token: Option<String>,
}

impl Default for RuntimeHttpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_runtime_http_bind_addr(),
            runtime_id: None,
            display_name: None,
            limits: RuntimeLimits::default(),
            store: RuntimeHttpStoreSelection::Memory,
            local_token: None,
        }
    }
}

impl fmt::Debug for RuntimeHttpServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHttpServerConfig")
            .field("bind_addr", &self.bind_addr)
            .field("runtime_id", &self.runtime_id)
            .field("display_name", &self.display_name)
            .field("limits", &self.limits)
            .field("store", &self.store)
            .field(
                "local_token",
                &self.local_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// v0 Runtime store selection for the REST process adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeHttpStoreSelection {
    Memory,
    /// Filesystem-backed Runtime store. Available only when `fs-store` is also
    /// enabled; no new persistence model is introduced by the REST adapter.
    #[cfg(feature = "fs-store")]
    Fs {
        root: PathBuf,
    },
}

/// Serve an existing Runtime on a pre-bound listener.
pub async fn serve_runtime_http(
    runtime: Runtime,
    listener: TcpListener,
    local_token: Option<String>,
) -> Result<(), RuntimeHttpServerError> {
    axum::serve(listener, runtime_http_router(runtime, local_token)).await?;
    Ok(())
}

/// Build the REST router for an existing Runtime.
///
/// Handlers delegate to [`Runtime`] methods and keep Worker authority as
/// `(runtime_id, worker_id)`. The path contains only a Runtime-local
/// `worker_id`; the server supplies its own Runtime id instead of accepting a
/// legacy pod/socket/session path as authority.
pub fn runtime_http_router(runtime: Runtime, local_token: Option<String>) -> Router {
    let state = RuntimeHttpState {
        runtime,
        local_token: local_token.map(Arc::<str>::from),
    };

    let router = Router::new()
        .route("/v1/runtime", get(get_runtime))
        .route(
            "/v1/config-bundles",
            get(list_config_bundles).post(store_config_bundle),
        )
        .route(
            "/v1/config-bundles/{bundle_id}/availability",
            get(check_config_bundle),
        )
        .route(
            "/v1/working-directories",
            get(list_working_directories).post(create_working_directory),
        )
        .route(
            "/v1/working-directories/{working_directory_id}",
            get(get_working_directory).delete(cleanup_working_directory),
        )
        .route("/v1/workers", get(list_workers).post(create_worker))
        .route("/v1/workers/{worker_id}", get(get_worker))
        .route("/v1/workers/{worker_id}/input", post(send_worker_input))
        .route("/v1/workers/{worker_id}/stop", post(stop_worker))
        .route("/v1/workers/{worker_id}/cancel", post(cancel_worker))
        .route(
            "/v1/workers/{worker_id}/transcript",
            get(get_worker_transcript),
        );

    #[cfg(feature = "ws-server")]
    let router = router.route("/v1/workers/{worker_id}/events/ws", get(worker_events_ws));

    router
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, require_local_token))
}

#[derive(Clone)]
struct RuntimeHttpState {
    runtime: Runtime,
    local_token: Option<Arc<str>>,
}

/// `GET /v1/runtime` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpSummaryResponse {
    pub runtime: RuntimeSummary,
}

/// `GET /v1/config-bundles` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpConfigBundlesResponse {
    pub bundles: Vec<ConfigBundleSummary>,
}

/// `POST /v1/config-bundles` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpConfigBundleSyncRequest {
    pub bundle: ConfigBundle,
}

/// Config bundle availability response used by sync/check endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpConfigBundleAvailabilityResponse {
    pub availability: ConfigBundleAvailability,
}

#[derive(Clone, Debug, Deserialize)]
struct RuntimeHttpConfigBundleAvailabilityQuery {
    digest: String,
}

/// `GET /v1/workers` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkersResponse {
    pub workers: Vec<WorkerSummary>,
}

/// `GET /v1/working-directories` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkingDirectoriesResponse {
    pub working_directories: Vec<WorkingDirectoryStatus>,
}

/// Working directory response used by create/detail/delete endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkingDirectoryResponse {
    pub working_directory: WorkingDirectoryStatus,
}

/// Worker detail response used by create/detail endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerResponse {
    pub worker: WorkerDetail,
}

/// Worker input acknowledgement response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerInputResponse {
    pub ack: WorkerInteractionAck,
}

/// Worker lifecycle request body used by stop/cancel endpoints.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerLifecycleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Worker lifecycle acknowledgement response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerLifecycleResponse {
    pub ack: WorkerLifecycleAck,
}

/// `GET /v1/workers/{worker_id}/transcript` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpTranscriptResponse {
    pub transcript: TranscriptProjection,
}

/// Typed REST error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpErrorResponse {
    pub error: RuntimeHttpErrorDetail,
}

/// Typed REST error payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpErrorDetail {
    pub code: String,
    pub message: String,
}

/// Runtime-owned WebSocket frame for worker-scoped observation.
#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeWorkerEventWsFrame {
    Event {
        envelope: RuntimeWorkerEventWsEnvelope,
    },
    Diagnostic {
        diagnostic: RuntimeWorkerEventWsDiagnostic,
    },
}

/// Runtime-local protocol event envelope.
#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeWorkerEventWsEnvelope {
    pub cursor: String,
    pub event_id: String,
    pub worker_id: WorkerId,
    pub payload: protocol::Event,
}

/// Runtime-local observation diagnostic.
#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeWorkerEventWsDiagnostic {
    pub code: String,
    pub message: String,
}

#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, Default, Deserialize)]
struct RuntimeWorkerEventsWsQuery {
    cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RuntimeHttpTranscriptQuery {
    #[serde(default)]
    start: usize,
    #[serde(default = "default_transcript_limit")]
    limit: usize,
}

fn default_transcript_limit() -> usize {
    256
}

#[cfg(feature = "ws-server")]
impl RuntimeWorkerEventWsFrame {
    fn event(
        cursor: String,
        event_id: String,
        worker_id: WorkerId,
        payload: protocol::Event,
    ) -> Self {
        Self::Event {
            envelope: RuntimeWorkerEventWsEnvelope {
                cursor,
                event_id,
                worker_id,
                payload,
            },
        }
    }

    fn diagnostic(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Diagnostic {
            diagnostic: RuntimeWorkerEventWsDiagnostic {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

#[cfg(feature = "ws-server")]
async fn send_ws_frame(socket: &mut WebSocket, frame: &RuntimeWorkerEventWsFrame) -> bool {
    match serde_json::to_string(frame) {
        Ok(text) => socket.send(WsMessage::Text(text.into())).await.is_ok(),
        Err(error) => {
            let fallback = RuntimeWorkerEventWsFrame::diagnostic(
                "runtime.serialize_failed",
                format!("failed to serialize observation frame: {error}"),
            );
            let Ok(text) = serde_json::to_string(&fallback) else {
                return false;
            };
            socket.send(WsMessage::Text(text.into())).await.is_ok()
        }
    }
}

type RestResult<T> = Result<Json<T>, RuntimeHttpRestError>;

async fn get_runtime(
    State(state): State<RuntimeHttpState>,
) -> RestResult<RuntimeHttpSummaryResponse> {
    let runtime = state
        .runtime
        .summary()
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpSummaryResponse { runtime }))
}

async fn list_config_bundles(
    State(state): State<RuntimeHttpState>,
) -> RestResult<RuntimeHttpConfigBundlesResponse> {
    let bundles = state
        .runtime
        .list_config_bundles()
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpConfigBundlesResponse { bundles }))
}

async fn store_config_bundle(
    State(state): State<RuntimeHttpState>,
    body: Result<Json<RuntimeHttpConfigBundleSyncRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpConfigBundleAvailabilityResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let availability = state
        .runtime
        .store_config_bundle(request.bundle)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpConfigBundleAvailabilityResponse {
        availability,
    }))
}

async fn check_config_bundle(
    State(state): State<RuntimeHttpState>,
    Path(bundle_id): Path<String>,
    query: Result<Query<RuntimeHttpConfigBundleAvailabilityQuery>, QueryRejection>,
) -> RestResult<RuntimeHttpConfigBundleAvailabilityResponse> {
    let Query(query) = query.map_err(RuntimeHttpRestError::query_rejection)?;
    let availability = state
        .runtime
        .check_config_bundle(&ConfigBundleRef {
            id: bundle_id,
            digest: query.digest,
        })
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpConfigBundleAvailabilityResponse {
        availability,
    }))
}

async fn list_workers(
    State(state): State<RuntimeHttpState>,
) -> RestResult<RuntimeHttpWorkersResponse> {
    let workers = state
        .runtime
        .list_workers()
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkersResponse { workers }))
}

async fn list_working_directories(
    State(state): State<RuntimeHttpState>,
) -> RestResult<RuntimeHttpWorkingDirectoriesResponse> {
    let working_directories = state
        .runtime
        .list_working_directories()
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkingDirectoriesResponse {
        working_directories,
    }))
}

async fn create_working_directory(
    State(state): State<RuntimeHttpState>,
    body: Result<Json<WorkingDirectoryRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkingDirectoryResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let working_directory = state
        .runtime
        .create_working_directory(request)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkingDirectoryResponse {
        working_directory,
    }))
}

async fn get_working_directory(
    State(state): State<RuntimeHttpState>,
    Path(working_directory_id): Path<String>,
) -> RestResult<RuntimeHttpWorkingDirectoryResponse> {
    let working_directory = state
        .runtime
        .working_directory(&working_directory_id)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkingDirectoryResponse {
        working_directory,
    }))
}

async fn cleanup_working_directory(
    State(state): State<RuntimeHttpState>,
    Path(working_directory_id): Path<String>,
) -> RestResult<RuntimeHttpWorkingDirectoryResponse> {
    let working_directory = state
        .runtime
        .cleanup_working_directory(&working_directory_id)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkingDirectoryResponse {
        working_directory,
    }))
}

async fn get_worker(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let worker = state
        .runtime
        .worker_detail(&worker_ref)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

async fn create_worker(
    State(state): State<RuntimeHttpState>,
    body: Result<Json<CreateWorkerRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let worker = state
        .runtime
        .create_worker(request)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

#[cfg(feature = "ws-server")]
async fn worker_events_ws(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
    Query(query): Query<RuntimeWorkerEventsWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, RuntimeHttpRestError> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    state
        .runtime
        .worker_detail(&worker_ref)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(ws
        .on_upgrade(move |socket| {
            worker_events_ws_session(state.runtime, worker_ref, query, socket)
        })
        .into_response())
}

#[cfg(feature = "ws-server")]
async fn worker_events_ws_session(
    runtime: Runtime,
    worker_ref: WorkerRef,
    query: RuntimeWorkerEventsWsQuery,
    mut socket: WebSocket,
) {
    let mut cursor = match query.cursor.as_deref() {
        Some(raw) => match WorkerObservationCursor::decode(raw) {
            Some(cursor) => cursor,
            None => {
                let frame = RuntimeWorkerEventWsFrame::diagnostic(
                    "runtime.cursor_malformed",
                    format!("malformed worker observation cursor: {raw}"),
                );
                let _ = send_ws_frame(&mut socket, &frame).await;
                return;
            }
        },
        None => match runtime.worker_observation_cursor_now(&worker_ref) {
            Ok(cursor) => cursor,
            Err(error) => {
                let frame = RuntimeWorkerEventWsFrame::diagnostic(
                    "runtime.worker_not_found",
                    error.to_string(),
                );
                let _ = send_ws_frame(&mut socket, &frame).await;
                return;
            }
        },
    };

    let mut receiver = match runtime.subscribe_worker_observation() {
        Ok(receiver) => receiver,
        Err(error) => {
            let frame = RuntimeWorkerEventWsFrame::diagnostic(
                "runtime.unavailable",
                format!("runtime observation bus unavailable: {error}"),
            );
            let _ = send_ws_frame(&mut socket, &frame).await;
            return;
        }
    };

    let snapshot = match runtime.worker_observation_snapshot(&worker_ref) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let frame = RuntimeWorkerEventWsFrame::diagnostic(
                "runtime.worker_not_found",
                error.to_string(),
            );
            let _ = send_ws_frame(&mut socket, &frame).await;
            return;
        }
    };
    let snapshot_cursor = cursor.encode();
    let snapshot_frame = RuntimeWorkerEventWsFrame::event(
        snapshot_cursor.clone(),
        format!("snapshot:{snapshot_cursor}"),
        worker_ref.worker_id.clone(),
        snapshot,
    );
    if !send_ws_frame(&mut socket, &snapshot_frame).await {
        return;
    }

    match runtime.read_worker_observation_events(&worker_ref, cursor) {
        Ok(backlog) => {
            for event in backlog {
                cursor = WorkerObservationCursor::new(event.sequence);
                let frame = RuntimeWorkerEventWsFrame::event(
                    event.cursor,
                    event.event_id,
                    event.worker_ref.worker_id,
                    event.payload,
                );
                if !send_ws_frame(&mut socket, &frame).await {
                    return;
                }
            }
        }
        Err(error) => {
            let frame = RuntimeWorkerEventWsFrame::diagnostic(
                "runtime.cursor_unknown_or_expired",
                error.to_string(),
            );
            let _ = send_ws_frame(&mut socket, &frame).await;
            return;
        }
    }

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
                        let frame = RuntimeWorkerEventWsFrame::diagnostic(
                            "runtime.observation_only",
                            "runtime worker event WebSocket is observation-only",
                        );
                        let _ = send_ws_frame(&mut socket, &frame).await;
                        return;
                    }
                    Some(Err(error)) => {
                        let frame = RuntimeWorkerEventWsFrame::diagnostic(
                            "runtime.websocket_error",
                            format!("runtime WebSocket receive error: {error}"),
                        );
                        let _ = send_ws_frame(&mut socket, &frame).await;
                        return;
                    }
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) if event.worker_ref == worker_ref && event.sequence > cursor.sequence => {
                        cursor = WorkerObservationCursor::new(event.sequence);
                        let frame = RuntimeWorkerEventWsFrame::event(
                            event.cursor,
                            event.event_id,
                            event.worker_ref.worker_id,
                            event.payload,
                        );
                        if !send_ws_frame(&mut socket, &frame).await {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let frame = RuntimeWorkerEventWsFrame::diagnostic(
                            "runtime.cursor_expired",
                            "runtime observation backlog was overrun",
                        );
                        let _ = send_ws_frame(&mut socket, &frame).await;
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let frame = RuntimeWorkerEventWsFrame::diagnostic(
                            "runtime.upstream_closed",
                            "runtime observation bus closed",
                        );
                        let _ = send_ws_frame(&mut socket, &frame).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn send_worker_input(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerInput>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerInputResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let Json(input) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let ack = state
        .runtime
        .send_input(&worker_ref, input)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerInputResponse { ack }))
}

async fn stop_worker(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
    body: Bytes,
) -> RestResult<RuntimeHttpWorkerLifecycleResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let request = parse_optional_lifecycle_request(body)?;
    let ack = state
        .runtime
        .stop_worker(&worker_ref, request.reason)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerLifecycleResponse { ack }))
}

async fn cancel_worker(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
    body: Bytes,
) -> RestResult<RuntimeHttpWorkerLifecycleResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let request = parse_optional_lifecycle_request(body)?;
    let ack = state
        .runtime
        .cancel_worker(&worker_ref, request.reason)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerLifecycleResponse { ack }))
}

async fn get_worker_transcript(
    State(state): State<RuntimeHttpState>,
    Path(worker_id): Path<String>,
    query: Result<Query<RuntimeHttpTranscriptQuery>, QueryRejection>,
) -> RestResult<RuntimeHttpTranscriptResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let Query(query) = query.map_err(RuntimeHttpRestError::query_rejection)?;
    let transcript = state
        .runtime
        .transcript_projection(&worker_ref, TranscriptQuery::new(query.start, query.limit))
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpTranscriptResponse { transcript }))
}

fn worker_ref_for(runtime: &Runtime, worker_id: String) -> Result<WorkerRef, RuntimeHttpRestError> {
    let worker_id = WorkerId::new(worker_id).ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "invalid_worker_id",
            "worker_id must not be empty",
        )
    })?;
    let runtime_id = runtime
        .runtime_id()
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(WorkerRef::new(runtime_id, worker_id))
}

fn parse_optional_lifecycle_request(
    body: Bytes,
) -> Result<RuntimeHttpWorkerLifecycleRequest, RuntimeHttpRestError> {
    if body.is_empty() {
        return Ok(RuntimeHttpWorkerLifecycleRequest::default());
    }
    serde_json::from_slice(&body).map_err(|error| {
        RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            format!("invalid lifecycle request JSON: {error}"),
        )
    })
}

async fn require_local_token(
    State(state): State<RuntimeHttpState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Some(expected) = state.local_token.as_deref() {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied != Some(expected) {
            return RuntimeHttpRestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid local Runtime bearer token",
            )
            .into_response();
        }
    }
    next.run(request).await
}

#[derive(Debug)]
struct RuntimeHttpRestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl RuntimeHttpRestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn runtime(error: RuntimeError) -> Self {
        let status = status_for_runtime_error(&error);
        let code = code_for_runtime_error(&error);
        Self::new(status, code, error.to_string())
    }

    fn json_rejection(error: JsonRejection) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            format!("invalid JSON request body: {error}"),
        )
    }

    fn query_rejection(error: QueryRejection) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            format!("invalid query parameters: {error}"),
        )
    }
}

impl IntoResponse for RuntimeHttpRestError {
    fn into_response(self) -> Response {
        let body = RuntimeHttpErrorResponse {
            error: RuntimeHttpErrorDetail {
                code: self.code.to_string(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

fn status_for_runtime_error(error: &RuntimeError) -> StatusCode {
    match error {
        RuntimeError::WorkerNotFound { .. } | RuntimeError::ConfigBundleMissing { .. } => {
            StatusCode::NOT_FOUND
        }
        RuntimeError::RuntimeStopped { .. }
        | RuntimeError::WorkerExecutionUnavailable { .. }
        | RuntimeError::ExecutionBackendUnavailable { .. }
        | RuntimeError::WorkerExecutionRejected { .. } => StatusCode::CONFLICT,
        RuntimeError::LimitTooLarge { .. }
        | RuntimeError::InvalidRequest(_)
        | RuntimeError::InvalidInitialInputKind { .. }
        | RuntimeError::ConfigBundleDigestMismatch { .. }
        | RuntimeError::InvalidProfileSelector { .. }
        | RuntimeError::UnsupportedConfigDeclaration { .. }
        | RuntimeError::WrongRuntime { .. }
        | RuntimeError::WrongRuntimeCursor { .. } => StatusCode::BAD_REQUEST,
        RuntimeError::StoreIo { .. }
        | RuntimeError::StoreMissing { .. }
        | RuntimeError::StoreCorrupt { .. }
        | RuntimeError::StatePoisoned => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn code_for_runtime_error(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::RuntimeStopped { .. } => "runtime_stopped",
        RuntimeError::WrongRuntime { .. } => "wrong_runtime",
        RuntimeError::WrongRuntimeCursor { .. } => "wrong_runtime_cursor",
        RuntimeError::WorkerNotFound { .. } => "worker_not_found",
        RuntimeError::WorkerExecutionUnavailable { .. } => "worker_execution_unavailable",
        RuntimeError::ExecutionBackendUnavailable { .. } => "execution_backend_unavailable",
        RuntimeError::WorkerExecutionRejected { .. } => "worker_execution_rejected",
        RuntimeError::LimitTooLarge { .. } => "limit_too_large",
        RuntimeError::InvalidRequest(_) => "invalid_request",
        RuntimeError::InvalidInitialInputKind { .. } => "invalid_initial_input_kind",
        RuntimeError::ConfigBundleMissing { .. } => "config_bundle_missing",
        RuntimeError::ConfigBundleDigestMismatch { .. } => "config_bundle_digest_mismatch",
        RuntimeError::InvalidProfileSelector { .. } => "invalid_profile_selector",
        RuntimeError::UnsupportedConfigDeclaration { .. } => "unsupported_config_declaration",
        RuntimeError::StoreIo { .. } => "store_io",
        RuntimeError::StoreMissing { .. } => "store_missing",
        RuntimeError::StoreCorrupt { .. } => "store_corrupt",
        RuntimeError::StatePoisoned => "state_poisoned",
    }
}

/// Errors raised while building or serving the Runtime REST process API.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeHttpServerError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("Runtime HTTP server I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ConfigBundleRef, ProfileSelector};
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
        WorkerExecutionResult, WorkerExecutionRunState, WorkerExecutionSpawnRequest,
        WorkerExecutionSpawnResult,
    };
    use crate::management::RuntimeOptions;
    use axum::body::to_bytes;
    use axum::http::Method;
    use tower::ServiceExt;

    fn test_bundle(profile: ProfileSelector) -> ConfigBundle {
        ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "http-test-bundle".to_string(),
                digest: String::new(),
                revision: "test".to_string(),
                workspace_id: "test-workspace".to_string(),
                created_at: "test".to_string(),
                provenance: ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![ConfigProfileDescriptor {
                selector: profile,
                label: Some("test".to_string()),
            }],
            declarations: Vec::new(),
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn task_request(_objective: &str) -> CreateWorkerRequest {
        let profile = ProfileSelector::Builtin("builtin:coder".to_string());
        let bundle = test_bundle(profile.clone());
        CreateWorkerRequest {
            profile,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Http {
                location: crate::catalog::ProfileSourceArchiveHttpRef {
                    url: "http://127.0.0.1/profile-source.tar".to_string(),
                    etag: None,
                    archive: crate::profile_archive::ProfileSourceArchiveRef {
                        id: "test-profile-source".to_string(),
                        digest: "test-digest".to_string(),
                        size_bytes: 0,
                        source_graph: crate::profile_archive::ProfileSourceGraphSummary {
                            source_count: 0,
                            total_source_bytes: 0,
                            entrypoints: std::collections::BTreeMap::new(),
                            import_count: 0,
                        },
                    },
                },
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            working_directory_request: None,
            working_directory: None,
        }
    }

    struct AcceptingBackend;

    impl WorkerExecutionBackend for AcceptingBackend {
        fn backend_id(&self) -> &str {
            "http-test"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request
                    .working_directory
                    .as_ref()
                    .map(|binding| binding.status()),
            }
        }

        fn dispatch_input(
            &self,
            _handle: &WorkerExecutionHandle,
            _input: WorkerInput,
        ) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Input,
                WorkerExecutionRunState::Idle,
            )
        }

        fn stop_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Stop,
                WorkerExecutionRunState::Stopped,
            )
        }
    }

    async fn json_request<T: Serialize>(
        app: Router,
        method: Method,
        uri: &str,
        body: &T,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn empty_request(app: Router, method: Method, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn read_json<T: for<'de> Deserialize<'de>>(response: Response) -> T {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn rest_command_api_delegates_to_runtime() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        runtime
            .store_config_bundle(test_bundle(ProfileSelector::Builtin(
                "builtin:coder".to_string(),
            )))
            .unwrap();
        let app = runtime_http_router(runtime.clone(), None);

        let response = json_request(
            app.clone(),
            Method::POST,
            "/v1/workers",
            &task_request("rest"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let created: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(
            created.worker.worker_ref.runtime_id,
            runtime.runtime_id().unwrap()
        );

        let input = WorkerInput::user("hello from backend");
        let response = json_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/input", created.worker.worker_id),
            &input,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let input_ack: RuntimeHttpWorkerInputResponse = read_json(response).await;
        assert_eq!(input_ack.ack.transcript_sequence, 1);

        let response = empty_request(
            app.clone(),
            Method::GET,
            &format!("/v1/workers/{}", created.worker.worker_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let detail: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(detail.worker.transcript_len, 1);

        let response = empty_request(
            app.clone(),
            Method::GET,
            &format!(
                "/v1/workers/{}/transcript?start=0&limit=1",
                created.worker.worker_id
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let transcript: RuntimeHttpTranscriptResponse = read_json(response).await;
        assert_eq!(transcript.transcript.items[0].content, "hello from backend");

        let response = empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/stop", created.worker.worker_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let stop: RuntimeHttpWorkerLifecycleResponse = read_json(response).await;
        assert_eq!(stop.ack.worker_ref, created.worker.worker_ref);

        let response = empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/cancel", created.worker.worker_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cancel: RuntimeHttpWorkerLifecycleResponse = read_json(response).await;
        assert_eq!(cancel.ack.worker_ref, created.worker.worker_ref);

        let response = empty_request(app.clone(), Method::GET, "/v1/workers").await;
        assert_eq!(response.status(), StatusCode::OK);
        let workers: RuntimeHttpWorkersResponse = read_json(response).await;
        assert_eq!(workers.workers.len(), 1);
        assert_eq!(workers.workers[0].transcript_len, 1);

        let response = empty_request(app, Method::GET, "/v1/runtime").await;
        assert_eq!(response.status(), StatusCode::OK);
        let summary: RuntimeHttpSummaryResponse = read_json(response).await;
        assert_eq!(summary.runtime.worker_count, 1);
        assert_eq!(summary.runtime.stopped_worker_count, 1);
    }

    #[tokio::test]
    async fn local_token_placeholder_rejects_missing_bearer_token() {
        let app = runtime_http_router(Runtime::new_memory(), Some("local-token".to_string()));

        let response = empty_request(app.clone(), Method::GET, "/v1/runtime").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "unauthorized");

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/runtime")
                    .header(header::AUTHORIZATION, "Bearer local-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn runtime_errors_use_typed_rest_error_shape() {
        let app = runtime_http_router(Runtime::new_memory(), None);
        let response = empty_request(app, Method::GET, "/v1/workers/worker-missing").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "worker_not_found");
        assert!(error.error.message.contains("worker-missing"));
    }
}

#[cfg(all(test, feature = "ws-server"))]
mod ws_tests {
    use super::*;
    use crate::catalog::{ConfigBundleRef, ProfileSelector};
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
        WorkerExecutionResult, WorkerExecutionRunState, WorkerExecutionSpawnRequest,
        WorkerExecutionSpawnResult,
    };
    use crate::management::RuntimeOptions;
    use futures::{SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    struct WsBackend;

    impl WorkerExecutionBackend for WsBackend {
        fn backend_id(&self) -> &str {
            "ws-test"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request
                    .working_directory
                    .as_ref()
                    .map(|binding| binding.status()),
            }
        }

        fn dispatch_input(
            &self,
            _handle: &WorkerExecutionHandle,
            _input: WorkerInput,
        ) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::Input,
                WorkerExecutionRunState::Idle,
            )
        }
    }

    fn ws_test_bundle(profile: ProfileSelector) -> ConfigBundle {
        ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "ws-test-bundle".to_string(),
                digest: String::new(),
                revision: "test".to_string(),
                workspace_id: "test".to_string(),
                created_at: "test".to_string(),
                provenance: ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![ConfigProfileDescriptor {
                selector: profile,
                label: Some("ws".to_string()),
            }],
            declarations: Vec::new(),
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn ws_create_request() -> CreateWorkerRequest {
        let bundle = ws_test_bundle(ProfileSelector::RuntimeDefault);
        CreateWorkerRequest {
            profile: ProfileSelector::RuntimeDefault,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Http {
                location: crate::catalog::ProfileSourceArchiveHttpRef {
                    url: "http://127.0.0.1/profile-source.tar".to_string(),
                    etag: None,
                    archive: crate::profile_archive::ProfileSourceArchiveRef {
                        id: "test-profile-source".to_string(),
                        digest: "test-digest".to_string(),
                        size_bytes: 0,
                        source_graph: crate::profile_archive::ProfileSourceGraphSummary {
                            source_count: 0,
                            total_source_bytes: 0,
                            entrypoints: std::collections::BTreeMap::new(),
                            import_count: 0,
                        },
                    },
                },
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            working_directory_request: None,
            working_directory: None,
        }
    }

    async fn spawn_runtime_server() -> (Runtime, WorkerRef, String) {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(WsBackend))
                .unwrap();
        runtime
            .store_config_bundle(ws_test_bundle(ProfileSelector::RuntimeDefault))
            .unwrap();
        let worker = runtime.create_worker(ws_create_request()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move { serve_runtime_http(runtime, listener, None).await.unwrap() }
        });
        (
            runtime,
            worker.worker_ref.clone(),
            format!(
                "ws://{addr}/v1/workers/{}/events/ws",
                worker.worker_ref.worker_id
            ),
        )
    }

    async fn next_frame(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> RuntimeWorkerEventWsFrame {
        let message = stream.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn runtime_ws_connect_sends_snapshot_and_live_worker_events() {
        let (runtime, worker_ref, url) = spawn_runtime_server().await;
        let (mut stream, _) = connect_async(&url).await.unwrap();

        match next_frame(&mut stream).await {
            RuntimeWorkerEventWsFrame::Event { envelope } => {
                assert_eq!(envelope.worker_id, worker_ref.worker_id);
                assert!(matches!(envelope.payload, protocol::Event::Snapshot { .. }));
            }
            RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                panic!("unexpected diagnostic: {diagnostic:?}");
            }
        }

        let stored = runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDelta {
                    text: "started".into(),
                },
            )
            .unwrap();
        match next_frame(&mut stream).await {
            RuntimeWorkerEventWsFrame::Event { envelope } => {
                assert_eq!(envelope.worker_id, worker_ref.worker_id);
                assert_eq!(envelope.cursor, stored.cursor);
                assert!(matches!(
                    envelope.payload,
                    protocol::Event::TextDelta { .. }
                ));
            }
            RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                panic!("unexpected diagnostic: {diagnostic:?}");
            }
        }
    }

    #[tokio::test]
    async fn runtime_ws_cursor_resume_is_duplicate_safe_and_filters_workers() {
        let (runtime, worker_ref, url) = spawn_runtime_server().await;
        let other = runtime.create_worker(ws_create_request()).unwrap();
        let first = runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDelta {
                    text: "started".into(),
                },
            )
            .unwrap();
        runtime
            .observe_worker_event(
                &other.worker_ref,
                protocol::Event::TextDelta {
                    text: "started".into(),
                },
            )
            .unwrap();

        let (mut stream, _) = connect_async(format!("{url}?cursor={}", first.cursor))
            .await
            .unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            RuntimeWorkerEventWsFrame::Event { envelope } if matches!(envelope.payload, protocol::Event::Snapshot { .. })
        ));

        let second = runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDone {
                    text: "done".into(),
                },
            )
            .unwrap();
        match next_frame(&mut stream).await {
            RuntimeWorkerEventWsFrame::Event { envelope } => {
                assert_eq!(envelope.cursor, second.cursor);
                assert_ne!(envelope.cursor, first.cursor);
                assert!(matches!(envelope.payload, protocol::Event::TextDone { .. }));
            }
            RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                panic!("unexpected diagnostic: {diagnostic:?}");
            }
        }
    }

    #[tokio::test]
    async fn runtime_ws_reports_malformed_cursor_and_observation_only_input() {
        let (_runtime, _worker_ref, url) = spawn_runtime_server().await;
        let (mut malformed, _) = connect_async(format!("{url}?cursor=bad")).await.unwrap();
        match next_frame(&mut malformed).await {
            RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                assert_eq!(diagnostic.code, "runtime.cursor_malformed");
            }
            RuntimeWorkerEventWsFrame::Event { envelope } => {
                panic!("unexpected event: {envelope:?}");
            }
        }

        let (mut stream, _) = connect_async(&url).await.unwrap();
        let _ = next_frame(&mut stream).await;
        stream.send(Message::Text("{}".into())).await.unwrap();
        match next_frame(&mut stream).await {
            RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                assert_eq!(diagnostic.code, "runtime.observation_only");
            }
            RuntimeWorkerEventWsFrame::Event { envelope } => {
                panic!("unexpected event: {envelope:?}");
            }
        }
    }
}
