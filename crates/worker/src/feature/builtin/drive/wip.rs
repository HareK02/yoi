//! Native Drive projection with no indexable entry children.
//!
//! `/drive` is a discovery entrance; Tree does not enumerate file inventory. Only entries selected
//! from bounded list/search results resolve directly. Paths contain a hex-encoded Workspace ID and
//! decimal node ID, preventing substitution with another Workspace or ID. Previously acquired
//! Client entries are not added to Tree either.
//!
//! The provider resolves one entry under current Backend authorization and creates a self-scoped
//! contract for its kind. Metadata, descriptor, validator, and handler are fixed to one publication;
//! the route binds the entry/creation parent. Move destinations accept typed Entry paths.
//! Interfaces describe operations for the entry kind; the Backend rejects writes under read-only
//! grants. Native Invoke still applies original Tool permissions. [`crate::wip`] owns the shared
//! cache, validator, and cancellation contracts.
use super::{
    DriveFeature, SPECS, Spec,
    backend::{DriveError, decimal},
    digest, entry_path,
};
use crate::wip::{
    WipCallContext, WipMountError, WipMountRegistry, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, WipPublication, WipSubtreeMount,
    WipSubtreeProvider, contextual_reference, json_to_wip, wip_to_json,
};
use async_trait::async_trait;
use serde_json::json;
use server_api::{DriveEntry, DriveEntryKind, DriveEntryRef};
use std::{collections::BTreeMap, sync::Arc};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};

const ROOT: &str = "/drive";
const INTERFACE: &str = "yoi.drive/entry/v1";

pub fn mount_drive_wip(
    registry: &mut WipMountRegistry,
    feature: &DriveFeature,
) -> Result<(), WipMountError> {
    registry.allocate_namespace("drive", "drive")?;
    // Registration is only a namespace entrance. The subtree provides the
    // current target/scope/descriptor/handler in a single self-scoped snapshot.
    // This unresolved entrance has no live capability evidence.
    let names: Vec<_> = root_specs()
        .into_iter()
        .filter(|spec| !mutation(spec))
        .collect();
    let descriptor = descriptor(&names, true);
    registry.mount(WipProjection {
        route: ROOT.into(),
        capability: "drive:content".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: "drive".into(),
            description: Some(
                "Authorized Workspace Drive; discover entries via bounded list/search, not tree"
                    .into(),
            ),
            interfaces: vec![contextual_reference(INTERFACE, ROOT)],
            r#ref: None,
            validator: None,
        },
        interface: contextual_reference(INTERFACE, ROOT),
        interface_validator: Some(digest(format!("{descriptor:?}").as_bytes()).into_bytes()),
        descriptor,
        handler: Arc::new(Unresolved),
    })?;
    registry.mount_subtree(WipSubtreeMount {
        root: ROOT.into(),
        provider: Arc::new(DriveProvider::new(feature.clone())),
    })?;
    registry.replace_compatibility_tools(ROOT, SPECS.iter().map(|spec| spec.name))?;
    Ok(())
}
struct Unresolved;
#[async_trait]
impl WipOperationHandler for Unresolved {
    async fn call(
        &self,
        _: &str,
        _: &BTreeMap<String, Value>,
        _: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        Err(map_error(DriveError::Denied))
    }
}
#[derive(Clone)]
pub(super) struct DriveProvider {
    feature: DriveFeature,
    observation_nonce: Arc<str>,
}
impl DriveProvider {
    pub(super) fn new(feature: DriveFeature) -> Self {
        Self {
            feature,
            observation_nonce: uuid::Uuid::now_v7().to_string().into(),
        }
    }
}
#[async_trait]
impl WipSubtreeProvider for DriveProvider {
    // Arbitrary tree depth is safe: there are deliberately no indexable entries.
    fn max_depth(&self) -> u32 {
        32
    }
    fn max_nodes(&self) -> usize {
        1
    }
    async fn publication(&self, path: &str) -> Result<Option<WipPublication>, ProtocolError> {
        let result = if path == ROOT {
            self.feature.backend.root().await
        } else {
            let Some(entry) = parse_entry(&self.feature, path).map_err(protocol)? else {
                return Ok(None);
            };
            self.feature.backend.metadata(&entry).await
        };
        let entry = match result {
            Ok(entry) => entry,
            Err(DriveError::Denied | DriveError::NotFound) => return Ok(None),
            Err(error) => return Err(protocol(error)),
        };
        let capabilities = match self.feature.backend.capabilities().await {
            Ok(capabilities) => capabilities,
            Err(DriveError::Denied | DriveError::NotFound) => return Ok(None),
            Err(error) => return Err(protocol(error)),
        };
        let root = path == ROOT;
        let specs: Vec<_> = if root {
            root_specs()
        } else {
            entry_specs(&entry)
        }
        .into_iter()
        .filter(|spec| {
            !mutation(spec) || (capabilities.write && publication_may_allow(&self.feature, spec))
        })
        .collect();
        let common_mutations: Vec<_> = specs
            .iter()
            .filter_map(|spec| match spec.operation {
                "edit" => Some(crate::file_operation::Contract::Edit),
                "write" => Some(crate::file_operation::Contract::Write),
                _ => None,
            })
            .collect();
        let descriptor = descriptor(&specs, root);
        let reference = contextual_reference(INTERFACE, path);
        let validator = object_validator(
            &self.observation_nonce,
            &entry,
            &entry.last_mutation_id,
            &specs,
        );
        let object = Object {
            name: if root {
                "drive".into()
            } else {
                entry.entry.node_id.clone()
            },
            description: Some(format!(
                "Workspace Drive {:?} {:?}; stable ID {}, bodies only through bounded operations",
                entry.kind, entry.name, entry.entry.node_id
            )),
            interfaces: vec![reference.clone()],
            r#ref: Some(format!(
                "drive:{}:{}",
                entry.entry.workspace_id, entry.entry.node_id
            )),
            validator: Some(validator),
        };
        let common_read = entry.kind == DriveEntryKind::File;
        let mut projection = WipProjection {
            route: path.into(),
            capability: "drive:content".into(),
            kind: WipProjectionKind::Native,
            object,
            interface: reference,
            interface_validator: Some(digest(format!("{descriptor:?}").as_bytes()).into_bytes()),
            descriptor,
            handler: Arc::new(DriveHandler {
                feature: self.feature.clone(),
                observation_nonce: self.observation_nonce.clone(),
                entry,
                specs,
                root,
            }),
        };
        // This is publication evidence only. Backend execution rechecks current
        // authority and the captured commit precondition on every mutation.
        crate::file_operation::expose(&mut projection, common_mutations);
        if common_read {
            crate::file_operation::expose(
                &mut projection,
                [
                    crate::file_operation::Contract::ReadLines,
                    crate::file_operation::Contract::SearchText,
                ],
            );
        }
        WipPublication::self_scoped(projection).map(Some)
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, ProtocolError> {
        Ok(Vec::new())
    }
}
fn object_validator(
    observation_nonce: &str,
    entry: &DriveEntry,
    mutation: &str,
    specs: &[Spec],
) -> Vec<u8> {
    // Object observations belong to one mount, even when a restored Host reuses
    // the same Feature instance. Feature.nonce separately identifies mutations.
    // Current membership is part of Object representation, not the shared Interface lifetime.
    let operations: Vec<_> = specs.iter().map(|spec| spec.operation).collect();
    digest(
        format!(
            "{observation_nonce:?}:{:?}:{:?}:{mutation:?}:{operations:?}",
            entry.entry.workspace_id, entry.entry.node_id
        )
        .as_bytes(),
    )
    .into_bytes()
}

fn mutation(spec: &Spec) -> bool {
    matches!(
        spec.operation,
        "write" | "edit" | "delete" | "relocate" | "create_folder" | "create_text" | "save_workdir"
    )
}

fn publication_may_allow(feature: &DriveFeature, spec: &Spec) -> bool {
    let Some(permissions) = &feature.permissions else {
        return true;
    };
    // Like checkout publication, retain potentially allowed argument-dependent
    // operations but hide an unconditional Deny/Ask ceiling. This is not an
    // authorization decision: exact arguments are still checked at execution.
    for rule in permissions
        .rules
        .iter()
        .filter(|rule| rule.tool.eq_ignore_ascii_case(spec.name))
    {
        if rule.action == manifest::ToolPermissionAction::Allow {
            return true;
        }
        if rule.pattern == "*" {
            return false;
        }
    }
    permissions.default_action == manifest::ToolPermissionAction::Allow
}

fn root_specs() -> Vec<Spec> {
    SPECS
        .iter()
        .copied()
        .filter(|spec| {
            matches!(
                spec.operation,
                "root"
                    | "metadata"
                    | "list"
                    | "search"
                    | "create_folder"
                    | "create_text"
                    | "save_workdir"
                    | "request_status"
            )
        })
        .collect()
}
fn entry_specs(entry: &DriveEntry) -> Vec<Spec> {
    SPECS
        .iter()
        .copied()
        .filter(|spec| match spec.operation {
            "metadata" => true,
            "relocate" | "delete" => entry.parent.is_some(),
            "list" | "create_folder" | "create_text" | "save_workdir" => {
                entry.kind == DriveEntryKind::Folder
            }
            "read" | "write" | "edit" | "view_image" => entry.kind == DriveEntryKind::File,
            _ => false,
        })
        .collect()
}
fn parameter(name: &str, required: bool, r#type: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type,
    }
}
fn descriptor(specs: &[Spec], _root: bool) -> InterfaceDescriptor {
    InterfaceDescriptor {format:INTERFACE_FORMAT_V1.into(),documentation:Some(Documentation {summary:"Backend-authorized Workspace Drive operations".into(),details:Some("Entry IDs are Workspace-bound. The Host/Client retain the last committed request observed for an entry. Never supply validators or silently retry a conflict against a newer observation. On outcome-unknown, query the original request receipt. Read-only grants reject mutations at Backend. List/search return paths for direct Inspect, never tree inventories. Image bytes attach to durable ToolOutput; URLs are not image viewing. Workdir source read and Drive destination write are independent.".into())}),types:Vec::new(),operations:specs.iter().filter(|spec| !matches!(spec.operation,"edit"|"write")).map(|spec|OperationDeclaration {name:spec.operation.into(),documentation:Some(Documentation {summary:spec.description.into(),details:None}),parameters:match spec.operation {
        "list"=>vec![parameter("limit",false,TypeExpr::Integer),parameter("after",false,TypeExpr::String)],
        "search"=>vec![parameter("query",true,TypeExpr::String),parameter("include_text",false,TypeExpr::Boolean),parameter("limit",false,TypeExpr::Integer),parameter("after",false,TypeExpr::String)],
        "read"=>vec![parameter("max_bytes",false,TypeExpr::Integer)],
        "create_text"=>vec![parameter("name",true,TypeExpr::String),parameter("content",true,TypeExpr::String),parameter("content_type",false,TypeExpr::String)],
        "create_folder"=>vec![parameter("name",true,TypeExpr::String)],
        "relocate"=>vec![parameter("parent",true,TypeExpr::Entry),parameter("name",true,TypeExpr::String)],
        "save_workdir"=>vec![parameter("target_workdir",true,TypeExpr::String),parameter("path",true,TypeExpr::String),parameter("name",true,TypeExpr::String),parameter("content_type",false,TypeExpr::String)],
        "request_status"=>vec![parameter("request_id",true,TypeExpr::String)],
        _=>vec![],
    },returns:ReturnDeclaration {documentation:None,r#type:TypeExpr::Json}}).collect()}
}
struct DriveHandler {
    feature: DriveFeature,
    observation_nonce: Arc<str>,
    entry: DriveEntry,
    specs: Vec<Spec>,
    root: bool,
}
#[async_trait]
impl WipOperationHandler for DriveHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let common_text = matches!(operation, "read_lines" | "search_text");
        let backend_operation = if common_text { "read" } else { operation };
        let spec = self
            .specs
            .iter()
            .find(|spec| spec.operation == backend_operation)
            .ok_or_else(|| {
                failure(
                    ProtocolErrorCode::OperationNotFound,
                    "Drive operation is not published",
                )
            })?;
        let contract = match operation {
            "read_lines" => Some(crate::file_operation::Contract::ReadLines),
            "search_text" => Some(crate::file_operation::Contract::SearchText),
            "edit" => Some(crate::file_operation::Contract::Edit),
            "write" => Some(crate::file_operation::Contract::Write),
            _ => None,
        };
        let declaration = if let Some(contract) = contract {
            contract.descriptor().operations.into_iter().next().unwrap()
        } else {
            descriptor(&self.specs, self.root)
                .operations
                .into_iter()
                .find(|op| op.name == operation)
                .unwrap()
        };
        if arguments
            .keys()
            .any(|key| !declaration.parameters.iter().any(|p| p.name == *key))
        {
            return Err(map_error(DriveError::Invalid(
                "unrecognized arguments; committed-request observations are Client-managed".into(),
            )));
        }
        let mut input = if common_text {
            // Validate the common input before performing any source read. Its
            // line window is evaluated only after the complete bounded read.
            crate::file_operation::text_result(operation, arguments, "")?;
            if self
                .entry
                .size
                .is_some_and(|size| size > server_api::DRIVE_CHUNK_MAX_BYTES)
            {
                return Err(map_error(DriveError::Limit));
            }
            json!({"entry": self.entry.entry, "max_bytes": server_api::DRIVE_CHUNK_MAX_BYTES})
        } else {
            wip_to_json(&Value::Record(arguments.clone()))
                .map_err(|_| map_error(DriveError::Invalid("invalid WIP arguments".into())))?
        };
        let fields = input
            .as_object_mut()
            .ok_or_else(|| map_error(DriveError::Invalid("named arguments required".into())))?;
        match operation {
            "metadata" | "read" | "write" | "edit" | "delete" | "view_image" => {
                fields.insert("entry".into(), json!(self.entry.entry));
            }
            "list" | "create_folder" | "create_text" | "save_workdir" => {
                fields.insert("parent".into(), json!(self.entry.entry));
            }
            "relocate" => {
                let path = match arguments.get("parent") {
                    Some(Value::String(path)) => path.as_str(),
                    _ => {
                        return Err(map_error(DriveError::Invalid(
                            "parent must be a Drive entry".into(),
                        )));
                    }
                };
                let parent = if path == ROOT {
                    self.feature.backend.root().await.map_err(map_error)?.entry
                } else {
                    parse_entry(&self.feature, path)
                        .map_err(map_error)?
                        .ok_or_else(|| map_error(DriveError::Denied))?
                };
                fields.insert("parent".into(), json!(parent));
                fields.insert("entry".into(), json!(self.entry.entry));
            }
            _ => {}
        }
        if let Some(permissions) = &self.feature.permissions {
            if !matches!(
                crate::permission::permission_action_for(permissions, spec.name, &input),
                manifest::ToolPermissionAction::Allow
            ) {
                return Err(failure(
                    ProtocolErrorCode::PermissionDenied,
                    "Drive operation denied by tool permission; approval unavailable in native invocation",
                ));
            }
        }
        let output = self
            .feature
            .execute(
                spec.name,
                &input.to_string(),
                &context.execution,
                Some(&self.entry),
            )
            .await
            .map_err(map_error)?;
        if common_text {
            let content = complete_text(&output.value)?;
            let value = crate::file_operation::text_result(operation, arguments, content)?;
            return Ok(WipOperationOutput::native_with_validator(
                value,
                object_validator(
                    &self.observation_nonce,
                    &self.entry,
                    &self.entry.last_mutation_id,
                    &self.specs,
                ),
            ));
        }
        let value = json_to_wip(&output.value).map_err(|_| {
            WipOperationError::OutcomeUnknown(
                "Completed Drive result unavailable; query request_status before another mutation"
                    .into(),
            )
        })?;
        // Attachments use the existing ToolOutput side channel and durable Engine
        // capture, not transient request injection or URL-only success.
        let result = if matches!(operation, "write" | "edit" | "relocate") {
            let last_mutation_id = output.value["entry"]["metadata"]["last_mutation_id"]
                .as_str()
                .ok_or_else(|| {
                    WipOperationError::OutcomeUnknown(
                        "Committed Drive request identity unavailable".into(),
                    )
                })?;
            WipOperationOutput::native_with_validator(
                value,
                object_validator(
                    &self.observation_nonce,
                    &self.entry,
                    last_mutation_id,
                    &self.specs,
                ),
            )
        } else {
            WipOperationOutput::native(value)
        };
        Ok(result.with_attachments(output.attachments))
    }
}
/// A bounded line/search contract cannot claim that a partial blob is the full
/// source. Retain Drive's separate whole-text read with its truncation flag.
fn complete_text(value: &serde_json::Value) -> Result<&str, WipOperationError> {
    let content = value["text"]
        .as_str()
        .ok_or_else(|| map_error(DriveError::Unavailable))?;
    let size = value["entry"]["metadata"]["size"]
        .as_u64()
        .ok_or_else(|| map_error(DriveError::Unavailable))?;
    if value["truncated"].as_bool() != Some(false)
        || size != content.len() as u64
        || content.len() > server_api::DRIVE_CHUNK_MAX_BYTES as usize
    {
        return Err(map_error(DriveError::Limit));
    }
    Ok(content)
}

fn parse_entry(feature: &DriveFeature, path: &str) -> Result<Option<DriveEntryRef>, DriveError> {
    let workspace = feature.backend.workspace()?;
    let prefix = format!(
        "/drive/{}/",
        crate::feature::builtin::manage_workdir::wip::encode_identity(workspace)
    );
    let Some(id) = path.strip_prefix(&prefix) else {
        return Ok(None);
    };
    if !decimal(id) {
        return Ok(None);
    }
    let entry = DriveEntryRef {
        workspace_id: workspace.into(),
        node_id: id.into(),
    };
    if entry_path(&entry) != path {
        return Ok(None);
    }
    Ok(Some(entry))
}
fn failure(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}
fn map_error(error: DriveError) -> WipOperationError {
    let code = match &error {
        DriveError::Denied | DriveError::Authorization { denied: true, .. } => {
            ProtocolErrorCode::PermissionDenied
        }
        DriveError::Authorization { denied: false, .. } => ProtocolErrorCode::Internal,
        DriveError::NotFound => ProtocolErrorCode::NotFound,
        DriveError::Conflict => ProtocolErrorCode::ValidatorMismatch,
        DriveError::Invalid(_) => ProtocolErrorCode::InvalidArguments,
        DriveError::Limit => ProtocolErrorCode::ResourceLimitExceeded,
        DriveError::Unavailable => ProtocolErrorCode::Internal,
        DriveError::OutcomeUnknown(_) => {
            return WipOperationError::OutcomeUnknown(error.to_string());
        }
    };
    WipOperationError::Protocol(ProtocolError {
        code,
        message: error.to_string(),
    })
}
fn protocol(error: DriveError) -> ProtocolError {
    match map_error(error) {
        WipOperationError::Protocol(error) => error,
        _ => ProtocolError {
            code: ProtocolErrorCode::Internal,
            message: "Drive observation unavailable".into(),
        },
    }
}
