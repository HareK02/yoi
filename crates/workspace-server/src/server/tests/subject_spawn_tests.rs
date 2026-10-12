use super::*;

#[tokio::test]
async fn subject_session_callback_completes_while_create_and_deletion_are_waiting() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let subjektiv = open_subjektiv_store(&api).unwrap();
    let subject = subjektiv
        .create_subject(crate::subjektiv::SubjectRole::new("companion").unwrap())
        .unwrap();
    let singleton_key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reserved = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            "runtime-test",
            "subject-start",
            &"f".repeat(64),
            Some(&singleton_key),
            &memory,
        )
        .unwrap();
    let worker_id = reserved.worker_id.to_string();
    let server = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
    // Reuse the fixture API, but exercise the real outer dispatcher and both auth layers.
    server
        .apis
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.to_string(), api.clone());
    server.routers.lock().await.insert(
        TEST_WORKSPACE_ID.to_string(),
        build_inner_router(api.clone()),
    );
    let gate = server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    let create = gate.admit(false).unwrap();
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    let app = workspace_server_router(server);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions");
    let session_id = session_store::new_session_id().to_string();
    let body = serde_json::to_vec(&json!({ "session_id": session_id, "create_if_missing": true }))
        .unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        app.clone().oneshot(runtime_source_request(
            &identity,
            Some(&worker_id),
            "POST",
            &path,
            body.clone(),
        )),
    )
    .await
    .expect("Session attribution must not wait for its own Worker creation")
    .unwrap();
    let status = response.status();
    let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{response_body:?}");
    let response: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(response["subject_id"], subject.id);
    assert_eq!(response["session_id"], session_id);
    let stored = subjektiv.session_attribution(&session_id).unwrap().unwrap();
    assert_eq!(stored.worker_id, worker_id);
    assert_eq!(stored.subject_id, subject.id);

    // The callback lane does not bypass authentication.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(futures::poll!(&mut deletion).is_pending());
    drop(create);
    let _deletion = tokio::time::timeout(std::time::Duration::from_secs(2), deletion)
        .await
        .unwrap();
    assert!(gate.admit(true).is_none());
}

#[tokio::test]
async fn workspace_deletion_waits_for_session_attribution_and_fences_both_lanes() {
    let gate = Arc::new(WorkspaceAdmission::default());
    let attribution = gate.admit(true).unwrap();
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    assert!(gate.admit(false).is_none());
    assert!(gate.admit(true).is_none());
    drop(attribution);
    let deletion = deletion.await;
    assert!(gate.admit(false).is_none());
    assert!(gate.admit(true).is_none());
    drop(deletion);
    assert!(gate.admit(false).is_some());
    assert!(gate.admit(true).is_some());
}

#[tokio::test]
async fn only_exact_authenticated_callbacks_can_reenter_deletion_drain() {
    let gate = Arc::new(WorkspaceAdmission::default());
    let _create = gate.admit(false).unwrap();
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    for (method, path) in [
        (Method::POST, "/api/w/workspace-a/subjektiv/sessions"),
        (Method::POST, "/api/w/workspace-a/subjektiv/memory"),
        (Method::POST, "/api/w/workspace-b/subjektiv/sessions"),
        (Method::POST, "/api/w/workspace-a/subjektiv/sessions/extra"),
        (Method::DELETE, "/api/w/workspace-a/subjektiv/sessions"),
        (Method::POST, "/api/w/workspace-a/workers"),
    ] {
        let available = gate
            .admit(workspace_request_is_callback(&method, path, "workspace-a"))
            .is_some();
        assert_eq!(
            available,
            method == Method::POST && path == "/api/w/workspace-a/subjektiv/sessions",
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn rejected_create_releases_reservation_and_key_without_cleanup_errors() {
    let workspace = tempfile::tempdir().unwrap();
    let api = test_api(workspace.path()).await;
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reserved = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            EMBEDDED_RUNTIME_ID,
            "rejected-create",
            &"f".repeat(64),
            Some("subjektiv:rejected-create"),
            &memory,
        )
        .unwrap();
    let worker = RuntimeWorkerRef::new(EMBEDDED_RUNTIME_ID, reserved.worker_id.to_string());
    assert!(
        api.store
            .worker_resource_key(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_some()
    );
    let context = WorkerSpawnCompensationContext {
        assignment: None,
        spawned_workdir_ids: &[],
    };
    for _ in 0..2 {
        let diagnostics = compensate_failed_workspace_worker_create(
            &api,
            EMBEDDED_RUNTIME_ID,
            reserved.worker_id,
            &reserved.create_fingerprint,
            "runtime_spawn_rejected",
            None,
            &context,
            &[],
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(
            !api.store
                .has_active_worker_create_reservation(TEST_WORKSPACE_ID, &worker)
                .unwrap()
        );
        assert!(
            api.store
                .worker_resource_key(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, "subjektiv:rejected-create")
                .unwrap()
                .is_none()
        );
    }
}
