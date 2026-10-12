use super::*;

#[tokio::test(flavor = "current_thread")]
async fn subject_callbacks_finish_before_waiting_host_parents_release_capacity() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let subjektiv = open_subjektiv_store(&api).unwrap();
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let mut callbacks = Vec::new();
    for index in 0..2 {
        let subject = subjektiv
            .create_subject(
                crate::subjektiv::SubjectRole::new(format!("companion-{index}")).unwrap(),
            )
            .unwrap();
        let singleton_key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
        let reserved = api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                "runtime-test",
                &format!("subject-start-{index}"),
                &"f".repeat(64),
                Some(&singleton_key),
                &memory,
            )
            .unwrap();
        callbacks.push((
            subject.id,
            reserved.worker_id.to_string(),
            session_store::new_session_id().to_string(),
        ));
    }
    let server = WorkspaceServerApi::new_for_test(api.config.clone(), api.store.clone());
    // Real dispatcher, source proof authentication, live singleton authority and
    // attribution SQL. Only Runtime's create/restore wait is modeled below; no
    // product process or complete create endpoint is spawned by this route test.
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
    let host = api.host_executors.host().unwrap(); // production 2 workers/16 queued
    let mut parents = Vec::new();
    let mut finish_callbacks = Vec::new();
    for _ in 0..2 {
        let create = gate.admit(false).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (finish_callback, callback_reply) = std::sync::mpsc::channel();
        finish_callbacks.push(finish_callback);
        parents.push(tokio::spawn(run_host_workflow(host.clone(), async move {
            let _create = create;
            entered.send(()).unwrap();
            // The actual Host parent cannot finish until its mandatory callbacks
            // return. Timeout/disconnection is failure cleanup, never success.
            callback_reply
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| {
                    Error::InvalidInput(format!("Runtime callback did not complete: {error}"))
                })?;
            Ok::<_, Error>(())
        })));
        tokio::time::timeout(std::time::Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
    }
    let app = workspace_server_router(server);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions");
    let memory_path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/memory");
    for (_, worker_id, _) in &callbacks {
        // Resident context is also a pure Subject callback used by Runtime
        // bootstrap/restore. It must not reenter the waiting Host lane either.
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            app.clone().oneshot(runtime_source_request(
                &identity,
                Some(worker_id),
                "POST",
                &memory_path,
                serde_json::to_vec(&server_api::SubjektivMemoryBackendRequest {
                    operation: server_api::SubjektivMemoryBackendOperation::ResidentContext(
                        Default::default(),
                    ),
                })
                .unwrap(),
            )),
        )
        .await
        .expect("resident context must not wait for its Host parents")
        .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body:?}");
        let resident: server_api::SubjektivMemoryBackendResponse =
            serde_json::from_slice(&body).unwrap();
        assert!(matches!(
            resident,
            server_api::SubjektivMemoryBackendResponse::ResidentContext(_)
        ));
    }
    // Resident memory is not an allowed deletion-drain callback. Preserve that
    // admission fence; only the exact Session attribution route reenters below.
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    for (subject_id, worker_id, session_id) in &callbacks {
        let body =
            serde_json::to_vec(&json!({ "session_id": session_id, "create_if_missing": true }))
                .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            app.clone().oneshot(runtime_source_request(
                &identity,
                Some(worker_id),
                "POST",
                &path,
                body,
            )),
        )
        .await
        .expect("Session attribution must not wait for its Host parents")
        .unwrap();
        let status = response.status();
        let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{response_body:?}");
        let response: Value = serde_json::from_slice(&response_body).unwrap();
        assert_eq!(response["subject_id"], *subject_id);
        assert_eq!(response["session_id"], *session_id);
        let stored = subjektiv.session_attribution(session_id).unwrap().unwrap();
        assert_eq!(stored.worker_id, *worker_id);
        assert_eq!(stored.subject_id, *subject_id);
    }
    // Neither actual parent nor its admission lease was released to make these
    // callbacks succeed. Only durable attribution completion releases them.
    assert!(parents.iter().all(|parent| !parent.is_finished()));
    assert!(futures::poll!(&mut deletion).is_pending());
    for finish_callback in finish_callbacks {
        finish_callback.send(()).unwrap();
    }
    for parent in parents {
        parent.await.unwrap().unwrap();
    }
    host.drain().await.unwrap();
    api.store.db_executor().drain().await.unwrap();
    let _deletion = tokio::time::timeout(std::time::Duration::from_secs(2), deletion)
        .await
        .unwrap();
    assert!(gate.admit(true).is_none());
}

#[tokio::test]
async fn consolidation_subject_callbacks_are_denied_without_runtime_lookup() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let subjektiv = open_subjektiv_store(&api).unwrap();
    let subject = subjektiv
        .create_subject(crate::subjektiv::SubjectRole::new("companion").unwrap())
        .unwrap();
    let key = format!("{SUBJEKTIV_CONSOLIDATION_SINGLETON_PREFIX}{}", subject.id);
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            "runtime-test",
            "consolidation-start",
            &"f".repeat(64),
            Some(&key),
            &memory,
        )
        .unwrap();
    // runtime-test has no Worker adapter: authority denial must not try to look
    // up its profile/tags through Runtime before rejecting this disallowed role.
    let server = WorkspaceServerApi::new_for_test(api.config.clone(), api.store.clone());
    server
        .apis
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), api.clone());
    server
        .routers
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), build_inner_router(api.clone()));
    let app = workspace_server_router(server);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions");
    let worker_id = reservation.worker_id.to_string();
    let session_id = session_store::new_session_id().to_string();
    for (callback_path, callback_body) in [
        (
            path.clone(),
            serde_json::to_vec(&json!({"session_id": session_id, "create_if_missing": true}))
                .unwrap(),
        ),
        (
            format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/memory"),
            serde_json::to_vec(&server_api::SubjektivMemoryBackendRequest {
                operation: server_api::SubjektivMemoryBackendOperation::ResidentContext(
                    Default::default(),
                ),
            })
            .unwrap(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(runtime_source_request(
                &identity,
                Some(&worker_id),
                "POST",
                &callback_path,
                callback_body,
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "{callback_path}: {body:?}");
    }
    assert!(
        subjektiv
            .session_attribution(&session_id)
            .unwrap()
            .is_none()
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"session_id": session_id, "create_if_missing": true}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
