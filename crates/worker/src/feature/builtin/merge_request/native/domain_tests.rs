//! Normal Tool and native adapters deliberately have different inputs but identical domain dispatch.

use super::super::tests::{RecordingWorkspaceClient, response};
use super::tests::{arguments, roots, runtime};
use super::*;

#[tokio::test]
async fn tool_and_native_open_contracts_produce_the_same_authorized_domain_request() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        response(json!({"merge_request_id":"MR-1"})),
        response(json!({"merge_request_id":"MR-1"})),
    ]));
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::Open,
    };
    tool.execute(r#"{"ticket":"T-797","repository_key":"main","selector_from":"work","selector_to":"develop","summary":"candidate"}"#, ToolExecutionContext::direct()).await.unwrap();
    let runtime = runtime(client.clone());
    runtime.invoke("/changes".into(), crate::wip::root_reference(MERGE_REQUEST_COLLECTION_INTERFACE), "open".into(),
        json!({"ticket":"/cases/T-797","repository":{"key":"main","source":"work","target":"develop"},"summary":"candidate"}), ToolExecutionContext::direct()).await.unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, requests[1].path);
    assert_eq!(requests[0].body, requests[1].body);
    let authorizations = client.authorizations.lock().unwrap();
    assert_eq!(authorizations.len(), 2);
    assert_eq!(
        serde_json::to_value(&authorizations[0]).unwrap(),
        serde_json::to_value(&authorizations[1]).unwrap()
    );
    // Neither adapter accepts the other adapter's public form.
    assert!(tool.execute(r#"{"ticket":"/cases/T-797","repository":{"key":"main","source":"work","target":"develop"}}"#, ToolExecutionContext::direct()).await.is_err());
    assert!(convert(&contract(Kind::Open), &arguments(json!({"ticket":"T-797","repository_key":"main","selector_from":"work","selector_to":"develop"})), &roots()).is_err());
}

#[tokio::test]
async fn native_backend_conflict_is_a_domain_rejection_not_a_type_rejection_or_retry() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        crate::worker::WorkspaceResponse {
            status: 409,
            body: "approval snapshot is stale".into(),
        },
    ]));
    let tool = native_merge_request_tools(
        client.clone(),
        MergeRequestFeatureConfig {
            complete: true,
            ..Default::default()
        },
    )
    .unwrap()
    .remove(0);
    let input = arguments(
        json!({"operation_id":"op-1","integration":{"approval_event_id":"approval-1","target":{"before":"before","after":"after"},"strategy":"merge","resolution":"clean"}}),
    );
    let rejected = execute_native_merge_request_tool(
        &tool,
        &None,
        &input,
        Some("MR-1"),
        WipCallContext {
            execution: ToolExecutionContext::direct(),
            security_context: "test".into(),
        },
    )
    .await;
    assert!(
        matches!(rejected, Err(WipOperationError::Protocol(ProtocolError { code:ProtocolErrorCode::InvalidArguments, message })) if message.contains("HTTP 409") && message.contains("approval snapshot is stale"))
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(client.authorizations.lock().unwrap().len(), 1);
}
