use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

#[derive(Debug)]
struct Transport {
    requests: Mutex<Vec<WorkspaceRequest>>,
    response: Result<WorkspaceResponse, String>,
    available: bool,
}
impl WorkspaceClient for Transport {
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
fn cases() -> Vec<(PublicAssignmentKind, Value, Value)> {
    vec![
        (
            PublicAssignmentKind::Assign,
            json!({"ticket":"T-743", "principal":{"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}, "expected_assignment_id":null, "operation_id":"assign-1"}),
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
            PublicAssignmentKind::Answer,
            json!({"ticket":"T-743", "assignment_id":"delegation", "expected_last_event_id":"waiting", "operation_key":"answer-1", "body":"Proceed with option A"}),
            json!({"assignment_id":"delegation", "input_event_id":"ticket-event:8", "event_index":8}),
        ),
    ]
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
            observations: Arc::new(Mutex::new(TicketObservationState::default())),
        };
        input.as_object_mut().unwrap().remove("ticket");
        let args: BTreeMap<_, _> = input
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), json_to_wip(value).unwrap()))
            .collect();
        let operation = native_ticket_operation(kind.name()).unwrap().operation;
        handler
            .call(
                operation,
                &args,
                WipCallContext {
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
        observations: Arc::new(Mutex::new(TicketObservationState::default())),
    };
    let args = input
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.as_str() != "ticket")
        .map(|(k, v)| (k.clone(), json_to_wip(v).unwrap()))
        .collect();
    assert!(matches!(
        denied
            .call(
                "assignment_answer",
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
    assert!(client.requests.lock().unwrap().is_empty());
}
