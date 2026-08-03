//! Error types for builtin tools.
//!
//! `ToolsError` keeps tool-specific policy failures separate from WorkdirSession
//! operation failures. Filesystem, search, and command errors originate in
//! `workdir` and remain transparent here.

use std::path::PathBuf;

use llm_engine::tool::ToolError;

#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    #[error(transparent)]
    FileSystem(#[from] fs_operation::FsError),

    #[error(transparent)]
    WorkdirSession(#[from] workdir::WorkdirError),

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
}

impl From<ToolsError> for ToolError {
    fn from(err: ToolsError) -> Self {
        match &err {
            ToolsError::WorkdirSession(
                workdir::WorkdirError::NotFound(_)
                | workdir::WorkdirError::Io { .. }
                | workdir::WorkdirError::Unavailable(_),
            ) => ToolError::ExecutionFailed(err.to_string()),
            ToolsError::FileSystem(_)
            | ToolsError::WorkdirSession(_)
            | ToolsError::NotRead(_)
            | ToolsError::ExternallyModified(_)
            | ToolsError::StringNotFound { .. }
            | ToolsError::NotUnique { .. }
            | ToolsError::InvalidArgument(_) => ToolError::InvalidArgument(err.to_string()),
        }
    }
}
