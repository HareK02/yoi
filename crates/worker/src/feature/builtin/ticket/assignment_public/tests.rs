use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

#[derive(Debug)]
struct Transport {
    requests: Mutex<Vec<WorkspaceRequest>>,
    response: Result<WorkspaceResponse, String>,
    available: bool,
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
        self.available.then_some("workspace")
    }
    fn kind(&self) -> &str {
        "assignment-test"
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request);
        self.response.clone().map_err(WorkspaceClientError::Request)
    }
}
fn transport(status: u16, body: Value) -> Arc<Transport> {
    Arc::new(Transport {
        requests: Mutex::new(vec![]),
        available: true,
        response: Ok(WorkspaceResponse {
            status,
            body: body.to_string(),
        }),
    })
}
pub(super) fn cases() -> Vec<(PublicAssignmentKind, Value, Value)> {
    vec![
        (
            PublicAssignmentKind::Assign,
            json!({"ticket":"T-743", "principal":{"kind":"subject", "subject_id":"subject-a"}, "expected_assignment_id":null, "operation_id":"assign-1"}),
            json!({"workspace_id":"workspace", "ticket_id":"T-743", "assignment":null}),
        ),
        (
            PublicAssignmentKind::Clear,
            json!({"ticket":"T-743", "assignment_id":"delegation", "operation_id":"clear-1"}),
            json!({"workspace_id":"workspace", "ticket_id":"T-743", "assignment":null}),
        ),
        (
            PublicAssignmentKind::Assignments,
            json!({"ticket":"T-743", "after_assignment_id":"old/delegation?", "limit":10}),
            json!({"items":[], "next_assignment_id":null}),
        ),
        (
            PublicAssignmentKind::Diagnostics,
            json!({"ticket":"T-743", "assignment_id":"delegation", "after_input_event_id":"ticket-event:7", "after_start_operation_id":"start/7", "limit":10}),
            json!({"assignment_id":"delegation", "current":true, "input_origin_known":true, "actionable":true, "inputs":[], "next_input_event_id":null, "starts":[], "next_start_operation_id":null}),
        ),
        (
            PublicAssignmentKind::ImplementationResult,
            json!({"ticket":"T-743","kind":"result","body":"Research without an MR","merge_request":null}),
            json!({"result_id":"result-id","ticket_id":"T-743","created_at":"now","request":{"operation_key":"result-1","kind":"result","body":"Research without an MR","merge_request":null}}),
        ),
        (
            PublicAssignmentKind::ImplementationResultQuery,
            json!({"ticket":"T-743","result_id":"result-id"}),
            json!({"result_id":"result-id","ticket_id":"T-743","created_at":"now","request":{"operation_key":"result-1","kind":"result","body":"Research without an MR","merge_request":null}}),
        ),
        (
            PublicAssignmentKind::Answer,
            json!({"ticket":"T-743", "assignment_id":"delegation", "expected_last_event_id":"waiting", "operation_key":"answer-1", "body":"Proceed with option A"}),
            json!({"assignment_id":"delegation", "input_event_id":"ticket-event:8", "event_index":8}),
        ),
    ]
}
// Paired inputs exercise the two public contracts against identical domain requests.
pub(super) fn native_args(kind: PublicAssignmentKind, input: &Value) -> BTreeMap<String, WipValue> {
    let value = match kind {
        PublicAssignmentKind::Assign => {
            let mut principal = input["principal"].clone();
            let case = principal.as_object_mut().unwrap().remove("kind").unwrap();
            json!({"principal":{"$case":case,"value":principal},"observed":{"assignment_id":if input["expected_assignment_id"].is_null(){json!({"$case":"null"})}else{json!({"$case":"value","value":input["expected_assignment_id"]})}},"operation_id":input["operation_id"]})
        }
        PublicAssignmentKind::Clear => {
            json!({"assignment_id":input["assignment_id"],"operation_id":input["operation_id"]})
        }
        PublicAssignmentKind::Assignments => {
            json!({"page":{"cursor":input["after_assignment_id"],"limit":input["limit"]}})
        }
        PublicAssignmentKind::Diagnostics => {
            json!({"assignment_id":input["assignment_id"],"page":{"after_input_event_id":input["after_input_event_id"],"after_start_operation_id":input["after_start_operation_id"],"limit":input["limit"]}})
        }
        PublicAssignmentKind::Answer => {
            json!({"observed":{"assignment_id":input["assignment_id"],"expected_last_event_id":input["expected_last_event_id"]},"operation_key":input["operation_key"],"body":input["body"]})
        }
        PublicAssignmentKind::ImplementationResult => {
            json!({"kind":input["kind"],"body":input["body"]})
        }
        PublicAssignmentKind::ImplementationResultQuery => json!({"result_id":input["result_id"]}),
    };
    match json_to_wip(&value).unwrap() {
        WipValue::Record(fields) => fields,
        _ => unreachable!(),
    }
}
#[tokio::test]
async fn public_assignment_schemas_require_observations_and_exclude_model_actor_claims() {
    for (kind, input, response) in cases() {
        let client = transport(200, response);
        let (meta, tool) = definition(client.clone(), kind)();
        let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
        assert!(
            schema.is_valid(&input),
            "{}: {}",
            meta.name,
            meta.input_schema
        );
        for field in meta.input_schema["required"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let mut missing = input.clone();
            missing
                .as_object_mut()
                .unwrap()
                .remove(field.as_str().unwrap());
            assert!(!schema.is_valid(&missing), "{} missing {field}", meta.name);
            assert!(matches!(
                tool.execute(&missing.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
        for actor_claim in [
            "actor",
            "author",
            "role",
            "runtime_id",
            "worker_id",
            "authentication_binding_id",
        ] {
            let mut forged = input.clone();
            forged[actor_claim] = json!("authority");
            assert!(
                !schema.is_valid(&forged),
                "{} accepts {actor_claim}",
                meta.name
            );
            assert!(matches!(
                tool.execute(&forged.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
        assert!(client.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn new_assignment_rejects_retained_legacy_principals_before_transport() {
    for principal in [
        json!({"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}),
        json!({"kind":"user", "account_id":"owner"}),
        json!({"kind":"worker", "runtime_id":"r", "worker_id":"old-body"}),
    ] {
        let client = transport(200, json!({}));
        let (_, tool) = definition(client.clone(), PublicAssignmentKind::Assign)();
        let input = json!({"ticket":"T-743", "principal":principal,
            "expected_assignment_id":null, "operation_id":"legacy-is-not-new-authority"});
        assert!(matches!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
        assert!(client.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn assign_reissue_and_clear_forward_observed_identity_and_exact_retry_without_new_key() {
    for (kind, mut input, response) in cases().into_iter().filter(|(kind, _, _)| {
        matches!(
            kind,
            PublicAssignmentKind::Assign | PublicAssignmentKind::Clear
        )
    }) {
        if matches!(kind, PublicAssignmentKind::Assign) {
            input["expected_assignment_id"] = json!("previous-done");
        }
        let client = transport(200, response);
        let (_, tool) = definition(client.clone(), kind)();
        for _ in 0..2 {
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        assert!(
            requests[0]
                .path
                .starts_with("/api/w/workspace/tickets/T-743/assignments/orchestrator")
        );
        match kind {
            PublicAssignmentKind::Assign => {
                assert_eq!(requests[0].method, WorkspaceRequestMethod::Put);
                let body: Value =
                    serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
                assert_eq!(body["expected_assignment_id"], "previous-done");
                assert_eq!(body["operation_id"], input["operation_id"]);
                assert_eq!(body["principal"], input["principal"]);
                assert!(body.get("actor").is_none());
                assert!(body.get("orchestrator_assignment_id").is_none());
                assert_eq!(body["workdir_bindings"], json!([]));
            }
            PublicAssignmentKind::Clear => {
                assert_eq!(requests[0].method, WorkspaceRequestMethod::Delete);
                assert!(requests[0].path.contains("assignment_id=delegation"));
                assert!(requests[0].path.contains("operation_id=clear-1"));
                assert!(requests[0].body.is_none());
            }
            _ => unreachable!(),
        }
    }
}
#[tokio::test]
async fn implementation_result_schema_exposes_only_report_facts_and_rejects_forged_fields() {
    let (kind, input, response) = cases().remove(4);
    let client = transport(200, response);
    let (meta, tool) = definition(client.clone(), kind)();
    let properties = meta.input_schema["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 4);
    for field in ["ticket", "kind", "body", "merge_request"] {
        assert!(properties.contains_key(field), "missing {field}");
    }
    assert_eq!(meta.input_schema["additionalProperties"], false);
    let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
    let mut minimal = input.clone();
    minimal.as_object_mut().unwrap().remove("merge_request");
    assert!(schema.is_valid(&minimal));
    assert!(serde_json::from_value::<ImplementationResultInput>(minimal).is_ok());
    for field in [
        "operation_key",
        "assignment_id",
        "orchestrator_assignment_id",
        "result_id",
        "call_id",
        "request",
        "actor",
        "runtime_id",
        "worker_id",
    ] {
        let mut forged = input.clone();
        forged[field] = json!("forged");
        assert!(!schema.is_valid(&forged), "schema accepts {field}");
        assert!(serde_json::from_value::<ImplementationResultInput>(forged.clone()).is_err());
        assert!(matches!(
            tool.execute(&forged.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    assert!(client.requests.lock().unwrap().is_empty());
    let query = PublicAssignmentKind::ImplementationResultQuery.schema();
    assert!(query["properties"].get("result_id").is_some());
    assert!(
        query["required"]
            .as_array()
            .unwrap()
            .contains(&json!("result_id"))
    );
}

#[tokio::test]
async fn implementation_result_transport_key_uses_only_trusted_invocation_identity() {
    let (kind, input, response) = cases().remove(4);
    let client = transport(200, response);
    let (_, tool) = definition(client.clone(), kind)();
    for ctx in [
        ToolExecutionContext::new("call-a", "batch-a", 0),
        ToolExecutionContext::new("call-a", "batch-b", 7),
        ToolExecutionContext::new("call-b", "batch-a", 0),
    ] {
        tool.execute(&input.to_string(), ctx).await.unwrap();
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0], requests[1]);
    assert_ne!(requests[0], requests[2]);
    for (request, call_id) in requests.iter().zip(["call-a", "call-a", "call-b"]) {
        assert_eq!(request.method, WorkspaceRequestMethod::Post);
        assert!(
            request
                .path
                .ends_with("/tickets/T-743/implementation-results")
        );
        let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let mut expected = input.clone();
        expected.as_object_mut().unwrap().remove("ticket");
        expected["operation_key"] = json!(format!("ticket-implementation-result:T-743:{call_id}"));
        assert_eq!(body, expected);
        assert!(body.get("assignment_id").is_none());
        assert!(body.get("orchestrator_assignment_id").is_none());
    }
}

#[tokio::test]
async fn implementation_result_missing_or_invalid_context_fails_before_dispatch() {
    let (kind, input, response) = cases().remove(4);
    let client = transport(200, response);
    let (_, tool) = definition(client.clone(), kind)();
    for call_id in [
        String::new(),
        " \t".into(),
        "call\nforged".into(),
        "x".repeat(1025),
    ] {
        let error = tool
            .execute(
                &input.to_string(),
                ToolExecutionContext::new(call_id, "batch", 0),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgument(_)));
        assert!(error.to_string().contains("tool call_id"));
    }
    // A valid call ID must also fit the bounded transport key after namespacing.
    assert!(matches!(
        tool.execute(
            &input.to_string(),
            ToolExecutionContext::new("x".repeat(1024), "batch", 0),
        )
        .await,
        Err(ToolError::InvalidArgument(_))
    ));
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn implementation_result_validates_report_body_ticket_kind_and_optional_mr() {
    let (kind, input, response) = cases().remove(4);
    let client = transport(200, response);
    let (meta, tool) = definition(client.clone(), kind)();
    let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
    let mut invalid_inputs = vec![];
    for body in [
        String::new(),
        " \n".into(),
        "x".repeat(16385),
        "text\0hidden".into(),
        "é".repeat(8193),
    ] {
        let mut invalid = input.clone();
        invalid["body"] = json!(body);
        invalid_inputs.push(invalid);
    }
    for value in [String::new(), " \t".into(), "x\n".into(), "x".repeat(1025)] {
        let mut invalid = input.clone();
        invalid["ticket"] = json!(value);
        invalid_inputs.push(invalid);
        for field in ["merge_request_id", "subject_ref"] {
            let mut invalid = input.clone();
            invalid["merge_request"] = json!({"merge_request_id":"MR-1","subject_ref":"source"});
            invalid["merge_request"][field] = json!(value);
            invalid_inputs.push(invalid);
        }
    }
    for value in [json!("done"), json!(null), json!(42)] {
        let mut invalid = input.clone();
        invalid["kind"] = value;
        assert!(!schema.is_valid(&invalid));
        invalid_inputs.push(invalid);
    }
    for mr in [
        json!({"merge_request_id":"MR-1"}),
        json!({"merge_request_id":"MR-1","subject_ref":"source","operation_key":"forged"}),
    ] {
        let mut invalid = input.clone();
        invalid["merge_request"] = mr;
        assert!(!schema.is_valid(&invalid));
        invalid_inputs.push(invalid);
    }
    for invalid in invalid_inputs {
        assert!(matches!(
            tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    assert!(client.requests.lock().unwrap().is_empty());
    for (index, report_kind) in ["handoff", "result", "blocker"].into_iter().enumerate() {
        let mut valid = input.clone();
        valid["kind"] = json!(report_kind);
        valid["body"] = json!("é".repeat(8192));
        if index == 0 {
            valid.as_object_mut().unwrap().remove("merge_request");
        } else if index == 2 {
            valid["merge_request"] = json!({"merge_request_id":"MR-1","subject_ref":"source"});
        }
        assert!(schema.is_valid(&valid));
        tool.execute(
            &valid.to_string(),
            ToolExecutionContext::new(format!("call-{index}"), "batch", index),
        )
        .await
        .unwrap();
    }
    assert_eq!(client.requests.lock().unwrap().len(), 3);
}

#[test]
fn implementation_result_guidance_keeps_retry_identity_host_owned() {
    let description = description("TicketImplementationResult");
    assert!(description.contains("authenticated active Workspace Worker"));
    assert!(description.contains("Host owns transport retry identity"));
    assert!(description.contains("a new invocation is a new report"));
    assert!(description.contains("rather than blindly submitting"));
    assert!(!description.contains("operation_key"));
    assert!(!description.contains("fences both assignments"));
}

#[tokio::test]
async fn continuing_answer_forwards_same_assignment_event_cas_and_saved_receipt() {
    let (kind, input, response) = cases().pop().unwrap();
    let client = transport(200, response.clone());
    let (_, tool) = definition(client.clone(), kind)();
    for _ in 0..2 {
        let output = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap(),
            response
        );
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests[0], requests[1]);
    assert_eq!(
        requests[0].path,
        "/workspaces/workspace/tickets/T-743/assignment-answer"
    );
    let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    let mut expected = input;
    expected.as_object_mut().unwrap().remove("ticket");
    assert_eq!(body, expected);
    assert!(body.get("tracking").is_none());
}
#[tokio::test]
async fn assignment_pages_forward_independent_encoded_cursors_and_reject_unbounded_input() {
    for (kind, input, response) in cases().into_iter().filter(|(kind, _, _)| {
        matches!(
            kind,
            PublicAssignmentKind::Assignments | PublicAssignmentKind::Diagnostics
        )
    }) {
        let client = transport(200, response);
        let (_, tool) = definition(client.clone(), kind)();
        for limit in [0, 101] {
            let mut invalid = input.clone();
            invalid["limit"] = json!(limit);
            assert!(matches!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
        assert!(requests[0].path.contains("limit=10"));
        match kind {
            PublicAssignmentKind::Assignments => assert!(
                requests[0]
                    .path
                    .contains("after_assignment_id=old%2Fdelegation%3F")
            ),
            PublicAssignmentKind::Diagnostics => {
                assert!(
                    requests[0]
                        .path
                        .contains("after_input_event_id=ticket-event%3A7")
                );
                assert!(
                    requests[0]
                        .path
                        .contains("after_start_operation_id=start%2F7")
                );
            }
            _ => unreachable!(),
        }
    }
}
#[tokio::test]
async fn public_assignment_rejections_and_unknown_outcomes_never_auto_retry_or_replace_identity() {
    for (kind, input, _) in cases() {
        for response in [
            Ok(WorkspaceResponse {
                status: 403,
                body: "permission denied".into(),
            }),
            Ok(WorkspaceResponse {
                status: 409,
                body: "stale assignment or operation fingerprint mismatch".into(),
            }),
            Ok(WorkspaceResponse {
                status: 200,
                body: "invalid response after dispatch".into(),
            }),
            Err("response lost; outcome unknown".into()),
        ] {
            let client = Arc::new(Transport {
                requests: Mutex::new(vec![]),
                available: true,
                response,
            });
            let (_, tool) = definition(client.clone(), kind)();
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert_eq!(
                client.requests.lock().unwrap().len(),
                1,
                "{} retried",
                kind.name()
            );
        }
        let client = Arc::new(Transport {
            requests: Mutex::new(vec![]),
            available: false,
            response: Err("unavailable".into()),
        });
        let (_, tool) = definition(client.clone(), kind)();
        assert!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap_err()
                .to_string()
                .contains("Backend Workspace identity")
        );
        assert!(client.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn diagnostics_strip_undeclared_content_and_preserve_unknown_processing() {
    let (kind, input, mut response) = cases().remove(3);
    response["rendered_content"] = json!("must not reach model");
    response["authentication_binding_id"] = json!("must not reach model");
    response["inputs"] = json!([{"input_event_id":"ticket-event:7", "delivery_id":null, "notification":"not_reserved", "acceptance_outcome_unknown":true, "rendered_content":"hidden"}]);
    response["starts"] = json!([{"operation_id":"start", "claim_state":"reserved", "assignment_id":null, "runtime_id":null, "worker_id":null, "actual_processing_unknown":true, "authentication_binding_id":"hidden"}]);
    let (_, tool) = definition(transport(200, response), kind)();
    let output = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let output: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert!(output.get("rendered_content").is_none());
    assert!(output.get("authentication_binding_id").is_none());
    assert_eq!(output["inputs"][0]["acceptance_outcome_unknown"], true);
    assert!(output["inputs"][0].get("rendered_content").is_none());
    assert_eq!(output["starts"][0]["actual_processing_unknown"], true);
    assert!(
        output["starts"][0]
            .get("authentication_binding_id")
            .is_none()
    );
}
#[test]
fn public_assignment_normal_and_wip_access_excludes_mutations_for_read_only_and_review() {
    for access in [
        TicketFeatureAccess::read_only(),
        TicketFeatureAccess::review(),
    ] {
        let client = transport(200, json!({}));
        let normal = enabled_ticket_definitions(client.clone(), access);
        let native = native_ticket_tools(client, access).unwrap();
        for name in READ_TOOL_NAMES {
            assert!(normal.iter().any(|d| d().0.name == *name));
            assert!(native.iter().any(|t| t.name == *name));
        }
        for name in [
            "TicketOrchestratorAssign",
            "TicketOrchestratorClear",
            "TicketAssignmentAnswer",
        ] {
            assert!(!normal.iter().any(|d| d().0.name == name));
            assert!(!native.iter().any(|t| t.name == name));
        }
    }
    for name in TOOL_NAMES {
        let native = native_ticket_operation(name).unwrap();
        assert_eq!(native.identity_field, Some("ticket"));
        assert_eq!(native.mutating, !READ_TOOL_NAMES.contains(name));
    }
}
#[tokio::test]
async fn ticket_query_closure_reason_is_typed_and_not_a_classification_tag() {
    let client = transport(
        200,
        json!({"items":[],"page":{"has_more":false,"next_cursor":null}}),
    );
    let (meta, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
    let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
    let input = json!({"states":["closed"],"closure_reason":"completed","tag_ids":["tag-bug"]});
    assert!(schema.is_valid(&input));
    let invalid = json!({"closure_reason":"bug"});
    assert!(!schema.is_valid(&invalid));
    assert!(matches!(
        tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
            .await,
        Err(ToolError::InvalidArgument(_))
    ));
    tool.execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["closure_reason"], "completed");
    assert_eq!(body["tag_ids"], json!(["tag-bug"]));
}

#[tokio::test]
async fn ticket_query_forwards_current_principal_and_self_reported_handling_filters() {
    let client = transport(
        200,
        json!({"items":[],"page":{"has_more":false,"next_cursor":null}}),
    );
    let (meta, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
    let schema = jsonschema::validator_for(&meta.input_schema).unwrap();
    let input = json!({"assigned_principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"},"handling_status":"waiting","states":["open"],"limit":5});
    assert!(schema.is_valid(&input));
    for status in ["running", "idle", "stopped", "closed"] {
        let mut invalid = input.clone();
        invalid["handling_status"] = json!(status);
        assert!(!schema.is_valid(&invalid));
        assert!(matches!(
            tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    tool.execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["assigned_principal"], input["assigned_principal"]);
    assert_eq!(body["handling_status"], "waiting");
    assert_eq!(body["states"], json!(["open"]));
}

#[test]
fn resolved_builtin_profiles_expose_assignment_capabilities_without_repo_or_mr_prerequisites() {
    let dir = tempfile::tempdir().unwrap();
    for profile in [
        "intake",
        "orchestrator",
        "coder",
        "ticket-worker",
        "reviewer",
    ] {
        let resolved = manifest::ProfileResolver::new()
            .with_workspace_base(dir.path())
            .resolve(
                &manifest::ProfileSelector::parse_cli(&format!("builtin:{profile}")),
                manifest::ProfileResolveOptions::with_worker_name("assignment-fixture"),
            )
            .unwrap();
        let config = resolved.manifest.feature.ticket;
        assert!(config.enabled, "{profile}");
        let delegates = config.authoring || config.workflow;
        let answers = delegates || config.thread;
        let reports = config.workflow;
        let access = TicketFeatureAccess {
            authoring: config.authoring,
            thread: config.thread,
            intake: config.intake,
            workflow: config.workflow,
        };
        let names = access.tool_names();
        assert_eq!(
            names.contains(&"TicketOrchestratorAssign"),
            delegates,
            "{profile}"
        );
        assert_eq!(
            names.contains(&"TicketOrchestratorClear"),
            delegates,
            "{profile}"
        );
        assert_eq!(
            names.contains(&"TicketAssignmentAnswer"),
            answers,
            "{profile}"
        );
        assert_eq!(
            names.contains(&"TicketAssignmentTrackingUpdate"),
            reports,
            "{profile}"
        );
        assert!(
            names.contains(&"TicketAssignmentsQuery")
                && names.contains(&"TicketAssignmentDiagnostics"),
            "{profile}"
        );
        assert!(
            !names.iter().any(|name| matches!(
                *name,
                "TicketQueue" | "TicketMarkReady" | "TicketIntakeReady"
            )),
            "{profile}"
        );
    }
}

#[tokio::test]
async fn invalid_answer_body_and_assignment_identities_fail_before_dispatch() {
    let (kind, input, response) = cases().pop().unwrap();
    let client = transport(200, response);
    let (_, tool) = definition(client.clone(), kind)();
    for body in [
        String::new(),
        " ".into(),
        "x".repeat(16385),
        "text\0hidden".into(),
    ] {
        let mut invalid = input.clone();
        invalid["body"] = json!(body);
        assert!(matches!(
            tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    for field in ["assignment_id", "expected_last_event_id", "operation_key"] {
        for value in [String::new(), "x\n".into(), "x".repeat(1025)] {
            let mut invalid = input.clone();
            invalid[field] = json!(value);
            assert!(matches!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn implementation_result_wip_and_native_dispatch_preserve_trusted_call_context() {
    let (kind, input, response) = cases().remove(4);
    let client = transport(200, response);
    let (_, direct_tool) = definition(client.clone(), kind)();
    direct_tool
        .execute(
            &input.to_string(),
            ToolExecutionContext::new("call-a", "native", 0),
        )
        .await
        .unwrap();
    let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
    let handler = TicketItemWipHandler {
        tools: operation_map(
            tools
                .into_iter()
                .filter(|t| t.projection.surface == NativeTicketSurface::Item)
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
    for call_id in ["call-a", "call-b"] {
        handler
            .call(
                "implementation_result",
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::new(call_id, "wip", 3),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP implementation_result rejected {call_id}"));
    }
    {
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0], requests[1]);
        assert_ne!(requests[1], requests[2]);
        let body: Value = serde_json::from_str(requests[2].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body["operation_key"],
            "ticket-implementation-result:T-743:call-b"
        );
    }
    assert!(matches!(
        handler
            .call(
                "implementation_result",
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::new("", "wip", 0),
                    security_context: "worker".into(),
                },
            )
            .await,
        Err(WipOperationError::Protocol(ProtocolError {
            code: ProtocolErrorCode::InvalidArguments,
            ..
        }))
    ));
    for field in [
        "operation_key",
        "assignment_id",
        "orchestrator_assignment_id",
        "call_id",
    ] {
        let mut forged = args.clone();
        forged.insert(field.into(), WipValue::String("forged".into()));
        assert!(matches!(
            handler
                .call(
                    "implementation_result",
                    &forged,
                    WipCallContext {
                        result_namespaces: Default::default(),
                        execution: ToolExecutionContext::new("call-c", "wip", 0),
                        security_context: "worker".into(),
                    },
                )
                .await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InvalidArguments,
                ..
            }))
        ));
    }
    assert_eq!(client.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn assignment_wip_binds_ticket_and_preserves_normal_permission_and_cas_inputs() {
    for (kind, mut input, response) in cases() {
        let client = transport(200, response);
        let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
        let handler = TicketItemWipHandler {
            tools: operation_map(
                tools
                    .into_iter()
                    .filter(|t| t.projection.surface == NativeTicketSurface::Item)
                    .collect(),
            ),
            permissions: None,
            ticket_reference: "T-743".into(),
            collection_route: "/tickets".into(),
            merge_request_route: "/merge-requests".into(),
            objective_route: "/objectives".into(),
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        input.as_object_mut().unwrap().remove("ticket");
        let args = native_args(kind, &input);
        let operation = native_ticket_operation(kind.name()).unwrap().operation;
        handler
            .call(
                operation,
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .unwrap_or_else(|_| panic!("WIP {operation} rejected"));
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let mut forged = args.clone();
        forged.insert("ticket".into(), WipValue::String("T-other".into()));
        assert!(
            handler
                .call(
                    operation,
                    &forged,
                    WipCallContext {
                        result_namespaces: Default::default(),
                        execution: ToolExecutionContext::direct(),
                        security_context: "worker".into()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
    let (_, input, response) = cases().pop().unwrap();
    let client = transport(200, response);
    let tools = native_ticket_tools(client.clone(), TicketFeatureAccess::workflow()).unwrap();
    let denied = TicketItemWipHandler {
        tools: operation_map(
            tools
                .into_iter()
                .filter(|t| t.projection.surface == NativeTicketSurface::Item)
                .collect(),
        ),
        permissions: Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: vec![],
        }),
        ticket_reference: "T-743".into(),
        collection_route: "/tickets".into(),
        merge_request_route: "/merge-requests".into(),
        objective_route: "/objectives".into(),
        observations: Arc::new(Mutex::new(TicketObservationState::default())),
    };
    let args = native_args(PublicAssignmentKind::Answer, &input);
    assert!(matches!(
        denied
            .call(
                "assignment_answer",
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
}
