//! Feature contribution registry for Pod-hosted builtin/plugin modules.
//!
//! This module defines the Pod-side feature boundary used to collect
//! descriptor metadata, capability requests, tool contributions, safe hook
//! contributions, background task declarations, and service declarations before
//! installing them into the existing Worker/HookRegistry host surfaces.
//!
//! The first implementation slice is intentionally host-mediated and
//! descriptor-first: tools are installed through the normal Worker tool path,
//! hooks are installed through [`crate::hook::HookRegistryBuilder`], while
//! service and background-task contributions are represented in descriptors and
//! install reports without starting an independent runtime lifecycle.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use llm_worker::Worker;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::state::Mutable;
use llm_worker::tool::ToolDefinition;
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

/// Runtime/source class for a feature module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureRuntimeKind {
    Builtin,
    LuaProfile,
    ExternalPlugin,
}

/// Host capability requested by a feature before it contributes host-visible
/// behavior. Grants are additive and do not replace manifest/tool permission
/// checks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCapability {
    ContributeTool { name: String },
    ContributeHook { point: FeatureHookPoint },
    DeclareBackgroundTask { name: String },
    ProvideService { service: ServiceId },
    RequireService { service: ServiceId },
    EmitNotification,
    EmitAlert,
    EmitDiagnostic,
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

/// Capability request declared by a feature descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub capability: HostCapability,
    pub required: bool,
    pub reason: String,
}

impl CapabilityRequest {
    pub fn required(capability: HostCapability, reason: impl Into<String>) -> Self {
        Self {
            capability,
            required: true,
            reason: reason.into(),
        }
    }

    pub fn optional(capability: HostCapability, reason: impl Into<String>) -> Self {
        Self {
            capability,
            required: false,
            reason: reason.into(),
        }
    }
}

/// Capability grants resolved by the host for one feature installation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrantSet {
    granted: HashSet<HostCapability>,
    denied: Vec<CapabilityDenial>,
}

impl CapabilityGrantSet {
    pub fn grant_all(requests: &[CapabilityRequest]) -> Self {
        Self {
            granted: requests
                .iter()
                .map(|request| request.capability.clone())
                .collect(),
            denied: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn contains(&self, capability: &HostCapability) -> bool {
        self.granted.contains(capability)
    }

    pub fn denied(&self) -> &[CapabilityDenial] {
        &self.denied
    }

    pub fn grant(&mut self, capability: HostCapability) {
        self.granted.insert(capability);
    }

    pub fn deny(&mut self, capability: HostCapability, reason: impl Into<String>) {
        self.granted.remove(&capability);
        self.denied.push(CapabilityDenial {
            capability,
            reason: reason.into(),
        });
    }
}

/// Host-side denial of a requested feature capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDenial {
    pub capability: HostCapability,
    pub reason: String,
}

/// Serializable declaration of a tool contribution. The executable factory is
/// carried by [`ToolContribution`] during installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub required_capabilities: Vec<HostCapability>,
}

impl ToolDeclaration {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            required_capabilities: vec![HostCapability::ContributeTool { name: name.clone() }],
            name,
            description: description.into(),
        }
    }
}

/// Executable tool contribution wrapper.
pub struct ToolContribution {
    name: String,
    definition: ToolDefinition,
    required_capabilities: Vec<HostCapability>,
}

impl ToolContribution {
    pub fn new(name: impl Into<String>, definition: ToolDefinition) -> Self {
        let name = name.into();
        Self {
            required_capabilities: vec![HostCapability::ContributeTool { name: name.clone() }],
            name,
            definition,
        }
    }

    pub fn with_required_capabilities(
        mut self,
        required_capabilities: Vec<HostCapability>,
    ) -> Self {
        self.required_capabilities = required_capabilities;
        self
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
    pub required_capabilities: Vec<HostCapability>,
}

impl HookDeclaration {
    pub fn new(name: impl Into<String>, point: FeatureHookPoint) -> Self {
        Self {
            name: name.into(),
            required_capabilities: vec![HostCapability::ContributeHook {
                point: point.clone(),
            }],
            point,
        }
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
    pub required_capabilities: Vec<HostCapability>,
}

impl BackgroundTaskDeclaration {
    pub fn descriptor_only(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            required_capabilities: vec![HostCapability::DeclareBackgroundTask {
                name: name.clone(),
            }],
            name,
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

/// Feature service requirement used for host-mediated dependency resolution.
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

/// Host-mediated service registry skeleton used during feature installation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

    fn register_provider(
        &mut self,
        feature_id: FeatureId,
        declaration: ServiceDeclaration,
    ) -> Result<(), FeatureInstallError> {
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
            },
        );
        Ok(())
    }
}

/// Provider metadata for one service declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureServiceProvider {
    pub feature_id: FeatureId,
    pub declaration: ServiceDeclaration,
}

/// Feature descriptor advertised before installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureDescriptor {
    pub id: FeatureId,
    pub runtime: FeatureRuntimeKind,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub requested_capabilities: Vec<CapabilityRequest>,
    pub tools: Vec<ToolDeclaration>,
    pub hooks: Vec<HookDeclaration>,
    pub background_tasks: Vec<BackgroundTaskDeclaration>,
    pub provides_services: Vec<ServiceDeclaration>,
    pub requires_services: Vec<ServiceRequirement>,
}

impl FeatureDescriptor {
    pub fn builtin(id: impl AsRef<str>, display_name: impl Into<String>) -> Self {
        Self {
            id: FeatureId::builtin(id),
            runtime: FeatureRuntimeKind::Builtin,
            display_name: display_name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: String::new(),
            requested_capabilities: Vec::new(),
            tools: Vec::new(),
            hooks: Vec::new(),
            background_tasks: Vec::new(),
            provides_services: Vec::new(),
            requires_services: Vec::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_capability(mut self, request: CapabilityRequest) -> Self {
        self.requested_capabilities.push(request);
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
    BackgroundTask,
    Service,
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
    pub granted_capabilities: CapabilityGrantSet,
    pub installed_tools: Vec<String>,
    pub installed_hooks: Vec<HookDeclaration>,
    pub declared_background_tasks: Vec<BackgroundTaskDeclaration>,
    pub provided_services: Vec<ServiceDeclaration>,
    pub resolved_service_requirements: Vec<ServiceRequirement>,
    pub skipped: Vec<SkippedContribution>,
    pub diagnostics: Vec<FeatureDiagnostic>,
}

impl FeatureInstallReport {
    fn new(descriptor: &FeatureDescriptor, granted_capabilities: CapabilityGrantSet) -> Self {
        Self {
            feature_id: descriptor.id.clone(),
            runtime: descriptor.runtime.clone(),
            installed: false,
            granted_capabilities,
            installed_tools: Vec::new(),
            installed_hooks: Vec::new(),
            declared_background_tasks: Vec::new(),
            provided_services: Vec::new(),
            resolved_service_requirements: Vec::new(),
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

fn require_capability(
    grants: &CapabilityGrantSet,
    report: &mut FeatureInstallReport,
    kind: FeatureContributionKind,
    name: impl Into<String>,
    capability: &HostCapability,
) -> Result<(), FeatureInstallError> {
    if grants.contains(capability) {
        return Ok(());
    }

    let reason = format!("required capability was not granted: {capability:?}");
    report.mark_skipped(kind, name, reason.clone());
    Err(FeatureInstallError::CapabilityDenied(reason))
}

fn require_background_task_capability(
    grants: &CapabilityGrantSet,
    report: &mut FeatureInstallReport,
    declaration: &BackgroundTaskDeclaration,
) -> Result<(), FeatureInstallError> {
    let default_capability = HostCapability::DeclareBackgroundTask {
        name: declaration.name.clone(),
    };
    require_capability(
        grants,
        report,
        FeatureContributionKind::BackgroundTask,
        declaration.name.clone(),
        &default_capability,
    )?;
    for capability in &declaration.required_capabilities {
        require_capability(
            grants,
            report,
            FeatureContributionKind::BackgroundTask,
            declaration.name.clone(),
            capability,
        )?;
    }
    Ok(())
}

fn require_service_provider_capability(
    grants: &CapabilityGrantSet,
    report: &mut FeatureInstallReport,
    declaration: &ServiceDeclaration,
) -> Result<(), FeatureInstallError> {
    let capability = HostCapability::ProvideService {
        service: declaration.id.clone(),
    };
    require_capability(
        grants,
        report,
        FeatureContributionKind::Service,
        declaration.id.to_string(),
        &capability,
    )
}

/// Model-visible durable notification sink skeleton. The first slice exposes
/// the boundary without implementing a new event channel.
pub struct FeatureNotificationSink<'a> {
    grants: &'a CapabilityGrantSet,
    report: &'a mut FeatureInstallReport,
}

impl FeatureNotificationSink<'_> {
    pub fn notify_model(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        require_capability(
            self.grants,
            self.report,
            FeatureContributionKind::Notification,
            "notify_model",
            &HostCapability::EmitNotification,
        )?;
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
    grants: &'a CapabilityGrantSet,
    report: &'a mut FeatureInstallReport,
}

impl FeatureAlertSink<'_> {
    pub fn alert(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        require_capability(
            self.grants,
            self.report,
            FeatureContributionKind::Alert,
            "alert",
            &HostCapability::EmitAlert,
        )?;
        let message = message.into();
        self.report
            .diagnostics
            .push(FeatureDiagnostic::info(format!("feature alert: {message}")));
        self.report.mark_skipped(
            FeatureContributionKind::Alert,
            "alert",
            "transient alert host is not connected during feature installation",
        );
        Ok(())
    }
}

/// Diagnostic sink available to feature installers.
pub struct FeatureDiagnosticSink<'a> {
    grants: &'a CapabilityGrantSet,
    report: &'a mut FeatureInstallReport,
}

impl FeatureDiagnosticSink<'_> {
    pub fn push(&mut self, diagnostic: FeatureDiagnostic) -> Result<(), FeatureInstallError> {
        require_capability(
            self.grants,
            self.report,
            FeatureContributionKind::Diagnostic,
            "diagnostic",
            &HostCapability::EmitDiagnostic,
        )?;
        self.report.diagnostics.push(diagnostic);
        Ok(())
    }

    pub fn info(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        self.push(FeatureDiagnostic::info(message))
    }

    pub fn warning(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        self.push(FeatureDiagnostic::warning(message))
    }

    pub fn error(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        self.push(FeatureDiagnostic::error(message))
    }
}

/// Tool contribution registrar exposed inside [`FeatureInstallContext`].
pub struct ToolContributionRegistrar<'a> {
    feature_id: &'a FeatureId,
    grants: &'a CapabilityGrantSet,
    pending_tools: &'a mut Vec<ToolDefinition>,
    installed_tool_names: &'a mut HashMap<String, FeatureId>,
    report: &'a mut FeatureInstallReport,
}

impl ToolContributionRegistrar<'_> {
    pub fn register(&mut self, contribution: ToolContribution) -> Result<(), FeatureInstallError> {
        let model_visible_name = (contribution.definition)().0.name;
        if contribution.name != model_visible_name {
            let error = FeatureInstallError::ToolNameMismatch {
                declared: contribution.name,
                model_visible: model_visible_name.clone(),
            };
            self.report.mark_skipped(
                FeatureContributionKind::Tool,
                model_visible_name,
                error.to_string(),
            );
            return Err(error);
        }

        let tool_capability = HostCapability::ContributeTool {
            name: model_visible_name.clone(),
        };
        require_capability(
            self.grants,
            self.report,
            FeatureContributionKind::Tool,
            model_visible_name.clone(),
            &tool_capability,
        )?;
        for capability in &contribution.required_capabilities {
            require_capability(
                self.grants,
                self.report,
                FeatureContributionKind::Tool,
                model_visible_name.clone(),
                capability,
            )?;
        }

        if let Some(first) = self.installed_tool_names.get(&model_visible_name) {
            let error = FeatureInstallError::DuplicateToolName {
                tool: model_visible_name.clone(),
                first_feature: first.to_string(),
                duplicate_feature: self.feature_id.to_string(),
            };
            self.report.mark_skipped(
                FeatureContributionKind::Tool,
                model_visible_name,
                error.to_string(),
            );
            return Err(error);
        }

        self.installed_tool_names
            .insert(model_visible_name.clone(), self.feature_id.clone());
        self.report.installed_tools.push(model_visible_name);
        self.pending_tools.push(contribution.definition);
        Ok(())
    }
}

/// Safe hook contribution registrar backed by [`HookRegistryBuilder`].
pub struct HookContributionRegistrar<'a> {
    grants: &'a CapabilityGrantSet,
    hook_builder: &'a mut HookRegistryBuilder,
    report: &'a mut FeatureInstallReport,
}

impl HookContributionRegistrar<'_> {
    pub fn add_pre_request(
        &mut self,
        name: impl Into<String>,
        hook: impl Hook<PreLlmRequest> + 'static,
    ) -> Result<(), FeatureInstallError> {
        let declaration = HookDeclaration::new(name, FeatureHookPoint::PreRequest);
        self.require_hook_capability(&declaration)?;
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
        self.require_hook_capability(&declaration)?;
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
        self.require_hook_capability(&declaration)?;
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
        self.require_hook_capability(&declaration)?;
        self.hook_builder.add_on_turn_end(hook);
        self.report.installed_hooks.push(declaration);
        Ok(())
    }

    fn require_hook_capability(
        &mut self,
        declaration: &HookDeclaration,
    ) -> Result<(), FeatureInstallError> {
        for capability in &declaration.required_capabilities {
            if !self.grants.contains(capability) {
                let reason = format!("required capability was not granted: {capability:?}");
                self.report.mark_skipped(
                    FeatureContributionKind::Hook,
                    declaration.name.clone(),
                    reason.clone(),
                );
                return Err(FeatureInstallError::CapabilityDenied(reason));
            }
        }
        Ok(())
    }
}

/// Background task registrar for descriptor/report-only contributions.
pub struct BackgroundTaskRegistrar<'a> {
    grants: &'a CapabilityGrantSet,
    report: &'a mut FeatureInstallReport,
}

impl BackgroundTaskRegistrar<'_> {
    pub fn declare(
        &mut self,
        declaration: BackgroundTaskDeclaration,
    ) -> Result<(), FeatureInstallError> {
        require_background_task_capability(self.grants, self.report, &declaration)?;
        self.report.declared_background_tasks.push(declaration);
        Ok(())
    }
}

/// Service registrar for descriptor/report-only provider metadata.
pub struct FeatureServiceRegistrar<'a> {
    feature_id: &'a FeatureId,
    grants: &'a CapabilityGrantSet,
    service_registry: &'a mut FeatureServiceRegistry,
    report: &'a mut FeatureInstallReport,
}

impl FeatureServiceRegistrar<'_> {
    pub fn provide(&mut self, declaration: ServiceDeclaration) -> Result<(), FeatureInstallError> {
        require_service_provider_capability(self.grants, self.report, &declaration)?;
        self.service_registry
            .register_provider(self.feature_id.clone(), declaration.clone())?;
        self.report.provided_services.push(declaration);
        Ok(())
    }
}

/// Install-time context provided to a feature module.
pub struct FeatureInstallContext<'a> {
    feature_id: &'a FeatureId,
    grants: &'a CapabilityGrantSet,
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

    pub fn grants(&self) -> &CapabilityGrantSet {
        self.grants
    }

    pub fn tools(&mut self) -> ToolContributionRegistrar<'_> {
        ToolContributionRegistrar {
            feature_id: self.feature_id,
            grants: self.grants,
            pending_tools: self.pending_tools,
            installed_tool_names: self.installed_tool_names,
            report: self.report,
        }
    }

    pub fn hooks(&mut self) -> HookContributionRegistrar<'_> {
        HookContributionRegistrar {
            grants: self.grants,
            hook_builder: self.hook_builder,
            report: self.report,
        }
    }

    pub fn background_tasks(&mut self) -> BackgroundTaskRegistrar<'_> {
        BackgroundTaskRegistrar {
            grants: self.grants,
            report: self.report,
        }
    }

    pub fn services(&mut self) -> FeatureServiceRegistrar<'_> {
        FeatureServiceRegistrar {
            feature_id: self.feature_id,
            grants: self.grants,
            service_registry: self.service_registry,
            report: self.report,
        }
    }

    pub fn notifications(&mut self) -> FeatureNotificationSink<'_> {
        FeatureNotificationSink {
            grants: self.grants,
            report: self.report,
        }
    }

    pub fn alerts(&mut self) -> FeatureAlertSink<'_> {
        FeatureAlertSink {
            grants: self.grants,
            report: self.report,
        }
    }

    pub fn diagnostics(&mut self) -> FeatureDiagnosticSink<'_> {
        FeatureDiagnosticSink {
            grants: self.grants,
            report: self.report,
        }
    }
}

/// Aggregate install output for a registry installation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureRegistryInstallReport {
    pub reports: Vec<FeatureInstallReport>,
    pub services: FeatureServiceRegistry,
}

impl FeatureRegistryInstallReport {
    pub fn installed_tool_names(&self) -> Vec<String> {
        self.reports
            .iter()
            .flat_map(|report| report.installed_tools.iter().cloned())
            .collect()
    }
}

/// Builder/installer for enabled feature modules.
#[derive(Default)]
pub struct FeatureRegistryBuilder {
    modules: Vec<Arc<dyn FeatureModule>>,
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

    /// Install modules into the existing Worker tool path and hook builder.
    pub(crate) fn install_into_worker<C: LlmClient>(
        self,
        worker: &mut Worker<C, Mutable>,
        hook_builder: &mut HookRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        let mut pending_tools = Vec::new();
        let report = self.install_into_pending(&mut pending_tools, hook_builder);
        worker.register_tools(pending_tools);
        report
    }

    pub(crate) fn install_into_pending(
        self,
        pending_tools: &mut Vec<ToolDefinition>,
        hook_builder: &mut HookRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        let descriptors: Vec<_> = self
            .modules
            .iter()
            .map(|module| module.descriptor())
            .collect();
        let mut service_registry = FeatureServiceRegistry::default();
        let mut reports = Vec::with_capacity(self.modules.len());
        let mut installed_tool_names = HashMap::new();
        let mut seen_features = HashSet::new();

        for (module, descriptor) in self.modules.into_iter().zip(descriptors.into_iter()) {
            let grants = CapabilityGrantSet::grant_all(&descriptor.requested_capabilities);
            let mut report = FeatureInstallReport::new(&descriptor, grants.clone());

            if !seen_features.insert(descriptor.id.clone()) {
                report.diagnostics.push(FeatureDiagnostic::error(format!(
                    "duplicate feature id: {}",
                    descriptor.id
                )));
                report.mark_skipped(
                    FeatureContributionKind::Diagnostic,
                    descriptor.id.to_string(),
                    "duplicate feature id",
                );
                reports.push(report);
                continue;
            }

            for capability in grants.denied() {
                report.diagnostics.push(FeatureDiagnostic::warning(format!(
                    "capability denied: {:?}: {}",
                    capability.capability, capability.reason
                )));
            }

            let mut required_service_failed = false;
            for requirement in descriptor.requires_services.iter().cloned() {
                let capability = HostCapability::RequireService {
                    service: requirement.id.clone(),
                };
                if let Err(error) = require_capability(
                    &grants,
                    &mut report,
                    FeatureContributionKind::Service,
                    requirement.id.to_string(),
                    &capability,
                ) {
                    if requirement.required {
                        report
                            .diagnostics
                            .push(FeatureDiagnostic::error(error.to_string()));
                        required_service_failed = true;
                    } else {
                        report
                            .diagnostics
                            .push(FeatureDiagnostic::warning(error.to_string()));
                    }
                    continue;
                }

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
                match require_background_task_capability(&grants, &mut report, &background_task) {
                    Ok(()) => report.declared_background_tasks.push(background_task),
                    Err(error) => report
                        .diagnostics
                        .push(FeatureDiagnostic::warning(error.to_string())),
                }
            }

            for service in descriptor.provides_services.iter().cloned() {
                match require_service_provider_capability(&grants, &mut report, &service) {
                    Ok(()) => match service_registry
                        .register_provider(descriptor.id.clone(), service.clone())
                    {
                        Ok(()) => report.provided_services.push(service),
                        Err(error) => {
                            report
                                .diagnostics
                                .push(FeatureDiagnostic::error(error.to_string()));
                            report.mark_skipped(
                                FeatureContributionKind::Service,
                                service.id.to_string(),
                                error.to_string(),
                            );
                        }
                    },
                    Err(error) => report
                        .diagnostics
                        .push(FeatureDiagnostic::warning(error.to_string())),
                }
            }

            let install_result = {
                let mut context = FeatureInstallContext {
                    feature_id: &descriptor.id,
                    grants: &grants,
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
        "duplicate service declaration `{service}` from feature `{duplicate_feature}`; first provided by `{first_feature}`"
    )]
    DuplicateService {
        service: String,
        first_feature: String,
        duplicate_feature: String,
    },
    #[error("feature capability denied: {0}")]
    CapabilityDenied(String),
    #[error("feature install failed: {0}")]
    Install(String),
}

/// Builtin task tools feature used to prove existing builtin tool registration
/// through the feature registry without changing tool names, schemas, or
/// permission behavior.
pub mod builtin {
    use super::*;

    pub fn task_feature(task_store: tools::TaskStore) -> impl FeatureModule {
        TaskFeature { task_store }
    }

    struct TaskFeature {
        task_store: tools::TaskStore,
    }

    impl FeatureModule for TaskFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            FeatureDescriptor::builtin("task-tools", "Task tools")
                .with_description("Session-lifetime task tracking builtin tools")
                .with_capability(CapabilityRequest::required(
                    HostCapability::ContributeTool {
                        name: "TaskCreate".into(),
                    },
                    "register TaskCreate builtin tool",
                ))
                .with_capability(CapabilityRequest::required(
                    HostCapability::ContributeTool {
                        name: "TaskUpdate".into(),
                    },
                    "register TaskUpdate builtin tool",
                ))
                .with_capability(CapabilityRequest::required(
                    HostCapability::ContributeTool {
                        name: "TaskGet".into(),
                    },
                    "register TaskGet builtin tool",
                ))
                .with_capability(CapabilityRequest::required(
                    HostCapability::ContributeTool {
                        name: "TaskList".into(),
                    },
                    "register TaskList builtin tool",
                ))
                .with_tool(ToolDeclaration::new(
                    "TaskCreate",
                    "Create a session-lifetime user-visible task",
                ))
                .with_tool(ToolDeclaration::new(
                    "TaskUpdate",
                    "Update a session-lifetime user-visible task",
                ))
                .with_tool(ToolDeclaration::new(
                    "TaskGet",
                    "Get one session-lifetime user-visible task",
                ))
                .with_tool(ToolDeclaration::new(
                    "TaskList",
                    "List session-lifetime user-visible tasks",
                ))
        }

        fn install(
            &self,
            context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            for definition in tools::task_tools(self.task_store.clone()) {
                let name = (definition)().0.name;
                context
                    .tools()
                    .register(ToolContribution::new(name, definition))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use llm_worker::llm_client::{ClientError, Request, ResponseStream};
    use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
    use serde_json::json;

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
        async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
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

    #[test]
    fn descriptor_capabilities_and_install_report_are_recorded() {
        let descriptor = FeatureDescriptor::builtin("dummy", "Dummy")
            .with_capability(CapabilityRequest::required(
                HostCapability::ContributeTool {
                    name: "Dummy".into(),
                },
                "test",
            ))
            .with_capability(CapabilityRequest::required(
                HostCapability::DeclareBackgroundTask {
                    name: "daily".into(),
                },
                "test background task declaration",
            ))
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
        assert!(
            feature_report
                .granted_capabilities
                .contains(&HostCapability::ContributeTool {
                    name: "Dummy".into()
                })
        );
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let descriptor_a = FeatureDescriptor::builtin("a", "A")
            .with_capability(CapabilityRequest::required(
                HostCapability::ContributeTool {
                    name: "Duplicate".into(),
                },
                "test duplicate handling",
            ))
            .with_tool(ToolDeclaration::new("Duplicate", "first tool"));
        let descriptor_b = FeatureDescriptor::builtin("b", "B")
            .with_capability(CapabilityRequest::required(
                HostCapability::ContributeTool {
                    name: "Duplicate".into(),
                },
                "test duplicate handling",
            ))
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
            .with_capability(CapabilityRequest::required(
                HostCapability::ContributeTool {
                    name: "Actual".into(),
                },
                "test mismatch handling",
            ))
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

    struct ServiceFeature {
        descriptor: FeatureDescriptor,
    }

    impl FeatureModule for ServiceFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            self.descriptor.clone()
        }

        fn install(
            &self,
            _context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
            Ok(())
        }
    }

    #[test]
    fn service_requirements_resolve_against_prior_providers() {
        let service = ServiceId::builtin("demo-service");
        let provider = FeatureDescriptor::builtin("provider", "Provider")
            .with_capability(CapabilityRequest::required(
                HostCapability::ProvideService {
                    service: service.clone(),
                },
                "provide demo service",
            ))
            .with_provided_service(ServiceDeclaration::new(
                service.clone(),
                "1",
                "demo service",
            ));
        let consumer = FeatureDescriptor::builtin("consumer", "Consumer")
            .with_capability(CapabilityRequest::required(
                HostCapability::RequireService {
                    service: service.clone(),
                },
                "require demo service",
            ))
            .with_service_requirement(ServiceRequirement::required(service.clone(), "needs demo"));
        let missing_service = ServiceId::builtin("missing-service");
        let missing = FeatureDescriptor::builtin("missing", "Missing")
            .with_capability(CapabilityRequest::required(
                HostCapability::RequireService {
                    service: missing_service.clone(),
                },
                "require missing service",
            ))
            .with_service_requirement(ServiceRequirement::required(
                missing_service,
                "needs missing",
            ));
        let optional_service = ServiceId::builtin("optional-service");
        let optional = FeatureDescriptor::builtin("optional", "Optional")
            .with_capability(CapabilityRequest::required(
                HostCapability::RequireService {
                    service: optional_service.clone(),
                },
                "optionally require service",
            ))
            .with_service_requirement(ServiceRequirement::optional(
                optional_service,
                "nice to have",
            ));
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
                descriptor: missing,
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
        assert!(!report.reports[2].installed);
        assert!(
            report.reports[2]
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("required service requirement"))
        );
        assert!(report.reports[3].installed);
        assert_eq!(
            report.reports[3].skipped[0].kind,
            FeatureContributionKind::Service
        );
    }

    #[test]
    fn background_task_declaration_without_capability_is_skipped() {
        let descriptor = FeatureDescriptor::builtin("background-denied", "Background denied")
            .with_background_task(BackgroundTaskDeclaration::descriptor_only(
                "denied-task",
                "should be skipped",
            ));
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature { descriptor })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.reports[0].installed);
        assert!(report.reports[0].declared_background_tasks.is_empty());
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::BackgroundTask
        );
        assert!(report.reports[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("required capability was not granted")
        }));
    }

    #[test]
    fn service_provider_without_capability_is_skipped() {
        let service = ServiceId::builtin("denied-service");
        let descriptor =
            FeatureDescriptor::builtin("service-denied", "Service denied").with_provided_service(
                ServiceDeclaration::new(service.clone(), "1", "should be skipped"),
            );
        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(ServiceFeature { descriptor })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(report.reports[0].installed);
        assert!(!report.services.provides(&service));
        assert!(report.reports[0].provided_services.is_empty());
        assert_eq!(
            report.reports[0].skipped[0].kind,
            FeatureContributionKind::Service
        );
        assert!(report.reports[0].diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("required capability was not granted")
        }));
    }

    #[test]
    fn builtin_task_feature_installs_through_worker_tool_path() {
        let task_store = tools::TaskStore::new();
        let mut worker = Worker::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(builtin::task_feature(task_store))
            .install_into_worker(&mut worker, &mut hook_builder);

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
