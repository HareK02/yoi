use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceResponse};
use server_api::authorization::{
    OperationAuthorizationRequest, PermissionResource, ScopedPermission,
};

// Component connection: real Tools/native WIP and WorkspaceClient authorization
// forwarding; the Host HTTP boundary evaluates exact operation/resource grants.
#[derive(Debug)]
pub(in crate::feature::builtin::ticket) struct AuthorizedClient {
    pub grants: Mutex<Vec<ScopedPermission>>,
    pub authorizations: Mutex<Vec<OperationAuthorizationRequest>>,
    pub requests: Mutex<Vec<WorkspaceRequest>>,
    response: Value,
}
impl AuthorizedClient {
    pub fn new(response: Value) -> Arc<Self> {
        Arc::new(Self {
            grants: Mutex::new(vec![]),
            authorizations: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            response,
        })
    }
    pub fn grant(&self, operation: &str, resource: PermissionResource) {
        self.grants.lock().unwrap().push(ScopedPermission {
            operation: operation.into(),
            resource,
        });
    }
}
impl WorkspaceClient for AuthorizedClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "authorized-assignment-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path == "/api/w/workspace/authorization/check" {
            let auth: OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let requested = ScopedPermission {
                operation: auth.operation.clone(),
                resource: auth.resource.clone(),
            };
            let allowed = self
                .grants
                .lock()
                .unwrap()
                .iter()
                .any(|grant| grant.covers(&requested));
            self.authorizations.lock().unwrap().push(auth);
            return Ok(WorkspaceResponse {
                status: if allowed { 200 } else { 403 },
                body: if allowed {
                    json!({"scope": requested.resource})
                } else {
                    json!({"message":"not granted"})
                }
                .to_string(),
            });
        }
        self.requests.lock().unwrap().push(request);
        Ok(WorkspaceResponse {
            status: 200,
            body: self.response.to_string(),
        })
    }
}
pub(in crate::feature::builtin::ticket) fn ticket(id: &str) -> PermissionResource {
    PermissionResource::Ticket {
        workspace_id: "workspace".into(),
        ticket_id: id.into(),
    }
}

#[tokio::test]
async fn tracking_operations_authorize_exact_ticket_before_dispatch_and_recheck_revocation() {
    for (kind, operation, input) in [
        (
            TrackingToolKind::Query,
            "ticket.assignment.query",
            json!({"ticket":"T-A"}),
        ),
        (
            TrackingToolKind::History,
            "ticket.assignment.history",
            json!({"ticket":"T-A","assignment_id":"a"}),
        ),
        (
            TrackingToolKind::Update,
            "ticket.assignment.update",
            json!({"ticket":"T-A","assignment_id":"a","expected_last_event_id":"before","operation_key":"report","tracking":{"status":"done","waiting_reason":null,"progress":null,"next_action":null,"result":"Returned options"}}),
        ),
    ] {
        let client = AuthorizedClient::new(json!({"assignment_id":"a","last_event_id":"after"}));
        let (_, tool) = definition(client.clone(), kind)();
        assert!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("HTTP status 403")
        );
        assert!(client.requests.lock().unwrap().is_empty());
        client.grant(operation, ticket("T-A"));
        let output = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap()["last_event_id"],
            "after"
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let mut other = input.clone();
        other["ticket"] = json!("T-B");
        assert!(
            tool.execute(&other.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
        client.grants.lock().unwrap().clear();
        assert!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let auth = client.authorizations.lock().unwrap();
        assert_eq!(auth.len(), 4);
        assert!(auth.iter().all(|request| request.operation == operation));
        assert_eq!(auth[0].resource, ticket("T-A"));
        assert_eq!(auth[2].resource, ticket("T-B"));
        drop(auth);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        let handler = TicketItemWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
                    .collect(),
            ),
            permissions: None,
            ticket_reference: "T-A".into(),
            collection_route: "/tickets".into(),
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let mut native = input.clone();
        native.as_object_mut().unwrap().remove("ticket");
        if matches!(kind, TrackingToolKind::Update) {
            let fields = native.as_object_mut().unwrap();
            let assignment = fields.remove("assignment_id").unwrap();
            let event = fields.remove("expected_last_event_id").unwrap();
            fields.insert(
                "observed".into(),
                json!({"assignment_id":assignment,"expected_last_event_id":event}),
            );
            for field in ["waiting_reason", "progress", "next_action", "result"] {
                if let Some(value) = native["tracking"].get_mut(field) {
                    *value = if value.is_null() {
                        json!({"$case":"null"})
                    } else {
                        json!({"$case":"value","value":value.clone()})
                    };
                }
            }
        }
        if matches!(kind, TrackingToolKind::History) {
            let fields = native.as_object_mut().unwrap();
            let cursor = fields.remove("after_event_id");
            let limit = fields.remove("limit");
            let mut page = serde_json::Map::new();
            if let Some(v) = cursor {
                page.insert("cursor".into(), v);
            }
            if let Some(v) = limit {
                page.insert("limit".into(), v);
            }
            fields.insert("page".into(), Value::Object(page));
        }
        let WipValue::Record(args) = json_to_wip(&native).unwrap() else {
            unreachable!()
        };
        let projection = native_ticket_operation(kind.name()).unwrap();
        assert!(matches!(
            handler
                .call(
                    projection.operation,
                    &args,
                    WipCallContext {
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
        client.grant(operation, ticket("T-A"));
        handler
            .call(
                projection.operation,
                &args,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP rejected allowed tracking operation"));
        assert_eq!(client.requests.lock().unwrap().len(), 2);
    }
}
