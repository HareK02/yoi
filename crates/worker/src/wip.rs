//! Explicit-opt-in Web Interface Protocol (WIP) Worker surface.
//!
//! The adapter intentionally keeps authority in the existing Worker host. In WIP
//! mode ordinary tools are mounted below `/tools`, removed from the model-visible
//! tool list, and invoked through three stateful tree/inspect/invoke tools. The
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
    INTERFACE_FORMAT_V1, InterfaceDescriptor, InterfaceReference, Object, ObjectObservation,
    OperationDeclaration, ParameterDeclaration, ProtocolError, ProtocolErrorCode,
    ProtocolInteraction, ReturnDeclaration, TypeExpr, Value,
};

use crate::permission::permission_action_for;

mod binding;
#[cfg(test)]
mod provider_tests;
use binding::wip_tool_definitions;

#[cfg(test)]
#[path = "checkout_http_tests.rs"]
mod checkout_http_tests;
#[cfg(test)]
#[path = "checkout_wip_tests.rs"]
mod checkout_wip_tests;

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
    #[error("WIP interface `{interface:?}` has conflicting descriptors")]
    InterfaceCollision { interface: InterfaceReference },
    #[error("WIP operation `{operation}` on `{route}` is already contributed by `{existing}`")]
    OperationCollision {
        route: String,
        operation: String,
        existing: String,
    },
    #[error("WIP operation contribution target `{route}` is not a mounted Object")]
    OperationTargetNotFound { route: String },
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
    // Exact post-operation provider state; never substitute a later path stat.
    validator: Option<Vec<u8>>,
}

impl WipOperationOutput {
    pub fn native(value: Value) -> Self {
        Self {
            value,
            tool_output: None,
            validator: None,
        }
    }

    pub fn native_with_validator(value: Value, validator: Vec<u8>) -> Self {
        Self {
            value,
            tool_output: None,
            validator: Some(validator),
        }
    }

    fn compatibility(value: Value, output: ToolOutput) -> Self {
        Self {
            value,
            tool_output: Some(output),
            validator: None,
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
    /// Whether the owning provider currently exposes this Object to its subject.
    fn is_visible(&self) -> bool {
        true
    }

    /// Override the Object's registered validator with current provider state.
    /// None preserves the registered validator and requires no inventory fetch.
    fn object_validator(&self) -> Option<Vec<u8>> {
        None
    }

    /// Current target capability and subject permission intersection. Providers
    /// must still enforce authority at execution; this hook controls publication.
    fn operation_available(&self, _operation: &str) -> bool {
        true
    }

    /// Opt into path-qualified, freshly resolved interface descriptors.
    fn contextual_interface(&self) -> bool {
        false
    }

    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError>;

    async fn cancel(&self, _context: &ToolExecutionContext) -> Result<(), ToolError> {
        Ok(())
    }

    /// Cancel the selected operation on this exact handler instance. Providers
    /// that share a cancellation boundary may keep implementing `cancel`.
    async fn cancel_operation(
        &self,
        _operation: &str,
        context: &ToolExecutionContext,
    ) -> Result<(), ToolError> {
        self.cancel(context).await
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
    pub interface: InterfaceReference,
    pub descriptor: InterfaceDescriptor,
    pub interface_validator: Option<Vec<u8>>,
    pub handler: Arc<dyn WipOperationHandler>,
}

/// An additional Feature's operations for an Object whose existence and route
/// are already owned by the Host registry. Contributions may extend the mounted
/// interface, but cannot replace Object resolution or another operation.
#[derive(Clone)]
pub struct WipOperationContribution {
    pub route: String,
    pub contributor: String,
    pub interface: InterfaceReference,
    pub descriptor: InterfaceDescriptor,
    pub handler: Arc<dyn WipOperationHandler>,
}

/// One dynamically resolved direct-child object beneath a Host-owned collection
/// route. Object resolution stays unique; operation contribution is independent
/// for statically mounted Objects and can be extended without remounting them.
pub struct WipDynamicItem {
    pub object: Object,
    pub handler: Arc<dyn WipOperationHandler>,
}

pub trait WipDynamicItemResolver: Send + Sync {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem>;

    /// Family-level opt-in, needed to deny unqualified descriptor fetches before
    /// resolving any item. A contextual collection owner also opts in its family.
    fn contextual_interface(&self) -> bool {
        false
    }
}

pub struct WipDynamicMount {
    pub collection_route: String,
    pub capability: String,
    pub interface: InterfaceReference,
    pub descriptor: InterfaceDescriptor,
    pub interface_validator: Option<Vec<u8>>,
    pub resolver: Arc<dyn WipDynamicItemResolver>,
}

/// Explicit arbitrary-depth request-time subtree provider. Unlike item families,
/// this resolves both files and directories and enumerates authorized children.
#[async_trait]
pub trait WipSubtreeProvider: Send + Sync {
    /// Provider-specific observation bounds, capped by the Host global bounds.
    fn max_depth(&self) -> u32 {
        32
    }
    fn max_nodes(&self) -> usize {
        1024
    }
    async fn projection(&self, path: &str) -> Result<Option<WipProjection>, ProtocolError>;
    async fn children(&self, path: &str) -> Result<Vec<String>, ProtocolError>;
}

pub struct WipSubtreeMount {
    pub root: String,
    pub provider: Arc<dyn WipSubtreeProvider>,
}

pub trait WipDynamicOperationResolver: Send + Sync {
    fn handler(&self, item_reference: &str) -> Arc<dyn WipOperationHandler>;
}

pub struct WipDynamicOperationContribution {
    pub collection_route: String,
    pub contributor: String,
    pub interface: InterfaceReference,
    pub descriptor: InterfaceDescriptor,
    pub resolver: Arc<dyn WipDynamicOperationResolver>,
}

struct MountedDynamicOperationContribution {
    contributor: String,
    operations: Vec<String>,
    resolver: Arc<dyn WipDynamicOperationResolver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipNamespaceRoute {
    root: String,
}

impl WipNamespaceRoute {
    pub fn root(&self) -> &str {
        &self.root
    }
}

struct MountedProjection {
    projection: WipProjection,
    operation_handlers: BTreeMap<String, (String, Arc<dyn WipOperationHandler>)>,
}

impl MountedProjection {
    fn resolved(&self) -> WipProjection {
        let mut projection = self.projection.clone();
        projection.handler = Arc::new(OperationDispatchHandler {
            owner: Arc::clone(&self.projection.handler),
            handlers: self
                .operation_handlers
                .iter()
                .map(|(operation, (_, handler))| (operation.clone(), Arc::clone(handler)))
                .collect(),
        });
        projection
    }
}

struct OperationDispatchHandler {
    owner: Arc<dyn WipOperationHandler>,
    handlers: BTreeMap<String, Arc<dyn WipOperationHandler>>,
}

#[async_trait]
impl WipOperationHandler for OperationDispatchHandler {
    fn is_visible(&self) -> bool {
        self.owner.is_visible()
    }

    fn object_validator(&self) -> Option<Vec<u8>> {
        self.owner.object_validator()
    }

    fn operation_available(&self, operation: &str) -> bool {
        self.handlers
            .get(operation)
            .is_some_and(|handler| handler.operation_available(operation))
    }

    fn contextual_interface(&self) -> bool {
        self.owner.contextual_interface()
            || self
                .handlers
                .values()
                .any(|handler| handler.contextual_interface())
    }

    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let handler = self.handlers.get(operation).ok_or_else(|| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::OperationNotFound,
                "operation is not published by the selected interface",
            ))
        })?;
        handler.call(operation, arguments, context).await
    }

    async fn cancel_operation(
        &self,
        operation: &str,
        context: &ToolExecutionContext,
    ) -> Result<(), ToolError> {
        let handler = self.handlers.get(operation).ok_or_else(|| {
            ToolError::InvalidArgument("WIP cancellation operation is not published".into())
        })?;
        handler.cancel_operation(operation, context).await
    }
}

#[derive(Default)]
pub struct WipMountRegistry {
    mounts: BTreeMap<String, MountedProjection>,
    additional_interfaces: BTreeMap<String, Vec<WipProjection>>,
    dynamic_mounts: Vec<WipDynamicMount>,
    subtree_mounts: Vec<WipSubtreeMount>,
    dynamic_operation_contributions: BTreeMap<String, Vec<MountedDynamicOperationContribution>>,
    namespaces: BTreeMap<String, String>,
    replaced_compatibility_capabilities: BTreeMap<String, String>,
}

impl WipMountRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate one Host-owned root namespace to its unique Object provider.
    /// The provider identity is registry metadata, never a public path segment.
    pub fn allocate_namespace(
        &mut self,
        owner: &str,
        namespace: &str,
    ) -> Result<WipNamespaceRoute, WipMountError> {
        let valid_segment = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if !valid_segment(owner) {
            return Err(WipMountError::InvalidRoute {
                route: format!("/{namespace}"),
                message: "namespace owner must use lowercase ASCII letters, digits, or '-'".into(),
            });
        }
        if !valid_segment(namespace) {
            return Err(WipMountError::InvalidRoute {
                route: format!("/{namespace}"),
                message: "root namespace must be one lowercase ASCII path segment".into(),
            });
        }
        if matches!(namespace, "features" | "tools") {
            return Err(WipMountError::InvalidRoute {
                route: format!("/{namespace}"),
                message: "the Host reserves this root namespace".into(),
            });
        }
        let root = format!("/{namespace}");
        if let Some(existing) = self.namespaces.get(&root) {
            return Err(WipMountError::RouteCollision {
                route: root,
                existing: format!("namespace owner `{existing}`"),
            });
        }
        if self.mounts.contains_key(&root)
            || self
                .mounts
                .keys()
                .any(|route| route.starts_with(&format!("{root}/")))
        {
            return Err(WipMountError::RouteCollision {
                route: root,
                existing: "already mounted route".into(),
            });
        }
        self.namespaces.insert(root.clone(), owner.to_string());
        Ok(WipNamespaceRoute { root })
    }

    pub fn mount(
        &mut self,
        projection: WipProjection,
    ) -> Result<WipMountDisposition, WipMountError> {
        validate_projection(&projection)?;
        if projection.kind == WipProjectionKind::Compatibility
            && self.replaces_compatibility(&projection.capability)
        {
            return Ok(WipMountDisposition::NativeAlreadySelected);
        }
        if projection.kind == WipProjectionKind::Compatibility && !is_tool_route(&projection.route)
        {
            return Err(WipMountError::InvalidProjection {
                route: projection.route,
                message: "compatibility Objects must use /tools/<tool-name>".into(),
            });
        }
        if projection.kind == WipProjectionKind::Native
            && projection.route != WIP_ROOT
            && !is_tool_route(&projection.route)
        {
            let namespace_root = projection
                .route
                .strip_prefix('/')
                .and_then(|path| path.split('/').next())
                .map(|segment| format!("/{segment}"))
                .unwrap_or_default();
            let Some(owner) = self.namespaces.get(&namespace_root) else {
                return Err(WipMountError::InvalidProjection {
                    route: projection.route,
                    message: "native Object route requires an allocated Host namespace".into(),
                });
            };
            if projection.capability.split(':').next() != Some(owner.as_str()) {
                return Err(WipMountError::InvalidProjection {
                    route: projection.route,
                    message: format!(
                        "native Object capability does not belong to namespace owner `{owner}`"
                    ),
                });
            }
        }
        if self.subtree_mounts.iter().any(|mount| {
            projection.route == mount.root
                || projection.route.starts_with(&format!("{}/", mount.root))
        }) {
            return Err(WipMountError::RouteCollision {
                route: projection.route,
                existing: "request-time subtree".into(),
            });
        }
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
                    let operation_handlers =
                        operation_handlers(&projection, &projection.capability);
                    self.mounts.insert(
                        projection.route.clone(),
                        MountedProjection {
                            projection,
                            operation_handlers,
                        },
                    );
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
        let operation_handlers = operation_handlers(&projection, &projection.capability);
        self.mounts.insert(
            route,
            MountedProjection {
                projection,
                operation_handlers,
            },
        );
        Ok(WipMountDisposition::Mounted)
    }

    /// Publish a separate Interface on an existing Object without flattening its
    /// operation namespace. Object resolution/identity and authority remain owned
    /// by the original provider; each Interface keeps its exact dispatch handler.
    pub fn mount_interface(&mut self, projection: WipProjection) -> Result<(), WipMountError> {
        validate_projection(&projection)?;
        let Some(owner) = self.mounts.get(&projection.route) else {
            return Err(WipMountError::OperationTargetNotFound {
                route: projection.route,
            });
        };
        let object = &owner.projection.object;
        if projection.capability != owner.projection.capability
            || projection.kind != owner.projection.kind
            || projection.object.name != object.name
            || projection.object.r#ref != object.r#ref
            || projection.object.validator != object.validator
            || projection.object.description != object.description
        {
            return Err(WipMountError::InvalidProjection {
                route: projection.route,
                message: "additional Interface cannot replace Object ownership or identity".into(),
            });
        }
        if owner.projection.interface == projection.interface
            || self
                .additional_interfaces
                .get(&projection.route)
                .is_some_and(|all| all.iter().any(|p| p.interface == projection.interface))
        {
            return Err(WipMountError::InterfaceCollision {
                interface: projection.interface,
            });
        }
        self.ensure_interface_available(&projection, None)?;
        self.additional_interfaces
            .entry(projection.route.clone())
            .or_default()
            .push(projection);
        Ok(())
    }

    /// Add operations from another Feature without changing Object ownership or
    /// resolution. Interface shape and operation names are global protocol
    /// contracts, so incompatible declarations fail at registration time.
    pub fn contribute_operations(
        &mut self,
        contribution: WipOperationContribution,
    ) -> Result<(), WipMountError> {
        contribution
            .descriptor
            .validate()
            .map_err(|error| WipMountError::InvalidProjection {
                route: contribution.route.clone(),
                message: error.to_string(),
            })?;
        let Some(mounted) = self.mounts.get(&contribution.route) else {
            return Err(WipMountError::OperationTargetNotFound {
                route: contribution.route,
            });
        };
        if mounted.projection.interface != contribution.interface {
            return Err(WipMountError::InterfaceCollision {
                interface: contribution.interface,
            });
        }
        if !same_interface_shape(&mounted.projection.descriptor, &contribution.descriptor) {
            return Err(WipMountError::InterfaceCollision {
                interface: contribution.interface,
            });
        }
        for operation in &contribution.descriptor.operations {
            if let Some((existing, _)) = mounted.operation_handlers.get(&operation.name) {
                return Err(WipMountError::OperationCollision {
                    route: contribution.route,
                    operation: operation.name.clone(),
                    existing: existing.clone(),
                });
            }
        }

        let mut merged = mounted.projection.descriptor.clone();
        merged
            .operations
            .extend(contribution.descriptor.operations.iter().cloned());
        merged
            .operations
            .sort_by(|left, right| left.name.cmp(&right.name));
        merged
            .validate()
            .map_err(|error| WipMountError::InvalidProjection {
                route: contribution.route.clone(),
                message: error.to_string(),
            })?;
        if self.interface_conflicts(
            &contribution.interface,
            &merged,
            Some(&descriptor_validator(&merged)),
            Some(&contribution.route),
        ) {
            return Err(WipMountError::InterfaceCollision {
                interface: contribution.interface,
            });
        }

        let mounted = self
            .mounts
            .get_mut(&contribution.route)
            .expect("operation contribution target was checked");
        for operation in contribution.descriptor.operations {
            mounted.operation_handlers.insert(
                operation.name,
                (
                    contribution.contributor.clone(),
                    Arc::clone(&contribution.handler),
                ),
            );
        }
        mounted.projection.descriptor = merged;
        mounted.projection.interface_validator =
            Some(descriptor_validator(&mounted.projection.descriptor));
        Ok(())
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
        if self.subtree_mounts.iter().any(|m| {
            m.root == mount.collection_route
                || m.root.starts_with(&format!("{}/", mount.collection_route))
                || mount.collection_route.starts_with(&format!("{}/", m.root))
        }) {
            return Err(WipMountError::RouteCollision {
                route: mount.collection_route,
                existing: "request-time subtree".into(),
            });
        }
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
        if mount.capability.split(':').next() != collection.projection.capability.split(':').next()
        {
            return Err(WipMountError::InvalidProjection {
                route: mount.collection_route,
                message: "dynamic Object capability must belong to the collection owner".into(),
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

    /// Delegate an entire descendant namespace to one request-time provider.
    /// The root Object remains registry-owned; nested/static/item collisions fail.
    pub fn mount_subtree(&mut self, mount: WipSubtreeMount) -> Result<(), WipMountError> {
        let Some(root) = self.mounts.get(&mount.root) else {
            return Err(WipMountError::OperationTargetNotFound { route: mount.root });
        };
        if root.projection.kind != WipProjectionKind::Native {
            return Err(WipMountError::InvalidProjection {
                route: mount.root,
                message: "subtree requires a native root Object".into(),
            });
        }
        let overlaps = |path: &str| {
            path == mount.root
                || path.starts_with(&format!("{}/", mount.root))
                || mount.root.starts_with(&format!("{path}/"))
        };
        if self.subtree_mounts.iter().any(|m| overlaps(&m.root))
            || self
                .dynamic_mounts
                .iter()
                .any(|m| overlaps(&m.collection_route))
            || self
                .mounts
                .keys()
                .any(|p| p != &mount.root && p.starts_with(&format!("{}/", mount.root)))
        {
            return Err(WipMountError::RouteCollision {
                route: mount.root,
                existing: "subtree/static/item provider".into(),
            });
        }
        self.subtree_mounts.push(mount);
        Ok(())
    }

    /// Add operations to every item resolved by one existing dynamic Object
    /// family. The family keeps exactly one Object resolver; each Feature only
    /// supplies handlers for its own disjoint operation names.
    pub fn contribute_dynamic_operations(
        &mut self,
        contribution: WipDynamicOperationContribution,
    ) -> Result<(), WipMountError> {
        contribution
            .descriptor
            .validate()
            .map_err(|error| WipMountError::InvalidProjection {
                route: contribution.collection_route.clone(),
                message: error.to_string(),
            })?;
        let Some(index) = self
            .dynamic_mounts
            .iter()
            .position(|mount| mount.collection_route == contribution.collection_route)
        else {
            return Err(WipMountError::OperationTargetNotFound {
                route: contribution.collection_route,
            });
        };
        let mounted = &self.dynamic_mounts[index];
        if mounted.interface != contribution.interface
            || !same_interface_shape(&mounted.descriptor, &contribution.descriptor)
        {
            return Err(WipMountError::InterfaceCollision {
                interface: contribution.interface,
            });
        }
        for operation in &contribution.descriptor.operations {
            if mounted
                .descriptor
                .operations
                .iter()
                .any(|existing| existing.name == operation.name)
            {
                let existing = self
                    .dynamic_operation_contributions
                    .get(&contribution.collection_route)
                    .and_then(|contributions| {
                        contributions.iter().find(|existing| {
                            existing
                                .operations
                                .iter()
                                .any(|name| name == &operation.name)
                        })
                    })
                    .map(|existing| existing.contributor.clone())
                    .unwrap_or_else(|| mounted.capability.clone());
                return Err(WipMountError::OperationCollision {
                    route: contribution.collection_route,
                    operation: operation.name.clone(),
                    existing,
                });
            }
        }
        let mut merged = mounted.descriptor.clone();
        merged
            .operations
            .extend(contribution.descriptor.operations.iter().cloned());
        merged
            .operations
            .sort_by(|left, right| left.name.cmp(&right.name));
        merged
            .validate()
            .map_err(|error| WipMountError::InvalidProjection {
                route: contribution.collection_route.clone(),
                message: error.to_string(),
            })?;
        let validator = descriptor_validator(&merged);
        let conflicts_static = self.registry_interface_conflicts_static(
            &contribution.interface,
            &merged,
            Some(&validator),
        );
        let conflicts_dynamic = self.dynamic_mounts.iter().any(|other| {
            other.collection_route != contribution.collection_route
                && other.interface == contribution.interface
                && (other.descriptor != merged
                    || other.interface_validator.as_deref() != Some(validator.as_slice()))
        });
        if conflicts_static || conflicts_dynamic {
            return Err(WipMountError::InterfaceCollision {
                interface: contribution.interface,
            });
        }

        let operations = contribution
            .descriptor
            .operations
            .into_iter()
            .map(|operation| operation.name)
            .collect();
        let mounted = &mut self.dynamic_mounts[index];
        mounted.descriptor = merged;
        mounted.interface_validator = Some(validator);
        self.dynamic_operation_contributions
            .entry(contribution.collection_route)
            .or_default()
            .push(MountedDynamicOperationContribution {
                contributor: contribution.contributor,
                operations,
                resolver: contribution.resolver,
            });
        Ok(())
    }

    fn registry_interface_conflicts_static(
        &self,
        interface: &InterfaceReference,
        descriptor: &InterfaceDescriptor,
        validator: Option<&[u8]>,
    ) -> bool {
        self.mounts.values().any(|mounted| {
            mounted.projection.interface == *interface
                && (mounted.projection.descriptor != *descriptor
                    || mounted.projection.interface_validator.as_deref() != validator)
        })
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
            self.mounts.retain(|_, mounted| {
                mounted.projection.kind != WipProjectionKind::Compatibility
                    || mounted.projection.capability != capability
            });
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
        interface: &InterfaceReference,
        descriptor: &InterfaceDescriptor,
        validator: Option<&[u8]>,
        replacing_route: Option<&str>,
    ) -> bool {
        self.additional_interfaces.iter().any(|(route, all)| {
            Some(route.as_str()) != replacing_route
                && all.iter().any(|p| {
                    p.interface == *interface
                        && (p.descriptor != *descriptor
                            || p.interface_validator.as_deref() != validator)
                })
        }) || self.mounts.iter().any(|(route, mounted)| {
            Some(route.as_str()) != replacing_route
                && mounted.projection.interface == *interface
                && (mounted.projection.descriptor != *descriptor
                    || mounted.projection.interface_validator.as_deref() != validator)
        }) || self.dynamic_mounts.iter().any(|mounted| {
            mounted.interface == *interface
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

fn operation_handlers(
    projection: &WipProjection,
    contributor: &str,
) -> BTreeMap<String, (String, Arc<dyn WipOperationHandler>)> {
    projection
        .descriptor
        .operations
        .iter()
        .map(|operation| {
            (
                operation.name.clone(),
                (contributor.to_string(), Arc::clone(&projection.handler)),
            )
        })
        .collect()
}

fn same_interface_shape(left: &InterfaceDescriptor, right: &InterfaceDescriptor) -> bool {
    left.format == right.format
        && left.documentation == right.documentation
        && left.types == right.types
}

fn descriptor_validator(descriptor: &InterfaceDescriptor) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(format!("{descriptor:?}").as_bytes());
    digest.finalize().to_vec()
}

/// Explicit deployment root registration, never a legacy wire parser.
pub(crate) fn root_reference(name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: "/".into(),
        name: name.into(),
    }
}
/// A contextual descriptor belongs to the resolved Object's publication.
pub(crate) fn contextual_reference(base: &str, path: &str) -> InterfaceReference {
    InterfaceReference {
        scope: path.into(),
        name: base.into(),
    }
}

fn is_tool_route(route: &str) -> bool {
    route
        .strip_prefix("/tools/")
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

fn validate_projection(projection: &WipProjection) -> Result<(), WipMountError> {
    wip_protocol::validate_path(&projection.route).map_err(|error| {
        WipMountError::InvalidRoute {
            route: projection.route.clone(),
            message: error.to_string(),
        }
    })?;
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

    fn subtree(&self, path: &str) -> Option<&WipSubtreeMount> {
        self.registry
            .subtree_mounts
            .iter()
            .find(|m| path == m.root || path.starts_with(&format!("{}/", m.root)))
    }

    async fn projection_live(&self, path: &str) -> Result<Option<WipProjection>, ProtocolError> {
        wip_protocol::validate_path(path).map_err(|_| unpublished_path_error(path))?;
        if let Some(mount) = self.subtree(path) {
            let Some(projection) = mount.provider.projection(path).await? else {
                return Ok(None);
            };
            validate_projection(&projection).map_err(|_| {
                protocol_error(
                    ProtocolErrorCode::Internal,
                    "subtree returned an invalid Object",
                )
            })?;
            let owner = &self.registry.mounts[&mount.root].projection.capability;
            if projection.route != path
                || projection.kind != WipProjectionKind::Native
                || projection.capability.split(':').next() != owner.split(':').next()
            {
                return Err(protocol_error(
                    ProtocolErrorCode::Internal,
                    "subtree changed route or ownership",
                ));
            }
            return Ok(Some(projection));
        }
        Ok(self.projection(path))
    }

    async fn projection_for_interface_live(
        &self,
        path: &str,
        reference: &InterfaceReference,
    ) -> Result<Option<WipProjection>, ProtocolError> {
        let Some(owner) = self.projection_live(path).await? else {
            return Ok(None);
        };
        if owner.interface == *reference {
            return Ok(Some(owner));
        }
        if let Some(all) = self.registry.additional_interfaces.get(path) {
            for other in all {
                if let Some(mut selected) = self.project_current(other.clone(), false)
                    && selected.interface == *reference
                {
                    selected.object = owner.object;
                    return Ok(Some(selected));
                }
            }
        }
        // Retain the owner so preconditions report InterfaceMismatch with target precedence.
        Ok(Some(owner))
    }

    async fn descriptor_live(
        &self,
        reference: &InterfaceReference,
    ) -> Result<Option<(InterfaceDescriptor, Option<Vec<u8>>)>, ProtocolError> {
        if self.subtree(&reference.scope).is_some() {
            return Ok(self
                .projection_live(&reference.scope)
                .await?
                .filter(|p| p.interface == *reference)
                .map(|p| (p.descriptor, p.interface_validator)));
        }
        // Never expose the static registration placeholder descriptor of a subtree.
        if self
            .registry
            .subtree_mounts
            .iter()
            .any(|m| self.registry.mounts[&m.root].projection.interface == *reference)
        {
            return Ok(None);
        }
        Ok(self.descriptor(reference))
    }

    async fn fetch_interface_live(
        &self,
        reference: &InterfaceReference,
    ) -> Result<FetchInterfaceResponse, ProtocolError> {
        reference
            .validate()
            .map_err(|e| protocol_error(ProtocolErrorCode::InvalidRequest, e.to_string()))?;
        if let Some(projection) = self
            .projection_for_interface_live(&reference.scope, reference)
            .await?
            && projection.interface == *reference
        {
            return Ok(FetchInterfaceResponse {
                interface: reference.clone(),
                scope_ref: projection.object.r#ref,
                descriptor: projection.descriptor,
                validator: projection.interface_validator,
            });
        }
        // Shared static registrations belong to the pinned published scope, not
        // to an arbitrary target's identity. A missing scope ends publication.
        let scope = self.object_at(&reference.scope).ok_or_else(|| {
            protocol_error(
                ProtocolErrorCode::InterfaceNotFound,
                "scope is not published",
            )
        })?;
        let (descriptor, validator) = self.descriptor_live(reference).await?.ok_or_else(|| {
            protocol_error(
                ProtocolErrorCode::InterfaceNotFound,
                "interface is not published",
            )
        })?;
        Ok(FetchInterfaceResponse {
            interface: reference.clone(),
            scope_ref: scope.r#ref,
            descriptor,
            validator,
        })
    }

    async fn observe_live(
        &self,
        path: &str,
        depth: u32,
    ) -> Result<ObjectObservation, ProtocolError> {
        if depth > 32 {
            return Err(protocol_error(
                ProtocolErrorCode::ResourceLimitExceeded,
                "observation depth exceeds 32",
            ));
        }
        let mut remaining = 1024usize;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.observe_live_node(path, depth, &mut remaining),
        )
        .await
        .map_err(|_| {
            protocol_error(
                ProtocolErrorCode::ResourceLimitExceeded,
                "observation deadline exceeded",
            )
        })?
    }

    fn observe_live_node<'a>(
        &'a self,
        path: &'a str,
        depth: u32,
        remaining: &'a mut usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ObjectObservation, ProtocolError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if let Some(mount) = self.subtree(path) {
                if depth > mount.provider.max_depth() {
                    return Err(protocol_error(
                        ProtocolErrorCode::ResourceLimitExceeded,
                        "observation exceeds provider depth limit",
                    ));
                }
                *remaining = (*remaining).min(mount.provider.max_nodes());
            }
            if *remaining == 0 {
                return Err(protocol_error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "observation exceeds 1024 nodes",
                ));
            }
            *remaining -= 1;
            let object = if self.subtree(path).is_some() {
                self.projection_live(path).await?.map(|p| p.object)
            } else {
                self.object_at(path)
            }
            .ok_or_else(|| unpublished_path_error(path))?;
            let children = if depth == 0 {
                None
            } else {
                let paths = if let Some(mount) = self.subtree(path) {
                    mount.provider.children(path).await?
                } else {
                    self.children(path)
                };
                let mut values = Vec::new();
                let mut seen = BTreeSet::new();
                for child in paths {
                    let prefix = if path == "/" {
                        "/".to_string()
                    } else {
                        format!("{path}/")
                    };
                    let suffix = child.strip_prefix(&prefix).ok_or_else(|| {
                        protocol_error(ProtocolErrorCode::Internal, "subtree child outside parent")
                    })?;
                    if suffix.is_empty() || suffix.contains('/') || !seen.insert(child.clone()) {
                        return Err(protocol_error(
                            ProtocolErrorCode::Internal,
                            "invalid/duplicate subtree child",
                        ));
                    }
                    values.push(self.observe_live_node(&child, depth - 1, remaining).await?);
                }
                Some(values)
            };
            Ok(ObjectObservation { object, children })
        })
    }

    fn descriptor(
        &self,
        reference: &InterfaceReference,
    ) -> Option<(InterfaceDescriptor, Option<Vec<u8>>)> {
        for (path, all) in &self.registry.additional_interfaces {
            if self.projection(path).is_none() {
                continue;
            }
            for p in all {
                if let Some(current) = self.project_current(p.clone(), false)
                    && current.interface == *reference
                {
                    return Some((current.descriptor, current.interface_validator));
                }
            }
        }
        for (path, mounted) in &self.registry.mounts {
            if mounted.projection.interface == *reference {
                let Some(projection) = self.projection(path) else {
                    continue;
                };
                if projection.interface == *reference {
                    return Some((projection.descriptor, projection.interface_validator));
                }
            }
        }
        if let Some(descriptor) = self.registry.dynamic_mounts.iter().find_map(|mounted| {
            if mounted.interface != *reference || self.dynamic_contextual(mounted) {
                return None;
            }
            // Legacy families retain their shared descriptor. Contextual
            // families must always resolve a qualified item reference below.
            Some((
                mounted.descriptor.clone(),
                mounted.interface_validator.clone(),
            ))
        }) {
            return Some(descriptor);
        }
        let projection = self.projection(&reference.scope)?;
        // Re-resolve the exact current Object, not a cached descriptor or an
        // arbitrary base interface with an appended path. Exact legacy refs
        // above remain opaque even if they contain our contextual delimiter.
        if projection.interface != *reference {
            return None;
        }
        Some((projection.descriptor, projection.interface_validator))
    }

    fn dynamic_contextual(&self, mounted: &WipDynamicMount) -> bool {
        mounted.resolver.contextual_interface()
            || self.registry.mounts[&mounted.collection_route]
                .resolved()
                .handler
                .contextual_interface()
    }

    fn project_current(
        &self,
        mut projection: WipProjection,
        contextual_family: bool,
    ) -> Option<WipProjection> {
        if !projection.handler.is_visible() {
            return None;
        }
        if let Some(validator) = projection.handler.object_validator() {
            projection.object.validator = Some(validator);
        }
        let contextual = contextual_family || projection.handler.contextual_interface();
        let original_len = projection.descriptor.operations.len();
        projection
            .descriptor
            .operations
            .retain(|operation| projection.handler.operation_available(&operation.name));
        if contextual || projection.descriptor.operations.len() != original_len {
            let mut digest = Sha256::new();
            digest.update(descriptor_validator(&projection.descriptor));
            // Keep an explicit provider version authoritative even when the
            // filtered shape is unchanged.
            if let Some(validator) = &projection.interface_validator {
                digest.update(validator);
            }
            projection.interface_validator = Some(digest.finalize().to_vec());
        }
        if contextual {
            projection.interface =
                contextual_reference(&projection.interface.name, &projection.route);
            projection.object.interfaces = vec![projection.interface.clone()];
        }
        Some(projection)
    }

    fn projection(&self, path: &str) -> Option<WipProjection> {
        if let Some(mounted) = self.registry.mounts.get(path) {
            let mut projection = self.project_current(mounted.resolved(), false)?;
            if let Some(all) = self.registry.additional_interfaces.get(path) {
                for other in all {
                    if let Some(current) = self.project_current(other.clone(), false) {
                        projection.object.interfaces.push(current.interface);
                    }
                }
            }
            return Some(projection);
        }
        self.registry.dynamic_mounts.iter().find_map(|mounted| {
            let item_reference = path.strip_prefix(&format!("{}/", mounted.collection_route))?;
            if item_reference.is_empty() || item_reference.contains('/') {
                return None;
            }
            // Collection listing authority is independent of item read authority.
            // The resolved item's owner gates its own visibility below.
            let item = mounted.resolver.resolve(item_reference)?;
            let contributions = self
                .registry
                .dynamic_operation_contributions
                .get(&mounted.collection_route);
            let contributed_names = contributions
                .into_iter()
                .flatten()
                .flat_map(|contribution| contribution.operations.iter())
                .collect::<BTreeSet<_>>();
            let mut handlers = mounted
                .descriptor
                .operations
                .iter()
                .filter(|operation| !contributed_names.contains(&operation.name))
                .map(|operation| (operation.name.clone(), Arc::clone(&item.handler)))
                .collect::<BTreeMap<_, _>>();
            for contribution in contributions.into_iter().flatten() {
                let handler = contribution.resolver.handler(item_reference);
                for operation in &contribution.operations {
                    handlers.insert(operation.clone(), Arc::clone(&handler));
                }
            }
            let projection = WipProjection {
                route: path.to_string(),
                capability: mounted.capability.clone(),
                kind: WipProjectionKind::Native,
                object: item.object,
                interface: mounted.interface.clone(),
                descriptor: mounted.descriptor.clone(),
                interface_validator: mounted.interface_validator.clone(),
                handler: Arc::new(OperationDispatchHandler {
                    owner: item.handler,
                    handlers,
                }),
            };
            validate_projection(&projection).ok()?;
            self.project_current(projection, self.dynamic_contextual(mounted))
        })
    }

    fn object_at(&self, path: &str) -> Option<Object> {
        // An explicit Object can also have static descendants. Do not replace
        // its identity, interfaces, or validator with a synthetic namespace.
        if let Some(projection) = self.projection(path) {
            return Some(projection.object);
        }
        if self.registry.mounts.contains_key(path)
            || self
                .registry
                .dynamic_mounts
                .iter()
                .any(|mount| path.starts_with(&format!("{}/", mount.collection_route)))
        {
            // A denied native Object must not reappear as a synthetic namespace.
            return None;
        }
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
        None
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
            .any(|route| route.starts_with(&prefix) && self.projection(route).is_some())
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
            if self.object_at(&child).is_some() {
                children.insert(child);
            }
        }
        children.into_iter().collect()
    }

    #[cfg(test)]
    fn observe(&self, path: &str, depth: u32) -> Result<ObjectObservation, ProtocolError> {
        let object = self
            .object_at(path)
            .ok_or_else(|| unpublished_path_error(path))?;
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

    #[cfg(test)]
    fn fetch_interface(
        &self,
        reference: &InterfaceReference,
    ) -> Result<FetchInterfaceResponse, ProtocolError> {
        let (descriptor, validator) = self.descriptor(reference).ok_or_else(|| {
            protocol_error(
                ProtocolErrorCode::InterfaceNotFound,
                "interface is not published",
            )
        })?;
        Ok(FetchInterfaceResponse {
            interface: reference.clone(),
            scope_ref: self
                .object_at(&reference.scope)
                .and_then(|object| object.r#ref),
            descriptor,
            validator,
        })
    }

    #[cfg(test)]
    async fn call(
        &self,
        request: CallOperationRequest,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        request.validate().map_err(|e| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidRequest,
                e.to_string(),
            ))
        })?;
        let projection = self
            .projection_for_interface_live(&request.target.path, &request.interface.reference)
            .await
            .map_err(WipOperationError::Protocol)?
            .ok_or_else(|| {
                WipOperationError::Protocol(unpublished_path_error(&request.target.path))
            })?;
        self.call_projection(projection, request, context).await
    }

    // The deployment root is pinned; self-scoped interfaces use the exact same
    // resolved Object snapshot as descriptor/handler selection, not an artifact lookup.
    fn scope_matches(
        &self,
        projection: &WipProjection,
        interface: &wip_protocol::InterfaceTarget,
    ) -> Result<bool, ProtocolError> {
        let scope = if interface.reference.scope == projection.route {
            Some(projection.object.clone())
        } else {
            self.object_at(&interface.reference.scope)
        };
        Ok(scope.is_some_and(|scope| {
            interface
                .scope_ref
                .as_ref()
                .is_none_or(|expected| scope.r#ref.as_ref() == Some(expected))
        }))
    }

    fn check_call_preconditions(
        &self,
        projection: &WipProjection,
        target: &wip_protocol::Target,
        interface: &wip_protocol::InterfaceTarget,
        operation_name: &str,
    ) -> Result<(), ProtocolError> {
        if let Some(actual) = target.validator.as_deref()
            && projection.object.validator.as_deref() != Some(actual)
        {
            return Err(protocol_error(
                ProtocolErrorCode::ValidatorMismatch,
                "object validator is stale or unavailable",
            ));
        }
        if interface.reference.validate_for_path(&target.path).is_err()
            || interface.reference != projection.interface
            || !projection.object.interfaces.contains(&interface.reference)
            || !self.scope_matches(projection, interface)?
        {
            return Err(protocol_error(
                ProtocolErrorCode::InterfaceMismatch,
                "interface is not a member of the target object",
            ));
        }
        match (
            projection.interface_validator.as_deref(),
            interface.validator.as_deref(),
        ) {
            (Some(_), None) => {
                return Err(protocol_error(
                    ProtocolErrorCode::InterfaceValidatorRequired,
                    "interface validator is required",
                ));
            }
            (expected, Some(actual)) if expected != Some(actual) => {
                return Err(protocol_error(
                    ProtocolErrorCode::InterfaceValidatorMismatch,
                    "interface validator is stale",
                ));
            }
            _ => {}
        }
        if !projection
            .descriptor
            .operations
            .iter()
            .any(|operation| operation.name == operation_name)
        {
            return Err(protocol_error(
                ProtocolErrorCode::OperationNotFound,
                "operation is not published by the selected interface",
            ));
        }
        if projection.object.validator.is_some() && target.validator.is_none() {
            return Err(protocol_error(
                ProtocolErrorCode::ValidatorRequired,
                "object validator is required",
            ));
        }
        Ok(())
    }

    async fn call_projection(
        &self,
        projection: WipProjection,
        request: CallOperationRequest,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        request.validate().map_err(|e| {
            WipOperationError::Protocol(protocol_error(
                ProtocolErrorCode::InvalidRequest,
                e.to_string(),
            ))
        })?;
        self.check_call_preconditions(
            &projection,
            &request.target,
            &request.interface,
            &request.operation,
        )
        .map_err(WipOperationError::Protocol)?;
        self.execute_projection(projection, request, context).await
    }

    // Preconditions have already fixed publication; do not resolve target or
    // scope again between argument decoding and the provider execution boundary.
    async fn execute_projection(
        &self,
        projection: WipProjection,
        request: CallOperationRequest,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
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
}

fn unpublished_path_error(path: &str) -> ProtocolError {
    let message = if path == "/features" || path.starts_with("/features/") {
        "legacy /features/... routes are no longer published; rediscover from `/` and use the root namespace"
    } else {
        "target path is not published"
    };
    protocol_error(ProtocolErrorCode::NotFound, message)
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
    let interface = root_reference(&format!("yoi.tool/{}/v1", meta.name));
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

#[derive(Clone)]
struct ActiveOperation {
    operation: String,
    handler: Arc<dyn WipOperationHandler>,
}

pub struct WipRuntime {
    endpoint: Endpoint,
    wire_limits: Limits,
    #[cfg(test)]
    client_limits: ClientLimits,
    security_context: SecurityContext,
    state: Arc<Mutex<ClientState>>,
    host: Arc<WipHost>,
    active: Arc<Mutex<HashMap<String, ActiveOperation>>>,
    audit: Arc<Mutex<VecDeque<WipAuditRecord>>>,
    metrics: Arc<WipMetrics>,
}

impl WipRuntime {
    /// Build an isolated Client/Host endpoint from native application mounts.
    /// Useful to embedders and in-process integration tests without an LLM Engine.
    pub fn from_mounts(
        registry: WipMountRegistry,
        security_context: String,
    ) -> Result<Self, String> {
        Self::new(
            WipHost::new(registry),
            SecurityContext::new(security_context),
            0,
        )
    }

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
            #[cfg(test)]
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

    #[cfg(test)]
    fn reset_observations(&self) -> Result<(), ToolError> {
        let mut client = Client::new(self.client_limits, self.wire_limits);
        let session = client
            .open_session(WIP_ENDPOINT, self.security_context.clone())
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        *self.state.lock().unwrap_or_else(|error| error.into_inner()) =
            ClientState { client, session };
        Ok(())
    }

    pub(crate) async fn call(
        &self,
        path: String,
        interface: InterfaceReference,
        operation: String,
        arguments: Json,
        execution: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let descriptor = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state
                .client
                .interface(&state.session, &interface)
                .and_then(|observation| observation.descriptor.clone())
                .ok_or_else(|| {
                    ToolError::InvalidArgument("Interface has not been observed".into())
                })?
        };
        let arguments = decode_native_arguments(
            &path,
            &interface,
            &operation,
            &arguments,
            &descriptor,
            self.wire_limits,
        )
        .map_err(ToolError::InvalidArgument)?;
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
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&execution.execution_id())
            .cloned();
        match active {
            Some(active) => {
                active
                    .handler
                    .cancel_operation(&active.operation, execution)
                    .await
            }
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
                match self
                    .host
                    .observe_live(&request_value.path, request_value.depth)
                    .await
                {
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
                match self
                    .host
                    .fetch_interface_live(&request_value.interface)
                    .await
                {
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
        let rejected = |error: ProtocolError| -> Result<_, ToolError> {
            Ok((
                encode_protocol_error_response(
                    ProtocolInteraction::CallOperation,
                    &error,
                    self.wire_limits,
                )
                .map_err(|e| ToolError::Internal(e.to_string()))?,
                None,
                WipAuditOutcome::Rejected,
            ))
        };
        let metadata = match decode_call_operation_metadata(request.body(), self.wire_limits) {
            Ok(metadata) => metadata,
            Err(error) => {
                return rejected(protocol_error(
                    ProtocolErrorCode::InvalidRequest,
                    error.to_string(),
                ));
            }
        };
        let projection = match self
            .host
            .projection_for_interface_live(&metadata.target.path, &metadata.interface.reference)
            .await
        {
            Ok(Some(projection)) => projection,
            Ok(None) => return rejected(unpublished_path_error(&metadata.target.path)),
            Err(error) => return rejected(error),
        };
        if let Err(error) = self.host.check_call_preconditions(
            &projection,
            &metadata.target,
            &metadata.interface,
            &metadata.operation,
        ) {
            return rejected(error);
        }
        let descriptor = projection.descriptor.clone();
        let target_validator = projection.object.validator.clone();
        let request_value =
            match metadata.decode_request(request.body(), &descriptor, self.wire_limits) {
                Ok(request) => request,
                Err(error) => {
                    return rejected(protocol_error(
                        ProtocolErrorCode::InvalidArguments,
                        error.to_string(),
                    ));
                }
            };
        let context = WipCallContext {
            execution,
            security_context: self.security_context.as_str().to_string(),
        };
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                context.execution.execution_id(),
                ActiveOperation {
                    operation: request_value.operation.clone(),
                    handler: Arc::clone(&projection.handler),
                },
            );
        let result = self
            .host
            .execute_projection(projection, request_value.clone(), context)
            .await;
        match result {
            Ok(output) => {
                let response = encode_call_operation_response(
                    &request_value,
                    &descriptor,
                    &CallOperationResponse {
                        result: output.value,
                        validator: match output.validator {
                            Some(validator) => Some(validator),
                            None if self.host.subtree(&request_value.target.path).is_some() => None,
                            None => target_validator,
                        },
                    },
                    self.wire_limits,
                );
                let response = match response {
                    Ok(response) => response,
                    Err(_) => {
                        // Dispatch has completed: losing its result is not a pre-effect refusal.
                        let message = "completed operation result could not be encoded; inspect effects before retry";
                        let response = encode_protocol_error_response(
                            ProtocolInteraction::CallOperation,
                            &protocol_error(ProtocolErrorCode::OperationOutcomeUnknown, message),
                            self.wire_limits,
                        )
                        .map_err(|error| ToolError::Internal(error.to_string()))?;
                        return Ok((
                            response,
                            Some(Err(ToolError::ExecutionFailed(format!(
                                "WIP operation outcome unknown; do not retry automatically: {message}"
                            )))),
                            WipAuditOutcome::OutcomeUnknown,
                        ));
                    }
                };
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
    active: Arc<Mutex<HashMap<String, ActiveOperation>>>,
    request_id: RequestId,
    execution_id: String,
    armed: bool,
}

impl DispatchedCallGuard {
    fn new(
        state: Arc<Mutex<ClientState>>,
        active: Arc<Mutex<HashMap<String, ActiveOperation>>>,
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

fn check_retrieval_completion(completion: Completion) -> Result<(), ToolError> {
    match completion {
        Completion::Object(_) | Completion::Interface(_) => Ok(()),
        Completion::ProtocolFailure(error) => Err(ToolError::ExecutionFailed(error.to_string())),
        Completion::StaleResponseRejected(_) => Err(ToolError::ExecutionFailed(
            "WIP retrieval superseded; refresh observations".into(),
        )),
        Completion::Call(_) => Err(ToolError::Internal(
            "WIP retrieval returned a call result".into(),
        )),
    }
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
    interface: &InterfaceReference,
    operation: &str,
    arguments: &Json,
    descriptor: &InterfaceDescriptor,
    limits: Limits,
) -> Result<BTreeMap<String, Value>, String> {
    let body = serde_json::to_vec(&json!({
        "target": {"path": path},
        "interface": {"reference": {"scope":interface.scope,"name":interface.name}},
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

#[cfg(test)]
mod tests {
    use super::*;
    use agen::llm_client::client::LlmClient;
    use agen::llm_client::{ClientError, Request as LlmRequest, ResponseStream};
    use futures::stream;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
            ["Inspect", "Invoke", "Tree"]
        );
        let metrics = runtime.metrics();
        assert!(metrics.ordinary_schema_bytes > metrics.wip_schema_bytes);
    }

    #[tokio::test]
    async fn object_centered_tools_inspect_without_tree_and_invoke_original_schema() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        engine.register_tool(echo_definition("Echo".into(), Arc::clone(&calls)));
        install_wip_mode(&mut engine, None, "worker-a".into()).unwrap();
        let tools = engine.tool_server_handle();
        tools.flush_pending();
        let inspected = transport_output_json(
            tools
                .call_tool(
                    "Inspect",
                    r#"{"path":"/tools/Echo"}"#,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap(),
        );
        assert_eq!(inspected["path"], "/tools/Echo");
        let interface = &inspected["interfaces"][0]["reference"];
        assert_eq!(interface, &json!({"scope":"/","name":"yoi.tool/Echo/v1"}));
        assert!(
            inspected["interfaces"][0]["signature"]
                .as_str()
                .unwrap()
                .contains("operation call(")
        );
        let tree = transport_output_json(
            tools
                .call_tool(
                    "Tree",
                    r#"{"path":"/","depth":2}"#,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap(),
        );
        assert_eq!(tree["coverage"]["complete"], true);
        assert!(tree.to_string().contains("/tools/Echo"));
        let arguments = json!({"message":"hello"});
        let output = tools.call_tool("Invoke", &json!({"path":"/tools/Echo","interface":interface,"operation":"call","arguments":{"input":arguments}}).to_string(), ToolExecutionContext::direct()).await.unwrap();
        assert_eq!(output.summary, arguments.to_string());
        assert!(output.content.is_none());
        assert!(output.attachments.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
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

    #[tokio::test]
    async fn completed_native_result_encoding_failure_is_unknown_and_not_retried() {
        struct OversizedResult(Arc<AtomicUsize>);
        #[async_trait]
        impl WipOperationHandler for OversizedResult {
            async fn call(
                &self,
                _: &str,
                _: &BTreeMap<String, Value>,
                _: WipCallContext,
            ) -> Result<WipOperationOutput, WipOperationError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(WipOperationOutput::native(Value::String(
                    "x".repeat(3 * 1024 * 1024),
                )))
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut p = contribution_projection(Arc::new(OversizedResult(calls.clone())));
        p.descriptor.operations[0].returns.r#type = TypeExpr::String;
        p.interface_validator = Some(descriptor_validator(&p.descriptor));
        let mut registry = WipMountRegistry::new();
        registry.allocate_namespace("asset", "assets").unwrap();
        registry.mount(p).unwrap();
        let runtime = WipRuntime::new(
            WipHost::new(registry),
            SecurityContext::new("encoding-test"),
            0,
        )
        .unwrap();
        runtime.tree("/assets/A-1".into(), 0, true).await.unwrap();
        runtime.inspect("/assets/A-1".into(), true).await.unwrap();
        let error = runtime
            .call(
                "/assets/A-1".into(),
                crate::wip::root_reference("test.asset/item/v1"),
                "read".into(),
                Json::Object(Default::default()),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("outcome unknown"), "{error}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct ContributionHandler {
        calls: Arc<AtomicUsize>,
        allowed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl WipOperationHandler for ContributionHandler {
        async fn call(
            &self,
            _operation: &str,
            _arguments: &BTreeMap<String, Value>,
            _context: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            if !self.allowed.load(Ordering::SeqCst) {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::PermissionDenied,
                    "current subject is not authorized",
                )));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(WipOperationOutput::native(Value::Unit))
        }
    }

    fn contributed_operation(name: &str) -> OperationDeclaration {
        OperationDeclaration {
            name: name.into(),
            documentation: documentation(name),
            parameters: Vec::new(),
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Unit,
            },
        }
    }

    fn contribution_projection(handler: Arc<dyn WipOperationHandler>) -> WipProjection {
        let descriptor = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: documentation("shared object interface"),
            types: Vec::new(),
            operations: vec![contributed_operation("read")],
        };
        WipProjection {
            route: "/assets/A-1".into(),
            capability: "asset:object".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: "A-1".into(),
                description: Some("host-resolved asset".into()),
                interfaces: vec![root_reference("test.asset/item/v1")],
                r#ref: Some("asset:A-1".into()),
                validator: Some(vec![1]),
            },
            interface: root_reference("test.asset/item/v1"),
            interface_validator: Some(descriptor_validator(&descriptor)),
            descriptor,
            handler,
        }
    }

    #[tokio::test]
    async fn host_namespace_object_resolution_and_operation_contributions_are_independent() {
        let read_calls = Arc::new(AtomicUsize::new(0));
        let manage_calls = Arc::new(AtomicUsize::new(0));
        let read_allowed = Arc::new(AtomicBool::new(true));
        let manage_allowed = Arc::new(AtomicBool::new(false));
        let mut registry = WipMountRegistry::new();
        assert!(matches!(
            registry.allocate_namespace("host", "features"),
            Err(WipMountError::InvalidRoute { .. })
        ));
        assert!(matches!(
            registry.allocate_namespace("host", "tools"),
            Err(WipMountError::InvalidRoute { .. })
        ));
        let namespace = registry.allocate_namespace("asset", "assets").unwrap();
        assert_eq!(namespace.root(), "/assets");
        assert!(matches!(
            registry.allocate_namespace("other", "assets"),
            Err(WipMountError::RouteCollision { .. })
        ));
        registry
            .mount(contribution_projection(Arc::new(ContributionHandler {
                calls: Arc::clone(&read_calls),
                allowed: Arc::clone(&read_allowed),
            })))
            .unwrap();

        let host_without_management = WipHost::new(registry);
        let resolved = host_without_management.projection("/assets/A-1").unwrap();
        assert_eq!(resolved.object.r#ref.as_deref(), Some("asset:A-1"));
        assert_eq!(
            resolved
                .descriptor
                .operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            ["read"]
        );

        let mut registry = host_without_management.registry;
        let contribution_descriptor = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: documentation("shared object interface"),
            types: Vec::new(),
            operations: vec![contributed_operation("manage")],
        };
        registry
            .contribute_operations(WipOperationContribution {
                route: "/assets/A-1".into(),
                contributor: "asset-management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor: contribution_descriptor.clone(),
                handler: Arc::new(ContributionHandler {
                    calls: Arc::clone(&manage_calls),
                    allowed: Arc::clone(&manage_allowed),
                }),
            })
            .unwrap();
        assert!(matches!(
            registry.contribute_operations(WipOperationContribution {
                route: "/assets/A-1".into(),
                contributor: "duplicate-management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor: contribution_descriptor,
                handler: Arc::new(ContributionHandler {
                    calls: Arc::clone(&manage_calls),
                    allowed: Arc::clone(&manage_allowed),
                }),
            }),
            Err(WipMountError::OperationCollision { .. })
        ));
        let host = WipHost::new(registry);
        let projection = host.projection("/assets/A-1").unwrap();
        assert_eq!(
            projection
                .descriptor
                .operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            ["manage", "read"]
        );

        let request = |operation: &str| CallOperationRequest {
            target: wip_protocol::Target {
                path: "/assets/A-1".into(),
                validator: projection.object.validator.clone(),
            },
            interface: wip_protocol::InterfaceTarget {
                scope_ref: None,
                reference: projection.interface.clone(),
                validator: projection.interface_validator.clone(),
            },
            operation: operation.into(),
            arguments: BTreeMap::new(),
        };
        let context = || WipCallContext {
            execution: ToolExecutionContext::direct(),
            security_context: "worker-a".into(),
        };
        assert!(matches!(
            host.call(request("manage"), context()).await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        manage_allowed.store(true, Ordering::SeqCst);
        host.call(request("manage"), context())
            .await
            .unwrap_or_else(|_| panic!("authorized management contribution should run"));
        manage_allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            host.call(request("manage"), context()).await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        host.call(request("read"), context())
            .await
            .unwrap_or_else(|_| panic!("independent read contribution should run"));
        assert_eq!(manage_calls.load(Ordering::SeqCst), 1);
        assert_eq!(read_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn operation_contributions_reject_interface_and_target_conflicts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let allowed = Arc::new(AtomicBool::new(true));
        let handler: Arc<dyn WipOperationHandler> =
            Arc::new(ContributionHandler { calls, allowed });
        let mut registry = WipMountRegistry::new();
        let projection = contribution_projection(Arc::clone(&handler));
        assert!(matches!(
            registry.mount(projection.clone()),
            Err(WipMountError::InvalidProjection { .. })
        ));
        registry.allocate_namespace("asset", "assets").unwrap();
        registry.mount(projection).unwrap();
        let mut inconsistent = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: documentation("different interface contract"),
            types: Vec::new(),
            operations: vec![contributed_operation("manage")],
        };
        assert!(matches!(
            registry.contribute_operations(WipOperationContribution {
                route: "/assets/A-1".into(),
                contributor: "management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor: inconsistent.clone(),
                handler: Arc::clone(&handler),
            }),
            Err(WipMountError::InterfaceCollision { .. })
        ));
        inconsistent.documentation = documentation("shared object interface");
        assert!(matches!(
            registry.contribute_operations(WipOperationContribution {
                route: "/assets/A-2".into(),
                contributor: "management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor: inconsistent,
                handler,
            }),
            Err(WipMountError::OperationTargetNotFound { .. })
        ));
    }

    fn asset_registry(
        read: Arc<AtomicBool>,
        manage: Option<Arc<AtomicBool>>,
        calls: Arc<AtomicUsize>,
    ) -> WipMountRegistry {
        let mut registry = WipMountRegistry::new();
        registry.allocate_namespace("asset", "assets").unwrap();
        let projection = contribution_projection(Arc::new(ContributionHandler {
            calls: Arc::clone(&calls),
            allowed: read,
        }));
        let mut descriptor = projection.descriptor.clone();
        descriptor.operations = vec![contributed_operation("manage")];
        registry.mount(projection).unwrap();
        if let Some(allowed) = manage {
            registry
                .contribute_operations(WipOperationContribution {
                    route: "/assets/A-1".into(),
                    contributor: "asset-management".into(),
                    interface: root_reference("test.asset/item/v1"),
                    descriptor,
                    handler: Arc::new(ContributionHandler { calls, allowed }),
                })
                .unwrap();
        }
        registry
    }

    fn asset_request(projection: &WipProjection, operation: &str) -> CallOperationRequest {
        CallOperationRequest {
            target: wip_protocol::Target {
                path: projection.route.clone(),
                validator: projection.object.validator.clone(),
            },
            interface: wip_protocol::InterfaceTarget {
                scope_ref: None,
                reference: projection.interface.clone(),
                validator: projection.interface_validator.clone(),
            },
            operation: operation.into(),
            arguments: BTreeMap::new(),
        }
    }

    fn direct_wip_context() -> WipCallContext {
        WipCallContext {
            execution: ToolExecutionContext::direct(),
            security_context: "worker-a".into(),
        }
    }

    #[tokio::test]
    async fn management_enablement_is_independent_of_read_and_operation_permissions() {
        for enabled in [false, true] {
            for can_read in [false, true] {
                for can_manage in [false, true] {
                    let calls = Arc::new(AtomicUsize::new(0));
                    let host = WipHost::new(asset_registry(
                        Arc::new(AtomicBool::new(can_read)),
                        enabled.then(|| Arc::new(AtomicBool::new(can_manage))),
                        Arc::clone(&calls),
                    ));
                    // Metadata resolution is not authorization to read domain data.
                    let projection = host.projection("/assets/A-1").unwrap();
                    let read = host
                        .call(asset_request(&projection, "read"), direct_wip_context())
                        .await;
                    if can_read {
                        assert!(read.is_ok());
                    } else {
                        assert!(matches!(
                            read,
                            Err(WipOperationError::Protocol(ProtocolError {
                                code: ProtocolErrorCode::PermissionDenied,
                                ..
                            }))
                        ));
                    }
                    let manage = host
                        .call(asset_request(&projection, "manage"), direct_wip_context())
                        .await;
                    if !enabled {
                        assert!(matches!(
                            manage,
                            Err(WipOperationError::Protocol(ProtocolError {
                                code: ProtocolErrorCode::OperationNotFound,
                                ..
                            }))
                        ));
                    } else if !can_manage {
                        assert!(matches!(
                            manage,
                            Err(WipOperationError::Protocol(ProtocolError {
                                code: ProtocolErrorCode::PermissionDenied,
                                ..
                            }))
                        ));
                    } else {
                        assert!(manage.is_ok());
                    }
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        usize::from(can_read) + usize::from(enabled && can_manage)
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn contributed_interface_invalidates_observations_and_restore_requires_discovery() {
        let calls = Arc::new(AtomicUsize::new(0));
        let allowed = Arc::new(AtomicBool::new(true));
        let old_host = WipHost::new(asset_registry(
            Arc::clone(&allowed),
            None,
            Arc::clone(&calls),
        ));
        let old_projection = old_host.projection("/assets/A-1").unwrap();
        let old_runtime = WipRuntime::new(old_host, SecurityContext::new("worker-a"), 0).unwrap();
        old_runtime
            .tree("/assets/A-1".into(), 0, false)
            .await
            .unwrap();
        old_runtime
            .inspect(old_projection.route.clone(), false)
            .await
            .unwrap();
        let restored_host = WipHost::new(asset_registry(
            Arc::clone(&allowed),
            Some(Arc::clone(&allowed)),
            Arc::clone(&calls),
        ));
        let current = restored_host.projection("/assets/A-1").unwrap();
        assert_eq!(old_projection.object, current.object);
        assert_ne!(
            old_projection.interface_validator,
            current.interface_validator
        );
        assert!(matches!(
            restored_host
                .call(asset_request(&old_projection, "read"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InterfaceValidatorMismatch,
                ..
            }))
        ));
        let restored = WipRuntime::new(restored_host, SecurityContext::new("worker-a"), 0).unwrap();
        // Same Worker security identity, fresh runtime: no inherited observations.
        assert!(
            restored
                .call(
                    "/assets/A-1".into(),
                    current.interface.clone(),
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        restored.tree("/assets/A-1".into(), 0, false).await.unwrap();
        restored
            .inspect(current.route.clone(), false)
            .await
            .unwrap();
        restored
            .call(
                "/assets/A-1".into(),
                current.interface.clone(),
                "manage".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        allowed.store(false, Ordering::SeqCst);
        assert!(
            restored
                .call(
                    "/assets/A-1".into(),
                    current.interface.clone(),
                    "manage".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("PermissionDenied")
        );
        restored.reset_observations().unwrap();
        assert!(
            restored
                .call(
                    "/assets/A-1".into(),
                    current.interface,
                    "manage".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mounted_collection_keeps_identity_and_interface_with_static_children() {
        let mut registry = asset_registry(
            Arc::new(AtomicBool::new(true)),
            None,
            Arc::new(AtomicUsize::new(0)),
        );
        let mut collection = contribution_projection(Arc::new(ContributionHandler {
            calls: Arc::new(AtomicUsize::new(0)),
            allowed: Arc::new(AtomicBool::new(true)),
        }));
        collection.route = "/assets".into();
        collection.object.name = "assets".into();
        collection.object.r#ref = Some("asset:collection".into());
        let expected = collection.object.clone();
        registry.mount(collection).unwrap();
        let observation = WipHost::new(registry).observe("/assets", 1).unwrap();
        assert_eq!(observation.object, expected);
        assert_eq!(observation.children.unwrap()[0].object.name, "A-1");
    }

    #[test]
    fn compatibility_mounts_cannot_claim_host_or_legacy_namespaces() {
        for route in [
            "/features/ticket/tickets",
            "/assets/A-1",
            "/tools/Echo/child",
        ] {
            let mut projection = compatibility_projection(
                meta("Echo"),
                Arc::new(EchoTool {
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                None,
            )
            .unwrap();
            projection.route = route.into();
            projection.object.name = route.rsplit('/').next().unwrap().into();
            let mut registry = WipMountRegistry::new();
            registry.allocate_namespace("asset", "assets").unwrap();
            assert!(matches!(
                registry.mount(projection),
                Err(WipMountError::InvalidProjection { .. })
            ));
            assert!(registry.routes().next().is_none());
        }
    }

    #[tokio::test]
    async fn legacy_feature_routes_fail_with_rediscovery_guidance() {
        let host = WipHost::new(WipMountRegistry::new());
        let error = host.observe("/features/ticket/tickets/T-1", 0).unwrap_err();
        assert_eq!(error.code, ProtocolErrorCode::NotFound);
        assert!(error.message.contains("rediscover from `/`"));
        assert!(error.message.contains("no longer published"));

        let runtime = WipRuntime::new(host, SecurityContext::new("worker-a"), 0).unwrap();
        let error = runtime
            .invoke(
                "/features/ticket/tickets/T-1".into(),
                crate::wip::root_reference("yoi.ticket/item/v1"),
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rediscover from `/`"));
    }

    #[derive(Clone)]
    struct ContextualHandler {
        visible: Arc<AtomicBool>,
        permitted: Arc<AtomicBool>,
        capable: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    impl ContextualHandler {
        fn new(permitted: bool, capable: bool) -> Self {
            Self {
                visible: Arc::new(AtomicBool::new(true)),
                permitted: Arc::new(AtomicBool::new(permitted)),
                capable: Arc::new(AtomicBool::new(capable)),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl WipOperationHandler for ContextualHandler {
        fn is_visible(&self) -> bool {
            self.visible.load(Ordering::SeqCst)
        }

        fn operation_available(&self, _operation: &str) -> bool {
            self.permitted.load(Ordering::SeqCst) && self.capable.load(Ordering::SeqCst)
        }

        fn contextual_interface(&self) -> bool {
            true
        }

        async fn call(
            &self,
            operation: &str,
            _arguments: &BTreeMap<String, Value>,
            _context: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            if !self.is_visible() || !self.operation_available(operation) {
                return Err(WipOperationError::Protocol(protocol_error(
                    ProtocolErrorCode::PermissionDenied,
                    "current native authority denied",
                )));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(WipOperationOutput::native(Value::Unit))
        }
    }

    fn operation_names(descriptor: &InterfaceDescriptor) -> Vec<&str> {
        descriptor
            .operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect()
    }

    fn contextual_static_registry(
        owner: Arc<ContextualHandler>,
        contributor: Arc<ContextualHandler>,
    ) -> WipMountRegistry {
        let mut registry = WipMountRegistry::new();
        registry.allocate_namespace("asset", "assets").unwrap();
        let projection = contribution_projection(owner);
        let mut descriptor = projection.descriptor.clone();
        descriptor.operations = vec![contributed_operation("manage")];
        registry.mount(projection).unwrap();
        registry
            .contribute_operations(WipOperationContribution {
                route: "/assets/A-1".into(),
                contributor: "management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor,
                handler: contributor,
            })
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn contextual_static_filters_contributors_and_rechecks_old_references() {
        let owner = Arc::new(ContextualHandler::new(true, true));
        let contributor = Arc::new(ContextualHandler::new(false, true));
        let mut registry = contextual_static_registry(Arc::clone(&owner), Arc::clone(&contributor));
        // Same base contract, different Object/capability. It must not share the
        // first Object's cached contextual descriptor.
        let mut other = registry.mounts["/assets/A-1"].projection.clone();
        other.route = "/assets/A-2".into();
        other.object.name = "A-2".into();
        other.handler = Arc::new(ContextualHandler::new(true, false));
        registry.mount(other).unwrap();
        let host = WipHost::new(registry);
        let initial = host.projection("/assets/A-1").unwrap();
        let other = host.projection("/assets/A-2").unwrap();
        assert_eq!(
            initial.interface,
            contextual_reference("test.asset/item/v1", "/assets/A-1")
        );
        assert_ne!(initial.interface, other.interface);
        assert_eq!(operation_names(&initial.descriptor), ["read"]);
        assert!(other.descriptor.operations.is_empty());
        assert!(
            host.fetch_interface(&root_reference("test.asset/item/v1"))
                .is_err()
        );
        assert_eq!(
            host.fetch_interface(&initial.interface).unwrap().descriptor,
            initial.descriptor
        );
        host.call(asset_request(&initial, "read"), direct_wip_context())
            .await
            .unwrap_or_else(|_| panic!("available owner operation must run"));
        assert!(matches!(
            host.call(asset_request(&initial, "manage"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::OperationNotFound,
                ..
            }))
        ));

        contributor.permitted.store(true, Ordering::SeqCst);
        let enabled = host.projection("/assets/A-1").unwrap();
        assert_eq!(enabled.interface, initial.interface);
        assert_eq!(
            operation_names(&host.fetch_interface(&initial.interface).unwrap().descriptor),
            ["manage", "read"]
        );
        assert_ne!(enabled.interface_validator, initial.interface_validator);
        assert!(matches!(
            host.call(asset_request(&initial, "read"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InterfaceValidatorMismatch,
                ..
            }))
        ));
        host.call(asset_request(&enabled, "manage"), direct_wip_context())
            .await
            .unwrap_or_else(|_| panic!("available contributor operation must run"));
        contributor.capable.store(false, Ordering::SeqCst);
        assert_eq!(
            operation_names(&host.fetch_interface(&initial.interface).unwrap().descriptor),
            ["read"]
        );
        assert!(matches!(
            host.call(asset_request(&enabled, "manage"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InterfaceValidatorMismatch,
                ..
            }))
        ));
        let current = host.projection("/assets/A-1").unwrap();
        assert!(matches!(
            host.call(asset_request(&current, "manage"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::OperationNotFound,
                ..
            }))
        ));
        let mut unqualified = asset_request(&current, "read");
        unqualified.interface.reference = crate::wip::root_reference("test.asset/item/v1");
        assert!(matches!(
            host.call(unqualified, direct_wip_context()).await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InterfaceMismatch,
                ..
            }))
        ));
        owner.visible.store(false, Ordering::SeqCst);
        contributor.permitted.store(true, Ordering::SeqCst);
        contributor.capable.store(true, Ordering::SeqCst);
        assert!(host.projection("/assets/A-1").is_none());
        assert!(host.fetch_interface(&initial.interface).is_err());
        assert!(host.observe("/assets/A-1", 0).is_err());
        let observed = host.observe("/assets", 1).unwrap();
        assert_eq!(
            observed
                .children
                .unwrap()
                .iter()
                .map(|item| item.object.name.as_str())
                .collect::<Vec<_>>(),
            ["A-2"]
        );
        assert!(matches!(
            host.call(asset_request(&enabled, "manage"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::NotFound,
                ..
            }))
        ));
        assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(contributor.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn contextual_visibility_cannot_fall_back_to_namespace_and_provider_versions_stay_current() {
        let owner = Arc::new(ContextualHandler::new(true, true));
        let mut registry = contextual_static_registry(
            Arc::clone(&owner),
            Arc::new(ContextualHandler::new(true, true)),
        );
        let mut child = contribution_projection(Arc::new(DynamicHandler {
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        child.route = "/assets/A-1/child".into();
        child.object.name = "child".into();
        // Legacy interface references stay opaque, including this delimiter.
        child.interface = crate::wip::root_reference("legacy/@/opaque");
        child.object.interfaces = vec![child.interface.clone()];
        registry.mount(child.clone()).unwrap();
        let mut host = WipHost::new(registry);
        assert_eq!(
            host.fetch_interface(&child.interface).unwrap().descriptor,
            child.descriptor
        );
        let initial = host.projection("/assets/A-1").unwrap();
        host.registry
            .mounts
            .get_mut("/assets/A-1")
            .unwrap()
            .projection
            .interface_validator = Some(vec![99]);
        let current = host.fetch_interface(&initial.interface).unwrap();
        assert_eq!(current.descriptor, initial.descriptor);
        assert_ne!(current.validator, initial.interface_validator);
        owner.visible.store(false, Ordering::SeqCst);
        assert!(host.object_at("/assets/A-1").is_none());
        assert!(host.observe("/assets/A-1", 1).is_err());
        assert!(
            host.observe("/assets", 1)
                .unwrap()
                .children
                .unwrap()
                .is_empty()
        );
    }

    struct ContextualItemResolver {
        owner: Arc<ContextualHandler>,
        revision: Arc<AtomicUsize>,
        contextual_family: bool,
    }

    impl WipDynamicItemResolver for ContextualItemResolver {
        fn contextual_interface(&self) -> bool {
            self.contextual_family
        }

        fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
            if !matches!(item_reference, "O-1" | "O-2") {
                return None;
            }
            let mut handler = self.owner.as_ref().clone();
            if item_reference == "O-2" {
                handler.capable = Arc::new(AtomicBool::new(false));
            }
            Some(WipDynamicItem {
                object: Object {
                    name: item_reference.into(),
                    description: None,
                    interfaces: vec![root_reference("test.dynamic/v1")],
                    r#ref: Some(format!("objective:{item_reference}")),
                    validator: Some(vec![self.revision.load(Ordering::SeqCst) as u8]),
                },
                handler: Arc::new(handler),
            })
        }
    }

    struct ContextualContributionResolver(Arc<ContextualHandler>);

    impl WipDynamicOperationResolver for ContextualContributionResolver {
        fn handler(&self, _item_reference: &str) -> Arc<dyn WipOperationHandler> {
            self.0.clone()
        }
    }

    fn contextual_dynamic_registry(
        owner: Arc<ContextualHandler>,
        contributor: Arc<ContextualHandler>,
        revision: Arc<AtomicUsize>,
        contextual_collection: bool,
    ) -> WipMountRegistry {
        let mut registry = dynamic_registry(Arc::clone(&revision), Arc::clone(&owner.calls));
        let mut mounted = registry.dynamic_mounts.pop().unwrap();
        mounted.descriptor.operations = vec![contributed_operation("read")];
        mounted.interface_validator = Some(descriptor_validator(&mounted.descriptor));
        mounted.resolver = Arc::new(ContextualItemResolver {
            owner,
            revision,
            contextual_family: !contextual_collection,
        });
        if contextual_collection {
            let collection = registry.mounts.get_mut("/objectives").unwrap();
            collection.projection.handler = Arc::new(ContextualHandler::new(true, true));
            collection.operation_handlers =
                operation_handlers(&collection.projection, "objective:collection");
        }
        let mut descriptor = mounted.descriptor.clone();
        descriptor.operations = vec![contributed_operation("manage")];
        registry.mount_dynamic(mounted).unwrap();
        registry
            .contribute_dynamic_operations(WipDynamicOperationContribution {
                collection_route: "/objectives".into(),
                contributor: "management".into(),
                interface: root_reference("test.dynamic/v1"),
                descriptor,
                resolver: Arc::new(ContextualContributionResolver(contributor)),
            })
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn contextual_dynamic_filters_by_path_and_rechecks_permission_and_object_revision() {
        // Both ways to opt in a family must block unqualified descriptor fetches.
        for contextual_collection in [false, true] {
            let owner = Arc::new(ContextualHandler::new(true, true));
            let contributor = Arc::new(ContextualHandler::new(true, true));
            let revision = Arc::new(AtomicUsize::new(1));
            let host = WipHost::new(contextual_dynamic_registry(
                Arc::clone(&owner),
                Arc::clone(&contributor),
                Arc::clone(&revision),
                contextual_collection,
            ));
            let initial = host.projection("/objectives/O-1").unwrap();
            let other = host.projection("/objectives/O-2").unwrap();
            assert_ne!(initial.interface, other.interface);
            assert_eq!(
                operation_names(&host.fetch_interface(&initial.interface).unwrap().descriptor),
                ["manage", "read"]
            );
            assert_eq!(
                operation_names(&host.fetch_interface(&other.interface).unwrap().descriptor),
                ["manage"]
            );
            assert!(
                host.fetch_interface(&root_reference("test.dynamic/v1"))
                    .is_err()
            );
            assert!(
                host.fetch_interface(&contextual_reference("wrong.base/v1", "/objectives/O-1"))
                    .is_err()
            );
            assert!(
                host.fetch_interface(&contextual_reference(
                    "test.dynamic/v1",
                    "/objectives/missing"
                ))
                .is_err()
            );
            assert!(
                host.fetch_interface(&root_reference("test.dynamic/v1/@/ff"))
                    .is_err()
            );
            assert!(
                host.fetch_interface(&root_reference("test.dynamic/v1/@/2f0"))
                    .is_err()
            );
            let mut wrong_path = asset_request(&initial, "read");
            wrong_path.target.path = other.route.clone();
            assert!(matches!(
                host.call(wrong_path, direct_wip_context()).await,
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::InterfaceMismatch,
                    ..
                }))
            ));
            contributor.permitted.store(false, Ordering::SeqCst);
            assert_eq!(
                operation_names(&host.fetch_interface(&initial.interface).unwrap().descriptor),
                ["read"]
            );
            assert!(matches!(
                host.call(asset_request(&initial, "manage"), direct_wip_context())
                    .await,
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::InterfaceValidatorMismatch,
                    ..
                }))
            ));
            let current = host.projection("/objectives/O-1").unwrap();
            assert!(matches!(
                host.call(asset_request(&current, "manage"), direct_wip_context())
                    .await,
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::OperationNotFound,
                    ..
                }))
            ));
            revision.store(2, Ordering::SeqCst);
            assert!(matches!(
                host.call(asset_request(&current, "read"), direct_wip_context())
                    .await,
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::ValidatorMismatch,
                    ..
                }))
            ));
            let current = host.projection("/objectives/O-1").unwrap();
            host.call(asset_request(&current, "read"), direct_wip_context())
                .await
                .unwrap_or_else(|_| panic!("fresh available dynamic operation must run"));
            owner.visible.store(false, Ordering::SeqCst);
            assert!(host.observe("/objectives/O-1", 0).is_err());
            assert!(host.fetch_interface(&initial.interface).is_err());
            assert!(host.fetch_interface(&other.interface).is_err());
            assert!(matches!(
                host.call(asset_request(&current, "read"), direct_wip_context())
                    .await,
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::NotFound,
                    ..
                }))
            ));
            assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
            assert_eq!(contributor.calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn contextual_runtime_cached_descriptor_does_not_authorize_removed_operations() {
        let owner = Arc::new(ContextualHandler::new(true, true));
        let contributor = Arc::new(ContextualHandler::new(true, true));
        let runtime = WipRuntime::new(
            WipHost::new(contextual_static_registry(
                Arc::clone(&owner),
                Arc::clone(&contributor),
            )),
            SecurityContext::new("worker-a"),
            0,
        )
        .unwrap();
        runtime.tree("/assets/A-1".into(), 0, false).await.unwrap();
        let interface = runtime.host.projection("/assets/A-1").unwrap().interface;
        runtime.inspect("/assets/A-1".into(), false).await.unwrap();
        contributor.permitted.store(false, Ordering::SeqCst);
        // The Client still knows the old descriptor; the Host must independently
        // check its current filtered descriptor rather than trust Known Space.
        let error = runtime
            .call(
                "/assets/A-1".into(),
                interface.clone(),
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("InterfaceValidatorMismatch"));
        assert!(
            runtime
                .call(
                    "/assets/A-1".into(),
                    interface.clone(),
                    "manage".into(),
                    json!({}),
                    ToolExecutionContext::direct(),
                )
                .await
                .is_err()
        );
        assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(contributor.calls.load(Ordering::SeqCst), 0);
        runtime.inspect("/assets/A-1".into(), true).await.unwrap();
        runtime
            .call(
                "/assets/A-1".into(),
                interface.clone(),
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        owner.visible.store(false, Ordering::SeqCst);
        assert!(runtime.host.fetch_interface(&interface).is_err());
        let error = runtime
            .inspect("/assets/A-1".into(), true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("NotFound"));
        {
            let state = runtime.state.lock().unwrap();
            assert!(state.client.interface(&state.session, &interface).is_none());
        }
        assert!(
            runtime
                .call(
                    "/assets/A-1".into(),
                    interface,
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct(),
                )
                .await
                .is_err()
        );
        assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
    }

    struct CurrentObjectValidatorHandler {
        inner: Arc<ContextualHandler>,
        revision: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WipOperationHandler for CurrentObjectValidatorHandler {
        fn contextual_interface(&self) -> bool {
            true
        }

        fn object_validator(&self) -> Option<Vec<u8>> {
            Some(self.revision.load(Ordering::SeqCst).to_be_bytes().to_vec())
        }

        async fn call(
            &self,
            operation: &str,
            arguments: &BTreeMap<String, Value>,
            context: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            self.inner.call(operation, arguments, context).await
        }
    }

    #[tokio::test]
    async fn static_object_validator_hook_rejects_current_owner_state_changes() {
        let owner = Arc::new(ContextualHandler::new(true, true));
        let revision = Arc::new(AtomicUsize::new(1));
        let mut registry = WipMountRegistry::new();
        registry.allocate_namespace("asset", "assets").unwrap();
        registry
            .mount(contribution_projection(Arc::new(
                CurrentObjectValidatorHandler {
                    inner: Arc::clone(&owner),
                    revision: Arc::clone(&revision),
                },
            )))
            .unwrap();
        // Contributors cannot override Object ownership or its current validator.
        let mut descriptor = registry.mounts["/assets/A-1"].projection.descriptor.clone();
        descriptor.operations = vec![contributed_operation("manage")];
        registry
            .contribute_operations(WipOperationContribution {
                route: "/assets/A-1".into(),
                contributor: "management".into(),
                interface: root_reference("test.asset/item/v1"),
                descriptor,
                handler: Arc::new(CurrentObjectValidatorHandler {
                    inner: Arc::new(ContextualHandler::new(true, true)),
                    revision: Arc::new(AtomicUsize::new(99)),
                }),
            })
            .unwrap();
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        let interface = observed_contextual_interface(&runtime, "/assets/A-1").await;
        let initial = runtime.host.projection("/assets/A-1").unwrap();
        assert_eq!(
            initial.object.validator,
            Some(1usize.to_be_bytes().to_vec())
        );
        revision.store(2, Ordering::SeqCst);
        assert_eq!(
            runtime.host.object_at("/assets/A-1").unwrap().validator,
            Some(2usize.to_be_bytes().to_vec())
        );
        assert!(matches!(
            runtime
                .host
                .call(asset_request(&initial, "read"), direct_wip_context())
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::ValidatorMismatch,
                ..
            }))
        ));
        let error = runtime
            .call(
                "/assets/A-1".into(),
                interface.clone(),
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ValidatorMismatch"));
        assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            observed_contextual_interface(&runtime, "/assets/A-1").await,
            interface
        );
        transport_call_json(&runtime, "/assets/A-1", &interface, "read", json!({})).await;
        assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dynamic_item_read_is_independent_of_collection_visibility_and_list_permission() {
        let owner = Arc::new(ContextualHandler::new(true, true));
        let mut registry = contextual_dynamic_registry(
            Arc::clone(&owner),
            Arc::new(ContextualHandler::new(false, true)),
            Arc::new(AtomicUsize::new(1)),
            true,
        );
        let denied_collection = Arc::new(ContextualHandler::new(false, true));
        denied_collection.visible.store(false, Ordering::SeqCst);
        let collection = registry.mounts.get_mut("/objectives").unwrap();
        collection.projection.handler = denied_collection;
        collection.operation_handlers =
            operation_handlers(&collection.projection, "objective:collection");
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        assert!(runtime.host.projection("/objectives").is_none());
        let interface = observed_contextual_interface(&runtime, "/objectives/O-1").await;
        transport_call_json(&runtime, "/objectives/O-1", &interface, "read", json!({})).await;
        owner.visible.store(false, Ordering::SeqCst);
        assert!(runtime.host.fetch_interface(&interface).is_err());
        assert!(
            runtime
                .call(
                    "/objectives/O-1".into(),
                    interface,
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
        assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
    }

    fn transport_output_json(output: ToolOutput) -> Json {
        serde_json::from_str(output.content.as_deref().expect("transport JSON output")).unwrap()
    }

    async fn observed_contextual_interface(runtime: &WipRuntime, path: &str) -> InterfaceReference {
        runtime.inspect(path.into(), true).await.unwrap();
        let state = runtime.state.lock().unwrap();
        state
            .client
            .object(&state.session, path)
            .unwrap()
            .object
            .unwrap()
            .interfaces[0]
            .clone()
    }

    async fn transport_call_json(
        runtime: &WipRuntime,
        path: &str,
        interface: &InterfaceReference,
        operation: &str,
        arguments: Json,
    ) -> Json {
        transport_output_json(
            runtime
                .call(
                    path.into(),
                    interface.clone(),
                    operation.into(),
                    arguments,
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap(),
        )
    }

    fn catalog_transport_runtime(
        client: Arc<dyn crate::worker::WorkspaceClient>,
        manage: bool,
        permissions: Option<ToolPermissionConfig>,
    ) -> Arc<WipRuntime> {
        use crate::feature::FeatureRegistryBuilder;
        use crate::feature::builtin::manage_workdir::{
            ManageWorkdirFeature, wip::mount_workspace_workdir_wip,
        };
        let feature = ManageWorkdirFeature::new(client);
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        let report = FeatureRegistryBuilder::new()
            .with_module(feature.clone())
            .install_into_engine(&mut engine, &mut crate::hook::HookRegistryBuilder::new());
        assert!(!report.has_errors());
        assert_eq!(report.installed_tool_names().len(), 5);
        let mut registry = WipMountRegistry::new();
        mount_workspace_workdir_wip(&mut registry, &feature, true, manage, permissions.clone())
            .unwrap();
        let runtime =
            install_wip_mode_with_mounts(&mut engine, permissions, "worker-a".into(), registry)
                .unwrap();
        for name in [
            "WorkdirList",
            "WorkdirCreate",
            "WorkdirAttach",
            "WorkdirDetach",
            "WorkdirDelete",
        ] {
            assert!(runtime.host.projection(&format!("/tools/{name}")).is_none());
        }
        assert!(runtime.host.object_at("/features/manage-workdir").is_none());
        runtime
    }

    #[tokio::test]
    async fn catalog_native_transport_pages_all_workdirs_beyond_legacy_200_cap() {
        use crate::feature::builtin::manage_workdir::wip::tests::CatalogClient;
        let client = Arc::new(CatalogClient::default());
        client.state.lock().unwrap().workdirs = (0..205).map(|n| json!({
            "working_directory_id":format!("wd-{n:03}"), "source":{"kind":"repository", "repository_key":"registered-key"}, "status":"active"
        })).collect();
        let runtime = catalog_transport_runtime(client.clone(), false, None);
        let interface = observed_contextual_interface(&runtime, "/workdirs").await;
        let mut cursor = Json::Null;
        let mut ids = BTreeSet::new();
        let mut pages = 0;
        loop {
            let page = transport_call_json(
                &runtime,
                "/workdirs",
                &interface,
                "list",
                if cursor.is_null() {
                    json!({"limit":100})
                } else {
                    json!({"limit":100, "cursor":cursor})
                },
            )
            .await;
            pages += 1;
            let items = page["items"].as_array().unwrap();
            assert!(items.len() <= 100);
            for item in items {
                assert!(ids.insert(item["working_directory_id"].as_str().unwrap().to_owned()));
            }
            if !page["has_more"].as_bool().unwrap() {
                assert!(page["next_cursor"].is_null());
                break;
            }
            cursor = page["next_cursor"].clone();
            assert!(pages < 4, "cursor must progress");
        }
        assert_eq!(pages, 3);
        assert_eq!(ids.len(), 205);
        assert!(ids.contains("wd-000"));
        let item_interface = observed_contextual_interface(&runtime, "/workdirs/wd-000").await;
        let read = transport_call_json(
            &runtime,
            "/workdirs/wd-000",
            &item_interface,
            "read",
            json!({}),
        )
        .await;
        assert_eq!(read["working_directory_id"], "wd-000");
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|r| !r.path.ends_with("/working-directories")),
            "WIP must not use a capped legacy inventory"
        );

        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: vec![
                manifest::ToolPermissionRule {
                    tool: "WorkdirList".into(),
                    pattern: "*".into(),
                    action: ToolPermissionAction::Allow,
                },
                manifest::ToolPermissionRule {
                    tool: "WorkdirRead".into(),
                    pattern: "*wd-204*".into(),
                    action: ToolPermissionAction::Allow,
                },
            ],
        });
        let runtime = catalog_transport_runtime(client, false, permissions);
        let interface = observed_contextual_interface(&runtime, "/workdirs").await;
        let first = transport_call_json(
            &runtime,
            "/workdirs",
            &interface,
            "list",
            json!({"limit":100}),
        )
        .await;
        assert_eq!(first["empty"], true);
        assert_eq!(
            first["has_more"], true,
            "empty filtered page is not terminal"
        );
        let second = transport_call_json(
            &runtime,
            "/workdirs",
            &interface,
            "list",
            json!({"limit":100, "cursor":first["next_cursor"]}),
        )
        .await;
        assert_eq!(second["has_more"], true);
        let last = transport_call_json(
            &runtime,
            "/workdirs",
            &interface,
            "list",
            json!({"limit":100, "cursor":second["next_cursor"]}),
        )
        .await;
        assert_eq!(last["items"][0]["working_directory_id"], "wd-204");
        assert_eq!(last["has_more"], false);
    }

    #[tokio::test]
    async fn catalog_native_transport_invalidates_attachment_change_beyond_first_page() {
        use crate::feature::builtin::manage_workdir::wip::tests::CatalogClient;
        for capability_change in [false, true] {
            let client = Arc::new(CatalogClient::default());
            client.state.lock().unwrap().attachments = (0..65).map(|n| json!({
                "connection_id":format!("lifetime-{n:03}"), "alias":format!("alias-{n:03}"), "working_directory_id":"wd", "capabilities":{"bits":25}
            })).collect();
            let first_fifty = client.state.lock().unwrap().attachments[..50].to_vec();
            let runtime = catalog_transport_runtime(client.clone(), true, None);
            let interface = observed_contextual_interface(&runtime, "/workdir-attachments").await;
            let old_validator = runtime
                .host
                .projection("/workdir-attachments")
                .unwrap()
                .object
                .validator;
            if capability_change {
                client.state.lock().unwrap().attachments[64]["capabilities"] = json!({"bits":63});
            } else {
                client.state.lock().unwrap().attachments[64]["connection_id"] =
                    json!("new-lifetime");
            }
            assert_eq!(client.state.lock().unwrap().attachments[..50], first_fifty);
            assert_ne!(
                runtime
                    .host
                    .projection("/workdir-attachments")
                    .unwrap()
                    .object
                    .validator,
                old_validator
            );
            let error = runtime
                .call(
                    "/workdir-attachments".into(),
                    interface.clone(),
                    "list".into(),
                    json!({"cursor":"offset:50", "limit":100}),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("ValidatorMismatch"), "{error}");
            let interface = observed_contextual_interface(&runtime, "/workdir-attachments").await;
            let page = transport_call_json(
                &runtime,
                "/workdir-attachments",
                &interface,
                "list",
                json!({"cursor":"offset:50"}),
            )
            .await;
            assert_eq!(page["items"].as_array().unwrap().len(), 15);
            if capability_change {
                assert_eq!(page["items"][14]["capabilities"]["bits"], 63);
            } else {
                assert_eq!(page["items"][14]["connection_id"], "new-lifetime");
            }
        }
    }

    #[tokio::test]
    async fn catalog_native_transport_discovers_creates_attaches_reads_and_detaches() {
        use crate::feature::builtin::manage_workdir::wip::tests::CatalogClient;
        use crate::worker::WorkspaceRequestMethod;
        let client = Arc::new(CatalogClient::default());
        let runtime = catalog_transport_runtime(client.clone(), true, None);
        let root = transport_output_json(runtime.tree("/".into(), 1, false).await.unwrap());
        let paths = root["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["path"].as_str().unwrap())
            .collect::<Vec<_>>();
        for path in ["/repositories", "/workdirs", "/workdir-attachments"] {
            assert!(paths.contains(&path));
        }
        assert!(!paths.iter().any(|path| path.starts_with("/tools/Workdir")
            || path.starts_with("/features/manage-workdir")));
        let repositories_interface = observed_contextual_interface(&runtime, "/repositories").await;
        let listed = transport_call_json(
            &runtime,
            "/repositories",
            &repositories_interface,
            "list",
            json!({}),
        )
        .await;
        let repository = &listed["items"][0];
        assert_eq!(repository["repository_key"], "registered-key");
        assert!(repository["default_selector"].is_null());
        assert!(!listed.to_string().contains("SECRET"));
        assert!(!listed.to_string().contains("/secret/host"));
        let repository_path = repository["path"].as_str().unwrap();
        let repository_interface = observed_contextual_interface(&runtime, repository_path).await;
        let read = transport_call_json(
            &runtime,
            repository_path,
            &repository_interface,
            "read",
            json!({}),
        )
        .await;
        assert_eq!(read["repository_key"], repository["repository_key"]);
        let workdirs_interface = observed_contextual_interface(&runtime, "/workdirs").await;
        let created = transport_call_json(
            &runtime,
            "/workdirs",
            &workdirs_interface,
            "create",
            json!({"repository_key":read["repository_key"]}),
        )
        .await;
        assert_eq!(created["item"]["path"], "/workdirs/wd-created");
        {
            let state = client.state.lock().unwrap();
            assert_eq!(state.workdirs.len(), 1);
            assert!(state.attachments.is_empty(), "creation must not attach");
            assert!(
                !state
                    .requests
                    .iter()
                    .any(|request| request.method == WorkspaceRequestMethod::Post
                        && request.path.ends_with("workdir-attachments"))
            );
        }
        let workdir_path = created["item"]["path"].as_str().unwrap();
        let workdir_interface = observed_contextual_interface(&runtime, workdir_path).await;
        let attached = transport_call_json(
            &runtime,
            workdir_path,
            &workdir_interface,
            "attach",
            json!({"alias":"checkout"}),
        )
        .await;
        assert_eq!(attached["attachments_path"], "/workdir-attachments");
        let attachments_interface =
            observed_contextual_interface(&runtime, "/workdir-attachments").await;
        let listed = transport_call_json(
            &runtime,
            "/workdir-attachments",
            &attachments_interface,
            "list",
            json!({}),
        )
        .await;
        let attachment = &listed["items"][0];
        assert_eq!(attachment["alias"], "checkout");
        assert_eq!(attachment["workdir_path"], workdir_path);
        let attachment_path = attachment["path"].as_str().unwrap();
        let attachment_interface = observed_contextual_interface(&runtime, attachment_path).await;
        let read = transport_call_json(
            &runtime,
            attachment_path,
            &attachment_interface,
            "read",
            json!({}),
        )
        .await;
        assert_eq!(read["connection_id"], attachment["connection_id"]);
        transport_call_json(
            &runtime,
            attachment_path,
            &attachment_interface,
            "detach",
            json!({}),
        )
        .await;
        let attachments_interface =
            observed_contextual_interface(&runtime, "/workdir-attachments").await;
        let listed = transport_call_json(
            &runtime,
            "/workdir-attachments",
            &attachments_interface,
            "list",
            json!({}),
        )
        .await;
        assert_eq!(listed["empty"], true);
        assert!(client.state.lock().unwrap().attachments.is_empty());
        assert_eq!(
            client.state.lock().unwrap().workdirs.len(),
            1,
            "detach must preserve persistent Workdir"
        );
        assert!(runtime.host.projection(attachment_path).is_none());
        assert!(runtime.host.fetch_interface(&attachment_interface).is_err());
        let state = client.state.lock().unwrap();
        let writes = state
            .requests
            .iter()
            .filter(|request| request.method != WorkspaceRequestMethod::Get)
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 3);
        assert!(
            writes[2]
                .path
                .contains("expected_connection_id=connection-1")
        );
        drop(state);
        // A dirty provider result is a retained lifecycle outcome, not transport
        // uncertainty or implicit destructive success.
        client.state.lock().unwrap().workdirs[0]["cleanliness"] = json!("dirty");
        let workdir_interface = observed_contextual_interface(&runtime, workdir_path).await;
        let read = transport_call_json(
            &runtime,
            workdir_path,
            &workdir_interface,
            "read",
            json!({}),
        )
        .await;
        assert_eq!(read["cleanliness"], "dirty");
        let retained = transport_call_json(
            &runtime,
            workdir_path,
            &workdir_interface,
            "delete",
            json!({"reason":"dirty checkout retained"}),
        )
        .await;
        assert_eq!(retained["disposition"], "retained");
        assert_eq!(client.state.lock().unwrap().workdirs.len(), 1);
        assert_eq!(
            runtime.audit().last().unwrap().outcome,
            WipAuditOutcome::Success
        );
    }

    #[tokio::test]
    async fn catalog_native_transport_read_only_and_read_without_list_permissions() {
        use crate::feature::builtin::manage_workdir::wip::tests::CatalogClient;
        use crate::worker::WorkspaceRequestMethod;
        let client = Arc::new(CatalogClient::default());
        client.state.lock().unwrap().workdirs.push(json!({
            "working_directory_id":"wd-read", "status":"active", "cleanliness":"clean",
            "source":{"kind":"repository", "repository_key":"registered-key"},
        }));
        let runtime = catalog_transport_runtime(client.clone(), false, None);
        runtime.tree("/".into(), 1, false).await.unwrap();
        let collection_interface = observed_contextual_interface(&runtime, "/workdirs").await;
        assert_eq!(
            operation_names(
                &runtime
                    .host
                    .fetch_interface(&collection_interface)
                    .unwrap()
                    .descriptor
            ),
            ["list"]
        );
        let listed = transport_call_json(
            &runtime,
            "/workdirs",
            &collection_interface,
            "list",
            json!({}),
        )
        .await;
        let path = listed["items"][0]["path"].as_str().unwrap();
        let interface = observed_contextual_interface(&runtime, path).await;
        assert_eq!(
            operation_names(&runtime.host.fetch_interface(&interface).unwrap().descriptor),
            ["read"]
        );
        transport_call_json(&runtime, path, &interface, "read", json!({})).await;
        for (target, reference, operation, input) in [
            (
                "/workdirs",
                &collection_interface,
                "create",
                json!({"repository_key":"registered-key"}),
            ),
            (path, &interface, "attach", json!({"alias":"checkout"})),
            (path, &interface, "delete", json!({"reason":"done"})),
        ] {
            assert!(
                runtime
                    .call(
                        target.into(),
                        reference.clone(),
                        operation.into(),
                        input,
                        ToolExecutionContext::direct()
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|request| request.method == WorkspaceRequestMethod::Get)
        );
        let read_only = ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: ["RepositoryRead", "WorkdirRead"]
                .iter()
                .map(|name| manifest::ToolPermissionRule {
                    tool: (*name).into(),
                    pattern: "*".into(),
                    action: ToolPermissionAction::Allow,
                })
                .collect(),
        };
        let runtime = catalog_transport_runtime(client.clone(), false, Some(read_only));
        for path in ["/repositories/registered-key", "/workdirs/wd-read"] {
            let interface = observed_contextual_interface(&runtime, path).await;
            assert_eq!(
                operation_names(&runtime.host.fetch_interface(&interface).unwrap().descriptor),
                ["read"]
            );
            transport_call_json(&runtime, path, &interface, "read", json!({})).await;
        }
        client.state.lock().unwrap().denied = true;
        let interface = contextual_reference("yoi.workdir/item/v1", "/workdirs/wd-read");
        assert!(runtime.host.fetch_interface(&interface).is_err());
        assert!(
            runtime
                .call(
                    "/workdirs/wd-read".into(),
                    interface,
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|request| request.method == WorkspaceRequestMethod::Get)
        );
    }

    #[derive(Debug)]
    struct UncertainCatalogClient {
        catalog: Arc<crate::feature::builtin::manage_workdir::wip::tests::CatalogClient>,
    }

    impl crate::worker::WorkspaceClient for UncertainCatalogClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("test-workspace")
        }
        fn kind(&self) -> &str {
            "uncertain-catalog-test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: crate::worker::WorkspaceRequest,
        ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
            use crate::worker::{WorkspaceClientError, WorkspaceRequestMethod};
            if request.method == WorkspaceRequestMethod::Post
                && request.path.ends_with("/working-directories")
            {
                self.catalog.state.lock().unwrap().requests.push(request);
                return Err(WorkspaceClientError::Unavailable(
                    "SECRET /host/path uncertain write".into(),
                ));
            }
            self.catalog.execute(request)
        }
    }

    #[tokio::test]
    async fn catalog_native_transport_unknown_mutation_is_terminal_and_not_retried() {
        use crate::feature::builtin::manage_workdir::wip::tests::CatalogClient;
        use crate::worker::WorkspaceRequestMethod;
        let catalog = Arc::new(CatalogClient::default());
        let runtime = catalog_transport_runtime(
            Arc::new(UncertainCatalogClient {
                catalog: catalog.clone(),
            }),
            true,
            None,
        );
        let interface = observed_contextual_interface(&runtime, "/workdirs").await;
        let error = runtime
            .call(
                "/workdirs".into(),
                interface,
                "create".into(),
                json!({"repository_key":"registered-key"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        let error = error.to_string();
        assert!(error.contains("outcome unknown"));
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("/host/path"));
        assert_eq!(
            runtime.audit().last().unwrap().outcome,
            WipAuditOutcome::OutcomeUnknown
        );
        let state = runtime.state.lock().unwrap();
        assert!(matches!(
            state
                .client
                .call_history(&state.session)
                .unwrap()
                .back()
                .unwrap()
                .outcome,
            CallOutcome::Unknown { .. }
        ));
        let state = catalog.state.lock().unwrap();
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|request| request.method == WorkspaceRequestMethod::Post)
                .count(),
            1
        );
        assert!(state.workdirs.is_empty());
        assert!(state.attachments.is_empty());
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
                interfaces: vec![root_reference("yoi.native/typed/v1")],
                r#ref: Some("native:typed".into()),
                validator: Some(vec![1]),
            },
            interface: root_reference("yoi.native/typed/v1"),
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
        registry.allocate_namespace("native", "native").unwrap();
        registry.mount(native_typed_projection()).unwrap();
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        runtime.tree("/".into(), 2, false).await.unwrap();
        let inspected = runtime
            .inspect("/native/typed".into(), false)
            .await
            .unwrap();
        let inspected: Json = serde_json::from_str(inspected.content.as_deref().unwrap()).unwrap();
        let signature = inspected["interfaces"][0]["signature"].as_str().unwrap();
        assert!(signature.contains("type Payload"));
        assert!(signature.contains("red tag"));
        assert!(signature.contains("tags?: [enum"));
        assert!(signature.contains("found(entry)"));
        assert!(signature.contains("operation accept("));
        let output = runtime
            .call(
                "/native/typed".into(),
                crate::wip::root_reference("yoi.native/typed/v1"),
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
                crate::wip::root_reference("yoi.native/typed/v1"),
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

        runtime.tree("/".into(), 2, false).await.unwrap();
        runtime.inspect("/tools/Echo".into(), false).await.unwrap();
        let output = runtime
            .call(
                "/tools/Echo".into(),
                crate::wip::root_reference("yoi.tool/Echo/v1"),
                "call".into(),
                json!({"input":{"message": "ok"}}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(output.summary, "{\"message\":\"ok\"}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let invalid = runtime
            .call(
                "/tools/Echo".into(),
                crate::wip::root_reference("yoi.tool/Echo/v1"),
                "call".into(),
                json!({"input":{"message": "x"}}),
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
        runtime.tree("/".into(), 2, false).await.unwrap();
        runtime.inspect("/tools/Echo".into(), false).await.unwrap();

        let projection = runtime.host.projection("/tools/Echo").unwrap();
        let mut request = CallOperationRequest {
            target: wip_protocol::Target {
                path: "/tools/Echo".into(),
                validator: Some(vec![0]),
            },
            interface: wip_protocol::InterfaceTarget {
                scope_ref: None,
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
                    interfaces: vec![root_reference("test.dynamic/v1")],
                    r#ref: Some(format!("objective:{item_reference}")),
                    validator: Some(vec![self.revision.load(Ordering::SeqCst) as u8]),
                },
                handler: Arc::new(DynamicHandler {
                    calls: Arc::clone(&self.calls),
                }),
            })
        }
    }

    struct DynamicContributionResolver {
        calls: Arc<AtomicUsize>,
        allowed: Arc<AtomicBool>,
    }

    impl WipDynamicOperationResolver for DynamicContributionResolver {
        fn handler(&self, _item_reference: &str) -> Arc<dyn WipOperationHandler> {
            Arc::new(ContributionHandler {
                calls: Arc::clone(&self.calls),
                allowed: Arc::clone(&self.allowed),
            })
        }
    }

    fn dynamic_registry(revision: Arc<AtomicUsize>, calls: Arc<AtomicUsize>) -> WipMountRegistry {
        let mut registry = WipMountRegistry::new();
        let namespace = registry
            .allocate_namespace("objective", "objectives")
            .unwrap();
        let collection_route = namespace.root().to_string();
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
        collection.object.interfaces =
            vec![crate::wip::root_reference("test.dynamic.collection/v1")];
        collection.interface = crate::wip::root_reference("test.dynamic.collection/v1");
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
                interface: root_reference("test.dynamic/v1"),
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
        let initial = host.projection("/objectives/O-3").unwrap();
        assert!(host.projection("/objectives/O-3/other").is_none());
        assert!(host.projection("/objectives/T-3").is_none());
        assert!(host.projection("/hidden/objectives/O-3").is_none());

        revision.store(2, Ordering::SeqCst);
        let result = host
            .call(
                CallOperationRequest {
                    target: wip_protocol::Target {
                        path: "/objectives/O-3".into(),
                        validator: initial.object.validator,
                    },
                    interface: wip_protocol::InterfaceTarget {
                        scope_ref: None,
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
    fn dynamic_registration_rejects_foreign_owners_and_operation_collisions_atomically() {
        let revision = Arc::new(AtomicUsize::new(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = dynamic_registry(Arc::clone(&revision), Arc::clone(&calls));
        let mounted = registry.dynamic_mounts.pop().unwrap();
        let foreign = WipDynamicMount {
            collection_route: mounted.collection_route.clone(),
            capability: "unrelated:item".into(),
            interface: mounted.interface.clone(),
            descriptor: mounted.descriptor.clone(),
            interface_validator: mounted.interface_validator.clone(),
            resolver: Arc::new(DynamicResolver {
                revision,
                calls: Arc::clone(&calls),
            }),
        };
        assert!(matches!(
            registry.mount_dynamic(foreign),
            Err(WipMountError::InvalidProjection { .. })
        ));
        assert!(registry.dynamic_mounts.is_empty());
        registry.mount_dynamic(mounted).unwrap();
        let original = registry.dynamic_mounts[0].descriptor.clone();
        let contribution = |descriptor| WipDynamicOperationContribution {
            collection_route: "/objectives".into(),
            contributor: "management".into(),
            interface: root_reference("test.dynamic/v1"),
            descriptor,
            resolver: Arc::new(DynamicContributionResolver {
                calls: Arc::clone(&calls),
                allowed: Arc::new(AtomicBool::new(true)),
            }),
        };
        assert!(matches!(
            registry.contribute_dynamic_operations(contribution(original.clone())),
            Err(WipMountError::OperationCollision { .. })
        ));
        let mut inconsistent = original.clone();
        inconsistent.operations = vec![contributed_operation("manage")];
        inconsistent.documentation = documentation("different shape");
        assert!(matches!(
            registry.contribute_dynamic_operations(contribution(inconsistent)),
            Err(WipMountError::InterfaceCollision { .. })
        ));
        assert_eq!(registry.dynamic_mounts[0].descriptor, original);
        assert!(registry.dynamic_operation_contributions.is_empty());
        let mut valid = original;
        valid.operations = vec![contributed_operation("manage")];
        registry
            .contribute_dynamic_operations(contribution(valid.clone()))
            .unwrap();
        assert!(matches!(
            registry.contribute_dynamic_operations(contribution(valid)),
            Err(WipMountError::OperationCollision { .. })
        ));
        assert_eq!(
            registry.dynamic_operation_contributions["/objectives"].len(),
            1
        );
    }

    #[tokio::test]
    async fn dynamic_object_resolver_accepts_disjoint_feature_operations() {
        let revision = Arc::new(AtomicUsize::new(1));
        let base_calls = Arc::new(AtomicUsize::new(0));
        let contribution_calls = Arc::new(AtomicUsize::new(0));
        let allowed = Arc::new(AtomicBool::new(true));
        let mut registry = dynamic_registry(revision, base_calls);
        let mounted = registry
            .dynamic_mounts
            .first()
            .expect("dynamic object family is mounted");
        let mut descriptor = mounted.descriptor.clone();
        descriptor.operations = vec![contributed_operation("manage")];
        registry
            .contribute_dynamic_operations(WipDynamicOperationContribution {
                collection_route: "/objectives".into(),
                contributor: "objective-management".into(),
                interface: root_reference("test.dynamic/v1"),
                descriptor,
                resolver: Arc::new(DynamicContributionResolver {
                    calls: Arc::clone(&contribution_calls),
                    allowed,
                }),
            })
            .unwrap();
        let host = WipHost::new(registry);
        let projection = host.projection("/objectives/O-7").unwrap();
        assert_eq!(
            projection
                .descriptor
                .operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            ["call", "manage"]
        );
        host.call(
            CallOperationRequest {
                target: wip_protocol::Target {
                    path: "/objectives/O-7".into(),
                    validator: projection.object.validator,
                },
                interface: wip_protocol::InterfaceTarget {
                    scope_ref: None,
                    reference: projection.interface,
                    validator: projection.interface_validator,
                },
                operation: "manage".into(),
                arguments: BTreeMap::new(),
            },
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "worker-a".into(),
            },
        )
        .await
        .unwrap_or_else(|_| panic!("dynamic contributed operation should run"));
        assert_eq!(contribution_calls.load(Ordering::SeqCst), 1);
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
            .replace_compatibility_tools("/objectives", ["Echo"])
            .unwrap();
        let runtime =
            install_wip_mode_with_mounts(&mut engine, None, "worker-a".into(), registry).unwrap();
        assert!(runtime.host.projection("/tools/Echo").is_none());
        assert!(runtime.host.projection("/tools/Other").is_some());
        assert!(runtime.host.projection("/objectives").is_some());
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
        let route = authoring_registry
            .allocate_namespace("ticket", "tickets")
            .unwrap();
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

        let discovered = authoring.tree("/".into(), 3, false).await.unwrap();
        let discovered = discovered.content.unwrap();
        assert!(discovered.contains("/tickets"));
        assert!(!discovered.contains("/features"));
        assert!(!discovered.contains("/tools/QueryTicket"));
        assert!(!discovered.contains("/tools/TicketCreate"));
        authoring.inspect("/tickets".into(), false).await.unwrap();
        let collection = "/tickets";
        authoring
            .call(
                collection.into(),
                crate::wip::root_reference("yoi.ticket/collection/v1"),
                "query".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let created = authoring
            .call(
                collection.into(),
                crate::wip::root_reference("yoi.ticket/collection/v1"),
                "create".into(),
                json!({"title": "New ticket"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(created.content.unwrap().contains("/tickets/T-9"));
        let item = "/tickets/T-9";
        authoring.tree(item.into(), 0, false).await.unwrap();
        authoring.inspect(item.into(), false).await.unwrap();
        authoring
            .call(
                item.into(),
                crate::wip::root_reference("yoi.ticket/item/v1"),
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
        let route = workflow_registry
            .allocate_namespace("ticket", "tickets")
            .unwrap();
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
        workflow.tree(item.into(), 0, false).await.unwrap();
        workflow.inspect(item.into(), false).await.unwrap();
        let result = workflow
            .call(
                item.into(),
                crate::wip::root_reference("yoi.ticket/item/v1"),
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
    async fn merge_request_native_runtime_covers_coder_reviewer_and_orchestrator_authority() {
        use crate::worker::{ReviewerChildWorkspaceClient, ReviewerContext};
        use manifest::MergeRequestFeatureConfig;

        let coder_client = Arc::new(ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "workspace_id": "workspace",
                    "merge_request_id": "MR-1",
                    "repository_key": "main",
                    "state": "open",
                    "selector_from": "work/T-685-native-mr",
                    "selector_to": "develop",
                    "ticket_ids": ["ticket-internal"],
                    "created_at": "2026-10-03T00:00:00Z",
                    "updated_at": "2026-10-03T00:00:00Z",
                    "thread": []
                })
                .to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "workspace_id": "workspace",
                    "merge_request_id": "MR-1",
                    "repository_key": "main",
                    "state": "open",
                    "selector_from": "work/T-685-native-mr",
                    "selector_to": "develop",
                    "ticket_ids": ["ticket-internal"],
                    "created_at": "2026-10-03T00:00:00Z",
                    "updated_at": "2026-10-03T00:00:00Z",
                    "thread": [],
                    "source": {"status": "known", "ref": "source-1", "observed_at": "2026-10-03T00:00:00Z"},
                    "target": {"status": "known", "ref": "target-1", "observed_at": "2026-10-03T00:00:00Z"},
                    "linked_tickets": [{"ticket_id": "ticket-internal", "key": "T-685"}]
                })
                .to_string(),
            },
        ]));
        let coder_config = MergeRequestFeatureConfig {
            show: true,
            open: true,
            ..Default::default()
        };
        let mut coder_engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in crate::feature::builtin::merge_request::enabled_merge_request_definitions(
            coder_client.clone(),
            coder_config,
        ) {
            coder_engine.register_tool(definition);
        }
        let mut coder_registry = WipMountRegistry::new();
        let route = coder_registry
            .allocate_namespace("merge-request", "merge-requests")
            .unwrap();
        crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
            &mut coder_registry,
            coder_client.clone(),
            coder_config,
            None,
            &route,
        )
        .unwrap();
        let coder = install_wip_mode_with_mounts(
            &mut coder_engine,
            None,
            "coder-worker".into(),
            coder_registry,
        )
        .unwrap();
        let discovered = coder.tree("/".into(), 3, false).await.unwrap();
        let discovered = discovered.content.unwrap();
        assert!(discovered.contains("/merge-requests"));
        assert!(!discovered.contains("/tools/OpenMergeRequest"));
        assert!(!discovered.contains("/tools/ShowMergeRequest"));
        coder
            .inspect("/merge-requests".into(), false)
            .await
            .unwrap();
        let collection = "/merge-requests";
        let opened = coder
            .call(
                collection.into(),
                crate::wip::root_reference("yoi.merge-request/collection/v1"),
                "open".into(),
                json!({
                    "ticket": "T-685",
                    "repository_key": "main",
                    "selector_from": "work/T-685-native-mr",
                    "selector_to": "develop"
                }),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(
            opened
                .content
                .unwrap()
                .contains(&format!("{collection}/MR-1"))
        );
        let item = format!("{collection}/MR-1");
        coder.tree(item.clone(), 0, false).await.unwrap();
        coder.inspect(item.clone(), false).await.unwrap();
        coder
            .call(
                item.clone(),
                crate::wip::root_reference("yoi.merge-request/item/v1"),
                "read".into(),
                json!({"after": 7, "limit": 25}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let coder_requests = coder_client.requests();
        assert_eq!(coder_requests.len(), 2);
        assert_eq!(
            coder_requests[0].path,
            "/api/w/workspace/tickets/T-685/merge-request"
        );
        let open_body: Json =
            serde_json::from_str(coder_requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(open_body["repository_key"], "main");
        assert_eq!(open_body["selector_from"], "work/T-685-native-mr");
        assert_eq!(open_body["selector_to"], "develop");
        assert_eq!(
            coder_requests[1].path,
            "/api/w/workspace/merge-requests/MR-1?after=7&limit=25"
        );

        let reviewer_inner = Arc::new(ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "event_id": "review-1",
                    "sequence": 2,
                    "request_event_id": "request-1",
                    "subject_ref": "source-1",
                    "ticket_item_revision": "ticket-rev-1",
                    "ticket_merge_request_subjects": [{"merge_request_id": "MR-1", "subject_ref": "source-1"}],
                    "decision": "approve",
                    "body": "approved",
                    "findings": [],
                    "reviewer": {"runtime_id": "runtime", "worker_id": "reviewer"},
                    "created_at": "2026-10-03T00:00:00Z"
                })
                .to_string(),
            },
        ]));
        let reviewer_client: Arc<dyn crate::worker::WorkspaceClient> =
            Arc::new(ReviewerChildWorkspaceClient::new(
                reviewer_inner.clone(),
                ReviewerContext {
                    ticket_id: "T-685".into(),
                    merge_request_id: "MR-1".into(),
                },
                "review-capability-secret".into(),
            ));
        let reviewer_config = MergeRequestFeatureConfig {
            show: true,
            review: true,
            ..Default::default()
        };
        let mut reviewer_engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in crate::feature::builtin::merge_request::enabled_merge_request_definitions(
            reviewer_client.clone(),
            reviewer_config,
        ) {
            reviewer_engine.register_tool(definition);
        }
        let mut reviewer_registry = WipMountRegistry::new();
        let route = reviewer_registry
            .allocate_namespace("merge-request", "merge-requests")
            .unwrap();
        crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
            &mut reviewer_registry,
            reviewer_client,
            reviewer_config,
            None,
            &route,
        )
        .unwrap();
        let reviewer = install_wip_mode_with_mounts(
            &mut reviewer_engine,
            None,
            "reviewer-worker".into(),
            reviewer_registry,
        )
        .unwrap();
        reviewer.tree(item.clone(), 0, false).await.unwrap();
        reviewer.inspect(item.clone(), false).await.unwrap();
        reviewer
            .call(
                item.clone(),
                crate::wip::root_reference("yoi.merge-request/item/v1"),
                "review".into(),
                json!({"decision": "approve", "body": "approved", "findings": []}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let reviewer_requests = reviewer_inner.requests();
        assert_eq!(reviewer_requests.len(), 1);
        assert_eq!(
            reviewer_requests[0].path,
            "/api/w/workspace/merge-requests/MR-1/reviews"
        );
        let review_body: Json =
            serde_json::from_str(reviewer_requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(review_body["capability_token"], "review-capability-secret");

        let orchestrator_client = Arc::new(ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "ready": true,
                    "blockers": [],
                    "subject_ref": "source-1",
                    "review": {"event_id": "review-1", "decision": "approve"}
                })
                .to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({
                    "event_id": "merge-1",
                    "approved_source_ref": "source-1",
                    "target_ref_before": "target-1",
                    "target_ref_after": "result-1"
                })
                .to_string(),
            },
        ]));
        let orchestrator_config = MergeRequestFeatureConfig {
            readiness_check: true,
            complete: true,
            ..Default::default()
        };
        let mut orchestrator_engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        for definition in crate::feature::builtin::merge_request::enabled_merge_request_definitions(
            orchestrator_client.clone(),
            orchestrator_config,
        ) {
            orchestrator_engine.register_tool(definition);
        }
        let mut orchestrator_registry = WipMountRegistry::new();
        let route = orchestrator_registry
            .allocate_namespace("merge-request", "merge-requests")
            .unwrap();
        crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
            &mut orchestrator_registry,
            orchestrator_client.clone(),
            orchestrator_config,
            None,
            &route,
        )
        .unwrap();
        let orchestrator = install_wip_mode_with_mounts(
            &mut orchestrator_engine,
            None,
            "orchestrator-worker".into(),
            orchestrator_registry,
        )
        .unwrap();
        orchestrator.tree(item.clone(), 0, false).await.unwrap();
        orchestrator.inspect(item.clone(), false).await.unwrap();
        orchestrator
            .call(
                item.clone(),
                crate::wip::root_reference("yoi.merge-request/item/v1"),
                "check_readiness".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        orchestrator.tree(item.clone(), 0, true).await.unwrap();
        orchestrator
            .call(
                item.clone(),
                crate::wip::root_reference("yoi.merge-request/item/v1"),
                "complete".into(),
                json!({
                    "operation_id": "merge-op-1",
                    "approval_event_id": "review-1",
                    "target_ref_before": "target-1",
                    "target_ref_after": "result-1",
                    "strategy": "fast_forward",
                    "resolution": "clean"
                }),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        orchestrator
            .tree(collection.into(), 0, false)
            .await
            .unwrap();
        orchestrator
            .inspect("/merge-requests".into(), false)
            .await
            .unwrap();
        let rejected = orchestrator
            .call(
                collection.into(),
                crate::wip::root_reference("yoi.merge-request/collection/v1"),
                "complete_ticket".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .expect_err("Ticket completion is not a Merge Request operation");
        assert!(
            rejected.to_string().contains("operation is not declared"),
            "{rejected}"
        );
        let orchestrator_requests = orchestrator_client.requests();
        assert_eq!(orchestrator_requests.len(), 2);
        assert_eq!(
            orchestrator_requests[0].path,
            "/api/w/workspace/merge-requests/MR-1/readiness"
        );
        assert_eq!(
            orchestrator_requests[1].path,
            "/api/w/workspace/merge-requests/MR-1/complete"
        );
        let completion_body: Json =
            serde_json::from_str(orchestrator_requests[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(completion_body["approval_event_id"], "review-1");
        assert_eq!(completion_body["target_ref_before"], "target-1");
        assert_eq!(completion_body["target_ref_after"], "result-1");
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
        let namespace_route = registry
            .allocate_namespace("objective", "objectives")
            .unwrap();
        crate::feature::builtin::objective::mount_workspace_http_objective_wip(
            &mut registry,
            client.clone(),
            permissions.clone(),
            &namespace_route,
        )
        .unwrap();
        let runtime =
            install_wip_mode_with_mounts(&mut engine, permissions, "worker-a".into(), registry)
                .unwrap();

        let discovered = runtime.tree("/".into(), 3, false).await.unwrap();
        let discovered = discovered.content.unwrap();
        assert!(discovered.contains("/objectives"));
        assert!(!discovered.contains("/tools/QueryObjective"));
        runtime.inspect("/objectives".into(), false).await.unwrap();
        let collection_path = "/objectives";
        runtime
            .call(
                collection_path.into(),
                crate::wip::root_reference("yoi.objective/collection/v1"),
                "query".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let denied = runtime
            .call(
                collection_path.into(),
                crate::wip::root_reference("yoi.objective/collection/v1"),
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
                crate::wip::root_reference("yoi.objective/collection/v1"),
                "create".into(),
                json!({"title": "Objective"}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(created.content.unwrap().contains("/objectives/O-3"));

        let item_path = "/objectives/O-3";
        runtime.tree(item_path.into(), 0, false).await.unwrap();
        runtime.inspect(item_path.into(), false).await.unwrap();
        for (operation, arguments) in [
            ("read", json!({})),
            ("edit", json!({"title": "Changed"})),
            ("set_state", json!({"state": "paused"})),
            ("link_ticket", json!({"ticket_id": "T-7"})),
            ("unlink_ticket", json!({"ticket_id": "T-7"})),
        ] {
            runtime.tree(item_path.into(), 0, true).await.unwrap();
            runtime
                .call(
                    item_path.into(),
                    crate::wip::root_reference("yoi.objective/item/v1"),
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
        runtime.tree("/".into(), 2, false).await.unwrap();
        runtime
            .inspect(format!("/tools/{name}"), false)
            .await
            .unwrap();
        runtime
    }

    struct UnrelatedCancellation {
        cancellations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WipOperationHandler for UnrelatedCancellation {
        async fn call(
            &self,
            _: &str,
            _: &BTreeMap<String, Value>,
            _: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            panic!("unrelated provider must not execute");
        }

        async fn cancel(&self, _: &ToolExecutionContext) -> Result<(), ToolError> {
            self.cancellations.fetch_add(1, Ordering::SeqCst);
            Err(ToolError::InvalidArgument(
                "unknown execution in unrelated provider".into(),
            ))
        }
    }

    struct PendingCancellation {
        started: Arc<tokio::sync::Notify>,
        cancelled: tokio::sync::Notify,
        was_called: AtomicBool,
        cancellations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WipOperationHandler for PendingCancellation {
        async fn call(
            &self,
            _: &str,
            _: &BTreeMap<String, Value>,
            _: WipCallContext,
        ) -> Result<WipOperationOutput, WipOperationError> {
            self.was_called.store(true, Ordering::SeqCst);
            self.started.notify_one();
            self.cancelled.notified().await;
            Err(WipOperationError::Cancelled(
                "provider confirmed cancellation".to_string().into(),
            ))
        }

        async fn cancel(&self, _: &ToolExecutionContext) -> Result<(), ToolError> {
            // A second resolution of the same path would produce another instance.
            assert!(
                self.was_called.load(Ordering::SeqCst),
                "cancel must use the dispatched instance"
            );
            self.cancellations.fetch_add(1, Ordering::SeqCst);
            self.cancelled.notify_one();
            Ok(())
        }
    }

    struct PendingCancellationResolver {
        started: Arc<tokio::sync::Notify>,
        cancellations: Arc<AtomicUsize>,
    }

    impl WipDynamicOperationResolver for PendingCancellationResolver {
        fn handler(&self, _: &str) -> Arc<dyn WipOperationHandler> {
            Arc::new(PendingCancellation {
                started: Arc::clone(&self.started),
                cancelled: tokio::sync::Notify::new(),
                was_called: AtomicBool::new(false),
                cancellations: Arc::clone(&self.cancellations),
            })
        }
    }

    struct CancellationItemResolver {
        unrelated: Arc<AtomicUsize>,
    }

    impl WipDynamicItemResolver for CancellationItemResolver {
        fn resolve(&self, item: &str) -> Option<WipDynamicItem> {
            if item != "A-1" {
                return None;
            }
            let projection = contribution_projection(Arc::new(UnrelatedCancellation {
                cancellations: Arc::clone(&self.unrelated),
            }));
            Some(WipDynamicItem {
                object: projection.object,
                handler: projection.handler,
            })
        }
    }

    async fn assert_contributed_cancellation_is_selected_once(dynamic: bool) {
        let started = Arc::new(tokio::sync::Notify::new());
        let cancellations = Arc::new(AtomicUsize::new(0));
        let unrelated = Arc::new(AtomicUsize::new(0));
        let mut registry = WipMountRegistry::new();
        registry.allocate_namespace("asset", "assets").unwrap();
        let projection = contribution_projection(Arc::new(UnrelatedCancellation {
            cancellations: Arc::clone(&unrelated),
        }));
        let mut descriptor = projection.descriptor.clone();
        // Two names share one provider; read sorts first and fails cancellation.
        descriptor.operations = vec![
            contributed_operation("write"),
            contributed_operation("write_again"),
        ];
        if dynamic {
            let mut collection = projection.clone();
            collection.route = "/assets".into();
            collection.object.name = "assets".into();
            collection.interface = crate::wip::root_reference("test.asset/collection/v1");
            collection.object.interfaces = vec![collection.interface.clone()];
            registry.mount(collection).unwrap();
            registry
                .mount_dynamic(WipDynamicMount {
                    collection_route: "/assets".into(),
                    capability: "asset:item".into(),
                    interface: projection.interface.clone(),
                    descriptor: projection.descriptor,
                    interface_validator: projection.interface_validator,
                    resolver: Arc::new(CancellationItemResolver {
                        unrelated: Arc::clone(&unrelated),
                    }),
                })
                .unwrap();
            registry
                .contribute_dynamic_operations(WipDynamicOperationContribution {
                    collection_route: "/assets".into(),
                    contributor: "management".into(),
                    interface: projection.interface,
                    descriptor,
                    resolver: Arc::new(PendingCancellationResolver {
                        started: Arc::clone(&started),
                        cancellations: Arc::clone(&cancellations),
                    }),
                })
                .unwrap();
        } else {
            registry.mount(projection).unwrap();
            registry
                .contribute_operations(WipOperationContribution {
                    route: "/assets/A-1".into(),
                    contributor: "management".into(),
                    interface: root_reference("test.asset/item/v1"),
                    descriptor,
                    handler: PendingCancellationResolver {
                        started: Arc::clone(&started),
                        cancellations: Arc::clone(&cancellations),
                    }
                    .handler("A-1"),
                })
                .unwrap();
        }
        let runtime =
            WipRuntime::new(WipHost::new(registry), SecurityContext::new("worker-a"), 0).unwrap();
        runtime.tree("/assets/A-1".into(), 0, false).await.unwrap();
        runtime.inspect("/assets/A-1".into(), false).await.unwrap();
        let execution = ToolExecutionContext::direct();
        let call = runtime.call(
            "/assets/A-1".into(),
            crate::wip::root_reference("test.asset/item/v1"),
            "write".into(),
            json!({}),
            execution.clone(),
        );
        let cancel = async {
            started.notified().await;
            // Unknown executions must not invoke any provider.
            runtime
                .cancel(&ToolExecutionContext::new("unknown", "other-batch", 0))
                .await
                .unwrap();
            runtime.cancel(&execution).await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(call, cancel)
        })
        .await
        .expect("selected provider must receive cancellation");
        assert!(matches!(result, Err(ToolError::Cancelled(_))));
        assert_eq!(unrelated.load(Ordering::SeqCst), 0);
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
        assert!(runtime.active.lock().unwrap().is_empty());
        assert_eq!(runtime.audit()[0].outcome, WipAuditOutcome::Cancelled);
        runtime.cancel(&execution).await.unwrap();
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn static_contributed_cancellation_ignores_unrelated_and_shared_handlers() {
        assert_contributed_cancellation_is_selected_once(false).await;
    }

    #[tokio::test]
    async fn dynamic_contributed_cancellation_pins_the_dispatched_instance() {
        assert_contributed_cancellation_is_selected_once(true).await;
    }

    #[tokio::test]
    async fn cancelled_unknown_and_disconnected_calls_have_distinct_terminal_states() {
        let cancelled =
            prepare_runtime_with_tool("Cancelled", Arc::new(FailingTool { cancelled: true })).await;
        let error = cancelled
            .call(
                "/tools/Cancelled".into(),
                crate::wip::root_reference("yoi.tool/Cancelled/v1"),
                "call".into(),
                json!({"input":{"message": "ok"}}),
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
                crate::wip::root_reference("yoi.tool/Unknown/v1"),
                "call".into(),
                json!({"input":{"message": "ok"}}),
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
            .tree("/tools/Unknown".into(), 0, true)
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
                    &root_reference("yoi.tool/Unknown/v1"),
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
        runtime.tree("/".into(), 2, false).await.unwrap();
        runtime.inspect("/tools/Echo".into(), false).await.unwrap();
        let error = runtime
            .call(
                "/tools/Echo".into(),
                crate::wip::root_reference("yoi.tool/Echo/v1"),
                "call".into(),
                json!({"input":{"message": "ok"}}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("PermissionDenied"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
