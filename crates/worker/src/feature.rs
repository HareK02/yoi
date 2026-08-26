//! Feature contribution registry for Worker-hosted builtin/plugin modules.
//!
//! This module defines the Worker-side feature boundary used to collect
//! descriptor metadata, tool contributions, safe hook contributions, background
//! task declarations, service declarations, and protocol-backed provider
//! startup discovery before installing them into the existing Engine/HookRegistry
//! host surfaces.
//!
//! The implementation is intentionally host-mediated: tools are installed through
//! the normal Engine tool path, hooks are installed through
//! [`crate::hook::HookRegistryBuilder`], and provider output is represented as
//! ordinary feature reports/diagnostics instead of a separate authority layer.

use std::any::{Any, type_name};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use agen::Engine;
use agen::llm_client::client::LlmClient;
use agen::state::Mutable;
use agen::tool::ToolDefinition;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hook::{Hook, HookRegistryBuilder, OnTurnEnd, PostToolCall, PreLlmRequest, PreToolCall};

/// Stable source-qualified identifier for a feature module.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeatureId(String);

impl FeatureId {
    pub fn new(value: impl Into<String>) -> Result<Self, FeatureInstallError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(FeatureInstallError::InvalidDescriptor(
                "feature id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn builtin(slug: impl AsRef<str>) -> Self {
        Self(format!("builtin:{}", slug.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<FeatureId> for String {
    fn from(value: FeatureId) -> Self {
        value.0
    }
}

/// Stable source-qualified identifier for a protocol-backed provider instance.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, FeatureInstallError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(FeatureInstallError::InvalidDescriptor(
                "provider id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn builtin(slug: impl AsRef<str>) -> Self {
        Self(format!("builtin:{}", slug.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Startup/lifecycle state for a protocol-backed provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolProviderLifecycleState {
    Starting,
    Ready,
    Degraded,
    Failed,
    Stopped,
}

impl ProtocolProviderLifecycleState {
    pub fn can_contribute(&self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

/// Approved protocol-backed provider declaration in a feature descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolProviderDeclaration {
    pub id: ProviderId,
    pub protocol: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
}

impl ProtocolProviderDeclaration {
    pub fn new(
        id: ProviderId,
        protocol: impl Into<String>,
        display_name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            id,
            protocol: protocol.into(),
            display_name: display_name.into(),
            version: version.into(),
            description: String::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Lifecycle diagnostic captured for a protocol-backed provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolProviderLifecycleDiagnostic {
    pub provider_id: ProviderId,
    pub state: ProtocolProviderLifecycleState,
    pub severity: FeatureDiagnosticSeverity,
    pub message: String,
}

impl ProtocolProviderLifecycleDiagnostic {
    pub fn new(
        provider_id: ProviderId,
        state: ProtocolProviderLifecycleState,
        severity: FeatureDiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider_id,
            state,
            severity,
            message: message.into(),
        }
    }
}

/// Startup-discovered contribution set returned by a protocol-backed provider.
///
/// Tool definitions are materialized exactly once when registered, then inserted
/// into the normal Engine tool path as stable metadata plus executable tool
/// handles for the remainder of the run. Execution still flows through the
/// Engine, permission, history, and bounded-result machinery.
#[derive(Clone)]
pub struct ProtocolProviderContribution {
    declaration: ProtocolProviderDeclaration,
    state: ProtocolProviderLifecycleState,
    tools: Vec<ToolContribution>,
    services: Vec<ServiceDeclaration>,
    background_tasks: Vec<BackgroundTaskDeclaration>,
    diagnostics: Vec<FeatureDiagnostic>,
}

impl ProtocolProviderContribution {
    pub fn new(
        declaration: ProtocolProviderDeclaration,
        state: ProtocolProviderLifecycleState,
    ) -> Self {
        Self {
            declaration,
            state,
            tools: Vec::new(),
            services: Vec::new(),
            background_tasks: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn ready(declaration: ProtocolProviderDeclaration) -> Self {
        Self::new(declaration, ProtocolProviderLifecycleState::Ready)
    }

    pub fn failed(declaration: ProtocolProviderDeclaration, message: impl Into<String>) -> Self {
        Self::new(declaration.clone(), ProtocolProviderLifecycleState::Failed).with_diagnostic(
            FeatureDiagnostic::error(format!(
                "provider {} failed during startup: {}",
                declaration.id,
                message.into()
            )),
        )
    }

    pub fn provider(&self) -> &ProtocolProviderDeclaration {
        &self.declaration
    }

    pub fn state(&self) -> &ProtocolProviderLifecycleState {
        &self.state
    }

    pub fn with_tool(mut self, tool: ToolContribution) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_service(mut self, service: ServiceDeclaration) -> Self {
        self.services.push(service);
        self
    }

    pub fn with_background_task(mut self, task: BackgroundTaskDeclaration) -> Self {
        self.background_tasks.push(task);
        self
    }

    pub fn with_diagnostic(mut self, diagnostic: FeatureDiagnostic) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }
}

/// Runtime/source class for a feature module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureRuntimeKind {
    Builtin,
    Profile,
    ExternalPlugin,
    ProtocolProvider,
}

/// A safe hook contribution point exposed to feature modules.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureHookPoint {
    PreRequest,
    PreToolCall,
    ToolResult,
    TurnEnd,
}

/// Serializable declaration of a tool contribution. The executable factory is
/// carried by [`ToolContribution`] during installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
}

impl ToolDeclaration {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Executable tool contribution wrapper.
#[derive(Clone)]
pub struct ToolContribution {
    name: String,
    definition: ToolDefinition,
}

impl ToolContribution {
    pub fn new(name: impl Into<String>, definition: ToolDefinition) -> Self {
        Self {
            name: name.into(),
            definition,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Serializable declaration of a hook contribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDeclaration {
    pub name: String,
    pub point: FeatureHookPoint,
}

impl HookDeclaration {
    pub fn new(name: impl Into<String>, point: FeatureHookPoint) -> Self {
        Self {
            name: name.into(),
            point,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeatureInstructionId(String);

impl FeatureInstructionId {
    pub fn new(value: impl Into<String>) -> Result<Self, FeatureInstallError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(FeatureInstallError::InvalidDescriptor(
                "feature instruction id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn builtin(slug: impl AsRef<str>) -> Self {
        Self(format!("builtin:{}", slug.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FeatureInstructionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureInstructionDeclaration {
    pub id: FeatureInstructionId,
    pub prompt_ref: String,
    pub description: String,
}

impl FeatureInstructionDeclaration {
    pub fn new(
        id: FeatureInstructionId,
        prompt_ref: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, FeatureInstallError> {
        let prompt_ref = prompt_ref.into();
        if prompt_ref.trim().is_empty() {
            return Err(FeatureInstallError::InvalidDescriptor(
                "feature instruction prompt_ref must not be empty".into(),
            ));
        }
        Ok(Self {
            id,
            prompt_ref,
            description: description.into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureInstructionContribution {
    pub declaration: FeatureInstructionDeclaration,
}

impl FeatureInstructionContribution {
    pub fn new(declaration: FeatureInstructionDeclaration) -> Self {
        Self { declaration }
    }
}

/// Background task lifecycle phase represented by this registry slice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskLifecycle {
    DescriptorOnly,
    HostManaged,
}

/// Declaration for a feature-provided background task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTaskDeclaration {
    pub name: String,
    pub description: String,
    pub lifecycle: BackgroundTaskLifecycle,
}

impl BackgroundTaskDeclaration {
    pub fn descriptor_only(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            lifecycle: BackgroundTaskLifecycle::DescriptorOnly,
        }
    }
}

/// Source-qualified service identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ServiceId(String);

impl ServiceId {
    pub fn new(value: impl Into<String>) -> Result<Self, FeatureInstallError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(FeatureInstallError::InvalidDescriptor(
                "service id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn builtin(slug: impl AsRef<str>) -> Self {
        Self(format!("builtin:{}", slug.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Minimal version requirement placeholder for service resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceVersionReq {
    pub requirement: String,
}

impl ServiceVersionReq {
    pub fn any() -> Self {
        Self {
            requirement: "*".into(),
        }
    }

    pub fn exact(version: impl Into<String>) -> Self {
        Self {
            requirement: version.into(),
        }
    }

    fn matches(&self, provider_version: &str) -> bool {
        self.requirement == "*" || self.requirement == provider_version
    }
}

/// Feature-provided service declaration. This first slice records provider
/// metadata and supports requirement matching; it does not expose concrete
/// provider objects across feature boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDeclaration {
    pub id: ServiceId,
    pub version: String,
    pub description: String,
}

impl ServiceDeclaration {
    pub fn new(id: ServiceId, version: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id,
            version: version.into(),
            description: description.into(),
        }
    }
}

/// Feature service requirement used for contribution dependency resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRequirement {
    pub id: ServiceId,
    pub version: ServiceVersionReq,
    pub required: bool,
    pub reason: String,
}

impl ServiceRequirement {
    pub fn required(id: ServiceId, reason: impl Into<String>) -> Self {
        Self {
            id,
            version: ServiceVersionReq::any(),
            required: true,
            reason: reason.into(),
        }
    }

    pub fn optional(id: ServiceId, reason: impl Into<String>) -> Self {
        Self {
            id,
            version: ServiceVersionReq::any(),
            required: false,
            reason: reason.into(),
        }
    }
}

/// Typed concrete services installed by stateful Feature modules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureServiceRegistry {
    providers: HashMap<ServiceId, FeatureServiceProvider>,
}

impl FeatureServiceRegistry {
    pub fn providers(&self) -> &HashMap<ServiceId, FeatureServiceProvider> {
        &self.providers
    }

    pub fn provides(&self, id: &ServiceId) -> bool {
        self.providers.contains_key(id)
    }

    pub fn service<T>(&self, id: &ServiceId) -> Result<Arc<T>, FeatureInstallError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let provider = self.providers.get(id).ok_or_else(|| {
            FeatureInstallError::InvalidDescriptor(format!(
                "feature service provider is unavailable: {id}"
            ))
        })?;
        provider.service::<T>().ok_or_else(|| {
            FeatureInstallError::InvalidDescriptor(format!(
                "feature service {id} has incompatible concrete type; requested {}",
                type_name::<T>()
            ))
        })
    }

    fn register_provider<T>(
        &mut self,
        feature_id: FeatureId,
        declaration: ServiceDeclaration,
        instance: Arc<T>,
    ) -> Result<(), FeatureInstallError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        if let Some(existing) = self.providers.get(&declaration.id) {
            return Err(FeatureInstallError::DuplicateService {
                service: declaration.id.to_string(),
                first_feature: existing.feature_id.to_string(),
                duplicate_feature: feature_id.to_string(),
            });
        }
        self.providers.insert(
            declaration.id.clone(),
            FeatureServiceProvider {
                feature_id,
                declaration,
                instance: Arc::new(instance),
            },
        );
        Ok(())
    }
}

/// Provider metadata and concrete instance for one service declaration.
#[derive(Clone, Debug)]
pub struct FeatureServiceProvider {
    pub feature_id: FeatureId,
    pub declaration: ServiceDeclaration,
    instance: Arc<dyn Any + Send + Sync>,
}

impl FeatureServiceProvider {
    fn service<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.instance.downcast_ref::<Arc<T>>().map(Arc::clone)
    }
}

impl PartialEq for FeatureServiceProvider {
    fn eq(&self, other: &Self) -> bool {
        self.feature_id == other.feature_id && self.declaration == other.declaration
    }
}

impl Eq for FeatureServiceProvider {}

/// Feature descriptor advertised before installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureDescriptor {
    pub id: FeatureId,
    pub runtime: FeatureRuntimeKind,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub tools: Vec<ToolDeclaration>,
    pub hooks: Vec<HookDeclaration>,
    pub instructions: Vec<FeatureInstructionDeclaration>,
    pub background_tasks: Vec<BackgroundTaskDeclaration>,
    pub provides_services: Vec<ServiceDeclaration>,
    pub requires_services: Vec<ServiceRequirement>,
    pub protocol_providers: Vec<ProtocolProviderDeclaration>,
}

impl FeatureDescriptor {
    pub fn builtin(id: impl AsRef<str>, display_name: impl Into<String>) -> Self {
        Self {
            id: FeatureId::builtin(id),
            runtime: FeatureRuntimeKind::Builtin,
            display_name: display_name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: String::new(),
            tools: Vec::new(),
            hooks: Vec::new(),
            instructions: Vec::new(),
            background_tasks: Vec::new(),
            provides_services: Vec::new(),
            requires_services: Vec::new(),
            protocol_providers: Vec::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_tool(mut self, tool: ToolDeclaration) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_hook(mut self, hook: HookDeclaration) -> Self {
        self.hooks.push(hook);
        self
    }

    pub fn with_instruction(mut self, instruction: FeatureInstructionDeclaration) -> Self {
        self.instructions.push(instruction);
        self
    }

    pub fn with_background_task(mut self, task: BackgroundTaskDeclaration) -> Self {
        self.background_tasks.push(task);
        self
    }

    pub fn with_provided_service(mut self, service: ServiceDeclaration) -> Self {
        self.provides_services.push(service);
        self
    }

    pub fn with_service_requirement(mut self, requirement: ServiceRequirement) -> Self {
        self.requires_services.push(requirement);
        self
    }

    pub fn with_protocol_provider(mut self, provider: ProtocolProviderDeclaration) -> Self {
        self.protocol_providers.push(provider);
        self
    }
}

/// Feature module contribution boundary.
pub trait FeatureModule: Send + Sync {
    fn descriptor(&self) -> FeatureDescriptor;
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError>;
}

/// Severity for feature installation diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

/// Installation diagnostic emitted by the feature host or feature module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureDiagnostic {
    pub severity: FeatureDiagnosticSeverity,
    pub message: String,
}

impl FeatureDiagnostic {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: FeatureDiagnosticSeverity::Info,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: FeatureDiagnosticSeverity::Warning,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: FeatureDiagnosticSeverity::Error,
            message: message.into(),
        }
    }
}

/// Kind of contribution represented in install reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureContributionKind {
    Tool,
    Hook,
    Instruction,
    BackgroundTask,
    Service,
    ProtocolProvider,
    Notification,
    Alert,
    Diagnostic,
}

/// A contribution intentionally skipped by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedContribution {
    pub kind: FeatureContributionKind,
    pub name: String,
    pub reason: String,
}

/// Per-feature installation report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureInstallReport {
    pub feature_id: FeatureId,
    pub runtime: FeatureRuntimeKind,
    pub installed: bool,
    pub installed_tools: Vec<String>,
    pub installed_hooks: Vec<HookDeclaration>,
    pub installed_instructions: Vec<FeatureInstructionDeclaration>,
    pub declared_background_tasks: Vec<BackgroundTaskDeclaration>,
    pub provided_services: Vec<ServiceDeclaration>,
    pub resolved_service_requirements: Vec<ServiceRequirement>,
    pub protocol_providers: Vec<ProtocolProviderLifecycleDiagnostic>,
    pub skipped: Vec<SkippedContribution>,
    pub diagnostics: Vec<FeatureDiagnostic>,
}

impl FeatureInstallReport {
    fn new(descriptor: &FeatureDescriptor) -> Self {
        Self {
            feature_id: descriptor.id.clone(),
            runtime: descriptor.runtime.clone(),
            installed: false,
            installed_tools: Vec::new(),
            installed_hooks: Vec::new(),
            installed_instructions: Vec::new(),
            declared_background_tasks: Vec::new(),
            provided_services: Vec::new(),
            resolved_service_requirements: Vec::new(),
            protocol_providers: Vec::new(),
            skipped: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn mark_skipped(
        &mut self,
        kind: FeatureContributionKind,
        name: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.skipped.push(SkippedContribution {
            kind,
            name: name.into(),
            reason: reason.into(),
        });
    }
}

#[derive(Clone, Debug)]
struct FeatureContributionDeclarations {
    tools: HashSet<String>,
    hooks: HashSet<(String, FeatureHookPoint)>,
    instructions: HashSet<FeatureInstructionId>,
    background_tasks: HashSet<String>,
    provided_services: HashSet<(ServiceId, String)>,
    required_services: HashSet<ServiceId>,
    protocol_providers: HashSet<ProviderId>,
}

impl FeatureContributionDeclarations {
    fn from_descriptor(descriptor: &FeatureDescriptor) -> Self {
        Self {
            tools: descriptor
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
            hooks: descriptor
                .hooks
                .iter()
                .map(|hook| (hook.name.clone(), hook.point.clone()))
                .collect(),
            instructions: descriptor
                .instructions
                .iter()
                .map(|instruction| instruction.id.clone())
                .collect(),
            background_tasks: descriptor
                .background_tasks
                .iter()
                .map(|task| task.name.clone())
                .collect(),
            provided_services: descriptor
                .provides_services
                .iter()
                .map(|service| (service.id.clone(), service.version.clone()))
                .collect(),
            required_services: descriptor
                .requires_services
                .iter()
                .map(|service| service.id.clone())
                .collect(),
            protocol_providers: descriptor
                .protocol_providers
                .iter()
                .map(|provider| provider.id.clone())
                .collect(),
        }
    }

    fn contains_tool(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    fn contains_hook(&self, declaration: &HookDeclaration) -> bool {
        self.hooks
            .contains(&(declaration.name.clone(), declaration.point.clone()))
    }

    fn contains_instruction(&self, declaration: &FeatureInstructionDeclaration) -> bool {
        self.instructions.contains(&declaration.id)
    }

    fn contains_background_task(&self, declaration: &BackgroundTaskDeclaration) -> bool {
        self.background_tasks.contains(&declaration.name)
    }

    fn contains_provided_service(&self, declaration: &ServiceDeclaration) -> bool {
        self.provided_services
            .contains(&(declaration.id.clone(), declaration.version.clone()))
    }

    fn contains_protocol_provider(&self, declaration: &ProtocolProviderDeclaration) -> bool {
        self.protocol_providers.contains(&declaration.id)
    }
}

fn reject_undeclared_contribution(
    feature_id: &FeatureId,
    report: &mut FeatureInstallReport,
    kind: FeatureContributionKind,
    name: impl Into<String>,
) -> FeatureInstallError {
    let name = name.into();
    let error = FeatureInstallError::UndeclaredContribution {
        kind: kind.clone(),
        name: name.clone(),
        feature: feature_id.to_string(),
    };
    report.mark_skipped(kind, name, error.to_string());
    error
}

/// Model-visible durable notification sink skeleton. The first slice exposes
/// the boundary without implementing a new event channel.
pub struct FeatureNotificationSink<'a> {
    report: &'a mut FeatureInstallReport,
}

impl FeatureNotificationSink<'_> {
    pub fn notify_model(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        let message = message.into();
        self.report.diagnostics.push(FeatureDiagnostic::warning(format!(
            "model notification requested during feature installation but no durable Notify host is attached: {message}"
        )));
        self.report.mark_skipped(
            FeatureContributionKind::Notification,
            "notify_model",
            "durable Notify/SystemItem host is not connected during feature installation",
        );
        Ok(())
    }
}

/// Transient human-facing alert sink skeleton.
pub struct FeatureAlertSink<'a> {
    report: &'a mut FeatureInstallReport,
}

impl FeatureAlertSink<'_> {
    pub fn alert(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.report
            .diagnostics
            .push(FeatureDiagnostic::info(format!("feature alert: {message}")));
        self.report.mark_skipped(
            FeatureContributionKind::Alert,
            "alert",
            "transient alert host is not connected during feature installation",
        );
    }
}

/// Diagnostic sink available to feature installers.
pub struct FeatureDiagnosticSink<'a> {
    report: &'a mut FeatureInstallReport,
}

impl FeatureDiagnosticSink<'_> {
    pub fn push(&mut self, diagnostic: FeatureDiagnostic) {
        self.report.diagnostics.push(diagnostic);
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(FeatureDiagnostic::info(message));
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.push(FeatureDiagnostic::warning(message));
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(FeatureDiagnostic::error(message));
    }
}

fn register_tool_contribution(
    feature_id: &FeatureId,
    report: &mut FeatureInstallReport,
    pending_tools: &mut Vec<ToolDefinition>,
    installed_tool_names: &mut HashMap<String, FeatureId>,
    contribution: ToolContribution,
    require_declared: impl FnOnce(&str) -> bool,
) -> Result<(), FeatureInstallError> {
    let (tool_meta, tool) = (contribution.definition)();
    let model_visible_name = tool_meta.name.clone();
    if contribution.name != model_visible_name {
        let error = FeatureInstallError::ToolNameMismatch {
            declared: contribution.name,
            model_visible: model_visible_name.clone(),
        };
        report.mark_skipped(
            FeatureContributionKind::Tool,
            model_visible_name,
            error.to_string(),
        );
        return Err(error);
    }

    if !require_declared(&model_visible_name) {
        return Err(reject_undeclared_contribution(
            feature_id,
            report,
            FeatureContributionKind::Tool,
            model_visible_name,
        ));
    }

    if let Some(first) = installed_tool_names.get(&model_visible_name) {
        let error = FeatureInstallError::DuplicateToolName {
            tool: model_visible_name.clone(),
            first_feature: first.to_string(),
            duplicate_feature: feature_id.to_string(),
        };
        report.mark_skipped(
            FeatureContributionKind::Tool,
            model_visible_name,
            error.to_string(),
        );
        return Err(error);
    }

    installed_tool_names.insert(model_visible_name.clone(), feature_id.clone());
    report.installed_tools.push(model_visible_name);
    pending_tools.push(Arc::new(move || (tool_meta.clone(), Arc::clone(&tool))));
    Ok(())
}

/// Tool contribution registrar exposed inside [`FeatureInstallContext`].
pub struct ToolContributionRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    pending_tools: &'a mut Vec<ToolDefinition>,
    installed_tool_names: &'a mut HashMap<String, FeatureId>,
    report: &'a mut FeatureInstallReport,
}

impl ToolContributionRegistrar<'_> {
    pub fn register(&mut self, contribution: ToolContribution) -> Result<(), FeatureInstallError> {
        register_tool_contribution(
            self.feature_id,
            self.report,
            self.pending_tools,
            self.installed_tool_names,
            contribution,
            |model_visible_name| self.declarations.contains_tool(model_visible_name),
        )
    }
}

/// Safe hook contribution registrar backed by [`HookRegistryBuilder`].
pub struct HookContributionRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    hook_builder: &'a mut HookRegistryBuilder,
    report: &'a mut FeatureInstallReport,
}

impl HookContributionRegistrar<'_> {
    fn require_declared(
        &mut self,
        declaration: &HookDeclaration,
    ) -> Result<(), FeatureInstallError> {
        if self.declarations.contains_hook(declaration) {
            return Ok(());
        }
        Err(reject_undeclared_contribution(
            self.feature_id,
            self.report,
            FeatureContributionKind::Hook,
            format!("{}:{:?}", declaration.name, declaration.point),
        ))
    }

    pub fn add_pre_request(
        &mut self,
        name: impl Into<String>,
        hook: impl Hook<PreLlmRequest> + 'static,
    ) -> Result<(), FeatureInstallError> {
        let declaration = HookDeclaration::new(name, FeatureHookPoint::PreRequest);
        self.require_declared(&declaration)?;
        self.hook_builder.add_pre_llm_request(hook);
        self.report.installed_hooks.push(declaration);
        Ok(())
    }

    pub fn add_pre_tool_call(
        &mut self,
        name: impl Into<String>,
        hook: impl Hook<PreToolCall> + 'static,
    ) -> Result<(), FeatureInstallError> {
        let declaration = HookDeclaration::new(name, FeatureHookPoint::PreToolCall);
        self.require_declared(&declaration)?;
        self.hook_builder.add_pre_tool_call(hook);
        self.report.installed_hooks.push(declaration);
        Ok(())
    }

    pub fn add_tool_result(
        &mut self,
        name: impl Into<String>,
        hook: impl Hook<PostToolCall> + 'static,
    ) -> Result<(), FeatureInstallError> {
        let declaration = HookDeclaration::new(name, FeatureHookPoint::ToolResult);
        self.require_declared(&declaration)?;
        self.hook_builder.add_post_tool_call(hook);
        self.report.installed_hooks.push(declaration);
        Ok(())
    }

    pub fn add_turn_end(
        &mut self,
        name: impl Into<String>,
        hook: impl Hook<OnTurnEnd> + 'static,
    ) -> Result<(), FeatureInstallError> {
        let declaration = HookDeclaration::new(name, FeatureHookPoint::TurnEnd);
        self.require_declared(&declaration)?;
        self.hook_builder.add_on_turn_end(hook);
        self.report.installed_hooks.push(declaration);
        Ok(())
    }
}

/// Prompt instruction registrar for mandatory feature guidance contributions.
pub struct FeatureInstructionRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    report: &'a mut FeatureInstallReport,
}

impl FeatureInstructionRegistrar<'_> {
    pub fn register(
        &mut self,
        contribution: FeatureInstructionContribution,
    ) -> Result<(), FeatureInstallError> {
        let declaration = contribution.declaration;
        if !self.declarations.contains_instruction(&declaration) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::Instruction,
                declaration.id.to_string(),
            ));
        }
        if !self
            .report
            .installed_instructions
            .iter()
            .any(|instruction| instruction.id == declaration.id)
        {
            self.report.installed_instructions.push(declaration);
        }
        Ok(())
    }
}

/// Background task registrar for descriptor/report-only contributions.
pub struct BackgroundTaskRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    report: &'a mut FeatureInstallReport,
}

impl BackgroundTaskRegistrar<'_> {
    pub fn declare(
        &mut self,
        declaration: BackgroundTaskDeclaration,
    ) -> Result<(), FeatureInstallError> {
        if !self.declarations.contains_background_task(&declaration) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::BackgroundTask,
                declaration.name,
            ));
        }
        if !self
            .report
            .declared_background_tasks
            .iter()
            .any(|task| task.name == declaration.name)
        {
            self.report.declared_background_tasks.push(declaration);
        }
        Ok(())
    }
}

/// Service registrar for concrete typed Feature state.
pub struct FeatureServiceRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    service_registry: &'a mut FeatureServiceRegistry,
    report: &'a mut FeatureInstallReport,
}

impl FeatureServiceRegistrar<'_> {
    pub fn provide<T>(
        &mut self,
        declaration: ServiceDeclaration,
        instance: Arc<T>,
    ) -> Result<(), FeatureInstallError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        if !self.declarations.contains_provided_service(&declaration) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::Service,
                declaration.id.to_string(),
            ));
        }
        self.service_registry.register_provider(
            self.feature_id.clone(),
            declaration.clone(),
            instance,
        )?;
        self.report.provided_services.push(declaration);
        Ok(())
    }

    pub fn require<T>(&self, service: &ServiceId) -> Result<Arc<T>, FeatureInstallError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        if !self.declarations.required_services.contains(service) {
            return Err(FeatureInstallError::UndeclaredContribution {
                kind: FeatureContributionKind::Service,
                name: service.to_string(),
                feature: self.feature_id.to_string(),
            });
        }
        self.service_registry.service(service)
    }
}

/// Registrar for startup-discovered protocol-backed provider contributions.
pub struct ProtocolProviderRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    pending_tools: &'a mut Vec<ToolDefinition>,
    installed_tool_names: &'a mut HashMap<String, FeatureId>,
    service_registry: &'a mut FeatureServiceRegistry,
    report: &'a mut FeatureInstallReport,
}

impl ProtocolProviderRegistrar<'_> {
    pub fn register(
        &mut self,
        contribution: ProtocolProviderContribution,
    ) -> Result<(), FeatureInstallError> {
        let ProtocolProviderContribution {
            declaration,
            state,
            tools,
            services,
            background_tasks,
            diagnostics,
        } = contribution;

        if !self.declarations.contains_protocol_provider(&declaration) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::ProtocolProvider,
                declaration.id.to_string(),
            ));
        }

        if self
            .report
            .protocol_providers
            .iter()
            .any(|provider| provider.provider_id == declaration.id)
        {
            let reason = format!(
                "duplicate protocol provider contribution: {}",
                declaration.id
            );
            let error = FeatureInstallError::InvalidDescriptor(reason.clone());
            self.report.mark_skipped(
                FeatureContributionKind::ProtocolProvider,
                declaration.id.to_string(),
                reason,
            );
            return Err(error);
        }

        for diagnostic in diagnostics {
            self.report.diagnostics.push(FeatureDiagnostic {
                severity: diagnostic.severity.clone(),
                message: format!("provider {}: {}", declaration.id, diagnostic.message),
            });
        }

        self.report
            .protocol_providers
            .push(ProtocolProviderLifecycleDiagnostic::new(
                declaration.id.clone(),
                state.clone(),
                if state.can_contribute() {
                    FeatureDiagnosticSeverity::Info
                } else {
                    FeatureDiagnosticSeverity::Error
                },
                format!(
                    "protocol provider {} ({}) is {:?}",
                    declaration.display_name, declaration.protocol, state
                ),
            ));

        if !state.can_contribute() {
            let reason = format!("protocol provider is not available: {:?}", state);
            for tool in tools {
                self.report
                    .mark_skipped(FeatureContributionKind::Tool, tool.name, reason.clone());
            }
            for service in services {
                self.report.mark_skipped(
                    FeatureContributionKind::Service,
                    service.id.to_string(),
                    reason.clone(),
                );
            }
            for task in background_tasks {
                self.report.mark_skipped(
                    FeatureContributionKind::BackgroundTask,
                    task.name,
                    reason.clone(),
                );
            }
            return Ok(());
        }

        for tool in tools {
            register_tool_contribution(
                self.feature_id,
                self.report,
                self.pending_tools,
                self.installed_tool_names,
                tool,
                |_| true,
            )?;
        }

        for service in services {
            if !self
                .report
                .provided_services
                .iter()
                .any(|provided| provided.id == service.id && provided.version == service.version)
            {
                self.service_registry.register_provider(
                    self.feature_id.clone(),
                    service.clone(),
                    Arc::new(()),
                )?;
                self.report.provided_services.push(service);
            }
        }

        for task in background_tasks {
            if !self
                .report
                .declared_background_tasks
                .iter()
                .any(|declared| declared.name == task.name)
            {
                self.report.declared_background_tasks.push(task);
            }
        }

        Ok(())
    }
}

/// Install-time context provided to a feature module.
pub struct FeatureInstallContext<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    pending_tools: &'a mut Vec<ToolDefinition>,
    installed_tool_names: &'a mut HashMap<String, FeatureId>,
    hook_builder: &'a mut HookRegistryBuilder,
    service_registry: &'a mut FeatureServiceRegistry,
    report: &'a mut FeatureInstallReport,
}

impl FeatureInstallContext<'_> {
    pub fn feature_id(&self) -> &FeatureId {
        self.feature_id
    }

    pub fn tools(&mut self) -> ToolContributionRegistrar<'_> {
        ToolContributionRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            pending_tools: self.pending_tools,
            installed_tool_names: self.installed_tool_names,
            report: self.report,
        }
    }

    pub fn hooks(&mut self) -> HookContributionRegistrar<'_> {
        HookContributionRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            hook_builder: self.hook_builder,
            report: self.report,
        }
    }

    pub fn instructions(&mut self) -> FeatureInstructionRegistrar<'_> {
        FeatureInstructionRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            report: self.report,
        }
    }

    pub fn background_tasks(&mut self) -> BackgroundTaskRegistrar<'_> {
        BackgroundTaskRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            report: self.report,
        }
    }

    pub fn services(&mut self) -> FeatureServiceRegistrar<'_> {
        FeatureServiceRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            service_registry: self.service_registry,
            report: self.report,
        }
    }

    pub fn protocol_providers(&mut self) -> ProtocolProviderRegistrar<'_> {
        ProtocolProviderRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            pending_tools: self.pending_tools,
            installed_tool_names: self.installed_tool_names,
            service_registry: self.service_registry,
            report: self.report,
        }
    }

    pub fn notifications(&mut self) -> FeatureNotificationSink<'_> {
        FeatureNotificationSink {
            report: self.report,
        }
    }

    pub fn alerts(&mut self) -> FeatureAlertSink<'_> {
        FeatureAlertSink {
            report: self.report,
        }
    }

    pub fn diagnostics(&mut self) -> FeatureDiagnosticSink<'_> {
        FeatureDiagnosticSink {
            report: self.report,
        }
    }
}

/// Aggregate install output for a registry installation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureRegistryInstallReport {
    pub reports: Vec<FeatureInstallReport>,
    pub services: FeatureServiceRegistry,
    pub plan_error: Option<FeaturePlanError>,
}

impl FeatureRegistryInstallReport {
    pub fn has_errors(&self) -> bool {
        self.plan_error.is_some()
            || self.reports.iter().any(|report| {
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == FeatureDiagnosticSeverity::Error)
            })
    }

    pub fn error_message(&self) -> String {
        let mut errors = self
            .plan_error
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        errors.extend(self.reports.iter().flat_map(|report| {
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == FeatureDiagnosticSeverity::Error)
                .map(move |diagnostic| format!("{}: {}", report.feature_id, diagnostic.message))
        }));
        errors.join("; ")
    }

    pub fn installed_tool_names(&self) -> Vec<String> {
        self.reports
            .iter()
            .flat_map(|report| report.installed_tools.iter().cloned())
            .collect()
    }

    pub fn installed_instruction_contributions(&self) -> Vec<FeatureInstructionDeclaration> {
        dedupe_instruction_contributions(
            self.reports
                .iter()
                .flat_map(|report| report.installed_instructions.iter().cloned()),
        )
    }
}

pub fn dedupe_instruction_contributions(
    instructions: impl IntoIterator<Item = FeatureInstructionDeclaration>,
) -> Vec<FeatureInstructionDeclaration> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for instruction in instructions {
        if seen.insert(instruction.id.clone()) {
            deduped.push(instruction);
        }
    }
    deduped
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedServiceProvider {
    pub service: ServiceId,
    pub provider: FeatureId,
    pub version: String,
}

/// A validated, deterministic installation order and its selected service providers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureInstallPlan {
    ordered_features: Vec<FeatureId>,
    service_providers: BTreeMap<ServiceId, PlannedServiceProvider>,
    ordered_indices: Vec<usize>,
}

impl FeatureInstallPlan {
    pub fn ordered_features(&self) -> &[FeatureId] {
        &self.ordered_features
    }

    pub fn service_providers(&self) -> &BTreeMap<ServiceId, PlannedServiceProvider> {
        &self.service_providers
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum FeaturePlanError {
    #[error("feature `{feature}` is registered more than once")]
    DuplicateFeature { feature: FeatureId },
    #[error("service `{service}` has multiple selected providers: {providers:?}")]
    AmbiguousServiceProvider {
        service: ServiceId,
        providers: Vec<FeatureId>,
    },
    #[error("feature `{consumer}` requires service `{service}`, but no provider is selected")]
    MissingServiceProvider {
        consumer: FeatureId,
        service: ServiceId,
    },
    #[error(
        "feature `{consumer}` requires service `{service}` version `{requirement}`, but provider `{provider}` provides `{provider_version}`"
    )]
    ServiceVersionMismatch {
        consumer: FeatureId,
        service: ServiceId,
        requirement: String,
        provider: FeatureId,
        provider_version: String,
    },
    #[error(
        "service dependency cycle involves features {features:?} through services {services:?}"
    )]
    ServiceDependencyCycle {
        features: Vec<FeatureId>,
        services: Vec<ServiceId>,
    },
}

/// Builder/installer for enabled feature modules.
pub struct FeatureRegistryBuilder {
    modules: Vec<Arc<dyn FeatureModule>>,
}

fn build_feature_install_plan(
    descriptors: &[FeatureDescriptor],
) -> Result<FeatureInstallPlan, FeaturePlanError> {
    let mut feature_ids = BTreeSet::new();
    for descriptor in descriptors {
        if !feature_ids.insert(descriptor.id.clone()) {
            return Err(FeaturePlanError::DuplicateFeature {
                feature: descriptor.id.clone(),
            });
        }
    }

    let mut provider_candidates: BTreeMap<ServiceId, Vec<(usize, ServiceDeclaration)>> =
        BTreeMap::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        for service in &descriptor.provides_services {
            provider_candidates
                .entry(service.id.clone())
                .or_default()
                .push((index, service.clone()));
        }
    }

    let mut selected_providers = BTreeMap::new();
    for (service, candidates) in provider_candidates {
        if candidates.len() > 1 {
            let mut providers = candidates
                .iter()
                .map(|(index, _)| descriptors[*index].id.clone())
                .collect::<Vec<_>>();
            providers.sort();
            return Err(FeaturePlanError::AmbiguousServiceProvider { service, providers });
        }
        let (provider_index, declaration) = candidates
            .into_iter()
            .next()
            .expect("provider candidates are non-empty");
        selected_providers.insert(service, (provider_index, declaration));
    }

    let active = (0..descriptors.len()).collect::<BTreeSet<_>>();
    let mut adjacency = vec![BTreeSet::new(); descriptors.len()];
    let mut indegree = vec![0usize; descriptors.len()];
    let mut edge_services: BTreeMap<(usize, usize), BTreeSet<ServiceId>> = BTreeMap::new();

    for &consumer_index in &active {
        let consumer = &descriptors[consumer_index];
        for requirement in &consumer.requires_services {
            let provider = selected_providers.get(&requirement.id);
            let Some((provider_index, declaration)) = provider else {
                if requirement.required {
                    return Err(FeaturePlanError::MissingServiceProvider {
                        consumer: consumer.id.clone(),
                        service: requirement.id.clone(),
                    });
                }
                continue;
            };
            if !requirement.version.matches(&declaration.version) {
                return Err(FeaturePlanError::ServiceVersionMismatch {
                    consumer: consumer.id.clone(),
                    service: requirement.id.clone(),
                    requirement: requirement.version.requirement.clone(),
                    provider: descriptors[*provider_index].id.clone(),
                    provider_version: declaration.version.clone(),
                });
            }
            if adjacency[*provider_index].insert(consumer_index) {
                indegree[consumer_index] += 1;
            }
            edge_services
                .entry((*provider_index, consumer_index))
                .or_default()
                .insert(requirement.id.clone());
        }
    }

    let mut ready = active
        .iter()
        .copied()
        .filter(|index| indegree[*index] == 0)
        .collect::<BTreeSet<_>>();
    let mut ordered_indices = Vec::with_capacity(active.len());
    while let Some(index) = ready.pop_first() {
        ordered_indices.push(index);
        for &dependent in &adjacency[index] {
            indegree[dependent] -= 1;
            if indegree[dependent] == 0 {
                ready.insert(dependent);
            }
        }
    }

    if ordered_indices.len() != active.len() {
        let cycle_indices = find_service_dependency_cycle(&active, &adjacency);
        let features = cycle_indices
            .iter()
            .map(|index| descriptors[*index].id.clone())
            .collect::<Vec<_>>();
        let mut services = BTreeSet::new();
        for edge in cycle_indices.windows(2) {
            if let Some(ids) = edge_services.get(&(edge[0], edge[1])) {
                services.extend(ids.iter().cloned());
            }
        }
        if let (Some(first), Some(last)) = (cycle_indices.first(), cycle_indices.last())
            && let Some(ids) = edge_services.get(&(*last, *first))
        {
            services.extend(ids.iter().cloned());
        }
        return Err(FeaturePlanError::ServiceDependencyCycle {
            features,
            services: services.into_iter().collect(),
        });
    }

    let service_providers = selected_providers
        .into_iter()
        .map(|(service, (provider_index, declaration))| {
            let planned = PlannedServiceProvider {
                service: service.clone(),
                provider: descriptors[provider_index].id.clone(),
                version: declaration.version,
            };
            (service, planned)
        })
        .collect();
    let ordered_features = ordered_indices
        .iter()
        .map(|index| descriptors[*index].id.clone())
        .collect();
    Ok(FeatureInstallPlan {
        ordered_features,
        service_providers,
        ordered_indices,
    })
}

fn find_service_dependency_cycle(
    active: &BTreeSet<usize>,
    adjacency: &[BTreeSet<usize>],
) -> Vec<usize> {
    fn visit(
        index: usize,
        active: &BTreeSet<usize>,
        adjacency: &[BTreeSet<usize>],
        state: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[index] = 1;
        stack.push(index);
        for &next in &adjacency[index] {
            if !active.contains(&next) {
                continue;
            }
            match state[next] {
                0 => {
                    if let Some(cycle) = visit(next, active, adjacency, state, stack) {
                        return Some(cycle);
                    }
                }
                1 => {
                    let start = stack
                        .iter()
                        .position(|candidate| *candidate == next)
                        .expect("visiting node is present in DFS stack");
                    return Some(stack[start..].to_vec());
                }
                _ => {}
            }
        }
        stack.pop();
        state[index] = 2;
        None
    }

    let mut state = vec![0u8; adjacency.len()];
    let mut stack = Vec::new();
    for &index in active {
        if state[index] == 0
            && let Some(cycle) = visit(index, active, adjacency, &mut state, &mut stack)
        {
            return cycle;
        }
    }
    Vec::new()
}

impl Default for FeatureRegistryBuilder {
    fn default() -> Self {
        Self {
            modules: Vec::new(),
        }
    }
}

impl FeatureRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module<M>(&mut self, module: M) -> &mut Self
    where
        M: FeatureModule + 'static,
    {
        self.modules.push(Arc::new(module));
        self
    }

    pub fn with_module<M>(mut self, module: M) -> Self
    where
        M: FeatureModule + 'static,
    {
        self.add_module(module);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn descriptors(&self) -> Vec<FeatureDescriptor> {
        self.modules
            .iter()
            .map(|module| module.descriptor())
            .collect()
    }

    pub fn plan(&self) -> Result<FeatureInstallPlan, FeaturePlanError> {
        build_feature_install_plan(&self.descriptors())
    }

    /// Install modules into the existing Engine tool path and hook builder.
    pub(crate) fn install_into_engine<C: LlmClient>(
        self,
        worker: &mut Engine<C, Mutable>,
        hook_builder: &mut HookRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        let mut pending_tools = Vec::new();
        worker.tool_server_handle().flush_pending();
        let registered_tool_names = worker
            .tool_server_handle()
            .tool_definitions_sorted()
            .into_iter()
            .map(|definition| (definition.name, FeatureId::builtin("preexisting-tool")))
            .collect();
        let report = self.install_into_pending_with_registered(
            &mut pending_tools,
            hook_builder,
            registered_tool_names,
        );
        if !report.has_errors() {
            worker.register_tools(pending_tools);
        }
        report
    }

    #[allow(dead_code)]
    pub(crate) fn install_into_pending(
        self,
        pending_tools: &mut Vec<ToolDefinition>,
        hook_builder: &mut HookRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        self.install_into_pending_with_registered(pending_tools, hook_builder, HashMap::new())
    }

    fn install_into_pending_with_registered(
        self,
        pending_tools: &mut Vec<ToolDefinition>,
        hook_builder: &mut HookRegistryBuilder,
        mut installed_tool_names: HashMap<String, FeatureId>,
    ) -> FeatureRegistryInstallReport {
        let descriptors: Vec<_> = self
            .modules
            .iter()
            .map(|module| module.descriptor())
            .collect();
        let plan = match build_feature_install_plan(&descriptors) {
            Ok(plan) => plan,
            Err(error) => {
                let message = error.to_string();
                let reports = descriptors
                    .iter()
                    .map(|descriptor| {
                        let mut report = FeatureInstallReport::new(descriptor);
                        report
                            .diagnostics
                            .push(FeatureDiagnostic::error(message.clone()));
                        report.mark_skipped(
                            FeatureContributionKind::Diagnostic,
                            descriptor.id.to_string(),
                            "feature installation plan rejected before installation",
                        );
                        report
                    })
                    .collect();
                return FeatureRegistryInstallReport {
                    reports,
                    services: FeatureServiceRegistry::default(),
                    plan_error: Some(error),
                };
            }
        };
        let mut service_registry = FeatureServiceRegistry::default();
        let mut reports = Vec::with_capacity(plan.ordered_indices.len());
        let mut modules = self.modules.into_iter().map(Some).collect::<Vec<_>>();
        let ordered_modules = plan
            .ordered_indices
            .into_iter()
            .map(|index| {
                (
                    modules[index]
                        .take()
                        .expect("planned feature module is selected exactly once"),
                    descriptors[index].clone(),
                )
            })
            .collect::<Vec<_>>();

        for (module, descriptor) in ordered_modules {
            let declarations = FeatureContributionDeclarations::from_descriptor(&descriptor);
            let mut report = FeatureInstallReport::new(&descriptor);

            let mut required_service_failed = false;
            for requirement in descriptor.requires_services.iter().cloned() {
                if service_registry.provides(&requirement.id) {
                    report.resolved_service_requirements.push(requirement);
                } else if requirement.required {
                    let reason = format!(
                        "required service requirement is not available: {}",
                        requirement.id
                    );
                    report
                        .diagnostics
                        .push(FeatureDiagnostic::error(reason.clone()));
                    report.mark_skipped(
                        FeatureContributionKind::Service,
                        requirement.id.to_string(),
                        reason,
                    );
                    required_service_failed = true;
                } else {
                    report.diagnostics.push(FeatureDiagnostic::warning(format!(
                        "optional service requirement is not available: {}",
                        requirement.id
                    )));
                    report.mark_skipped(
                        FeatureContributionKind::Service,
                        requirement.id.to_string(),
                        "optional service requirement is not available",
                    );
                }
            }
            if required_service_failed {
                reports.push(report);
                continue;
            }

            for background_task in descriptor.background_tasks.iter().cloned() {
                report.declared_background_tasks.push(background_task);
            }

            let install_result = {
                let mut context = FeatureInstallContext {
                    feature_id: &descriptor.id,
                    declarations: &declarations,
                    pending_tools,
                    installed_tool_names: &mut installed_tool_names,
                    hook_builder,
                    service_registry: &mut service_registry,
                    report: &mut report,
                };
                module.install(&mut context)
            };

            match install_result {
                Ok(()) => report.installed = true,
                Err(error) => {
                    report
                        .diagnostics
                        .push(FeatureDiagnostic::error(error.to_string()));
                }
            }
            reports.push(report);
        }

        FeatureRegistryInstallReport {
            reports,
            services: service_registry,
            plan_error: None,
        }
    }
}

/// Feature installation errors.
#[derive(Debug, Error)]
pub enum FeatureInstallError {
    #[error("invalid feature descriptor: {0}")]
    InvalidDescriptor(String),
    #[error(
        "duplicate tool contribution `{tool}` from feature `{duplicate_feature}`; first registered by `{first_feature}`"
    )]
    DuplicateToolName {
        tool: String,
        first_feature: String,
        duplicate_feature: String,
    },
    #[error(
        "tool contribution declared name `{declared}` does not match model-visible tool name `{model_visible}`"
    )]
    ToolNameMismatch {
        declared: String,
        model_visible: String,
    },
    #[error(
        "undeclared {kind:?} contribution `{name}` from feature `{feature}` is not present in the approved feature descriptor"
    )]
    UndeclaredContribution {
        kind: FeatureContributionKind,
        name: String,
        feature: String,
    },
    #[error(
        "duplicate service declaration `{service}` from feature `{duplicate_feature}`; first provided by `{first_feature}`"
    )]
    DuplicateService {
        service: String,
        first_feature: String,
        duplicate_feature: String,
    },
    #[error("feature install failed: {0}")]
    Install(String),
}

pub mod builtin;
pub mod mcp;
pub mod plugin;

#[cfg(test)]
mod tests {
    use super::*;
    use agen::llm_client::{ClientError, Request, ResponseStream};
    use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
    use async_trait::async_trait;
    use futures::stream;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct DummyClient;

    #[async_trait]
    impl LlmClient for DummyClient {
        async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
            Ok(Box::pin(stream::empty()))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        async fn execute(
            &self,
            _input_json: &str,
            _ctx: agen::tool::ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::from("ok".to_string()))
        }
    }

    fn dummy_tool(name: &'static str) -> ToolDefinition {
        Arc::new(move || {
            (
                ToolMeta::new(name)
                    .description("dummy")
                    .input_schema(json!({})),
                Arc::new(DummyTool) as Arc<dyn Tool>,
            )
        })
    }

    struct ToolFeature {
        descriptor: FeatureDescriptor,
        contribution_name: &'static str,
        model_visible_name: &'static str,
    }

    impl FeatureModule for ToolFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            context.tools().register(ToolContribution::new(
                self.contribution_name,
                dummy_tool(self.model_visible_name),
            ))
        }
    }

    struct InstructionFeature {
        descriptor: FeatureDescriptor,
        instruction: FeatureInstructionDeclaration,
    }

    impl FeatureModule for InstructionFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            context
                .instructions()
                .register(FeatureInstructionContribution::new(
                    self.instruction.clone(),
                ))
        }
    }

    struct PlannedServiceFeature {
        descriptor: FeatureDescriptor,
        install_calls: Arc<AtomicUsize>,
        fail_install: bool,
    }

    impl PlannedServiceFeature {
        fn new(descriptor: FeatureDescriptor) -> Self {
            Self {
                descriptor,
                install_calls: Arc::new(AtomicUsize::new(0)),
                fail_install: false,
            }
        }
    }

    impl FeatureModule for PlannedServiceFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            self.install_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_install {
                return Err(FeatureInstallError::Install(
                    "injected provider failure".into(),
                ));
            }
            for service in &self.descriptor.provides_services {
                context
                    .services()
                    .provide(service.clone(), Arc::new(service.id.to_string()))?;
            }
            Ok(())
        }
    }

    fn provided_service(id: &'static str, version: &'static str) -> ServiceDeclaration {
        ServiceDeclaration::new(ServiceId::builtin(id), version, "test service")
    }

    fn required_service(id: &'static str, version: ServiceVersionReq) -> ServiceRequirement {
        {
            let mut requirement =
                ServiceRequirement::required(ServiceId::builtin(id), "test dependency");
            requirement.version = version;
            requirement
        }
    }

    fn instruction(id: &'static str, prompt_ref: &'static str) -> FeatureInstructionDeclaration {
        FeatureInstructionDeclaration::new(
            FeatureInstructionId::builtin(id),
            prompt_ref,
            "test instruction",
        )
        .unwrap()
    }

    #[test]
    fn install_plan_orders_provider_before_consumer_stably() {
        let consumer = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("consumer", "Consumer")
                .with_service_requirement(required_service("catalog", ServiceVersionReq::any())),
        );
        let unrelated =
            PlannedServiceFeature::new(FeatureDescriptor::builtin("unrelated", "Unrelated"));
        let provider = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("provider", "Provider")
                .with_provided_service(provided_service("catalog", "1")),
        );
        let builder = FeatureRegistryBuilder::new()
            .with_module(consumer)
            .with_module(unrelated)
            .with_module(provider);

        let plan = builder.plan().expect("service graph should be valid");
        assert_eq!(
            plan.ordered_features(),
            &[
                FeatureId::builtin("unrelated"),
                FeatureId::builtin("provider"),
                FeatureId::builtin("consumer"),
            ]
        );
    }

    #[test]
    fn install_plan_orders_compatible_optional_provider_before_consumer() {
        let service = ServiceId::builtin("catalog");
        let consumer = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("consumer", "Consumer").with_service_requirement(
                ServiceRequirement::optional(service.clone(), "optional catalog"),
            ),
        );
        let provider = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("provider", "Provider")
                .with_provided_service(provided_service("catalog", "1")),
        );
        let builder = FeatureRegistryBuilder::new()
            .with_module(consumer)
            .with_module(provider);

        let plan = builder.plan().expect("optional binding should be valid");
        assert_eq!(
            plan.ordered_features(),
            &[
                FeatureId::builtin("provider"),
                FeatureId::builtin("consumer"),
            ]
        );

        let mut hooks = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = builder.install_into_pending(&mut pending_tools, &mut hooks);
        assert!(!report.has_errors());
        let consumer = report
            .reports
            .iter()
            .find(|report| report.feature_id == FeatureId::builtin("consumer"))
            .expect("consumer report");
        assert!(
            consumer
                .resolved_service_requirements
                .iter()
                .any(|requirement| requirement.id == service)
        );
    }

    #[test]
    fn install_plan_rejects_missing_required_service() {
        let builder = FeatureRegistryBuilder::new().with_module(PlannedServiceFeature::new(
            FeatureDescriptor::builtin("consumer", "Consumer")
                .with_service_requirement(required_service("missing", ServiceVersionReq::any())),
        ));

        assert_eq!(
            builder.plan(),
            Err(FeaturePlanError::MissingServiceProvider {
                consumer: FeatureId::builtin("consumer"),
                service: ServiceId::builtin("missing"),
            })
        );
    }

    #[test]
    fn install_plan_rejects_ambiguous_provider_independent_of_registration_order() {
        fn plan(reverse: bool) -> FeaturePlanError {
            let provider_a = PlannedServiceFeature::new(
                FeatureDescriptor::builtin("provider-a", "Provider A")
                    .with_provided_service(provided_service("catalog", "1")),
            );
            let provider_b = PlannedServiceFeature::new(
                FeatureDescriptor::builtin("provider-b", "Provider B")
                    .with_provided_service(provided_service("catalog", "1")),
            );
            let mut builder = FeatureRegistryBuilder::new();
            if reverse {
                builder.add_module(provider_b).add_module(provider_a);
            } else {
                builder.add_module(provider_a).add_module(provider_b);
            }
            builder.plan().expect_err("duplicate providers must fail")
        }

        let expected = FeaturePlanError::AmbiguousServiceProvider {
            service: ServiceId::builtin("catalog"),
            providers: vec![
                FeatureId::builtin("provider-a"),
                FeatureId::builtin("provider-b"),
            ],
        };
        assert_eq!(plan(false), expected);
        assert_eq!(plan(true), expected);
    }

    #[test]
    fn install_plan_rejects_service_version_mismatch() {
        let builder = FeatureRegistryBuilder::new()
            .with_module(PlannedServiceFeature::new(
                FeatureDescriptor::builtin("consumer", "Consumer").with_service_requirement(
                    required_service("catalog", ServiceVersionReq::exact("2")),
                ),
            ))
            .with_module(PlannedServiceFeature::new(
                FeatureDescriptor::builtin("provider", "Provider")
                    .with_provided_service(provided_service("catalog", "1")),
            ));

        assert_eq!(
            builder.plan(),
            Err(FeaturePlanError::ServiceVersionMismatch {
                consumer: FeatureId::builtin("consumer"),
                service: ServiceId::builtin("catalog"),
                requirement: "2".into(),
                provider: FeatureId::builtin("provider"),
                provider_version: "1".into(),
            })
        );
    }

    #[test]
    fn install_plan_reports_service_cycle_members() {
        let builder = FeatureRegistryBuilder::new()
            .with_module(PlannedServiceFeature::new(
                FeatureDescriptor::builtin("alpha", "Alpha")
                    .with_provided_service(provided_service("alpha-service", "1"))
                    .with_service_requirement(required_service(
                        "beta-service",
                        ServiceVersionReq::any(),
                    )),
            ))
            .with_module(PlannedServiceFeature::new(
                FeatureDescriptor::builtin("beta", "Beta")
                    .with_provided_service(provided_service("beta-service", "1"))
                    .with_service_requirement(required_service(
                        "alpha-service",
                        ServiceVersionReq::any(),
                    )),
            ));

        let error = builder.plan().expect_err("cycle must fail before install");
        let FeaturePlanError::ServiceDependencyCycle { features, services } = error else {
            panic!("unexpected plan error: {error:?}");
        };
        assert_eq!(
            features.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([FeatureId::builtin("alpha"), FeatureId::builtin("beta")])
        );
        assert_eq!(
            services.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ServiceId::builtin("alpha-service"),
                ServiceId::builtin("beta-service"),
            ])
        );
    }

    #[test]
    fn plan_failure_has_no_install_side_effects_and_reports_identities() {
        let first = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("first", "First")
                .with_provided_service(provided_service("control", "1")),
        );
        let first_calls = Arc::clone(&first.install_calls);
        let second = PlannedServiceFeature::new(
            FeatureDescriptor::builtin("second", "Second")
                .with_provided_service(provided_service("control", "1")),
        );
        let second_calls = Arc::clone(&second.install_calls);
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();

        let report = FeatureRegistryBuilder::new()
            .with_module(first)
            .with_module(second)
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.has_errors());
        assert!(matches!(
            report.plan_error,
            Some(FeaturePlanError::AmbiguousServiceProvider { .. })
        ));
        assert!(report.error_message().contains("builtin:control"));
        assert!(report.error_message().contains("builtin:first"));
        assert!(report.error_message().contains("builtin:second"));
        assert_eq!(first_calls.load(Ordering::SeqCst), 0);
        assert_eq!(second_calls.load(Ordering::SeqCst), 0);
        assert!(pending_tools.is_empty());
    }

    #[test]
    fn provider_install_failure_is_fatal_and_does_not_queue_tools() {
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let provider = PlannedServiceFeature {
            descriptor: FeatureDescriptor::builtin("provider", "Provider")
                .with_provided_service(provided_service("control", "1")),
            install_calls: Arc::clone(&provider_calls),
            fail_install: true,
        };
        let consumer = ToolFeature {
            descriptor: FeatureDescriptor::builtin("consumer", "Consumer")
                .with_service_requirement(required_service("control", ServiceVersionReq::any()))
                .with_tool(ToolDeclaration::new("Dependent", "dependent tool")),
            contribution_name: "Dependent",
            model_visible_name: "Dependent",
        };
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();

        let report = FeatureRegistryBuilder::new()
            .with_module(consumer)
            .with_module(provider)
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.has_errors());
        assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
        assert!(pending_tools.is_empty());
        assert!(report.error_message().contains("builtin:provider"));
        assert!(report.error_message().contains("injected provider failure"));
    }

    #[test]
    fn descriptor_contributions_are_recorded() {
        let descriptor = FeatureDescriptor::builtin("dummy", "Dummy")
            .with_tool(ToolDeclaration::new("Dummy", "dummy tool"))
            .with_background_task(BackgroundTaskDeclaration::descriptor_only(
                "daily",
                "descriptor-only background task",
            ));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ToolFeature {
                descriptor,
                contribution_name: "Dummy",
                model_visible_name: "Dummy",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert_eq!(pending_tools.len(), 1);
        assert_eq!(report.reports.len(), 1);
        let feature_report = &report.reports[0];
        assert!(feature_report.installed);
        assert_eq!(feature_report.installed_tools, vec!["Dummy"]);
        assert_eq!(feature_report.declared_background_tasks[0].name, "daily");
    }

    #[test]
    fn instruction_contributions_are_deduped_in_registration_order() {
        let workflow = instruction("workflow", "common.tickets");
        let orchestration = instruction("orchestration", "common.worker_orchestration");
        let contributions = dedupe_instruction_contributions([
            workflow.clone(),
            orchestration.clone(),
            workflow.clone(),
        ]);

        assert_eq!(contributions, vec![workflow, orchestration]);
    }

    #[test]
    fn undeclared_instruction_contribution_is_rejected() {
        let declared = instruction("declared", "common.tickets");
        let undeclared = instruction("undeclared", "common.tickets");
        let descriptor =
            FeatureDescriptor::builtin("instruction", "Instruction").with_instruction(declared);
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(InstructionFeature {
                descriptor,
                instruction: undeclared,
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(!report.reports[0].installed);
        assert!(report.reports[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("undeclared Instruction contribution")
        }));
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let descriptor_a = FeatureDescriptor::builtin("a", "A")
            .with_tool(ToolDeclaration::new("Duplicate", "first tool"));
        let descriptor_b = FeatureDescriptor::builtin("b", "B")
            .with_tool(ToolDeclaration::new("Duplicate", "second tool"));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ToolFeature {
                descriptor: descriptor_a,
                contribution_name: "Duplicate",
                model_visible_name: "Duplicate",
            })
            .with_module(ToolFeature {
                descriptor: descriptor_b,
                contribution_name: "Duplicate",
                model_visible_name: "Duplicate",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert_eq!(pending_tools.len(), 1);
        assert!(report.reports[0].installed);
        assert!(!report.reports[1].installed);
        assert!(
            report.reports[1]
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate tool contribution"))
        );
        assert_eq!(
            report.reports[1].skipped[0].kind,
            FeatureContributionKind::Tool
        );
    }

    #[test]
    fn mismatched_tool_contribution_name_is_rejected_before_queueing() {
        let descriptor = FeatureDescriptor::builtin("mismatch", "Mismatch")
            .with_tool(ToolDeclaration::new("Actual", "actual model-visible tool"));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ToolFeature {
                descriptor,
                contribution_name: "Declared",
                model_visible_name: "Actual",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(pending_tools.is_empty());
        assert!(!report.reports[0].installed);
        assert!(report.reports[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("does not match model-visible tool name")
        }));
        assert_eq!(report.reports[0].skipped[0].name, "Actual");
    }

    struct ProviderFeature {
        descriptor: FeatureDescriptor,
        provider: ProtocolProviderDeclaration,
        calls: Arc<AtomicUsize>,
        state: ProtocolProviderLifecycleState,
    }

    impl FeatureModule for ProviderFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            let calls = Arc::clone(&self.calls);
            let definition: ToolDefinition = Arc::new(move || {
                let call_index = calls.fetch_add(1, Ordering::SeqCst);
                let name = if call_index == 0 {
                    "DynamicTool"
                } else {
                    "ChangedDynamicTool"
                };
                (
                    ToolMeta::new(name)
                        .description("startup-discovered")
                        .input_schema(json!({ "type": "object" })),
                    Arc::new(DummyTool) as Arc<dyn Tool>,
                )
            });
            let contribution =
                ProtocolProviderContribution::new(self.provider.clone(), self.state.clone())
                    .with_tool(ToolContribution::new("DynamicTool", definition))
                    .with_service(ServiceDeclaration::new(
                        ServiceId::builtin("dynamic-service"),
                        "1.0.0",
                        "startup-discovered service",
                    ))
                    .with_background_task(BackgroundTaskDeclaration::descriptor_only(
                        "provider-poller",
                        "provider lifecycle poller",
                    ))
                    .with_diagnostic(FeatureDiagnostic::info("startup discovery completed"));
            context.protocol_providers().register(contribution)
        }
    }

    #[test]
    fn protocol_provider_registers_startup_discovered_contributions_through_worker_path() {
        let provider = ProtocolProviderDeclaration::new(
            ProviderId::builtin("dynamic-provider"),
            "test-protocol",
            "Dynamic provider",
            "1",
        );
        let descriptor = FeatureDescriptor::builtin("provider-feature", "Provider feature")
            .with_protocol_provider(provider.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut worker = Engine::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();

        let report = FeatureRegistryBuilder::new()
            .with_module(ProviderFeature {
                descriptor,
                provider,
                calls: Arc::clone(&calls),
                state: ProtocolProviderLifecycleState::Ready,
            })
            .install_into_engine(&mut worker, &mut hook_builder);

        worker.tool_server_handle().flush_pending();
        let tool_names: Vec<_> = worker
            .tool_server_handle()
            .tool_definitions_sorted()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let feature_report = &report.reports[0];

        assert!(feature_report.installed);
        assert_eq!(feature_report.installed_tools, vec!["DynamicTool"]);
        assert_eq!(tool_names, vec!["DynamicTool"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(feature_report.provided_services.len(), 1);
        assert_eq!(
            feature_report.provided_services[0].id,
            ServiceId::builtin("dynamic-service")
        );
        assert_eq!(
            feature_report.declared_background_tasks[0].name,
            "provider-poller"
        );
        assert_eq!(feature_report.protocol_providers.len(), 1);
        assert_eq!(
            feature_report.protocol_providers[0].state,
            ProtocolProviderLifecycleState::Ready
        );
        assert!(
            feature_report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("startup discovery completed"))
        );
    }

    #[test]
    fn unavailable_protocol_provider_records_lifecycle_and_skips_discovered_contributions() {
        let provider = ProtocolProviderDeclaration::new(
            ProviderId::builtin("failed-provider"),
            "test-protocol",
            "Failed provider",
            "1",
        );
        let descriptor = FeatureDescriptor::builtin("failed-provider-feature", "Failed provider")
            .with_protocol_provider(provider.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();

        let report = FeatureRegistryBuilder::new()
            .with_module(ProviderFeature {
                descriptor,
                provider,
                calls: Arc::clone(&calls),
                state: ProtocolProviderLifecycleState::Failed,
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        let feature_report = &report.reports[0];
        assert!(feature_report.installed);
        assert!(pending_tools.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            feature_report.protocol_providers[0].state,
            ProtocolProviderLifecycleState::Failed
        );
        assert!(feature_report.provided_services.is_empty());
        assert!(feature_report.declared_background_tasks.is_empty());
        assert!(
            feature_report
                .skipped
                .iter()
                .any(|skipped| skipped.kind == FeatureContributionKind::Tool)
        );
    }

    #[test]
    fn undeclared_protocol_provider_is_rejected_before_registering_tools() {
        let provider = ProtocolProviderDeclaration::new(
            ProviderId::builtin("undeclared-provider"),
            "test-protocol",
            "Undeclared provider",
            "1",
        );
        let descriptor = FeatureDescriptor::builtin("undeclared-provider-feature", "Undeclared");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();

        let report = FeatureRegistryBuilder::new()
            .with_module(ProviderFeature {
                descriptor,
                provider,
                calls: Arc::clone(&calls),
                state: ProtocolProviderLifecycleState::Ready,
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(!report.reports[0].installed);
        assert!(pending_tools.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::ProtocolProvider
        );
    }

    #[test]
    fn stateful_tool_definition_is_materialized_once_for_report_and_worker() {
        struct StatefulToolFeature {
            calls: Arc<AtomicUsize>,
        }

        impl FeatureModule for StatefulToolFeature {
            fn descriptor(&self) -> FeatureDescriptor {
                FeatureDescriptor::builtin("stateful-tool", "Stateful tool")
                    .with_tool(ToolDeclaration::new("First", "stateful tool"))
            }

            fn install(
                &self,
                context: &mut FeatureInstallContext<'_>,
            ) -> Result<(), FeatureInstallError> {
                let calls = Arc::clone(&self.calls);
                let definition: ToolDefinition = Arc::new(move || {
                    let call_index = calls.fetch_add(1, Ordering::SeqCst);
                    let name = if call_index == 0 { "First" } else { "Second" };
                    (
                        ToolMeta::new(name)
                            .description("stateful")
                            .input_schema(json!({})),
                        Arc::new(DummyTool) as Arc<dyn Tool>,
                    )
                });
                context
                    .tools()
                    .register(ToolContribution::new("First", definition))
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut worker = Engine::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(StatefulToolFeature {
                calls: Arc::clone(&calls),
            })
            .install_into_engine(&mut worker, &mut hook_builder);

        worker.tool_server_handle().flush_pending();
        let names: Vec<_> = worker
            .tool_server_handle()
            .tool_definitions_sorted()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        assert_eq!(report.installed_tool_names(), vec!["First"]);
        assert_eq!(names, vec!["First"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct ServiceFeature {
        descriptor: FeatureDescriptor,
    }

    impl FeatureModule for ServiceFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            for service in self.descriptor.provides_services.iter().cloned() {
                context.services().provide(service, Arc::new(()))?;
            }
            Ok(())
        }
    }

    struct DummyPreToolHook;

    #[async_trait]
    impl Hook<PreToolCall> for DummyPreToolHook {
        async fn call(
            &self,
            _input: &crate::hook::ToolCallSummary,
        ) -> crate::hook::HookPreToolAction {
            crate::hook::HookPreToolAction::Continue
        }
    }

    struct HookFeature {
        descriptor: FeatureDescriptor,
        hook_name: &'static str,
    }

    impl FeatureModule for HookFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            context
                .hooks()
                .add_pre_tool_call(self.hook_name, DummyPreToolHook)
        }
    }

    struct BackgroundFeature {
        descriptor: FeatureDescriptor,
        task_name: &'static str,
    }

    impl FeatureModule for BackgroundFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            context
                .background_tasks()
                .declare(BackgroundTaskDeclaration::descriptor_only(
                    self.task_name,
                    "runtime background task",
                ))
        }
    }

    struct ServiceProviderFeature {
        descriptor: FeatureDescriptor,
        service: ServiceId,
    }

    impl FeatureModule for ServiceProviderFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            context.services().provide(
                ServiceDeclaration::new(self.service.clone(), "1", "runtime service provider"),
                Arc::new(()),
            )
        }
    }

    #[test]
    fn undeclared_tool_contribution_is_rejected() {
        let descriptor = FeatureDescriptor::builtin("undeclared-tool", "Undeclared tool");
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ToolFeature {
                descriptor,
                contribution_name: "HiddenTool",
                model_visible_name: "HiddenTool",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(pending_tools.is_empty());
        assert!(!report.reports[0].installed);
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::Tool
        );
        assert_eq!(report.reports[0].skipped[0].name, "HiddenTool");
        assert!(report.reports[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("is not present in the approved feature descriptor")
        }));
    }

    #[test]
    fn undeclared_hook_contribution_is_rejected() {
        let descriptor = FeatureDescriptor::builtin("undeclared-hook", "Undeclared hook");
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(HookFeature {
                descriptor,
                hook_name: "hidden-hook",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(!report.reports[0].installed);
        assert!(report.reports[0].installed_hooks.is_empty());
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::Hook
        );
        assert!(report.reports[0].skipped[0].name.contains("hidden-hook"));
    }

    #[test]
    fn undeclared_background_task_contribution_is_rejected() {
        let descriptor =
            FeatureDescriptor::builtin("undeclared-background", "Undeclared background");
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(BackgroundFeature {
                descriptor,
                task_name: "hidden-task",
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(!report.reports[0].installed);
        assert!(report.reports[0].declared_background_tasks.is_empty());
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::BackgroundTask
        );
        assert_eq!(report.reports[0].skipped[0].name, "hidden-task");
    }

    #[test]
    fn undeclared_service_provider_contribution_is_rejected() {
        let service = ServiceId::builtin("hidden-service");
        let descriptor = FeatureDescriptor::builtin("undeclared-service", "Undeclared service");
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceProviderFeature {
                descriptor,
                service: service.clone(),
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(!report.reports[0].installed);
        assert!(!report.services.provides(&service));
        assert!(report.reports[0].provided_services.is_empty());
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::Service
        );
        assert_eq!(report.reports[0].skipped[0].name, service.to_string());
    }

    #[test]
    fn service_requirements_resolve_against_prior_providers() {
        let service = ServiceId::builtin("demo-service");
        let provider = FeatureDescriptor::builtin("provider", "Provider").with_provided_service(
            ServiceDeclaration::new(service.clone(), "1", "demo service"),
        );
        let consumer = FeatureDescriptor::builtin("consumer", "Consumer")
            .with_service_requirement(ServiceRequirement::required(service.clone(), "needs demo"));
        let optional_service = ServiceId::builtin("optional-service");
        let optional = FeatureDescriptor::builtin("optional", "Optional").with_service_requirement(
            ServiceRequirement::optional(optional_service, "nice to have"),
        );
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature {
                descriptor: provider,
            })
            .with_module(ServiceFeature {
                descriptor: consumer,
            })
            .with_module(ServiceFeature {
                descriptor: optional,
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.services.provides(&service));
        assert!(report.reports[1].installed);
        assert_eq!(
            report.reports[1].resolved_service_requirements[0].id,
            service
        );
        let optional_report = report
            .reports
            .iter()
            .find(|feature| feature.feature_id == FeatureId::builtin("optional"))
            .unwrap();
        assert!(optional_report.installed);
        assert_eq!(
            optional_report.skipped[0].kind,
            FeatureContributionKind::Service
        );
    }

    #[test]
    fn background_task_declaration_is_descriptor_contribution() {
        let descriptor = FeatureDescriptor::builtin("background", "Background")
            .with_background_task(BackgroundTaskDeclaration::descriptor_only(
                "declared-task",
                "descriptor contribution",
            ));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature { descriptor })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.reports[0].installed);
        assert_eq!(
            report.reports[0].declared_background_tasks[0].name,
            "declared-task"
        );
        assert!(report.reports[0].skipped.is_empty());
    }

    #[test]
    fn service_provider_declaration_is_descriptor_contribution() {
        let service = ServiceId::builtin("declared-service");
        let descriptor = FeatureDescriptor::builtin("service", "Service").with_provided_service(
            ServiceDeclaration::new(service.clone(), "1", "descriptor contribution"),
        );
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature { descriptor })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.reports[0].installed);
        assert!(report.services.provides(&service));
        assert_eq!(report.reports[0].provided_services[0].id, service);
        assert!(report.reports[0].skipped.is_empty());
    }

    #[test]
    fn service_registry_keeps_typed_concrete_instance() {
        trait CounterService: Send + Sync {
            fn value(&self) -> usize;
        }
        struct Counter(usize);
        impl CounterService for Counter {
            fn value(&self) -> usize {
                self.0
            }
        }
        let mut registry = FeatureServiceRegistry::default();
        let service_id = ServiceId::builtin("typed-counter");
        let service: Arc<dyn CounterService> = Arc::new(Counter(7));
        registry
            .register_provider(
                FeatureId::builtin("provider"),
                ServiceDeclaration::new(service_id.clone(), "1", "typed counter"),
                service,
            )
            .unwrap();
        assert_eq!(
            registry
                .service::<dyn CounterService>(&service_id)
                .unwrap()
                .value(),
            7
        );
    }

    #[test]
    fn service_dependencies_install_in_provider_order() {
        let service = ServiceId::builtin("ordered-service");
        let provider = FeatureDescriptor::builtin("provider", "Provider").with_provided_service(
            ServiceDeclaration::new(service.clone(), "1", "ordered service"),
        );
        let consumer = FeatureDescriptor::builtin("consumer", "Consumer").with_service_requirement(
            ServiceRequirement::required(service, "consumer depends on provider"),
        );
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature {
                descriptor: consumer,
            })
            .with_module(ServiceFeature {
                descriptor: provider,
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        assert!(!report.has_errors(), "{}", report.error_message());
        assert_eq!(report.reports[0].feature_id, FeatureId::builtin("provider"));
        assert_eq!(report.reports[1].feature_id, FeatureId::builtin("consumer"));
    }

    #[test]
    fn missing_required_service_is_fatal() {
        let descriptor = FeatureDescriptor::builtin("consumer", "Consumer")
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin("missing-service"),
                "must fail closed",
            ));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature { descriptor })
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        assert!(report.has_errors());
        assert!(matches!(
            report.plan_error,
            Some(FeaturePlanError::MissingServiceProvider { .. })
        ));
        assert!(report.error_message().contains("builtin:consumer"));
        assert!(report.error_message().contains("builtin:missing-service"));
    }

    #[test]
    fn builtin_internal_task_feature_descriptor_has_exact_tools_hooks() {
        let descriptor = builtin::task_tools_feature().descriptor();
        let tool_names: Vec<_> = descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        let hook_points: Vec<_> = descriptor
            .hooks
            .iter()
            .map(|hook| hook.point.clone())
            .collect();

        assert_eq!(descriptor.id.as_str(), "builtin:task-tools");
        assert_eq!(descriptor.runtime, FeatureRuntimeKind::Builtin);
        assert_eq!(
            hook_points,
            vec![FeatureHookPoint::PreRequest, FeatureHookPoint::PreToolCall]
        );
        assert!(descriptor.background_tasks.is_empty());
        assert!(descriptor.provides_services.is_empty());
        assert!(descriptor.requires_services.is_empty());
        assert_eq!(
            tool_names,
            vec!["TaskCreate", "TaskUpdate", "TaskGet", "TaskList"]
        );
    }

    #[test]
    fn builtin_internal_task_feature_installs_declared_tools() {
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let mut builder = FeatureRegistryBuilder::new();
        builder.add_module(builtin::task_tools_feature());
        let mut declared_names: Vec<_> = builder.descriptors()[0]
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        let report = builder.install_into_pending(&mut pending_tools, &mut hook_builder);
        let pending_names: Vec<_> = pending_tools
            .iter()
            .map(|definition| definition().0.name)
            .collect();
        let installed_names = report.installed_tool_names();
        let mut sorted_installed_names = installed_names.clone();
        declared_names.sort();
        sorted_installed_names.sort();

        assert_eq!(report.reports.len(), 1);
        assert!(report.reports[0].installed);
        assert!(report.reports[0].skipped.is_empty());
        assert!(report.reports[0].diagnostics.is_empty());
        assert_eq!(report.reports[0].installed_hooks.len(), 2);
        let hook_registry = hook_builder.build();
        assert_eq!(hook_registry.pre_llm_request.len(), 1);
        assert_eq!(hook_registry.pre_tool_call.len(), 1);
        assert_eq!(declared_names, sorted_installed_names);
        assert_eq!(
            installed_names,
            vec!["TaskCreate", "TaskList", "TaskGet", "TaskUpdate"]
        );
        assert_eq!(
            pending_names,
            vec!["TaskCreate", "TaskList", "TaskGet", "TaskUpdate"]
        );
    }

    #[test]
    fn builtin_task_feature_installs_through_worker_tool_path() {
        let mut worker = Engine::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(builtin::task_tools_feature())
            .install_into_engine(&mut worker, &mut hook_builder);

        worker.tool_server_handle().flush_pending();
        let names: Vec<_> = worker
            .tool_server_handle()
            .tool_definitions_sorted()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        assert_eq!(
            names,
            vec!["TaskCreate", "TaskGet", "TaskList", "TaskUpdate"]
        );
        assert_eq!(
            report.installed_tool_names(),
            vec!["TaskCreate", "TaskList", "TaskGet", "TaskUpdate"]
        );
        assert!(report.reports[0].installed);
    }
}
