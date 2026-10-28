//! Normal worker lib coverage from WIP/common backend through the real WorkspaceClient
//! authorization transport. The router substitutes Host/provider transport, using the shared
//! scoped-grant evaluator; it does not prove live Host identity, policy storage or delegation.
use super::*;
use server_api::authorization::{PermissionResource, ScopedPermission};

const A: &str = "main.dcdl";
const B: &str = "profiles/team/deep/model.dcdl";

fn grant(operation: &str, path: &str, recursive: bool) -> ScopedPermission {
    ScopedPermission {
        operation: format!("workspace_config.{operation}"),
        resource: PermissionResource::WorkspaceConfig {
            workspace_id: "workspace-1".into(),
            path: path.into(),
            recursive,
        },
    }
}

fn assert_code<T>(result: Result<T, WipOperationError>, expected: ProtocolErrorCode) {
    match result {
        Err(WipOperationError::Protocol(error)) => assert_eq!(error.code, expected),
        _ => panic!("expected a classified protocol failure"),
    }
}

fn provider_calls(client: &Router, suffix: &str) -> usize {
    client
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request.path.ends_with(suffix))
        .count()
}

#[tokio::test]
async fn wip_scoped_read_returns_allowed_content_denies_other_path_and_rechecks_revocation() {
    let client = Router::new();
    client.state.lock().unwrap().permissions = Some(vec![
        grant("current", "", false),
        grant("attach", "", false),
        grant("observe", A, false),
        grant("observe", B, false),
        grant("read", A, false),
    ]);
    let (feature, runtime) = runtime(client.clone());
    feature
        .attach("scoped", None)
        .await
        .unwrap_or_else(|_| panic!("authorized provider operation failed"));
    for path in [A, B] {
        observe_interface(&runtime, &format!("{CONTENT_ROOT}/{path}"), true).await;
    }
    let output = call(&runtime, &format!("{CONTENT_ROOT}/{A}"), "read", json!({}))
        .await
        .unwrap()
        .content
        .unwrap();
    assert!(output.contains("model = old"), "{output}");
    assert!(!output.contains("model = deep"));
    assert_eq!(provider_calls(&client, "/read"), 1);

    let error = call(&runtime, &format!("{CONTENT_ROOT}/{B}"), "read", json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("PermissionDenied"), "{error}");
    assert!(!error.to_string().contains("model = deep"));
    assert_eq!(
        provider_calls(&client, "/read"),
        1,
        "B must not reach provider read"
    );
    {
        let mut state = client.state.lock().unwrap();
        assert!(
            state.authorizations.iter().any(|request| {
                request.operation == "workspace_config.read"
                    && request.resource == grant("read", B, false).resource
            }),
            "deny must occur at read authorization, not metadata discovery"
        );
        state
            .permissions
            .as_mut()
            .unwrap()
            .retain(|permission| permission.operation != "workspace_config.read");
    }
    let error = call(&runtime, &format!("{CONTENT_ROOT}/{A}"), "read", json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("PermissionDenied"), "{error}");
    assert_eq!(
        provider_calls(&client, "/read"),
        1,
        "prior successful read is not authority for next read"
    );
    assert_eq!(client.state.lock().unwrap().applied_commits, 0);
}

#[tokio::test]
async fn scoped_observe_requires_every_path_and_explicit_recursive_scope_before_dispatch() {
    let client = Router::new();
    let (feature, _) = runtime(client.clone());
    feature
        .attach("setup", None)
        .await
        .unwrap_or_else(|_| panic!("authorized provider operation failed"));
    let backend = backend::WorkspaceConfigBackend::new(client.clone());
    let connection = client
        .state
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .connection_id
        .clone();
    client.state.lock().unwrap().permissions = Some(vec![grant("observe", A, false)]);
    let before = client.state.lock().unwrap().requests.len();
    assert_code(
        backend
            .post::<WorkspaceConfigObserveResponse, _>(
                "/observe",
                &json!({
                    "connection_id":connection, "paths":[A, B], "depth":0
                }),
                false,
            )
            .await,
        ProtocolErrorCode::PermissionDenied,
    );
    assert_eq!(
        client.state.lock().unwrap().requests.len(),
        before,
        "partial allow must not dispatch the batch"
    );

    client.state.lock().unwrap().permissions = Some(vec![grant("observe", "profiles", false)]);
    let request = json!({"connection_id":connection, "paths":["profiles"], "depth":3});
    assert_code(
        backend
            .post::<WorkspaceConfigObserveResponse, _>("/observe", &request, false)
            .await,
        ProtocolErrorCode::PermissionDenied,
    );
    assert_eq!(client.state.lock().unwrap().requests.len(), before);
    client.state.lock().unwrap().permissions = Some(vec![grant("observe", "profiles", true)]);
    let response: WorkspaceConfigObserveResponse = backend
        .post("/observe", &request, false)
        .await
        .unwrap_or_else(|_| panic!("authorized provider operation failed"));
    assert!(response.nodes.iter().any(|node| node.path == B));
    assert_eq!(client.state.lock().unwrap().requests.len(), before + 1);
}

#[tokio::test]
async fn current_authorization_denial_is_distinct_from_authority_outage_without_provider_access() {
    for (status, code) in [
        (401, ProtocolErrorCode::PermissionDenied),
        (403, ProtocolErrorCode::PermissionDenied),
        (503, ProtocolErrorCode::Internal),
    ] {
        let client = Router::new();
        client.state.lock().unwrap().permissions = Some(vec![]);
        client.state.lock().unwrap().authorization_status = Some(status);
        let backend = backend::WorkspaceConfigBackend::new(client.clone());
        assert_code(backend.current_async().await, code);
        let (_, runtime) = runtime(client.clone());
        let message = runtime
            .tree(format!("{CONTENT_ROOT}/{A}"), 0, true)
            .await
            .unwrap_err()
            .to_string();
        if status == 503 {
            assert!(message.contains("Internal"), "{message}");
        } else {
            // Denied metadata is absent, not an authority/transport failure.
            assert!(!message.contains("Internal"), "{message}");
            assert!(!message.contains("model = old"));
        }
        let state = client.state.lock().unwrap();
        assert!(
            state.requests.is_empty(),
            "status {status} reached provider"
        );
        assert!(state.active.is_none());
        assert!(
            state
                .authorizations
                .iter()
                .all(|request| request.operation == "workspace_config.current"
                    && request.resource == grant("current", "", false).resource)
        );
    }
}

fn commit_body(client: &Router, entrypoints: &[&str], changes: Json) -> Json {
    let state = client.state.lock().unwrap();
    json!({
        "connection_id":state.active.as_ref().unwrap().connection_id,
        "validator":state.validator(""),
        "request":{
            "base_digest":state.tree_digest(),
            "entrypoints":entrypoints,
            "changes":changes
        }
    })
}

#[tokio::test]
async fn entrypoint_replacement_requires_explicit_root_grant_even_without_file_changes() {
    let client = Router::new();
    let (feature, _) = runtime(client.clone());
    feature
        .attach("setup", None)
        .await
        .unwrap_or_else(|_| panic!("authorized provider operation failed"));
    let backend = backend::WorkspaceConfigBackend::new(client.clone());
    let request = commit_body(&client, &[B], json!([]));
    client.state.lock().unwrap().permissions = Some(vec![grant("commit", B, false)]);
    let before = client.state.lock().unwrap().requests.len();
    assert_code(
        backend
            .post::<WorkspaceConfigCommitResponse, _>("/commit", &request, true)
            .await,
        ProtocolErrorCode::PermissionDenied,
    );
    {
        let state = client.state.lock().unwrap();
        assert_eq!(state.requests.len(), before);
        assert_eq!(state.applied_commits, 0);
        assert_eq!(state.entrypoints, [A]);
    }
    client.state.lock().unwrap().permissions = Some(vec![grant("commit", "", false)]);
    let response: WorkspaceConfigCommitResponse = backend
        .post("/commit", &request, true)
        .await
        .unwrap_or_else(|_| panic!("authorized provider operation failed"));
    {
        let state = client.state.lock().unwrap();
        assert_eq!(state.applied_commits, 1);
        assert_eq!(state.entrypoints, [B]);
        assert_eq!(state.entries[A].1, "model = old");
        assert_eq!(response.digest, state.tree_digest());
        assert_eq!(response.validator, state.validator(""));
    }
    // Confirm the replacement is returned by the provider, not just accepted as a request.
    client.state.lock().unwrap().permissions = Some(vec![grant("observe", "", false)]);
    let connection = client
        .state
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .connection_id
        .clone();
    let observed: WorkspaceConfigObserveResponse = backend
        .post(
            "/observe",
            &json!({"connection_id":connection, "paths":[""], "depth":0}),
            false,
        )
        .await
        .unwrap_or_else(|_| panic!("authorized observation failed"));
    assert_eq!(observed.entrypoints, [B]);
}

#[tokio::test]
async fn commit_requires_root_and_each_file_grant_including_both_rename_ends() {
    let cases = [
        (
            "create",
            json!({"kind":"create", "path":"new.dcdl", "content_type":"decodal", "content":"model = new"}),
            vec!["", "new.dcdl"],
        ),
        (
            "update",
            json!({"kind":"update", "path":B, "expected_digest":digest("model = deep"), "content":"model = changed"}),
            vec!["", B],
        ),
        (
            "delete",
            json!({"kind":"delete", "path":B, "expected_digest":digest("model = deep")}),
            vec!["", B],
        ),
        (
            "rename",
            json!({"kind":"rename", "from":B, "to":"destination.dcdl", "expected_digest":digest("model = deep")}),
            vec!["", B, "destination.dcdl"],
        ),
    ];
    for (kind, change, required) in cases {
        let client = Router::new();
        let (feature, _) = runtime(client.clone());
        feature
            .attach("setup", None)
            .await
            .unwrap_or_else(|_| panic!("authorized provider operation failed"));
        let backend = backend::WorkspaceConfigBackend::new(client.clone());
        let request = commit_body(&client, &[A], json!([change]));
        let before = client.state.lock().unwrap().requests.len();
        let original = client.state.lock().unwrap().entries.clone();
        for missing in &required {
            client.state.lock().unwrap().permissions = Some(
                required
                    .iter()
                    .filter(|path| *path != missing)
                    .map(|path| grant("commit", path, false))
                    .collect(),
            );
            assert_code(
                backend
                    .post::<WorkspaceConfigCommitResponse, _>("/commit", &request, true)
                    .await,
                ProtocolErrorCode::PermissionDenied,
            );
            let state = client.state.lock().unwrap();
            assert_eq!(state.requests.len(), before, "{kind}, missing {missing:?}");
            assert!(state.commits.is_empty());
            assert_eq!(state.applied_commits, 0);
            assert_eq!(state.entries, original);
            assert_eq!(
                state.authorizations.last().unwrap().resource,
                grant("commit", missing, false).resource
            );
        }
        // No recursive root grant: each exact file grant is independently necessary.
        client.state.lock().unwrap().permissions = Some(
            required
                .iter()
                .map(|path| grant("commit", path, false))
                .collect(),
        );
        let response: WorkspaceConfigCommitResponse = backend
            .post("/commit", &request, true)
            .await
            .unwrap_or_else(|_| panic!("authorized provider operation failed"));
        let state = client.state.lock().unwrap();
        assert_eq!(state.applied_commits, 1, "{kind}");
        assert_eq!(state.requests.len(), before + 1);
        assert_eq!(response.digest, state.tree_digest());
        assert_eq!(response.validator, state.validator(""));
        assert_eq!(state.entries[A], original[A]);
        match kind {
            "create" => assert_eq!(state.entries["new.dcdl"].1, "model = new"),
            "update" => assert_eq!(state.entries[B].1, "model = changed"),
            "delete" => assert!(!state.entries.contains_key(B)),
            "rename" => {
                assert!(!state.entries.contains_key(B));
                assert_eq!(state.entries["destination.dcdl"], original[B]);
            }
            _ => unreachable!(),
        }
    }
}
