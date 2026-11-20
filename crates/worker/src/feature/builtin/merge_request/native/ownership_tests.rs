//! Tool/native ownership connection tests use the production contracts, WIP runtime and dispatcher.
//! Only Workspace HTTP is replaced; Backend CAS persistence and authorization decisions are not mocked proofs.

use super::super::tests::{RecordingWorkspaceClient, response};
use super::tests::{arguments, roots, runtime};
use super::*;
use crate::worker::WorkspaceResponse;

fn set_input() -> Value {
    json!({"merge_request_id":"MR-1", "operation_id":"assign-1", "expected_operation_id":null,
        "worker":{"runtime_id":"runtime-1","worker_id":"worker-1"}})
}

fn native_set(input: &Value) -> Value {
    let nullable = |value: &Value| {
        if value.is_null() {
            json!({"$case":"null"})
        } else {
            json!({"$case":"value","value":value})
        }
    };
    json!({"operation_id":input["operation_id"],
        "expected_operation_id":nullable(&input["expected_operation_id"]),
        "worker":nullable(&input["worker"])})
}

#[tokio::test]
async fn ownership_tool_requires_explicit_nullable_cas_and_valid_identity_before_dispatch() {
    let schema = Kind::WorkerSet.schema();
    for field in [
        "merge_request_id",
        "operation_id",
        "expected_operation_id",
        "worker",
    ] {
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field)),
            "{field}"
        );
    }
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let tool = MergeRequestTool {
        client: client.clone(),
        kind: Kind::WorkerSet,
    };
    for field in ["expected_operation_id", "worker"] {
        let mut missing = set_input();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            tool.execute(&missing.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
    }
    for (field, value) in [
        ("operation_id", json!("")),
        ("expected_operation_id", json!(" ")),
        ("worker", json!({"runtime_id":"runtime-1"})),
        ("worker", json!({"runtime_id":"", "worker_id":"worker-1"})),
        (
            "worker",
            json!({"runtime_id":"runtime-1", "worker_id":"worker\n1"}),
        ),
        (
            "worker",
            json!({"runtime_id":"runtime-1", "worker_id":"worker-1", "authority":true}),
        ),
        ("unexpected", json!(true)),
    ] {
        let mut input = set_input();
        input[field] = value;
        assert!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .is_err(),
            "{field}"
        );
    }
    assert!(client.authorizations.lock().unwrap().is_empty());
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ownership_tool_and_native_preserve_assign_replace_release_and_exact_retry_inputs() {
    for (expected, worker) in [
        (
            Value::Null,
            json!({"runtime_id":"runtime-1","worker_id":"worker-1"}),
        ),
        (
            json!("assign-previous"),
            json!({"runtime_id":"runtime-2","worker_id":"worker-2"}),
        ),
        (json!("assign-previous"), Value::Null),
    ] {
        let receipt = json!({"event_id":"event-1", "operation_id":"assign-1", "previous_operation_id":expected, "worker":worker});
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(receipt.clone());
            3
        ]));
        let mut input = set_input();
        input["expected_operation_id"] = expected;
        input["worker"] = worker;
        let tool = MergeRequestTool {
            client: client.clone(),
            kind: Kind::WorkerSet,
        };
        let output = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.content.as_deref().unwrap()).unwrap(),
            receipt
        );
        let runtime = runtime(client.clone());
        for _ in 0..2 {
            // A second explicit invocation is an exact retry, not adapter-owned recovery.
            runtime
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    "worker_set".into(),
                    native_set(&input),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            assert_eq!(request.method, WorkspaceRequestMethod::Put);
            assert_eq!(request.path, "/api/w/ws/merge-requests/MR-1/worker");
            assert_eq!(request.body, requests[0].body);
            let body: server_api::SetMergeRequestWorkerRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert_eq!(body.operation_id, "assign-1");
            assert_eq!(
                json!(body.expected_operation_id),
                input["expected_operation_id"]
            );
            assert_eq!(json!(body.worker), input["worker"]);
        }
        let authorizations = client.authorizations.lock().unwrap();
        assert_eq!(authorizations.len(), 3);
        for authorization in authorizations.iter() {
            assert_eq!(authorization.operation, "merge_request.assign");
            assert_eq!(
                authorization.resource,
                PermissionResource::MergeRequest {
                    workspace_id: "ws".into(),
                    merge_request_id: "MR-1".into(),
                }
            );
        }
    }
}

#[tokio::test]
async fn ownership_query_preserves_never_assigned_and_released_observations_without_mutation() {
    for observation in [
        json!({"assignment":null}),
        json!({"assignment":{"operation_id":"release-1","worker":null}}),
    ] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(
                observation.clone()
            );
            2
        ]));
        let tool = MergeRequestTool {
            client: client.clone(),
            kind: Kind::WorkerQuery,
        };
        let result = tool
            .execute(
                r#"{"merge_request_id":"MR/1"}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(result.content.as_deref().unwrap()).unwrap(),
            observation
        );
        let runtime = runtime(client.clone());
        let output = runtime
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "worker_query".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let result: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(result["data"], observation);
        assert_eq!(result["entry"], "/changes/MR-1");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/api/w/ws/merge-requests/MR%2F1/worker");
        assert_eq!(requests[1].path, "/api/w/ws/merge-requests/MR-1/worker");
        assert!(
            requests
                .iter()
                .all(|r| r.method == WorkspaceRequestMethod::Get && r.body.is_none())
        );
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
async fn native_ownership_rejects_omitted_plain_null_or_spoofed_route_before_authorization() {
    let declaration = contract(Kind::WorkerSet).declaration(&roots().contract_roots());
    assert!(declaration.parameters.iter().all(|p| p.required));
    for parameter in declaration.parameters.iter().skip(1) {
        assert!(matches!(
            parameter.r#type,
            wip_protocol::TypeExpr::Union { .. }
        ));
    }
    let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
    let runtime = runtime(client.clone());
    for (field, value) in [
        ("expected_operation_id", None),
        ("worker", None),
        ("expected_operation_id", Some(Value::Null)),
        ("worker", Some(Value::Null)),
        (
            "worker",
            Some(json!({"$case":"value", "value":{"worker_id":"worker-1"}})),
        ),
        ("merge_request_id", Some(json!("MR-2"))),
    ] {
        let mut input = native_set(&set_input());
        match value {
            Some(value) => input[field] = value,
            None => {
                input.as_object_mut().unwrap().remove(field);
            }
        }
        assert!(
            runtime
                .invoke(
                    "/changes/MR-1".into(),
                    crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                    "worker_set".into(),
                    input,
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
    }
    assert!(
        runtime
            .invoke(
                "/changes/MR-1".into(),
                crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
                "worker_query".into(),
                json!({"merge_request_id":"MR-2"}),
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .invoke(
                "/changes".into(),
                crate::wip::root_reference(MERGE_REQUEST_COLLECTION_INTERFACE),
                "worker_set".into(),
                native_set(&set_input()),
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert!(client.authorizations.lock().unwrap().is_empty());
    assert!(client.requests.lock().unwrap().is_empty());
}

fn context() -> WipCallContext {
    WipCallContext {
        result_namespaces: Default::default(),
        execution: ToolExecutionContext::direct(),
        security_context: "test".into(),
    }
}

#[tokio::test]
async fn ownership_denial_is_before_dispatch_for_both_surfaces_and_both_operations() {
    for kind in [Kind::WorkerQuery, Kind::WorkerSet] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
        client
            .auth_denied
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let input = if kind == Kind::WorkerSet {
            set_input()
        } else {
            json!({"merge_request_id":"MR-1"})
        };
        let tool = MergeRequestTool {
            client: client.clone(),
            kind,
        };
        assert!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
        let native = native_merge_request_tools(
            client.clone(),
            MergeRequestFeatureConfig {
                worker_query: true,
                worker_set: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .find(|t| t.kind == kind)
        .unwrap();
        let args = if kind == Kind::WorkerSet {
            arguments(native_set(&input))
        } else {
            BTreeMap::new()
        };
        assert!(matches!(
            execute_native_merge_request_tool(&native, &args, Some("MR-1"), context()).await,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert_eq!(client.authorizations.lock().unwrap().len(), 2);
        assert!(client.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn ownership_rejection_and_unknown_outcome_never_retry_or_refresh_cas() {
    for (status, body, code) in [
        (403, "denied", Some(ProtocolErrorCode::PermissionDenied)),
        (404, "missing", Some(ProtocolErrorCode::NotFound)),
        (
            409,
            "stale assignment",
            Some(ProtocolErrorCode::InvalidArguments),
        ),
        (503, "unavailable", None),
        (200, "invalid JSON", None),
        (0, "transport failure", None),
    ] {
        // Empty responses inject a post-dispatch transport error in the recording client.
        let responses = if status == 0 {
            vec![]
        } else {
            vec![
                WorkspaceResponse {
                    status,
                    body: body.into(),
                    content_type: None
                };
                2
            ]
        };
        let client = Arc::new(RecordingWorkspaceClient::new(responses));
        let tool = MergeRequestTool {
            client: client.clone(),
            kind: Kind::WorkerSet,
        };
        let error = tool
            .execute(&set_input().to_string(), ToolExecutionContext::direct())
            .await
            .unwrap_err();
        if code.is_none() {
            assert!(error.to_string().contains("outcome is unknown"));
            assert!(
                error
                    .to_string()
                    .contains("original operation_id and full input")
            );
        }
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        let native = native_merge_request_tools(
            client.clone(),
            MergeRequestFeatureConfig {
                worker_set: true,
                ..Default::default()
            },
        )
        .unwrap()
        .remove(0);
        let result = execute_native_merge_request_tool(
            &native,
            &arguments(native_set(&set_input())),
            Some("MR-1"),
            context(),
        )
        .await;
        match code {
            Some(expected) => assert!(
                matches!(result, Err(WipOperationError::Protocol(ProtocolError { code, .. })) if code == expected)
            ),
            None => assert!(matches!(result, Err(WipOperationError::OutcomeUnknown(_)))),
        }
        let requests = client.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "neither adapter may retry, reread or create a new operation"
        );
        assert_eq!(requests[0].body, requests[1].body);
    }
}

#[test]
fn ownership_flags_publish_independent_tool_and_native_operations_without_grants() {
    for (config, tool_name, operation) in [
        (
            MergeRequestFeatureConfig {
                worker_query: true,
                ..Default::default()
            },
            "QueryMergeRequestWorker",
            "worker_query",
        ),
        (
            MergeRequestFeatureConfig {
                worker_set: true,
                ..Default::default()
            },
            "SetMergeRequestWorker",
            "worker_set",
        ),
    ] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
        let definitions = enabled_merge_request_definitions(client.clone(), config);
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0]().0.name, tool_name);
        let tools = native_merge_request_tools(client.clone(), config).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].contract.name, operation);
        assert_eq!(tools[0].projection.surface, NativeMergeRequestSurface::Item);
        assert!(
            !tools[0]
                .contract
                .parameters
                .iter()
                .any(|p| p.name == "merge_request_id")
        );
        assert!(config.any());
        assert!(client.authorizations.lock().unwrap().is_empty());
        assert!(client.requests.lock().unwrap().is_empty());
    }
}
