//! Signed self Workdir HTTP -> real Ticket assignment/storage/capability checks
//! -> local provider. Only Runtime session lookup is replaced by the fixture.
use super::*;

#[tokio::test]
async fn self_workdir_operations_ignore_creation_selector_but_enforce_current_repository_access() {
    use workdir::WorkdirSessionCapabilities as Caps;
    let mut fixture = manual_worker_assignment_fixture().await;
    let Json(assignment) = scoped_set_ticket_assignment(
        State(fixture.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            fixture.ticket_id.clone(),
            "worker".into(),
        )),
        Json(manual_worker_assignment_request(
            &fixture,
            "selector-provenance",
        )),
    )
    .await
    .unwrap();
    assert!(assignment.assignment.is_some());
    for id in [&fixture.main_workdir_id, &fixture.docs_workdir_id] {
        let mut record = fixture
            .api
            .store
            .get_workdir_registry(TEST_WORKSPACE_ID, id)
            .unwrap()
            .unwrap();
        record.creation_selector = Some("HEAD".into());
        record.current_selector = Some("work/topic".into());
        fixture.api.store.upsert_workdir_registry(&record).unwrap();
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("proof.txt"), "provenance is not authority").unwrap();
    let provider_root = root.path().to_path_buf();
    let scope = manifest::Scope::writable(&provider_root).unwrap();
    fixture.runtime.workdir_session_factory = Some(Arc::new(move |id| {
        Ok(Arc::new(workdir::LocalWorkdirSession::materialized_bound(
            workdir::Workdir::new(id),
            provider_root.clone(),
            provider_root.clone(),
            manifest::SharedScope::new(scope.clone()),
            Caps::ALL,
        )) as WorkdirSessionHandle)
    }));
    fixture
        .api
        .runtime
        .register_or_replace(fixture.runtime.clone());
    let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let app = build_router(fixture.api.clone());
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let request = |alias: &str, operation: Value| {
        runtime_source_request(
            &identity,
            Some(&fixture.worker.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&json!({"target_workdir": alias, "operation": operation})).unwrap(),
        )
    };
    let read = json!({"operation":"read", "request":{"path":"proof.txt", "offset":0, "limit":10, "max_bytes":1024}});
    for alias in ["checkout", "docs"] {
        let (status, body, events) =
            request_with_logs(app.clone(), request(alias, read.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let WorkdirSessionOperationResult::Read(read) = serde_json::from_value(body).unwrap()
        else {
            panic!("expected file content");
        };
        assert_eq!(read.bytes, b"provenance is not authority");
        assert!(events.is_empty(), "{events:?}");
    }
    let command = json!({"operation":"command_start", "request":{
        "command":"printf selector-provenance", "cwd":"", "timeout_secs":1,
        "transport_byte_budget":1048576, "tool_call_id":"selector-provenance-command"
    }});
    let (status, body, events) =
        request_with_logs(app.clone(), request("checkout", command.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(events.is_empty(), "{events:?}");
    let WorkdirSessionOperationResult::CommandStart(handle) = serde_json::from_value(body).unwrap()
    else {
        panic!("expected a registered command");
    };
    let (status, body, _) = request_with_logs(
        app.clone(),
        request(
            "checkout",
            json!({
                "operation":"command_cancel", "request":handle
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for operation in [
        command,
        json!({"operation":"write", "request":{"path":"blocked.txt", "content":[120]}}),
    ] {
        let (status, body, _) = request_with_logs(app.clone(), request("docs", operation)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    assert!(!root.path().join("blocked.txt").exists());
    let backend = browser_ticket_backend(&fixture.api).unwrap();
    backend
        .edit_item(
            fixture.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Set {
                    targets: vec![
                        test_ticket_target(
                            "test-repository",
                            "release",
                            TicketTargetAccess::ReadOnly,
                        ),
                        test_ticket_target("docs", "release", TicketTargetAccess::ReadWrite),
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    for alias in ["checkout", "docs"] {
        let (status, body, _) = request_with_logs(app.clone(), request(alias, read.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body, _) = request_with_logs(
            app.clone(),
            request(
                alias,
                json!({
                    "operation":"write", "request":{"path":"blocked.txt", "content":[120]}
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    assert!(!root.path().join("blocked.txt").exists());
    backend
        .edit_item(
            fixture.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Clear),
                ..Default::default()
            },
        )
        .unwrap();
    let (status, body, _) = request_with_logs(app, request("checkout", read)).await;
    assert!(!status.is_success(), "{body}");
    for id in [&fixture.main_workdir_id, &fixture.docs_workdir_id] {
        assert_eq!(
            fixture
                .api
                .store
                .get_workdir_registry(TEST_WORKSPACE_ID, id)
                .unwrap()
                .unwrap()
                .creation_selector
                .as_deref(),
            Some("HEAD")
        );
    }
}
