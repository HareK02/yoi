//! Shared native self-attach entrance and production request-time config subtree.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};

use super::{
    ATTACHMENT_ALIAS, CONTENT_ROOT, ConfigAttachError, WorkspaceConfigFeature, requested_access,
};
use crate::feature::builtin::manage_workdir::wip::encode_identity;
use crate::wip::{
    WipCallContext, WipMountError, WipMountRegistry, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, json_to_wip,
};

/// Mount the Feature's native attachment entrance and, for the production
/// adapter, the grant-bound config subtree. No static file inventory is created.
pub fn mount_workspace_config_wip(
    registry: &mut WipMountRegistry,
    feature: &WorkspaceConfigFeature,
) -> Result<(), WipMountError> {
    mount_workspace_config_attach_wip(registry, feature)?;
    if feature.wip_mode
        && feature
            .client
            .workspace_id()
            .is_some_and(|id| !id.is_empty())
        && let Some(backend) = &feature.content_backend
    {
        registry.mount_subtree(crate::wip::WipSubtreeMount {
            root: CONTENT_ROOT.into(),
            provider: Arc::new(super::content::ConfigProvider {
                feature: feature.clone(),
                backend: backend.clone(),
            }),
        })?;
    }
    Ok(())
}

/// Shared root registration; no OS session or duplicate attachment inventory.
pub fn mount_workspace_config_attach_wip(
    registry: &mut WipMountRegistry,
    feature: &WorkspaceConfigFeature,
) -> Result<(), WipMountError> {
    if !feature.wip_mode || feature.client.workspace_id().is_none_or(str::is_empty) {
        return Ok(());
    }
    registry.allocate_namespace("workspace-config", "workspace-config")?;
    registry.mount(attach_projection(feature))?;
    Ok(())
}

pub(super) fn attach_projection(feature: &WorkspaceConfigFeature) -> WipProjection {
    let interface = "yoi.workspace-config/attachment/v1";
    let descriptor = InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: "Attach this Workspace's logical configuration".into(),
            details: Some("Uses the same Backend ledger operation as /workspace-config. Default access is the effective grant; explicit read_write is never downgraded. No host path, Workdir session or Bash capability is created.".into()),
        }),
        types: Vec::new(),
        operations: vec![OperationDeclaration {
            name: "attach".into(),
            documentation: Some(Documentation {
                summary: "Attach or resolve the existing workspace-config connection".into(),
                details: None,
            }),
            parameters: vec![ParameterDeclaration {
                name: "access".into(),
                required: false,
                documentation: None,
                r#type: TypeExpr::String,
            }],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Json,
            },
        }],
    };
    let interface_validator = Some(Sha256::digest(format!("{descriptor:?}")).to_vec());
    WipProjection {
        route: CONTENT_ROOT.into(),
        capability: "workspace-config:attachment".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: ATTACHMENT_ALIAS.into(),
            description: Some("Logical Workspace configuration attachment entrance".into()),
            interfaces: vec![interface.into()],
            r#ref: None,
            // Attaching does not modify config content. The content subtree will
            // supply its own Backend validators for file/directory Operations.
            validator: None,
        },
        interface: interface.into(),
        descriptor,
        interface_validator,
        handler: Arc::new(AttachHandler(feature.clone())),
    }
}

struct AttachHandler(WorkspaceConfigFeature);

#[async_trait]
impl WipOperationHandler for AttachHandler {
    fn is_visible(&self) -> bool {
        self.0.is_available() && self.0.validate_access(None).is_ok()
    }

    fn operation_available(&self, operation: &str) -> bool {
        operation == "attach" && self.0.validate_access(None).is_ok()
    }

    fn contextual_interface(&self) -> bool {
        true
    }

    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if operation != "attach" {
            return Err(failure(
                ProtocolErrorCode::OperationNotFound,
                "Unknown config attachment operation",
            ));
        }
        if arguments.keys().any(|key| key != "access") {
            return Err(map_error(ConfigAttachError::InvalidRequest));
        }
        let access = match arguments.get("access") {
            None => requested_access(None),
            Some(Value::String(value)) => requested_access(Some(value)),
            _ => Err(ConfigAttachError::InvalidRequest),
        }
        .map_err(map_error)?;
        // Host execution identity, never a model-supplied source or grant.
        let key = format!("wip:{}", context.execution.execution_id());
        let result = self.0.attach(&key, access).await.map_err(map_error)?;
        let value = json_to_wip(&json!({
            "alias": ATTACHMENT_ALIAS,
            "purpose": "Workspace configuration",
            "access": result.access.as_str(),
            "connection_id": result.connection_id,
            "attachment_path": format!("/workdir-attachments/{}", encode_identity(&result.connection_id)),
            "content_path": CONTENT_ROOT,
            "already_attached": result.already_attached,
        })).map_err(|_| WipOperationError::OutcomeUnknown("Completed config attachment response unavailable".into()))?;
        Ok(WipOperationOutput::native(value))
    }
}

fn failure(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}

fn map_error(error: ConfigAttachError) -> WipOperationError {
    match error {
        ConfigAttachError::InvalidRequest => {
            failure(ProtocolErrorCode::InvalidArguments, error.message())
        }
        ConfigAttachError::Denied => failure(ProtocolErrorCode::PermissionDenied, error.message()),
        ConfigAttachError::Unavailable => {
            failure(ProtocolErrorCode::ResourceLimitExceeded, error.message())
        }
        ConfigAttachError::OutcomeUnknown => {
            WipOperationError::OutcomeUnknown(error.message().into())
        }
    }
}
