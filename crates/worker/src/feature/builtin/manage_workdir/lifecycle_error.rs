//! Cause disclosure never changes the lifecycle caller's original disposition.
//! Native WIP classifies the source before attaching the shared Tool report.
use agen::tool::{ToolError, ToolOutput};
use error_contract::{FailureFacts, Tool, project_failure};
use server_api::workdir_management_error::{
    WorkdirAttachmentRelatedFailure, WorkdirManagementCause,
};
use workdir::management_failure::WorkdirManagementVisibility;

#[derive(Debug)]
pub(super) struct LifecycleError {
    pub source: ToolError,
    pub report: Option<ToolOutput>,
}

impl LifecycleError {
    pub fn route(
        error: workdir::WorkdirError,
        backend_attachment: &'static str,
        rollback: Option<Self>,
    ) -> Self {
        let rollback_failure = rollback.and_then(|error| error.report);
        let report = FailureFacts::new(workdir::failure::WorkdirFailure::new(
            workdir::failure::WorkdirFailureOperation::AttachmentProjection,
            error,
        ))
        .and_then(|facts| {
            project_failure::<Tool, _>(
                &facts,
                &workdir::failure::WorkdirFailureVisibility {
                    can_view_cause: true,
                },
            )
        })
        .ok()
        .map(|projected| {
            let mut output = tools::failure::failure_output(
                &projected,
                tools::failure::ToolFailureBoundary::Execution,
                &serde_json::json!({
                    "backend_attachment": backend_attachment,
                    "rollback_failure": rollback_failure,
                }),
                error_contract::MAX_FAILURE_BYTES,
            );
            output.summary.push_str(" Inspect the current Backend attachment and local route separately; do not automatically repeat attachment mutations.");
            output
        });
        let source = ToolError::ExecutionFailed(
            report.as_ref().map(|report| report.summary.clone()).unwrap_or_else(|| {
                "Workdir attachment route could not be confirmed; inspect the current Backend attachment before deciding recovery".into()
            }),
        );
        Self { source, report }
    }

    pub fn route_cleanup(mut result: ToolOutput, error: workdir::WorkdirError) -> ToolOutput {
        let projected = FailureFacts::new(workdir::failure::WorkdirFailure::new(
            workdir::failure::WorkdirFailureOperation::SessionClose,
            error,
        ))
        .and_then(|facts| {
            project_failure::<Tool, _>(
                &facts,
                &workdir::failure::WorkdirFailureVisibility {
                    can_view_cause: true,
                },
            )
        })
        .ok();
        result.summary.push_str(" Backend detach succeeded; local session cleanup is unconfirmed. Do not repeat Backend detach.");
        if let Some(projected) = projected {
            result.summary.push(' ');
            result.summary.push_str(projected.explanation());
            if let Some(mut value) = result
                .content
                .as_deref()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
                && let Some(object) = value.as_object_mut()
                && let Ok(attachment) =
                    protocol::startup_failure::StartupFailureAttachment::from_projected(&projected)
            {
                object.insert("cleanup_failures".into(), serde_json::json!([attachment]));
                result.content = serde_json::to_string(&value).ok();
            }
        }
        result
    }

    pub fn received(
        source: ToolError,
        status: u16,
        cause: WorkdirManagementCause,
        related_failures: &[WorkdirAttachmentRelatedFailure],
        related_failures_unavailable: bool,
    ) -> Self {
        let (error_class, conflict_code) = match &source {
            ToolError::StructuredConflict { code, .. } => {
                ("structured_conflict", Some(code.clone()))
            }
            _ => ("execution_failed", None),
        };
        let report = FailureFacts::new(cause)
            .and_then(|facts| {
                project_failure::<Tool, _>(
                    &facts,
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
            })
            .ok()
            .map(|projected| {
                let related_failures: Vec<_> = related_failures
                    .iter()
                    .map(|related| WorkdirAttachmentRelatedFailure {
                        stage: related.stage,
                        http_status: related.http_status,
                        failure: related.failure.as_ref().and_then(|attachment| {
                            attachment
                                .project_for::<Tool, WorkdirManagementCause>(
                                    &WorkdirManagementVisibility {
                                        can_view_cause: true,
                                    },
                                )
                                .ok()
                                .and_then(|projected| {
                                    protocol::startup_failure::StartupFailureAttachment::from_projected(
                                        &projected,
                                    )
                                    .ok()
                                })
                        }),
                    })
                    .collect();
                let mut result = serde_json::json!({
                    "http_status": status,
                    "error_class": error_class,
                    "conflict_code": conflict_code,
                    "mutation": "not_reported",
                    "related_failures": related_failures,
                    "related_failures_unavailable": related_failures_unavailable,
                });
                let mut output = tools::failure::failure_output(
                    &projected,
                    tools::failure::ToolFailureBoundary::Observation,
                    &result,
                    error_contract::MAX_FAILURE_BYTES,
                );
                // Optional compensation observations must not displace the primary report.
                if output.content.is_none() && !related_failures.is_empty() {
                    result["related_failures"] = serde_json::json!([]);
                    result["related_failures_unavailable"] = serde_json::json!(true);
                    output = tools::failure::failure_output(
                        &projected,
                        tools::failure::ToolFailureBoundary::Observation,
                        &result,
                        error_contract::MAX_FAILURE_BYTES,
                    );
                }
                output.summary.push_str(" The HTTP failure does not establish the mutation outcome; inspect current state and do not resend automatically.");
                output
            });
        Self { source, report }
    }
}
impl From<ToolError> for LifecycleError {
    fn from(source: ToolError) -> Self {
        Self {
            source,
            report: None,
        }
    }
}
impl From<LifecycleError> for ToolError {
    fn from(error: LifecycleError) -> Self {
        error
            .report
            .map_or(error.source, ToolError::ExecutionFailure)
    }
}
impl std::fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}
