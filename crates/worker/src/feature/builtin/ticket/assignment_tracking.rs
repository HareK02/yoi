//! Self-reported handling of one immutable delegation. This transport never
//! supplies actor/role authority or converts tracking into execution lifecycle.
//!
//! Updates forward the complete handling payload with the observed event of that exact assignment,
//! not a status-only comparison. A replayed receipt is not a fresh observation for the next update.
//! Query/history may inspect retained assignments and unknown legacy handling; read access does not
//! enable self-report mutation. Unknown handling is not inferred waiting or successful done.
use super::*;

pub(super) const TOOL_NAMES: &[&str] = &[
    "TicketAssignmentTrackingQuery",
    "TicketAssignmentTrackingHistory",
    "TicketAssignmentTrackingUpdate",
];

#[derive(Clone, Copy)]
pub(super) enum TrackingToolKind {
    Query,
    History,
    Update,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryInput {
    /// Ticket reference, normally T-*.
    ticket: String,
    /// Omit for the current delegation; set to inspect a retained assignment and release.
    assignment_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HistoryInput {
    ticket: String,
    /// Immutable delegation identity, including released or replaced assignments.
    assignment_id: String,
    /// Continue after an event identity returned by this assignment's history.
    after_event_id: Option<String>,
    #[schemars(range(min = 1, max = 100))]
    limit: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    ticket: String,
    assignment_id: String,
    /// Last event observed for this exact delegation, not Ticket status or Worker state.
    expected_last_event_id: String,
    /// Reuse the same key and complete input after an uncertain result.
    operation_key: String,
    tracking: ticket::TicketAssignmentTracking,
}

impl TrackingToolKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Query => TOOL_NAMES[0],
            Self::History => TOOL_NAMES[1],
            Self::Update => TOOL_NAMES[2],
        }
    }
    fn schema(self) -> Value {
        match self {
            Self::Query => json!(schemars::schema_for!(QueryInput)),
            Self::History => json!(schemars::schema_for!(HistoryInput)),
            Self::Update => json!(schemars::schema_for!(UpdateInput)),
        }
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "TicketAssignmentTrackingQuery" => {
            "Read the current Ticket delegation, or one retained assignment_id with its role/principal, Assign actor/time and separate release/replacement record, and durable self-reported waiting/inprogress/done, last event identity, waiting reason, progress, next action and result. Tracking is independent of Ticket open/closed, Worker running/idle/stop, MR and delivery state."
        }
        "TicketAssignmentTrackingHistory" => {
            "Read bounded immutable history for one assignment_id, including released/replaced delegations. Continue with the returned event identity. Historical results do not grant execution rights or create a new request."
        }
        "TicketAssignmentTrackingUpdate" => {
            "Record the current Workspace Orchestrator's self-reported handling of one delegation with assignment_id, observed last_event_id CAS and an exact-retry operation key. Direct waiting-to-done and post-close result recording are allowed. Answers during waiting continue the same row; explicit reissue after done requires a new Assign row, not overwriting the previous result. Send the complete tracking payload. Exact retry uses the same operation_key and full input; different input with that key is rejected. On CAS/stale conflict reread current authority and reconcile; unknown outcome is not success or failure and must not become a fresh operation automatically. Backend authenticates the current registered Orchestrator; model role names never authorize. This neither closes/reopens the Ticket, releases actual work, restores grants, starts/stops Workers nor approves/merges MRs. Released/replaced assignments reject new updates; exact retry receipts are not fresh observations."
        }
        _ => unreachable!("unknown assignment tracking tool"),
    }
}

fn required(name: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        return Err(ToolError::InvalidArgument(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub(super) fn authorize_ticket_target(
    client: &dyn WorkspaceClient,
    operation: &str,
    ticket_id: &str,
) -> Result<(), ToolError> {
    let workspace_id = client
        .workspace_id()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            ToolError::InvalidArgument(
                "Ticket authorization requires Backend Workspace identity".into(),
            )
        })?;
    client
        .authorize_operation(
            operation,
            &server_api::authorization::PermissionResource::Ticket {
                workspace_id: workspace_id.into(),
                ticket_id: ticket_id.into(),
            },
        )
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))
}

#[derive(Clone)]
struct TrackingTool {
    client: Arc<dyn WorkspaceClient>,
    kind: TrackingToolKind,
}

// Internal command DTOs belong to the shared execution boundary, not a native schema.
enum Command {
    Query(QueryInput),
    History(HistoryInput),
    Update(UpdateInput),
}

impl Command {
    fn kind(&self) -> TrackingToolKind {
        match self {
            Self::Query(..) => TrackingToolKind::Query,
            Self::History(..) => TrackingToolKind::History,
            Self::Update(..) => TrackingToolKind::Update,
        }
    }
}

#[async_trait]
impl Tool for TrackingTool {
    async fn execute(
        &self,
        input: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let command = match self.kind {
            TrackingToolKind::Query => Command::Query(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TrackingToolKind::History => Command::History(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TrackingToolKind::Update => Command::Update(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
        };
        execute_domain(self.client.clone(), command, ctx).await
    }
}

/// Preserve the regular Tool presentation for internal callers.
#[cfg(test)]
pub(super) async fn execute_native(
    client: Arc<dyn WorkspaceClient>,
    tool_name: &str,
    command: Value,
    ctx: ToolExecutionContext,
) -> Option<Result<ToolOutput, ToolError>> {
    execute_native_value(client, tool_name, command, ctx)
        .await
        .map(|result| result.map(|content| tool_output(tool_name, content)))
}

/// Return the shared domain projection before Tool presentation serialization.
pub(super) async fn execute_native_value(
    client: Arc<dyn WorkspaceClient>,
    tool_name: &str,
    command: Value,
    ctx: ToolExecutionContext,
) -> Option<Result<Value, ToolError>> {
    let parse = |error: serde_json::Error| ToolError::InvalidArgument(error.to_string());
    let command = match tool_name {
        "TicketAssignmentTrackingQuery" => serde_json::from_value(command).map(Command::Query),
        "TicketAssignmentTrackingHistory" => serde_json::from_value(command).map(Command::History),
        "TicketAssignmentTrackingUpdate" => serde_json::from_value(command).map(Command::Update),
        _ => return None,
    };
    Some(match command {
        Ok(command) => execute_value(client, command, ctx).await,
        Err(error) => Err(parse(error)),
    })
}

async fn execute_domain(
    client: Arc<dyn WorkspaceClient>,
    command: Command,
    ctx: ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let name = command.kind().name();
    execute_value(client, command, ctx)
        .await
        .map(|content| tool_output(name, content))
}

fn tool_output(name: &str, content: Value) -> ToolOutput {
    ToolOutput {
        summary: name.into(),
        content: Some(content.to_string()),
        attachments: vec![],
    }
}

async fn execute_value(
    client: Arc<dyn WorkspaceClient>,
    command: Command,
    _ctx: ToolExecutionContext,
) -> Result<Value, ToolError> {
    let kind = command.kind();
    let workspace = client
        .workspace_id()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            ToolError::InvalidArgument(
                "Assignment tracking requires Backend Workspace identity; no local fallback".into(),
            )
        })?;
    let prefix = format!("/workspaces/{}/tickets", encode_segment(workspace));
    let (method, path, body, ticket_id) = match command {
        Command::Query(input) => {
            required("ticket", &input.ticket)?;
            let mut path = format!(
                "{prefix}/{}/assignment-tracking",
                encode_segment(&input.ticket)
            );
            if let Some(assignment_id) = input.assignment_id {
                required("assignment_id", &assignment_id)?;
                path.push_str(&format!(
                    "?assignment_id={}",
                    encode_segment(&assignment_id)
                ));
            }
            (WorkspaceRequestMethod::Get, path, None, input.ticket)
        }
        Command::History(input) => {
            required("ticket", &input.ticket)?;
            required("assignment_id", &input.assignment_id)?;
            let limit = input.limit.unwrap_or(100);
            if !(1..=100).contains(&limit) {
                return Err(ToolError::InvalidArgument(
                    "limit must be between 1 and 100".into(),
                ));
            }
            let mut path = format!(
                "{prefix}/{}/assignment-tracking/history?assignment_id={}&limit={limit}",
                encode_segment(&input.ticket),
                encode_segment(&input.assignment_id)
            );
            if let Some(after) = input.after_event_id {
                required("after_event_id", &after)?;
                path.push_str(&format!("&after_event_id={}", encode_segment(&after)));
            }
            (WorkspaceRequestMethod::Get, path, None, input.ticket)
        }
        Command::Update(input) => {
            for (name, value) in [
                ("ticket", &input.ticket),
                ("assignment_id", &input.assignment_id),
                ("expected_last_event_id", &input.expected_last_event_id),
                ("operation_key", &input.operation_key),
            ] {
                required(name, value)?;
            }
            // Unknown fields (including author, role, runtime and worker) are
            // rejected before transport; authenticated context belongs to Backend.
            let body = json!({"assignment_id": input.assignment_id, "expected_last_event_id": input.expected_last_event_id, "operation_key": input.operation_key, "tracking": input.tracking});
            (
                WorkspaceRequestMethod::Post,
                format!(
                    "{prefix}/{}/assignment-tracking",
                    encode_segment(&input.ticket)
                ),
                Some(body.to_string()),
                input.ticket,
            )
        }
    };
    let request = match body {
        Some(body) => WorkspaceRequest::json(method, path, body),
        None => WorkspaceRequest::get(path),
    };
    authorize_ticket_target(
        client.as_ref(),
        match kind {
            TrackingToolKind::Query => "ticket.assignment.query",
            TrackingToolKind::History => "ticket.assignment.history",
            TrackingToolKind::Update => "ticket.assignment.update",
        },
        &ticket_id,
    )?;
    let response = client
        .execute(request)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    if !response.is_success() {
        let detail: String = response.body.chars().take(1024).collect();
        return Err(ToolError::ExecutionFailed(format!(
            "Assignment tracking request failed with HTTP status {}: {}",
            response.status, detail
        )));
    }
    if response.body.len() > 1024 * 1024 {
        return Err(ToolError::ExecutionFailed(
            "Assignment tracking response exceeds 1 MiB; request a smaller history page".into(),
        ));
    }
    let content: Value = serde_json::from_str(&response.body).map_err(|error| {
        ToolError::ExecutionFailed(format!(
            "Backend returned invalid assignment tracking JSON: {error}"
        ))
    })?;
    Ok(content)
}

pub(super) fn definition(
    client: Arc<dyn WorkspaceClient>,
    kind: TrackingToolKind,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(description(kind.name()))
                .input_schema(kind.schema()),
            Arc::new(TrackingTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}

#[cfg(test)]
pub(super) mod authorization_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};

    #[derive(Debug)]
    struct Transport {
        workspace: Option<String>,
        response: WorkspaceResponse,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }
    impl WorkspaceClient for Transport {
        fn authorize_operation(
            &self,
            _: &str,
            _: &server_api::authorization::PermissionResource,
        ) -> Result<(), WorkspaceClientError> {
            Ok(())
        }
        fn workspace_id(&self) -> Option<&str> {
            self.workspace.as_deref()
        }
        fn kind(&self) -> &str {
            "tracking-contract-test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }
    }
    fn transport(status: u16, body: &str) -> Arc<Transport> {
        Arc::new(Transport {
            workspace: Some("workspace".into()),
            response: WorkspaceResponse {
                status,
                body: body.into(),
            },
            requests: Mutex::new(Vec::new()),
        })
    }
    fn update() -> Value {
        json!({"ticket":"T-741", "assignment_id":"delegation", "expected_last_event_id":"assigned-event", "operation_key":"report", "tracking":{"status":"done", "waiting_reason":null, "progress":"Alternatives compared", "next_action":"Human decision", "result":"Returned options"}})
    }

    #[tokio::test]
    async fn assignment_tracking_update_requires_observed_identity_and_excludes_actor_claims() {
        let client = transport(
            200,
            r#"{"assignment_id":"delegation","last_event_id":"reported-event","tracking":{"status":"done"}}"#,
        );
        let (meta, tool) = definition(client.clone(), TrackingToolKind::Update)();
        assert_eq!(meta.name, "TicketAssignmentTrackingUpdate");
        let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
        assert!(validator.is_valid(&update()));
        for field in [
            "assignment_id",
            "expected_last_event_id",
            "operation_key",
            "tracking",
        ] {
            let mut input = update();
            input.as_object_mut().unwrap().remove(field);
            assert!(!validator.is_valid(&input), "required {field}");
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
        }
        for field in [
            "author",
            "role",
            "runtime_id",
            "worker_id",
            "source_context",
        ] {
            let mut input = update();
            input[field] = json!("orchestrator");
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err(),
                "reject {field}"
            );
        }
        assert!(client.requests.lock().unwrap().is_empty());
        for _ in 0..2 {
            let output = tool
                .execute(&update().to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&output.content.unwrap()).unwrap()["last_event_id"],
                "reported-event"
            );
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(
            requests[0], requests[1],
            "uncertain retry preserves full input"
        );
        assert_eq!(
            requests[0].path,
            "/workspaces/workspace/tickets/T-741/assignment-tracking"
        );
        let body: Value = serde_json::from_str(requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["tracking"]["status"], "done");
        assert!(body.get("ticket").is_none());
        assert!(body.get("author").is_none());
    }

    #[tokio::test]
    async fn assignment_tracking_reads_are_bounded_and_carry_history_cursor() {
        let client = transport(200, r#"{"items":[],"next_event_id":null}"#);
        let (_, tool) = definition(client.clone(), TrackingToolKind::History)();
        for limit in [0, 101] {
            assert!(
                tool.execute(
                    &json!({"ticket":"T-741", "assignment_id":"a", "limit":limit}).to_string(),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
            );
        }
        tool.execute(
            &json!({"ticket":"T-741", "assignment_id":"a", "after_event_id":"event/1", "limit":10})
                .to_string(),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
        assert!(
            requests[0]
                .path
                .ends_with("?assignment_id=a&limit=10&after_event_id=event%2F1")
        );
        drop(requests);
        let oversized = transport(200, &"x".repeat(1024 * 1024 + 1));
        let (_, tool) = definition(oversized, TrackingToolKind::Query)();
        assert!(
            tool.execute(r#"{"ticket":"T-741"}"#, ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
    }

    #[tokio::test]
    async fn retained_assignment_query_encodes_identity_without_changing_workspace_scope() {
        let client = transport(
            200,
            r#"[{"assignment_id":"old/delegation?x","current":false,"release":{"action":"replaced"}}]"#,
        );
        let (_, tool) = definition(client.clone(), TrackingToolKind::Query)();
        let output = tool
            .execute(
                r#"{"ticket":"T-741","assignment_id":"old/delegation?x"}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let content: Value = serde_json::from_str(&output.content.unwrap()).unwrap();
        assert_eq!(content[0]["current"], false);
        assert_eq!(content[0]["release"]["action"], "replaced");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
        assert_eq!(
            requests[0].path,
            "/workspaces/workspace/tickets/T-741/assignment-tracking?assignment_id=old%2Fdelegation%3Fx"
        );
    }

    #[tokio::test]
    async fn assignment_tracking_requires_backend_and_propagates_authority_rejection() {
        let client = Arc::new(Transport {
            workspace: None,
            response: WorkspaceResponse {
                status: 200,
                body: "{}".into(),
            },
            requests: Mutex::new(Vec::new()),
        });
        let (_, tool) = definition(client.clone(), TrackingToolKind::Update)();
        assert!(
            tool.execute(&update().to_string(), ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("Backend Workspace identity")
        );
        assert!(client.requests.lock().unwrap().is_empty());
        let client = transport(403, r#"{"code":"current_orchestrator_required"}"#);
        let (_, tool) = definition(client, TrackingToolKind::Update)();
        assert!(
            tool.execute(&update().to_string(), ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("403")
        );
    }

    #[test]
    fn assignment_tracking_tools_share_native_wip_and_access_contract() {
        let client = transport(200, "{}");
        let definitions = enabled_ticket_definitions(client, TicketFeatureAccess::workflow());
        for name in TOOL_NAMES {
            let (meta, _) = definitions
                .iter()
                .find(|definition| definition().0.name == *name)
                .unwrap()();
            let native = native_ticket_operation(name).unwrap();
            assert_eq!(native.identity_field, Some("ticket"));
            assert_eq!(native.mutating, *name == "TicketAssignmentTrackingUpdate");
            assert!(
                meta.description.contains("delegation")
                    || meta.description.contains("assignment_id")
            );
        }
        let reads = TicketFeatureAccess::read_only().tool_names();
        assert!(reads.contains(&"TicketAssignmentTrackingQuery"));
        assert!(reads.contains(&"TicketAssignmentTrackingHistory"));
        assert!(!reads.contains(&"TicketAssignmentTrackingUpdate"));
    }
}

#[cfg(test)]
mod native_command_tests {
    use super::authorization_tests::{AuthorizedClient, ticket};
    use super::*;

    #[tokio::test]
    async fn internal_tracking_commands_share_authorization_and_full_cas_payload_with_tools() {
        for (kind, operation, input) in [
            (
                TrackingToolKind::Query,
                "ticket.assignment.query",
                json!({"ticket":"T-A", "assignment_id":"old/delegation?"}),
            ),
            (
                TrackingToolKind::History,
                "ticket.assignment.history",
                json!({"ticket":"T-A", "assignment_id":"a", "after_event_id":"event/7?", "limit":5}),
            ),
            (
                TrackingToolKind::Update,
                "ticket.assignment.update",
                json!({"ticket":"T-A", "assignment_id":"a", "expected_last_event_id":"observed", "operation_key":"stable", "tracking":{"status":"done", "waiting_reason":null, "progress":"Compared", "next_action":null, "result":"Returned"}}),
            ),
        ] {
            let client =
                AuthorizedClient::new(json!({"assignment_id":"a", "last_event_id":"next"}));
            assert!(
                execute_native(
                    client.clone(),
                    kind.name(),
                    input.clone(),
                    ToolExecutionContext::direct()
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(client.requests.lock().unwrap().is_empty());
            client.grant(operation, ticket("T-A"));
            let (_, tool) = definition(client.clone(), kind)();
            let ordinary = tool
                .execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            let native = execute_native(
                client.clone(),
                kind.name(),
                input.clone(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap()
            .unwrap();
            let value = execute_native_value(
                client.clone(),
                kind.name(),
                input.clone(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                value,
                serde_json::from_str::<Value>(ordinary.content.as_deref().unwrap()).unwrap()
            );
            assert_eq!(native.content, ordinary.content);
            assert_eq!(native.summary, ordinary.summary);
            assert_eq!(ordinary.summary, kind.name());
            assert!(native.attachments.is_empty());
            assert_eq!(client.requests.lock().unwrap().len(), 3);
            {
                let requests = client.requests.lock().unwrap();
                assert_eq!(requests[0], requests[1]);
                assert_eq!(requests[1], requests[2]);
            }
            client.grants.lock().unwrap().clear();
            assert!(
                execute_native_value(
                    client.clone(),
                    kind.name(),
                    input,
                    ToolExecutionContext::direct()
                )
                .await
                .unwrap()
                .is_err()
            );
            assert_eq!(client.requests.lock().unwrap().len(), 3);
            assert!(
                execute_native_value(
                    client,
                    "Unknown",
                    Value::Null,
                    ToolExecutionContext::direct()
                )
                .await
                .is_none()
            );
        }
    }
}
