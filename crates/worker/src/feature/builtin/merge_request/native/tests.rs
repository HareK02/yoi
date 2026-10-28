//! Component/connection tests use the production WIP Host, Client, Inspect and Invoke binding,
//! native contracts and domain dispatcher. Only Workspace HTTP is replaced by the existing MR
//! recording fixture; these tests assert dispatch/authorization, not Backend transaction durability.

use super::super::tests::{RecordingWorkspaceClient, response};
use super::*;
use crate::wip::WipRuntime;
use wip_protocol::TypeExpr;

pub(super) fn arguments(value: Value) -> BTreeMap<String, WipValue> {
    let WipValue::Record(arguments) = json_to_wip(&value).unwrap() else {
        panic!("expected arguments")
    };
    arguments
}

pub(super) fn roots() -> ReferenceRoots {
    ReferenceRoots {
        ticket: "/cases".into(),
        merge_request: "/changes".into(),
        objective: String::new(),
    }
}

fn all_config() -> MergeRequestFeatureConfig {
    MergeRequestFeatureConfig {
        show: true,
        open: true,
        review: true,
        readiness_check: true,
        complete: true,
    }
}

pub(super) fn runtime(client: Arc<dyn WorkspaceClient>) -> WipRuntime {
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("ticket", "cases").unwrap();
    let route = registry
        .allocate_namespace("merge-request", "changes")
        .unwrap();
    mount_workspace_http_merge_request_wip(&mut registry, client, all_config(), None, &route)
        .unwrap();
    WipRuntime::from_mounts(registry, "mr-native-test".into()).unwrap()
}

async fn inspect_interface(runtime: &WipRuntime, path: &str) -> String {
    let object = runtime.inspect(path.into(), false).await.unwrap();
    let object: Value = serde_json::from_str(object.content.as_deref().unwrap()).unwrap();
    let interface = runtime
        .inspect(
            object["interfaces"][0]["path"].as_str().unwrap().into(),
            false,
        )
        .await
        .unwrap();
    let interface: Value = serde_json::from_str(interface.content.as_deref().unwrap()).unwrap();
    interface["interface_signature"].as_str().unwrap().into()
}

#[test]
fn native_contract_exposes_records_enums_nullable_and_entry_without_tool_schema() {
    let open = contract(Kind::Open).declaration(&roots().contract_roots());
    assert!(matches!(open.parameters[0].r#type, TypeExpr::Entry));
    let TypeExpr::Record { ref fields } = open.parameters[1].r#type else {
        panic!("repository record")
    };
    assert_eq!(
        fields
            .iter()
            .map(|f| (f.name.as_str(), f.required))
            .collect::<Vec<_>>(),
        [("key", true), ("source", true), ("target", true)]
    );
    let review = contract(Kind::Review).declaration(&roots().contract_roots());
    assert!(review.parameters[0].required);
    assert!(matches!(review.parameters[0].r#type, TypeExpr::Enum { .. }));
    let TypeExpr::List { items } = &review.parameters[2].r#type else {
        panic!("finding list")
    };
    let TypeExpr::Record { fields } = items.as_ref() else {
        panic!("typed finding")
    };
    assert!(
        fields
            .iter()
            .find(|f| f.name == "severity")
            .unwrap()
            .required
    );
    assert!(matches!(
        fields.iter().find(|f| f.name == "line").unwrap().r#type,
        TypeExpr::Union { .. }
    ));
    assert!(!fields.iter().find(|f| f.name == "line").unwrap().required);
    // Tool mode intentionally accepts any severity string for Backend validation. Native has its
    // own enum contract; equality with the Tool schema is not the acceptance criterion.
    assert_eq!(
        Kind::Review.schema()["$defs"]["ReviewFindingInput"]["properties"]["severity"]["type"],
        "string"
    );
    assert!(
        convert(
            &contract(Kind::Review),
            &arguments(
                json!({"decision":"approve","findings":[{"severity":"arbitrary","body":"x"}]})
            ),
            &roots()
        )
        .is_err()
    );
}

#[test]
fn native_mapping_groups_repository_and_exact_integration_observations() {
    let opened = convert(
        &contract(Kind::Open),
        &arguments(json!({
            "ticket":"/cases/T-797", "repository":{"key":"main","source":"work","target":"develop"}
        })),
        &roots(),
    )
    .unwrap_or_else(|_| panic!("valid native fixture was rejected"));
    assert_eq!(
        opened,
        json!({"ticket":"T-797","repository_key":"main","selector_from":"work","selector_to":"develop"})
    );
    assert!(
        matches!(command(Kind::Open, opened).unwrap_or_else(|_| panic!("valid native command was rejected")), command::Command::Open { ticket, summary, .. } if ticket == "T-797" && summary.is_empty())
    );
    let complete = convert(&contract(Kind::Complete), &arguments(json!({
        "operation_id":"op-1", "integration":{"approval_event_id":"approval-1", "target":{"before":"before","after":"after"}, "strategy":"merge","resolution":"conflicts_resolved"}
    })), &roots()).unwrap_or_else(|_| panic!("valid native fixture was rejected"));
    assert_eq!(
        complete,
        json!({"operation_id":"op-1","approval_event_id":"approval-1","target_ref_before":"before","target_ref_after":"after","strategy":"merge","resolution":"conflicts_resolved"})
    );
}

#[test]
fn native_omission_and_explicit_null_have_distinct_declared_forms() {
    assert_eq!(
        convert(&contract(Kind::Show), &BTreeMap::new(), &roots())
            .unwrap_or_else(|_| panic!("valid native fixture was rejected")),
        json!({})
    );
    assert_eq!(
        convert(
            &contract(Kind::Show),
            &arguments(
                json!({"page":{"after":{"$case":"null"},"limit":{"$case":"value","value":25}}})
            ),
            &roots()
        )
        .unwrap_or_else(|_| panic!("valid native fixture was rejected")),
        json!({"after":null,"limit":25})
    );
    let review = convert(&contract(Kind::Review), &arguments(json!({"decision":"approve", "findings":[{"severity":"note","body":"x","code":{"$case":"null"},"line":{"$case":"value","value":7}}]})), &roots()).unwrap_or_else(|_| panic!("valid native fixture was rejected"));
    assert_eq!(
        review["findings"][0],
        json!({"severity":"note","body":"x","code":null,"line":7})
    );
    for invalid in [
        json!({"page":null}),
        json!({"page":{"limit":null}}),
        json!({"page":{"limit":25}}),
    ] {
        assert!(convert(&contract(Kind::Show), &arguments(invalid), &roots()).is_err());
    }
    for invalid in [
        json!({"decision":null}),
        json!({"decision":"approve","body":null}),
        json!({"decision":"approve","findings":null}),
    ] {
        assert!(convert(&contract(Kind::Review), &arguments(invalid), &roots()).is_err());
    }
}

#[test]
fn native_references_use_the_allocated_namespace_and_reject_identity_spoofing() {
    for ticket in [
        "T-797",
        "/tickets/T-797",
        "/cases2/T-797",
        "/changes/MR-1",
        "/cases/T-797/extra",
        "/cases/T%2D797",
    ] {
        assert!(convert(&contract(Kind::Open), &arguments(json!({"ticket":ticket,"repository":{"key":"main","source":"work","target":"develop"}})), &roots()).is_err(), "accepted {ticket}");
    }
    assert!(
        convert(
            &contract(Kind::Show),
            &arguments(json!({"merge_request_id":"MR-2"})),
            &roots()
        )
        .is_err()
    );
    assert!(
        convert(
            &contract(Kind::Review),
            &arguments(json!({"decision":"approve","capability_token":"spoof"})),
            &roots()
        )
        .is_err()
    );
}

#[tokio::test]
async fn inspect_then_invoke_native_open_read_readiness_and_complete_reaches_shared_domain() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        response(json!({"merge_request_id":"MR-1"})),
        response(json!({"merge_request_id":"MR-1","thread":[]})),
        response(json!({"merge_request_id":"MR-1","ready":true})),
        response(json!({"merge_request_id":"MR-1","event_id":"integrated-1"})),
    ]));
    let runtime = runtime(client.clone());
    let collection = inspect_interface(&runtime, "/changes").await;
    assert!(collection.contains("ticket: entry"));
    assert!(collection.contains("repository:"));
    runtime.invoke("/changes".into(), crate::wip::root_reference(MERGE_REQUEST_COLLECTION_INTERFACE), "open".into(),
        json!({"ticket":"/cases/T-797","repository":{"key":"main","source":"work","target":"develop"}}), ToolExecutionContext::direct()).await.unwrap();
    let item = inspect_interface(&runtime, "/changes/MR-1").await;
    assert!(item.contains("operation readiness("));
    assert!(item.contains("integration:"));
    assert!(item.contains("findings?:"));
    assert!(!item.contains("merge_request_id:"));
    for (operation, input) in [
        (
            "read",
            json!({"page":{"after":{"$case":"value","value":7},"limit":{"$case":"value","value":25}}}),
        ),
        ("readiness", json!({})),
        (
            "complete",
            json!({"operation_id":"op-1","integration":{"approval_event_id":"approval-1","target":{"before":"before","after":"after"},"strategy":"fast_forward","resolution":"clean"}}),
        ),
    ] {
        // Read observations update the object's content validator. Explicitly refresh between
        // independent operations; Invoke must not retry a dispatched validator rejection.
        runtime.inspect("/changes/MR-1".into(), true).await.unwrap();
        runtime
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                operation.into(),
                input,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].path, "/api/w/ws/tickets/T-797/merge-request");
    assert_eq!(
        requests[1].path,
        "/api/w/ws/merge-requests/MR-1?after=7&limit=25"
    );
    assert_eq!(requests[2].path, "/api/w/ws/merge-requests/MR-1/readiness");
    assert_eq!(
        serde_json::from_str::<Value>(requests[3].body.as_deref().unwrap()).unwrap(),
        json!({"operation_id":"op-1","approval_event_id":"approval-1","target_ref_before":"before","target_ref_after":"after","strategy":"fast_forward","resolution":"clean"})
    );
    assert_eq!(
        client
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.operation.as_str())
            .collect::<Vec<_>>(),
        [
            "merge_request.open",
            "merge_request.read",
            "merge_request.readiness",
            "merge_request.complete"
        ]
    );
}

#[tokio::test]
async fn native_contract_and_command_rejections_precede_authorization_and_dispatch() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let runtime = runtime(client.clone());
    for (operation, input) in [
        (
            "read",
            json!({"page":{"limit":{"$case":"value","value":201}}}),
        ),
        (
            "read",
            json!({"page":{"after":{"$case":"value","value":-1}}}),
        ),
        ("read", json!({"after":7})),
        ("read", json!({"merge_request_id":"MR-2"})),
        ("check_readiness", json!({})),
        (
            "complete",
            json!({"operation_id":"op","integration":{"approval_event_id":"approval","target":{"before":"before"},"strategy":"merge","resolution":"clean"}}),
        ),
        (
            "complete",
            json!({"operation_id":"op","integration":{"approval_event_id":"approval","target":{"before":"before","after":"after"},"strategy":"squash","resolution":"clean"}}),
        ),
    ] {
        assert!(
            runtime
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    operation.into(),
                    input,
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
    }
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(client.authorizations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_review_typed_findings_reaches_only_injected_route_and_never_retries_unknown() {
    use crate::worker::{ReviewerChildWorkspaceClient, ReviewerContext};
    let inner = Arc::new(RecordingWorkspaceClient::new(vec![
        response(json!({"event_id":"review-1"})),
        crate::worker::WorkspaceResponse {
            status: 503,
            body: "unavailable".into(),
        },
    ]));
    let client: Arc<dyn WorkspaceClient> = Arc::new(ReviewerChildWorkspaceClient::new(
        inner.clone(),
        ReviewerContext {
            ticket_id: "T-797".into(),
            merge_request_id: "MR-1".into(),
        },
        "secret-capability".into(),
    ));
    let runtime = runtime(client);
    let signature = inspect_interface(&runtime, "/changes/MR-1").await;
    assert!(signature.contains("request_changes"));
    assert!(signature.contains("major"));
    assert!(!signature.contains("secret-capability"));
    let input = json!({"decision":"request_changes","findings":[{"severity":"major","body":"Fix it","path":{"$case":"value","value":"src/main.rs"},"line":{"$case":"value","value":12},"code":{"$case":"null"}}]});
    assert!(
        runtime
            .invoke(
                "/changes/MR-2".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "review".into(),
                input.clone(),
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert!(inner.requests.lock().unwrap().is_empty());
    for invalid in [
        json!({"decision":"approve","findings":[{"severity":"major","body":"x","line":{"$case":"value","value":-1}}]}),
        json!({"decision":"approve","findings":[{"severity":"major","body":"x","line":{"$case":"value","value":4294967296_u64}}]}),
        json!({"decision":"defer"}),
    ] {
        assert!(
            runtime
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    "review".into(),
                    invalid,
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
    }
    assert!(inner.authorizations.lock().unwrap().is_empty());
    runtime
        .invoke(
            "/changes/MR-1".into(),
            crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
            "review".into(),
            input.clone(),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let failed = runtime
        .invoke(
            "/changes/MR-1".into(),
            crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
            "review".into(),
            input,
            ToolExecutionContext::direct(),
        )
        .await;
    assert!(failed.is_err());
    let requests = inner.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "unknown dispatch must not retry");
    let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["findings"][0],
        json!({"severity":"major","body":"Fix it","path":"src/main.rs","line":12,"code":null})
    );
    assert_eq!(body["capability_token"], "secret-capability");
}

#[tokio::test]
async fn mr_only_inspect_supplies_reserved_ticket_coordinates_without_observing_ticket() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(
        json!({"merge_request_id":"MR-1"}),
    )]));
    let mut registry = WipMountRegistry::new();
    registry
        .allocate_namespace("ticket", "hidden-cases")
        .unwrap();
    let route = registry
        .allocate_namespace("merge-request", "changes")
        .unwrap();
    mount_workspace_http_merge_request_wip(
        &mut registry,
        client.clone(),
        MergeRequestFeatureConfig {
            open: true,
            ..Default::default()
        },
        None,
        &route,
    )
    .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "mr-only-input-discovery".into()).unwrap();
    let tree = runtime.tree("/".into(), 1, false).await.unwrap();
    assert!(!tree.content.unwrap().contains("hidden-cases"));
    let object = runtime.inspect("/changes".into(), false).await.unwrap();
    let object: Value = serde_json::from_str(&object.content.unwrap()).unwrap();
    let shown = &object["interfaces"][0];
    let definition = runtime
        .inspect(shown["path"].as_str().unwrap().into(), false)
        .await
        .unwrap();
    let definition: Value = serde_json::from_str(&definition.content.unwrap()).unwrap();
    let signature = definition["interface_signature"].as_str().unwrap();
    // The model has a known Ticket ID, but obtains the required input coordinates solely
    // from the native Inspect contract; it never observes or invokes the reserved domain.
    let doc = signature
        .lines()
        .filter_map(|line| line.trim().strip_prefix("/// "))
        .map(|text| serde_json::from_str::<String>(text).unwrap())
        .find(|text| text.starts_with("Ticket Entry: collection root "))
        .unwrap();
    let encoded = doc
        .strip_prefix("Ticket Entry: collection root ")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let root: String = serde_json::from_str(encoded).unwrap();
    assert_eq!(root, "/hidden-cases");
    assert!(signature.contains("Coordinates do not imply Object publication"));
    assert!(runtime.inspect(root.clone(), false).await.is_err());
    let input = json!({"ticket":format!("{root}/T-797"),"repository":{"key":"main","source":"work","target":"develop"}});
    runtime
        .invoke(
            "/changes".into(),
            wip_protocol::InterfaceReference {
                scope: shown["reference"]["scope"].as_str().unwrap().into(),
                name: shown["reference"]["name"].as_str().unwrap().into(),
            },
            "open".into(),
            input,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/w/ws/tickets/T-797/merge-request");
    assert_eq!(
        client.authorizations.lock().unwrap()[0].resource,
        server_api::authorization::PermissionResource::Ticket {
            workspace_id: "ws".into(),
            ticket_id: "T-797".into()
        }
    );
    assert!(
        runtime
            .inspect(format!("{root}/T-797"), false)
            .await
            .is_err()
    );
}
