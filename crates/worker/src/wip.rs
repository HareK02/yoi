//! Explicit-opt-in Web Interface Protocol (WIP) Worker surface.
//!
//! The adapter intentionally keeps authority in the existing Worker host. In WIP
//! mode ordinary tools are mounted below `/tools`, removed from the model-visible
//! tool list, and invoked through three stateful discover/inspect/call tools. The
//! published WIP crates own protocol validation, HTTP encoding, Known Space, and
//! operation lifecycle state; this module owns only the in-process transport and
//! the compatibility projection into existing async `Tool` implementations.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agen::Engine;
use agen::llm_client::client::LlmClient;
use agen::state::Mutable;
use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use http::{Request, Response};
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wip_client::{
    CallOutcome, Client, ClientLimits, Completion, DispatchedTransportFailure, ObservationState,
    RequestId, SecurityContext, SessionId,
};
use wip_http::{
    Endpoint, Limits, Route, decode_call_operation_metadata, decode_call_operation_request,
    decode_fetch_interface_request, decode_observe_request, encode_call_operation_response,
    encode_fetch_interface_response, encode_observe_response, encode_protocol_error_response,
};
use wip_protocol::{
    CallOperationRequest, CallOperationResponse, Documentation, FetchInterfaceResponse,
    INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, ObjectObservation, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ProtocolInteraction, ReturnDeclaration,
    TypeExpr, Value,
};

use crate::permission::permission_action_for;

const WIP_ENDPOINT: &str = "https://worker.wip.invalid/v1/";
const WIP_ROOT: &str = "/";
const WIP_TOOLS_ROOT: &str = "/tools";
const MAX_AUDIT_RECORDS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipProjectionKind {
    Compatibility,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipMountDisposition {
    Mounted,
    ReplacedCompatibility,
    NativeAlreadySelected,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WipMountError {
    #[error("invalid WIP route `{route}`: {message}")]
    InvalidRoute { route: String, message: String },
    #[error("WIP route `{route}` is already owned by capability `{existing}`")]
    RouteCollision { route: String, existing: String },
    #[error("WIP interface `{interface}` has conflicting descriptors")]
    InterfaceCollision { interface: String },
    #[error("invalid WIP projection for `{route}`: {message}")]
    InvalidProjection { route: String, message: String },
}

/// Runtime-neutral call context supplied to native WIP projections.
#[derive(Debug, Clone)]
pub struct WipCallContext {
    pub execution: ToolExecutionContext,
    pub security_context: String,
}

/// Successful operation value. Compatibility projections can retain an ordinary
/// ToolOutput so images and pruning semantics remain on the existing Engine path.
pub struct WipOperationOutput {
    pub value: Value,
    tool_output: Option<ToolOutput>,
}

impl WipOperationOutput {
    pub fn native(value: Value) -> Self {
        Self {
            value,
            tool_output: None,
        }
    }

    fn compatibility(value: Value, output: ToolOutput) -> Self {
        Self {
            value,
            tool_output: Some(output),
        }
    }
}

pub enum WipOperationError {
    Protocol(ProtocolError),
    Cancelled(ToolOutput),
    Interrupted(ToolOutput),
    OutcomeUnknown(String),
}

/// Async extension point used by future native Feature projections. Host-side
/// routing and validators are resolved before this trait is called.
#[async_trait]
pub trait WipOperationHandler: Send + Sync {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError>;

    async fn cancel(&self, _context: &ToolExecutionContext) -> Result<(), ToolError> {
        Ok(())
    }
}

/// One route contribution. Native projections replace compatibility projections
/// only when both declare the same semantic capability; unrelated collisions fail.
#[derive(Clone)]
pub struct WipProjection {
    pub route: String,
    pub capability: String,
    pub kind: WipProjectionKind,
    pub object: Object,
    pub interface: String,
    pub descriptor: InterfaceDescriptor,
    pub interface_validator: Option<Vec<u8>>,
    pub handler: Arc<dyn WipOperationHandler>,
}

/// One dynamically resolved direct-child object beneath a Feature-owned
/// collection route. The Host retains route allocation and descriptor authority;
/// the Feature only binds a validated child segment to an object and handler.
pub struct WipDynamicItem {
    pub object: Object,
    pub handler: Arc<dyn WipOperationHandler>,
}

pub trait WipDynamicItemResolver: Send + Sync {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem>;
}

pub struct WipDynamicMount {
    pub collection_route: String,
    pub capability: String,
    pub interface: String,
    pub descriptor: InterfaceDescriptor,
    pub interface_validator: Option<Vec<u8>>,
    pub resolver: Arc<dyn WipDynamicItemResolver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipFeatureRoute {
    root: String,
}

impl WipFeatureRoute {
    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn child(&self, segment: &str) -> Result<String, WipMountError> {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('/')
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(WipMountError::InvalidRoute {
                route: format!("{}/{}", self.root, segment),
                message: "Feature child must be one canonical path segment".into(),
            });
        }
        Ok(format!("{}/{}", self.root, segment))
    }
}

struct MountedProjection {
    projection: WipProjection,
}

#[derive(Default)]
pub struct WipMountRegistry {
    mounts: BTreeMap<String, MountedProjection>,
    dynamic_mounts: Vec<WipDynamicMount>,
    feature_routes: BTreeSet<String>,
    replaced_compatibility_capabilities: BTreeMap<String, String>,
}

impl WipMountRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the Host namespace owned by one enabled Feature. Features join
    /// checked relative segments to this route instead of selecting global paths.
    pub fn allocate_feature_route(
        &mut self,
        feature: &str,
    ) -> Result<WipFeatureRoute, WipMountError> {
        if feature.is_empty()
            || !feature
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(WipMountError::InvalidRoute {
                route: format!("/features/{feature}"),
                message: "Feature name must use lowercase ASCII letters, digits, or '-'".into(),
            });
        }
        let root = format!("/features/{feature}");
        if !self.feature_routes.insert(root.clone()) {
            return Err(WipMountError::RouteCollision {
                route: root,
                existing: format!("feature:{feature}"),
            });
        }
        Ok(WipFeatureRoute { root })
    }

    pub fn mount(
        &mut self,
        projection: WipProjection,
    ) -> Result<WipMountDisposition, WipMountError> {
        validate_projection(&projection)?;
        if self.dynamic_mounts.iter().any(|dynamic| {
            projection
                .route
                .starts_with(&format!("{}/", dynamic.collection_route))
        }) {
            return Err(WipMountError::RouteCollision {
                route: projection.route,
                existing: "dynamic Feature collection".into(),
            });
        }
        if let Some(existing) = self.mounts.get(&projection.route) {
            if existing.projection.capability != projection.capability {
                return Err(WipMountError::RouteCollision {
                    route: projection.route,
                    existing: existing.projection.capability.clone(),
                });
            }
            match (existing.projection.kind, projection.kind) {
                (WipProjectionKind::Compatibility, WipProjectionKind::Native) => {
                    self.ensure_interface_available(&projection, Some(&existing.projection.route))?;
                    self.mounts
                        .insert(projection.route.clone(), MountedProjection { projection });
                    return Ok(WipMountDisposition::ReplacedCompatibility);
                }
                (WipProjectionKind::Native, WipProjectionKind::Compatibility) => {
                    return Ok(WipMountDisposition::NativeAlreadySelected);
                }
                _ => {
                    return Err(WipMountError::RouteCollision {
                        route: projection.route,
                        existing: existing.projection.capability.clone(),
                    });
                }
            }
        }
        self.ensure_interface_available(&projection, None)?;
        let route = projection.route.clone();
        self.mounts.insert(route, MountedProjection { projection });
        Ok(WipMountDisposition::Mounted)
    }

    /// Mount a one-segment item family below an already mounted collection.
    /// Dynamic resolution never creates authority: the collection route must be
    /// native and every resolved child is validated before it is exposed.
    pub fn mount_dynamic(&mut self, mount: WipDynamicMount) -> Result<(), WipMountError> {
        wip_protocol::validate_path(&mount.collection_route).map_err(|error| {
            WipMountError::InvalidRoute {
                route: mount.collection_route.clone(),
                message: error.to_string(),
            }
        })?;
        let Some(collection) = self.mounts.get(&mount.collection_route) else {
            return Err(WipMountError::InvalidProjection {
                route: mount.collection_route,
                message: "dynamic item mount requires an existing collection projection".into(),
            });
        };
        if collection.projection.kind != WipProjectionKind::Native {
            return Err(WipMountError::InvalidProjection {
                route: mount.collection_route,
                message: "dynamic item mount requires a native collection projection".into(),
            });
        }
        mount
            .descriptor
            .validate()
            .map_err(|error| WipMountError::InvalidProjection {
                route: mount.collection_route.clone(),
                message: error.to_string(),
            })?;
        if let Some(existing) = self.dynamic_mounts.iter().find(|existing| {
            existing.collection_route == mount.collection_route
                || mount
                    .collection_route
                    .starts_with(&format!("{}/", existing.collection_route))
                || existing
                    .collection_route
                    .starts_with(&format!("{}/", mount.collection_route))
        }) {
            return Err(WipMountError::RouteCollision {
                route: mount.collection_route,
                existing: existing.capability.clone(),
            });
        }
        if let Some((_, existing)) = self
            .mounts
            .iter()
            .find(|(route, _)| route.starts_with(&format!("{}/", mount.collection_route)))
        {
            return Err(WipMountError::RouteCollision {
                route: mount.collection_route,
                existing: existing.projection.capability.clone(),
            });
        }
        if self.interface_conflicts(
            &mount.interface,
            &mount.descriptor,
            mount.interface_validator.as_deref(),
            None,
        ) {
            return Err(WipMountError::InterfaceCollision {
                interface: mount.interface,
            });
        }
        self.dynamic_mounts.push(mount);
        Ok(())
    }

    /// Declare ordinary tool capabilities fully represented by native mounts.
    /// Claimed compatibility projections are removed from WIP mode while the
    /// normal Tool-mode registration remains unchanged.
    pub fn replace_compatibility_tools<'a>(
        &mut self,
        owner_route: &str,
        tool_names: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), WipMountError> {
        if !self.mounts.contains_key(owner_route)
            && !self
                .dynamic_mounts
                .iter()
                .any(|mount| mount.collection_route == owner_route)
        {
            return Err(WipMountError::InvalidProjection {
                route: owner_route.to_string(),
                message: "compatibility replacement owner is not mounted".into(),
            });
        }
        let capabilities = tool_names
            .into_iter()
            .map(|name| format!("tool:{name}"))
            .collect::<Vec<_>>();
        for capability in &capabilities {
            if let Some(existing) = self
                .replaced_compatibility_capabilities
                .get(capability)
                .filter(|existing| existing.as_str() != owner_route)
            {
                return Err(WipMountError::RouteCollision {
                    route: owner_route.to_string(),
                    existing: existing.clone(),
                });
            }
        }
        for capability in capabilities {
            self.replaced_compatibility_capabilities
                .insert(capability, owner_route.to_string());
        }
        Ok(())
    }

    fn replaces_compatibility(&self, capability: &str) -> bool {
        self.replaced_compatibility_capabilities
            .contains_key(capability)
    }

    fn interface_conflicts(
        &self,
        interface: &str,
        descriptor: &InterfaceDescriptor,
        validator: Option<&[u8]>,
        replacing_route: Option<&str>,
    ) -> bool {
        self.mounts.iter().any(|(route, mounted)| {
            Some(route.as_str()) != replacing_route
                && mounted.projection.interface == interface
                && (mounted.projection.descriptor != *descriptor
                    || mounted.projection.interface_validator.as_deref() != validator)
        }) || self.dynamic_mounts.iter().any(|mounted| {
            mounted.interface == interface
                && (mounted.descriptor != *descriptor
                    || mounted.interface_validator.as_deref() != validator)
        })
    }

    fn ensure_interface_available(
        &self,
        candidate: &WipProjection,
        replacing_route: Option<&str>,
    ) -> Result<(), WipMountError> {
        if self.interface_conflicts(
            &candidate.interface,
            &candidate.descriptor,
            candidate.interface_validator.as_deref(),
            replacing_route,
        ) {
            return Err(WipMountError::InterfaceCollision {
                interface: candidate.interface.clone(),
            });
        }
        Ok(())
    }

    pub fn routes(&self) -> impl Iterator<Item = &str> {
        self.mounts.keys().map(String::as_str)
    }
}

fn validate_projection(projection: &WipProjection) -> Result<(), WipMountError> {
    wip_protocol::validate_path(&projection.route).map_err(|error| {
        WipMountError::InvalidRoute {
            route: projection.route.clone(),
            message: error.to_string(),
        }
    })?;
    if projection.route == WIP_ROOT {
        return Err(WipMountError::InvalidRoute {
            route: projection.route.clone(),
            message: "the Host reserves the Worldspace root".into(),
        });
    }
    projection
        .object
        .validate_at_path(&projection.route)
        .map_err(|error| WipMountError::InvalidProjection {
            route: projection.route.clone(),
            message: error.to_string(),
        })?;
    if projection.object.interfaces != [projection.interface.clone()] {
        return Err(WipMountError::InvalidProjection {
            route: projection.route.clone(),
            message: "projected object must expose exactly its declared interface".into(),
        });
    }
    projection
        .descriptor
        .validate()
        .map_err(|error| WipMountError::InvalidProjection {
            route: projection.route.clone(),
            message: error.to_string(),
        })
}

struct WipHost {
    registry: WipMountRegistry,
    generation: Vec<u8>,
}

impl WipHost {
    fn new(registry: WipMountRegistry) -> Self {
        let mut digest = Sha256::new();
        for route in registry.routes() {
            digest.update(route.as_bytes());
            digest.update([0]);
        }
        Self {
            registry,
            generation: digest.finalize().to_vec(),
        }
    }

    fn descriptor(&self, reference: &str) -> Option<(&InterfaceDescriptor, Option<&[u8]>)> {
        self.registry
            .mounts
            .values()
            .find(|mounted| mounted.projection.interface == reference)
            .map(|mounted| {
                (
                    &mounted.projection.descriptor,
                    mounted.projection.interface_validator.as_deref(),
                )
            })
            .or_else(|| {
                self.registry
                    .dynamic_mounts
                    .iter()
                    .find(|mounted| mounted.interface == reference)
                    .map(|mounted| (&mounted.descriptor, mounted.interface_validator.as_deref()))
            })
    }

    fn projection(&self, path: &str) -> Option<WipProjection> {
        if let Some(mounted) = self.registry.mounts.get(path) {
            return Some(mounted.projection.clone());
        }
        self.registry.dynamic_mounts.iter().find_map(|mounted| {
            let item_reference = path.strip_prefix(&format!("{}/", mounted.collection_route))?;
            if item_reference.is_empty() || item_reference.contains('/') {
                return None;
            }
            let item = mounted.resolver.resolve(item_reference)?;
            let projection = WipProjection {
                route: path.to_string(),
                capability: mounted.capability.clone(),
                kind: WipProjectionKind::Native,
                object: item.object,
                interface: mounted.interface.clone(),
                descriptor: mounted.descriptor.clone(),
                interface_validator: mounted.interface_validator.clone(),
                handler: item.handler,
            };
            validate_projection(&projection).ok()?;
            Some(projection)
        })
    }

    fn object_at(&self, path: &str) -> Option<Object> {
        if path == WIP_ROOT || self.is_namespace(path) {
            let name = if path == WIP_ROOT {
                String::new()
            } else {
                path.rsplit('/').next().unwrap_or_default().to_string()
            };
            return Some(Object {
                name,
                description: Some("WIP Worldspace namespace".into()),
                interfaces: Vec::new(),
                r#ref: Some(format!("namespace:{path}")),
                validator: Some(self.generation.clone()),
            });
        }
        self.projection(path)
            .map(|projection| projection.object.clone())
    }

    fn is_namespace(&self, path: &str) -> bool {
        let prefix = if path == WIP_ROOT {
            "/".to_string()
        } else {
            format!("{path}/")
        };
        self.registry
            .mounts
            .keys()
            .any(|route| route.starts_with(&prefix))
    }

    fn children(&self, path: &str) -> Vec<String> {
        let prefix = if path == WIP_ROOT {
            "/".to_string()
        } else {
            format!("{path}/")
        };
        let mut children = BTreeSet::new();
        for route in self.registry.mounts.keys() {
            let Some(rest) = route.strip_prefix(&prefix) else {
                continue;
            };
            let Some(segment) = rest.split('/').next() else {
                continue;
            };
            if segment.is_empty() {
                continue;
            }
            let child = if path == WIP_ROOT {
                format!("/{segment}")
            } else {
                format!("{path}/{segment}")
            };
            children.insert(child);
        }
        children.into_iter().collect()
    }

    fn observe(&self, path: &str, depth: u32) -> Result<ObjectObservation, ProtocolError> {
        let object = self.object_at(path).ok_or_else(|| {
            protocol_error(ProtocolErrorCode::NotFound, "target path is not published")
        })?;
        let children = if depth == 0 {
            None
        } else {
            let mut values = Vec::new();
            for child in self.children(path) {
                values.push(self.observe(&child, depth - 1)?);
            }
            Some(values)
        };
        Ok(ObjectObservation { object, children })
    }

    fn fetch_interface(&self, reference: &str) -> Result<FetchInterfaceResponse, ProtocolError> {
        let (descriptor, validator) = self.descriptor(reference).ok_or_else(|| {
            protocol_error(
                ProtocolErrorCode::InterfaceNotFound,
                "interface is not published",
            )
        })?;
        Ok(FetchInterfaceResponse {
            interface: reference.to_string(),
            descriptor: descriptor.clone(),
            validator: validator.map(<[u8]>::to_vec),
        })
    }

    async fn call(
        &self,
        request: CallOperationRequest,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let projection = self.projection(&request.target.path).ok_or_else(|| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::NotFound,
                "target path is not published",
            ))
        })?;
        let expected_object_validator = projection.object.validator.as_deref();
        match (
            expected_object_validator,
            request.target.validator.as_deref(),
        ) {
            (Some(_), None) => {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::ValidatorRequired,
                    "object validator is required",
                )));
            }
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::ValidatorMismatch,
                    "object validator is stale",
                )));
            }
            _ => {}
        }
        if request.interface.reference != projection.interface {
            return Err(WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InterfaceMismatch,
                "interface is not a member of the target object",
            )));
        }
        match (
            projection.interface_validator.as_deref(),
            request.interface.validator.as_deref(),
        ) {
            (Some(_), None) => {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::InterfaceValidatorRequired,
                    "interface validator is required",
                )));
            }
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::InterfaceValidatorMismatch,
                    "interface validator is stale",
                )));
            }
            _ => {}
        }
        if !projection
            .descriptor
            .operations
            .iter()
            .any(|operation| operation.name == request.operation)
        {
            return Err(WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::OperationNotFound,
                "operation is not published by the selected interface",
            )));
        }
        projection
            .descriptor
            .validate_call(&request)
            .map_err(|error| {
                WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::InvalidArguments,
                    error.to_string(),
                ))
            })?;
        projection
            .handler
            .call(&request.operation, &request.arguments, context)
            .await
    }

    async fn cancel(&self, path: &str, context: &ToolExecutionContext) -> Result<(), ToolError> {
        let projection = self.projection(path).ok_or_else(|| {
            ToolError::InvalidArgument("WIP cancellation target is no longer published".into())
        })?;
        projection.handler.cancel(context).await
    }
}

fn protocol_error(code: ProtocolErrorCode, message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code,
        message: message.into(),
    }
}

struct CompatibilityHandler {
    name: String,
    schema: Json,
    tool: Arc<dyn Tool>,
    permissions: Option<ToolPermissionConfig>,
}

#[async_trait]
impl WipOperationHandler for CompatibilityHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if operation != "call" {
            return Err(WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::OperationNotFound,
                "operation is not published",
            )));
        }
        let Some(input) = arguments.get("input") else {
            return Err(WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidArguments,
                "compatibility call requires `input`",
            )));
        };
        let input = wip_to_json(input).map_err(|message| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidArguments,
                message,
            ))
        })?;
        let validator = jsonschema::validator_for(&self.schema).map_err(|error| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::Internal,
                format!("stored tool schema is invalid: {error}"),
            ))
        })?;
        if let Err(error) = validator.validate(&input) {
            return Err(WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidArguments,
                format!("tool input violates its original JSON Schema: {error}"),
            )));
        }
        if let Some(permissions) = &self.permissions {
            match permission_action_for(permissions, &self.name, &input) {
                ToolPermissionAction::Allow => {}
                ToolPermissionAction::Deny => {
                    return Err(WipOperationError::Protocol(protocol_error(
                        ProtocolErrorCode::PermissionDenied,
                        format!("permission denied for projected tool `{}`", self.name),
                    )));
                }
                ToolPermissionAction::Ask => {
                    return Err(WipOperationError::Protocol(protocol_error(
                        ProtocolErrorCode::PermissionDenied,
                        format!(
                            "permission approval is unavailable for projected tool `{}`; denied fail-closed",
                            self.name
                        ),
                    )));
                }
            }
        }
        let input_json = serde_json::to_string(&input).map_err(|error| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidArguments,
                error.to_string(),
            ))
        })?;
        match self.tool.execute(&input_json, context.execution).await {
            Ok(output) => {
                let value = tool_output_value(&output);
                Ok(WipOperationOutput::compatibility(value, output))
            }
            Err(ToolError::InvalidArgument(message)) => Err(WipOperationError::Protocol(
                protocol_error(ProtocolErrorCode::InvalidArguments, message),
            )),
            Err(ToolError::Cancelled(output)) => Err(WipOperationError::Cancelled(output)),
            Err(ToolError::Interrupted(output)) => Err(WipOperationError::Interrupted(output)),
            Err(ToolError::StructuredConflict { code, message }) => {
                Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::InvalidArguments,
                    format!("{code}: {message}"),
                )))
            }
            Err(ToolError::ExecutionFailed(message) | ToolError::Internal(message)) => {
                Err(WipOperationError::OutcomeUnknown(message))
            }
        }
    }

    async fn cancel(&self, context: &ToolExecutionContext) -> Result<(), ToolError> {
        self.tool.cancel_execution(context).await
    }
}

fn compatibility_projection(
    meta: ToolMeta,
    tool: Arc<dyn Tool>,
    permissions: Option<ToolPermissionConfig>,
) -> Result<WipProjection, WipMountError> {
    jsonschema::validator_for(&meta.input_schema).map_err(|error| {
        WipMountError::InvalidProjection {
            route: format!("{WIP_TOOLS_ROOT}/{}", meta.name),
            message: format!("tool JSON Schema cannot be retained: {error}"),
        }
    })?;
    let route = format!("{WIP_TOOLS_ROOT}/{}", meta.name);
    let interface = format!("yoi.tool/{}/v1", meta.name);
    let schema = serde_json::to_string(&meta.input_schema).map_err(|error| {
        WipMountError::InvalidProjection {
            route: route.clone(),
            message: error.to_string(),
        }
    })?;
    let mut digest = Sha256::new();
    digest.update(meta.name.as_bytes());
    digest.update([0]);
    digest.update(schema.as_bytes());
    let validator = digest.finalize().to_vec();
    let descriptor = InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: meta.description.clone(),
            details: Some(format!(
                "Compatibility projection of tool `{}`. The `input` parameter preserves and is validated against this exact JSON Schema before the original tool runs:\n{}",
                meta.name, schema
            )),
        }),
        types: Vec::new(),
        operations: vec![OperationDeclaration {
            name: "call".into(),
            documentation: Some(Documentation {
                summary: format!("Invoke {} through its existing authority boundary", meta.name),
                details: Some(
                    "Pass the original tool argument object as `input`; IDs remain ordinary values and are never interpreted as Worldspace object paths."
                        .into(),
                ),
            }),
            parameters: vec![ParameterDeclaration {
                name: "input".into(),
                required: true,
                documentation: Some(Documentation {
                    summary: "Original JSON tool arguments".into(),
                    details: Some(schema),
                }),
                r#type: TypeExpr::Json,
            }],
            returns: ReturnDeclaration {
                documentation: Some(Documentation {
                    summary: "Existing ToolOutput projected as JSON".into(),
                    details: None,
                }),
                r#type: TypeExpr::Json,
            },
        }],
    };
    let handler = Arc::new(CompatibilityHandler {
        name: meta.name.clone(),
        schema: meta.input_schema,
        tool,
        permissions,
    });
    Ok(WipProjection {
        route: route.clone(),
        capability: format!("tool:{}", meta.name),
        kind: WipProjectionKind::Compatibility,
        object: Object {
            name: meta.name.clone(),
            description: Some(meta.description),
            interfaces: vec![interface.clone()],
            r#ref: Some(format!("tool:{}", meta.name)),
            validator: Some(validator.clone()),
        },
        interface,
        descriptor,
        interface_validator: Some(validator),
        handler,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipAuditOutcome {
    Success,
    Rejected,
    Cancelled,
    Interrupted,
    Disconnected,
    OutcomeUnknown,
}

#[derive(Debug, Clone)]
pub struct WipAuditRecord {
    pub request_id: u64,
    pub path: String,
    pub operation: String,
    pub outcome: WipAuditOutcome,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WipMetricsSnapshot {
    pub ordinary_schema_bytes: u64,
    pub wip_schema_bytes: u64,
    pub discover_round_trips: u64,
    pub inspect_round_trips: u64,
    pub operation_round_trips: u64,
    pub operation_successes: u64,
}

#[derive(Default)]
struct WipMetrics {
    ordinary_schema_bytes: AtomicU64,
    wip_schema_bytes: AtomicU64,
    discover_round_trips: AtomicU64,
    inspect_round_trips: AtomicU64,
    operation_round_trips: AtomicU64,
    operation_successes: AtomicU64,
}

impl WipMetrics {
    fn snapshot(&self) -> WipMetricsSnapshot {
        WipMetricsSnapshot {
            ordinary_schema_bytes: self.ordinary_schema_bytes.load(Ordering::Relaxed),
            wip_schema_bytes: self.wip_schema_bytes.load(Ordering::Relaxed),
            discover_round_trips: self.discover_round_trips.load(Ordering::Relaxed),
            inspect_round_trips: self.inspect_round_trips.load(Ordering::Relaxed),
            operation_round_trips: self.operation_round_trips.load(Ordering::Relaxed),
            operation_successes: self.operation_successes.load(Ordering::Relaxed),
        }
    }
}

struct ClientState {
    client: Client,
    session: SessionId,
}

pub struct WipRuntime {
    endpoint: Endpoint,
    wire_limits: Limits,
    client_limits: ClientLimits,
    security_context: SecurityContext,
    state: Arc<Mutex<ClientState>>,
    host: Arc<WipHost>,
    active: Arc<Mutex<HashMap<String, String>>>,
    audit: Arc<Mutex<VecDeque<WipAuditRecord>>>,
    metrics: Arc<WipMetrics>,
}

impl WipRuntime {
    fn new(
        host: WipHost,
        security_context: SecurityContext,
        ordinary_schema_bytes: u64,
    ) -> Result<Self, String> {
        let endpoint = Endpoint::parse(WIP_ENDPOINT).map_err(|error| error.to_string())?;
        let client_limits = ClientLimits::new(4, 2048, 2048, 64, MAX_AUDIT_RECORDS)
            .map_err(|error| error.to_string())?;
        let wire_limits = Limits::new(2 * 1024 * 1024, 2 * 1024 * 1024, 128)
            .map_err(|error| error.to_string())?;
        let mut client = Client::new(client_limits, wire_limits);
        let session = client
            .open_session(WIP_ENDPOINT, security_context.clone())
            .map_err(|error| error.to_string())?;
        let metrics = Arc::new(WipMetrics::default());
        metrics
            .ordinary_schema_bytes
            .store(ordinary_schema_bytes, Ordering::Relaxed);
        Ok(Self {
            endpoint,
            wire_limits,
            client_limits,
            security_context,
            state: Arc::new(Mutex::new(ClientState { client, session })),
            host: Arc::new(host),
            active: Arc::new(Mutex::new(HashMap::new())),
            audit: Arc::new(Mutex::new(VecDeque::new())),
            metrics,
        })
    }

    pub fn metrics(&self) -> WipMetricsSnapshot {
        self.metrics.snapshot()
    }

    pub fn audit(&self) -> Vec<WipAuditRecord> {
        self.audit
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn reset_observations(&self) -> Result<(), ToolError> {
        let mut client = Client::new(self.client_limits, self.wire_limits);
        let session = client
            .open_session(WIP_ENDPOINT, self.security_context.clone())
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        *self.state.lock().unwrap_or_else(|error| error.into_inner()) =
            ClientState { client, session };
        Ok(())
    }

    async fn discover(
        &self,
        path: String,
        depth: u32,
        refresh: bool,
    ) -> Result<ToolOutput, ToolError> {
        let prepared = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let session = state.session.clone();
            if refresh {
                state
                    .client
                    .refresh_observed(&session, path.clone(), depth)
                    .map(Some)
            } else {
                state.client.ensure_observed(&session, path.clone(), depth)
            }
            .map_err(client_tool_error)?
        };
        if let Some(prepared) = prepared {
            self.metrics
                .discover_round_trips
                .fetch_add(1, Ordering::Relaxed);
            let response = self.dispatch_retrieval(&prepared.request).await?;
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state
                .client
                .complete(prepared.id, response)
                .map_err(client_tool_error)?;
        }
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let values = state
            .client
            .known_space(&state.session)
            .ok_or_else(|| ToolError::ExecutionFailed("WIP session disappeared".into()))?;
        let visible = values
            .into_iter()
            .filter(|observation| {
                observation.path == path || is_descendant(&path, &observation.path)
            })
            .map(object_observation_json)
            .collect::<Vec<_>>();
        Ok(json_output(
            format!("Observed {} WIP object(s)", visible.len()),
            json!({
                "known_space": visible,
                "metrics": metrics_json(self.metrics.snapshot()),
                "cache_policy": "fresh observations are isolated to this Worker endpoint/security context; use refresh or reset after external authority changes"
            }),
        ))
    }

    async fn inspect(&self, interface: String, refresh: bool) -> Result<ToolOutput, ToolError> {
        let prepared = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let session = state.session.clone();
            if refresh {
                state
                    .client
                    .prepare_interface(&session, interface.clone())
                    .map(Some)
            } else {
                state.client.ensure_interface(&session, interface.clone())
            }
            .map_err(client_tool_error)?
        };
        if let Some(prepared) = prepared {
            self.metrics
                .inspect_round_trips
                .fetch_add(1, Ordering::Relaxed);
            let response = self.dispatch_retrieval(&prepared.request).await?;
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state
                .client
                .complete(prepared.id, response)
                .map_err(client_tool_error)?;
        }
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let observed = state
            .client
            .interface(&state.session, &interface)
            .ok_or_else(|| ToolError::InvalidArgument("interface has not been observed".into()))?;
        Ok(json_output(
            format!("Inspected WIP interface `{interface}`"),
            json!({
                "interface": interface,
                "state": observation_state_name(&observed.state),
                "validator": observed.validator.as_ref().map(hex),
                "descriptor": observed.descriptor.as_ref().map(descriptor_json),
                "metrics": metrics_json(self.metrics.snapshot())
            }),
        ))
    }

    async fn call(
        &self,
        path: String,
        interface: String,
        operation: String,
        arguments: Json,
        execution: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let arguments = match self.host.projection(&path) {
            Some(projection) if projection.kind == WipProjectionKind::Compatibility => {
                BTreeMap::from([(
                    "input".to_string(),
                    json_to_wip(&arguments).map_err(ToolError::InvalidArgument)?,
                )])
            }
            Some(projection) => decode_native_arguments(
                &path,
                &interface,
                &operation,
                &arguments,
                &projection.descriptor,
                self.wire_limits,
            )
            .map_err(ToolError::InvalidArgument)?,
            None => {
                return Err(ToolError::InvalidArgument(format!(
                    "WIP target `{path}` is not published"
                )));
            }
        };
        let prepared = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let session = state.session.clone();
            state
                .client
                .prepare_call(&session, &path, &interface, operation.clone(), arguments)
                .map_err(client_tool_error)?
        };
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state
                .client
                .mark_dispatched(prepared.id)
                .map_err(client_tool_error)?;
        }
        self.metrics
            .operation_round_trips
            .fetch_add(1, Ordering::Relaxed);
        let execution_id = execution.execution_id();
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(execution_id.clone(), path.clone());
        let mut guard = DispatchedCallGuard::new(
            Arc::clone(&self.state),
            Arc::clone(&self.active),
            prepared.id,
            execution_id,
        );
        let exchange = self
            .dispatch_call(&prepared.request, execution.clone())
            .await;
        let (response, passthrough, audit_outcome) = match exchange {
            Ok(exchange) => exchange,
            Err(error) => {
                guard.fail_before_response();
                self.push_audit(prepared.id, path, operation, WipAuditOutcome::Disconnected);
                return Err(error);
            }
        };
        let completion = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.client.complete(prepared.id, response)
        };
        guard.disarm();
        let completion = completion.map_err(client_tool_error)?;
        self.push_audit(prepared.id, path, operation, audit_outcome);
        if let Some(result) = passthrough {
            if result.is_ok() {
                self.metrics
                    .operation_successes
                    .fetch_add(1, Ordering::Relaxed);
            }
            return result;
        }
        render_call_completion(completion).map(|output| {
            self.metrics
                .operation_successes
                .fetch_add(1, Ordering::Relaxed);
            output
        })
    }

    async fn cancel(&self, execution: &ToolExecutionContext) -> Result<(), ToolError> {
        let path = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&execution.execution_id())
            .cloned();
        match path {
            Some(path) => self.host.cancel(&path, execution).await,
            None => Ok(()),
        }
    }

    fn push_audit(
        &self,
        request_id: RequestId,
        path: String,
        operation: String,
        outcome: WipAuditOutcome,
    ) {
        let mut audit = self.audit.lock().unwrap_or_else(|error| error.into_inner());
        audit.push_back(WipAuditRecord {
            request_id: request_id.get(),
            path,
            operation,
            outcome,
        });
        while audit.len() > MAX_AUDIT_RECORDS {
            audit.pop_front();
        }
    }

    async fn dispatch_retrieval(
        &self,
        request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, ToolError> {
        let route = self
            .endpoint
            .recognize(&request.uri().to_string().parse().map_err(|error| {
                ToolError::Internal(format!("invalid prepared WIP URI: {error}"))
            })?)
            .ok_or_else(|| ToolError::Internal("prepared WIP route is not recognized".into()))?;
        match route {
            Route::Observe => {
                let request_value = decode_observe_request(request.body(), self.wire_limits)
                    .map_err(|error| ToolError::Internal(error.to_string()))?;
                match self.host.observe(&request_value.path, request_value.depth) {
                    Ok(value) => encode_observe_response(&request_value, &value, self.wire_limits),
                    Err(error) => encode_protocol_error_response(
                        ProtocolInteraction::Observe,
                        &error,
                        self.wire_limits,
                    ),
                }
                .map_err(|error| ToolError::Internal(error.to_string()))
            }
            Route::FetchInterface => {
                let request_value =
                    decode_fetch_interface_request(request.body(), self.wire_limits)
                        .map_err(|error| ToolError::Internal(error.to_string()))?;
                match self.host.fetch_interface(&request_value.interface) {
                    Ok(value) => {
                        encode_fetch_interface_response(&request_value, &value, self.wire_limits)
                    }
                    Err(error) => encode_protocol_error_response(
                        ProtocolInteraction::FetchInterface,
                        &error,
                        self.wire_limits,
                    ),
                }
                .map_err(|error| ToolError::Internal(error.to_string()))
            }
            Route::CallOperation => Err(ToolError::Internal(
                "call operation used retrieval transport".into(),
            )),
        }
    }

    async fn dispatch_call(
        &self,
        request: &Request<Vec<u8>>,
        execution: ToolExecutionContext,
    ) -> Result<
        (
            Response<Vec<u8>>,
            Option<Result<ToolOutput, ToolError>>,
            WipAuditOutcome,
        ),
        ToolError,
    > {
        let metadata = decode_call_operation_metadata(request.body(), self.wire_limits)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let descriptor = self
            .host
            .descriptor(&metadata.interface.reference)
            .map(|(descriptor, _)| descriptor.clone())
            .ok_or_else(|| ToolError::InvalidArgument("interface is not published".into()))?;
        let request_value = metadata
            .decode_request(request.body(), &descriptor, self.wire_limits)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let context = WipCallContext {
            execution,
            security_context: self.security_context.as_str().to_string(),
        };
        match self.host.call(request_value.clone(), context).await {
            Ok(output) => {
                let response = encode_call_operation_response(
                    &request_value,
                    &descriptor,
                    &CallOperationResponse {
                        result: output.value,
                        validator: self
                            .host
                            .projection(&request_value.target.path)
                            .and_then(|projection| projection.object.validator.clone()),
                    },
                    self.wire_limits,
                )
                .map_err(|error| ToolError::Internal(error.to_string()))?;
                Ok((
                    response,
                    output.tool_output.map(Ok),
                    WipAuditOutcome::Success,
                ))
            }
            Err(WipOperationError::Protocol(error)) => {
                let response = encode_protocol_error_response(
                    ProtocolInteraction::CallOperation,
                    &error,
                    self.wire_limits,
                )
                .map_err(|encode| ToolError::Internal(encode.to_string()))?;
                Ok((response, None, WipAuditOutcome::Rejected))
            }
            Err(WipOperationError::Cancelled(output)) => {
                let response = encode_protocol_error_response(
                    ProtocolInteraction::CallOperation,
                    &protocol_error(
                        ProtocolErrorCode::ResourceLimitExceeded,
                        "operation cancelled",
                    ),
                    self.wire_limits,
                )
                .map_err(|encode| ToolError::Internal(encode.to_string()))?;
                Ok((
                    response,
                    Some(Err(ToolError::Cancelled(output))),
                    WipAuditOutcome::Cancelled,
                ))
            }
            Err(WipOperationError::Interrupted(output)) => {
                let response = encode_protocol_error_response(
                    ProtocolInteraction::CallOperation,
                    &protocol_error(
                        ProtocolErrorCode::OperationOutcomeUnknown,
                        "operation interrupted after dispatch",
                    ),
                    self.wire_limits,
                )
                .map_err(|encode| ToolError::Internal(encode.to_string()))?;
                Ok((
                    response,
                    Some(Err(ToolError::Interrupted(output))),
                    WipAuditOutcome::Interrupted,
                ))
            }
            Err(WipOperationError::OutcomeUnknown(message)) => {
                let response = encode_protocol_error_response(
                    ProtocolInteraction::CallOperation,
                    &protocol_error(ProtocolErrorCode::OperationOutcomeUnknown, message.clone()),
                    self.wire_limits,
                )
                .map_err(|encode| ToolError::Internal(encode.to_string()))?;
                Ok((
                    response,
                    Some(Err(ToolError::ExecutionFailed(format!(
                        "WIP operation outcome unknown; do not retry automatically: {message}"
                    )))),
                    WipAuditOutcome::OutcomeUnknown,
                ))
            }
        }
    }
}

struct DispatchedCallGuard {
    state: Arc<Mutex<ClientState>>,
    active: Arc<Mutex<HashMap<String, String>>>,
    request_id: RequestId,
    execution_id: String,
    armed: bool,
}

impl DispatchedCallGuard {
    fn new(
        state: Arc<Mutex<ClientState>>,
        active: Arc<Mutex<HashMap<String, String>>>,
        request_id: RequestId,
        execution_id: String,
    ) -> Self {
        Self {
            state,
            active,
            request_id,
            execution_id,
            armed: true,
        }
    }

    fn fail_before_response(&mut self) {
        if self.armed {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let _ = state.client.fail_dispatched_call(
                self.request_id,
                DispatchedTransportFailure::Disconnect,
                "in-process WIP transport disconnected after dispatch",
            );
            self.armed = false;
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DispatchedCallGuard {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let _ = state.client.fail_dispatched_call(
                self.request_id,
                DispatchedTransportFailure::Disconnect,
                "WIP call future ended without a terminal response",
            );
        }
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.execution_id);
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WipDiscoverInput {
    /// Canonical Worldspace path. Begin at `/`.
    #[serde(default = "root_path")]
    path: String,
    /// Descendant depth to materialize. Use 1 to explore one level at a time.
    #[serde(default = "one")]
    depth: u32,
    /// Explicitly supersede cached observations for this path.
    #[serde(default)]
    refresh: bool,
    /// Drop all Known Space for this Worker/security context before observing.
    #[serde(default)]
    reset: bool,
}

fn root_path() -> String {
    WIP_ROOT.into()
}

fn one() -> u32 {
    1
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WipInspectInput {
    /// Opaque interface reference returned by WipDiscover.
    interface: String,
    /// Explicitly supersede the cached descriptor observation.
    #[serde(default)]
    refresh: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WipCallInput {
    /// Exact object path returned by WipDiscover.
    path: String,
    /// Exact interface reference selected from that object.
    interface: String,
    /// Exact operation name from WipInspect.
    operation: String,
    /// Original operation arguments. Compatibility operations accept the original tool object here.
    arguments: Json,
}

struct WipDiscoverTool {
    runtime: Arc<WipRuntime>,
}

#[async_trait]
impl Tool for WipDiscoverTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: WipDiscoverInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        if input.reset {
            self.runtime.reset_observations()?;
        }
        self.runtime
            .discover(input.path, input.depth.min(8), input.refresh)
            .await
    }
}

struct WipInspectTool {
    runtime: Arc<WipRuntime>,
}

#[async_trait]
impl Tool for WipInspectTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: WipInspectInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        self.runtime.inspect(input.interface, input.refresh).await
    }
}

struct WipCallTool {
    runtime: Arc<WipRuntime>,
}

#[async_trait]
impl Tool for WipCallTool {
    async fn execute(
        &self,
        input_json: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: WipCallInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        self.runtime
            .call(
                input.path,
                input.interface,
                input.operation,
                input.arguments,
                context,
            )
            .await
    }

    async fn cancel_execution(&self, context: &ToolExecutionContext) -> Result<(), ToolError> {
        self.runtime.cancel(context).await
    }
}

fn wip_tool_definitions(runtime: Arc<WipRuntime>) -> Vec<ToolDefinition> {
    let discover_runtime = Arc::clone(&runtime);
    let inspect_runtime = Arc::clone(&runtime);
    let call_runtime = Arc::clone(&runtime);
    vec![
        Arc::new(move || {
            let schema = schemars::schema_for!(WipDiscoverInput);
            (
                ToolMeta::new("WipDiscover")
                    .description("Explore the authorized WIP Worldspace and stateful Known Space. Start at `/`; use reset after reconnect or authority changes.")
                    .input_schema(serde_json::to_value(schema).expect("WIP discover schema serializes")),
                Arc::new(WipDiscoverTool {
                    runtime: Arc::clone(&discover_runtime),
                }) as Arc<dyn Tool>,
            )
        }),
        Arc::new(move || {
            let schema = schemars::schema_for!(WipInspectInput);
            (
                ToolMeta::new("WipInspect")
                    .description("Fetch and validate one descriptor returned by WipDiscover. Inspect before calling an operation.")
                    .input_schema(serde_json::to_value(schema).expect("WIP inspect schema serializes")),
                Arc::new(WipInspectTool {
                    runtime: Arc::clone(&inspect_runtime),
                }) as Arc<dyn Tool>,
            )
        }),
        Arc::new(move || {
            let schema = schemars::schema_for!(WipCallInput);
            (
                ToolMeta::new("WipCall")
                    .description("Invoke an operation only from fresh object/interface observations. Stale validators and invalid original JSON Schema inputs fail closed; unknown write outcomes must not be retried automatically.")
                    .input_schema(serde_json::to_value(schema).expect("WIP call schema serializes")),
                Arc::new(WipCallTool {
                    runtime: Arc::clone(&call_runtime),
                }) as Arc<dyn Tool>,
            )
        }),
    ]
}

/// Replace all currently registered ordinary tools with one WIP client surface.
/// Call this once after every enabled Feature has contributed its tools.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn install_wip_mode<C: LlmClient, A: Send + Sync>(
    engine: &mut Engine<C, Mutable, A>,
    permissions: Option<ToolPermissionConfig>,
    security_context: String,
) -> Result<Arc<WipRuntime>, String> {
    install_wip_mode_with_mounts(
        engine,
        permissions,
        security_context,
        WipMountRegistry::new(),
    )
}

/// Host extension point for aggregating Feature-owned native projections before
/// compatibility tools are finalized. Native mounts already present in `registry`
/// replace a matching `tool:<name>` compatibility capability and all unrelated
/// route/interface collisions remain fatal.
pub fn install_wip_mode_with_mounts<C: LlmClient, A: Send + Sync>(
    engine: &mut Engine<C, Mutable, A>,
    permissions: Option<ToolPermissionConfig>,
    security_context: String,
    mut registry: WipMountRegistry,
) -> Result<Arc<WipRuntime>, String> {
    let handle = engine.tool_server_handle();
    handle.flush_pending();
    let definitions = handle.tool_definitions_sorted();
    let ordinary_schema_bytes = definitions
        .iter()
        .map(|definition| {
            definition.name.len()
                + definition
                    .description
                    .as_deref()
                    .map(str::len)
                    .unwrap_or_default()
                + serde_json::to_vec(&definition.input_schema)
                    .map(|bytes| bytes.len())
                    .unwrap_or_default()
        })
        .sum::<usize>() as u64;
    let mut names = Vec::new();
    for definition in definitions {
        let (meta, tool) = handle
            .get_tool(&definition.name)
            .ok_or_else(|| format!("registered tool `{}` disappeared", definition.name))?;
        let capability = format!("tool:{}", meta.name);
        let name = meta.name.clone();
        if !registry.replaces_compatibility(&capability) {
            let projection = compatibility_projection(meta, tool, permissions.clone())
                .map_err(|error| error.to_string())?;
            registry
                .mount(projection)
                .map_err(|error| error.to_string())?;
        }
        names.push(name);
    }
    let runtime = Arc::new(WipRuntime::new(
        WipHost::new(registry),
        SecurityContext::new(security_context),
        ordinary_schema_bytes,
    )?);
    for name in names {
        handle
            .unregister(&name)
            .map_err(|error| error.to_string())?;
    }
    let definitions = wip_tool_definitions(Arc::clone(&runtime));
    let wip_schema_bytes = definitions
        .iter()
        .map(|definition| {
            let (meta, _) = definition();
            meta.name.len()
                + meta.description.len()
                + serde_json::to_vec(&meta.input_schema)
                    .map(|bytes| bytes.len())
                    .unwrap_or_default()
        })
        .sum::<usize>() as u64;
    runtime
        .metrics
        .wip_schema_bytes
        .store(wip_schema_bytes, Ordering::Relaxed);
    engine.register_tools(definitions);
    Ok(runtime)
}

fn client_tool_error(error: wip_client::ClientError) -> ToolError {
    ToolError::ExecutionFailed(error.to_string())
}

fn is_descendant(parent: &str, candidate: &str) -> bool {
    if parent == WIP_ROOT {
        candidate.starts_with('/')
    } else {
        candidate.starts_with(&format!("{parent}/"))
    }
}

fn object_observation_json(observation: wip_client::ObjectObservation) -> Json {
    json!({
        "path": observation.path,
        "state": observation_state_name(&observation.state),
        "validator": observation.validator.as_ref().map(hex),
        "object": observation.object.map(|object| json!({
            "name": object.name,
            "description": object.description,
            "interfaces": object.interfaces,
            "ref": object.r#ref,
            "validator": object.validator.as_ref().map(hex),
        }))
    })
}

fn observation_state_name(state: &ObservationState) -> &'static str {
    match state {
        ObservationState::Loading(_) => "loading",
        ObservationState::Fresh => "fresh",
        ObservationState::Stale => "stale",
        ObservationState::Error(_) => "error",
    }
}

fn descriptor_json(descriptor: &InterfaceDescriptor) -> Json {
    json!({
        "format": descriptor.format,
        "documentation": descriptor.documentation.as_ref().map(documentation_json),
        "types": descriptor.types.iter().map(|declaration| json!({
            "name": declaration.name,
            "documentation": declaration.documentation.as_ref().map(documentation_json),
            "definition": type_json(&declaration.definition),
        })).collect::<Vec<_>>(),
        "operations": descriptor.operations.iter().map(|operation| json!({
            "name": operation.name,
            "documentation": operation.documentation.as_ref().map(documentation_json),
            "parameters": operation.parameters.iter().map(|parameter| json!({
                "name": parameter.name,
                "required": parameter.required,
                "documentation": parameter.documentation.as_ref().map(documentation_json),
                "type": type_json(&parameter.r#type),
            })).collect::<Vec<_>>(),
            "returns": {
                "documentation": operation.returns.documentation.as_ref().map(documentation_json),
                "type": type_json(&operation.returns.r#type),
            }
        })).collect::<Vec<_>>()
    })
}

fn documentation_json(documentation: &Documentation) -> Json {
    json!({
        "summary": documentation.summary,
        "details": documentation.details,
    })
}

fn type_json(value: &TypeExpr) -> Json {
    match value {
        TypeExpr::Unit => json!("unit"),
        TypeExpr::Boolean => json!("boolean"),
        TypeExpr::Integer => json!("integer"),
        TypeExpr::Number => json!("number"),
        TypeExpr::String => json!("string"),
        TypeExpr::Bytes => json!("bytes"),
        TypeExpr::Json => json!("json"),
        TypeExpr::Entry => json!("entry"),
        TypeExpr::Named { name } => json!({"named": name}),
        TypeExpr::Record { fields } => json!({
            "record": {
                "fields": fields.iter().map(|field| json!({
                    "name": field.name,
                    "required": field.required,
                    "documentation": field.documentation.as_ref().map(documentation_json),
                    "type": type_json(&field.r#type),
                })).collect::<Vec<_>>()
            }
        }),
        TypeExpr::List { items } => json!({"list": {"items": type_json(items)}}),
        TypeExpr::Enum { cases } => json!({
            "enum": {
                "cases": cases.iter().map(|case| json!({
                    "name": case.name,
                    "documentation": case.documentation.as_ref().map(documentation_json),
                })).collect::<Vec<_>>()
            }
        }),
        TypeExpr::Union { cases } => json!({
            "union": {
                "cases": cases.iter().map(|case| json!({
                    "name": case.name,
                    "documentation": case.documentation.as_ref().map(documentation_json),
                    "payload": case.payload.as_ref().map(type_json),
                })).collect::<Vec<_>>()
            }
        }),
    }
}

fn metrics_json(metrics: WipMetricsSnapshot) -> Json {
    json!({
        "ordinary_schema_bytes": metrics.ordinary_schema_bytes,
        "wip_schema_bytes": metrics.wip_schema_bytes,
        "discover_round_trips": metrics.discover_round_trips,
        "inspect_round_trips": metrics.inspect_round_trips,
        "operation_round_trips": metrics.operation_round_trips,
        "operation_successes": metrics.operation_successes,
    })
}

fn tool_output_value(output: &ToolOutput) -> Value {
    let mut record = BTreeMap::new();
    record.insert("summary".into(), Value::String(output.summary.clone()));
    record.insert(
        "content".into(),
        output
            .content
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Unit),
    );
    record.insert(
        "attachment_count".into(),
        Value::Integer(i64::try_from(output.attachments.len()).unwrap_or(i64::MAX)),
    );
    Value::Record(record)
}

fn render_call_completion(completion: Completion) -> Result<ToolOutput, ToolError> {
    let Completion::Call(record) = completion else {
        return Err(ToolError::Internal(
            "WIP operation completed with a retrieval result".into(),
        ));
    };
    match record.outcome {
        CallOutcome::Success(response) => Ok(json_output(
            "WIP operation completed".into(),
            wip_to_json(&response.result).map_err(ToolError::Internal)?,
        )),
        CallOutcome::ProtocolFailure(error) => Err(ToolError::ExecutionFailed(error.to_string())),
        CallOutcome::Unknown { reason, failure } => Err(ToolError::ExecutionFailed(format!(
            "WIP operation outcome unknown ({reason:?}); do not retry automatically{}",
            failure
                .map(|failure| format!(": {failure}"))
                .unwrap_or_default()
        ))),
        CallOutcome::NotDispatched(error) => Err(ToolError::ExecutionFailed(format!(
            "WIP operation was not dispatched: {error}"
        ))),
    }
}

fn json_output(summary: String, value: Json) -> ToolOutput {
    ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())),
        attachments: Vec::new(),
    }
}

fn decode_native_arguments(
    path: &str,
    interface: &str,
    operation: &str,
    arguments: &Json,
    descriptor: &InterfaceDescriptor,
    limits: Limits,
) -> Result<BTreeMap<String, Value>, String> {
    let body = serde_json::to_vec(&json!({
        "target": {"path": path},
        "interface": {"reference": interface},
        "operation": operation,
        "arguments": arguments,
    }))
    .map_err(|error| format!("failed to encode native WIP arguments: {error}"))?;
    decode_call_operation_request(&body, descriptor, limits)
        .map(|request| request.arguments)
        .map_err(|error| format!("native WIP arguments do not match the descriptor: {error}"))
}

pub(crate) fn json_to_wip(value: &Json) -> Result<Value, String> {
    match value {
        Json::Null => Ok(Value::Unit),
        Json::Bool(value) => Ok(Value::Boolean(*value)),
        Json::Number(value) => {
            if let Some(value) = value.as_i64() {
                if (wip_protocol::MIN_SAFE_INTEGER..=wip_protocol::MAX_SAFE_INTEGER)
                    .contains(&value)
                {
                    Ok(Value::Integer(value))
                } else {
                    Err("integer is outside the WIP lossless range".into())
                }
            } else {
                let value = value
                    .as_f64()
                    .ok_or_else(|| "JSON number is not representable".to_string())?;
                if value.is_finite() {
                    Ok(Value::Number(value))
                } else {
                    Err("WIP numbers must be finite".into())
                }
            }
        }
        Json::String(value) => Ok(Value::String(value.clone())),
        Json::Array(values) => values
            .iter()
            .map(json_to_wip)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        Json::Object(values) => values
            .iter()
            .map(|(name, value)| Ok((name.clone(), json_to_wip(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Record),
    }
}

pub(crate) fn wip_to_json(value: &Value) -> Result<Json, String> {
    match value {
        Value::Unit => Ok(Json::Null),
        Value::Boolean(value) => Ok(Json::Bool(*value)),
        Value::Integer(value) => Ok(json!(value)),
        Value::Number(value) if value.is_finite() => Ok(json!(value)),
        Value::Number(_) => Err("WIP number is not finite".into()),
        Value::String(value) => Ok(Json::String(value.clone())),
        Value::Bytes(value) => Ok(Json::String(BASE64_STANDARD.encode(value))),
        Value::Record(values) => values
            .iter()
            .map(|(name, value)| Ok((name.clone(), wip_to_json(value)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Json::Object),
        Value::List(values) => values
            .iter()
            .map(wip_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
    }
}

fn hex(bytes: &Vec<u8>) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::llm_client::client::LlmClient;
    use agen::llm_client::{ClientError, Request as LlmRequest, ResponseStream};
    use futures::stream;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wip_protocol::{EnumCase, FieldDeclaration, TypeDeclaration, UnionCase};

    struct EchoTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for EchoTool {
        async fn execute(
            &self,
            input: &str,
            _context: ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(input.to_string().into())
        }
    }

    #[derive(Debug)]
    struct ScriptedWorkspaceClient {
        responses: Mutex<VecDeque<crate::worker::WorkspaceResponse>>,
        requests: Mutex<Vec<crate::worker::WorkspaceRequest>>,
    }

    impl ScriptedWorkspaceClient {
        fn new(responses: impl IntoIterator<Item = crate::worker::WorkspaceResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<crate::worker::WorkspaceRequest> {
            self.requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    impl crate::worker::WorkspaceClient for ScriptedWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }

        fn kind(&self) -> &str {
            "scripted-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: crate::worker::WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            self.requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(request);
            self.responses
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
                .ok_or_else(|| {
                    crate::worker::WorkspaceClientError::Unavailable(
                        "scripted response queue is empty".into(),
                    )
                })
        }
    }

    fn objective_detail_response(revision: &str) -> crate::worker::WorkspaceResponse {
        crate::worker::WorkspaceResponse {
            status: 200,
            body: json!({
                "id": "00001OBJECTIVE",
                "resource_key": "O-3",
                "title": "Objective",
                "body": "Body",
                "body_truncated": false,
                "state": "active",
                "revision": revision,
                "created_at": null,
                "updated_at": null,
                "linked_ticket_summaries": [],
                "events": [],
                "event_page": {"next_cursor": null, "has_more": false}
            })
            .to_string(),
        }
    }

    fn meta(name: &str) -> ToolMeta {
        ToolMeta::new(name)
            .description("test tool")
            .input_schema(json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"message": {"type": "string", "minLength": 2}},
                "required": ["message"]
            }))
    }

    fn registry_with_tool(
        name: &str,
        calls: Arc<AtomicUsize>,
        permissions: Option<ToolPermissionConfig>,
    ) -> WipMountRegistry {
        let mut registry = WipMountRegistry::new();
        registry
            .mount(
                compatibility_projection(meta(name), Arc::new(EchoTool { calls }), permissions)
                    .unwrap(),
            )
            .unwrap();
        registry
    }

    #[derive(Clone)]
    struct DummyClient;

    #[async_trait]
    impl LlmClient for DummyClient {
        async fn stream(&self, _request: LlmRequest) -> Result<ResponseStream, ClientError> {
            Ok(Box::pin(stream::empty()))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    fn echo_definition(name: String, calls: Arc<AtomicUsize>) -> ToolDefinition {
        Arc::new(move || {
            (
                meta(&name),
                Arc::new(EchoTool {
                    calls: Arc::clone(&calls),
                }) as Arc<dyn Tool>,
            )
        })
    }

    #[test]
    fn mode_installation_replaces_tool_surface_without_changing_normal_engines() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut normal = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        normal.register_tool(echo_definition("Echo".into(), Arc::clone(&calls)));
        normal.tool_server_handle().flush_pending();
        assert_eq!(
            normal
                .tool_server_handle()
                .tool_definitions_sorted()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            ["Echo"]
        );

        let mut wip = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for index in 0..24 {
            wip.register_tool(echo_definition(format!("Echo{index}"), Arc::clone(&calls)));
        }
        let runtime = install_wip_mode(&mut wip, None, "worker-a".into()).unwrap();
        wip.tool_server_handle().flush_pending();
        assert_eq!(
            wip.tool_server_handle()
                .tool_definitions_sorted()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            ["WipCall", "WipDiscover", "WipInspect"]
        );
        let metrics = runtime.metrics();
        assert!(metrics.ordinary_schema_bytes > metrics.wip_schema_bytes);
    }

    #[test]
    fn mount_collision_and_native_replacement_are_explicit() {
        struct Native;
        #[async_trait]
        impl WipOperationHandler for Native {
            async fn call(
                &self,
                _operation: &str,
                _arguments: &BTreeMap<String, Value>,
                _context: WipCallContext,
            ) -> Result<WipOperationOutput, WipOperationError> {
                Ok(WipOperationOutput::native(Value::Unit))
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = registry_with_tool("Echo", calls, None);
        let mut native = compatibility_projection(
            meta("Echo"),
            Arc::new(EchoTool {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            None,
        )
        .unwrap();
        native.kind = WipProjectionKind::Native;
        native.handler = Arc::new(Native);
        assert_eq!(
            registry.mount(native).unwrap(),
            WipMountDisposition::ReplacedCompatibility
        );

        let mut collision = compatibility_projection(
            meta("Other"),
            Arc::new(EchoTool {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            None,
        )
        .unwrap();
        collision.route = "/tools/Echo".into();
        collision.object.name = "Echo".into();
        assert!(matches!(
            registry.mount(collision),
            Err(WipMountError::RouteCollision { .. })
        ));
    }

    struct NativeTyped;

    #[async_trait]
    impl WipOperationHandler for NativeTyped {
        async fn call(
            &self,
            operation: &str,
            arguments: &BTreeMap<String, Value>,
            _context: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            assert_eq!(operation, "accept");
            assert_eq!(arguments.get("number"), Some(&Value::Number(1.0)));
            assert_eq!(
                arguments.get("bytes"),
                Some(&Value::Bytes(vec![0, 1, 2, 255]))
            );
            assert_eq!(
                arguments.get("payload"),
                Some(&Value::Record(BTreeMap::from([
                    ("label".into(), Value::String("item".into())),
                    (
                        "tags".into(),
                        Value::List(vec![
                            Value::String("red".into()),
                            Value::String("blue".into()),
                        ]),
                    ),
                    (
                        "choice".into(),
                        Value::Record(BTreeMap::from([
                            ("$case".into(), Value::String("found".into())),
                            ("value".into(), Value::String("/items/1".into())),
                        ])),
                    ),
                ])))
            );
            Ok(WipOperationOutput::native(Value::Bytes(vec![255, 0, 1])))
        }
    }

    fn documentation(summary: &str) -> Option<Documentation> {
        Some(Documentation {
            summary: summary.into(),
            details: None,
        })
    }

    fn native_typed_projection() -> WipProjection {
        WipProjection {
            route: "/native/typed".into(),
            capability: "native:typed".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: "typed".into(),
                description: Some("native typed sample".into()),
                interfaces: vec!["yoi.native/typed/v1".into()],
                r#ref: Some("native:typed".into()),
                validator: Some(vec![1]),
            },
            interface: "yoi.native/typed/v1".into(),
            descriptor: InterfaceDescriptor {
                format: INTERFACE_FORMAT_V1.into(),
                documentation: documentation("typed interface"),
                types: vec![TypeDeclaration {
                    name: "Payload".into(),
                    documentation: documentation("named payload"),
                    definition: TypeExpr::Record {
                        fields: vec![
                            FieldDeclaration {
                                name: "label".into(),
                                required: true,
                                documentation: documentation("payload label"),
                                r#type: TypeExpr::String,
                            },
                            FieldDeclaration {
                                name: "tags".into(),
                                required: false,
                                documentation: documentation("payload tags"),
                                r#type: TypeExpr::List {
                                    items: Box::new(TypeExpr::Enum {
                                        cases: vec![
                                            EnumCase {
                                                name: "red".into(),
                                                documentation: documentation("red tag"),
                                            },
                                            EnumCase {
                                                name: "blue".into(),
                                                documentation: documentation("blue tag"),
                                            },
                                        ],
                                    }),
                                },
                            },
                            FieldDeclaration {
                                name: "choice".into(),
                                required: true,
                                documentation: documentation("payload choice"),
                                r#type: TypeExpr::Union {
                                    cases: vec![
                                        UnionCase {
                                            name: "empty".into(),
                                            documentation: documentation("empty choice"),
                                            payload: None,
                                        },
                                        UnionCase {
                                            name: "found".into(),
                                            documentation: documentation("found entry"),
                                            payload: Some(TypeExpr::Entry),
                                        },
                                    ],
                                },
                            },
                        ],
                    },
                }],
                operations: vec![OperationDeclaration {
                    name: "accept".into(),
                    documentation: documentation("accept typed values"),
                    parameters: vec![
                        ParameterDeclaration {
                            name: "payload".into(),
                            required: true,
                            documentation: documentation("named payload argument"),
                            r#type: TypeExpr::Named {
                                name: "Payload".into(),
                            },
                        },
                        ParameterDeclaration {
                            name: "number".into(),
                            required: true,
                            documentation: None,
                            r#type: TypeExpr::Number,
                        },
                        ParameterDeclaration {
                            name: "bytes".into(),
                            required: true,
                            documentation: None,
                            r#type: TypeExpr::Bytes,
                        },
                    ],
                    returns: ReturnDeclaration {
                        documentation: None,
                        r#type: TypeExpr::Bytes,
                    },
                }],
            },
            interface_validator: Some(vec![1]),
            handler: Arc::new(NativeTyped),
        }
    }

    #[tokio::test]
    async fn native_projection_preserves_descriptor_and_decodes_typed_arguments_end_to_end() {
        let mut registry = WipMountRegistry::new();
        registry.mount(native_typed_projection()).unwrap();
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        runtime.discover("/".into(), 2, false).await.unwrap();
        let inspected = runtime
            .inspect("yoi.native/typed/v1".into(), false)
            .await
            .unwrap();
        let inspected: Json = serde_json::from_str(inspected.content.as_deref().unwrap()).unwrap();
        assert_eq!(
            inspected.pointer(
                "/descriptor/types/0/definition/record/fields/1/type/list/items/enum/cases/0/documentation/summary"
            ),
            Some(&json!("red tag"))
        );
        assert_eq!(
            inspected.pointer("/descriptor/types/0/definition/record/fields/1/required"),
            Some(&json!(false))
        );
        assert_eq!(
            inspected.pointer(
                "/descriptor/types/0/definition/record/fields/2/type/union/cases/1/payload"
            ),
            Some(&json!("entry"))
        );

        let output = runtime
            .call(
                "/native/typed".into(),
                "yoi.native/typed/v1".into(),
                "accept".into(),
                json!({
                    "payload": {
                        "label": "item",
                        "tags": ["red", "blue"],
                        "choice": {"$case": "found", "value": "/items/1"}
                    },
                    "number": 1,
                    "bytes": "AAEC/w=="
                }),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(output.summary, "WIP operation completed");
        assert_eq!(output.content.as_deref(), Some("\"/wAB\""));

        let invalid = runtime
            .call(
                "/native/typed".into(),
                "yoi.native/typed/v1".into(),
                "accept".into(),
                json!({
                    "payload": {
                        "label": "item",
                        "choice": {"$case": "empty"}
                    },
                    "number": 1,
                    "bytes": "AAEC_w=="
                }),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("canonical base64"));
    }

    #[tokio::test]
    async fn stateful_discover_inspect_call_round_trip_validates_original_schema() {
        let calls = Arc::new(AtomicUsize::new(0));
        let host = WipHost::new(registry_with_tool("Echo", Arc::clone(&calls), None));
        let runtime = WipRuntime::new(host, SecurityContext::new("worker-a"), 4096).unwrap();

        runtime.discover("/".into(), 2, false).await.unwrap();
        runtime
            .inspect("yoi.tool/Echo/v1".into(), false)
            .await
            .unwrap();
        let output = runtime
            .call(
                "/tools/Echo".into(),
                "yoi.tool/Echo/v1".into(),
                "call".into(),
                json!({"message": "ok"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(output.summary, "{\"message\":\"ok\"}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let invalid = runtime
            .call(
                "/tools/Echo".into(),
                "yoi.tool/Echo/v1".into(),
                "call".into(),
                json!({"message": "x"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("InvalidArguments"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let metrics = runtime.metrics();
        assert_eq!(metrics.discover_round_trips, 1);
        assert_eq!(metrics.inspect_round_trips, 1);
        assert_eq!(metrics.operation_successes, 1);
    }

    #[tokio::test]
    async fn sessions_are_isolated_and_stale_validators_fail_before_dispatch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let host = WipHost::new(registry_with_tool("Echo", Arc::clone(&calls), None));
        let runtime = WipRuntime::new(host, SecurityContext::new("worker-a"), 0).unwrap();
        runtime.discover("/".into(), 2, false).await.unwrap();
        runtime
            .inspect("yoi.tool/Echo/v1".into(), false)
            .await
            .unwrap();

        let projection = runtime.host.projection("/tools/Echo").unwrap();
        let mut request = CallOperationRequest {
            target: wip_protocol::Target {
                path: "/tools/Echo".into(),
                validator: Some(vec![0]),
            },
            interface: wip_protocol::InterfaceTarget {
                reference: projection.interface.clone(),
                validator: projection.interface_validator.clone(),
            },
            operation: "call".into(),
            arguments: BTreeMap::from([(
                "input".into(),
                json_to_wip(&json!({"message": "ok"})).unwrap(),
            )]),
        };
        let result = runtime
            .host
            .call(
                request.clone(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker-a".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::ValidatorMismatch,
                ..
            }))
        ));
        request.target.validator = projection.object.validator.clone();
        request.interface.validator = Some(vec![0]);
        let result = runtime
            .host
            .call(
                request.clone(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker-a".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InterfaceValidatorMismatch,
                ..
            }))
        ));

        request.interface.validator = projection.interface_validator.clone();
        request.operation = "forged".into();
        let result = runtime
            .host
            .call(
                request.clone(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker-a".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::OperationNotFound,
                ..
            }))
        ));

        request.target.path = "/tools/NotPublished".into();
        let result = runtime
            .host
            .call(
                request,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker-a".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::NotFound,
                ..
            }))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        runtime.reset_observations().unwrap();
        let state = runtime.state.lock().unwrap();
        assert!(state.client.known_space(&state.session).unwrap().is_empty());
        assert_eq!(state.session.security_context().as_str(), "worker-a");
    }

    struct DynamicResolver {
        revision: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
    }

    struct DynamicHandler {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WipOperationHandler for DynamicHandler {
        async fn call(
            &self,
            _operation: &str,
            _arguments: &BTreeMap<String, Value>,
            _context: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(WipOperationOutput::native(Value::Unit))
        }
    }

    impl WipDynamicItemResolver for DynamicResolver {
        fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
            let sequence = item_reference.strip_prefix("O-")?;
            if sequence.is_empty() || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some(WipDynamicItem {
                object: Object {
                    name: item_reference.into(),
                    description: None,
                    interfaces: vec!["test.dynamic/v1".into()],
                    r#ref: Some(format!("objective:{item_reference}")),
                    validator: Some(vec![self.revision.load(Ordering::SeqCst) as u8]),
                },
                handler: Arc::new(DynamicHandler {
                    calls: Arc::clone(&self.calls),
                }),
            })
        }
    }

    fn dynamic_registry(revision: Arc<AtomicUsize>, calls: Arc<AtomicUsize>) -> WipMountRegistry {
        let mut registry = WipMountRegistry::new();
        let feature = registry.allocate_feature_route("objective").unwrap();
        let collection_route = feature.child("objectives").unwrap();
        let mut collection = compatibility_projection(
            meta("Dynamic"),
            Arc::new(EchoTool {
                calls: Arc::clone(&calls),
            }),
            None,
        )
        .unwrap();
        collection.route = collection_route.clone();
        collection.capability = "objective:collection".into();
        collection.kind = WipProjectionKind::Native;
        collection.object.name = "objectives".into();
        collection.object.interfaces = vec!["test.dynamic/v1".into()];
        collection.interface = "test.dynamic/v1".into();
        collection.handler = Arc::new(DynamicHandler {
            calls: Arc::clone(&calls),
        });
        let descriptor = collection.descriptor.clone();
        let interface_validator = collection.interface_validator.clone();
        registry.mount(collection).unwrap();
        registry
            .mount_dynamic(WipDynamicMount {
                collection_route,
                capability: "objective:item".into(),
                interface: "test.dynamic/v1".into(),
                descriptor,
                interface_validator,
                resolver: Arc::new(DynamicResolver { revision, calls }),
            })
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn dynamic_routes_resolve_only_direct_children_and_reject_stale_calls() {
        let revision = Arc::new(AtomicUsize::new(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let host = WipHost::new(dynamic_registry(Arc::clone(&revision), Arc::clone(&calls)));
        let initial = host
            .projection("/features/objective/objectives/O-3")
            .unwrap();
        assert!(
            host.projection("/features/objective/objectives/O-3/other")
                .is_none()
        );
        assert!(
            host.projection("/features/objective/objectives/T-3")
                .is_none()
        );
        assert!(host.projection("/hidden/objectives/O-3").is_none());

        revision.store(2, Ordering::SeqCst);
        let result = host
            .call(
                CallOperationRequest {
                    target: wip_protocol::Target {
                        path: "/features/objective/objectives/O-3".into(),
                        validator: initial.object.validator,
                    },
                    interface: wip_protocol::InterfaceTarget {
                        reference: initial.interface,
                        validator: initial.interface_validator,
                    },
                    operation: "call".into(),
                    arguments: BTreeMap::from([(
                        "input".into(),
                        json_to_wip(&json!({"message": "ok"})).unwrap(),
                    )]),
                },
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker-a".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::ValidatorMismatch,
                ..
            }))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn native_capability_claim_hides_only_claimed_compatibility_route() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        engine.register_tool(echo_definition("Echo".into(), Arc::clone(&calls)));
        engine.register_tool(echo_definition("Other".into(), Arc::clone(&calls)));

        let revision = Arc::new(AtomicUsize::new(1));
        let mut registry = dynamic_registry(revision, calls);
        registry
            .replace_compatibility_tools("/features/objective/objectives", ["Echo"])
            .unwrap();
        let runtime =
            install_wip_mode_with_mounts(&mut engine, None, "worker-a".into(), registry).unwrap();
        assert!(runtime.host.projection("/tools/Echo").is_none());
        assert!(runtime.host.projection("/tools/Other").is_some());
        assert!(
            runtime
                .host
                .projection("/features/objective/objectives")
                .is_some()
        );
    }

    #[tokio::test]
    async fn ticket_native_runtime_covers_authoring_and_workflow_role_surfaces() {
        let query_response = crate::worker::WorkspaceResponse {
            status: 200,
            body: json!({
                "items": [],
                "page": {"next_cursor": null, "has_more": false}
            })
            .to_string(),
        };
        let create_response = crate::worker::WorkspaceResponse {
            status: 200,
            body: json!({
                "id": "00001TICKET",
                "resource_key": "T-9",
                "slug": "new-ticket",
                "status": "Open"
            })
            .to_string(),
        };
        let comment_response = crate::worker::WorkspaceResponse {
            status: 204,
            body: String::new(),
        };
        let authoring_client = Arc::new(ScriptedWorkspaceClient::new([
            query_response,
            create_response,
            comment_response,
        ]));
        let authoring_access =
            crate::feature::builtin::ticket::TicketFeatureAccess::workspace_authoring();
        let mut authoring_engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in crate::feature::builtin::ticket::enabled_ticket_definitions(
            authoring_client.clone(),
            authoring_access,
        ) {
            authoring_engine.register_tool(definition);
        }
        let mut authoring_registry = WipMountRegistry::new();
        let route = authoring_registry.allocate_feature_route("ticket").unwrap();
        crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            &mut authoring_registry,
            authoring_client.clone(),
            authoring_access,
            None,
            &route,
        )
        .unwrap();
        let authoring = install_wip_mode_with_mounts(
            &mut authoring_engine,
            None,
            "authoring-worker".into(),
            authoring_registry,
        )
        .unwrap();

        let discovered = authoring.discover("/".into(), 3, false).await.unwrap();
        let discovered = discovered.content.unwrap();
        assert!(discovered.contains("/features/ticket/tickets"));
        assert!(!discovered.contains("/tools/QueryTicket"));
        assert!(!discovered.contains("/tools/TicketCreate"));
        authoring
            .inspect("yoi.ticket/collection/v1".into(), false)
            .await
            .unwrap();
        let collection = "/features/ticket/tickets";
        authoring
            .call(
                collection.into(),
                "yoi.ticket/collection/v1".into(),
                "query".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let created = authoring
            .call(
                collection.into(),
                "yoi.ticket/collection/v1".into(),
                "create".into(),
                json!({"title": "New ticket"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(
            created
                .content
                .unwrap()
                .contains("/features/ticket/tickets/T-9")
        );
        let item = "/features/ticket/tickets/T-9";
        authoring.discover(item.into(), 0, false).await.unwrap();
        authoring
            .inspect("yoi.ticket/item/v1".into(), false)
            .await
            .unwrap();
        authoring
            .call(
                item.into(),
                "yoi.ticket/item/v1".into(),
                "comment".into(),
                json!({"body": "Native comment"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let authoring_requests = authoring_client.requests();
        assert_eq!(authoring_requests.len(), 3);
        assert!(authoring_requests[0].path.ends_with("/tickets/query"));
        assert!(authoring_requests[1].path.ends_with("/tickets"));
        assert!(
            authoring_requests[2]
                .path
                .ends_with("/tickets/T-9/thread-events")
        );

        let workflow_client = Arc::new(ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"id": "00001SOURCE", "resource_key": "T-9"}).to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"id": "00001TARGET", "resource_key": "T-10"}).to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "ticket_id": "00001SOURCE",
                    "kind": "depends_on",
                    "target": "00001TARGET",
                    "note": null,
                    "author": "workspace",
                    "at": "2026-10-03T00:00:00Z"
                })
                .to_string(),
            },
        ]));
        let workflow_access = crate::feature::builtin::ticket::TicketFeatureAccess::workflow();
        let mut workflow_engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in crate::feature::builtin::ticket::enabled_ticket_definitions(
            workflow_client.clone(),
            workflow_access,
        ) {
            workflow_engine.register_tool(definition);
        }
        let mut workflow_registry = WipMountRegistry::new();
        let route = workflow_registry.allocate_feature_route("ticket").unwrap();
        crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            &mut workflow_registry,
            workflow_client.clone(),
            workflow_access,
            None,
            &route,
        )
        .unwrap();
        let workflow = install_wip_mode_with_mounts(
            &mut workflow_engine,
            None,
            "workflow-worker".into(),
            workflow_registry,
        )
        .unwrap();
        workflow.discover(item.into(), 0, false).await.unwrap();
        workflow
            .inspect("yoi.ticket/item/v1".into(), false)
            .await
            .unwrap();
        let result = workflow
            .call(
                item.into(),
                "yoi.ticket/item/v1".into(),
                "record_relation".into(),
                json!({"kind": "depends_on", "target": "T-10"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let result = result.content.unwrap();
        assert!(result.contains("T-9"));
        assert!(result.contains("T-10"));
        let workflow_requests = workflow_client.requests();
        assert_eq!(workflow_requests.len(), 3);
        assert!(workflow_requests[0].path.ends_with("/tickets/T-9"));
        assert!(workflow_requests[1].path.ends_with("/tickets/T-10"));
        assert!(
            workflow_requests[2]
                .path
                .ends_with("/tickets/T-9/relations")
        );
    }

    #[tokio::test]
    async fn objective_native_runtime_covers_discovery_and_all_backend_operations() {
        let query_response = crate::worker::WorkspaceResponse {
            status: 200,
            body: json!({
                "items": [{
                    "id": "00001OBJECTIVE",
                    "resource_key": "O-3",
                    "title": "Objective",
                    "state": "active",
                    "created_at": null,
                    "updated_at": null,
                    "matched_fields": [],
                    "snippet": null,
                    "linked_ticket_count": 0,
                    "linked_tickets": [],
                    "linked_ticket_keys": []
                }],
                "page": {"next_cursor": null, "has_more": false},
                "record_authority": "workspace_sqlite"
            })
            .to_string(),
        };
        let ticket_response = || crate::worker::WorkspaceResponse {
            status: 200,
            body: json!({"resource_key": "T-7"}).to_string(),
        };
        let client = Arc::new(ScriptedWorkspaceClient::new([
            query_response,
            objective_detail_response("rev-1"),
            objective_detail_response("rev-1"),
            objective_detail_response("rev-2"),
            objective_detail_response("rev-3"),
            ticket_response(),
            objective_detail_response("rev-4"),
            ticket_response(),
            objective_detail_response("rev-5"),
        ]));
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in
            crate::feature::builtin::objective::workspace_http_objective_tools(client.clone())
        {
            engine.register_tool(definition);
        }
        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Allow,
            rules: vec![manifest::ToolPermissionRule {
                tool: "ObjectiveCreate".into(),
                pattern: serde_json::to_string(&json!({"title": "Denied"})).unwrap(),
                action: ToolPermissionAction::Deny,
            }],
        });
        let mut registry = WipMountRegistry::new();
        let feature_route = registry.allocate_feature_route("objective").unwrap();
        crate::feature::builtin::objective::mount_workspace_http_objective_wip(
            &mut registry,
            client.clone(),
            permissions.clone(),
            &feature_route,
        )
        .unwrap();
        let runtime =
            install_wip_mode_with_mounts(&mut engine, permissions, "worker-a".into(), registry)
                .unwrap();

        let discovered = runtime.discover("/".into(), 3, false).await.unwrap();
        let discovered = discovered.content.unwrap();
        assert!(discovered.contains("/features/objective/objectives"));
        assert!(!discovered.contains("/tools/QueryObjective"));
        runtime
            .inspect("yoi.objective/collection/v1".into(), false)
            .await
            .unwrap();
        let collection_path = "/features/objective/objectives";
        runtime
            .call(
                collection_path.into(),
                "yoi.objective/collection/v1".into(),
                "query".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let denied = runtime
            .call(
                collection_path.into(),
                "yoi.objective/collection/v1".into(),
                "create".into(),
                json!({"title": "Denied"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(denied.to_string().contains("PermissionDenied"));
        assert_eq!(client.requests().len(), 1);
        let created = runtime
            .call(
                collection_path.into(),
                "yoi.objective/collection/v1".into(),
                "create".into(),
                json!({"title": "Objective"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(
            created
                .content
                .unwrap()
                .contains("/features/objective/objectives/O-3")
        );

        let item_path = "/features/objective/objectives/O-3";
        runtime.discover(item_path.into(), 0, false).await.unwrap();
        runtime
            .inspect("yoi.objective/item/v1".into(), false)
            .await
            .unwrap();
        for (operation, arguments) in [
            ("read", json!({})),
            ("edit", json!({"title": "Changed"})),
            ("set_state", json!({"state": "paused"})),
            ("link_ticket", json!({"ticket_id": "T-7"})),
            ("unlink_ticket", json!({"ticket_id": "T-7"})),
        ] {
            runtime
                .call(
                    item_path.into(),
                    "yoi.objective/item/v1".into(),
                    operation.into(),
                    arguments,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
        }

        let requests = client.requests();
        assert_eq!(requests.len(), 9);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.contains("ticket-links"))
                .count(),
            2
        );
        assert!(requests.iter().all(|request| !request.path.contains("//")));
    }

    struct FailingTool {
        cancelled: bool,
    }

    #[async_trait]
    impl Tool for FailingTool {
        async fn execute(
            &self,
            _input: &str,
            _context: ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            if self.cancelled {
                Err(ToolError::Cancelled("cancelled safely".to_string().into()))
            } else {
                Err(ToolError::ExecutionFailed("provider reply was lost".into()))
            }
        }
    }

    async fn prepare_runtime_with_tool(name: &str, tool: Arc<dyn Tool>) -> WipRuntime {
        let mut registry = WipMountRegistry::new();
        registry
            .mount(compatibility_projection(meta(name), tool, None).unwrap())
            .unwrap();
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        runtime.discover("/".into(), 2, false).await.unwrap();
        runtime
            .inspect(format!("yoi.tool/{name}/v1"), false)
            .await
            .unwrap();
        runtime
    }

    #[tokio::test]
    async fn cancelled_unknown_and_disconnected_calls_have_distinct_terminal_states() {
        let cancelled =
            prepare_runtime_with_tool("Cancelled", Arc::new(FailingTool { cancelled: true })).await;
        let error = cancelled
            .call(
                "/tools/Cancelled".into(),
                "yoi.tool/Cancelled/v1".into(),
                "call".into(),
                json!({"message": "ok"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::Cancelled(_)));
        assert_eq!(
            cancelled.audit().last().unwrap().outcome,
            WipAuditOutcome::Cancelled
        );

        let unknown =
            prepare_runtime_with_tool("Unknown", Arc::new(FailingTool { cancelled: false })).await;
        let error = unknown
            .call(
                "/tools/Unknown".into(),
                "yoi.tool/Unknown/v1".into(),
                "call".into(),
                json!({"message": "ok"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("outcome unknown"));
        assert_eq!(
            unknown.audit().last().unwrap().outcome,
            WipAuditOutcome::OutcomeUnknown
        );
        unknown
            .discover("/tools/Unknown".into(), 0, true)
            .await
            .unwrap();

        let prepared = {
            let mut state = unknown.state.lock().unwrap();
            let session = state.session.clone();
            let prepared = state
                .client
                .prepare_call(
                    &session,
                    "/tools/Unknown",
                    "yoi.tool/Unknown/v1",
                    "call",
                    BTreeMap::from([(
                        "input".into(),
                        json_to_wip(&json!({"message": "ok"})).unwrap(),
                    )]),
                )
                .unwrap();
            state.client.mark_dispatched(prepared.id).unwrap();
            prepared
        };
        drop(DispatchedCallGuard::new(
            Arc::clone(&unknown.state),
            Arc::clone(&unknown.active),
            prepared.id,
            "dropped-execution".into(),
        ));
        let state = unknown.state.lock().unwrap();
        let record = state
            .client
            .call_history(&state.session)
            .unwrap()
            .back()
            .unwrap();
        assert!(matches!(
            record.outcome,
            CallOutcome::Unknown {
                reason: wip_client::OutcomeUnknownReason::Disconnect,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn projected_permission_policy_is_applied_to_original_tool_identity() {
        let calls = Arc::new(AtomicUsize::new(0));
        let permissions = ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: Vec::new(),
        };
        let host = WipHost::new(registry_with_tool(
            "Echo",
            Arc::clone(&calls),
            Some(permissions),
        ));
        let runtime = WipRuntime::new(host, SecurityContext::new("worker-a"), 0).unwrap();
        runtime.discover("/".into(), 2, false).await.unwrap();
        runtime
            .inspect("yoi.tool/Echo/v1".into(), false)
            .await
            .unwrap();
        let error = runtime
            .call(
                "/tools/Echo".into(),
                "yoi.tool/Echo/v1".into(),
                "call".into(),
                json!({"message": "ok"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("PermissionDenied"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
