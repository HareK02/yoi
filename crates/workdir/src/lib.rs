//! Persistent Workdir identity and Worker-bound operation sessions.
//!
//! A [`Workdir`] identifies a materialized repository execution context across
//! Worker lifetimes. A [`WorkdirSession`] is the live operation attachment
//! bound to one Worker. Tools consume sessions; they do not own Workdir
//! materialization or cleanup.

mod delegation;
pub mod http;
mod local;
mod operation;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use delegation::{
    AppliedWorkdirDelegation, ReadOnlyWorkdirSession, WorkdirDelegation,
    WorkdirDelegationPermission, WorkdirDelegationRequest, WorkdirDelegationRule,
    apply_delegation_chain, delegation_capable_session,
};
pub use fs_operation::{
    ContentHash, EditRequest, EditResult, EntryKind, FsPath as WorkdirPath, GlobRequest,
    GlobResult, GrepOutputMode, GrepRequest, GrepResult, ListEntry, ListRequest, ListResult,
    ReadRequest, ReadResult, StatRequest, StatResult, WriteRequest, WriteResult,
};
pub use local::{
    LocalWorkdirSession, SymlinkInfo, WorkdirSessionResource, direct_symlink, first_symlink,
};
pub use operation::*;

/// Persistent, opaque identity of one materialized Workdir.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Workdir {
    id: WorkdirId,
}

impl Workdir {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WorkdirId(id.into()),
        }
    }

    pub fn id(&self) -> &WorkdirId {
        &self.id
    }
}

/// Opaque Workdir identifier assigned by the materialization authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkdirId(String);

impl WorkdirId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkdirId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirSessionCapability {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkdirSessionCapabilities {
    bits: u8,
}

impl WorkdirSessionCapabilities {
    const READ: u8 = 1 << 0;
    const WRITE: u8 = 1 << 1;
    const EDIT: u8 = 1 << 2;
    const GLOB: u8 = 1 << 3;
    const GREP: u8 = 1 << 4;
    const COMMAND: u8 = 1 << 5;

    pub const EMPTY: Self = Self { bits: 0 };

    pub fn from_capabilities(
        capabilities: impl IntoIterator<Item = WorkdirSessionCapability>,
    ) -> Self {
        capabilities
            .into_iter()
            .fold(Self::EMPTY, |set, capability| set.with(capability))
    }

    pub const fn with(mut self, capability: WorkdirSessionCapability) -> Self {
        self.bits |= match capability {
            WorkdirSessionCapability::Read => Self::READ,
            WorkdirSessionCapability::Write => Self::WRITE,
            WorkdirSessionCapability::Edit => Self::EDIT,
            WorkdirSessionCapability::Glob => Self::GLOB,
            WorkdirSessionCapability::Grep => Self::GREP,
            WorkdirSessionCapability::Command => Self::COMMAND,
        };
        self
    }

    pub const ALL: Self = Self {
        bits: Self::READ | Self::WRITE | Self::EDIT | Self::GLOB | Self::GREP | Self::COMMAND,
    };

    pub const READ_ONLY: Self = Self {
        bits: Self::READ | Self::GLOB | Self::GREP,
    };

    pub const fn supports(self, capability: WorkdirSessionCapability) -> bool {
        let bit = match capability {
            WorkdirSessionCapability::Read => Self::READ,
            WorkdirSessionCapability::Write => Self::WRITE,
            WorkdirSessionCapability::Edit => Self::EDIT,
            WorkdirSessionCapability::Glob => Self::GLOB,
            WorkdirSessionCapability::Grep => Self::GREP,
            WorkdirSessionCapability::Command => Self::COMMAND,
        };
        self.bits & bit != 0
    }
}

pub type WriteOutcome = WriteResult;

/// Live, Worker-bound operations for one persistent [`Workdir`].
///
/// Implementations execute filesystem search and command work on the host
/// that owns the materialization. Structured requests and results never
/// contain the raw materialized root. Closing a session is terminal and does
/// not delete the persistent Workdir or its materialization.
#[async_trait]
pub trait WorkdirSession: std::fmt::Debug + Send + Sync {
    fn workdir(&self) -> &Workdir;
    fn capabilities(&self) -> WorkdirSessionCapabilities;

    fn is_delegation_capable(&self) -> bool {
        false
    }

    /// Whether this session transports the delegation chain to another
    /// provider boundary that will apply logical cwd/path resolution there.
    fn transports_delegation_context(&self) -> bool {
        false
    }

    /// Capture a provider-specific source for a delegated child session.
    /// Remote providers use this boundary to pin attachment identity without
    /// exposing transport handles or host paths.
    async fn capture_delegation_source(
        &self,
        _request: &WorkdirDelegationRequest,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        Err(WorkdirError::Denied(
            "workdir provider does not support delegated sessions".into(),
        ))
    }

    /// Attenuate this session into a revocable child lease. Only sessions
    /// created with [`delegation_capable_session`] implement this operation.
    async fn delegate(
        &self,
        _request: WorkdirDelegationRequest,
    ) -> Result<WorkdirDelegation, WorkdirError> {
        Err(WorkdirError::Denied(
            "workdir session is not delegation-capable".into(),
        ))
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError>;
    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError>;
    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError>;
    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError>;
    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError>;
    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError>;
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError>;
    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError>;
    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError>;
    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError>;
    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError>;

    /// Subscribe to bounded provider-owned command telemetry. Implementations
    /// that do not expose live command observation may keep the default.
    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        None
    }

    /// Return the bounded current command state used to recover from a lagged
    /// provider subscription without replaying command output into history.
    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        Vec::new()
    }

    /// Terminal, idempotent release of this Worker-bound operation session.
    async fn close(&self) -> Result<(), WorkdirError>;
}

pub type WorkdirSessionHandle = Arc<dyn WorkdirSession>;

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error("Workdir operation denied: {0}")]
    Denied(String),

    #[error("Workdir session is closed")]
    SessionClosed,

    #[error("Workdir session does not support {0:?}")]
    Unsupported(WorkdirSessionCapability),

    #[error("Workdir operation is unsupported: {0}")]
    UnsupportedOperation(String),

    #[error("invalid Workdir path: {0}")]
    InvalidPath(String),

    #[error("Workdir session is unavailable: {0}")]
    Unavailable(String),

    #[error("Workdir transport failed: {0}")]
    Transport(String),

    #[error("Workdir content was modified externally before the operation could be applied: {0}")]
    Conflict(String),

    #[error("unknown Workdir session command: {0}")]
    UnknownCommand(String),

    #[error("path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),

    #[error("path is outside allowed scope: {}", .0.display())]
    OutOfScope(PathBuf),

    #[error(
        "path resolves through a symlink outside allowed {required_permission} scope: {} -> {}; add the symlink target to the Worker {required_permission} scope, copy it into the workspace, or recreate the symlink with the correct target",
        .path.display(),
        .target.display()
    )]
    SymlinkOutOfScope {
        path: PathBuf,
        target: PathBuf,
        required_permission: &'static str,
    },

    #[error(
        "broken symlink while resolving {}: {} -> {} (target does not exist); recreate the symlink with an absolute target or a correct relative target",
        .path.display(),
        .link.display(),
        .target.display()
    )]
    BrokenSymlink {
        path: PathBuf,
        link: PathBuf,
        target: PathBuf,
    },

    #[error(
        "path resolves through a symlink to a directory, but this tool requires a file: {} -> {}; choose a file inside that directory",
        .path.display(),
        .target.display()
    )]
    SymlinkTargetIsDirectory { path: PathBuf, target: PathBuf },

    #[error("path is read-only: {}", .0.display())]
    ReadOnly(PathBuf),

    #[error("expected file but path is a directory: {}", .0.display())]
    IsDirectory(PathBuf),

    #[error("file not found: {}", .0.display())]
    NotFound(PathBuf),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("invalid glob pattern: {0}")]
    InvalidGlob(String),

    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),

    #[error("{tool} does not follow symlink directories: {} -> {}", .path.display(), .target.display())]
    SymlinkDirectoryNotTraversed {
        tool: &'static str,
        path: PathBuf,
        target: PathBuf,
    },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl WorkdirError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl From<fs_operation::FsError> for WorkdirError {
    fn from(error: fs_operation::FsError) -> Self {
        match error {
            fs_operation::FsError::InvalidPath(message) => Self::InvalidPath(message),
            fs_operation::FsError::RelativePath(path) => Self::RelativePath(path),
            fs_operation::FsError::OutOfScope(path) => Self::OutOfScope(path),
            fs_operation::FsError::NotFound(path) => Self::NotFound(path),
            fs_operation::FsError::BrokenSymlink { path, link, target } => {
                Self::BrokenSymlink { path, link, target }
            }
            fs_operation::FsError::SymlinkOutOfScope {
                path,
                target,
                required_permission,
            } => Self::SymlinkOutOfScope {
                path,
                target,
                required_permission,
            },
            fs_operation::FsError::SymlinkDirectoryNotTraversed { tool, path, target } => {
                Self::SymlinkDirectoryNotTraversed { tool, path, target }
            }
            fs_operation::FsError::ReadOnly(path) => Self::ReadOnly(path),
            fs_operation::FsError::IsDirectory(path) => Self::IsDirectory(path),
            fs_operation::FsError::NotDirectory(path) => {
                Self::InvalidArgument(format!("path is not a directory: {}", path.display()))
            }
            fs_operation::FsError::SymlinkTargetIsDirectory { path, target } => {
                Self::SymlinkTargetIsDirectory { path, target }
            }
            fs_operation::FsError::Conflict(message) => Self::Conflict(message),
            fs_operation::FsError::InvalidGlob(message) => Self::InvalidGlob(message),
            fs_operation::FsError::InvalidRegex(message) => Self::InvalidRegex(message),
            fs_operation::FsError::InvalidArgument(message) => Self::InvalidArgument(message),
            fs_operation::FsError::Io { path, source } => Self::Io { path, source },
        }
    }
}
