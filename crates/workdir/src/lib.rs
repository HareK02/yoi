//! Persistent Workdir identity and Worker-bound operation sessions.
//!
//! A [`Workdir`] identifies a materialized repository execution context across
//! Worker lifetimes. A [`WorkdirSession`] is the live operation attachment
//! bound to one Worker. Tools consume sessions; they do not own Workdir
//! materialization or cleanup.

pub mod http;
mod local;
mod operation;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use fs_operation::{
    ContentHash, EditRequest, EditResult, EntryKind, FsPath as WorkdirPath, GlobRequest,
    GlobResult, GrepOutputMode, GrepRequest, GrepResult, ListEntry, ListRequest, ListResult,
    ReadRequest, ReadResult, StatRequest, StatResult, WriteRequest, WriteResult,
};
pub use local::{LocalWorkdirSession, SymlinkInfo, direct_symlink, first_symlink};
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
    /// Terminal, idempotent release of this Worker-bound operation session.
    async fn close(&self) -> Result<(), WorkdirError>;
}

pub type WorkdirSessionHandle = Arc<dyn WorkdirSession>;

/// Ephemeral least-authority view over an existing Workdir session.
///
/// The wrapper exposes only stat/read/list/glob/grep and never forwards write,
/// edit, command, or close authority to the underlying Worker session. Closing
/// the wrapper is terminal for the view but deliberately leaves the owner's
/// source session open.
#[derive(Debug)]
pub struct ReadOnlyWorkdirSession {
    source: WorkdirSessionHandle,
    closed: AtomicBool,
}

impl ReadOnlyWorkdirSession {
    pub fn new(source: WorkdirSessionHandle) -> Self {
        Self {
            source,
            closed: AtomicBool::new(false),
        }
    }

    fn ensure_open(&self) -> Result<(), WorkdirError> {
        if self.closed.load(Ordering::Acquire) {
            Err(WorkdirError::Unavailable(
                "read-only Workdir session is closed".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl WorkdirSession for ReadOnlyWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.source.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        WorkdirSessionCapabilities::READ_ONLY
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.ensure_open()?;
        self.source.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.ensure_open()?;
        self.source.read(request).await
    }

    async fn write(&self, _request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Write))
    }

    async fn edit(&self, _request: EditRequest) -> Result<EditResult, WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Edit))
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.ensure_open()?;
        self.source.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.ensure_open()?;
        self.source.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.ensure_open()?;
        self.source.grep(request).await
    }

    async fn start_command(&self, _request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Command))
    }

    async fn command_status(&self, _handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Command))
    }

    async fn command_output(
        &self,
        _request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Command))
    }

    async fn cancel_command(&self, _handle: CommandHandle) -> Result<(), WorkdirError> {
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Command))
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error("Workdir session does not support {0:?}")]
    Unsupported(WorkdirSessionCapability),

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
