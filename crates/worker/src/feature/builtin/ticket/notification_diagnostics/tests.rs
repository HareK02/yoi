use super::*;

#[tokio::test]
async fn notification_wip_executes_same_read_and_honors_profile_denial() {
    for denied in [false, true] {
        let client = Arc::new(Client {
            requests: Mutex::new(vec![]),
            status: 200,
            body: json!({"items":[],"next_cursor":null,"source_checks":[]}).to_string(),
        });
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::read_only()).unwrap();
        let handler = TicketCollectionWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|t| t.projection.surface == NativeTicketSurface::Collection)
                    .collect(),
            ),
            permissions: denied.then_some(ToolPermissionConfig {
                default_action: ToolPermissionAction::Deny,
                rules: vec![],
            }),
            collection_route: "/tickets".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let args = BTreeMap::from([("ticket".into(), WipValue::String("T-755".into()))]);
        let result = handler
            .call(
                "notification_diagnostics",
                &args,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await;
        assert_eq!(result.is_err(), denied);
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), usize::from(!denied));
        if !denied {
            assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
            assert_eq!(
                requests[0].path,
                "/api/w/workspace/notification-diagnostics?ticket=T-755"
            );
        }
    }
}
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
#[derive(Debug)]
struct Client {
    requests: Mutex<Vec<WorkspaceRequest>>,
    status: u16,
    body: String,
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "notification-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request);
        Ok(WorkspaceResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn notification_tool_and_wip_share_bounded_read_contract() {
    let client = Arc::new(Client {
        requests: Mutex::new(vec![]),
        status: 200,
        body: json!({"items":[{"source_id":"notification-source:5","source_event_id":"mr-event:m:e","event_kind":"mr_review","recipient_role":"orchestrator","stage":"acceptance_unknown","acceptance_outcome_unknown":true,"target_resolution":"multiple_authorized_targets","retry_candidate":true,"model_processing_confirmed":false}],"next_cursor":null,"source_checks":[]}).to_string(),
    });
    let (meta, tool) = definition(client.clone())();
    let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
    let input = json!({"ticket":"T-755","merge_request_id":"mr/1?","job_id":"job","cursor":"notification-source:4","limit":10});
    assert!(schema.is_valid(&input));
    let output = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let page: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(page["items"][0]["acceptance_outcome_unknown"], true);
    assert_eq!(
        page["items"][0]["target_resolution"],
        "multiple_authorized_targets"
    );
    assert!(page["items"][0]["target_worker_id"].is_null());
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
    assert!(
        requests[0]
            .path
            .starts_with("/api/w/workspace/notification-diagnostics?")
    );
    assert!(requests[0].path.contains("merge_request_id=mr%2F1%3F"));
    assert!(requests[0].body.is_none());
    drop(requests);
    for bad in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"job_id":""}),
        json!({"source_event_id":"x".repeat(513)}),
        json!({"worker_id":"forged"}),
        json!({"workspace_id":"other"}),
    ] {
        assert!(
            tool.execute(&bad.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
    }
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let projection = native_ticket_operation(NAME).unwrap();
    assert_eq!(projection.operation, "notification_diagnostics");
    assert!(!projection.mutating);
    assert!(projection.identity_field.is_none());
    for access in [
        TicketFeatureAccess::read_only(),
        TicketFeatureAccess::workspace_authoring(),
        TicketFeatureAccess::workflow(),
    ] {
        assert!(access.tool_names().contains(&NAME));
    }
}
#[tokio::test]
async fn notification_tool_rejects_raw_or_undeclared_backend_content() {
    for (status, body) in [
        (403, "SECRET-ERROR".to_string()),
        (
            200,
            json!({"items":[],"next_cursor":null,"source_checks":[],"credential":"SECRET"})
                .to_string(),
        ),
        (200, "not-json-SECRET".into()),
    ] {
        let (_, tool) = definition(Arc::new(Client {
            requests: Mutex::new(vec![]),
            status,
            body,
        }))();
        let error = tool
            .execute("{}", ToolExecutionContext::direct())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("SECRET"));
    }
}
