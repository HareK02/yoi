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
    // admission fence; only exact Session attribution and startup read callbacks reenter below.
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
        let startup_path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/journal/startup");
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            app.clone().oneshot(runtime_source_request(
                &identity,
                Some(worker_id),
                "GET",
                &startup_path,
                Vec::new(),
            )),
        )
        .await
        .expect("startup snapshot must not wait for Runtime's occupied Host parents")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: subjektiv::journal::startup::JournalStartupSnapshot =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(snapshot.subject_id, *subject_id);
        assert!(snapshot.journals.is_empty());
        assert!(!snapshot.omitted);
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
async fn rejected_create_releases_key_but_retains_subject_occupancy_without_release_proof() {
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
            "manual:rejected-create",
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
        assert_eq!(
            api.store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, "subjektiv:rejected-create")
                .unwrap()
                .expect("failed creation is not explicit Subject release authority")
                .worker,
            worker,
        );
        // Retained Subject occupancy is not a live create reservation. A new
        // manual request must not send the terminal Worker's identity to Runtime.
        let retry = api.config_store.reserve_worker_create(
            TEST_WORKSPACE_ID,
            EMBEDDED_RUNTIME_ID,
            "manual:retry-rejected-create",
            &"f".repeat(64),
            Some("subjektiv:rejected-create"),
            &memory,
        );
        assert!(
            matches!(retry, Err(Error::RepositoryConflict(ref message))
                if message.contains("worker_singleton_release_required")),
            "terminal reservation must fail before Runtime dispatch: {retry:?}"
        );
    }
}

async fn unpublished_recovery_fixture(
    root: &std::path::Path,
) -> (
    WorkspaceApi,
    Arc<DeterministicExecutionBackend>,
    String,
    crate::store::WorkerCreateReservation,
) {
    let (api, backend) = test_api_with_recording_backend(root).await;
    let subject = open_subjektiv_store(&api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("recovery-test").unwrap())
        .unwrap();
    let key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let old = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            EMBEDDED_RUNTIME_ID,
            "manual:failed-subject",
            &"f".repeat(64),
            Some(&key),
            &memory,
        )
        .unwrap();
    api.config_store
        .fail_worker_create_reservation(
            TEST_WORKSPACE_ID,
            EMBEDDED_RUNTIME_ID,
            old.worker_id,
            &old.create_fingerprint,
        )
        .unwrap();
    (api, backend, subject.id, old)
}

fn recovery_spawn_request(subject: &str) -> WorkerSpawnRequest {
    serde_json::from_value(json!({
        "intent": WorkerSpawnIntent::WorkspaceCompanion,
        "acceptance": WorkerSpawnAcceptanceRequirement::RunAccepted { expected_segments: 0 },
        "profile": worker_runtime::catalog::ProfileSelector::Builtin("builtin:companion".into()),
        "singleton_key": crate::subjektiv::subject_worker_singleton_key(subject).unwrap(),
    }))
    .unwrap()
}

#[tokio::test]
async fn unpublished_create_recovery_retries_same_fence_then_creates_new_worker() {
    let root = tempfile::tempdir().unwrap();
    let (api, backend, subject, old) = unpublished_recovery_fixture(root.path()).await;
    let old_worker = RuntimeWorkerRef::new(EMBEDDED_RUNTIME_ID, old.worker_id.to_string());
    let lease = old.singleton.as_ref().unwrap();
    let late = crate::subjektiv::SubjectSessionAttribution::new(
        &subject,
        EMBEDDED_RUNTIME_ID,
        old.worker_id.to_string(),
        session_store::new_session_id().to_string(),
    )
    .unwrap();
    let late_session = late.session_id.clone();
    // Models an already-authorized callback arriving while Runtime fencing is
    // in flight. The Host intent must reject it BEFORE attribution insertion.
    let callback_api = api.clone();
    *backend.recovery_hook.lock().unwrap() = Some(Box::new(move || {
        assert!(subjektiv_journal::bind_host_session(&callback_api, late).is_err());
    }));
    *backend.recovery_failure.lock().unwrap() = Some("unconfirmed execution absence".into());
    assert!(
        api.spawn_workspace_worker(EMBEDDED_RUNTIME_ID, recovery_spawn_request(&subject))
            .is_err()
    );
    assert!(
        api.config_store
            .subject_release_pending(TEST_WORKSPACE_ID, &old_worker)
            .unwrap()
    );
    assert_eq!(
        api.config_store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
            .unwrap(),
        Some(lease.clone())
    );
    let store = open_subjektiv_store(&api).unwrap();
    assert!(store.session_attribution(&late_session).unwrap().is_none());
    assert!(
        store
            .journal_body_is_unpublished(&subject, EMBEDDED_RUNTIME_ID, &old_worker.worker_id)
            .unwrap()
    );

    let replacement = api
        .spawn_workspace_worker(EMBEDDED_RUNTIME_ID, recovery_spawn_request(&subject))
        .unwrap();
    assert_eq!(replacement.state, InternalWorkerOperationState::Accepted);
    let replacement = replacement.worker.unwrap().worker;
    assert_ne!(replacement, old_worker);
    let current = api
        .config_store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
        .unwrap()
        .unwrap();
    assert_eq!(current.worker, replacement);
    assert_eq!(current.generation, lease.generation + 1);
    let requests = backend.recovery_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0], requests[1],
        "unknown result must reuse exact recovery identity"
    );
    // A stale completion cannot release the replacement generation.
    api.config_store
        .complete_subject_create_recovery(
            TEST_WORKSPACE_ID,
            lease,
            &requests[0].operation_id,
            &old.create_fingerprint,
        )
        .unwrap();
    assert_eq!(
        api.config_store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
            .unwrap(),
        Some(current)
    );
}

#[tokio::test]
async fn unpublished_create_recovery_refuses_any_attribution_or_orphan_writer() {
    for orphan_writer in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (api, backend, subject, old) = unpublished_recovery_fixture(root.path()).await;
        let store = open_subjektiv_store(&api).unwrap();
        if orphan_writer {
            store
                .claim_journal_writer(&subject, EMBEDDED_RUNTIME_ID, &old.worker_id.to_string())
                .unwrap();
        } else {
            store
                .record_session_attribution(
                    crate::subjektiv::SubjectSessionAttribution::new(
                        &subject,
                        EMBEDDED_RUNTIME_ID,
                        old.worker_id.to_string(),
                        session_store::new_session_id().to_string(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        assert!(
            api.spawn_workspace_worker(EMBEDDED_RUNTIME_ID, recovery_spawn_request(&subject))
                .is_err()
        );
        assert!(backend.recovery_requests.lock().unwrap().is_empty());
        assert_eq!(
            api.config_store
                .current_worker_singleton_owner(
                    TEST_WORKSPACE_ID,
                    &old.singleton.as_ref().unwrap().key
                )
                .unwrap(),
            old.singleton
        );
    }
}

#[tokio::test]
async fn unpublished_create_recovery_resumes_after_runtime_proof_before_host_commit() {
    let root = tempfile::tempdir().unwrap();
    let (api, backend, subject, old) = unpublished_recovery_fixture(root.path()).await;
    let lease = old.singleton.as_ref().unwrap();
    let op = format!(
        "subject-create-recovery:{}:{}",
        old.worker_id, lease.generation
    );
    let fingerprint = api
        .config_store
        .prepare_subject_create_recovery(TEST_WORKSPACE_ID, lease, &op)
        .unwrap();
    let request = runtime_api::SubjectCreateRecoveryRequest {
        workspace_id: TEST_WORKSPACE_ID.into(),
        worker_id: old.worker_id,
        create_fingerprint: fingerprint,
        operation_id: op,
    };
    api.runtime
        .recover_subject_create(EMBEDDED_RUNTIME_ID, request)
        .unwrap();
    // Simulate loss of the receipt before Host completion. The Host must not
    // infer completion from missing catalog state, and can retry the exact op.
    assert_eq!(
        api.config_store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
            .unwrap(),
        old.singleton
    );
    subjektiv_journal::recover_failed_subject_create(&api, &subject).unwrap();
    assert!(
        api.config_store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
            .unwrap()
            .is_none()
    );
    assert_eq!(backend.recovery_requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn failed_subject_spawn_can_recover_on_next_explicit_create() {
    let root = tempfile::tempdir().unwrap();
    let (api, backend) = test_api_with_recording_backend(root.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("failed-spawn").unwrap())
        .unwrap();
    let key = crate::subjektiv::subject_worker_singleton_key(&subject.id).unwrap();
    *backend.spawn_failure.lock().unwrap() =
        Some("Session registration failed before publication".into());
    let rejected = api
        .spawn_workspace_worker(EMBEDDED_RUNTIME_ID, recovery_spawn_request(&subject.id))
        .unwrap();
    assert_eq!(rejected.state, InternalWorkerOperationState::Rejected);
    let old = api
        .config_store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
        .unwrap()
        .unwrap();
    assert!(
        api.config_store
            .subject_create_recovery_candidate(TEST_WORKSPACE_ID, &old)
            .unwrap()
            .is_some()
    );
    assert!(
        backend.recovery_requests.lock().unwrap().is_empty(),
        "failure alone must not release occupancy"
    );
    let result = api
        .spawn_workspace_worker(EMBEDDED_RUNTIME_ID, recovery_spawn_request(&subject.id))
        .unwrap();
    assert_eq!(result.state, InternalWorkerOperationState::Accepted);
    let current = api
        .config_store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
        .unwrap()
        .unwrap();
    assert_ne!(current.worker, old.worker);
    assert_eq!(current.generation, old.generation + 1);
    assert_eq!(backend.recovery_requests.lock().unwrap().len(), 1);
}
