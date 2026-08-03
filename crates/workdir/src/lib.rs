//! Workdir authority and local materialization provider.
//!
//! A Workdir is the host-owned execution context bound to one Worker. Tools
//! consume this interface; they do not own Workdir identity, paths, scope, or
//! lifecycle.

mod local;
mod operation;
mod search;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use local::{LocalWorkdir, SymlinkInfo, direct_symlink, first_symlink};
pub use operation::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirCapability {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkdirCapabilities {
    bits: u8,
}

impl WorkdirCapabilities {
    const READ: u8 = 1 << 0;
    const WRITE: u8 = 1 << 1;
    const EDIT: u8 = 1 << 2;
    const GLOB: u8 = 1 << 3;
    const GREP: u8 = 1 << 4;
    const COMMAND: u8 = 1 << 5;

    pub const EMPTY: Self = Self { bits: 0 };

    pub fn from_capabilities(capabilities: impl IntoIterator<Item = WorkdirCapability>) -> Self {
        capabilities
            .into_iter()
            .fold(Self::EMPTY, |set, capability| set.with(capability))
    }

    pub const fn with(mut self, capability: WorkdirCapability) -> Self {
        self.bits |= match capability {
            WorkdirCapability::Read => Self::READ,
            WorkdirCapability::Write => Self::WRITE,
            WorkdirCapability::Edit => Self::EDIT,
            WorkdirCapability::Glob => Self::GLOB,
            WorkdirCapability::Grep => Self::GREP,
            WorkdirCapability::Command => Self::COMMAND,
        };
        self
    }

    pub const ALL: Self = Self {
        bits: Self::READ | Self::WRITE | Self::EDIT | Self::GLOB | Self::GREP | Self::COMMAND,
    };

    pub const READ_ONLY: Self = Self {
        bits: Self::READ | Self::GLOB | Self::GREP,
    };

    pub const fn supports(self, capability: WorkdirCapability) -> bool {
        let bit = match capability {
            WorkdirCapability::Read => Self::READ,
            WorkdirCapability::Write => Self::WRITE,
            WorkdirCapability::Edit => Self::EDIT,
            WorkdirCapability::Glob => Self::GLOB,
            WorkdirCapability::Grep => Self::GREP,
            WorkdirCapability::Command => Self::COMMAND,
        };
        self.bits & bit != 0
    }
}

pub type WriteOutcome = WriteResult;

/// Network-capable operations available on one bound Workdir.
///
/// Implementations execute filesystem search and command work on the host
/// that owns the materialization. Requests and results never contain the raw
/// materialized root.
#[async_trait]
pub trait Workdir: std::fmt::Debug + Send + Sync {
    fn binding_id(&self) -> Option<&str>;
    fn capabilities(&self) -> WorkdirCapabilities;

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
    async fn shutdown(&self) -> Result<(), WorkdirError>;
}

pub type WorkdirHandle = Arc<dyn Workdir>;

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error("Workdir does not support {0:?}")]
    Unsupported(WorkdirCapability),

    #[error("invalid Workdir path: {0}")]
    InvalidPath(String),

    #[error("Workdir provider is unavailable: {0}")]
    Unavailable(String),

    #[error("Workdir content was modified externally before the operation could be applied: {0}")]
    Conflict(String),

    #[error("unknown Workdir command: {0}")]
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
