//! Typed Workspace configuration transport bound to Worker identity.
//!
//! Uses `workers/self/workspace-config`, never bypassing it for the owner editor API. Responses are
//! checked for Workspace, alias, content root, and connection lifetime, distinguishing typed Backend
//! `NotCommitted` from `Unknown`. Authorization, CAS, and input rejections become safe fixed
//! diagnostics; raw provider/transport bodies never reach the model. Missing, oversized, or
//! undecodable mutation responses do not prove non-commit; dispatched requests are never resent.
//! This adapter does not reimplement Backend source-persistence transactions or activation.
use std::sync::Arc;
use std::time::Duration;

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
use crate::feature::builtin::backend_diagnostic::{Adapter, Diagnostic, Reason, Stage};
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
    deadline: Duration,
}

impl WorkspaceConfigBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self {
            client,
            deadline: DEADLINE,
        }
    }

    #[cfg(test)]
    pub(super) fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    fn base(&self) -> Result<String, WipOperationError> {
        let workspace = self
            .client
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                diagnostic("authority", false).record(Stage::Request, Reason::Identity, None);
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
        diagnostic: &Diagnostic,
    ) -> Result<T, WipOperationError> {
        let response = self
            .client
            .execute_with_timeout(request, self.deadline)
            .map_err(|error| {
                diagnostic.transport(&error);
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
            diagnostic.record(Stage::Response, Reason::ResponseSize, Some(response.status));
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
                    diagnostic.record(
                        Stage::Response,
                        Reason::BackendUnknown,
                        Some(response.status),
                    );
                    return Err(unknown());
                }
                diagnostic.record(
                    Stage::Response,
                    Reason::BackendRejected,
                    Some(response.status),
                );
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
            diagnostic.record(
                Stage::Response,
                Reason::UnrecognizedResponse,
                Some(response.status),
            );
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
            diagnostic.record(Stage::Response, Reason::Decode, Some(response.status));
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
        self.current_diagnosed(&diagnostic("current", false))
    }

    fn current_diagnosed(
        &self,
        diagnostic: &Diagnostic,
    ) -> Result<Option<WorkspaceConfigAttachment>, WipOperationError> {
        let result: Option<WorkspaceConfigAttachment> =
            self.request(WorkspaceRequest::get(self.base()?), false, diagnostic)?;
        if let Some(attachment) = &result {
            self.validate_attachment(attachment, diagnostic)?;
        }
        Ok(result)
    }

    fn validate_attachment(
        &self,
        attachment: &WorkspaceConfigAttachment,
        diagnostic: &Diagnostic,
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
            diagnostic.record(Stage::Response, Reason::Identity, None);
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
        let operation = match suffix {
            "/observe" => "observe",
            "/read" => "read",
            "/commit" => "commit",
            "" => "attach",
            _ => "post",
        };
        self.post_diagnosed(suffix, request, mutation, diagnostic(operation, mutation))
            .await
    }

    pub(super) async fn post_diagnosed<T: DeserializeOwned + Send + 'static, R: Serialize>(
        &self,
        suffix: &str,
        request: &R,
        mutation: bool,
        diagnostic: Diagnostic,
    ) -> Result<T, WipOperationError> {
        let body = serde_json::to_string(request).map_err(|_| {
            diagnostic.record(Stage::Request, Reason::Decode, None);
            failure(
                ProtocolErrorCode::InvalidArguments,
                "Invalid Workspace config request",
            )
        })?;
        if body.len() > MAX_RESPONSE_BYTES {
            diagnostic.record(Stage::Request, Reason::RequestSize, None);
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
        let request_diagnostic = diagnostic.clone();
        tokio::time::timeout(
            self.deadline,
            tokio::task::spawn_blocking(move || {
                this.request(request, mutation, &request_diagnostic)
            }),
        )
        .await
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::Deadline, None);
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
            diagnostic.record(Stage::Completion, Reason::TaskJoin, None);
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
        let diagnostic = diagnostic("current", false);
        let request_diagnostic = diagnostic.clone();
        tokio::time::timeout(
            self.deadline,
            tokio::task::spawn_blocking(move || this.current_diagnosed(&request_diagnostic)),
        )
        .await
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::Deadline, None);
            failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config request deadline exceeded",
            )
        })?
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::TaskJoin, None);
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
        let diagnostic = diagnostic("validate_access", false);
        let current = self
            .current_diagnosed(&diagnostic)
            .map_err(|error| attach_error(error, &diagnostic))?;
        if access == Some(ConfigAccess::ReadWrite)
            && current.is_some_and(|a| a.access == WorkspaceConfigAccess::ReadOnly)
        {
            diagnostic.record(Stage::Attach, Reason::AccessCeiling, None);
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
        let diagnostic = diagnostic("attach", true);
        let attachment: WorkspaceConfigAttachment = self
            .post_diagnosed("", &request, true, diagnostic.clone())
            .await
            .map_err(|error| attach_error(error, &diagnostic))?;
        self.validate_attachment(&attachment, &diagnostic)
            .map_err(|_| {
                diagnostic.record(Stage::Attach, Reason::Identity, None);
                ConfigAttachError::OutcomeUnknown
            })?;
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

fn attach_error(error: WipOperationError, diagnostic: &Diagnostic) -> ConfigAttachError {
    diagnostic.record(Stage::Attach, Reason::AttachConversion, None);
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

pub(super) fn diagnostic(operation: &'static str, mutation: bool) -> Diagnostic {
    Diagnostic::new(Adapter::WorkspaceConfig, operation, mutation, None)
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

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
