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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_job_removal_reconciles_exact_input_then_erases_copies_across_http_retry_and_restart() {
    use worker_runtime::retention::{DiagnosticsDisposition, SessionDisposition};
    for phase in [
        "planned",
        "executing",
        "failed",
        "response_lost",
        "catalog_failure",
        "purge_ack_commit_failure",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(
                crate::subjektiv::SubjectRole::new(format!("old plan {phase}")).unwrap(),
            )
            .unwrap();
        let other = store
            .create_subject_with_behavior(
                crate::subjektiv::SubjectRole::new("keep other").unwrap(),
                "untouched".into(),
            )
            .unwrap();
        let candidate = serde_json::from_value(json!({
            "schema_version": 2, "id": "old-job-candidate", "extract_run_id": "extract",
            "source": {"segment_id":"segment", "range":[1,2]}, "kind":"constraint",
            "claim":"private candidate", "why_useful":"erase after explicit deletion",
            "evidence": [], "source_refs": []
        }))
        .unwrap();
        store
            .stage_candidate(crate::subjektiv::SubjectStagingRecord::attach(
                &subject.id,
                candidate,
            ))
            .unwrap();
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            MemoryConsolidateStagingOperation { force: true },
        )
        .await
        .unwrap();
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
        api.runtime
            .stop_worker(
                &worker,
                WorkerLifecycleRequest {
                    reason: Some("Prior independent removal".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let diagnostic = dir
            .path()
            .join(".test-embedded-runtime-store/workers")
            .join(&worker.worker_id)
            .join("runs/old/worker.out.log");
        std::fs::create_dir_all(diagnostic.parent().unwrap()).unwrap();
        std::fs::write(&diagnostic, "private old Job diagnostic").unwrap();
        let policy = api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        api.config_store
            .update_worker_retention_policy(
                TEST_WORKSPACE_ID,
                &policy.policy_digest,
                &crate::retention::WorkerRetentionPolicyUpdate {
                    policy_id: "prior-job-archive-policy".into(),
                    session_disposition: SessionDisposition::Archive,
                    metadata_disposition: crate::retention::MetadataDisposition::Tombstone,
                    archive_retention: crate::retention::ArchiveRetention::ForSeconds {
                        seconds: 600,
                    },
                    diagnostics_disposition: DiagnosticsDisposition::Retain,
                    diagnostics_retention_seconds: Some(600),
                },
            )
            .unwrap();
        let inventory = api.runtime.worker_retention_inventory(&worker).unwrap();
        assert!(inventory.diagnostics_bytes > 0);
        let old = api
            .config_store
            .plan_worker_removal(
                &crate::retention::WorkerRemovalPlanRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    worker: worker.clone(),
                    reason: "Previously requested retention".into(),
                    journal_erasure: None,
                },
                &inventory,
            )
            .unwrap();
        assert_eq!(old.session_disposition, SessionDisposition::Archive);
        let saved = if phase == "planned" {
            api.config_store
                .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .unwrap()
        } else {
            api.config_store
                .prepare_worker_removal_execution(
                    TEST_WORKSPACE_ID,
                    &old.plan_id,
                    &old.input_fingerprint,
                )
                .unwrap()
        };
        if phase == "failed" {
            api.config_store
                .fail_worker_removal(
                    TEST_WORKSPACE_ID,
                    &old.operation_id,
                    &old.input_fingerprint,
                    "old_unknown_outcome",
                )
                .unwrap();
        }
        if phase == "catalog_failure" {
            api.config_store.with_conn(|c| { c.execute_batch("CREATE TRIGGER injected_old_catalog_failure BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'catalog failure'); END;")?; Ok(()) }).unwrap();
        }
        let old_result = if matches!(phase, "response_lost" | "catalog_failure") {
            let result = api
                .runtime
                .execute_worker_retention(&worker, saved.runtime_request.clone())
                .unwrap();
            if phase == "catalog_failure" {
                assert!(
                    api.config_store
                        .commit_worker_removal(
                            TEST_WORKSPACE_ID,
                            &old.operation_id,
                            &old.input_fingerprint,
                            &result
                        )
                        .is_err()
                );
            }
            Some(result)
        } else {
            None
        };
        if phase == "purge_ack_commit_failure" {
            api.config_store.with_conn(|c| { c.execute_batch("CREATE TRIGGER injected_purge_ack_commit_failure BEFORE UPDATE OF completed ON subject_deletion_removals WHEN NEW.completed=1 BEGIN SELECT RAISE(ABORT,'purge receipt commit failure'); END;")?; Ok(()) }).unwrap();
        }
        // Force a midpoint AFTER domain/snapshot commit but BEFORE any deletion-owned
        // Runtime effect. It must retain exactly the original canonical input.
        *execution.subject_delete_failure.lock().unwrap() = Some("shutdown outcome unknown".into());
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), &format!("old-removal-{phase}"))
                .await;
        let (status, pending) = delete_request(
            &api,
            &subject.id,
            "same-delete",
            Some(&owner),
            TEST_WORKSPACE_ID,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{phase}: {pending}");
        assert_eq!(pending["completed"], false, "{phase}");
        let pinned = api
            .config_store
            .subject_deletion_removal(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .unwrap();
        assert_eq!(pinned.request, saved.runtime_request, "{phase}");
        assert!(!pinned.completed);
        assert!(pinned.is_job);
        assert!(
            api.config_store
                .erase_subject_deleted_jobs(TEST_WORKSPACE_ID, &subject.id, "same-delete")
                .is_err()
        );
        *execution.subject_delete_failure.lock().unwrap() = None;
        let mut reopened = api.clone();
        let reopened_store =
            Arc::new(SqliteWorkspaceStore::open_for_test(&api.config.database_path).unwrap());
        reopened.config_store = reopened_store.clone();
        reopened.store = reopened_store;
        if matches!(phase, "catalog_failure" | "purge_ack_commit_failure") {
            let (status, pending) = delete_request(
                &reopened,
                &subject.id,
                "same-delete",
                Some(&owner),
                TEST_WORKSPACE_ID,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{phase}: {pending}");
            assert_eq!(pending["completed"], false, "{phase}");
            assert!(
                !store
                    .subject_deletion(&subject.id)
                    .unwrap()
                    .unwrap()
                    .completed
            );
            assert!(
                reopened
                    .store
                    .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
                    .unwrap()
                    .is_some()
            );
            reopened
                .config_store
                .with_conn(|c| {
                    c.execute_batch(if phase == "catalog_failure" {
                        "DROP TRIGGER injected_old_catalog_failure;"
                    } else {
                        "DROP TRIGGER injected_purge_ack_commit_failure;"
                    })?;
                    Ok(())
                })
                .unwrap();
        }
        // Exercise the same production restart recovery that starts from the saved
        // domain operation, then HTTP replay of the same full request.
        let recovery = reopened.clone();
        let handle = tokio::runtime::Handle::current();
        reopened
            .host_executors
            .host()
            .unwrap()
            .run(move || handle.block_on(subjektiv_deletion::recover(&recovery)))
            .await
            .unwrap();
        let (status, done) = delete_request(
            &reopened,
            &subject.id,
            "same-delete",
            Some(&owner),
            TEST_WORKSPACE_ID,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{phase}: {done}");
        assert_eq!(done["completed"], true, "{phase}: {done}");
        let reconciled = reopened
            .config_store
            .subject_deletion_removal(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .unwrap();
        assert!(reconciled.completed);
        assert_eq!(reconciled.request, saved.runtime_request);
        let canonical = reopened
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            canonical.plan.state,
            crate::retention::WorkerRemovalPlanState::Succeeded
        );
        assert_eq!(canonical.runtime_request, saved.runtime_request);
        assert_eq!(
            canonical.plan.session_disposition,
            SessionDisposition::Archive,
            "old result was not relabeled Purge"
        );
        let result = reopened
            .runtime
            .execute_worker_retention(&worker, saved.runtime_request.clone())
            .unwrap();
        if let Some(old_result) = old_result {
            assert_eq!(result, old_result);
        }
        assert_eq!(result.session_disposition, SessionDisposition::Archive);
        assert!(result.source_removed);
        let archive = result.archive.as_ref().unwrap();
        let root = dir.path().join(".test-embedded-runtime-store");
        assert!(
            !root
                .join("archives/workers")
                .join(&archive.archive_id)
                .join("session")
                .exists(),
            "new Job archive copy survives {phase}"
        );
        if result.diagnostics_disposition == DiagnosticsDisposition::Retain {
            assert!(
                !root
                    .join("archives/diagnostics")
                    .join(&old.operation_id)
                    .exists(),
                "Job diagnostics survive {phase}"
            );
        }
        assert!(
            reopened
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .store
                .current_worker_singleton_owner(
                    TEST_WORKSPACE_ID,
                    &crate::backend_job::subjektiv_serialization_key(&subject.id)
                )
                .unwrap()
                .is_none()
        );
        assert!(store.subject(&subject.id).unwrap().is_none());
        assert_eq!(
            store.subject(&other.id).unwrap().unwrap().behavior_md,
            "untouched"
        );
        let (status, replay) = delete_request(
            &reopened,
            &subject.id,
            "same-delete",
            Some(&owner),
            TEST_WORKSPACE_ID,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done, replay);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_body_archive_reconciles_unchanged_and_remains_retained_after_subject_deletion() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("old body archive").unwrap())
        .unwrap();
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "Existing body removal".into(),
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
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: Some("Earlier removal".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    let inventory = api.runtime.worker_retention_inventory(&worker).unwrap();
    let old = api
        .config_store
        .plan_worker_removal(
            &crate::retention::WorkerRemovalPlanRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: worker.clone(),
                reason: "Earlier authorized archive".into(),
                journal_erasure: None,
            },
            &inventory,
        )
        .unwrap();
    let saved = api
        .config_store
        .prepare_worker_removal_execution(TEST_WORKSPACE_ID, &old.plan_id, &old.input_fingerprint)
        .unwrap();
    let result = api
        .runtime
        .execute_worker_retention(&worker, saved.runtime_request.clone())
        .unwrap();
    // Simulate a lost result before canonical Backend commit. Deletion must pin an
    // unfinished request, not retrospectively classify its archive as a prior success.
    *execution.subject_delete_failure.lock().unwrap() = Some("unknown shutdown".into());
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "old-body-removal").await;
    let (status, pending) = delete_request(
        &api,
        &subject.id,
        "same-body-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    assert_eq!(pending["completed"], false);
    let snapshot = api
        .config_store
        .subject_deletion_removal(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .unwrap();
    assert!(!snapshot.is_job && !snapshot.completed);
    assert_eq!(snapshot.request, saved.runtime_request);
    *execution.subject_delete_failure.lock().unwrap() = None;
    let (status, done) = delete_request(
        &api,
        &subject.id,
        "same-body-delete",
        Some(&owner),
        TEST_WORKSPACE_ID,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["completed"], true, "{done}");
    let completed = api
        .config_store
        .subject_deletion_removal(TEST_WORKSPACE_ID, &worker)
        .unwrap()
        .unwrap();
    assert!(completed.completed);
    assert_eq!(completed.request, saved.runtime_request);
    let replay = api
        .runtime
        .execute_worker_retention(&worker, saved.runtime_request)
        .unwrap();
    assert_eq!(replay, result);
    let archive = result.archive.unwrap();
    assert!(
        dir.path()
            .join(".test-embedded-runtime-store/archives/workers")
            .join(&archive.archive_id)
            .join("session")
            .is_dir()
    );
    assert!(api.config_store.with_conn(|c| Ok(c.query_row("SELECT prior_removal_operation_id IS NULL FROM subject_deletion_workers WHERE workspace_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4", rusqlite::params![TEST_WORKSPACE_ID,subject.id,worker.runtime_id,worker.worker_id], |r|r.get::<_,bool>(0))?)).unwrap());
    assert!(
        store
            .subject_deletion(&subject.id)
            .unwrap()
            .unwrap()
            .completed
    );
}
