use super::tests::{RecordingWorkspaceClient, response};
use super::*;

fn page() -> Value {
    json!({"items":[{"summary":{"merge_request_id":"MR-1","state":{"kind":"integration"}},"ticket_ids":["T-1"]}],"next_cursor":"page-token"})
}

#[tokio::test]
async fn query_tool_forwards_phase_self_and_encoded_selectors_with_collection_authorization() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(page())]));
    let output = MergeRequestTool { client: client.clone(), kind: Kind::Query }.execute(
        r#"{"repository_key":"main","selector_to":"refs/heads/work/a&b","phase":"integration","assigned_to_self":true,"cursor":"page/token?","limit":3}"#,
        ToolExecutionContext::direct(),
    ).await.unwrap();
    assert_eq!(output.summary, "QueryMergeRequest");
    assert_eq!(
        serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap(),
        page()
    );
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .path
            .contains("selector_to=refs%2Fheads%2Fwork%2Fa%26b")
    );
    assert!(requests[0].path.contains("phase=integration"));
    assert!(requests[0].path.contains("assigned_to_self=true"));
    assert!(requests[0].path.contains("cursor=page%2Ftoken%3F"));
    let checks = client.authorizations.lock().unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].operation, "merge_request.query");
    assert_eq!(
        checks[0].resource,
        PermissionResource::Workspace {
            workspace_id: "ws".into()
        }
    );
}

#[tokio::test]
async fn query_validates_bounds_and_denial_never_dispatches_or_retries() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Query,
    };
    for invalid in [
        r#"{"limit":0}"#,
        r#"{"limit":101}"#,
        r#"{"phase":"approved"}"#,
        r#"{"assigned_to_self":"true"}"#,
        r#"{"repository_key":" "}"#,
        r#"{"worker_id":"forged"}"#,
    ] {
        assert!(
            tool.execute(invalid, ToolExecutionContext::direct())
                .await
                .is_err(),
            "{invalid}"
        );
    }
    assert!(client.authorizations.lock().unwrap().is_empty());
    client
        .auth_denied
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        tool.execute("{}", ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn query_native_collection_exposes_typed_navigation_and_preserves_page_metadata() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(page())]));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("ticket", "cases").unwrap();
    let route = registry
        .allocate_namespace("merge-request", "changes")
        .unwrap();
    mount_workspace_http_merge_request_wip(
        &mut registry,
        client.clone(),
        MergeRequestFeatureConfig {
            query: true,
            show: true,
            ..Default::default()
        },
        &route,
    )
    .unwrap();
    let runtime = crate::wip::WipRuntime::from_mounts(registry, "mr-query".into()).unwrap();
    runtime.inspect("/changes".into(), false).await.unwrap();
    let output = runtime.invoke("/changes".into(), crate::wip::root_reference(MERGE_REQUEST_COLLECTION_INTERFACE), "query".into(),
        json!({"phase":"integration","ticket":"/cases/T-1","assigned_to_self":true,"page":{"limit":{"$case":"value","value":2}}}), ToolExecutionContext::direct()).await.unwrap();
    let text = output.content.unwrap();
    assert!(text.contains("/changes/MR-1"), "{text}");
    assert!(text.contains("page-token"), "{text}");
    assert!(
        client.requests.lock().unwrap()[0]
            .path
            .contains("ticket_ref=T-1")
    );
    assert!(
        runtime
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "query".into(),
                json!({}),
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
}
