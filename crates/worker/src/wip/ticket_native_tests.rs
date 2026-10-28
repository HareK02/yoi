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
    let path = output["tickets"][0]["path"].as_str().unwrap();
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
