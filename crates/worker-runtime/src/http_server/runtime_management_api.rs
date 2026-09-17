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
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
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
    if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
        return RuntimeHttpRestError::new(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            "request body must be valid JSON",
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
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-workspace-id",
            workspace_id.parse().map_err(|error| {
                runtime_api::RuntimeApiError::new(
                    StatusCode::BAD_REQUEST.as_u16(),
                    "invalid_workspace_header",
                    format!("invalid Workspace ping header: {error}"),
                )
            })?,
        );
        let Json(value) = get_runtime_ping(State(self.state.clone()), required_auth()?, headers)
            .await
            .map_err(api_error)?;
        response(value)
    }

    async fn runtime_summary(
        &self,
    ) -> Result<runtime_api::RuntimeSummaryResponse, runtime_api::RuntimeApiError> {
        let Json(value) = get_runtime(State(self.state.clone()))
            .await
            .map_err(api_error)?;
        response(value)
    }

    async fn list_workers(
        &self,
        query: runtime_api::WorkerListQuery,
    ) -> Result<runtime_api::WorkersResponse, runtime_api::RuntimeApiError> {
        let Json(value) = list_workers(
            State(self.state.clone()),
            auth_extension(),
            Ok(Query(request(query)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn get_worker(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let Json(value) = get_worker(State(self.state.clone()), auth_extension(), Path(worker_id))
            .await
            .map_err(api_error)?;
        response(value)
    }

    async fn create_worker(
        &self,
        value: runtime_api::CreateWorkerRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let Json(value) = create_worker(
            State(self.state.clone()),
            auth_extension(),
            Ok(Json(request(value)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn delete_worker(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerDeleteResponse, runtime_api::RuntimeApiError> {
        let Json(value) =
            delete_worker(State(self.state.clone()), auth_extension(), Path(worker_id))
                .await
                .map_err(api_error)?;
        response(value)
    }

    async fn send_worker_input(
        &self,
        worker_id: String,
        input: runtime_api::WorkerInput,
    ) -> Result<runtime_api::WorkerInputResponse, runtime_api::RuntimeApiError> {
        let Json(value) = send_worker_input(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Ok(Json(request(input)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn stop_worker(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerLifecycleResponse, runtime_api::RuntimeApiError> {
        let body = serde_json::to_vec(&request::<_, RuntimeHttpWorkerLifecycleRequest>(value)?)
            .map_err(|error| {
                runtime_api::RuntimeApiError::new(
                    500,
                    "runtime_request_conversion_failed",
                    error.to_string(),
                )
            })?;
        let Json(value) = stop_worker(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Bytes::from(body),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn cancel_worker(
        &self,
        worker_id: String,
        value: runtime_api::WorkerLifecycleRequest,
    ) -> Result<runtime_api::WorkerLifecycleResponse, runtime_api::RuntimeApiError> {
        let body = serde_json::to_vec(&request::<_, RuntimeHttpWorkerLifecycleRequest>(value)?)
            .map_err(|error| {
                runtime_api::RuntimeApiError::new(
                    500,
                    "runtime_request_conversion_failed",
                    error.to_string(),
                )
            })?;
        let Json(value) = cancel_worker(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Bytes::from(body),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn restore_worker(
        &self,
        worker_id: String,
        _request: runtime_api::EmptyObjectRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let Json(value) =
            restore_worker(State(self.state.clone()), auth_extension(), Path(worker_id))
                .await
                .map_err(api_error)?;
        response(value)
    }

    async fn replace_worker_workspace_api(
        &self,
        worker_id: String,
        value: runtime_api::WorkerWorkspaceApiRequest,
    ) -> Result<runtime_api::WorkerResponse, runtime_api::RuntimeApiError> {
        let Json(value) = replace_worker_workspace_api(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Ok(Json(request(value)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn complete_worker_arguments(
        &self,
        worker_id: String,
        value: runtime_api::CompletionRequest,
    ) -> Result<runtime_api::CompletionResponse, runtime_api::RuntimeApiError> {
        let Json(value) = worker_completions(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Ok(Json(request(value)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn retention_inventory(
        &self,
        worker_id: String,
    ) -> Result<runtime_api::WorkerRetentionInventory, runtime_api::RuntimeApiError> {
        let Json(value) = worker_retention_inventory(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }

    async fn execute_retention(
        &self,
        worker_id: String,
        value: runtime_api::WorkerRetentionExecutionRequest,
    ) -> Result<runtime_api::WorkerRetentionExecutionResult, runtime_api::RuntimeApiError> {
        let Json(value) = execute_worker_retention(
            State(self.state.clone()),
            auth_extension(),
            Path(worker_id),
            Ok(Json(request(value)?)),
        )
        .await
        .map_err(api_error)?;
        response(value)
    }
}
