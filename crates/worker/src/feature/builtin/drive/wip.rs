//! Native non-indexable Drive entries. Publication resolves just one ID through
//! current Backend authorization; the Client owns identity/revision validators.
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
    let names = root_specs();
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
        provider: Arc::new(DriveProvider(feature.clone())),
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
pub(super) struct DriveProvider(pub(super) DriveFeature);
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
            self.0.backend.root().await
        } else {
            let Some(entry) = parse_entry(&self.0, path).map_err(protocol)? else {
                return Ok(None);
            };
            self.0.backend.metadata(&entry).await
        };
        let entry = match result {
            Ok(entry) => entry,
            Err(DriveError::Denied | DriveError::NotFound) => return Ok(None),
            Err(error) => return Err(protocol(error)),
        };
        let root = path == ROOT;
        let specs = if root {
            root_specs()
        } else {
            entry_specs(&entry)
        };
        let descriptor = descriptor(&specs, root);
        let reference = contextual_reference(INTERFACE, path);
        let validator = entry.revision.as_bytes().to_vec();
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
        let projection = WipProjection {
            route: path.into(),
            capability: "drive:content".into(),
            kind: WipProjectionKind::Native,
            object,
            interface: reference,
            interface_validator: Some(digest(format!("{descriptor:?}").as_bytes()).into_bytes()),
            descriptor,
            handler: Arc::new(DriveHandler {
                feature: self.0.clone(),
                entry,
                specs,
                root,
            }),
        };
        WipPublication::self_scoped(projection).map(Some)
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, ProtocolError> {
        Ok(Vec::new())
    }
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
    InterfaceDescriptor {format:INTERFACE_FORMAT_V1.into(),documentation:Some(Documentation {summary:"Backend-authorized Workspace Drive operations".into(),details:Some("Entry IDs are Workspace-bound. The Host/Client manage revisions; never supply validators or retry conflicts/outcome-unknown with a new revision. Read-only grants reject mutations at Backend. List/search return paths for direct Inspect, never tree inventories. Image bytes attach to durable ToolOutput; URLs are not image viewing. Workdir source read and Drive destination write are independent.".into())}),types:Vec::new(),operations:specs.iter().map(|spec|OperationDeclaration {name:spec.operation.into(),documentation:Some(Documentation {summary:spec.description.into(),details:None}),parameters:match spec.operation {
        "list"=>vec![parameter("limit",false,TypeExpr::Integer),parameter("after",false,TypeExpr::String)],
        "search"=>vec![parameter("query",true,TypeExpr::String),parameter("include_text",false,TypeExpr::Boolean),parameter("limit",false,TypeExpr::Integer),parameter("after",false,TypeExpr::String)],
        "read"=>vec![parameter("max_bytes",false,TypeExpr::Integer)],
        "write"|"edit"=>crate::file_operation::parameters(spec.operation,false).expect("shared text arguments"),
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
        let spec = self
            .specs
            .iter()
            .find(|spec| spec.operation == operation)
            .ok_or_else(|| {
                failure(
                    ProtocolErrorCode::OperationNotFound,
                    "Drive operation is not published",
                )
            })?;
        let declaration = descriptor(&self.specs, self.root)
            .operations
            .into_iter()
            .find(|op| op.name == operation)
            .unwrap();
        if arguments
            .keys()
            .any(|key| !declaration.parameters.iter().any(|p| p.name == *key))
        {
            return Err(map_error(DriveError::Invalid(
                "unrecognized arguments; revision metadata is Client-managed".into(),
            )));
        }
        let mut input = wip_to_json(&Value::Record(arguments.clone()))
            .map_err(|_| map_error(DriveError::Invalid("invalid WIP arguments".into())))?;
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
        let value = json_to_wip(&output.value).map_err(|_| {
            WipOperationError::OutcomeUnknown(
                "Completed Drive result unavailable; query request_status before another mutation"
                    .into(),
            )
        })?;
        // Attachments use the existing ToolOutput side channel and durable Engine
        // capture, not transient request injection or URL-only success.
        let result = if matches!(operation, "write" | "edit" | "relocate") {
            let revision = output.value["entry"]["metadata"]["revision"]
                .as_str()
                .ok_or_else(|| {
                    WipOperationError::OutcomeUnknown("Committed Drive revision unavailable".into())
                })?;
            WipOperationOutput::native_with_validator(value, revision.as_bytes().to_vec())
        } else {
            WipOperationOutput::native(value)
        };
        Ok(result.with_attachments(output.attachments))
    }
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
        DriveError::Denied => ProtocolErrorCode::PermissionDenied,
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
