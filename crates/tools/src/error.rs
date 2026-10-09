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
            ToolsError::WorkdirSession(error) => {
                // Diagnostic context is transparent to the established normal-tool
                // classification. Preserve the original display, including unknown
                // outcome guidance, while classifying through nested wrappers.
                let mut classification = error;
                while let workdir::WorkdirError::DenialContext { source, .. } = classification {
                    classification = source.as_ref();
                }
                match classification {
                    workdir::WorkdirError::NotFound(_)
                    | workdir::WorkdirError::Io { .. }
                    | workdir::WorkdirError::Unavailable(_)
                    | workdir::WorkdirError::OperationFailed
                    | workdir::WorkdirError::Transport(_)
                    | workdir::WorkdirError::Conflict(_)
                    | workdir::WorkdirError::OutcomeUnknown(_) => {
                        ToolError::ExecutionFailed(err.to_string())
                    }
                    _ => ToolError::InvalidArgument(err.to_string()),
                }
            }
            ToolsError::FileSystem(_)
            | ToolsError::NotRead(_)
            | ToolsError::ExternallyModified(_)
            | ToolsError::StringNotFound { .. }
            | ToolsError::NotUnique { .. }
            | ToolsError::InvalidArgument(_) => ToolError::InvalidArgument(err.to_string()),
        }
    }
}

/// Pure text failures are pre-dispatch argument failures, never unknown outcomes.
pub(crate) fn text_error(error: fs_operation::text::TextError) -> ToolError {
    ToolError::InvalidArgument(error.to_string())
}

pub(crate) fn decode_file_input<T: serde::de::DeserializeOwned>(
    input: &str,
    name: &str,
) -> Result<T, ToolError> {
    fs_operation::text::decode_json(input).map_err(|error| decode_error(error, name))
}

pub(crate) fn decode_file_value<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    name: &str,
) -> Result<T, ToolError> {
    fs_operation::text::decode(value).map_err(|error| decode_error(error, name))
}

fn decode_error(error: fs_operation::text::TextError, name: &str) -> ToolError {
    match error {
        fs_operation::text::TextError::Decode(error) => {
            ToolError::InvalidArgument(format!("invalid {name} input: {error}"))
        }
        error => text_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workdir::http::{WorkdirTransportError, WorkdirTransportErrorCode};

    #[test]
    fn flattened_file_tool_arguments_keep_raw_json_duplicate_rejection() {
        let error = decode_file_input::<crate::edit::EditParams>(
            r#"{"file_path":"a.txt","old_string":"a","old_string":"b","new_string":"c"}"#,
            "Edit",
        )
        .unwrap_err();
        assert!(
            matches!(error, ToolError::InvalidArgument(message) if message.contains("duplicate field `old_string`"))
        );
    }

    #[test]
    fn checkout_outcome_unknown_remains_execution_failure() {
        let error = ToolError::from(ToolsError::WorkdirSession(
            workdir::WorkdirError::OutcomeUnknown(
                "mutation may have committed; do not retry automatically".into(),
            ),
        ));
        match error {
            ToolError::ExecutionFailed(message) => {
                assert!(message.contains("mutation may have committed"));
                assert!(message.contains("do not retry automatically"));
            }
            other => panic!("outcome unknown must not become invalid input: {other:?}"),
        }
    }

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
    fn transport_diagnostic_reasons_preserve_normal_tool_classification_and_display() {
        use WorkdirTransportErrorCode as Code;
        use workdir::{WorkdirDenialReason as Reason, WorkdirError};
        for (code, execution_failure) in [
            (Code::NotFound, true),
            (Code::Conflict, true),
            (Code::OutcomeUnknown, true),
            (Code::Unsupported, false),
            (Code::InvalidRequest, false),
            (Code::Denied, false),
            (Code::OutOfScope, false),
            (Code::SymlinkOutOfScope, false),
            (Code::BrokenSymlink, false),
            (Code::SymlinkTargetIsDirectory, false),
            (Code::ReadOnly, false),
            (Code::IsDirectory, false),
            (Code::SymlinkDirectoryNotTraversed, false),
            (Code::UnknownCommand, false),
            (Code::Unavailable, true),
            (Code::Io, true),
            (Code::Transport, true),
            (Code::Internal, true),
        ] {
            // Wire data may carry a reason independently of its public code.
            // Neither enrichment nor nested wrappers may change normal-tool behavior.
            let baseline_message = serde_json::from_value::<WorkdirTransportError>(
                serde_json::json!({"code": code, "message": "safe fixture message"}),
            )
            .unwrap()
            .into_workdir_error()
            .to_string();
            for depth in [0, 1, 2] {
                let wire = serde_json::json!({
                    "code": code,
                    "message": "safe fixture message",
                    "denial_reason": (depth > 0).then_some(Reason::OsPermissionDenied),
                });
                let mut error = serde_json::from_value::<WorkdirTransportError>(wire)
                    .unwrap()
                    .into_workdir_error();
                if depth == 2 {
                    error = WorkdirError::DenialContext {
                        reason: Reason::ReadOnlySession,
                        source: Box::new(error),
                    };
                }
                let expected_message = error.to_string();
                assert_eq!(
                    expected_message, baseline_message,
                    "{code:?}, depth {depth}"
                );
                let tool_error = ToolError::from(ToolsError::from(error));
                let (actual_execution_failure, actual_message) = match tool_error {
                    ToolError::ExecutionFailed(message) => (true, message),
                    ToolError::InvalidArgument(message) => (false, message),
                    other => panic!("unexpected classification for {code:?}: {other:?}"),
                };
                assert_eq!(
                    actual_execution_failure, execution_failure,
                    "{code:?}, depth {depth}"
                );
                assert_eq!(actual_message, expected_message, "{code:?}, depth {depth}");
                if code == Code::OutcomeUnknown {
                    assert!(actual_message.contains("inspect before retrying"));
                }
            }
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
