//! HTTP transport contract and client for remote Workdir sessions.
//!
//! The protocol keeps filesystem/search/process operations provider-side: one
//! [`WorkdirSessionOperation`] is one bounded HTTP request. The HTTP client is
//! optional so Runtime servers can share these DTOs without depending on a
//! client stack.

use serde::{Deserialize, Serialize};

use crate::{
    CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest, CommandStatus, EditRequest,
    EditResult, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest, ListResult,
    ReadRequest, ReadResult, StatRequest, StatResult, WorkdirError, WorkdirId,
    WorkdirSessionCapabilities, WriteRequest, WriteResult,
};

/// Opaque Runtime-owned identifier for one ephemeral Workdir session.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenWorkdirSessionRequest {
    /// Optional Runtime Worker whose persisted binding establishes workspace
    /// ownership of the Workdir. Runtime servers reject cross-workspace owners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_worker_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenWorkdirSessionResponse {
    pub session_id: WorkdirSessionId,
    pub workdir_id: WorkdirId,
    pub capabilities: WorkdirSessionCapabilities,
}

/// One provider-side Workdir operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "request", rename_all = "snake_case")]
pub enum WorkdirSessionOperation {
    Stat(StatRequest),
    Read(ReadRequest),
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

/// Typed result paired with [`WorkdirSessionOperation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "result", rename_all = "snake_case")]
pub enum WorkdirSessionOperationResult {
    Stat(StatResult),
    Read(ReadResult),
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

/// Stable, host-path-free error code crossing the Runtime boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirTransportErrorCode {
    NotFound,
    Conflict,
    Unsupported,
    InvalidRequest,
    UnknownCommand,
    Unavailable,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkdirTransportError {
    pub code: WorkdirTransportErrorCode,
    pub message: String,
}

impl WorkdirTransportError {
    /// Convert a provider error without exposing materialization paths or raw I/O errors.
    pub fn from_workdir_error(error: &WorkdirError) -> Self {
        use WorkdirTransportErrorCode as Code;
        let (code, message) = match error {
            WorkdirError::NotFound(_) => (Code::NotFound, "Workdir path was not found"),
            WorkdirError::Conflict(_) => (Code::Conflict, "Workdir content changed"),
            WorkdirError::Unsupported(capability) => {
                return Self {
                    code: Code::Unsupported,
                    message: format!("Workdir capability {capability:?} is not available"),
                };
            }
            WorkdirError::UnknownCommand(_) => {
                (Code::UnknownCommand, "Workdir command was not found")
            }
            WorkdirError::Unavailable(_) => (Code::Unavailable, "Workdir session is unavailable"),
            WorkdirError::InvalidPath(_)
            | WorkdirError::RelativePath(_)
            | WorkdirError::InvalidGlob(_)
            | WorkdirError::InvalidRegex(_)
            | WorkdirError::InvalidArgument(_) => {
                (Code::InvalidRequest, "Workdir operation request is invalid")
            }
            WorkdirError::OutOfScope(_)
            | WorkdirError::SymlinkOutOfScope { .. }
            | WorkdirError::BrokenSymlink { .. }
            | WorkdirError::SymlinkTargetIsDirectory { .. }
            | WorkdirError::ReadOnly(_)
            | WorkdirError::IsDirectory(_)
            | WorkdirError::SymlinkDirectoryNotTraversed { .. }
            | WorkdirError::Io { .. } => (Code::Internal, "Workdir operation failed"),
        };
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub fn into_workdir_error(self) -> WorkdirError {
        use WorkdirTransportErrorCode as Code;
        match self.code {
            Code::NotFound => WorkdirError::NotFound("<remote>".into()),
            Code::Conflict => WorkdirError::Conflict(self.message),
            Code::Unsupported => WorkdirError::Unavailable(self.message),
            Code::UnknownCommand => WorkdirError::UnknownCommand("<remote>".to_string()),
            Code::InvalidRequest => WorkdirError::InvalidArgument(self.message),
            Code::Unavailable | Code::Internal => WorkdirError::Unavailable(self.message),
        }
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

    /// Provides a fresh bearer token for each Runtime request. Backend
    /// implementations can mint short-lived capability tokens without making a
    /// Worker-bound session expire with the token used to open it.
    pub trait WorkdirHttpAuthorization: std::fmt::Debug + Send + Sync {
        fn bearer_token(&self) -> Result<String, WorkdirError>;
    }

    struct FixedBearerToken(Arc<str>);

    impl std::fmt::Debug for FixedBearerToken {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("FixedBearerToken(<redacted>)")
        }
    }

    impl WorkdirHttpAuthorization for FixedBearerToken {
        fn bearer_token(&self) -> Result<String, WorkdirError> {
            Ok(self.0.to_string())
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
            let response = client
                .post(url)
                .bearer_auth(authorization.bearer_token()?)
                .json(&request)
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
            let response = self
                .client
                .post(url)
                .bearer_auth(self.authorization.bearer_token()?)
                .json(&operation)
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
            let wait = request.wait;
            loop {
                match self
                    .operate(WorkdirSessionOperation::CommandOutput(request.clone()))
                    .await?
                {
                    WorkdirSessionOperationResult::CommandOutput(result)
                        if wait && result.status == CommandStatus::Running => {}
                    WorkdirSessionOperationResult::CommandOutput(result) => return Ok(result),
                    _ => return Err(Self::mismatch("command_output")),
                }
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
            let response = self
                .client
                .delete(url)
                .bearer_auth(self.authorization.bearer_token()?)
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
            response.json().await.map_err(http_unavailable)
        } else {
            Err(decode_error(response).await)
        }
    }

    async fn decode_error(response: reqwest::Response) -> WorkdirError {
        response
            .json::<WorkdirTransportError>()
            .await
            .map(WorkdirTransportError::into_workdir_error)
            .unwrap_or_else(|error| {
                WorkdirError::Unavailable(format!("Runtime HTTP error: {error}"))
            })
    }

    fn http_unavailable(error: reqwest::Error) -> WorkdirError {
        WorkdirError::Unavailable(format!("Runtime Workdir HTTP request failed: {error}"))
    }

    pub use self::RemoteWorkdirSession as ClientSession;
}

#[cfg(feature = "http-client")]
pub use client::{ClientSession as RemoteWorkdirSession, WorkdirHttpAuthorization};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_does_not_expose_host_path() {
        let error = WorkdirError::Io {
            path: "/secret/runtime/root/file".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "host detail"),
        };
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(transport.code, WorkdirTransportErrorCode::Internal);
        assert!(!transport.message.contains("/secret"));
        assert!(!transport.message.contains("host detail"));
    }
}
