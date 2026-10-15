//! Host-bound read-only notification evidence, shared by normal Tools and WIP.
#[cfg(test)]
mod tests;
use super::*;
use crate::worker::WorkspaceServerOperation;

pub(super) const NAME: &str = "NotificationDiagnostics";
pub(super) const DESCRIPTION: &str = "Read bounded, paginated saved notification routing and delivery evidence within the current Workspace. Filter by Ticket, MR, Job, source event, assignment, delivery or exact source identity; continue with next_cursor and unchanged filters (limit 1..100). Distinguishes awaiting routing, suppression, deferred delivery, rejection, unknown acceptance and Runtime acceptance. Acceptance does not prove model processing or successful work. Historical evidence and retry_candidate are not execution authority. This read never routes, sends, assigns, starts/restores Workers or changes case/handling; no notification bodies or binding credentials are exposed.";

struct DiagnosticsTool {
    client: Arc<dyn WorkspaceClient>,
}
#[async_trait]
impl Tool for DiagnosticsTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        self.client
            .workspace_id()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArgument(
                    "Notification diagnostics require a Backend Workspace; no local fallback"
                        .into(),
                )
            })?;
        let query: server_api::NotificationDiagnosticsQuery =
            serde_json::from_str(input).map_err(|e| ToolError::InvalidArgument(e.to_string()))?;
        if !(1..=100).contains(&query.limit.unwrap_or(50)) {
            return Err(ToolError::InvalidArgument(
                "limit must be between 1 and 100".into(),
            ));
        }
        for value in [
            &query.ticket,
            &query.merge_request_id,
            &query.job_id,
            &query.source_event_id,
            &query.assignment_id,
            &query.delivery_id,
            &query.source_id,
            &query.cursor,
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(ToolError::InvalidArgument(
                    "Notification filters must be nonempty, control-free and at most 512 bytes"
                        .into(),
                ));
            }
        }
        let response = self
            .client
            .execute_server_operation(WorkspaceServerOperation::NotificationDiagnostics { query })
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        if !response.is_success() {
            // Do not expose an arbitrary error body from upstream.
            return Err(ToolError::ExecutionFailed(format!(
                "Notification diagnostics failed with HTTP status {}",
                response.status
            )));
        }
        if response.body.len() > 1024 * 1024 {
            return Err(ToolError::ExecutionFailed(
                "Notification response exceeds 1 MiB".into(),
            ));
        }
        let page: server_api::NotificationDiagnosticsPage = serde_json::from_str(&response.body)
            .map_err(|_| {
                ToolError::ExecutionFailed("Invalid notification diagnostics response".into())
            })?;
        if page.items.len() > 100 {
            return Err(ToolError::ExecutionFailed(
                "Notification response exceeds 100 items".into(),
            ));
        }
        Ok(ToolOutput {
            summary: NAME.into(),
            content: Some(
                serde_json::to_string(&page).map_err(|e| ToolError::Internal(e.to_string()))?,
            ),
            attachments: vec![],
        })
    }
}
pub(super) fn definition(client: Arc<dyn WorkspaceClient>) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(NAME)
                .description(DESCRIPTION)
                .input_schema(json!(schemars::schema_for!(
                    server_api::NotificationDiagnosticsQuery
                ))),
            Arc::new(DiagnosticsTool {
                client: client.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}
