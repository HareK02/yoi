//! Registered Drive Tool failures retain domain control facts alongside the shared cause.
use agen::tool::ToolError;
use error_contract::{FailureFacts, Tool, project_failure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::failure::{ToolFailureBoundary, execution_failure};

use super::{
    backend::DriveError,
    failure::{DriveClientFailure, DriveClientFailureOperation, DriveClientFailureVisibility},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveToolErrorClass {
    InvalidArgument,
    StructuredConflict,
    ExecutionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DriveToolConflictCode {
    #[serde(rename = "drive_conflict")]
    DriveConflict,
}

/// Absence of a reported mutation outcome does not attest to non-execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriveToolMutationObservation {
    NotReported,
    OutcomeUnknown { request_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriveToolFailureResult {
    pub error_class: DriveToolErrorClass,
    pub code: Option<DriveToolConflictCode>,
    pub mutation: DriveToolMutationObservation,
}

/// This entrance is only for the registered Tool's current call, not a stored
/// cause or another caller's error. The closed operation/reason are the same facts
/// already public in the old Tool error. No protected resource identity, grant,
/// provider prose or historical visibility is disclosed. The sole identity in
/// `result` is this call's existing Host-generated unknown mutation request ID.
pub(super) fn tool_error(
    operation: DriveClientFailureOperation,
    error: DriveError,
    expected_request_id: &str,
) -> ToolError {
    if let DriveError::OutcomeUnknown(request_id) = &error {
        if request_id != expected_request_id
            || request_id.len() != 64
            || !request_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return ToolError::ExecutionFailed("Drive mutation outcome unknown; original request identity could not be safely displayed; do not resend the mutation.".into());
        }
    }
    let (error_class, code) = match &error {
        DriveError::Conflict => (
            DriveToolErrorClass::StructuredConflict,
            Some(DriveToolConflictCode::DriveConflict),
        ),
        DriveError::Invalid(_) | DriveError::Limit => (DriveToolErrorClass::InvalidArgument, None),
        _ => (DriveToolErrorClass::ExecutionFailed, None),
    };
    let mutation = match &error {
        DriveError::OutcomeUnknown(request_id) => DriveToolMutationObservation::OutcomeUnknown {
            request_id: request_id.clone(),
        },
        _ => DriveToolMutationObservation::NotReported,
    };
    let boundary = match &error {
        DriveError::Unavailable
        | DriveError::OutcomeUnknown(_)
        | DriveError::Authorization { denied: false, .. } => ToolFailureBoundary::Observation,
        _ => ToolFailureBoundary::Execution,
    };
    let result = DriveToolFailureResult {
        error_class,
        code,
        mutation,
    };
    let failure = DriveClientFailure::new(operation, error);
    let projected = FailureFacts::new(failure).and_then(|facts| {
        project_failure::<Tool, _>(
            &facts,
            &DriveClientFailureVisibility {
                can_view_cause: true,
            },
        )
    });
    let Ok(projected) = projected else {
        return ToolError::ExecutionFailed("Drive failure presentation unavailable; the operation outcome is not confirmed; do not resend a mutation.".into());
    };
    let mut error = execution_failure(
        &projected,
        boundary,
        &result,
        error_contract::MAX_FAILURE_BYTES,
    );
    // Tool content is prunable. Keep the original identity and non-resend guidance
    // in the normal retained summary, independently of the cause's explanation.
    if let (
        DriveToolMutationObservation::OutcomeUnknown { request_id },
        ToolError::ExecutionFailure(output),
    ) = (&result.mutation, &mut error)
    {
        output.summary.push_str(&format!(" Original request_id={request_id}; query the same request_status; do not resend the mutation."));
    }
    error
}
