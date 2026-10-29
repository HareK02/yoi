//! Both authenticated Remote requests and Embedded Host calls enter this owner.
//! It does not trust human catalog credentials or accept execution identities in
//! JSON. Network dispatch is single-attempt and follows durable effect admission.
use super::*;
use crate::{plugin_host as policy, plugin_https as https, plugin_receipt as receipt};
use rusqlite::OptionalExtension;
use server_api::*;

impl WorkspaceApi {
    /// Host plan creation after the canonical reservation and grant/Job binding,
    /// before Runtime dispatch. The reservation is re-read, never trusted as a
    /// caller-issued principal. This derives identity only, not execution rights.
    pub(crate) fn plugin_reserved_execution_identity(
        &self,
        runtime_id: &str,
        reservation: &crate::store::WorkerCreateReservation,
        expected_grant: Option<&server_api::authorization::WorkerExecutionGrant>,
    ) -> crate::Result<crate::plugin_plan::HostExecutionIdentity> {
        let worker = RuntimeWorkerRef::new(runtime_id, reservation.worker_id.to_string());
        self.plugin_reserved_identity(
            &worker,
            Some(&reservation.create_fingerprint),
            expected_grant,
        )
    }

    pub(super) fn plugin_reserved_identity(
        &self,
        worker: &RuntimeWorkerRef,
        expected_fingerprint: Option<&str>,
        expected_grant: Option<&server_api::authorization::WorkerExecutionGrant>,
    ) -> crate::Result<crate::plugin_plan::HostExecutionIdentity> {
        let refused = || policy::failure(PluginHostErrorCode::Denied);
        let (fingerprint, allocation) = self.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT create_fingerprint,allocation_key FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state='reserved'",
                rusqlite::params![self.workspace_id(), worker.runtime_id, worker.worker_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).optional()?)
        })?.ok_or_else(refused)?;
        // A Job which has not yet acquired its canonical attempt/Worker binding
        // must not accidentally become a legacy ordinary Worker principal.
        if allocation.starts_with("backend-job:")
            && self
                .store
                .worker_registry_projection(self.workspace_id(), worker)?
                .and_then(|projection| projection.job)
                .is_none()
        {
            return Err(refused());
        }
        if expected_fingerprint.is_some_and(|expected| expected != fingerprint) {
            return Err(refused());
        }
        self.config_store
            .get_workspace_runtime_binding(self.workspace_id(), &worker.runtime_id)?
            .filter(|binding| binding.revoked_at.is_none())
            .ok_or_else(refused)?;
        self.store
            .require_current_worker_singleton_owner(self.workspace_id(), worker)?;
        if let Some(expected) = expected_grant {
            let mut expected = expected.clone();
            expected.worker_id = worker.worker_id.clone();
            let actual = open_subjektiv_store(self)
                .map_err(|e| e.error)?
                .worker_execution_grant(&worker.runtime_id, &worker.worker_id)
                .map_err(|_| refused())?;
            if actual.as_ref() != Some(&expected) {
                return Err(refused());
            }
        }
        // This is the same owner as live identity. Do not manufacture a separate
        // principal/delegation spelling or copy policy into the reservation.
        subject_authorization::plugin_execution_identity(self, worker).map_err(|e| e.error)
    }
}

use tokio_util::sync::CancellationToken;

const RECEIPT_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub(super) fn router(service: Arc<ServerApiContractService>) -> Router {
    // Vec<u8> uses JSON's numeric array representation (up to four wire bytes
    // per decoded byte). Reserve bounded room for IDs, URL/query and escaped
    // headers; the shared owner still enforces every decoded field/body limit.
    let body_ceiling = crate::plugin_binding::PluginConnectionPolicyCeiling::default()
        .limits
        .max_request_bytes as usize;
    server_api_axum::plugin_host(service)
        .layer(DefaultBodyLimit::max(4 * body_ceiling + 512 * 1024))
}

fn denied() -> PluginHostError {
    policy::map_error(policy::failure(PluginHostErrorCode::Denied))
}
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn operation_name(op: &PluginHostOperation) -> &'static str {
    match op {
        PluginHostOperation::Admit => "plugin.admit",
        PluginHostOperation::Publish => "plugin.publish",
        PluginHostOperation::Open { .. } => "plugin.connection.open",
        PluginHostOperation::Send { .. } => "plugin.http.send",
        PluginHostOperation::Log => "plugin.log",
    }
}

async fn check(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    plan_id: String,
    installation: String,
    consumer: PluginConsumer,
    operation: PluginHostOperation,
) -> std::result::Result<policy::AdmittedPluginOperation, PluginHostError> {
    let tools = match consumer {
        PluginConsumer::ToolCall {
            tool_call_id,
            tool_name,
        } if !tool_call_id.is_empty()
            && tool_call_id.len() <= 512
            && !tool_call_id.chars().any(char::is_control) =>
        {
            vec![tool_name]
        }
        _ => {
            return Err(policy::map_error(policy::failure(
                PluginHostErrorCode::Unsupported,
            )));
        }
    };
    check_tools(
        api,
        context,
        workspace,
        plan_id,
        installation,
        tools,
        operation,
        false,
    )
    .await
}

async fn check_tools(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    plan_id: String,
    installation: String,
    tools: Vec<String>,
    operation: PluginHostOperation,
    preparing: bool,
) -> std::result::Result<policy::AdmittedPluginOperation, PluginHostError> {
    validate_workspace_scope(&api, &workspace).map_err(|e| policy::map_error(e.error))?;
    let worker = subject_authorization::source_from_context(&context)
        .map_err(|e| policy::map_error(e.error))?;
    let execution = if preparing
        && api
            .store
            .has_active_worker_create_reservation(api.workspace_id(), &worker)
            .map_err(policy::map_error)?
    {
        api.plugin_reserved_identity(&worker, None, None)
            .map_err(policy::map_error)?
    } else {
        api.plugin_execution_identity(&worker)
            .await
            .map_err(policy::map_error)?
    };
    let executor = api.config_store.db_executor();
    executor
        .run(move || {
            subject_authorization::authorize_scope(
                &api,
                &worker,
                &server_api::authorization::OperationAuthorizationRequest {
                    operation: operation_name(&operation).into(),
                    resource: server_api::authorization::PermissionResource::PluginInstallation {
                        workspace_id: workspace.clone(),
                        installation_id: installation.clone(),
                    },
                },
            )
            .map_err(|e| e.error)?;
            api.config_store.with_immediate_transaction(|tx| {
                policy::authorize(
                    tx,
                    &workspace,
                    &execution,
                    &plan_id,
                    &installation,
                    &tools,
                    &operation,
                )
            })
        })
        .await
        .map_err(policy::map_error)
}

async fn invocation(
    api: Arc<WorkspaceApi>,
    context: &ServerRequestContext,
    workspace: &str,
    invocation_id: &str,
    operation: PluginHostOperation,
) -> std::result::Result<
    (receipt::InvocationRecord, policy::AdmittedPluginOperation),
    PluginHostError,
> {
    let store = api.config_store.clone();
    let w = workspace.to_owned();
    let id = invocation_id.to_owned();
    let record = store
        .db_executor()
        .run(move || store.with_conn(|conn| receipt::lookup(conn, &w, &id)))
        .await
        .map_err(policy::map_error)?;
    let admitted = check(
        api,
        context.clone(),
        workspace.into(),
        record.plan_id.clone(),
        record.installation_id.clone(),
        record.consumer.clone(),
        operation,
    )
    .await?;
    if record.execution != *admitted.plan.plan.execution() {
        return Err(denied());
    }
    if record.cancelled {
        return Err(PluginHostError {
            code: PluginHostErrorCode::Cancelled,
            effect: PluginEffectObservation::Indeterminate,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        });
    }
    if record.expired || record.expires_at_ms <= now_ms() {
        return Err(PluginHostError {
            code: PluginHostErrorCode::ReceiptExpired,
            effect: PluginEffectObservation::Indeterminate,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        });
    }
    Ok((record, admitted))
}

/// An Embedded transport retains this context only after the ordinary Runtime
/// source proof verifier has established it. Merely holding WorkspaceApi or human
/// management access does not produce a Host adapter.
#[derive(Clone)]
pub struct EmbeddedPluginHostOwner {
    api: Arc<WorkspaceApi>,
    source: EmbeddedPluginSource,
    workspace: String,
    transport_override: Option<Arc<https::HttpsTransport>>,
}

#[derive(Clone)]
enum EmbeddedPluginSource {
    Signed(Arc<dyn server_api::client_support::RequestAuthorizer>),
    #[cfg(test)]
    Verified(ServerRequestContext),
}

impl EmbeddedPluginHostOwner {
    /// Uses the Runtime's existing signer. Every request is verified by the same
    /// proof owner as Remote; this factory neither creates Worker claims nor
    /// borrows human credentials. No HTTP loopback or standalone fallback.
    pub(crate) fn authenticated(
        api: Arc<WorkspaceApi>,
        workspace: String,
        authorizer: Arc<dyn server_api::client_support::RequestAuthorizer>,
    ) -> Self {
        Self {
            api,
            workspace,
            source: EmbeddedPluginSource::Signed(authorizer),
            transport_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        api: Arc<WorkspaceApi>,
        context: ServerRequestContext,
        workspace: String,
    ) -> Self {
        Self {
            api,
            source: EmbeddedPluginSource::Verified(context),
            workspace,
            transport_override: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn with_transport(
        mut self,
        transport: impl Into<Arc<https::HttpsTransport>>,
    ) -> Self {
        self.transport_override = Some(transport.into());
        self
    }
}
#[async_trait]
impl yoi_plugin_runtime::owner::PluginHostOwner for EmbeddedPluginHostOwner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> std::result::Result<PluginHostResponse, PluginHostError> {
        let context = match &self.source {
            #[cfg(test)]
            EmbeddedPluginSource::Verified(context) => context.clone(),
            EmbeddedPluginSource::Signed(authorizer) => {
                let body = serde_json::to_vec(&request).map_err(|_| denied())?;
                let mut url =
                    url::Url::parse("https://plugin-owner.invalid/").map_err(|_| denied())?;
                url.path_segments_mut().map_err(|_| denied())?.extend([
                    "api",
                    "w",
                    &self.workspace,
                    "plugin-host",
                ]);
                let path = url.path();
                let headers = authorizer
                    .authorize(server_api::client_support::AuthorizerRequest {
                        method: &reqwest::Method::POST,
                        path_and_query: path,
                        body: &body,
                    })
                    .map_err(|_| denied())?;
                let proof = headers
                    .get(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(denied)?;
                let source = crate::worker_source::verify_runtime_request_source_proof(
                    &self.api,
                    proof,
                    &self.workspace,
                    worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                    "POST",
                    path,
                    &worker_runtime::auth::request_body_digest(&body),
                )
                .await
                .map_err(|_| denied())?;
                server_request_context(
                    None,
                    None,
                    Some(ServerRuntimeSource {
                        authentication_binding_id: source.authentication_binding_id,
                        runtime_id: source.runtime_id,
                        worker_id: source.worker_id,
                    }),
                    None,
                    &headers,
                )
            }
        };
        execute_with_transport(
            self.api.clone(),
            context,
            self.workspace.clone(),
            request,
            self.transport_override.clone(),
        )
        .await
    }
}

pub(super) async fn execute(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    request: PluginHostRequest,
) -> std::result::Result<PluginHostResponse, PluginHostError> {
    #[cfg(test)]
    let transport_override = api.plugin_https_transport.clone();
    #[cfg(not(test))]
    let transport_override = None;
    execute_with_transport(api, context, workspace, request, transport_override).await
}

async fn execute_with_transport(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    request: PluginHostRequest,
    transport_override: Option<Arc<https::HttpsTransport>>,
) -> std::result::Result<PluginHostResponse, PluginHostError> {
    // Human management and a Runtime-only (no Worker) source both fail closed.
    subject_authorization::source_from_context(&context).map_err(|_| denied())?;
    match request {
        PluginHostRequest::Prepare {
            plan_id,
            installation_id,
            tools,
        } => {
            check_tools(
                api,
                context,
                workspace,
                plan_id,
                installation_id,
                tools,
                PluginHostOperation::Admit,
                true,
            )
            .await?;
            Ok(PluginHostResponse::Authorized)
        }
        PluginHostRequest::Begin {
            plan_id,
            installation_id,
            consumer,
        } => {
            let admitted = check(
                api.clone(),
                context,
                workspace,
                plan_id,
                installation_id.clone(),
                consumer.clone(),
                PluginHostOperation::Admit,
            )
            .await?;
            let store = api.config_store.clone();
            let record = store
                .db_executor()
                .run(move || {
                    store.with_immediate_transaction(|tx| {
                        receipt::begin(
                            tx,
                            &admitted.plan,
                            &installation_id,
                            &consumer,
                            now_ms(),
                            RECEIPT_RETENTION_MS,
                        )
                    })
                })
                .await
                .map_err(policy::map_error)?;
            if record.cancelled || record.expired || record.expires_at_ms <= now_ms() {
                return Err(PluginHostError {
                    code: if record.cancelled {
                        PluginHostErrorCode::Cancelled
                    } else {
                        PluginHostErrorCode::ReceiptExpired
                    },
                    effect: PluginEffectObservation::Indeterminate,
                    diagnostic_id: uuid::Uuid::now_v7().to_string(),
                });
            }
            Ok(PluginHostResponse::Invocation {
                invocation_id: record.invocation_id,
                expires_at_ms: record.expires_at_ms,
            })
        }
        PluginHostRequest::Cancel { invocation_id } => {
            // Do not use invocation(): exact cancellation replay must work after
            // the irreversible bit has already been committed.
            let store = api.config_store.clone();
            let w = workspace.clone();
            let id = invocation_id.clone();
            let record = store
                .db_executor()
                .run(move || store.with_conn(|conn| receipt::lookup(conn, &w, &id)))
                .await
                .map_err(policy::map_error)?;
            check(
                api.clone(),
                context,
                workspace.clone(),
                record.plan_id,
                record.installation_id,
                record.consumer,
                PluginHostOperation::Admit,
            )
            .await?;
            let store = api.config_store.clone();
            store
                .db_executor()
                .run(move || {
                    store.with_immediate_transaction(|tx| {
                        receipt::cancel(tx, &workspace, &invocation_id)
                    })
                })
                .await
                .map_err(policy::map_error)?;
            Ok(PluginHostResponse::Authorized)
        }
        PluginHostRequest::Authorize {
            invocation_id,
            operation,
        } => {
            invocation(api, &context, &workspace, &invocation_id, operation).await?;
            Ok(PluginHostResponse::Authorized)
        }
        PluginHostRequest::Open {
            invocation_id,
            slot,
        } => {
            invocation(
                api,
                &context,
                &workspace,
                &invocation_id,
                PluginHostOperation::Open { slot },
            )
            .await?;
            Ok(PluginHostResponse::Opened)
        }
        PluginHostRequest::Log {
            invocation_id,
            entry,
        } => {
            invocation(
                api,
                &context,
                &workspace,
                &invocation_id,
                PluginHostOperation::Log,
            )
            .await?;
            // Guest text/fields are untrusted and may contain echoed credentials.
            // Standard diagnostics retain only this safe event, not their payload.
            if entry.message.len() > 4096
                || entry.event_code.is_empty()
                || entry.event_code.len() > 128
                || !entry
                    .event_code
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                || entry.fields.len() > 32 || entry.fields.iter().any(|field| {
                field.name.len() > 128
                    || field.name.is_empty()
                    || field.name.chars().any(char::is_control)
                    || matches!(&field.value, PluginLogScalar::Text(value) if value.len() > 1024)
                    || matches!(&field.value, PluginLogScalar::Number(value) if !value.is_finite())
            }) {
                return Err(policy::map_error(policy::failure(
                    PluginHostErrorCode::QuotaExceeded,
                )));
            }
            tracing::debug!(invocation = %invocation_id, "plugin log event admitted");
            Ok(PluginHostResponse::Logged)
        }
        PluginHostRequest::Send {
            invocation_id,
            slot,
            request,
        } => {
            let operation = PluginHostOperation::Send { slot: slot.clone() };
            let (record, admitted) = invocation(
                api.clone(),
                &context,
                &workspace,
                &invocation_id,
                operation.clone(),
            )
            .await?;
            let binding = admitted.binding.as_ref().ok_or_else(denied)?;
            let prepared = https::prepare(
                binding,
                https::HttpsRequest {
                    method: request.method.clone(),
                    path: request.path.clone(),
                    query: request.query.clone(),
                    headers: request.headers.clone(),
                    body: request.body.clone(),
                    timeout_ms: request.timeout_ms,
                    effect_key: request.effect_key.clone(),
                },
            )
            .map_err(transport_error)?;
            let normalized = prepared.normalized();
            let normalized_dto = PluginHttpRequest {
                method: normalized.method.clone(),
                path: normalized.path.clone(),
                query: normalized.query.clone(),
                headers: normalized.headers.clone(),
                body: normalized.body.clone(),
                timeout_ms: normalized.timeout_ms,
                effect_key: normalized.effect_key.clone(),
            };
            let fingerprint = policy::request_fingerprint(&admitted, &slot, &normalized_dto)
                .map_err(policy::map_error)?;
            let store = api.config_store.clone();
            let r = record.clone();
            let fp = fingerprint.clone();
            let key = request.effect_key.clone();
            let admission = store
                .db_executor()
                .run(move || {
                    store.with_immediate_transaction(|tx| {
                        receipt::effect_admit(tx, &r, &key, &fp, now_ms())
                    })
                })
                .await
                .map_err(policy::map_error)?;
            let result = match admission {
                receipt::EffectAdmission::Replay(result) => result,
                receipt::EffectAdmission::Dispatch => {
                    // A second current check fences cancellation/revocation that
                    // happened while waiting for durable admission. Any lost
                    // acknowledgement from this point remains indeterminate.
                    invocation(
                        api.clone(),
                        &context,
                        &workspace,
                        &invocation_id,
                        operation.clone(),
                    )
                    .await?;
                    let secret = if binding.secret_ref.is_empty() {
                        None
                    } else {
                        let root = ServerConfig::workspace_backend_data_root_for_data_dir(
                            &api.config.data_root,
                            &workspace,
                        );
                        Some(zeroize::Zeroizing::new(
                            secrets::SecretStore::new(root)
                                .get(&format!("plugin/{}", binding.secret_ref))
                                .map_err(|_| denied())?
                                .into_string(),
                        ))
                    };
                    let transport = match transport_override {
                        Some(transport) => transport,
                        None => Arc::new(transport(&api).await?),
                    };
                    let cancellation = CancellationToken::new();
                    let sending = transport.execute_with_guard(
                        binding,
                        prepared,
                        secret.as_deref().map(|v| v.as_str()),
                        &cancellation,
                        || async {
                            invocation(
                                api.clone(),
                                &context,
                                &workspace,
                                &invocation_id,
                                operation.clone(),
                            )
                            .await
                            .map(|_| ())
                            .map_err(|_| https::TransportError::Denied)
                        },
                    );
                    // Monitor the existing durable owners rather than maintaining
                    // another revocation registry. Dropping the transport future
                    // shuts down its single-use client/socket. A stalled authority
                    // owner also closes the attempt; it is never permission to run
                    // until the HTTP deadline. Admission remains consumed even if
                    // the shutdown acknowledgement or completion write is lost.
                    let result = tokio::select! {
                        result = sending => result.map(|response| PluginHttpResponse {
                            status: response.status,
                            headers: response.headers,
                            body: response.body,
                        }).map_err(sent_transport_error),
                        error = monitor_invocation(api.clone(), &context, &workspace, &invocation_id, operation.clone()) => {
                            cancellation.cancel();
                            Err(error)
                        }
                    };
                    let store = api.config_store.clone();
                    let r = record;
                    let fp = fingerprint;
                    let key = request.effect_key;
                    let saved = result.clone();
                    store
                        .db_executor()
                        .run(move || {
                            store.with_immediate_transaction(|tx| {
                                receipt::effect_complete(tx, &r, &key, &fp, &saved)
                            })
                        })
                        .await
                        .map_err(|_| PluginHostError {
                            code: PluginHostErrorCode::Indeterminate,
                            effect: PluginEffectObservation::Indeterminate,
                            diagnostic_id: uuid::Uuid::now_v7().to_string(),
                        })?;
                    result
                }
            };
            // Both saved response replay and new network results require current
            // read/publication authority. No denied payload reaches the Runtime.
            let observation = result
                .as_ref()
                .map(|_| PluginEffectObservation::ResponseObserved)
                .unwrap_or_else(|error| error.effect);
            invocation(api.clone(), &context, &workspace, &invocation_id, operation)
                .await
                .map_err(|mut error| {
                    error.effect = observation;
                    error
                })?;
            invocation(
                api,
                &context,
                &workspace,
                &invocation_id,
                PluginHostOperation::Publish,
            )
            .await
            .map_err(|mut error| {
                error.effect = observation;
                error
            })?;
            result.map(|response| PluginHostResponse::Response { response })
        }
    }
}

async fn monitor_invocation(
    api: Arc<WorkspaceApi>,
    context: &ServerRequestContext,
    workspace: &str,
    invocation_id: &str,
    operation: PluginHostOperation,
) -> PluginHostError {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let checked = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            invocation(
                api.clone(),
                context,
                workspace,
                invocation_id,
                operation.clone(),
            ),
        )
        .await;
        let error = match checked {
            Ok(Ok(_)) => continue,
            Ok(Err(error)) => error,
            Err(_) => policy::map_error(policy::failure(PluginHostErrorCode::Unavailable)),
        };
        // After admission the request may have reached the peer. Neither a
        // failed check nor a local connection shutdown proves remote rollback.
        return PluginHostError {
            effect: PluginEffectObservation::Indeterminate,
            ..error
        };
    }
}

async fn transport(
    api: &WorkspaceApi,
) -> std::result::Result<https::HttpsTransport, PluginHostError> {
    let policy = transport_policy(api, |host, port| async move {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.take(65).collect::<Vec<_>>())
            .map_err(|_| https::TransportError::Dns)
    })
    .await?;
    Ok(https::HttpsTransport::new(policy))
}

pub(super) async fn transport_policy<F, Fut>(
    api: &WorkspaceApi,
    resolve: F,
) -> std::result::Result<https::TransportPolicy, PluginHostError>
where
    F: Fn(String, u16) -> Fut,
    Fut: std::future::Future<
            Output = std::result::Result<Vec<std::net::SocketAddr>, https::TransportError>,
        >,
{
    let mut hosts = std::collections::BTreeSet::new();
    let mut ips = std::collections::BTreeSet::new();
    // Current server-owned bindings include dynamically registered endpoints in
    // other Workspaces, even when they were absent from the startup config.
    let store = api.config_store.clone();
    let registered = store
        .db_executor()
        .run(move || {
            store.with_conn(|conn| {
                let mut statement =
                    conn.prepare("SELECT DISTINCT base_url FROM workspace_runtime_bindings")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            })
        })
        .await
        .map_err(policy::map_error)?;
    for address in api
        .config
        .plugin_control_plane_endpoints()
        .iter()
        .chain(registered.iter())
    {
        if let Ok(url) = url::Url::parse(address) {
            if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
                continue;
            }
            if let Some(host) = url.host_str() {
                let host = host.trim_start_matches('[').trim_end_matches(']');
                hosts.insert(host.to_ascii_lowercase());
                if let Ok(ip) = host.parse() {
                    ips.insert(ip);
                } else {
                    let port = url.port_or_known_default().ok_or_else(denied)?;
                    let resolved = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        resolve(host.to_owned(), port),
                    )
                    .await
                    .map_err(|_| transport_error(https::TransportError::Dns))?
                    .map_err(|_| transport_error(https::TransportError::Dns))?;
                    if resolved.is_empty() {
                        return Err(transport_error(https::TransportError::Dns));
                    }
                    for (index, address) in resolved.into_iter().enumerate() {
                        if index >= 64 {
                            return Err(transport_error(https::TransportError::Dns));
                        }
                        ips.insert(address.ip());
                    }
                }
            }
        }
    }
    Ok(https::TransportPolicy {
        denied_addresses: ips,
        denied_hosts: hosts,
    })
}
fn sent_transport_error(error: https::TransportError) -> PluginHostError {
    match error {
        https::TransportError::ResponseLimit => PluginHostError {
            code: PluginHostErrorCode::QuotaExceeded,
            effect: PluginEffectObservation::ResponseObserved,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        },
        https::TransportError::UnsupportedEncoding => PluginHostError {
            code: PluginHostErrorCode::Unsupported,
            effect: PluginEffectObservation::ResponseObserved,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        },
        https::TransportError::HeaderLimit => PluginHostError {
            code: PluginHostErrorCode::QuotaExceeded,
            effect: PluginEffectObservation::Indeterminate,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        },
        https::TransportError::Unavailable
        | https::TransportError::DeadlineExceeded
        | https::TransportError::Cancelled => PluginHostError {
            code: PluginHostErrorCode::Indeterminate,
            effect: PluginEffectObservation::Indeterminate,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        },
        other => transport_error(other),
    }
}
fn transport_error(error: https::TransportError) -> PluginHostError {
    let code = match error {
        https::TransportError::Denied | https::TransportError::InvalidBinding => {
            PluginHostErrorCode::Denied
        }
        https::TransportError::InvalidRequest => PluginHostErrorCode::InvalidInput,
        https::TransportError::HeaderLimit
        | https::TransportError::RequestLimit
        | https::TransportError::ResponseLimit => PluginHostErrorCode::QuotaExceeded,
        https::TransportError::DeadlineExceeded => PluginHostErrorCode::DeadlineExceeded,
        https::TransportError::Cancelled => PluginHostErrorCode::Cancelled,
        _ => PluginHostErrorCode::Unavailable,
    };
    PluginHostError {
        code,
        effect: PluginEffectObservation::NotDispatched,
        diagnostic_id: uuid::Uuid::now_v7().to_string(),
    }
}
