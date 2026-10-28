// Model-facing integration: production Inspect, official Text View, Invoke, native conversion,
// shared domain authorization/forwarding and output projection. Only the Workspace transport is
// scripted; this test does not claim persistence or server-side policy evaluation.
#[tokio::test]
async fn ticket_inspect_contract_drives_query_read_and_comment_without_tool_schema() {
    let detail = json!({
        "resource_key":"T-42", "title":"Intent", "body":"Requirements", "body_truncated":false,
        "content_digest":"observed", "state":"open", "targets":[], "events":[], "linked_objectives":[],
        "initial_post":{"sequence":0,"event_ref":"initial","is_initial":true,"kind":"comment","body":"Requirements","body_truncated":false,"body_digest":"a".repeat(64),"editable":true,"author":null,"at":null,"edited_at":null,"edited_by":null},
        "closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"closure_history":[],
        "assignment_tracking":[],"tags":[],"assignments":[],"implementation_reports":[],"merge_requests":[]
    });
    let query_item = json!({"resource_key":"T-42","title":"Intent","state":"open","closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"assignment_tracking":[],"tags":[],"matched_fields":[],"linked_objective_keys":[],"relation_count":0,"blocker_count":0,"unresolved_blocker_count":0,"unresolved_review_count":0,"merge_requests":[]});
    let client = Arc::new(
        ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"items":[query_item],"page":{"next_cursor":null,"has_more":false}})
                    .to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: detail.to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 204,
                body: String::new(),
            },
        ])
        .with_authorizations([
            expected_authorization("ticket.query", "workspace", None),
            expected_authorization("ticket.read", "ticket", Some(("ticket_id", "T-42"))),
            expected_authorization("ticket.event.add", "ticket", Some(("ticket_id", "T-42"))),
        ]),
    );
    let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
    let mut registry = WipMountRegistry::new();
    let namespace = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::workspace_authoring(),
        None,
        &namespace,
    )
    .unwrap();
    let runtime =
        install_wip_mode_with_mounts(&mut engine, None, "model".into(), registry).unwrap();
    async fn inspect(runtime: &WipRuntime, path: &str) -> Json {
        serde_json::from_str(
            &runtime
                .inspect(path.into(), false)
                .await
                .unwrap()
                .content
                .unwrap(),
        )
        .unwrap()
    }
    let object = inspect(&runtime, "/cases").await;
    let interface = &object["interfaces"][0];
    let declaration = inspect(&runtime, interface["path"].as_str().unwrap()).await;
    let signature = declaration["interface_signature"].as_str().unwrap();
    assert!(signature.contains("query("));
    assert!(signature.contains("filter?"));
    assert!(signature.contains("`entry`: entry"), "{signature}");
    assert!(signature.contains("data: json"));
    assert!(signature.contains("next_cursor?: string"));
    assert!(signature.contains("states?"));
    assert!(signature.contains("open"));
    assert!(!signature.contains("Tool schema"));
    let reference = InterfaceReference {
        scope: interface["reference"]["scope"].as_str().unwrap().into(),
        name: interface["reference"]["name"].as_str().unwrap().into(),
    };
    let output = runtime
        .invoke(
            "/cases".into(),
            reference,
            "query".into(),
            json!({"filter":{"states":["open"]},"page":{"limit":3}}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let output: Json = serde_json::from_str(&output.content.unwrap()).unwrap();
    let path = output["items"][0]["entry"].as_str().unwrap();
    assert_eq!(path, "/cases/T-42");
    let object = inspect(&runtime, path).await;
    let interface = &object["interfaces"][0];
    let declaration = inspect(&runtime, interface["path"].as_str().unwrap()).await;
    let signature = declaration["interface_signature"].as_str().unwrap();
    assert!(signature.contains("read("));
    assert!(signature.contains("page?"));
    assert!(signature.contains("comment("));
    assert!(signature.contains("body: string"));
    assert!(!signature.contains("ticket: string"));
    let reference = InterfaceReference {
        scope: interface["reference"]["scope"].as_str().unwrap().into(),
        name: interface["reference"]["name"].as_str().unwrap().into(),
    };
    runtime
        .invoke(
            path.into(),
            reference.clone(),
            "read".into(),
            json!({"page":{"limit":2}}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    runtime.inspect(path.into(), true).await.unwrap();
    runtime
        .invoke(
            path.into(),
            reference,
            "comment".into(),
            json!({"body":"Confirmed requirement"}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    client.assert_authorizations_consumed();
    let requests = client.requests();
    assert_eq!(requests.len(), 3);
    let query: Json = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(query["states"], json!(["open"]));
    assert_eq!(query["limit"], 3);
    let read: Json = serde_json::from_str(requests[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(read["event_limit"], 2);
    assert!(requests[2].path.ends_with("/tickets/T-42/thread-events"));
}

#[test]
fn native_namespace_reference_lookup_is_explicit_unique_and_not_publication() {
    let mut registry = WipMountRegistry::new();
    assert_eq!(registry.namespace_root("ticket"), None);
    registry.allocate_namespace("ticket", "cases").unwrap();
    assert_eq!(registry.namespace_root("ticket"), Some("/cases"));
    assert!(
        registry.routes().next().is_none(),
        "allocation does not publish a provider"
    );
    registry
        .allocate_namespace("ticket", "other-cases")
        .unwrap();
    assert_eq!(
        registry.namespace_root("ticket"),
        None,
        "ambiguous input coordinates fail closed"
    );
}

#[tokio::test]
async fn large_ticket_interface_is_explicitly_bounded_and_selected_operation_remains_invokable() {
    let client = Arc::new(
        ScriptedWorkspaceClient::new([crate::worker::WorkspaceResponse {
            status: 204,
            body: String::new(),
        }])
        .with_authorizations([expected_authorization(
            "ticket.event.add",
            "ticket",
            Some(("ticket_id", "T-42")),
        )]),
    );
    let mut registry = WipMountRegistry::new();
    let namespace = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess {
            authoring: true,
            thread: true,
            intake: true,
            workflow: true,
        },
        None,
        &namespace,
    )
    .unwrap();
    let mut runtime = WipRuntime::from_mounts(registry, "selection".into()).unwrap();
    let object: Json = serde_json::from_str(
        &runtime
            .inspect("/cases/T-42".into(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap();
    let shown = &object["interfaces"][0];
    let address = shown["path"].as_str().unwrap();
    let reference = InterfaceReference {
        scope: shown["reference"]["scope"].as_str().unwrap().into(),
        name: shown["reference"]["name"].as_str().unwrap().into(),
    };
    let complete = runtime.inspect(address.into(), false).await.unwrap();
    assert!(complete.content.unwrap().len() > 1800);
    runtime.wire_limits = Limits::new(2 * 1024 * 1024, 1800, 128).unwrap();
    let error = runtime
        .inspect(address.into(), false)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no partial signature returned"), "{error}");
    let names = error
        .split("Available operations (complete index): ")
        .nth(1)
        .unwrap();
    let names: Vec<String> = serde_json::from_str(names).unwrap();
    assert!(names.iter().any(|name| name == "comment"));
    assert!(names.iter().any(|name| name == "case_update"));
    let selected = runtime
        .inspect_operation(address.into(), false, Some("comment"))
        .await
        .unwrap();
    let selected: Json = serde_json::from_str(&selected.content.unwrap()).unwrap();
    assert_eq!(
        selected["coverage"],
        json!({"operation":"comment","complete_interface":false,"complete_operation":true})
    );
    let signature = selected["interface_signature"].as_str().unwrap();
    assert!(signature.contains("comment("));
    assert!(signature.contains("body: string"));
    assert!(!signature.contains("case_update("));
    assert!(
        runtime
            .inspect_operation("/cases/T-42".into(), false, Some("comment"))
            .await
            .is_err()
    );
    assert!(
        runtime
            .inspect_operation(address.into(), false, Some("missing"))
            .await
            .is_err()
    );
    let projection = runtime.host.projection("/cases/T-42").unwrap();
    assert!(
        projection
            .descriptor
            .operations
            .iter()
            .any(|op| op.name == "case_update"),
        "selection must not replace the call descriptor"
    );
    runtime
        .invoke(
            "/cases/T-42".into(),
            reference,
            "comment".into(),
            json!({"body":"Bounded contract"}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(client.requests().len(), 1);
    client.assert_authorizations_consumed();
}

// The transport adapter applies the real SQLite query to an isolated multi-kind fixture.
// Authorization is scripted; the native conversion, shared command, HTTP request and persisted
// filter semantics are production paths. This does not claim Server authentication coverage.
#[tokio::test]
async fn native_plan_kind_reaches_persisted_query_through_inspect_and_dispatch() {
    use crate::worker::{
        WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse,
    };
    use ticket::{
        AcceptedOrchestrationPlan, NewOrchestrationPlanRecord, NewTicket, OrchestrationPlanKind,
        SqliteTicketBackend, TicketBackend as _, TicketIdOrSlug,
    };
    #[derive(Debug)]
    struct PersistedPlans {
        backend: SqliteTicketBackend,
        ticket: String,
        requests: Mutex<Vec<Json>>,
    }
    impl WorkspaceClient for PersistedPlans {
        fn workspace_id(&self) -> Option<&str> {
            Some("plan-fixture")
        }
        fn kind(&self) -> &str {
            "persisted-plan-filter"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if request.path.ends_with("/authorization/check") {
                let check: server_api::authorization::OperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(check.operation, "ticket.orchestration_plan.query");
                assert_eq!(
                    check.resource,
                    server_api::authorization::PermissionResource::Ticket {
                        workspace_id: "plan-fixture".into(),
                        ticket_id: self.ticket.clone()
                    }
                );
                return Ok(WorkspaceResponse {
                    status: 200,
                    body: "{}".into(),
                });
            }
            assert_eq!(
                request.path,
                "/api/w/plan-fixture/tickets/orchestration-plans/search"
            );
            let body: Json = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let records = self
                .backend
                .query_orchestration_plan_records(
                    serde_json::from_value(body["ticket"].clone()).unwrap(),
                    serde_json::from_value(body["kind"].clone()).unwrap(),
                )
                .unwrap();
            self.requests.lock().unwrap().push(body);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&records).unwrap(),
            })
        }
    }
    let temp = tempfile::TempDir::new().unwrap();
    let backend =
        SqliteTicketBackend::open(temp.path().join("plans.sqlite3"), "plan-fixture").unwrap();
    let id = backend
        .create(NewTicket::new("Synthetic native plan fixture"))
        .unwrap()
        .id;
    for kind in [
        OrchestrationPlanKind::AcceptedPlan,
        OrchestrationPlanKind::WaitingCapacityNote,
    ] {
        backend
            .add_orchestration_plan_record(
                TicketIdOrSlug::Id(id.clone()),
                NewOrchestrationPlanRecord {
                    kind,
                    related_ticket: None,
                    note: Some("Synthetic plan context".into()),
                    author: None,
                    accepted_plan: (kind == OrchestrationPlanKind::AcceptedPlan).then(|| {
                        AcceptedOrchestrationPlan {
                            summary: "Synthetic accepted plan".into(),
                            branch: None,
                            worktree: None,
                            role_plan: None,
                        }
                    }),
                },
            )
            .unwrap();
    }
    let client = Arc::new(PersistedPlans {
        backend,
        ticket: id.clone(),
        requests: Mutex::new(vec![]),
    });
    let mut registry = WipMountRegistry::new();
    let namespace = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess {
            workflow: true,
            ..Default::default()
        },
        None,
        &namespace,
    )
    .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "plans".into()).unwrap();
    let path = format!("/cases/{id}");
    let object: Json = serde_json::from_str(
        &runtime
            .inspect(path.clone(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap();
    let shown = &object["interfaces"][0];
    let definition: Json = serde_json::from_str(
        &runtime
            .inspect(shown["path"].as_str().unwrap().into(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap();
    let signature = definition["interface_signature"].as_str().unwrap();
    assert!(signature.contains("query_orchestration_plans("));
    assert!(signature.contains("kind?:"));
    assert!(signature.contains("accepted_plan"));
    let reference = InterfaceReference {
        scope: shown["reference"]["scope"].as_str().unwrap().into(),
        name: shown["reference"]["name"].as_str().unwrap().into(),
    };
    for (arguments, expected, kind) in [
        (json!({}), 2, None),
        (json!({"kind":"accepted_plan"}), 1, Some("accepted_plan")),
        (
            json!({"kind":"waiting_capacity_note","limit":1}),
            1,
            Some("waiting_capacity_note"),
        ),
    ] {
        runtime.inspect(path.clone(), true).await.unwrap();
        let result = runtime
            .invoke(
                path.clone(),
                reference.clone(),
                "query_orchestration_plans".into(),
                arguments,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let result: Json = serde_json::from_str(&result.content.unwrap()).unwrap();
        assert_eq!(result["data"]["count"], expected);
        assert_eq!(result["entry"], path);
        assert_eq!(
            result["data"]["records"].as_array().unwrap().len(),
            expected as usize
        );
        if let Some(kind) = kind {
            assert_eq!(result["data"]["records"][0]["kind"], kind);
        }
        assert_eq!(
            client.requests.lock().unwrap().last().unwrap()["kind"],
            kind.map(Json::from).unwrap_or(Json::Null)
        );
    }
}

#[tokio::test]
async fn ticket_inspect_explains_unpublished_objective_and_mr_input_roots() {
    let client = Arc::new(
        ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"items":[],"page":{"next_cursor":null,"has_more":false}}).to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"items":[],"next_cursor":null,"source_checks":[]}).to_string(),
            },
        ])
        .with_authorizations([
            expected_authorization("ticket.query", "workspace", None),
            expected_authorization(
                "notification.diagnostics.read",
                "merge_request",
                Some(("merge_request_id", "MR-42")),
            ),
        ]),
    );
    let mut registry = WipMountRegistry::new();
    registry
        .allocate_namespace("objective", "hidden-goals")
        .unwrap();
    registry
        .allocate_namespace("merge-request", "hidden-changes")
        .unwrap();
    let namespace = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &namespace,
    )
    .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "unpublished-inputs".into()).unwrap();
    let tree = runtime.tree("/".into(), 1, false).await.unwrap();
    assert!(!tree.content.unwrap().contains("hidden-"));
    let object: Json = serde_json::from_str(
        &runtime
            .inspect("/cases".into(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap();
    let shown = &object["interfaces"][0];
    let definition: Json = serde_json::from_str(
        &runtime
            .inspect(shown["path"].as_str().unwrap().into(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap();
    let signature = definition["interface_signature"].as_str().unwrap();
    let root = |domain| -> String {
        let prefix = format!("{domain} Entry: collection root ");
        let doc = signature
            .lines()
            .filter_map(|line| line.trim().strip_prefix("/// "))
            .map(|text| serde_json::from_str::<String>(text).unwrap())
            .find(|text| text.starts_with(&prefix))
            .unwrap();
        let encoded = doc
            .strip_prefix(&prefix)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        serde_json::from_str(encoded).unwrap()
    };
    let objective = root("Objective");
    let mr = root("Merge Request");
    assert_eq!(objective, "/hidden-goals");
    assert_eq!(mr, "/hidden-changes");
    let reference = InterfaceReference {
        scope: shown["reference"]["scope"].as_str().unwrap().into(),
        name: shown["reference"]["name"].as_str().unwrap().into(),
    };
    for (operation, arguments) in [
        (
            "query",
            json!({"filter":{"objective":format!("{objective}/O-42")}}),
        ),
        (
            "notification_diagnostics",
            json!({"filter":{"merge_request":format!("{mr}/MR-42")}}),
        ),
    ] {
        runtime
            .invoke(
                "/cases".into(),
                reference.clone(),
                operation.into(),
                arguments,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
    }
    client.assert_authorizations_consumed();
    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    let query: Json = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(query["linked_objective_id"], "O-42");
    assert!(
        requests[1]
            .path
            .ends_with("notification-diagnostics?merge_request_id=MR-42")
    );
    for path in [objective, mr] {
        assert!(
            runtime.inspect(path, false).await.is_err(),
            "input metadata does not publish Objects"
        );
    }
}

fn ticket_result_detail(key: &str) -> Json {
    json!({
        "resource_key":key,"title":"Intent","body":"/cases/T-999 is untrusted text", "body_truncated":false,
        "content_digest":"observed","state":"open","targets":[],"events":[],"linked_objectives":[],
        "initial_post":{"sequence":0,"event_ref":"initial","is_initial":true,"kind":"comment","body":"Requirements","body_truncated":false,"body_digest":"a".repeat(64),"editable":true,"author":null,"at":null,"edited_at":null,"edited_by":null},
        "closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"closure_history":[],
        "assignment_tracking":[],"tags":[],"assignments":[],"implementation_reports":[],"merge_requests":[]
    })
}

// The response transport is JSON, but navigation is selected by the declared WIP types, not by
// looking for path-like strings or property names inside opaque domain data or rendered signatures.
fn ticket_result_declared_entries(ty: &TypeExpr, value: &Json, entries: &mut Vec<String>) {
    match ty {
        TypeExpr::Entry => entries.push(value.as_str().unwrap().into()),
        TypeExpr::Record { fields } => {
            for field in fields {
                if let Some(value) = value.get(&field.name) {
                    ticket_result_declared_entries(&field.r#type, value, entries);
                }
            }
        }
        TypeExpr::List { items } => {
            for value in value.as_array().unwrap() {
                ticket_result_declared_entries(items, value, entries);
            }
        }
        _ => {} // Json and strings are deliberately not traversed.
    }
}

async fn ticket_result_model_call(
    tools: &agen::tool_server::ToolServerHandle,
    name: &str,
    input: Json,
) -> Json {
    transport_output_json(
        tools
            .call_tool(name, &input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap(),
    )
}
async fn ticket_result_model_inspect(
    tools: &agen::tool_server::ToolServerHandle,
    entry: &Json,
) -> Json {
    ticket_result_model_call(tools, "Inspect", json!({"path":entry})).await
}
async fn ticket_result_model_invoke(
    tools: &agen::tool_server::ToolServerHandle,
    object: &Json,
    operation: &str,
    arguments: Json,
) -> Json {
    ticket_result_model_call(tools, "Invoke", json!({"path":object["path"],"interface":object["interfaces"][0]["reference"],"operation":operation,"arguments":arguments})).await
}

#[tokio::test]
async fn ticket_typed_results_drive_model_query_create_read_and_all_related_domain_transitions() {
    for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
        let mut detail = ticket_result_detail("T-42");
        detail["linked_objectives"] =
            json!([{"resource_key":"O-3","title":"Objective","state":"active"}]);
        detail["relations"] = json!({"outgoing":[{"target_resource_key":"T-7","kind":"related","note":null,"at":null}],"incoming":[],"blockers":[],"notices":[]});
        detail["merge_requests"] = json!([{"merge_request_id":"MR-1","repository_key":"main","state":"open","selector_from":"work/source","selector_to":"develop","review_status":"pending","subject_ref":null,"source_ref_observation":{"status":"not_required"}}]);
        let query_item = json!({"resource_key":"T-42","title":"Intent","state":"open","closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"assignment_tracking":[],"tags":[],"matched_fields":[],"linked_objective_keys":["O-3"],"relation_count":1,"blocker_count":0,"unresolved_blocker_count":0,"unresolved_review_count":0,"merge_requests":detail["merge_requests"], "snippet":"/cases/T-999"});
        let mut second_item = query_item.clone();
        second_item["resource_key"] = json!("T-43");
        let response = |value: Json| crate::worker::WorkspaceResponse {
            status: 200,
            body: value.to_string(),
        };
        let client = Arc::new(ScriptedWorkspaceClient::new([
        response(json!({"items":[query_item, second_item],"page":{"next_cursor":"opaque-next","has_more":true}})),
        response(json!({"items":[],"page":{"next_cursor":null,"has_more":false}})),
        response(detail),
        response(ticket_result_detail("T-7")),
        objective_detail_response("objective-observed"),
        response(json!({"workspace_id":"workspace","merge_request_id":"MR-1","repository_key":"main","state":"open","selector_from":"work/source","selector_to":"develop","ticket_ids":[],"thread":[]})),
        response(json!({"id":"internal123","resource_key":"T-9","slug":"new","status":"Open"})),
    ]).with_authorizations([
        expected_authorization("ticket.query","workspace",None),
        expected_authorization("ticket.query","workspace",None),
        expected_authorization("ticket.read","ticket",Some(("ticket_id","T-42"))),
        expected_authorization("ticket.read","ticket",Some(("ticket_id","T-7"))),
        expected_authorization("objective.read","objective",Some(("objective_id","O-3"))),
        expected_authorization("merge_request.read","merge_request",Some(("merge_request_id","MR-1"))),
        expected_authorization("ticket.create","workspace",None),
    ]));
        let mut registry = WipMountRegistry::new();
        let tickets = registry.allocate_namespace("ticket", "cases").unwrap();
        let objectives = registry.allocate_namespace("objective", "goals").unwrap();
        let mrs = registry
            .allocate_namespace("merge-request", "changes")
            .unwrap();
        for domain in order {
            match domain {
                0 => crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
                    &mut registry,
                    client.clone(),
                    crate::feature::builtin::ticket::TicketFeatureAccess::workspace_authoring(),
                    None,
                    &tickets,
                )
                .unwrap(),
                1 => crate::feature::builtin::objective::mount_workspace_http_objective_wip(
                    &mut registry,
                    client.clone(),
                    None,
                    &objectives,
                )
                .unwrap(),
                2 => {
                    crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
                        &mut registry,
                        client.clone(),
                        manifest::MergeRequestFeatureConfig {
                            show: true,
                            ..Default::default()
                        },
                        None,
                        &mrs,
                    )
                    .unwrap()
                }
                _ => unreachable!(),
            }
        }
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        let runtime = install_wip_mode_with_mounts(
            &mut engine,
            None,
            "ticket-model-results".into(),
            registry,
        )
        .unwrap();
        let tools = engine.tool_server_handle();
        tools.flush_pending();
        let collection = ticket_result_model_inspect(&tools, &json!("/cases")).await;
        let interface =
            ticket_result_model_inspect(&tools, &collection["interfaces"][0]["path"]).await;
        let signature = interface["interface_signature"].as_str().unwrap();
        assert!(signature.contains("`entry`: entry"), "{signature}");
        assert!(signature.contains("`entry`?: entry"));
        assert!(signature.contains("data: json"));
        let query =
            ticket_result_model_invoke(&tools, &collection, "query", json!({"page":{"limit":2}}))
                .await;
        assert_eq!(query["next_cursor"], "opaque-next");
        assert_eq!(query["has_more"], true);
        assert_eq!(
            client.requests().len(),
            1,
            "query does not prefetch any returned entry"
        );
        let declaration = runtime
            .host
            .projection("/cases")
            .unwrap()
            .descriptor
            .operations
            .into_iter()
            .find(|op| op.name == "query")
            .unwrap();
        let mut paths = vec![];
        ticket_result_declared_entries(&declaration.returns.r#type, &query, &mut paths);
        assert!(paths.contains(&"/cases/T-42".into()));
        assert!(!paths.contains(&"/cases/T-999".into()));
        let page = ticket_result_model_invoke(
            &tools,
            &collection,
            "query",
            json!({"page":{"cursor":query["next_cursor"],"limit":2}}),
        )
        .await;
        assert_eq!(page, json!({"items":[],"has_more":false}));
        let selected = ticket_result_model_inspect(&tools, &query["items"][0]["entry"]).await;
        assert_eq!(
            client.requests().len(),
            2,
            "Inspect resolves only route metadata, not related reads"
        );
        let read =
            ticket_result_model_invoke(&tools, &selected, "read", json!({"page":{"limit":1}}))
                .await;
        assert_eq!(read["entry"], query["items"][0]["entry"]);
        for domain in ["tickets", "objectives", "merge_requests"] {
            let related =
                ticket_result_model_inspect(&tools, &read["links"][domain][0]["entry"]).await;
            let interface =
                ticket_result_model_inspect(&tools, &related["interfaces"][0]["path"]).await;
            assert!(
                interface["interface_signature"]
                    .as_str()
                    .unwrap()
                    .contains("read(")
            );
            ticket_result_model_invoke(&tools, &related, "read", json!({})).await;
        }
        let created = ticket_result_model_invoke(
            &tools,
            &collection,
            "create",
            json!({"content":{"title":"New"}}),
        )
        .await;
        let created_object = ticket_result_model_inspect(&tools, &created["entry"]).await;
        assert_eq!(created_object["path"], created["entry"]);
        assert_eq!(client.requests().len(), 7);
        client.assert_authorizations_consumed();
        let tree = ticket_result_model_call(&tools, "Tree", json!({"path":"/","depth":3})).await;
        assert!(!tree.to_string().contains("/cases/T-42"));
        assert!(!tree.to_string().contains("/goals/O-3"));
        assert!(!tree.to_string().contains("/changes/MR-1"));
        assert!(!tree.to_string().contains("/cases/T-43"));
        assert_eq!(client.requests().len(), 7);
    }
}

#[tokio::test]
async fn ticket_successful_create_is_never_replayed_after_selected_deleted_or_denied_read() {
    let client = Arc::new(
        ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"id":"internal123","resource_key":"T-9","slug":"new","status":"Open"})
                    .to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 404,
                body: json!({"error":"not_found","message":"deleted"}).to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 403,
                body: json!({"error":"permission_denied","message":"denied"}).to_string(),
            },
        ])
        .with_authorizations([
            expected_authorization("ticket.create", "workspace", None),
            expected_authorization("ticket.read", "ticket", Some(("ticket_id", "T-9"))),
            expected_authorization("ticket.read", "ticket", Some(("ticket_id", "T-9"))),
        ]),
    );
    let mut registry = WipMountRegistry::new();
    let route = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::workspace_authoring(),
        None,
        &route,
    )
    .unwrap();
    let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
    install_wip_mode_with_mounts(
        &mut engine,
        None,
        "ticket-negative-results".into(),
        registry,
    )
    .unwrap();
    let tools = engine.tool_server_handle();
    tools.flush_pending();
    let collection = ticket_result_model_inspect(&tools, &json!("/cases")).await;
    let created = ticket_result_model_invoke(
        &tools,
        &collection,
        "create",
        json!({"content":{"title":"Created once"}}),
    )
    .await;
    assert_eq!(client.requests().len(), 1);
    let object = ticket_result_model_inspect(&tools, &created["entry"]).await;
    for expected in ["NotFound", "PermissionDenied"] {
        let error=tools.call_tool("Invoke",&json!({"path":created["entry"],"interface":object["interfaces"][0]["reference"],"operation":"read","arguments":{}}).to_string(),ToolExecutionContext::direct()).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    let requests = client.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/api/w/workspace/tickets")
            .count(),
        1
    );
    client.assert_authorizations_consumed();
}

#[test]
fn result_namespaces_follow_committed_registry_rollback_and_independent_host_builds() {
    let client = Arc::new(ScriptedWorkspaceClient::new([]));
    let mut registry = WipMountRegistry::new();
    let ticket = registry.allocate_namespace("ticket", "cases").unwrap();
    let objective = registry.allocate_namespace("objective", "goals").unwrap();
    let checkpoint = registry.clone();
    assert!(registry.publish_result_namespace(&ticket).is_err());
    assert!(
        registry
            .result_namespace_snapshot()
            .root("ticket")
            .is_none()
    );
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &ticket,
    )
    .unwrap();
    let ticket_host = WipHost::new(registry.clone());
    assert_eq!(ticket_host.result_namespaces.root("ticket"), Some("/cases"));
    assert_eq!(ticket_host.result_namespaces.root("objective"), None);
    crate::feature::builtin::objective::mount_workspace_http_objective_wip(
        &mut registry,
        client,
        None,
        &objective,
    )
    .unwrap();
    let both = WipHost::new(registry.clone());
    assert_eq!(both.result_namespaces.root("objective"), Some("/goals"));
    assert_eq!(
        ticket_host.result_namespaces.root("objective"),
        None,
        "building another Host cannot mutate or freeze a prior snapshot"
    );
    let rolled_back = WipHost::new(checkpoint);
    assert_eq!(rolled_back.result_namespaces.root("ticket"), None);
    assert_eq!(rolled_back.result_namespaces.root("objective"), None);
    assert_eq!(both.result_namespaces.root("ticket"), Some("/cases"));
    registry
        .dynamic_mounts
        .retain(|mount| mount.collection_route != "/goals");
    let partial = WipHost::new(registry.clone());
    assert_eq!(
        partial.result_namespaces.root("objective"),
        None,
        "a marker alone cannot publish a missing/rolled-back item mount"
    );
    registry
        .allocate_namespace("ticket", "ambiguous-cases")
        .unwrap();
    assert_eq!(
        WipHost::new(registry).result_namespaces.root("ticket"),
        None
    );
}

// Production Feature installer + Objective composition + final WIP Host snapshot.
// Only Workspace transport is scripted; registration must perform no requests.
#[test]
fn feature_install_publication_snapshot_preserves_host_roots_and_rolls_back_new_markers() {
    use crate::feature::{
        FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
        FeatureRegistryBuilder,
    };
    struct LateFailure;
    impl FeatureModule for LateFailure {
        fn descriptor(&self) -> FeatureDescriptor {
            FeatureDescriptor::builtin("late-failure", "Late failure")
        }
        fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
            Err(FeatureInstallError::InvalidDescriptor("late failure".into()))
        }
    }
    for fail in [false, true] {
        let client = Arc::new(ScriptedWorkspaceClient::new([]));
        let mut mounts = WipMountRegistry::new();
        let ticket = mounts.allocate_namespace("ticket", "cases").unwrap();
        crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            &mut mounts,
            client.clone(),
            crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
            None,
            &ticket,
        )
        .unwrap();
        let before = WipHost::new(mounts.clone());
        let objective = mounts.allocate_namespace("objective", "goals").unwrap();
        let mut features = FeatureRegistryBuilder::new();
        crate::feature::builtin::objective::composition::compose(
            &mut features, true, client.clone(), Some((objective, None)),
        )
        .unwrap();
        if fail {
            features.add_module(LateFailure);
        }
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        let report = features.install_into_engine_with_wip(
            &mut engine, &mut crate::hook::HookRegistryBuilder::default(), Some(&mut mounts),
        );
        assert_eq!(report.has_errors(), fail);
        let after = WipHost::new(mounts);
        assert_eq!(after.result_namespaces.root("ticket"), Some("/cases"));
        assert_eq!(after.result_namespaces.root("objective"), (!fail).then_some("/goals"));
        assert_eq!(before.result_namespaces.root("objective"), None);
        assert_eq!(before.result_namespaces.root("ticket"), Some("/cases"));
        assert_eq!(after.observe("/goals", 1).is_ok(), !fail);
        assert!(client.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn host_overwrites_forged_result_context_and_omits_unpublished_cross_domain_entries() {
    let mut detail = ticket_result_detail("T-42");
    detail["linked_objectives"] =
        json!([{"resource_key":"O-3","title":"Visible ID only","state":"active"}]);
    let client = Arc::new(
        ScriptedWorkspaceClient::new([crate::worker::WorkspaceResponse {
            status: 200,
            body: detail.to_string(),
        }])
        .with_authorizations([expected_authorization(
            "ticket.read",
            "ticket",
            Some(("ticket_id", "T-42")),
        )]),
    );
    let mut registry = WipMountRegistry::new();
    let route = registry.allocate_namespace("ticket", "cases").unwrap();
    registry
        .allocate_namespace("objective", "reserved-goals")
        .unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &route,
    )
    .unwrap();
    let host = WipHost::new(registry);
    let projection = host.projection("/cases/T-42").unwrap();
    let output = host
        .call(
            CallOperationRequest {
                target: wip_protocol::Target {
                    path: "/cases/T-42".into(),
                    validator: projection.object.validator,
                },
                interface: wip_protocol::InterfaceTarget {
                    reference: projection.interface,
                    scope_ref: None,
                    validator: projection.interface_validator,
                },
                operation: "read".into(),
                arguments: BTreeMap::new(),
            },
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "forged".into(),
                result_namespaces: WipResultNamespaces(Arc::new(BTreeMap::from([(
                    "objective".into(),
                    "/fake-goals".into(),
                )]))),
            },
        )
        .await
        .unwrap_or_else(|_| panic!("read"));
    let output = wip_to_json(&output.value).unwrap();
    assert_eq!(output["links"]["objectives"], json!([{"id":"O-3"}]));
    assert_eq!(client.requests().len(), 1);
    client.assert_authorizations_consumed();
}

struct HiddenResultNamespace;
#[async_trait]
impl WipOperationHandler for HiddenResultNamespace {
    fn is_visible(&self) -> bool {
        false
    }
    async fn call(
        &self,
        _: &str,
        _: &BTreeMap<String, Value>,
        _: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        unreachable!("hidden fixture must never dispatch")
    }
}

#[test]
fn result_namespace_registration_rejects_failed_partial_mounts_and_final_hidden_collections() {
    let client = Arc::new(ScriptedWorkspaceClient::new([]));
    let mut registry = WipMountRegistry::new();
    let route = registry.allocate_namespace("ticket", "cases").unwrap();
    let reference = root_reference("yoi.ticket/item/v2");
    registry
        .mount(WipProjection {
            route: "/tools/conflicting-item-interface".into(),
            capability: "fixture:conflict".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: "conflicting-item-interface".into(),
                description: None,
                interfaces: vec![reference.clone()],
                r#ref: None,
                validator: None,
            },
            interface: reference,
            interface_validator: None,
            descriptor: InterfaceDescriptor {
                format: INTERFACE_FORMAT_V1.into(),
                documentation: None,
                types: vec![],
                operations: vec![],
            },
            handler: Arc::new(HiddenResultNamespace),
        })
        .unwrap();
    let result = crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &route,
    );
    assert!(matches!(
        result,
        Err(WipMountError::InterfaceCollision { .. })
    ));
    assert!(
        registry.mounts.contains_key("/cases"),
        "the failure happened after collection staging"
    );
    assert_eq!(
        WipHost::new(registry).result_namespaces.root("ticket"),
        None
    );
    let mut registry = WipMountRegistry::new();
    let route = registry.allocate_namespace("ticket", "cases").unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client,
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &route,
    )
    .unwrap();
    registry
        .mounts
        .get_mut("/cases")
        .unwrap()
        .projection
        .handler = Arc::new(HiddenResultNamespace);
    assert_eq!(
        WipHost::new(registry).result_namespaces.root("ticket"),
        None,
        "final visibility overrides a stale success marker"
    );
}

#[tokio::test]
async fn retained_ticket_result_mr_reference_is_model_navigation_not_body_or_snapshot_authority() {
    let receipt = json!({"result_id":"saved","ticket_id":"T-42","created_at":"now",
        "request":{"operation_key":"saved-operation","kind":"result","body":"/changes/MR-999 is prose",
            "merge_request":{"merge_request_id":"MR-1","subject_ref":"historical-source"}}});
    let client = Arc::new(
        ScriptedWorkspaceClient::new([
            crate::worker::WorkspaceResponse {
                status: 200,
                body: receipt.to_string(),
            },
            crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"merge_request_id":"MR-1","ticket_ids":[],"thread":[]}).to_string(),
            },
        ])
        .with_authorizations([
            expected_authorization(
                "ticket.implementation_result.read",
                "ticket",
                Some(("ticket_id", "T-42")),
            ),
            expected_authorization(
                "merge_request.read",
                "merge_request",
                Some(("merge_request_id", "MR-1")),
            ),
        ]),
    );
    let mut registry = WipMountRegistry::new();
    let tickets = registry.allocate_namespace("ticket", "cases").unwrap();
    let changes = registry
        .allocate_namespace("merge-request", "changes")
        .unwrap();
    crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut registry,
        client.clone(),
        crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
        None,
        &tickets,
    )
    .unwrap();
    crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
        &mut registry,
        client.clone(),
        manifest::MergeRequestFeatureConfig {
            show: true,
            ..Default::default()
        },
        None,
        &changes,
    )
    .unwrap();
    let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
    let runtime = install_wip_mode_with_mounts(
        &mut engine,
        None,
        "retained-result-entries".into(),
        registry,
    )
    .unwrap();
    let tools = engine.tool_server_handle();
    tools.flush_pending();
    let selected = ticket_result_model_inspect(&tools, &json!("/cases/T-42")).await;
    let result = ticket_result_model_invoke(
        &tools,
        &selected,
        "implementation_result_query",
        json!({"result_id":"saved"}),
    )
    .await;
    assert_eq!(result["data"], receipt);
    assert_eq!(
        client.requests().len(),
        1,
        "saved references do not prefetch current state"
    );
    let declaration = runtime
        .host
        .projection("/cases/T-42")
        .unwrap()
        .descriptor
        .operations
        .into_iter()
        .find(|op| op.name == "implementation_result_query")
        .unwrap();
    let mut entries = vec![];
    ticket_result_declared_entries(&declaration.returns.r#type, &result, &mut entries);
    assert!(entries.contains(&"/changes/MR-1".into()));
    assert!(!entries.contains(&"/changes/MR-999".into()));
    let related =
        ticket_result_model_inspect(&tools, &result["links"]["merge_requests"][0]["entry"]).await;
    let interface = ticket_result_model_inspect(&tools, &related["interfaces"][0]["path"]).await;
    assert!(
        interface["interface_signature"]
            .as_str()
            .unwrap()
            .contains("read(")
    );
    ticket_result_model_invoke(&tools, &related, "read", json!({})).await;
    assert_eq!(client.requests().len(), 2);
    client.assert_authorizations_consumed();
}

#[tokio::test]
async fn diagnostics_uncaptured_mr_origin_is_typed_lazy_model_navigation_even_on_empty_pages() {
    for published in [false, true] {
        let page = json!({"items":[],"next_cursor":"opaque-next",
            "source_checks":[{"resource_kind":"merge_request_events","reference":"MR-1","origin_present":true,"captured":false}]});
        let mut responses = vec![crate::worker::WorkspaceResponse {
            status: 200,
            body: page.to_string(),
        }];
        let mut authorizations = vec![expected_authorization(
            "notification.diagnostics.read",
            "merge_request",
            Some(("merge_request_id", "MR-1")),
        )];
        if published {
            responses.push(crate::worker::WorkspaceResponse {
                status: 200,
                body: json!({"merge_request_id":"MR-1","ticket_ids":[],"thread":[]}).to_string(),
            });
            authorizations.push(expected_authorization(
                "merge_request.read",
                "merge_request",
                Some(("merge_request_id", "MR-1")),
            ));
        }
        let client =
            Arc::new(ScriptedWorkspaceClient::new(responses).with_authorizations(authorizations));
        let mut registry = WipMountRegistry::new();
        let tickets = registry.allocate_namespace("ticket", "cases").unwrap();
        let changes = registry
            .allocate_namespace("merge-request", "changes")
            .unwrap();
        crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            &mut registry,
            client.clone(),
            crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
            None,
            &tickets,
        )
        .unwrap();
        if published {
            crate::feature::builtin::merge_request::mount_workspace_http_merge_request_wip(
                &mut registry,
                client.clone(),
                manifest::MergeRequestFeatureConfig {
                    show: true,
                    ..Default::default()
                },
                None,
                &changes,
            )
            .unwrap();
        }
        let mut engine = Engine::<_, Mutable, ()>::new_annotated(DummyClient);
        let runtime = install_wip_mode_with_mounts(
            &mut engine,
            None,
            "diagnostics-origin-model".into(),
            registry,
        )
        .unwrap();
        let tools = engine.tool_server_handle();
        tools.flush_pending();
        let collection = ticket_result_model_inspect(&tools, &json!("/cases")).await;
        let result=ticket_result_model_invoke(&tools,&collection,"notification_diagnostics",json!({"filter":{"merge_request":"/changes/MR-1"},"page":{"cursor":"previous-page","limit":1}})).await;
        assert_eq!(result["data"], page);
        assert_eq!(result["links"]["merge_requests"][0]["id"], "MR-1");
        assert_eq!(
            client.requests().len(),
            1,
            "origin reference never prefetches current MR"
        );
        let declaration = runtime
            .host
            .projection("/cases")
            .unwrap()
            .descriptor
            .operations
            .into_iter()
            .find(|op| op.name == "notification_diagnostics")
            .unwrap();
        let mut entries = vec![];
        ticket_result_declared_entries(&declaration.returns.r#type, &result, &mut entries);
        if published {
            assert_eq!(entries, vec!["/changes/MR-1"]);
            let mr =
                ticket_result_model_inspect(&tools, &result["links"]["merge_requests"][0]["entry"])
                    .await;
            let interface = ticket_result_model_inspect(&tools, &mr["interfaces"][0]["path"]).await;
            assert!(
                interface["interface_signature"]
                    .as_str()
                    .unwrap()
                    .contains("read(")
            );
            ticket_result_model_invoke(&tools, &mr, "read", json!({})).await;
            assert_eq!(client.requests().len(), 2);
        } else {
            assert!(entries.is_empty());
            assert!(result["links"]["merge_requests"][0].get("entry").is_none());
        }
        client.assert_authorizations_consumed();
        let request = &client.requests()[0];
        assert!(request.path.contains("cursor=previous-page"));
        assert!(request.path.contains("merge_request_id=MR-1"));
        let tree = ticket_result_model_call(&tools, "Tree", json!({"path":"/","depth":3})).await;
        assert!(!tree.to_string().contains("/changes/MR-1"));
    }
}
