//! Shared provider dispatch for Tool and checkout file processing.
use agen::tool::ToolError;
use workdir::{
    CheckoutOperation, CheckoutOutput, CheckoutRequest, ContentHash, EditRequest, EditResult,
    ReadRequest, ReadResult, WorkdirPath, WorkdirSessionHandle, WriteRequest, WriteResult,
};

use crate::ToolsError;

pub(crate) struct FileTarget {
    pub session: WorkdirSessionHandle,
    pub path: WorkdirPath,
    pub validator: Option<Vec<u8>>,
}

impl FileTarget {
    async fn checked(
        &self,
        operation: CheckoutOperation,
    ) -> Result<(CheckoutOutput, Option<Vec<u8>>), ToolError> {
        let result = self
            .session
            .checkout_execute(CheckoutRequest {
                target: self.path.clone(),
                validator: self
                    .validator
                    .clone()
                    .expect("checked dispatch requires validator"),
                operation,
            })
            .await
            .map_err(checked_error)?;
        Ok((result.output, Some(result.observation.validator)))
    }

    pub async fn read(
        &self,
        offset: usize,
        limit: usize,
        max_bytes: usize,
    ) -> Result<(ReadResult, Option<Vec<u8>>), ToolError> {
        if self.validator.is_some() {
            let (output, validator) = self
                .checked(CheckoutOperation::Read {
                    offset,
                    limit,
                    max_bytes,
                })
                .await?;
            match output {
                CheckoutOutput::Read(result) => Ok((result, validator)),
                _ => Err(readonly_result_error("Read")),
            }
        } else {
            let result = self
                .session
                .read(ReadRequest {
                    path: self.path.clone(),
                    offset,
                    limit,
                    max_bytes,
                })
                .await
                .map_err(ToolsError::from)?;
            Ok((result, None))
        }
    }

    pub async fn edit(
        &self,
        old_string: String,
        new_string: String,
        replace_all: bool,
        expected_hash: ContentHash,
    ) -> Result<(EditResult, Option<Vec<u8>>), ToolError> {
        if self.validator.is_some() {
            let (output, validator) = self
                .checked(CheckoutOperation::Edit {
                    old_string,
                    new_string,
                    replace_all,
                    expected_hash,
                })
                .await?;
            match output {
                CheckoutOutput::Edit(result) => Ok((result, validator)),
                _ => Err(unknown_result("Edit")),
            }
        } else {
            let result = self
                .session
                .edit(EditRequest {
                    path: self.path.clone(),
                    old_string,
                    new_string,
                    replace_all,
                    expected_hash,
                })
                .await
                .map_err(ToolsError::from)?;
            Ok((result, None))
        }
    }

    pub async fn write(
        &self,
        content: Vec<u8>,
        expected_hash: Option<ContentHash>,
        create_path: Option<WorkdirPath>,
    ) -> Result<(WriteResult, Option<Vec<u8>>), ToolError> {
        if self.validator.is_some() {
            let operation = match create_path {
                Some(path) => CheckoutOperation::Create { path, content },
                None => CheckoutOperation::Write {
                    content,
                    expected_hash: expected_hash
                        .expect("existing checkout Write requires prior read"),
                },
            };
            let (output, validator) = self.checked(operation).await?;
            match output {
                CheckoutOutput::Write(result) => Ok((result, validator)),
                _ => Err(unknown_result("Write/Create")),
            }
        } else {
            let result = self
                .session
                .write(WriteRequest {
                    path: self.path.clone(),
                    content,
                    expected_hash,
                })
                .await
                .map_err(ToolsError::from)?;
            Ok((result, None))
        }
    }
}

pub(crate) fn checked_error(error: workdir::WorkdirError) -> ToolError {
    use workdir::WorkdirError;
    if let WorkdirError::DenialContext { source, .. } = error {
        // Diagnostic enrichment must not change the established tool classification.
        return checked_error(*source);
    }
    let code = match &error {
        WorkdirError::DenialContext { .. } => unreachable!("unwrapped above"),
        WorkdirError::Conflict(_) | WorkdirError::NotFound(_) => "checkout_stale",
        WorkdirError::Denied { .. }
        | WorkdirError::OutOfScope { .. }
        | WorkdirError::ReadOnly { .. }
        | WorkdirError::Unsupported { .. }
        | WorkdirError::UnsupportedOperation { .. }
        | WorkdirError::SymlinkOutOfScope { .. }
        | WorkdirError::SymlinkDirectoryNotTraversed { .. } => "checkout_denied",
        WorkdirError::InvalidArgument { .. }
        | WorkdirError::InvalidPath { .. }
        | WorkdirError::UnknownCommand { .. }
        | WorkdirError::RelativePath { .. }
        | WorkdirError::BrokenSymlink { .. }
        | WorkdirError::SymlinkTargetIsDirectory { .. }
        | WorkdirError::IsDirectory { .. }
        | WorkdirError::InvalidGlob { .. }
        | WorkdirError::InvalidRegex { .. } => "checkout_invalid",
        WorkdirError::Unavailable { .. } | WorkdirError::SessionClosed => "checkout_unavailable",
        // IO/transport failures may arrive after effects have begun. Do not infer
        // no-effects from their text or strip explicit OutcomeUnknown evidence.
        WorkdirError::Io { .. }
        | WorkdirError::Transport { .. }
        | WorkdirError::OutcomeUnknown { .. }
        | WorkdirError::OperationFailed => {
            return ToolsError::from(error).into();
        }
    };
    ToolError::StructuredConflict {
        code: code.into(),
        message: error.to_string(),
    }
}

pub(crate) fn readonly_result_error(operation: &str) -> ToolError {
    checked_error(workdir::WorkdirError::Unavailable(format!(
        "provider returned an invalid readonly checkout {operation} result"
    )))
}

fn unknown_result(operation: &str) -> ToolError {
    ToolError::ExecutionFailed(format!(
        "Outcome unknown: provider returned wrong checkout {operation} result after mutation; do not automatically retry"
    ))
}
