mod service;
pub use service::{CompiledService, ServiceInvocationResult, ServiceOutput};
mod ticket;
use crate::sandbox::SandboxLimits;
use crate::{
    bindings,
    host::{self, Connection, Host, InvocationContext, Operation, errors, http, logging},
};
use manifest::plugin::{self, PluginPackageLimits, PluginPackageManifest, PluginToolManifest};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::{Config, Engine, Store};

#[derive(Clone, Debug)]
pub struct Limits {
    pub payload_bytes: usize,
    pub config_bytes: usize,
    pub memory_bytes: usize,
    pub table_elements: usize,
    pub instances: usize,
    pub memories: usize,
    pub tables: usize,
    pub resources: usize,
    pub fuel: u64,
    pub wall_time: Duration,
    pub host_operations: usize,
    pub http_body_bytes: usize,
    pub http_header_bytes: usize,
    pub http_header_pairs: usize,
    pub log_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            payload_bytes: 64 * 1024,
            config_bytes: 64 * 1024,
            memory_bytes: 256 * 1024 * 1024,
            table_elements: 10_000,
            instances: 32,
            memories: 4,
            tables: 8,
            resources: 32,
            fuel: 10_000_000,
            wall_time: Duration::from_secs(30),
            host_operations: 32,
            http_body_bytes: 1024 * 1024,
            http_header_bytes: 16 * 1024,
            http_header_pairs: 64,
            log_bytes: 16 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("invalid artifact: {0}")]
    Artifact(String),
    #[error("unsupported Component execution policy: {0}")]
    UnsupportedComponent(String),
    #[error("invalid invocation: {0}")]
    InvalidInput(String),
    #[error("invalid output: {0}")]
    InvalidOutput(String),
    #[error("invocation cancelled")]
    Cancelled,
    #[error("invocation deadline exceeded")]
    Deadline,
    #[error("Host denied or failed operation: {0:?}")]
    Host(errors::HostError),
    #[error("guest error {code}: {message}")]
    Guest { code: String, message: String },
    #[error("Component trap or resource limit: {0}")]
    Trap(String),
}

/// Content identity only; both digests are independently verified by the package owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactIdentity {
    pub package_digest: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
}

/// Immutable compiled bytes and declarations, not execution authority.
/// Private fields prevent mutable package metadata from bypassing verification.
pub struct CompiledTool {
    component: Component,
    manifest: PluginPackageManifest,
    identity: ArtifactIdentity,
}
impl CompiledTool {
    pub fn identity(&self) -> &ArtifactIdentity {
        &self.identity
    }

    /// Declarations from the verified immutable package, not execution grants.
    pub fn tools(&self) -> &[PluginToolManifest] {
        &self.manifest.tools
    }
}

#[derive(Debug)]
pub struct InvocationResult {
    pub output: bindings::exports::yoi::plugin::tool::Output,
    pub dropped_logs: u64,
}

pub struct Executor {
    engine: Engine,
    limits: Limits,
}
impl Executor {
    /// Limits are fixed by the Host, never selected by guest input.
    pub fn new(limits: Limits) -> wasmtime::Result<Self> {
        let ceiling = Limits::default();
        macro_rules! check_ceiling {
            ($($field:ident),+ $(,)?) => { $(
                if limits.$field > ceiling.$field {
                    return Err(wasmtime::Error::msg(concat!("limit exceeds supported ceiling: ", stringify!($field))));
                }
            )+ };
        }
        check_ceiling!(
            payload_bytes,
            config_bytes,
            memory_bytes,
            table_elements,
            instances,
            memories,
            tables,
            resources,
            fuel,
            wall_time,
            host_operations,
            http_body_bytes,
            http_header_bytes,
            http_header_pairs,
            log_bytes
        );
        let mut config = Config::new();
        config
            .async_support(true)
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .wasm_threads(false)
            .wasm_shared_everything_threads(false);
        Ok(Self {
            engine: Engine::new(&config)?,
            limits,
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Reuses the canonical package/digest/ABI validator; no path discovery or
    /// duplicate pack/hash implementation is involved. Compilation executes no guest.
    pub fn compile(
        &self,
        artifact: &[u8],
        package_digest: &str,
        artifact_digest: &str,
        artifact_size: u64,
    ) -> Result<CompiledTool, ExecutionError> {
        let verified = plugin::verify_plugin_artifact(
            artifact,
            package_digest,
            artifact_digest,
            artifact_size,
            &PluginPackageLimits::default(),
        )
        .map_err(|e| ExecutionError::Artifact(e.message))?;
        let manifest = verified.package.manifest;
        let component_bytes = &verified.files[&manifest.runtime.component];
        crate::sandbox::check_component_resources(component_bytes)
            .map_err(|e| ExecutionError::UnsupportedComponent(e.to_string()))?;
        let component = Component::new(&self.engine, component_bytes)
            .map_err(|e| ExecutionError::Artifact(e.to_string()))?;
        Ok(CompiledTool {
            component,
            manifest,
            identity: ArtifactIdentity {
                package_digest: package_digest.into(),
                artifact_digest: artifact_digest.into(),
                artifact_size,
            },
        })
    }

    /// All computation and Host waits share the earlier of the caller deadline
    /// and local ceiling. Dropping this future also drops its Store and resources.
    pub async fn invoke(
        &self,
        compiled: &CompiledTool,
        mut context: InvocationContext,
        provider: Arc<dyn Host>,
        tool_name: &str,
        input_json: &str,
        config_json: &str,
    ) -> Result<InvocationResult, ExecutionError> {
        context.deadline = context.deadline.min(Instant::now() + self.limits.wall_time);
        context.cancellation = context.cancellation.child_token();
        let _lifetime = InvocationLifetime(context.cancellation.clone());
        let deadline = context.deadline;
        let cancellation = context.cancellation.clone();
        let execution = self.run(
            compiled,
            context,
            provider,
            tool_name,
            input_json,
            config_json,
        );
        tokio::pin!(execution);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ExecutionError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => return Err(ExecutionError::Deadline),
                result = &mut execution => {
                    if cancellation.is_cancelled() { return Err(ExecutionError::Cancelled); }
                    if Instant::now() >= deadline { return Err(ExecutionError::Deadline); }
                    return result;
                },
                _ = tick.tick() => self.engine.increment_epoch(),
            }
        }
    }

    async fn run(
        &self,
        compiled: &CompiledTool,
        context: InvocationContext,
        provider: Arc<dyn Host>,
        tool_name: &str,
        input_json: &str,
        config_json: &str,
    ) -> Result<InvocationResult, ExecutionError> {
        if !Engine::same(&self.engine, compiled.component.engine()) {
            return Err(ExecutionError::Artifact(
                "Component belongs to another executor".into(),
            ));
        }
        if context.artifact != compiled.identity || context.tool_name != tool_name {
            return Err(ExecutionError::InvalidInput(
                "artifact/Tool does not match Host-bound context".into(),
            ));
        }
        let tool = compiled
            .manifest
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| ExecutionError::InvalidInput("undeclared Tool".into()))?;
        if input_json.len() > self.limits.payload_bytes
            || config_json.len() > self.limits.config_bytes
            || context.invocation_id.len() > 1024
        {
            return Err(ExecutionError::InvalidInput(
                "input/config/identity exceeds limit".into(),
            ));
        }
        validate_json(&tool.input_schema, input_json).map_err(ExecutionError::InvalidInput)?;
        serde_json::from_str::<serde_json::Value>(config_json)
            .map_err(|e| ExecutionError::InvalidInput(e.to_string()))?;
        provider
            .authorize(&context, Operation::Admit)
            .await
            .map_err(ExecutionError::Host)?;
        check_current(&context).map_err(ExecutionError::Host)?;
        let limits = &self.limits;
        let state = State {
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
            logging_only: false,
            effects: Arc::new(service::Effects::default()),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.store_limits);
        store.set_fuel(limits.fuel).map_err(trap)?;
        store
            .fuel_async_yield_interval(Some(10_000))
            .map_err(trap)?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        let mut linker = Linker::new(&self.engine);
        bindings::HttpsLoggedToolPlugin::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            |s| s,
        )
        .map_err(trap)?;
        if compiled.manifest.runtime.world == plugin::PLUGIN_COMPONENT_TICKET_TOOL_WORLD {
            use crate::host::{extension_data, objects, ticket_read};
            objects::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |s| s)
                .map_err(trap)?;
            ticket_read::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |s| s)
                .map_err(trap)?;
            extension_data::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |s| s)
                .map_err(trap)?;
        }
        let instance = bindings::HttpsLoggedToolPlugin::instantiate_async(
            &mut store,
            &compiled.component,
            &linker,
        )
        .await
        .map_err(trap)?;
        // No import during instantiate/start may act, including logging. Service
        // on-start is a different world and has no adapter here.
        check_current(&context).map_err(ExecutionError::Host)?;
        store.data_mut().context = Some(context.clone());
        let invocation = bindings::exports::yoi::plugin::tool::Invocation {
            invocation_id: context.invocation_id.clone(),
            tool_name: tool_name.into(),
            input_json: input_json.into(),
            config_json: config_json.into(),
            remaining_ms: context
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX as u128) as u32,
        };
        let result = instance
            .yoi_plugin_tool()
            .call_invoke(&mut store, &invocation)
            .await
            .map_err(trap)?;
        check_current(&context).map_err(ExecutionError::Host)?;
        provider
            .authorize(&context, Operation::Publish)
            .await
            .map_err(ExecutionError::Host)?;
        check_current(&context).map_err(ExecutionError::Host)?;
        let output = match result {
            Ok(output) => {
                validate_output(tool, &output, limits.payload_bytes)?;
                output
            }
            Err(error) => {
                if error.message.len().saturating_add(error.code.len()) > limits.payload_bytes {
                    return Err(ExecutionError::InvalidOutput(
                        "guest error exceeds limit".into(),
                    ));
                }
                let code = if tool.error_codes.contains(&error.code) {
                    error.code
                } else {
                    "plugin-failed".into()
                };
                #[derive(Serialize)]
                struct Failure<'a> {
                    code: &'a str,
                    message: &'a str,
                }
                if serde_json::to_vec(&Failure {
                    code: &code,
                    message: &error.message,
                })
                .unwrap()
                .len()
                    > limits.payload_bytes
                {
                    return Err(ExecutionError::InvalidOutput(
                        "serialized guest error exceeds limit".into(),
                    ));
                }
                return Err(ExecutionError::Guest {
                    code,
                    message: error.message,
                });
            }
        };
        Ok(InvocationResult {
            output,
            dropped_logs: store.data().dropped_logs,
        })
    }
}
fn trap(e: wasmtime::Error) -> ExecutionError {
    ExecutionError::Trap(format!("{e:#}"))
}
fn validate_json(schema: &serde_json::Value, json: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    // Schema admission already forbids external refs/dialects in manifest::plugin.
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|e| e.to_string())?;
    validator.validate(&value).map_err(|e| e.to_string())
}
fn validate_output(
    tool: &PluginToolManifest,
    output: &bindings::exports::yoi::plugin::tool::Output,
    max: usize,
) -> Result<(), ExecutionError> {
    let raw_bytes = output
        .summary
        .len()
        .saturating_add(output.content.as_ref().map_or(0, String::len))
        .saturating_add(output.structured_json.as_ref().map_or(0, String::len));
    if raw_bytes > max {
        return Err(ExecutionError::InvalidOutput("output exceeds limit".into()));
    }
    // Use the complete serialized envelope budget, not separate per-field limits.
    #[derive(Serialize)]
    struct Envelope<'a> {
        summary: &'a str,
        content: &'a Option<String>,
        structured_json: &'a Option<String>,
    }
    let bytes = serde_json::to_vec(&Envelope {
        summary: &output.summary,
        content: &output.content,
        structured_json: &output.structured_json,
    })
    .unwrap();
    if bytes.len() > max {
        return Err(ExecutionError::InvalidOutput("output exceeds limit".into()));
    }
    let json = output
        .structured_json
        .as_deref()
        .ok_or_else(|| ExecutionError::InvalidOutput("missing structured output".into()))?;
    validate_json(&tool.output_schema, json).map_err(ExecutionError::InvalidOutput)
}

struct InvocationLifetime(host::CancellationToken);
impl Drop for InvocationLifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct State {
    context: Option<InvocationContext>,
    provider: Arc<dyn Host>,
    limits: Limits,
    manifest: PluginPackageManifest,
    table: ResourceTable,
    operations: usize,
    resources: usize,
    log_bytes: usize,
    dropped_logs: u64,
    store_limits: SandboxLimits,
    logging_only: bool,
    effects: Arc<service::Effects>,
}
fn check_current(context: &InvocationContext) -> Result<(), errors::HostError> {
    if context.cancellation.is_cancelled() {
        return Err(host::error(errors::Code::Cancelled, "invocation cancelled"));
    }
    if Instant::now() >= context.deadline {
        return Err(host::error(
            errors::Code::DeadlineExceeded,
            "invocation deadline exceeded",
        ));
    }
    Ok(())
}
impl State {
    fn current(&self) -> wasmtime::Result<InvocationContext> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| wasmtime::Error::msg("Host import outside export context"))?;
        check_current(context).map_err(|_| wasmtime::Error::msg("invocation no longer active"))?;
        Ok(context.clone())
    }
    fn protected_current(&self) -> wasmtime::Result<InvocationContext> {
        let context = self.current()?;
        if self.logging_only {
            return Err(wasmtime::Error::msg(
                "Service lifecycle permits logging only",
            ));
        }
        Ok(context)
    }
    fn operation(&mut self) -> Result<(), errors::HostError> {
        if self.operations >= self.limits.host_operations {
            return Err(host::error(
                errors::Code::QuotaExceeded,
                "Host operation limit",
            ));
        }
        self.operations += 1;
        Ok(())
    }
}
impl bindings::yoi::host::errors::Host for State {}
impl bindings::yoi::host::connections::HostConnection for State {
    async fn drop(&mut self, resource: Resource<Connection>) -> wasmtime::Result<()> {
        self.table.delete(resource)?;
        self.resources -= 1;
        Ok(())
    }
}
impl bindings::yoi::host::connections::Host for State {
    async fn open(
        &mut self,
        slot: String,
    ) -> wasmtime::Result<Result<Resource<Connection>, errors::HostError>> {
        let context = self.protected_current()?;
        if let Err(e) = self.operation() {
            return Ok(Err(e));
        }
        let declared = self.manifest.connections.iter().any(|s| s.name == slot);
        if !declared || !context.grants.connection_slots.contains(&slot) {
            return Ok(Err(host::error(
                errors::Code::Denied,
                "connection slot not granted",
            )));
        }
        if self.resources >= self.limits.resources {
            return Ok(Err(host::error(
                errors::Code::QuotaExceeded,
                "resource limit",
            )));
        }
        if let Err(e) = self
            .provider
            .authorize(&context, Operation::Open { slot: &slot })
            .await
        {
            return Ok(Err(e));
        }
        check_current(&context).map_err(|_| wasmtime::Error::msg("invocation no longer active"))?;
        let binding = match self.provider.open(&context, &slot).await {
            Ok(v) => v,
            Err(e) => return Ok(Err(e)),
        };
        check_current(&context).map_err(|_| wasmtime::Error::msg("late connection result"))?;
        let resource = self.table.push(Connection { slot, binding })?;
        self.resources += 1;
        Ok(Ok(resource))
    }
}
impl bindings::yoi::host::http::Host for State {
    async fn send(
        &mut self,
        target: Resource<Connection>,
        mut input: http::Request,
    ) -> wasmtime::Result<Result<http::Response, errors::HostError>> {
        let context = self.protected_current()?;
        if let Err(e) = self.operation() {
            return Ok(Err(e));
        }
        let connection = self.table.get(&target)?;
        let slot = connection.slot.clone();
        let binding = connection.binding.clone();
        if !context.grants.connection_slots.contains(&slot) {
            return Ok(Err(host::error(
                errors::Code::Denied,
                "connection slot not granted",
            )));
        }
        let declaration = self
            .manifest
            .connections
            .iter()
            .find(|s| s.name == slot)
            .unwrap();
        let method = match input.method {
            http::Method::Get => plugin::PluginHttpMethod::Get,
            http::Method::Head => plugin::PluginHttpMethod::Head,
            http::Method::Post => plugin::PluginHttpMethod::Post,
            http::Method::Put => plugin::PluginHttpMethod::Put,
            http::Method::Patch => plugin::PluginHttpMethod::Patch,
            http::Method::Delete => plugin::PluginHttpMethod::Delete,
            http::Method::Options => plugin::PluginHttpMethod::Options,
        };
        // Canonical URL/DNS/TLS/secret and narrower binding checks belong to the
        // provider. These static ceilings are not a network protection claim.
        if !declaration.methods.contains(&method)
            || !declaration.path_prefixes.iter().any(|p| {
                input.path == *p
                    || input
                        .path
                        .strip_prefix(p)
                        .is_some_and(|rest| p.ends_with('/') || rest.starts_with('/'))
            })
        {
            return Ok(Err(host::error(
                errors::Code::Denied,
                "request exceeds package ceiling",
            )));
        }
        if !request_bounded(&input, &self.limits) {
            return Ok(Err(host::error(
                errors::Code::QuotaExceeded,
                "request exceeds limit",
            )));
        }
        input.timeout_ms = input.timeout_ms.min(15_000).min(
            context
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX as u128) as u32,
        );
        let send_deadline = context
            .deadline
            .min(Instant::now() + Duration::from_millis(input.timeout_ms as u64));
        match tokio::time::timeout_at(
            send_deadline,
            self.provider.authorize(
                &context,
                Operation::Send {
                    slot: &slot,
                    binding: binding.as_ref(),
                    request: &input,
                },
            ),
        )
        .await
        {
            Ok(Ok(())) => (),
            Ok(Err(error)) => return Ok(Err(error)),
            Err(_) => {
                return Ok(Err(host::error(
                    errors::Code::DeadlineExceeded,
                    "HTTP authorization deadline exceeded",
                )));
            }
        }
        check_current(&context).map_err(|_| wasmtime::Error::msg("invocation no longer active"))?;
        if Instant::now() >= send_deadline {
            return Ok(Err(host::error(
                errors::Code::DeadlineExceeded,
                "HTTP deadline exceeded before dispatch",
            )));
        }
        let effect = self.effects.begin();
        let response = match tokio::time::timeout_at(
            send_deadline,
            self.provider.send(&context, binding.as_ref(), input),
        )
        .await
        {
            Ok(response) => response,
            Err(_) => Err(errors::HostError {
                code: errors::Code::DeadlineExceeded,
                message: "HTTP wait deadline exceeded".into(),
                diagnostic_id: String::new(),
                effect: errors::EffectObservation::Indeterminate,
            }),
        };
        effect.finish(&response);
        check_current(&context).map_err(|_| wasmtime::Error::msg("late HTTP result"))?;
        Ok(response.and_then(|response| {
            if response.body.len() > self.limits.http_body_bytes
                || !pairs_bounded(&response.headers, &self.limits)
            {
                Err(errors::HostError {
                    code: errors::Code::QuotaExceeded,
                    message: "response exceeds limit".into(),
                    diagnostic_id: String::new(),
                    effect: errors::EffectObservation::ResponseObserved,
                })
            } else {
                Ok(response)
            }
        }))
    }
}
fn pairs_bounded(pairs: &[http::Pair], limits: &Limits) -> bool {
    pairs.len() <= limits.http_header_pairs
        && pairs
            .iter()
            .try_fold(0usize, |n, p| {
                n.checked_add(p.name.len())?.checked_add(p.value.len())
            })
            .is_some_and(|n| n <= limits.http_header_bytes)
}
fn request_bounded(request: &http::Request, limits: &Limits) -> bool {
    request.body.len() <= limits.http_body_bytes
        && request.path.len() <= limits.payload_bytes
        && request.effect_key.len() <= 128
        && !request.effect_key.is_empty()
        && pairs_bounded(&request.headers, limits)
        && pairs_bounded(&request.query, limits)
}
impl bindings::yoi::host::logging::Host for State {
    async fn log(
        &mut self,
        input: logging::Entry,
    ) -> wasmtime::Result<Result<(), errors::HostError>> {
        let context = self.current()?;
        if !context.grants.logging
            || !self
                .manifest
                .requested_grants
                .contains(&plugin::PluginRequestedGrant::Logging)
        {
            return Ok(Err(host::error(
                errors::Code::Denied,
                "logging not granted",
            )));
        }
        let size = input
            .event_code
            .len()
            .saturating_add(input.message.len())
            .saturating_add(
                input
                    .fields
                    .iter()
                    .map(|f| {
                        f.name.len().saturating_add(match &f.value {
                            logging::Scalar::Text(s) => s.len(),
                            _ => 8,
                        })
                    })
                    .sum::<usize>(),
            );
        if size > self.limits.log_bytes.saturating_sub(self.log_bytes) {
            self.dropped_logs = self.dropped_logs.saturating_add(1);
            return Ok(Ok(()));
        }
        if let Err(e) = self.operation() {
            return Ok(Err(e));
        }
        if let Err(e) = self.provider.authorize(&context, Operation::Log).await {
            return Ok(Err(e));
        }
        check_current(&context).map_err(|_| wasmtime::Error::msg("invocation no longer active"))?;
        self.log_bytes += size;
        let result = self.provider.log(&context, input).await;
        check_current(&context).map_err(|_| wasmtime::Error::msg("late log result"))?;
        Ok(result)
    }
}
