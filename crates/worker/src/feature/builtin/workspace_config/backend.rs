//! Typed Workspace configuration transport bound to Worker identity.
//!
//! Uses `workers/self/workspace-config`, never the owner editor API. Current grants,
//! attachment identity and existing WIP outcomes remain authoritative. Typed causes
//! are captured at deciding branches, independently of the existing diagnostic sink.
//! Missing mutation responses do not prove non-commit; dispatched requests are never resent.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use server_api::authorization::PermissionResource;
use server_api::{
    ConfigTreeChange, WorkspaceConfigAccess, WorkspaceConfigApiError, WorkspaceConfigAttachRequest,
    WorkspaceConfigAttachment, WorkspaceConfigCommitRequest, WorkspaceConfigFailureClassification,
    WorkspaceConfigObserveRequest, WorkspaceConfigReadRequest,
};
use wip_protocol::{ProtocolError, ProtocolErrorCode};

use super::failure::{
    ConfigFailure, ConfigFailureOperation as Operation, ConfigFailureReason as Cause,
    ConfigFailureStage as FailureStage,
};
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

    fn authorize(
        &self,
        operation: Operation,
        path: &str,
        recursive: bool,
    ) -> Result<(), ConfigFailure> {
        let workspace_id = self
            .client
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                failed(
                    operation,
                    FailureStage::Request,
                    Cause::Identity,
                    ProtocolErrorCode::PermissionDenied,
                    "Workspace config authority unavailable",
                )
            })?;
        let resource = PermissionResource::WorkspaceConfig {
            workspace_id: workspace_id.into(),
            path: path.into(),
            recursive,
        };
        self.client.authorize_operation(operation.authorization(), &resource).map_err(|error| {
            let denied = error.is_authorization_denial();
            failed(operation, FailureStage::Request,
                if denied { Cause::AuthorizationDenied } else { Cause::AuthorizationFailed },
                if denied { ProtocolErrorCode::PermissionDenied } else { ProtocolErrorCode::Internal },
                &format!("Workspace config operation authorization failed before dispatch: {}; resource={resource:?}; denied={denied}", operation.authorization()))
        })
    }

    fn authorize_post(&self, suffix: &str, body: &str) -> Result<(), ConfigFailure> {
        let operation = Operation::from_suffix(suffix);
        let invalid = || {
            failed(
                operation,
                FailureStage::Request,
                Cause::InvalidRequest,
                ProtocolErrorCode::InvalidArguments,
                "Invalid Workspace config operation",
            )
        };
        match suffix {
            "" => {
                let _: WorkspaceConfigAttachRequest =
                    serde_json::from_str(body).map_err(|_| invalid())?;
                self.authorize(operation, "", false)
            }
            "/observe" => {
                let request: WorkspaceConfigObserveRequest =
                    serde_json::from_str(body).map_err(|_| invalid())?;
                if request.paths.is_empty() {
                    return Err(invalid());
                }
                for path in request.paths {
                    self.authorize(operation, &path, request.depth > 0)?;
                }
                Ok(())
            }
            "/read" => {
                let request: WorkspaceConfigReadRequest =
                    serde_json::from_str(body).map_err(|_| invalid())?;
                self.authorize(operation, &request.path, false)
            }
            "/commit" => {
                let request: WorkspaceConfigCommitRequest =
                    serde_json::from_str(body).map_err(|_| invalid())?;
                // The commit can also replace the whole-tree entrypoint list.
                self.authorize(operation, "", false)?;
                for change in request.request.changes {
                    match change {
                        ConfigTreeChange::Create { path, .. }
                        | ConfigTreeChange::Update { path, .. }
                        | ConfigTreeChange::Delete { path, .. } => {
                            self.authorize(operation, &path, false)?
                        }
                        ConfigTreeChange::Rename { from, to, .. } => {
                            self.authorize(operation, &from, false)?;
                            self.authorize(operation, &to, false)?;
                        }
                    }
                }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }

    fn base(&self, operation: Operation) -> Result<String, ConfigFailure> {
        let workspace = self
            .client
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                diagnostic("authority", false).record(Stage::Request, Reason::Identity, None);
                failed(
                    operation,
                    FailureStage::Request,
                    Cause::Identity,
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
        operation: Operation,
        diagnostic: &Diagnostic,
    ) -> Result<T, ConfigFailure> {
        let response = self
            .client
            .execute_with_timeout(request, self.deadline)
            .map_err(|error| {
                diagnostic.transport(&error);
                let cause = match &error {
                    WorkspaceClientError::Unavailable(_) => Cause::TransportUnavailable,
                    WorkspaceClientError::InvalidPath(_) => Cause::InvalidPath,
                    WorkspaceClientError::Request(_) => Cause::Communication,
                    WorkspaceClientError::AuthorizationRejected { .. } => Cause::BackendRejected,
                };
                if mutation && !matches!(error, WorkspaceClientError::InvalidPath(_)) {
                    uncertain(operation, FailureStage::Request, cause)
                } else {
                    failed(
                        operation,
                        FailureStage::Request,
                        cause,
                        ProtocolErrorCode::Internal,
                        "Workspace config transport unavailable",
                    )
                }
            })?;
        if response.body.len() > MAX_RESPONSE_BYTES {
            diagnostic.record(Stage::Response, Reason::ResponseSize, Some(response.status));
            return Err(if mutation {
                uncertain(operation, FailureStage::Response, Cause::ResponseSize)
            } else {
                failed(
                    operation,
                    FailureStage::Response,
                    Cause::ResponseSize,
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
                    return Err(uncertain(
                        operation,
                        FailureStage::Response,
                        Cause::BackendUnknown,
                    ));
                }
                diagnostic.record(
                    Stage::Response,
                    Reason::BackendRejected,
                    Some(response.status),
                );
                let code = match error.status {
                    401 | 403 | 404 => ProtocolErrorCode::PermissionDenied,
                    409 => ProtocolErrorCode::ValidatorMismatch,
                    413 | 429 => ProtocolErrorCode::ResourceLimitExceeded,
                    400 | 422 => ProtocolErrorCode::InvalidArguments,
                    _ => ProtocolErrorCode::Internal,
                };
                return Err(failed(
                    operation,
                    FailureStage::Response,
                    Cause::BackendRejected,
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
                uncertain(
                    operation,
                    FailureStage::Response,
                    Cause::UnrecognizedResponse,
                )
            } else {
                failed(
                    operation,
                    FailureStage::Response,
                    Cause::UnrecognizedResponse,
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
                uncertain(operation, FailureStage::Response, Cause::Decode)
            } else {
                failed(
                    operation,
                    FailureStage::Response,
                    Cause::Decode,
                    ProtocolErrorCode::Internal,
                    "Invalid Workspace config response",
                )
            }
        })
    }

    /// GET checks the current grant even when the Worker has not attached yet.
    pub fn current(&self) -> Result<Option<WorkspaceConfigAttachment>, ConfigFailure> {
        self.current_diagnosed(&diagnostic("current", false))
    }

    fn current_diagnosed(
        &self,
        diagnostic: &Diagnostic,
    ) -> Result<Option<WorkspaceConfigAttachment>, ConfigFailure> {
        self.authorize(Operation::Current, "", false)?;
        let result: Option<WorkspaceConfigAttachment> = self.request(
            WorkspaceRequest::get(self.base(Operation::Current)?),
            false,
            Operation::Current,
            diagnostic,
        )?;
        if let Some(attachment) = &result {
            self.validate_attachment(attachment, Operation::Current, diagnostic)?;
        }
        Ok(result)
    }

    fn validate_attachment(
        &self,
        attachment: &WorkspaceConfigAttachment,
        operation: Operation,
        diagnostic: &Diagnostic,
    ) -> Result<(), ConfigFailure> {
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
            return Err(failed(
                operation,
                FailureStage::Response,
                Cause::Identity,
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
    ) -> Result<T, ConfigFailure> {
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
    ) -> Result<T, ConfigFailure> {
        let operation = Operation::from_suffix(suffix);
        let body = serde_json::to_string(request).map_err(|_| {
            diagnostic.record(Stage::Request, Reason::Decode, None);
            failed(
                operation,
                FailureStage::Request,
                Cause::Decode,
                ProtocolErrorCode::InvalidArguments,
                "Invalid Workspace config request",
            )
        })?;
        if body.len() > MAX_RESPONSE_BYTES {
            diagnostic.record(Stage::Request, Reason::RequestSize, None);
            return Err(failed(
                operation,
                FailureStage::Request,
                Cause::RequestSize,
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config request exceeds limit",
            ));
        }
        self.authorize_post(suffix, &body)?;
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("{}{suffix}", self.base(operation)?),
            body,
        );
        let this = self.clone();
        let request_diagnostic = diagnostic.clone();
        tokio::time::timeout(
            self.deadline,
            tokio::task::spawn_blocking(move || {
                this.request(request, mutation, operation, &request_diagnostic)
            }),
        )
        .await
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::Deadline, None);
            if mutation {
                uncertain(operation, FailureStage::Completion, Cause::Deadline)
            } else {
                failed(
                    operation,
                    FailureStage::Completion,
                    Cause::Deadline,
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "Workspace config request deadline exceeded",
                )
            }
        })?
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::TaskJoin, None);
            if mutation {
                uncertain(operation, FailureStage::Completion, Cause::TaskJoin)
            } else {
                failed(
                    operation,
                    FailureStage::Completion,
                    Cause::TaskJoin,
                    ProtocolErrorCode::Internal,
                    "Workspace config transport unavailable",
                )
            }
        })?
    }

    pub async fn current_async(&self) -> Result<Option<WorkspaceConfigAttachment>, ConfigFailure> {
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
            failed(
                Operation::Current,
                FailureStage::Completion,
                Cause::Deadline,
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config request deadline exceeded",
            )
        })?
        .map_err(|_| {
            diagnostic.record(Stage::Completion, Reason::TaskJoin, None);
            failed(
                Operation::Current,
                FailureStage::Completion,
                Cause::TaskJoin,
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
        // Backend alias attach is idempotent under its session lock. The existing
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
        self.validate_attachment(&attachment, Operation::Attach, &diagnostic)
            .map_err(|error| {
                diagnostic.record(Stage::Attach, Reason::Identity, None);
                ConfigAttachError::Backend(Box::new(error.into_attach_completion_error()))
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

fn attach_error(error: ConfigFailure, diagnostic: &Diagnostic) -> ConfigAttachError {
    diagnostic.record(Stage::Attach, Reason::AttachConversion, None);
    ConfigAttachError::Backend(Box::new(error))
}

fn failed(
    operation: Operation,
    stage: FailureStage,
    cause: Cause,
    code: ProtocolErrorCode,
    message: &str,
) -> ConfigFailure {
    ConfigFailure::new(operation, stage, cause, failure(code, message))
}
fn uncertain(operation: Operation, stage: FailureStage, cause: Cause) -> ConfigFailure {
    ConfigFailure::new(operation, stage, cause, unknown())
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
