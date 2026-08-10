//! Plugin package contributions for model-visible Tool schemas.
//!
//! This module registers *enabled* plugin package tool surface definitions and
//! executes Tool calls through the sandboxed Component Model `wasm-component`
//! runtime. It deliberately does not grant filesystem, environment, hook,
//! service, ingress, or ambient network authority. Components can only reach
//! host APIs through explicit imports with matching permissions and scoped
//! allowlist grants.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOrigin, ToolOutput,
};
use manifest::plugin::{
    PLUGIN_COMPONENT_INSTANCE_WORLD, PLUGIN_COMPONENT_TOOL_WORLD, PLUGIN_RUNTIME_COMPONENT_KIND,
    PluginConfig, PluginDiscoveryLimits, PluginFsGrant, PluginFsOperation, PluginHostApi,
    PluginPermission, PluginRequestGrant, PluginSurface, PluginToolManifest, PluginWebSocketGrant,
    ResolvedPluginRecord, read_resolved_plugin_runtime_component,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::runtime::{
    Builder as TokioRuntimeBuilder, Handle as TokioHandle, Runtime as TokioRuntime,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};

const LEGACY_PLUGIN_RUNTIME_WASM_KIND: &str = "wasm";
const PLUGIN_SERVICE_WEBSOCKET_RECV_TIMEOUT: Duration = Duration::from_millis(250);

use super::{
    FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureInstallError, FeatureModule,
    FeatureRuntimeKind, ServiceDeclaration, ServiceId, ToolContribution, ToolDeclaration,
};

/// Build Feature modules for enabled plugin packages when the profile exposes
/// the plugin Tool surface feature.
pub fn plugin_tool_features_if_enabled(
    feature_enabled: bool,
    config: &PluginConfig,
) -> Vec<PluginToolFeature> {
    if !feature_enabled {
        return Vec::new();
    }
    plugin_tool_features(config)
}

/// Build Feature modules for enabled plugin packages that declare Tool/Service/Ingress surfaces.
pub fn plugin_tool_features(config: &PluginConfig) -> Vec<PluginToolFeature> {
    config
        .resolved
        .iter()
        .filter(|record| {
            record.enabled_surfaces.contains(&PluginSurface::Tool)
                || record.enabled_surfaces.contains(&PluginSurface::Service)
                || record.enabled_surfaces.contains(&PluginSurface::Ingress)
        })
        .filter(|record| {
            !record.manifest.tools.is_empty()
                || !record.manifest.services.is_empty()
                || !record.manifest.ingresses.is_empty()
        })
        .cloned()
        .map(PluginToolFeature::new)
        .collect()
}

#[derive(Clone)]
pub struct PluginToolFeature {
    record: ResolvedPluginRecord,
    feature_id: FeatureId,
    registry: PluginInstanceRegistry,
}

impl PluginToolFeature {
    pub fn new(record: ResolvedPluginRecord) -> Self {
        let feature_id = FeatureId::new(format!("plugin:{}:tool", record.identity))
            .expect("source-qualified plugin identity yields non-empty feature id");
        Self {
            record,
            feature_id,
            registry: PluginInstanceRegistry::default(),
        }
    }

    fn ensure_instance(&self) -> Result<PluginInstanceHandle, FeatureInstallError> {
        self.registry.register(self.record.clone())
    }

    pub fn instance_status(&self) -> Option<PluginInstanceStatus> {
        self.registry.status(&self.record.identity.to_string())
    }

    pub fn dispatch_ingress(
        &self,
        ingress_name: &str,
        event: PluginIngressEvent,
    ) -> Result<PluginIngressDispatchReport, PluginIngressDispatchError> {
        if !surface_enabled(&self.record, PluginSurface::Ingress) {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "plugin ingress surface is not enabled".to_string(),
            ));
        }
        let handle = self
            .registry
            .handle(&self.record.identity.to_string())
            .ok_or_else(|| PluginIngressDispatchError::ServiceUnavailable {
                state: PluginInstanceLifecycleState::Stopped,
            })?;
        handle.deliver_ingress(ingress_name, event)
    }

    pub fn origin(&self) -> ToolOrigin {
        ToolOrigin {
            kind: "plugin".into(),
            plugin_id: self.record.manifest.id.clone(),
            plugin_ref: self.record.identity.to_string(),
            source: self.record.identity.source.to_string(),
            digest: self.record.digest.clone(),
            package_version: self.record.version.clone(),
            package_api_version: self.record.manifest.schema_version,
            surface: "tool".into(),
        }
    }
}

fn surface_enabled(record: &ResolvedPluginRecord, surface: PluginSurface) -> bool {
    record.enabled_surfaces.contains(&surface)
}

fn plugin_tool_origin(record: &ResolvedPluginRecord) -> ToolOrigin {
    ToolOrigin {
        kind: "plugin".into(),
        plugin_id: record.manifest.id.clone(),
        plugin_ref: record.identity.to_string(),
        source: record.identity.source.to_string(),
        digest: record.digest.clone(),
        package_version: record.version.clone(),
        package_api_version: record.manifest.schema_version,
        surface: "tool".into(),
    }
}

fn plugin_service_id(record: &ResolvedPluginRecord, name: &str) -> ServiceId {
    ServiceId::new(format!("plugin:{}:{name}", record.identity.to_string()))
        .expect("plugin service id is generated from safe plugin identity/name")
}

/// Static, read-only eligibility information for a resolved plugin package.
///
/// This inspection mirrors the registration-time permission checks without
/// loading the WASM module, calling a plugin Tool, or executing plugin code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginStaticInspection {
    pub runtime: PluginRuntimeEligibility,
    pub host_apis: Vec<PluginPermissionEligibility>,
    pub tools: Vec<PluginToolEligibility>,
    pub services: Vec<PluginSurfaceEligibility>,
    pub ingresses: Vec<PluginSurfaceEligibility>,
}

impl PluginStaticInspection {
    pub fn statically_eligible(&self) -> bool {
        self.runtime.eligible
            && self.host_apis.iter().all(|api| api.eligible)
            && self.tools.iter().all(|tool| tool.eligible)
            && self.services.iter().all(|service| service.eligible)
            && self.ingresses.iter().all(|ingress| ingress.eligible)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginRuntimeEligibility {
    pub eligible: bool,
    pub status: String,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginPermissionEligibility {
    pub permission: String,
    pub requested: bool,
    pub granted: bool,
    pub eligible: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginToolEligibility {
    pub name: String,
    pub permission: String,
    pub requested: bool,
    pub granted: bool,
    pub eligible: bool,
    pub external_write: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginSurfaceEligibility {
    pub name: String,
    pub permission: String,
    pub requested: bool,
    pub granted: bool,
    pub eligible: bool,
    pub diagnostic: Option<String>,
}

/// Inspect static plugin runtime/tool eligibility without executing plugin code.
pub fn inspect_resolved_plugin_static(record: &ResolvedPluginRecord) -> PluginStaticInspection {
    let runtime = match &record.manifest.runtime {
        Some(runtime)
            if runtime.kind == PLUGIN_RUNTIME_COMPONENT_KIND
                && matches!(
                    runtime.world.as_deref(),
                    Some(PLUGIN_COMPONENT_TOOL_WORLD) | Some(PLUGIN_COMPONENT_INSTANCE_WORLD)
                )
                && runtime.component.is_some() =>
        {
            PluginRuntimeEligibility {
                eligible: true,
                status: format!(
                    "{}/{}",
                    PLUGIN_RUNTIME_COMPONENT_KIND,
                    runtime
                        .world
                        .as_deref()
                        .unwrap_or(PLUGIN_COMPONENT_TOOL_WORLD)
                ),
                diagnostic: None,
            }
        }
        Some(runtime) if runtime.kind == LEGACY_PLUGIN_RUNTIME_WASM_KIND => {
            let status = runtime
                .abi
                .as_deref()
                .map(|abi| format!("{LEGACY_PLUGIN_RUNTIME_WASM_KIND}/{abi}"))
                .unwrap_or_else(|| format!("{LEGACY_PLUGIN_RUNTIME_WASM_KIND}/<missing-abi>"));
            PluginRuntimeEligibility {
                eligible: false,
                status,
                diagnostic: Some(
                    "legacy raw wasm plugin runtime is not an active execution path; use wasm-component"
                        .to_string(),
                ),
            }
        }
        Some(runtime) if runtime.kind == PLUGIN_RUNTIME_COMPONENT_KIND => {
            let status = runtime
                .world
                .as_deref()
                .map(|world| format!("{PLUGIN_RUNTIME_COMPONENT_KIND}/{world}"))
                .unwrap_or_else(|| format!("{PLUGIN_RUNTIME_COMPONENT_KIND}/<missing-world>"));
            PluginRuntimeEligibility {
                eligible: false,
                status,
                diagnostic: Some("unsupported or missing plugin component world".to_string()),
            }
        }
        Some(runtime) => PluginRuntimeEligibility {
            eligible: false,
            status: runtime.kind.clone(),
            diagnostic: Some(format!(
                "unsupported plugin runtime kind `{}`",
                runtime.kind
            )),
        },
        None => PluginRuntimeEligibility {
            eligible: false,
            status: "none".to_string(),
            diagnostic: Some("plugin runtime is not declared".to_string()),
        },
    };

    let mut host_apis: Vec<_> = [
        PluginHostApi::Request,
        PluginHostApi::WebSocket,
        PluginHostApi::Fs,
    ]
    .into_iter()
    .filter_map(|api| {
        let permission = PluginPermission::host_api(api);
        let requested = permission_requested(record, &permission);
        let granted = grant_allows(record, &permission);
        if !requested && !granted {
            return None;
        }
        let diagnostic = authorize_plugin_host_api(record, api)
            .err()
            .map(|error| error.bounded_message());
        Some(PluginPermissionEligibility {
            permission: permission.label(),
            requested,
            granted,
            eligible: diagnostic.is_none(),
            diagnostic,
        })
    })
    .collect();
    append_request_target_inspection(record, &mut host_apis);
    append_websocket_target_inspection(record, &mut host_apis);

    let duplicate_tool_names = duplicate_tool_names(record);
    let tools = if surface_enabled(record, PluginSurface::Tool) {
        record
            .manifest
            .tools
            .iter()
            .map(|tool| {
                let permission = PluginPermission::tool(&tool.name);
                let requested = permission_requested(record, &permission);
                let granted = grant_allows(record, &permission);
                let mut diagnostics = validate_plugin_tool_definition(tool, &duplicate_tool_names);
                if let Err(error) = authorize_plugin_tool(record, tool) {
                    diagnostics.push(error.bounded_message());
                }
                let diagnostic = join_tool_diagnostics(diagnostics);
                PluginToolEligibility {
                    name: tool.name.clone(),
                    permission: permission.label(),
                    requested,
                    granted,
                    eligible: diagnostic.is_none(),
                    external_write: tool.external_write,
                    diagnostic,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let instance_world = record.manifest.runtime.as_ref().is_some_and(|runtime| {
        runtime.kind == PLUGIN_RUNTIME_COMPONENT_KIND
            && runtime.world.as_deref() == Some(PLUGIN_COMPONENT_INSTANCE_WORLD)
    });
    let services = if surface_enabled(record, PluginSurface::Service) {
        record
            .manifest
            .services
            .iter()
            .map(|service| {
                let permission = PluginPermission::service(&service.name);
                let requested = permission_requested(record, &permission);
                let granted = grant_allows(record, &permission);
                let mut diagnostics = Vec::new();
                if !instance_world {
                    diagnostics
                        .push("service requires instance-capable component world".to_string());
                }
                if let Err(error) = authorize_plugin_service(record, &service.name) {
                    diagnostics.push(error.bounded_message());
                }
                let diagnostic = join_tool_diagnostics(diagnostics);
                PluginSurfaceEligibility {
                    name: service.name.clone(),
                    permission: permission.label(),
                    requested,
                    granted,
                    eligible: diagnostic.is_none(),
                    diagnostic,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let ingresses = if surface_enabled(record, PluginSurface::Ingress) {
        record
            .manifest
            .ingresses
            .iter()
            .map(|ingress| {
                let permission = PluginPermission::ingress(&ingress.name);
                let requested = permission_requested(record, &permission);
                let granted = grant_allows(record, &permission);
                let mut diagnostics = Vec::new();
                if !instance_world {
                    diagnostics
                        .push("ingress requires instance-capable component world".to_string());
                }
                if let Err(error) = authorize_plugin_ingress(record, &ingress.name) {
                    diagnostics.push(error.bounded_message());
                }
                let diagnostic = join_tool_diagnostics(diagnostics);
                PluginSurfaceEligibility {
                    name: ingress.name.clone(),
                    permission: permission.label(),
                    requested,
                    granted,
                    eligible: diagnostic.is_none(),
                    diagnostic,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    PluginStaticInspection {
        runtime,
        host_apis,
        tools,
        services,
        ingresses,
    }
}

fn permission_requested(record: &ResolvedPluginRecord, permission: &PluginPermission) -> bool {
    record
        .manifest
        .permissions
        .iter()
        .any(|requested| requested == permission)
}

fn append_request_target_inspection(
    record: &ResolvedPluginRecord,
    host_apis: &mut Vec<PluginPermissionEligibility>,
) {
    for target in &record.manifest.request {
        let covering_grant = record
            .grants
            .request
            .iter()
            .find(|grant| request_target_covers(grant, target));
        let intersecting_grant = covering_grant.or_else(|| {
            record
                .grants
                .request
                .iter()
                .find(|grant| request_targets_intersect(target, grant))
        });
        let granted = intersecting_grant.is_some();
        let diagnostic = match (granted, covering_grant, target.is_broad()) {
            (false, _, broad) => Some(format!(
                "missing enabled request grant for manifest target{}",
                if broad { "; broad/arbitrary target" } else { "" }
            )),
            (true, None, true) => Some(
                "partially covered by enabled request grant; broad manifest target is constrained by narrower grants"
                    .to_string(),
            ),
            (true, None, false) => Some(
                "partially covered by enabled request grant; only intersecting URLs are allowed"
                    .to_string(),
            ),
            (true, Some(grant), _) if grant.is_broad() => {
                Some("covered by broad/arbitrary enabled request grant".to_string())
            }
            _ => None,
        };
        host_apis.push(PluginPermissionEligibility {
            permission: format!("host_api.request target {}", target.label()),
            requested: true,
            granted,
            eligible: granted,
            diagnostic,
        });
    }
    for grant in &record.grants.request {
        let matching_manifest = record
            .manifest
            .request
            .iter()
            .find(|target| request_targets_intersect(target, grant));
        if let Some(target) = matching_manifest {
            let diagnostic = if grant.is_broad() {
                Some(
                    "broad/arbitrary enabled request grant is constrained by manifest declarations"
                        .to_string(),
                )
            } else if !request_target_covers(target, grant) {
                Some(
                    "enabled request grant is only usable where it intersects manifest declarations"
                        .to_string(),
                )
            } else {
                None
            };
            host_apis.push(PluginPermissionEligibility {
                permission: format!("host_api.request grant {}", grant.label()),
                requested: true,
                granted: true,
                eligible: true,
                diagnostic,
            });
        } else {
            let broad = if grant.is_broad() {
                "; broad/arbitrary target"
            } else {
                ""
            };
            host_apis.push(PluginPermissionEligibility {
                permission: format!("host_api.request grant-only {}", grant.label()),
                requested: false,
                granted: true,
                eligible: false,
                diagnostic: Some(format!(
                    "enabled request grant has no matching manifest declaration{broad}"
                )),
            });
        }
    }
}

fn append_websocket_target_inspection(
    record: &ResolvedPluginRecord,
    host_apis: &mut Vec<PluginPermissionEligibility>,
) {
    for target in &record.manifest.websocket {
        let covering_grant = record
            .grants
            .websocket
            .iter()
            .find(|grant| websocket_target_covers(grant, target));
        let intersecting_grant = covering_grant.or_else(|| {
            record
                .grants
                .websocket
                .iter()
                .find(|grant| websocket_targets_intersect(target, grant))
        });
        let granted = intersecting_grant.is_some();
        let diagnostic = match (granted, covering_grant, target.is_broad()) {
            (false, _, broad) => Some(format!(
                "missing enabled WebSocket grant for manifest target{}",
                if broad { "; broad/arbitrary target" } else { "" }
            )),
            (true, None, true) => Some(
                "partially covered by enabled WebSocket grant; broad manifest target is constrained by narrower grants"
                    .to_string(),
            ),
            (true, None, false) => Some(
                "partially covered by enabled WebSocket grant; only intersecting URLs are allowed"
                    .to_string(),
            ),
            (true, Some(grant), _) if grant.is_broad() => {
                Some("covered by broad/arbitrary enabled WebSocket grant".to_string())
            }
            _ => None,
        };
        host_apis.push(PluginPermissionEligibility {
            permission: format!("host_api.websocket target {}", target.label()),
            requested: true,
            granted,
            eligible: granted,
            diagnostic,
        });
    }
    for grant in &record.grants.websocket {
        let matching_manifest = record
            .manifest
            .websocket
            .iter()
            .find(|target| websocket_targets_intersect(target, grant));
        if let Some(target) = matching_manifest {
            let diagnostic = if grant.is_broad() {
                Some(
                    "broad/arbitrary enabled WebSocket grant is constrained by manifest declarations"
                        .to_string(),
                )
            } else if !websocket_target_covers(target, grant) {
                Some(
                    "enabled WebSocket grant is only usable where it intersects manifest declarations"
                        .to_string(),
                )
            } else {
                None
            };
            host_apis.push(PluginPermissionEligibility {
                permission: format!("host_api.websocket grant {}", grant.label()),
                requested: true,
                granted: true,
                eligible: true,
                diagnostic,
            });
        } else {
            let broad = if grant.is_broad() {
                "; broad/arbitrary target"
            } else {
                ""
            };
            host_apis.push(PluginPermissionEligibility {
                permission: format!("host_api.websocket grant-only {}", grant.label()),
                requested: false,
                granted: true,
                eligible: false,
                diagnostic: Some(format!(
                    "enabled WebSocket grant has no matching manifest declaration{broad}"
                )),
            });
        }
    }
}

fn grant_allows(record: &ResolvedPluginRecord, permission: &PluginPermission) -> bool {
    record
        .grants
        .permissions
        .iter()
        .any(|granted| granted == permission)
}

fn duplicate_tool_names(record: &ResolvedPluginRecord) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();
    for tool in &record.manifest.tools {
        if !seen.insert(tool.name.clone()) {
            duplicates.insert(tool.name.clone());
        }
    }
    duplicates
}

fn validate_plugin_tool_definition(
    tool: &PluginToolManifest,
    duplicate_tool_names: &HashSet<String>,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if duplicate_tool_names.contains(&tool.name) {
        diagnostics.push(format!(
            "tool `{}` has duplicate name within plugin manifest",
            tool.name
        ));
    }
    if let Err(reason) = validate_tool_name(&tool.name) {
        diagnostics.push(format!("tool `{}` has invalid name: {reason}", tool.name));
    }
    if let Err(reason) = validate_input_schema(&tool.input_schema) {
        diagnostics.push(format!(
            "tool `{}` has invalid input_schema: {reason}",
            tool.name
        ));
    }
    diagnostics
}

fn join_tool_diagnostics(diagnostics: Vec<String>) -> Option<String> {
    if diagnostics.is_empty() {
        None
    } else {
        Some(bounded_message(diagnostics.join("; ")))
    }
}

impl FeatureModule for PluginToolFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor =
            FeatureDescriptor {
                id: self.feature_id.clone(),
                runtime: FeatureRuntimeKind::ExternalPlugin,
                display_name: self.record.manifest.name.clone(),
                version: self.record.manifest.version.clone(),
                description: self.record.manifest.description.clone().unwrap_or_else(|| {
                    format!("Plugin tool surface from {}", self.record.identity)
                }),
                tools: Vec::new(),
                hooks: Vec::new(),
                instructions: Vec::new(),
                background_tasks: Vec::new(),
                provides_services: Vec::new(),
                requires_services: Vec::new(),
                protocol_providers: Vec::new(),
            };
        if surface_enabled(&self.record, PluginSurface::Service) {
            for service in &self.record.manifest.services {
                descriptor.provides_services.push(ServiceDeclaration::new(
                    plugin_service_id(&self.record, &service.name),
                    self.record.manifest.version.clone(),
                    service.description.clone(),
                ));
            }
        }
        if surface_enabled(&self.record, PluginSurface::Tool) {
            for tool in &self.record.manifest.tools {
                descriptor = descriptor.with_tool(ToolDeclaration::new(
                    tool.name.clone(),
                    tool.description.clone(),
                ));
            }
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        if surface_enabled(&self.record, PluginSurface::Tool) {
            validate_declared_tool_names(&self.record)?;
        }
        let mut instance: Option<PluginInstanceHandle> = None;
        let mut exposed = 0usize;
        let mut denied = Vec::new();
        if surface_enabled(&self.record, PluginSurface::Service) {
            for service in &self.record.manifest.services {
                validate_tool_name(&service.name).map_err(|reason| {
                    FeatureInstallError::Install(format!(
                        "plugin {} service {} has invalid name: {reason}",
                        self.record.identity, service.name
                    ))
                })?;
                if let Err(error) = authorize_plugin_service(&self.record, &service.name) {
                    let message = format!(
                        "plugin {} service {} registration denied: {}",
                        self.record.identity,
                        service.name,
                        error.bounded_message()
                    );
                    context.diagnostics().warning(message.clone());
                    denied.push(message);
                    continue;
                }
                if instance.is_none() {
                    instance = Some(self.ensure_instance()?);
                }
                context.services().provide(ServiceDeclaration::new(
                    plugin_service_id(&self.record, &service.name),
                    self.record.manifest.version.clone(),
                    service.description.clone(),
                ))?;
                exposed += 1;
            }
        }
        if surface_enabled(&self.record, PluginSurface::Ingress) {
            for ingress in &self.record.manifest.ingresses {
                validate_tool_name(&ingress.name).map_err(|reason| {
                    FeatureInstallError::Install(format!(
                        "plugin {} ingress {} has invalid name: {reason}",
                        self.record.identity, ingress.name
                    ))
                })?;
                if let Err(error) = authorize_plugin_ingress(&self.record, &ingress.name) {
                    let message = format!(
                        "plugin {} ingress {} registration denied: {}",
                        self.record.identity,
                        ingress.name,
                        error.bounded_message()
                    );
                    context.diagnostics().warning(message.clone());
                    denied.push(message);
                } else {
                    if instance.is_none() {
                        instance = Some(self.ensure_instance()?);
                    }
                    exposed += 1;
                }
            }
        }
        if surface_enabled(&self.record, PluginSurface::Tool) {
            for tool in &self.record.manifest.tools {
                validate_tool_name(&tool.name).map_err(|reason| {
                    FeatureInstallError::Install(format!(
                        "plugin {} tool {} has invalid name: {reason}",
                        self.record.identity, tool.name
                    ))
                })?;
                validate_input_schema(&tool.input_schema).map_err(|reason| {
                    FeatureInstallError::Install(format!(
                        "plugin {} tool {} has invalid input_schema: {reason}",
                        self.record.identity, tool.name
                    ))
                })?;
                if let Err(error) = authorize_plugin_tool(&self.record, tool) {
                    let message = format!(
                        "plugin {} tool {} registration denied: {}",
                        self.record.identity,
                        tool.name,
                        error.bounded_message()
                    );
                    context.diagnostics().warning(message.clone());
                    denied.push(message);
                    continue;
                }
                let tool_instance = match &instance {
                    Some(instance) => instance.clone(),
                    None => {
                        let created = self.ensure_instance()?;
                        instance = Some(created.clone());
                        created
                    }
                };
                context.tools().register(ToolContribution::new(
                    tool.name.clone(),
                    plugin_instance_tool_definition(
                        tool_instance,
                        tool.name.clone(),
                        tool.description.clone(),
                        tool.input_schema.clone(),
                    ),
                ))?;
                exposed += 1;
            }
        }
        if exposed == 0 && !denied.is_empty() {
            let summary = if denied.len() == 1 {
                denied.remove(0)
            } else {
                format!(
                    "{} plugin tool registrations denied; first denial: {}",
                    denied.len(),
                    denied[0]
                )
            };
            return Err(FeatureInstallError::Install(bounded_message(summary)));
        }
        Ok(())
    }
}

impl PluginRequestClient for ReqwestPluginRequestClient {
    fn execute(
        &self,
        request: &PluginRequestRequest,
        url: &reqwest::Url,
        limits: PluginRequestLimits,
    ) -> Result<PluginRequestResponse, PluginRequestError> {
        let pinned_resolution =
            resolve_request_target_for_client(url, &SystemPluginRequestResolver)?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            PluginRequestError::new(format!("unsupported request method `{}`", request.method))
        })?;
        let mut client_builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(limits.timeout)
            .no_proxy()
            .user_agent("yoi-plugin-request-host-api/0.1");
        if let Some(pinned_resolution) = &pinned_resolution {
            for domain in &pinned_resolution.domains {
                client_builder = client_builder.resolve_to_addrs(domain, &pinned_resolution.addrs);
            }
        }
        let client = client_builder.build().map_err(|error| {
            PluginRequestError::new(format!("request client build failed: {error}"))
        })?;
        let mut builder = client.request(method, url.clone()).timeout(limits.timeout);
        for header in &request.headers {
            let name =
                reqwest::header::HeaderName::from_bytes(header.name.as_bytes()).map_err(|_| {
                    PluginRequestError::new(format!(
                        "invalid request header name `{}`",
                        header.name
                    ))
                })?;
            let value = reqwest::header::HeaderValue::from_str(&header.value).map_err(|_| {
                PluginRequestError::new(format!(
                    "invalid request header value for `{}`",
                    header.name
                ))
            })?;
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let mut response = builder.send().map_err(|error| {
            if error.is_timeout() {
                PluginRequestError::new(format!("request to {} timed out", safe_url(url)))
            } else {
                PluginRequestError::new(format!("request to {} failed: {error}", safe_url(url)))
            }
        })?;
        let status = response.status().as_u16();
        let headers = collect_request_response_headers(response.headers());
        let mut body = Vec::new();
        let read_limit = limits.max_response_bytes.saturating_add(1) as u64;
        response
            .by_ref()
            .take(read_limit)
            .read_to_end(&mut body)
            .map_err(|error| {
                PluginRequestError::new(format!("request response read failed: {error}"))
            })?;
        let truncated = body.len() > limits.max_response_bytes;
        if truncated {
            body.truncate(limits.max_response_bytes);
        }
        Ok(PluginRequestResponse {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
            truncated,
        })
    }
}

fn execute_plugin_request_request(
    record: &ResolvedPluginRecord,
    client: &dyn PluginRequestClient,
    request_bytes: &[u8],
) -> Result<Vec<u8>, PluginRequestError> {
    if request_bytes.len() > PLUGIN_REQUEST_MAX_REQUEST_BYTES {
        return Err(PluginRequestError::new(format!(
            "request descriptor exceeds {} bytes",
            PLUGIN_REQUEST_MAX_REQUEST_BYTES
        )));
    }
    authorize_plugin_host_api(record, PluginHostApi::Request).map_err(|error| {
        PluginRequestError::new(format!(
            "plugin host API dispatch denied: {}",
            error.bounded_message()
        ))
    })?;
    let request: PluginRequestRequest = serde_json::from_slice(request_bytes)
        .map_err(|error| PluginRequestError::new(format!("invalid request JSON: {error}")))?;
    let url = validate_plugin_request_request(record, &request)?;
    let mut response = client.execute(&request, &url, PluginRequestLimits::default())?;
    enforce_request_response_bounds(&mut response, PluginRequestLimits::default());
    serde_json::to_vec(&response).map_err(|error| {
        PluginRequestError::new(format!("failed to encode request response: {error}"))
    })
}

fn execute_plugin_websocket_open(
    record: &ResolvedPluginRecord,
    client: &dyn PluginWebSocketClient,
    handles: &PluginWebSocketHandles,
    bytes: &[u8],
) -> Result<Vec<u8>, PluginWebSocketError> {
    let (request, url) = validate_plugin_websocket_open_request(record, bytes)?;
    let limits = PluginWebSocketLimits::default();
    if !client.supports_bounded_open() {
        return Err(PluginWebSocketError::new(
            "host_api.websocket client cannot guarantee bounded/cancellable open; refusing to dial",
        ));
    }
    let reservation = handles.reserve_open()?;
    let connection = client.open(&request, &url, limits)?;
    let handle = reservation.commit(connection)?;
    serde_json::to_vec(&PluginWebSocketOpenResponse {
        handle,
        url: safe_url(&url),
    })
    .map_err(|error| PluginWebSocketError::new(error.to_string()))
}

fn execute_plugin_websocket_send_text(
    handles: &PluginWebSocketHandles,
    handle: u32,
    bytes: &[u8],
) -> Result<Vec<u8>, PluginWebSocketError> {
    if bytes.len() > PLUGIN_WEBSOCKET_MAX_TEXT_BYTES {
        return Err(PluginWebSocketError::new(format!(
            "WebSocket text frame exceeds {} bytes",
            PLUGIN_WEBSOCKET_MAX_TEXT_BYTES
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PluginWebSocketError::new("WebSocket send_text requires UTF-8 text"))?;
    handles.with_connection(handle, |connection| connection.send_text(text))?;
    serde_json::to_vec(&PluginWebSocketSendResponse {
        sent: true,
        bytes: bytes.len(),
    })
    .map_err(|error| PluginWebSocketError::new(error.to_string()))
}

fn execute_plugin_websocket_recv(
    handles: &PluginWebSocketHandles,
    handle: u32,
    timeout_ms: u32,
) -> Result<Vec<u8>, PluginWebSocketError> {
    let timeout = websocket_timeout(timeout_ms);
    let response = handles.with_connection(handle, |connection| {
        connection.recv_text(timeout, PLUGIN_WEBSOCKET_MAX_MESSAGE_BYTES)
    })?;
    serde_json::to_vec(&response).map_err(|error| PluginWebSocketError::new(error.to_string()))
}

fn execute_plugin_websocket_close(
    handles: &PluginWebSocketHandles,
    handle: u32,
) -> Result<Vec<u8>, PluginWebSocketError> {
    let closed = handles.close(handle)?;
    serde_json::to_vec(&PluginWebSocketCloseResponse { closed })
        .map_err(|error| PluginWebSocketError::new(error.to_string()))
}
fn execute_plugin_fs_request(
    record: &ResolvedPluginRecord,
    operation: PluginFsRuntimeOperation,
    request_bytes: &[u8],
) -> Result<Vec<u8>, PluginFsError> {
    if request_bytes.len() > PLUGIN_FS_MAX_REQUEST_BYTES {
        return Err(PluginFsError::new(format!(
            "FS request descriptor exceeds {} bytes",
            PLUGIN_FS_MAX_REQUEST_BYTES
        )));
    }
    authorize_plugin_host_api(record, PluginHostApi::Fs).map_err(|error| {
        PluginFsError::new(format!(
            "plugin host API dispatch denied: {}",
            error.bounded_message()
        ))
    })?;

    match operation {
        PluginFsRuntimeOperation::Read => {
            let request: PluginFsPathRequest =
                serde_json::from_slice(request_bytes).map_err(|error| {
                    PluginFsError::new(format!("invalid FS read request JSON: {error}"))
                })?;
            execute_plugin_fs_read(record, &request.path)
        }
        PluginFsRuntimeOperation::List => {
            let request: PluginFsPathRequest =
                serde_json::from_slice(request_bytes).map_err(|error| {
                    PluginFsError::new(format!("invalid FS list request JSON: {error}"))
                })?;
            execute_plugin_fs_list(record, &request.path)
        }
        PluginFsRuntimeOperation::Write => {
            let request: PluginFsWriteRequest =
                serde_json::from_slice(request_bytes).map_err(|error| {
                    PluginFsError::new(format!("invalid FS write request JSON: {error}"))
                })?;
            execute_plugin_fs_write(record, &request.path, request.content.as_bytes())
        }
    }
}

fn execute_plugin_fs_read(
    record: &ResolvedPluginRecord,
    path: &str,
) -> Result<Vec<u8>, PluginFsError> {
    let target = authorize_fs_path(record, PluginFsRuntimeOperation::Read, path)?;
    let meta = fs::metadata(&target.resolved).map_err(|error| {
        PluginFsError::new(format!(
            "FS read metadata failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    if !meta.is_file() {
        return Err(PluginFsError::new(format!(
            "FS read target is not a regular file: {}",
            safe_fs_path(path)
        )));
    }
    let mut file = fs::File::open(&target.resolved).map_err(|error| {
        PluginFsError::new(format!(
            "FS read failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take((PLUGIN_FS_MAX_READ_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            PluginFsError::new(format!(
                "FS read failed for {}: {error}",
                safe_fs_path(path)
            ))
        })?;
    let truncated = bytes.len() > PLUGIN_FS_MAX_READ_BYTES;
    if truncated {
        bytes.truncate(PLUGIN_FS_MAX_READ_BYTES);
    }
    let response = PluginFsReadResponse {
        path: safe_fs_path(path),
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    };
    serde_json::to_vec(&response)
        .map_err(|error| PluginFsError::new(format!("failed to encode FS read response: {error}")))
}

fn execute_plugin_fs_list(
    record: &ResolvedPluginRecord,
    path: &str,
) -> Result<Vec<u8>, PluginFsError> {
    let target = authorize_fs_path(record, PluginFsRuntimeOperation::List, path)?;
    let meta = fs::metadata(&target.resolved).map_err(|error| {
        PluginFsError::new(format!(
            "FS list metadata failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    if !meta.is_dir() {
        return Err(PluginFsError::new(format!(
            "FS list target is not a directory: {}",
            safe_fs_path(path)
        )));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fs::read_dir(&target.resolved).map_err(|error| {
        PluginFsError::new(format!(
            "FS list failed for {}: {error}",
            safe_fs_path(path)
        ))
    })? {
        let entry = entry.map_err(|error| {
            PluginFsError::new(format!(
                "FS list failed for {}: {error}",
                safe_fs_path(path)
            ))
        })?;
        if entries.len() >= PLUGIN_FS_MAX_LIST_ENTRIES {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type().map_err(|error| {
            PluginFsError::new(format!(
                "FS list failed for {}: {error}",
                safe_fs_path(path)
            ))
        })?;
        let kind = if file_type.is_dir() {
            "dir"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_symlink() {
            "symlink"
        } else {
            "other"
        };
        entries.push(PluginFsDirEntry {
            name,
            kind: kind.to_string(),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let response = PluginFsListResponse {
        path: safe_fs_path(path),
        entries,
        truncated,
    };
    serde_json::to_vec(&response)
        .map_err(|error| PluginFsError::new(format!("failed to encode FS list response: {error}")))
}

fn execute_plugin_fs_write(
    record: &ResolvedPluginRecord,
    path: &str,
    content: &[u8],
) -> Result<Vec<u8>, PluginFsError> {
    if content.len() > PLUGIN_FS_MAX_WRITE_BYTES {
        return Err(PluginFsError::new(format!(
            "FS write content exceeds {} bytes",
            PLUGIN_FS_MAX_WRITE_BYTES
        )));
    }
    let target = authorize_fs_write_path(record, path)?;
    let lock = plugin_fs_write_lock(target.lock_key.clone());
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&target.resolved).map_err(|error| {
        PluginFsError::new(format!(
            "FS write failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    file.write_all(content).map_err(|error| {
        PluginFsError::new(format!(
            "FS write failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    file.sync_all().map_err(|error| {
        PluginFsError::new(format!(
            "FS write failed for {}: {error}",
            safe_fs_path(path)
        ))
    })?;
    let response = PluginFsWriteResponse {
        path: safe_fs_path(path),
        bytes_written: content.len(),
    };
    serde_json::to_vec(&response)
        .map_err(|error| PluginFsError::new(format!("failed to encode FS write response: {error}")))
}

fn enforce_request_response_bounds(
    response: &mut PluginRequestResponse,
    limits: PluginRequestLimits,
) {
    if response.body.len() > limits.max_response_bytes {
        truncate_string_to_boundary(&mut response.body, limits.max_response_bytes);
        response.truncated = true;
    }
    if response.headers.len() > PLUGIN_REQUEST_MAX_RESPONSE_HEADERS {
        response
            .headers
            .truncate(PLUGIN_REQUEST_MAX_RESPONSE_HEADERS);
    }
    for header in &mut response.headers {
        header.value = bounded_header_value(&header.value);
    }
}

fn truncate_string_to_boundary(value: &mut String, max_len: usize) {
    if value.len() <= max_len {
        return;
    }
    let mut boundary = max_len;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

#[derive(Clone, Debug)]
struct PluginFsResolvedPath {
    resolved: PathBuf,
}

#[derive(Clone, Debug)]
struct PluginFsWritePath {
    resolved: PathBuf,
    lock_key: PathBuf,
}

fn authorize_fs_path(
    record: &ResolvedPluginRecord,
    operation: PluginFsRuntimeOperation,
    path: &str,
) -> Result<PluginFsResolvedPath, PluginFsError> {
    let relative = parse_plugin_fs_relative_path(path)?;
    let mut saw_operation_grant = false;
    let mut last_error: Option<PluginFsError> = None;
    for grant in &record.grants.fs {
        if !grant_allows_operation(grant, operation) {
            continue;
        }
        saw_operation_grant = true;
        match resolve_plugin_fs_grant_root(grant)
            .and_then(|root| resolve_existing_plugin_fs_path(&root, &relative, safe_fs_path(path)))
        {
            Ok(resolved) => return Ok(PluginFsResolvedPath { resolved }),
            Err(error) => last_error = Some(error),
        }
    }
    if !saw_operation_grant {
        return Err(PluginFsError::new(format!(
            "host_api.fs {} denied: no matching operation grant",
            operation.as_str()
        )));
    }
    Err(last_error.unwrap_or_else(|| {
        PluginFsError::new(format!(
            "host_api.fs {} denied by scoped path policy",
            operation.as_str()
        ))
    }))
}

fn authorize_fs_write_path(
    record: &ResolvedPluginRecord,
    path: &str,
) -> Result<PluginFsWritePath, PluginFsError> {
    let relative = parse_plugin_fs_relative_path(path)?;
    if relative.as_os_str().is_empty() || relative.file_name().is_none() {
        return Err(PluginFsError::new("FS write path must name a file"));
    }
    let mut saw_operation_grant = false;
    let mut last_error: Option<PluginFsError> = None;
    for grant in &record.grants.fs {
        if !grant_allows_operation(grant, PluginFsRuntimeOperation::Write) {
            continue;
        }
        saw_operation_grant = true;
        match resolve_plugin_fs_grant_root(grant)
            .and_then(|root| resolve_writable_plugin_fs_path(&root, &relative, safe_fs_path(path)))
        {
            Ok(resolved) => return Ok(resolved),
            Err(error) => last_error = Some(error),
        }
    }
    if !saw_operation_grant {
        return Err(PluginFsError::new(
            "host_api.fs write denied: no matching operation grant",
        ));
    }
    Err(last_error
        .unwrap_or_else(|| PluginFsError::new("host_api.fs write denied by scoped path policy")))
}

fn grant_allows_operation(grant: &PluginFsGrant, operation: PluginFsRuntimeOperation) -> bool {
    let requested = operation.grant_operation();
    grant.operations.iter().any(|allowed| *allowed == requested)
}

fn parse_plugin_fs_relative_path(path: &str) -> Result<PathBuf, PluginFsError> {
    if path.as_bytes().len() > PLUGIN_FS_MAX_PATH_BYTES {
        return Err(PluginFsError::new(format!(
            "FS path exceeds {} bytes",
            PLUGIN_FS_MAX_PATH_BYTES
        )));
    }
    if path.is_empty() {
        return Err(PluginFsError::new("FS path must not be empty"));
    }
    if path.contains('\0') {
        return Err(PluginFsError::new("FS path contains a NUL byte"));
    }
    let input = Path::new(path);
    if input.is_absolute() {
        return Err(PluginFsError::new(
            "FS request paths must be relative to a configured fs grant root",
        ));
    }
    let mut relative = PathBuf::new();
    for component in input.components() {
        match component {
            Component::Normal(component) => relative.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PluginFsError::new(
                    "FS path traversal outside the grant root is denied",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PluginFsError::new(
                    "FS request paths must be relative to a configured fs grant root",
                ));
            }
        }
    }
    Ok(relative)
}

fn resolve_plugin_fs_grant_root(grant: &PluginFsGrant) -> Result<PathBuf, PluginFsError> {
    let root = grant.root.trim();
    if root.is_empty() || root.as_bytes().len() > PLUGIN_FS_MAX_PATH_BYTES {
        return Err(PluginFsError::new("configured fs grant root is invalid"));
    }
    let root_path = Path::new(root);
    if !root_path.is_absolute() {
        return Err(PluginFsError::new(
            "configured fs grant root must be an absolute path",
        ));
    }
    let root_meta = fs::symlink_metadata(root_path)
        .map_err(|_| PluginFsError::new("configured fs grant root is unavailable"))?;
    if root_meta.file_type().is_symlink() {
        return Err(PluginFsError::new(
            "configured fs grant root must not be a symlink",
        ));
    }
    let canonical = fs::canonicalize(root_path)
        .map_err(|_| PluginFsError::new("configured fs grant root is unavailable"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|_| PluginFsError::new("configured fs grant root is unavailable"))?;
    if !metadata.is_dir() {
        return Err(PluginFsError::new(
            "configured fs grant root must be a directory",
        ));
    }
    Ok(canonical)
}

fn resolve_existing_plugin_fs_path(
    root: &Path,
    relative: &Path,
    requested: String,
) -> Result<PathBuf, PluginFsError> {
    let target = root.join(relative);
    reject_symlink_components(root, relative, true, &requested)?;
    let canonical = fs::canonicalize(&target).map_err(|error| {
        PluginFsError::new(format!("FS path is unavailable for {requested}: {error}"))
    })?;
    if !canonical.starts_with(root) {
        return Err(PluginFsError::new(format!(
            "FS path escapes the configured grant root: {requested}"
        )));
    }
    Ok(canonical)
}

fn resolve_writable_plugin_fs_path(
    root: &Path,
    relative: &Path,
    requested: String,
) -> Result<PluginFsWritePath, PluginFsError> {
    let Some(file_name) = relative.file_name() else {
        return Err(PluginFsError::new("FS write path must name a file"));
    };
    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    reject_symlink_components(root, parent_relative, true, &requested)?;
    let parent = fs::canonicalize(root.join(parent_relative)).map_err(|error| {
        PluginFsError::new(format!(
            "FS write parent is unavailable for {requested}: {error}"
        ))
    })?;
    if !parent.starts_with(root) {
        return Err(PluginFsError::new(format!(
            "FS write parent escapes the configured grant root: {requested}"
        )));
    }
    let resolved = parent.join(file_name);
    if let Ok(meta) = fs::symlink_metadata(&resolved) {
        if meta.file_type().is_symlink() {
            return Err(PluginFsError::new(format!(
                "FS write target is a symlink: {requested}"
            )));
        }
        if meta.is_dir() {
            return Err(PluginFsError::new(format!(
                "FS write target is a directory: {requested}"
            )));
        }
        let canonical = fs::canonicalize(&resolved).map_err(|error| {
            PluginFsError::new(format!(
                "FS write target is unavailable for {requested}: {error}"
            ))
        })?;
        if !canonical.starts_with(root) {
            return Err(PluginFsError::new(format!(
                "FS write target escapes the configured grant root: {requested}"
            )));
        }
        return Ok(PluginFsWritePath {
            resolved,
            lock_key: canonical,
        });
    }
    if !resolved.starts_with(root) {
        return Err(PluginFsError::new(format!(
            "FS write target escapes the configured grant root: {requested}"
        )));
    }
    Ok(PluginFsWritePath {
        lock_key: resolved.clone(),
        resolved,
    })
}

fn reject_symlink_components(
    root: &Path,
    relative: &Path,
    include_leaf: bool,
    requested: &str,
) -> Result<(), PluginFsError> {
    let mut current = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        if !include_leaf && index + 1 == components.len() {
            break;
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            PluginFsError::new(format!(
                "FS path component is unavailable for {requested}: {error}"
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PluginFsError::new(format!(
                "FS path symlink escape is denied: {requested}"
            )));
        }
    }
    Ok(())
}

fn plugin_fs_write_lock(path: PathBuf) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(path)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn safe_fs_path(path: &str) -> String {
    let mut sanitized = redact_secret_like(path).replace('\0', " ");
    if sanitized.len() > 160 {
        let mut boundary = 160;
        while boundary > 0 && !sanitized.is_char_boundary(boundary) {
            boundary -= 1;
        }
        sanitized.truncate(boundary);
        sanitized.push('…');
    }
    if sanitized.is_empty() {
        ".".to_string()
    } else {
        sanitized
    }
}

fn validate_plugin_request_request(
    record: &ResolvedPluginRecord,
    request: &PluginRequestRequest,
) -> Result<reqwest::Url, PluginRequestError> {
    let method = request.method.trim().to_ascii_uppercase();
    if method != request.method || !PLUGIN_REQUEST_ALLOWED_METHODS.contains(&method.as_str()) {
        return Err(PluginRequestError::new(format!(
            "request method `{}` is not allowed",
            request.method
        )));
    }
    if request.headers.len() > PLUGIN_REQUEST_MAX_REQUEST_HEADERS {
        return Err(PluginRequestError::new(format!(
            "request descriptor has too many headers (max {})",
            PLUGIN_REQUEST_MAX_REQUEST_HEADERS
        )));
    }
    for header in &request.headers {
        validate_request_header(header)?;
    }
    if let Some(body) = &request.body {
        if body.len() > PLUGIN_REQUEST_MAX_REQUEST_BODY_BYTES {
            return Err(PluginRequestError::new(format!(
                "request body exceeds {} bytes",
                PLUGIN_REQUEST_MAX_REQUEST_BODY_BYTES
            )));
        }
    }
    let url = reqwest::Url::parse(&request.url)
        .map_err(|error| PluginRequestError::new(format!("invalid request URL: {error}")))?;
    match url.scheme() {
        "http" | "https" => {}
        "ws" | "wss" => {
            return Err(PluginRequestError::new(
                "WebSocket URLs are not supported by host_api.request",
            ));
        }
        scheme => {
            return Err(PluginRequestError::new(format!(
                "unsupported request URL scheme {scheme:?}; only http and https are allowed"
            )));
        }
    }
    if url.host_str().is_none() {
        return Err(PluginRequestError::new("request URL must include a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PluginRequestError::new(
            "request URLs with embedded credentials are not allowed",
        ));
    }
    validate_static_request_target(&url)?;
    authorize_request_allowlist(record, &method, &url)?;
    Ok(url)
}

fn validate_request_header(header: &PluginRequestHeader) -> Result<(), PluginRequestError> {
    if header.name.is_empty() || header.name.len() > PLUGIN_REQUEST_MAX_HEADER_NAME_BYTES {
        return Err(PluginRequestError::new("request header name is invalid"));
    }
    if header.value.len() > PLUGIN_REQUEST_MAX_HEADER_VALUE_BYTES {
        return Err(PluginRequestError::new(format!(
            "request header `{}` exceeds {} bytes",
            header.name, PLUGIN_REQUEST_MAX_HEADER_VALUE_BYTES
        )));
    }
    if is_sensitive_header(&header.name) {
        return Err(PluginRequestError::new(format!(
            "request header `{}` is credential-like and must be supplied by an explicit future secret-ref grant, not guest memory",
            header.name
        )));
    }
    if header.name.eq_ignore_ascii_case("upgrade")
        || (header.name.eq_ignore_ascii_case("connection")
            && header
                .value
                .split(',')
                .any(|value| value.trim().eq_ignore_ascii_case("upgrade")))
    {
        return Err(PluginRequestError::new(
            "persistent connection upgrade headers are not supported by host_api.request",
        ));
    }
    if header.name.eq_ignore_ascii_case("accept")
        && header
            .value
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
    {
        return Err(PluginRequestError::new(
            "SSE/event-stream requests are not supported by host_api.request",
        ));
    }
    reqwest::header::HeaderName::from_bytes(header.name.as_bytes()).map_err(|_| {
        PluginRequestError::new(format!("invalid request header name `{}`", header.name))
    })?;
    reqwest::header::HeaderValue::from_str(&header.value).map_err(|_| {
        PluginRequestError::new(format!(
            "invalid request header value for `{}`",
            header.name
        ))
    })?;
    Ok(())
}

fn authorize_request_allowlist(
    record: &ResolvedPluginRecord,
    method: &str,
    url: &reqwest::Url,
) -> Result<(), PluginRequestError> {
    if !request_targets_allow(&record.manifest.request, method, url) {
        return Err(PluginRequestError::new(format!(
            "host_api.request target {} {} is not declared by the plugin manifest",
            method,
            safe_url(url)
        )));
    }
    if !request_targets_allow(&record.grants.request, method, url) {
        return Err(PluginRequestError::new(format!(
            "host_api.request target {} {} is not covered by enabled request grants",
            method,
            safe_url(url)
        )));
    }
    Ok(())
}

fn validate_plugin_websocket_open_request(
    record: &ResolvedPluginRecord,
    bytes: &[u8],
) -> Result<(PluginWebSocketOpenRequest, reqwest::Url), PluginWebSocketError> {
    if bytes.len() > PLUGIN_WEBSOCKET_MAX_OPEN_REQUEST_BYTES {
        return Err(PluginWebSocketError::new(format!(
            "WebSocket open descriptor exceeds {} bytes",
            PLUGIN_WEBSOCKET_MAX_OPEN_REQUEST_BYTES
        )));
    }
    let request: PluginWebSocketOpenRequest = serde_json::from_slice(bytes).map_err(|error| {
        PluginWebSocketError::new(format!("invalid WebSocket open request JSON: {error}"))
    })?;
    if !request.protocols.is_empty() {
        return Err(PluginWebSocketError::new(
            "WebSocket subprotocol negotiation is not supported by host_api.websocket v1",
        ));
    }
    if !request.headers.is_empty() {
        return Err(PluginWebSocketError::new(
            "WebSocket handshake headers from guest memory are not supported; future secret-ref grants must inject credential-bearing headers explicitly",
        ));
    }
    let url = reqwest::Url::parse(&request.url)
        .map_err(|error| PluginWebSocketError::new(format!("invalid WebSocket URL: {error}")))?;
    match url.scheme() {
        "ws" | "wss" => {}
        "http" | "https" => {
            return Err(PluginWebSocketError::new(
                "HTTP URLs are not supported by host_api.websocket",
            ));
        }
        scheme => {
            return Err(PluginWebSocketError::new(format!(
                "unsupported WebSocket URL scheme {scheme:?}; only ws and wss are allowed"
            )));
        }
    }
    if url.host_str().is_none() {
        return Err(PluginWebSocketError::new(
            "WebSocket URL must include a host",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PluginWebSocketError::new(
            "WebSocket URLs with embedded credentials are not allowed",
        ));
    }
    validate_static_request_target(&url).map_err(|error| PluginWebSocketError::new(error.0))?;
    authorize_websocket_allowlist(record, &url)?;
    Ok((request, url))
}

fn authorize_websocket_allowlist(
    record: &ResolvedPluginRecord,
    url: &reqwest::Url,
) -> Result<(), PluginWebSocketError> {
    if !websocket_targets_allow(&record.manifest.websocket, url) {
        return Err(PluginWebSocketError::new(format!(
            "host_api.websocket target {} is not declared by the plugin manifest",
            safe_url(url)
        )));
    }
    if !websocket_targets_allow(&record.grants.websocket, url) {
        return Err(PluginWebSocketError::new(format!(
            "host_api.websocket target {} is not covered by enabled WebSocket grants",
            safe_url(url)
        )));
    }
    Ok(())
}

fn websocket_timeout(timeout_ms: u32) -> Duration {
    if timeout_ms == 0 {
        return PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT;
    }
    let requested = Duration::from_millis(u64::from(timeout_ms));
    requested.min(PLUGIN_WEBSOCKET_MAX_TIMEOUT)
}

fn request_targets_allow(targets: &[PluginRequestGrant], method: &str, url: &reqwest::Url) -> bool {
    targets
        .iter()
        .any(|target| request_target_allows(target, method, url))
}

fn request_target_allows(target: &PluginRequestGrant, method: &str, url: &reqwest::Url) -> bool {
    let scheme = target.scheme.trim().to_ascii_lowercase();
    if scheme.is_empty() || (scheme != "*" && scheme != url.scheme()) {
        return false;
    }
    let Ok(host) = canonical_host(url) else {
        return false;
    };
    let target_host = normalize_host_literal(&target.host);
    if target_host.is_empty() || (target_host != "*" && target_host != host) {
        return false;
    }
    if let Some(port) = target.port {
        if url.port_or_known_default() != Some(port) {
            return false;
        }
    }
    if target.methods.is_empty()
        || !target
            .methods
            .iter()
            .any(|allowed_method| allowed_method.trim().eq_ignore_ascii_case(method))
    {
        return false;
    }
    target.path_prefixes.is_empty()
        || target
            .path_prefixes
            .iter()
            .any(|prefix| !prefix.is_empty() && url.path().starts_with(prefix))
}

fn request_targets_intersect(left: &PluginRequestGrant, right: &PluginRequestGrant) -> bool {
    request_scheme_intersects(&left.scheme, &right.scheme)
        && request_host_intersects(&left.host, &right.host)
        && request_port_intersects(left.port, right.port)
        && request_methods_intersect(&left.methods, &right.methods)
        && request_paths_intersect(&left.path_prefixes, &right.path_prefixes)
}

fn request_target_covers(covering: &PluginRequestGrant, covered: &PluginRequestGrant) -> bool {
    request_scheme_covers(&covering.scheme, &covered.scheme)
        && request_host_covers(&covering.host, &covered.host)
        && request_port_covers(covering.port, covered.port)
        && request_methods_cover(&covering.methods, &covered.methods)
        && request_paths_cover(&covering.path_prefixes, &covered.path_prefixes)
}

fn websocket_targets_allow(targets: &[PluginWebSocketGrant], url: &reqwest::Url) -> bool {
    targets
        .iter()
        .any(|target| websocket_target_allows(target, url))
}

fn websocket_target_allows(target: &PluginWebSocketGrant, url: &reqwest::Url) -> bool {
    let scheme = target.scheme.trim().to_ascii_lowercase();
    if scheme.is_empty() || (scheme != "*" && scheme != url.scheme()) {
        return false;
    }
    let Ok(host) = canonical_host(url) else {
        return false;
    };
    let target_host = normalize_host_literal(&target.host);
    if target_host.is_empty() || (target_host != "*" && target_host != host) {
        return false;
    }
    if let Some(port) = target.port {
        if url.port_or_known_default() != Some(port) {
            return false;
        }
    }
    target.path_prefixes.is_empty()
        || target
            .path_prefixes
            .iter()
            .any(|prefix| !prefix.is_empty() && url.path().starts_with(prefix))
}

fn websocket_targets_intersect(left: &PluginWebSocketGrant, right: &PluginWebSocketGrant) -> bool {
    request_scheme_intersects(&left.scheme, &right.scheme)
        && request_host_intersects(&left.host, &right.host)
        && request_port_intersects(left.port, right.port)
        && request_paths_intersect(&left.path_prefixes, &right.path_prefixes)
}

fn websocket_target_covers(
    covering: &PluginWebSocketGrant,
    covered: &PluginWebSocketGrant,
) -> bool {
    request_scheme_covers(&covering.scheme, &covered.scheme)
        && request_host_covers(&covering.host, &covered.host)
        && request_port_covers(covering.port, covered.port)
        && request_paths_cover(&covering.path_prefixes, &covered.path_prefixes)
}

fn request_scheme_intersects(left: &str, right: &str) -> bool {
    let left = left.trim().to_ascii_lowercase();
    let right = right.trim().to_ascii_lowercase();
    !left.is_empty() && !right.is_empty() && (left == "*" || right == "*" || left == right)
}

fn request_scheme_covers(covering: &str, covered: &str) -> bool {
    let covering = covering.trim().to_ascii_lowercase();
    let covered = covered.trim().to_ascii_lowercase();
    !covering.is_empty() && !covered.is_empty() && (covering == "*" || covering == covered)
}

fn request_host_intersects(left: &str, right: &str) -> bool {
    let left = normalize_host_literal(left);
    let right = normalize_host_literal(right);
    !left.is_empty() && !right.is_empty() && (left == "*" || right == "*" || left == right)
}

fn request_host_covers(covering: &str, covered: &str) -> bool {
    let covering = normalize_host_literal(covering);
    let covered = normalize_host_literal(covered);
    !covering.is_empty() && !covered.is_empty() && (covering == "*" || covering == covered)
}

fn request_port_intersects(left: Option<u16>, right: Option<u16>) -> bool {
    left.is_none() || right.is_none() || left == right
}

fn request_port_covers(covering: Option<u16>, covered: Option<u16>) -> bool {
    covering.is_none() || covering == covered
}

fn request_methods_intersect(left: &[String], right: &[String]) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && left.iter().any(|left_method| {
            right
                .iter()
                .any(|right_method| left_method.trim().eq_ignore_ascii_case(right_method.trim()))
        })
}

fn request_methods_cover(covering: &[String], covered: &[String]) -> bool {
    !covering.is_empty()
        && !covered.is_empty()
        && covered.iter().all(|covered_method| {
            covering.iter().any(|covering_method| {
                covering_method
                    .trim()
                    .eq_ignore_ascii_case(covered_method.trim())
            })
        })
}

fn request_paths_intersect(left: &[String], right: &[String]) -> bool {
    left.is_empty()
        || right.is_empty()
        || left.iter().any(|left_prefix| {
            !left_prefix.is_empty()
                && right.iter().any(|right_prefix| {
                    !right_prefix.is_empty()
                        && (left_prefix.starts_with(right_prefix)
                            || right_prefix.starts_with(left_prefix))
                })
        })
}

fn request_paths_cover(covering: &[String], covered: &[String]) -> bool {
    if covering.is_empty() {
        return true;
    }
    if covered.is_empty() {
        return false;
    }
    covered.iter().all(|covered_prefix| {
        !covered_prefix.is_empty()
            && covering.iter().any(|covering_prefix| {
                !covering_prefix.is_empty() && covered_prefix.starts_with(covering_prefix)
            })
    })
}

fn normalize_host_literal(host: &str) -> String {
    host.trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn has_usable_request_grant(record: &ResolvedPluginRecord) -> bool {
    record.grants.request.iter().any(|grant| {
        !grant.scheme.trim().is_empty()
            && !grant.host.trim().is_empty()
            && grant.methods.iter().any(|method| {
                let method = method.trim().to_ascii_uppercase();
                PLUGIN_REQUEST_ALLOWED_METHODS.contains(&method.as_str())
            })
    })
}

fn has_declared_request_target(record: &ResolvedPluginRecord) -> bool {
    record.manifest.request.iter().any(|target| {
        !target.scheme.trim().is_empty()
            && !target.host.trim().is_empty()
            && target.methods.iter().any(|method| {
                let method = method.trim().to_ascii_uppercase();
                PLUGIN_REQUEST_ALLOWED_METHODS.contains(&method.as_str())
            })
    })
}

fn has_usable_websocket_grant(record: &ResolvedPluginRecord) -> bool {
    record.grants.websocket.iter().any(|grant| {
        let scheme = grant.scheme.trim().to_ascii_lowercase();
        (scheme == "ws" || scheme == "wss" || scheme == "*") && !grant.host.trim().is_empty()
    })
}

fn has_declared_websocket_target(record: &ResolvedPluginRecord) -> bool {
    record.manifest.websocket.iter().any(|target| {
        let scheme = target.scheme.trim().to_ascii_lowercase();
        (scheme == "ws" || scheme == "wss" || scheme == "*") && !target.host.trim().is_empty()
    })
}

fn has_usable_fs_grant(record: &ResolvedPluginRecord) -> bool {
    record.grants.fs.iter().any(|grant| {
        !grant.root.trim().is_empty()
            && Path::new(&grant.root).is_absolute()
            && !grant.operations.is_empty()
    })
}

fn canonical_host(url: &reqwest::Url) -> Result<String, PluginRequestError> {
    url.host_str()
        .map(normalize_host_literal)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| PluginRequestError::new("request URL must include a host"))
}

fn validate_static_request_target(url: &reqwest::Url) -> Result<(), PluginRequestError> {
    let _host = canonical_host(url)?;
    if url.cannot_be_a_base() {
        return Err(PluginRequestError::new(
            "request URL target is not hierarchical",
        ));
    }
    Ok(())
}

fn resolve_request_target_for_client(
    url: &reqwest::Url,
    resolver: &dyn PluginRequestResolver,
) -> Result<Option<PinnedRequestResolution>, PluginRequestError> {
    let host = canonical_host(url)?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(None);
    }
    let port = url.port_or_known_default().ok_or_else(|| {
        PluginRequestError::new("request URL uses a scheme without a default port")
    })?;
    let addrs = resolver.resolve(&host, port)?;
    if addrs.is_empty() {
        return Err(PluginRequestError::new(format!(
            "DNS lookup for {:?} returned no addresses",
            host
        )));
    }
    let mut domains = Vec::new();
    if let Some(raw_host) = url.host_str() {
        let raw_host = raw_host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        if !raw_host.is_empty() {
            domains.push(raw_host);
        }
    }
    if !domains.contains(&host) {
        domains.push(host);
    }
    Ok(Some(PinnedRequestResolution { domains, addrs }))
}

fn collect_request_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> Vec<PluginRequestHeader> {
    headers
        .iter()
        .filter(|(name, _)| !is_sensitive_header(name.as_str()))
        .take(PLUGIN_REQUEST_MAX_RESPONSE_HEADERS)
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|value| PluginRequestHeader {
                name: name.as_str().to_string(),
                value: bounded_header_value(value),
            })
        })
        .collect()
}

fn bounded_header_value(value: &str) -> String {
    let mut redacted = redact_secret_like(value);
    if redacted.len() > PLUGIN_REQUEST_MAX_HEADER_VALUE_BYTES {
        truncate_string_to_boundary(&mut redacted, PLUGIN_REQUEST_MAX_HEADER_VALUE_BYTES);
        redacted.push('…');
    }
    redacted
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-auth-token"
            | "api-key"
            | "apikey"
    )
}

fn safe_url(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or("<missing-host>");
    let mut path = url.path().to_string();
    if path.len() > 120 {
        truncate_string_to_boundary(&mut path, 120);
        path.push('…');
    }
    match url.port() {
        Some(port) => format!("{}://{host}:{port}{path}", url.scheme()),
        None => format!("{}://{host}{path}", url.scheme()),
    }
}

fn redact_secret_like(message: &str) -> String {
    let mut value = message.to_string();
    for needle in [
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
        "x-api-key",
        "x-auth-token",
        "api-key",
        "token",
        "secret",
        "password",
    ] {
        value = redact_after_secret_word(&value, needle);
    }
    value
}

fn redact_after_secret_word(input: &str, needle: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::new();
    let mut cursor = 0usize;
    while let Some(relative) = lower[cursor..].find(needle) {
        let start = cursor + relative;
        let mut end = start + needle.len();
        out.push_str(&input[cursor..end]);
        let bytes = input.as_bytes();
        while end < input.len() && matches!(bytes[end], b' ' | b'=' | b':' | b'\t') {
            out.push(bytes[end] as char);
            end += 1;
        }
        let secret_start = end;
        while end < input.len() && !matches!(bytes[end], b' ' | b',' | b';' | b'\n' | b'\r') {
            end += 1;
        }
        if end > secret_start {
            out.push_str(PLUGIN_REQUEST_REDACTION);
        }
        cursor = end;
    }
    out.push_str(&input[cursor..]);
    out
}

#[derive(Debug)]
struct PluginPermissionError(String);

impl PluginPermissionError {
    fn bounded_message(&self) -> String {
        bounded_message(&self.0)
    }
}

fn authorize_plugin_tool(
    record: &ResolvedPluginRecord,
    tool: &PluginToolManifest,
) -> Result<(), PluginPermissionError> {
    validate_grant_binding(record)?;
    require_permission(
        &record.manifest.permissions,
        &PluginPermission::surface(PluginSurface::Tool),
        "requested surfaces.tool permission is missing",
    )?;
    require_permission(
        &record.grants.permissions,
        &PluginPermission::surface(PluginSurface::Tool),
        "granted surfaces.tool permission is missing",
    )?;
    if !permission_allows_tool(&record.manifest.permissions, &tool.name) {
        return Err(PluginPermissionError(format!(
            "requested tool permission for `{}` is missing",
            tool.name
        )));
    }
    if !permission_allows_tool(&record.grants.permissions, &tool.name) {
        return Err(PluginPermissionError(format!(
            "granted tool permission for `{}` is missing",
            tool.name
        )));
    }
    if tool.external_write {
        require_permission(
            &record.manifest.permissions,
            &PluginPermission::ExternalWrite,
            "requested external_write permission is missing",
        )?;
        require_permission(
            &record.grants.permissions,
            &PluginPermission::ExternalWrite,
            "granted external_write permission is missing",
        )?;
    }
    Ok(())
}

fn authorize_plugin_service(
    record: &ResolvedPluginRecord,
    service_name: &str,
) -> Result<(), PluginPermissionError> {
    validate_grant_binding(record)?;
    require_permission(
        &record.manifest.permissions,
        &PluginPermission::surface(PluginSurface::Service),
        "requested surfaces.service permission is missing",
    )?;
    require_permission(
        &record.grants.permissions,
        &PluginPermission::surface(PluginSurface::Service),
        "granted surfaces.service permission is missing",
    )?;
    let permission = PluginPermission::service(service_name);
    require_permission(
        &record.manifest.permissions,
        &permission,
        &format!("requested service permission for `{service_name}` is missing"),
    )?;
    require_permission(
        &record.grants.permissions,
        &permission,
        &format!("granted service permission for `{service_name}` is missing"),
    )?;
    Ok(())
}

fn authorize_plugin_ingress(
    record: &ResolvedPluginRecord,
    ingress_name: &str,
) -> Result<(), PluginPermissionError> {
    validate_grant_binding(record)?;
    require_permission(
        &record.manifest.permissions,
        &PluginPermission::surface(PluginSurface::Ingress),
        "requested surfaces.ingress permission is missing",
    )?;
    require_permission(
        &record.grants.permissions,
        &PluginPermission::surface(PluginSurface::Ingress),
        "granted surfaces.ingress permission is missing",
    )?;
    let permission = PluginPermission::ingress(ingress_name);
    require_permission(
        &record.manifest.permissions,
        &permission,
        &format!("requested ingress permission for `{ingress_name}` is missing"),
    )?;
    require_permission(
        &record.grants.permissions,
        &permission,
        &format!("granted ingress permission for `{ingress_name}` is missing"),
    )?;
    Ok(())
}

fn authorize_plugin_host_api(
    record: &ResolvedPluginRecord,
    api: PluginHostApi,
) -> Result<(), PluginPermissionError> {
    validate_grant_binding(record)?;
    let permission = PluginPermission::host_api(api);
    require_permission(
        &record.manifest.permissions,
        &permission,
        &format!("requested host_api.{api} permission is missing"),
    )?;
    require_permission(
        &record.grants.permissions,
        &permission,
        &format!("granted host_api.{api} permission is missing"),
    )?;
    match api {
        PluginHostApi::Request => {
            if !has_declared_request_target(record) {
                return Err(PluginPermissionError(
                    "manifest host_api.request target declaration is missing".to_string(),
                ));
            }
            if !has_usable_request_grant(record) {
                return Err(PluginPermissionError(
                    "enabled host_api.request grants are missing".to_string(),
                ));
            }
            Ok(())
        }
        PluginHostApi::WebSocket => {
            if !has_declared_websocket_target(record) {
                return Err(PluginPermissionError(
                    "manifest host_api.websocket target declaration is missing".to_string(),
                ));
            }
            if !has_usable_websocket_grant(record) {
                return Err(PluginPermissionError(
                    "enabled host_api.websocket grants are missing".to_string(),
                ));
            }
            Ok(())
        }
        PluginHostApi::Fs => {
            if !has_usable_fs_grant(record) {
                return Err(PluginPermissionError(
                    "granted host_api.fs scope allowlist is missing".to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn validate_grant_binding(record: &ResolvedPluginRecord) -> Result<(), PluginPermissionError> {
    if let Some(message) =
        record
            .grants
            .binding_error(&record.identity, &record.digest, &record.manifest.version)
    {
        return Err(PluginPermissionError(message.to_string()));
    }
    Ok(())
}

fn require_permission(
    permissions: &[PluginPermission],
    expected: &PluginPermission,
    missing_message: &str,
) -> Result<(), PluginPermissionError> {
    if permissions.iter().any(|permission| permission == expected) {
        return Ok(());
    }
    Err(PluginPermissionError(missing_message.to_string()))
}

fn permission_allows_tool(permissions: &[PluginPermission], tool_name: &str) -> bool {
    permissions.iter().any(|permission| match permission {
        PluginPermission::Tool { name } => name == tool_name,
        PluginPermission::ToolNamespace { namespace } => {
            !namespace.is_empty() && tool_name.starts_with(namespace)
        }
        _ => false,
    })
}

const PLUGIN_WASM_MAX_INPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_SUMMARY_BYTES: usize = 1024;
const PLUGIN_WASM_FUEL: u64 = 5_000_000;
const PLUGIN_WASM_TIMEOUT: Duration = Duration::from_secs(1);
const PLUGIN_SERVICE_INGRESS_QUEUE_CAPACITY: usize = 32;
const PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_COUNT: usize = 16;
const PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_PAYLOAD_BYTES: usize = 16 * 1024;
const PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_RESULTS: usize = 32;
#[cfg(test)]
const PLUGIN_SERVICE_INGRESS_DISPATCH_TIMEOUT: Duration = Duration::from_millis(25);
#[cfg(not(test))]
const PLUGIN_SERVICE_INGRESS_DISPATCH_TIMEOUT: Duration = Duration::from_secs(1);
const PLUGIN_WASM_MEMORY_BYTES: usize = 2 * 1024 * 1024;
const PLUGIN_WASM_TABLE_ELEMENTS: usize = 256;
const PLUGIN_REQUEST_MAX_REQUEST_BYTES: usize = 48 * 1024;
const PLUGIN_REQUEST_MAX_REQUEST_BODY_BYTES: usize = 32 * 1024;
const PLUGIN_REQUEST_MAX_REQUEST_HEADERS: usize = 16;
const PLUGIN_REQUEST_MAX_RESPONSE_HEADERS: usize = 16;
const PLUGIN_REQUEST_MAX_HEADER_NAME_BYTES: usize = 64;
const PLUGIN_REQUEST_MAX_HEADER_VALUE_BYTES: usize = 1024;
const PLUGIN_REQUEST_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const PLUGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const PLUGIN_REQUEST_ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];
const PLUGIN_REQUEST_REDACTION: &str = "<redacted>";
const PLUGIN_WEBSOCKET_MAX_OPEN_REQUEST_BYTES: usize = 8 * 1024;
const PLUGIN_WEBSOCKET_MAX_TEXT_BYTES: usize = 32 * 1024;
const PLUGIN_WEBSOCKET_MAX_FRAME_BYTES: usize = 32 * 1024;
const PLUGIN_WEBSOCKET_MAX_MESSAGE_BYTES: usize = 64 * 1024;
const PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS: usize = 4;
const PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const PLUGIN_WEBSOCKET_MAX_TIMEOUT: Duration = Duration::from_secs(30);
const PLUGIN_WEBSOCKET_MAX_HANDLE_AGE: Duration = Duration::from_secs(15 * 60);
const PLUGIN_WEBSOCKET_MAX_CONTROL_FRAMES: usize = 16;
const PLUGIN_FS_MAX_REQUEST_BYTES: usize = 64 * 1024;
const PLUGIN_FS_MAX_PATH_BYTES: usize = 4096;
const PLUGIN_FS_MAX_READ_BYTES: usize = 64 * 1024;
const PLUGIN_FS_MAX_WRITE_BYTES: usize = 64 * 1024;
const PLUGIN_FS_MAX_LIST_ENTRIES: usize = 256;

fn wasm_component_store_limits() -> wasmtime::StoreLimits {
    wasmtime::StoreLimitsBuilder::new()
        .memory_size(PLUGIN_WASM_MEMORY_BYTES)
        .table_elements(PLUGIN_WASM_TABLE_ELEMENTS)
        .instances(1)
        .tables(1)
        .memories(1)
        .trap_on_grow_failure(true)
        .build()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginFsRuntimeOperation {
    Read,
    List,
    Write,
}

impl PluginFsRuntimeOperation {
    fn grant_operation(self) -> PluginFsOperation {
        match self {
            Self::Read => PluginFsOperation::Read,
            Self::List => PluginFsOperation::List,
            Self::Write => PluginFsOperation::Write,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::List => "list",
            Self::Write => "write",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginFsPathRequest {
    path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginFsWriteRequest {
    path: String,
    content: String,
}

#[derive(Clone, Debug, Serialize)]
struct PluginFsReadResponse {
    path: String,
    content: String,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PluginFsListResponse {
    path: String,
    entries: Vec<PluginFsDirEntry>,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PluginFsDirEntry {
    name: String,
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
struct PluginFsWriteResponse {
    path: String,
    bytes_written: usize,
}

#[derive(Debug)]
struct PluginFsError {
    message: String,
}

impl PluginFsError {
    fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            message: bounded_message(redact_secret_like(&message)),
        }
    }
}

impl std::fmt::Display for PluginFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PluginFsError {}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginRequestRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<PluginRequestHeader>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginRequestHeader {
    name: String,
    value: String,
}

#[derive(Clone, Debug, Serialize)]
struct PluginRequestResponse {
    status: u16,
    headers: Vec<PluginRequestHeader>,
    body: String,
    truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PluginWebSocketOpenRequest {
    url: String,
    #[serde(default)]
    protocols: Vec<String>,
    #[serde(default)]
    headers: Vec<PluginRequestHeader>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginServiceWebSocketSendCommandPayload {
    url: String,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginServiceDiagnosticStatusCommandPayload {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    status: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PluginWebSocketOpenResponse {
    handle: u32,
    url: String,
}

#[derive(Clone, Debug, Serialize)]
struct PluginWebSocketSendResponse {
    sent: bool,
    bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PluginWebSocketRecvResponse {
    Text { text: String },
    Closed,
}

#[derive(Clone, Debug, Serialize)]
struct PluginWebSocketCloseResponse {
    closed: bool,
}

#[derive(Clone, Copy, Debug)]
struct PluginWebSocketLimits {
    timeout: Duration,
    max_message_bytes: usize,
}

impl Default for PluginWebSocketLimits {
    fn default() -> Self {
        Self {
            timeout: PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT,
            max_message_bytes: PLUGIN_WEBSOCKET_MAX_MESSAGE_BYTES,
        }
    }
}

#[derive(Debug)]
struct PluginWebSocketError(String);

impl PluginWebSocketError {
    fn new(message: impl Into<String>) -> Self {
        Self(redact_secret_like(&bounded_message(message.into())))
    }
}

impl std::fmt::Display for PluginWebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PluginWebSocketError {}

trait PluginWebSocketConnection: Send {
    fn send_text(&mut self, text: &str) -> Result<(), PluginWebSocketError>;
    fn recv_text(
        &mut self,
        timeout: Duration,
        max_message_bytes: usize,
    ) -> Result<PluginWebSocketRecvResponse, PluginWebSocketError>;
    fn close(&mut self) -> Result<(), PluginWebSocketError>;
}

trait PluginWebSocketClient: Send + Sync {
    fn supports_bounded_open(&self) -> bool;

    fn open(
        &self,
        request: &PluginWebSocketOpenRequest,
        url: &reqwest::Url,
        limits: PluginWebSocketLimits,
    ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError>;
}

struct TungstenitePluginWebSocketClient;

type AsyncSystemWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct PluginWebSocketRuntime {
    runtime: Option<TokioRuntime>,
}

impl PluginWebSocketRuntime {
    fn new(runtime: TokioRuntime) -> Self {
        Self {
            runtime: Some(runtime),
        }
    }

    fn get(&self) -> &TokioRuntime {
        self.runtime
            .as_ref()
            .expect("plugin websocket runtime missing")
    }
}

impl Drop for PluginWebSocketRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        if TokioHandle::try_current().is_ok() {
            let _ = tokio::task::spawn_blocking(move || drop(runtime));
        } else {
            drop(runtime);
        }
    }
}

struct TungstenitePluginWebSocketConnection {
    runtime: PluginWebSocketRuntime,
    socket: AsyncSystemWebSocket,
}

impl PluginWebSocketClient for TungstenitePluginWebSocketClient {
    fn supports_bounded_open(&self) -> bool {
        true
    }

    fn open(
        &self,
        _request: &PluginWebSocketOpenRequest,
        url: &reqwest::Url,
        limits: PluginWebSocketLimits,
    ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError> {
        let mut request = url.as_str().into_client_request().map_err(|error| {
            PluginWebSocketError::new(format!("WebSocket request build failed: {error}"))
        })?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::USER_AGENT,
            tokio_tungstenite::tungstenite::http::HeaderValue::from_static("yoi-plugin-host/1"),
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(limits.max_message_bytes))
            .max_frame_size(Some(PLUGIN_WEBSOCKET_MAX_FRAME_BYTES))
            .accept_unmasked_frames(false);
        let runtime = new_websocket_runtime()?;
        let open = async {
            tokio::time::timeout(
                limits.timeout,
                tokio_tungstenite::connect_async_tls_with_config(
                    request,
                    Some(config),
                    false,
                    None,
                ),
            )
            .await
        };
        let (socket, _response) = block_on_websocket_future(runtime.get(), open)
            .map_err(|error| {
                PluginWebSocketError::new(format!(
                    "WebSocket open timed out after {} ms for {}: {error}",
                    limits.timeout.as_millis(),
                    safe_url(url)
                ))
            })?
            .map_err(|error| {
                PluginWebSocketError::new(format!(
                    "WebSocket connection failed for {}: {error}",
                    safe_url(url)
                ))
            })?;
        Ok(Box::new(TungstenitePluginWebSocketConnection {
            runtime,
            socket,
        }))
    }
}

impl PluginWebSocketConnection for TungstenitePluginWebSocketConnection {
    fn send_text(&mut self, text: &str) -> Result<(), PluginWebSocketError> {
        let send = tokio::time::timeout(
            PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT,
            self.socket.send(Message::Text(text.to_string().into())),
        );
        block_on_websocket_future(self.runtime.get(), send)
            .map_err(|_| PluginWebSocketError::new("WebSocket send timed out"))?
            .map_err(|error| PluginWebSocketError::new(format!("WebSocket send failed: {error}")))
    }

    fn recv_text(
        &mut self,
        timeout: Duration,
        max_message_bytes: usize,
    ) -> Result<PluginWebSocketRecvResponse, PluginWebSocketError> {
        for _ in 0..PLUGIN_WEBSOCKET_MAX_CONTROL_FRAMES {
            let next = tokio::time::timeout(timeout, self.socket.next());
            let message = block_on_websocket_future(self.runtime.get(), next)
                .map_err(|_| PluginWebSocketError::new("WebSocket receive timed out"))?
                .ok_or_else(|| PluginWebSocketError::new("WebSocket stream ended"))?
                .map_err(|error| {
                    PluginWebSocketError::new(format!("WebSocket receive failed: {error}"))
                })?;
            match message {
                Message::Text(text) => {
                    if text.len() > max_message_bytes {
                        return Err(PluginWebSocketError::new(format!(
                            "WebSocket text message exceeds {} bytes",
                            max_message_bytes
                        )));
                    }
                    return Ok(PluginWebSocketRecvResponse::Text {
                        text: text.to_string(),
                    });
                }
                Message::Binary(_) => {
                    return Err(PluginWebSocketError::new(
                        "binary WebSocket messages are not supported by host_api.websocket v1",
                    ));
                }
                Message::Close(_) => return Ok(PluginWebSocketRecvResponse::Closed),
                Message::Ping(payload) => {
                    let send = tokio::time::timeout(
                        PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT,
                        self.socket.send(Message::Pong(payload)),
                    );
                    block_on_websocket_future(self.runtime.get(), send)
                        .map_err(|_| PluginWebSocketError::new("WebSocket pong timed out"))?
                        .map_err(|error| {
                            PluginWebSocketError::new(format!("WebSocket pong failed: {error}"))
                        })?;
                }
                Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
        Err(PluginWebSocketError::new(
            "WebSocket receive exceeded bounded control-frame budget",
        ))
    }

    fn close(&mut self) -> Result<(), PluginWebSocketError> {
        let close = tokio::time::timeout(PLUGIN_WEBSOCKET_DEFAULT_TIMEOUT, self.socket.close(None));
        block_on_websocket_future(self.runtime.get(), close)
            .map_err(|_| PluginWebSocketError::new("WebSocket close timed out"))?
            .map_err(|error| PluginWebSocketError::new(format!("WebSocket close failed: {error}")))
    }
}

fn new_websocket_runtime() -> Result<PluginWebSocketRuntime, PluginWebSocketError> {
    let runtime = TokioRuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            PluginWebSocketError::new(format!("WebSocket runtime build failed: {error}"))
        })?;
    Ok(PluginWebSocketRuntime::new(runtime))
}

fn block_on_websocket_future<F: std::future::Future>(
    runtime: &TokioRuntime,
    future: F,
) -> F::Output {
    if TokioHandle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(future))
    } else {
        runtime.block_on(future)
    }
}

#[derive(Clone, Default)]
struct PluginWebSocketHandles {
    inner: Arc<Mutex<PluginWebSocketHandleTable>>,
}

impl PluginWebSocketHandles {
    fn reserve_open(&self) -> Result<PluginWebSocketOpenReservation, PluginWebSocketError> {
        self.inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .reserve_open()?;
        Ok(PluginWebSocketOpenReservation {
            handles: self.clone(),
            active: true,
        })
    }

    fn with_connection<T>(
        &self,
        handle: u32,
        f: impl FnOnce(&mut dyn PluginWebSocketConnection) -> Result<T, PluginWebSocketError>,
    ) -> Result<T, PluginWebSocketError> {
        self.inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .with_connection(handle, f)
    }

    fn close(&self, handle: u32) -> Result<bool, PluginWebSocketError> {
        self.inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .close(handle)
    }

    fn close_all(&self) {
        self.inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .close_all();
    }

    #[cfg(test)]
    fn reservation_count(&self) -> usize {
        self.inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .reservations
    }
}

struct PluginWebSocketOpenReservation {
    handles: PluginWebSocketHandles,
    active: bool,
}

impl PluginWebSocketOpenReservation {
    fn commit(
        mut self,
        connection: Box<dyn PluginWebSocketConnection>,
    ) -> Result<u32, PluginWebSocketError> {
        let result = self
            .handles
            .inner
            .lock()
            .expect("plugin websocket handle table poisoned")
            .insert_reserved(connection);
        self.active = false;
        result
    }
}

impl Drop for PluginWebSocketOpenReservation {
    fn drop(&mut self) {
        if self.active {
            self.handles
                .inner
                .lock()
                .expect("plugin websocket handle table poisoned")
                .release_reservation();
            self.active = false;
        }
    }
}

#[derive(Default)]
struct PluginWebSocketHandleTable {
    next: u32,
    reservations: usize,
    connections: HashMap<u32, PluginWebSocketHandleEntry>,
}

struct PluginWebSocketHandleEntry {
    opened_at: Instant,
    connection: Box<dyn PluginWebSocketConnection>,
}

impl PluginWebSocketHandleTable {
    fn reserve_open(&mut self) -> Result<(), PluginWebSocketError> {
        self.expire_stale();
        if self.connections.len() + self.reservations >= PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS {
            return Err(PluginWebSocketError::new(format!(
                "host_api.websocket open connection limit ({}) exceeded before dialing",
                PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS
            )));
        }
        self.reservations += 1;
        Ok(())
    }

    fn release_reservation(&mut self) {
        self.reservations = self.reservations.saturating_sub(1);
    }

    fn insert_reserved(
        &mut self,
        mut connection: Box<dyn PluginWebSocketConnection>,
    ) -> Result<u32, PluginWebSocketError> {
        self.release_reservation();
        self.expire_stale();
        if self.connections.len() >= PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS {
            let _ = connection.close();
            return Err(PluginWebSocketError::new(format!(
                "host_api.websocket open connection limit ({}) exceeded while committing reserved handle",
                PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS
            )));
        }
        let mut attempts = 0usize;
        loop {
            self.next = self.next.wrapping_add(1).max(1);
            attempts += 1;
            if !self.connections.contains_key(&self.next) {
                let handle = self.next;
                self.connections.insert(
                    handle,
                    PluginWebSocketHandleEntry {
                        opened_at: Instant::now(),
                        connection,
                    },
                );
                return Ok(handle);
            }
            if attempts > u32::MAX as usize {
                return Err(PluginWebSocketError::new(
                    "WebSocket handle space exhausted",
                ));
            }
        }
    }

    fn with_connection<T>(
        &mut self,
        handle: u32,
        f: impl FnOnce(&mut dyn PluginWebSocketConnection) -> Result<T, PluginWebSocketError>,
    ) -> Result<T, PluginWebSocketError> {
        self.expire_stale();
        let entry = self.connections.get_mut(&handle).ok_or_else(|| {
            PluginWebSocketError::new(format!("unknown WebSocket handle {handle}"))
        })?;
        f(entry.connection.as_mut())
    }

    fn close(&mut self, handle: u32) -> Result<bool, PluginWebSocketError> {
        if let Some(mut entry) = self.connections.remove(&handle) {
            entry.connection.close()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn close_all(&mut self) {
        self.reservations = 0;
        for (_, mut entry) in self.connections.drain() {
            let _ = entry.connection.close();
        }
    }

    fn expire_stale(&mut self) {
        let now = Instant::now();
        let stale: Vec<_> = self
            .connections
            .iter()
            .filter_map(|(handle, entry)| {
                (now.duration_since(entry.opened_at) > PLUGIN_WEBSOCKET_MAX_HANDLE_AGE)
                    .then_some(*handle)
            })
            .collect();
        for handle in stale {
            let _ = self.close(handle);
        }
    }
}

impl Drop for PluginWebSocketHandleTable {
    fn drop(&mut self) {
        self.close_all();
    }
}

#[derive(Clone, Copy, Debug)]
struct PluginRequestLimits {
    timeout: Duration,
    max_response_bytes: usize,
}

impl Default for PluginRequestLimits {
    fn default() -> Self {
        Self {
            timeout: PLUGIN_REQUEST_TIMEOUT,
            max_response_bytes: PLUGIN_REQUEST_MAX_RESPONSE_BYTES,
        }
    }
}

trait PluginRequestClient: Send + Sync {
    fn execute(
        &self,
        request: &PluginRequestRequest,
        url: &reqwest::Url,
        limits: PluginRequestLimits,
    ) -> Result<PluginRequestResponse, PluginRequestError>;
}

struct ReqwestPluginRequestClient;
struct SystemPluginRequestResolver;

#[derive(Clone, Debug)]
struct PinnedRequestResolution {
    domains: Vec<String>,
    addrs: Vec<SocketAddr>,
}

trait PluginRequestResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, PluginRequestError>;
}

impl PluginRequestResolver for SystemPluginRequestResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, PluginRequestError> {
        let mut addrs = Vec::new();
        for addr in (host, port).to_socket_addrs().map_err(|error| {
            PluginRequestError::new(format!("DNS lookup failed for {:?}: {error}", host))
        })? {
            addrs.push(addr);
        }
        Ok(addrs)
    }
}

#[derive(Debug)]
struct PluginRequestError(String);

impl PluginRequestError {
    fn new(message: impl Into<String>) -> Self {
        Self(redact_secret_like(&bounded_message(message.into())))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PluginInstanceLifecycleState {
    Ready,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PluginInstanceDiagnosticKind {
    Lifecycle,
    InvalidEvent,
    QueueFull,
    DispatchTimeout,
    DispatchFailed,
    ServiceUnavailable,
    ServiceFailed,
    ServiceStopped,
    ServiceOutputCommandRecorded,
    ServiceOutputCommandRejected,
    ServiceOutputCommandUnsupported,
    ServiceWebSocketConnected,
    ServiceWebSocketClosed,
    ServiceWebSocketError,
    ServiceWebSocketSendFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginInstanceDiagnostic {
    pub kind: PluginInstanceDiagnosticKind,
    pub state: PluginInstanceLifecycleState,
    pub message: String,
}

impl PluginInstanceDiagnostic {
    pub fn new(state: PluginInstanceLifecycleState, message: impl Into<String>) -> Self {
        Self::with_kind(PluginInstanceDiagnosticKind::Lifecycle, state, message)
    }

    pub fn with_kind(
        kind: PluginInstanceDiagnosticKind,
        state: PluginInstanceLifecycleState,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            state,
            message: bounded_message(redact_secret_like(&message.into())),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PluginIngressDispatchCounters {
    pub enqueued: u64,
    pub dispatched: u64,
    pub rejected: u64,
    pub failed: u64,
    pub timed_out: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PluginServiceWebSocketConnectionState {
    Connecting,
    Connected,
    Closed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginServiceWebSocketConnectionStatus {
    pub url: String,
    pub ingress_name: String,
    pub state: PluginServiceWebSocketConnectionState,
    pub last_frame_at: Option<String>,
    pub last_error: Option<String>,
    pub received_text_frames: u64,
    pub sent_text_frames: u64,
    pub queue_drops: u64,
    pub send_failures: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginInstanceStatus {
    pub plugin_ref: String,
    pub lifecycle: PluginInstanceLifecycleState,
    pub component_status: Option<Value>,
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub last_error: Option<String>,
    pub dispatch_counters: PluginIngressDispatchCounters,
    /// Last bounded Service output command outcomes. These are produced only by
    /// Service/Ingress dispatch and are intentionally separate from ToolOutput.
    pub output_command_results: Vec<PluginServiceOutputCommandResult>,
    pub websocket_connections: Vec<PluginServiceWebSocketConnectionStatus>,
    pub diagnostics: Vec<PluginInstanceDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginIngressEvent {
    pub kind: String,
    pub source: String,
    pub ingress_name: String,
    pub payload: Value,
    pub created_at: String,
    pub attempt: u32,
    pub correlation_id: String,
}

impl PluginIngressEvent {
    pub fn new(
        ingress_name: impl Into<String>,
        kind: impl Into<String>,
        source: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            kind: kind.into(),
            source: source.into(),
            ingress_name: ingress_name.into(),
            payload,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            attempt: 1,
            correlation_id: uuid::Uuid::now_v7().to_string(),
        }
    }
}

/// Host-validated output command envelope returned by Service/Ingress handlers.
///
/// Service output commands are intentionally distinct from ordinary plugin
/// `ToolOutput`: handlers can request bounded side effects, but the host parses,
/// validates, grant-checks, records diagnostics, and fail-closes before executing
/// any supported command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginServiceOutputCommandEnvelope {
    pub correlation_id: String,
    pub source_event_id: String,
    pub command_id: String,
    pub kind: PluginServiceOutputCommandKind,
    pub payload: Value,
    pub requested_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginServiceOutputCommandKind {
    DiagnosticStatusUpdate,
    HostRequestDispatch,
    #[serde(rename = "websocket_send")]
    WebSocketSend,
}

impl PluginServiceOutputCommandKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DiagnosticStatusUpdate => "diagnostic_status_update",
            Self::HostRequestDispatch => "host_request_dispatch",
            Self::WebSocketSend => "websocket_send",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PluginServiceOutputCommandStatus {
    Recorded,
    Rejected,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginServiceOutputCommandResult {
    pub correlation_id: Option<String>,
    pub source_event_id: Option<String>,
    pub command_id: Option<String>,
    pub kind: Option<PluginServiceOutputCommandKind>,
    pub status: PluginServiceOutputCommandStatus,
    pub message: String,
    pub recorded_at: String,
}

impl PluginServiceOutputCommandResult {
    fn rejected(message: impl Into<String>) -> Self {
        Self::from_parts(
            None,
            None,
            None,
            None,
            PluginServiceOutputCommandStatus::Rejected,
            message,
        )
    }

    fn rejected_for(
        command: &PluginServiceOutputCommandEnvelope,
        message: impl Into<String>,
    ) -> Self {
        Self::from_command(command, PluginServiceOutputCommandStatus::Rejected, message)
    }

    fn unsupported(
        command: &PluginServiceOutputCommandEnvelope,
        message: impl Into<String>,
    ) -> Self {
        Self::from_command(
            command,
            PluginServiceOutputCommandStatus::Unsupported,
            message,
        )
    }

    fn recorded(command: &PluginServiceOutputCommandEnvelope, message: impl Into<String>) -> Self {
        Self::from_command(command, PluginServiceOutputCommandStatus::Recorded, message)
    }

    fn from_command(
        command: &PluginServiceOutputCommandEnvelope,
        status: PluginServiceOutputCommandStatus,
        message: impl Into<String>,
    ) -> Self {
        Self::from_parts(
            Some(command.correlation_id.clone()),
            Some(command.source_event_id.clone()),
            Some(command.command_id.clone()),
            Some(command.kind),
            status,
            message,
        )
    }

    fn from_parts(
        correlation_id: Option<String>,
        source_event_id: Option<String>,
        command_id: Option<String>,
        kind: Option<PluginServiceOutputCommandKind>,
        status: PluginServiceOutputCommandStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id,
            source_event_id,
            command_id,
            kind,
            status,
            message: bounded_message(redact_secret_like(&message.into())),
            recorded_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginIngressDispatchReport {
    pub plugin_ref: String,
    pub ingress: String,
    pub accepted: bool,
    pub output: Value,
    /// Results of host-side Service output command parsing/validation/grant-checking.
    /// This path never feeds ordinary plugin ToolOutput handling.
    pub output_command_results: Vec<PluginServiceOutputCommandResult>,
    pub queue_depth: usize,
    pub dispatch_counters: PluginIngressDispatchCounters,
    pub diagnostics: Vec<PluginInstanceDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginIngressDispatchError {
    InvalidEvent(String),
    QueueFull { capacity: usize },
    ServiceUnavailable { state: PluginInstanceLifecycleState },
    ServiceFailed(String),
    ServiceStopped { state: PluginInstanceLifecycleState },
    DispatchTimeout { timeout: Duration },
    DispatchFailed(String),
}

impl PluginIngressDispatchError {
    fn bounded_message(&self) -> String {
        match self {
            Self::InvalidEvent(message) => bounded_message(format!(
                "invalid plugin ingress event: {}",
                redact_secret_like(message)
            )),
            Self::QueueFull { capacity } => bounded_message(format!(
                "plugin ingress queue is full (capacity {capacity})"
            )),
            Self::ServiceUnavailable { state } => bounded_message(format!(
                "plugin service is not running for ingress dispatch (state {state:?})"
            )),
            Self::ServiceFailed(message) => bounded_message(format!(
                "plugin service is failed for ingress dispatch: {}",
                redact_secret_like(message)
            )),
            Self::ServiceStopped { state } => bounded_message(format!(
                "plugin service rejects ingress while stopping/stopped (state {state:?})"
            )),
            Self::DispatchTimeout { timeout } => bounded_message(format!(
                "plugin ingress dispatch timed out after {timeout:?}"
            )),
            Self::DispatchFailed(message) => bounded_message(format!(
                "plugin ingress dispatch failed closed: {}",
                redact_secret_like(message)
            )),
        }
    }

    fn diagnostic_kind(&self) -> PluginInstanceDiagnosticKind {
        match self {
            Self::InvalidEvent(_) => PluginInstanceDiagnosticKind::InvalidEvent,
            Self::QueueFull { .. } => PluginInstanceDiagnosticKind::QueueFull,
            Self::ServiceUnavailable { .. } => PluginInstanceDiagnosticKind::ServiceUnavailable,
            Self::ServiceFailed(_) => PluginInstanceDiagnosticKind::ServiceFailed,
            Self::ServiceStopped { .. } => PluginInstanceDiagnosticKind::ServiceStopped,
            Self::DispatchTimeout { .. } => PluginInstanceDiagnosticKind::DispatchTimeout,
            Self::DispatchFailed(_) => PluginInstanceDiagnosticKind::DispatchFailed,
        }
    }
}

impl std::fmt::Display for PluginIngressDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.bounded_message())
    }
}

impl std::error::Error for PluginIngressDispatchError {}

#[derive(Clone, Debug)]
struct QueuedPluginIngress {
    ingress_name: String,
    event: PluginIngressEvent,
    enqueued_at: Instant,
}

#[derive(Clone, Default)]
pub struct PluginInstanceRegistry {
    instances: Arc<Mutex<HashMap<String, PluginInstanceHandle>>>,
}

impl PluginInstanceRegistry {
    pub fn register(
        &self,
        record: ResolvedPluginRecord,
    ) -> Result<PluginInstanceHandle, FeatureInstallError> {
        let key = record.identity.to_string();
        let mut instances = self
            .instances
            .lock()
            .expect("plugin instance registry poisoned");
        if let Some(existing) = instances.get(&key) {
            return Ok(existing.clone());
        }
        let handle = PluginInstanceHandle::new(record).map_err(|error| {
            FeatureInstallError::Install(format!(
                "plugin instance startup failed closed: {}",
                error.bounded_message()
            ))
        })?;
        instances.insert(key, handle.clone());
        Ok(handle)
    }

    pub fn status(&self, plugin_ref: &str) -> Option<PluginInstanceStatus> {
        self.instances
            .lock()
            .expect("plugin instance registry poisoned")
            .get(plugin_ref)
            .map(PluginInstanceHandle::status)
    }

    pub fn handle(&self, plugin_ref: &str) -> Option<PluginInstanceHandle> {
        self.instances
            .lock()
            .expect("plugin instance registry poisoned")
            .get(plugin_ref)
            .cloned()
    }

    pub fn stop(&self, plugin_ref: &str) -> Result<Option<PluginInstanceStatus>, PluginWasmError> {
        let handle = self
            .instances
            .lock()
            .expect("plugin instance registry poisoned")
            .get(plugin_ref)
            .cloned();
        handle.map(|handle| handle.stop()).transpose()
    }
}

#[derive(Clone, Debug)]
struct PluginServiceWebSocketSubscription {
    ingress_name: String,
    source: String,
    url: String,
}

#[derive(Clone, Default)]
struct PluginServiceWebSocketDriver {
    inner: Arc<Mutex<HashMap<String, PluginServiceWebSocketConnection>>>,
}

struct PluginServiceWebSocketConnection {
    status: PluginServiceWebSocketConnectionStatus,
    connection: Option<Arc<Mutex<Box<dyn PluginWebSocketConnection>>>>,
    stop: Arc<AtomicBool>,
}

impl PluginServiceWebSocketDriver {
    fn start_connection(
        &self,
        handle: PluginInstanceHandle,
        client: Arc<dyn PluginWebSocketClient>,
        subscription: PluginServiceWebSocketSubscription,
    ) {
        if self.connection_count() >= PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS {
            let message = format!(
                "host-owned WebSocket connection limit ({}) exceeded for {}",
                PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS, subscription.url
            );
            self.insert_failed_status(&subscription, message.clone());
            handle.record_service_websocket_diagnostic(
                PluginInstanceDiagnosticKind::ServiceWebSocketError,
                message,
                true,
            );
            return;
        }

        let request = PluginWebSocketOpenRequest {
            url: subscription.url.clone(),
            protocols: Vec::new(),
            headers: Vec::new(),
        };
        self.insert_status(
            &subscription,
            PluginServiceWebSocketConnectionState::Connecting,
            None,
        );

        let (request, url) = match validate_plugin_service_websocket_open_request(&handle, &request)
        {
            Ok(value) => value,
            Err(message) => {
                self.update_status_error(
                    &subscription.url,
                    PluginServiceWebSocketConnectionState::Failed,
                    message.clone(),
                );
                handle.record_service_websocket_diagnostic(
                    PluginInstanceDiagnosticKind::ServiceWebSocketError,
                    message,
                    true,
                );
                return;
            }
        };
        if !client.supports_bounded_open() {
            let message = "host-owned WebSocket client cannot guarantee bounded/cancellable open; refusing to dial".to_string();
            self.update_status_error(
                &subscription.url,
                PluginServiceWebSocketConnectionState::Failed,
                message.clone(),
            );
            handle.record_service_websocket_diagnostic(
                PluginInstanceDiagnosticKind::ServiceWebSocketError,
                message,
                true,
            );
            return;
        }
        let connection = match client.open(&request, &url, PluginWebSocketLimits::default()) {
            Ok(connection) => Arc::new(Mutex::new(connection)),
            Err(error) => {
                let message = format!(
                    "host-owned WebSocket open failed for {}: {}",
                    safe_url(&url),
                    error.0
                );
                self.update_status_error(
                    &subscription.url,
                    PluginServiceWebSocketConnectionState::Failed,
                    message.clone(),
                );
                handle.record_service_websocket_diagnostic(
                    PluginInstanceDiagnosticKind::ServiceWebSocketError,
                    message,
                    true,
                );
                return;
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        self.attach_connection(&subscription.url, connection.clone(), stop.clone());
        self.update_status_connected(&subscription.url);
        handle.record_service_websocket_diagnostic(
            PluginInstanceDiagnosticKind::ServiceWebSocketConnected,
            format!("host-owned WebSocket connected: {}", safe_url(&url)),
            false,
        );

        let driver = self.clone();
        std::thread::spawn(move || {
            driver.reader_loop(handle, subscription, connection, stop);
        });
    }

    fn reader_loop(
        &self,
        handle: PluginInstanceHandle,
        subscription: PluginServiceWebSocketSubscription,
        connection: Arc<Mutex<Box<dyn PluginWebSocketConnection>>>,
        stop: Arc<AtomicBool>,
    ) {
        while !stop.load(Ordering::SeqCst) {
            let recv = {
                let mut connection = connection
                    .lock()
                    .expect("service websocket connection poisoned");
                connection.recv_text(
                    PLUGIN_SERVICE_WEBSOCKET_RECV_TIMEOUT,
                    PLUGIN_WEBSOCKET_MAX_MESSAGE_BYTES,
                )
            };
            match recv {
                Ok(PluginWebSocketRecvResponse::Text { text }) => {
                    self.record_frame(&subscription.url);
                    let event = PluginIngressEvent::new(
                        subscription.ingress_name.clone(),
                        "websocket_text",
                        subscription.source.clone(),
                        serde_json::json!({
                            "url": subscription.url,
                            "text": text,
                        }),
                    );
                    if let Err(error) = handle.deliver_ingress(&subscription.ingress_name, event) {
                        let message = error.bounded_message();
                        self.record_queue_drop(&subscription.url, message.clone());
                        handle.record_service_websocket_diagnostic(
                            error.diagnostic_kind(),
                            format!("host-owned WebSocket ingress drop: {message}"),
                            true,
                        );
                    }
                }
                Ok(PluginWebSocketRecvResponse::Closed) => {
                    let message = "host-owned WebSocket closed by peer".to_string();
                    self.update_status_error(
                        &subscription.url,
                        PluginServiceWebSocketConnectionState::Closed,
                        message.clone(),
                    );
                    handle.record_service_websocket_diagnostic(
                        PluginInstanceDiagnosticKind::ServiceWebSocketClosed,
                        message.clone(),
                        false,
                    );
                    let event = PluginIngressEvent::new(
                        subscription.ingress_name.clone(),
                        "websocket_close",
                        subscription.source.clone(),
                        serde_json::json!({"url": subscription.url, "reason": message}),
                    );
                    let _ = handle.deliver_ingress(&subscription.ingress_name, event);
                    break;
                }
                Err(error) if error.0.contains("timed out") => continue,
                Err(error) => {
                    let message = format!("host-owned WebSocket receive failed: {}", error.0);
                    self.update_status_error(
                        &subscription.url,
                        PluginServiceWebSocketConnectionState::Failed,
                        message.clone(),
                    );
                    handle.record_service_websocket_diagnostic(
                        PluginInstanceDiagnosticKind::ServiceWebSocketError,
                        message.clone(),
                        true,
                    );
                    let event = PluginIngressEvent::new(
                        subscription.ingress_name.clone(),
                        "websocket_error",
                        subscription.source.clone(),
                        serde_json::json!({"url": subscription.url, "error": message}),
                    );
                    let _ = handle.deliver_ingress(&subscription.ingress_name, event);
                    break;
                }
            }
        }
    }

    fn send_text(&self, url: &str, text: &str) -> Result<String, String> {
        if text.len() > PLUGIN_WEBSOCKET_MAX_TEXT_BYTES {
            return Err(format!(
                "websocket_send text exceeds {} bytes",
                PLUGIN_WEBSOCKET_MAX_TEXT_BYTES
            ));
        }
        let parsed =
            reqwest::Url::parse(url).map_err(|error| format!("invalid WebSocket URL: {error}"))?;
        let key = parsed.as_str().to_string();
        let (display_url, connection) = {
            let guard = self
                .inner
                .lock()
                .expect("service websocket driver poisoned");
            let entry = guard.get(&key).ok_or_else(|| {
                format!(
                    "no host-owned WebSocket connection is active for {}",
                    safe_url(&parsed)
                )
            })?;
            let Some(connection) = &entry.connection else {
                return Err(format!(
                    "host-owned WebSocket connection is not connected for {}",
                    safe_url(&parsed)
                ));
            };
            (entry.status.url.clone(), connection.clone())
        };
        let send = connection
            .lock()
            .expect("service websocket connection poisoned")
            .send_text(text);
        match send {
            Ok(()) => {
                self.record_send_success(&key);
                Ok(format!(
                    "websocket_send sent {} bytes to {display_url}",
                    text.len()
                ))
            }
            Err(error) => {
                let message = format!("websocket_send failed for {display_url}: {}", error.0);
                self.record_send_failure(&key, message.clone());
                Err(message)
            }
        }
    }

    fn statuses(&self) -> Vec<PluginServiceWebSocketConnectionStatus> {
        let mut statuses: Vec<_> = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .values()
            .map(|entry| entry.status.clone())
            .collect();
        statuses.sort_by(|left, right| left.url.cmp(&right.url));
        statuses
    }

    fn stop_all(&self) {
        let connections: Vec<_> = {
            let mut guard = self
                .inner
                .lock()
                .expect("service websocket driver poisoned");
            guard
                .values_mut()
                .filter_map(|entry| {
                    entry.stop.store(true, Ordering::SeqCst);
                    entry.status.state = PluginServiceWebSocketConnectionState::Closed;
                    entry.connection.clone()
                })
                .collect()
        };
        for connection in connections {
            if let Ok(mut connection) = connection.lock() {
                let _ = connection.close();
            }
        }
    }

    fn connection_count(&self) -> usize {
        self.inner
            .lock()
            .expect("service websocket driver poisoned")
            .len()
    }

    fn insert_status(
        &self,
        subscription: &PluginServiceWebSocketSubscription,
        state: PluginServiceWebSocketConnectionState,
        error: Option<String>,
    ) {
        let url = reqwest::Url::parse(&subscription.url)
            .map(|url| safe_url(&url))
            .unwrap_or_else(|_| safe_fs_path(&subscription.url));
        self.inner
            .lock()
            .expect("service websocket driver poisoned")
            .insert(
                subscription.url.clone(),
                PluginServiceWebSocketConnection {
                    status: PluginServiceWebSocketConnectionStatus {
                        url,
                        ingress_name: subscription.ingress_name.clone(),
                        state,
                        last_frame_at: None,
                        last_error: error,
                        received_text_frames: 0,
                        sent_text_frames: 0,
                        queue_drops: 0,
                        send_failures: 0,
                    },
                    connection: None,
                    stop: Arc::new(AtomicBool::new(false)),
                },
            );
    }

    fn insert_failed_status(
        &self,
        subscription: &PluginServiceWebSocketSubscription,
        error: String,
    ) {
        self.insert_status(
            subscription,
            PluginServiceWebSocketConnectionState::Failed,
            Some(error),
        );
    }

    fn attach_connection(
        &self,
        key: &str,
        connection: Arc<Mutex<Box<dyn PluginWebSocketConnection>>>,
        stop: Arc<AtomicBool>,
    ) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.connection = Some(connection);
            entry.stop = stop;
        }
    }

    fn update_status_connected(&self, key: &str) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.state = PluginServiceWebSocketConnectionState::Connected;
            entry.status.last_error = None;
        }
    }

    fn update_status_error(
        &self,
        key: &str,
        state: PluginServiceWebSocketConnectionState,
        error: String,
    ) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.state = state;
            entry.status.last_error = Some(bounded_message(redact_secret_like(&error)));
        }
    }

    fn record_frame(&self, key: &str) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.last_frame_at =
                Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
            entry.status.received_text_frames += 1;
        }
    }

    fn record_queue_drop(&self, key: &str, error: String) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.queue_drops += 1;
            entry.status.last_error = Some(bounded_message(redact_secret_like(&error)));
        }
    }

    fn record_send_success(&self, key: &str) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.sent_text_frames += 1;
        }
    }

    fn record_send_failure(&self, key: &str, error: String) {
        if let Some(entry) = self
            .inner
            .lock()
            .expect("service websocket driver poisoned")
            .get_mut(key)
        {
            entry.status.send_failures += 1;
            entry.status.last_error = Some(bounded_message(redact_secret_like(&error)));
        }
    }
}

fn validate_plugin_service_websocket_open_request(
    handle: &PluginInstanceHandle,
    request: &PluginWebSocketOpenRequest,
) -> Result<(PluginWebSocketOpenRequest, reqwest::Url), String> {
    let record = {
        let instance = handle.0.lock().expect("plugin instance poisoned");
        instance.record.clone()
    };
    authorize_plugin_host_api(&record, PluginHostApi::WebSocket)
        .map_err(|error| format!("host-owned websocket not granted: {}", error.0))?;
    let bytes = serde_json::to_vec(request)
        .map_err(|error| format!("failed to encode host-owned websocket open request: {error}"))?;
    validate_plugin_websocket_open_request(&record, &bytes).map_err(|error| error.0)
}

fn parse_plugin_service_websocket_source(source: &str) -> Option<Result<String, String>> {
    let trimmed = source.trim();
    let raw = if let Some(rest) = trimmed.strip_prefix("websocket:") {
        rest.trim()
    } else if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed
    } else {
        return None;
    };
    if raw.is_empty() {
        return Some(Err("websocket source URL is empty".to_string()));
    }
    let parsed = match reqwest::Url::parse(raw) {
        Ok(parsed) => parsed,
        Err(error) => return Some(Err(format!("invalid WebSocket URL: {error}"))),
    };
    match parsed.scheme() {
        "ws" | "wss" => Some(Ok(parsed.as_str().to_string())),
        other => Some(Err(format!("unsupported WebSocket URL scheme: {other}"))),
    }
}

#[derive(Clone)]
pub struct PluginInstanceHandle(Arc<Mutex<PluginInstance>>);

impl PluginInstanceHandle {
    fn new(record: ResolvedPluginRecord) -> Result<Self, PluginWasmError> {
        Self::new_with_service_websocket_client(record, Arc::new(TungstenitePluginWebSocketClient))
    }

    #[cfg(test)]
    fn new_with_test_websocket_client(
        record: ResolvedPluginRecord,
        service_websocket_client: Arc<dyn PluginWebSocketClient>,
    ) -> Result<Self, PluginWasmError> {
        Self::new_with_service_websocket_client(record, service_websocket_client)
    }

    fn new_with_service_websocket_client(
        record: ResolvedPluginRecord,
        service_websocket_client: Arc<dyn PluginWebSocketClient>,
    ) -> Result<Self, PluginWasmError> {
        let runtime = PluginInstanceRuntime::new(&record)?;
        let mut instance = PluginInstance {
            record,
            runtime,
            lifecycle: PluginInstanceLifecycleState::Ready,
            component_status: None,
            ingress_queue: VecDeque::new(),
            ingress_queue_capacity: PLUGIN_SERVICE_INGRESS_QUEUE_CAPACITY,
            dispatch_counters: PluginIngressDispatchCounters::default(),
            last_error: None,
            output_command_results: Vec::new(),
            service_websockets: PluginServiceWebSocketDriver::default(),
            service_websocket_client,
            diagnostics: Vec::new(),
        };
        instance.start()?;
        let handle = Self(Arc::new(Mutex::new(instance)));
        handle.start_service_websockets();
        Ok(handle)
    }

    fn handle_tool(&self, tool_name: &str, input: Vec<u8>) -> Result<ToolOutput, PluginWasmError> {
        self.0
            .lock()
            .expect("plugin instance poisoned")
            .handle_tool(tool_name, input)
    }

    pub fn deliver_ingress(
        &self,
        ingress_name: &str,
        event: PluginIngressEvent,
    ) -> Result<PluginIngressDispatchReport, PluginIngressDispatchError> {
        let mut instance = self.0.lock().expect("plugin instance poisoned");
        instance.enqueue_ingress(ingress_name, event)?;
        instance.dispatch_next_ingress()
    }

    pub fn status(&self) -> PluginInstanceStatus {
        let mut instance = self.0.lock().expect("plugin instance poisoned");
        instance.status()
    }

    pub fn stop(&self) -> Result<PluginInstanceStatus, PluginWasmError> {
        let mut instance = self.0.lock().expect("plugin instance poisoned");
        instance.stop()?;
        Ok(instance.snapshot_status())
    }

    fn record_diagnostic(&self, diagnostic: PluginInstanceDiagnostic) {
        if let Ok(mut instance) = self.0.lock() {
            instance.lifecycle = diagnostic.state.clone();
            instance.last_error = Some(diagnostic.message.clone());
            instance.diagnostics.push(diagnostic);
        }
    }

    fn record_service_websocket_diagnostic(
        &self,
        kind: PluginInstanceDiagnosticKind,
        message: impl Into<String>,
        mark_error: bool,
    ) {
        if let Ok(mut instance) = self.0.lock() {
            let message = bounded_message(redact_secret_like(&message.into()));
            if mark_error {
                instance.last_error = Some(message.clone());
            }
            let state = instance.lifecycle.clone();
            instance
                .diagnostics
                .push(PluginInstanceDiagnostic::with_kind(kind, state, message));
        }
    }

    fn start_service_websockets(&self) {
        let (driver, client, subscriptions, diagnostics) = {
            let instance = self.0.lock().expect("plugin instance poisoned");
            let (subscriptions, diagnostics) = instance.service_websocket_subscriptions();
            let driver = instance.service_websockets.clone();
            let client = instance.service_websocket_client.clone();
            (driver, client, subscriptions, diagnostics)
        };
        for diagnostic in diagnostics {
            self.record_service_websocket_diagnostic(
                PluginInstanceDiagnosticKind::ServiceWebSocketError,
                diagnostic,
                true,
            );
        }
        for subscription in subscriptions {
            driver.start_connection(self.clone(), client.clone(), subscription);
        }
    }
}

struct PluginInstance {
    record: ResolvedPluginRecord,
    runtime: PluginInstanceRuntime,
    lifecycle: PluginInstanceLifecycleState,
    component_status: Option<Value>,
    ingress_queue: VecDeque<QueuedPluginIngress>,
    ingress_queue_capacity: usize,
    dispatch_counters: PluginIngressDispatchCounters,
    last_error: Option<String>,
    output_command_results: Vec<PluginServiceOutputCommandResult>,
    service_websockets: PluginServiceWebSocketDriver,
    service_websocket_client: Arc<dyn PluginWebSocketClient>,
    diagnostics: Vec<PluginInstanceDiagnostic>,
}

impl PluginInstance {
    fn start(&mut self) -> Result<(), PluginWasmError> {
        self.lifecycle = PluginInstanceLifecycleState::Starting;
        let start_result = match &mut self.runtime {
            PluginInstanceRuntime::ComponentToolAdapter => {
                self.lifecycle = PluginInstanceLifecycleState::Ready;
                self.diagnostics.push(PluginInstanceDiagnostic::new(
                    PluginInstanceLifecycleState::Ready,
                    "component tool runtime registered behind PluginInstanceRegistry",
                ));
                Ok(())
            }
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { .. } => {
                self.lifecycle = PluginInstanceLifecycleState::Running;
                self.diagnostics.push(PluginInstanceDiagnostic::new(
                    PluginInstanceLifecycleState::Running,
                    "test ingress runtime initialized",
                ));
                Ok(())
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                match runtime.start(&self.record) {
                    Ok(status) => {
                        self.component_status = Some(status);
                        self.lifecycle = PluginInstanceLifecycleState::Running;
                        self.diagnostics.push(PluginInstanceDiagnostic::new(
                        PluginInstanceLifecycleState::Running,
                        "component instance start returned; host-managed ingress queue is running",
                    ));
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = start_result {
            self.lifecycle = PluginInstanceLifecycleState::Failed;
            self.record_runtime_error(
                PluginInstanceDiagnosticKind::ServiceFailed,
                format!("plugin component start failed: {}", error.bounded_message()),
            );
            return Err(error);
        }
        Ok(())
    }

    fn service_websocket_subscriptions(
        &self,
    ) -> (Vec<PluginServiceWebSocketSubscription>, Vec<String>) {
        if !surface_enabled(&self.record, PluginSurface::Service)
            || !surface_enabled(&self.record, PluginSurface::Ingress)
        {
            return (Vec::new(), Vec::new());
        }
        let mut subscriptions = Vec::new();
        let mut diagnostics = Vec::new();
        for ingress in &self.record.manifest.ingresses {
            for source in &ingress.sources {
                match parse_plugin_service_websocket_source(source) {
                    None => {}
                    Some(Ok(url)) => subscriptions.push(PluginServiceWebSocketSubscription {
                        ingress_name: ingress.name.clone(),
                        source: source.clone(),
                        url,
                    }),
                    Some(Err(message)) => diagnostics.push(format!(
                        "invalid WebSocket ingress source for {}: {message}",
                        ingress.name
                    )),
                }
            }
        }
        (subscriptions, diagnostics)
    }

    fn handle_tool(
        &mut self,
        tool_name: &str,
        input: Vec<u8>,
    ) -> Result<ToolOutput, PluginWasmError> {
        if !surface_enabled(&self.record, PluginSurface::Tool) {
            return Err(PluginWasmError::Module(
                "plugin tool surface is not enabled".to_string(),
            ));
        }
        let tool = self
            .record
            .manifest
            .tools
            .iter()
            .find(|tool| tool.name == tool_name)
            .ok_or_else(|| {
                PluginWasmError::Module(
                    "requested tool is not declared by plugin manifest".to_string(),
                )
            })?;
        authorize_plugin_tool(&self.record, tool).map_err(|error| {
            PluginWasmError::Module(format!(
                "plugin permission denied: {}",
                error.bounded_message()
            ))
        })?;
        match &mut self.runtime {
            PluginInstanceRuntime::ComponentToolAdapter => {
                run_plugin_component_tool(self.record.clone(), tool_name.to_string(), input)
            }
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { tool_calls, .. } => {
                *tool_calls += 1;
                Ok(ToolOutput {
                    summary: format!("{tool_name}: {tool_calls}"),
                    content: Some(String::from_utf8_lossy(&input).to_string()),

                    attachments: Vec::new(),
                })
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                runtime.handle_tool(tool_name, input)
            }
        }
    }

    fn enqueue_ingress(
        &mut self,
        ingress_name: &str,
        event: PluginIngressEvent,
    ) -> Result<(), PluginIngressDispatchError> {
        self.validate_ingress_event(ingress_name, &event)
            .map_err(|error| self.record_rejection(error))?;
        if self.ingress_queue.len() >= self.ingress_queue_capacity {
            let error = PluginIngressDispatchError::QueueFull {
                capacity: self.ingress_queue_capacity,
            };
            return Err(self.record_rejection(error));
        }
        self.ingress_queue.push_back(QueuedPluginIngress {
            ingress_name: ingress_name.to_string(),
            event,
            enqueued_at: Instant::now(),
        });
        self.dispatch_counters.enqueued += 1;
        Ok(())
    }

    fn validate_ingress_event(
        &self,
        ingress_name: &str,
        event: &PluginIngressEvent,
    ) -> Result<(), PluginIngressDispatchError> {
        if !surface_enabled(&self.record, PluginSurface::Ingress) {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "plugin ingress surface is not enabled".to_string(),
            ));
        }
        match self.lifecycle {
            PluginInstanceLifecycleState::Running => {}
            PluginInstanceLifecycleState::Failed => {
                return Err(PluginIngressDispatchError::ServiceFailed(
                    self.last_error
                        .clone()
                        .unwrap_or_else(|| "service is failed".to_string()),
                ));
            }
            PluginInstanceLifecycleState::Stopping | PluginInstanceLifecycleState::Stopped => {
                return Err(PluginIngressDispatchError::ServiceStopped {
                    state: self.lifecycle.clone(),
                });
            }
            _ => {
                return Err(PluginIngressDispatchError::ServiceUnavailable {
                    state: self.lifecycle.clone(),
                });
            }
        }
        if event.source.trim().is_empty() {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "source must not be empty".to_string(),
            ));
        }
        if event.ingress_name != ingress_name {
            return Err(PluginIngressDispatchError::InvalidEvent(format!(
                "event ingress `{}` does not match dispatch ingress `{ingress_name}`",
                event.ingress_name
            )));
        }
        if event.kind.trim().is_empty() {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "event kind must not be empty".to_string(),
            ));
        }
        if event.created_at.trim().is_empty() {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "created_at must not be empty".to_string(),
            ));
        }
        if event.attempt == 0 {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "attempt must be greater than zero".to_string(),
            ));
        }
        if event.correlation_id.trim().is_empty() {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "correlation_id must not be empty".to_string(),
            ));
        }
        if serde_json::to_vec(event)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX)
            > PLUGIN_WASM_MAX_INPUT_BYTES
        {
            return Err(PluginIngressDispatchError::InvalidEvent(format!(
                "plugin ingress event exceeds {} bytes",
                PLUGIN_WASM_MAX_INPUT_BYTES
            )));
        }
        let ingress = self
            .record
            .manifest
            .ingresses
            .iter()
            .find(|ingress| ingress.name == ingress_name)
            .ok_or_else(|| {
                PluginIngressDispatchError::InvalidEvent(
                    "requested ingress is not declared by plugin manifest".to_string(),
                )
            })?;
        if !ingress.event_kinds.is_empty()
            && !ingress.event_kinds.iter().any(|kind| kind == &event.kind)
        {
            return Err(PluginIngressDispatchError::InvalidEvent(format!(
                "event kind `{}` is not declared for ingress `{ingress_name}`",
                event.kind
            )));
        }
        authorize_plugin_ingress(&self.record, ingress_name).map_err(|error| {
            PluginIngressDispatchError::InvalidEvent(format!(
                "plugin ingress permission denied: {}",
                error.bounded_message()
            ))
        })?;
        Ok(())
    }

    fn dispatch_next_ingress(
        &mut self,
    ) -> Result<PluginIngressDispatchReport, PluginIngressDispatchError> {
        let Some(queued) = self.ingress_queue.pop_front() else {
            return Err(PluginIngressDispatchError::InvalidEvent(
                "plugin ingress queue is empty".to_string(),
            ));
        };
        let started_at = Instant::now();
        let result = self.dispatch_ingress_now(&queued.ingress_name, queued.event);
        let elapsed = started_at.elapsed();
        if elapsed > PLUGIN_SERVICE_INGRESS_DISPATCH_TIMEOUT {
            let error = PluginIngressDispatchError::DispatchTimeout {
                timeout: PLUGIN_SERVICE_INGRESS_DISPATCH_TIMEOUT,
            };
            self.dispatch_counters.timed_out += 1;
            self.dispatch_counters.failed += 1;
            self.lifecycle = PluginInstanceLifecycleState::Failed;
            self.record_dispatch_error(&error);
            return Err(error);
        }
        match result {
            Ok(mut report) => {
                let queue_latency_ms = queued.enqueued_at.elapsed().as_millis() as u64;
                if report.output.get("queue_latency_ms").is_none() {
                    if let Some(map) = report.output.as_object_mut() {
                        map.insert(
                            "queue_latency_ms".to_string(),
                            Value::from(queue_latency_ms),
                        );
                    }
                }
                self.dispatch_counters.dispatched += 1;
                report.queue_depth = self.ingress_queue.len();
                report.dispatch_counters = self.dispatch_counters.clone();
                report.diagnostics = self.diagnostics.clone();
                Ok(report)
            }
            Err(error) => {
                self.dispatch_counters.failed += 1;
                self.lifecycle = PluginInstanceLifecycleState::Failed;
                let dispatch_error =
                    PluginIngressDispatchError::DispatchFailed(error.bounded_message());
                self.record_dispatch_error(&dispatch_error);
                Err(dispatch_error)
            }
        }
    }

    fn dispatch_ingress_now(
        &mut self,
        ingress_name: &str,
        event: PluginIngressEvent,
    ) -> Result<PluginIngressDispatchReport, PluginWasmError> {
        let output = match &mut self.runtime {
            PluginInstanceRuntime::ComponentToolAdapter => {
                return Err(PluginWasmError::Module(
                    "component tool runtime does not expose ingress dispatch".to_string(),
                ));
            }
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress {
                tool_calls,
                ingress_calls,
            } => {
                if let Some(sleep_ms) = event.payload.get("sleep_ms").and_then(Value::as_u64) {
                    std::thread::sleep(Duration::from_millis(sleep_ms));
                }
                if event.payload.get("fail").and_then(Value::as_bool) == Some(true) {
                    return Err(PluginWasmError::Execution(
                        "test ingress requested failure".to_string(),
                    ));
                }
                *ingress_calls += 1;
                let mut output = serde_json::json!({
                    "ingress": ingress_name,
                    "kind": event.kind.clone(),
                    "source": event.source.clone(),
                    "ingress_name": event.ingress_name.clone(),
                    "attempt": event.attempt,
                    "correlation_id": event.correlation_id.clone(),
                    "calls": *tool_calls,
                    "ingress_calls": *ingress_calls,
                    "payload": event.payload.clone(),
                });
                if let (Some(map), Some(commands)) = (
                    output.as_object_mut(),
                    event.payload.get("output_commands").cloned(),
                ) {
                    map.insert("output_commands".to_string(), commands);
                }
                output
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                runtime.handle_ingress(ingress_name, &event)?
            }
        };
        let output_command_results = self.process_service_output_commands(&output, &event);
        Ok(PluginIngressDispatchReport {
            plugin_ref: self.record.identity.to_string(),
            ingress: ingress_name.to_string(),
            accepted: true,
            output,
            output_command_results,
            queue_depth: self.ingress_queue.len(),
            dispatch_counters: self.dispatch_counters.clone(),
            diagnostics: self.diagnostics.clone(),
        })
    }

    fn stop(&mut self) -> Result<(), PluginWasmError> {
        if self.lifecycle == PluginInstanceLifecycleState::Stopped {
            return Ok(());
        }
        self.lifecycle = PluginInstanceLifecycleState::Stopping;
        self.diagnostics.push(PluginInstanceDiagnostic::new(
            PluginInstanceLifecycleState::Stopping,
            "plugin service stop requested; ingress queue is closed",
        ));
        self.ingress_queue.clear();
        self.service_websockets.stop_all();
        let stop_result = match &mut self.runtime {
            PluginInstanceRuntime::ComponentToolAdapter => Ok(()),
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { .. } => Ok(()),
            PluginInstanceRuntime::ComponentInstance(runtime) => match runtime.stop() {
                Ok(status) => {
                    self.component_status = Some(status);
                    Ok(())
                }
                Err(error) => Err(error),
            },
        };
        if let Err(error) = stop_result {
            self.lifecycle = PluginInstanceLifecycleState::Failed;
            self.record_runtime_error(
                PluginInstanceDiagnosticKind::ServiceFailed,
                format!("plugin component stop failed: {}", error.bounded_message()),
            );
            return Err(error);
        }
        self.lifecycle = PluginInstanceLifecycleState::Stopped;
        self.diagnostics.push(PluginInstanceDiagnostic::new(
            PluginInstanceLifecycleState::Stopped,
            "plugin service stopped",
        ));
        Ok(())
    }

    fn snapshot_status(&self) -> PluginInstanceStatus {
        PluginInstanceStatus {
            plugin_ref: self.record.identity.to_string(),
            lifecycle: self.lifecycle.clone(),
            component_status: self.component_status.clone(),
            queue_depth: self.ingress_queue.len(),
            queue_capacity: self.ingress_queue_capacity,
            last_error: self.last_error.clone(),
            dispatch_counters: self.dispatch_counters.clone(),
            output_command_results: self.output_command_results.clone(),
            websocket_connections: self.service_websockets.statuses(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn status(&mut self) -> PluginInstanceStatus {
        if self.lifecycle == PluginInstanceLifecycleState::Running {
            if let PluginInstanceRuntime::ComponentInstance(runtime) = &mut self.runtime {
                match runtime.status() {
                    Ok(status) => self.component_status = Some(status),
                    Err(error) => {
                        self.lifecycle = PluginInstanceLifecycleState::Failed;
                        self.record_runtime_error(
                            PluginInstanceDiagnosticKind::ServiceFailed,
                            format!(
                                "plugin component status failed: {}",
                                error.bounded_message()
                            ),
                        );
                    }
                }
            }
        }
        self.snapshot_status()
    }

    fn record_rejection(
        &mut self,
        error: PluginIngressDispatchError,
    ) -> PluginIngressDispatchError {
        self.dispatch_counters.rejected += 1;
        self.record_dispatch_error(&error);
        error
    }

    fn record_dispatch_error(&mut self, error: &PluginIngressDispatchError) {
        let state = match error {
            PluginIngressDispatchError::DispatchFailed(_)
            | PluginIngressDispatchError::DispatchTimeout { .. }
            | PluginIngressDispatchError::ServiceFailed(_) => PluginInstanceLifecycleState::Failed,
            PluginIngressDispatchError::ServiceStopped { .. } => self.lifecycle.clone(),
            _ => self.lifecycle.clone(),
        };
        self.record_runtime_error(error.diagnostic_kind(), error.bounded_message());
        if matches!(state, PluginInstanceLifecycleState::Failed) {
            self.lifecycle = PluginInstanceLifecycleState::Failed;
        }
    }

    fn process_service_output_commands(
        &mut self,
        output: &Value,
        event: &PluginIngressEvent,
    ) -> Vec<PluginServiceOutputCommandResult> {
        let results = match self.validate_service_output_commands(output, event) {
            Ok(commands) => {
                let mut results = Vec::with_capacity(commands.len());
                for command in commands {
                    results.push(self.execute_service_output_command(command));
                }
                results
            }
            Err(results) => results,
        };
        self.record_service_output_command_results(&results);
        results
    }

    fn validate_service_output_commands(
        &self,
        output: &Value,
        event: &PluginIngressEvent,
    ) -> Result<Vec<PluginServiceOutputCommandEnvelope>, Vec<PluginServiceOutputCommandResult>>
    {
        let Some(values) = output.get("output_commands") else {
            return Ok(Vec::new());
        };
        let Some(values) = values.as_array() else {
            return Err(vec![PluginServiceOutputCommandResult::rejected(
                "service output_commands must be an array",
            )]);
        };
        if values.len() > PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_COUNT {
            return Err(vec![PluginServiceOutputCommandResult::rejected(format!(
                "service output_commands exceeds {} commands",
                PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_COUNT
            ))]);
        }

        let mut commands = Vec::with_capacity(values.len());
        let mut rejected = Vec::new();
        for value in values {
            let command: PluginServiceOutputCommandEnvelope =
                match serde_json::from_value(value.clone()) {
                    Ok(command) => command,
                    Err(error) => {
                        rejected.push(PluginServiceOutputCommandResult::rejected(format!(
                            "malformed service output command envelope: {error}"
                        )));
                        continue;
                    }
                };
            if let Err(message) = self.validate_service_output_command_envelope(&command, event) {
                rejected.push(PluginServiceOutputCommandResult::rejected_for(
                    &command, message,
                ));
                continue;
            }
            if let Err(message) = self.grant_check_service_output_command(&command) {
                rejected.push(PluginServiceOutputCommandResult::rejected_for(
                    &command, message,
                ));
                continue;
            }
            commands.push(command);
        }

        if rejected.is_empty() {
            Ok(commands)
        } else {
            Err(rejected)
        }
    }

    fn validate_service_output_command_envelope(
        &self,
        command: &PluginServiceOutputCommandEnvelope,
        event: &PluginIngressEvent,
    ) -> Result<(), String> {
        validate_output_command_id("correlation_id", &command.correlation_id)?;
        validate_output_command_id("source_event_id", &command.source_event_id)?;
        validate_output_command_id("command_id", &command.command_id)?;
        if command.source_event_id != event.correlation_id {
            return Err("source_event_id must match the ingress event correlation_id".to_string());
        }
        chrono::DateTime::parse_from_rfc3339(&command.requested_at)
            .map_err(|error| format!("requested_at must be RFC3339: {error}"))?;
        let payload_bytes = serde_json::to_vec(&command.payload)
            .map_err(|error| format!("payload is not serializable JSON: {error}"))?;
        if payload_bytes.len() > PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_PAYLOAD_BYTES {
            return Err(format!(
                "payload exceeds {} bytes",
                PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_PAYLOAD_BYTES
            ));
        }
        match command.kind {
            PluginServiceOutputCommandKind::DiagnosticStatusUpdate => {
                serde_json::from_value::<PluginServiceDiagnosticStatusCommandPayload>(
                    command.payload.clone(),
                )
                .map_err(|error| format!("invalid diagnostic_status_update payload: {error}"))?;
            }
            PluginServiceOutputCommandKind::HostRequestDispatch => {
                let request: PluginRequestRequest = serde_json::from_value(command.payload.clone())
                    .map_err(|error| format!("invalid host_request_dispatch payload: {error}"))?;
                validate_plugin_request_request(&self.record, &request)
                    .map_err(|error| format!("host_request_dispatch target denied: {}", error.0))?;
            }
            PluginServiceOutputCommandKind::WebSocketSend => {
                let payload: PluginServiceWebSocketSendCommandPayload =
                    serde_json::from_value(command.payload.clone())
                        .map_err(|error| format!("invalid websocket_send payload: {error}"))?;
                self.validate_service_websocket_send_payload(&payload)?;
            }
        }
        Ok(())
    }

    fn grant_check_service_output_command(
        &self,
        command: &PluginServiceOutputCommandEnvelope,
    ) -> Result<(), String> {
        match command.kind {
            PluginServiceOutputCommandKind::DiagnosticStatusUpdate => Ok(()),
            PluginServiceOutputCommandKind::HostRequestDispatch => {
                authorize_plugin_host_api(&self.record, PluginHostApi::Request)
                    .map_err(|error| format!("host_request_dispatch not granted: {}", error.0))
            }
            PluginServiceOutputCommandKind::WebSocketSend => {
                authorize_plugin_host_api(&self.record, PluginHostApi::WebSocket)
                    .map_err(|error| format!("websocket_send not granted: {}", error.0))
            }
        }
    }

    fn validate_service_websocket_send_payload(
        &self,
        payload: &PluginServiceWebSocketSendCommandPayload,
    ) -> Result<(), String> {
        if payload.text.len() > PLUGIN_WEBSOCKET_MAX_TEXT_BYTES {
            return Err(format!(
                "websocket_send text exceeds {} bytes",
                PLUGIN_WEBSOCKET_MAX_TEXT_BYTES
            ));
        }
        let url = reqwest::Url::parse(&payload.url)
            .map_err(|error| format!("invalid WebSocket URL: {error}"))?;
        match url.scheme() {
            "ws" | "wss" => {}
            "http" | "https" => {
                return Err("HTTP URLs are not supported by websocket_send".to_string());
            }
            scheme => {
                return Err(format!(
                    "unsupported WebSocket URL scheme {scheme:?}; only ws and wss are allowed"
                ));
            }
        }
        if url.host_str().is_none() {
            return Err("WebSocket URL must include a host".to_string());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("WebSocket URLs with embedded credentials are not allowed".to_string());
        }
        validate_static_request_target(&url).map_err(|error| error.0)?;
        authorize_websocket_allowlist(&self.record, &url).map_err(|error| error.0)?;
        Ok(())
    }

    fn execute_service_output_command(
        &mut self,
        command: PluginServiceOutputCommandEnvelope,
    ) -> PluginServiceOutputCommandResult {
        match command.kind {
            PluginServiceOutputCommandKind::DiagnosticStatusUpdate => {
                let payload: PluginServiceDiagnosticStatusCommandPayload =
                    match serde_json::from_value(command.payload.clone()) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return PluginServiceOutputCommandResult::rejected_for(
                                &command,
                                format!("invalid diagnostic_status_update payload: {error}"),
                            );
                        }
                    };
                if let Some(status) = payload.status {
                    self.component_status = Some(status);
                }
                let message = payload
                    .message
                    .as_deref()
                    .map(bounded_message)
                    .unwrap_or_else(|| "plugin service status update recorded".to_string());
                PluginServiceOutputCommandResult::recorded(&command, message)
            }
            PluginServiceOutputCommandKind::HostRequestDispatch => {
                PluginServiceOutputCommandResult::unsupported(
                    &command,
                    "host_request_dispatch output command is grant-checked but transport dispatch is unsupported in v0",
                )
            }
            PluginServiceOutputCommandKind::WebSocketSend => {
                let payload: PluginServiceWebSocketSendCommandPayload =
                    match serde_json::from_value(command.payload.clone()) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return PluginServiceOutputCommandResult::rejected_for(
                                &command,
                                format!("invalid websocket_send payload: {error}"),
                            );
                        }
                    };
                match self
                    .service_websockets
                    .send_text(&payload.url, &payload.text)
                {
                    Ok(message) => PluginServiceOutputCommandResult::recorded(&command, message),
                    Err(message) => {
                        self.diagnostics.push(PluginInstanceDiagnostic::with_kind(
                            PluginInstanceDiagnosticKind::ServiceWebSocketSendFailed,
                            self.lifecycle.clone(),
                            bounded_message(redact_secret_like(&message)),
                        ));
                        PluginServiceOutputCommandResult::rejected_for(&command, message)
                    }
                }
            }
        }
    }

    fn record_service_output_command_results(
        &mut self,
        results: &[PluginServiceOutputCommandResult],
    ) {
        if results.is_empty() {
            return;
        }
        for result in results {
            let kind = match result.status {
                PluginServiceOutputCommandStatus::Recorded => {
                    PluginInstanceDiagnosticKind::ServiceOutputCommandRecorded
                }
                PluginServiceOutputCommandStatus::Rejected => {
                    PluginInstanceDiagnosticKind::ServiceOutputCommandRejected
                }
                PluginServiceOutputCommandStatus::Unsupported => {
                    PluginInstanceDiagnosticKind::ServiceOutputCommandUnsupported
                }
            };
            let command_label = result.command_id.as_deref().unwrap_or("<malformed>");
            let command_kind = result
                .kind
                .map(PluginServiceOutputCommandKind::as_str)
                .unwrap_or("unknown");
            let message = bounded_message(format!(
                "service output command {command_label} ({command_kind}): {}",
                result.message
            ));
            if !matches!(result.status, PluginServiceOutputCommandStatus::Recorded) {
                self.last_error = Some(message.clone());
            }
            self.diagnostics.push(PluginInstanceDiagnostic::with_kind(
                kind,
                self.lifecycle.clone(),
                message,
            ));
        }
        self.output_command_results.extend_from_slice(results);
        if self.output_command_results.len() > PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_RESULTS {
            let keep_from =
                self.output_command_results.len() - PLUGIN_SERVICE_OUTPUT_COMMAND_MAX_RESULTS;
            self.output_command_results.drain(0..keep_from);
        }
    }

    fn record_runtime_error(
        &mut self,
        kind: PluginInstanceDiagnosticKind,
        message: impl Into<String>,
    ) {
        let message = bounded_message(redact_secret_like(&message.into()));
        self.last_error = Some(message.clone());
        self.diagnostics.push(PluginInstanceDiagnostic::with_kind(
            kind,
            self.lifecycle.clone(),
            message,
        ));
    }
}

enum PluginInstanceRuntime {
    ComponentToolAdapter,
    #[cfg(test)]
    TestIngress {
        tool_calls: u64,
        ingress_calls: u64,
    },
    ComponentInstance(PluginComponentInstanceRuntime),
}

impl PluginInstanceRuntime {
    fn new(record: &ResolvedPluginRecord) -> Result<Self, PluginWasmError> {
        let Some(runtime) = record.manifest.runtime.as_ref() else {
            return Err(PluginWasmError::Module(
                "plugin runtime is not declared".to_string(),
            ));
        };
        match runtime.kind.as_str() {
            #[cfg(test)]
            "test-ingress" => Ok(Self::TestIngress {
                tool_calls: 0,
                ingress_calls: 0,
            }),
            PLUGIN_RUNTIME_COMPONENT_KIND
                if runtime.world.as_deref() == Some(PLUGIN_COMPONENT_INSTANCE_WORLD) =>
            {
                Ok(Self::ComponentInstance(
                    PluginComponentInstanceRuntime::instantiate(record)?,
                ))
            }
            PLUGIN_RUNTIME_COMPONENT_KIND
                if runtime.world.as_deref() == Some(PLUGIN_COMPONENT_TOOL_WORLD) =>
            {
                Ok(Self::ComponentToolAdapter)
            }
            PLUGIN_RUNTIME_COMPONENT_KIND => Err(PluginWasmError::Module(
                "unsupported or missing plugin component world".to_string(),
            )),
            LEGACY_PLUGIN_RUNTIME_WASM_KIND => Err(PluginWasmError::Module(
                "legacy raw wasm plugin runtime is not supported; use wasm-component".to_string(),
            )),
            other => Err(PluginWasmError::Module(format!(
                "unsupported plugin runtime kind `{other}`"
            ))),
        }
    }
}

struct PluginComponentInstanceRuntime {
    store: wasmtime::Store<PluginComponentHostState>,
    instance: wasmtime::component::Instance,
}

impl PluginComponentInstanceRuntime {
    fn instantiate(record: &ResolvedPluginRecord) -> Result<Self, PluginWasmError> {
        let limits = PluginDiscoveryLimits::default();
        let component_bytes = read_resolved_plugin_runtime_component(record, &limits)
            .map_err(|diagnostic| PluginWasmError::Package(diagnostic.message))?;
        if component_bytes.len() > limits.max_file_size_bytes as usize {
            return Err(PluginWasmError::Package(format!(
                "WASM component runtime artifact exceeds {} bytes",
                limits.max_file_size_bytes
            )));
        }
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.max_wasm_stack(8 * 1024 * 1024);
        let engine = wasmtime::Engine::new(&config)
            .map_err(|error| PluginWasmError::Module(error.to_string()))?;
        let component =
            wasmtime::component::Component::new(&engine, &component_bytes).map_err(|error| {
                PluginWasmError::Module(format!("component is incompatible: {error:?}"))
            })?;
        validate_component_imports(record, &engine, &component)?;
        let mut linker = wasmtime::component::Linker::<PluginComponentHostState>::new(&engine);
        define_plugin_component_host_imports(&mut linker)?;
        let mut store = wasmtime::Store::new(
            &engine,
            PluginComponentHostState {
                record: record.clone(),
                request_client: Arc::new(ReqwestPluginRequestClient),
                websocket_client: Arc::new(TungstenitePluginWebSocketClient),
                websocket_handles: PluginWebSocketHandles::default(),
                store_limits: wasm_component_store_limits(),
            },
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(PLUGIN_WASM_FUEL)
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        Ok(Self { store, instance })
    }

    fn reset_fuel(&mut self) -> Result<(), PluginWasmError> {
        self.store
            .set_fuel(PLUGIN_WASM_FUEL)
            .map_err(|error| PluginWasmError::Execution(error.to_string()))
    }

    fn start(&mut self, record: &ResolvedPluginRecord) -> Result<Value, PluginWasmError> {
        self.reset_fuel()?;
        let start = self
            .instance
            .get_typed_func::<(&str,), (String,)>(&mut self.store, "start")
            .map_err(|error| {
                PluginWasmError::Module(format!(
                    "component does not export expected `{}` start function: {error}",
                    PLUGIN_COMPONENT_INSTANCE_WORLD
                ))
            })?;
        let config_json = plugin_config_json(record);
        let (status,) = start
            .call(&mut self.store, (&config_json,))
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        decode_plugin_lifecycle_output("start", &status)
    }

    fn handle_tool(
        &mut self,
        tool_name: &str,
        input: Vec<u8>,
    ) -> Result<ToolOutput, PluginWasmError> {
        self.reset_fuel()?;
        let call = self
            .instance
            .get_typed_func::<(&str, &str), (String,)>(&mut self.store, "handle-tool")
            .map_err(|error| {
                PluginWasmError::Module(format!(
                    "component does not export expected `{}` handle-tool function: {error}",
                    PLUGIN_COMPONENT_INSTANCE_WORLD
                ))
            })?;
        let input_json = std::str::from_utf8(&input).map_err(|error| {
            PluginWasmError::Output(format!("plugin component input is not UTF-8: {error}"))
        })?;
        let (output,) = call
            .call(&mut self.store, (tool_name, input_json))
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        decode_plugin_wasm_output(output.as_bytes())
    }

    fn handle_ingress(
        &mut self,
        ingress_name: &str,
        event: &PluginIngressEvent,
    ) -> Result<Value, PluginWasmError> {
        self.reset_fuel()?;
        let call = self
            .instance
            .get_typed_func::<(&str, &str), (String,)>(&mut self.store, "handle-ingress")
            .map_err(|error| {
                PluginWasmError::Module(format!(
                    "component does not export expected `{}` handle-ingress function: {error}",
                    PLUGIN_COMPONENT_INSTANCE_WORLD
                ))
            })?;
        let event_json = serde_json::to_string(event)
            .map_err(|error| PluginWasmError::Output(error.to_string()))?;
        let (output,) = call
            .call(&mut self.store, (ingress_name, event_json.as_str()))
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        if output.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
            return Err(PluginWasmError::Output(format!(
                "plugin ingress output exceeds {} bytes",
                PLUGIN_WASM_MAX_OUTPUT_BYTES
            )));
        }
        serde_json::from_str(&output).map_err(|error| {
            PluginWasmError::Output(format!("plugin ingress output is not JSON: {error}"))
        })
    }

    fn stop(&mut self) -> Result<Value, PluginWasmError> {
        self.reset_fuel()?;
        let stop = self
            .instance
            .get_typed_func::<(), (String,)>(&mut self.store, "stop")
            .map_err(|error| {
                PluginWasmError::Module(format!(
                    "component does not export expected `{}` stop function: {error}",
                    PLUGIN_COMPONENT_INSTANCE_WORLD
                ))
            })?;
        let (status,) = stop
            .call(&mut self.store, ())
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        self.store.data().websocket_handles.close_all();
        decode_plugin_lifecycle_output("stop", &status)
    }

    fn status(&mut self) -> Result<Value, PluginWasmError> {
        self.reset_fuel()?;
        let status = self
            .instance
            .get_typed_func::<(), (String,)>(&mut self.store, "status")
            .map_err(|error| {
                PluginWasmError::Module(format!(
                    "component does not export expected `{}` status function: {error}",
                    PLUGIN_COMPONENT_INSTANCE_WORLD
                ))
            })?;
        let (status,) = status
            .call(&mut self.store, ())
            .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
        decode_plugin_lifecycle_output("status", &status)
    }
}

fn decode_plugin_lifecycle_output(phase: &str, output: &str) -> Result<Value, PluginWasmError> {
    if output.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
        return Err(PluginWasmError::Output(format!(
            "plugin component {phase} output exceeds {} bytes",
            PLUGIN_WASM_MAX_OUTPUT_BYTES
        )));
    }
    let value: Value = serde_json::from_str(output).map_err(|error| {
        PluginWasmError::Output(format!(
            "plugin component {phase} output is not JSON: {error}"
        ))
    })?;
    if let Some(error) = value.get("error") {
        return Err(PluginWasmError::Execution(format!(
            "plugin component {phase} returned error: {}",
            bounded_message(error.to_string())
        )));
    }
    if value.get("state").and_then(Value::as_str) == Some("failed") {
        return Err(PluginWasmError::Execution(format!(
            "plugin component {phase} returned failed status: {}",
            bounded_message(value.to_string())
        )));
    }
    Ok(value)
}

fn plugin_config_json(record: &ResolvedPluginRecord) -> String {
    serde_json::to_string(&record.config).unwrap_or_else(|_| "{}".to_string())
}

fn plugin_instance_tool_definition(
    instance: PluginInstanceHandle,
    name: String,
    description: String,
    input_schema: Value,
) -> ToolDefinition {
    let origin = {
        let guard = instance.0.lock().expect("plugin instance poisoned");
        plugin_tool_origin(&guard.record)
    };
    Arc::new(move || {
        (
            ToolMeta::new(name.clone())
                .description(description.clone())
                .input_schema(input_schema.clone())
                .origin(origin.clone()),
            Arc::new(PluginInstanceTool {
                instance: instance.clone(),
                name: name.clone(),
                origin: origin.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

struct PluginInstanceTool {
    instance: PluginInstanceHandle,
    name: String,
    origin: ToolOrigin,
}

#[async_trait]
impl Tool for PluginInstanceTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        if input_json.len() > PLUGIN_WASM_MAX_INPUT_BYTES {
            return Err(ToolError::InvalidArgument(format!(
                "plugin tool `{}` input exceeds {} bytes",
                self.name, PLUGIN_WASM_MAX_INPUT_BYTES
            )));
        }
        serde_json::from_str::<Value>(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!(
                "plugin tool `{}` input is not valid JSON: {}",
                self.name,
                bounded_message(error.to_string())
            ))
        })?;
        let instance = self.instance.clone();
        let name = self.name.clone();
        let plugin_ref = self.origin.plugin_ref.clone();
        let digest = self.origin.digest.clone();
        let input = input_json.as_bytes().to_vec();
        let execution = tokio::task::spawn_blocking(move || instance.handle_tool(&name, input));
        match tokio::time::timeout(PLUGIN_WASM_TIMEOUT, execution).await {
            Ok(Ok(Ok(output))) => Ok(output),
            Ok(Ok(Err(error))) => Err(ToolError::ExecutionFailed(format!(
                "plugin tool `{}` from `{}` (digest {}) failed closed: {}",
                self.name,
                plugin_ref,
                digest,
                error.bounded_message()
            ))),
            Ok(Err(error)) => Err(ToolError::ExecutionFailed(format!(
                "plugin tool `{}` from `{}` (digest {}) cancelled/failed to join: {}",
                self.name,
                plugin_ref,
                digest,
                bounded_message(error.to_string())
            ))),
            Err(_) => {
                self.instance
                    .record_diagnostic(PluginInstanceDiagnostic::new(
                        PluginInstanceLifecycleState::Failed,
                        format!("plugin tool timed out after {:?}", PLUGIN_WASM_TIMEOUT),
                    ));
                Err(ToolError::ExecutionFailed(format!(
                    "plugin tool `{}` from `{}` (digest {}) timed out after {:?}",
                    self.name, plugin_ref, digest, PLUGIN_WASM_TIMEOUT
                )))
            }
        }
    }
}

#[derive(Debug)]
pub enum PluginWasmError {
    Package(String),
    Module(String),
    Execution(String),
    Output(String),
}

impl PluginWasmError {
    fn bounded_message(&self) -> String {
        match self {
            Self::Package(message) => {
                bounded_message(format!("package/module load error: {message}"))
            }
            Self::Module(message) => bounded_message(format!("WASM module error: {message}")),
            Self::Execution(message) => bounded_message(format!("WASM execution error: {message}")),
            Self::Output(message) => bounded_message(format!("WASM output error: {message}")),
        }
    }
}

#[derive(Clone)]
struct PluginComponentHostState {
    record: ResolvedPluginRecord,
    request_client: Arc<dyn PluginRequestClient>,
    websocket_client: Arc<dyn PluginWebSocketClient>,
    websocket_handles: PluginWebSocketHandles,
    store_limits: wasmtime::StoreLimits,
}

fn run_plugin_component_tool(
    record: ResolvedPluginRecord,
    tool_name: String,
    input: Vec<u8>,
) -> Result<ToolOutput, PluginWasmError> {
    run_plugin_component_tool_with_request_client(
        record,
        tool_name,
        input,
        Arc::new(ReqwestPluginRequestClient),
    )
}

fn run_plugin_component_tool_with_request_client(
    record: ResolvedPluginRecord,
    tool_name: String,
    input: Vec<u8>,
    request_client: Arc<dyn PluginRequestClient>,
) -> Result<ToolOutput, PluginWasmError> {
    let tool = record
        .manifest
        .tools
        .iter()
        .find(|tool| tool.name == tool_name)
        .ok_or_else(|| {
            PluginWasmError::Module("requested tool is not declared by plugin manifest".to_string())
        })?;
    authorize_plugin_tool(&record, tool).map_err(|error| {
        PluginWasmError::Module(format!(
            "plugin permission denied: {}",
            error.bounded_message()
        ))
    })?;

    let limits = PluginDiscoveryLimits::default();
    let component_bytes = read_resolved_plugin_runtime_component(&record, &limits)
        .map_err(|diagnostic| PluginWasmError::Package(diagnostic.message))?;
    if component_bytes.len() > limits.max_file_size_bytes as usize {
        return Err(PluginWasmError::Package(format!(
            "WASM component runtime artifact exceeds {} bytes",
            limits.max_file_size_bytes
        )));
    }

    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.max_wasm_stack(8 * 1024 * 1024);
    let engine = wasmtime::Engine::new(&config)
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    let component =
        wasmtime::component::Component::new(&engine, &component_bytes).map_err(|error| {
            PluginWasmError::Module(format!("component is incompatible: {error:?}"))
        })?;
    validate_component_imports(&record, &engine, &component)?;

    let mut linker = wasmtime::component::Linker::<PluginComponentHostState>::new(&engine);
    define_plugin_component_host_imports(&mut linker)?;
    let mut store = wasmtime::Store::new(
        &engine,
        PluginComponentHostState {
            record: record.clone(),
            request_client,
            websocket_client: Arc::new(TungstenitePluginWebSocketClient),
            websocket_handles: PluginWebSocketHandles::default(),
            store_limits: wasm_component_store_limits(),
        },
    );
    store.limiter(|state| &mut state.store_limits);
    store
        .set_fuel(PLUGIN_WASM_FUEL)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
    let call = instance
        .get_typed_func::<(&str, &str), (String,)>(&mut store, "call")
        .map_err(|error| {
            PluginWasmError::Module(format!(
                "component does not export expected `{}` call function: {error}",
                PLUGIN_COMPONENT_TOOL_WORLD
            ))
        })?;
    let input_json = std::str::from_utf8(&input).map_err(|error| {
        PluginWasmError::Output(format!("plugin component input is not UTF-8: {error}"))
    })?;
    // Wasmtime lifts the returned WIT `string` into a host `String` before the
    // ordinary ToolOutput JSON cap can be applied. Keep the component store on
    // the same memory/table/instance limits as the raw WASM runtime so an
    // untrusted component can only force host string allocation from bounded
    // component memory; oversized memories/tables/instances fail closed during
    // instantiation/growth before this lift succeeds.
    let (output,) = call
        .call(&mut store, (&tool_name, input_json))
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
    decode_plugin_wasm_output(output.as_bytes())
}

fn validate_component_imports(
    record: &ResolvedPluginRecord,
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
) -> Result<(), PluginWasmError> {
    for (name, _) in component.component_type().imports(engine) {
        match name {
            "yoi:host/request@1.0.0" => {
                authorize_plugin_host_api(record, PluginHostApi::Request).map_err(|error| {
                    PluginWasmError::Module(format!(
                        "plugin host API dispatch denied: {}",
                        error.bounded_message()
                    ))
                })?;
            }
            "yoi:host/websocket@1.0.0" => {
                authorize_plugin_host_api(record, PluginHostApi::WebSocket).map_err(|error| {
                    PluginWasmError::Module(format!(
                        "plugin host API dispatch denied: {}",
                        error.bounded_message()
                    ))
                })?;
            }
            "yoi:host/fs@1.0.0" => {
                authorize_plugin_host_api(record, PluginHostApi::Fs).map_err(|error| {
                    PluginWasmError::Module(format!(
                        "plugin host API dispatch denied: {}",
                        error.bounded_message()
                    ))
                })?;
            }
            other => {
                return Err(PluginWasmError::Module(format!(
                    "unsupported component import `{other}`; no WASI filesystem, ambient network, environment, or other imports are available"
                )));
            }
        }
    }
    Ok(())
}

fn define_plugin_component_host_imports(
    linker: &mut wasmtime::component::Linker<PluginComponentHostState>,
) -> Result<(), PluginWasmError> {
    linker
        .root()
        .instance("yoi:host/request@1.0.0")
        .map_err(|error| PluginWasmError::Module(error.to_string()))?
        .func_wrap(
            "request",
            |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
             (request,): (String,)|
             -> wasmtime::Result<(String,)> {
                authorize_plugin_host_api(&store.data().record, PluginHostApi::Request)
                    .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
                let response = execute_plugin_request_request(
                    &store.data().record,
                    store.data().request_client.as_ref(),
                    request.as_bytes(),
                )
                .map_err(|error| wasmtime::Error::msg(error.0))?;
                Ok((String::from_utf8_lossy(&response).into_owned(),))
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;

    let mut root = linker.root();
    let mut websocket = root
        .instance("yoi:host/websocket@1.0.0")
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    websocket
        .func_wrap(
            "open",
            |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
             (request,): (String,)|
             -> wasmtime::Result<(String,)> {
                authorize_plugin_host_api(&store.data().record, PluginHostApi::WebSocket)
                    .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
                execute_plugin_websocket_open(
                    &store.data().record,
                    store.data().websocket_client.as_ref(),
                    &store.data().websocket_handles,
                    request.as_bytes(),
                )
                .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
                .map_err(|error| wasmtime::Error::msg(error.0))
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    websocket
        .func_wrap(
            "send-text",
            |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
             (handle, text): (u32, String)|
             -> wasmtime::Result<(String,)> {
                authorize_plugin_host_api(&store.data().record, PluginHostApi::WebSocket)
                    .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
                execute_plugin_websocket_send_text(
                    &store.data().websocket_handles,
                    handle,
                    text.as_bytes(),
                )
                .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
                .map_err(|error| wasmtime::Error::msg(error.0))
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    websocket
        .func_wrap(
            "recv",
            |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
             (handle, timeout_ms): (u32, u32)|
             -> wasmtime::Result<(String,)> {
                authorize_plugin_host_api(&store.data().record, PluginHostApi::WebSocket)
                    .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
                execute_plugin_websocket_recv(&store.data().websocket_handles, handle, timeout_ms)
                    .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
                    .map_err(|error| wasmtime::Error::msg(error.0))
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    websocket
        .func_wrap(
            "close",
            |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
             (handle,): (u32,)|
             -> wasmtime::Result<(String,)> {
                authorize_plugin_host_api(&store.data().record, PluginHostApi::WebSocket)
                    .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
                execute_plugin_websocket_close(&store.data().websocket_handles, handle)
                    .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
                    .map_err(|error| wasmtime::Error::msg(error.0))
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;

    let mut fs = root
        .instance("yoi:host/fs@1.0.0")
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    fs.func_wrap(
        "read",
        |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
         (request,): (String,)|
         -> wasmtime::Result<(String,)> {
            authorize_plugin_host_api(&store.data().record, PluginHostApi::Fs)
                .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
            execute_plugin_fs_request(
                &store.data().record,
                PluginFsRuntimeOperation::Read,
                request.as_bytes(),
            )
            .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
            .map_err(|error| wasmtime::Error::msg(error.message))
        },
    )
    .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    fs.func_wrap(
        "list",
        |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
         (request,): (String,)|
         -> wasmtime::Result<(String,)> {
            authorize_plugin_host_api(&store.data().record, PluginHostApi::Fs)
                .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
            execute_plugin_fs_request(
                &store.data().record,
                PluginFsRuntimeOperation::List,
                request.as_bytes(),
            )
            .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
            .map_err(|error| wasmtime::Error::msg(error.message))
        },
    )
    .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    fs.func_wrap(
        "write",
        |store: wasmtime::StoreContextMut<'_, PluginComponentHostState>,
         (request,): (String,)|
         -> wasmtime::Result<(String,)> {
            authorize_plugin_host_api(&store.data().record, PluginHostApi::Fs)
                .map_err(|error| wasmtime::Error::msg(error.bounded_message()))?;
            execute_plugin_fs_request(
                &store.data().record,
                PluginFsRuntimeOperation::Write,
                request.as_bytes(),
            )
            .map(|bytes| (String::from_utf8_lossy(&bytes).into_owned(),))
            .map_err(|error| wasmtime::Error::msg(error.message))
        },
    )
    .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    Ok(())
}
fn decode_plugin_wasm_output(bytes: &[u8]) -> Result<ToolOutput, PluginWasmError> {
    if bytes.is_empty() {
        return Err(PluginWasmError::Output(
            "guest did not call output_write".into(),
        ));
    }
    if bytes.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
        return Err(PluginWasmError::Output(format!(
            "guest output exceeds {} bytes",
            PLUGIN_WASM_MAX_OUTPUT_BYTES
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| PluginWasmError::Output(format!("guest output is not UTF-8: {error}")))?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        PluginWasmError::Output(format!("guest output is not valid JSON: {error}"))
    })?;
    let Value::Object(map) = value else {
        return Err(PluginWasmError::Output(
            "guest output JSON must be an object".into(),
        ));
    };
    for key in map.keys() {
        if key != "summary" && key != "content" {
            return Err(PluginWasmError::Output(format!(
                "guest output contains unsupported key `{key}`"
            )));
        }
    }
    let summary = match map.get("summary") {
        Some(Value::String(summary)) if !summary.is_empty() => summary.clone(),
        Some(Value::String(_)) => {
            return Err(PluginWasmError::Output(
                "guest output summary must not be empty".into(),
            ));
        }
        Some(_) => {
            return Err(PluginWasmError::Output(
                "guest output summary must be a string".into(),
            ));
        }
        None => {
            return Err(PluginWasmError::Output(
                "guest output must include a summary string".into(),
            ));
        }
    };
    if summary.len() > PLUGIN_WASM_MAX_SUMMARY_BYTES {
        return Err(PluginWasmError::Output(format!(
            "guest output summary exceeds {} bytes",
            PLUGIN_WASM_MAX_SUMMARY_BYTES
        )));
    }
    let content = match map.get("content") {
        Some(Value::String(content)) => {
            if content.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
                return Err(PluginWasmError::Output(format!(
                    "guest output content exceeds {} bytes",
                    PLUGIN_WASM_MAX_OUTPUT_BYTES
                )));
            }
            Some(content.clone())
        }
        Some(Value::Null) | None => None,
        Some(_) => {
            return Err(PluginWasmError::Output(
                "guest output content must be a string or null".into(),
            ));
        }
    };
    Ok(ToolOutput {
        summary,
        content,
        attachments: Vec::new(),
    })
}

fn bounded_message(message: impl Into<String>) -> String {
    let message = message.into();
    let mut sanitized = String::with_capacity(message.len().min(512));
    for ch in message.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            sanitized.push(' ');
        } else {
            sanitized.push(ch);
        }
        if sanitized.len() >= 512 {
            sanitized.truncate(512);
            sanitized.push('…');
            break;
        }
    }
    sanitized
}

fn validate_output_command_id(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(format!(
            "{field} is empty, too long, or contains control characters"
        ));
    }
    Ok(())
}

fn validate_declared_tool_names(record: &ResolvedPluginRecord) -> Result<(), FeatureInstallError> {
    let mut seen = HashSet::new();
    for tool in &record.manifest.tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(FeatureInstallError::DuplicateToolName {
                tool: tool.name.clone(),
                first_feature: format!("{} (same plugin package)", record.identity),
                duplicate_feature: record.identity.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name must not be empty");
    }
    if name.len() > 128 {
        return Err("name is longer than 128 bytes");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("name must not contain whitespace or control characters");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupportedSchemaType {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

impl SupportedSchemaType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "boolean" => Some(Self::Boolean),
            "null" => Some(Self::Null),
            _ => None,
        }
    }
}

fn validate_input_schema(schema: &Value) -> Result<(), String> {
    let ty = validate_schema_node(schema, "$", true)?;
    if ty != SupportedSchemaType::Object {
        return Err("root schema type must be `object`".into());
    }
    Ok(())
}

fn validate_schema_node(
    schema: &Value,
    path: &str,
    root: bool,
) -> Result<SupportedSchemaType, String> {
    let Value::Object(map) = schema else {
        return Err(format!("{path}: schema node must be a JSON object"));
    };

    for key in map.keys() {
        if !is_supported_schema_keyword(key) {
            return Err(format!("{path}: unsupported schema keyword `{key}`"));
        }
    }

    let ty = match map.get("type") {
        Some(Value::String(value)) => SupportedSchemaType::parse(value)
            .ok_or_else(|| format!("{path}: unsupported schema type `{value}`"))?,
        Some(_) => return Err(format!("{path}: type must be a string")),
        None if root => return Err("root schema must declare type = `object`".into()),
        None => return Err(format!("{path}: schema node must declare type")),
    };

    if let Some(title) = map.get("title") {
        if !title.is_string() {
            return Err(format!("{path}: title must be a string"));
        }
    }
    if let Some(description) = map.get("description") {
        if !description.is_string() {
            return Err(format!("{path}: description must be a string"));
        }
    }

    let properties = map.get("properties");
    if let Some(properties) = properties {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: properties is only supported for object schemas"
            ));
        }
        let Some(properties) = properties.as_object() else {
            return Err(format!("{path}: properties must be a JSON object"));
        };
        for (name, child_schema) in properties {
            validate_schema_node(child_schema, &format!("{path}.properties.{name}"), false)?;
        }
    }

    if let Some(required) = map.get("required") {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: required is only supported for object schemas"
            ));
        }
        let Some(required) = required.as_array() else {
            return Err(format!("{path}: required must be an array"));
        };
        let mut seen = HashSet::new();
        for entry in required {
            let Some(name) = entry.as_str() else {
                return Err(format!("{path}: required entries must be strings"));
            };
            if !seen.insert(name) {
                return Err(format!("{path}: required entries must be unique"));
            }
            if let Some(properties) = properties.and_then(Value::as_object) {
                if !properties.contains_key(name) {
                    return Err(format!(
                        "{path}: required entry `{name}` is not declared in properties"
                    ));
                }
            }
        }
    }

    if let Some(additional) = map.get("additionalProperties") {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: additionalProperties is only supported for object schemas"
            ));
        }
        match additional {
            Value::Bool(_) => {}
            Value::Object(_) => {
                validate_schema_node(additional, &format!("{path}.additionalProperties"), false)?;
            }
            _ => {
                return Err(format!(
                    "{path}: additionalProperties must be boolean or schema object"
                ));
            }
        }
    }

    if let Some(items) = map.get("items") {
        if ty != SupportedSchemaType::Array {
            return Err(format!("{path}: items is only supported for array schemas"));
        }
        validate_schema_node(items, &format!("{path}.items"), false)?;
    }

    if let Some(enum_values) = map.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(format!("{path}: enum must be an array"));
        };
        if enum_values.is_empty() {
            return Err(format!("{path}: enum must not be empty"));
        }
        for (index, value) in enum_values.iter().enumerate() {
            if enum_values
                .iter()
                .skip(index + 1)
                .any(|other| other == value)
            {
                return Err(format!("{path}: enum entries must be unique"));
            }
        }
    }

    Ok(ty)
}

fn is_supported_schema_keyword(key: &str) -> bool {
    matches!(
        key,
        "type"
            | "title"
            | "description"
            | "properties"
            | "required"
            | "additionalProperties"
            | "items"
            | "enum"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::plugin::{
        PluginDiscoveryOptions, PluginEnablementConfig, PluginExactVersion, PluginGrantConfig,
        PluginPackageManifest, PluginRequestGrant, PluginRuntimeManifest, SourceQualifiedPluginId,
        resolve_plugin_config_for_startup,
    };
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn websocket_runtime_drop_is_safe_inside_async_context() {
        let runtime = new_websocket_runtime().unwrap();
        drop(runtime);
    }

    fn tool(name: &str) -> manifest::plugin::PluginToolManifest {
        manifest::plugin::PluginToolManifest {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            external_write: false,
        }
    }

    fn record(tools: Vec<manifest::plugin::PluginToolManifest>) -> ResolvedPluginRecord {
        record_with_identity("project:example", tools)
    }

    fn record_with_identity(
        identity: &str,
        tools: Vec<manifest::plugin::PluginToolManifest>,
    ) -> ResolvedPluginRecord {
        let parsed_identity = SourceQualifiedPluginId::parse(identity).unwrap();
        let permissions = tool_permissions(&tools);
        ResolvedPluginRecord {
            identity: parsed_identity.clone(),
            source: parsed_identity.source,
            package_path: std::path::PathBuf::from("/tmp/example.zip"),
            package_label: "example.zip".into(),
            digest: "sha256:abc".into(),
            version: "0.1.0".into(),
            manifest: PluginPackageManifest {
                schema_version: 1,
                id: "example".into(),
                name: "Example".into(),
                version: "0.1.0".into(),
                description: None,
                surfaces: vec![PluginSurface::Tool],
                runtime: Some(manifest::plugin::PluginRuntimeManifest {
                    kind: "test-ingress".to_string(),
                    entry: None,
                    abi: None,
                    component: None,
                    world: None,
                }),
                hooks: Vec::new(),
                tools,
                services: Vec::new(),
                ingresses: Vec::new(),
                permissions: permissions.clone(),
                request: Vec::new(),
                websocket: Vec::new(),
            },
            enabled_surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some(parsed_identity.to_string()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                digest: Some("sha256:abc".to_string()),
                permissions,
                request: Vec::new(),
                websocket: Vec::new(),
                fs: Vec::new(),
            },
            config: None,
        }
    }

    fn tool_permissions(tools: &[manifest::plugin::PluginToolManifest]) -> Vec<PluginPermission> {
        let mut permissions = vec![PluginPermission::surface(PluginSurface::Tool)];
        permissions.extend(
            tools
                .iter()
                .map(|tool| PluginPermission::tool(tool.name.clone())),
        );
        permissions
    }

    fn install_feature(
        feature: PluginToolFeature,
    ) -> (
        super::super::FeatureRegistryInstallReport,
        Vec<ToolDefinition>,
    ) {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(feature)
            .install_into_pending(&mut pending, &mut hooks);
        (report, pending)
    }

    #[test]
    fn component_lifecycle_rejects_start_error_status() {
        let component = component_instance_with_outputs(
            br#"{"error":{"message":"boom"}}"#,
            br#"{"state":"ready"}"#,
            br#"{"state":"stopped"}"#,
            br#"{"summary":"tool"}"#,
            br#"{"accepted":true}"#,
        );
        let (_dir, mut record) = resolved_record_with_component(component);
        record.manifest.runtime.as_mut().unwrap().world =
            Some(PLUGIN_COMPONENT_INSTANCE_WORLD.into());
        let error = match PluginInstanceHandle::new(record) {
            Ok(_) => panic!("component start error should fail instance creation"),
            Err(error) => error,
        };
        assert!(error.bounded_message().contains("start returned error"));
    }

    #[test]
    fn component_lifecycle_reports_status_and_stop_outputs() {
        let component = component_instance_with_outputs(
            br#"{"state":"ready","data":{"phase":"start"}}"#,
            br#"{"state":"ready","data":{"phase":"status"}}"#,
            br#"{"state":"stopped","data":{"phase":"stop"}}"#,
            br#"{"summary":"tool"}"#,
            br#"{"accepted":true}"#,
        );
        let (_dir, mut record) = resolved_record_with_component(component);
        record.manifest.runtime.as_mut().unwrap().world =
            Some(PLUGIN_COMPONENT_INSTANCE_WORLD.into());
        let handle = PluginInstanceHandle::new(record).unwrap();
        let status = handle.status();
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Running);
        assert_eq!(status.component_status.unwrap()["data"]["phase"], "status");
        let stopped = handle.stop().unwrap();
        assert_eq!(stopped.lifecycle, PluginInstanceLifecycleState::Stopped);
        assert_eq!(stopped.component_status.unwrap()["data"]["phase"], "stop");
    }

    fn add_service(record: &mut ResolvedPluginRecord, name: &str) {
        record.manifest.surfaces.push(PluginSurface::Service);
        record.enabled_surfaces.push(PluginSurface::Service);
        record
            .manifest
            .services
            .push(manifest::plugin::PluginServiceManifest {
                name: name.into(),
                description: "test service".into(),
                lifecycle: "host-managed".into(),
                status_schema: None,
                side_effects: Vec::new(),
            });
        record
            .manifest
            .permissions
            .push(PluginPermission::surface(PluginSurface::Service));
        record
            .manifest
            .permissions
            .push(PluginPermission::service(name));
        record
            .grants
            .permissions
            .push(PluginPermission::surface(PluginSurface::Service));
        record
            .grants
            .permissions
            .push(PluginPermission::service(name));
    }

    fn add_ingress(record: &mut ResolvedPluginRecord, name: &str) {
        record.manifest.surfaces.push(PluginSurface::Ingress);
        record.enabled_surfaces.push(PluginSurface::Ingress);
        record
            .manifest
            .ingresses
            .push(manifest::plugin::PluginIngressManifest {
                name: name.into(),
                description: "test ingress".into(),
                event_kinds: vec!["test".into()],
                input_schema: None,
                sources: Vec::new(),
                side_effects: Vec::new(),
            });
        record
            .manifest
            .permissions
            .push(PluginPermission::surface(PluginSurface::Ingress));
        record
            .manifest
            .permissions
            .push(PluginPermission::ingress(name));
        record
            .grants
            .permissions
            .push(PluginPermission::surface(PluginSurface::Ingress));
        record
            .grants
            .permissions
            .push(PluginPermission::ingress(name));
    }

    fn test_ingress_event(ingress_name: &str, payload: Value) -> PluginIngressEvent {
        PluginIngressEvent::new(ingress_name, "test", "unit", payload)
    }

    fn service_output_command(
        event: &PluginIngressEvent,
        command_id: &str,
        kind: &str,
        payload: Value,
    ) -> Value {
        json!({
            "correlation_id": event.correlation_id.clone(),
            "source_event_id": event.correlation_id.clone(),
            "command_id": command_id,
            "kind": kind,
            "payload": payload,
            "requested_at": event.created_at.clone(),
        })
    }

    fn add_request_output_grant(record: &mut ResolvedPluginRecord) {
        let permission = PluginPermission::host_api(PluginHostApi::Request);
        record.manifest.permissions.push(permission.clone());
        record.grants.permissions.push(permission);
        let target = PluginRequestGrant {
            scheme: "https".to_string(),
            host: "api.example.test".to_string(),
            port: None,
            methods: vec!["POST".to_string()],
            path_prefixes: vec!["/v1".to_string()],
        };
        record.manifest.request.push(target.clone());
        record.grants.request.push(target);
    }

    fn add_websocket_output_grant(record: &mut ResolvedPluginRecord) {
        let permission = PluginPermission::host_api(PluginHostApi::WebSocket);
        record.manifest.permissions.push(permission.clone());
        record.grants.permissions.push(permission);
        let target = PluginWebSocketGrant {
            scheme: "wss".to_string(),
            host: "ws.example.test".to_string(),
            port: None,
            path_prefixes: vec!["/events".to_string()],
        };
        record.manifest.websocket.push(target.clone());
        record.grants.websocket.push(target);
    }

    fn add_websocket_ingress_source(record: &mut ResolvedPluginRecord, source: &str) {
        let ingress = record
            .manifest
            .ingresses
            .iter_mut()
            .find(|ingress| ingress.name == "shared_ingress")
            .expect("shared ingress");
        ingress.event_kinds = vec![
            "test".into(),
            "websocket_text".into(),
            "websocket_close".into(),
            "websocket_error".into(),
        ];
        ingress.sources.push(source.to_string());
    }

    fn service_websocket_record() -> ResolvedPluginRecord {
        let mut record = test_service_ingress_record();
        add_websocket_output_grant(&mut record);
        add_websocket_ingress_source(&mut record, "websocket:wss://ws.example.test/events");
        record
    }

    #[derive(Clone, Default)]
    struct ServiceWebSocketClient {
        events: Arc<Mutex<VecDeque<Result<PluginWebSocketRecvResponse, String>>>>,
        sent: Arc<Mutex<Vec<String>>>,
        opens: Arc<std::sync::atomic::AtomicUsize>,
        send_error: Arc<Mutex<Option<String>>>,
    }

    impl ServiceWebSocketClient {
        fn with_events(events: Vec<Result<PluginWebSocketRecvResponse, String>>) -> Self {
            Self {
                events: Arc::new(Mutex::new(events.into())),
                ..Self::default()
            }
        }

        fn sent(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }

        fn fail_sends_with(&self, message: &str) {
            *self.send_error.lock().unwrap() = Some(message.to_string());
        }
    }

    impl PluginWebSocketClient for ServiceWebSocketClient {
        fn supports_bounded_open(&self) -> bool {
            true
        }

        fn open(
            &self,
            _request: &PluginWebSocketOpenRequest,
            _url: &reqwest::Url,
            _limits: PluginWebSocketLimits,
        ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError> {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(ServiceWebSocketConnection {
                events: self.events.clone(),
                sent: self.sent.clone(),
                send_error: self.send_error.clone(),
                closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }))
        }
    }

    struct ServiceWebSocketConnection {
        events: Arc<Mutex<VecDeque<Result<PluginWebSocketRecvResponse, String>>>>,
        sent: Arc<Mutex<Vec<String>>>,
        send_error: Arc<Mutex<Option<String>>>,
        closed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl PluginWebSocketConnection for ServiceWebSocketConnection {
        fn send_text(&mut self, text: &str) -> Result<(), PluginWebSocketError> {
            if let Some(error) = self.send_error.lock().unwrap().clone() {
                return Err(PluginWebSocketError::new(error));
            }
            self.sent.lock().unwrap().push(text.to_string());
            Ok(())
        }

        fn recv_text(
            &mut self,
            timeout: Duration,
            _max_message_bytes: usize,
        ) -> Result<PluginWebSocketRecvResponse, PluginWebSocketError> {
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(PluginWebSocketRecvResponse::Closed);
            }
            if let Some(event) = self.events.lock().unwrap().pop_front() {
                return event.map_err(PluginWebSocketError::new);
            }
            std::thread::sleep(timeout.min(Duration::from_millis(10)));
            Err(PluginWebSocketError::new("receive timed out"))
        }

        fn close(&mut self) -> Result<(), PluginWebSocketError> {
            self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(condition(), "condition did not become true before timeout");
    }

    #[test]
    fn service_selected_ignores_unselected_tool_without_grants() {
        let mut record = record(vec![tool("hidden_tool")]);
        add_service(&mut record, "svc");
        record.enabled_surfaces = vec![PluginSurface::Service];
        record.manifest.permissions = vec![
            PluginPermission::surface(PluginSurface::Service),
            PluginPermission::service("svc"),
        ];
        record.grants.permissions = record.manifest.permissions.clone();
        let feature = PluginToolFeature::new(record);
        assert!(feature.descriptor().tools.is_empty());
        assert_eq!(feature.descriptor().provides_services.len(), 1);
        let (report, pending) = install_feature(feature.clone());
        assert!(
            report.reports.iter().all(|report| report.installed),
            "{report:#?}"
        );
        assert!(pending.is_empty(), "unselected Tool must not register");
        assert_eq!(report.reports[0].provided_services.len(), 1);
        assert_eq!(
            feature.instance_status().unwrap().lifecycle,
            PluginInstanceLifecycleState::Running
        );
    }

    #[test]
    fn tool_selected_ignores_unselected_service_ingress_even_with_grants() {
        let mut record = record(vec![tool("visible_tool")]);
        add_service(&mut record, "hidden_service");
        add_ingress(&mut record, "hidden_ingress");
        record.enabled_surfaces = vec![PluginSurface::Tool];
        let feature = PluginToolFeature::new(record);
        assert!(feature.descriptor().provides_services.is_empty());
        assert_eq!(feature.descriptor().tools.len(), 1);
        let (report, pending) = install_feature(feature.clone());
        assert!(
            report.reports.iter().all(|report| report.installed),
            "{report:#?}"
        );
        assert_eq!(pending.len(), 1);
        assert!(report.reports[0].provided_services.is_empty());
        let dispatch = feature.dispatch_ingress(
            "hidden_ingress",
            test_ingress_event("hidden_ingress", serde_json::json!({})),
        );
        assert!(
            dispatch
                .unwrap_err()
                .bounded_message()
                .contains("not enabled")
        );
    }

    #[test]
    fn service_only_install_retains_host_managed_instance() {
        let mut record = record(Vec::new());
        add_service(&mut record, "svc");
        record.manifest.runtime = Some(manifest::plugin::PluginRuntimeManifest {
            kind: "test-ingress".into(),
            entry: None,
            abi: None,
            component: None,
            world: Some(PLUGIN_COMPONENT_INSTANCE_WORLD.into()),
        });
        let feature = PluginToolFeature::new(record);
        let (report, _pending) = install_feature(feature.clone());
        assert!(
            report.reports.iter().all(|report| report.installed),
            "{report:#?}"
        );
        let status = feature.instance_status().expect("service instance started");
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Running);
        assert_eq!(status.queue_depth, 0);
        assert_eq!(status.queue_capacity, PLUGIN_SERVICE_INGRESS_QUEUE_CAPACITY);
        assert!(status.last_error.is_none());
    }

    fn test_service_ingress_record() -> ResolvedPluginRecord {
        let mut record = record(Vec::new());
        add_service(&mut record, "svc");
        add_ingress(&mut record, "shared_ingress");
        record.manifest.runtime = Some(manifest::plugin::PluginRuntimeManifest {
            kind: "test-ingress".into(),
            entry: None,
            abi: None,
            component: None,
            world: Some(PLUGIN_COMPONENT_INSTANCE_WORLD.into()),
        });
        record
    }

    #[test]
    fn ingress_queue_dispatches_serially_and_reports_status() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        assert_eq!(
            handle.status().lifecycle,
            PluginInstanceLifecycleState::Running
        );

        let first = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 1 })),
            )
            .unwrap();
        let second = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 2 })),
            )
            .unwrap();

        assert_eq!(first.output["ingress_calls"], 1);
        assert_eq!(second.output["ingress_calls"], 2);
        assert_eq!(second.queue_depth, 0);
        assert_eq!(second.dispatch_counters.enqueued, 2);
        assert_eq!(second.dispatch_counters.dispatched, 2);
        let status = handle.status();
        assert_eq!(status.queue_depth, 0);
        assert_eq!(status.dispatch_counters.enqueued, 2);
        assert_eq!(status.dispatch_counters.dispatched, 2);
        assert!(status.last_error.is_none());
    }

    #[test]
    fn bounded_ingress_queue_rejects_full_queue() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let mut instance = handle.0.lock().unwrap();
        instance.ingress_queue_capacity = 1;
        instance
            .enqueue_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 1 })),
            )
            .unwrap();
        let error = instance
            .enqueue_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 2 })),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PluginIngressDispatchError::QueueFull { capacity: 1 }
        ));
        assert_eq!(instance.ingress_queue.len(), 1);
        assert_eq!(instance.dispatch_counters.rejected, 1);
        assert_eq!(
            instance.diagnostics.last().unwrap().kind,
            PluginInstanceDiagnosticKind::QueueFull
        );
    }

    #[test]
    fn ingress_dispatch_failure_marks_service_failed_and_rejects_later_events() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let error = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "fail": true })),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PluginIngressDispatchError::DispatchFailed(_)
        ));
        let status = handle.status();
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Failed);
        assert_eq!(status.dispatch_counters.failed, 1);
        assert_eq!(
            status.diagnostics.last().unwrap().kind,
            PluginInstanceDiagnosticKind::DispatchFailed
        );

        let retry = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 3 })),
            )
            .unwrap_err();
        assert!(matches!(
            retry,
            PluginIngressDispatchError::ServiceFailed(_)
        ));
    }

    #[test]
    fn ingress_dispatch_timeout_records_typed_diagnostic() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let error = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "sleep_ms": 50 })),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PluginIngressDispatchError::DispatchTimeout { .. }
        ));
        let status = handle.status();
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Failed);
        assert_eq!(status.dispatch_counters.timed_out, 1);
        assert_eq!(
            status.diagnostics.last().unwrap().kind,
            PluginInstanceDiagnosticKind::DispatchTimeout
        );
    }

    #[test]
    fn stopped_service_rejects_ingress_events() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        handle.stop().unwrap();
        let error = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "seq": 1 })),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PluginIngressDispatchError::ServiceStopped {
                state: PluginInstanceLifecycleState::Stopped
            }
        ));
    }

    #[test]
    fn invalid_ingress_event_is_typed_rejection() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let mut event = test_ingress_event("shared_ingress", serde_json::json!({}));
        event.correlation_id.clear();
        let error = handle.deliver_ingress("shared_ingress", event).unwrap_err();
        assert!(matches!(error, PluginIngressDispatchError::InvalidEvent(_)));
        let status = handle.status();
        assert_eq!(status.dispatch_counters.rejected, 1);
        assert_eq!(
            status.diagnostics.last().unwrap().kind,
            PluginInstanceDiagnosticKind::InvalidEvent
        );
    }

    #[test]
    fn service_output_command_records_diagnostic_status_separately_from_tool_output() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let mut event = test_ingress_event("shared_ingress", json!({}));
        let command = service_output_command(
            &event,
            "cmd-status",
            "diagnostic_status_update",
            json!({
                "message": "service became ready",
                "status": {"ready": true}
            }),
        );
        event.payload = json!({"output_commands": [command]});

        let report = handle.deliver_ingress("shared_ingress", event).unwrap();

        assert_eq!(report.output_command_results.len(), 1);
        assert_eq!(
            report.output_command_results[0].kind,
            Some(PluginServiceOutputCommandKind::DiagnosticStatusUpdate)
        );
        assert_eq!(
            report.output_command_results[0].status,
            PluginServiceOutputCommandStatus::Recorded
        );
        assert_eq!(
            report.output["payload"]["output_commands"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let status = handle.status();
        assert_eq!(status.component_status, Some(json!({"ready": true})));
        assert_eq!(status.output_command_results.len(), 1);
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceOutputCommandRecorded
                && diagnostic.message.contains("cmd-status")
        }));
    }

    #[test]
    fn service_output_command_rejects_ungranted_request_without_executing_status_update() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let mut event = test_ingress_event("shared_ingress", json!({}));
        let status_command = service_output_command(
            &event,
            "cmd-status",
            "diagnostic_status_update",
            json!({"status": {"should_not_record": true}}),
        );
        let request_command = service_output_command(
            &event,
            "cmd-request",
            "host_request_dispatch",
            json!({
                "method": "POST",
                "url": "https://api.example.test/v1/events"
            }),
        );
        event.payload = json!({"output_commands": [status_command, request_command]});

        let report = handle.deliver_ingress("shared_ingress", event).unwrap();

        assert_eq!(report.output_command_results.len(), 1);
        assert_eq!(
            report.output_command_results[0].status,
            PluginServiceOutputCommandStatus::Rejected
        );
        assert_eq!(handle.status().component_status, None);
        assert!(handle.status().diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceOutputCommandRejected
                && diagnostic.message.contains("cmd-request")
        }));
    }

    #[test]
    fn service_output_commands_are_grant_checked_and_supported_transport_rejects_without_connection()
     {
        let mut record = test_service_ingress_record();
        add_request_output_grant(&mut record);
        add_websocket_output_grant(&mut record);
        let handle = PluginInstanceHandle::new(record).unwrap();
        let mut event = test_ingress_event("shared_ingress", json!({}));
        let request_command = service_output_command(
            &event,
            "cmd-request",
            "host_request_dispatch",
            json!({
                "method": "POST",
                "url": "https://api.example.test/v1/events"
            }),
        );
        let websocket_command = service_output_command(
            &event,
            "cmd-websocket",
            "websocket_send",
            json!({
                "url": "wss://ws.example.test/events",
                "text": "hello"
            }),
        );
        event.payload = json!({"output_commands": [request_command, websocket_command]});

        let report = handle.deliver_ingress("shared_ingress", event).unwrap();

        assert_eq!(report.output_command_results.len(), 2);
        let request_result = report
            .output_command_results
            .iter()
            .find(|result| result.command_id.as_deref() == Some("cmd-request"))
            .unwrap();
        let websocket_result = report
            .output_command_results
            .iter()
            .find(|result| result.command_id.as_deref() == Some("cmd-websocket"))
            .unwrap();
        assert_eq!(
            request_result.status,
            PluginServiceOutputCommandStatus::Unsupported
        );
        assert_eq!(
            websocket_result.status,
            PluginServiceOutputCommandStatus::Rejected
        );
        let status = handle.status();
        assert_eq!(status.output_command_results.len(), 2);
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceOutputCommandUnsupported
                && diagnostic.message.contains("cmd-request")
        }));
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceOutputCommandRejected
                && diagnostic.message.contains("cmd-websocket")
        }));
    }

    #[test]
    fn service_websocket_driver_enqueues_incoming_text_and_reports_close() {
        let client = ServiceWebSocketClient::with_events(vec![
            Ok(PluginWebSocketRecvResponse::Text {
                text: "hello service".into(),
            }),
            Ok(PluginWebSocketRecvResponse::Closed),
        ]);
        let handle = PluginInstanceHandle::new_with_test_websocket_client(
            service_websocket_record(),
            Arc::new(client.clone()),
        )
        .unwrap();

        wait_until(|| handle.status().dispatch_counters.dispatched >= 1);
        let status = handle.status();
        assert!(status.dispatch_counters.dispatched >= 1);
        assert_eq!(status.websocket_connections.len(), 1);
        let connection = &status.websocket_connections[0];
        assert_eq!(connection.received_text_frames, 1);
        assert!(connection.last_frame_at.is_some());
        assert_eq!(connection.queue_drops, 0);
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceWebSocketClosed
        }));
        handle.stop().unwrap();
    }

    #[test]
    fn websocket_send_output_command_sends_on_host_owned_connection() {
        let client = ServiceWebSocketClient::default();
        let handle = PluginInstanceHandle::new_with_test_websocket_client(
            service_websocket_record(),
            Arc::new(client.clone()),
        )
        .unwrap();
        wait_until(|| {
            handle
                .status()
                .websocket_connections
                .iter()
                .any(|connection| {
                    connection.state == PluginServiceWebSocketConnectionState::Connected
                })
        });
        let event = test_ingress_event("shared_ingress", json!({}));
        let command_value = service_output_command(
            &event,
            "cmd-websocket",
            "websocket_send",
            json!({"url": "wss://ws.example.test/events", "text": "pong"}),
        );
        let command: PluginServiceOutputCommandEnvelope =
            serde_json::from_value(command_value).unwrap();

        let result = handle
            .0
            .lock()
            .unwrap()
            .execute_service_output_command(command);

        assert_eq!(result.status, PluginServiceOutputCommandStatus::Recorded);
        assert_eq!(client.sent(), vec!["pong".to_string()]);
        let status = handle.status();
        assert_eq!(status.websocket_connections[0].sent_text_frames, 1);
        handle.stop().unwrap();
    }

    #[test]
    fn websocket_send_output_command_records_send_failure_diagnostic() {
        let client = ServiceWebSocketClient::default();
        client.fail_sends_with("transport write failed");
        let handle = PluginInstanceHandle::new_with_test_websocket_client(
            service_websocket_record(),
            Arc::new(client),
        )
        .unwrap();
        wait_until(|| {
            handle
                .status()
                .websocket_connections
                .iter()
                .any(|connection| {
                    connection.state == PluginServiceWebSocketConnectionState::Connected
                })
        });
        let event = test_ingress_event("shared_ingress", json!({}));
        let command: PluginServiceOutputCommandEnvelope =
            serde_json::from_value(service_output_command(
                &event,
                "cmd-websocket-fail",
                "websocket_send",
                json!({"url": "wss://ws.example.test/events", "text": "pong"}),
            ))
            .unwrap();

        let result = handle
            .0
            .lock()
            .unwrap()
            .execute_service_output_command(command);

        assert_eq!(result.status, PluginServiceOutputCommandStatus::Rejected);
        let status = handle.status();
        assert_eq!(status.websocket_connections[0].send_failures, 1);
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceWebSocketSendFailed
                && diagnostic.message.contains("transport write failed")
        }));
        handle.stop().unwrap();
    }

    #[test]
    fn websocket_send_rejects_unauthorized_target_before_execution() {
        let mut record = service_websocket_record();
        let handle = PluginInstanceHandle::new_with_test_websocket_client(
            record.clone(),
            Arc::new(ServiceWebSocketClient::default()),
        )
        .unwrap();
        let event = test_ingress_event("shared_ingress", json!({}));
        let command: PluginServiceOutputCommandEnvelope =
            serde_json::from_value(service_output_command(
                &event,
                "cmd-websocket-denied",
                "websocket_send",
                json!({"url": "wss://ws.example.test/private", "text": "nope"}),
            ))
            .unwrap();

        let error = handle
            .0
            .lock()
            .unwrap()
            .validate_service_output_command_envelope(&command, &event)
            .unwrap_err();

        assert!(
            error.contains("websocket_send target denied")
                || error.contains("not declared by the plugin manifest"),
            "{error}"
        );
        assert_eq!(handle.status().websocket_connections[0].sent_text_frames, 0);
        record.grants.permissions.retain(|permission| {
            *permission != PluginPermission::host_api(PluginHostApi::WebSocket)
        });
        let denied_handle = PluginInstanceHandle::new_with_test_websocket_client(
            record,
            Arc::new(ServiceWebSocketClient::default()),
        )
        .unwrap();
        wait_until(|| !denied_handle.status().diagnostics.is_empty());
        assert!(denied_handle.status().diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceWebSocketError
                && diagnostic.message.contains("not granted")
        }));
        handle.stop().unwrap();
        denied_handle.stop().unwrap();
    }

    #[test]
    fn service_websocket_driver_reports_receive_error_as_diagnostic() {
        let client = ServiceWebSocketClient::with_events(vec![Err(
            "binary frames are not supported by host-owned service websocket".into(),
        )]);
        let handle = PluginInstanceHandle::new_with_test_websocket_client(
            service_websocket_record(),
            Arc::new(client),
        )
        .unwrap();

        wait_until(|| {
            handle.status().websocket_connections[0].state
                == PluginServiceWebSocketConnectionState::Failed
        });
        let status = handle.status();
        assert!(status.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PluginInstanceDiagnosticKind::ServiceWebSocketError
                && diagnostic.message.contains("binary frames")
        }));
        assert!(
            status.websocket_connections[0]
                .last_error
                .as_deref()
                .unwrap()
                .contains("binary frames")
        );
        handle.stop().unwrap();
    }

    #[test]
    fn service_output_command_rejects_malformed_envelope() {
        let handle = PluginInstanceHandle::new(test_service_ingress_record()).unwrap();
        let mut event = test_ingress_event("shared_ingress", json!({}));
        event.payload = json!({"output_commands": [{"kind": "diagnostic_status_update"}]});

        let report = handle.deliver_ingress("shared_ingress", event).unwrap();

        assert_eq!(report.output_command_results.len(), 1);
        assert_eq!(
            report.output_command_results[0].status,
            PluginServiceOutputCommandStatus::Rejected
        );
        assert!(
            report.output_command_results[0]
                .message
                .contains("malformed service output command envelope")
        );
    }

    #[test]
    fn installed_ingress_dispatch_uses_retained_shared_instance() {
        let mut record = record(vec![tool("shared_tool")]);
        add_ingress(&mut record, "shared_ingress");
        record.manifest.runtime = Some(manifest::plugin::PluginRuntimeManifest {
            kind: "test-ingress".into(),
            entry: None,
            abi: None,
            component: None,
            world: Some(PLUGIN_COMPONENT_INSTANCE_WORLD.into()),
        });
        let feature = PluginToolFeature::new(record);
        let (report, pending) = install_feature(feature.clone());
        assert!(
            report.reports.iter().all(|report| report.installed),
            "{report:#?}"
        );
        let (_meta, tool) = pending
            .into_iter()
            .map(|definition| definition())
            .find(|(meta, _tool)| meta.name == "shared_tool")
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let output = runtime
            .block_on(tool.execute(r#"{"first":true}"#, ToolExecutionContext::default()))
            .unwrap();
        assert!(output.summary.contains("shared_tool"));
        let report = feature
            .dispatch_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "hello": "world" })),
            )
            .unwrap();
        assert!(report.accepted);
        assert_eq!(report.output["calls"], 1);
    }

    #[test]
    fn instance_ingress_dispatch_uses_shared_in_process_instance() {
        let mut record = record(vec![tool("shared_tool")]);
        record.manifest.surfaces.push(PluginSurface::Ingress);
        record.enabled_surfaces.push(PluginSurface::Ingress);
        record
            .manifest
            .ingresses
            .push(manifest::plugin::PluginIngressManifest {
                name: "shared_ingress".into(),
                description: "test ingress".into(),
                event_kinds: vec!["test".into()],
                input_schema: None,
                sources: Vec::new(),
                side_effects: Vec::new(),
            });
        record
            .manifest
            .permissions
            .push(PluginPermission::surface(PluginSurface::Ingress));
        record
            .manifest
            .permissions
            .push(PluginPermission::ingress("shared_ingress"));
        record
            .grants
            .permissions
            .push(PluginPermission::surface(PluginSurface::Ingress));
        record
            .grants
            .permissions
            .push(PluginPermission::ingress("shared_ingress"));
        let handle = PluginInstanceHandle(Arc::new(Mutex::new(PluginInstance {
            record,
            runtime: PluginInstanceRuntime::TestIngress {
                tool_calls: 0,
                ingress_calls: 0,
            },
            lifecycle: PluginInstanceLifecycleState::Running,
            component_status: None,
            ingress_queue: VecDeque::new(),
            ingress_queue_capacity: PLUGIN_SERVICE_INGRESS_QUEUE_CAPACITY,
            dispatch_counters: PluginIngressDispatchCounters::default(),
            last_error: None,
            output_command_results: Vec::new(),
            service_websockets: PluginServiceWebSocketDriver::default(),
            service_websocket_client: Arc::new(TungstenitePluginWebSocketClient),
            diagnostics: Vec::new(),
        })));

        let _tool = handle
            .handle_tool("shared_tool", br#"{"first":true}"#.to_vec())
            .unwrap();
        let report = handle
            .deliver_ingress(
                "shared_ingress",
                test_ingress_event("shared_ingress", serde_json::json!({ "hello": "world" })),
            )
            .unwrap();
        assert!(report.accepted);
        assert_eq!(report.output["calls"], 1);
        assert_eq!(report.output["ingress"], "shared_ingress");
    }

    fn skipped_count(report: &super::super::FeatureRegistryInstallReport) -> usize {
        report
            .reports
            .iter()
            .map(|feature_report| feature_report.skipped.len())
            .sum()
    }

    fn has_diagnostic(report: &super::super::FeatureRegistryInstallReport, needle: &str) -> bool {
        report.reports.iter().any(|feature_report| {
            feature_report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(needle))
        })
    }

    fn install_plugin_record(
        record: ResolvedPluginRecord,
    ) -> (
        super::super::FeatureRegistryInstallReport,
        Vec<ToolDefinition>,
    ) {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record))
            .install_into_pending(&mut pending, &mut hooks);
        (report, pending)
    }

    fn record_with_request_grant() -> ResolvedPluginRecord {
        let mut record = record(vec![tool("RequestTool")]);
        let request_permission = PluginPermission::HostApi {
            api: PluginHostApi::Request,
        };
        record.manifest.permissions.push(request_permission.clone());
        record.manifest.request.push(PluginRequestGrant {
            scheme: "https".to_string(),
            host: "api.example.test".to_string(),
            port: None,
            methods: vec!["GET".to_string(), "POST".to_string()],
            path_prefixes: vec!["/v1".to_string()],
        });
        record.grants.permissions.push(request_permission);
        record.grants.request.push(PluginRequestGrant {
            scheme: "https".to_string(),
            host: "api.example.test".to_string(),
            port: None,
            methods: vec!["GET".to_string(), "POST".to_string()],
            path_prefixes: vec!["/v1".to_string()],
        });
        record
    }

    struct MockRequestClient {
        calls: Mutex<usize>,
        response_body: String,
        error: Mutex<Option<String>>,
    }

    impl Default for MockRequestClient {
        fn default() -> Self {
            Self {
                calls: Mutex::new(0),
                response_body: "ok".to_string(),
                error: Mutex::new(None),
            }
        }
    }

    impl MockRequestClient {
        fn call_count(&self) -> usize {
            *self.calls.lock().expect("mock call lock")
        }
    }

    impl PluginRequestClient for MockRequestClient {
        fn execute(
            &self,
            _request: &PluginRequestRequest,
            _url: &reqwest::Url,
            _limits: PluginRequestLimits,
        ) -> Result<PluginRequestResponse, PluginRequestError> {
            *self.calls.lock().expect("mock call lock") += 1;
            if let Some(error) = self.error.lock().expect("mock error lock").take() {
                return Err(PluginRequestError::new(error));
            }
            Ok(PluginRequestResponse {
                status: 200,
                headers: vec![PluginRequestHeader {
                    name: "content-type".to_string(),
                    value: "text/plain".to_string(),
                }],
                body: self.response_body.clone(),
                truncated: false,
            })
        }
    }

    struct FakeRequestResolver {
        calls: Mutex<Vec<(String, u16)>>,
        addrs: Vec<SocketAddr>,
    }

    impl FakeRequestResolver {
        fn new(addrs: Vec<SocketAddr>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                addrs,
            }
        }

        fn calls(&self) -> Vec<(String, u16)> {
            self.calls.lock().expect("resolver calls lock").clone()
        }
    }

    impl PluginRequestResolver for FakeRequestResolver {
        fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, PluginRequestError> {
            self.calls
                .lock()
                .expect("resolver calls lock")
                .push((host.to_string(), port));
            Ok(self.addrs.clone())
        }
    }

    fn request_request_json(method: &str, url: &str) -> String {
        json!({ "method": method, "url": url }).to_string()
    }

    fn fs_request_json(path: &str) -> String {
        json!({ "path": path }).to_string()
    }

    fn fs_write_request_json(path: &str, content: &str) -> String {
        json!({ "path": path, "content": content }).to_string()
    }

    fn record_with_fs_grant(
        root: &Path,
        operations: Vec<PluginFsOperation>,
    ) -> ResolvedPluginRecord {
        let mut record = record(vec![tool("FsTool")]);
        let fs_permission = PluginPermission::HostApi {
            api: PluginHostApi::Fs,
        };
        record.manifest.permissions.push(fs_permission.clone());
        record.grants.permissions.push(fs_permission);
        record.grants.fs.push(PluginFsGrant {
            root: root.to_string_lossy().into_owned(),
            operations,
        });
        record
    }

    #[test]
    fn rejects_invalid_root_schema() {
        let schema = json!({"type":"string"});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("type must be `object`")
        );
    }

    #[test]
    fn rejects_unsupported_schema_keyword() {
        let schema = json!({"type":"object","oneOf":[]});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("unsupported schema keyword")
        );
    }

    #[test]
    fn rejects_invalid_nested_property_schema_node() {
        let schema = json!({
            "type":"object",
            "properties":{"query":"not-a-schema"},
            "required":["query"],
            "additionalProperties":false
        });
        let error = validate_input_schema(&schema).unwrap_err();
        assert!(error.contains("$.properties.query"));
        assert!(error.contains("schema node must be a JSON object"));
    }

    #[test]
    fn rejects_invalid_recursive_schema_members() {
        let duplicate_required = json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query", "query"]
        });
        assert!(
            validate_input_schema(&duplicate_required)
                .unwrap_err()
                .contains("required entries must be unique")
        );

        let invalid_items = json!({
            "type":"object",
            "properties":{"values":{"type":"array", "items":"not-a-schema"}}
        });
        assert!(
            validate_input_schema(&invalid_items)
                .unwrap_err()
                .contains("$.properties.values.items")
        );

        let invalid_additional = json!({
            "type":"object",
            "additionalProperties":{"type":"unsupported"}
        });
        assert!(
            validate_input_schema(&invalid_additional)
                .unwrap_err()
                .contains("unsupported schema type")
        );
    }

    #[test]
    fn accepts_object_tool_schema() {
        validate_input_schema(&json!({
            "type":"object",
            "properties":{
                "query":{"type":"string", "description":"Search text"},
                "limit":{"type":"integer", "enum":[1, 5, 10]},
                "tags":{"type":"array", "items":{"type":"string"}}
            },
            "required":["query"],
            "additionalProperties":{"type":"string"}
        }))
        .unwrap();
    }

    #[test]
    fn granted_fs_read_list_and_write_are_scoped() {
        let root = TempDir::new().expect("temp root");
        fs::write(root.path().join("read.txt"), "hello fs").expect("write fixture");
        fs::create_dir(root.path().join("dir")).expect("create dir");
        fs::write(root.path().join("dir").join("entry.txt"), "entry").expect("write entry");
        let record = record_with_fs_grant(
            root.path(),
            vec![
                PluginFsOperation::Read,
                PluginFsOperation::List,
                PluginFsOperation::Write,
            ],
        );

        let read = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("read.txt").as_bytes(),
        )
        .expect("read allowed");
        let read: serde_json::Value = serde_json::from_slice(&read).expect("read response json");
        assert_eq!(read["content"], "hello fs");
        assert_eq!(read["truncated"], false);

        let list = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::List,
            fs_request_json("dir").as_bytes(),
        )
        .expect("list allowed");
        let list: serde_json::Value = serde_json::from_slice(&list).expect("list response json");
        assert_eq!(list["entries"][0]["name"], "entry.txt");
        assert_eq!(list["entries"][0]["kind"], "file");

        let write = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Write,
            fs_write_request_json("written.txt", "new content").as_bytes(),
        )
        .expect("write allowed");
        let write: serde_json::Value = serde_json::from_slice(&write).expect("write response json");
        assert_eq!(write["bytes_written"], "new content".len());
        assert_eq!(
            fs::read_to_string(root.path().join("written.txt")).expect("read written"),
            "new content"
        );
    }

    #[test]
    fn missing_fs_grant_denies_even_when_manifest_requests_api() {
        let root = TempDir::new().expect("temp root");
        fs::write(root.path().join("secret.txt"), "must not leak").expect("write fixture");
        let mut record = record(vec![tool("FsTool")]);
        record.manifest.permissions.push(PluginPermission::HostApi {
            api: PluginHostApi::Fs,
        });
        let error = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("secret.txt").as_bytes(),
        )
        .expect_err("grant denied");
        assert!(error.message.contains("host_api.fs"));
        assert!(!error.message.contains("must not leak"));
    }

    #[test]
    fn workspace_scope_is_not_inherited_without_plugin_fs_grant() {
        let root = TempDir::new().expect("temp root");
        fs::write(root.path().join("workspace.txt"), "workspace authority").expect("write fixture");
        let record = record(vec![tool("FsTool")]);
        let error = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("workspace.txt").as_bytes(),
        )
        .expect_err("plugin grant required");
        assert!(error.message.contains("host_api.fs"));
        assert!(!error.message.contains("workspace authority"));
    }

    #[test]
    fn fs_traversal_and_absolute_paths_are_rejected() {
        let root = TempDir::new().expect("temp root");
        let record = record_with_fs_grant(root.path(), vec![PluginFsOperation::Read]);
        for path in ["../outside.txt", "/etc/passwd"] {
            let error = execute_plugin_fs_request(
                &record,
                PluginFsRuntimeOperation::Read,
                fs_request_json(path).as_bytes(),
            )
            .expect_err("path denied");
            assert!(
                error.message.contains("relative") || error.message.contains("traversal"),
                "unexpected error: {}",
                error.message
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fs_symlink_escape_is_rejected() {
        let root = TempDir::new().expect("temp root");
        let outside = TempDir::new().expect("outside root");
        fs::write(outside.path().join("outside.txt"), "outside secret").expect("write outside");
        std::os::unix::fs::symlink(outside.path(), root.path().join("link"))
            .expect("create symlink");
        let record = record_with_fs_grant(root.path(), vec![PluginFsOperation::Read]);
        let error = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("link/outside.txt").as_bytes(),
        )
        .expect_err("symlink denied");
        assert!(error.message.contains("symlink"));
        assert!(!error.message.contains("outside secret"));
    }

    #[test]
    fn fs_read_write_and_list_bounds_are_enforced() {
        let root = TempDir::new().expect("temp root");
        fs::write(
            root.path().join("big.txt"),
            vec![b'a'; PLUGIN_FS_MAX_READ_BYTES + 1],
        )
        .expect("write big file");
        let list_dir = root.path().join("many");
        fs::create_dir(&list_dir).expect("create list dir");
        for index in 0..=PLUGIN_FS_MAX_LIST_ENTRIES {
            fs::write(list_dir.join(format!("entry-{index:03}.txt")), "x")
                .expect("write list entry");
        }
        let record = record_with_fs_grant(
            root.path(),
            vec![
                PluginFsOperation::Read,
                PluginFsOperation::List,
                PluginFsOperation::Write,
            ],
        );
        let read = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("big.txt").as_bytes(),
        )
        .expect("bounded read allowed");
        let read: serde_json::Value = serde_json::from_slice(&read).expect("read response json");
        assert_eq!(read["truncated"], true);
        assert_eq!(
            read["content"].as_str().expect("content").len(),
            PLUGIN_FS_MAX_READ_BYTES
        );

        let list = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::List,
            fs_request_json("many").as_bytes(),
        )
        .expect("bounded list allowed");
        let list: serde_json::Value = serde_json::from_slice(&list).expect("list response json");
        assert_eq!(list["truncated"], true);
        assert_eq!(
            list["entries"].as_array().expect("entries").len(),
            PLUGIN_FS_MAX_LIST_ENTRIES
        );

        let too_large = "x".repeat(PLUGIN_FS_MAX_WRITE_BYTES + 1);
        let error = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Write,
            fs_write_request_json("too-large.txt", &too_large).as_bytes(),
        )
        .expect_err("oversize write denied");
        assert!(error.message.contains("exceeds"));
        assert!(!root.path().join("too-large.txt").exists());
    }

    #[test]
    fn fs_diagnostics_redact_secret_like_path_segments() {
        let root = TempDir::new().expect("temp root");
        let record = record_with_fs_grant(root.path(), vec![PluginFsOperation::Read]);
        let error = execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Read,
            fs_request_json("secret=my-token-value.txt").as_bytes(),
        )
        .expect_err("missing file");
        assert!(error.message.contains("secret"));
        assert!(error.message.contains("<redacted>"));
        assert!(!error.message.contains("my-token-value"));
    }

    #[test]
    fn fs_writes_serialize_to_normalized_target() {
        let root = TempDir::new().expect("temp root");
        let record = record_with_fs_grant(root.path(), vec![PluginFsOperation::Write]);
        execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Write,
            fs_write_request_json("target.txt", "first").as_bytes(),
        )
        .expect("first write");
        execute_plugin_fs_request(
            &record,
            PluginFsRuntimeOperation::Write,
            fs_write_request_json("./target.txt", "second").as_bytes(),
        )
        .expect("second write");
        assert_eq!(
            fs::read_to_string(root.path().join("target.txt")).expect("read target"),
            "second"
        );
    }

    #[test]
    fn origin_retains_plugin_metadata() {
        let feature = PluginToolFeature::new(record(Vec::new()));
        let origin = feature.origin();
        assert_eq!(origin.kind, "plugin");
        assert_eq!(origin.plugin_id, "example");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.package_version, "0.1.0");
        assert_eq!(origin.package_api_version, 1);
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn disallowed_request_targets_deny_before_network() {
        let record = record_with_request_grant();
        let client = MockRequestClient::default();
        for (method, url, needle) in [
            ("GET", "http://api.example.test/v1/data", "manifest"),
            ("TRACE", "https://api.example.test/v1/data", "method"),
            ("GET", "https://other.example.test/v1/data", "manifest"),
            ("GET", "http://localhost/v1/data", "manifest"),
            ("GET", "http://127.0.0.1/v1/data", "manifest"),
            ("GET", "file:///tmp/secret", "scheme"),
            ("GET", "ws://api.example.test/v1/data", "WebSocket"),
        ] {
            let error = execute_plugin_request_request(
                &record,
                &client,
                request_request_json(method, url).as_bytes(),
            )
            .expect_err("request denied");
            assert!(
                error.0.contains(needle),
                "{method} {url} produced {:?}, expected {needle}",
                error.0
            );
        }
        assert_eq!(client.call_count(), 0);
    }

    #[test]
    fn loopback_request_requires_explicit_manifest_and_grant() {
        let mut record = record_with_request_grant();
        record.manifest.request.push(PluginRequestGrant {
            scheme: "http".to_string(),
            host: "localhost".to_string(),
            port: Some(8080),
            methods: vec!["GET".to_string()],
            path_prefixes: vec!["/health".to_string()],
        });
        let client = MockRequestClient::default();
        let denied = execute_plugin_request_request(
            &record,
            &client,
            request_request_json("GET", "http://localhost:8080/health").as_bytes(),
        )
        .expect_err("manifest-only local target denied");
        assert!(denied.0.contains("grants"), "{}", denied.0);
        record.grants.request.push(PluginRequestGrant {
            scheme: "http".to_string(),
            host: "localhost".to_string(),
            port: Some(8080),
            methods: vec!["GET".to_string()],
            path_prefixes: vec!["/health".to_string()],
        });
        execute_plugin_request_request(
            &record,
            &client,
            request_request_json("GET", "http://localhost:8080/health").as_bytes(),
        )
        .expect("explicit loopback target allowed");
        assert_eq!(client.call_count(), 1);
    }

    #[test]
    fn upgrade_and_sse_headers_are_rejected() {
        let record = record_with_request_grant();
        let client = MockRequestClient::default();
        for headers in [
            r#"[{"name":"Upgrade","value":"websocket"}]"#,
            r#"[{"name":"Connection","value":"keep-alive, Upgrade"}]"#,
            r#"[{"name":"Accept","value":"text/event-stream"}]"#,
        ] {
            let body = format!(
                r#"{{"method":"GET","url":"https://api.example.test/v1/data","headers":{headers}}}"#
            );
            let error = execute_plugin_request_request(&record, &client, body.as_bytes())
                .expect_err("persistent request header denied");
            assert!(
                error.0.contains("not supported") || error.0.contains("upgrade"),
                "{}",
                error.0
            );
        }
        assert_eq!(client.call_count(), 0);
    }

    #[test]
    fn dns_resolution_is_pinned_to_validated_public_socket_addresses() {
        let url = reqwest::Url::parse("https://api.example.test:8443/v1/data").unwrap();
        let resolver = FakeRequestResolver::new(vec!["93.184.216.34:8443".parse().unwrap()]);
        let pinned = resolve_request_target_for_client(&url, &resolver)
            .expect("resolution")
            .expect("hostname resolution is pinned");
        assert_eq!(
            resolver.calls(),
            vec![("api.example.test".to_string(), 8443)]
        );
        assert_eq!(pinned.domains, vec!["api.example.test".to_string()]);
        assert_eq!(pinned.addrs, vec!["93.184.216.34:8443".parse().unwrap()]);

        let mut builder = reqwest::blocking::Client::builder().no_proxy();
        for domain in &pinned.domains {
            builder = builder.resolve_to_addrs(domain, &pinned.addrs);
        }
        builder.build().expect("client accepts pinned resolver");
    }

    #[test]
    fn dns_resolution_can_pin_explicit_private_addresses() {
        let url = reqwest::Url::parse("https://api.example.test/v1/data").unwrap();
        let resolver = FakeRequestResolver::new(vec!["127.0.0.1:443".parse().unwrap()]);
        let pinned = resolve_request_target_for_client(&url, &resolver)
            .expect("explicit request grants, not ambient network policy, govern local targets")
            .expect("hostname resolution is pinned");
        assert_eq!(pinned.addrs, vec!["127.0.0.1:443".parse().unwrap()]);
    }

    #[test]
    fn timeout_and_secret_diagnostics_are_bounded_and_redacted() {
        let record = record_with_request_grant();
        let client = MockRequestClient::default();
        *client.error.lock().expect("mock error lock") =
            Some("timeout while using Authorization: Bearer SUPER_SECRET_TOKEN".to_string());
        let error = execute_plugin_request_request(
            &record,
            &client,
            request_request_json("GET", "https://api.example.test/v1/data").as_bytes(),
        )
        .expect_err("timeout error");
        assert!(error.0.contains("timeout"));
        assert!(error.0.contains(PLUGIN_REQUEST_REDACTION));
        assert!(!error.0.contains("SUPER_SECRET_TOKEN"));
        assert!(error.0.len() <= 513);
        assert_eq!(client.call_count(), 1);
    }

    #[test]
    fn response_size_bound_truncates() {
        let record = record_with_request_grant();
        let client = MockRequestClient {
            calls: Mutex::new(0),
            response_body: "x".repeat(PLUGIN_REQUEST_MAX_RESPONSE_BYTES + 8),
            error: Mutex::new(None),
        };
        let response = execute_plugin_request_request(
            &record,
            &client,
            request_request_json("GET", "https://api.example.test/v1/data").as_bytes(),
        )
        .expect("response");
        let value: Value = serde_json::from_slice(&response).expect("response json");
        assert_eq!(value["truncated"], true);
        assert_eq!(
            value["body"].as_str().expect("body").len(),
            PLUGIN_REQUEST_MAX_RESPONSE_BYTES
        );
    }

    #[test]
    fn enabled_plugin_tool_registers_model_visible_schema_and_origin() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(
            report
                .reports
                .iter()
                .all(|feature_report| feature_report.diagnostics.is_empty()),
            "{:#?}",
            report.reports
        );
        assert_eq!(report.installed_tool_names(), vec!["PluginSearch"]);
        assert_eq!(pending.len(), 1);
        let (meta, _) = pending[0]();
        assert_eq!(meta.name, "PluginSearch");
        assert_eq!(meta.input_schema["type"], "object");
        let origin = meta.origin.expect("plugin origin metadata");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn no_grant_denies_plugin_tool_registration_and_runtime_execution() {
        let mut record = record(vec![tool("PluginSearch")]);
        record.grants = PluginGrantConfig::default();

        let (report, pending) = install_plugin_record(record.clone());
        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "registration denied"));
        assert!(has_diagnostic(
            &report,
            "granted surfaces.tool permission is missing"
        ));

        let (_dir, mut runtime_record) = resolved_record_with_component(
            component_tool_that_returns(br#"{"summary":"should not run"}"#),
        );
        runtime_record.grants = PluginGrantConfig::default();
        let error =
            run_plugin_component_tool(runtime_record, "PluginEcho".into(), br#"{}"#.to_vec())
                .unwrap_err()
                .bounded_message();
        assert!(error.contains("plugin permission denied"), "{error}");
        assert!(
            error.contains("granted surfaces.tool permission is missing"),
            "{error}"
        );
        assert!(error.len() < 700, "{error}");
    }

    #[test]
    fn specific_tool_grant_registers_only_intended_plugin_tool() {
        let mut record = record(vec![tool("PluginAllowed"), tool("PluginDenied")]);
        record.grants.permissions = vec![
            PluginPermission::surface(PluginSurface::Tool),
            PluginPermission::tool("PluginAllowed"),
        ];

        let (report, pending) = install_plugin_record(record);
        assert_eq!(pending.len(), 1);
        let (meta, _) = pending[0]();
        assert_eq!(meta.name, "PluginAllowed");
        assert_eq!(report.installed_tool_names(), vec!["PluginAllowed"]);
        assert!(has_diagnostic(
            &report,
            "granted tool permission for `PluginDenied` is missing"
        ));
    }

    #[test]
    fn grant_binding_mismatches_do_not_authorize_plugin_tool() {
        let mut unrelated = record(vec![tool("PluginSearch")]);
        unrelated.grants.id = Some("project:other".to_string());
        let error = authorize_plugin_tool(&unrelated, &unrelated.manifest.tools[0])
            .unwrap_err()
            .bounded_message();
        assert!(
            error.contains("package id binding does not match"),
            "{error}"
        );

        let mut bad_digest = record(vec![tool("PluginSearch")]);
        bad_digest.grants.digest = Some("sha256:not-the-package".to_string());
        let error = authorize_plugin_tool(&bad_digest, &bad_digest.manifest.tools[0])
            .unwrap_err()
            .bounded_message();
        assert!(error.contains("digest binding does not match"), "{error}");

        let mut bad_version = record(vec![tool("PluginSearch")]);
        bad_version.grants.version = Some(PluginExactVersion("9.9.9".to_string()));
        let error = authorize_plugin_tool(&bad_version, &bad_version.manifest.tools[0])
            .unwrap_err()
            .bounded_message();
        assert!(error.contains("version binding does not match"), "{error}");
    }

    #[test]
    fn requested_surface_tool_and_external_write_permissions_are_required() {
        let mut missing_surface = record(vec![tool("PluginSearch")]);
        missing_surface.manifest.permissions = vec![PluginPermission::tool("PluginSearch")];
        let (report, pending) = install_plugin_record(missing_surface);
        assert!(pending.is_empty());
        assert!(has_diagnostic(
            &report,
            "requested surfaces.tool permission is missing"
        ));

        let mut missing_tool = record(vec![tool("PluginSearch")]);
        missing_tool.manifest.permissions = vec![PluginPermission::surface(PluginSurface::Tool)];
        let (report, pending) = install_plugin_record(missing_tool);
        assert!(pending.is_empty());
        assert!(has_diagnostic(
            &report,
            "requested tool permission for `PluginSearch` is missing"
        ));

        let mut external_tool = tool("PluginWrite");
        external_tool.external_write = true;
        let mut missing_external_request = record(vec![external_tool]);
        let (report, pending) = install_plugin_record(missing_external_request.clone());
        assert!(pending.is_empty());
        assert!(has_diagnostic(
            &report,
            "requested external_write permission is missing"
        ));

        missing_external_request
            .manifest
            .permissions
            .push(PluginPermission::ExternalWrite);
        let (report, pending) = install_plugin_record(missing_external_request);
        assert!(pending.is_empty());
        assert!(has_diagnostic(
            &report,
            "granted external_write permission is missing"
        ));
    }

    #[test]
    fn component_host_api_imports_are_permission_checked_by_manifest_and_grants() {
        let (_dir, mut record) = resolved_record_with_component(component_tool_importing_request(
            br#"{"summary":"should not run"}"#,
        ));
        record.manifest.permissions.retain(|permission| {
            !matches!(
                permission,
                PluginPermission::HostApi {
                    api: PluginHostApi::Request
                }
            )
        });
        record.manifest.request.clear();
        record.grants.permissions.retain(|permission| {
            !matches!(
                permission,
                PluginPermission::HostApi {
                    api: PluginHostApi::Request
                }
            )
        });
        record.grants.request.clear();

        let error =
            run_plugin_component_tool(record.clone(), "PluginEcho".into(), br#"{}"#.to_vec())
                .unwrap_err()
                .bounded_message();
        assert!(
            error.contains("requested host_api.request permission is missing"),
            "{error}"
        );

        record
            .manifest
            .permissions
            .push(PluginPermission::host_api(PluginHostApi::Request));
        record
            .grants
            .permissions
            .push(PluginPermission::host_api(PluginHostApi::Request));
        let error =
            run_plugin_component_tool(record.clone(), "PluginEcho".into(), br#"{}"#.to_vec())
                .unwrap_err()
                .bounded_message();
        assert!(
            error.contains("manifest host_api.request target declaration is missing"),
            "{error}"
        );

        record.manifest.request.push(PluginRequestGrant {
            scheme: "https".to_string(),
            host: "api.example.test".to_string(),
            port: None,
            methods: vec!["GET".to_string()],
            path_prefixes: vec!["/".to_string()],
        });
        let error = run_plugin_component_tool(record, "PluginEcho".into(), br#"{}"#.to_vec())
            .unwrap_err()
            .bounded_message();
        assert!(
            error.contains("enabled host_api.request grants are missing"),
            "{error}"
        );
    }

    #[test]
    fn package_without_enabled_tool_surface_registers_no_schema() {
        let mut config = PluginConfig::default();
        let mut disabled = record(vec![tool("PluginSearch")]);
        disabled.enabled_surfaces.clear();
        config.resolved.push(disabled);

        assert!(plugin_tool_features(&config).is_empty());
    }

    #[test]
    fn disabled_profile_feature_registers_no_schema() {
        let mut config = PluginConfig::default();
        config.resolved.push(record(vec![tool("PluginSearch")]));

        assert!(plugin_tool_features_if_enabled(false, &config).is_empty());
        assert_eq!(plugin_tool_features_if_enabled(true, &config).len(), 1);
    }

    #[test]
    fn duplicate_plugin_tool_names_are_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .with_module(PluginToolFeature::new(record_with_identity(
                "project:other",
                vec![tool("PluginSearch")],
            )))
            .install_into_pending(&mut pending, &mut hooks);

        assert_eq!(pending.len(), 1);
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn builtin_tool_name_collision_is_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let mut registered = std::collections::HashMap::new();
        registered.insert("Read".to_string(), FeatureId::builtin("preexisting-tool"));

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("Read")])))
            .install_into_pending_with_registered(&mut pending, &mut hooks, registered);

        assert!(pending.is_empty());
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn invalid_input_schema_is_rejected_with_diagnostic() {
        let mut invalid = tool("BadSchema");
        invalid.input_schema = json!({"type":"object","$ref":"#/defs/input"});
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![invalid])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "invalid input_schema"));
    }

    #[test]
    fn nested_invalid_input_schema_does_not_register_plugin_tool() {
        let mut invalid = tool("BadNestedSchema");
        invalid.input_schema = json!({
            "type":"object",
            "properties":{"query":"not-a-schema"},
            "required":["query"],
            "additionalProperties":false
        });
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![invalid])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "invalid input_schema"));
        assert!(has_diagnostic(&report, "$.properties.query"));
    }

    #[test]
    fn pdk_tool_output_shape_is_accepted_by_wasm_decoder() {
        let pdk_output =
            yoi_plugin_pdk::ToolOutput::json("pdk ok", serde_json::json!({"answer": 42}))
                .unwrap()
                .to_json_string();

        let output = decode_plugin_wasm_output(pdk_output.as_bytes()).unwrap();
        assert_eq!(output.summary, "pdk ok");
        assert_eq!(output.content.as_deref(), Some(r#"{"answer":42}"#));
    }

    fn write_component_plugin_package(path: &Path, component: &[u8], world: &str) {
        let manifest = format!(
            r#"schema_version = 1
id = "example"
name = "Example"
version = "1.0.0"
description = "Example component plugin"
surfaces = ["tool"]

[runtime]
kind = "wasm-component"
component = "plugin.component.wasm"
world = "{}"

[[permissions]]
kind = "surface"
surface = "tool"

[[permissions]]
kind = "tool"
name = "PluginEcho"

[[tools]]
name = "PluginEcho"
description = "Echo plugin tool"
input_schema = {{ type = "object", additionalProperties = true }}
"#,
            world
        );
        write_stored_zip(
            path,
            &[
                ("plugin.toml", manifest.as_bytes()),
                ("plugin.component.wasm", component),
            ],
        );
    }

    fn resolved_record_with_component(component: Vec<u8>) -> (TempDir, ResolvedPluginRecord) {
        let dir = TempDir::new().unwrap();
        let package_dir = dir.path().join(".yoi/plugins");
        fs::create_dir_all(&package_dir).unwrap();
        let package_path = package_dir.join("component.yoi-plugin");
        write_component_plugin_package(&package_path, &component, PLUGIN_COMPONENT_TOOL_WORLD);
        let config = PluginConfig {
            enabled: vec![PluginEnablementConfig {
                id: "project:example".parse().unwrap(),
                surfaces: vec![PluginSurface::Tool],
                ..PluginEnablementConfig::default()
            }],
            resolved: Vec::new(),
            diagnostics: Vec::new(),
        };
        let options = PluginDiscoveryOptions::new(dir.path());
        let resolved = resolve_plugin_config_for_startup(&config, &options);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        assert_eq!(resolved.resolved.len(), 1);
        let mut record = resolved.resolved[0].clone();
        record.grants = PluginGrantConfig {
            id: Some(record.identity.to_string()),
            version: Some(PluginExactVersion(record.version.clone())),
            digest: Some(record.digest.clone()),
            permissions: tool_permissions(&record.manifest.tools),
            request: Vec::new(),
            websocket: Vec::new(),
            fs: Vec::new(),
        };
        (dir, record)
    }

    fn wat_bytes(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!(r#"\{:02x}"#, byte))
            .collect()
    }

    fn component_instance_with_outputs(
        start: &[u8],
        status: &[u8],
        stop: &[u8],
        tool: &[u8],
        ingress: &[u8],
    ) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (core module $m
                (memory (export "memory") 1)
                (func (export "realloc") (param i32 i32 i32 i32) (result i32)
                  (if (result i32) (i32.eqz (local.get 0))
                    (then (i32.const 8192))
                    (else (local.get 0))))
                (data (i32.const 1024) "{}")
                (data (i32.const 2048) "{}")
                (data (i32.const 3072) "{}")
                (data (i32.const 4096) "{}")
                (data (i32.const 5120) "{}")
                (func $write (param i32 i32)
                  (i32.store (i32.const 6144) (local.get 0))
                  (i32.store (i32.const 6148) (local.get 1)))
                (func (export "start") (param i32 i32) (result i32)
                  (call $write (i32.const 1024) (i32.const {}))
                  (i32.const 6144))
                (func (export "status") (result i32)
                  (call $write (i32.const 2048) (i32.const {}))
                  (i32.const 6144))
                (func (export "stop") (result i32)
                  (call $write (i32.const 3072) (i32.const {}))
                  (i32.const 6144))
                (func (export "tool") (param i32 i32 i32 i32) (result i32)
                  (call $write (i32.const 4096) (i32.const {}))
                  (i32.const 6144))
                (func (export "ingress") (param i32 i32 i32 i32) (result i32)
                  (call $write (i32.const 5120) (i32.const {}))
                  (i32.const 6144))
              )
              (core instance $i (instantiate $m))
              (alias core export $i "memory" (core memory $mem))
              (alias core export $i "realloc" (core func $realloc))
              (alias core export $i "start" (core func $start_core))
              (alias core export $i "status" (core func $status_core))
              (alias core export $i "stop" (core func $stop_core))
              (alias core export $i "tool" (core func $tool_core))
              (alias core export $i "ingress" (core func $ingress_core))
              (type $start_ty (func (param "config-json" string) (result string)))
              (type $noarg_ty (func (result string)))
              (type $twoarg_ty (func (param "name" string) (param "json" string) (result string)))
              (func $start (type $start_ty) (canon lift (core func $start_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (func $status (type $noarg_ty) (canon lift (core func $status_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (func $stop (type $noarg_ty) (canon lift (core func $stop_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (func $tool (type $twoarg_ty) (canon lift (core func $tool_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (func $ingress (type $twoarg_ty) (canon lift (core func $ingress_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (export "start" (func $start))
              (export "status" (func $status))
              (export "stop" (func $stop))
              (export "handle-tool" (func $tool))
              (export "handle-ingress" (func $ingress))
            )"#,
            wat_bytes(start),
            wat_bytes(status),
            wat_bytes(stop),
            wat_bytes(tool),
            wat_bytes(ingress),
            start.len(),
            status.len(),
            stop.len(),
            tool.len(),
            ingress.len(),
        ))
        .unwrap()
    }

    fn component_tool_that_returns(output: &[u8]) -> Vec<u8> {
        component_tool_with_memory_pages(output, 1)
    }

    fn component_tool_with_memory_pages(output: &[u8], memory_pages: usize) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (core module $m
                (memory (export "memory") {})
                (func (export "realloc") (param i32 i32 i32 i32) (result i32)
                  (if (result i32) (i32.eqz (local.get 0))
                    (then (i32.const 8192))
                    (else (local.get 0))))
                (data (i32.const 1024) "{}")
                (func (export "call") (param i32 i32 i32 i32) (result i32)
                  (i32.store (i32.const 2048) (i32.const 1024))
                  (i32.store (i32.const 2052) (i32.const {}))
                  (i32.const 2048))
              )
              (core instance $i (instantiate $m))
              (alias core export $i "memory" (core memory $mem))
              (alias core export $i "realloc" (core func $realloc))
              (alias core export $i "call" (core func $call_core))
              (type $call_ty (func (param "tool-name" string) (param "input-json" string) (result string)))
              (func $call (type $call_ty) (canon lift (core func $call_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (export "call" (func $call))
            )"#,
            memory_pages,
            wat_bytes(output),
            output.len()
        ))
        .expect("valid component wat")
    }

    fn component_tool_with_table_elements(output: &[u8], table_elements: usize) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (core module $m
                (memory (export "memory") 1)
                (table {} funcref)
                (func (export "realloc") (param i32 i32 i32 i32) (result i32)
                  (if (result i32) (i32.eqz (local.get 0))
                    (then (i32.const 8192))
                    (else (local.get 0))))
                (data (i32.const 1024) "{}")
                (func (export "call") (param i32 i32 i32 i32) (result i32)
                  (i32.store (i32.const 2048) (i32.const 1024))
                  (i32.store (i32.const 2052) (i32.const {}))
                  (i32.const 2048))
              )
              (core instance $i (instantiate $m))
              (alias core export $i "memory" (core memory $mem))
              (alias core export $i "realloc" (core func $realloc))
              (alias core export $i "call" (core func $call_core))
              (type $call_ty (func (param "tool-name" string) (param "input-json" string) (result string)))
              (func $call (type $call_ty) (canon lift (core func $call_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (export "call" (func $call))
            )"#,
            table_elements,
            wat_bytes(output),
            output.len()
        ))
        .expect("valid component wat")
    }

    fn component_tool_importing_request(output: &[u8]) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(component
              (import "yoi:host/request@1.0.0" (instance $request (export "request" (func $request (param "request-json" string) (result string)))))
              (core module $m
                (memory (export "memory") 1)
                (func (export "realloc") (param i32 i32 i32 i32) (result i32)
                  (if (result i32) (i32.eqz (local.get 0))
                    (then (i32.const 8192))
                    (else (local.get 0))))
                (data (i32.const 1024) "{}")
                (func (export "call") (param i32 i32 i32 i32) (result i32)
                  (i32.store (i32.const 2048) (i32.const 1024))
                  (i32.store (i32.const 2052) (i32.const {}))
                  (i32.const 2048))
              )
              (core instance $i (instantiate $m))
              (alias core export $i "memory" (core memory $mem))
              (alias core export $i "realloc" (core func $realloc))
              (alias core export $i "call" (core func $call_core))
              (type $call_ty (func (param "tool-name" string) (param "input-json" string) (result string)))
              (func $call (type $call_ty) (canon lift (core func $call_core) (memory $mem) (realloc $realloc) string-encoding=utf8))
              (export "call" (func $call))
            )"#,
            wat_bytes(output),
            output.len()
        ))
        .expect("valid component wat")
    }

    fn component_without_call_export() -> Vec<u8> {
        wat::parse_str(r#"(component (core module $m) (core instance $i (instantiate $m)))"#)
            .unwrap()
    }

    fn raw_module_bytes() -> Vec<u8> {
        wat::parse_str(r#"(module (func (export "call")))"#).unwrap()
    }

    #[test]
    fn component_tool_executes_through_ordinary_tool_result_path() {
        let (_dir, record) = resolved_record_with_component(component_tool_that_returns(
            br#"{"summary":"component ok","content":"ordinary tool result path"}"#,
        ));

        let output = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect("component tool output");

        assert_eq!(output.summary, "component ok");
        assert_eq!(output.content.as_deref(), Some("ordinary tool result path"));
    }

    #[test]
    fn component_memory_limit_fails_closed_before_string_lift() {
        let oversized_memory_pages = (PLUGIN_WASM_MEMORY_BYTES / 65_536) + 1;
        let (_dir, record) = resolved_record_with_component(component_tool_with_memory_pages(
            br#"{"summary":"should not lift"}"#,
            oversized_memory_pages,
        ));

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("component memory limit is enforced");

        assert!(format!("{error:?}").contains("growing memory"), "{error:?}");
    }

    #[test]
    fn component_table_limit_fails_closed() {
        let (_dir, record) = resolved_record_with_component(component_tool_with_table_elements(
            br#"{"summary":"should not run"}"#,
            PLUGIN_WASM_TABLE_ELEMENTS + 1,
        ));

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("component table limit is enforced");

        assert!(format!("{error:?}").contains("growing table"), "{error:?}");
    }

    #[test]
    fn component_output_cap_still_fails_closed_after_bounded_lift() {
        let output = format!(
            r#"{{"summary":"too big","content":"{}"}}"#,
            "x".repeat(PLUGIN_WASM_MAX_OUTPUT_BYTES)
        );
        let (_dir, record) =
            resolved_record_with_component(component_tool_with_memory_pages(output.as_bytes(), 2));

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("component output cap is enforced");

        assert!(format!("{error:?}").contains("output exceeds"), "{error:?}");
    }

    #[test]
    fn component_tool_denies_host_import_without_matching_grant() {
        let (_dir, record) = resolved_record_with_component(component_tool_importing_request(
            br#"{"summary":"component ok"}"#,
        ));

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("host import without grant is denied");

        assert!(
            format!("{error:?}").contains("plugin host API dispatch denied"),
            "{error:?}"
        );
    }

    #[test]
    fn component_tool_missing_export_fails_closed() {
        let (_dir, record) = resolved_record_with_component(component_without_call_export());

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("missing export fails closed");

        assert!(
            format!("{error:?}").contains("does not export expected"),
            "{error:?}"
        );
    }

    #[test]
    fn core_wasm_is_not_silently_reinterpreted_as_component() {
        let (_dir, record) = resolved_record_with_component(raw_module_bytes());

        let error = run_plugin_component_tool(record, "PluginEcho".to_string(), b"{}".to_vec())
            .expect_err("core module is incompatible with component runtime");

        assert!(
            format!("{error:?}").contains("component is incompatible"),
            "{error:?}"
        );
    }

    #[test]
    fn component_wrong_world_fails_closed_during_discovery() {
        let dir = TempDir::new().unwrap();
        let package_dir = dir.path().join(".yoi/plugins");
        fs::create_dir_all(&package_dir).unwrap();
        let package_path = package_dir.join("component.yoi-plugin");
        write_component_plugin_package(
            &package_path,
            &component_tool_that_returns(br#"{"summary":"component ok"}"#),
            "example:other/world@1.0.0",
        );
        let config = PluginConfig {
            enabled: vec![PluginEnablementConfig {
                id: "project:example".parse().unwrap(),
                surfaces: vec![PluginSurface::Tool],
                ..PluginEnablementConfig::default()
            }],
            resolved: Vec::new(),
            diagnostics: Vec::new(),
        };
        let options = PluginDiscoveryOptions::new(dir.path());
        let resolved = resolve_plugin_config_for_startup(&config, &options);

        assert!(resolved.resolved.is_empty());
        assert!(
            resolved.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("component world is unsupported")),
            "{:#?}",
            resolved.diagnostics
        );
    }

    #[test]
    fn component_tool_registration_uses_existing_tool_registry_path() {
        let (_dir, record) = resolved_record_with_component(component_tool_that_returns(
            br#"{"summary":"component ok"}"#,
        ));
        let (report, pending) = install_plugin_record(record);

        assert_eq!(skipped_count(&report), 0, "{report:#?}");
        assert_eq!(pending.len(), 1);
        let (meta, _) = pending[0]();
        assert_eq!(meta.name, "PluginEcho");
    }

    #[test]
    fn static_inspection_reports_covering_request_grants_as_runtime_eligible() {
        let mut exact_manifest_broad_grant = record_with_request_grant();
        exact_manifest_broad_grant.grants.request = vec![PluginRequestGrant {
            scheme: "*".to_string(),
            host: "*".to_string(),
            port: None,
            methods: vec!["GET".to_string()],
            path_prefixes: Vec::new(),
        }];

        let inspection = inspect_resolved_plugin_static(&exact_manifest_broad_grant);
        let target = inspection
            .host_apis
            .iter()
            .find(|api| {
                api.permission
                    .starts_with("host_api.request target https://api.example.test")
            })
            .expect("manifest request target inspection");
        assert!(target.requested);
        assert!(target.granted);
        assert!(target.eligible);
        let diagnostic = target.diagnostic.as_deref().unwrap_or_default();
        assert!(
            diagnostic.contains("broad/arbitrary") || diagnostic.contains("partially covered"),
            "{target:#?}"
        );
        let broad_grant = inspection
            .host_apis
            .iter()
            .find(|api| api.permission.starts_with("host_api.request grant *://*"))
            .expect("broad grant inspection");
        assert!(broad_grant.requested);
        assert!(broad_grant.granted);
        assert!(broad_grant.eligible);
        assert!(
            !inspection
                .host_apis
                .iter()
                .any(|api| api.permission.starts_with("host_api.request grant-only")),
            "{:#?}",
            inspection.host_apis
        );
    }

    #[test]
    fn static_inspection_reports_request_grant_intersections_as_runtime_eligible() {
        let mut broad_manifest_exact_grant = record_with_request_grant();
        broad_manifest_exact_grant.manifest.request = vec![PluginRequestGrant {
            scheme: "*".to_string(),
            host: "*".to_string(),
            port: None,
            methods: vec!["GET".to_string(), "POST".to_string()],
            path_prefixes: Vec::new(),
        }];

        let inspection = inspect_resolved_plugin_static(&broad_manifest_exact_grant);
        let target = inspection
            .host_apis
            .iter()
            .find(|api| api.permission.starts_with("host_api.request target *://*"))
            .expect("broad manifest target inspection");
        assert!(target.requested);
        assert!(target.granted);
        assert!(target.eligible);
        assert!(
            target
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("partially covered"),
            "{target:#?}"
        );
        let exact_grant = inspection
            .host_apis
            .iter()
            .find(|api| {
                api.permission
                    .starts_with("host_api.request grant https://api.example.test")
            })
            .expect("exact grant inspection");
        assert!(exact_grant.requested);
        assert!(exact_grant.granted);
        assert!(exact_grant.eligible);
        assert!(
            !inspection
                .host_apis
                .iter()
                .any(|api| api.permission.starts_with("host_api.request grant-only")),
            "{:#?}",
            inspection.host_apis
        );
    }

    #[test]
    fn legacy_raw_wasm_runtime_is_rejected_without_fallback_execution() {
        let mut record = record(vec![tool("PluginEcho")]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: LEGACY_PLUGIN_RUNTIME_WASM_KIND.to_string(),
            entry: Some("plugin.wasm".to_string()),
            abi: Some("yoi-plugin-wasm-1".to_string()),
            component: None,
            world: None,
        });

        let inspection = inspect_resolved_plugin_static(&record);
        assert!(!inspection.runtime.eligible);
        assert!(
            inspection
                .runtime
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("legacy raw wasm plugin runtime is not an active execution path")
        );

        let error = match PluginInstanceHandle::new(record) {
            Ok(_) => panic!("legacy raw wasm runtime unexpectedly instantiated"),
            Err(error) => error.bounded_message(),
        };
        assert!(
            error.contains("legacy raw wasm plugin runtime is not supported"),
            "{error}"
        );
    }

    #[test]
    fn component_static_inspection_reports_component_runtime_without_execution() {
        let mut record = record(vec![tool("Echo")]);
        record.package_path = std::path::PathBuf::from("/no/such/component.wasm");
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_COMPONENT_KIND.to_string(),
            entry: None,
            abi: None,
            component: Some("plugin.component.wasm".to_string()),
            world: Some(PLUGIN_COMPONENT_TOOL_WORLD.to_string()),
        });

        let inspection = inspect_resolved_plugin_static(&record);

        assert!(inspection.runtime.eligible);
        assert_eq!(
            inspection.runtime.status,
            format!("{PLUGIN_RUNTIME_COMPONENT_KIND}/{PLUGIN_COMPONENT_TOOL_WORLD}")
        );
        assert!(inspection.runtime.diagnostic.is_none());
    }

    #[test]
    fn static_inspection_does_not_read_or_execute_package() {
        let mut record = record(vec![tool("Echo")]);
        record.package_path = std::path::PathBuf::from("/no/such/plugin.wasm");
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_COMPONENT_KIND.to_string(),
            entry: None,
            abi: None,
            component: Some("plugin.component.wasm".to_string()),
            world: Some(PLUGIN_COMPONENT_TOOL_WORLD.to_string()),
        });

        let inspection = inspect_resolved_plugin_static(&record);

        assert!(inspection.runtime.eligible);
        assert_eq!(inspection.tools.len(), 1);
        assert!(inspection.tools[0].eligible);
        assert!(inspection.statically_eligible());
    }

    #[test]
    fn static_inspection_reports_missing_tool_grant() {
        let mut record = record(vec![tool("Echo")]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_COMPONENT_KIND.to_string(),
            entry: None,
            abi: None,
            component: Some("plugin.component.wasm".to_string()),
            world: Some(PLUGIN_COMPONENT_TOOL_WORLD.to_string()),
        });
        record.grants.permissions = vec![PluginPermission::surface(PluginSurface::Tool)];

        let inspection = inspect_resolved_plugin_static(&record);

        assert!(!inspection.statically_eligible());
        assert!(!inspection.tools[0].eligible);
        assert!(
            inspection.tools[0]
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("grant")
        );
    }

    #[test]
    fn static_inspection_reports_invalid_tool_definition() {
        let mut bad_schema = tool("Echo");
        bad_schema.input_schema = json!({"type":"string"});
        let mut record = record(vec![bad_schema]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_COMPONENT_KIND.to_string(),
            entry: None,
            abi: None,
            component: Some("plugin.component.wasm".to_string()),
            world: Some(PLUGIN_COMPONENT_TOOL_WORLD.to_string()),
        });

        let inspection = inspect_resolved_plugin_static(&record);

        assert!(!inspection.statically_eligible());
        assert!(!inspection.tools[0].eligible);
        let diagnostic = inspection.tools[0]
            .diagnostic
            .as_deref()
            .unwrap_or_default();
        assert!(diagnostic.contains("invalid input_schema"));
        assert!(diagnostic.contains("root schema type must be `object`"));
    }

    #[test]
    fn static_inspection_reports_invalid_and_duplicate_tool_names() {
        let mut invalid = tool("Bad Tool");
        invalid.input_schema = json!({"type":"object"});
        let mut first_duplicate = tool("Echo");
        let mut second_duplicate = tool("Echo");
        first_duplicate.input_schema = json!({"type":"object"});
        second_duplicate.input_schema = json!({"type":"object"});
        let mut record = record(vec![invalid, first_duplicate, second_duplicate]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_COMPONENT_KIND.to_string(),
            entry: None,
            abi: None,
            component: Some("plugin.component.wasm".to_string()),
            world: Some(PLUGIN_COMPONENT_TOOL_WORLD.to_string()),
        });

        let inspection = inspect_resolved_plugin_static(&record);

        assert!(!inspection.statically_eligible());
        assert!(
            inspection.tools[0]
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("invalid name")
        );
        assert!(
            inspection.tools[1]
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("duplicate name")
        );
        assert!(
            inspection.tools[2]
                .diagnostic
                .as_deref()
                .unwrap_or_default()
                .contains("duplicate name")
        );
    }

    fn write_stored_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let name_bytes = name.as_bytes();
            let crc = crc32(data);
            write_u32(&mut out, 0x0403_4b50);
            write_u16(&mut out, 20);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u32(&mut out, crc);
            write_u32(&mut out, data.len() as u32);
            write_u32(&mut out, data.len() as u32);
            write_u16(&mut out, name_bytes.len() as u16);
            write_u16(&mut out, 0);
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            write_u32(&mut central, 0x0201_4b50);
            write_u16(&mut central, 20);
            write_u16(&mut central, 20);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, crc);
            write_u32(&mut central, data.len() as u32);
            write_u32(&mut central, data.len() as u32);
            write_u16(&mut central, name_bytes.len() as u16);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, 0);
            write_u32(&mut central, offset);
            central.extend_from_slice(name_bytes);
        }
        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);
        write_u32(&mut out, 0x0605_4b50);
        write_u16(&mut out, 0);
        write_u16(&mut out, 0);
        write_u16(&mut out, files.len() as u16);
        write_u16(&mut out, files.len() as u16);
        write_u32(&mut out, central_size);
        write_u32(&mut out, central_offset);
        write_u16(&mut out, 0);
        fs::write(path, out).unwrap();
    }

    fn write_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    #[derive(Clone, Default)]
    struct MockWebSocketClient {
        closed: Arc<std::sync::atomic::AtomicUsize>,
        opens: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PluginWebSocketClient for MockWebSocketClient {
        fn supports_bounded_open(&self) -> bool {
            true
        }

        fn open(
            &self,
            _request: &PluginWebSocketOpenRequest,
            _url: &reqwest::Url,
            _limits: PluginWebSocketLimits,
        ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError> {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(MockWebSocketConnection {
                closed: self.closed.clone(),
                next_recv: Some(PluginWebSocketRecvResponse::Text {
                    text: "hello".to_string(),
                }),
            }))
        }
    }

    struct MockWebSocketConnection {
        closed: Arc<std::sync::atomic::AtomicUsize>,
        next_recv: Option<PluginWebSocketRecvResponse>,
    }

    impl PluginWebSocketConnection for MockWebSocketConnection {
        fn send_text(&mut self, _text: &str) -> Result<(), PluginWebSocketError> {
            Ok(())
        }

        fn recv_text(
            &mut self,
            _timeout: Duration,
            _max_message_bytes: usize,
        ) -> Result<PluginWebSocketRecvResponse, PluginWebSocketError> {
            Ok(self
                .next_recv
                .take()
                .unwrap_or(PluginWebSocketRecvResponse::Closed))
        }

        fn close(&mut self) -> Result<(), PluginWebSocketError> {
            self.closed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct FailingWebSocketClient {
        opens: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PluginWebSocketClient for FailingWebSocketClient {
        fn supports_bounded_open(&self) -> bool {
            true
        }

        fn open(
            &self,
            _request: &PluginWebSocketOpenRequest,
            _url: &reqwest::Url,
            _limits: PluginWebSocketLimits,
        ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError> {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(PluginWebSocketError::new("simulated bounded open failure"))
        }
    }

    #[derive(Clone, Default)]
    struct UnboundedWebSocketClient {
        opens: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PluginWebSocketClient for UnboundedWebSocketClient {
        fn supports_bounded_open(&self) -> bool {
            false
        }

        fn open(
            &self,
            _request: &PluginWebSocketOpenRequest,
            _url: &reqwest::Url,
            _limits: PluginWebSocketLimits,
        ) -> Result<Box<dyn PluginWebSocketConnection>, PluginWebSocketError> {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(PluginWebSocketError::new("should not dial"))
        }
    }
    fn websocket_grant(
        scheme: &str,
        host: &str,
        port: Option<u16>,
        paths: &[&str],
    ) -> PluginWebSocketGrant {
        PluginWebSocketGrant {
            scheme: scheme.to_string(),
            host: host.to_string(),
            port,
            path_prefixes: paths.iter().map(|path| (*path).to_string()).collect(),
        }
    }

    fn record_with_websocket(
        manifest_targets: Vec<PluginWebSocketGrant>,
        grants: Vec<PluginWebSocketGrant>,
    ) -> ResolvedPluginRecord {
        let mut record = record(vec![]);
        record.manifest.permissions = vec![PluginPermission::host_api(PluginHostApi::WebSocket)];
        record.manifest.websocket = manifest_targets;
        record.grants.websocket = grants;
        record
    }

    #[test]
    fn websocket_max_open_connections_rejects_before_network_open() {
        let record = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let client = MockWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        for _ in 0..PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS {
            execute_plugin_websocket_open(
                &record,
                &client,
                &handles,
                br#"{"url":"wss://gateway.example.com/gateway"}"#,
            )
            .unwrap();
        }
        assert_eq!(
            client.opens.load(std::sync::atomic::Ordering::SeqCst),
            PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS
        );
        let error = execute_plugin_websocket_open(
            &record,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("before dialing"));
        assert_eq!(
            client.opens.load(std::sync::atomic::Ordering::SeqCst),
            PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS
        );
    }

    #[test]
    fn websocket_open_failure_releases_capacity_reservation() {
        let record = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let failing = FailingWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        let error = execute_plugin_websocket_open(
            &record,
            &failing,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("simulated bounded open failure"));
        assert_eq!(handles.reservation_count(), 0);

        let client = MockWebSocketClient::default();
        let open = execute_plugin_websocket_open(
            &record,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap();
        let open: PluginWebSocketOpenResponse = serde_json::from_slice(&open).unwrap();
        assert_eq!(open.handle, 1);
    }

    #[test]
    fn websocket_unbounded_open_client_fails_closed_before_dialing() {
        let record = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let client = UnboundedWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        let error = execute_plugin_websocket_open(
            &record,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap_err();
        assert!(
            error
                .0
                .contains("cannot guarantee bounded/cancellable open")
        );
        assert_eq!(client.opens.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(handles.reservation_count(), 0);
    }
    #[test]
    fn websocket_open_send_recv_close_is_bounded_and_explicit() {
        let record = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let client = MockWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        let open = execute_plugin_websocket_open(
            &record,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway?v=10"}"#,
        )
        .unwrap();
        let open: PluginWebSocketOpenResponse = serde_json::from_slice(&open).unwrap();
        assert_eq!(open.handle, 1);
        assert_eq!(open.url, "wss://gateway.example.com/gateway");

        let send = execute_plugin_websocket_send_text(&handles, open.handle, b"ping").unwrap();
        assert_eq!(send, br#"{"sent":true,"bytes":4}"#);
        let recv = execute_plugin_websocket_recv(&handles, open.handle, 1).unwrap();
        assert_eq!(recv, br#"{"type":"text","text":"hello"}"#);
        let close = execute_plugin_websocket_close(&handles, open.handle).unwrap();
        assert_eq!(close, br#"{"closed":true}"#);
        assert_eq!(client.closed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn websocket_open_requires_manifest_and_grant() {
        let client = MockWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        let missing_grant = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![],
        );
        let error = execute_plugin_websocket_open(
            &missing_grant,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("enabled WebSocket grants"));

        let missing_manifest = record_with_websocket(
            vec![],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let error = execute_plugin_websocket_open(
            &missing_manifest,
            &client,
            &handles,
            br#"{"url":"wss://gateway.example.com/gateway"}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("not declared"));
        assert_eq!(client.opens.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn websocket_loopback_requires_explicit_manifest_and_grant() {
        let client = MockWebSocketClient::default();
        let handles = PluginWebSocketHandles::default();
        let denied = record_with_websocket(
            vec![websocket_grant("ws", "127.0.0.1", Some(8080), &["/socket"])],
            vec![],
        );
        assert!(
            execute_plugin_websocket_open(
                &denied,
                &client,
                &handles,
                br#"{"url":"ws://127.0.0.1:8080/socket"}"#,
            )
            .is_err()
        );
        let allowed = record_with_websocket(
            vec![websocket_grant("ws", "127.0.0.1", Some(8080), &["/socket"])],
            vec![websocket_grant("ws", "127.0.0.1", Some(8080), &["/socket"])],
        );
        assert!(
            execute_plugin_websocket_open(
                &allowed,
                &client,
                &handles,
                br#"{"url":"ws://127.0.0.1:8080/socket"}"#,
            )
            .is_ok()
        );
    }

    #[test]
    fn websocket_rejects_guest_headers_and_binary_send_surface() {
        let record = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant(
                "wss",
                "gateway.example.com",
                None,
                &["/gateway"],
            )],
        );
        let error = validate_plugin_websocket_open_request(
            &record,
            br#"{"url":"wss://gateway.example.com/gateway","headers":[{"name":"authorization","value":"secret"}]}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("handshake headers"));
        let handles = PluginWebSocketHandles::default();
        let invalid_utf8 = [0xff, 0xfe];
        let error = execute_plugin_websocket_send_text(&handles, 1, &invalid_utf8).unwrap_err();
        assert!(error.0.contains("UTF-8"));
    }

    #[test]
    fn websocket_static_inspection_reports_grant_only_missing_and_broad() {
        let grant_only = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "declared.example.com",
                None,
                &["/gateway"],
            )],
            vec![websocket_grant("*", "*", None, &[])],
        );
        let inspection = inspect_resolved_plugin_static(&grant_only);
        let ws: Vec<_> = inspection
            .host_apis
            .iter()
            .filter(|item| item.permission.contains("host_api.websocket"))
            .collect();
        assert!(ws.iter().any(|item| item.permission.contains("target")));
        assert!(ws.iter().any(|item| {
            item.permission.contains("grant")
                && item
                    .diagnostic
                    .as_deref()
                    .unwrap_or_default()
                    .contains("broad")
        }));

        let missing = record_with_websocket(
            vec![websocket_grant(
                "wss",
                "missing.example.com",
                None,
                &["/gateway"],
            )],
            vec![],
        );
        let inspection = inspect_resolved_plugin_static(&missing);
        assert!(inspection.host_apis.iter().any(|item| {
            item.permission.contains("host_api.websocket target")
                && item
                    .diagnostic
                    .as_deref()
                    .unwrap_or_default()
                    .contains("missing")
        }));
    }

    #[test]
    fn request_host_api_still_rejects_websocket_urls() {
        let record = record_with_request_grant();
        let client = MockRequestClient::default();
        let error = execute_plugin_request_request(
            &record,
            &client,
            br#"{"method":"GET","url":"wss://api.example.com/socket"}"#,
        )
        .unwrap_err();
        assert!(error.0.contains("WebSocket"));
    }
}
