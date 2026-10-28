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
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        let args = BTreeMap::from([(
            "filter".into(),
            json_to_wip(&json!({"ticket":"/tickets/T-755"})).unwrap(),
        )]);
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
        if request.path == "/api/w/workspace/authorization/check" {
            let _: server_api::authorization::OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            return Ok(WorkspaceResponse {
                status: 200,
                body: json!({"scope":null}).to_string(),
            });
        }
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
        body: json!({"items":[{"source_id":"notification-source:5","source_event_id":"mr-event:m:e","event_kind":"mr_review","recipient_role":"orchestrator","stage":"acceptance_unknown","legacy_evidence":false,"acceptance_outcome_unknown":true,"target_resolution":"multiple_authorized_targets","retry_candidate":true,"model_processing_confirmed":false}],"next_cursor":null,"source_checks":[]}).to_string(),
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

// Transport fixture for the Feature contract; real Host/store binding is covered
// by the Server router connection test, not claimed by this model of the Host.
#[derive(Debug)]
pub(super) struct BoundClient {
    pub inner: Arc<assignment_tracking::authorization_tests::AuthorizedClient>,
    pub bound: Mutex<Vec<String>>,
}
impl BoundClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: assignment_tracking::authorization_tests::AuthorizedClient::new(json!({
                "items":[],"next_cursor":null,"source_checks":[{
                    "resource_kind":"source_event","reference":"saved","origin_present":true,"captured":true
                }]
            })),
            bound: Mutex::new(vec![]),
        })
    }
}
impl WorkspaceClient for BoundClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "bound-notification-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        mut request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path == "/api/w/workspace/authorization/bound-check" {
            use server_api::authorization::{
                BoundAuthorizationTarget, BoundOperationAuthorizationRequest,
                OperationAuthorizationRequest, PermissionResource,
            };
            let auth: BoundOperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let BoundAuthorizationTarget::NotificationSource { source_event_id } = auth.target
            else {
                panic!("wrong binding")
            };
            self.bound.lock().unwrap().push(source_event_id.clone());
            let job_id = match source_event_id.as_str() {
                "backend-job-notification:hash-A" | "legacy-A" => "Job-A",
                "backend-job-notification:hash-B" | "legacy-B" | "mr-event:legacy-B:e" => "Job-B",
                _ => return Ok(WorkspaceResponse {
                    status: 403,
                    body: json!({"message":"Notification source is unavailable or not authorized"})
                        .to_string(),
                }),
            };
            request.path = "/api/w/workspace/authorization/check".into();
            request.body = Some(
                serde_json::to_string(&OperationAuthorizationRequest {
                    operation: auth.operation,
                    resource: PermissionResource::Job {
                        workspace_id: "workspace".into(),
                        job_id: job_id.into(),
                    },
                })
                .unwrap(),
            );
        }
        self.inner.execute(request)
    }
}
