//! Optional REST process adapter for the Runtime command API.
//!
//! This module is intentionally gated by the `http-server` feature so embedded
//! Runtime users do not pull HTTP dependencies.  The server is a process-local
//! command surface for a trusted backend/proxy. Browsers must not connect to the
//! Runtime process directly; a backend is expected to own any browser-facing
//! credentials, registration, and policy.

use crate::auth::{
    RuntimeAuthContext, RuntimeAuthError, RuntimeHttpAuthConfig, unix_now_seconds,
    verify_capability_token,
};
use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, WorkerDetail, WorkerLifecycleAck, WorkerSummary,
    WorkingDirectoryRequest, WorkingDirectoryStatus,
};
use crate::config_bundle::{ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary};
use crate::error::RuntimeError;
use crate::identity::{WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInteractionAck};
use crate::management::{RuntimeLimits, RuntimeSummary, WorkerDeleteResult};
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationCursor;
use crate::{Runtime, RuntimeWorkspaceScope};
use axum::body::{Body, Bytes};
use axum::extract::rejection::{JsonRejection, QueryRejection};
#[cfg(feature = "ws-server")]
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{Method, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
#[cfg(feature = "ws-server")]
use futures::StreamExt;
#[cfg(feature = "ws-server")]
use protocol::stream::{decode_method, encode_event};
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
    /// Optional display label surfaced by `GET /v1/runtime`.
    pub display_name: Option<String>,
    /// Bounded Runtime API limits.
    pub limits: RuntimeLimits,
    /// v0 store selection for the Runtime process.
    pub store: RuntimeHttpStoreSelection,
    /// Minimal local bearer token placeholder for backend-to-Runtime calls.
    /// This is not a browser-facing credential model.
    pub local_token: Option<String>,
    /// Optional signed Server-to-Runtime capability token authority.
    pub auth: Option<RuntimeHttpAuthConfig>,
}

impl Default for RuntimeHttpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_runtime_http_bind_addr(),
            display_name: None,
            limits: RuntimeLimits::default(),
            store: RuntimeHttpStoreSelection::Memory,
            local_token: None,
            auth: None,
        }
    }
}

impl fmt::Debug for RuntimeHttpServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHttpServerConfig")
            .field("bind_addr", &self.bind_addr)
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
    let local_token = local_token.ok_or(RuntimeHttpServerError::AuthRequired)?;
    axum::serve(listener, runtime_http_router(runtime, local_token)).await?;
    Ok(())
}

/// Serve an existing Runtime on a pre-bound listener with signed capability-token auth.
pub async fn serve_runtime_http_with_auth(
    runtime: Runtime,
    listener: TcpListener,
    local_token: Option<String>,
    auth: Option<RuntimeHttpAuthConfig>,
) -> Result<(), RuntimeHttpServerError> {
    if local_token.is_none() && auth.is_none() {
        return Err(RuntimeHttpServerError::AuthRequired);
    }
    axum::serve(
        listener,
        runtime_http_router_with_optional_auth(runtime, local_token, auth),
    )
    .await?;
    Ok(())
}

/// Build the REST router for an existing Runtime.
///
/// Handlers delegate to [`Runtime`] methods and keep Worker authority Runtime-local.
/// The path contains only a Runtime-local `worker_id`; backend aliases are not
/// accepted or forwarded as Runtime authority.
pub fn runtime_http_router(runtime: Runtime, local_token: String) -> Router {
    runtime_http_router_with_optional_auth(runtime, Some(local_token), None)
}

/// Build the REST router for an existing Runtime with signed capability-token auth.
pub fn runtime_http_router_with_auth(
    runtime: Runtime,
    local_token: Option<String>,
    auth: RuntimeHttpAuthConfig,
) -> Router {
    runtime_http_router_with_optional_auth(runtime, local_token, Some(auth))
}

fn runtime_http_router_with_optional_auth(
    runtime: Runtime,
    local_token: Option<String>,
    auth: Option<RuntimeHttpAuthConfig>,
) -> Router {
    let state = RuntimeHttpState {
        runtime,
        local_token: local_token.map(Arc::<str>::from),
        auth: auth.map(Arc::new),
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
        .route(
            "/v1/workers/{worker_id}",
            get(get_worker).delete(delete_worker),
        )
        .route("/v1/workers/{worker_id}/input", post(send_worker_input))
        .route("/v1/workers/{worker_id}/restore", post(restore_worker))
        .route(
            "/v1/workers/{worker_id}/completions",
            post(worker_completions),
        )
        .route("/v1/workers/{worker_id}/stop", post(stop_worker))
        .route("/v1/workers/{worker_id}/cancel", post(cancel_worker));

    #[cfg(feature = "ws-server")]
    let router = router.route(
        "/v1/workers/{worker_id}/protocol/ws",
        get(worker_protocol_ws),
    );

    router
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, require_runtime_auth))
}

#[derive(Clone)]
struct RuntimeHttpState {
    runtime: Runtime,
    local_token: Option<Arc<str>>,
    auth: Option<Arc<RuntimeHttpAuthConfig>>,
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

#[derive(Clone, Debug, Default, Deserialize)]
struct RuntimeHttpWorkersQuery {
    status: Option<RuntimeHttpWorkerStatusFilter>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeHttpWorkerStatusFilter {
    Stopped,
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

/// Worker delete response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerDeleteResponse {
    pub worker: WorkerDeleteResult,
}

/// Worker input acknowledgement response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerInputResponse {
    pub ack: WorkerInteractionAck,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerCompletionsRequest {
    pub kind: protocol::CompletionKind,
    #[serde(default)]
    pub prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerCompletionsResponse {
    pub kind: protocol::CompletionKind,
    pub prefix: String,
    pub entries: Vec<protocol::CompletionEntry>,
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

#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, Default, Deserialize)]
struct RuntimeWorkerEventsWsQuery {
    cursor: Option<String>,
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
    auth: Option<Extension<RuntimeAuthContext>>,
    query: Result<Query<RuntimeHttpWorkersQuery>, QueryRejection>,
) -> RestResult<RuntimeHttpWorkersResponse> {
    let Query(query) = query.map_err(RuntimeHttpRestError::query_rejection)?;
    let scope = auth_workspace_scope(&state, auth.as_ref())?;
    let workers = match (query.status, scope.as_ref()) {
        (Some(RuntimeHttpWorkerStatusFilter::Stopped), Some(scope)) => {
            state.runtime.list_stopped_workers_scoped(scope)
        }
        (Some(RuntimeHttpWorkerStatusFilter::Stopped), None) => {
            state.runtime.list_stopped_workers()
        }
        (None, Some(scope)) => state.runtime.list_workers_scoped(scope),
        (None, None) => state.runtime.list_workers(),
    }
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
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let worker = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.worker_detail_scoped(&scope, &worker_ref),
        None => state.runtime.worker_detail(&worker_ref),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

async fn delete_worker(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
) -> RestResult<RuntimeHttpWorkerDeleteResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let worker = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.delete_worker_scoped(&scope, &worker_ref),
        None => state.runtime.delete_worker(&worker_ref),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerDeleteResponse { worker }))
}

async fn create_worker(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    body: Result<Json<CreateWorkerRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let worker = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.create_worker_scoped(&scope, request),
        None => state.runtime.create_worker(request),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

async fn restore_worker(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let worker = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.restore_worker_scoped(&scope, &worker_ref),
        None => state.runtime.restore_worker(&worker_ref),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

#[cfg(feature = "ws-server")]
async fn worker_protocol_ws(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    Query(query): Query<RuntimeWorkerEventsWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, RuntimeHttpRestError> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let scope = auth_workspace_scope(&state, auth.as_ref())?;
    match scope.as_ref() {
        Some(scope) => state
            .runtime
            .worker_detail_scoped(scope, &worker_ref)
            .map(|_| ()),
        None => state.runtime.worker_detail(&worker_ref).map(|_| ()),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(ws
        .on_upgrade(move |socket| {
            worker_protocol_ws_session(state.runtime, scope, worker_ref, query, socket)
        })
        .into_response())
}

#[cfg(feature = "ws-server")]
async fn worker_protocol_ws_session(
    runtime: Runtime,
    scope: Option<RuntimeWorkspaceScope>,
    worker_ref: WorkerRef,
    query: RuntimeWorkerEventsWsQuery,
    mut socket: WebSocket,
) {
    let mut cursor = match query.cursor.as_deref() {
        Some(raw) => match WorkerObservationCursor::decode(raw) {
            Some(cursor) => cursor,
            None => {
                let event =
                    protocol_error_event(format!("malformed worker observation cursor: {raw}"));
                let _ = send_protocol_event(&mut socket, &event).await;
                return;
            }
        },
        None => match runtime.worker_observation_cursor_now(&worker_ref) {
            Ok(cursor) => cursor,
            Err(error) => {
                let event = protocol_error_event(error.to_string());
                let _ = send_protocol_event(&mut socket, &event).await;
                return;
            }
        },
    };
    // Observation cursors are process-local. After a Runtime restart (or
    // bounded backlog expiry), the current Worker snapshot is authoritative
    // and replay resumes from the current in-memory tail.
    if runtime
        .read_worker_observation_events(&worker_ref, cursor)
        .is_err()
    {
        cursor = match runtime.worker_observation_cursor_now(&worker_ref) {
            Ok(cursor) => cursor,
            Err(error) => {
                let event = protocol_error_event(error.to_string());
                let _ = send_protocol_event(&mut socket, &event).await;
                return;
            }
        };
    }

    let mut receiver = match runtime.subscribe_worker_observation() {
        Ok(receiver) => receiver,
        Err(error) => {
            let event =
                protocol_error_event(format!("runtime observation bus unavailable: {error}"));
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };

    let snapshot = match runtime.worker_observation_snapshot(&worker_ref) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let event = protocol_error_event(error.to_string());
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };
    if !send_protocol_event(&mut socket, &snapshot).await {
        return;
    }

    match runtime.read_worker_observation_events(&worker_ref, cursor) {
        Ok(backlog) => {
            for event in backlog {
                cursor = WorkerObservationCursor::new(event.sequence);
                if !send_protocol_event(&mut socket, &event.payload).await {
                    return;
                }
            }
        }
        Err(error) => {
            let event = protocol_error_event(error.to_string());
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    }

    loop {
        tokio::select! {
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(WsMessage::Text(text))) => match decode_method(&text) {
                        Ok(method) => {
                            let result = match scope.as_ref() {
                                Some(scope) => {
                                    runtime.send_protocol_method_scoped(scope, &worker_ref, method)
                                }
                                None => runtime.send_protocol_method(&worker_ref, method),
                            };
                            match result {
                                Ok(events) => {
                                    for event in events {
                                        if !send_protocol_event(&mut socket, &event).await {
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    let event = protocol_error_event(error.to_string());
                                    if !send_protocol_event(&mut socket, &event).await {
                                        return;
                                    }
                                }
                            }
                        },
                        Err(error) => {
                            let event = protocol_error_event(format!(
                                "malformed protocol method frame: {error}"
                            ));
                            if !send_protocol_event(&mut socket, &event).await {
                                return;
                            }
                        }
                    },
                    Some(Ok(WsMessage::Close(_))) | None => return,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if socket.send(WsMessage::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(WsMessage::Pong(_))) | Some(Ok(WsMessage::Binary(_))) => {}
                    Some(Err(error)) => {
                        let event = protocol_error_event(format!("protocol WebSocket error: {error}"));
                        let _ = send_protocol_event(&mut socket, &event).await;
                        return;
                    }
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) if event.worker_ref == worker_ref && event.sequence > cursor.sequence => {
                        cursor = WorkerObservationCursor::new(event.sequence);
                        if !send_protocol_event(&mut socket, &event.payload).await {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let event = protocol_error_event("runtime observation backlog was overrun");
                        let _ = send_protocol_event(&mut socket, &event).await;
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let event = protocol_error_event("runtime observation bus closed");
                        let _ = send_protocol_event(&mut socket, &event).await;
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(feature = "ws-server")]
async fn send_protocol_event(socket: &mut WebSocket, event: &protocol::Event) -> bool {
    match encode_event(event) {
        Ok(text) => socket.send(WsMessage::Text(text.into())).await.is_ok(),
        Err(error) => {
            let fallback = protocol_error_event(format!(
                "failed to serialize protocol response event: {error}"
            ));
            let Ok(text) = encode_event(&fallback) else {
                return false;
            };
            socket.send(WsMessage::Text(text.into())).await.is_ok()
        }
    }
}

#[cfg(feature = "ws-server")]
fn protocol_error_event(message: impl Into<String>) -> protocol::Event {
    protocol::Event::Error {
        code: protocol::ErrorCode::Internal,
        message: message.into(),
    }
}

async fn send_worker_input(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerInput>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerInputResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let Json(input) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let ack = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.send_input_scoped(&scope, &worker_ref, input),
        None => state.runtime.send_input(&worker_ref, input),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerInputResponse { ack }))
}

async fn worker_completions(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Result<Json<RuntimeHttpWorkerCompletionsRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerCompletionsResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let entries = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.worker_completions_scoped(
            &scope,
            &worker_ref,
            request.kind,
            &request.prefix,
        ),
        None => state
            .runtime
            .worker_completions(&worker_ref, request.kind, &request.prefix),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerCompletionsResponse {
        kind: request.kind,
        prefix: request.prefix,
        entries,
    }))
}

async fn stop_worker(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Bytes,
) -> RestResult<RuntimeHttpWorkerLifecycleResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let request = parse_optional_lifecycle_request(body)?;
    let ack = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state
            .runtime
            .stop_worker_scoped(&scope, &worker_ref, request.reason),
        None => state.runtime.stop_worker(&worker_ref, request.reason),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerLifecycleResponse { ack }))
}

async fn cancel_worker(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Bytes,
) -> RestResult<RuntimeHttpWorkerLifecycleResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let request = parse_optional_lifecycle_request(body)?;
    let ack = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state
            .runtime
            .cancel_worker_scoped(&scope, &worker_ref, request.reason),
        None => state.runtime.cancel_worker(&worker_ref, request.reason),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerLifecycleResponse { ack }))
}

fn worker_ref_for(
    _runtime: &Runtime,
    worker_id: String,
) -> Result<WorkerRef, RuntimeHttpRestError> {
    let worker_id = WorkerId::parse(&worker_id).ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "invalid_worker_id",
            "worker_id must be an unsigned integer",
        )
    })?;
    Ok(WorkerRef::new(worker_id))
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

async fn require_runtime_auth(
    State(state): State<RuntimeHttpState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    if let Some(auth) = state.auth.as_deref() {
        let Some(token) = supplied else {
            return RuntimeHttpRestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing Runtime capability bearer token",
            )
            .into_response();
        };
        match verify_capability_token(
            auth,
            token,
            required_runtime_permission(request.method(), request.uri().path()),
            unix_now_seconds(),
        ) {
            Ok(context) => {
                request.extensions_mut().insert(context);
                return next.run(request).await;
            }
            Err(error) => {
                return runtime_auth_error_response(error).into_response();
            }
        }
    }

    if let Some(expected) = state.local_token.as_deref() {
        if supplied != Some(expected) {
            return RuntimeHttpRestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid local Runtime bearer token",
            )
            .into_response();
        }
        request.extensions_mut().insert(RuntimeAuthContext {
            server_id: "local-token".to_string(),
            workspace_id: "local".to_string(),
            permissions: Vec::new(),
            token_id: "local-token".to_string(),
            expires_at: 0,
        });
    }
    next.run(request).await
}

fn runtime_auth_error_response(error: RuntimeAuthError) -> RuntimeHttpRestError {
    match error {
        RuntimeAuthError::MissingPermission(permission) => RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("Runtime capability token is missing required permission `{permission}`"),
        ),
        RuntimeAuthError::MissingWorkspaceScope => RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "workspace_scope_required",
            "Runtime capability token is missing workspace scope",
        ),
        other => RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            format!("invalid Runtime capability token: {other}"),
        ),
    }
}

fn auth_workspace_scope(
    state: &RuntimeHttpState,
    auth: Option<&Extension<RuntimeAuthContext>>,
) -> Result<Option<RuntimeWorkspaceScope>, RuntimeHttpRestError> {
    let Some(Extension(context)) = auth else {
        if state.auth.is_some() || state.local_token.is_some() {
            return Err(RuntimeHttpRestError::new(
                StatusCode::FORBIDDEN,
                "workspace_scope_required",
                "Runtime worker operation requires a workspace-scoped authorization context",
            ));
        }
        return Ok(None);
    };
    let workspace_id = context.workspace_id.trim();
    if workspace_id.is_empty() {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "workspace_scope_required",
            "Runtime worker operation requires a non-empty workspace scope",
        ));
    }
    let server_id = context.server_id.trim();
    if server_id.is_empty() {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "server_scope_required",
            "Runtime worker operation requires a non-empty server scope",
        ));
    }
    Ok(Some(RuntimeWorkspaceScope::new(workspace_id, server_id)))
}

fn required_runtime_permission(method: &Method, path: &str) -> Option<&'static str> {
    if path == "/v1/runtime" {
        return None;
    }
    if path == "/v1/workers" && *method == Method::GET {
        return Some("workers:list");
    }
    if path == "/v1/workers" && *method == Method::POST {
        return Some("workers:create");
    }
    if path.starts_with("/v1/config-bundles") || path.starts_with("/v1/working-directories") {
        return Some("workers:create");
    }
    if path.ends_with("/input") || path.ends_with("/restore") {
        return Some("workers:input");
    }
    if path.ends_with("/stop") || path.ends_with("/cancel") {
        return Some("workers:stop");
    }
    if path.ends_with("/protocol") || path.ends_with("/protocol/ws") {
        return Some("workers:protocol");
    }
    if path.ends_with("/completions") {
        return Some("workers:read");
    }
    if path.starts_with("/v1/workers/") && *method == Method::DELETE {
        return Some("workers:delete");
    }
    if path.starts_with("/v1/workers/") && *method == Method::GET {
        return Some("workers:read");
    }
    None
}

#[derive(Debug)]
struct RuntimeHttpRestError {
    status: StatusCode,
    code: String,
    message: String,
}

impl RuntimeHttpRestError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
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
                code: self.code,
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
        RuntimeError::WorkingDirectory(diagnostic)
            if diagnostic.code == "working_directory_not_found" =>
        {
            StatusCode::NOT_FOUND
        }
        RuntimeError::RuntimeStopped
        | RuntimeError::WorkerExecutionUnavailable { .. }
        | RuntimeError::ExecutionBackendUnavailable { .. }
        | RuntimeError::WorkerExecutionRejected { .. } => StatusCode::CONFLICT,
        RuntimeError::WorkspaceOwnerMismatch { .. } => StatusCode::FORBIDDEN,
        RuntimeError::LimitTooLarge { .. }
        | RuntimeError::InvalidRequest(_)
        | RuntimeError::InvalidInitialInputKind { .. }
        | RuntimeError::ConfigBundleDigestMismatch { .. }
        | RuntimeError::InvalidProfileSelector { .. }
        | RuntimeError::UnsupportedConfigDeclaration { .. }
        | RuntimeError::WorkingDirectory(_) => StatusCode::BAD_REQUEST,
        RuntimeError::StoreIo { .. }
        | RuntimeError::StoreMissing { .. }
        | RuntimeError::StoreCorrupt { .. }
        | RuntimeError::StatePoisoned => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn code_for_runtime_error(error: &RuntimeError) -> String {
    match error {
        RuntimeError::RuntimeStopped => "runtime_stopped".to_string(),
        RuntimeError::WorkerNotFound { .. } => "worker_not_found".to_string(),
        RuntimeError::WorkerExecutionUnavailable { .. } => {
            "worker_execution_unavailable".to_string()
        }
        RuntimeError::ExecutionBackendUnavailable { .. } => {
            "execution_backend_unavailable".to_string()
        }
        RuntimeError::WorkerExecutionRejected { .. } => "worker_execution_rejected".to_string(),
        RuntimeError::WorkspaceOwnerMismatch { .. } => "workspace_owner_mismatch".to_string(),
        RuntimeError::LimitTooLarge { .. } => "limit_too_large".to_string(),
        RuntimeError::InvalidRequest(_) => "invalid_request".to_string(),
        RuntimeError::WorkingDirectory(diagnostic) => diagnostic.code.clone(),
        RuntimeError::InvalidInitialInputKind { .. } => "invalid_initial_input_kind".to_string(),
        RuntimeError::ConfigBundleMissing { .. } => "config_bundle_missing".to_string(),
        RuntimeError::ConfigBundleDigestMismatch { .. } => {
            "config_bundle_digest_mismatch".to_string()
        }
        RuntimeError::InvalidProfileSelector { .. } => "invalid_profile_selector".to_string(),
        RuntimeError::UnsupportedConfigDeclaration { .. } => {
            "unsupported_config_declaration".to_string()
        }
        RuntimeError::StoreIo { .. } => "store_io".to_string(),
        RuntimeError::StoreMissing { .. } => "store_missing".to_string(),
        RuntimeError::StoreCorrupt { .. } => "store_corrupt".to_string(),
        RuntimeError::StatePoisoned => "state_poisoned".to_string(),
    }
}

/// Errors raised while building or serving the Runtime REST process API.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeHttpServerError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("Runtime HTTP server requires capability-token auth or a local bearer token")]
    AuthRequired,
    #[error("Runtime HTTP server I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        CapabilityTokenSigner, RuntimeHttpAuthConfig, RuntimeIdentityMaterial, TrustedServerKey,
        capability_claims,
    };
    use crate::catalog::{ConfigBundleRef, ProfileSelector, WorkerStatus, WorkspaceApiRef};
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation,
        WorkerExecutionRestoreRequest, WorkerExecutionResult, WorkerExecutionRunState,
        WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
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

    fn scoped_task_request(objective: &str, workspace_id: &str) -> CreateWorkerRequest {
        let mut request = task_request(objective);
        request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: workspace_id.to_string(),
            base_url: format!("https://workspace.example/{workspace_id}"),
            runtime_id: None,
            access_token: None,
        });
        request
    }

    fn auth_config_and_signer() -> (RuntimeHttpAuthConfig, CapabilityTokenSigner) {
        let identity = RuntimeIdentityMaterial::generate("server-a").unwrap();
        let signer = CapabilityTokenSigner::new(identity.identity_id.clone(), identity.private_key);
        let auth = RuntimeHttpAuthConfig {
            runtime_id: "runtime-test".to_string(),
            trusted_servers: vec![TrustedServerKey {
                server_id: identity.identity_id,
                public_key: identity.public_key,
                display_name: None,
            }],
        };
        (auth, signer)
    }

    fn auth_config_and_two_signers() -> (
        RuntimeHttpAuthConfig,
        CapabilityTokenSigner,
        CapabilityTokenSigner,
    ) {
        let identity_a = RuntimeIdentityMaterial::generate("server-a").unwrap();
        let identity_b = RuntimeIdentityMaterial::generate("server-b").unwrap();
        let signer_a =
            CapabilityTokenSigner::new(identity_a.identity_id.clone(), identity_a.private_key);
        let signer_b =
            CapabilityTokenSigner::new(identity_b.identity_id.clone(), identity_b.private_key);
        let auth = RuntimeHttpAuthConfig {
            runtime_id: "runtime-test".to_string(),
            trusted_servers: vec![
                TrustedServerKey {
                    server_id: identity_a.identity_id,
                    public_key: identity_a.public_key,
                    display_name: None,
                },
                TrustedServerKey {
                    server_id: identity_b.identity_id,
                    public_key: identity_b.public_key,
                    display_name: None,
                },
            ],
        };
        (auth, signer_a, signer_b)
    }

    fn token_for_workspace(signer: &CapabilityTokenSigner, workspace_id: &str) -> String {
        token_for_workspace_with_permissions(
            signer,
            workspace_id,
            [
                "workers:list",
                "workers:create",
                "workers:read",
                "workers:input",
                "workers:stop",
                "workers:protocol",
                "workers:delete",
            ],
        )
    }

    fn token_for_workspace_with_permissions<const N: usize>(
        signer: &CapabilityTokenSigner,
        workspace_id: &str,
        permissions: [&str; N],
    ) -> String {
        let claims = capability_claims(
            signer.server_id(),
            "runtime-test",
            workspace_id,
            permissions.into_iter().map(str::to_string).collect(),
            3600,
        )
        .unwrap();
        signer.sign(&claims).unwrap()
    }

    fn bearer_request(
        method: Method,
        uri: impl AsRef<str>,
        token: &str,
        body: impl Into<Body>,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.into())
            .unwrap()
    }

    #[tokio::test]
    async fn capability_workspace_scope_filters_list_and_hides_detail() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        let (auth, signer) = auth_config_and_signer();
        let token_a = token_for_workspace(&signer, "workspace-a");
        let token_b = token_for_workspace(&signer, "workspace-b");
        let app = runtime_http_router_with_auth(runtime, None, auth);

        let create_a = scoped_task_request("a", "workspace-a");
        let response = app
            .clone()
            .oneshot(bearer_request(
                Method::POST,
                "/v1/workers",
                &token_a,
                serde_json::to_vec(&create_a).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let worker_a: RuntimeHttpWorkerResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(worker_a.worker.workspace_id.as_deref(), Some("workspace-a"));

        let create_b = scoped_task_request("b", "workspace-b");
        let response = app
            .clone()
            .oneshot(bearer_request(
                Method::POST,
                "/v1/workers",
                &token_b,
                serde_json::to_vec(&create_b).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let worker_b: RuntimeHttpWorkerResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(worker_b.worker.workspace_id.as_deref(), Some("workspace-b"));

        let response = app
            .clone()
            .oneshot(bearer_request(
                Method::GET,
                "/v1/workers",
                &token_a,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let workers: RuntimeHttpWorkersResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(workers.workers.len(), 1);
        assert_eq!(workers.workers[0].worker_ref, worker_a.worker.worker_ref);
        assert_eq!(
            workers.workers[0].workspace_id.as_deref(),
            Some("workspace-a")
        );

        let response = app
            .oneshot(bearer_request(
                Method::GET,
                format!("/v1/workers/{}", worker_b.worker.worker_id),
                &token_a,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn capability_workspace_owner_binding_rejects_other_trusted_server() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        let (auth, signer_a, signer_b) = auth_config_and_two_signers();
        let token_a = token_for_workspace(&signer_a, "workspace-a");
        let token_b = token_for_workspace(&signer_b, "workspace-a");
        let app = runtime_http_router_with_auth(runtime, None, auth);

        let create_a = scoped_task_request("a", "workspace-a");
        let response = app
            .clone()
            .oneshot(bearer_request(
                Method::POST,
                "/v1/workers",
                &token_a,
                serde_json::to_vec(&create_a).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let create_b = scoped_task_request("b", "workspace-a");
        let response = app
            .oneshot(bearer_request(
                Method::POST,
                "/v1/workers",
                &token_b,
                serde_json::to_vec(&create_b).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn capability_token_without_workspace_scope_is_forbidden() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        let (auth, signer) = auth_config_and_signer();
        let token = token_for_workspace_with_permissions(&signer, "", ["workers:list"]);
        let app = runtime_http_router_with_auth(runtime, None, auth);

        let response = app
            .oneshot(bearer_request(
                Method::GET,
                "/v1/workers",
                &token,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn capability_token_without_worker_permission_is_forbidden() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        let (auth, signer) = auth_config_and_signer();
        let token = token_for_workspace_with_permissions(&signer, "workspace-a", ["workers:list"]);
        let app = runtime_http_router_with_auth(runtime, None, auth);
        let create = scoped_task_request("a", "workspace-a");

        let response = app
            .oneshot(bearer_request(
                Method::POST,
                "/v1/workers",
                &token,
                serde_json::to_vec(&create).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    fn task_request(_objective: &str) -> CreateWorkerRequest {
        let profile = ProfileSelector::Builtin("builtin:coder".to_string());
        let bundle = test_bundle(profile.clone());
        CreateWorkerRequest {
            idempotency_key: None,
            idempotency_fingerprint: None,
            profile,
            display_name: None,
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
            workspace_api: None,
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

        fn restore_worker(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::Connected {
                handle: WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
                run_state: WorkerExecutionRunState::Idle,
                working_directory: request.previous_working_directory,
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

    async fn authed_json_request<T: Serialize>(
        app: Router,
        method: Method,
        uri: &str,
        token: &str,
        body: &T,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
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

    async fn authed_empty_request(
        app: Router,
        method: Method,
        uri: &str,
        token: &str,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
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
        let token = "local-token";
        let app = runtime_http_router(runtime.clone(), token.to_string());

        let response = authed_json_request(
            app.clone(),
            Method::POST,
            "/v1/workers",
            token,
            &task_request("rest"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let created: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(
            created.worker.worker_ref.worker_id,
            created.worker.worker_id
        );

        let input = WorkerInput::user("hello from backend");
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/input", created.worker.worker_id),
            token,
            &input,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let _input_ack: RuntimeHttpWorkerInputResponse = read_json(response).await;

        let response = authed_empty_request(
            app.clone(),
            Method::GET,
            &format!("/v1/workers/{}", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let _detail: RuntimeHttpWorkerResponse = read_json(response).await;

        let response = authed_empty_request(
            app.clone(),
            Method::GET,
            &format!("/v1/workers/{}/transcript", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/stop", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let stop: RuntimeHttpWorkerLifecycleResponse = read_json(response).await;
        assert_eq!(stop.ack.worker_ref, created.worker.worker_ref);

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/restore", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let restored: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(restored.worker.status, WorkerStatus::Idle);

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/stop", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/cancel", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cancel: RuntimeHttpWorkerLifecycleResponse = read_json(response).await;
        assert_eq!(cancel.ack.worker_ref, created.worker.worker_ref);

        let response = authed_empty_request(app.clone(), Method::GET, "/v1/workers", token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let workers: RuntimeHttpWorkersResponse = read_json(response).await;
        assert_eq!(workers.workers.len(), 1);

        let response = authed_empty_request(app, Method::GET, "/v1/runtime", token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let summary: RuntimeHttpSummaryResponse = read_json(response).await;
        assert_eq!(summary.runtime.worker_count, 1);
        assert_eq!(summary.runtime.stopped_worker_count, 1);
    }

    #[tokio::test]
    async fn local_token_placeholder_rejects_missing_bearer_token() {
        let app = runtime_http_router(Runtime::new_memory(), "local-token".to_string());

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
        let token = "local-token";
        let app = runtime_http_router(Runtime::new_memory(), token.to_string());
        let response = authed_empty_request(app, Method::GET, "/v1/workers/999", token).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "worker_not_found");
        assert!(error.error.message.contains("999"));
    }

    #[tokio::test]
    async fn serve_runtime_http_rejects_missing_auth_configuration() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = serve_runtime_http(Runtime::new_memory(), listener, None)
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeHttpServerError::AuthRequired));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = serve_runtime_http_with_auth(Runtime::new_memory(), listener, None, None)
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeHttpServerError::AuthRequired));
    }

    #[test]
    fn workdir_runtime_errors_preserve_diagnostic_code() {
        let error =
            RuntimeError::WorkingDirectory(crate::working_directory::WorkingDirectoryDiagnostic {
                code: "working_directory_not_found".to_string(),
                message: "working directory missing-workdir was not found".to_string(),
            });

        assert_eq!(status_for_runtime_error(&error), StatusCode::NOT_FOUND);
        assert_eq!(
            code_for_runtime_error(&error),
            "working_directory_not_found"
        );
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
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header as ws_header;

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

        fn dispatch_method(
            &self,
            _handle: &WorkerExecutionHandle,
            _method: protocol::Method,
        ) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(
                WorkerExecutionOperation::ProtocolMethod,
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
        let bundle = ws_test_bundle(ProfileSelector::Builtin("builtin:companion".to_string()));
        CreateWorkerRequest {
            idempotency_key: None,
            idempotency_fingerprint: None,
            profile: ProfileSelector::Builtin("builtin:companion".to_string()),
            display_name: None,
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
            workspace_api: None,
        }
    }

    async fn spawn_runtime_server() -> (Runtime, WorkerRef, String) {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(WsBackend))
                .unwrap();
        runtime
            .store_config_bundle(ws_test_bundle(ProfileSelector::Builtin(
                "builtin:companion".to_string(),
            )))
            .unwrap();
        let worker = runtime
            .create_worker_scoped(
                &RuntimeWorkspaceScope::new("local", "local-token"),
                ws_create_request(),
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                serve_runtime_http(runtime, listener, Some("local-token".to_string()))
                    .await
                    .unwrap()
            }
        });
        (
            runtime,
            worker.worker_ref.clone(),
            format!(
                "ws://{addr}/v1/workers/{}/protocol/ws",
                worker.worker_ref.worker_id
            ),
        )
    }

    fn authed_ws_request(url: &str) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            ws_header::AUTHORIZATION,
            "Bearer local-token".parse().unwrap(),
        );
        request
    }

    async fn next_frame(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> protocol::Event {
        let message = stream.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn protocol_ws_connect_sends_snapshot_and_live_worker_events() {
        let (runtime, worker_ref, url) = spawn_runtime_server().await;
        let (mut stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();

        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));

        runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDelta {
                    text: "started".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::TextDelta { .. }
        ));
    }

    #[tokio::test]
    async fn protocol_ws_cursor_resume_is_duplicate_safe_and_filters_workers() {
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
                    text: "other".into(),
                },
            )
            .unwrap();

        let resume_url = format!("{url}?cursor={}", first.cursor);
        let (mut stream, _) = connect_async(authed_ws_request(&resume_url)).await.unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));

        runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDone {
                    text: "done".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::TextDone { .. }
        ));
    }

    #[tokio::test]
    async fn protocol_ws_reports_malformed_cursor_and_method_frame() {
        let (_runtime, _worker_ref, url) = spawn_runtime_server().await;
        let malformed_url = format!("{url}?cursor=bad");
        let (mut malformed, _) = connect_async(authed_ws_request(&malformed_url))
            .await
            .unwrap();
        assert!(matches!(
            next_frame(&mut malformed).await,
            protocol::Event::Error { .. }
        ));

        let (mut stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
        let _ = next_frame(&mut stream).await;
        stream.send(Message::Text("{}".into())).await.unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Error { .. }
        ));
    }
}
