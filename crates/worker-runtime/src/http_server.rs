//! Optional REST process adapter for the Runtime command API.
//!
//! This module is intentionally gated by the `http-server` feature so embedded
//! Runtime users do not pull HTTP dependencies.  The server is a process-local
//! command surface for a trusted backend/proxy. Browsers must not connect to the
//! Runtime process directly; a backend is expected to own any browser-facing
//! credentials, registration, and policy.

mod runtime_management_api;

use crate::auth::{RuntimeAuthContext, new_token_id, unix_now_seconds};
use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, RepositoryRefObservationRequest, WorkerDetail,
    WorkerLifecycleAck, WorkerSummary, WorkingDirectoryRepositoryAccessRequest,
    WorkingDirectoryRequest, WorkingDirectoryStatus, WorkspaceApiRef,
};
use crate::config_bundle::{ConfigBundle, ConfigBundleAvailability, ConfigBundleSummary};
use crate::error::RuntimeError;
use crate::identity::{WorkerId, WorkerRef};
use crate::interaction::{WorkerInput, WorkerInteractionAck};
use crate::management::{RuntimeSummary, WorkerDeleteResult};
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationCursor;
use crate::retention::{
    WorkerRetentionExecutionRequest, WorkerRetentionExecutionResult, WorkerRetentionInventory,
};
#[cfg(feature = "ws-server")]
use crate::runtime::RuntimeSubscriptionRecvError;
use crate::ssh_host_key_probe::{
    SSH_HOST_KEY_PROBE_OPERATION, SSH_KEYSCAN_TIMEOUT, SshHostKeyProbeError,
    SshHostKeyProbeRequest, SshHostKeyProbeResponse, probe_ssh_host_keys_with_program,
};
use crate::workspace_issuer::{
    RuntimeVerificationSigner, VerifiedWorkspaceCapability, WORKSPACE_VERIFICATION_OPERATION,
    WorkspaceCapabilityExpectation, WorkspaceCapabilityVerifier,
    WorkspaceRuntimeVerificationAcknowledgement, WorkspaceRuntimeVerificationAuthority,
    WorkspaceRuntimeVerificationChallenge, WorkspaceRuntimeVerificationReceipt,
    WorkspaceRuntimeVerificationRecord, WorkspaceRuntimeVerificationResponse,
    inspect_workspace_capability_claims, workspace_request_body_digest,
};
use crate::{Runtime, RuntimeWorkspaceScope};
use axum::body::{Body, Bytes};
use axum::extract::rejection::{JsonRejection, QueryRejection};
#[cfg(feature = "ws-server")]
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
#[cfg(feature = "ws-server")]
use futures::{SinkExt, StreamExt};
#[cfg(feature = "ws-server")]
use protocol::stream::{decode_method, encode_event};
#[cfg(feature = "ws-server")]
use protocol::subscription::{
    SubscriptionEvent, SubscriptionFrame, SubscriptionFramePayload, SubscriptionId,
    SubscriptionRejectionCode, SubscriptionRequest, SubscriptionResponse,
    SubscriptionTerminationCode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use workdir::{
    CommandOutput, CommandStatus, WorkdirSessionHandle, dispatch_workdir_session_operation,
    http::{
        OpenWorkdirSessionRequest, OpenWorkdirSessionResponse, WorkdirSessionId,
        WorkdirSessionOperation, WorkdirSessionOperationRequest, WorkdirSessionOperationResult,
        WorkdirTransportError, WorkdirTransportErrorCode,
    },
};

const DEFAULT_RUNTIME_HTTP_PORT: u16 = 38800;
pub const RUNTIME_HTTP_PROTOCOL_MIN_VERSION: u32 = 1;
pub const RUNTIME_HTTP_PROTOCOL_MAX_VERSION: u32 = 1;
pub const RUNTIME_HTTP_PROTOCOL_VERSION: u32 = RUNTIME_HTTP_PROTOCOL_MAX_VERSION;
pub const RUNTIME_PING_PERMISSION: &str = "runtime:ping";
pub const RUNTIME_WORKSPACE_SCOPE_HEADER: &str = "x-yoi-workspace-id";

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
    /// v0 store selection for the Runtime process.
    pub store: RuntimeHttpStoreSelection,
    /// Minimal local bearer token for explicitly local Runtime calls.
    /// This is not a browser-facing credential model.
    pub local_token: Option<String>,
}

impl Default for RuntimeHttpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_runtime_http_bind_addr(),
            display_name: None,
            store: RuntimeHttpStoreSelection::Memory,
            local_token: None,
        }
    }
}

impl fmt::Debug for RuntimeHttpServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHttpServerConfig")
            .field("bind_addr", &self.bind_addr)
            .field("display_name", &self.display_name)
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

pub async fn serve_runtime_http_with_shutdown<F>(
    runtime: Runtime,
    listener: TcpListener,
    local_token: Option<String>,
    shutdown: F,
) -> Result<(), RuntimeHttpServerError>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let local_token = local_token.ok_or(RuntimeHttpServerError::AuthRequired)?;
    axum::serve(listener, runtime_http_router(runtime, local_token))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
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

pub async fn serve_runtime_http_with_workspace_auth_shutdown<F>(
    runtime: Runtime,
    listener: TcpListener,
    local_token: Option<String>,
    workspace_auth: WorkspaceRuntimeHttpAuth,
    shutdown: F,
) -> Result<(), RuntimeHttpServerError>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    axum::serve(
        listener,
        runtime_http_router_with_optional_auth(runtime, local_token, Some(workspace_auth)),
    )
    .with_graceful_shutdown(shutdown)
    .await?;
    Ok(())
}

pub async fn serve_runtime_http_with_workspace_auth(
    runtime: Runtime,
    listener: TcpListener,
    local_token: Option<String>,
    workspace_auth: WorkspaceRuntimeHttpAuth,
) -> Result<(), RuntimeHttpServerError> {
    axum::serve(
        listener,
        runtime_http_router_with_optional_auth(runtime, local_token, Some(workspace_auth)),
    )
    .await?;
    Ok(())
}

fn remaining_route(path: &'static str, methods: &[&str]) -> &'static str {
    for method in methods {
        assert!(
            runtime_api::REMAINING_RUNTIME_ROUTES
                .iter()
                .any(|route| route.method == *method && route.path == path),
            "handwritten Runtime route {method} {path} is missing from runtime-api inventory",
        );
    }
    path
}

/// Build the REST router for an existing Runtime.
///
/// Handlers delegate to [`Runtime`] methods and keep Worker authority Runtime-local.
/// The path contains only a Runtime-local `worker_id`; backend aliases are not
/// accepted or forwarded as Runtime authority.
pub fn runtime_http_router(runtime: Runtime, local_token: String) -> Router {
    runtime_http_router_with_optional_auth(runtime, Some(local_token), None)
}

pub fn runtime_http_router_with_workspace_auth(
    runtime: Runtime,
    local_token: Option<String>,
    workspace_auth: WorkspaceRuntimeHttpAuth,
) -> Router {
    runtime_http_router_with_optional_auth(runtime, local_token, Some(workspace_auth))
}

fn runtime_http_router_with_optional_auth(
    runtime: Runtime,
    local_token: Option<String>,
    workspace_auth: Option<WorkspaceRuntimeHttpAuth>,
) -> Router {
    runtime_http_router_with_auth_and_ssh_keyscan_program(
        runtime,
        local_token,
        workspace_auth,
        PathBuf::from("ssh-keyscan"),
    )
}

fn runtime_http_router_with_auth_and_ssh_keyscan_program(
    runtime: Runtime,
    local_token: Option<String>,
    workspace_auth: Option<WorkspaceRuntimeHttpAuth>,
    ssh_keyscan_program: PathBuf,
) -> Router {
    let state = RuntimeHttpState {
        runtime,
        local_token: local_token.map(Arc::<str>::from),
        workspace_auth: workspace_auth.map(Arc::new),
        workdir_sessions: Arc::new(Mutex::new(HashMap::new())),
        ssh_keyscan_program: Arc::new(ssh_keyscan_program),
    };

    let router = Router::new()
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE, &["POST"]),
            post(post_workspace_verification_challenge),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK, &["POST"]),
            post(post_workspace_verification_acknowledgement),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLES, &["GET", "POST"]),
            get(list_config_bundles).post(store_config_bundle),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLE_AVAILABILITY,
                &["GET"],
            ),
            get(check_config_bundle),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKSPACE_PROMPT_PROJECTIONS,
                &["POST"],
            ),
            post(observe_workspace_prompt_projection),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORIES,
                &["GET", "POST"],
            ),
            get(list_working_directories).post(create_working_directory),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS,
                &["POST"],
            ),
            post(authorize_working_directory_repository_access),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_SSH_PROBE, &["POST"]),
            post(probe_repository_ssh_host_keys),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE,
                &["POST"],
            ),
            post(observe_repository_ref),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_WORKDIR_SESSIONS, &["POST"]),
            post(open_workdir_session),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKDIR_SESSION_OPERATIONS,
                &["POST"],
            ),
            post(run_workdir_session_operation),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_WORKDIR_SESSION, &["DELETE"]),
            delete(close_workdir_session),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY,
                &["GET", "DELETE"],
            ),
            get(get_working_directory).delete(cleanup_working_directory),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_WORKER_ATTACHMENTS, &["POST"]),
            post(upload_worker_file).layer(DefaultBodyLimit::max(MAX_WORKER_FILE_UPLOAD_BYTES)),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_WORKER_ATTACHMENT, &["DELETE"]),
            delete(delete_worker_uploaded_file),
        )
        .route(
            remaining_route(
                runtime_api::RUNTIME_ROUTE_WORKER_SESSION_ATTACHMENT,
                &["GET"],
            ),
            get(get_worker_session_attachment),
        );

    #[cfg(feature = "ws-server")]
    let router = router
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_PROTOCOL_WS, &["GET"]),
            get(runtime_protocol_ws),
        )
        .route(
            remaining_route(runtime_api::RUNTIME_ROUTE_WORKER_PROTOCOL_WS, &["GET"]),
            get(worker_protocol_ws),
        );

    router
        .with_state(state.clone())
        .merge(runtime_management_api::router(state.clone()))
        .layer(middleware::from_fn_with_state(state, require_runtime_auth))
}

pub const MAX_WORKER_FILE_UPLOAD_BYTES: usize =
    session_store::DEFAULT_MAX_UPLOADED_FILE_BYTES as usize;

#[derive(Clone)]
struct RuntimeHttpState {
    runtime: Runtime,
    local_token: Option<Arc<str>>,
    workspace_auth: Option<Arc<WorkspaceRuntimeHttpAuth>>,
    workdir_sessions: Arc<Mutex<HashMap<String, RuntimeHttpWorkdirSession>>>,
    ssh_keyscan_program: Arc<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRuntimeHttpAuth {
    pub verifier: WorkspaceCapabilityVerifier,
    pub signer: RuntimeVerificationSigner,
    pub verifications: Arc<dyn WorkspaceRuntimeVerificationAuthority>,
}

struct RuntimeHttpWorkdirSession {
    owner: RuntimeWorkspaceScope,
    session: WorkdirSessionHandle,
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

/// Server-owned notification carrying the Workspace's current immutable Prompt projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkspacePromptProjectionRequest {
    pub projection: worker::WorkspacePromptProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkspacePromptProjectionResponse {
    pub workspace_id: String,
    pub config_revision: u64,
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

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
struct RuntimeHttpWorkersQuery {
    status: Option<RuntimeHttpWorkerStatusFilter>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeHttpWorkerStatusFilter {
    Stopped,
}

/// `GET /v1/ping` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeHttpPingResponse {
    pub runtime_id: String,
    pub protocol_version: u32,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpRepositoryAccessResponse {
    pub authorized: bool,
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

/// Replace the Workspace API binding for an existing Worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpWorkerWorkspaceApiRequest {
    pub workspace_api: WorkspaceApiRef,
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

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeHttpUploadFileQuery {
    pub file_name: String,
    pub media_type: String,
    #[serde(default)]
    pub upload_id: Option<String>,
    #[serde(default)]
    pub principal_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub runtime_id: Option<String>,
    #[serde(default)]
    pub owner_worker_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpUploadedFileResponse {
    pub file: protocol::UploadedFileRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpUploadedFileDeleteResponse {
    pub deleted: bool,
}

pub type RuntimeHttpWorkerCompletionsRequest = runtime_api::CompletionRequest;
pub type RuntimeHttpWorkerCompletionsResponse = runtime_api::CompletionResponse;

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

fn unix_now_i64() -> i64 {
    i64::try_from(unix_now_seconds()).unwrap_or(i64::MAX)
}

async fn post_workspace_verification_challenge(
    State(state): State<RuntimeHttpState>,
    Extension(verified): Extension<VerifiedWorkspaceCapability>,
    Json(challenge): Json<WorkspaceRuntimeVerificationChallenge>,
) -> RestResult<WorkspaceRuntimeVerificationResponse> {
    let auth = state.workspace_auth.as_deref().ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::NOT_IMPLEMENTED,
            "workspace_runtime_verification_unavailable",
            "Workspace Runtime verification is not configured",
        )
    })?;
    if challenge.workspace_id != verified.workspace_id
        || challenge.runtime_id != verified.runtime_id
        || challenge.binding_revision != verified.binding_revision
        || challenge.workspace_key_id != verified.issuer_key_id
        || challenge.workspace_identity_revision != verified.issuer_identity_revision
        || challenge.workspace_trust_generation != verified.trust_generation
        || challenge.runtime_id != auth.signer.runtime_id()
        || challenge.runtime_public_key_fingerprint != auth.signer.public_key_fingerprint()
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "workspace_runtime_verification_rejected",
            "Workspace Runtime verification challenge does not match authenticated authority",
        ));
    }
    let response = auth
        .signer
        .sign_response(&challenge, uuid::Uuid::now_v7().to_string(), unix_now_i64())
        .map_err(|error| {
            RuntimeHttpRestError::new(
                StatusCode::UNAUTHORIZED,
                "workspace_runtime_verification_rejected",
                error.to_string(),
            )
        })?;
    Ok(Json(response))
}

async fn post_workspace_verification_acknowledgement(
    State(state): State<RuntimeHttpState>,
    Extension(verified): Extension<VerifiedWorkspaceCapability>,
    Json(acknowledgement): Json<WorkspaceRuntimeVerificationAcknowledgement>,
) -> RestResult<WorkspaceRuntimeVerificationReceipt> {
    let auth = state.workspace_auth.as_deref().ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::NOT_IMPLEMENTED,
            "workspace_runtime_verification_unavailable",
            "Workspace Runtime verification is not configured",
        )
    })?;
    if acknowledgement.workspace_id != verified.workspace_id
        || acknowledgement.runtime_id != verified.runtime_id
        || acknowledgement.binding_revision != verified.binding_revision
        || acknowledgement.workspace_key_id != verified.issuer_key_id
        || acknowledgement.workspace_identity_revision != verified.issuer_identity_revision
        || acknowledgement.workspace_trust_generation != verified.trust_generation
        || acknowledgement.runtime_id != auth.signer.runtime_id()
        || acknowledgement.runtime_public_key_fingerprint != auth.signer.public_key_fingerprint()
        || acknowledgement.expires_at <= unix_now_i64()
        || acknowledgement.binding_revision == 0
        || acknowledgement.runtime_identity_revision == 0
        || acknowledgement.workspace_identity_revision == 0
        || acknowledgement.workspace_trust_generation == 0
        || acknowledgement.workspace_nonce.is_empty()
        || acknowledgement.runtime_nonce.is_empty()
        || acknowledgement.response_digest.len() != workspace_request_body_digest(&[]).len()
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "workspace_runtime_verification_rejected",
            "Workspace Runtime verification acknowledgement is invalid",
        ));
    }
    let challenge = WorkspaceRuntimeVerificationChallenge {
        challenge_id: acknowledgement.challenge_id.clone(),
        workspace_id: acknowledgement.workspace_id.clone(),
        runtime_id: acknowledgement.runtime_id.clone(),
        binding_revision: acknowledgement.binding_revision,
        workspace_key_id: acknowledgement.workspace_key_id.clone(),
        workspace_identity_revision: acknowledgement.workspace_identity_revision,
        workspace_trust_generation: acknowledgement.workspace_trust_generation,
        runtime_public_key_fingerprint: acknowledgement.runtime_public_key_fingerprint.clone(),
        runtime_identity_revision: acknowledgement.runtime_identity_revision,
        workspace_nonce: acknowledgement.workspace_nonce.clone(),
        expires_at: acknowledgement.expires_at,
    };
    let response = acknowledgement.response.clone();
    if response.runtime_nonce != acknowledgement.runtime_nonce
        || response.workspace_nonce != acknowledgement.workspace_nonce
        || response.response_proof.is_empty()
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "workspace_runtime_verification_rejected",
            "Workspace Runtime verification acknowledgement does not match its response",
        ));
    }
    let response_bytes = serde_json::to_vec(&response).map_err(|_| {
        RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "workspace_runtime_verification_rejected",
            "Workspace Runtime verification response could not be canonicalized",
        )
    })?;
    if workspace_request_body_digest(&response_bytes) != acknowledgement.response_digest {
        return Err(RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "workspace_runtime_verification_rejected",
            "Workspace Runtime verification acknowledgement has the wrong response digest",
        ));
    }
    crate::workspace_issuer::verify_runtime_verification_response(
        &response,
        &challenge,
        auth.signer.public_key(),
        unix_now_i64(),
    )
    .map_err(|error| {
        RuntimeHttpRestError::new(
            StatusCode::UNAUTHORIZED,
            "workspace_runtime_verification_rejected",
            error.to_string(),
        )
    })?;
    auth.verifications
        .record(WorkspaceRuntimeVerificationRecord {
            workspace_id: acknowledgement.workspace_id.clone(),
            runtime_id: acknowledgement.runtime_id.clone(),
            binding_revision: acknowledgement.binding_revision,
            workspace_key_id: acknowledgement.workspace_key_id.clone(),
            workspace_identity_revision: acknowledgement.workspace_identity_revision,
            workspace_trust_generation: acknowledgement.workspace_trust_generation,
            runtime_public_key_fingerprint: acknowledgement.runtime_public_key_fingerprint.clone(),
            runtime_identity_revision: acknowledgement.runtime_identity_revision,
            verified_at: unix_now_i64(),
        })
        .map_err(|error| {
            RuntimeHttpRestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "workspace_runtime_verification_unavailable",
                error.to_string(),
            )
        })?;
    Ok(Json(WorkspaceRuntimeVerificationReceipt {
        challenge_id: acknowledgement.challenge_id,
        workspace_id: acknowledgement.workspace_id,
        runtime_id: acknowledgement.runtime_id,
        binding_revision: acknowledgement.binding_revision,
        accepted_at: unix_now_i64(),
    }))
}

#[allow(dead_code)]
async fn get_runtime_ping(
    State(state): State<RuntimeHttpState>,
    Extension(auth): Extension<RuntimeAuthContext>,
    headers: HeaderMap,
) -> RestResult<RuntimeHttpPingResponse> {
    let requested_workspace_id = headers
        .get(RUNTIME_WORKSPACE_SCOPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RuntimeHttpRestError::new(
                StatusCode::FORBIDDEN,
                "runtime_ping_workspace_scope_required",
                "Runtime ping requires the target Workspace scope",
            )
        })?;
    if requested_workspace_id != auth.workspace_id {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "runtime_ping_workspace_scope_mismatch",
            "Runtime ping Workspace scope does not match the authenticated capability",
        ));
    }
    let runtime_id = state
        .workspace_auth
        .as_ref()
        .map(|auth| auth.signer.runtime_id().trim())
        .filter(|runtime_id| !runtime_id.is_empty())
        .ok_or_else(|| {
            RuntimeHttpRestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_ping_identity_unavailable",
                "Runtime ping identity is not configured",
            )
        })?;
    Ok(Json(RuntimeHttpPingResponse {
        runtime_id: runtime_id.to_string(),
        protocol_version: RUNTIME_HTTP_PROTOCOL_VERSION,
    }))
}

#[allow(dead_code)]
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

async fn observe_workspace_prompt_projection(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    body: Result<Json<RuntimeHttpWorkspacePromptProjectionRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkspacePromptProjectionResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    if let Some(scope) = auth_workspace_scope(&state, auth.as_ref())?
        && request.projection.workspace_id != scope.workspace_id
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "workspace_scope_mismatch",
            "Workspace Prompt projection is outside the authenticated Workspace scope",
        ));
    }
    let workspace_id = request.projection.workspace_id.clone();
    let config_revision = request.projection.config_revision;
    state
        .runtime
        .observe_workspace_prompt_projection(request.projection)
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkspacePromptProjectionResponse {
        workspace_id,
        config_revision,
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

#[allow(dead_code)]
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

async fn authorize_working_directory_repository_access(
    State(state): State<RuntimeHttpState>,
    Extension(auth): Extension<RuntimeAuthContext>,
    body: Result<Json<WorkingDirectoryRepositoryAccessRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpRepositoryAccessResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    if request.materialization.workspace_id != auth.workspace_id {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "working_directory_materialization_workspace_mismatch",
            "Repository access authority does not match the authenticated Workspace",
        ));
    }
    state
        .runtime
        .authorize_working_directory_repository_access_from_resource(request)
        .await
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpRepositoryAccessResponse {
        authorized: true,
    }))
}

async fn probe_repository_ssh_host_keys(
    State(state): State<RuntimeHttpState>,
    Extension(_auth): Extension<RuntimeAuthContext>,
    body: Result<Json<SshHostKeyProbeRequest>, JsonRejection>,
) -> RestResult<SshHostKeyProbeResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let response = probe_ssh_host_keys_with_program(
        &request,
        state.ssh_keyscan_program.as_path(),
        SSH_KEYSCAN_TIMEOUT,
    )
    .await
    .map_err(|error| match error {
        SshHostKeyProbeError::InvalidHostname | SshHostKeyProbeError::InvalidPort => {
            RuntimeHttpRestError::new(
                StatusCode::BAD_REQUEST,
                "ssh_host_key_probe_invalid_request",
                error.to_string(),
            )
        }
        SshHostKeyProbeError::Unavailable => RuntimeHttpRestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ssh_host_key_probe_unavailable",
            error.to_string(),
        ),
        SshHostKeyProbeError::Timeout => RuntimeHttpRestError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "ssh_host_key_probe_timeout",
            error.to_string(),
        ),
        SshHostKeyProbeError::Failed { .. } => RuntimeHttpRestError::new(
            StatusCode::BAD_GATEWAY,
            "ssh_host_key_probe_failed",
            error.to_string(),
        ),
    })?;
    Ok(Json(response))
}

async fn observe_repository_ref(
    State(state): State<RuntimeHttpState>,
    Extension(auth): Extension<RuntimeAuthContext>,
    body: Result<Json<RepositoryRefObservationRequest>, JsonRejection>,
) -> RestResult<crate::catalog::RepositoryRefObservation> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    if request
        .materialization
        .as_ref()
        .is_some_and(|materialization| materialization.workspace_id != auth.workspace_id)
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "repository_ref_observation_workspace_mismatch",
            "Repository ref observation authority does not match the authenticated Workspace",
        ));
    }
    let observation = state
        .runtime
        .observe_repository_ref_from_resource(request)
        .await
        .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(observation))
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
    Extension(auth): Extension<RuntimeAuthContext>,
    body: Result<Json<WorkingDirectoryRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkingDirectoryResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    if let Some(materialization) = request.materialization.as_ref()
        && materialization.workspace_id != auth.workspace_id
    {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "working_directory_materialization_workspace_mismatch",
            "Repository materialization authority does not match the authenticated Workspace",
        ));
    }
    let working_directory = state
        .runtime
        .create_working_directory_from_resource(request)
        .await
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

async fn open_workdir_session(
    State(state): State<RuntimeHttpState>,
    Path(working_directory_id): Path<String>,
    auth: Option<Extension<RuntimeAuthContext>>,
    body: Result<Json<OpenWorkdirSessionRequest>, JsonRejection>,
) -> Result<Json<OpenWorkdirSessionResponse>, RuntimeHttpWorkdirError> {
    let Json(request) = body.map_err(|_| RuntimeHttpWorkdirError::invalid_request())?;
    let owner = required_workdir_owner(auth)?;
    let owner_worker_ref = request
        .owner_worker_id
        .as_deref()
        .map(|worker_id| {
            WorkerId::parse(worker_id)
                .map(WorkerRef::new)
                .ok_or_else(RuntimeHttpWorkdirError::invalid_request)
        })
        .transpose()?;
    let session = state
        .runtime
        .open_workdir_session_scoped(&owner, &working_directory_id, owner_worker_ref.as_ref())
        .map_err(RuntimeHttpWorkdirError::runtime)?;
    let session_id =
        WorkdirSessionId::new(new_token_id().map_err(|_| RuntimeHttpWorkdirError::internal())?)
            .map_err(|_| RuntimeHttpWorkdirError::internal())?;
    let response = OpenWorkdirSessionResponse {
        session_id: session_id.clone(),
        workdir_id: session.workdir().id().clone(),
        capabilities: session.capabilities(),
    };
    state
        .workdir_sessions
        .lock()
        .map_err(|_| RuntimeHttpWorkdirError::internal())?
        .insert(
            session_id.as_str().to_string(),
            RuntimeHttpWorkdirSession { owner, session },
        );
    Ok(Json(response))
}

async fn run_workdir_session_operation(
    State(state): State<RuntimeHttpState>,
    Path(session_id): Path<String>,
    auth: Option<Extension<RuntimeAuthContext>>,
    body: Result<Json<WorkdirSessionOperationRequest>, JsonRejection>,
) -> Result<Json<WorkdirSessionOperationResult>, RuntimeHttpWorkdirError> {
    let Json(request) = body.map_err(|_| RuntimeHttpWorkdirError::invalid_request())?;
    let owner = required_workdir_owner(auth)?;
    let source = {
        let sessions = state
            .workdir_sessions
            .lock()
            .map_err(|_| RuntimeHttpWorkdirError::internal())?;
        let record = sessions
            .get(&session_id)
            .filter(|record| record.owner == owner)
            .ok_or_else(RuntimeHttpWorkdirError::not_found)?;
        record.session.clone()
    };
    let session = source.as_ref();
    let operation = request.operation;

    let result = match operation {
        WorkdirSessionOperation::CommandOutput(request) if request.wait => {
            let cursor = request.cursor;
            let output = match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                session.command_output(request),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => CommandOutput {
                    exit_code: None,
                    timed_out: false,
                    status: CommandStatus::Running,
                    content: String::new(),
                    next_cursor: Some(cursor),
                    truncated: false,
                    output_path: None,
                },
            };
            WorkdirSessionOperationResult::CommandOutput(output)
        }
        operation => dispatch_workdir_session_operation(session, operation).await?,
    };
    Ok(Json(result))
}

async fn close_workdir_session(
    State(state): State<RuntimeHttpState>,
    Path(session_id): Path<String>,
    auth: Option<Extension<RuntimeAuthContext>>,
) -> Result<StatusCode, RuntimeHttpWorkdirError> {
    let owner = required_workdir_owner(auth)?;
    let session = {
        let mut sessions = state
            .workdir_sessions
            .lock()
            .map_err(|_| RuntimeHttpWorkdirError::internal())?;
        if sessions
            .get(&session_id)
            .is_some_and(|record| record.owner == owner)
        {
            sessions.remove(&session_id).map(|record| record.session)
        } else {
            None
        }
    };
    if let Some(session) = session {
        session.close().await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn required_workdir_owner(
    auth: Option<Extension<RuntimeAuthContext>>,
) -> Result<RuntimeWorkspaceScope, RuntimeHttpWorkdirError> {
    let Extension(auth) = auth.ok_or_else(RuntimeHttpWorkdirError::forbidden)?;
    if auth.workspace_id.trim().is_empty() || auth.server_id.trim().is_empty() {
        return Err(RuntimeHttpWorkdirError::forbidden());
    }
    Ok(RuntimeWorkspaceScope::new(
        auth.workspace_id,
        auth.server_id,
    ))
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
async fn replace_worker_workspace_api(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Result<Json<RuntimeHttpWorkerWorkspaceApiRequest>, JsonRejection>,
) -> RestResult<RuntimeHttpWorkerResponse> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let worker = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => state.runtime.replace_worker_workspace_api_scoped(
            &scope,
            &worker_ref,
            request.workspace_api,
        ),
        None => state
            .runtime
            .replace_worker_workspace_api(&worker_ref, request.workspace_api),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerResponse { worker }))
}

#[cfg(feature = "ws-server")]
const RUNTIME_PROTOCOL_OUTBOUND_CAPACITY: usize = 256;

#[cfg(feature = "ws-server")]
async fn runtime_protocol_ws(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, Response> {
    let scope =
        auth_workspace_scope(&state, auth.as_ref()).map_err(|error| error.into_response())?;
    Ok(ws
        .on_upgrade(move |socket| runtime_protocol_ws_session(state.runtime, scope, socket))
        .into_response())
}

#[cfg(feature = "ws-server")]
async fn runtime_protocol_ws_session(
    runtime: Runtime,
    scope: Option<RuntimeWorkspaceScope>,
    socket: axum::extract::ws::WebSocket,
) {
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let (outbound, mut outbound_receiver) = tokio::sync::mpsc::channel::<axum::extract::ws::Message>(
        RUNTIME_PROTOCOL_OUTBOUND_CAPACITY,
    );
    let writer = tokio::spawn(async move {
        while let Some(message) = outbound_receiver.recv().await {
            if socket_sender.send(message).await.is_err() {
                break;
            }
        }
    });

    let mut next_subscription_id = 1_u64;
    let mut subscriptions = HashMap::<SubscriptionId, tokio::task::JoinHandle<()>>::new();
    while let Some(message) = socket_receiver.next().await {
        let Ok(message) = message else {
            break;
        };
        match message {
            axum::extract::ws::Message::Text(text) => {
                let Ok(frame) = serde_json::from_str::<SubscriptionFrame>(text.as_str()) else {
                    break;
                };
                let request_id = subscription_frame_request_id(&frame);
                if let Err(error) = frame.validate() {
                    let Some(request_id) = request_id else {
                        break;
                    };
                    let code = if frame.protocol_version
                        != protocol::subscription::SUBSCRIPTION_PROTOCOL_VERSION
                    {
                        SubscriptionRejectionCode::UnsupportedProtocolVersion
                    } else {
                        SubscriptionRejectionCode::InvalidRequest
                    };
                    if send_runtime_subscription_frame(
                        &outbound,
                        SubscriptionFrame::new(SubscriptionFramePayload::Response(
                            SubscriptionResponse::SubscriptionRejected {
                                request_id,
                                subscription_id: None,
                                code,
                                message: error.to_string(),
                            },
                        )),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    continue;
                }

                subscriptions.retain(|_, task| !task.is_finished());
                let SubscriptionFramePayload::Request(request) = frame.payload else {
                    break;
                };
                match request {
                    SubscriptionRequest::SubscribeEvents {
                        request_id,
                        selector,
                    } => {
                        let subscription = match scope.as_ref() {
                            Some(scope) => {
                                runtime.subscribe_event_selector_scoped(scope, selector.clone())
                            }
                            None => runtime.subscribe_event_selector(selector.clone()),
                        };
                        let mut subscription = match subscription {
                            Ok(subscription) => subscription,
                            Err(error) => {
                                if send_runtime_subscription_frame(
                                    &outbound,
                                    SubscriptionFrame::new(SubscriptionFramePayload::Response(
                                        SubscriptionResponse::SubscriptionRejected {
                                            request_id,
                                            subscription_id: None,
                                            code: runtime_subscription_rejection_code(&error),
                                            message: error.to_string(),
                                        },
                                    )),
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };
                        let subscription_id = SubscriptionId::new(format!(
                            "runtime-subscription-{next_subscription_id}"
                        ))
                        .expect("generated Runtime subscription id is valid");
                        next_subscription_id = next_subscription_id.saturating_add(1);
                        let response = SubscriptionFrame::new(SubscriptionFramePayload::Response(
                            SubscriptionResponse::Subscribed {
                                request_id,
                                subscription_id: subscription_id.clone(),
                                selector: selector.clone(),
                                snapshot: subscription.snapshot().clone(),
                            },
                        ));
                        if send_runtime_subscription_frame(&outbound, response)
                            .await
                            .is_err()
                        {
                            break;
                        }

                        let event_outbound = outbound.clone();
                        let event_subscription_id = subscription_id.clone();
                        let task = tokio::spawn(async move {
                            loop {
                                let frame = match subscription.recv().await {
                                    Ok(update) => SubscriptionFrame::new(
                                        SubscriptionFramePayload::Event(
                                            SubscriptionEvent::Event {
                                                subscription_id: event_subscription_id.clone(),
                                                payload: update.payload,
                                            },
                                        ),
                                    ),
                                    Err(RuntimeSubscriptionRecvError::Lagged) => {
                                        SubscriptionFrame::new(SubscriptionFramePayload::Event(
                                            SubscriptionEvent::SubscriptionClosed {
                                                subscription_id: event_subscription_id.clone(),
                                                code: SubscriptionTerminationCode::Lagged,
                                                message: "Runtime subscription lagged; resubscribe for a fresh snapshot"
                                                    .to_string(),
                                            },
                                        ))
                                    }
                                    Err(RuntimeSubscriptionRecvError::Closed) => {
                                        SubscriptionFrame::new(SubscriptionFramePayload::Event(
                                            SubscriptionEvent::SubscriptionClosed {
                                                subscription_id: event_subscription_id.clone(),
                                                code: SubscriptionTerminationCode::ServerShutdown,
                                                message: "Runtime subscription closed".to_string(),
                                            },
                                        ))
                                    }
                                };
                                let terminal = matches!(
                                    &frame.payload,
                                    SubscriptionFramePayload::Event(
                                        SubscriptionEvent::SubscriptionClosed { .. }
                                    )
                                );
                                if send_runtime_subscription_frame(&event_outbound, frame)
                                    .await
                                    .is_err()
                                    || terminal
                                {
                                    break;
                                }
                            }
                        });
                        subscriptions.insert(subscription_id, task);
                    }
                    SubscriptionRequest::UnsubscribeEvents {
                        request_id,
                        subscription_id,
                    } => {
                        if let Some(task) = subscriptions.remove(&subscription_id) {
                            task.abort();
                        }
                        if send_runtime_subscription_frame(
                            &outbound,
                            SubscriptionFrame::new(SubscriptionFramePayload::Response(
                                SubscriptionResponse::Unsubscribed {
                                    request_id,
                                    subscription_id,
                                },
                            )),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            axum::extract::ws::Message::Ping(payload) => {
                if outbound
                    .send(axum::extract::ws::Message::Pong(payload))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            axum::extract::ws::Message::Pong(_) => {}
            axum::extract::ws::Message::Close(_) => break,
            axum::extract::ws::Message::Binary(_) => break,
        }
    }

    for (_, task) in subscriptions {
        task.abort();
    }
    drop(outbound);
    let _ = writer.await;
}

#[cfg(feature = "ws-server")]
fn subscription_frame_request_id(
    frame: &SubscriptionFrame,
) -> Option<protocol::subscription::SubscriptionRequestId> {
    let SubscriptionFramePayload::Request(request) = &frame.payload else {
        return None;
    };
    Some(match request {
        SubscriptionRequest::SubscribeEvents { request_id, .. }
        | SubscriptionRequest::UnsubscribeEvents { request_id, .. } => request_id.clone(),
    })
}

#[cfg(feature = "ws-server")]
fn runtime_subscription_rejection_code(error: &RuntimeError) -> SubscriptionRejectionCode {
    match error {
        RuntimeError::WorkerNotFound { .. } => SubscriptionRejectionCode::ResourceNotFound,
        RuntimeError::InvalidRequest(_) => SubscriptionRejectionCode::UnsupportedSelector,
        _ => SubscriptionRejectionCode::Internal,
    }
}

#[cfg(feature = "ws-server")]
async fn send_runtime_subscription_frame(
    outbound: &tokio::sync::mpsc::Sender<axum::extract::ws::Message>,
    frame: SubscriptionFrame,
) -> Result<(), ()> {
    frame.validate().map_err(|_| ())?;
    let text = serde_json::to_string(&frame).map_err(|_| ())?;
    outbound
        .send(axum::extract::ws::Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

#[cfg(feature = "ws-server")]
async fn worker_protocol_ws(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    Query(query): Query<RuntimeWorkerEventsWsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, RuntimeHttpRestError> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let scope = auth_workspace_scope(&state, auth.as_ref())?;
    let input_source = authenticated_protocol_input_source(&headers)?;
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
            worker_protocol_ws_session(
                state.runtime,
                scope,
                worker_ref,
                query,
                input_source,
                socket,
            )
        })
        .into_response())
}

#[cfg(feature = "ws-server")]
fn authenticated_protocol_input_source(
    headers: &HeaderMap,
) -> Result<Option<protocol::AuthenticatedInputSource>, RuntimeHttpRestError> {
    let Some(value) = headers.get(protocol::AUTHENTICATED_ACCOUNT_ID_HEADER) else {
        return Ok(None);
    };
    let account_id = value.to_str().map_err(|_| {
        RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "authenticated_input_source_invalid",
            "authenticated Worker input source is invalid",
        )
    })?;
    if account_id.trim().is_empty() || account_id.len() > 128 {
        return Err(RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "authenticated_input_source_invalid",
            "authenticated Worker input source is invalid",
        ));
    }
    Ok(Some(protocol::AuthenticatedInputSource::Account {
        account_id: account_id.to_owned(),
    }))
}

#[cfg(feature = "ws-server")]
fn authorize_runtime_protocol_method(
    method: protocol::Method,
    transport_source: Option<&protocol::AuthenticatedInputSource>,
) -> protocol::Method {
    match method {
        protocol::Method::SubmitTracked {
            submission_request_id,
            input,
            ..
        } => protocol::Method::SubmitTracked {
            source: transport_source.cloned().unwrap_or_else(|| {
                protocol::AuthenticatedInputSource::Backend {
                    operation_id: submission_request_id.clone(),
                }
            }),
            submission_request_id,
            input,
        },
        protocol::Method::SubmitIfIdle {
            submission_request_id,
            input,
            ..
        } => protocol::Method::SubmitIfIdle {
            source: transport_source.cloned().unwrap_or_else(|| {
                protocol::AuthenticatedInputSource::Backend {
                    operation_id: submission_request_id.clone(),
                }
            }),
            submission_request_id,
            input,
        },
        protocol::Method::NotifyTracked {
            notification_request_id,
            message,
            ..
        } => protocol::Method::NotifyTracked {
            source: transport_source.cloned().unwrap_or_else(|| {
                protocol::AuthenticatedInputSource::Backend {
                    operation_id: notification_request_id.clone(),
                }
            }),
            notification_request_id,
            message,
        },
        other => other,
    }
}

#[cfg(feature = "ws-server")]
async fn worker_protocol_ws_session(
    runtime: Runtime,
    scope: Option<RuntimeWorkspaceScope>,
    worker_ref: WorkerRef,
    query: RuntimeWorkerEventsWsQuery,
    input_source: Option<protocol::AuthenticatedInputSource>,
    mut socket: WebSocket,
) {
    if let Some(raw) = query.cursor.as_deref()
        && WorkerObservationCursor::decode(raw).is_none()
    {
        let event = protocol_error_event("malformed worker observation cursor");
        let _ = send_protocol_event(&mut socket, &event).await;
        return;
    }
    // The complete Controller snapshot reconstitutes this protocol connection.
    // A WorkerRef-wide catalog cursor cannot attribute historical operational
    // events to this captured execution; never append that bus after its snapshot.
    let attached = match scope.as_ref() {
        Some(scope) => runtime.attach_worker_protocol_scoped(scope, &worker_ref),
        None => runtime.attach_worker_protocol(&worker_ref),
    };
    let mut transport = match attached {
        Ok(transport) => transport,
        Err(error) => {
            let event = protocol_error_event(error.to_string());
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };
    if !send_protocol_event(&mut socket, &transport.snapshot).await {
        return;
    }

    loop {
        tokio::select! {
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(WsMessage::Text(text))) => match decode_method(&text) {
                        Ok(method) => {
                            let method =
                                authorize_runtime_protocol_method(method, input_source.as_ref());
                            let result = match scope.as_ref() {
                                Some(scope) => {
                                    runtime.send_connected_protocol_method_scoped(
                                        scope, &worker_ref, &transport, method,
                                    )
                                }
                                None => runtime.send_connected_protocol_method(
                                    &worker_ref, &transport, method,
                                ),
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
                                    if transport.validate().is_err() {
                                        let _ = socket.send(WsMessage::Close(None)).await;
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
            event = transport.events.recv() => {
                match event {
                    Some(event) => {
                        let terminal = matches!(event, protocol::Event::Shutdown);
                        if !send_protocol_event(&mut socket, &event).await {
                            return;
                        }
                        if terminal {
                            let _ = socket.send(WsMessage::Close(None)).await;
                            return;
                        }
                    }
                    None => {
                        let _ = socket.send(WsMessage::Close(None)).await;
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

#[allow(dead_code)]
async fn worker_retention_inventory(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
) -> RestResult<WorkerRetentionInventory> {
    let scope = auth_workspace_scope(&state, auth.as_ref())?.ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "workspace_scope_required",
            "Worker retention inventory requires workspace-scoped authorization",
        )
    })?;
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    state
        .runtime
        .worker_retention_inventory(&scope.workspace_id, &worker_ref)
        .map(Json)
        .map_err(RuntimeHttpRestError::runtime)
}

#[allow(dead_code)]
async fn execute_worker_retention(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    body: Result<Json<WorkerRetentionExecutionRequest>, JsonRejection>,
) -> RestResult<WorkerRetentionExecutionResult> {
    let Json(request) = body.map_err(RuntimeHttpRestError::json_rejection)?;
    if request.worker_id.to_string() != worker_id {
        return Err(RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "worker_id_mismatch",
            "Retention request worker_id does not match the route",
        ));
    }
    let scope = auth_workspace_scope(&state, auth.as_ref())?.ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "workspace_scope_required",
            "Worker retention execution requires workspace-scoped authorization",
        )
    })?;
    if request.workspace_id != scope.workspace_id {
        return Err(RuntimeHttpRestError::new(
            StatusCode::NOT_FOUND,
            "worker_not_found",
            "Worker was not found in the authenticated Workspace",
        ));
    }
    state
        .runtime
        .execute_worker_retention(&request)
        .map(Json)
        .map_err(RuntimeHttpRestError::runtime)
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
            request.context.as_ref(),
        ),
        None => state.runtime.worker_completions(
            &worker_ref,
            request.kind,
            &request.prefix,
            request.context.as_ref(),
        ),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpWorkerCompletionsResponse {
        kind: request.kind,
        prefix: request.prefix,
        context: request.context,
        request_id: request.request_id,
        entries,
    }))
}

async fn upload_worker_file(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path(worker_id): Path<String>,
    Query(query): Query<RuntimeHttpUploadFileQuery>,
    body: Bytes,
) -> RestResult<RuntimeHttpUploadedFileResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let context = match (
        query.upload_id,
        query.principal_id,
        query.workspace_id,
        query.runtime_id,
        query.owner_worker_id,
    ) {
        (None, None, None, None, None) => None,
        (
            Some(upload_id),
            Some(principal_id),
            Some(workspace_id),
            Some(runtime_id),
            Some(owner_worker_id),
        ) => {
            if owner_worker_id != worker_ref.worker_id.to_string() {
                return Err(RuntimeHttpRestError::new(
                    StatusCode::FORBIDDEN,
                    "uploaded_file_owner_mismatch",
                    "uploaded file context does not match the target Worker",
                ));
            }
            Some(session_store::UploadedFileUploadContext {
                upload_id,
                principal_id,
                workspace_id,
                runtime_id,
                worker_id: owner_worker_id,
            })
        }
        _ => {
            return Err(RuntimeHttpRestError::new(
                StatusCode::BAD_REQUEST,
                "uploaded_file_context_incomplete",
                "uploaded file context fields must be provided together",
            ));
        }
    };
    let file = match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => {
            if context
                .as_ref()
                .is_some_and(|context| context.workspace_id != scope.workspace_id)
            {
                return Err(RuntimeHttpRestError::new(
                    StatusCode::FORBIDDEN,
                    "uploaded_file_workspace_mismatch",
                    "uploaded file context does not match the authenticated Workspace",
                ));
            }
            match context.as_ref() {
                Some(context) => state.runtime.upload_worker_file_with_context_scoped(
                    &scope,
                    &worker_ref,
                    &query.file_name,
                    &query.media_type,
                    &body,
                    context,
                ),
                None => state.runtime.upload_worker_file_scoped(
                    &scope,
                    &worker_ref,
                    &query.file_name,
                    &query.media_type,
                    &body,
                ),
            }
        }
        None => state.runtime.upload_worker_file(
            &worker_ref,
            &query.file_name,
            &query.media_type,
            &body,
        ),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpUploadedFileResponse { file }))
}

async fn delete_worker_uploaded_file(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path((worker_id, artifact_id)): Path<(String, String)>,
) -> RestResult<RuntimeHttpUploadedFileDeleteResponse> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    match auth_workspace_scope(&state, auth.as_ref())? {
        Some(scope) => {
            state
                .runtime
                .delete_worker_uploaded_file_scoped(&scope, &worker_ref, &artifact_id)
        }
        None => state
            .runtime
            .delete_worker_uploaded_file(&worker_ref, &artifact_id),
    }
    .map_err(RuntimeHttpRestError::runtime)?;
    Ok(Json(RuntimeHttpUploadedFileDeleteResponse {
        deleted: true,
    }))
}

async fn get_worker_session_attachment(
    State(state): State<RuntimeHttpState>,
    auth: Option<Extension<RuntimeAuthContext>>,
    Path((worker_id, session_id, attachment_id)): Path<(String, String, String)>,
    Query(request): Query<runtime_api::WorkerSessionRequest>,
) -> Result<Response, RuntimeHttpRestError> {
    let worker_ref = worker_ref_for(&state.runtime, worker_id)?;
    let scope = auth_workspace_scope(&state, auth.as_ref())?.ok_or_else(|| {
        RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "runtime_worker_session_scope_required",
            "Worker Session attachment reads require a Workspace-scoped capability",
        )
    })?;
    if scope.workspace_id != request.workspace_id {
        return Err(RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "runtime_worker_session_workspace_scope_mismatch",
            "Worker Session Workspace scope does not match the authenticated capability",
        ));
    }
    let attachment = state
        .runtime
        .worker_session_attachment_scoped(&scope, &worker_ref, session_id, attachment_id)
        .map_err(|error| {
            use session_store::RetainedAttachmentReadError as Error;
            let (status, code) = match error {
                Error::RetentionMissing | Error::ActivePointerMissing => {
                    (StatusCode::GONE, "session_attachment_expired")
                }
                Error::SessionMismatch | Error::NotFound => {
                    (StatusCode::NOT_FOUND, "session_attachment_not_found")
                }
                Error::ResourceLimit => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "session_attachment_too_large",
                ),
                Error::MigrationRequired | Error::CorruptLog | Error::StorageUnavailable => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "session_attachment_unavailable",
                ),
            };
            RuntimeHttpRestError::new(status, code, error.to_string())
        })?;
    let content_type = axum::http::HeaderValue::from_str(&attachment.media_type).map_err(|_| {
        RuntimeHttpRestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_attachment_media_type_invalid",
            "Session attachment media type is invalid",
        )
    })?;
    Ok(([(header::CONTENT_TYPE, content_type)], attachment.data).into_response())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);

    if let Some(workspace_auth) = state.workspace_auth.as_deref()
        && let Some(token) = supplied.as_deref()
        && let Ok(claims) = inspect_workspace_capability_claims(token)
    {
        let method = request.method().as_str().to_string();
        let path_and_query = request
            .uri()
            .path_and_query()
            .map_or_else(|| request.uri().path().to_string(), ToString::to_string);
        let required_permission =
            workspace_runtime_operation(request.method(), request.uri().path());
        let expected_worker_id = worker_id_from_runtime_path(request.uri().path());
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, 8 * 1024 * 1024).await {
            Ok(body) => body,
            Err(_) => {
                return RuntimeHttpRestError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request_body_too_large",
                    "Runtime request body exceeds the verification limit",
                )
                .into_response();
            }
        };
        let body_digest = workspace_request_body_digest(&body);
        let expected = WorkspaceCapabilityExpectation {
            workspace_id: &claims.issuer_workspace_id,
            binding_revision: claims.binding_revision,
            runtime_id: workspace_auth.signer.runtime_id(),
            worker_id: expected_worker_id.as_deref(),
            operation: required_permission,
            method: &method,
            path_and_query: &path_and_query,
            body_digest: &body_digest,
            now_unix: unix_now_i64(),
        };
        match workspace_auth.verifier.verify(token, &expected) {
            Ok(verified) => {
                let is_verification = path_and_query
                    == runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE
                    || path_and_query == runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK;
                if !is_verification {
                    let record = match workspace_auth
                        .verifications
                        .get(&verified.workspace_id, workspace_auth.signer.runtime_id())
                    {
                        Ok(Some(record)) => record,
                        Ok(None) => {
                            return RuntimeHttpRestError::new(
                                StatusCode::FORBIDDEN,
                                "workspace_runtime_verification_required",
                                "Workspace Runtime binding has not completed signed verification",
                            )
                            .into_response();
                        }
                        Err(error) => {
                            return RuntimeHttpRestError::new(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "workspace_runtime_verification_unavailable",
                                error.to_string(),
                            )
                            .into_response();
                        }
                    };
                    if record.binding_revision != verified.binding_revision
                        || record.workspace_key_id != verified.issuer_key_id
                        || record.workspace_identity_revision != verified.issuer_identity_revision
                        || record.workspace_trust_generation != verified.trust_generation
                        || record.runtime_public_key_fingerprint
                            != workspace_auth.signer.public_key_fingerprint()
                        || record.runtime_identity_revision == 0
                    {
                        return RuntimeHttpRestError::new(
                            StatusCode::FORBIDDEN,
                            "workspace_runtime_verification_stale",
                            "Workspace Runtime verification does not match current request authority",
                        )
                        .into_response();
                    }
                }
                request = Request::from_parts(parts, Body::from(body));
                request.extensions_mut().insert(verified.clone());
                request.extensions_mut().insert(RuntimeAuthContext {
                    server_id: verified.issuer,
                    workspace_id: verified.workspace_id,
                    permissions: vec![required_permission.to_string()],
                    token_id: verified.token_id,
                    expires_at: u64::try_from(verified.expires_at).unwrap_or(0),
                });
                let context = request.extensions().get::<RuntimeAuthContext>().cloned();
                return runtime_management_api::scope_auth(context, next.run(request)).await;
            }
            Err(error) => {
                return RuntimeHttpRestError::new(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    format!("invalid Workspace capability token: {error}"),
                )
                .into_response();
            }
        }
    }

    let workspace_bootstrap_request = request.method() == Method::POST
        && matches!(
            request.uri().path(),
            runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE
                | runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK
        );
    if state.workspace_auth.is_some() && !workspace_bootstrap_request {
        let local_token_matches = state
            .local_token
            .as_deref()
            .is_some_and(|expected| supplied.as_deref() == Some(expected));
        if !local_token_matches {
            return RuntimeHttpRestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid Workspace capability bearer token",
            )
            .into_response();
        }
    }

    if let Some(expected) = state.local_token.as_deref() {
        if supplied.as_deref() != Some(expected) {
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
    let context = request.extensions().get::<RuntimeAuthContext>().cloned();
    runtime_management_api::scope_auth(context, next.run(request)).await
}

fn auth_workspace_scope(
    state: &RuntimeHttpState,
    auth: Option<&Extension<RuntimeAuthContext>>,
) -> Result<Option<RuntimeWorkspaceScope>, RuntimeHttpRestError> {
    let Some(Extension(context)) = auth else {
        if state.workspace_auth.is_some() || state.local_token.is_some() {
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

fn workspace_runtime_operation(method: &Method, path: &str) -> &'static str {
    if (path == runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE
        || path == runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK)
        && *method == Method::POST
    {
        return WORKSPACE_VERIFICATION_OPERATION;
    }
    required_runtime_permission(method, path).unwrap_or("runtime:read")
}

fn worker_id_from_runtime_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/v1/workers/")?;
    let worker_id = rest.split('/').next()?;
    (!worker_id.is_empty()).then(|| worker_id.to_string())
}

fn required_runtime_permission(method: &Method, path: &str) -> Option<&'static str> {
    if path == "/v1/ping" && *method == Method::GET {
        return Some(RUNTIME_PING_PERMISSION);
    }
    if path == "/v1/runtime" {
        return None;
    }
    if path == "/v1/workers" && *method == Method::GET {
        return Some("workers:list");
    }
    if path == "/v1/workers" && *method == Method::POST {
        return Some("workers:create");
    }
    if (path == runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS
        || path == runtime_api::RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE
        || path == runtime_api::RUNTIME_ROUTE_SSH_PROBE)
        && *method == Method::POST
    {
        return Some(SSH_HOST_KEY_PROBE_OPERATION);
    }
    if path.starts_with("/v1/workdir-sessions")
        || (path.starts_with("/v1/working-directories/") && path.ends_with("/sessions"))
    {
        return Some("workdirs:operate");
    }
    if path.starts_with(runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLES)
        || path.starts_with(runtime_api::RUNTIME_ROUTE_WORKSPACE_PROMPT_PROJECTIONS)
        || path.starts_with(runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORIES)
    {
        return Some("workers:create");
    }
    if path.ends_with("/workspace-api") {
        return Some("workers:create");
    }
    if path.ends_with("/input")
        || path.ends_with("/restore")
        || path.ends_with("/restore/coordinate")
        || (path.contains("/attachments") && *method != Method::GET)
        || path.contains("/workdir-attachments")
    {
        return Some("workers:input");
    }
    if path.ends_with("/stop") || path.ends_with("/cancel") {
        return Some("workers:stop");
    }
    if path == runtime_api::RUNTIME_ROUTE_PROTOCOL_WS {
        return Some("workers:list");
    }
    if path.ends_with("/protocol") || path.ends_with("/protocol/ws") {
        return Some("workers:protocol");
    }
    if path.ends_with("/completions") {
        return Some("workers:read");
    }
    if path.contains("/retention/") {
        return Some("workers:delete");
    }
    if path.starts_with("/v1/workers/") && *method == Method::DELETE {
        return Some("workers:delete");
    }
    if path.starts_with("/v1/session-public/") && *method == Method::POST {
        return Some("workers:read");
    }
    if path.starts_with("/v1/workers/") && *method == Method::GET {
        return Some("workers:read");
    }
    None
}

#[derive(Debug)]
struct RuntimeHttpWorkdirError {
    status: StatusCode,
    payload: WorkdirTransportError,
}

impl RuntimeHttpWorkdirError {
    fn new(
        status: StatusCode,
        code: WorkdirTransportErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            payload: WorkdirTransportError {
                code,
                message: message.into(),
            },
        }
    }

    fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            WorkdirTransportErrorCode::InvalidRequest,
            "Workdir operation request is invalid",
        )
    }

    fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            WorkdirTransportErrorCode::Unavailable,
            "Workdir session is unavailable",
        )
    }

    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            WorkdirTransportErrorCode::NotFound,
            "Workdir session was not found",
        )
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            WorkdirTransportErrorCode::Internal,
            "Workdir operation failed",
        )
    }

    fn runtime(error: RuntimeError) -> Self {
        match error {
            RuntimeError::WorkingDirectory(diagnostic)
                if diagnostic.code == "working_directory_not_found" =>
            {
                Self::not_found()
            }
            RuntimeError::WorkspaceOwnerMismatch { .. } => Self::not_found(),
            RuntimeError::InvalidRequest(_) => Self::invalid_request(),
            _ => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                WorkdirTransportErrorCode::Unavailable,
                "Workdir session is unavailable",
            ),
        }
    }
}

impl From<workdir::WorkdirError> for RuntimeHttpWorkdirError {
    fn from(error: workdir::WorkdirError) -> Self {
        let payload = WorkdirTransportError::from_workdir_error(&error);
        let status = StatusCode::from_u16(payload.code.http_status())
            .expect("Workdir transport error status is valid");
        Self { status, payload }
    }
}

impl IntoResponse for RuntimeHttpWorkdirError {
    fn into_response(self) -> Response {
        (self.status, Json(self.payload)).into_response()
    }
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
        RuntimeError::WorkingDirectory(diagnostic)
            if matches!(
                diagnostic.code.as_str(),
                "repository_ref_provider_unavailable"
                    | "repository_ref_provider_timeout"
                    | "repository_access_provider_unavailable"
            ) =>
        {
            StatusCode::SERVICE_UNAVAILABLE
        }
        RuntimeError::WorkingDirectory(diagnostic)
            if matches!(
                diagnostic.code.as_str(),
                "repository_ref_provider_auth_failed"
                    | "repository_access_credential_expired"
                    | "repository_access_credential_unavailable"
                    | "repository_access_credential_unauthorized"
                    | "repository_access_credential_invalid"
            ) =>
        {
            StatusCode::FORBIDDEN
        }
        RuntimeError::WorkingDirectory(diagnostic)
            if diagnostic.code == "repository_ref_not_found" =>
        {
            StatusCode::NOT_FOUND
        }
        RuntimeError::InvalidRequest(message)
            if message.contains("already created with a different fingerprint") =>
        {
            StatusCode::CONFLICT
        }
        RuntimeError::RuntimeStopped
        | RuntimeError::RestoreObservationConflict { .. }
        | RuntimeError::RuntimeStoreAlreadyOpen { .. }
        | RuntimeError::WorkerExecutionUnavailable { .. }
        | RuntimeError::ExecutionBackendUnavailable { .. }
        | RuntimeError::WorkerExecutionRejected { .. } => StatusCode::CONFLICT,
        RuntimeError::WorkspaceOwnerMismatch { .. } => StatusCode::FORBIDDEN,
        RuntimeError::LimitTooLarge { .. }
        | RuntimeError::InvalidRequest(_)
        | RuntimeError::InvalidInitialInputKind { .. }
        | RuntimeError::ConfigBundleDigestMismatch { .. }
        | RuntimeError::InvalidProfileSelector { .. }
        | RuntimeError::UnsupportedConfigDeclaration { .. } => StatusCode::BAD_REQUEST,
        RuntimeError::WorkingDirectory(_) => StatusCode::BAD_REQUEST,
        RuntimeError::StoreIo { .. }
        | RuntimeError::StoreCommitOutcomeUnknown { .. }
        | RuntimeError::StoreMissing { .. }
        | RuntimeError::StoreCorrupt { .. }
        | RuntimeError::WorkerDeletePersistenceFailed { .. }
        | RuntimeError::StatePoisoned => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn code_for_runtime_error(error: &RuntimeError) -> String {
    match error {
        RuntimeError::RuntimeStopped => "runtime_stopped".to_string(),
        RuntimeError::RestoreObservationConflict { .. } => {
            "restore_observation_conflict".to_string()
        }
        RuntimeError::RuntimeStoreAlreadyOpen { .. } => "runtime_store_already_open".to_string(),
        RuntimeError::WorkerNotFound { .. } => "worker_not_found".to_string(),
        RuntimeError::WorkerExecutionUnavailable { .. } => {
            "worker_execution_unavailable".to_string()
        }
        RuntimeError::WorkerDeletePersistenceFailed { .. } => {
            "worker_delete_persistence_failed".to_string()
        }
        RuntimeError::ExecutionBackendUnavailable { .. } => {
            "execution_backend_unavailable".to_string()
        }
        RuntimeError::WorkerExecutionRejected { .. } => "worker_execution_rejected".to_string(),
        RuntimeError::WorkspaceOwnerMismatch { .. } => "workspace_owner_mismatch".to_string(),
        RuntimeError::LimitTooLarge { .. } => "limit_too_large".to_string(),
        RuntimeError::InvalidRequest(message)
            if message.contains("already created with a different fingerprint") =>
        {
            "worker_create_conflict".to_string()
        }
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
        RuntimeError::StoreCommitOutcomeUnknown { .. } => {
            "store_commit_outcome_unknown".to_string()
        }
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
    #[error("Runtime HTTP server requires Workspace issuer auth or a local bearer token")]
    AuthRequired,
    #[error("Runtime HTTP server I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::RuntimeIdentityMaterial;
    use crate::catalog::{ConfigBundleRef, ProfileSelector, WorkspaceApiRef};
    use crate::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor,
    };
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionOperation, WorkerExecutionRestoreRequest,
        WorkerExecutionResult, WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
    };
    use crate::management::RuntimeOptions;
    use crate::retention::{DiagnosticsDisposition, SessionDisposition};
    use crate::workspace_issuer::{
        InMemoryWorkspaceClaimReplayProtection, InMemoryWorkspaceRuntimeVerificationAuthority,
        WorkspaceCapabilityClaims, WorkspaceCapabilityVerifier, WorkspaceIssuerTrustRecord,
        WorkspaceIssuerTrustState, issue_workspace_capability_token,
        verify_runtime_verification_response,
    };
    use axum::body::to_bytes;
    use axum::http::Method;
    use manifest::{Scope, SharedScope};
    use runtime_api::WorkerRestoreState;
    use sha2::Digest as _;
    use tower::ServiceExt;

    #[derive(Clone)]
    struct TestRuntimeAuthorizer(String);

    impl runtime_api::client_support::RequestAuthorizer for TestRuntimeAuthorizer {
        fn authorize(
            &self,
            _request: runtime_api::client_support::AuthorizerRequest<'_>,
        ) -> Result<
            runtime_api::client_support::framework::header::HeaderMap,
            runtime_api::client_support::AuthorizationError,
        > {
            let mut headers = runtime_api::client_support::framework::header::HeaderMap::new();
            headers.insert(
                runtime_api::client_support::framework::header::AUTHORIZATION,
                format!("Bearer {}", self.0)
                    .parse()
                    .map_err(|_| runtime_api::client_support::AuthorizationError::new())?,
            );
            Ok(headers)
        }
    }

    use workdir::{
        GrepOutputMode, GrepRequest, LocalWorkdirSession, StatRequest, Workdir, WorkdirPath,
        WorkdirSessionCapabilities,
    };

    #[test]
    fn generated_runtime_routes_are_classified_by_permission_authority() {
        for operation in <runtime_api::RuntimeApiMetadata as runtime_api::ApiContract>::OPERATIONS {
            let method = match operation.method {
                runtime_api::HttpMethod::Get => Method::GET,
                runtime_api::HttpMethod::Post => Method::POST,
                runtime_api::HttpMethod::Put => Method::PUT,
                runtime_api::HttpMethod::Patch => Method::PATCH,
                runtime_api::HttpMethod::Delete => Method::DELETE,
                runtime_api::HttpMethod::Head | runtime_api::HttpMethod::Options => {
                    panic!("unexpected management contract method")
                }
            };
            let path = operation.path.replace("{worker_id}", "worker-1");
            let permission = required_runtime_permission(&method, &path);
            if operation.operation_id == "runtime_summary" {
                assert_eq!(
                    permission, None,
                    "{:?} {}",
                    operation.method, operation.path
                );
            } else {
                assert!(
                    permission.is_some(),
                    "unclassified generated route: {:?} {}",
                    operation.method,
                    operation.path
                );
            }
        }
    }

    #[test]
    fn remaining_route_inventory_covers_every_handwritten_router_path() {
        let inventoried = runtime_api::REMAINING_RUNTIME_ROUTES
            .iter()
            .map(|route| route.path)
            .collect::<std::collections::BTreeSet<_>>();
        for path in [
            runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE,
            runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK,
            runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLES,
            runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLE_AVAILABILITY,
            runtime_api::RUNTIME_ROUTE_WORKSPACE_PROMPT_PROJECTIONS,
            runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORIES,
            runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS,
            runtime_api::RUNTIME_ROUTE_SSH_PROBE,
            runtime_api::RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE,
            runtime_api::RUNTIME_ROUTE_WORKDIR_SESSIONS,
            runtime_api::RUNTIME_ROUTE_WORKDIR_SESSION_OPERATIONS,
            runtime_api::RUNTIME_ROUTE_WORKDIR_SESSION,
            runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY,
            runtime_api::RUNTIME_ROUTE_WORKER_ATTACHMENTS,
            runtime_api::RUNTIME_ROUTE_WORKER_ATTACHMENT,
            runtime_api::RUNTIME_ROUTE_PROTOCOL_WS,
            runtime_api::RUNTIME_ROUTE_WORKER_PROTOCOL_WS,
        ] {
            assert!(
                inventoried.contains(path),
                "missing handwritten route inventory for {path}"
            );
        }
    }

    #[tokio::test]
    async fn workspace_signed_verification_requires_exact_request_and_acknowledges_response() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        runtime
            .store_config_bundle(test_bundle(ProfileSelector::Builtin(
                "builtin:coder".to_string(),
            )))
            .unwrap();
        let workspace_identity = RuntimeIdentityMaterial::generate("workspace-key").unwrap();
        let runtime_identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        let workspace_public_key =
            crate::auth::decode_public_key(&workspace_identity.public_key).unwrap();
        let workspace_fingerprint = format!(
            "sha256:{}",
            crate::workspace_issuer::hex_lower(&sha2::Sha256::digest(workspace_public_key))
        );
        let runtime_public_key =
            crate::auth::decode_public_key(&runtime_identity.public_key).unwrap();
        let runtime_fingerprint = format!(
            "sha256:{}",
            crate::workspace_issuer::hex_lower(&sha2::Sha256::digest(runtime_public_key))
        );
        let verifier = WorkspaceCapabilityVerifier::new(
            vec![WorkspaceIssuerTrustRecord {
                workspace_id: "workspace-a".to_string(),
                backend_url: "https://backend.test".to_string(),
                key_id: "workspace-key".to_string(),
                algorithm: "ed25519".to_string(),
                public_key: workspace_identity.public_key.clone(),
                public_key_fingerprint: workspace_fingerprint,
                identity_revision: 1,
                trust_generation: 1,
                state: WorkspaceIssuerTrustState::Active,
                registered_at_unix: 1,
                updated_at_unix: 1,
            }],
            Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
        )
        .unwrap();
        let app = runtime_http_router_with_workspace_auth(
            runtime.clone(),
            None,
            WorkspaceRuntimeHttpAuth {
                verifier,
                signer: RuntimeVerificationSigner::from_identity(&runtime_identity).unwrap(),
                verifications: Arc::new(InMemoryWorkspaceRuntimeVerificationAuthority::default()),
            },
        );
        let challenge = WorkspaceRuntimeVerificationChallenge {
            challenge_id: "challenge-1".to_string(),
            workspace_id: "workspace-a".to_string(),
            runtime_id: "runtime-test".to_string(),
            binding_revision: 4,
            workspace_key_id: "workspace-key".to_string(),
            workspace_identity_revision: 1,
            workspace_trust_generation: 1,
            runtime_public_key_fingerprint: runtime_fingerprint,
            runtime_identity_revision: 1,
            workspace_nonce: "workspace-nonce".to_string(),
            expires_at: unix_now_i64() + 60,
        };
        let body = serde_json::to_vec(&challenge).unwrap();
        let claims = WorkspaceCapabilityClaims {
            issuer: "https://backend.test".to_string(),
            issuer_workspace_id: "workspace-a".to_string(),
            issuer_key_id: "workspace-key".to_string(),
            issuer_identity_revision: 1,
            trust_generation: 1,
            binding_revision: 4,
            runtime_id: "runtime-test".to_string(),
            worker_id: None,
            operation: WORKSPACE_VERIFICATION_OPERATION.to_string(),
            method: "POST".to_string(),
            path_and_query: runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE.to_string(),
            body_digest: workspace_request_body_digest(&body),
            iat: unix_now_i64(),
            exp: challenge.expires_at,
            jti: "challenge-token".to_string(),
        };
        let token =
            issue_workspace_capability_token(&workspace_identity.signing_key().unwrap(), &claims)
                .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(runtime_api::RUNTIME_ROUTE_VERIFICATION_CHALLENGE)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&response_body)
        );
        let verification_response =
            serde_json::from_slice::<WorkspaceRuntimeVerificationResponse>(&response_body).unwrap();
        verify_runtime_verification_response(
            &verification_response,
            &challenge,
            &runtime_identity.public_key,
            unix_now_i64(),
        )
        .unwrap();

        let acknowledgement = WorkspaceRuntimeVerificationAcknowledgement {
            challenge_id: verification_response.challenge_id.clone(),
            workspace_id: verification_response.workspace_id.clone(),
            runtime_id: verification_response.runtime_id.clone(),
            binding_revision: verification_response.binding_revision,
            workspace_key_id: verification_response.workspace_key_id.clone(),
            workspace_identity_revision: verification_response.workspace_identity_revision,
            workspace_trust_generation: verification_response.workspace_trust_generation,
            runtime_public_key_fingerprint: verification_response
                .runtime_public_key_fingerprint
                .clone(),
            runtime_identity_revision: verification_response.runtime_identity_revision,
            workspace_nonce: verification_response.workspace_nonce.clone(),
            runtime_nonce: verification_response.runtime_nonce.clone(),
            response_digest: workspace_request_body_digest(&response_body),
            response: verification_response.clone(),
            expires_at: verification_response.expires_at,
        };
        let acknowledgement_body = serde_json::to_vec(&acknowledgement).unwrap();
        let acknowledgement_claims = WorkspaceCapabilityClaims {
            path_and_query: runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK.to_string(),
            body_digest: workspace_request_body_digest(&acknowledgement_body),
            jti: "ack-token".to_string(),
            ..claims
        };
        let acknowledgement_token = issue_workspace_capability_token(
            &workspace_identity.signing_key().unwrap(),
            &acknowledgement_claims,
        )
        .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(runtime_api::RUNTIME_ROUTE_VERIFICATION_ACK)
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {acknowledgement_token}"),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(acknowledgement_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

        for authorization in [None, Some("Bearer malformed")] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri(runtime_api::RUNTIME_ROUTE_CONFIG_BUNDLES);
            if let Some(authorization) = authorization {
                request = request.header(header::AUTHORIZATION, authorization);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let ping_claims = WorkspaceCapabilityClaims {
            operation: RUNTIME_PING_PERMISSION.to_string(),
            method: "GET".to_string(),
            path_and_query: "/v1/ping".to_string(),
            body_digest: workspace_request_body_digest(&[]),
            jti: "ping-token".to_string(),
            ..acknowledgement_claims
        };
        let ping_token = issue_workspace_capability_token(
            &workspace_identity.signing_key().unwrap(),
            &ping_claims,
        )
        .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/ping")
                    .header(header::AUTHORIZATION, format!("Bearer {ping_token}"))
                    .header(RUNTIME_WORKSPACE_SCOPE_HEADER, "workspace-a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let mut retention_create = task_request("retention-scope");
        retention_create.workspace_api = Some(WorkspaceApiRef {
            workspace_id: "workspace-a".to_string(),
            base_url: "https://backend.test".to_string(),
        });
        let retention_worker_id = retention_create.worker_id.clone();
        runtime.create_worker(retention_create).unwrap();
        let retention_path = format!("/v1/workers/{retention_worker_id}/retention/execute");
        let retention_request = WorkerRetentionExecutionRequest {
            operation_id: "retention-op".to_string(),
            input_fingerprint: "fingerprint".to_string(),
            archive_id: None,
            workspace_id: "workspace-b".to_string(),
            source_runtime_id: "runtime-a".to_string(),
            worker_id: retention_worker_id,
            expected_worker_revision: "revision".to_string(),
            source_created_at: "2025-01-01T00:00:00Z".to_string(),
            removed_at: "2025-01-02T00:00:00Z".to_string(),
            effective_profile: None,
            retention_class: None,
            policy_id: "policy".to_string(),
            policy_revision: 1,
            session_disposition: SessionDisposition::Purge,
            diagnostics_disposition: DiagnosticsDisposition::Retain,
        };
        let retention_body = serde_json::to_vec(&retention_request).unwrap();
        let retention_claims = WorkspaceCapabilityClaims {
            operation: required_runtime_permission(&Method::POST, &retention_path)
                .unwrap()
                .to_string(),
            method: "POST".to_string(),
            path_and_query: retention_path.clone(),
            worker_id: Some(retention_worker_id.to_string()),
            body_digest: workspace_request_body_digest(&retention_body),
            jti: "retention-scope-token".to_string(),
            ..ping_claims
        };
        let retention_token = issue_workspace_capability_token(
            &workspace_identity.signing_key().unwrap(),
            &retention_claims,
        )
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(retention_path)
                    .header(header::AUTHORIZATION, format!("Bearer {retention_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(retention_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "worker_not_found");
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn runtime_protocol_replaces_serialized_tracked_source() {
        let wire = serde_json::to_string(&protocol::Method::SubmitTracked {
            submission_request_id: "request-1".into(),
            input: vec![protocol::Segment::text("hello")],
            source: protocol::AuthenticatedInputSource::Account {
                account_id: "forged".into(),
            },
        })
        .unwrap();
        let decoded: protocol::Method = serde_json::from_str(&wire).unwrap();
        assert!(matches!(
            decoded,
            protocol::Method::SubmitTracked {
                source: protocol::AuthenticatedInputSource::UntrustedWire,
                ..
            }
        ));
        assert!(matches!(
            authorize_runtime_protocol_method(decoded, None),
            protocol::Method::SubmitTracked {
                source: protocol::AuthenticatedInputSource::Backend { operation_id },
                ..
            } if operation_id == "request-1"
        ));
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn runtime_protocol_uses_transport_authenticated_account_source() {
        let mut headers = HeaderMap::new();
        headers.insert(
            protocol::AUTHENTICATED_ACCOUNT_ID_HEADER,
            "account-1".parse().unwrap(),
        );
        let source = authenticated_protocol_input_source(&headers)
            .unwrap()
            .expect("account source header must resolve");
        let wire = serde_json::to_string(&protocol::Method::NotifyTracked {
            notification_request_id: "notification-1".into(),
            message: "hello".into(),
            source: protocol::AuthenticatedInputSource::Account {
                account_id: "forged".into(),
            },
        })
        .unwrap();
        let decoded: protocol::Method = serde_json::from_str(&wire).unwrap();

        assert!(matches!(
            authorize_runtime_protocol_method(decoded, Some(&source)),
            protocol::Method::NotifyTracked {
                source: protocol::AuthenticatedInputSource::Account { account_id },
                ..
            } if account_id == "account-1"
        ));
    }

    #[test]
    fn attachment_routes_require_worker_input_permission() {
        assert_eq!(
            required_runtime_permission(&Method::POST, "/v1/workers/7/attachments"),
            Some("workers:input")
        );
        assert_eq!(
            required_runtime_permission(
                &Method::DELETE,
                "/v1/workers/7/attachments/019ca7c8-57b6-7f05-8edf-524147aba7b3"
            ),
            Some("workers:input")
        );
        assert_eq!(
            required_runtime_permission(
                &Method::GET,
                "/v1/workers/7/sessions/session-1/attachments/attachment-1"
            ),
            Some("workers:read")
        );
        assert_eq!(
            required_runtime_permission(&Method::POST, "/v1/workers/7/workdir-attachments"),
            Some("workers:input")
        );
    }

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
            prompt_catalog: None,
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn task_request(_objective: &str) -> CreateWorkerRequest {
        let profile = ProfileSelector::Builtin("builtin:coder".to_string());
        let bundle = test_bundle(profile.clone());
        CreateWorkerRequest {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "test-create".to_string(),
            profile,
            display_name: None,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Embedded {
                archive: crate::profile_archive::ProfileSourceArchive::build(
                    crate::profile_archive::ProfileSourceArchiveInput {
                        id: "test-profile-source".to_string(),
                        entrypoints: std::collections::BTreeMap::from([(
                            "builtin:coder".to_string(),
                            "profiles/coder.dcdl".to_string(),
                        )]),
                        imports: std::collections::BTreeMap::new(),
                        sources: std::collections::BTreeMap::from([(
                            "profiles/coder.dcdl".to_string(),
                            "{}".to_string(),
                        )]),
                    },
                )
                .unwrap(),
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            workdir_attachment_requests: Vec::new(),
            workdir_attachments: Vec::new(),
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: Some(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "local".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            }),
            subjektiv_attached: false,
        }
    }

    #[test]
    fn retention_routes_require_worker_delete_permission() {
        assert_eq!(
            required_runtime_permission(&Method::GET, "/v1/workers/worker-1/retention/inventory",),
            Some("workers:delete")
        );
        assert_eq!(
            required_runtime_permission(&Method::POST, "/v1/workers/worker-1/retention/execute",),
            Some("workers:delete")
        );
    }

    #[test]
    fn workdir_routes_require_dedicated_operation_permission() {
        assert_eq!(
            required_runtime_permission(
                &Method::POST,
                runtime_api::RUNTIME_ROUTE_WORKING_DIRECTORY_REPOSITORY_ACCESS,
            ),
            Some("workdirs:operate")
        );
        assert_eq!(
            required_runtime_permission(
                &Method::POST,
                runtime_api::RUNTIME_ROUTE_REPOSITORY_REFS_OBSERVE
            ),
            Some("workdirs:operate")
        );
        assert_eq!(
            required_runtime_permission(&Method::POST, runtime_api::RUNTIME_ROUTE_SSH_PROBE),
            Some(SSH_HOST_KEY_PROBE_OPERATION)
        );
        assert_eq!(
            required_runtime_permission(&Method::POST, "/v1/working-directories/wd-1/sessions"),
            Some("workdirs:operate")
        );
        assert_eq!(
            required_runtime_permission(&Method::POST, "/v1/workdir-sessions/session-1/operations"),
            Some("workdirs:operate")
        );
        assert_eq!(
            required_runtime_permission(&Method::DELETE, "/v1/workdir-sessions/session-1"),
            Some("workdirs:operate")
        );
    }

    #[tokio::test]
    async fn workdir_session_operations_enforce_owner_and_close_terminally() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("hello.txt"), "hello").expect("write fixture");
        let scope = SharedScope::new(Scope::writable(temp.path()).expect("scope"));
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
            Workdir::new("wd-1"),
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            scope,
            WorkdirSessionCapabilities::ALL,
        ));
        let owner = RuntimeWorkspaceScope::new("workspace-a", "server-a");
        let state = RuntimeHttpState {
            runtime: Runtime::with_execution_backend(
                RuntimeOptions::default(),
                Arc::new(AcceptingBackend),
            )
            .expect("runtime"),
            local_token: Some(Arc::from("token")),
            workspace_auth: None,
            workdir_sessions: Arc::new(Mutex::new(HashMap::from([(
                "session-1".to_string(),
                RuntimeHttpWorkdirSession {
                    owner: owner.clone(),
                    session: session.clone(),
                },
            )]))),
            ssh_keyscan_program: Arc::new(PathBuf::from("ssh-keyscan")),
        };
        let auth = RuntimeAuthContext {
            server_id: "server-a".to_string(),
            workspace_id: "workspace-a".to_string(),
            permissions: vec!["workdirs:operate".to_string()],
            token_id: "token-a".to_string(),
            expires_at: u64::MAX,
        };
        let operation = WorkdirSessionOperationRequest {
            operation: WorkdirSessionOperation::Stat(StatRequest {
                path: WorkdirPath::new("hello.txt").expect("logical path"),
            }),
        };

        let Json(result) = run_workdir_session_operation(
            State(state.clone()),
            Path("session-1".to_string()),
            Some(Extension(auth.clone())),
            Ok(Json(operation.clone())),
        )
        .await
        .expect("owned operation");
        assert!(matches!(result, WorkdirSessionOperationResult::Stat(_)));

        let authorization = WorkdirSessionOperationRequest {
            operation: WorkdirSessionOperation::AuthorizeScope(
                workdir::WorkdirScopeAuthorizationRequest {
                    rules: vec![workdir::WorkdirToolScopeRule {
                        target: WorkdirPath::new("hello.txt").unwrap(),
                        permission: workdir::WorkdirToolScopePermission::Read,
                        recursive: false,
                        symlink_policy: Default::default(),
                    }],
                    path: WorkdirPath::new("hello.txt").unwrap(),
                    permission: workdir::WorkdirToolScopePermission::Read,
                },
            ),
        };
        let Json(result) = run_workdir_session_operation(
            State(state.clone()),
            Path("session-1".to_string()),
            Some(Extension(auth.clone())),
            Ok(Json(authorization)),
        )
        .await
        .expect("provider-side scope authorization");
        assert!(matches!(
            result,
            WorkdirSessionOperationResult::AuthorizeScope
        ));

        let overlap_rule = workdir::WorkdirToolScopeRule {
            target: WorkdirPath::new("hello.txt").unwrap(),
            permission: workdir::WorkdirToolScopePermission::Write,
            recursive: false,
            symlink_policy: Default::default(),
        };
        let overlap = WorkdirSessionOperationRequest {
            operation: WorkdirSessionOperation::ScopeRulesOverlap(
                workdir::WorkdirScopeOverlapRequest {
                    left: overlap_rule.clone(),
                    right: overlap_rule,
                },
            ),
        };
        let Json(result) = run_workdir_session_operation(
            State(state.clone()),
            Path("session-1".to_string()),
            Some(Extension(auth.clone())),
            Ok(Json(overlap)),
        )
        .await
        .expect("provider-side resolved overlap check");
        assert!(matches!(
            result,
            WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps: true }
        ));

        let grep = WorkdirSessionOperationRequest {
            operation: WorkdirSessionOperation::Grep(GrepRequest {
                pattern: "hello".into(),
                path: WorkdirPath::new("hello.txt").unwrap(),
                glob: Some("*.txt".into()),
                file_type: Some("txt".into()),
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: GrepOutputMode::Content,
                limit: 10,
                offset: 0,
            }),
        };
        let Json(result) = run_workdir_session_operation(
            State(state.clone()),
            Path("session-1".to_string()),
            Some(Extension(auth.clone())),
            Ok(Json(grep)),
        )
        .await
        .expect("grep direct file through provider operation");
        assert!(matches!(result, WorkdirSessionOperationResult::Grep(_)));

        let wrong_owner = RuntimeAuthContext {
            workspace_id: "workspace-b".to_string(),
            ..auth.clone()
        };
        let error = run_workdir_session_operation(
            State(state.clone()),
            Path("session-1".to_string()),
            Some(Extension(wrong_owner)),
            Ok(Json(operation)),
        )
        .await
        .expect_err("cross-workspace session access must fail");
        assert_eq!(error.status, StatusCode::NOT_FOUND);

        assert_eq!(
            close_workdir_session(
                State(state.clone()),
                Path("session-1".to_string()),
                Some(Extension(auth)),
            )
            .await
            .expect("close"),
            StatusCode::NO_CONTENT
        );
        let closed_error = session
            .stat(StatRequest {
                path: WorkdirPath::new("hello.txt").expect("logical path"),
            })
            .await
            .expect_err("close must be terminal");
        assert!(matches!(
            closed_error,
            workdir::WorkdirError::Unavailable(_)
        ));
        assert!(
            state
                .workdir_sessions
                .lock()
                .expect("session registry")
                .is_empty()
        );
    }

    struct AcceptingBackend;

    impl WorkerExecutionBackend for AcceptingBackend {
        fn backend_id(&self) -> &str {
            "http-test"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::connected(
                protocol::WorkerStatus::Idle.into(),
                request
                    .workdir_attachments
                    .iter()
                    .map(
                        |(alias, binding)| crate::catalog::WorkingDirectoryAttachmentStatus {
                            alias: alias.clone(),
                            working_directory: binding.status(),
                        },
                    )
                    .collect(),
            )
        }

        fn restore_worker(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> WorkerExecutionSpawnResult {
            WorkerExecutionSpawnResult::connected(
                protocol::WorkerStatus::Idle.into(),
                request.previous_workdir_attachments,
            )
        }

        fn dispatch_input(
            &self,
            _worker_ref: &WorkerRef,
            input: WorkerInput,
        ) -> WorkerExecutionResult {
            if let Some(submission_id) = input.submission_request_id {
                WorkerExecutionResult::accepted_submission(
                    WorkerExecutionOperation::Input,
                    submission_id.clone(),
                    submission_id,
                    protocol::SubmissionDisposition::Started,
                )
            } else {
                WorkerExecutionResult::accepted(WorkerExecutionOperation::Input)
            }
        }

        fn stop_worker(&self, _worker_ref: &WorkerRef) -> WorkerExecutionResult {
            WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop)
        }
    }

    #[cfg(feature = "fs-store")]
    #[derive(Default)]
    struct RejectingRestoreBackend {
        restores: Mutex<Vec<WorkerRef>>,
        inputs: Mutex<Vec<WorkerRef>>,
        stops: Mutex<Vec<WorkerRef>>,
    }

    #[cfg(feature = "fs-store")]
    impl WorkerExecutionBackend for RejectingRestoreBackend {
        fn backend_id(&self) -> &str {
            "http-test"
        }

        fn spawn_worker(
            &self,
            _request: WorkerExecutionSpawnRequest,
        ) -> WorkerExecutionSpawnResult {
            panic!("a saved Worker must be restored, not spawned again");
        }

        fn restore_worker(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> WorkerExecutionSpawnResult {
            self.restores.lock().unwrap().push(request.worker_ref);
            WorkerExecutionSpawnResult::Rejected(WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Restore,
                "repository access must be reacquired",
            ))
        }

        fn dispatch_input(
            &self,
            worker_ref: &WorkerRef,
            _input: WorkerInput,
        ) -> WorkerExecutionResult {
            self.inputs.lock().unwrap().push(worker_ref.clone());
            WorkerExecutionResult::rejected(
                WorkerExecutionOperation::Input,
                "Worker execution is unavailable after restore rejection",
            )
        }

        fn stop_worker(&self, worker_ref: &WorkerRef) -> WorkerExecutionResult {
            self.stops.lock().unwrap().push(worker_ref.clone());
            WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop)
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
    async fn rest_feature_completions_echo_each_nonce_and_qualified_context() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(AcceptingBackend))
                .unwrap();
        runtime
            .store_config_bundle(test_bundle(ProfileSelector::Builtin(
                "builtin:coder".into(),
            )))
            .unwrap();
        let token = "completion-token";
        let app = runtime_http_router(runtime, token.into());
        let created = authed_json_request(
            app.clone(),
            Method::POST,
            "/v1/workers",
            token,
            &task_request("completion"),
        )
        .await;
        assert_eq!(created.status(), StatusCode::OK);
        let created: RuntimeHttpWorkerResponse = read_json(created).await;
        let context = protocol::CompletionContext {
            invocation: protocol::FeatureInvocationIdentity("builtin:test/prepare".into()),
            argument: Some("path".into()),
        };
        for nonce in ["first", "second", "first"] {
            let request = runtime_api::CompletionRequest {
                kind: protocol::CompletionKind::FeatureArgument,
                prefix: "資料/".into(),
                context: Some(context.clone()),
                request_id: Some(nonce.into()),
            };
            let response = authed_json_request(
                app.clone(),
                Method::POST,
                &format!("/v1/workers/{}/completions", created.worker.worker_id),
                token,
                &request,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let response: runtime_api::CompletionResponse = read_json(response).await;
            assert_eq!(response.kind, request.kind);
            assert_eq!(response.prefix, request.prefix);
            assert_eq!(response.context, request.context);
            assert_eq!(response.request_id, request.request_id);
        }
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

        let malformed_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/workers")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(malformed_response.status(), StatusCode::BAD_REQUEST);
        let malformed: RuntimeHttpErrorResponse = read_json(malformed_response).await;
        assert_eq!(malformed.error.code, "invalid_json");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_app = app.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, server_app).await.unwrap();
        });
        let client = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .authorizer(TestRuntimeAuthorizer(token.to_string()))
            .build()
            .unwrap();
        let generated_request: runtime_api::CreateWorkerRequest = serde_json::from_value(
            serde_json::to_value(task_request("generated-roundtrip")).unwrap(),
        )
        .unwrap();
        let generated_worker_id = generated_request.worker_id.to_string();
        let mut conflicting_request = generated_request.clone();
        conflicting_request.create_fingerprint = "different-fingerprint".to_string();
        let generated = client.create_worker(generated_request).await.unwrap();
        assert_eq!(generated.worker.worker_id.to_string(), generated_worker_id);
        for kind in [
            crate::interaction::WorkerInputKind::User,
            crate::interaction::WorkerInputKind::UserIfIdle,
        ] {
            let mut input = WorkerInput::user("generated client input");
            input.kind = kind.clone();
            input.submission_request_id = Some(format!("generated-{kind:?}"));
            // The remote adapter performs this same conversion before sending.
            // Both the generated API contract and the HTTP receiver must retain
            // Idle-only admission rather than silently downgrading it to User.
            let generated_input: runtime_api::WorkerInput =
                serde_json::from_value(serde_json::to_value(&input).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(&generated_input).unwrap(),
                serde_json::to_value(&input).unwrap()
            );
            let response = client
                .send_worker_input(generated_worker_id.clone(), generated_input)
                .await
                .unwrap();
            assert_eq!(
                Some(response.ack.submission.unwrap().submission_request_id),
                input.submission_request_id
            );
        }
        client
            .stop_worker(
                generated_worker_id.clone(),
                runtime_api::WorkerLifecycleRequest::default(),
            )
            .await
            .unwrap();
        client
            .restore_worker(
                generated_worker_id.clone(),
                runtime_api::WorkerRestoreRequest {
                    expected_observation_token: client
                        .get_worker(generated_worker_id.clone())
                        .await
                        .unwrap()
                        .worker
                        .restore_observation_token
                        .unwrap(),
                    request_id: "generated-restore".into(),
                    preparation: Some(runtime_api::WorkerRestorePreparation::default()),
                },
            )
            .await
            .unwrap();
        let conflict = client.create_worker(conflicting_request).await.unwrap_err();
        assert!(
            matches!(
                &conflict,
                runtime_api::client_support::ClientError::Public { status, .. }
                    if *status == StatusCode::CONFLICT
            ),
            "{conflict:?}"
        );
        client
            .stop_worker(
                generated_worker_id.clone(),
                runtime_api::WorkerLifecycleRequest::default(),
            )
            .await
            .unwrap();
        client
            .delete_worker(generated_worker_id.clone())
            .await
            .unwrap();
        let missing = client
            .get_worker(generated_worker_id.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            missing,
            runtime_api::client_support::ClientError::Public { status, .. }
                if status == StatusCode::NOT_FOUND
        ));
        let unauthorized = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .build()
            .unwrap()
            .list_workers(runtime_api::WorkerListQuery::default())
            .await
            .unwrap_err();
        assert!(matches!(
            unauthorized,
            runtime_api::client_support::ClientError::Public { status, .. }
                if status == StatusCode::UNAUTHORIZED
        ));
        let forbidden_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/ping")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);
        let bounded_client = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .authorizer(TestRuntimeAuthorizer(token.to_string()))
            .response_body_limit(1)
            .build()
            .unwrap();
        assert!(matches!(
            bounded_client.runtime_summary().await.unwrap_err(),
            runtime_api::client_support::ClientError::Failure(
                runtime_api::client_support::ClientFailure::ResponseTooLarge { .. }
            )
        ));
        server.abort();

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

        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/workspace-api", created.worker.worker_id),
            token,
            &RuntimeHttpWorkerWorkspaceApiRequest {
                workspace_api: WorkspaceApiRef {
                    workspace_id: "local".to_string(),
                    base_url: "http://127.0.0.1:8787".to_string(),
                },
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

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

        let restore_uri = format!("/v1/workers/{}/restore", created.worker.worker_id);
        let missing = authed_empty_request(app.clone(), Method::POST, &restore_uri, token).await;
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        let stale = runtime_api::WorkerRestoreRequest {
            expected_observation_token: created.worker.restore_observation_token.clone().unwrap(),
            request_id: "stale-http-restore".into(),
            preparation: None,
        };
        let response =
            authed_json_request(app.clone(), Method::POST, &restore_uri, token, &stale).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let conflict: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(conflict.error.code, "restore_observation_conflict");

        let mut restore_request = runtime.test_restore_request(&created.worker.worker_ref);
        let coordinate_uri = format!("{restore_uri}/coordinate");
        let mut coordination = runtime_api::WorkerRestoreCoordinationRequest {
            expected_observation_token: restore_request.expected_observation_token.clone(),
            request_id: restore_request.request_id.clone(),
            preparation: None,
        };
        let before_lookup = runtime.worker_detail(&created.worker.worker_ref).unwrap();
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &coordinate_uri,
            token,
            &coordination,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let missing_owner: runtime_api::WorkerRestoreCoordinationResponse =
            read_json(response).await;
        assert!(missing_owner.result.is_none() && missing_owner.preparation.is_none());
        assert_eq!(
            runtime.worker_detail(&created.worker.worker_ref).unwrap(),
            before_lookup
        );
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &coordinate_uri,
            "wrong-token",
            &coordination,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        coordination.preparation = Some(runtime_api::WorkerRestorePreparation::default());
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &coordinate_uri,
            token,
            &coordination,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let admitted: runtime_api::WorkerRestoreCoordinationResponse = read_json(response).await;
        assert!(admitted.result.is_none() && admitted.preparation.is_some());
        assert_eq!(
            runtime
                .worker_detail(&created.worker.worker_ref)
                .unwrap()
                .status,
            crate::catalog::WorkerStatus::Stopped
        );
        restore_request.preparation = admitted.preparation;
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &restore_uri,
            token,
            &restore_request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let restored: runtime_api::WorkerRestoreResponse = read_json(response).await;
        assert_eq!(restored.state, WorkerRestoreState::Accepted);
        assert_eq!(
            restored.worker.as_ref().unwrap().status,
            runtime_api::WorkerStatus::Idle
        );

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("/v1/workers/{}/stop", created.worker.worker_id),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        coordination.preparation = None;
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &coordinate_uri,
            token,
            &coordination,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let recovered: runtime_api::WorkerRestoreCoordinationResponse = read_json(response).await;
        assert!(recovered.preparation.is_none());
        assert_eq!(recovered.result, Some(restored.clone()));

        restore_request.preparation = Some(serde_json::from_value(serde_json::json!({
            "workspace_api": { "workspace_id": "must-not-bind", "base_url": "https://fresh.invalid.example" },
            "workdir_attachments": [],
            "repository_access": []
        })).unwrap());
        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &restore_uri,
            token,
            &restore_request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let replayed: runtime_api::WorkerRestoreResponse = read_json(response).await;
        assert_eq!(
            serde_json::to_value(replayed).unwrap(),
            serde_json::to_value(restored).unwrap()
        );
        assert_eq!(
            runtime
                .worker_detail(&created.worker.worker_ref)
                .unwrap()
                .status,
            crate::catalog::WorkerStatus::Stopped
        );

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

    #[cfg(feature = "fs-store")]
    #[tokio::test]
    async fn rest_saved_active_worker_restore_rejection_is_observable_and_can_be_stopped_removed() {
        let temp = tempfile::tempdir().unwrap();
        let options = crate::fs_store::FsRuntimeStoreOptions::new(temp.path().join("runtime"));
        let runtime = Runtime::with_fs_store_and_execution_backend(
            options.clone(),
            Arc::new(AcceptingBackend),
        )
        .unwrap();
        runtime
            .store_config_bundle(test_bundle(ProfileSelector::Builtin(
                "builtin:coder".to_string(),
            )))
            .unwrap();
        let mut request = task_request("saved active Worker");
        request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: "local".to_string(),
            base_url: "http://127.0.0.1:8787".to_string(),
        });
        let created = runtime
            .create_worker_scoped(&RuntimeWorkspaceScope::new("local", "local-token"), request)
            .unwrap();
        assert_eq!(created.status, crate::catalog::WorkerStatus::Idle);
        assert!(created.worker_state.is_some());
        drop(runtime);

        let backend = Arc::new(RejectingRestoreBackend::default());
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
        // Startup attempted automatic restore without turning rejection into a
        // fabricated live Worker or silently changing its saved lifecycle status.
        assert_eq!(
            *backend.restores.lock().unwrap(),
            vec![created.worker_ref.clone()],
            "startup diagnostics: {:?}; Worker: {:?}",
            runtime.diagnostics().unwrap(),
            runtime.worker_detail(&created.worker_ref)
        );
        let token = "local-token";
        let app = runtime_http_router(runtime.clone(), token.to_string());
        let worker_uri = format!("/v1/workers/{}", created.worker_id);

        let response = authed_empty_request(app.clone(), Method::GET, &worker_uri, token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let detail: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(detail.worker.worker_ref, created.worker_ref);
        assert_eq!(detail.worker.status, crate::catalog::WorkerStatus::Idle);
        assert!(detail.worker.execution_metadata_available);
        assert!(detail.worker.worker_state.is_none());

        let response = authed_empty_request(app.clone(), Method::GET, "/v1/workers", token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed: RuntimeHttpWorkersResponse = read_json(response).await;
        assert_eq!(listed.workers.len(), 1);
        assert_eq!(listed.workers[0].worker_ref, created.worker_ref);
        assert_eq!(listed.workers[0].status, crate::catalog::WorkerStatus::Idle);
        assert!(listed.workers[0].worker_state.is_none());

        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &format!("{worker_uri}/restore"),
            token,
            &runtime.test_restore_request(&created.worker_ref),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let rejected: runtime_api::WorkerRestoreResponse = read_json(response).await;
        assert_eq!(rejected.state, WorkerRestoreState::Rejected);
        assert_eq!(
            rejected.reason_code.as_deref(),
            Some("worker_restore_rejected")
        );
        assert!(rejected.worker.is_none());
        assert_eq!(
            *backend.restores.lock().unwrap(),
            vec![created.worker_ref.clone(), created.worker_ref.clone()]
        );

        let response = authed_json_request(
            app.clone(),
            Method::POST,
            &format!("{worker_uri}/input"),
            token,
            &WorkerInput::user("must not be accepted by a fabricated execution"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "worker_execution_rejected");
        assert_eq!(
            *backend.inputs.lock().unwrap(),
            vec![created.worker_ref.clone()]
        );

        #[cfg(feature = "ws-server")]
        {
            use tokio_tungstenite::tungstenite::Message;
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;

            assert!(matches!(
                runtime.worker_observation_snapshot(&created.worker_ref),
                Err(RuntimeError::WorkerExecutionUnavailable { worker_id, .. })
                    if worker_id == created.worker_id
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server_app = app.clone();
            let server = tokio::spawn(async move {
                axum::serve(listener, server_app).await.unwrap();
            });
            let mut request = format!("ws://{address}{worker_uri}/protocol/ws")
                .into_client_request()
                .unwrap();
            request.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
            let (mut stream, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            let frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .expect("unavailable snapshot must produce a protocol error promptly")
                .unwrap()
                .unwrap();
            let Message::Text(text) = frame else {
                panic!("expected protocol error frame, not a fabricated snapshot");
            };
            let event: protocol::Event = serde_json::from_str(&text).unwrap();
            assert!(matches!(event, protocol::Event::Error { .. }));
            drop(stream);
            server.abort();
            let _ = server.await;
        }

        // Saved active status still requires a normal stop before removal,
        // even though automatic restore never connected a live execution.
        let response = authed_empty_request(app.clone(), Method::DELETE, &worker_uri, token).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "invalid_request");
        assert!(backend.stops.lock().unwrap().is_empty());

        let response = authed_empty_request(
            app.clone(),
            Method::POST,
            &format!("{worker_uri}/stop"),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let stopped: RuntimeHttpWorkerLifecycleResponse = read_json(response).await;
        assert_eq!(stopped.ack.worker_ref, created.worker_ref);
        assert_eq!(stopped.ack.status, crate::catalog::WorkerStatus::Stopped);
        assert_eq!(
            *backend.stops.lock().unwrap(),
            vec![created.worker_ref.clone()]
        );

        let response = authed_empty_request(app.clone(), Method::GET, &worker_uri, token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let detail: RuntimeHttpWorkerResponse = read_json(response).await;
        assert_eq!(detail.worker.status, crate::catalog::WorkerStatus::Stopped);

        let response = authed_empty_request(app.clone(), Method::DELETE, &worker_uri, token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let removed: RuntimeHttpWorkerDeleteResponse = read_json(response).await;
        assert_eq!(removed.worker.worker_id, created.worker_id);
        assert!(removed.worker.deleted);
        // Removal asks the backend to reconfirm shutdown even for a saved
        // Stopped Worker; it must use the same WorkerRef as the original stop.
        assert_eq!(
            *backend.stops.lock().unwrap(),
            vec![created.worker_ref.clone(), created.worker_ref.clone()]
        );
        assert!(
            !temp
                .path()
                .join("runtime/workers")
                .join(created.worker_id.to_string())
                .exists()
        );

        let response = authed_empty_request(app, Method::GET, &worker_uri, token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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

    #[test]
    fn ssh_probe_uses_workdir_operation_capability() {
        assert_eq!(
            required_runtime_permission(&Method::POST, runtime_api::RUNTIME_ROUTE_SSH_PROBE),
            Some(SSH_HOST_KEY_PROBE_OPERATION)
        );
        assert_eq!(
            workspace_runtime_operation(&Method::POST, runtime_api::RUNTIME_ROUTE_SSH_PROBE),
            SSH_HOST_KEY_PROBE_OPERATION
        );
    }

    #[tokio::test]
    async fn ssh_host_key_probe_rejects_invalid_hostname_before_execution() {
        let token = "local-token";
        let app = runtime_http_router_with_auth_and_ssh_keyscan_program(
            Runtime::new_memory(),
            Some(token.to_string()),
            None,
            PathBuf::from("/definitely/missing/ssh-keyscan"),
        );
        let response = authed_json_request(
            app,
            Method::POST,
            runtime_api::RUNTIME_ROUTE_SSH_PROBE,
            token,
            &SshHostKeyProbeRequest {
                hostname: "-oProxyCommand=malicious".to_string(),
                port: 22,
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "ssh_host_key_probe_invalid_request");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn authenticated_ssh_host_key_probe_returns_deduplicated_candidates() {
        use base64::Engine as _;
        use std::os::unix::fs::PermissionsExt as _;

        let mut blob = Vec::new();
        blob.extend_from_slice(&11_u32.to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32_u32.to_be_bytes());
        blob.extend_from_slice(&[9_u8; 32]);
        let key = base64::engine::general_purpose::STANDARD.encode(blob);
        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("ssh-keyscan");
        let recorded_arguments = temp.path().join("arguments");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'private diagnostic' >&2\nprintf '%s\\n' 'example.test ssh-ed25519 {key}' '[example.test]:2222 ssh-ed25519 {key}'\n",
                recorded_arguments.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();

        let token = "local-token";
        let app = runtime_http_router_with_auth_and_ssh_keyscan_program(
            Runtime::new_memory(),
            Some(token.to_string()),
            None,
            program,
        );
        let body = SshHostKeyProbeRequest {
            hostname: "example.test".to_string(),
            port: 2222,
        };

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/repositories/ssh/probe")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let response = authed_json_request(
            app,
            Method::POST,
            runtime_api::RUNTIME_ROUTE_SSH_PROBE,
            token,
            &body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            std::fs::read_to_string(recorded_arguments).unwrap(),
            "-T\n5\n-p\n2222\n-t\ned25519\nexample.test\n"
        );
        let response: SshHostKeyProbeResponse = read_json(response).await;
        assert_eq!(response.candidates.len(), 1);
        assert_eq!(response.candidates[0].algorithm, "ssh-ed25519");
        assert_eq!(
            response.candidates[0].public_key,
            format!("ssh-ed25519 {key}")
        );
        assert!(response.candidates[0].fingerprint.starts_with("SHA256:"));
    }

    #[tokio::test]
    async fn runtime_errors_use_typed_rest_error_shape() {
        let token = "local-token";
        let app = runtime_http_router(Runtime::new_memory(), token.to_string());
        let missing = crate::identity::WorkerId::from_legacy_u64(999);
        let response =
            authed_empty_request(app, Method::GET, &format!("/v1/workers/{missing}"), token).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: RuntimeHttpErrorResponse = read_json(response).await;
        assert_eq!(error.error.code, "worker_not_found");
        assert!(error.error.message.contains(&missing.to_string()));
    }

    #[tokio::test]
    async fn serve_runtime_http_rejects_missing_auth_configuration() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = serve_runtime_http(Runtime::new_memory(), listener, None)
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeHttpServerError::AuthRequired));
    }

    #[test]
    fn worker_delete_persistence_error_is_bounded_and_typed() {
        let worker_id = crate::identity::WorkerId::now_v7();
        let error = RuntimeError::WorkerDeletePersistenceFailed {
            worker_id,
            message: "Worker metadata deletion failed; the persisted Worker identity was retained for retry".to_string(),
        };

        assert_eq!(
            status_for_runtime_error(&error),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            code_for_runtime_error(&error),
            "worker_delete_persistence_failed"
        );
        assert!(!error.to_string().contains('/'));
    }

    #[test]
    fn workdir_runtime_errors_preserve_diagnostic_code() {
        let cases = [
            ("working_directory_not_found", StatusCode::NOT_FOUND),
            (
                "repository_ref_provider_timeout",
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            ("repository_ref_provider_auth_failed", StatusCode::FORBIDDEN),
            ("repository_ref_not_found", StatusCode::NOT_FOUND),
        ];
        for (code, expected_status) in cases {
            let error = RuntimeError::WorkingDirectory(
                crate::working_directory::WorkingDirectoryDiagnostic {
                    code: code.to_string(),
                    message: "bounded diagnostic".to_string(),
                },
            );
            assert_eq!(status_for_runtime_error(&error), expected_status);
            assert_eq!(code_for_runtime_error(&error), code);
        }
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
        WorkerExecutionBackend, WorkerExecutionContext, WorkerExecutionOperation,
        WorkerExecutionRestoreRequest, WorkerExecutionResult, WorkerExecutionSpawnRequest,
        WorkerExecutionSpawnResult, WorkerProtocolTransport,
    };
    use crate::management::RuntimeOptions;
    use futures::{SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header as ws_header;

    struct WsController {
        commands: tokio::sync::mpsc::UnboundedReceiver<protocol::Method>,
        connections: Vec<tokio::sync::mpsc::Sender<protocol::Event>>,
    }

    struct WsExecution {
        snapshot: protocol::Event,
        context: WorkerExecutionContext,
        methods: tokio::sync::mpsc::UnboundedSender<protocol::Method>,
        controller: Mutex<WsController>,
    }

    impl WsExecution {
        fn validate(&self) -> Result<(), WorkerExecutionResult> {
            if self.methods.is_closed() {
                Err(WorkerExecutionResult::rejected(
                    WorkerExecutionOperation::ProtocolMethod,
                    "mock Controller method channel is closed",
                ))
            } else {
                Ok(())
            }
        }

        fn dispatch(
            &self,
            method: protocol::Method,
        ) -> Result<Vec<protocol::Event>, WorkerExecutionResult> {
            let mut controller = self.controller.lock().unwrap();
            self.validate()?;
            match method {
                protocol::Method::ListCompletions {
                    kind,
                    prefix,
                    request_id,
                    context,
                } => Ok(vec![protocol::Event::Completions {
                    kind,
                    prefix,
                    request_id,
                    context,
                    entries: Vec::new(),
                }]),
                protocol::Method::Shutdown { .. } => {
                    controller.commands.close();
                    controller.connections.clear();
                    Ok(Vec::new())
                }
                method => {
                    self.methods.send(method).map_err(|error| {
                        WorkerExecutionResult::rejected(
                            WorkerExecutionOperation::ProtocolMethod,
                            error.to_string(),
                        )
                    })?;
                    Ok(Vec::new())
                }
            }
        }

        fn close(&self) {
            let mut controller = self.controller.lock().unwrap();
            controller.commands.close();
            controller.connections.clear();
        }
    }

    #[derive(Default)]
    struct WsBackend {
        executions: Mutex<HashMap<WorkerRef, Arc<WsExecution>>>,
    }

    impl WsBackend {
        fn insert_execution(&self, worker_ref: WorkerRef, context: WorkerExecutionContext) {
            let (methods, commands) = tokio::sync::mpsc::unbounded_channel();
            let execution = Arc::new(WsExecution {
                snapshot: ws_snapshot(&worker_ref),
                context,
                methods,
                controller: Mutex::new(WsController {
                    commands,
                    connections: Vec::new(),
                }),
            });
            if let Some(previous) = self
                .executions
                .lock()
                .unwrap()
                .insert(worker_ref, execution)
            {
                previous.close();
            }
        }

        fn publish(
            &self,
            worker_ref: &WorkerRef,
            event: protocol::Event,
        ) -> crate::observation::WorkerObservationEvent {
            let execution = self
                .executions
                .lock()
                .unwrap()
                .get(worker_ref)
                .unwrap()
                .clone();
            let observation = execution
                .context
                .publish_protocol_event(event.clone())
                .unwrap();
            // The fixture publishes separately to Runtime history and to this
            // Controller's attached streams, just like the backend event relay.
            execution
                .controller
                .lock()
                .unwrap()
                .connections
                .retain(|sender| match sender.try_send(event.clone()) {
                    Ok(()) => true,
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        panic!("mock protocol event receiver lagged")
                    }
                });
            observation
        }
    }

    impl WorkerExecutionBackend for WsBackend {
        fn backend_id(&self) -> &str {
            "ws-test"
        }

        fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            self.insert_execution(request.worker_ref, request.context);
            WorkerExecutionSpawnResult::connected(
                protocol::WorkerStatus::Idle.into(),
                request
                    .workdir_attachments
                    .iter()
                    .map(
                        |(alias, binding)| crate::catalog::WorkingDirectoryAttachmentStatus {
                            alias: alias.clone(),
                            working_directory: binding.status(),
                        },
                    )
                    .collect(),
            )
        }

        fn dispatch_input(
            &self,
            _worker_ref: &WorkerRef,
            input: WorkerInput,
        ) -> WorkerExecutionResult {
            if let Some(submission_id) = input.submission_request_id {
                WorkerExecutionResult::accepted_submission(
                    WorkerExecutionOperation::Input,
                    submission_id.clone(),
                    submission_id,
                    protocol::SubmissionDisposition::Started,
                )
            } else {
                WorkerExecutionResult::accepted(WorkerExecutionOperation::Input)
            }
        }

        fn worker_snapshot(&self, worker_ref: &WorkerRef) -> Option<protocol::Event> {
            self.executions
                .lock()
                .unwrap()
                .get(worker_ref)
                .map(|execution| execution.snapshot.clone())
        }

        fn restore_worker(
            &self,
            request: WorkerExecutionRestoreRequest,
        ) -> WorkerExecutionSpawnResult {
            self.insert_execution(request.worker_ref, request.context);
            WorkerExecutionSpawnResult::connected(
                protocol::WorkerStatus::Idle.into(),
                request.previous_workdir_attachments,
            )
        }

        fn stop_worker(&self, worker_ref: &WorkerRef) -> WorkerExecutionResult {
            if let Some(execution) = self.executions.lock().unwrap().remove(worker_ref) {
                execution.close();
            }
            WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop)
        }

        fn attach_worker_protocol(
            self: Arc<Self>,
            worker_ref: &WorkerRef,
        ) -> Result<WorkerProtocolTransport, WorkerExecutionResult> {
            let execution = self
                .executions
                .lock()
                .unwrap()
                .get(worker_ref)
                .cloned()
                .ok_or_else(|| {
                    WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::ProtocolMethod,
                        "mock Controller is unavailable",
                    )
                })?;
            let (sender, events) = tokio::sync::mpsc::channel(32);
            {
                let mut controller = execution.controller.lock().unwrap();
                execution.validate()?;
                controller.connections.push(sender);
            }
            let dispatcher = execution.clone();
            let validator = execution.clone();
            Ok(WorkerProtocolTransport::new(
                worker_ref.clone(),
                execution.snapshot.clone(),
                events,
                Arc::new(move |method| dispatcher.dispatch(method)),
                Arc::new(move || validator.validate()),
            ))
        }
    }

    fn ws_snapshot(worker_ref: &WorkerRef) -> protocol::Event {
        protocol::Event::Snapshot {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            greeting: protocol::Greeting {
                worker_name: worker_ref.worker_id.to_string(),
                cwd: String::new(),
                provider: "ws-test".to_string(),
                model: "ws-test".to_string(),
                scope_summary: "WebSocket test execution snapshot".to_string(),
                tools: Vec::new(),
                context_window: 0,
                context_tokens: 0,
                reasoning: None,
                context_usage: None,
            },
            state: protocol::WorkerStateSnapshot::initial(),
            in_flight: protocol::InFlightSnapshot {
                blocks: Vec::new(),
                commands: Vec::new(),
                compaction: None,
            },
            internal_workers: Vec::new(),
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
            prompt_catalog: None,
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn ws_create_request() -> CreateWorkerRequest {
        let bundle = ws_test_bundle(ProfileSelector::Builtin("builtin:companion".to_string()));
        CreateWorkerRequest {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "test-create".to_string(),
            profile: ProfileSelector::Builtin("builtin:companion".to_string()),
            display_name: None,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Embedded {
                archive: crate::profile_archive::ProfileSourceArchive::build(
                    crate::profile_archive::ProfileSourceArchiveInput {
                        id: "test-profile-source".to_string(),
                        entrypoints: std::collections::BTreeMap::from([(
                            "builtin:coder".to_string(),
                            "profiles/coder.dcdl".to_string(),
                        )]),
                        imports: std::collections::BTreeMap::new(),
                        sources: std::collections::BTreeMap::from([(
                            "profiles/coder.dcdl".to_string(),
                            "{}".to_string(),
                        )]),
                    },
                )
                .unwrap(),
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            workdir_attachment_requests: Vec::new(),
            workdir_attachments: Vec::new(),
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: Some(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "local".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            }),
            subjektiv_attached: false,
        }
    }

    async fn spawn_runtime_server() -> (Runtime, Arc<WsBackend>, WorkerRef, String) {
        let backend = Arc::new(WsBackend::default());
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), backend.clone()).unwrap();
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
            backend,
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
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("Worker protocol frame must arrive promptly")
            .unwrap()
            .unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn next_subscription_frame(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> SubscriptionFrame {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("Worker protocol frame must arrive promptly")
            .unwrap()
            .unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn send_subscription_request(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        request: SubscriptionRequest,
    ) {
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::Request(request));
        stream
            .send(Message::Text(serde_json::to_string(&frame).unwrap().into()))
            .await
            .unwrap();
    }

    fn runtime_protocol_url(worker_protocol_url: &str) -> String {
        let base = worker_protocol_url
            .split_once("/v1/workers/")
            .map(|(base, _)| base)
            .unwrap();
        format!("{base}/v1/protocol/ws")
    }

    #[tokio::test]
    async fn runtime_protocol_ws_subscribes_and_filters_worker_lifecycle() {
        let (runtime, backend, worker_ref, worker_url) = spawn_runtime_server().await;
        let other = runtime
            .create_worker_scoped(
                &RuntimeWorkspaceScope::new("local", "local-token"),
                ws_create_request(),
            )
            .unwrap();
        let url = runtime_protocol_url(&worker_url);
        let (mut stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
        let request_id = protocol::subscription::SubscriptionRequestId::new("subscribe-1").unwrap();
        send_subscription_request(
            &mut stream,
            SubscriptionRequest::SubscribeEvents {
                request_id: request_id.clone(),
                selector: protocol::subscription::EventSubscriptionSelector::WorkerLifecycle {
                    worker_ids: protocol::subscription::SubscriptionWorkerIds::new([
                        protocol::subscription::SubscriptionWorkerId::new(
                            worker_ref.worker_id.to_string(),
                        )
                        .unwrap(),
                    ])
                    .unwrap(),
                },
            },
        )
        .await;
        let subscribed = next_subscription_frame(&mut stream).await;
        let SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed {
            request_id: response_request_id,
            subscription_id,
            snapshot,
            ..
        }) = subscribed.payload
        else {
            panic!("expected subscribed response");
        };
        assert_eq!(response_request_id, request_id);
        let protocol::subscription::SubscriptionSnapshot::Workers { workers } = snapshot else {
            panic!("expected Worker snapshot");
        };
        assert_eq!(workers.len(), 1);
        assert_eq!(
            workers[0].worker_id.as_str(),
            worker_ref.worker_id.to_string()
        );

        let running_snapshot = |worker_ref: &WorkerRef| {
            let mut snapshot = runtime
                .worker_detail(worker_ref)
                .unwrap()
                .worker_state
                .expect("connected test Worker must expose its initial state");
            snapshot.state = protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            ));
            snapshot
        };
        backend.publish(
            &other.worker_ref,
            protocol::Event::WorkerState {
                snapshot: running_snapshot(&other.worker_ref),
            },
        );
        backend.publish(
            &worker_ref,
            protocol::Event::WorkerState {
                snapshot: running_snapshot(&worker_ref),
            },
        );
        let event = next_subscription_frame(&mut stream).await;
        assert!(matches!(
            event.payload,
            SubscriptionFramePayload::Event(SubscriptionEvent::Event {
                subscription_id: delivered_subscription_id,
                payload: protocol::subscription::SubscriptionEventPayload::WorkerUpserted {
                    worker
                },
                ..
            }) if delivered_subscription_id == subscription_id
                && worker.worker_id.as_str() == worker_ref.worker_id.to_string()
                && worker.state == protocol::subscription::SubscriptionWorkerState::Idle
                && matches!(
                    worker.worker_state,
                    Some(protocol::WorkerStateSnapshot {
                        state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                            protocol::WorkerRunState::Running
                        )),
                        ..
                    })
                )
        ));

        let unsubscribe_request_id =
            protocol::subscription::SubscriptionRequestId::new("unsubscribe-1").unwrap();
        send_subscription_request(
            &mut stream,
            SubscriptionRequest::UnsubscribeEvents {
                request_id: unsubscribe_request_id.clone(),
                subscription_id: subscription_id.clone(),
            },
        )
        .await;
        let unsubscribed = next_subscription_frame(&mut stream).await;
        assert!(matches!(
            unsubscribed.payload,
            SubscriptionFramePayload::Response(SubscriptionResponse::Unsubscribed {
                request_id,
                subscription_id: response_subscription_id,
            }) if request_id == unsubscribe_request_id
                && response_subscription_id == subscription_id
        ));
    }

    #[tokio::test]
    async fn protocol_ws_connect_sends_snapshot_and_live_worker_events() {
        let (_runtime, backend, worker_ref, url) = spawn_runtime_server().await;
        let (mut stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();

        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));

        backend.publish(
            &worker_ref,
            protocol::Event::TextDelta {
                text: "started".into(),
            },
        );
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::TextDelta { .. }
        ));
    }

    #[tokio::test]
    async fn protocol_ws_connected_dispatch_preserves_transport_source_and_completion_reply() {
        let (_runtime, backend, worker_ref, url) = spawn_runtime_server().await;
        let mut request = authed_ws_request(&url);
        request.headers_mut().insert(
            protocol::AUTHENTICATED_ACCOUNT_ID_HEADER,
            "account-1".parse().unwrap(),
        );
        let (mut stream, _) = connect_async(request).await.unwrap();
        let _ = next_frame(&mut stream).await;
        let notification = protocol::Method::NotifyTracked {
            notification_request_id: "notification-1".into(),
            message: "hello".into(),
            source: protocol::AuthenticatedInputSource::Account {
                account_id: "forged".into(),
            },
        };
        let completions = protocol::Method::ListCompletions {
            kind: protocol::CompletionKind::Feature,
            prefix: "builtin:".into(),
            request_id: Some("completion-1".into()),
            context: None,
        };
        for method in [notification, completions] {
            stream
                .send(Message::Text(
                    serde_json::to_string(&method).unwrap().into(),
                ))
                .await
                .unwrap();
        }
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Completions { request_id: Some(request_id), prefix, .. }
                if request_id == "completion-1" && prefix == "builtin:"
        ));
        // The reply orders this assertion after dispatch of the preceding frame.
        let execution = backend
            .executions
            .lock()
            .unwrap()
            .get(&worker_ref)
            .unwrap()
            .clone();
        let method = execution
            .controller
            .lock()
            .unwrap()
            .commands
            .try_recv()
            .unwrap();
        assert!(matches!(
            method,
            protocol::Method::NotifyTracked {
                source: protocol::AuthenticatedInputSource::Account { account_id },
                ..
            } if account_id == "account-1"
        ));
    }

    #[tokio::test]
    async fn protocol_ws_stop_closes_old_stream_and_restore_requires_new_connection() {
        let (runtime, backend, worker_ref, url) = spawn_runtime_server().await;
        let (mut old_stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
        let _ = next_frame(&mut old_stream).await;
        runtime.stop_worker(&worker_ref, None).unwrap();
        runtime
            .restore_worker(&worker_ref, runtime.test_restore_request(&worker_ref))
            .unwrap();
        backend.publish(
            &worker_ref,
            protocol::Event::TextDelta {
                text: "new execution".into(),
            },
        );
        let closed = tokio::time::timeout(std::time::Duration::from_secs(5), old_stream.next())
            .await
            .expect("the old protocol connection must close on stop");
        assert!(matches!(closed, Some(Ok(Message::Close(_))) | None));

        let (mut new_stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
        assert!(matches!(
            next_frame(&mut new_stream).await,
            protocol::Event::Snapshot { .. }
        ));
        backend.publish(
            &worker_ref,
            protocol::Event::TextDone {
                text: "restored execution".into(),
            },
        );
        assert!(matches!(
            next_frame(&mut new_stream).await,
            protocol::Event::TextDone { text } if text == "restored execution"
        ));
    }

    #[tokio::test]
    async fn protocol_ws_cursor_resume_is_duplicate_safe_and_filters_workers() {
        let (runtime, backend, worker_ref, url) = spawn_runtime_server().await;
        let other = runtime.create_worker(ws_create_request()).unwrap();
        let first = backend.publish(
            &worker_ref,
            protocol::Event::TextDelta {
                text: "started".into(),
            },
        );
        backend.publish(
            &other.worker_ref,
            protocol::Event::TextDelta {
                text: "other".into(),
            },
        );

        let resume_url = format!("{url}?cursor={}", first.cursor);
        let (mut stream, _) = connect_async(authed_ws_request(&resume_url)).await.unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));

        backend.publish(
            &worker_ref,
            protocol::Event::TextDone {
                text: "done".into(),
            },
        );
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::TextDone { .. }
        ));
    }

    #[tokio::test]
    async fn protocol_ws_old_cursor_never_replays_stopped_execution_after_new_snapshot() {
        let (runtime, backend, worker_ref, url) = spawn_runtime_server().await;
        let old = backend.publish(
            &worker_ref,
            protocol::Event::TextDelta {
                text: "old cursor".into(),
            },
        );
        backend.publish(
            &worker_ref,
            protocol::Event::TextDone {
                text: "old output".into(),
            },
        );
        backend.publish(&worker_ref, protocol::Event::Shutdown);
        runtime.stop_worker(&worker_ref, None).unwrap();
        runtime
            .restore_worker(&worker_ref, runtime.test_restore_request(&worker_ref))
            .unwrap();
        let resume_url = format!("{url}?cursor={}", old.cursor);
        let (mut stream, _) = connect_async(authed_ws_request(&resume_url)).await.unwrap();
        assert!(matches!(
            next_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));
        backend.publish(
            &worker_ref,
            protocol::Event::TextDone {
                text: "new output".into(),
            },
        );
        assert!(
            matches!(next_frame(&mut stream).await, protocol::Event::TextDone { text } if text == "new output")
        );
    }

    #[tokio::test]
    async fn protocol_ws_reports_malformed_cursor_and_method_frame() {
        let (_runtime, _backend, _worker_ref, url) = spawn_runtime_server().await;
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
