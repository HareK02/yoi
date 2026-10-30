use super::*;
use crate::service_bindings::{self, exports::yoi::plugin::service as abi};
use manifest::plugin::{PluginServiceManifest, PluginTicketEventType};
use server_api::{PluginServiceEventResult, PluginTicketEventEnvelope};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Shares compiled bytes and the existing Host/resource implementation, not authority.
pub struct CompiledService(CompiledTool);
impl CompiledService {
    pub fn identity(&self) -> &ArtifactIdentity {
        self.0.identity()
    }
    pub fn services(&self) -> &[PluginServiceManifest] {
        &self.0.manifest.services
    }
}

/// Guest acknowledgement is only a proposal. The driver must persist the cursor
/// through its owner and pause on an indeterminate Host effect, even on guest ack.
#[derive(Debug)]
pub struct ServiceInvocationResult {
    pub outcome: Result<ServiceOutput, ExecutionError>,
    pub effect: errors::EffectObservation,
}
#[derive(Debug)]
pub enum ServiceOutput {
    Started,
    Event(PluginServiceEventResult),
    Stopped,
}

enum Call<'a> {
    Start(&'a str),
    Event {
        envelope: &'a PluginTicketEventEnvelope,
        config: &'a str,
    },
    Stop(&'a str),
}

/// Kept outside the Store so cancellation, timeout and traps cannot erase a
/// dispatched-but-unobserved effect. This is observation, not a receipt ledger.
#[derive(Default)]
pub(super) struct Effects {
    pending: AtomicUsize,
    unknown: AtomicBool,
    response: AtomicBool,
}
impl Effects {
    pub(super) fn begin(self: &Arc<Self>) -> EffectWait {
        self.pending.fetch_add(1, Ordering::SeqCst);
        EffectWait {
            effects: self.clone(),
            completed: false,
        }
    }
    pub(super) fn unresolved(&self) {
        self.unknown.store(true, Ordering::SeqCst);
    }
    pub(super) fn observe(&self, error: &errors::HostError) {
        if matches!(error.effect, errors::EffectObservation::Indeterminate)
            || matches!(
                error.code,
                errors::Code::Indeterminate | errors::Code::ReceiptExpired
            )
        {
            self.unknown.store(true, Ordering::SeqCst);
        }
        if matches!(error.effect, errors::EffectObservation::ResponseObserved) {
            self.response.store(true, Ordering::SeqCst);
        }
    }
    fn snapshot(&self) -> errors::EffectObservation {
        if self.unknown.load(Ordering::SeqCst) || self.pending.load(Ordering::SeqCst) != 0 {
            errors::EffectObservation::Indeterminate
        } else if self.response.load(Ordering::SeqCst) {
            errors::EffectObservation::ResponseObserved
        } else {
            errors::EffectObservation::NotDispatched
        }
    }
}
pub(super) struct EffectWait {
    effects: Arc<Effects>,
    completed: bool,
}
impl EffectWait {
    pub(super) fn finish<T>(mut self, result: &Result<T, errors::HostError>) {
        match result {
            Ok(_) => {
                self.effects.response.store(true, Ordering::SeqCst);
            }
            Err(e) => self.effects.observe(e),
        }
        self.completed = true;
    }
}
impl Drop for EffectWait {
    fn drop(&mut self) {
        if !self.completed {
            self.effects.unknown.store(true, Ordering::SeqCst);
        }
        self.effects.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Executor {
    pub fn compile_service(
        &self,
        artifact: &[u8],
        package_digest: &str,
        artifact_digest: &str,
        artifact_size: u64,
    ) -> Result<CompiledService, ExecutionError> {
        let compiled = self.compile(artifact, package_digest, artifact_digest, artifact_size)?;
        if compiled.manifest.runtime.world != plugin::PLUGIN_COMPONENT_SERVICE_WORLD {
            return Err(ExecutionError::Artifact(
                "not the exact Service world".into(),
            ));
        }
        Ok(CompiledService(compiled))
    }

    pub async fn service_start(
        &self,
        compiled: &CompiledService,
        context: InvocationContext,
        provider: Arc<dyn Host>,
        service_name: &str,
        config_json: &str,
    ) -> ServiceInvocationResult {
        self.service_call(
            compiled,
            context,
            provider,
            service_name,
            Call::Start(config_json),
        )
        .await
    }
    pub async fn service_event(
        &self,
        compiled: &CompiledService,
        context: InvocationContext,
        provider: Arc<dyn Host>,
        service_name: &str,
        envelope: &PluginTicketEventEnvelope,
        config_json: &str,
    ) -> ServiceInvocationResult {
        self.service_call(
            compiled,
            context,
            provider,
            service_name,
            Call::Event {
                envelope,
                config: config_json,
            },
        )
        .await
    }
    pub async fn service_stop(
        &self,
        compiled: &CompiledService,
        context: InvocationContext,
        provider: Arc<dyn Host>,
        service_name: &str,
        reason: &str,
    ) -> ServiceInvocationResult {
        self.service_call(
            compiled,
            context,
            provider,
            service_name,
            Call::Stop(reason),
        )
        .await
    }

    async fn service_call(
        &self,
        compiled: &CompiledService,
        mut context: InvocationContext,
        provider: Arc<dyn Host>,
        service_name: &str,
        call: Call<'_>,
    ) -> ServiceInvocationResult {
        let ceiling = match call {
            Call::Start(_) => Duration::from_secs(5),
            Call::Stop(_) => Duration::from_secs(2),
            Call::Event { .. } => self.limits.wall_time,
        };
        context.deadline = context
            .deadline
            .min(Instant::now() + ceiling.min(self.limits.wall_time));
        context.cancellation = context.cancellation.child_token();
        let _lifetime = InvocationLifetime(context.cancellation.clone());
        let effects = Arc::new(Effects::default());
        let execution = self.run_service(
            compiled,
            &context,
            provider,
            service_name,
            call,
            effects.clone(),
        );
        tokio::pin!(execution);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        let outcome = loop {
            tokio::select! {
                biased;
                _ = context.cancellation.cancelled() => break Err(ExecutionError::Cancelled),
                _ = tokio::time::sleep_until(context.deadline) => break Err(ExecutionError::Deadline),
                result = &mut execution => break result,
                _ = tick.tick() => self.engine.increment_epoch(),
            }
        };
        ServiceInvocationResult {
            outcome,
            effect: effects.snapshot(),
        }
    }

    async fn run_service(
        &self,
        compiled: &CompiledService,
        context: &InvocationContext,
        provider: Arc<dyn Host>,
        service_name: &str,
        call: Call<'_>,
        effects: Arc<Effects>,
    ) -> Result<ServiceOutput, ExecutionError> {
        let compiled = &compiled.0;
        if !Engine::same(&self.engine, compiled.component.engine())
            || context.artifact != compiled.identity
            || context.tool_name != service_name
        {
            return Err(ExecutionError::InvalidInput(
                "Service artifact/name does not match Host context".into(),
            ));
        }
        let declaration = compiled
            .manifest
            .services
            .iter()
            .find(|s| s.name == service_name)
            .ok_or_else(|| ExecutionError::InvalidInput("undeclared Service".into()))?;
        if context.invocation_id.is_empty() || context.invocation_id.len() > 1024 {
            return Err(ExecutionError::InvalidInput(
                "invalid invocation identity".into(),
            ));
        }
        let config = match &call {
            Call::Start(c) | Call::Event { config: c, .. } => Some(*c),
            Call::Stop(_) => None,
        };
        if let Some(config) = config {
            if config.len() > self.limits.config_bytes {
                return Err(ExecutionError::InvalidInput("config exceeds limit".into()));
            }
            serde_json::from_str::<serde_json::Value>(config)
                .map_err(|e| ExecutionError::InvalidInput(e.to_string()))?;
        }
        if let Call::Event { envelope: e, .. } = &call {
            let kind = match e.event_type {
                server_api::PluginTicketEventType::Changed => PluginTicketEventType::TicketChanged,
                server_api::PluginTicketEventType::Deleted => PluginTicketEventType::TicketDeleted,
            };
            if !context.grants.ticket_events
                || e.version != 1
                || e.attempt == 0
                || !declaration.subscriptions.contains(&kind)
                || serde_json::to_vec(e)
                    .map_err(|e| ExecutionError::InvalidInput(e.to_string()))?
                    .len()
                    > self.limits.payload_bytes
                || [&e.event_id, &e.ticket_id, &e.domain_event_ref]
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 1024)
            {
                return Err(ExecutionError::InvalidInput(
                    "invalid, oversized or ungranted Service event".into(),
                ));
            }
        }
        if let Call::Stop(reason) = &call {
            if reason.len() > self.limits.payload_bytes {
                return Err(ExecutionError::InvalidInput(
                    "stop reason exceeds limit".into(),
                ));
            }
        }
        provider
            .authorize(context, Operation::Admit)
            .await
            .map_err(ExecutionError::Host)?;
        check_current(context).map_err(ExecutionError::Host)?;
        let limits = &self.limits;
        let mut store = Store::new(
            &self.engine,
            State {
                context: None,
                provider: provider.clone(),
                limits: limits.clone(),
                manifest: compiled.manifest.clone(),
                table: ResourceTable::new(),
                operations: 0,
                resources: 0,
                log_bytes: 0,
                dropped_logs: 0,
                store_limits: SandboxLimits::new(limits),
                logging_only: !matches!(call, Call::Event { .. }),
                effects,
            },
        );
        store.limiter(|s| &mut s.store_limits);
        store.set_fuel(limits.fuel).map_err(trap)?;
        store
            .fuel_async_yield_interval(Some(10_000))
            .map_err(trap)?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        let mut linker = Linker::new(&self.engine);
        service_bindings::TicketServicePlugin::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            |s| s,
        )
        .map_err(trap)?;
        let instance = service_bindings::TicketServicePlugin::instantiate_async(
            &mut store,
            &compiled.component,
            &linker,
        )
        .await
        .map_err(trap)?;
        check_current(context).map_err(ExecutionError::Host)?;
        // Instantiation/start sections never receive an export context.
        store.data_mut().context = Some(context.clone());
        let service = instance.yoi_plugin_service();
        let output = match call {
            Call::Start(config) => {
                service
                    .call_on_start(&mut store, config)
                    .await
                    .map_err(trap)?
                    .map_err(guest_error)?;
                ServiceOutput::Started
            }
            Call::Stop(reason) => {
                service
                    .call_on_stop(&mut store, reason)
                    .await
                    .map_err(trap)?
                    .map_err(guest_error)?;
                ServiceOutput::Stopped
            }
            Call::Event {
                envelope: e,
                config,
            } => {
                let input = abi::Envelope {
                    event_id: e.event_id.clone(),
                    event_type: match e.event_type {
                        server_api::PluginTicketEventType::Changed => {
                            abi::TicketEventType::TicketChanged
                        }
                        server_api::PluginTicketEventType::Deleted => {
                            abi::TicketEventType::TicketDeleted
                        }
                    },
                    event_version: e.version,
                    occurred_at: e.occurred_at.clone(),
                    ticket_ref: e.ticket_id.clone(),
                    domain_event_id: e.domain_event_ref.clone(),
                    content_digest: if e.content_digest.is_empty() {
                        None
                    } else {
                        Some(e.content_digest.clone())
                    },
                    correlation_id: if e.correlation_id.is_empty() {
                        None
                    } else {
                        Some(e.correlation_id.clone())
                    },
                    causation_id: e.causation_id.clone(),
                    invocation_id: context.invocation_id.clone(),
                    service_name: service_name.into(),
                    config_json: config.into(),
                    attempt: e.attempt,
                    remaining_ms: context
                        .deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(u32::MAX as u128) as u32,
                };
                let result = match service
                    .call_on_event(&mut store, &input)
                    .await
                    .map_err(trap)?
                {
                    abi::EventResult::Ack => PluginServiceEventResult::Ack,
                    abi::EventResult::RetryAfter(delay_ms) if delay_ms <= 300_000 => {
                        PluginServiceEventResult::RetryAfter { delay_ms }
                    }
                    abi::EventResult::Reject(reason)
                        if reason.len() <= self.limits.payload_bytes =>
                    {
                        PluginServiceEventResult::Reject
                    }
                    _ => {
                        return Err(ExecutionError::InvalidOutput(
                            "Service result exceeds limit".into(),
                        ));
                    }
                };
                ServiceOutput::Event(result)
            }
        };
        check_current(context).map_err(ExecutionError::Host)?;
        provider
            .authorize(context, Operation::Publish)
            .await
            .map_err(ExecutionError::Host)?;
        check_current(context).map_err(ExecutionError::Host)?;
        Ok(output)
    }
}
fn guest_error(error: abi::ServiceError) -> ExecutionError {
    if error.code.len().saturating_add(error.message.len()) > 4096 {
        ExecutionError::InvalidOutput("Service error exceeds limit".into())
    } else {
        ExecutionError::Guest {
            code: "plugin-failed".into(),
            message: error.message,
        }
    }
}
