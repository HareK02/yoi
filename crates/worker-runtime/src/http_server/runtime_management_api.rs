use super::*;

use std::future::Future;

const MANAGEMENT_BODY_LIMIT: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod attachment_control_tests {
    use super::*;

    #[test]
    fn attachment_control_dto_does_not_retain_raw_storage_or_provider_text() {
        for error in [
            RuntimeError::StoreCommitOutcomeUnknown {
                operation: "write Worker aggregate",
                path: std::path::PathBuf::from("/private/runtime/worker.json"),
                source: std::io::Error::other("private-provider-token"),
            },
            RuntimeError::InvalidRequest(
                "private-provider-token /private/runtime/worker.json".into(),
            ),
        ] {
            let cause = error.control_cause();
            let dto = api_error(RuntimeHttpRestError::runtime_control(error));
            assert_eq!(dto.error.code, cause.reason());
            assert_eq!(dto.error.message, cause.explanation());
            assert_eq!(dto.control_cause(), Some(cause));
            assert!(dto.error.startup.is_none());
            let serialized = serde_json::to_string(&dto).unwrap();
            assert!(!serialized.contains("/private/runtime"));
            assert!(!serialized.contains("private-provider-token"));
        }
    }
}

#[cfg(test)]
mod release_capture_tests {
    use super::*;

    #[test]
    fn release_capture_uses_worker_bound_retention_authority() {
        let path = "/v1/workers/released-worker/retention/release-capture";
        assert_eq!(
            required_runtime_permission(&Method::POST, path),
            Some("workers:delete")
        );
        assert_eq!(
            worker_id_from_runtime_path(path).as_deref(),
            Some("released-worker")
        );
    }
}

// Axum request extensions remain the authorization authority. The generated adapter deliberately
// has no authentication model, so the verified extension is scoped across its service call.
tokio::task_local! {
    static AUTH_CONTEXT: Option<RuntimeAuthContext>;
}

pub(super) async fn scope_auth<F>(context: Option<RuntimeAuthContext>, future: F) -> F::Output
where
    F: Future,
{
    AUTH_CONTEXT.scope(context, future).await
}

fn auth_extension() -> Option<Extension<RuntimeAuthContext>> {
    AUTH_CONTEXT
        .try_with(Clone::clone)
        .ok()
        .flatten()
        .map(Extension)
}

fn required_auth() -> Result<Extension<RuntimeAuthContext>, runtime_api::RuntimeApiError> {
    auth_extension().ok_or_else(|| {
        runtime_api::RuntimeApiError::new(
            StatusCode::FORBIDDEN.as_u16(),
            "workspace_scope_required",
            "Runtime operation requires an authorization context",
        )
    })
}

fn material_scope(
    state: &RuntimeHttpState,
    workspace_id: &str,
) -> Result<RuntimeWorkspaceScope, runtime_api::RuntimeApiError> {
    let scope = auth_workspace_scope(state, auth_extension().as_ref())
        .map_err(api_error)?
        .ok_or_else(|| {
            runtime_api::RuntimeApiError::new(
                403,
                "runtime_session_material_scope_required",
                "Canonical Session material requires a Workspace-scoped capability",
            )
        })?;
    if scope.workspace_id != workspace_id {
        return Err(runtime_api::RuntimeApiError::new(
            403,
            "runtime_session_material_scope_mismatch",
            "Canonical Session Workspace does not match the authenticated capability",
        ));
    }
    Ok(scope)
}

fn material_pin(
    state: &RuntimeHttpState,
    request: runtime_api::SessionMaterialPinRequest,
    release: bool,
) -> Result<runtime_api::SessionMaterialPinResponse, runtime_api::RuntimeApiError> {
    let scope = material_scope(state, &request.workspace_id)?;
    #[cfg(feature = "fs-store")]
    {
        let pin = request
            .pin_id
            .parse::<uuid::Uuid>()
            .map(session_store::retention::SessionRetentionPin)
            .map_err(|_| {
                runtime_api::RuntimeApiError::new(
                    400,
                    "invalid_session_retention_pin",
                    "Invalid Session retention pin identity",
                )
            })?;
        if release {
            state.runtime.release_session_public_source_pin_scoped(
                &scope,
                &request.source,
                &request.expected_session_id,
                pin,
            )
        } else {
            state.runtime.pin_session_public_source_scoped(
                &scope,
                &request.source,
                &request.expected_session_id,
                pin,
            )
        }
        .map_err(|_| {
            runtime_api::RuntimeApiError::new(
                409,
                "session_retention_rejected",
                "Session retention operation could not be verified",
            )
        })?;
        Ok(runtime_api::SessionMaterialPinResponse {
            session_id: request.expected_session_id,
            pin_id: request.pin_id,
        })
    }
    #[cfg(not(feature = "fs-store"))]
    {
        let _ = (scope, release);
        Err(runtime_api::RuntimeApiError::new(
            503,
            "session_retention_unavailable",
            "Session retention requires filesystem storage",
        ))
    }
}

#[derive(Clone)]
struct RuntimeManagementApi {
    state: RuntimeHttpState,
}

pub(super) fn router(state: RuntimeHttpState) -> Router {
    runtime_api::RuntimeApiAxum::router(RuntimeManagementApi { state })
        .layer(axum::extract::DefaultBodyLimit::max(MANAGEMENT_BODY_LIMIT))
        .layer(middleware::from_fn(runtime_api_wire_compatibility))
}

async fn runtime_api_wire_compatibility(
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let is_json_request = !matches!(*request.method(), Method::GET | Method::HEAD);
    let is_query_request = request.method() == Method::GET;
    if request.uri().path() == "/v1/ping" && !request.headers().contains_key("x-yoi-workspace-id") {
        return RuntimeHttpRestError::new(
            StatusCode::FORBIDDEN,
            "runtime_ping_workspace_scope_required",
            "Runtime ping requires an authenticated Workspace scope",
        )
        .into_response();
    }
    if is_json_request {
        if !request.headers().contains_key(header::CONTENT_TYPE) {
            request.headers_mut().insert(
                header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
        }
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, MANAGEMENT_BODY_LIMIT).await {
            Ok(body) if body.is_empty() => Bytes::from_static(b"{}"),
            Ok(body) => body,
            Err(_) => {
                return RuntimeHttpRestError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request_body_too_large",
                    "request body exceeds Runtime management limit",
                )
                .into_response();
            }
        };
        request = axum::extract::Request::from_parts(parts, Body::from(body));
    }
    let response = next.run(request).await;
    let is_json_error = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if matches!(
        response.status(),
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
    ) && !is_json_error
        && response
            .extensions()
            .get::<Arc<runtime_api::RuntimeApiError>>()
            .is_none()
    {
        return RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            if is_query_request {
                "invalid_query"
            } else {
                "invalid_json"
            },
            if is_query_request {
                "request query parameters are invalid"
            } else {
                "request body must be valid JSON"
            },
        )
        .into_response();
    }
    response
}

fn request<T, U>(value: T) -> Result<U, runtime_api::RuntimeApiError>
where
    T: Serialize,
    U: serde::de::DeserializeOwned,
{
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| {
            runtime_api::RuntimeApiError::new(
                StatusCode::BAD_REQUEST.as_u16(),
                "invalid_request",
                error.to_string(),
            )
        })
}

fn response<T, U>(value: T) -> Result<U, runtime_api::RuntimeApiError>
where
    T: Serialize,
    U: serde::de::DeserializeOwned,
{
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| {
            runtime_api::RuntimeApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                "runtime_response_conversion_failed",
                error.to_string(),
            )
        })
}

fn api_error(error: RuntimeHttpRestError) -> runtime_api::RuntimeApiError {
    let mut response =
        runtime_api::RuntimeApiError::new(error.status.as_u16(), error.code, error.message);
    if error.startup.is_none() {
        if let Some(cause) = error.control {
            response = response.with_control(cause).with_command(error.command);
        }
    }
    response.error.startup = error.startup;
    if let Some(cause) = error.management {
        response = response.with_management(cause);
    }
    if let Some(failure) = error.create_failure {
        return response.with_create_failure(failure);
    }
    response
}

impl runtime_api::RuntimeApi for RuntimeManagementApi {
    async fn ping(
        &self,
        workspace_id: String,
    ) -> Result<runtime_api::RuntimePingResponse, runtime_api::RuntimeApiError> {
        let Extension(auth) = required_auth()?;
        if workspace_id != auth.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::FORBIDDEN.as_u16(),
                "runtime_ping_workspace_scope_mismatch",
                "Runtime ping Workspace scope does not match the authenticated capability",
            ));
        }
        let runtime_id = self
            .state
            .workspace_auth
            .as_ref()
            .map(|auth| auth.runtime_id.trim())
            .filter(|runtime_id| !runtime_id.is_empty())
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    "runtime_ping_identity_unavailable",
                    "Runtime ping identity is not configured",
                )
            })?;
        Ok(runtime_api::RuntimePingResponse {
            runtime_id: runtime_id.to_string(),
            protocol_version: RUNTIME_HTTP_PROTOCOL_VERSION,
        })
    }

    async fn runtime_summary(
        &self,
    ) -> Result<runtime_api::RuntimeSummaryResponse, runtime_api::RuntimeApiError> {
        let runtime = self
            .state
            .runtime
            .summary()
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)?;
        let runtime = response(runtime)?;
        Ok(runtime_api::RuntimeSummaryResponse { runtime })
    }

    async fn list_workers(
        &self,
        query: runtime_api::WorkerListQuery,
    ) -> Result<runtime_api::WorkersResponse, runtime_api::RuntimeApiError> {
        let scope =
            auth_workspace_scope(&self.state, auth_extension().as_ref()).map_err(api_error)?;
        let workers = match (query.status, scope.as_ref()) {
            (Some(runtime_api::WorkerStatusFilter::Stopped), Some(scope)) => {
                self.state.runtime.list_stopped_workers_scoped(scope)
            }
            (Some(runtime_api::WorkerStatusFilter::Stopped), None) => {
                self.state.runtime.list_stopped_workers()
            }
            (None, Some(scope)) => self.state.runtime.list_workers_scoped(scope),
            (None, None) => self.state.runtime.list_workers(),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let workers = response(workers)?;
        Ok(runtime_api::WorkersResponse { workers })
    }

    async fn get_worker(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let worker = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self.state.runtime.worker_detail_scoped(&scope, &worker_ref),
            None => self.state.runtime.worker_detail(&worker_ref),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let worker = response(worker)?;
        Ok(runtime_api::WorkerResponse { worker })
    }

    async fn worker_session(
        &self,
        worker_id: String,
        request: runtime_api::WorkerSessionRequest,
    ) -> Result<runtime_api::WorkerSessionAvailability, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "runtime_worker_session_scope_required",
                    "Worker Session observation requires a Workspace-scoped capability",
                )
            })?;
        if scope.workspace_id != request.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::FORBIDDEN.as_u16(),
                "runtime_worker_session_workspace_scope_mismatch",
                "Worker Session Workspace scope does not match the authenticated capability",
            ));
        }
        self.state
            .runtime
            .worker_session_scoped(&scope, &worker_ref)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn worker_session_history(
        &self,
        worker_id: String,
        request: runtime_api::WorkerSessionHistoryRequest,
    ) -> Result<runtime_api::WorkerSessionHistoryAvailability, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "runtime_worker_session_scope_required",
                    "Worker Session history requires a Workspace-scoped capability",
                )
            })?;
        if scope.workspace_id != request.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::FORBIDDEN.as_u16(),
                "runtime_worker_session_workspace_scope_mismatch",
                "Worker Session Workspace scope does not match the authenticated capability",
            ));
        }
        self.state
            .runtime
            .worker_session_history_scoped(&scope, &worker_ref, request.cursor, request.limit)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn session_material_erase(
        &self,
        request: runtime_api::SessionMaterialEraseRequest,
    ) -> Result<runtime_api::SessionMaterialEraseResponse, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        #[cfg(feature = "fs-store")]
        {
            self.state
                .runtime
                .delete_session_public_source_scoped(
                    &scope,
                    &request.source,
                    &request.expected_session_id,
                    session_store::retention::SessionDeletionIntent::ErasePermanently,
                )
                .map_err(|_| {
                    runtime_api::RuntimeApiError::new(
                        409,
                        "session_erasure_incomplete",
                        "Session erasure could not complete; retain erasure intent and retry",
                    )
                })?;
            Ok(runtime_api::SessionMaterialEraseResponse {
                session_id: request.expected_session_id,
            })
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = scope;
            Err(runtime_api::RuntimeApiError::new(
                503,
                "session_retention_unavailable",
                "Session erasure requires filesystem storage",
            ))
        }
    }

    async fn purge_subject_job_removal(
        &self,
        request: runtime_api::SubjectJobRemovalPurgeRequest,
    ) -> Result<runtime_api::SubjectJobRemovalPurgeResponse, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.deletion.workspace_id)?;
        self.state
            .runtime
            .purge_subject_job_removal_scoped(&scope, request)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn subject_delete_worker(
        &self,
        request: runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<runtime_api::SubjectDeleteWorkerResponse, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        self.state
            .runtime
            .subject_delete_worker_scoped(&scope, request)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn recover_subject_create(
        &self,
        request: runtime_api::SubjectCreateRecoveryRequest,
    ) -> Result<runtime_api::SubjectCreateRecoveryResponse, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        self.state
            .runtime
            .recover_subject_create_scoped(&scope, request)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn subject_release(
        &self,
        request: runtime_api::SubjectReleaseRequest,
    ) -> Result<runtime_api::SubjectReleaseResponse, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        self.state
            .runtime
            .subject_release_scoped(&scope, request)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)
    }

    async fn session_material_pin(
        &self,
        request: runtime_api::SessionMaterialPinRequest,
    ) -> Result<runtime_api::SessionMaterialPinResponse, runtime_api::RuntimeApiError> {
        material_pin(&self.state, request, false)
    }

    async fn session_material_release(
        &self,
        request: runtime_api::SessionMaterialPinRequest,
    ) -> Result<runtime_api::SessionMaterialPinResponse, runtime_api::RuntimeApiError> {
        material_pin(&self.state, request, true)
    }

    async fn session_recording_capture(
        &self,
        request: runtime_api::SessionRecordingCaptureRequest,
    ) -> Result<runtime_api::SessionRecordingCaptureAvailability, runtime_api::RuntimeApiError>
    {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        #[cfg(feature = "fs-store")]
        {
            Ok(self
                .state
                .runtime
                .session_recording_capture_scoped(&scope, &request))
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = scope;
            Ok(
                runtime_api::SessionRecordingCaptureAvailability::Unavailable {
                    reason: runtime_api::SessionPublicUnavailableReason::StorageUnavailable,
                    message: "Committed Session storage unavailable".into(),
                },
            )
        }
    }

    async fn session_material_catalog(
        &self,
        request: runtime_api::SessionMaterialCatalogRequest,
    ) -> Result<runtime_api::SessionMaterialCatalogAvailability, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        #[cfg(feature = "fs-store")]
        {
            Ok(self
                .state
                .runtime
                .session_material_catalog_scoped(&scope, &request))
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = scope;
            Ok(
                runtime_api::SessionMaterialCatalogAvailability::Unavailable {
                    reason: runtime_api::SessionPublicUnavailableReason::StorageUnavailable,
                    message: "Canonical Session storage unavailable".into(),
                },
            )
        }
    }

    async fn session_material_read(
        &self,
        request: runtime_api::SessionMaterialReadRequest,
    ) -> Result<runtime_api::SessionMaterialReadAvailability, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        #[cfg(feature = "fs-store")]
        {
            Ok(self
                .state
                .runtime
                .session_material_read_scoped(&scope, &request))
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = scope;
            Ok(runtime_api::SessionMaterialReadAvailability::Unavailable {
                reason: runtime_api::SessionPublicUnavailableReason::StorageUnavailable,
                message: "Canonical Session storage unavailable".into(),
            })
        }
    }

    async fn session_public_search(
        &self,
        request: runtime_api::SessionPublicSearchRequest,
    ) -> Result<runtime_api::SessionPublicSearchAvailability, runtime_api::RuntimeApiError> {
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "runtime_session_public_scope_required",
                    "Session public search requires a Workspace-scoped capability",
                )
            })?;
        if scope.workspace_id != request.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::FORBIDDEN.as_u16(),
                "runtime_session_public_workspace_scope_mismatch",
                "Session public search Workspace scope does not match the authenticated capability",
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            self.state
                .runtime
                .session_public_search_scoped(&scope, &request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            Ok(runtime_api::SessionPublicSearchAvailability::Unavailable {
                reason: runtime_api::SessionPublicUnavailableReason::StorageUnavailable,
                message: "Session public storage is unavailable".to_string(),
            })
        }
    }

    async fn session_public_read(
        &self,
        request: runtime_api::SessionPublicReadRequest,
    ) -> Result<runtime_api::SessionPublicReadAvailability, runtime_api::RuntimeApiError> {
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "runtime_session_public_scope_required",
                    "Session public read requires a Workspace-scoped capability",
                )
            })?;
        if scope.workspace_id != request.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::FORBIDDEN.as_u16(),
                "runtime_session_public_workspace_scope_mismatch",
                "Session public read Workspace scope does not match the authenticated capability",
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            self.state
                .runtime
                .session_public_read_scoped(&scope, &request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            Ok(runtime_api::SessionPublicReadAvailability::Unavailable {
                reason: runtime_api::SessionPublicUnavailableReason::StorageUnavailable,
                message: "Session public storage is unavailable".to_string(),
            })
        }
    }

    async fn create_worker(
        &self,
        value: runtime_api::CreateWorkerRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let request: CreateWorkerRequest = request(value)?;
        let worker = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self.state.runtime.create_worker_scoped(&scope, request),
            None => self.state.runtime.create_worker(request),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let worker = response(worker)?;
        Ok(runtime_api::WorkerResponse { worker })
    }

    async fn delete_worker(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerDeleteResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let worker = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self.state.runtime.delete_worker_scoped(&scope, &worker_ref),
            None => self.state.runtime.delete_worker(&worker_ref),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let worker = response(worker)?;
        Ok(runtime_api::WorkerDeleteResponse { worker })
    }

    async fn send_worker_input(
        &self,
        worker_id: String,
        input: runtime_api::WorkerInput,
    ) -> Result<runtime_api::WorkerInputResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let input: WorkerInput = request(input)?;
        let ack = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self
                .state
                .runtime
                .send_input_scoped(&scope, &worker_ref, input),
            None => self.state.runtime.send_input(&worker_ref, input),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let ack = response(ack)?;
        Ok(runtime_api::WorkerInputResponse { ack })
    }

    async fn stop_worker(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerLifecycleResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let ack = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => {
                self.state
                    .runtime
                    .stop_worker_request_scoped(&scope, &worker_ref, value)
            }
            None => self.state.runtime.stop_worker_request(&worker_ref, value),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let ack = response(ack)?;
        Ok(runtime_api::WorkerLifecycleResponse { ack })
    }

    async fn coordinate_worker_stop(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerStopOutcome, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let outcome = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => {
                self.state
                    .runtime
                    .coordinate_worker_stop_scoped(&scope, &worker_ref, value)
            }
            None => self
                .state
                .runtime
                .coordinate_worker_stop(&worker_ref, value),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        response(outcome)
    }

    async fn worker_stop_outcome(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerStopOutcome, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let outcome = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => {
                self.state
                    .runtime
                    .worker_stop_outcome_scoped(&scope, &worker_ref, value)
            }
            None => self.state.runtime.worker_stop_outcome(&worker_ref, value),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        response(outcome)
    }

    async fn cancel_worker(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerLifecycleResponse, runtime_api::RuntimeApiError> {
        validate_cancel_request(&value).map_err(api_error)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let ack = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => {
                self.state
                    .runtime
                    .cancel_worker_scoped(&scope, &worker_ref, value.reason)
            }
            None => self.state.runtime.cancel_worker(&worker_ref, value.reason),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let ack = response(ack)?;
        Ok(runtime_api::WorkerLifecycleResponse { ack })
    }

    async fn coordinate_worker_restore(
        &self,
        worker_id: String,
        request: runtime_api::WorkerRestoreCoordinationRequest,
    ) -> Result<runtime_api::WorkerRestoreCoordinationResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let scope =
            auth_workspace_scope(&self.state, auth_extension().as_ref()).map_err(api_error)?;
        let runtime = self.state.runtime.clone();
        let (result, preparation) = tokio::task::spawn_blocking(move || {
            runtime.coordinate_worker_restore_operation(&worker_ref, request, scope.as_ref())
        })
        .await
        .map_err(|error| {
            runtime_api::RuntimeApiError::new(500, "runtime_restore_task_failed", error.to_string())
        })?
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let result = result
            .map(|result| -> Result<_, runtime_api::RuntimeApiError> {
                Ok(runtime_api::WorkerRestoreResponse {
                    control: result.control,
                    startup: result.startup,
                    state: match result.state {
                        server_api::WorkerRestoreState::Accepted => {
                            runtime_api::WorkerRestoreState::Accepted
                        }
                        server_api::WorkerRestoreState::Rejected => {
                            runtime_api::WorkerRestoreState::Rejected
                        }
                        server_api::WorkerRestoreState::RolledBack => {
                            runtime_api::WorkerRestoreState::RolledBack
                        }
                        server_api::WorkerRestoreState::Superseded => {
                            runtime_api::WorkerRestoreState::Superseded
                        }
                        server_api::WorkerRestoreState::ReconciliationRequired => {
                            runtime_api::WorkerRestoreState::ReconciliationRequired
                        }
                    },
                    worker: result.worker.map(response).transpose()?,
                    reason_code: result.reason_code,
                    message: result.message,
                })
            })
            .transpose()?;
        Ok(runtime_api::WorkerRestoreCoordinationResponse {
            result,
            preparation,
        })
    }

    async fn restore_worker(
        &self,
        worker_id: String,
        request: runtime_api::WorkerRestoreRequest,
    ) -> Result<runtime_api::WorkerRestoreResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let scope =
            auth_workspace_scope(&self.state, auth_extension().as_ref()).map_err(api_error)?;
        let runtime = self.state.runtime.clone();
        let result = tokio::task::spawn_blocking(move || match scope {
            Some(scope) => runtime.restore_worker_operation_scoped(&scope, &worker_ref, request),
            None => runtime.restore_worker_operation(&worker_ref, request),
        })
        .await
        .map_err(|error| {
            runtime_api::RuntimeApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                "runtime_restore_task_failed",
                error.to_string(),
            )
        })?
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        Ok(runtime_api::WorkerRestoreResponse {
            control: result.control,
            startup: result.startup,
            state: match result.state {
                server_api::WorkerRestoreState::Accepted => {
                    runtime_api::WorkerRestoreState::Accepted
                }
                server_api::WorkerRestoreState::Rejected => {
                    runtime_api::WorkerRestoreState::Rejected
                }
                server_api::WorkerRestoreState::RolledBack => {
                    runtime_api::WorkerRestoreState::RolledBack
                }
                server_api::WorkerRestoreState::Superseded => {
                    runtime_api::WorkerRestoreState::Superseded
                }
                server_api::WorkerRestoreState::ReconciliationRequired => {
                    runtime_api::WorkerRestoreState::ReconciliationRequired
                }
            },
            worker: result.worker.map(response).transpose()?,
            reason_code: result.reason_code,
            message: result.message,
        })
    }

    async fn replace_worker_workspace_api(
        &self,
        worker_id: String,
        value: runtime_api::WorkerWorkspaceApiRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let workspace_api: WorkspaceApiRef = request(value.workspace_api)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let worker = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self.state.runtime.replace_worker_workspace_api_scoped(
                &scope,
                &worker_ref,
                workspace_api,
            ),
            None => self
                .state
                .runtime
                .replace_worker_workspace_api(&worker_ref, workspace_api),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let worker = response(worker)?;
        Ok(runtime_api::WorkerResponse { worker })
    }

    async fn replace_worker_workdir_attachments(
        &self,
        worker_id: String,
        value: runtime_api::WorkerWorkdirAttachmentsRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let attachments = request(value.logical_workdir_attachments)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let worker = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self
                .state
                .runtime
                .replace_worker_workdir_attachments_scoped(&scope, &worker_ref, attachments),
            None => self
                .state
                .runtime
                .replace_worker_workdir_attachments(&worker_ref, attachments),
        }
        .map_err(RuntimeHttpRestError::runtime_control)
        .map_err(api_error)?;
        let worker = response(worker)?;
        Ok(runtime_api::WorkerResponse { worker })
    }

    async fn observe_worker_workdir_attachments(
        &self,
        worker_id: String,
        value: runtime_api::WorkerWorkdirAttachmentsRequest,
    ) -> Result<runtime_api::WorkerWorkdirAttachmentsObservation, runtime_api::RuntimeApiError>
    {
        let attachments = request(value.logical_workdir_attachments.clone())?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let result = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self
                .state
                .runtime
                .observe_worker_workdir_attachments_state_scoped(&scope, &worker_ref, attachments),
            None => self
                .state
                .runtime
                .observe_worker_workdir_attachments_state(&worker_ref, attachments),
        }
        .map_err(RuntimeHttpRestError::runtime_control)
        .map_err(api_error)?;
        // Preserve the caller's exact set ordering in the observation echo.
        Ok(runtime_api::WorkerWorkdirAttachmentsObservation {
            request: value,
            ..result
        })
    }

    async fn apply_worker_workdir_attachments_operation(
        &self,
        worker_id: String,
        request: runtime_api::WorkerWorkdirAttachmentsOperationRequest,
    ) -> Result<runtime_api::WorkerWorkdirAttachmentsOperationReceipt, runtime_api::RuntimeApiError>
    {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        match auth_workspace_scope(&self.state, auth_extension().as_ref()).map_err(api_error)? {
            Some(scope) => self
                .state
                .runtime
                .apply_worker_workdir_attachments_operation_scoped(&scope, &worker_ref, request),
            None => self
                .state
                .runtime
                .apply_worker_workdir_attachments_operation(&worker_ref, request),
        }
        .map_err(RuntimeHttpRestError::runtime_control)
        .map_err(api_error)
    }

    async fn observe_worker_workdir_attachments_operation(
        &self,
        worker_id: String,
        request: runtime_api::WorkerWorkdirAttachmentsOperationRequest,
    ) -> Result<
        runtime_api::WorkerWorkdirAttachmentsOperationObservation,
        runtime_api::RuntimeApiError,
    > {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        match auth_workspace_scope(&self.state, auth_extension().as_ref()).map_err(api_error)? {
            Some(scope) => self
                .state
                .runtime
                .observe_worker_workdir_attachments_operation_scoped(&scope, &worker_ref, request),
            None => self
                .state
                .runtime
                .observe_worker_workdir_attachments_operation(&worker_ref, request),
        }
        .map_err(RuntimeHttpRestError::runtime_control)
        .map_err(api_error)
    }

    async fn complete_worker_arguments(
        &self,
        worker_id: String,
        value: runtime_api::CompletionRequest,
    ) -> Result<runtime_api::CompletionResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let kind = value.kind;
        let prefix = value.prefix;
        let entries = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self.state.runtime.worker_completions_scoped(
                &scope,
                &worker_ref,
                kind,
                &prefix,
                value.context.as_ref(),
            ),
            None => self.state.runtime.worker_completions(
                &worker_ref,
                kind,
                &prefix,
                value.context.as_ref(),
            ),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let entries = response(entries)?;
        Ok(runtime_api::CompletionResponse {
            kind,
            prefix,
            context: value.context,
            request_id: value.request_id,
            entries,
        })
    }

    async fn retention_inventory(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerRetentionInventory, runtime_api::RuntimeApiError> {
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "workspace_scope_required",
                    "Worker retention inventory requires workspace-scoped authorization",
                )
            })?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        #[cfg(feature = "fs-store")]
        {
            let inventory = self
                .state
                .runtime
                .worker_retention_inventory(&scope.workspace_id, &worker_ref)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)?;
            response(inventory)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = (scope, worker_ref);
            Err(runtime_api::RuntimeApiError::new(
                503,
                "worker_retention_unavailable",
                "Worker retention requires filesystem storage",
            ))
        }
    }

    async fn freeze_worker_removal(
        &self,
        worker_id: String,
        value: runtime_api::WorkerRetentionExecutionRequest,
    ) -> Result<runtime_api::WorkerRemovalFreeze, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &value.workspace_id)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        if value.worker_id != worker_ref.worker_id {
            return Err(runtime_api::RuntimeApiError::new(
                400,
                "worker_id_mismatch",
                "Worker removal route identity mismatch",
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            let request: WorkerRetentionExecutionRequest = request(value)?;
            self.state
                .runtime
                .freeze_worker_removal_scoped(&scope, &request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = (scope, value);
            Err(runtime_api::RuntimeApiError::new(
                503,
                "worker_retention_unavailable",
                "Worker removal requires filesystem storage",
            ))
        }
    }

    async fn publish_worker_removal_archive(
        &self,
        worker_id: String,
        value: runtime_api::WorkerRetentionExecutionRequest,
    ) -> Result<runtime_api::WorkerSessionArchiveManifest, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &value.workspace_id)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        if value.worker_id != worker_ref.worker_id {
            return Err(runtime_api::RuntimeApiError::new(
                400,
                "worker_id_mismatch",
                "Frozen archive route identity mismatch",
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            let request: WorkerRetentionExecutionRequest = request(value)?;
            let manifest = self
                .state
                .runtime
                .publish_worker_removal_archive_scoped(&scope, &request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)?;
            response(manifest)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = (scope, value);
            Err(runtime_api::RuntimeApiError::new(
                503,
                "worker_retention_unavailable",
                "Frozen archive requires filesystem storage",
            ))
        }
    }

    async fn capture_subject_release_session(
        &self,
        worker_id: String,
        request: runtime_api::SubjectReleaseSessionCaptureRequest,
    ) -> Result<session_store::capture::RecordingCaptureSlice, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        #[cfg(feature = "fs-store")]
        {
            self.state
                .runtime
                .capture_subject_release_session_scoped(&scope, worker_ref.worker_id, &request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = (scope, worker_ref, request);
            Err(runtime_api::RuntimeApiError::new(
                503,
                "subject_release_capture_unavailable",
                "Subject release capture requires filesystem storage",
            ))
        }
    }

    async fn capture_worker_removal_session(
        &self,
        worker_id: String,
        request: runtime_api::WorkerRemovalSessionCaptureRequest,
    ) -> Result<session_store::capture::RecordingCaptureSlice, runtime_api::RuntimeApiError> {
        let scope = material_scope(&self.state, &request.workspace_id)?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        #[cfg(feature = "fs-store")]
        {
            self.state
                .runtime
                .capture_worker_removal_session_scoped(
                    &scope,
                    &request.operation_id,
                    worker_ref.worker_id,
                    request.session_id,
                    request.active_segment_id,
                    request.start,
                    request.max_entries,
                    request.max_serialized_bytes,
                )
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)
        }
        #[cfg(not(feature = "fs-store"))]
        {
            let _ = (scope, worker_ref, request);
            Err(runtime_api::RuntimeApiError::new(
                503,
                "worker_retention_unavailable",
                "Worker removal requires filesystem storage",
            ))
        }
    }

    async fn execute_retention(
        &self,
        worker_id: String,
        value: runtime_api::WorkerRetentionExecutionRequest,
    ) -> Result<runtime_api::WorkerRetentionExecutionResult, runtime_api::RuntimeApiError> {
        let scope = auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
            .ok_or_else(|| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::FORBIDDEN.as_u16(),
                    "workspace_scope_required",
                    "Worker retention execution requires workspace-scoped authorization",
                )
            })?;
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        if value.workspace_id != scope.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::NOT_FOUND.as_u16(),
                "worker_not_found",
                "worker does not belong to the authenticated Workspace",
            ));
        }
        if value.worker_id != worker_ref.worker_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::BAD_REQUEST.as_u16(),
                "worker_id_mismatch",
                "Worker retention request worker_id does not match route worker_id",
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            let request: WorkerRetentionExecutionRequest = request(value)?;
            let result = self
                .state
                .runtime
                .execute_worker_retention(&request)
                .map_err(RuntimeHttpRestError::runtime)
                .map_err(api_error)?;
            response(result)
        }
        #[cfg(not(feature = "fs-store"))]
        Err(runtime_api::RuntimeApiError::new(
            503,
            "worker_retention_unavailable",
            "Worker retention requires filesystem storage",
        ))
    }
}
