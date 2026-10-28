//! Real router/authentication, feature store and Backend DB boundaries. Runtime
//! execution/unknown and restart fences are covered in worker-runtime and the
//! canonical removal suites, rather than claiming HTTP acceptance proves shutdown.
use super::*;

async fn delete_request(
    api: &WorkspaceApi,
    subject: &str,
    key: &str,
    token: Option<&str>,
    workspace: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("DELETE")
        .uri(format!("/api/w/{workspace}/subjektiv/subjects/{subject}"))
        .header(CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = build_router(api.clone())
        .oneshot(
            builder
                .body(Body::from(json!({"operation_id":key}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}));
    (status, body)
}

#[tokio::test]
async fn deletion_owner_scope_exact_retry_and_subject_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("delete me").unwrap(),
            "private behavior".into(),
        )
        .unwrap();
    let other = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("keep me").unwrap(),
            "unchanged".into(),
        )
        .unwrap();
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-owner").await;
    let stranger = seed_test_api_token(api.store.as_ref(), "subject-delete-stranger");
    for (token, workspace, expected) in [
        (None, TEST_WORKSPACE_ID, StatusCode::UNAUTHORIZED),
        (
            Some(stranger.as_str()),
            TEST_WORKSPACE_ID,
            StatusCode::FORBIDDEN,
        ),
        (
            Some(owner.as_str()),
            "another-workspace",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, body) =
            delete_request(&api, &subject.id, "same-operation", token, workspace).await;
        assert_eq!(status, expected, "{body}");
        assert!(store.subject(&subject.id).unwrap().is_some());
    }
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "same-operation",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
    assert!(store.subject(&subject.id).unwrap().is_none());
    assert_eq!(
        store.subject(&other.id).unwrap().unwrap().behavior_md,
        "unchanged"
    );
    assert_eq!(store.list_subjects(100).unwrap().items.len(), 1);
    let (status, replay) = delete_request(
        &api,
        &subject.id,
        "same-operation",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, body);
    let (status, _) = delete_request(
        &api,
        &subject.id,
        "different-operation",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = delete_request(
        &api,
        &other.id,
        "same-operation",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(store.subject(&other.id).unwrap().is_some());
    let fresh = store
        .create_subject(crate::subjektiv::SubjectRole::new("fresh").unwrap())
        .unwrap();
    assert_ne!(fresh.id, subject.id);
    assert!(store.subject_permissions(&fresh.id).unwrap().is_none());
    assert_eq!(
        store
            .pending_staging_candidates(&fresh.id, 100)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn legacy_inactive_subject_is_preserved_until_explicit_delete_and_retire_route_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("legacy fixture").unwrap(),
            "retained original".into(),
        )
        .unwrap();
    api.feature_storage.open(&api.subjektiv_registration).unwrap()
        .try_transaction::<_, feature_storage::FeatureStorageError>(|tx| {
            tx.execute("UPDATE subjects SET state='retired',record_json=json_set(record_json,'$.state','retired') WHERE subject_id=?1", [&subject.id])?;
            Ok(())
        }).unwrap();
    assert_eq!(
        store.subject(&subject.id).unwrap().unwrap().state,
        crate::subjektiv::SubjectState::LegacyRetired
    );
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-legacy").await;
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/retire",
            subject.id
        ))
        .header("authorization", format!("Bearer {owner}"))
        .body(Body::empty())
        .unwrap();
    let old = build_router(api.clone()).oneshot(request).await.unwrap();
    assert_eq!(old.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        store.subject(&subject.id).unwrap().unwrap().behavior_md,
        "retained original"
    );
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "explicit-legacy-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unconfirmed_worker_shutdown_is_not_completion_and_exact_retry_converges() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("delete connected body").unwrap(),
            "private behavior".into(),
        )
        .unwrap();
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "Subject deletion boundary".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.id.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    seed_worker_session_for_cleanup(dir.path(), &worker);
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-unknown").await;
    *execution.subject_delete_failure.lock().unwrap() = Some("shutdown completion unknown".into());
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "unknown-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], false);
    assert!(store.subject(&subject.id).unwrap().is_none());
    assert!(
        !store
            .subject_deletion(&subject.id)
            .unwrap()
            .unwrap()
            .completed
    );
    assert!(subject_authorization::worker_permissions(&api, &worker).is_err());
    assert!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &format!("subjektiv:{}", subject.id))
            .unwrap()
            .is_some()
    );
    *execution.subject_delete_failure.lock().unwrap() = None;
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER injected_worker_catalog_failure BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'injected catalog failure'); END;")?;
        Ok(())
    }).unwrap();
    let (status, midpoint) = delete_request(
        &api,
        &subject.id,
        "unknown-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{midpoint}");
    assert_eq!(midpoint["completed"], false);
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_some()
    );
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER injected_worker_catalog_failure;")?;
            Ok(())
        })
        .unwrap();
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "unknown-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true, "{body}");
    assert!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &format!("subjektiv:{}", subject.id))
            .unwrap()
            .is_none()
    );
    let removal = api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        removal.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
    assert_eq!(
        removal.runtime_request.session_disposition,
        worker_runtime::retention::SessionDisposition::Archive
    );
    assert!(
        store
            .subject_deletion(&subject.id)
            .unwrap()
            .unwrap()
            .completed
    );
}

#[tokio::test]
async fn captured_subject_job_result_cannot_deliver_after_domain_fence() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("notification fence").unwrap())
        .unwrap();
    let target = spawn_ticket_check_source(&api, "subject-notification-target");
    let worker = spawn_ticket_check_source(&api, "subject-notification-result");
    let request = BackendJobRequest {
        job_id: "captured-subject-notification".into(),
        purpose: "subject_notification_contract".into(),
        input_ref: "subject:notification".into(),
        input: json!({"private":"input"}),
        instruction: "Read input".into(),
        profile: "builtin:backend-job".into(),
        grants: crate::backend_job::BackendJobGrants {
            subjektiv_consolidation: Some(crate::backend_job::SubjektivConsolidationGrant {
                subject_id: subject.id.clone(),
                candidate_ids: vec!["candidate".into()],
            }),
            ..Default::default()
        },
        serialization_key: None,
        source_worker: Some(target.clone()),
        notification_target: Some(target),
        limits: Default::default(),
    };
    // Enter at the committed result boundary: execution/result validation is
    // outside this notification test, but routing and Runtime transport are real.
    let now = Utc::now().to_rfc3339();
    let reserved = api
        .store
        .reserve_backend_job(TEST_WORKSPACE_ID, &request, &now)
        .unwrap();
    let attempt = &reserved.attempt.attempt_id;
    api.store
        .claim_backend_job_attempt_dispatch(TEST_WORKSPACE_ID, &request.job_id, attempt, &now)
        .unwrap();
    api.store
        .bind_backend_job_attempt_worker(
            TEST_WORKSPACE_ID,
            &request.job_id,
            attempt,
            &worker,
            Some("run"),
            &now,
        )
        .unwrap();
    let captured = api
        .store
        .accept_backend_job_result(
            TEST_WORKSPACE_ID,
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: attempt.clone(),
                input_digest: request.input_digest().unwrap(),
                result: json!({"private":"result"}),
            },
            &now,
        )
        .unwrap();
    let delivery = crate::backend_job::delivery_id(&request.job_id, attempt);
    assert!(
        api.store
            .job_notification_source(TEST_WORKSPACE_ID, &request.job_id, attempt)
            .is_ok()
    );
    execution.take_inputs();
    store
        .begin_subject_deletion(&subject.id, "notification-delete")
        .unwrap();
    notification_delivery::deliver_job_result(&api, &captured);
    notification_delivery::recover_job_notifications(&api);
    assert!(
        execution.take_inputs().is_empty(),
        "captured or recovered result must not escape the domain fence"
    );
    assert!(
        api.store
            .job_notification_request(TEST_WORKSPACE_ID, &delivery)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn domain_purge_failure_keeps_fence_and_backend_reopen_resumes_exact_operation() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("retry purge").unwrap(),
            "do not resurrect".into(),
        )
        .unwrap();
    let other = store
        .create_subject_with_behavior(
            crate::subjektiv::SubjectRole::new("untouched").unwrap(),
            "keep".into(),
        )
        .unwrap();
    let database = api
        .feature_storage
        .open(&api.subjektiv_registration)
        .unwrap();
    database.try_transaction::<_, feature_storage::FeatureStorageError>(|tx| {
        tx.execute_batch("CREATE TRIGGER injected_subject_purge_failure BEFORE DELETE ON subjects BEGIN SELECT RAISE(ABORT,'injected domain purge failure'); END;")?;
        Ok(())
    }).unwrap();
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-purge-failure").await;
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "purge-retry",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert!(status.is_server_error(), "{status}: {body}");
    assert!(
        !store
            .subject_deletion(&subject.id)
            .unwrap()
            .unwrap()
            .completed
    );
    assert!(store.subject(&subject.id).unwrap().is_none());
    assert_eq!(
        store.subject(&other.id).unwrap().unwrap().behavior_md,
        "keep"
    );
    database
        .try_transaction::<_, feature_storage::FeatureStorageError>(|tx| {
            tx.execute_batch("DROP TRIGGER injected_subject_purge_failure;")?;
            Ok(())
        })
        .unwrap();
    let mut reopened = api.clone();
    let config_store =
        Arc::new(SqliteWorkspaceStore::open_for_test(&api.config.database_path).unwrap());
    reopened.config_store = config_store.clone();
    reopened.store = config_store;
    let recovery_api = reopened.clone();
    let handle = tokio::runtime::Handle::current();
    reopened
        .host_executors
        .host()
        .unwrap()
        .run(move || handle.block_on(subjektiv_deletion::recover(&recovery_api)))
        .await
        .unwrap();
    let recovered = open_subjektiv_store(&reopened)
        .unwrap()
        .subject_deletion(&subject.id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.operation_id, "purge-retry");
    assert!(recovered.completed);
    let (status, body) = delete_request(
        &reopened,
        &subject.id,
        "purge-retry",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
    assert_eq!(
        store.subject(&other.id).unwrap().unwrap().behavior_md,
        "keep"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_consolidation_is_purged_and_captured_dispatch_and_late_result_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("delete running Job").unwrap())
        .unwrap();
    let candidate: memory::extract::StagingRecord = serde_json::from_value(json!({
        "schema_version": 2, "id": "delete-job-candidate", "extract_run_id": "extract",
        "source": {"segment_id":"segment", "range":[1,2]}, "kind":"constraint",
        "claim":"private candidate marker", "why_useful":"retained only until explicit erasure",
        "evidence": [], "source_refs": []
    }))
    .unwrap();
    store
        .stage_candidate(crate::subjektiv::SubjectStagingRecord::attach(
            &subject.id,
            candidate,
        ))
        .unwrap();
    let started = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    assert_eq!(started.status, "started");
    let job = api
        .store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(&subject.id),
        )
        .unwrap()
        .unwrap();
    let attempt = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap();
    let worker = attempt.worker.clone().unwrap();
    seed_worker_session_for_cleanup(dir.path(), &worker);
    let captured = BackendJobReservation {
        job: job.clone(),
        attempt: attempt.clone(),
        replayed: true,
        resource_reused: false,
    };
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-consolidation").await;
    *execution.subject_delete_failure.lock().unwrap() = Some("completion unknown".into());
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "delete-job",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], false);
    assert!(
        api.dispatch_backend_job_reservation(captured.clone())
            .is_err()
    );
    assert!(
        api.accept_backend_job_result(
            &worker,
            &BackendJobResultSubmission {
                job_id: job.request.job_id.clone(),
                attempt_id: attempt.attempt_id.clone(),
                input_digest: job.input_digest.clone(),
                result: json!({"late":true}),
            }
        )
        .is_err()
    );
    *execution.subject_delete_failure.lock().unwrap() = None;
    let (status, body) = delete_request(
        &api,
        &subject.id,
        "delete-job",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
    assert!(
        api.store
            .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
            .unwrap()
            .is_none()
    );
    assert!(
        api.store
            .get_backend_job_attempt(TEST_WORKSPACE_ID, &job.request.job_id, &attempt.attempt_id)
            .unwrap()
            .is_none()
    );
    assert!(api.dispatch_backend_job_reservation(captured).is_err());
    assert!(api.dispatch_backend_job(&job.request).is_err());
    let removal = api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        removal.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
    assert_eq!(
        removal.runtime_request.session_disposition,
        worker_runtime::retention::SessionDisposition::Purge
    );
    assert_eq!(
        removal.runtime_request.diagnostics_disposition,
        worker_runtime::retention::DiagnosticsDisposition::Purge
    );
    assert!(
        !dir.path()
            .join(".test-embedded-runtime-store/workers")
            .join(&worker.worker_id)
            .exists()
    );
}

#[tokio::test]
async fn unmaterialized_body_and_job_reservations_finish_only_with_runtime_storage_absence_proof() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "subject-delete-unmaterialized").await;
    for is_job in [false, true] {
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("reserved before deletion").unwrap())
            .unwrap();
        let singleton = (!is_job).then(|| format!("subjektiv:{}", subject.id));
        let allocation = if is_job {
            let request = BackendJobRequest {
                job_id: "unmaterialized-subject-job".into(),
                purpose: "subject_reservation_contract".into(),
                input_ref: "subject:reserved".into(),
                input: json!({"private":"reserved input"}),
                instruction: "read".into(),
                profile: "builtin:backend-job".into(),
                grants: crate::backend_job::BackendJobGrants {
                    subjektiv_consolidation: Some(
                        crate::backend_job::SubjektivConsolidationGrant {
                            subject_id: subject.id.clone(),
                            candidate_ids: vec!["candidate".into()],
                        },
                    ),
                    ..Default::default()
                },
                serialization_key: None,
                source_worker: None,
                notification_target: None,
                limits: Default::default(),
            };
            let reserved = api
                .store
                .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
                .unwrap();
            crate::backend_job::allocation_key(&reserved.attempt.attempt_id)
        } else {
            "unmaterialized-subject-body".into()
        };
        let settings = api
            .config_store
            .get_workspace_memory_settings(TEST_WORKSPACE_ID)
            .unwrap();
        let reserved = api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                EMBEDDED_WORKER_RUNTIME_ID,
                &allocation,
                "unmaterialized-fingerprint",
                singleton.as_deref(),
                &settings,
            )
            .unwrap();
        let worker =
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, reserved.worker_id.to_string());
        assert!(matches!(
            api.runtime.worker(&worker),
            Err(RuntimeRegistryError::UnknownWorker { .. })
        ));
        let (status, body) = delete_request(
            &api,
            &subject.id,
            &format!("delete-reserved-{is_job}"),
            Some(&owner),
            TEST_WORKSPACE_ID,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["completed"], true);
        assert!(subject_authorization::worker_permissions(&api, &worker).is_err());
        let proved = api.config_store.with_conn(|conn| Ok(conn.query_row("SELECT originals_absent FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4", rusqlite::params![TEST_WORKSPACE_ID,subject.id,worker.runtime_id,worker.worker_id], |row|row.get::<_,bool>(0))?)).unwrap();
        assert!(proved);
        assert!(
            api.config_store
                .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .is_none(),
            "absence is not a fabricated Purge/archive receipt"
        );
        assert!(
            api.config_store
                .reserve_worker_create(
                    TEST_WORKSPACE_ID,
                    EMBEDDED_WORKER_RUNTIME_ID,
                    &allocation,
                    "unmaterialized-fingerprint",
                    singleton.as_deref(),
                    &settings
                )
                .is_err()
        );
        if let Some(singleton) = singleton {
            assert!(
                api.store
                    .current_worker_singleton_owner(TEST_WORKSPACE_ID, &singleton)
                    .unwrap()
                    .is_none()
            );
        }
    }
}
