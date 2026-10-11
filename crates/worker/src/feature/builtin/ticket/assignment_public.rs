//! Public delegation operations over shared host-bound typed Workspace forwarding.
//! Source/actor authorization and assignment/receipt authority remain Backend-owned.
#[cfg(test)]
mod tests;
use super::*;
use crate::worker::WorkspaceServerOperation;

pub(super) const TOOL_NAMES: &[&str] = &[
    "TicketOrchestratorAssign",
    "TicketOrchestratorClear",
    "TicketAssignmentsQuery",
    "TicketAssignmentDiagnostics",
    "TicketAssignmentAnswer",
];
pub(super) const READ_TOOL_NAMES: &[&str] =
    &["TicketAssignmentsQuery", "TicketAssignmentDiagnostics"];

#[derive(Clone, Copy)]
pub(super) enum PublicAssignmentKind {
    Assign,
    Clear,
    Assignments,
    Diagnostics,
    Answer,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssignInput {
    ticket: String,
    /// Target principal, not the authenticated actor. Backend validates role/principal pairing.
    principal: server_api::TicketAssignmentPrincipal,
    /// Observed current assignment ID, or explicitly null for an unassigned Ticket.
    #[serde(deserialize_with = "observed_assignment")]
    expected_assignment_id: Option<String>,
    /// Stable identity of this Assign/reissue; exact retries keep the same full input.
    operation_id: String,
}
fn observed_assignment<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClearInput {
    ticket: String,
    /// Observed exact current delegation to release, not Worker identity/status.
    assignment_id: String,
    operation_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssignmentsInput {
    ticket: String,
    #[serde(flatten)]
    query: server_api::TicketAssignmentsQuery,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiagnosticsInput {
    ticket: String,
    #[serde(flatten)]
    query: server_api::TicketAssignmentDiagnosticsQuery,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AnswerInput {
    ticket: String,
    #[serde(flatten)]
    request: server_api::TicketAssignmentAnswerRequest,
}

impl PublicAssignmentKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Assign => TOOL_NAMES[0],
            Self::Clear => TOOL_NAMES[1],
            Self::Assignments => TOOL_NAMES[2],
            Self::Diagnostics => TOOL_NAMES[3],
            Self::Answer => TOOL_NAMES[4],
        }
    }
    fn schema(self) -> Value {
        match self {
            Self::Assign => {
                let mut schema = json!(schemars::schema_for!(AssignInput));
                let required = schema["required"]
                    .as_array_mut()
                    .expect("Assign object schema");
                if !required
                    .iter()
                    .any(|field| field == "expected_assignment_id")
                {
                    required.push(json!("expected_assignment_id"));
                }
                schema
            }
            Self::Clear => json!(schemars::schema_for!(ClearInput)),
            Self::Assignments => json!(schemars::schema_for!(AssignmentsInput)),
            Self::Diagnostics => json!(schemars::schema_for!(DiagnosticsInput)),
            Self::Answer => json!(schemars::schema_for!(AnswerInput)),
        }
    }
}
pub(super) fn description(name: &str) -> &'static str {
    match name {
        "TicketOrchestratorAssign" => {
            "Explicitly delegate or reissue this Ticket to a target Orchestrator principal using the existing role-assignment API. Query current assignment first; provide observed expected_assignment_id (explicit null only when unassigned), target principal and stable operation_id. Assign persists initial waiting without ready/Queue. Explicit reissue after done creates a new row even for the same principal and preserves the old result. Exact retry keeps the same operation_id and full input; never automatically generate a new key after conflict or unknown outcome. Reread and reconcile stale responsibility. Saved Assign and notification failure are separate outcomes, not a reason to reassign. Backend authenticates the actor and validates role/principal; target principal is not an authority claim. This does not start/stop Workers, close the case, or authorize review/merge."
        }
        "TicketOrchestratorClear" => {
            "Release the observed current Orchestrator delegation using assignment_id and stable operation_id. Query first; exact retry retains the same full input/key, stale conflict requires rereading, and unknown outcome is not automatic success/failure or a new operation. Preserve last handling/result and release history; do not turn release into done, Worker stop, work transfer or resource cleanup. Done/closed do not require clear. Actor authority is authenticated by Backend, never supplied by the model."
        }
        "TicketAssignmentsQuery" => {
            "Read a bounded page of retained Orchestrator assignment identities for this Ticket, role/principal, handling and independent release/replacement history. Use after_assignment_id from the returned next_assignment_id, limit 1..100. Historical done/results are not new work or execution authority. Use current tracking query before a new mutation; a replay receipt is not a fresh observation."
        }
        "TicketAssignmentDiagnostics" => {
            "Read bounded sanitized existing delivery/acceptance/start evidence for an assignment_id, with independent after_input_event_id and after_start_operation_id cursors (limit 1..100). Saved delegation, notification persistence, Runtime acceptance and actual processing are distinct; absent acknowledgement may mean unknown outcome, not failed Assign. History/unknown are not pending work. No rendered content or binding credentials are exposed. Never infer or update self-reported handling from these diagnostics, Worker liveness or MR state."
        }
        "TicketAssignmentAnswer" => {
            "Append continuing input to the observed current waiting/inprogress assignment with assignment_id, expected_last_event_id, stable operation_key and bounded answer body. Query current tracking first. Answers continue the same row without automatic inprogress; done requires explicit new Assign/reissue instead. Backend authenticates/fences source, current assignment and event CAS, then saves input/audit/exact receipt atomically and notifies committed input. Exact retry retains full input/key; stale/cleared/replaced conflicts require rereading, and unknown outcome must not be retried as a fresh operation. No model actor/role claims, new assignment, case closure or Worker start."
        }
        _ => unreachable!("unknown public assignment tool"),
    }
}
fn identity(name: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(ToolError::InvalidArgument(format!(
            "{name} must be nonempty, control-free and at most 1024 bytes"
        )));
    }
    Ok(())
}
fn page(limit: Option<usize>) -> Result<usize, ToolError> {
    let limit = limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(ToolError::InvalidArgument(
            "limit must be between 1 and 100".into(),
        ));
    }
    Ok(limit)
}
fn optional_identity(name: &str, value: &Option<String>) -> Result<(), ToolError> {
    value
        .as_deref()
        .map(|value| identity(name, value))
        .transpose()
        .map(|_| ())
}

struct PublicAssignmentTool {
    client: Arc<dyn WorkspaceClient>,
    kind: PublicAssignmentKind,
}
#[async_trait]
impl Tool for PublicAssignmentTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        self.client
            .workspace_id()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArgument(
                    "Assignment tools require Backend Workspace identity; no local fallback".into(),
                )
            })?;
        let parse = |error: serde_json::Error| ToolError::InvalidArgument(error.to_string());
        let operation = match self.kind {
            PublicAssignmentKind::Assign => {
                let input: AssignInput = serde_json::from_str(input).map_err(parse)?;
                identity("ticket", &input.ticket)?;
                identity("operation_id", &input.operation_id)?;
                optional_identity("expected_assignment_id", &input.expected_assignment_id)?;
                match &input.principal {
                    server_api::TicketAssignmentPrincipal::User { account_id } => {
                        identity("account_id", account_id)?
                    }
                    server_api::TicketAssignmentPrincipal::Worker {
                        runtime_id,
                        worker_id,
                    } => {
                        identity("runtime_id", runtime_id)?;
                        identity("worker_id", worker_id)?;
                    }
                    server_api::TicketAssignmentPrincipal::WorkspaceAgent { agent_key } => {
                        identity("agent_key", agent_key)?
                    }
                }
                WorkspaceServerOperation::TicketAssignmentSet {
                    ticket_id: input.ticket,
                    role: server_api::TicketAssignmentRole::Orchestrator,
                    request: server_api::SetTicketRoleAssignmentRequest {
                        orchestrator_assignment_id: None,
                        operation_id: input.operation_id,
                        principal: input.principal,
                        expected_assignment_id: input.expected_assignment_id,
                        workdir_bindings: vec![],
                    },
                }
            }
            PublicAssignmentKind::Clear => {
                let input: ClearInput = serde_json::from_str(input).map_err(parse)?;
                identity("ticket", &input.ticket)?;
                identity("assignment_id", &input.assignment_id)?;
                identity("operation_id", &input.operation_id)?;
                WorkspaceServerOperation::TicketAssignmentClear {
                    ticket_id: input.ticket,
                    role: server_api::TicketAssignmentRole::Orchestrator,
                    query: server_api::ClearTicketRoleAssignmentQuery {
                        operation_id: Some(input.operation_id),
                        assignment_id: Some(input.assignment_id),
                    },
                }
            }
            PublicAssignmentKind::Assignments => {
                let mut input: AssignmentsInput = serde_json::from_str(input).map_err(parse)?;
                identity("ticket", &input.ticket)?;
                optional_identity("after_assignment_id", &input.query.after_assignment_id)?;
                input.query.limit = Some(page(input.query.limit)?);
                WorkspaceServerOperation::TicketAssignmentsPage {
                    ticket_id: input.ticket,
                    query: input.query,
                }
            }
            PublicAssignmentKind::Diagnostics => {
                let mut input: DiagnosticsInput = serde_json::from_str(input).map_err(parse)?;
                identity("ticket", &input.ticket)?;
                identity("assignment_id", &input.query.assignment_id)?;
                optional_identity("after_input_event_id", &input.query.after_input_event_id)?;
                optional_identity(
                    "after_start_operation_id",
                    &input.query.after_start_operation_id,
                )?;
                input.query.limit = Some(page(input.query.limit)?);
                WorkspaceServerOperation::TicketAssignmentDiagnostics {
                    ticket_id: input.ticket,
                    query: input.query,
                }
            }
            PublicAssignmentKind::Answer => {
                let input: AnswerInput = serde_json::from_str(input).map_err(parse)?;
                identity("ticket", &input.ticket)?;
                for (name, value) in [
                    ("assignment_id", &input.request.0.assignment_id),
                    (
                        "expected_last_event_id",
                        &input.request.0.expected_last_event_id,
                    ),
                    ("operation_key", &input.request.0.operation_key),
                ] {
                    identity(name, value)?;
                }
                let body = &input.request.0.body;
                if body.trim().is_empty() || body.len() > 16384 || body.contains('\0') {
                    return Err(ToolError::InvalidArgument(
                        "Answer body must be nonempty, at most 16384 bytes and without NUL".into(),
                    ));
                }
                WorkspaceServerOperation::TicketAssignmentAnswer {
                    ticket_id: input.ticket,
                    request: input.request,
                }
            }
        };
        let response = self
            .client
            .execute_server_operation(operation)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Assignment request failed with HTTP status {}: {}",
                response.status,
                response.body.chars().take(1024).collect::<String>()
            )));
        }
        if response.body.len() > 1024 * 1024 {
            return Err(ToolError::ExecutionFailed(
                "Assignment response exceeds 1 MiB; request a smaller page".into(),
            ));
        }
        // Decode the actual public DTO and serialize only its declared fields; no raw
        // backend payload/credentials or alternative result authority reaches the model.
        let decode = |error: serde_json::Error| {
            ToolError::ExecutionFailed(format!(
                "Invalid Backend assignment response (outcome unknown after dispatch): {error}"
            ))
        };
        let content = match self.kind {
            PublicAssignmentKind::Assign | PublicAssignmentKind::Clear => serde_json::to_value(
                serde_json::from_str::<server_api::TicketRoleAssignmentMutationResponse>(
                    &response.body,
                )
                .map_err(decode)?,
            ),
            PublicAssignmentKind::Assignments => {
                let result: server_api::TicketAssignmentsPage =
                    serde_json::from_str(&response.body).map_err(decode)?;
                if result.0.items.len() > 100 {
                    return Err(ToolError::ExecutionFailed(
                        "Assignment page exceeds 100 items".into(),
                    ));
                }
                serde_json::to_value(result)
            }
            PublicAssignmentKind::Diagnostics => {
                let result: server_api::TicketAssignmentDiagnosticsPage =
                    serde_json::from_str(&response.body).map_err(decode)?;
                if result.inputs.len() > 100 || result.starts.len() > 100 {
                    return Err(ToolError::ExecutionFailed(
                        "Diagnostic page exceeds 100 items".into(),
                    ));
                }
                serde_json::to_value(result)
            }
            PublicAssignmentKind::Answer => serde_json::to_value(
                serde_json::from_str::<server_api::TicketAssignmentAnswerResponse>(&response.body)
                    .map_err(decode)?,
            ),
        }
        .map_err(|error| ToolError::Internal(error.to_string()))?;
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(content.to_string()),
            attachments: vec![],
        })
    }
}
pub(super) fn definition(
    client: Arc<dyn WorkspaceClient>,
    kind: PublicAssignmentKind,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(description(kind.name()))
                .input_schema(kind.schema()),
            Arc::new(PublicAssignmentTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}
