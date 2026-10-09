//! WIP-only, grant-bound Workspace configuration Feature. No local config store
//! or OS Workdir/session is created by this logical attachment.

pub mod backend;
mod content;
pub mod wip;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use protocol::{
    FeatureInvocation, FeatureInvocationDescriptor, FeatureInvocationIdentity,
    FeatureInvocationResult, FeatureInvocationStatus, FeatureInvocationSyntax,
    InvocationArgumentDescriptor, InvocationArgumentType, InvocationCompletion, InvocationValue,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInvocationContext,
    FeatureInvocationHandler, FeatureInvocationHandlerError, FeatureModule,
};
use crate::worker::WorkspaceClient;

pub const FEATURE_ID: &str = "workspace-config";
pub const INVOCATION_ID: &str = "builtin:workspace-config/attach";
pub const ATTACHMENT_ALIAS: &str = "workspace-config";
pub const CONTENT_ROOT: &str = "/workspace-config";
const DEADLINE: Duration = Duration::from_secs(30);

/// Requested restriction, not an authorization grant. `None` selects the
/// effective Backend grant. Explicit read_write must never silently downgrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigAccess {
    ReadOnly,
    ReadWrite,
}

impl ConfigAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }
}

fn requested_access(value: Option<&str>) -> Result<Option<ConfigAccess>, ConfigAttachError> {
    match value {
        None | Some("effective") => Ok(None),
        Some("read_only") => Ok(Some(ConfigAccess::ReadOnly)),
        Some("read_write") => Ok(Some(ConfigAccess::ReadWrite)),
        _ => Err(ConfigAttachError::InvalidRequest),
    }
}

/// Adapter result from the *existing Backend attachment ledger*. No Worker-local
/// attachment registry or authorization state is created from this result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigAttachment {
    pub connection_id: String,
    pub access: ConfigAccess,
    pub already_attached: bool,
}

/// Safe diagnostics only. Raw Backend/transport errors may contain host paths or
/// secrets and must be classified by the adapter, never copied into these results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigAttachError {
    InvalidRequest,
    Denied,
    Unavailable,
    OutcomeUnknown,
}

impl ConfigAttachError {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "Invalid Workspace config attachment request",
            Self::Denied => "Workspace config access denied; explicit access is not downgraded",
            Self::Unavailable => "Workspace config attachment unavailable before effects",
            Self::OutcomeUnknown => {
                "Workspace config attachment outcome unknown; inspect current attachments before retrying"
            }
        }
    }

    fn invocation_error(self) -> FeatureInvocationHandlerError {
        if self == Self::OutcomeUnknown {
            FeatureInvocationHandlerError::outcome_unknown(self.message())
        } else {
            FeatureInvocationHandlerError::failed(self.message())
        }
    }
}

/// Narrow application adapter seam, not a config compatibility Tool/API.
///
/// The production implementation captures an identity-bound WorkspaceClient and
/// uses the shared typed Backend attach operation. Both validation (including
/// T696 replay/resume) and attach must consult current authority. Attach rechecks
/// authorization atomically with ledger mutation, enforces explicit access and
/// alias identity. The execution key identifies the host receipt, not a grant;
/// Backend alias attach is idempotent under its session lock. Never blind retry.
#[async_trait]
pub trait WorkspaceConfigAttachmentBackend: Send + Sync {
    fn is_available(&self) -> bool;
    fn validate_access(&self, access: Option<ConfigAccess>) -> Result<(), ConfigAttachError>;
    async fn attach(
        &self,
        idempotency_key: &str,
        access: Option<ConfigAccess>,
    ) -> Result<ConfigAttachment, ConfigAttachError>;
}

/// Shared slash/self-attach handler. Possession of Workdir tools is not a grant.
#[derive(Clone)]
pub struct WorkspaceConfigFeature {
    client: Arc<dyn WorkspaceClient>,
    backend: Arc<dyn WorkspaceConfigAttachmentBackend>,
    content_backend: Option<Arc<backend::WorkspaceConfigBackend>>,
    wip_mode: bool,
    permits: Arc<tokio::sync::Semaphore>,
}

impl WorkspaceConfigFeature {
    /// Host supplies the actual mode; a user invocation cannot enable WIP or
    /// change the Workspace/security context captured by the adapter.
    pub fn new(
        client: Arc<dyn WorkspaceClient>,
        backend: Arc<dyn WorkspaceConfigAttachmentBackend>,
        wip_mode: bool,
    ) -> Self {
        Self {
            client,
            backend,
            content_backend: None,
            wip_mode,
            permits: Arc::new(tokio::sync::Semaphore::new(16)),
        }
    }

    /// Controller registration gate: a flag cannot create Backend authority or
    /// turn a Tools/standalone Worker into a Workspace WIP Worker.
    pub fn configured(
        client: Arc<dyn WorkspaceClient>,
        enabled: bool,
        wip_mode: bool,
    ) -> Option<Self> {
        (enabled
            && wip_mode
            && client.is_available()
            && client
                .workspace_id()
                .is_some_and(|id| !id.is_empty() && !id.chars().any(char::is_control)))
        .then(|| Self::for_workspace(client, true))
    }

    /// Production adapter captures the injected identity-bound Workspace client.
    pub fn for_workspace(client: Arc<dyn WorkspaceClient>, wip_mode: bool) -> Self {
        let backend = Arc::new(backend::WorkspaceConfigBackend::new(client.clone()));
        let mut feature = Self::new(client, backend.clone(), wip_mode);
        feature.content_backend = Some(backend);
        feature
    }

    pub fn is_available(&self) -> bool {
        self.wip_mode
            && self.client.is_available()
            && self.client.workspace_id().is_some_and(|id| !id.is_empty())
            && self.backend.is_available()
    }

    fn validate_access(&self, access: Option<ConfigAccess>) -> Result<(), ConfigAttachError> {
        if !self.is_available() {
            return Err(ConfigAttachError::Denied);
        }
        self.backend.validate_access(access)
    }

    /// The only attach execution path used by T696 and native WIP. No router
    /// session, command capability, or attachment lifetime is synthesized here.
    pub async fn attach(
        &self,
        idempotency_key: &str,
        access: Option<ConfigAccess>,
    ) -> Result<ConfigAttachment, ConfigAttachError> {
        if idempotency_key.is_empty() || idempotency_key.len() > 256 {
            return Err(ConfigAttachError::InvalidRequest);
        }
        self.validate_access(access)?;
        let _permit = tokio::time::timeout(DEADLINE, self.permits.acquire())
            .await
            .map_err(|_| ConfigAttachError::Unavailable)?
            .map_err(|_| ConfigAttachError::Unavailable)?;
        // Authority can change while queued. Backend still rechecks on execution.
        self.validate_access(access)?;
        let result = tokio::time::timeout(DEADLINE, self.backend.attach(idempotency_key, access))
            .await
            .map_err(|_| ConfigAttachError::OutcomeUnknown)??;
        // The adapter must map only a safe lifetime identity from a validated
        // Backend result. Invalid completion metadata cannot imply no effects.
        if result.connection_id.is_empty()
            || result.connection_id.len() > 256
            || !result
                .connection_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || access.is_some_and(|requested| requested != result.access)
        {
            return Err(ConfigAttachError::OutcomeUnknown);
        }
        Ok(result)
    }
}

pub fn workspace_config_invocation_descriptor() -> FeatureInvocationDescriptor {
    FeatureInvocationDescriptor {
        identity: FeatureInvocationIdentity(INVOCATION_ID.into()),
        name: FEATURE_ID.into(),
        aliases: Vec::new(),
        display_name: "Workspace config".into(),
        description: "Select this Workspace's logical config attachment before the request reaches the assistant. Backend grants determine access; no host filesystem or command access is added.".into(),
        syntax: FeatureInvocationSyntax::Parenthesized,
        arguments: vec![InvocationArgumentDescriptor {
            name: "access".into(),
            position: Some(0),
            required: false,
            value_type: InvocationArgumentType::Enum {
                values: vec!["effective".into(), "read_only".into(), "read_write".into()],
            },
            completion: InvocationCompletion::None,
            description: Some("Default: effective Backend grant. read_only restricts the connection; explicit read_write fails if unavailable.".into()),
        }],
        client_adapter: None,
    }
}

impl FeatureModule for WorkspaceConfigFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let descriptor = FeatureDescriptor::builtin(FEATURE_ID, "Workspace config")
            .with_description("WIP-only logical Workspace config attachment preparation");
        if self.wip_mode && self.client.workspace_id().is_some_and(|id| !id.is_empty()) {
            descriptor.with_chat_invocation(workspace_config_invocation_descriptor())
        } else {
            descriptor
        }
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        if self.wip_mode && self.client.workspace_id().is_some_and(|id| !id.is_empty()) {
            context
                .chat_invocations()
                .register(workspace_config_invocation_descriptor(), self.clone())?;
        }
        // No tools are declared/installed, including in non-WIP mode.
        Ok(())
    }
}

fn invocation_access(
    invocation: &FeatureInvocation,
) -> Result<Option<ConfigAccess>, ConfigAttachError> {
    if invocation.identity.0 != INVOCATION_ID
        || invocation.name != FEATURE_ID
        || invocation.arguments.len() > 1
    {
        return Err(ConfigAttachError::InvalidRequest);
    }
    match invocation.arguments.first() {
        None => requested_access(None),
        Some(argument) if argument.name == "access" => match &argument.value {
            InvocationValue::String(value) => requested_access(Some(value)),
            _ => Err(ConfigAttachError::InvalidRequest),
        },
        _ => Err(ConfigAttachError::InvalidRequest),
    }
}

#[async_trait]
impl FeatureInvocationHandler for WorkspaceConfigFeature {
    fn is_available(&self) -> bool {
        WorkspaceConfigFeature::is_available(self)
    }

    fn validate(
        &self,
        invocation: &FeatureInvocation,
    ) -> Result<(), FeatureInvocationHandlerError> {
        let access = invocation_access(invocation).map_err(ConfigAttachError::invocation_error)?;
        self.validate_access(access)
            .map_err(ConfigAttachError::invocation_error)
    }

    async fn invoke(
        &self,
        context: FeatureInvocationContext,
        invocation: &FeatureInvocation,
    ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
        if context.invocation_id != invocation.invocation_id {
            return Err(ConfigAttachError::InvalidRequest.invocation_error());
        }
        let access = invocation_access(invocation).map_err(ConfigAttachError::invocation_error)?;
        let result = self
            .attach(&context.invocation_id, access)
            .await
            .map_err(ConfigAttachError::invocation_error)?;
        let action = if result.already_attached {
            "Already attached"
        } else {
            "Attached"
        };
        Ok(FeatureInvocationResult {
            invocation_id: context.invocation_id,
            identity: invocation.identity.clone(),
            status: FeatureInvocationStatus::Succeeded,
            message: format!(
                "{action}: {ATTACHMENT_ALIAS} (Workspace configuration; {}).",
                result.access.as_str()
            ),
            context: Some(format!(
                "Workspace configuration is available as logical attachment `{ATTACHMENT_ALIAS}` at WIP `{CONTENT_ROOT}` with {} access. Use a shallow Tree only when paths are unknown. Inspect a needed Object to learn its Interface references, then inspect only unknown Interface contracts via their returned paths. Reuse known exact references/contracts and Invoke directly; do not repeat Inspect before each operation. Invoke handles observation freshness. Use published read/list Operations for configuration content, not Inspect. This attachment grants no OS paths or command execution. Backend validation, concurrency checks and activation rules apply; saving does not imply running Workers adopted new configuration.",
                result.access.as_str()
            )),
        })
    }
}

#[cfg(test)]
#[path = "workspace_config_tests.rs"]
mod tests;

#[cfg(test)]
mod production_tests;
