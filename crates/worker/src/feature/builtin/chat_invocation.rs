//! Builtin metadata for client-local attachment staging.
//!
//! `/attach` uses the common invocation declaration and parser, but its path is
//! intentionally consumed by the client adapter. The Worker accepts only the
//! resulting immutable `UploadedFile` segment and never resolves a client path.

use async_trait::async_trait;
use protocol::{
    FeatureInvocation, FeatureInvocationDescriptor, FeatureInvocationIdentity,
    FeatureInvocationResult, FeatureInvocationSyntax, InvocationArgumentDescriptor,
    InvocationArgumentType, InvocationClientAdapter, InvocationCompletion,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInvocationContext,
    FeatureInvocationHandler, FeatureInvocationHandlerError, FeatureModule,
};

pub const ATTACH_FEATURE_ID: &str = "attachments";
pub const ATTACH_INVOCATION_ID: &str = "builtin:attachments/attach";

pub fn attach_invocation_descriptor() -> FeatureInvocationDescriptor {
    FeatureInvocationDescriptor {
        identity: FeatureInvocationIdentity(ATTACH_INVOCATION_ID.into()),
        name: "attach".into(),
        aliases: Vec::new(),
        display_name: "Attach file".into(),
        description: "Stage a client-local file as an editable attachment chip.".into(),
        syntax: FeatureInvocationSyntax::Parenthesized,
        arguments: vec![InvocationArgumentDescriptor {
            name: "file".into(),
            position: Some(0),
            required: true,
            value_type: InvocationArgumentType::ClientFile,
            completion: InvocationCompletion::ClientFile,
            description: Some("File available to this client".into()),
        }],
        client_adapter: Some(InvocationClientAdapter::Attachment),
    }
}

pub struct AttachmentInvocationFeature;

impl FeatureModule for AttachmentInvocationFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(ATTACH_FEATURE_ID, "Attachments")
            .with_description("Client-local attachment staging and typed uploaded-file input")
            .with_chat_invocation(attach_invocation_descriptor())
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.chat_invocations().register(
            attach_invocation_descriptor(),
            AttachmentMustBeStagedByClient,
        )
    }
}

struct AttachmentMustBeStagedByClient;

#[async_trait]
impl FeatureInvocationHandler for AttachmentMustBeStagedByClient {
    async fn invoke(
        &self,
        _context: FeatureInvocationContext,
        _invocation: &FeatureInvocation,
    ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
        Err(FeatureInvocationHandlerError::failed(
            "attachment invocation reached the Worker without client-local upload staging",
        ))
    }
}
