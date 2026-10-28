use super::tests::{cases, native_args};
use super::*;
use assignment_tracking::authorization_tests::{AuthorizedClient, ticket};

#[tokio::test]
async fn public_operations_require_exact_ticket_grants_on_tool_and_wip() {
    for (kind, input, response) in cases() {
        let operation = match kind {
            PublicAssignmentKind::Assign => "ticket.assignment.assign",
            PublicAssignmentKind::Clear => "ticket.assignment.clear",
            PublicAssignmentKind::Assignments => "ticket.assignment.query",
            PublicAssignmentKind::Diagnostics => "ticket.assignment.diagnostics",
            PublicAssignmentKind::Answer => "ticket.assignment.answer",
            PublicAssignmentKind::ImplementationResult => "ticket.implementation_result.write",
            PublicAssignmentKind::ImplementationResultQuery => "ticket.implementation_result.read",
        };
        let client = AuthorizedClient::new(response.clone());
        let (_, tool) = definition(client.clone(), kind)();
        let error = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("HTTP status 403"),
            "{}: {error}",
            kind.name()
        );
        assert!(client.requests.lock().unwrap().is_empty());
        client.grant(operation, ticket("T-743"));
        let output = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let actual: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        for (key, value) in response.as_object().unwrap() {
            assert_eq!(&actual[key], value);
        }
        let mut other = input.clone();
        other["ticket"] = json!("T-B");
        assert!(
            tool.execute(&other.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        let handler = TicketItemWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
                    .collect(),
            ),
            permissions: None,
            ticket_reference: "T-743".into(),
            collection_route: "/tickets".into(),
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let args = native_args(kind, &input);
        let projection = native_ticket_operation(kind.name()).unwrap();
        handler
            .call(
                projection.operation,
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP {} rejected allowed operation", kind.name()));
        assert_eq!(client.requests.lock().unwrap().len(), 2);
        client.grants.lock().unwrap().clear();
        assert!(matches!(
            handler
                .call(
                    projection.operation,
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
        assert_eq!(client.requests.lock().unwrap().len(), 2);
        let auth = client.authorizations.lock().unwrap();
        assert_eq!(auth.len(), 5);
        assert!(auth.iter().all(|request| request.operation == operation));
        assert_eq!(auth[0].resource, ticket("T-743"));
        assert_eq!(auth[2].resource, ticket("T-B"));
    }
}
