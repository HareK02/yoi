use super::*;

use std::future::Future;

const MANAGEMENT_BODY_LIMIT: usize = 8 * 1024 * 1024;

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
    runtime_api::RuntimeApiError::new(error.status.as_u16(), error.code, error.message)
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
            .map(|auth| auth.signer.runtime_id().trim())
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
            Some(scope) => self
                .state
                .runtime
                .stop_worker_scoped(&scope, &worker_ref, value.reason),
            None => self.state.runtime.stop_worker(&worker_ref, value.reason),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let ack = response(ack)?;
        Ok(runtime_api::WorkerLifecycleResponse { ack })
    }

    async fn cancel_worker(
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
                    .cancel_worker_scoped(&scope, &worker_ref, value.reason)
            }
            None => self.state.runtime.cancel_worker(&worker_ref, value.reason),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let ack = response(ack)?;
        Ok(runtime_api::WorkerLifecycleResponse { ack })
    }

    async fn restore_worker(
        &self,
        worker_id: String,
        _request: runtime_api::EmptyObjectRequest,
    ) -> Result<runtime_api::WorkerRestoreResponse, runtime_api::RuntimeApiError> {
        let worker_ref = worker_ref_for(&self.state.runtime, worker_id).map_err(api_error)?;
        let result = match auth_workspace_scope(&self.state, auth_extension().as_ref())
            .map_err(api_error)?
        {
            Some(scope) => self
                .state
                .runtime
                .restore_worker_operation_scoped(&scope, &worker_ref),
            None => self.state.runtime.restore_worker_operation(&worker_ref),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        Ok(runtime_api::WorkerRestoreResponse {
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
            Some(scope) => {
                self.state
                    .runtime
                    .worker_completions_scoped(&scope, &worker_ref, kind, &prefix)
            }
            None => self
                .state
                .runtime
                .worker_completions(&worker_ref, kind, &prefix),
        }
        .map_err(RuntimeHttpRestError::runtime)
        .map_err(api_error)?;
        let entries = response(entries)?;
        Ok(runtime_api::CompletionResponse {
            kind,
            prefix,
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
        let inventory = self
            .state
            .runtime
            .worker_retention_inventory(&scope.workspace_id, &worker_ref)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)?;
        response(inventory)
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
        let request: WorkerRetentionExecutionRequest = request(value)?;
        if request.workspace_id != scope.workspace_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::NOT_FOUND.as_u16(),
                "worker_not_found",
                "worker does not belong to the authenticated Workspace",
            ));
        }
        if request.worker_id != worker_ref.worker_id {
            return Err(runtime_api::RuntimeApiError::new(
                StatusCode::BAD_REQUEST.as_u16(),
                "worker_id_mismatch",
                "Worker retention request worker_id does not match route worker_id",
            ));
        }
        let result = self
            .state
            .runtime
            .execute_worker_retention(&request)
            .map_err(RuntimeHttpRestError::runtime)
            .map_err(api_error)?;
        response(result)
    }
}
