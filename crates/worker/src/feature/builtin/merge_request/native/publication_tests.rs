//! Production Tool/native adapters with only the HTTP transport replaced.
use super::super::tests::{RecordingWorkspaceClient, response};
use super::tests::runtime;
use super::*;

#[tokio::test]
async fn publication_and_comment_tool_native_dispatch_preserve_binding_and_replay_input() {
    for (kind, name, operation, suffix, input, receipt) in [
        (
            Kind::Publish,
            "publish",
            "merge_request.publish",
            "publications",
            json!({"operation_id":"publication-1", "approval_event_id":"approval-1"}),
            json!({"merge_request_id":"MR-1", "operation_id":"publication-1", "outcome":{"kind":"unconfirmed"}}),
        ),
        (
            Kind::Comment,
            "comment",
            "merge_request.comment",
            "comments",
            json!({"operation_id":"comment-1", "expected_owner_operation_id":"owner-1", "body":"Conflict resolved.\nTests passed."}),
            json!({"operation_id":"comment-1", "event_id":"event-1"}),
        ),
        (
            Kind::Publications,
            "publications",
            "merge_request.read",
            "publications",
            json!({}),
            json!({"publications":[]}),
        ),
    ] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(receipt.clone());
            3
        ]));
        let mut tool_input = input.clone();
        tool_input["merge_request_id"] = json!("MR-1");
        let tool = MergeRequestTool {
            client: client.clone(),
            kind,
        };
        tool.execute(&tool_input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let native = runtime(client.clone());
        let native_calls = if kind.mutating() { 2 } else { 1 };
        for _ in 0..native_calls {
            let output = native
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    name.into(),
                    input.clone(),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(value["data"], receipt);
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1 + native_calls);
        for request in requests.iter() {
            assert_eq!(
                request.path,
                format!("/api/w/ws/merge-requests/MR-1/{suffix}")
            );
            assert_eq!(request.body, requests[0].body);
            assert_eq!(
                request.method,
                if kind == Kind::Publications {
                    WorkspaceRequestMethod::Get
                } else {
                    WorkspaceRequestMethod::Post
                }
            );
            if kind != Kind::Publications {
                assert_eq!(
                    serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
                    input
                );
            }
        }
        assert!(
            client
                .authorizations
                .lock()
                .unwrap()
                .iter()
                .all(|a| a.operation == operation)
        );
    }
}

#[tokio::test]
async fn publication_invalid_input_and_denied_authority_never_dispatch() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let native = runtime(client.clone());
    for input in [
        json!({"operation_id":"p"}),
        json!({"operation_id":"p", "approval_event_id":""}),
        json!({"operation_id":"p", "approval_event_id":"a", "merge_request_id":"OTHER"}),
    ] {
        assert!(
            native
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    "publish".into(),
                    input,
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
    }
    assert!(client.requests.lock().unwrap().is_empty());
    client
        .auth_denied
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        native
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "publish".into(),
                json!({"operation_id":"p", "approval_event_id":"a"}),
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn publication_unknown_transport_outcome_does_not_retry() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        crate::worker::WorkspaceResponse {
            status: 500,
            body: "unavailable".into(),
            content_type: None,
        },
    ]));
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Publish,
    };
    let error = tool
        .execute(
            r#"{"merge_request_id":"MR-1","operation_id":"p","approval_event_id":"a"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("outcome is unknown"), "{error}");
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}
