//! Host-selected Plugin Tools use the ordinary Feature install transaction and
//! Agen execution/history path. This module neither selects a Plugin nor grants
//! access: callers bind exact persisted pins and current owner adapters before
//! controller publication. Verified bytes and compiled Components are not leases.

use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureId, FeatureInstallContext, FeatureInstallError,
    FeatureModule, FeatureRuntimeKind, HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    Hook, HookError, HookErrorCategory, HookExecutionPolicy, WorkerStopping, WorkerStoppingContext,
};
use agen::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOrigin, ToolOutput,
};
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;
use yoi_plugin_runtime::{
    CompiledTool, Executor, Limits,
    delivery::{ArtifactCache, ArtifactExpectation, ArtifactSource},
    host::{CancellationToken, Grants, Host, InvocationContext, Operation},
    owner::{OwnerHost, PluginHostOwner},
};

/// Upper bounds projected by the trusted launch owner from one persisted pin.
/// Never construct this from model input, current Profile, cache inventory, or
/// unvalidated delivery metadata. The expectation must match the persisted pin.
pub struct PluginToolPin {
    pub workspace_id: String,
    pub installation_id: String,
    pub plan_id: String,
    pub subject: String,
    pub request: server_api::PluginArtifactDeliveryRequest,
    pub expectation: ArtifactExpectation,
    pub tools: Vec<String>,
    pub config: Value,
    pub grants: Grants,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginPreparationError {
    #[error("invalid Plugin launch snapshot")]
    InvalidSnapshot,
    #[error("Plugin artifact delivery refused or unavailable")]
    Delivery,
    #[error("Plugin Component or execution policy is unsupported")]
    Component,
}

struct ActiveCall {
    cancellation: CancellationToken,
    host: Mutex<Option<Arc<OwnerHost>>>,
}
impl ActiveCall {
    async fn cancel(&self) -> Result<(), ToolError> {
        self.cancellation.cancel();
        let host = self.host.lock().expect("Plugin call lock").clone();
        if let Some(host) = host {
            host.cancel()
                .await
                .map_err(|_| failure("Plugin cancellation outcome requires reconciliation"))?;
        }
        Ok(())
    }
}
#[derive(Default)]
struct Calls {
    stopped: bool,
    active: BTreeMap<String, Arc<ActiveCall>>,
}
struct CallLease {
    key: String,
    state: Arc<Mutex<Calls>>,
    call: Arc<ActiveCall>,
}
impl Drop for CallLease {
    fn drop(&mut self) {
        self.call.cancellation.cancel();
        self.state
            .lock()
            .expect("Plugin calls lock")
            .active
            .remove(&self.key);
    }
}

/// Fully materialized Tool contributions. Preparation publishes nothing; the
/// existing Feature planner stages all names, schemas and hooks atomically.
#[derive(Clone)]
pub struct PluginToolsFeature {
    tools: Vec<(ToolMeta, Arc<PluginTool>)>,
    calls: Arc<Mutex<Calls>>,
}
impl PluginToolsFeature {
    pub async fn prepare(
        pins: Vec<PluginToolPin>,
        source: &dyn ArtifactSource,
        cache: &ArtifactCache,
        owner: Arc<dyn PluginHostOwner>,
        session_id: &str,
        limits: Limits,
    ) -> Result<Self, PluginPreparationError> {
        use PluginPreparationError::*;
        if session_id.is_empty() || pins.len() > 128 {
            return Err(InvalidSnapshot);
        }
        let executor = Arc::new(Executor::new(limits).map_err(|_| Component)?);
        let calls = Arc::new(Mutex::new(Calls::default()));
        let capacity = Arc::new(Semaphore::new(16));
        let mut names = BTreeSet::new();
        let mut installations = BTreeSet::new();
        let mut tools = Vec::new();
        for pin in pins {
            let descriptor = pin.expectation.descriptor();
            if pin.workspace_id.is_empty()
                || pin.installation_id.is_empty()
                || pin.plan_id.is_empty()
                || pin.plan_id != pin.request.plan_id
                || pin.installation_id != pin.request.installation_id
                || descriptor.artifact_digest != pin.request.artifact_digest
                || !pin.config.is_object()
                || !installations.insert((pin.workspace_id.clone(), pin.installation_id.clone()))
                || pin.tools.iter().collect::<BTreeSet<_>>().len() != pin.tools.len()
            {
                return Err(InvalidSnapshot);
            }
            let config_json = serde_json::to_string(&pin.config).map_err(|_| InvalidSnapshot)?;
            if config_json.len() > executor.limits().config_bytes {
                return Err(InvalidSnapshot);
            }
            let artifact = cache
                .resolve(source, &pin.workspace_id, &pin.request, &pin.expectation)
                .await
                .map_err(|_| Delivery)?;
            let compiled = Arc::new(
                executor
                    .compile(
                        artifact.bytes(),
                        &descriptor.package_digest,
                        &descriptor.artifact_digest,
                        u64::from(descriptor.artifact_size),
                    )
                    .map_err(|_| Component)?,
            );
            for name in &pin.tools {
                let declaration = compiled
                    .tools()
                    .iter()
                    .find(|tool| &tool.name == name)
                    .ok_or(InvalidSnapshot)?;
                let public_name = format!("{}__{}", pin.installation_id, name);
                if public_name.len() > 128 || !names.insert(public_name.clone()) {
                    return Err(InvalidSnapshot);
                }
                let meta = ToolMeta::new(public_name)
                    .description(declaration.description.clone())
                    .input_schema(declaration.input_schema.clone())
                    .origin(ToolOrigin {
                        kind: "plugin".into(),
                        plugin_id: descriptor.package_id.clone(),
                        plugin_ref: pin.installation_id.clone(),
                        source: "workspace".into(),
                        digest: descriptor.package_digest.clone(),
                        package_version: descriptor.version.clone(),
                        package_api_version: descriptor.schema_version,
                        surface: "tool".into(),
                    });
                let tool = Arc::new(PluginTool {
                    executor: executor.clone(),
                    compiled: compiled.clone(),
                    owner: owner.clone(),
                    workspace: pin.workspace_id.clone(),
                    installation: pin.installation_id.clone(),
                    plan: pin.plan_id.clone(),
                    subject: pin.subject.clone(),
                    grants: pin.grants.clone(),
                    tool_name: name.clone(),
                    config_json: config_json.clone(),
                    session_id: session_id.into(),
                    calls: calls.clone(),
                    capacity: capacity.clone(),
                });
                tools.push((meta, tool));
            }
        }
        Ok(Self { tools, calls })
    }
}
impl FeatureModule for PluginToolsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor =
            FeatureDescriptor::builtin("plugin-tools", "Workspace Plugin Tools").with_hook(
                HookDeclaration::new("stop", FeatureHookPoint::WorkerStopping),
            );
        descriptor.id = FeatureId::new("plugin:tools").expect("fixed Feature identity");
        descriptor.runtime = FeatureRuntimeKind::ExternalPlugin;
        for (meta, _) in &self.tools {
            descriptor
                .tools
                .push(ToolDeclaration::new(&meta.name, &meta.description));
        }
        descriptor
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for (meta, tool) in &self.tools {
            let meta = meta.clone();
            let tool = tool.clone();
            let name = meta.name.clone();
            let definition: ToolDefinition = Arc::new(move || (meta.clone(), tool.clone()));
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        context.hooks().add_worker_stopping(
            "stop",
            HookExecutionPolicy::fail_closed(),
            self.clone(),
        )
    }
}
#[async_trait]
impl Hook<WorkerStopping> for PluginToolsFeature {
    async fn call(&self, _: &WorkerStoppingContext) -> Result<(), HookError> {
        let active = {
            let mut calls = self.calls.lock().expect("Plugin calls lock");
            calls.stopped = true;
            calls.active.values().cloned().collect::<Vec<_>>()
        };
        let mut failed = false;
        for call in active {
            if call.cancel().await.is_err() {
                failed = true;
            }
        }
        if failed {
            return Err(HookError::new(
                HookErrorCategory::Dependency,
                "Plugin cancellation requires reconciliation",
            ));
        }
        Ok(())
    }
}
struct PluginTool {
    executor: Arc<Executor>,
    compiled: Arc<CompiledTool>,
    owner: Arc<dyn PluginHostOwner>,
    workspace: String,
    installation: String,
    plan: String,
    subject: String,
    grants: Grants,
    tool_name: String,
    config_json: String,
    session_id: String,
    calls: Arc<Mutex<Calls>>,
    capacity: Arc<Semaphore>,
}
fn failure(message: &str) -> ToolError {
    ToolError::Internal(message.into())
}
impl PluginTool {
    fn start(&self, context: &ToolExecutionContext) -> Result<CallLease, ToolError> {
        let key = context.execution_id();
        let mut calls = self.calls.lock().expect("Plugin calls lock");
        if calls.stopped || calls.active.contains_key(&key) {
            return Err(failure("Plugin Tool is stopping or already running"));
        }
        let call = Arc::new(ActiveCall {
            cancellation: CancellationToken::new(),
            host: Mutex::new(None),
        });
        calls.active.insert(key.clone(), call.clone());
        Ok(CallLease {
            key,
            state: self.calls.clone(),
            call,
        })
    }
}
#[async_trait]
impl Tool for PluginTool {
    async fn execute(
        &self,
        input_json: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        if context.call_id.is_empty() || context.call_id == "direct" {
            return Err(failure("Plugin Tool requires committed Tool-call identity"));
        }
        let _capacity = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure("Plugin invocation capacity exceeded"))?;
        let lease = self.start(&context)?;
        // The persisted Session and provider call ID survive Runtime restoration;
        // the engine-local batch ID identifies only cancellation of a live attempt.
        let id_bytes = serde_json::to_vec(&(&self.session_id, &context.call_id))
            .expect("identity serialization");
        let hex: String = Sha256::digest(&id_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let call_id = format!("tool-{hex}");
        let invocation = InvocationContext {
            workspace: self.workspace.clone(),
            installation: self.installation.clone(),
            plan: self.plan.clone(),
            subject: self.subject.clone(),
            invocation_id: String::new(),
            artifact: self.compiled.identity().clone(),
            tool_name: self.tool_name.clone(),
            grants: self.grants.clone(),
            deadline: tokio::time::Instant::now() + self.executor.limits().wall_time,
            cancellation: lease.call.cancellation.clone(),
        };
        let host = Arc::new(
            OwnerHost::begin(
                self.owner.clone(),
                invocation,
                server_api::PluginConsumer::ToolCall {
                    tool_call_id: call_id,
                    tool_name: self.tool_name.clone(),
                },
            )
            .await
            .map_err(|_| failure("Plugin live admission refused or unavailable"))?,
        );
        *lease.call.host.lock().expect("Plugin call lock") = Some(host.clone());
        if lease.call.cancellation.is_cancelled() {
            host.cancel()
                .await
                .map_err(|_| failure("Plugin cancellation requires reconciliation"))?;
            return Err(failure("Plugin invocation cancelled"));
        }
        let output = self
            .executor
            .invoke(
                &self.compiled,
                host.context().clone(),
                host.clone(),
                &self.tool_name,
                input_json,
                &self.config_json,
            )
            .await
            .map_err(|_| failure("Plugin invocation failed; no automatic retry"))?
            .output;
        // Fence again at the adapter publication boundary, not just on guest return.
        host.authorize(host.context(), Operation::Publish)
            .await
            .map_err(|_| failure("Plugin result publication refused"))?;
        let content = match output.structured_json {
            Some(structured) => Some(serde_json::json!({
                "content": output.content,
                "structured": serde_json::from_str::<Value>(&structured).map_err(|_| failure("Plugin output is invalid"))?,
            }).to_string()),
            None => output.content,
        };
        Ok(ToolOutput {
            summary: output.summary,
            content,
            attachments: Vec::new(),
        })
    }
    async fn cancel_execution(&self, context: &ToolExecutionContext) -> Result<(), ToolError> {
        let call = self
            .calls
            .lock()
            .expect("Plugin calls lock")
            .active
            .get(&context.execution_id())
            .cloned();
        if let Some(call) = call {
            call.cancel().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
