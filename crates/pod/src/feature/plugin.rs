//! Plugin package contributions for model-visible Tool schemas.
//!
//! This module registers *enabled* plugin package tool surface definitions and
//! executes Tool calls through the minimal sandboxed `yoi-plugin-wasm-1` WASM
//! ABI. It deliberately does not grant filesystem, environment, hook, service,
//! ingress, or ambient network authority. WASM Tools can only reach outbound Request
//! through the explicit `yoi:request` host import, and filesystem read/list/write
//! through the explicit `yoi:fs` host import, with matching permissions and
//! scoped allowlist grants.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use llm_worker::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOrigin, ToolOutput,
};
use manifest::plugin::{
    PLUGIN_COMPONENT_INSTANCE_WORLD, PLUGIN_COMPONENT_TOOL_WORLD, PLUGIN_RUNTIME_COMPONENT_KIND,
    PLUGIN_RUNTIME_WASM_ABI, PLUGIN_RUNTIME_WASM_KIND, PluginConfig, PluginDiscoveryLimits,
    PluginFsGrant, PluginFsOperation, PluginHostApi, PluginPermission, PluginRequestGrant,
    PluginSurface, PluginToolManifest, ResolvedPluginRecord,
    read_resolved_plugin_runtime_component, read_resolved_plugin_runtime_module,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    ) -> Result<PluginIngressDispatchReport, PluginWasmError> {
        if !surface_enabled(&self.record, PluginSurface::Ingress) {
            return Err(PluginWasmError::Module(
                "plugin ingress surface is not enabled".to_string(),
            ));
        }
        let handle = self
            .registry
            .handle(&self.record.identity.to_string())
            .ok_or_else(|| PluginWasmError::Module("plugin instance is not started".to_string()))?;
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
            if runtime.kind == PLUGIN_RUNTIME_WASM_KIND
                && runtime.abi.as_deref() == Some(PLUGIN_RUNTIME_WASM_ABI)
                && runtime.entry.is_some() =>
        {
            PluginRuntimeEligibility {
                eligible: true,
                status: format!("{PLUGIN_RUNTIME_WASM_KIND}/{PLUGIN_RUNTIME_WASM_ABI}"),
                diagnostic: None,
            }
        }
        Some(runtime) if runtime.kind == PLUGIN_RUNTIME_WASM_KIND => {
            let status = runtime
                .abi
                .as_deref()
                .map(|abi| format!("{PLUGIN_RUNTIME_WASM_KIND}/{abi}"))
                .unwrap_or_else(|| format!("{PLUGIN_RUNTIME_WASM_KIND}/<missing-abi>"));
            PluginRuntimeEligibility {
                eligible: false,
                status,
                diagnostic: Some("unsupported or missing plugin runtime ABI".to_string()),
            }
        }
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

    let mut host_apis: Vec<_> = [PluginHostApi::Request, PluginHostApi::Fs]
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

const PLUGIN_WASM_HOST_MODULE: &str = "yoi:tool";
const PLUGIN_WASM_ENTRYPOINT: &str = "yoi_tool_call";
const PLUGIN_WASM_MAX_INPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_SUMMARY_BYTES: usize = 1024;
const PLUGIN_WASM_FUEL: u64 = 5_000_000;
const PLUGIN_WASM_TIMEOUT: Duration = Duration::from_secs(1);
const PLUGIN_WASM_MEMORY_BYTES: usize = 2 * 1024 * 1024;
const PLUGIN_WASM_TABLE_ELEMENTS: usize = 256;
const PLUGIN_WASM_REQUEST_MODULE: &str = "yoi:request";
const PLUGIN_WASM_FS_MODULE: &str = "yoi:fs";
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
    Started,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginInstanceDiagnostic {
    pub state: PluginInstanceLifecycleState,
    pub message: String,
}

impl PluginInstanceDiagnostic {
    pub fn new(state: PluginInstanceLifecycleState, message: impl Into<String>) -> Self {
        Self {
            state,
            message: bounded_message(message.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginInstanceStatus {
    pub plugin_ref: String,
    pub lifecycle: PluginInstanceLifecycleState,
    pub component_status: Option<Value>,
    pub diagnostics: Vec<PluginInstanceDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginIngressEvent {
    pub kind: String,
    pub source: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PluginIngressDispatchReport {
    pub plugin_ref: String,
    pub ingress: String,
    pub accepted: bool,
    pub output: Value,
    pub diagnostics: Vec<PluginInstanceDiagnostic>,
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

#[derive(Clone)]
pub struct PluginInstanceHandle(Arc<Mutex<PluginInstance>>);

impl PluginInstanceHandle {
    fn new(record: ResolvedPluginRecord) -> Result<Self, PluginWasmError> {
        let runtime = PluginInstanceRuntime::new(&record)?;
        let mut instance = PluginInstance {
            record,
            runtime,
            lifecycle: PluginInstanceLifecycleState::Ready,
            component_status: None,
            diagnostics: Vec::new(),
        };
        instance.start()?;
        Ok(Self(Arc::new(Mutex::new(instance))))
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
    ) -> Result<PluginIngressDispatchReport, PluginWasmError> {
        self.0
            .lock()
            .expect("plugin instance poisoned")
            .deliver_ingress(ingress_name, event)
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
            instance.diagnostics.push(diagnostic);
        }
    }
}

struct PluginInstance {
    record: ResolvedPluginRecord,
    runtime: PluginInstanceRuntime,
    lifecycle: PluginInstanceLifecycleState,
    component_status: Option<Value>,
    diagnostics: Vec<PluginInstanceDiagnostic>,
}

impl PluginInstance {
    fn start(&mut self) -> Result<(), PluginWasmError> {
        match &mut self.runtime {
            PluginInstanceRuntime::LegacyToolAdapter => {
                self.lifecycle = PluginInstanceLifecycleState::Ready;
                self.diagnostics.push(PluginInstanceDiagnostic::new(
                    PluginInstanceLifecycleState::Ready,
                    "legacy tool runtime adapted behind PluginInstanceRegistry",
                ));
            }
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { .. } => {
                self.lifecycle = PluginInstanceLifecycleState::Started;
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                let status = runtime.start(&self.record)?;
                self.component_status = Some(status);
                self.lifecycle = PluginInstanceLifecycleState::Started;
            }
        }
        Ok(())
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
            PluginInstanceRuntime::LegacyToolAdapter => {
                run_plugin_tool(self.record.clone(), tool_name.to_string(), input)
            }
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { calls } => {
                *calls += 1;
                Ok(ToolOutput {
                    summary: format!("{tool_name}: {calls}"),
                    content: Some(String::from_utf8_lossy(&input).to_string()),
                })
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                runtime.handle_tool(tool_name, input)
            }
        }
    }

    fn deliver_ingress(
        &mut self,
        ingress_name: &str,
        event: PluginIngressEvent,
    ) -> Result<PluginIngressDispatchReport, PluginWasmError> {
        if !surface_enabled(&self.record, PluginSurface::Ingress) {
            return Err(PluginWasmError::Module(
                "plugin ingress surface is not enabled".to_string(),
            ));
        }
        if serde_json::to_vec(&event)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX)
            > PLUGIN_WASM_MAX_INPUT_BYTES
        {
            return Err(PluginWasmError::Module(format!(
                "plugin ingress event exceeds {} bytes",
                PLUGIN_WASM_MAX_INPUT_BYTES
            )));
        }
        self.record
            .manifest
            .ingresses
            .iter()
            .find(|ingress| ingress.name == ingress_name)
            .ok_or_else(|| {
                PluginWasmError::Module(
                    "requested ingress is not declared by plugin manifest".to_string(),
                )
            })?;
        authorize_plugin_ingress(&self.record, ingress_name).map_err(|error| {
            PluginWasmError::Module(format!(
                "plugin ingress permission denied: {}",
                error.bounded_message()
            ))
        })?;
        match &mut self.runtime {
            PluginInstanceRuntime::LegacyToolAdapter => Err(PluginWasmError::Module(
                "legacy tool runtime does not expose ingress dispatch".to_string(),
            )),
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { calls } => {
                let output = serde_json::json!({
                    "ingress": ingress_name,
                    "kind": event.kind,
                    "source": event.source,
                    "calls": *calls,
                    "payload": event.payload,
                });
                Ok(PluginIngressDispatchReport {
                    plugin_ref: self.record.identity.to_string(),
                    ingress: ingress_name.to_string(),
                    accepted: true,
                    output,
                    diagnostics: self.diagnostics.clone(),
                })
            }
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                let output = runtime.handle_ingress(ingress_name, &event)?;
                Ok(PluginIngressDispatchReport {
                    plugin_ref: self.record.identity.to_string(),
                    ingress: ingress_name.to_string(),
                    accepted: true,
                    output,
                    diagnostics: self.diagnostics.clone(),
                })
            }
        }
    }

    fn stop(&mut self) -> Result<(), PluginWasmError> {
        match &mut self.runtime {
            PluginInstanceRuntime::LegacyToolAdapter => {}
            #[cfg(test)]
            PluginInstanceRuntime::TestIngress { .. } => {}
            PluginInstanceRuntime::ComponentInstance(runtime) => {
                self.component_status = Some(runtime.stop()?);
            }
        }
        self.lifecycle = PluginInstanceLifecycleState::Stopped;
        Ok(())
    }

    fn snapshot_status(&self) -> PluginInstanceStatus {
        PluginInstanceStatus {
            plugin_ref: self.record.identity.to_string(),
            lifecycle: self.lifecycle.clone(),
            component_status: self.component_status.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn status(&mut self) -> PluginInstanceStatus {
        if let PluginInstanceRuntime::ComponentInstance(runtime) = &mut self.runtime {
            match runtime.status() {
                Ok(status) => self.component_status = Some(status),
                Err(error) => {
                    self.lifecycle = PluginInstanceLifecycleState::Failed;
                    self.diagnostics.push(PluginInstanceDiagnostic::new(
                        PluginInstanceLifecycleState::Failed,
                        format!(
                            "plugin component status failed: {}",
                            error.bounded_message()
                        ),
                    ));
                }
            }
        }
        PluginInstanceStatus {
            plugin_ref: self.record.identity.to_string(),
            lifecycle: self.lifecycle.clone(),
            component_status: self.component_status.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }
}

enum PluginInstanceRuntime {
    LegacyToolAdapter,
    #[cfg(test)]
    TestIngress {
        calls: u64,
    },
    ComponentInstance(PluginComponentInstanceRuntime),
}

impl PluginInstanceRuntime {
    fn new(record: &ResolvedPluginRecord) -> Result<Self, PluginWasmError> {
        let Some(runtime) = record.manifest.runtime.as_ref() else {
            return Ok(Self::LegacyToolAdapter);
        };
        match runtime.kind.as_str() {
            #[cfg(test)]
            "test-ingress" => Ok(Self::TestIngress { calls: 0 }),
            PLUGIN_RUNTIME_WASM_KIND => Ok(Self::LegacyToolAdapter),
            PLUGIN_RUNTIME_COMPONENT_KIND
                if runtime.world.as_deref() == Some(PLUGIN_COMPONENT_INSTANCE_WORLD) =>
            {
                Ok(Self::ComponentInstance(
                    PluginComponentInstanceRuntime::instantiate(record)?,
                ))
            }
            PLUGIN_RUNTIME_COMPONENT_KIND => Ok(Self::LegacyToolAdapter),
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

#[cfg(test)]
struct PluginWasmTool {
    record: ResolvedPluginRecord,
    name: String,
    origin: ToolOrigin,
}

#[cfg(test)]
#[async_trait]
impl Tool for PluginWasmTool {
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
        let record = self.record.clone();
        let name = self.name.clone();
        let plugin_ref = self.origin.plugin_ref.clone();
        let digest = self.origin.digest.clone();
        let input = input_json.as_bytes().to_vec();
        let execution = tokio::task::spawn_blocking(move || run_plugin_tool(record, name, input));
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
            Err(_) => Err(ToolError::ExecutionFailed(format!(
                "plugin tool `{}` from `{}` (digest {}) timed out after {:?}",
                self.name, plugin_ref, digest, PLUGIN_WASM_TIMEOUT
            ))),
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

struct PluginWasmHostState {
    record: ResolvedPluginRecord,
    request_client: Arc<dyn PluginRequestClient>,
    tool_name: Vec<u8>,
    input: Vec<u8>,
    output: Vec<u8>,
    output_error: Option<String>,
    request_response: Vec<u8>,
    fs_response: Vec<u8>,
    store_limits: wasmi::StoreLimits,
}

fn run_plugin_tool(
    record: ResolvedPluginRecord,
    tool_name: String,
    input: Vec<u8>,
) -> Result<ToolOutput, PluginWasmError> {
    match record
        .manifest
        .runtime
        .as_ref()
        .map(|runtime| runtime.kind.as_str())
    {
        Some(PLUGIN_RUNTIME_WASM_KIND) => run_plugin_wasm_tool(record, tool_name, input),
        Some(PLUGIN_RUNTIME_COMPONENT_KIND) => run_plugin_component_tool(record, tool_name, input),
        Some(other) => Err(PluginWasmError::Module(format!(
            "unsupported plugin runtime kind `{other}`"
        ))),
        None => Err(PluginWasmError::Package(
            "plugin runtime is not declared".to_string(),
        )),
    }
}

fn run_plugin_wasm_tool(
    record: ResolvedPluginRecord,
    tool_name: String,
    input: Vec<u8>,
) -> Result<ToolOutput, PluginWasmError> {
    run_plugin_wasm_tool_with_request_client(
        record,
        tool_name,
        input,
        Arc::new(ReqwestPluginRequestClient),
    )
}

fn run_plugin_wasm_tool_with_request_client(
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
    let module_bytes = read_resolved_plugin_runtime_module(&record, &limits)
        .map_err(|diagnostic| PluginWasmError::Package(diagnostic.message))?;
    if module_bytes.len() > limits.max_file_size_bytes as usize {
        return Err(PluginWasmError::Package(format!(
            "WASM runtime module exceeds {} bytes",
            limits.max_file_size_bytes
        )));
    }

    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    config.set_max_recursion_depth(64);
    config.set_max_stack_height(8 * 1024 * 1024);
    let engine = wasmi::Engine::new(&config);
    let module = wasmi::Module::new(&engine, &module_bytes[..])
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    validate_wasm_imports(&record, &module)?;

    let store_limits = wasmi::StoreLimitsBuilder::new()
        .memory_size(PLUGIN_WASM_MEMORY_BYTES)
        .table_elements(PLUGIN_WASM_TABLE_ELEMENTS)
        .instances(1)
        .tables(1)
        .memories(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = wasmi::Store::new(
        &engine,
        PluginWasmHostState {
            record: record.clone(),
            request_client,
            tool_name: tool_name.into_bytes(),
            input,
            output: Vec::new(),
            output_error: None,
            request_response: Vec::new(),
            fs_response: Vec::new(),
            store_limits,
        },
    );
    store.limiter(|state| &mut state.store_limits);
    store
        .set_fuel(PLUGIN_WASM_FUEL)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;

    let mut linker = wasmi::Linker::<PluginWasmHostState>::new(&engine);
    define_plugin_wasm_host_imports(&mut linker)?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
    let entry = instance
        .get_typed_func::<(), ()>(&store, PLUGIN_WASM_ENTRYPOINT)
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    entry
        .call(&mut store, ())
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;

    if let Some(error) = store.data().output_error.clone() {
        return Err(PluginWasmError::Output(error));
    }
    decode_plugin_wasm_output(&store.data().output)
}

#[derive(Clone)]
struct PluginComponentHostState {
    record: ResolvedPluginRecord,
    request_client: Arc<dyn PluginRequestClient>,
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

fn validate_wasm_imports(
    record: &ResolvedPluginRecord,
    module: &wasmi::Module,
) -> Result<(), PluginWasmError> {
    for import in module.imports() {
        match import.module() {
            PLUGIN_WASM_HOST_MODULE => match import.name() {
                "tool_name_len" | "tool_name_read" | "input_len" | "input_read"
                | "output_write" => {}
                other => {
                    return Err(PluginWasmError::Module(format!(
                        "unsupported host import `{}`; no filesystem, ambient network, environment, or WASI imports are available",
                        other
                    )));
                }
            },
            PLUGIN_WASM_REQUEST_MODULE => {
                authorize_plugin_host_api(record, PluginHostApi::Request).map_err(|error| {
                    PluginWasmError::Module(format!(
                        "plugin host API dispatch denied: {}",
                        error.bounded_message()
                    ))
                })?;
                match import.name() {
                    "request" | "response_len" | "response_read" => {}
                    other => {
                        return Err(PluginWasmError::Module(format!(
                            "unsupported request host import `{other}`"
                        )));
                    }
                }
            }
            PLUGIN_WASM_FS_MODULE => {
                authorize_plugin_host_api(record, PluginHostApi::Fs).map_err(|error| {
                    PluginWasmError::Module(format!(
                        "plugin host API dispatch denied: {}",
                        error.bounded_message()
                    ))
                })?;
                match import.name() {
                    "read" | "list" | "write" | "response_len" | "response_read" => {}
                    other => {
                        return Err(PluginWasmError::Module(format!(
                            "unsupported fs host import `{other}`"
                        )));
                    }
                }
            }
            other => {
                return Err(PluginWasmError::Module(format!(
                    "unsupported import module `{}`; only `{}`, `{}`, and `{}` are available",
                    other,
                    PLUGIN_WASM_HOST_MODULE,
                    PLUGIN_WASM_REQUEST_MODULE,
                    PLUGIN_WASM_FS_MODULE
                )));
            }
        }
    }
    Ok(())
}

fn define_plugin_wasm_host_imports(
    linker: &mut wasmi::Linker<PluginWasmHostState>,
) -> Result<(), PluginWasmError> {
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "tool_name_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().tool_name.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "input_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().input.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "tool_name_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::ToolName)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "input_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::Input)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "output_write",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_output(&mut caller, ptr, len)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_REQUEST_MODULE,
            "request",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_request_request(&mut caller, ptr, len)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_REQUEST_MODULE,
            "response_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().request_response.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_REQUEST_MODULE,
            "response_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::RequestResponse)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_FS_MODULE,
            "read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_fs_request(&mut caller, ptr, len, PluginFsRuntimeOperation::Read)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_FS_MODULE,
            "list",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_fs_request(&mut caller, ptr, len, PluginFsRuntimeOperation::List)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_FS_MODULE,
            "write",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_fs_request(&mut caller, ptr, len, PluginFsRuntimeOperation::Write)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_FS_MODULE,
            "response_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().fs_response.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_FS_MODULE,
            "response_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::FsResponse)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum HostBuffer {
    ToolName,
    Input,
    RequestResponse,
    FsResponse,
}

fn write_host_bytes_to_guest(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
    buffer: HostBuffer,
) -> i32 {
    if ptr < 0 || len < 0 {
        return -1;
    }
    let bytes = match buffer {
        HostBuffer::ToolName => caller.data().tool_name.clone(),
        HostBuffer::Input => caller.data().input.clone(),
        HostBuffer::RequestResponse => caller.data().request_response.clone(),
        HostBuffer::FsResponse => caller.data().fs_response.clone(),
    };
    if len as usize != bytes.len() {
        return -1;
    }
    let Some(memory) = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    else {
        return -1;
    };
    match memory.write(caller, ptr as usize, &bytes) {
        Ok(()) => bytes.len() as i32,
        Err(_) => -1,
    }
}

fn read_guest_request_request(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
) -> i32 {
    let bytes = match read_guest_bytes(caller, ptr, len, PLUGIN_REQUEST_MAX_REQUEST_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            caller.data_mut().output_error = Some(error);
            return -1;
        }
    };
    let record = caller.data().record.clone();
    let request_client = caller.data().request_client.clone();
    match execute_plugin_request_request(&record, request_client.as_ref(), &bytes) {
        Ok(response) => {
            caller.data_mut().request_response = response;
            caller.data().request_response.len() as i32
        }
        Err(error) => {
            caller.data_mut().output_error = Some(error.0);
            -1
        }
    }
}

fn read_guest_fs_request(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
    operation: PluginFsRuntimeOperation,
) -> i32 {
    let bytes = match read_guest_bytes(caller, ptr, len, PLUGIN_FS_MAX_REQUEST_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            caller.data_mut().output_error = Some(error);
            return -1;
        }
    };
    let record = caller.data().record.clone();
    match execute_plugin_fs_request(&record, operation, &bytes) {
        Ok(response) => {
            caller.data_mut().fs_response = response;
            caller.data().fs_response.len() as i32
        }
        Err(error) => {
            caller.data_mut().output_error = Some(error.message);
            -1
        }
    }
}

fn read_guest_bytes(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
    max_len: usize,
) -> Result<Vec<u8>, String> {
    if ptr < 0 || len < 0 {
        return Err("guest input pointer/length is invalid".into());
    }
    let len = len as usize;
    if len > max_len {
        return Err(format!("guest input exceeds {max_len} bytes"));
    }
    let Some(memory) = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    else {
        return Err("guest did not export linear memory".into());
    };
    let mut bytes = vec![0; len];
    memory
        .read(&*caller, ptr as usize, &mut bytes)
        .map_err(|_| "guest input memory range is invalid".to_string())?;
    Ok(bytes)
}

fn read_guest_output(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
) -> i32 {
    if ptr < 0 || len < 0 {
        caller.data_mut().output_error = Some("guest output pointer/length is invalid".into());
        return -1;
    }
    let len = len as usize;
    if len > PLUGIN_WASM_MAX_OUTPUT_BYTES {
        caller.data_mut().output_error = Some(format!(
            "guest output exceeds {} bytes",
            PLUGIN_WASM_MAX_OUTPUT_BYTES
        ));
        return -1;
    }
    let Some(memory) = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    else {
        caller.data_mut().output_error = Some("guest did not export linear memory".into());
        return -1;
    };
    let mut output = vec![0; len];
    if memory.read(&*caller, ptr as usize, &mut output).is_err() {
        caller.data_mut().output_error = Some("guest output memory range is invalid".into());
        return -1;
    }
    caller.data_mut().output = output;
    len as i32
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
    Ok(ToolOutput { summary, content })
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
    use std::sync::Mutex;
    use tempfile::TempDir;

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
                runtime: None,
                hooks: Vec::new(),
                tools,
                services: Vec::new(),
                ingresses: Vec::new(),
                permissions: permissions.clone(),
                request: Vec::new(),
            },
            enabled_surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some(parsed_identity.to_string()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                digest: Some("sha256:abc".to_string()),
                permissions,
                request: Vec::new(),
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
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Started);
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
            PluginInstanceLifecycleState::Ready
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
            PluginIngressEvent {
                kind: "test".into(),
                source: "unit".into(),
                payload: serde_json::json!({}),
            },
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
        assert_eq!(status.lifecycle, PluginInstanceLifecycleState::Started);
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
                PluginIngressEvent {
                    kind: "test".into(),
                    source: "unit".into(),
                    payload: serde_json::json!({ "hello": "world" }),
                },
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
            runtime: PluginInstanceRuntime::TestIngress { calls: 0 },
            lifecycle: PluginInstanceLifecycleState::Started,
            component_status: None,
            diagnostics: Vec::new(),
        })));

        let _tool = handle
            .handle_tool("shared_tool", br#"{"first":true}"#.to_vec())
            .unwrap();
        let report = handle
            .deliver_ingress(
                "shared_ingress",
                PluginIngressEvent {
                    kind: "test".into(),
                    source: "unit".into(),
                    payload: serde_json::json!({ "hello": "world" }),
                },
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

    fn wasm_tool_that_calls_request(request: &str) -> Vec<u8> {
        let output = br#"{"summary":"request ok","content":"ordinary tool result path"}"#;
        wat::parse_str(format!(
            r#"
            (module
              (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
              (import "yoi:request" "request" (func $request_request (param i32 i32) (result i32)))
              (import "yoi:request" "response_len" (func $request_response_len (result i32)))
              (memory (export "memory") 1)
              (data (i32.const 16) "{}")
              (data (i32.const 4096) "{}")
              (func (export "yoi_tool_call")
                (local $n i32)
                (local.set $n (call $request_request (i32.const 16) (i32.const {})))
                (if (i32.lt_s (local.get $n) (i32.const 0)) (then unreachable))
                (drop (call $request_response_len))
                (drop (call $output_write (i32.const 4096) (i32.const {})))
              )
            )
            "#,
            wat_bytes(request.as_bytes()),
            wat_bytes(output),
            request.len(),
            output.len()
        ))
        .expect("valid wat")
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

    fn runtime_record_with_fs_wasm(
        wasm: Vec<u8>,
        root: &Path,
        operations: Vec<PluginFsOperation>,
    ) -> (TempDir, ResolvedPluginRecord) {
        let (dir, mut record) = resolved_record_with_wasm(wasm);
        let fs_permission = PluginPermission::HostApi {
            api: PluginHostApi::Fs,
        };
        record.manifest.permissions.push(fs_permission.clone());
        record.grants.permissions.push(fs_permission);
        record.grants.fs.push(PluginFsGrant {
            root: root.to_string_lossy().into_owned(),
            operations,
        });
        (dir, record)
    }

    fn wasm_tool_that_calls_fs_read(request: &str) -> Vec<u8> {
        let output = br#"{"summary":"fs ok","content":"ordinary tool result path"}"#;
        wat::parse_str(format!(
            r#"
            (module
              (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
              (import "yoi:fs" "read" (func $fs_read (param i32 i32) (result i32)))
              (import "yoi:fs" "response_len" (func $fs_response_len (result i32)))
              (import "yoi:fs" "response_read" (func $fs_response_read (param i32 i32) (result i32)))
              (memory (export "memory") 1)
              (data (i32.const 16) "{}")
              (data (i32.const 4096) "{}")
              (func (export "yoi_tool_call")
                (local $n i32)
                (local.set $n (call $fs_read (i32.const 16) (i32.const {})))
                (if (i32.lt_s (local.get $n) (i32.const 0)) (then unreachable))
                (drop (call $fs_response_len))
                (drop (call $fs_response_read (i32.const 8192) (i32.const 4096)))
                (drop (call $output_write (i32.const 4096) (i32.const {})))
              )
            )
            "#,
            wat_bytes(request.as_bytes()),
            wat_bytes(output),
            request.len(),
            output.len()
        ))
        .expect("valid wat")
    }

    fn empty_wasm_tool() -> Vec<u8> {
        let output = br#"{"summary":"no network","content":"no request import"}"#;
        wat::parse_str(format!(
            r#"
            (module
              (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
              (memory (export "memory") 1)
              (data (i32.const 4096) "{}")
              (func (export "yoi_tool_call")
                (drop (call $output_write (i32.const 4096) (i32.const {})))
              )
            )
            "#,
            wat_bytes(output),
            output.len()
        ))
        .expect("valid wat")
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
    fn wasm_tool_can_call_granted_fs_read_host_api() {
        let root = TempDir::new().expect("temp root");
        fs::write(root.path().join("allowed.txt"), "hello fs").expect("write fixture");
        let (_dir, record) = runtime_record_with_fs_wasm(
            wasm_tool_that_calls_fs_read(&fs_request_json("allowed.txt")),
            root.path(),
            vec![PluginFsOperation::Read],
        );
        let output = run_plugin_wasm_tool(record, "PluginEcho".to_string(), Vec::new())
            .expect("tool output");
        assert_eq!(output.summary, "fs ok");
        assert_eq!(output.content.as_deref(), Some("ordinary tool result path"));
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
    fn wasm_tool_can_call_granted_request_host_api() {
        let (_dir, record) = runtime_record_with_request_wasm(wasm_tool_that_calls_request(
            &request_request_json("GET", "https://api.example.test/v1/data"),
        ));
        let client = Arc::new(MockRequestClient::default());
        let output = run_plugin_wasm_tool_with_request_client(
            record,
            "PluginEcho".to_string(),
            Vec::new(),
            client.clone(),
        )
        .expect("tool output");
        assert_eq!(client.call_count(), 1);
        assert_eq!(output.summary, "request ok");
        assert_eq!(output.content.as_deref(), Some("ordinary tool result path"));
    }

    #[test]
    fn missing_request_grant_denies_before_network() {
        let (_dir, mut record) = resolved_record_with_wasm(wasm_tool_that_calls_request(
            &request_request_json("GET", "https://api.example.test/v1/data"),
        ));
        record.manifest.permissions.push(PluginPermission::HostApi {
            api: PluginHostApi::Request,
        });
        let client = Arc::new(MockRequestClient::default());
        let error = run_plugin_wasm_tool_with_request_client(
            record,
            "PluginEcho".to_string(),
            Vec::new(),
            client.clone(),
        )
        .expect_err("grant denied");
        assert_eq!(client.call_count(), 0);
        assert!(error.bounded_message().contains("host_api.request"));
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
    fn no_network_without_request_import() {
        let (_dir, record) = runtime_record_with_request_wasm(empty_wasm_tool());
        let client = Arc::new(MockRequestClient::default());
        let output = run_plugin_wasm_tool_with_request_client(
            record,
            "PluginEcho".to_string(),
            Vec::new(),
            client.clone(),
        )
        .expect("tool output");
        assert_eq!(client.call_count(), 0);
        assert_eq!(output.summary, "no network");
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

        let error = run_plugin_wasm_tool(record, "PluginSearch".into(), br#"{}"#.to_vec())
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
    fn future_host_api_imports_are_permission_checked_before_unimplemented_boundary() {
        let (_dir, mut record) = resolved_record_with_wasm(request_import_module());
        let error = run_plugin_wasm_tool(record.clone(), "PluginEcho".into(), br#"{}"#.to_vec())
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
        let error = run_plugin_wasm_tool(record.clone(), "PluginEcho".into(), br#"{}"#.to_vec())
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
        let error = run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec())
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

    #[tokio::test]
    async fn registered_plugin_tool_executes_wasm_and_returns_normal_tool_result() {
        let (_dir, record) = resolved_record_with_wasm(input_reaches_guest_module());
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginEcho".into(),
            origin,
        };

        let output = tool
            .execute(r#"{"x":1}"#, ToolExecutionContext::default())
            .await
            .unwrap();
        assert_eq!(output.summary, "input reached");
        assert_eq!(output.content.as_deref(), Some("ordinary tool result path"));

        let result = llm_worker::tool::ToolResult::from_output("call-1", output);
        assert_eq!(result.summary, "input reached");
        assert!(
            result
                .content
                .unwrap()
                .contains("ordinary tool result path")
        );
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

    #[tokio::test]
    async fn malformed_input_json_fails_before_wasm_execution() {
        let (_dir, record) = resolved_record_with_wasm(input_reaches_guest_module());
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginEcho".into(),
            origin,
        };

        let error = tool
            .execute("not json", ToolExecutionContext::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("input is not valid JSON"));
    }

    #[tokio::test]
    async fn malformed_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(output_module(b"not json"));
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("not valid JSON"));
    }

    #[tokio::test]
    async fn schema_mismatch_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(output_module(br#"{"summary":1}"#));
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("summary must be a string"));
    }

    #[tokio::test]
    async fn oversize_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(oversize_output_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("exceeds"));
    }

    #[tokio::test]
    async fn nonterminating_execution_fails_closed_with_fuel_boundary() {
        let (_dir, record) = resolved_record_with_wasm(nonterminating_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        let message = error.bounded_message();
        assert!(
            message.contains("Execution")
                || message.contains("fuel")
                || message.contains("execution"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn missing_runtime_module_returns_safe_bounded_tool_error() {
        let record = record_with_missing_package_runtime();
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginSearch".into(),
            origin,
        };

        let error = tool
            .execute("{}", ToolExecutionContext::default())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("failed closed"));
        assert!(message.contains("metadata could not be read"));
        assert!(message.len() < 900);
        assert!(message.contains("project:example"));
    }

    #[tokio::test]
    async fn ambient_wasi_fs_network_env_imports_are_unavailable() {
        let (_dir, record) = resolved_record_with_wasm(wasi_import_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        let message = error.bounded_message();
        assert!(message.contains("unsupported import module"), "{message}");
        assert!(message.contains("wasi_snapshot_preview1"), "{message}");
    }

    fn record_with_missing_package_runtime() -> ResolvedPluginRecord {
        let mut record = record(vec![tool("PluginSearch")]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_WASM_KIND.into(),
            entry: Some("plugin.wasm".into()),
            abi: Some(PLUGIN_RUNTIME_WASM_ABI.into()),
            component: None,
            world: None,
        });
        record
    }

    fn runtime_record_with_request_wasm(wasm: Vec<u8>) -> (TempDir, ResolvedPluginRecord) {
        let (dir, mut record) = resolved_record_with_wasm(wasm);
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
        (dir, record)
    }

    fn resolved_record_with_wasm(wasm: Vec<u8>) -> (TempDir, ResolvedPluginRecord) {
        let dir = TempDir::new().unwrap();
        let package_dir = dir.path().join(".yoi/plugins");
        fs::create_dir_all(&package_dir).unwrap();
        let package_path = package_dir.join("example.yoi-plugin");
        write_plugin_package(&package_path, &wasm);
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
            fs: Vec::new(),
        };
        (dir, record)
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
            fs: Vec::new(),
        };
        (dir, record)
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

    fn write_plugin_package(path: &Path, wasm: &[u8]) {
        let manifest = br#"schema_version = 1
id = "example"
name = "Example"
version = "1.0.0"
description = "Example plugin"
surfaces = ["tool"]

[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"

[[permissions]]
kind = "surface"
surface = "tool"

[[permissions]]
kind = "tool"
name = "PluginEcho"

[[tools]]
name = "PluginEcho"
description = "Echo plugin tool"
input_schema = { type = "object", additionalProperties = true }
"#;
        write_stored_zip(
            path,
            &[("plugin.toml", manifest.as_slice()), ("plugin.wasm", wasm)],
        );
    }

    fn input_reaches_guest_module() -> Vec<u8> {
        let ok = br#"{"summary":"input reached","content":"ordinary tool result path"}"#;
        let bad = br#"{"summary":"input missing"}"#;
        let wat = format!(
            r#"(module
                (import "yoi:tool" "input_len" (func $input_len (result i32)))
                (import "yoi:tool" "input_read" (func $input_read (param i32 i32) (result i32)))
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{}")
                (data (i32.const 128) "{}")
                (func (export "yoi_tool_call")
                  (local $len i32)
                  (local.set $len (call $input_len))
                  (if (i32.eq (local.get $len) (i32.const 7))
                    (then
                      (drop (call $input_read (i32.const 512) (local.get $len)))
                      (if (i32.eq (i32.load8_u (i32.const 517)) (i32.const 49))
                        (then (drop (call $output_write (i32.const 0) (i32.const {}))))
                        (else (drop (call $output_write (i32.const 128) (i32.const {}))))
                      )
                    )
                    (else (drop (call $output_write (i32.const 128) (i32.const {}))))
                  )
                )
              )"#,
            wat_bytes(ok),
            wat_bytes(bad),
            ok.len(),
            bad.len(),
            bad.len()
        );
        wat::parse_str(wat).unwrap()
    }

    fn output_module(output: &[u8]) -> Vec<u8> {
        let wat = format!(
            r#"(module
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{}")
                (func (export "yoi_tool_call")
                  (drop (call $output_write (i32.const 0) (i32.const {})))
                )
              )"#,
            wat_bytes(output),
            output.len()
        );
        wat::parse_str(wat).unwrap()
    }

    fn oversize_output_module() -> Vec<u8> {
        let wat = format!(
            r#"(module
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 2)
                (func (export "yoi_tool_call")
                  (drop (call $output_write (i32.const 0) (i32.const {})))
                )
              )"#,
            PLUGIN_WASM_MAX_OUTPUT_BYTES + 1
        );
        wat::parse_str(wat).unwrap()
    }

    fn nonterminating_module() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "yoi_tool_call")
                  (local $remaining i32)
                  (local.set $remaining (i32.const 100000000))
                  (loop $again
                    (local.set $remaining (i32.sub (local.get $remaining) (i32.const 1)))
                    (br_if $again (local.get $remaining))
                  )
                )
              )"#,
        )
        .unwrap()
    }

    fn wasi_import_module() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (import "wasi_snapshot_preview1" "fd_write" (func $fd_write))
                (memory (export "memory") 1)
                (func (export "yoi_tool_call"))
              )"#,
        )
        .unwrap()
    }

    fn request_import_module() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (import "yoi:request" "request" (func $request))
                (memory (export "memory") 1)
                (func (export "yoi_tool_call"))
              )"#,
        )
        .unwrap()
    }

    fn wat_bytes(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!(r#"\{:02x}"#, byte))
            .collect()
    }

    #[test]
    fn static_inspection_does_not_read_or_execute_package() {
        let mut record = record(vec![tool("Echo")]);
        record.package_path = std::path::PathBuf::from("/no/such/plugin.wasm");
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: PLUGIN_RUNTIME_WASM_KIND.to_string(),
            entry: Some("plugin.wasm".to_string()),
            abi: Some(PLUGIN_RUNTIME_WASM_ABI.to_string()),
            component: None,
            world: None,
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
            kind: PLUGIN_RUNTIME_WASM_KIND.to_string(),
            entry: Some("plugin.wasm".to_string()),
            abi: Some(PLUGIN_RUNTIME_WASM_ABI.to_string()),
            component: None,
            world: None,
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
            kind: PLUGIN_RUNTIME_WASM_KIND.to_string(),
            entry: Some("plugin.wasm".to_string()),
            abi: Some(PLUGIN_RUNTIME_WASM_ABI.to_string()),
            component: None,
            world: None,
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
            kind: PLUGIN_RUNTIME_WASM_KIND.to_string(),
            entry: Some("plugin.wasm".to_string()),
            abi: Some(PLUGIN_RUNTIME_WASM_ABI.to_string()),
            component: None,
            world: None,
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
}
