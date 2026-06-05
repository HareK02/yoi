//! Feature contribution registry for Pod-hosted builtin/plugin modules.
//!
//! This module defines the Pod-side feature boundary used to collect
//! descriptor metadata, host authority requests, tool contributions, safe hook
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

/// Host authority requested by a feature for host-mediated operations that can
/// cross sandbox or model-context boundaries.
///
/// Contribution declarations such as tools, hooks, background tasks, and
/// services are descriptor/package-approved host-visible contributions, not
/// host authorities. Host authority grants are additive and do not replace
/// manifest/tool permission checks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAuthority {
    Filesystem,
    Network,
    SecretRef { id: String },
    ModelNotification,
    PodManagement,
    StateStore { name: String },
    ServiceAccess { service: ServiceId },
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

/// Host authority request declared by a feature descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthorityRequest {
    pub authority: HostAuthority,
    pub required: bool,
    pub reason: String,
}

impl HostAuthorityRequest {
    pub fn required(authority: HostAuthority, reason: impl Into<String>) -> Self {
        Self {
            authority,
            required: true,
            reason: reason.into(),
        }
    }

    pub fn optional(authority: HostAuthority, reason: impl Into<String>) -> Self {
        Self {
            authority,
            required: false,
            reason: reason.into(),
        }
    }
}

/// Host authority grants resolved by the host for one feature installation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthorityGrantSet {
    granted: HashSet<HostAuthority>,
    denied: Vec<HostAuthorityDenial>,
}

impl HostAuthorityGrantSet {
    pub fn grant_all(requests: &[HostAuthorityRequest]) -> Self {
        Self {
            granted: requests
                .iter()
                .map(|request| request.authority.clone())
                .collect(),
            denied: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn contains(&self, authority: &HostAuthority) -> bool {
        self.granted.contains(authority)
    }

    pub fn denied(&self) -> &[HostAuthorityDenial] {
        &self.denied
    }

    pub fn grant(&mut self, authority: HostAuthority) {
        self.granted.insert(authority);
    }

    pub fn deny(&mut self, authority: HostAuthority, reason: impl Into<String>) {
        self.granted.remove(&authority);
        self.denied.push(HostAuthorityDenial {
            authority,
            reason: reason.into(),
        });
    }
}

/// Host-side denial of a requested feature host authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthorityDenial {
    pub authority: HostAuthority,
    pub reason: String,
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

/// Executable tool contribution wrapper. Host-authority requirements are optional
/// per-tool gates for privileged host APIs, not permission to contribute a tool.
pub struct ToolContribution {
    name: String,
    definition: ToolDefinition,
    required_host_authorities: Vec<HostAuthority>,
}

impl ToolContribution {
    pub fn new(name: impl Into<String>, definition: ToolDefinition) -> Self {
        Self {
            name: name.into(),
            definition,
            required_host_authorities: Vec::new(),
        }
    }

    pub fn with_required_host_authorities(
        mut self,
        required_host_authorities: Vec<HostAuthority>,
    ) -> Self {
        self.required_host_authorities = required_host_authorities;
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
}

impl HookDeclaration {
    pub fn new(name: impl Into<String>, point: FeatureHookPoint) -> Self {
        Self {
            name: name.into(),
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

/// Contribution service registry skeleton used during feature installation.
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
    pub requested_host_authorities: Vec<HostAuthorityRequest>,
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
            requested_host_authorities: Vec::new(),
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

    pub fn with_host_authority(mut self, request: HostAuthorityRequest) -> Self {
        self.requested_host_authorities.push(request);
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
    pub host_authority_grants: HostAuthorityGrantSet,
    pub installed_tools: Vec<String>,
    pub installed_hooks: Vec<HookDeclaration>,
    pub declared_background_tasks: Vec<BackgroundTaskDeclaration>,
    pub provided_services: Vec<ServiceDeclaration>,
    pub resolved_service_requirements: Vec<ServiceRequirement>,
    pub skipped: Vec<SkippedContribution>,
    pub diagnostics: Vec<FeatureDiagnostic>,
}

impl FeatureInstallReport {
    fn new(descriptor: &FeatureDescriptor, host_authority_grants: HostAuthorityGrantSet) -> Self {
        Self {
            feature_id: descriptor.id.clone(),
            runtime: descriptor.runtime.clone(),
            installed: false,
            host_authority_grants,
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

#[derive(Clone, Debug)]
struct FeatureContributionDeclarations {
    tools: HashSet<String>,
    hooks: HashSet<(String, FeatureHookPoint)>,
    background_tasks: HashSet<String>,
    provided_services: HashSet<(ServiceId, String)>,
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
        }
    }

    fn contains_tool(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    fn contains_hook(&self, declaration: &HookDeclaration) -> bool {
        self.hooks
            .contains(&(declaration.name.clone(), declaration.point.clone()))
    }

    fn contains_background_task(&self, declaration: &BackgroundTaskDeclaration) -> bool {
        self.background_tasks.contains(&declaration.name)
    }

    fn contains_provided_service(&self, declaration: &ServiceDeclaration) -> bool {
        self.provided_services
            .contains(&(declaration.id.clone(), declaration.version.clone()))
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

fn require_host_authority(
    host_authority_grants: &HostAuthorityGrantSet,
    report: &mut FeatureInstallReport,
    kind: FeatureContributionKind,
    name: impl Into<String>,
    authority: &HostAuthority,
) -> Result<(), FeatureInstallError> {
    if host_authority_grants.contains(authority) {
        return Ok(());
    }

    let reason = format!("required host authority was not granted: {authority:?}");
    report.mark_skipped(kind, name, reason.clone());
    Err(FeatureInstallError::HostAuthorityDenied(reason))
}

/// Model-visible durable notification sink skeleton. The first slice exposes
/// the boundary without implementing a new event channel.
pub struct FeatureNotificationSink<'a> {
    host_authority_grants: &'a HostAuthorityGrantSet,
    report: &'a mut FeatureInstallReport,
}

impl FeatureNotificationSink<'_> {
    pub fn notify_model(&mut self, message: impl Into<String>) -> Result<(), FeatureInstallError> {
        require_host_authority(
            self.host_authority_grants,
            self.report,
            FeatureContributionKind::Notification,
            "notify_model",
            &HostAuthority::ModelNotification,
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

/// Tool contribution registrar exposed inside [`FeatureInstallContext`].
pub struct ToolContributionRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    host_authority_grants: &'a HostAuthorityGrantSet,
    pending_tools: &'a mut Vec<ToolDefinition>,
    installed_tool_names: &'a mut HashMap<String, FeatureId>,
    report: &'a mut FeatureInstallReport,
}

impl ToolContributionRegistrar<'_> {
    pub fn register(&mut self, contribution: ToolContribution) -> Result<(), FeatureInstallError> {
        let (tool_meta, tool) = (contribution.definition)();
        let model_visible_name = tool_meta.name.clone();
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

        if !self.declarations.contains_tool(&model_visible_name) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::Tool,
                model_visible_name,
            ));
        }

        for authority in &contribution.required_host_authorities {
            require_host_authority(
                self.host_authority_grants,
                self.report,
                FeatureContributionKind::Tool,
                model_visible_name.clone(),
                authority,
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
        self.pending_tools
            .push(Arc::new(move || (tool_meta.clone(), Arc::clone(&tool))));
        Ok(())
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

/// Service registrar for descriptor/report-only provider metadata.
pub struct FeatureServiceRegistrar<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    service_registry: &'a mut FeatureServiceRegistry,
    report: &'a mut FeatureInstallReport,
}

impl FeatureServiceRegistrar<'_> {
    pub fn provide(&mut self, declaration: ServiceDeclaration) -> Result<(), FeatureInstallError> {
        if !self.declarations.contains_provided_service(&declaration) {
            return Err(reject_undeclared_contribution(
                self.feature_id,
                self.report,
                FeatureContributionKind::Service,
                declaration.id.to_string(),
            ));
        }
        if self
            .report
            .provided_services
            .iter()
            .any(|service| service.id == declaration.id && service.version == declaration.version)
        {
            return Ok(());
        }
        self.service_registry
            .register_provider(self.feature_id.clone(), declaration.clone())?;
        self.report.provided_services.push(declaration);
        Ok(())
    }
}

/// Install-time context provided to a feature module.
pub struct FeatureInstallContext<'a> {
    feature_id: &'a FeatureId,
    declarations: &'a FeatureContributionDeclarations,
    host_authority_grants: &'a HostAuthorityGrantSet,
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

    pub fn host_authority_grants(&self) -> &HostAuthorityGrantSet {
        self.host_authority_grants
    }

    pub fn tools(&mut self) -> ToolContributionRegistrar<'_> {
        ToolContributionRegistrar {
            feature_id: self.feature_id,
            declarations: self.declarations,
            host_authority_grants: self.host_authority_grants,
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

    pub fn notifications(&mut self) -> FeatureNotificationSink<'_> {
        FeatureNotificationSink {
            host_authority_grants: self.host_authority_grants,
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
            let host_authority_grants =
                HostAuthorityGrantSet::grant_all(&descriptor.requested_host_authorities);
            let declarations = FeatureContributionDeclarations::from_descriptor(&descriptor);
            let mut report = FeatureInstallReport::new(&descriptor, host_authority_grants.clone());

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

            for authority in host_authority_grants.denied() {
                report.diagnostics.push(FeatureDiagnostic::warning(format!(
                    "host authority denied: {:?}: {}",
                    authority.authority, authority.reason
                )));
            }

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

            for service in descriptor.provides_services.iter().cloned() {
                match service_registry.register_provider(descriptor.id.clone(), service.clone()) {
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
                }
            }

            let install_result = {
                let mut context = FeatureInstallContext {
                    feature_id: &descriptor.id,
                    declarations: &declarations,
                    host_authority_grants: &host_authority_grants,
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
    #[error("feature host authority denied: {0}")]
    HostAuthorityDenied(String),
    #[error("feature install failed: {0}")]
    Install(String),
}

pub mod builtin;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use llm_worker::llm_client::{ClientError, Request, ResponseStream};
    use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
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
    fn descriptor_contributions_and_empty_host_authority_grants_are_recorded() {
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
        assert!(feature_report.host_authority_grants.denied().is_empty());
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

    #[test]
    fn tool_host_authority_requirements_use_host_authority_grants_not_contribution_declarations() {
        struct HostAuthorityToolFeature {
            descriptor: FeatureDescriptor,
            required_host_authorities: Vec<HostAuthority>,
        }

        impl FeatureModule for HostAuthorityToolFeature {
            fn descriptor(&self) -> FeatureDescriptor {
                self.descriptor.clone()
            }

            fn install(
                &self,
                context: &mut FeatureInstallContext<'_>,
            ) -> Result<(), FeatureInstallError> {
                context.tools().register(
                    ToolContribution::new("NetworkTool", dummy_tool("NetworkTool"))
                        .with_required_host_authorities(self.required_host_authorities.clone()),
                )
            }
        }

        let mut hook_builder = HookRegistryBuilder::default();
        let mut pending_tools = Vec::new();
        let missing_grant = FeatureDescriptor::builtin("missing-host-authority", "Missing")
            .with_tool(ToolDeclaration::new("NetworkTool", "network host API tool"));
        let missing_report = FeatureRegistryBuilder::new()
            .with_module(HostAuthorityToolFeature {
                descriptor: missing_grant,
                required_host_authorities: vec![HostAuthority::Network],
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(pending_tools.is_empty());
        assert!(!missing_report.reports[0].installed);
        assert!(
            missing_report.reports[0]
                .diagnostics
                .iter()
                .any(|diagnostic| {
                    diagnostic
                        .message
                        .contains("required host authority was not granted")
                })
        );
        assert_eq!(
            missing_report.reports[0].skipped[0].kind,
            FeatureContributionKind::Tool
        );

        let granted = FeatureDescriptor::builtin("granted-host-authority", "Granted")
            .with_host_authority(HostAuthorityRequest::required(
                HostAuthority::Network,
                "uses a host network API",
            ))
            .with_tool(ToolDeclaration::new("NetworkTool", "network host API tool"));
        let granted_report = FeatureRegistryBuilder::new()
            .with_module(HostAuthorityToolFeature {
                descriptor: granted,
                required_host_authorities: vec![HostAuthority::Network],
            })
            .install_into_pending(&mut pending_tools, &mut hook_builder);

        assert!(granted_report.reports[0].installed);
        assert!(
            granted_report.reports[0]
                .host_authority_grants
                .contains(&HostAuthority::Network)
        );
        assert_eq!(pending_tools.len(), 1);
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
        let mut worker = Worker::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(StatefulToolFeature {
                calls: Arc::clone(&calls),
            })
            .install_into_worker(&mut worker, &mut hook_builder);

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
            _context: &mut FeatureInstallContext<'_>,
        ) -> Result<(), FeatureInstallError> {
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
            context.services().provide(ServiceDeclaration::new(
                self.service.clone(),
                "1",
                "runtime service provider",
            ))
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
        let missing_service = ServiceId::builtin("missing-service");
        let missing = FeatureDescriptor::builtin("missing", "Missing").with_service_requirement(
            ServiceRequirement::required(missing_service, "needs missing"),
        );
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
    fn background_task_declaration_is_not_host_authority_gated() {
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
    fn service_provider_declaration_is_not_host_authority_gated() {
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
    fn builtin_internal_task_feature_descriptor_has_exact_tools_hooks_and_no_host_authorities() {
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
        assert!(descriptor.requested_host_authorities.is_empty());
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
    fn builtin_internal_task_feature_installs_declared_tools_without_host_authorities() {
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
        assert_eq!(
            report.reports[0].host_authority_grants,
            HostAuthorityGrantSet::empty()
        );
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
        let mut worker = Worker::new(DummyClient);
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(builtin::task_tools_feature())
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
