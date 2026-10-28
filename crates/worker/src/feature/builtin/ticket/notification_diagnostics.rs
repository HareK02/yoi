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
        let query = serde_json::from_str(input)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        execute_domain(self.client.clone(), query).await
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
    _ctx: ToolExecutionContext,
) -> Option<Result<Value, ToolError>> {
    if tool_name != NAME {
        return None;
    }
    Some(match serde_json::from_value(command) {
        Ok(query) => execute_value(client, query).await,
        Err(error) => Err(ToolError::InvalidArgument(error.to_string())),
    })
}

async fn execute_domain(
    client: Arc<dyn WorkspaceClient>,
    query: server_api::NotificationDiagnosticsQuery,
) -> Result<ToolOutput, ToolError> {
    execute_value(client, query)
        .await
        .map(|content| tool_output(NAME, content))
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
    query: server_api::NotificationDiagnosticsQuery,
) -> Result<Value, ToolError> {
    let workspace_id = client
        .workspace_id()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            ToolError::InvalidArgument(
                "Notification diagnostics require a Backend Workspace; no local fallback".into(),
            )
        })?;
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
                "Notification filters must be nonempty, control-free and at most 512 bytes".into(),
            ));
        }
    }
    use server_api::authorization::PermissionResource;
    let mut resources = Vec::new();
    if let Some(job_id) = &query.job_id {
        resources.push(PermissionResource::Job {
            workspace_id: workspace_id.into(),
            job_id: job_id.clone(),
        });
    }
    if let Some(ticket_id) = &query.ticket {
        resources.push(PermissionResource::Ticket {
            workspace_id: workspace_id.into(),
            ticket_id: ticket_id.clone(),
        });
    }
    if let Some(merge_request_id) = &query.merge_request_id {
        resources.push(PermissionResource::MergeRequest {
            workspace_id: workspace_id.into(),
            merge_request_id: merge_request_id.clone(),
        });
    }
    // source_checks probes origins independently of the page's other filters.
    // Opaque (including hashed and legacy Job) identities must resolve from
    // Host metadata, never from a caller-supplied Job filter.
    let mut bound_source = None;
    if let Some(event) = &query.source_event_id {
        // Ticket ordinals are scoped by source_checks to this Ticket. All
        // global identities use persisted ownership, including legacy Job
        // IDs whose spelling happens to resemble an MR identity.
        if !(event.starts_with("ticket-event:") && query.ticket.is_some()) {
            bound_source = Some(event.clone());
        }
    }
    if resources.is_empty() && bound_source.is_none() {
        resources.push(PermissionResource::Workspace {
            workspace_id: workspace_id.into(),
        });
    }
    for resource in resources {
        client
            .authorize_operation("notification.diagnostics.read", &resource)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    }
    if let Some(source_event_id) = bound_source {
        client
            .authorize_bound_operation(
                "notification.diagnostics.read",
                server_api::authorization::BoundAuthorizationTarget::NotificationSource {
                    source_event_id,
                },
            )
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    }
    let response = client
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
    serde_json::to_value(page).map_err(|e| ToolError::Internal(e.to_string()))
}
#[cfg(test)]
mod authorization_tests {
    use super::*;
    use assignment_tracking::authorization_tests::{AuthorizedClient, ticket};
    use server_api::authorization::PermissionResource;

    fn mr(id: &str) -> PermissionResource {
        PermissionResource::MergeRequest {
            workspace_id: "workspace".into(),
            merge_request_id: id.into(),
        }
    }
    fn job(id: &str) -> PermissionResource {
        PermissionResource::Job {
            workspace_id: "workspace".into(),
            job_id: id.into(),
        }
    }
    fn workspace() -> PermissionResource {
        PermissionResource::Workspace {
            workspace_id: "workspace".into(),
        }
    }
    #[tokio::test]
    async fn notification_filters_authorize_all_independent_origins_before_read() {
        for (input, resources) in [
            (json!({"job_id":"Job-A"}), vec![job("Job-A")]),
            (
                json!({"ticket":"T-A","merge_request_id":"MR-A","job_id":"Job-A"}),
                vec![job("Job-A"), ticket("T-A"), mr("MR-A")],
            ),
            (json!({"ticket":"T-A"}), vec![ticket("T-A")]),
            (json!({"merge_request_id":"MR-A"}), vec![mr("MR-A")]),
            (
                json!({"ticket":"T-A","merge_request_id":"MR-A"}),
                vec![ticket("T-A"), mr("MR-A")],
            ),
            (
                json!({"ticket":"T-A","source_event_id":"ticket-event:8"}),
                vec![ticket("T-A")],
            ),
            (
                json!({"source_id":"notification-source:8"}),
                vec![workspace()],
            ),
            (json!({}), vec![workspace()]),
        ] {
            let client = AuthorizedClient::new(
                json!({"items":[],"next_cursor":"notification-source:8","source_checks":[]}),
            );
            let (_, tool) = definition(client.clone())();
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("HTTP status 403")
            );
            assert!(client.requests.lock().unwrap().is_empty());
            for resource in &resources {
                client.grant("notification.diagnostics.read", resource.clone());
            }
            let output = tool
                .execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap()["next_cursor"],
                "notification-source:8"
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            let auth = client.authorizations.lock().unwrap();
            assert_eq!(
                auth[1..]
                    .iter()
                    .map(|request| request.resource.clone())
                    .collect::<Vec<_>>(),
                resources
            );
            drop(auth);
            client.grants.lock().unwrap().pop();
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert_eq!(
                client.requests.lock().unwrap().len(),
                1,
                "every filter needs authority"
            );
        }
    }

    #[tokio::test]
    async fn notification_ticket_grant_rejects_other_ticket_and_wip_rechecks_revocation() {
        let client =
            AuthorizedClient::new(json!({"items":[],"next_cursor":null,"source_checks":[]}));
        client.grant("notification.diagnostics.read", ticket("T-1"));
        let (_, tool) = definition(client.clone())();
        assert!(
            tool.execute(r#"{"ticket":"T-B"}"#, ToolExecutionContext::direct())
                .await
                .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::read_only()).unwrap();
        let handler = TicketCollectionWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|tool| tool.projection.surface == NativeTicketSurface::Collection)
                    .collect(),
            ),
            permissions: None,
            collection_route: "/tickets".into(),
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let args = BTreeMap::from([(
            "filter".into(),
            json_to_wip(&json!({"ticket":"/tickets/T-1"})).unwrap(),
        )]);
        handler
            .call(
                "notification_diagnostics",
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP rejected allowed notification read"));
        client.grants.lock().unwrap().clear();
        assert!(matches!(
            handler
                .call(
                    "notification_diagnostics",
                    &args,
                    WipCallContext {
                        result_namespaces: Default::default(),
                        execution: ToolExecutionContext::direct(),
                        security_context: "worker".into()
                    }
                )
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn notification_job_grant_rejects_other_job_and_revocation_without_downstream_read() {
        let response = json!({"items":[],"next_cursor":null,"source_checks":[{"resource_kind":"job_result","reference":"Job-A","origin_present":true,"captured":true}]});
        let client = AuthorizedClient::new(response.clone());
        let (_, tool) = definition(client.clone())();
        client.grant("notification.diagnostics.read", job("Job-A"));
        assert!(
            tool.execute(r#"{"job_id":"Job-B"}"#, ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("HTTP status 403")
        );
        assert!(client.requests.lock().unwrap().is_empty());
        let output = tool
            .execute(r#"{"job_id":"Job-A"}"#, ToolExecutionContext::direct())
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap(),
            response
        );
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::read_only()).unwrap();
        let handler = TicketCollectionWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|tool| tool.projection.surface == NativeTicketSurface::Collection)
                    .collect(),
            ),
            permissions: None,
            collection_route: "/tickets".into(),
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let args = BTreeMap::from([(
            "filter".into(),
            json_to_wip(&json!({"job_id":"Job-A"})).unwrap(),
        )]);
        handler
            .call(
                "notification_diagnostics",
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP rejected authorized Job diagnostics"));
        client.requests.lock().unwrap().clear();
        client.grants.lock().unwrap().clear();
        assert!(
            tool.execute(r#"{"job_id":"Job-A"}"#, ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("HTTP status 403")
        );
        assert!(matches!(
            handler
                .call(
                    "notification_diagnostics",
                    &args,
                    WipCallContext {
                        result_namespaces: Default::default(),
                        execution: ToolExecutionContext::direct(),
                        security_context: "worker".into()
                    }
                )
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert!(client.requests.lock().unwrap().is_empty());
        let auth = client.authorizations.lock().unwrap();
        assert_eq!(auth.len(), 5);
        assert!(auth.iter().all(
            |request| request.operation == "notification.diagnostics.read"
                && matches!(request.resource, PermissionResource::Job { .. })
        ));
        assert_eq!(auth[0].resource, job("Job-B"));
        assert!(
            auth[1..]
                .iter()
                .all(|request| request.resource == job("Job-A"))
        );
    }

    #[tokio::test]
    async fn notification_source_uses_host_target_not_asserted_job_or_workspace_fallback() {
        let client = tests::BoundClient::new();
        let (_, tool) = definition(client.clone())();
        client
            .inner
            .grant("notification.diagnostics.read", job("Job-A"));
        for event in [
            "backend-job-notification:hash-B",
            "legacy-B",
            "mr-event:legacy-B:e",
            "missing",
            "ambiguous",
        ] {
            let input = json!({"job_id":"Job-A","source_event_id":event});
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
        }
        assert!(client.inner.requests.lock().unwrap().is_empty());
        for event in ["backend-job-notification:hash-A", "legacy-A"] {
            let input = json!({"source_event_id":event});
            let output = tool
                .execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            let page: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(page["source_checks"][0]["captured"], true);
        }
        assert_eq!(client.inner.requests.lock().unwrap().len(), 2);
        client.inner.grants.lock().unwrap().clear();
        assert!(
            tool.execute(
                r#"{"source_event_id":"legacy-A"}"#,
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
        );
        assert_eq!(client.inner.requests.lock().unwrap().len(), 2);
        // A Workspace grant cannot authorize an individual Job source.
        client
            .inner
            .grant("notification.diagnostics.read", workspace());
        assert!(
            tool.execute(
                r#"{"source_event_id":"legacy-B"}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .is_err()
        );
        assert_eq!(client.inner.requests.lock().unwrap().len(), 2);
        client
            .inner
            .grant("notification.diagnostics.read", job("Job-B"));
        tool.execute(
            r#"{"source_event_id":"legacy-B"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        assert_eq!(client.bound.lock().unwrap().last().unwrap(), "legacy-B");
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

#[cfg(test)]
mod native_command_tests {
    use super::*;
    use assignment_tracking::authorization_tests::{AuthorizedClient, ticket};
    use server_api::authorization::PermissionResource;

    #[tokio::test]
    async fn internal_notification_command_requires_all_origins_and_shares_tool_request() {
        let client = AuthorizedClient::new(json!({
            "items":[], "next_cursor":"source/8?", "source_checks":[]
        }));
        let input = json!({"ticket":"T-A","job_id":"Job-A","cursor":"source/7?","limit":2});
        client.grant("notification.diagnostics.read", ticket("T-A"));
        assert!(
            execute_native(
                client.clone(),
                NAME,
                input.clone(),
                ToolExecutionContext::direct()
            )
            .await
            .unwrap()
            .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
        client.grant(
            "notification.diagnostics.read",
            PermissionResource::Job {
                workspace_id: "workspace".into(),
                job_id: "Job-A".into(),
            },
        );
        let (_, tool) = definition(client.clone())();
        let ordinary = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let native = execute_native(
            client.clone(),
            NAME,
            input.clone(),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap()
        .unwrap();
        let value = execute_native_value(
            client.clone(),
            NAME,
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
        assert_eq!(value["next_cursor"], "source/8?");
        assert_eq!(native.content, ordinary.content);
        assert_eq!(native.summary, ordinary.summary);
        assert_eq!(ordinary.summary, NAME);
        assert!(native.attachments.is_empty());
        assert_eq!(client.requests.lock().unwrap().len(), 3);
        {
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests[0], requests[1]);
            assert_eq!(requests[1], requests[2]);
        }
        client.grants.lock().unwrap().clear();
        assert!(
            execute_native_value(client.clone(), NAME, input, ToolExecutionContext::direct())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(client.requests.lock().unwrap().len(), 3);
        assert!(
            execute_native_value(
                client.clone(),
                "Unknown",
                Value::Null,
                ToolExecutionContext::direct()
            )
            .await
            .is_none()
        );
        assert!(matches!(
            execute_native(
                client,
                NAME,
                json!({"limit":0}),
                ToolExecutionContext::direct()
            )
            .await
            .unwrap(),
            Err(ToolError::InvalidArgument(_))
        ));
    }
}
