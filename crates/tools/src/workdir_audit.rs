//! Workdir receipts decorate the current Tool call without replacing its result.
//!
//! Only the closed audit projection crosses this boundary. Received IDs, provider
//! text and audit-store internals do not. The primary content and attachments stay
//! intact even if the secondary report exceeds the shared encoding budget.

use std::sync::Arc;

use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use error_contract::{FailureFacts, Tool as ToolAudience, project_failure};
use serde::Serialize;
use workdir::{
    audit::failure::WorkdirAuditFailureDetails, failure::WorkdirFailureVisibility,
    http::WorkdirAuditFailure,
};

use crate::failure::{ToolFailureBoundary, failure_output};

/// Wrap a Workdir-using Tool at its registered execution boundary. Cancellation
/// still goes to the original provider; neither audit loss nor encoding retries
/// an operation. Native callers may instead use `workdir::audit::capture`.
pub fn with_receipts(tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
    Arc::new(AuditTool { tool })
}

struct AuditTool {
    tool: Arc<dyn Tool>,
}

#[async_trait]
impl Tool for AuditTool {
    async fn execute(
        &self,
        input: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let (result, receipts) = workdir::audit::capture(self.tool.execute(input, ctx)).await;
        finish(result, &receipts, error_contract::MAX_FAILURE_BYTES)
    }
    async fn cancel(&self, call_id: &str) -> Result<(), ToolError> {
        self.tool.cancel(call_id).await
    }
    async fn cancel_execution(&self, ctx: &ToolExecutionContext) -> Result<(), ToolError> {
        self.tool.cancel_execution(ctx).await
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorClass {
    InvalidArgument,
    ExecutionFailed,
    ExecutionFailure,
    Internal,
    StructuredConflict,
    Cancelled,
    Interrupted,
}

#[derive(Serialize)]
struct AuditResult<'a> {
    primary_error_class: Option<ErrorClass>,
    primary_conflict_code: Option<&'a str>,
    audit_failures: Vec<WorkdirAuditFailureDetails>,
}

fn finish(
    result: Result<ToolOutput, ToolError>,
    receipts: &[WorkdirAuditFailure],
    budget: usize,
) -> Result<ToolOutput, ToolError> {
    if receipts.is_empty() {
        return result;
    }
    // Preserve the primary Tool disposition independently of every receipt's
    // operation_outcome (a Bash Tool can contain several provider operations).
    let (mut output, class, code) = match result {
        Ok(output) => (output, None, None),
        Err(ToolError::ExecutionFailure(output)) => {
            (output, Some(ErrorClass::ExecutionFailure), None)
        }
        Err(ToolError::Cancelled(output)) => (output, Some(ErrorClass::Cancelled), None),
        Err(ToolError::Interrupted(output)) => (output, Some(ErrorClass::Interrupted), None),
        Err(error) => {
            let (class, code) = match &error {
                ToolError::InvalidArgument(_) => (ErrorClass::InvalidArgument, None),
                ToolError::ExecutionFailed(_) => (ErrorClass::ExecutionFailed, None),
                ToolError::Internal(_) => (ErrorClass::Internal, None),
                ToolError::StructuredConflict { code, .. } => {
                    (ErrorClass::StructuredConflict, Some(code.clone()))
                }
                ToolError::ExecutionFailure(_)
                | ToolError::Cancelled(_)
                | ToolError::Interrupted(_) => unreachable!("output-bearing errors handled above"),
            };
            (
                ToolOutput {
                    summary: error.to_string(),
                    content: None,
                    attachments: Vec::new(),
                },
                Some(class),
                code,
            )
        }
    };
    let facts = FailureFacts::new(WorkdirAuditFailureDetails::from(&receipts[0]));
    let projected = facts.and_then(|facts| {
        project_failure::<ToolAudience, _>(
            &facts,
            &WorkdirFailureVisibility {
                can_view_cause: true,
            },
        )
    });
    if let Ok(projected) = projected {
        let report = failure_output(
            &projected,
            ToolFailureBoundary::Observation,
            &AuditResult {
                primary_error_class: class,
                primary_conflict_code: code.as_deref(),
                audit_failures: receipts
                    .iter()
                    .map(WorkdirAuditFailureDetails::from)
                    .collect(),
            },
            budget,
        );
        output.summary.push_str(" Workdir audit warning: ");
        output.summary.push_str(&report.summary);
        if let Some(report) = report.content {
            let content = output.content.get_or_insert_with(String::new);
            content.push_str("\n\nWorkdir audit report:\n");
            content.push_str(&report);
        }
    } else {
        output.summary.push_str(" Workdir audit receipt could not be presented; preserve the original operation result and do not resend for audit loss.");
    }
    match class {
        None => Ok(output),
        Some(ErrorClass::Cancelled) => Err(ToolError::Cancelled(output)),
        Some(ErrorClass::Interrupted) => Err(ToolError::Interrupted(output)),
        Some(_) => Err(ToolError::ExecutionFailure(output)),
    }
}

#[cfg(test)]
mod tests;
