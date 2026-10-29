//! Shared owner transport and WIT Host adapter. In-process owners implement the
//! same trait directly; a failed remote call never selects a local fallback.
use crate::host::{Host, InvocationContext, Operation, errors, http, logging};
use async_trait::async_trait;
use server_api::{
    PluginConsumer, PluginEffectObservation, PluginHostError, PluginHostErrorCode,
    PluginHostOperation, PluginHostRequest, PluginHostResponse, PluginHttpMethod,
    PluginHttpRequest, PluginLogEntry, PluginLogField, PluginLogLevel, PluginLogScalar,
};
use std::{
    any::Any,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

/// Bound to one authenticated caller and Workspace by the integration. The owner
/// resolves all identity/policy from that binding and live-checks every operation,
/// including Open/Send/Log. Implementations must not detach effects or auto-retry.
#[async_trait]
pub trait PluginHostOwner: Send + Sync {
    async fn call(&self, request: PluginHostRequest)
    -> Result<PluginHostResponse, PluginHostError>;
}

/// Uses an injected, already configured generated client (including its existing
/// Runtime request authorizer). This adapter never creates or owns credentials.
pub struct RemotePluginHostOwner<A> {
    client: Arc<server_api::ServerApiClient<A>>,
    workspace_id: String,
}
impl<A> RemotePluginHostOwner<A> {
    pub fn new(client: Arc<server_api::ServerApiClient<A>>, workspace_id: String) -> Self {
        Self {
            client,
            workspace_id,
        }
    }
}
#[async_trait]
impl<A: server_api::client_support::RequestAuthorizer> PluginHostOwner
    for RemotePluginHostOwner<A>
{
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        let may_dispatch = matches!(request, PluginHostRequest::Send { .. });
        self.client
            .plugin_host(self.workspace_id.clone(), request)
            .await
            .map_err(|error| {
                match error {
                    server_api::client_support::ClientError::Public { error, .. } => error,
                    // A missing response is not proof that a Send was not dispatched.
                    // Preserve uncertainty; never synthesize a success or retry it.
                    server_api::client_support::ClientError::Failure(_) => local_error(
                        if may_dispatch {
                            PluginHostErrorCode::Indeterminate
                        } else {
                            PluginHostErrorCode::Unavailable
                        },
                        if may_dispatch {
                            PluginEffectObservation::Indeterminate
                        } else {
                            PluginEffectObservation::NotDispatched
                        },
                    ),
                }
            })
    }
}

/// Exactly one owner-issued invocation, with a local snapshot upper bound.
/// Obtain the execution context through `context()` after Begin; the caller's
/// pre-Begin invocation ID is never sent to, or adopted as authority by, the owner.
pub struct OwnerHost {
    owner: Arc<dyn PluginHostOwner>,
    context: InvocationContext,
    binding_identity: Arc<()>,
}
struct OwnerConnection {
    invocation_id: String,
    slot: String,
    identity: Arc<()>,
}
impl OwnerHost {
    pub async fn begin(
        owner: Arc<dyn PluginHostOwner>,
        mut context: InvocationContext,
        consumer: PluginConsumer,
    ) -> Result<Self, PluginHostError> {
        if let PluginConsumer::ToolCall { tool_name, .. } = &consumer {
            if *tool_name != context.tool_name {
                return Err(local_error(
                    PluginHostErrorCode::Denied,
                    PluginEffectObservation::NotDispatched,
                ));
            }
        }
        check_lifetime(&context)?;
        let response = call_owner(
            owner.as_ref(),
            &context,
            PluginHostRequest::Begin {
                plan_id: context.plan.clone(),
                installation_id: context.installation.clone(),
                consumer,
            },
        )
        .await?;
        let PluginHostResponse::Invocation {
            invocation_id,
            expires_at_ms,
        } = response
        else {
            return Err(protocol_error(false));
        };
        if invocation_id.is_empty() || invocation_id.len() > 1024 {
            return Err(protocol_error(false));
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .ok_or_else(|| protocol_error(false))?;
        let remaining = expires_at_ms
            .checked_sub(now_ms)
            .filter(|ms| *ms > 0)
            .ok_or_else(|| {
                local_error(
                    PluginHostErrorCode::DeadlineExceeded,
                    PluginEffectObservation::NotDispatched,
                )
            })?;
        let expires = Instant::now()
            .checked_add(Duration::from_millis(remaining as u64))
            .ok_or_else(|| protocol_error(false))?;
        context.deadline = context.deadline.min(expires);
        context.invocation_id = invocation_id;
        check_lifetime(&context)?;
        Ok(Self {
            owner,
            context,
            binding_identity: Arc::new(()),
        })
    }

    pub fn context(&self) -> &InvocationContext {
        &self.context
    }

    /// Persist cancellation at the owner as well as stopping local guest waits.
    /// This never claims to undo an already admitted external effect. The caller
    /// must reconcile an unavailable response, not restore under a new identity.
    pub async fn cancel(&self) -> Result<(), PluginHostError> {
        self.context.cancellation.cancel();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.owner.call(PluginHostRequest::Cancel {
                invocation_id: self.context.invocation_id.clone(),
            }),
        )
        .await
        .map_err(|_| {
            local_error(
                PluginHostErrorCode::Unavailable,
                PluginEffectObservation::Indeterminate,
            )
        })??;
        if response != PluginHostResponse::Authorized {
            return Err(protocol_error(false));
        }
        Ok(())
    }

    fn check_context(&self, context: &InvocationContext) -> Result<(), PluginHostError> {
        let bound = &self.context;
        if context.workspace != bound.workspace
            || context.installation != bound.installation
            || context.plan != bound.plan
            || context.subject != bound.subject
            || context.invocation_id != bound.invocation_id
            || context.artifact != bound.artifact
            || context.tool_name != bound.tool_name
            || context.grants.logging != bound.grants.logging
            || context.grants.connection_slots != bound.grants.connection_slots
            // The executor may shorten the deadline and derive a child token.
            || context.deadline > bound.deadline
        {
            return Err(local_error(
                PluginHostErrorCode::Denied,
                PluginEffectObservation::NotDispatched,
            ));
        }
        check_lifetime(bound)?;
        check_lifetime(context)
    }
    fn connection<'a>(
        &self,
        binding: &'a (dyn Any + Send + Sync),
    ) -> Result<&'a OwnerConnection, PluginHostError> {
        binding
            .downcast_ref::<OwnerConnection>()
            .filter(|b| {
                b.invocation_id == self.context.invocation_id
                    && Arc::ptr_eq(&b.identity, &self.binding_identity)
            })
            .ok_or_else(|| {
                local_error(
                    PluginHostErrorCode::Denied,
                    PluginEffectObservation::NotDispatched,
                )
            })
    }
    fn check_slot(&self, slot: &str) -> Result<(), PluginHostError> {
        if !self.context.grants.connection_slots.contains(slot) {
            return Err(local_error(
                PluginHostErrorCode::Denied,
                PluginEffectObservation::NotDispatched,
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl Host for OwnerHost {
    async fn authorize(
        &self,
        context: &InvocationContext,
        operation: Operation<'_>,
    ) -> Result<(), errors::HostError> {
        self.check_context(context).map_err(wit_error)?;
        let operation = match operation {
            Operation::Admit => PluginHostOperation::Admit,
            Operation::Publish => PluginHostOperation::Publish,
            Operation::Open { slot } => {
                self.check_slot(slot).map_err(wit_error)?;
                PluginHostOperation::Open { slot: slot.into() }
            }
            Operation::Send { slot, binding, .. } => {
                let connection = self.connection(binding).map_err(wit_error)?;
                self.check_slot(slot).map_err(wit_error)?;
                if connection.slot != slot {
                    return Err(wit_error(local_error(
                        PluginHostErrorCode::Denied,
                        PluginEffectObservation::NotDispatched,
                    )));
                }
                PluginHostOperation::Send { slot: slot.into() }
            }
            Operation::Log => {
                if !context.grants.logging {
                    return Err(wit_error(local_error(
                        PluginHostErrorCode::Denied,
                        PluginEffectObservation::NotDispatched,
                    )));
                }
                PluginHostOperation::Log
            }
        };
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::Authorize {
                invocation_id: self.context.invocation_id.clone(),
                operation,
            },
        )
        .await
        .map_err(wit_error)?
        {
            PluginHostResponse::Authorized => Ok(()),
            _ => Err(wit_error(protocol_error(false))),
        }
    }
    async fn open(
        &self,
        context: &InvocationContext,
        slot: &str,
    ) -> Result<Arc<dyn Any + Send + Sync>, errors::HostError> {
        self.check_context(context).map_err(wit_error)?;
        self.check_slot(slot).map_err(wit_error)?;
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::Open {
                invocation_id: self.context.invocation_id.clone(),
                slot: slot.into(),
            },
        )
        .await
        .map_err(wit_error)?
        {
            PluginHostResponse::Opened => Ok(Arc::new(OwnerConnection {
                invocation_id: self.context.invocation_id.clone(),
                slot: slot.into(),
                identity: self.binding_identity.clone(),
            })),
            _ => Err(wit_error(protocol_error(false))),
        }
    }
    async fn send(
        &self,
        context: &InvocationContext,
        binding: &(dyn Any + Send + Sync),
        request: http::Request,
    ) -> Result<http::Response, errors::HostError> {
        self.check_context(context).map_err(wit_error)?;
        let connection = self.connection(binding).map_err(wit_error)?;
        self.check_slot(&connection.slot).map_err(wit_error)?;
        let request = PluginHttpRequest {
            method: match request.method {
                http::Method::Get => PluginHttpMethod::Get,
                http::Method::Head => PluginHttpMethod::Head,
                http::Method::Post => PluginHttpMethod::Post,
                http::Method::Put => PluginHttpMethod::Put,
                http::Method::Patch => PluginHttpMethod::Patch,
                http::Method::Delete => PluginHttpMethod::Delete,
                http::Method::Options => PluginHttpMethod::Options,
            },
            path: request.path,
            query: pairs(request.query),
            headers: pairs(request.headers),
            body: request.body,
            timeout_ms: request.timeout_ms,
            effect_key: request.effect_key,
        };
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::Send {
                invocation_id: self.context.invocation_id.clone(),
                slot: connection.slot.clone(),
                request,
            },
        )
        .await
        .map_err(wit_error)?
        {
            PluginHostResponse::Response { response } => Ok(http::Response {
                status: response.status,
                headers: response
                    .headers
                    .into_iter()
                    .map(|(name, value)| http::Pair { name, value })
                    .collect(),
                body: response.body,
            }),
            _ => Err(wit_error(protocol_error(true))),
        }
    }
    async fn log(
        &self,
        context: &InvocationContext,
        entry: logging::Entry,
    ) -> Result<(), errors::HostError> {
        self.check_context(context).map_err(wit_error)?;
        if !context.grants.logging {
            return Err(wit_error(local_error(
                PluginHostErrorCode::Denied,
                PluginEffectObservation::NotDispatched,
            )));
        }
        let entry = PluginLogEntry {
            level: match entry.level {
                logging::Level::Trace => PluginLogLevel::Trace,
                logging::Level::Debug => PluginLogLevel::Debug,
                logging::Level::Info => PluginLogLevel::Info,
                logging::Level::Warn => PluginLogLevel::Warn,
                logging::Level::Error => PluginLogLevel::Error,
            },
            event_code: entry.event_code,
            message: entry.message,
            fields: entry
                .fields
                .into_iter()
                .map(|f| {
                    Ok(PluginLogField {
                        name: f.name,
                        value: match f.value {
                            logging::Scalar::Text(v) => PluginLogScalar::Text(v),
                            logging::Scalar::Boolean(v) => PluginLogScalar::Boolean(v),
                            logging::Scalar::Signed(v) => PluginLogScalar::Signed(v),
                            logging::Scalar::Unsigned(v) => PluginLogScalar::Unsigned(v),
                            logging::Scalar::Number(v) if v.is_finite() => {
                                PluginLogScalar::Number(v)
                            }
                            logging::Scalar::Number(_) => {
                                return Err(wit_error(local_error(
                                    PluginHostErrorCode::InvalidInput,
                                    PluginEffectObservation::NotDispatched,
                                )));
                            }
                        },
                    })
                })
                .collect::<Result<_, errors::HostError>>()?,
        };
        match call_owner(
            self.owner.as_ref(),
            context,
            PluginHostRequest::Log {
                invocation_id: self.context.invocation_id.clone(),
                entry,
            },
        )
        .await
        .map_err(wit_error)?
        {
            PluginHostResponse::Logged => Ok(()),
            _ => Err(wit_error(protocol_error(false))),
        }
    }
}
async fn call_owner(
    owner: &dyn PluginHostOwner,
    context: &InvocationContext,
    request: PluginHostRequest,
) -> Result<PluginHostResponse, PluginHostError> {
    check_lifetime(context)?;
    let effect = if matches!(request, PluginHostRequest::Send { .. }) {
        PluginEffectObservation::Indeterminate
    } else {
        PluginEffectObservation::NotDispatched
    };
    tokio::select! {
        biased;
        _ = context.cancellation.cancelled() => Err(local_error(PluginHostErrorCode::Cancelled, effect)),
        _ = tokio::time::sleep_until(context.deadline) => Err(local_error(PluginHostErrorCode::DeadlineExceeded, effect)),
        response = owner.call(request) => response,
    }
}

fn pairs(pairs: Vec<http::Pair>) -> Vec<(String, String)> {
    pairs.into_iter().map(|p| (p.name, p.value)).collect()
}
fn local_error(code: PluginHostErrorCode, effect: PluginEffectObservation) -> PluginHostError {
    PluginHostError {
        code,
        effect,
        diagnostic_id: String::new(),
    }
}
fn protocol_error(may_dispatch: bool) -> PluginHostError {
    local_error(
        if may_dispatch {
            PluginHostErrorCode::Indeterminate
        } else {
            PluginHostErrorCode::Unavailable
        },
        if may_dispatch {
            PluginEffectObservation::Indeterminate
        } else {
            PluginEffectObservation::NotDispatched
        },
    )
}
fn check_lifetime(context: &InvocationContext) -> Result<(), PluginHostError> {
    if context.cancellation.is_cancelled() {
        return Err(local_error(
            PluginHostErrorCode::Cancelled,
            PluginEffectObservation::NotDispatched,
        ));
    }
    if Instant::now() >= context.deadline {
        return Err(local_error(
            PluginHostErrorCode::DeadlineExceeded,
            PluginEffectObservation::NotDispatched,
        ));
    }
    Ok(())
}
fn wit_error(error: PluginHostError) -> errors::HostError {
    let message = error.message().to_owned();
    errors::HostError {
        code: match error.code {
            PluginHostErrorCode::InvalidInput => errors::Code::InvalidInput,
            PluginHostErrorCode::Denied => errors::Code::Denied,
            PluginHostErrorCode::NotFound => errors::Code::NotFound,
            PluginHostErrorCode::Conflict => errors::Code::Conflict,
            PluginHostErrorCode::Unsupported => errors::Code::Unsupported,
            PluginHostErrorCode::QuotaExceeded => errors::Code::QuotaExceeded,
            PluginHostErrorCode::DeadlineExceeded => errors::Code::DeadlineExceeded,
            PluginHostErrorCode::Cancelled => errors::Code::Cancelled,
            PluginHostErrorCode::Unavailable => errors::Code::Unavailable,
            PluginHostErrorCode::Indeterminate => errors::Code::Indeterminate,
            PluginHostErrorCode::ReceiptExpired => errors::Code::ReceiptExpired,
        },
        effect: match error.effect {
            PluginEffectObservation::NotDispatched => errors::EffectObservation::NotDispatched,
            PluginEffectObservation::ResponseObserved => {
                errors::EffectObservation::ResponseObserved
            }
            PluginEffectObservation::Indeterminate => errors::EffectObservation::Indeterminate,
        },
        message,
        diagnostic_id: error.diagnostic_id,
    }
}

#[cfg(test)]
mod tests {
    // Adapter contract tests: the real generated HTTP client and OwnerHost run
    // against one deterministic owner, directly or through an isolated loopback
    // endpoint. Server authentication, HTTPS policy and durable receipts are not
    // substituted claims of this suite; their concrete owner tests live in Server.
    use super::*;
    use crate::{
        ArtifactIdentity,
        host::{CancellationToken, Grants},
    };
    use server_api::{PluginHttpResponse, client_support, server_support};
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Default)]
    struct TestOwner {
        requests: Mutex<Vec<PluginHostRequest>>,
        revoked: AtomicBool,
    }
    #[async_trait]
    impl PluginHostOwner for TestOwner {
        async fn call(
            &self,
            request: PluginHostRequest,
        ) -> Result<PluginHostResponse, PluginHostError> {
            self.requests.lock().unwrap().push(request.clone());
            if self.revoked.load(Ordering::SeqCst) {
                return Err(PluginHostError {
                    code: PluginHostErrorCode::Denied,
                    effect: PluginEffectObservation::NotDispatched,
                    diagnostic_id: "revoked-diagnostic".into(),
                });
            }
            Ok(match request {
                PluginHostRequest::Begin { .. } => PluginHostResponse::Invocation {
                    invocation_id: "owner-issued".into(),
                    expires_at_ms: i64::try_from(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis(),
                    )
                    .unwrap()
                        + 60_000,
                },
                PluginHostRequest::Cancel { .. } | PluginHostRequest::Authorize { .. } => {
                    PluginHostResponse::Authorized
                }
                PluginHostRequest::Open { .. } => PluginHostResponse::Opened,
                PluginHostRequest::Send { request, .. } => PluginHostResponse::Response {
                    response: PluginHttpResponse {
                        status: 207,
                        headers: request.headers,
                        body: request.body,
                    },
                },
                PluginHostRequest::Log { .. } => PluginHostResponse::Logged,
            })
        }
    }
    fn context() -> InvocationContext {
        InvocationContext {
            workspace: "ws".into(),
            installation: "install".into(),
            plan: "plan".into(),
            subject: "subject".into(),
            invocation_id: "not-authority".into(),
            artifact: ArtifactIdentity {
                package_digest: "package".into(),
                artifact_digest: "artifact".into(),
                artifact_size: 123,
            },
            tool_name: "tool".into(),
            grants: Grants {
                logging: true,
                connection_slots: ["api".into(), "other".into()].into(),
            },
            deadline: Instant::now() + Duration::from_secs(60),
            cancellation: CancellationToken::new(),
        }
    }
    fn consumer() -> PluginConsumer {
        PluginConsumer::ToolCall {
            tool_call_id: "call".into(),
            tool_name: "tool".into(),
        }
    }
    fn request() -> http::Request {
        http::Request {
            method: http::Method::Post,
            path: "/v1/items".into(),
            query: vec![
                http::Pair {
                    name: "q".into(),
                    value: "one".into(),
                },
                http::Pair {
                    name: "q".into(),
                    value: "two".into(),
                },
            ],
            headers: vec![http::Pair {
                name: "x-test".into(),
                value: "value".into(),
            }],
            body: vec![0, 255, 42],
            timeout_ms: 1234,
            effect_key: "effect".into(),
        }
    }

    struct TestAuthorizer;
    impl client_support::RequestAuthorizer for TestAuthorizer {
        fn authorize(
            &self,
            _: client_support::AuthorizerRequest<'_>,
        ) -> Result<client_support::framework::header::HeaderMap, client_support::AuthorizationError>
        {
            let mut headers = client_support::framework::header::HeaderMap::new();
            headers.insert("authorization", "Bearer isolated-test".parse().unwrap());
            Ok(headers)
        }
    }
    struct Endpoint {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Endpoint {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn endpoint(owner: Arc<TestOwner>) -> Endpoint {
        use server_support::framework::{self as axum, HeaderMap, Json, Path, Response, State};
        async fn call(
            State(owner): State<Arc<TestOwner>>,
            Path(workspace): Path<String>,
            headers: HeaderMap,
            Json(request): Json<PluginHostRequest>,
        ) -> Response {
            assert_eq!(workspace, "ws");
            assert_eq!(
                headers.get("authorization").unwrap(),
                "Bearer isolated-test"
            );
            match owner.call(request).await {
                Ok(response) => server_support::json_response(axum::StatusCode::OK, response),
                Err(error) => server_support::error_response(axum::StatusCode::FORBIDDEN, error),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route("/api/w/{workspace_id}/plugin-host", axum::post(call))
            .with_state(owner);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Endpoint { url, task }
    }
    async fn remote(owner: Arc<TestOwner>) -> (Arc<dyn PluginHostOwner>, Endpoint) {
        let endpoint = endpoint(owner).await;
        let http = client_support::framework::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let core = client_support::ClientBuilder::try_new(&endpoint.url)
            .unwrap()
            .client(http)
            .authorizer(TestAuthorizer)
            .request_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let api = Arc::new(server_api::ServerApiClient::from_core(core));
        (
            Arc::new(RemotePluginHostOwner::new(api, "ws".into())),
            endpoint,
        )
    }

    #[tokio::test]
    async fn remote_and_direct_owner_preserve_requests_results_and_live_denial() {
        let mut transcripts = Vec::new();
        for use_remote in [false, true] {
            let owner = Arc::new(TestOwner::default());
            let (transport, _endpoint): (Arc<dyn PluginHostOwner>, _) = if use_remote {
                let (transport, endpoint) = remote(owner.clone()).await;
                (transport, Some(endpoint))
            } else {
                (owner.clone(), None)
            };
            let host = OwnerHost::begin(transport, context(), consumer())
                .await
                .unwrap();
            let ctx = host.context();
            assert_eq!(ctx.invocation_id, "owner-issued");
            host.authorize(ctx, Operation::Admit).await.unwrap();
            host.authorize(ctx, Operation::Open { slot: "api" })
                .await
                .unwrap();
            let connection = host.open(ctx, "api").await.unwrap();
            host.authorize(
                ctx,
                Operation::Send {
                    slot: "api",
                    binding: connection.as_ref(),
                    request: &request(),
                },
            )
            .await
            .unwrap();
            let response = host
                .send(ctx, connection.as_ref(), request())
                .await
                .unwrap();
            assert_eq!(response.status, 207);
            assert_eq!(response.body, vec![0, 255, 42]);
            assert_eq!(response.headers[0].value, "value");
            host.authorize(ctx, Operation::Log).await.unwrap();
            host.log(
                ctx,
                logging::Entry {
                    level: logging::Level::Warn,
                    event_code: "test.event".into(),
                    message: "bounded".into(),
                    fields: vec![logging::Field {
                        name: "count".into(),
                        value: logging::Scalar::Signed(-2),
                    }],
                },
            )
            .await
            .unwrap();
            host.authorize(ctx, Operation::Publish).await.unwrap();
            owner.revoked.store(true, Ordering::SeqCst);
            for operation in [
                Operation::Admit,
                Operation::Publish,
                Operation::Open { slot: "api" },
                Operation::Log,
            ] {
                let error = host.authorize(ctx, operation).await.unwrap_err();
                assert_eq!(error.code, errors::Code::Denied);
                assert_eq!(error.diagnostic_id, "revoked-diagnostic");
                assert_eq!(error.effect, errors::EffectObservation::NotDispatched);
            }
            assert_eq!(
                host.send(ctx, connection.as_ref(), request())
                    .await
                    .unwrap_err()
                    .code,
                errors::Code::Denied
            );
            transcripts.push(owner.requests.lock().unwrap().clone());
        }
        assert_eq!(transcripts[0], transcripts[1]);
    }

    #[tokio::test]
    async fn changed_context_or_cross_invocation_connection_never_reaches_owner() {
        let owner = Arc::new(TestOwner::default());
        let host = OwnerHost::begin(owner.clone(), context(), consumer())
            .await
            .unwrap();
        let other = OwnerHost::begin(owner.clone(), context(), consumer())
            .await
            .unwrap();
        let connection = other.open(other.context(), "api").await.unwrap();
        let count = owner.requests.lock().unwrap().len();
        // Even an owner reusing an invocation ID cannot make private handles interchangeable.
        assert_eq!(
            host.send(host.context(), connection.as_ref(), request())
                .await
                .unwrap_err()
                .code,
            errors::Code::Denied
        );
        let mutations: Vec<Box<dyn Fn(&mut InvocationContext)>> = vec![
            Box::new(|c| c.workspace.push('x')),
            Box::new(|c| c.installation.push('x')),
            Box::new(|c| c.plan.push('x')),
            Box::new(|c| c.subject.push('x')),
            Box::new(|c| c.invocation_id.push('x')),
            Box::new(|c| c.artifact.artifact_digest.push('x')),
            Box::new(|c| c.tool_name.push('x')),
            Box::new(|c| c.grants.logging = false),
            Box::new(|c| {
                c.grants.connection_slots.insert("new".into());
            }),
            Box::new(|c| c.deadline += Duration::from_secs(1)),
        ];
        for mutate in mutations {
            let mut changed = host.context().clone();
            mutate(&mut changed);
            assert_eq!(
                host.authorize(&changed, Operation::Admit)
                    .await
                    .unwrap_err()
                    .code,
                errors::Code::Denied
            );
        }
        let own = host.open(host.context(), "api").await.unwrap();
        assert_eq!(
            host.authorize(
                host.context(),
                Operation::Send {
                    slot: "other",
                    binding: own.as_ref(),
                    request: &request()
                }
            )
            .await
            .unwrap_err()
            .code,
            errors::Code::Denied
        );
        assert_eq!(owner.requests.lock().unwrap().len(), count + 1);
        host.context().cancellation.cancel();
        assert_eq!(
            host.authorize(host.context(), Operation::Admit)
                .await
                .unwrap_err()
                .code,
            errors::Code::Cancelled
        );
        assert_eq!(owner.requests.lock().unwrap().len(), count + 1);
    }

    struct PendingOwner {
        inner: TestOwner,
        begin: bool,
        dropped: Arc<AtomicBool>,
        started: tokio::sync::Notify,
    }
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl PluginHostOwner for PendingOwner {
        async fn call(
            &self,
            request: PluginHostRequest,
        ) -> Result<PluginHostResponse, PluginHostError> {
            if self.begin || matches!(request, PluginHostRequest::Send { .. }) {
                let _guard = Dropped(self.dropped.clone());
                self.started.notify_one();
                std::future::pending().await
            } else {
                self.inner.call(request).await
            }
        }
    }
    #[tokio::test(start_paused = true)]
    async fn begin_deadline_drops_pending_owner_call() {
        let dropped = Arc::new(AtomicBool::new(false));
        let owner = Arc::new(PendingOwner {
            inner: TestOwner::default(),
            begin: true,
            dropped: dropped.clone(),
            started: tokio::sync::Notify::new(),
        });
        let error = OwnerHost::begin(owner, context(), consumer())
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, PluginHostErrorCode::DeadlineExceeded);
        assert_eq!(error.effect, PluginEffectObservation::NotDispatched);
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn cancelled_pending_send_drops_owner_future_and_preserves_uncertainty() {
        let dropped = Arc::new(AtomicBool::new(false));
        let owner = Arc::new(PendingOwner {
            inner: TestOwner::default(),
            begin: false,
            dropped: dropped.clone(),
            started: tokio::sync::Notify::new(),
        });
        let host = OwnerHost::begin(owner.clone(), context(), consumer())
            .await
            .unwrap();
        let binding = host.open(host.context(), "api").await.unwrap();
        let cancellation = host.context().cancellation.clone();
        let (result, ()) = tokio::join!(
            host.send(host.context(), binding.as_ref(), request()),
            async {
                owner.started.notified().await;
                cancellation.cancel();
            }
        );
        let error = result.unwrap_err();
        assert_eq!(error.code, errors::Code::Cancelled);
        assert_eq!(error.effect, errors::EffectObservation::Indeterminate);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn remote_connection_loss_is_indeterminate_and_never_retried() {
        let owner = Arc::new(TestOwner::default());
        let (transport, mut endpoint) = remote(owner.clone()).await;
        let host = OwnerHost::begin(transport, context(), consumer())
            .await
            .unwrap();
        let binding = host.open(host.context(), "api").await.unwrap();
        endpoint.task.abort();
        // Await listener teardown, not a timing-based sleep.
        let _ = (&mut endpoint.task).await;
        let error = host
            .send(host.context(), binding.as_ref(), request())
            .await
            .unwrap_err();
        assert_eq!(error.code, errors::Code::Indeterminate);
        assert_eq!(error.effect, errors::EffectObservation::Indeterminate);
        assert!(!error.message.contains("127.0.0.1"));
        assert_eq!(owner.requests.lock().unwrap().len(), 2);
    }
}
