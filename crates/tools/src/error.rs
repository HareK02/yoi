//! Error types for builtin tools.
//!
//! `ToolsError` keeps tool-specific policy failures separate from WorkdirSession
//! operation failures. Filesystem, search, and command errors originate in
//! `workdir` and remain transparent here.

use std::path::PathBuf;

use agen::tool::ToolError;

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
                | workdir::WorkdirError::Unavailable(_)
                | workdir::WorkdirError::OperationFailed
                | workdir::WorkdirError::Transport(_)
                | workdir::WorkdirError::Conflict(_),
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

#[cfg(test)]
mod tests {
    use super::*;
    use workdir::http::{WorkdirTransportError, WorkdirTransportErrorCode};

    #[test]
    fn local_workdir_content_conflict_is_retryable_execution_failure() {
        let error = ToolError::from(ToolsError::WorkdirSession(
            fs_operation::FsError::Conflict("src/main.rs".to_string()).into(),
        ));

        match error {
            ToolError::ExecutionFailed(message) => assert_eq!(
                message,
                "The target file's content or existence changed since it was last observed; read the file again before retrying: src/main.rs"
            ),
            other => panic!("expected execution failure, got {other:?}"),
        }
    }

    #[test]
    fn remote_workdir_content_conflict_is_retryable_without_host_path() {
        let transport = WorkdirTransportError::from_workdir_error(
            &workdir::WorkdirError::Conflict("/runtime/private/checkout/src/main.rs".to_string()),
        );
        assert_eq!(transport.code, WorkdirTransportErrorCode::Conflict);
        assert_eq!(
            transport.message,
            "The target file's content or existence changed since it was last observed; read the file again before retrying"
        );
        let error = ToolError::from(ToolsError::WorkdirSession(transport.into_workdir_error()));

        match error {
            ToolError::ExecutionFailed(message) => {
                assert_eq!(
                    message,
                    "The target file's content or existence changed since it was last observed; read the file again before retrying"
                );
                assert!(!message.contains("/runtime/private"));
            }
            other => panic!("expected execution failure, got {other:?}"),
        }
    }
}
