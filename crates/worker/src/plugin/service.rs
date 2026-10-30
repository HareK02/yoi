//! Bounded outbox pumps run in the existing Feature task registry. Consumer and
//! effect truth stays with the owner; this module retains only an active lease.
#[cfg(test)]
mod tests;

use super::*;
use crate::feature::background::{
    BackgroundTaskCancellation, BackgroundTaskContext, FeatureBackgroundTask,
};
use server_api::*;
use std::{any::Any, time::Duration};
use yoi_plugin_runtime::{
    CompiledService, ServiceOutput,
    host::{errors, http, logging},
};

pub(super) struct ServiceDriver {
    pub executor: Arc<Executor>,
    pub compiled: Arc<CompiledService>,
    pub owner: Arc<dyn PluginHostOwner>,
    pub workspace: String,
    pub installation: String,
    pub plan: String,
    pub subject: String,
    pub grants: Grants,
    pub name: String,
    pub config: String,
    lease: Mutex<Option<PluginServiceLease>>,
    stop_started: Mutex<Option<PluginServiceLease>>,
    // A selector for read-only recovery observation, never a mirrored pause state.
    parked_consumer: Mutex<Option<String>>,
    // Unconfirmed control commit, not permission to replay an external effect.
    unconfirmed_indeterminate: Mutex<Option<String>>,
    prepared_start: std::sync::atomic::AtomicBool,
}
impl ServiceDriver {
    pub fn new(
        executor: Arc<Executor>,
        compiled: Arc<CompiledService>,
        owner: Arc<dyn PluginHostOwner>,
        pin: &PluginToolPin,
        name: String,
        config: String,
    ) -> Self {
        Self {
            executor,
            compiled,
            owner,
            workspace: pin.workspace_id.clone(),
            installation: pin.installation_id.clone(),
            plan: pin.plan_id.clone(),
            subject: pin.subject.clone(),
            grants: pin.grants.clone(),
            name,
            config,
            lease: Mutex::new(None),
            stop_started: Mutex::new(None),
            parked_consumer: Mutex::new(None),
            unconfirmed_indeterminate: Mutex::new(None),
            prepared_start: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn lifecycle_host(&self) -> Arc<LifecycleHost> {
        Arc::new(LifecycleHost {
            owner: self.owner.clone(),
            plan: self.plan.clone(),
            installation: self.installation.clone(),
            name: self.name.clone(),
        })
    }
    pub async fn prepare_start(&self) -> Result<(), HookError> {
        let output = self
            .executor
            .service_start(
                &self.compiled,
                self.context(
                    format!("{}:{}:start", self.installation, self.name),
                    CancellationToken::new(),
                ),
                self.lifecycle_host(),
                &self.name,
                &self.config,
            )
            .await;
        if !matches!(output.outcome, Ok(ServiceOutput::Started)) {
            return Err(dependency());
        }
        self.prepared_start
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    pub fn task_name(&self) -> String {
        format!("service:{}:{}", self.installation, self.name)
    }
    fn context(&self, invocation_id: String, cancellation: CancellationToken) -> InvocationContext {
        InvocationContext {
            workspace: self.workspace.clone(),
            installation: self.installation.clone(),
            plan: self.plan.clone(),
            subject: self.subject.clone(),
            invocation_id,
            artifact: self.compiled.identity().clone(),
            tool_name: self.name.clone(),
            grants: self.grants.clone(),
            deadline: tokio::time::Instant::now()
                + self
                    .executor
                    .limits()
                    .wall_time
                    .min(Duration::from_secs(20)),
            cancellation,
        }
    }
    async fn request(
        &self,
        request: PluginServiceRequest,
    ) -> Result<PluginServiceResponse, HookError> {
        request_service(self.owner.as_ref(), request)
            .await
            .map_err(|_| dependency())
    }
    pub(super) async fn reconcile_release(&self) -> Result<(), HookError> {
        let Some(lease) = self.lease.lock().expect("Service lease lock").clone() else {
            return Ok(());
        };
        let released = match request_service(
            self.owner.as_ref(),
            PluginServiceRequest::Release {
                lease: lease.clone(),
            },
        )
        .await
        {
            Ok(PluginServiceResponse::Released) => true,
            // Release authenticates this consumer's Worker before its exact-token
            // CAS. Conflict confirms this proof no longer owns the lease; do not
            // mutate or wait for the successor as though it were our resource.
            Err(error) if error.code == PluginHostErrorCode::Conflict => true,
            _ => false,
        };
        if !released {
            return Err(dependency());
        }
        let mut retained = self.lease.lock().expect("Service lease lock");
        if retained.as_ref() == Some(&lease) {
            *retained = None;
        }
        Ok(())
    }
    async fn acquire(&self) -> Result<PluginServiceLease, HookError> {
        if let Some(lease) = self.lease.lock().expect("Service lease lock").clone() {
            if self
                .stop_started
                .lock()
                .expect("Service stop lock")
                .as_ref()
                == Some(&lease)
            {
                return Err(dependency());
            }
            return Ok(lease);
        }
        let PluginServiceResponse::Acquired { lease } = self
            .request(PluginServiceRequest::Acquire {
                plan_id: self.plan.clone(),
                installation_id: self.installation.clone(),
                service_name: self.name.clone(),
            })
            .await?
        else {
            return Err(dependency());
        };
        *self.lease.lock().expect("Service lease lock") = Some(lease.clone());
        if !self
            .prepared_start
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            self.prepare_start().await?;
            self.prepared_start
                .store(false, std::sync::atomic::Ordering::Release);
        }
        Ok(lease)
    }
}
#[async_trait]
impl FeatureBackgroundTask for Arc<ServiceDriver> {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        context.generation_fence.ensure_current()?;
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let parked = self
            .parked_consumer
            .lock()
            .expect("Service observer lock")
            .clone();
        if let Some(consumer_id) = parked {
            let diagnostics = match self
                .request(PluginServiceRequest::Diagnostics { consumer_id })
                .await
            {
                Ok(PluginServiceResponse::Diagnostics { diagnostics }) => diagnostics,
                // Observation alone neither retries a guest nor overrides live
                // revocation. Keep the parked pump inert until its owner returns.
                _ => return Ok(()),
            };
            if !diagnostics.lease_active {
                // A fresh owner observation confirms any retained proof is now
                // fenced (release may have committed before its response was lost).
                // Never reuse that proof after expiry or audited recovery.
                *self.lease.lock().expect("Service lease lock") = None;
            }
            if diagnostics.pause_reason.is_some() || diagnostics.lease_active {
                return Ok(());
            }
            let unconfirmed = self
                .unconfirmed_indeterminate
                .lock()
                .expect("Service completion lock")
                .clone();
            if let Some(event_id) = unconfirmed {
                if diagnostics.current_event_id.as_ref() == Some(&event_id) {
                    // No pause is not evidence that an uncertain completion did
                    // not execute. Reconcile the same control intent under a new
                    // owner lease, without polling or invoking the guest again.
                    if let Ok(lease) = self.acquire().await {
                        if matches!(
                            self.request(PluginServiceRequest::Complete {
                                lease,
                                event_id,
                                result: PluginServiceEventResult::Indeterminate,
                            })
                            .await,
                            Ok(PluginServiceResponse::Completed)
                        ) {
                            *self
                                .unconfirmed_indeterminate
                                .lock()
                                .expect("Service completion lock") = None;
                        }
                        if self.stopped(context).await.is_err() {
                            tracing::warn!("Plugin Service stop requires owner reconciliation");
                        }
                    }
                    return Ok(());
                }
                // The owner has already advanced this head (including an audited
                // skip). Never recreate it or change its stable event identity.
                *self
                    .unconfirmed_indeterminate
                    .lock()
                    .expect("Service completion lock") = None;
            }
            // Only an audited owner recovery clears a domain pause. Observing
            // that decision wakes this consumer alone, not the entire registry.
            *self.parked_consumer.lock().expect("Service observer lock") = None;
        }
        if self.lease.lock().expect("Service lease lock").is_none() {
            self.acquire().await?;
            return Ok(());
        }
        let lease = self.acquire().await?;
        let response = match self
            .request(PluginServiceRequest::Poll {
                lease: lease.clone(),
            })
            .await
        {
            Ok(response) => response,
            Err(_) => {
                *self.parked_consumer.lock().expect("Service observer lock") =
                    Some(lease.consumer_id.clone());
                if self.stopped(context).await.is_err() {
                    tracing::warn!("Plugin Service stop requires owner reconciliation");
                }
                return Ok(());
            }
        };
        match response {
            PluginServiceResponse::Idle => Ok(()),
            PluginServiceResponse::Diagnostics { diagnostics } => {
                // Retain only the consumer selector while paused. Stop the guest
                // lifetime and release its lease so management can recover it.
                // The registry retains a bounded read-only observation pump.
                *self.parked_consumer.lock().expect("Service observer lock") =
                    Some(diagnostics.consumer_id);
                // Revoked logging or a failed stop export must not hide the
                // audited recovery observer. Release is attempted independently.
                if self.stopped(context).await.is_err() {
                    tracing::warn!("Plugin Service stop requires owner reconciliation");
                }
                Ok(())
            }
            PluginServiceResponse::Delivery {
                envelope,
                invocation_id,
                expires_at_ms,
            } => {
                context.generation_fence.ensure_current()?;
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                let consumer = PluginConsumer::Consumer {
                    consumer_id: lease.consumer_id.clone(),
                    event_id: envelope.event_id.clone(),
                    export_name: self.name.clone(),
                };
                let owner = Arc::new(DeliveryOwner {
                    owner: self.owner.clone(),
                    lease: lease.clone(),
                    consumer: consumer.clone(),
                    invocation_id: invocation_id.clone(),
                    expires_at_ms,
                });
                let token = CancellationToken::new();
                let host = Arc::new(
                    OwnerHost::begin(owner, self.context(invocation_id, token.clone()), consumer)
                        .await
                        .map_err(|_| dependency())?,
                );
                let call = self.executor.service_event(
                    &self.compiled,
                    host.context().clone(),
                    host.clone(),
                    &self.name,
                    &envelope,
                    &self.config,
                );
                tokio::pin!(call);
                let output = tokio::select! {
                    result = &mut call => result,
                    _ = cancellation.cancelled() => { token.cancel(); call.await },
                };
                let result = if matches!(output.effect, errors::EffectObservation::Indeterminate) {
                    PluginServiceEventResult::Indeterminate
                } else {
                    match output.outcome {
                        Ok(ServiceOutput::Event(result)) => result,
                        _ => PluginServiceEventResult::RetryAfter { delay_ms: 1_000 },
                    }
                };
                context.generation_fence.ensure_current()?;
                // Guest return is not ack. Only this fenced owner transaction
                // commits a cursor; transport uncertainty leaves it for recovery.
                let indeterminate = matches!(result, PluginServiceEventResult::Indeterminate);
                if !matches!(
                    self.request(PluginServiceRequest::Complete {
                        lease: lease.clone(),
                        event_id: envelope.event_id.clone(),
                        result
                    })
                    .await,
                    Ok(PluginServiceResponse::Completed)
                ) {
                    // The cursor/pause transaction may have committed even if its
                    // reply was lost. Keep the read-only observer alive instead
                    // of permanently deactivating this HostReady registration.
                    if indeterminate {
                        *self
                            .unconfirmed_indeterminate
                            .lock()
                            .expect("Service completion lock") = Some(envelope.event_id.clone());
                    }
                    *self.parked_consumer.lock().expect("Service observer lock") =
                        Some(lease.consumer_id.clone());
                    if self.stopped(context).await.is_err() {
                        tracing::warn!("Plugin Service stop requires owner reconciliation");
                    }
                }
                Ok(())
            }
            _ => Err(dependency()),
        }
    }
    async fn stopped(&self, _: BackgroundTaskContext) -> Result<(), HookError> {
        // Inline pause cleanup is cancellable. Retain the exact proof until the
        // owner confirms release, so protected registry cleanup can finish it if
        // the run future is dropped during on-stop or transport uncertainty.
        let lease = self.lease.lock().expect("Service lease lock").clone();
        let Some(lease) = lease else {
            return Ok(());
        };
        let attempt_stop = {
            let mut started = self.stop_started.lock().expect("Service stop lock");
            if started.as_ref() == Some(&lease) {
                false
            } else {
                *started = Some(lease.clone());
                true
            }
        };
        let stop_failed = if attempt_stop {
            let output = self
                .executor
                .service_stop(
                    &self.compiled,
                    self.context(
                        format!("{}:stop", lease.consumer_id),
                        CancellationToken::new(),
                    ),
                    self.lifecycle_host(),
                    &self.name,
                    "Host task stopped",
                )
                .await;
            !matches!(output.outcome, Ok(ServiceOutput::Stopped))
        } else {
            false
        };
        // Marking the attempt before the await prevents an interrupted inline
        // cleanup from rerunning the stop export in protected registry cleanup.
        // A failed/revoked/timed-out export never prevents fencing/release.
        self.reconcile_release().await?;
        if stop_failed {
            return Err(dependency());
        }
        Ok(())
    }
}
fn dependency() -> HookError {
    HookError::new(
        HookErrorCategory::Dependency,
        "Plugin Service requires owner reconciliation",
    )
}
fn unavailable() -> PluginHostError {
    PluginHostError {
        code: PluginHostErrorCode::Unavailable,
        effect: PluginEffectObservation::NotDispatched,
        diagnostic_id: String::new(),
    }
}
async fn request_service(
    owner: &dyn PluginHostOwner,
    request: PluginServiceRequest,
) -> Result<PluginServiceResponse, PluginHostError> {
    // Event imports already run under the executor's total invocation deadline.
    // A shorter transport deadline could abandon a still-running admitted effect
    // before the owner's HTTP/receipt budgets expire. Control requests remain
    // separately bounded because they run outside a guest invocation.
    let budget = if matches!(&request, PluginServiceRequest::Host { .. }) {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(2)
    };
    let response = tokio::time::timeout(budget, owner.call(PluginHostRequest::Service { request }))
        .await
        .map_err(|_| unavailable())??;
    if let PluginHostResponse::Service { response } = response {
        Ok(response)
    } else {
        Err(unavailable())
    }
}

/// Adapts an already owner-issued delivery to the existing receipt-aware Host.
/// Begin here only binds that receipt; it cannot create a logical invocation.
struct DeliveryOwner {
    owner: Arc<dyn PluginHostOwner>,
    lease: PluginServiceLease,
    consumer: PluginConsumer,
    invocation_id: String,
    expires_at_ms: i64,
}
#[async_trait]
impl PluginHostOwner for DeliveryOwner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        if let PluginHostRequest::Begin { consumer, .. } = &request {
            if consumer != &self.consumer {
                return Err(unavailable());
            }
            return Ok(PluginHostResponse::Invocation {
                invocation_id: self.invocation_id.clone(),
                expires_at_ms: self.expires_at_ms,
            });
        }
        let may_dispatch = matches!(
            request,
            PluginHostRequest::Send { .. } | PluginHostRequest::DataMutate { .. }
        );
        let response = request_service(
            self.owner.as_ref(),
            PluginServiceRequest::Host {
                lease: self.lease.clone(),
                request: Box::new(request.try_into().map_err(|_| unavailable())?),
            },
        )
        .await;
        match response {
            Ok(PluginServiceResponse::Host { response }) => Ok((*response).into()),
            Ok(_) => Err(unavailable()),
            Err(mut error) => {
                if may_dispatch && matches!(error.code, PluginHostErrorCode::Unavailable) {
                    error.code = PluginHostErrorCode::Indeterminate;
                    error.effect = PluginEffectObservation::Indeterminate;
                }
                Err(error)
            }
        }
    }
}

/// Lifecycle exports have no object/connection/data resources. Bounded
/// logging follows a current owner check, never an event receipt or guest grant.
struct LifecycleHost {
    owner: Arc<dyn PluginHostOwner>,
    plan: String,
    installation: String,
    name: String,
}
fn denied_host() -> errors::HostError {
    errors::HostError {
        code: errors::Code::Denied,
        message: "lifecycle operation denied".into(),
        diagnostic_id: String::new(),
        effect: errors::EffectObservation::NotDispatched,
    }
}
#[async_trait]
impl Host for LifecycleHost {
    async fn authorize(
        &self,
        _: &InvocationContext,
        operation: Operation<'_>,
    ) -> Result<(), errors::HostError> {
        if !matches!(
            operation,
            Operation::Admit | Operation::Publish | Operation::Log
        ) {
            return Err(denied_host());
        }
        if matches!(operation, Operation::Log) {
            return Ok(());
        }
        match request_service(
            self.owner.as_ref(),
            PluginServiceRequest::Prepare {
                plan_id: self.plan.clone(),
                installation_id: self.installation.clone(),
                service_name: self.name.clone(),
                operation: match operation {
                    Operation::Admit => PluginServiceLifecycleOperation::Admit,
                    Operation::Publish => PluginServiceLifecycleOperation::Publish,
                    _ => return Err(denied_host()),
                },
            },
        )
        .await
        {
            Ok(PluginServiceResponse::Prepared) => Ok(()),
            _ => Err(denied_host()),
        }
    }
    async fn open(
        &self,
        _: &InvocationContext,
        _: &str,
    ) -> Result<Arc<dyn Any + Send + Sync>, errors::HostError> {
        Err(denied_host())
    }
    async fn send(
        &self,
        _: &InvocationContext,
        _: &(dyn Any + Send + Sync),
        _: http::Request,
    ) -> Result<http::Response, errors::HostError> {
        Err(denied_host())
    }
    async fn log(
        &self,
        context: &InvocationContext,
        entry: logging::Entry,
    ) -> Result<(), errors::HostError> {
        self.authorize(context, Operation::Log).await?;
        // This request performs live plugin.log authorization without creating
        // an event receipt or allowing lifecycle access to effect APIs.
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
                .map(|field| {
                    Ok(PluginLogField {
                        name: field.name,
                        value: match field.value {
                            logging::Scalar::Text(v) => PluginLogScalar::Text(v),
                            logging::Scalar::Boolean(v) => PluginLogScalar::Boolean(v),
                            logging::Scalar::Signed(v) => PluginLogScalar::Signed(v),
                            logging::Scalar::Unsigned(v) => PluginLogScalar::Unsigned(v),
                            logging::Scalar::Number(v) if v.is_finite() => {
                                PluginLogScalar::Number(v)
                            }
                            logging::Scalar::Number(_) => return Err(denied_host()),
                        },
                    })
                })
                .collect::<Result<_, errors::HostError>>()?,
        };
        match request_service(
            self.owner.as_ref(),
            PluginServiceRequest::LifecycleLog {
                plan_id: self.plan.clone(),
                installation_id: self.installation.clone(),
                service_name: self.name.clone(),
                entry,
            },
        )
        .await
        {
            Ok(PluginServiceResponse::Logged) => Ok(()),
            _ => Err(denied_host()),
        }
    }
}
