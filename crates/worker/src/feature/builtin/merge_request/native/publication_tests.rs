//! Production Tool/native adapters with only the HTTP transport replaced.
use super::super::tests::{RecordingWorkspaceClient, response};
use super::tests::runtime;
use super::*;

#[tokio::test]
async fn show_tool_and_native_preserve_publications_in_summary_and_detail() {
    for view in ["summary", "detail"] {
        let evidence = json!({
            "worker_assignment":{"operation_id":"owner-1","worker":null},
            "publications":[{"operation_id":"publication-1","outcome":{"kind":"unconfirmed"}}]
        });
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(
                evidence.clone()
            );
            2
        ]));
        let tool = MergeRequestTool {
            client: client.clone(),
            kind: Kind::Show,
        };
        let output = tool
            .execute(
                &json!({"merge_request_id":"MR-1","view":view}).to_string(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap(),
            evidence
        );
        let output = runtime(client.clone())
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "read".into(),
                json!({"view":view}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let result: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(result["data"], evidence);
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(
                request.path,
                format!("/api/w/ws/merge-requests/MR-1?view={view}")
            );
            assert_eq!(request.method, WorkspaceRequestMethod::Get);
            assert!(request.body.is_none());
        }
        assert!(
            client
                .authorizations
                .lock()
                .unwrap()
                .iter()
                .all(|a| a.operation == "merge_request.read")
        );
    }
}

#[tokio::test]
async fn comment_tool_native_dispatch_preserves_binding_and_replay_input() {
    let input = json!({"operation_id":"comment-1", "expected_owner_operation_id":"owner-1", "body":"Conflict resolved.\nTests passed."});
    let receipt = json!({"operation_id":"comment-1", "event_id":"event-1"});
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        response(receipt.clone());
        3
    ]));
    let mut tool_input = input.clone();
    tool_input["merge_request_id"] = json!("MR-1");
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Comment,
    };
    tool.execute(&tool_input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let native = runtime(client.clone());
    for _ in 0..2 {
        let output = native
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "comment".into(),
                input.clone(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value["data"], receipt);
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request.path, "/api/w/ws/merge-requests/MR-1/comments");
        assert_eq!(request.body, requests[0].body);
        assert_eq!(request.method, WorkspaceRequestMethod::Post);
        assert_eq!(
            serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
            input
        );
    }
    assert!(
        client
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|a| a.operation == "merge_request.comment")
    );
}

#[tokio::test]
async fn comment_invalid_input_and_denied_authority_never_dispatch() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let native = runtime(client.clone());
    for input in [
        json!({"operation_id":"c"}),
        json!({"operation_id":"c", "expected_owner_operation_id":"o", "body":""}),
        json!({"operation_id":"c", "expected_owner_operation_id":"o", "body":"comment", "merge_request_id":"OTHER"}),
    ] {
        assert!(
            native
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    "comment".into(),
                    input,
                    ToolExecutionContext::direct(),
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
                "comment".into(),
                json!({"operation_id":"c", "expected_owner_operation_id":"o", "body":"comment"}),
                ToolExecutionContext::direct(),
            )
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn comment_unknown_transport_outcome_does_not_retry() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        crate::worker::WorkspaceResponse {
            status: 500,
            body: "unavailable".into(),
            content_type: None,
        },
    ]));
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Comment,
    };
    let error = tool.execute(
        r#"{"merge_request_id":"MR-1","operation_id":"c","expected_owner_operation_id":"o","body":"comment"}"#,
        ToolExecutionContext::direct(),
    ).await.unwrap_err();
    assert!(error.to_string().contains("outcome is unknown"), "{error}");
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn complete_tool_native_dispatch_preserves_binding_and_replay_input() {
    let input = json!({"operation_id":"comment-1", "expected_owner_operation_id":"owner-1", "approval_event_id":"approval-1", "expected_source_ref":"approved-source"});
    let receipt = json!({"operation_id":"comment-1", "event_id":"event-1"});
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        response(receipt.clone());
        3
    ]));
    let mut tool_input = input.clone();
    tool_input["merge_request_id"] = json!("MR-1");
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Complete,
    };
    tool.execute(&tool_input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let native = runtime(client.clone());
    for _ in 0..2 {
        let output = native
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "complete".into(),
                input.clone(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value["data"], receipt);
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request.path, "/api/w/ws/merge-requests/MR-1/complete");
        assert_eq!(request.body, requests[0].body);
        assert_eq!(request.method, WorkspaceRequestMethod::Post);
        assert_eq!(
            serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
            input
        );
    }
    assert!(
        client
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|a| a.operation == "merge_request.complete")
    );
}
