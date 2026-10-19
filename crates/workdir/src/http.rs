//! HTTP contracts and an optional client for remote Workdir sessions.
//!
//! Keeps filesystem/search/process operations provider-side and pairs typed operations with results.
//! The optional client lets Runtime share DTOs without the client stack. Lost mutation responses,
//! decode failures, or pairing mismatches after dispatch yield OutcomeUnknown; no automatic resend.
//!
//! Denial reasons travel as an optional closed enum; provider free text maps to safe vocabulary.
//! Missing reasons stay unknown. Diagnostic wrappers preserve original classifications such as
//! Conflict/OutcomeUnknown and do not infer meaning from unknown reason strings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest, CommandStatus, EditRequest,
    EditResult, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest, ListResult,
    ReadBytesRequest, ReadBytesResult, ReadRequest, ReadResult, StatRequest, StatResult,
    WorkdirError, WorkdirId, WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest,
    WorkdirSession, WorkdirSessionCapabilities, WriteRequest, WriteResult,
};

/// Opaque Runtime-owned identifier for one ephemeral Workdir session.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct WorkdirSessionId(String);

impl WorkdirSessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkdirError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkdirError::InvalidArgument(
                "Workdir session id must not be empty".to_string(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Open a fresh session for a persisted Workdir identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenWorkdirSessionRequest {
    /// Optional Runtime Worker whose persisted binding establishes workspace
    /// ownership of the Workdir. Runtime servers reject cross-workspace owners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_worker_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenWorkdirSessionResponse {
    pub session_id: WorkdirSessionId,
    pub workdir_id: WorkdirId,
    pub capabilities: WorkdirSessionCapabilities,
}

/// One provider-side Workdir operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkdirSessionOperation {
    AuthorizeScope(WorkdirScopeAuthorizationRequest),
    ScopeRulesOverlap(WorkdirScopeOverlapRequest),
    CheckoutSearch(crate::CheckoutSearchRequest),
    CheckoutObserve(crate::WorkdirPath),
    CheckoutExecute(crate::CheckoutRequest),
    Stat(StatRequest),
    Read(ReadRequest),
    ReadBytes(ReadBytesRequest),
    Write(WriteRequest),
    Edit(EditRequest),
    List(ListRequest),
    Glob(GlobRequest),
    Grep(GrepRequest),
    CommandStart(CommandRequest),
    CommandStatus(CommandHandle),
    CommandOutput(CommandOutputRequest),
    CommandCancel(CommandHandle),
}

/// Wire envelope for one provider operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkdirSessionOperationRequest {
    pub operation: WorkdirSessionOperation,
}

/// Typed result paired with [`WorkdirSessionOperation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkdirSessionOperationResult {
    AuthorizeScope,
    ScopeRulesOverlap { overlaps: bool },
    CheckoutSearch(crate::CheckoutSearchResult),
    CheckoutObserve(crate::CheckoutObservation),
    CheckoutExecute(crate::CheckoutResult),
    Stat(StatResult),
    Read(ReadResult),
    ReadBytes(ReadBytesResult),
    Write(WriteResult),
    Edit(EditResult),
    List(ListResult),
    Glob(GlobResult),
    Grep(GrepResult),
    CommandStart(CommandHandle),
    CommandStatus(CommandStatus),
    CommandOutput(CommandOutput),
    CommandCancel,
}

/// Dispatch one typed operation against any provider-neutral Workdir session.
///
/// Runtime HTTP handlers, outbound External providers, and Backend brokers can
/// share this operation/result pairing while keeping transport authorization,
/// generation fencing, cancellation timeouts, and session lookup at their own
/// boundaries.
pub async fn dispatch_workdir_session_operation(
    session: &dyn WorkdirSession,
    operation: WorkdirSessionOperation,
) -> Result<WorkdirSessionOperationResult, WorkdirError> {
    match operation {
        WorkdirSessionOperation::AuthorizeScope(request) => session
            .authorize_scope_path(request)
            .await
            .map(|()| WorkdirSessionOperationResult::AuthorizeScope),
        WorkdirSessionOperation::ScopeRulesOverlap(request) => session
            .scope_rules_overlap(request)
            .await
            .map(|overlaps| WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps }),
        WorkdirSessionOperation::CheckoutSearch(request) => session
            .checkout_search(request)
            .await
            .map(WorkdirSessionOperationResult::CheckoutSearch),
        WorkdirSessionOperation::CheckoutObserve(path) => session
            .checkout_observe(path)
            .await
            .map(WorkdirSessionOperationResult::CheckoutObserve),
        WorkdirSessionOperation::CheckoutExecute(request) => session
            .checkout_execute(request)
            .await
            .map(WorkdirSessionOperationResult::CheckoutExecute),
        WorkdirSessionOperation::Stat(request) => session
            .stat(request)
            .await
            .map(WorkdirSessionOperationResult::Stat),
        WorkdirSessionOperation::Read(request) => session
            .read(request)
            .await
            .map(WorkdirSessionOperationResult::Read),
        WorkdirSessionOperation::ReadBytes(request) => session
            .read_bytes(request)
            .await
            .map(WorkdirSessionOperationResult::ReadBytes),
        WorkdirSessionOperation::Write(request) => session
            .write(request)
            .await
            .map(WorkdirSessionOperationResult::Write),
        WorkdirSessionOperation::Edit(request) => session
            .edit(request)
            .await
            .map(WorkdirSessionOperationResult::Edit),
        WorkdirSessionOperation::List(request) => session
            .list(request)
            .await
            .map(WorkdirSessionOperationResult::List),
        WorkdirSessionOperation::Glob(request) => session
            .glob(request)
            .await
            .map(WorkdirSessionOperationResult::Glob),
        WorkdirSessionOperation::Grep(request) => session
            .grep(request)
            .await
            .map(WorkdirSessionOperationResult::Grep),
        WorkdirSessionOperation::CommandStart(request) => session
            .start_command(request)
            .await
            .map(WorkdirSessionOperationResult::CommandStart),
        WorkdirSessionOperation::CommandStatus(handle) => session
            .command_status(handle)
            .await
            .map(WorkdirSessionOperationResult::CommandStatus),
        WorkdirSessionOperation::CommandOutput(request) => session
            .command_output(request)
            .await
            .map(WorkdirSessionOperationResult::CommandOutput),
        WorkdirSessionOperation::CommandCancel(handle) => session
            .cancel_command(handle)
            .await
            .map(|()| WorkdirSessionOperationResult::CommandCancel),
    }
}

/// Stable, host-path-free error code crossing the Runtime boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirTransportErrorCode {
    NotFound,
    Conflict,
    OutcomeUnknown,
    Unsupported,
    InvalidRequest,
    Denied,
    OutOfScope,
    SymlinkOutOfScope,
    BrokenSymlink,
    SymlinkTargetIsDirectory,
    ReadOnly,
    IsDirectory,
    SymlinkDirectoryNotTraversed,
    UnknownCommand,
    Unavailable,
    Io,
    Transport,
    Internal,
}

impl WorkdirTransportErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Unsupported => "unsupported",
            Self::InvalidRequest => "invalid_request",
            Self::Denied => "denied",
            Self::OutOfScope => "out_of_scope",
            Self::SymlinkOutOfScope => "symlink_out_of_scope",
            Self::BrokenSymlink => "broken_symlink",
            Self::SymlinkTargetIsDirectory => "symlink_target_is_directory",
            Self::ReadOnly => "read_only",
            Self::IsDirectory => "is_directory",
            Self::SymlinkDirectoryNotTraversed => "symlink_directory_not_traversed",
            Self::UnknownCommand => "unknown_command",
            Self::Unavailable => "unavailable",
            Self::Io => "io",
            Self::Transport => "transport",
            Self::Internal => "internal",
        }
    }

    /// Shared public HTTP classification for Runtime and Workspace Workdir operation boundaries.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::NotFound | Self::UnknownCommand => 404,
            Self::Conflict => 409,
            Self::Denied | Self::OutOfScope | Self::SymlinkOutOfScope | Self::ReadOnly => 403,
            Self::Unsupported
            | Self::InvalidRequest
            | Self::BrokenSymlink
            | Self::SymlinkTargetIsDirectory
            | Self::IsDirectory
            | Self::SymlinkDirectoryNotTraversed => 400,
            Self::Unavailable => 503,
            Self::Io | Self::Internal | Self::OutcomeUnknown => 500,
            Self::Transport => 502,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkdirTransportError {
    pub code: WorkdirTransportErrorCode,
    pub message: String,
    /// Internal provider diagnostics. Public response boundaries may omit this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial_reason: Option<crate::WorkdirDenialReason>,
}

impl WorkdirTransportError {
    /// Convert a provider error without exposing materialization paths or raw I/O errors.
    pub fn from_workdir_error(error: &WorkdirError) -> Self {
        use WorkdirTransportErrorCode as Code;
        let (code, message) = match error {
            WorkdirError::DenialContext { reason, source } => {
                let mut transport = Self::from_workdir_error(source);
                transport.denial_reason = Some(*reason);
                return transport;
            }
            WorkdirError::OutcomeUnknown(_) => (
                Code::OutcomeUnknown,
                "Workdir operation may have effects; inspect before retrying",
            ),
            WorkdirError::NotFound(_) => (Code::NotFound, "Workdir path was not found"),
            WorkdirError::Conflict(_) => (
                Code::Conflict,
                "The target file's content or existence changed since it was last observed; read the file again before retrying",
            ),
            WorkdirError::Unsupported(capability) => {
                return Self {
                    code: Code::Unsupported,
                    message: format!("Workdir capability {capability:?} is not available"),
                    denial_reason: None,
                };
            }
            WorkdirError::UnsupportedOperation(_) => {
                (Code::Unsupported, "Workdir operation is not supported")
            }
            WorkdirError::UnknownCommand(_) => {
                (Code::UnknownCommand, "Workdir command was not found")
            }
            WorkdirError::Unavailable(_) | WorkdirError::SessionClosed => {
                (Code::Unavailable, "Workdir session is unavailable")
            }
            WorkdirError::Denied(_) => (Code::Denied, "Workdir operation was denied"),
            WorkdirError::OutOfScope(_) => (Code::OutOfScope, "Workdir path is out of scope"),
            WorkdirError::SymlinkOutOfScope { .. } => (
                Code::SymlinkOutOfScope,
                "Workdir symlink target is out of scope",
            ),
            WorkdirError::BrokenSymlink { .. } => {
                (Code::BrokenSymlink, "Workdir symlink target does not exist")
            }
            WorkdirError::SymlinkTargetIsDirectory { .. } => (
                Code::SymlinkTargetIsDirectory,
                "Workdir symlink target is a directory",
            ),
            WorkdirError::ReadOnly(_) => (Code::ReadOnly, "Workdir path is read-only"),
            WorkdirError::IsDirectory(_) => (Code::IsDirectory, "Workdir path is a directory"),
            WorkdirError::SymlinkDirectoryNotTraversed { .. } => (
                Code::SymlinkDirectoryNotTraversed,
                "Workdir symlink directory was not traversed",
            ),
            WorkdirError::Io { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => (Code::NotFound, "Workdir path was not found"),
                std::io::ErrorKind::PermissionDenied => {
                    (Code::Denied, "Workdir operation was denied")
                }
                std::io::ErrorKind::AlreadyExists => {
                    (Code::Conflict, "Workdir resource already exists")
                }
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                    (Code::InvalidRequest, "Workdir operation request is invalid")
                }
                std::io::ErrorKind::TimedOut => (Code::Unavailable, "Workdir operation timed out"),
                _ => (Code::Io, "Workdir I/O operation failed"),
            },
            WorkdirError::OperationFailed => (Code::Internal, "Workdir operation failed"),
            WorkdirError::Transport(_) => (Code::Transport, "Workdir transport failed"),
            WorkdirError::InvalidPath(_)
            | WorkdirError::RelativePath(_)
            | WorkdirError::InvalidGlob(_)
            | WorkdirError::InvalidRegex(_)
            | WorkdirError::InvalidArgument(_) => {
                (Code::InvalidRequest, "Workdir operation request is invalid")
            }
        };
        Self {
            code,
            message: message.to_string(),
            denial_reason: error.denial_reason(),
        }
    }

    pub fn into_workdir_error(self) -> WorkdirError {
        use WorkdirTransportErrorCode as Code;
        let error = match self.code {
            Code::NotFound => WorkdirError::NotFound("<remote>".into()),
            Code::Conflict => WorkdirError::Conflict(self.message),
            Code::OutcomeUnknown => WorkdirError::OutcomeUnknown(
                "Workdir operation may have effects; inspect before retrying".into(),
            ),
            Code::Unsupported => WorkdirError::UnsupportedOperation(self.message),
            Code::InvalidRequest => WorkdirError::InvalidArgument(self.message),
            Code::Denied => WorkdirError::Denied(crate::WorkdirDenial {
                reason: self.denial_reason,
                // Provider-authored text is not trusted diagnostic data.
                message: "Workdir operation was denied".into(),
            }),
            Code::OutOfScope => WorkdirError::OutOfScope("<remote>".into()),
            Code::SymlinkOutOfScope => WorkdirError::SymlinkOutOfScope {
                path: "<remote>".into(),
                target: "<remote-target>".into(),
                required_permission: "requested",
            },
            Code::BrokenSymlink => WorkdirError::BrokenSymlink {
                path: "<remote>".into(),
                link: "<remote-link>".into(),
                target: "<remote-target>".into(),
            },
            Code::SymlinkTargetIsDirectory => WorkdirError::SymlinkTargetIsDirectory {
                path: "<remote>".into(),
                target: "<remote-target>".into(),
            },
            Code::ReadOnly => WorkdirError::ReadOnly("<remote>".into()),
            Code::IsDirectory => WorkdirError::IsDirectory("<remote>".into()),
            Code::SymlinkDirectoryNotTraversed => WorkdirError::SymlinkDirectoryNotTraversed {
                tool: "remote operation",
                path: "<remote>".into(),
                target: "<remote-target>".into(),
            },
            Code::UnknownCommand => WorkdirError::UnknownCommand("<remote>".to_string()),
            Code::Unavailable => WorkdirError::Unavailable(self.message),
            Code::Io => WorkdirError::Io {
                path: "<remote>".into(),
                source: std::io::Error::other(self.message),
            },
            Code::Transport => WorkdirError::Transport(self.message),
            Code::Internal => WorkdirError::OperationFailed,
        };
        // Preserve any supplied reason independently of the public classification.
        // Without a reason, retain the existing classification's deliberate fallback.
        if let Some(reason) = self.denial_reason {
            if error.denial_reason() != Some(reason) {
                return WorkdirError::DenialContext {
                    reason,
                    source: Box::new(error),
                };
            }
        }
        error
    }
}

#[cfg(feature = "http-client")]
mod client {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;
    use reqwest::{Client, StatusCode, Url};

    use super::*;
    use crate::{Workdir, WorkdirSession};

    /// An owned, Send authorization future that can offload synchronous authority
    /// work without borrowing request data or the authorization provider.
    pub type WorkdirHttpAuthorizationFuture = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, WorkdirError>> + Send + 'static>,
    >;

    /// Provides a fresh bearer token asynchronously for each Runtime request.
    /// Backend implementations can mint short-lived capability tokens without
    /// making a Worker-bound session expire with the token used to open it.
    /// Blocking DB/signing work must be offloaded by the implementation.
    pub trait WorkdirHttpAuthorization: std::fmt::Debug + Send + Sync {
        fn bearer_token(
            &self,
            method: String,
            path_and_query: String,
            body: Vec<u8>,
        ) -> WorkdirHttpAuthorizationFuture;
    }

    struct FixedBearerToken(Arc<str>);

    impl std::fmt::Debug for FixedBearerToken {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("FixedBearerToken(<redacted>)")
        }
    }

    impl WorkdirHttpAuthorization for FixedBearerToken {
        fn bearer_token(
            &self,
            _method: String,
            _path_and_query: String,
            _body: Vec<u8>,
        ) -> WorkdirHttpAuthorizationFuture {
            let token = self.0.clone();
            Box::pin(async move { Ok(token.to_string()) })
        }
    }

    /// Authenticated HTTP implementation of [`WorkdirSession`].
    ///
    /// Clone and reuse one `reqwest::Client` per Runtime to preserve connection
    /// pooling and keep-alive across Worker-bound sessions.
    #[derive(Debug)]
    pub struct RemoteWorkdirSession {
        client: Client,
        base_url: Url,
        authorization: Arc<dyn WorkdirHttpAuthorization>,
        workdir: Workdir,
        session_id: WorkdirSessionId,
        capabilities: WorkdirSessionCapabilities,
        closed: AtomicBool,
    }

    impl RemoteWorkdirSession {
        pub async fn open(
            client: Client,
            base_url: Url,
            bearer_token: impl Into<Arc<str>>,
            workdir_id: WorkdirId,
            request: OpenWorkdirSessionRequest,
        ) -> Result<Self, WorkdirError> {
            Self::open_with_authorization(
                client,
                base_url,
                Arc::new(FixedBearerToken(bearer_token.into())),
                workdir_id,
                request,
            )
            .await
        }

        pub async fn open_with_authorization(
            client: Client,
            base_url: Url,
            authorization: Arc<dyn WorkdirHttpAuthorization>,
            workdir_id: WorkdirId,
            request: OpenWorkdirSessionRequest,
        ) -> Result<Self, WorkdirError> {
            let url = endpoint(
                &base_url,
                &["v1", "working-directories", workdir_id.as_str(), "sessions"],
            )?;
            let body = serde_json::to_vec(&request)
                .map_err(|error| WorkdirError::Unavailable(error.to_string()))?;
            let token = authorization
                .bearer_token("POST".to_string(), url.path().to_string(), body.clone())
                .await?;
            let response = client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(http_unavailable)?;
            let opened: OpenWorkdirSessionResponse = decode_response(response).await?;
            if opened.workdir_id.as_str() != workdir_id.as_str() {
                return Err(WorkdirError::Unavailable(
                    "Runtime opened a session for a different Workdir".to_string(),
                ));
            }
            Ok(Self {
                client,
                base_url,
                authorization,
                workdir: Workdir::new(opened.workdir_id.as_str()),
                session_id: opened.session_id,
                capabilities: opened.capabilities,
                closed: AtomicBool::new(false),
            })
        }

        pub fn session_id(&self) -> &WorkdirSessionId {
            &self.session_id
        }

        async fn operate(
            &self,
            operation: WorkdirSessionOperation,
        ) -> Result<WorkdirSessionOperationResult, WorkdirError> {
            if self.closed.load(Ordering::Acquire) {
                return Err(WorkdirError::Unavailable(
                    "Workdir session is closed".to_string(),
                ));
            }
            let url = endpoint(
                &self.base_url,
                &[
                    "v1",
                    "workdir-sessions",
                    self.session_id.as_str(),
                    "operations",
                ],
            )?;
            let operation = WorkdirSessionOperationRequest { operation };
            let body = serde_json::to_vec(&operation)
                .map_err(|error| WorkdirError::Unavailable(error.to_string()))?;
            let token = self
                .authorization
                .bearer_token("POST".to_string(), url.path().to_string(), body.clone())
                .await?;
            let response = self
                .client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(http_unavailable)?;
            decode_response(response).await
        }

        fn mismatch(expected: &str) -> WorkdirError {
            WorkdirError::Unavailable(format!(
                "Runtime returned a mismatched Workdir operation result; expected {expected}"
            ))
        }
    }

    #[async_trait]
    impl WorkdirSession for RemoteWorkdirSession {
        fn workdir(&self) -> &Workdir {
            &self.workdir
        }

        fn capabilities(&self) -> WorkdirSessionCapabilities {
            self.capabilities
        }

        async fn authorize_scope_path(
            &self,
            request: WorkdirScopeAuthorizationRequest,
        ) -> Result<(), WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::AuthorizeScope(request))
                .await?
            {
                WorkdirSessionOperationResult::AuthorizeScope => Ok(()),
                _ => Err(Self::mismatch("authorize_scope")),
            }
        }

        async fn scope_rules_overlap(
            &self,
            request: WorkdirScopeOverlapRequest,
        ) -> Result<bool, WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::ScopeRulesOverlap(request))
                .await?
            {
                WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps } => Ok(overlaps),
                _ => Err(Self::mismatch("scope_rules_overlap")),
            }
        }

        async fn checkout_search(
            &self,
            request: crate::CheckoutSearchRequest,
        ) -> Result<crate::CheckoutSearchResult, WorkdirError> {
            request.validate()?;
            let expected = request.operation.capability();
            match self
                .operate(WorkdirSessionOperation::CheckoutSearch(request))
                .await?
            {
                WorkdirSessionOperationResult::CheckoutSearch(result)
                    if matches!(
                        (&result, expected),
                        (
                            crate::CheckoutSearchResult::List(_),
                            crate::WorkdirSessionCapability::Read
                        ) | (
                            crate::CheckoutSearchResult::Glob(_),
                            crate::WorkdirSessionCapability::Glob
                        ) | (
                            crate::CheckoutSearchResult::Grep(_),
                            crate::WorkdirSessionCapability::Grep
                        )
                    ) =>
                {
                    crate::external::ExternalWorkdirOperationResult::try_from(
                        WorkdirSessionOperationResult::CheckoutSearch(result.clone()),
                    )
                    .map_err(|_| Self::mismatch("bounded checkout_search"))?;
                    Ok(result)
                }
                _ => Err(Self::mismatch("checkout_search")),
            }
        }

        async fn checkout_observe(
            &self,
            path: crate::WorkdirPath,
        ) -> Result<crate::CheckoutObservation, WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::CheckoutObserve(path.clone()))
                .await?
            {
                WorkdirSessionOperationResult::CheckoutObserve(mut result)
                    if result.path == path
                        && !result.validator.is_empty()
                        && result.validator.len() <= 256 =>
                {
                    result.capabilities = result.capabilities.intersection(self.capabilities());
                    Ok(result)
                }
                _ => Err(Self::mismatch("checkout_observe")),
            }
        }
        async fn checkout_execute(
            &self,
            request: crate::CheckoutRequest,
        ) -> Result<crate::CheckoutResult, WorkdirError> {
            let mutation = request.operation.capability() != crate::WorkdirSessionCapability::Read;
            let expected_path = request.target.clone();
            let expected_output = request.operation.capability();
            match self
                .operate(WorkdirSessionOperation::CheckoutExecute(request))
                .await
            {
                Ok(WorkdirSessionOperationResult::CheckoutExecute(mut result))
                    if result.observation.path == expected_path
                        && !result.observation.validator.is_empty()
                        && result.observation.validator.len() <= 256
                        && match (&result.output, expected_output) {
                            (
                                crate::CheckoutOutput::Read(read),
                                crate::WorkdirSessionCapability::Read,
                            ) => {
                                read.path == expected_path
                                    && read.bytes.len()
                                        <= crate::BoundedReadLimits::EXTERNAL_DEFAULT
                                            .max_response_bytes
                            }
                            (
                                crate::CheckoutOutput::Write(_),
                                crate::WorkdirSessionCapability::Write,
                            )
                            | (
                                crate::CheckoutOutput::Edit(_),
                                crate::WorkdirSessionCapability::Edit,
                            ) => true,
                            _ => false,
                        } =>
                {
                    result.observation.capabilities = result
                        .observation
                        .capabilities
                        .intersection(self.capabilities());
                    Ok(result)
                }
                Ok(_) if mutation => Err(WorkdirError::OutcomeUnknown(
                    "mismatched response after checkout mutation".into(),
                )),
                Ok(_) => Err(Self::mismatch("checkout_execute")),
                Err(error)
                    if mutation
                        && matches!(
                            error.classification_source(),
                            WorkdirError::Unavailable(_) | WorkdirError::Transport(_)
                        ) =>
                {
                    let outcome = WorkdirError::OutcomeUnknown(
                        "checkout transport failed; effects may have occurred".into(),
                    );
                    Err(match error.denial_reason() {
                        Some(reason) => WorkdirError::DenialContext {
                            reason,
                            source: Box::new(outcome),
                        },
                        None => outcome,
                    })
                }
                Err(error) => Err(error),
            }
        }
        async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::Stat(request)).await? {
                WorkdirSessionOperationResult::Stat(result) => Ok(result),
                _ => Err(Self::mismatch("stat")),
            }
        }

        async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::Read(request)).await? {
                WorkdirSessionOperationResult::Read(result) => Ok(result),
                _ => Err(Self::mismatch("read")),
            }
        }

        async fn read_bytes(
            &self,
            request: ReadBytesRequest,
        ) -> Result<ReadBytesResult, WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::ReadBytes(request))
                .await?
            {
                WorkdirSessionOperationResult::ReadBytes(result) => Ok(result),
                _ => Err(Self::mismatch("read_bytes")),
            }
        }

        async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::Write(request))
                .await?
            {
                WorkdirSessionOperationResult::Write(result) => Ok(result),
                _ => Err(Self::mismatch("write")),
            }
        }

        async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::Edit(request)).await? {
                WorkdirSessionOperationResult::Edit(result) => Ok(result),
                _ => Err(Self::mismatch("edit")),
            }
        }

        async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::List(request)).await? {
                WorkdirSessionOperationResult::List(result) => Ok(result),
                _ => Err(Self::mismatch("list")),
            }
        }

        async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::Glob(request)).await? {
                WorkdirSessionOperationResult::Glob(result) => Ok(result),
                _ => Err(Self::mismatch("glob")),
            }
        }

        async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
            match self.operate(WorkdirSessionOperation::Grep(request)).await? {
                WorkdirSessionOperationResult::Grep(result) => Ok(result),
                _ => Err(Self::mismatch("grep")),
            }
        }

        async fn start_command(
            &self,
            request: CommandRequest,
        ) -> Result<CommandHandle, WorkdirError> {
            request.validate()?;
            match self
                .operate(WorkdirSessionOperation::CommandStart(request))
                .await?
            {
                WorkdirSessionOperationResult::CommandStart(result) => Ok(result),
                _ => Err(Self::mismatch("command_start")),
            }
        }

        async fn command_status(
            &self,
            handle: CommandHandle,
        ) -> Result<CommandStatus, WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::CommandStatus(handle))
                .await?
            {
                WorkdirSessionOperationResult::CommandStatus(result) => Ok(result),
                _ => Err(Self::mismatch("command_status")),
            }
        }

        async fn command_output(
            &self,
            request: CommandOutputRequest,
        ) -> Result<CommandOutput, WorkdirError> {
            if !(1..=crate::MAX_COMMAND_READ_BYTES).contains(&request.limit) {
                return Err(WorkdirError::InvalidArgument(
                    "command byte limit is outside transport bounds".into(),
                ));
            }
            let wait = request.wait;
            match self
                .operate(WorkdirSessionOperation::CommandOutput(request.clone()))
                .await?
            {
                WorkdirSessionOperationResult::CommandOutput(result) => {
                    let size = result
                        .chunks
                        .iter()
                        .try_fold(0usize, |size, chunk| size.checked_add(chunk.bytes.len()));
                    if !result.transport_bounds_valid()
                        || size.is_none_or(|size| {
                            size > request.limit
                                || request.cursor.checked_add(size as u64)
                                    != Some(result.next_cursor)
                        })
                    {
                        return Err(Self::mismatch("bounded command byte page"));
                    }
                    if wait && result.chunks.is_empty() && result.terminal.is_none() {
                        // A waiting request must block at the provider. Never spin HTTP
                        // requests against an immediate empty response. A caller may retry
                        // this exact cursor with its own bounded transport retry policy.
                        return Err(Self::mismatch("waiting command byte page"));
                    }
                    return Ok(result);
                }
                _ => return Err(Self::mismatch("command_output")),
            }
        }

        async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
            match self
                .operate(WorkdirSessionOperation::CommandCancel(handle))
                .await?
            {
                WorkdirSessionOperationResult::CommandCancel => Ok(()),
                _ => Err(Self::mismatch("command_cancel")),
            }
        }

        async fn close(&self) -> Result<(), WorkdirError> {
            if self.closed.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            let url = endpoint(
                &self.base_url,
                &["v1", "workdir-sessions", self.session_id.as_str()],
            )?;
            let token = self
                .authorization
                .bearer_token("DELETE".to_string(), url.path().to_string(), Vec::new())
                .await?;
            let response = self
                .client
                .delete(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(http_unavailable)?;
            if response.status() == StatusCode::NO_CONTENT || response.status().is_success() {
                Ok(())
            } else {
                Err(decode_error(response).await)
            }
        }
    }

    fn endpoint(base_url: &Url, segments: &[&str]) -> Result<Url, WorkdirError> {
        let mut url = base_url.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| {
                WorkdirError::InvalidArgument(
                    "Runtime base URL cannot be used for path-based Workdir operations".to_string(),
                )
            })?;
            path.pop_if_empty();
            path.extend(segments.iter().copied());
        }
        Ok(url)
    }

    async fn decode_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T, WorkdirError> {
        if response.status().is_success() {
            let bytes = bounded_response_bytes(response).await?;
            serde_json::from_slice(&bytes)
                .map_err(|_| WorkdirError::Transport("Runtime Workdir response was invalid".into()))
        } else {
            Err(decode_error(response).await)
        }
    }

    async fn bounded_response_bytes(
        mut response: reqwest::Response,
    ) -> Result<Vec<u8>, WorkdirError> {
        const MAX_RESPONSE_BYTES: usize = crate::external::MAX_EXTERNAL_PROVIDER_FRAME_BYTES;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err(WorkdirError::Transport(
                "Runtime Workdir response exceeded transport bound".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_unavailable)? {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(WorkdirError::Transport(
                    "Runtime Workdir response exceeded transport bound".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn decode_error(response: reqwest::Response) -> WorkdirError {
        match bounded_response_bytes(response).await {
            Ok(bytes) => serde_json::from_slice::<WorkdirTransportError>(&bytes)
                .map(WorkdirTransportError::into_workdir_error)
                .unwrap_or_else(|_| {
                    WorkdirError::Transport("Runtime Workdir error response was invalid".into())
                }),
            Err(error) => error,
        }
    }

    fn http_unavailable(_error: reqwest::Error) -> WorkdirError {
        WorkdirError::Transport("Runtime Workdir HTTP request failed".to_string())
    }

    #[cfg(test)]
    mod authorization_tests {
        use super::*;
        use std::sync::Mutex;

        #[derive(Debug, Default)]
        struct RejectAuthorization {
            requests: Arc<Mutex<Vec<(String, String, Vec<u8>)>>>,
        }

        impl WorkdirHttpAuthorization for RejectAuthorization {
            fn bearer_token(
                &self,
                method: String,
                path_and_query: String,
                body: Vec<u8>,
            ) -> WorkdirHttpAuthorizationFuture {
                let requests = self.requests.clone();
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    let mut requests = requests.lock().unwrap();
                    requests.push((method, path_and_query, body));
                    Err(WorkdirError::Unavailable(format!(
                        "authorization rejected request {}",
                        requests.len()
                    )))
                })
            }
        }

        #[tokio::test]
        async fn open_operate_close_await_fresh_authorization_and_do_not_send_when_rejected() {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base_url: Url = format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap();
            let client = Client::builder().no_proxy().build().unwrap();
            let authorization = Arc::new(RejectAuthorization::default());
            let workdir = Workdir::new("w");
            let open_request = OpenWorkdirSessionRequest {
                owner_worker_id: Some("worker".to_string()),
            };
            let error = RemoteWorkdirSession::open_with_authorization(
                client.clone(),
                base_url.clone(),
                authorization.clone(),
                workdir.id().clone(),
                open_request.clone(),
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                WorkdirError::Unavailable(message) if message == "authorization rejected request 1"
            ));

            // Seed an already-open session to exercise the public operation and
            // close boundaries independently of the rejected open request.
            let remote = RemoteWorkdirSession {
                client,
                base_url,
                authorization: authorization.clone(),
                workdir,
                session_id: WorkdirSessionId::new("s").unwrap(),
                capabilities: WorkdirSessionCapabilities::EMPTY,
                closed: AtomicBool::new(false),
            };
            let stat = StatRequest {
                path: crate::WorkdirPath::new("file").unwrap(),
            };
            let error = remote.stat(stat.clone()).await.unwrap_err();
            assert!(matches!(
                error,
                WorkdirError::Unavailable(message) if message == "authorization rejected request 2"
            ));
            let error = remote.close().await.unwrap_err();
            assert!(matches!(
                error,
                WorkdirError::Unavailable(message) if message == "authorization rejected request 3"
            ));
            assert_eq!(
                *authorization.requests.lock().unwrap(),
                vec![
                    (
                        "POST".to_string(),
                        "/v1/working-directories/w/sessions".to_string(),
                        serde_json::to_vec(&open_request).unwrap(),
                    ),
                    (
                        "POST".to_string(),
                        "/v1/workdir-sessions/s/operations".to_string(),
                        serde_json::to_vec(&WorkdirSessionOperationRequest {
                            operation: WorkdirSessionOperation::Stat(stat),
                        })
                        .unwrap(),
                    ),
                    (
                        "DELETE".to_string(),
                        "/v1/workdir-sessions/s".to_string(),
                        Vec::new(),
                    ),
                ]
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    pub use self::RemoteWorkdirSession as ClientSession;
}

#[cfg(feature = "http-client")]
pub use client::{
    ClientSession as RemoteWorkdirSession, WorkdirHttpAuthorization, WorkdirHttpAuthorizationFuture,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_round_trip_keeps_public_classification() {
        for (code, expected_status, expected_error) in [
            (
                WorkdirTransportErrorCode::InvalidRequest,
                400,
                "invalid argument",
            ),
            (WorkdirTransportErrorCode::NotFound, 404, "file not found"),
            (
                WorkdirTransportErrorCode::UnknownCommand,
                404,
                "unknown Workdir session command",
            ),
            (
                WorkdirTransportErrorCode::Conflict,
                409,
                "The target file's content or existence changed since it was last observed",
            ),
            (WorkdirTransportErrorCode::Unsupported, 400, "unsupported"),
            (WorkdirTransportErrorCode::Denied, 403, "denied"),
            (
                WorkdirTransportErrorCode::OutOfScope,
                403,
                "outside allowed scope",
            ),
            (
                WorkdirTransportErrorCode::SymlinkOutOfScope,
                403,
                "outside allowed requested scope",
            ),
            (
                WorkdirTransportErrorCode::BrokenSymlink,
                400,
                "broken symlink",
            ),
            (
                WorkdirTransportErrorCode::SymlinkTargetIsDirectory,
                400,
                "symlink to a directory",
            ),
            (WorkdirTransportErrorCode::ReadOnly, 403, "read-only"),
            (WorkdirTransportErrorCode::IsDirectory, 400, "expected file"),
            (
                WorkdirTransportErrorCode::SymlinkDirectoryNotTraversed,
                400,
                "does not follow symlink directories",
            ),
            (WorkdirTransportErrorCode::Unavailable, 503, "unavailable"),
            (WorkdirTransportErrorCode::Io, 500, "I/O error"),
            (
                WorkdirTransportErrorCode::Transport,
                502,
                "transport failed",
            ),
            (WorkdirTransportErrorCode::Internal, 500, "operation failed"),
        ] {
            let transport = WorkdirTransportError {
                code,
                denial_reason: None,
                message: if code == WorkdirTransportErrorCode::Conflict {
                    "The target file's content or existence changed since it was last observed; read the file again before retrying"
                        .to_string()
                } else {
                    "safe provider message".to_string()
                },
            };
            assert_eq!(code.http_status(), expected_status);
            let workdir_error = transport.clone().into_workdir_error();
            assert!(workdir_error.to_string().contains(expected_error));
            assert_eq!(
                WorkdirTransportError::from_workdir_error(&workdir_error).code,
                code
            );
        }
    }

    #[test]
    fn local_validation_errors_share_invalid_request_classification() {
        for error in [
            WorkdirError::InvalidGlob("[".to_string()),
            WorkdirError::InvalidRegex("(".to_string()),
            WorkdirError::InvalidArgument("limit must be positive".to_string()),
        ] {
            let transport = WorkdirTransportError::from_workdir_error(&error);
            assert_eq!(transport.code, WorkdirTransportErrorCode::InvalidRequest);
            assert_eq!(transport.code.http_status(), 400);
            assert_eq!(transport.message, "Workdir operation request is invalid");
        }
    }

    #[test]
    fn transport_failure_remains_distinct_from_session_unavailable() {
        let transport = WorkdirTransportError::from_workdir_error(&WorkdirError::Transport(
            "Workspace API request timed out".to_string(),
        ));
        assert_eq!(transport.code, WorkdirTransportErrorCode::Transport);
        assert_eq!(transport.code.http_status(), 502);
        assert_eq!(transport.message, "Workdir transport failed");
        assert!(matches!(
            transport.into_workdir_error(),
            WorkdirError::Transport(_)
        ));
    }

    #[test]
    fn transport_error_does_not_expose_host_path() {
        let error = WorkdirError::Io {
            path: "/secret/runtime/root/file".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "host detail"),
        };
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(transport.code, WorkdirTransportErrorCode::Denied);
        assert!(!transport.message.contains("/secret"));
        assert!(!transport.message.contains("host detail"));
        assert!(matches!(
            transport.into_workdir_error(),
            WorkdirError::Denied(_)
        ));

        let error = WorkdirError::Io {
            path: "/secret/runtime/root/file".into(),
            source: std::io::Error::other("host detail"),
        };
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(transport.code, WorkdirTransportErrorCode::Io);
        assert!(!transport.message.contains("/secret"));
        assert!(!transport.message.contains("host detail"));
        assert!(matches!(
            transport.into_workdir_error(),
            WorkdirError::Io { .. }
        ));

        let error = WorkdirError::Conflict(
            "The target file's content or existence changed since it was last observed; read the file again before retrying: /secret/runtime/root/file"
                .to_string(),
        );
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(transport.code, WorkdirTransportErrorCode::Conflict);
        assert_eq!(
            transport.message,
            "The target file's content or existence changed since it was last observed; read the file again before retrying"
        );
        assert!(!transport.message.contains("/secret"));
        assert_eq!(
            transport.into_workdir_error().to_string(),
            "The target file's content or existence changed since it was last observed; read the file again before retrying"
        );
    }
}
