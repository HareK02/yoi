//! Identity-bound typed Workspace transport. Never retries a dispatched request.
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use server_api::{
    WorkspaceConfigAccess, WorkspaceConfigApiError, WorkspaceConfigAttachRequest,
    WorkspaceConfigAttachment, WorkspaceConfigFailureClassification,
};
use wip_protocol::{ProtocolError, ProtocolErrorCode};

use super::{
    ATTACHMENT_ALIAS, CONTENT_ROOT, ConfigAccess, ConfigAttachError, ConfigAttachment, DEADLINE,
    WorkspaceConfigAttachmentBackend,
};
use crate::wip::WipOperationError;
use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
};

pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_NODES: usize = 256;
pub const MAX_DEPTH: u32 = 8;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct WorkspaceConfigBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceConfigBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    fn base(&self) -> Result<String, WipOperationError> {
        let workspace = self
            .client
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                failure(
                    ProtocolErrorCode::PermissionDenied,
                    "Workspace config authority unavailable",
                )
            })?;
        let encoded: String = workspace
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                    (byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        Ok(format!("/api/w/{encoded}/workers/self/workspace-config"))
    }

    fn request<T: DeserializeOwned>(
        &self,
        request: WorkspaceRequest,
        mutation: bool,
    ) -> Result<T, WipOperationError> {
        let response = self
            .client
            .execute_with_timeout(request, DEADLINE)
            .map_err(|error| {
                if mutation && !matches!(error, WorkspaceClientError::InvalidPath(_)) {
                    unknown()
                } else {
                    failure(
                        ProtocolErrorCode::Internal,
                        "Workspace config transport unavailable",
                    )
                }
            })?;
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(if mutation {
                unknown()
            } else {
                failure(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "Workspace config response exceeds limit",
                )
            });
        }
        if !response.is_success() {
            if let Ok(error) = serde_json::from_str::<WorkspaceConfigApiError>(&response.body) {
                if error.classification == WorkspaceConfigFailureClassification::Unknown {
                    return Err(unknown());
                }
                // Do not reflect arbitrary transport/provider text into model output.
                let code = match error.status {
                    401 | 403 | 404 => ProtocolErrorCode::PermissionDenied,
                    409 => ProtocolErrorCode::ValidatorMismatch,
                    413 | 429 => ProtocolErrorCode::ResourceLimitExceeded,
                    400 | 422 => ProtocolErrorCode::InvalidArguments,
                    _ => ProtocolErrorCode::Internal,
                };
                return Err(failure(
                    code,
                    "Workspace config request rejected without commit",
                ));
            }
            return Err(if mutation {
                unknown()
            } else {
                failure(
                    if matches!(response.status, 401 | 403 | 404) {
                        ProtocolErrorCode::PermissionDenied
                    } else {
                        ProtocolErrorCode::Internal
                    },
                    "Workspace config request denied or unavailable",
                )
            });
        }
        serde_json::from_str(&response.body).map_err(|_| {
            if mutation {
                unknown()
            } else {
                failure(
                    ProtocolErrorCode::Internal,
                    "Invalid Workspace config response",
                )
            }
        })
    }

    /// GET checks the current grant even when the Worker has not attached yet.
    pub fn current(&self) -> Result<Option<WorkspaceConfigAttachment>, WipOperationError> {
        let result: Option<WorkspaceConfigAttachment> =
            self.request(WorkspaceRequest::get(self.base()?), false)?;
        if let Some(attachment) = &result {
            self.validate_attachment(attachment)?;
        }
        Ok(result)
    }

    fn validate_attachment(
        &self,
        attachment: &WorkspaceConfigAttachment,
    ) -> Result<(), WipOperationError> {
        if Some(attachment.workspace_id.as_str()) != self.client.workspace_id()
            || attachment.alias != ATTACHMENT_ALIAS
            || attachment.content_path != CONTENT_ROOT
            || attachment.connection_id.is_empty()
            || attachment.connection_id.len() > 256
            || !attachment
                .connection_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(failure(
                ProtocolErrorCode::PermissionDenied,
                "Invalid Workspace config attachment identity",
            ));
        }
        Ok(())
    }

    pub async fn post<T: DeserializeOwned + Send + 'static, R: Serialize>(
        &self,
        suffix: &str,
        request: &R,
        mutation: bool,
    ) -> Result<T, WipOperationError> {
        let body = serde_json::to_string(request).map_err(|_| {
            failure(
                ProtocolErrorCode::InvalidArguments,
                "Invalid Workspace config request",
            )
        })?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config request exceeds limit",
            ));
        }
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("{}{suffix}", self.base()?),
            body,
        );
        let this = self.clone();
        tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || this.request(request, mutation)),
        )
        .await
        .map_err(|_| {
            if mutation {
                unknown()
            } else {
                failure(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "Workspace config request deadline exceeded",
                )
            }
        })?
        .map_err(|_| {
            if mutation {
                unknown()
            } else {
                failure(
                    ProtocolErrorCode::Internal,
                    "Workspace config transport unavailable",
                )
            }
        })?
    }

    pub async fn current_async(
        &self,
    ) -> Result<Option<WorkspaceConfigAttachment>, WipOperationError> {
        let this = self.clone();
        tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || this.current()),
        )
        .await
        .map_err(|_| {
            failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config request deadline exceeded",
            )
        })?
        .map_err(|_| {
            failure(
                ProtocolErrorCode::Internal,
                "Workspace config transport unavailable",
            )
        })?
    }
}

#[async_trait]
impl WorkspaceConfigAttachmentBackend for WorkspaceConfigBackend {
    fn is_available(&self) -> bool {
        self.client.is_available()
    }
    fn validate_access(&self, access: Option<ConfigAccess>) -> Result<(), ConfigAttachError> {
        let current = self.current().map_err(attach_error)?;
        if access == Some(ConfigAccess::ReadWrite)
            && current.is_some_and(|a| a.access == WorkspaceConfigAccess::ReadOnly)
        {
            return Err(ConfigAttachError::Denied);
        }
        Ok(())
    }
    async fn attach(
        &self,
        _execution_key: &str,
        access: Option<ConfigAccess>,
    ) -> Result<ConfigAttachment, ConfigAttachError> {
        // Alias attach is idempotent under the Backend session lock. The T696
        // fenced receipt owns replay; no new idempotency protocol or blind retry.
        let request = WorkspaceConfigAttachRequest {
            alias: Some(ATTACHMENT_ALIAS.into()),
            access: access.map(|a| match a {
                ConfigAccess::ReadOnly => WorkspaceConfigAccess::ReadOnly,
                ConfigAccess::ReadWrite => WorkspaceConfigAccess::ReadWrite,
            }),
        };
        let attachment: WorkspaceConfigAttachment =
            self.post("", &request, true).await.map_err(attach_error)?;
        self.validate_attachment(&attachment)
            .map_err(|_| ConfigAttachError::OutcomeUnknown)?;
        Ok(ConfigAttachment {
            connection_id: attachment.connection_id,
            access: match attachment.access {
                WorkspaceConfigAccess::ReadOnly => ConfigAccess::ReadOnly,
                WorkspaceConfigAccess::ReadWrite => ConfigAccess::ReadWrite,
            },
            already_attached: attachment.already_attached,
        })
    }
}

fn attach_error(error: WipOperationError) -> ConfigAttachError {
    match error {
        WipOperationError::OutcomeUnknown(_) => ConfigAttachError::OutcomeUnknown,
        WipOperationError::Protocol(error) => match error.code {
            ProtocolErrorCode::PermissionDenied => ConfigAttachError::Denied,
            ProtocolErrorCode::InvalidArguments => ConfigAttachError::InvalidRequest,
            _ => ConfigAttachError::Unavailable,
        },
        _ => ConfigAttachError::Unavailable,
    }
}

pub(super) fn failure(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}
pub(super) fn unknown() -> WipOperationError {
    WipOperationError::OutcomeUnknown("Workspace config outcome unknown; refresh before any subsequent operation; no automatic retry".into())
}
