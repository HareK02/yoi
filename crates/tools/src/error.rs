//! Error type shared across the `tools` crate.
//!
//! `ToolsError` is the crate-level error returned by `ScopedFs` and each
//! builtin tool's internal logic. Tool `execute()` impls convert it to
//! [`llm_engine::tool::ToolError`] via the `From` impl defined here.

use std::path::PathBuf;

use llm_engine::tool::ToolError;

#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    #[error("path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),

    #[error("path is outside allowed scope: {}", .0.display())]
    OutOfScope(PathBuf),

    #[error(
        "path resolves through a symlink outside allowed {required_permission} scope: {} -> {}; add the symlink target to the Pod {required_permission} scope, copy it into the workspace, or recreate the symlink with the correct target",
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
        "path resolves through a symlink to a directory, not a file: {} -> {}",
        .path.display(),
        .target.display()
    )]
    SymlinkTargetIsDirectory { path: PathBuf, target: PathBuf },

    #[error(
        "{tool} does not follow symlink directories: {} -> {}; use the resolved target path directly, or add the target to read scope and reference it without the symlink",
        .path.display(),
        .target.display()
    )]
    SymlinkDirectoryNotTraversed {
        tool: &'static str,
        path: PathBuf,
        target: PathBuf,
    },

    #[error("path is read-only in this scope: {}", .0.display())]
    ReadOnly(PathBuf),

    #[error("path is a directory: {}", .0.display())]
    IsDirectory(PathBuf),

    #[error("file not found: {}", .0.display())]
    NotFound(PathBuf),

    #[error("file has not been read in this session; read it first: {}", .0.display())]
    NotRead(PathBuf),

    #[error("file was modified externally after last read: {}", .0.display())]
    ExternallyModified(PathBuf),

    #[error("string not found in file: {}", .path.display())]
    StringNotFound { path: PathBuf },

    #[error(
        "string is not unique in file ({count} occurrences); pass replace_all=true or disambiguate: {}",
        .path.display()
    )]
    NotUnique { path: PathBuf, count: usize },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("invalid regex: {0}")]
    InvalidRegex(String),

    #[error("invalid glob pattern: {0}")]
    InvalidGlob(String),

    #[error("I/O error at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ToolsError {
    /// Helper to wrap an [`std::io::Error`] with the path it occurred on.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<ToolsError> for ToolError {
    fn from(err: ToolsError) -> Self {
        use ToolsError::*;
        match err {
            RelativePath(_)
            | OutOfScope(_)
            | SymlinkOutOfScope { .. }
            | BrokenSymlink { .. }
            | SymlinkTargetIsDirectory { .. }
            | SymlinkDirectoryNotTraversed { .. }
            | ReadOnly(_)
            | IsDirectory(_)
            | NotRead(_)
            | ExternallyModified(_)
            | StringNotFound { .. }
            | NotUnique { .. }
            | InvalidArgument(_)
            | InvalidRegex(_)
            | InvalidGlob(_) => ToolError::InvalidArgument(err.to_string()),
            NotFound(_) => ToolError::ExecutionFailed(err.to_string()),
            Io { .. } => ToolError::ExecutionFailed(err.to_string()),
        }
    }
}
