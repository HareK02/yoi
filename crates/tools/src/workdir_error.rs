//! Keep Workdir cause disclosure independent of the original Tool disposition.
//!
//! Native checkout must classify `source` before attaching `failure`; converting
//! to ToolError is only for the registered compatibility Tool boundary. No caller
//! recovers execution state by parsing a report or provider-authored explanation.

use agen::tool::{ToolError, ToolOutput};
use error_contract::{FailureFacts, Tool, project_failure};
use serde::Serialize;
use workdir::{
    WorkdirError,
    failure::{WorkdirFailure, WorkdirFailureOperation, WorkdirFailureVisibility},
};

use crate::failure::{ToolFailureBoundary, failure_output};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorClass {
    InvalidArgument,
    ExecutionFailed,
    StructuredConflict,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckoutCode {
    CheckoutStale,
    CheckoutDenied,
    CheckoutInvalid,
    CheckoutUnavailable,
    CheckoutNotDispatched,
}

impl CheckoutCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::CheckoutStale => "checkout_stale",
            Self::CheckoutDenied => "checkout_denied",
            Self::CheckoutInvalid => "checkout_invalid",
            Self::CheckoutUnavailable => "checkout_unavailable",
            Self::CheckoutNotDispatched => "checkout_not_dispatched",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum MutationObservation {
    NotReported,
    OutcomeUnknown,
}

#[derive(Serialize)]
struct WorkdirToolResult {
    error_class: ErrorClass,
    conflict_code: Option<CheckoutCode>,
    // Absence of an explicit outcome must not attest to nonexecution.
    mutation: MutationObservation,
}

/// Domain-specific carrier for native checkout and the ordinary Workdir Tools.
/// Only the optional report is publicly disclosed; source retains the original
/// classification with a locally generated safe message, never provider text.
#[derive(Debug)]
pub struct WorkdirToolError {
    source: ToolError,
    failure: Option<ToolOutput>,
}

impl WorkdirToolError {
    pub fn into_parts(self) -> (ToolError, Option<ToolOutput>) {
        (self.source, self.failure)
    }

    pub(crate) fn operation(operation: WorkdirFailureOperation, error: WorkdirError) -> Self {
        Self::project(operation, error, None)
    }

    pub(crate) fn checked(
        operation: WorkdirFailureOperation,
        error: WorkdirError,
        code: Option<CheckoutCode>,
    ) -> Self {
        Self::project(operation, error, code)
    }

    fn project(
        operation: WorkdirFailureOperation,
        error: WorkdirError,
        code: Option<CheckoutCode>,
    ) -> Self {
        let source = error.classification_source();
        let error_class = if code.is_some() {
            ErrorClass::StructuredConflict
        } else {
            // Preserve ToolsError's established classification independently of
            // denial/failure wrappers and the projected cause category.
            match source {
                WorkdirError::NotFound(_)
                | WorkdirError::Io { .. }
                | WorkdirError::Unavailable(_)
                | WorkdirError::OperationFailed
                | WorkdirError::Transport(_)
                | WorkdirError::Conflict(_)
                | WorkdirError::OutcomeUnknown(_) => ErrorClass::ExecutionFailed,
                _ => ErrorClass::InvalidArgument,
            }
        };
        let mutation = if matches!(source, WorkdirError::OutcomeUnknown(_)) {
            MutationObservation::OutcomeUnknown
        } else {
            MutationObservation::NotReported
        };
        let boundary = if matches!(
            source,
            WorkdirError::Transport(_) | WorkdirError::OutcomeUnknown(_)
        ) {
            ToolFailureBoundary::Observation
        } else {
            ToolFailureBoundary::Execution
        };
        let guidance = match source {
            WorkdirError::OutcomeUnknown(_) => {
                " Outcome unknown; do not automatically resend; inspect current state before deciding recovery."
            }
            WorkdirError::Transport(_) | WorkdirError::Io { .. } => {
                " Inspect current state before retrying; this failure does not prove nonexecution."
            }
            WorkdirError::Conflict(_) | WorkdirError::NotFound(_) if code.is_some() => {
                " Resolve the checkout again and reread before retrying."
            }
            WorkdirError::Conflict(_) => " Read the file again before retrying.",
            WorkdirError::BrokenSymlink { .. } => {
                " Inspect the symbolic link and repair it with the correct relative target before retrying."
            }
            WorkdirError::SymlinkOutOfScope { .. } => {
                " Check the symbolic link target against the authorized scope; scope changes require separate authorization."
            }
            _ => "",
        };
        // Retain the real source until the owning domain has extracted its facts.
        // Tool visibility allows only this domain's closed enums/static templates.
        let failure = FailureFacts::new(WorkdirFailure::new(operation, error))
            .and_then(|facts| {
                project_failure::<Tool, _>(
                    &facts,
                    &WorkdirFailureVisibility {
                        can_view_cause: true,
                    },
                )
            })
            .ok()
            .map(|projected| {
                let mut output = failure_output(
                    &projected,
                    boundary,
                    &WorkdirToolResult {
                        error_class,
                        conflict_code: code,
                        mutation,
                    },
                    error_contract::MAX_FAILURE_BYTES,
                );
                // Content may be pruned; recovery constraints must remain visible.
                output.summary.push_str(guidance);
                output
            });
        let message = failure
            .as_ref()
            .map(|output| output.summary.clone())
            .unwrap_or_else(|| format!("Workdir operation could not complete.{guidance}"));
        let source = match error_class {
            ErrorClass::InvalidArgument => ToolError::InvalidArgument(message),
            ErrorClass::ExecutionFailed => ToolError::ExecutionFailed(message),
            ErrorClass::StructuredConflict => ToolError::StructuredConflict {
                code: code
                    .expect("structured conflict requires a closed checkout code")
                    .as_str()
                    .into(),
                message,
            },
        };
        Self { source, failure }
    }
}

impl std::fmt::Display for WorkdirToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}

impl From<ToolError> for WorkdirToolError {
    fn from(source: ToolError) -> Self {
        Self {
            source,
            failure: None,
        }
    }
}

impl From<crate::ToolsError> for WorkdirToolError {
    fn from(source: crate::ToolsError) -> Self {
        // Tracker/policy errors are not Workdir operation failures.
        ToolError::from(source).into()
    }
}

#[cfg(test)]
pub(crate) fn assert_report(
    error: ToolError,
    class: &str,
    classification: &str,
) -> serde_json::Value {
    let ToolError::ExecutionFailure(output) = error else {
        panic!("expected shared Workdir report, got {error:?}")
    };
    let report: serde_json::Value = serde_json::from_str(output.content.as_ref().unwrap()).unwrap();
    assert_eq!(report["result"]["error_class"], class);
    assert_eq!(
        report["failure"]["details"]["classification"],
        classification
    );
    assert_eq!(report["failure"]["domain"], "workdir");
    report
}

impl From<WorkdirToolError> for ToolError {
    fn from(error: WorkdirToolError) -> Self {
        let (source, failure) = error.into_parts();
        match failure {
            Some(output) => ToolError::ExecutionFailure(output),
            None => source,
        }
    }
}
