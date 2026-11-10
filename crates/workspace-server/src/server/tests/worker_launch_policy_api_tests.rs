//! Real generated routes, owner authentication, SQLite and live permission registry.
//! Runtime execution uses the shared deterministic fixture; no live environment is used.
use super::*;
use server_api::launch_policy::{WorkerLaunchPolicyAction, WorkerLaunchPolicyMutation};

async fn request(
    app: Router,
    method: &str,
    path: &str,
    token: &str,
    input: Value,
) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"text": String::from_utf8_lossy(&bytes)}));
    (status, body)
}

#[tokio::test]
async fn registered_launch_policy_routes_enforce_owner_and_preserve_replay_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "launch-policy-route-owner").await;
    let foreign = seed_test_api_token(api.store.as_ref(), "launch-policy-route-foreign");
    let app = build_router(api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/worker-launch-policies");
    let mutation = WorkerLaunchPolicyMutation {
        operation_key: "create-route-policy".into(),
        policy_id: "route-policy".into(),
        expected: None,
        action: WorkerLaunchPolicyAction::Create {
            name: "Route policy".into(),
            allowed_profiles: vec!["builtin:companion".into()],
            child_permissions: Default::default(),
        },
    };
    let input = serde_json::to_value(&mutation).unwrap();
    let (status, body) = request(app.clone(), "POST", &path, &foreign, input.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = request(app.clone(), "GET", &path, &owner, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["policies"], json!([]));

    let (status, receipt) = request(app.clone(), "POST", &path, &owner, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, replay) = request(app.clone(), "POST", &path, &owner, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, receipt);
    let (status, body) = request(app.clone(), "GET", &path, &owner, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["policies"].as_array().unwrap().len(), 1);
    assert_eq!(body["policies"][0]["policy_id"], "route-policy");
    let key = server_api::launch_policy::launch_policy_usage_key("route-policy").unwrap();
    assert!(
        api.permission_registry
            .read()
            .unwrap()
            .host()
            .catalog()
            .namespaces
            .iter()
            .flat_map(|namespace| &namespace.declaration.permissions)
            .any(|operation| operation.key == key)
    );
    let mut different = input;
    different["action"]["name"] = json!("Different input");
    let (status, body) = request(app.clone(), "POST", &path, &owner, different).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        api.config_store
            .worker_launch_policy(TEST_WORKSPACE_ID, "route-policy")
            .unwrap()
            .unwrap()
            .name,
        "Route policy"
    );
    let original = api
        .config_store
        .worker_launch_policy(TEST_WORKSPACE_ID, "route-policy")
        .unwrap()
        .unwrap();
    let rename = WorkerLaunchPolicyMutation {
        operation_key: "rename-route-policy".into(),
        policy_id: original.policy_id.clone(),
        expected: Some(original.observation()),
        action: WorkerLaunchPolicyAction::Rename {
            name: "Renamed".into(),
        },
    };
    let (status, body) = request(
        app.clone(),
        "POST",
        &path,
        &owner,
        serde_json::to_value(&rename).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stale = WorkerLaunchPolicyMutation {
        operation_key: "stale-route-policy".into(),
        action: WorkerLaunchPolicyAction::Revoke,
        ..rename
    };
    let (status, body) = request(
        app.clone(),
        "POST",
        &path,
        &owner,
        serde_json::to_value(stale).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let current = api
        .config_store
        .worker_launch_policy(TEST_WORKSPACE_ID, "route-policy")
        .unwrap()
        .unwrap();
    assert_eq!(current.name, "Renamed");
    assert_eq!(
        current.state,
        server_api::launch_policy::WorkerLaunchPolicyState::Active
    );
    let (status, historical) = request(
        app,
        "POST",
        &path,
        &owner,
        serde_json::to_value(mutation).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{historical}");
    assert_eq!(historical, receipt);
}

#[tokio::test]
async fn orchestrator_start_rejects_foreign_actor_before_reserving_worker() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let foreign = seed_test_api_token(api.store.as_ref(), "orchestrator-route-foreign");
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/orchestrator");
    let (status, body) = request(
        build_router(api.clone()),
        "POST",
        &path,
        &foreign,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(api.config_store.with_conn(|conn| {
        Ok(conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM worker_create_reservations WHERE workspace_id=?1)", [TEST_WORKSPACE_ID], |row| row.get::<_, bool>(0))?)
    }).unwrap());
}
