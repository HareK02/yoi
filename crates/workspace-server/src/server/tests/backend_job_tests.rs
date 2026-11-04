//! Server Job/Runtime integration; the execution backend is fixture-owned.
//! Persistent Job state and production dispatch/reconciliation/Host admission remain real.
use super::*;

#[tokio::test]
async fn backend_job_storage_lock_exhausts_budget_then_same_owner_probes_and_saves_result() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_server_config(temp.path());
    let store = Arc::new(SqliteWorkspaceStore::open_api_fixture(&config.database_path).unwrap());
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    let execution = Arc::new(DeterministicExecutionBackend::default());
    let api = WorkspaceApi::new_with_database_preparation(
        config,
        store.clone(),
        execution.clone(),
        BackendResourceBroker::default(),
        HostExecutors::default(),
        None,
        WorkspaceDatabasePreparation::PreparedFixture,
    )
    .await
    .unwrap();
    let request = recovery_request("storage-recovered");
    let reservation = api.dispatch_backend_job(&request).unwrap();
    let worker = reservation.attempt.worker.clone().unwrap();
    store
        .with_conn(|conn| {
            conn.busy_timeout(std::time::Duration::ZERO)?;
            Ok(())
        })
        .unwrap();
    let mut blocker = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let lock = blocker
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let submission = BackendJobResultSubmission {
        job_id: request.job_id.clone(),
        attempt_id: reservation.attempt.attempt_id.clone(),
        input_digest: reservation.attempt.input_digest.clone(),
        result: serde_json::json!({"saved":true}),
    };
    assert!(api.accept_backend_job_result(&worker, &submission).is_err());
    // Observe the actual owner transition after its finite settlement window;
    // busy callbacks on another connection or unrelated writers are not evidence.
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        api.backend_job_recovery.storage_probe_started.notified(),
    )
    .await
    .expect("accepted owner never entered storage probe after finite budget");
    assert_eq!(
        api.store
            .get_backend_job(api.workspace_id(), &request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Pending
    );
    lock.rollback().unwrap();
    let completed = wait_for_attempt(
        &api,
        &request.job_id,
        &reservation.attempt.attempt_id,
        |attempt| attempt.state == BackendJobAttemptState::Completed,
    )
    .await;
    assert_eq!(completed.result, Some(submission.result));
    assert_eq!(completed.attempt, 1);
    let probes: i64 = api.config_store.with_conn(|conn| {
        conn.query_row("SELECT successful_writes FROM backend_job_recovery_probes WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",
            rusqlite::params![api.workspace_id(), request.job_id, reservation.attempt.attempt_id], |row| row.get(0)).map_err(Error::from)
    }).unwrap();
    assert!(
        probes > 0,
        "only an actual exact-owner probe commit may refill the settlement budget"
    );
    assert_eq!(
        execution.take_inputs().len(),
        1,
        "result persistence recovery did not rerun the model"
    );
    api.stop_backend_job_recovery().await;
}

fn recovery_request(id: &str) -> BackendJobRequest {
    BackendJobRequest {
        job_id: id.into(),
        purpose: "receipt_recovery_contract".into(),
        input_ref: format!("test://saved/{id}"),
        input: serde_json::json!({"body":"frozen"}),
        instruction: "Return the bounded structured result".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits {
            timeout_seconds: 30,
            ..Default::default()
        },
    }
}

async fn wait_for_attempt(
    api: &WorkspaceApi,
    job: &str,
    attempt: &str,
    condition: impl Fn(&BackendJobAttemptRecord) -> bool,
) -> BackendJobAttemptRecord {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let current = api
                .store
                .get_backend_job_attempt(api.workspace_id(), job, attempt)
                .unwrap()
                .unwrap();
            if condition(&current) {
                return current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("same accepted attempt did not reach its expected durable state")
}

#[tokio::test]
async fn backend_job_lost_allocation_response_binds_original_receipt_without_host_admission() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = recovery_request("lost-allocation-response");
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    let reserved = api
        .store
        .reserve_backend_job(api.workspace_id(), &request, &now)
        .unwrap();
    let (claimed, acquired) = api
        .store
        .claim_backend_job_attempt_dispatch(
            api.workspace_id(),
            &request.job_id,
            &reserved.attempt.attempt_id,
            &now,
        )
        .unwrap();
    assert!(acquired);
    // Fault boundary: the real Host/Runtime creation committed, but its caller
    // never saved the binding or acquired the initial producer's Host admission.
    let spawn = api
        .backend_job_spawn_request(&claimed.job, &claimed.attempt)
        .unwrap();
    let created = api
        .spawn_workspace_worker(EMBEDDED_WORKER_RUNTIME_ID, spawn)
        .unwrap();
    let original = created.worker.unwrap().worker;
    assert!(execution.take_inputs().is_empty());
    // Fault injection: live configuration/registry Profile resolution is no
    // longer available. Accepted allocation evidence must not depend on it.
    api.config_store
        .with_conn_mut(|conn| {
            conn.execute(
                "DELETE FROM workspace_config_trees WHERE workspace_id=?1",
                [api.workspace_id()],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(api.resolve_backend_job_profile(&request).is_err());
    api.own_backend_job_recovery(&claimed.attempt);
    let recovered = wait_for_attempt(
        &api,
        &request.job_id,
        &claimed.attempt.attempt_id,
        |attempt| attempt.state == BackendJobAttemptState::Dispatched,
    )
    .await;
    assert_eq!(recovered.worker, Some(original.clone()));
    assert_eq!(recovered.attempt, 1);
    let owner = crate::backend_job_host_effects::JobHostOwner::try_from(&recovered).unwrap();
    assert!(
        api.backend_job_recovery.effects(&owner).is_err(),
        "a creation receipt must not synthesize a historical Host drain owner"
    );
    let replay = api.dispatch_backend_job(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.attempt.worker, Some(original));
    assert!(
        execution.take_inputs().is_empty(),
        "restored receipt evidence cannot authorize a new non-idempotent Submit"
    );
    api.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_executing_cleanup_restart_requires_fence_and_original_removal() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = recovery_request("executing-cleanup-restart");
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.clone().unwrap();
    execution.take_inputs();
    seed_worker_session_for_cleanup(temp.path(), &worker);
    // Stop only the recovery tasks, not execution. Inject a crash boundary after
    // cleanup claim without a live cleanup task. This is not a process-exit test.
    api.stop_backend_job_recovery().await;
    api.store
        .finish_backend_job_attempt(
            api.workspace_id(),
            &request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "fixture_failure",
            "retained original failure",
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    let current = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &request.job_id,
            &dispatched.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    let history = api
        .store
        .list_backend_job_attempts(api.workspace_id(), &request.job_id)
        .unwrap();
    let (claimed, acquired) = api
        .store
        .claim_backend_job_worker_cleanup_owned(
            api.workspace_id(),
            &history,
            &current,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    assert!(acquired);
    let mut restarted = api.clone();
    restarted.backend_job_recovery = Arc::default();
    let error = restarted
        .run_backend_job_cleanup(claimed.clone())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Host writer/drain evidence missing")
    );
    assert!(
        api.config_store
            .recover_worker_removal_execution(api.workspace_id(), &worker)
            .unwrap()
            .is_none()
    );

    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("fixture exact execution termination".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    api.fence_backend_job_effects(&claimed).await.unwrap();
    let error = restarted
        .run_backend_job_cleanup(claimed.clone())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no retained canonical removal operation")
    );
    assert!(
        api.config_store
            .recover_worker_removal_execution(api.workspace_id(), &worker)
            .unwrap()
            .is_none(),
        "a restarted Executing marker cannot authorize a new removal"
    );
    assert_eq!(
        api.store
            .get_backend_job_attempt(api.workspace_id(), &request.job_id, &claimed.attempt_id)
            .unwrap()
            .unwrap()
            .worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Executing)
    );

    // Checked restart entry cannot fall through to planning without the pinned
    // original operation, even though execution/Host fences now exist.
    assert!(
        WorkerRemovalService::new(&api)
            .execute_target_removal_on_host_checked(
                api.runtime.as_ref(),
                &worker,
                "must not create a replacement",
                None,
                Some(("missing-operation", "missing-fingerprint"))
            )
            .await
            .is_err()
    );
    assert!(
        api.config_store
            .recover_worker_removal_execution(api.workspace_id(), &worker)
            .unwrap()
            .is_none()
    );

    // The original owner performs canonical removal, then loses its cleanup save.
    let response = WorkerRemovalService::new(&api)
        .execute_target_removal_on_host(
            api.runtime.as_ref(),
            &worker,
            &format!("Terminal Backend Job attempt {}", claimed.attempt_id),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status, StatusCode::OK.as_u16());
    let original = api
        .config_store
        .recover_worker_removal_execution(api.workspace_id(), &worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        original.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
    restarted.recover_backend_job_worker_cleanups().unwrap();
    let cleaned = wait_for_backend_job_worker_cleanup(
        &restarted,
        &request.job_id,
        &claimed.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert_eq!(cleaned.failure_category.as_deref(), Some("fixture_failure"));
    let retained = api
        .config_store
        .recover_worker_removal_execution(api.workspace_id(), &worker)
        .unwrap()
        .unwrap();
    assert_eq!(retained.plan.operation_id, original.plan.operation_id);
    assert_eq!(
        retained.plan.input_fingerprint,
        original.plan.input_fingerprint
    );
    assert!(execution.take_inputs().is_empty());
    assert!(api.runtime.worker(&worker).is_err());
    restarted.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_lost_input_ack_uses_exact_receipt_without_second_submit() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    execution.lose_input_ack.store(true, Ordering::Release);
    let request = recovery_request("lost-ack");
    let reservation = api.dispatch_backend_job(&request).unwrap();
    assert_eq!(
        reservation.attempt.state,
        BackendJobAttemptState::Dispatched
    );
    let worker = reservation.attempt.worker.clone().unwrap();
    assert_eq!(execution.take_inputs().len(), 1);
    api.recover_backend_jobs().unwrap();
    let accepted = api
        .accept_backend_job_result(
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: reservation.attempt.attempt_id.clone(),
                input_digest: reservation.attempt.input_digest.clone(),
                result: serde_json::json!({"saved":true}),
            },
        )
        .unwrap();
    assert_eq!(accepted.attempt.attempt, 1);
    assert_eq!(accepted.job.state, BackendJobState::Completed);
    assert!(execution.take_inputs().is_empty());
    api.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_definitive_input_rejection_is_not_receipt_ambiguity() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    execution
        .reject_input_definitively
        .store(true, Ordering::Release);
    let request = recovery_request("rejected-input");
    assert!(api.dispatch_backend_job(&request).is_err());
    let attempt = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &request.job_id,
            &crate::backend_job::attempt_id(&request.job_id, 1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(attempt.state, BackendJobAttemptState::Failed);
    assert_eq!(attempt.failure_category.as_deref(), Some("input_rejected"));
    assert!(execution.receipts.lock().unwrap().is_empty());
    assert_eq!(execution.take_inputs().len(), 1);
    api.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_cancelled_callback_keeps_actual_host_db_effect_owned_until_drain() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = recovery_request("host-drain");
    let reservation = api.dispatch_backend_job(&request).unwrap();
    let owner =
        crate::backend_job_host_effects::JobHostOwner::try_from(&reservation.attempt).unwrap();
    let effects = api.backend_job_recovery.effects(&owner).unwrap();
    let worker = reservation.attempt.worker.clone().unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, permit) = std::sync::mpsc::channel();
    let gate = Arc::new(Mutex::new(Some((entered, permit))));
    let executor = api.store.db_executor();
    let router = Router::new()
        .route(
            "/callback",
            get(move || {
                let executor = executor.clone();
                let gate = gate.lock().unwrap().take().unwrap();
                async move {
                    let result: Result<()> = executor
                        .run(move || {
                            let _ = gate.0.send(());
                            gate.1
                                .recv_timeout(std::time::Duration::from_secs(10))
                                .unwrap();
                            Ok(())
                        })
                        .await;
                    result
                        .map(|_| StatusCode::NO_CONTENT)
                        .map_err(ApiError::from)
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            api.clone(),
            backend_job_recovery::admit_host_request,
        ));
    let mut callback = Request::builder()
        .uri("/callback")
        .body(Body::empty())
        .unwrap();
    // Authentication is the substituted outer boundary in this test. The real
    // Job Host middleware consumes only the verified extension, not hint headers.
    callback
        .extensions_mut()
        .insert(crate::worker_source::VerifiedRuntimeRequestSource {
            runtime_id: worker.runtime_id.clone(),
            worker_id: Some(worker.worker_id.clone()),
            authentication_binding_id: "fixture-verified-binding".into(),
            token_id_hash: None,
        });
    let waiter = tokio::spawn(async move { router.oneshot(callback).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    api.accept_backend_job_result(
        &worker,
        &BackendJobResultSubmission {
            job_id: request.job_id.clone(),
            attempt_id: reservation.attempt.attempt_id.clone(),
            input_digest: reservation.attempt.input_digest.clone(),
            result: serde_json::json!({"saved":true}),
        },
    )
    .unwrap();
    // The fake execution boundary is independently stopped; the accepted Host/DB
    // closure is real and still blocked despite its cancelled async waiter.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if !execution.terminations.lock().unwrap().is_empty() && effects.admit(&owner).is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &request.job_id,
            &reservation.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_ne!(
        pending.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Completed)
    );
    assert!(
        api.require_backend_job_attempt_effects_ended(&pending)
            .is_err()
    );
    assert!(api.runtime.worker(&worker).is_ok());
    release.send(()).unwrap();
    api.stop_backend_job_recovery().await;
    let drained = api
        .store
        .get_backend_job_attempt(
            api.workspace_id(),
            &request.job_id,
            &reservation.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert!(
        api.require_backend_job_attempt_effects_ended(&drained)
            .is_ok()
    );
}

#[tokio::test]
async fn backend_job_runner_uses_registered_embedded_worker_and_structured_result_authority() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-check-1".to_string(),
        purpose: "runner_contract_check".to_string(),
        input_ref: "test://runner/input/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };

    let stale_reservation = api
        .store
        .reserve_backend_job(
            TEST_WORKSPACE_ID,
            &request,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    let dispatched = api
        .dispatch_backend_job_reservation(stale_reservation.clone())
        .unwrap();
    let concurrent_replay = api
        .dispatch_backend_job_reservation(stale_reservation)
        .unwrap();
    assert!(concurrent_replay.replayed);
    assert_eq!(
        execution.take_inputs().len(),
        1,
        "a stale concurrent dispatch replay must not submit the attempt twice"
    );
    assert_eq!(dispatched.attempt.state, BackendJobAttemptState::Dispatched);
    assert!(
        dispatched.attempt.runtime_run_id.is_some(),
        "accepted Job input must be fenced to durable Runtime run evidence"
    );
    let worker = dispatched.attempt.worker.clone().expect("bound Job Worker");
    assert_eq!(worker.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_some(),
        "Job dispatch must use the ordinary Workspace Worker registry"
    );
    assert_eq!(
        api.store
            .worker_registry_projection(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .unwrap()
            .job,
        Some(crate::backend_job::BackendJobWorkerBinding {
            job_id: request.job_id.clone(),
            attempt_id: dispatched.attempt.attempt_id.clone(),
            purpose: request.purpose.clone(),
        })
    );
    let input_error = send_runtime_worker_input(
        State(api.clone()),
        AxumPath((worker.runtime_id.clone(), worker.worker_id.clone())),
        Json(server_api::RuntimeWorkerInputRequest {
            submission_request_id: None,
            kind: Some("user".to_string()),
            content: "attempt to rerun immutable Job input".to_string(),
            segments: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        input_error.into_response().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        execution.take_inputs().is_empty(),
        "Job-owned Workers must reject interactive reruns"
    );
    let replay = api.dispatch_backend_job(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.attempt.worker, Some(worker.clone()));

    let accepted = api
        .accept_backend_job_result(
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id,
                input_digest: request.input_digest().unwrap(),
                result: serde_json::json!({"valid": true}),
            },
        )
        .unwrap();
    assert_eq!(accepted.job.state, BackendJobState::Completed);
}

#[tokio::test]
async fn terminal_backend_job_cleanup_removes_only_bound_worker_and_retains_result_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let notification_target = spawn_ticket_check_source(&api, "job-cleanup-target");
    let request = BackendJobRequest {
        job_id: "runner-cleanup-success-1".to_string(),
        purpose: "cleanup_contract_check".to_string(),
        input_ref: "test://runner/cleanup/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: Some(notification_target.clone()),
        notification_target: Some(notification_target.clone()),
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    execution.take_inputs();
    execution.reject_inputs("notification unavailable");
    let worker = dispatched.attempt.worker.clone().unwrap();
    seed_worker_session_for_cleanup(temp.path(), &worker);
    let mut subscriber = api.worker_projection.subscribe_ordered(10).unwrap();
    let _ = subscriber.take_snapshot();
    let submission = BackendJobResultSubmission {
        job_id: request.job_id.clone(),
        attempt_id: dispatched.attempt.attempt_id.clone(),
        input_digest: request.input_digest().unwrap(),
        result: serde_json::json!({"valid": true}),
    };

    let accepted = api.accept_backend_job_result(&worker, &submission).unwrap();
    assert_eq!(accepted.job.state, BackendJobState::Completed);
    let cleaned = wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert_eq!(cleaned.result, Some(serde_json::json!({"valid": true})));
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
    assert!(
        !temp
            .path()
            .join(".test-embedded-runtime-store/workers")
            .join(&worker.worker_id)
            .exists(),
        "Runtime Worker and Session storage must be removed"
    );
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_none(),
        "canonical removal must delete the Backend catalog projection"
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let event = subscriber.recv().await.expect("projection event");
            if event.changes.iter().any(|change| {
                matches!(change, crate::store::WorkerCatalogChange::Removed(removed) if removed == &worker)
            }) {
                return;
            }
        }
    })
    .await
    .expect("Backend Job cleanup must publish canonical Worker removal");
    assert!(
        api.runtime.worker(&notification_target).is_ok(),
        "origin/notification Workers are not cleanup targets"
    );
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &notification_target)
            .unwrap()
            .is_some()
    );
    let replay = api.accept_backend_job_result(&worker, &submission).unwrap();
    assert!(
        replay.replayed,
        "duplicate result remains idempotent after cleanup"
    );
    let delivery_id =
        crate::backend_job::delivery_id(&request.job_id, &dispatched.attempt.attempt_id);
    let evidence = job_notification_evidence(&api, &delivery_id);
    assert_eq!(evidence.0, "deliver");
    assert!(!evidence.2);
    assert_eq!(evidence.3.as_deref(), Some("acceptance_unknown"));
    assert!(
        api.store
            .job_notification_request(TEST_WORKSPACE_ID, &delivery_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn backend_job_cleanup_guard_failure_retries_old_attempt_without_touching_new_worker() {
    let temp = tempfile::tempdir().unwrap();
    let (api, _execution) = test_api_with_recording_backend(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-cleanup-retry-1".to_string(),
        purpose: "cleanup_retry_contract_check".to_string(),
        input_ref: "test://runner/cleanup-retry/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let first = api.dispatch_backend_job(&request).unwrap();
    let first_worker = first.attempt.worker.clone().unwrap();
    seed_worker_session_for_cleanup(temp.path(), &first_worker);
    let _ = set_worker_retention(
        api.clone(),
        first_worker.runtime_id.clone(),
        first_worker.worker_id.clone(),
        true,
    )
    .await
    .unwrap();
    api.finish_backend_job_attempt(
        &request.job_id,
        &first.attempt.attempt_id,
        BackendJobAttemptState::Failed,
        "model_failure",
        "model rejected the request",
    )
    .unwrap();
    let blocked = wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &first.attempt.attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    assert_eq!(blocked.failure_category.as_deref(), Some("model_failure"));
    assert_eq!(
        blocked.worker_cleanup_failure_category.as_deref(),
        Some("backend_job_cleanup_removal_rejected")
    );

    assert!(
        api.retry_backend_job(&request.job_id).is_err(),
        "retry must not overlap an incompletely cleaned attempt"
    );
    let _ = set_worker_retention(
        api.clone(),
        first_worker.runtime_id.clone(),
        first_worker.worker_id.clone(),
        false,
    )
    .await
    .unwrap();
    api.recover_backend_jobs().unwrap();
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &first.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    let second = api.retry_backend_job(&request.job_id).unwrap();
    let second_worker = second.attempt.worker.clone().unwrap();
    seed_worker_session_for_cleanup(temp.path(), &second_worker);
    assert_ne!(first_worker, second_worker);
    api.recover_backend_jobs().unwrap();
    assert!(api.runtime.worker(&second_worker).is_ok());

    api.accept_backend_job_result(
        &second_worker,
        &BackendJobResultSubmission {
            job_id: request.job_id.clone(),
            attempt_id: second.attempt.attempt_id.clone(),
            input_digest: request.input_digest().unwrap(),
            result: serde_json::json!({"retried": true}),
        },
    )
    .unwrap();
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &second.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert_eq!(
        api.store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &request.job_id,
                &first.attempt.attempt_id,
            )
            .unwrap()
            .unwrap()
            .failure_detail
            .as_deref(),
        Some("model rejected the request")
    );
}

#[tokio::test]
async fn backend_job_restart_without_live_host_owner_keeps_cleanup_unconfirmed() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_server_config(temp.path());
    let request = BackendJobRequest {
        job_id: "runner-cleanup-pre-marker-restart-1".to_string(),
        purpose: "cleanup_pre_marker_restart_contract_check".to_string(),
        input_ref: "test://runner/cleanup-pre-marker-restart/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let store =
        Arc::new(SqliteWorkspaceStore::open_for_test(config.database_path.clone()).unwrap());
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    let execution = Arc::new(DeterministicExecutionBackend::default());
    let embedded = EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(
        TEST_WORKSPACE_ID,
        config.embedded_runtime_store_root.clone(),
        execution.clone(),
    )
    .unwrap();
    let binding = test_create_binding();
    let runtime = embedded.subscription_runtime();
    let runtime_worker_ref = worker_runtime::identity::WorkerRef::new(binding.worker_id);
    let detail = runtime
        .create_worker_scoped(
            &worker_runtime::RuntimeWorkspaceScope::new(TEST_WORKSPACE_ID, "embedded-backend"),
            worker_runtime::catalog::CreateWorkerRequest {
                plugin_execution: None,
                worker_id: binding.worker_id,
                create_fingerprint: binding.create_fingerprint,
                profile: ProfileSelector::Builtin(request.profile.clone()),
                display_name: Some("job:cleanup_pre_marker_restart_contract_check".to_string()),
                config_bundle: None,
                profile_source: worker_runtime::catalog::ProfileSourceArchiveSource::Embedded {
                    archive: crate::profile_settings::builtin_profile_source_archive(
                        &ProfileSelector::Builtin(request.profile.clone()),
                    )
                    .unwrap(),
                },
                initial_input: None,
                workdir_attachment_requests: Vec::new(),
                workdir_attachments: Vec::new(),
                worker_observation_enabled: false,
                worker_observation_grants: Vec::new(),
                workspace_api: Some(test_worker_workspace_api(EMBEDDED_WORKER_RUNTIME_ID)),
                memory_settings: Some(test_worker_memory_settings()),
                subjektiv_attached: false,
                backend_job: None,
            },
        )
        .unwrap();
    let worker = RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, detail.worker_id.to_string());
    runtime
        .observe_worker_event(
            &runtime_worker_ref,
            protocol::Event::WorkerState {
                snapshot: protocol::WorkerStateSnapshot {
                    last_command_id: 0,
                    last_finished_submission_request_id: None,
                    state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                        protocol::WorkerRunState::Running,
                    )),
                },
            },
        )
        .unwrap();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    let reservation = store
        .reserve_backend_job(TEST_WORKSPACE_ID, &request, &now)
        .unwrap();
    let (_, claimed) = store
        .claim_backend_job_attempt_dispatch(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reservation.attempt.attempt_id,
            &now,
        )
        .unwrap();
    assert!(claimed);
    let runtime_run_id = crate::backend_job::runtime_run_id(&reservation.attempt.attempt_id);
    store
        .upsert_worker_registry(&WorkerRegistryRecord {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            worker: worker.clone(),
            display_name: "job:cleanup_pre_marker_restart_contract_check".to_string(),
            profile: Some(request.profile.clone()),
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        })
        .unwrap();
    store
        .bind_backend_job_attempt_worker(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reservation.attempt.attempt_id,
            &worker,
            Some(&runtime_run_id),
            &now,
        )
        .unwrap();
    seed_worker_session_for_cleanup(temp.path(), &worker);
    let before_restart = runtime.worker_detail(&runtime_worker_ref).unwrap();
    assert!(!before_restart.execution_reconstructed);
    assert!(before_restart.worker_state.is_some_and(|snapshot| {
        snapshot.last_command_id == 0
            && snapshot.last_finished_submission_request_id.is_none()
            && snapshot.state
                == protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                    protocol::WorkerRunState::Running,
                ))
    }));
    store
        .accept_backend_job_result(
            TEST_WORKSPACE_ID,
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: reservation.attempt.attempt_id.clone(),
                input_digest: request.input_digest().unwrap(),
                result: serde_json::json!({"persisted": true}),
            },
            &now,
        )
        .unwrap();
    assert_eq!(
        store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &request.job_id,
                &reservation.attempt.attempt_id,
            )
            .unwrap()
            .unwrap()
            .worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Pending)
    );
    let attempt_id = reservation.attempt.attempt_id;
    drop(runtime);
    drop(embedded);
    drop(store);
    drop(execution);

    let restarted_store =
        SqliteWorkspaceStore::open_for_test(config.database_path.clone()).unwrap();
    let restarted_execution = Arc::new(DeterministicExecutionBackend::default());
    restarted_execution.accept_restores();
    let restarted = WorkspaceApi::new_with_execution_backend(
        config,
        Arc::new(restarted_store),
        restarted_execution,
    )
    .await
    .unwrap();
    let cleaned = wait_for_backend_job_worker_cleanup(
        &restarted,
        &request.job_id,
        &attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    assert_eq!(cleaned.result, Some(serde_json::json!({"persisted": true})));
    assert!(
        restarted
            .require_backend_job_attempt_effects_ended(&cleaned)
            .is_err()
    );
    assert!(
        restarted.runtime.worker(&worker).is_ok(),
        "missing Host drain is not removal authority"
    );
    restarted.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_fast_result_is_accepted_after_prebound_tracked_run() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "r".repeat(256),
        purpose: "fast_result_contract_check".to_string(),
        input_ref: "test://runner/fast-result/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let acceptance_observed = Arc::new(std::sync::Mutex::new(None));
    let hook_api = api.clone();
    let hook_request = request.clone();
    let hook_observed = acceptance_observed.clone();
    let hook_root = temp.path().to_path_buf();
    execution.before_input_returns(move |worker| {
        let source =
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, worker.worker_id.to_string());
        seed_worker_session_for_cleanup(&hook_root, &source);
        let accepted = hook_api
            .accept_backend_job_result(
                &source,
                &BackendJobResultSubmission {
                    job_id: hook_request.job_id.clone(),
                    attempt_id: crate::backend_job::attempt_id(&hook_request.job_id, 1),
                    input_digest: hook_request.input_digest().unwrap(),
                    result: serde_json::json!({"valid": true}),
                },
            )
            .is_ok();
        *hook_observed.lock().unwrap() = Some(accepted);
    });

    let dispatched = api.dispatch_backend_job(&request).unwrap();
    assert_eq!(*acceptance_observed.lock().unwrap(), Some(true));
    assert_eq!(dispatched.job.state, BackendJobState::Completed);
    let expected_run_id =
        crate::backend_job::runtime_run_id(&crate::backend_job::attempt_id(&request.job_id, 1));
    assert_eq!(
        dispatched.attempt.runtime_run_id.as_deref(),
        Some(expected_run_id.as_str())
    );
    let worker = dispatched.attempt.worker.clone().unwrap();
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
}

#[tokio::test]
async fn backend_job_result_http_response_precedes_cleanup_until_tracked_run_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-result-response-fence-1".to_string(),
        purpose: "result_response_fence_check".to_string(),
        input_ref: "test://runner/result-response-fence/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.clone().unwrap();
    let runtime_run_id = dispatched.attempt.runtime_run_id.clone().unwrap();
    seed_worker_session_for_cleanup(temp.path(), &worker);
    execution.publish_worker_state(
        &worker,
        protocol::WorkerStateSnapshot {
            last_command_id: 0,
            last_finished_submission_request_id: None,
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
        },
    );
    assert!(
        api.runtime
            .worker(&worker)
            .unwrap()
            .worker_state
            .is_some_and(|snapshot| snapshot.state
                == protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                    protocol::WorkerRunState::Running
                )))
    );

    let Json(acceptance) = scoped_submit_backend_job_result(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
        }),
        ticket_check_headers(&worker),
        Json(SubmitBackendJobResultRequest {
            result: serde_json::json!({"valid": true}),
        }),
    )
    .await
    .unwrap();
    assert_eq!(acceptance.job.state, BackendJobState::Completed);
    let still_running = api.runtime.worker(&worker).unwrap();
    assert!(still_running.worker_state.is_some_and(|snapshot| {
        snapshot.state
            == protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            ))
    }));
    // Cleanup admission precedes its durable claim. The accepted job may
    // still be queued, but neither queued nor executing ownership may stop
    // this Run before the exact tracked finish marker below is published.
    assert!(matches!(
        api.store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &request.job_id,
                &dispatched.attempt.attempt_id,
            )
            .unwrap()
            .unwrap()
            .worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Pending | BackendJobWorkerCleanupState::Executing)
    ));

    execution.publish_worker_state(
        &worker,
        protocol::WorkerStateSnapshot {
            last_command_id: 0,
            last_finished_submission_request_id: Some(runtime_run_id),
            state: protocol::WorkerState::Idle,
        },
    );
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
}

#[tokio::test]
async fn backend_job_delivery_recovery_replays_same_tracked_notification_id() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_server_config(temp.path());
    let store = SqliteWorkspaceStore::open_for_test(config.database_path.clone()).unwrap();
    let execution = Arc::new(DeterministicExecutionBackend::default());
    let api = WorkspaceApi::new_with_execution_backend(
        config.clone(),
        Arc::new(store),
        execution.clone(),
    )
    .await
    .unwrap();
    seed_test_delivery_binding(&api);
    let target = api
        .spawn_workspace_worker(
            EMBEDDED_WORKER_RUNTIME_ID,
            WorkerSpawnRequest {
                requested_worker_name: Some("job-notification-target".to_string()),
                intent: WorkerSpawnIntent::WorkspaceCompanion,
                singleton_key: None,
                acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                    expected_segments: 0,
                },
                profile: ProfileSelector::Builtin("builtin:companion".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                workdir_attachment_requests: Vec::new(),
                resolved_workdir_attachment_requests: Vec::new(),
                resolved_workdir_attachments: Vec::new(),
                resolved_config_bundle: Some(runtime_test_bundle()),
                resolved_worker_observation_enabled: false,
                resolved_worker_observation_grants: Vec::new(),
                resolved_workspace_api: None,
                resolved_memory_settings: None,
                resolved_subjektiv_attached: false,
                resolved_control_operation: None,
                resolved_execution_grant: None,
            },
        )
        .unwrap()
        .worker
        .unwrap()
        .worker;
    let request = BackendJobRequest {
        job_id: "n".repeat(256),
        purpose: "delivery_recovery_contract_check".to_string(),
        input_ref: "test://runner/delivery-recovery/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: Some(target.clone()),
        limits: crate::backend_job::BackendJobLimits {
            timeout_seconds: 1,
            ..Default::default()
        },
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.clone().unwrap();
    let acceptance = api
        .store
        .accept_backend_job_result(
            TEST_WORKSPACE_ID,
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id.clone(),
                input_digest: request.input_digest().unwrap(),
                result: serde_json::json!({"valid": true}),
            },
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    let delivery_id =
        crate::backend_job::delivery_id(&request.job_id, &acceptance.attempt.attempt_id);
    let (_, claimed) = api
        .store
        .reserve_backend_job_delivery(
            TEST_WORKSPACE_ID,
            &delivery_id,
            &request.job_id,
            &target,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    assert!(claimed);
    let recoverable = api
        .store
        .recover_backend_job_deliveries(
            TEST_WORKSPACE_ID,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    execution.take_input_request_ids();
    api.deliver_backend_job_result(&acceptance, &target);
    let request_ids = execution.take_input_request_ids();
    assert!(
        request_ids
            .iter()
            .any(|request_id| request_id.as_deref() == Some(delivery_id.as_str())),
        "recovery must replay the durable delivery id as the tracked notification id: {request_ids:?}"
    );
    assert_eq!(job_notification_evidence(&api, &delivery_id).2, true);
    assert!(
        api.store
            .list_pending_job_notifications(TEST_WORKSPACE_ID, 100)
            .unwrap()
            .is_empty()
    );
    notification_delivery::recover_job_notifications(&api);
    assert!(execution.take_input_request_ids().is_empty());
}

#[tokio::test]
async fn backend_job_missing_runtime_is_a_definitive_dispatch_rejection() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    api.runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 10)
        .unwrap();
    let request = BackendJobRequest {
        job_id: "runner-spawn-error-1".to_string(),
        purpose: "spawn_error_contract_check".to_string(),
        input_ref: "test://runner/spawn-error/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };

    assert!(api.dispatch_backend_job(&request).is_err());
    let attempt = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &crate::backend_job::attempt_id(&request.job_id, 1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(attempt.state, BackendJobAttemptState::Failed);
    assert_eq!(
        attempt.failure_category.as_deref(),
        Some("dispatch_rejected")
    );
}

#[tokio::test]
async fn backend_job_input_response_loss_is_owned_until_deadline_without_resubmission() {
    let temp = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    let cleanup_root = temp.path().to_path_buf();
    execution.before_input_returns(move |worker| {
        let worker =
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, worker.worker_id.to_string());
        seed_worker_session_for_cleanup(&cleanup_root, &worker);
    });
    execution.reject_inputs("model transport unavailable");
    let request = BackendJobRequest {
        job_id: "runner-input-failure-1".to_string(),
        purpose: "failure_contract_check".to_string(),
        input_ref: "test://runner/failure/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits {
            timeout_seconds: 1,
            ..Default::default()
        },
    };

    assert!(api.dispatch_backend_job(&request).is_err());
    let failed = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &crate::backend_job::attempt_id(&request.job_id, 1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, BackendJobAttemptState::Dispatched);
    assert_eq!(
        failed.failure_category.as_deref(),
        Some("submission_receipt_unconfirmed")
    );
    let worker = failed.worker.clone().unwrap();
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &failed.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
    assert_eq!(
        execution.take_inputs().len(),
        1,
        "ambiguous input was never blindly resubmitted"
    );
    assert!(
        api.store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &request.job_id,
                &crate::backend_job::attempt_id(&request.job_id, 2),
            )
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn backend_job_recovery_drains_more_than_one_reserved_page() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    api.runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 10)
        .unwrap();
    let mut jobs = Vec::new();
    for index in 0..(usize::from(ABSOLUTE_MAX_CONCURRENT_JOBS) + 6) {
        let request = BackendJobRequest {
            job_id: format!("restart-page-{index:03}"),
            purpose: "restart_page_contract_check".to_string(),
            input_ref: format!("fixture://restart-page/{index}"),
            input: serde_json::json!({"index": index}),
            instruction: "Return a structured fixture result.".to_string(),
            profile: "builtin:backend-job".to_string(),
            grants: Default::default(),
            serialization_key: None,
            source_worker: None,
            notification_target: None,
            limits: crate::backend_job::BackendJobLimits {
                max_concurrent_jobs: ABSOLUTE_MAX_CONCURRENT_JOBS,
                ..Default::default()
            },
        };
        api.store
            .reserve_backend_job(
                TEST_WORKSPACE_ID,
                &request,
                &format!("2026-09-01T00:{:02}:{:02}Z", index / 60, index % 60),
            )
            .unwrap();
        jobs.push(request.job_id);
    }

    api.recover_backend_jobs().unwrap();
    assert!(
        api.store
            .list_reserved_backend_job_attempts(TEST_WORKSPACE_ID, jobs.len())
            .unwrap()
            .is_empty(),
        "restart recovery must continue beyond its first bounded page when dispatches terminalize"
    );
    assert!(jobs.iter().all(|job_id| {
        api.store
            .get_backend_job(TEST_WORKSPACE_ID, job_id)
            .unwrap()
            .is_some_and(|job| job.state == BackendJobState::Failed)
    }));
}

#[tokio::test]
async fn backend_job_recovery_dispatches_reserved_attempt_without_duplicate_worker() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-restart-1".to_string(),
        purpose: "restart_contract_check".to_string(),
        input_ref: "test://runner/restart/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let reserved = api
        .store
        .reserve_backend_job(
            TEST_WORKSPACE_ID,
            &request,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    assert_eq!(reserved.attempt.state, BackendJobAttemptState::Reserved);
    api.recover_backend_jobs().unwrap();

    let recovered = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reserved.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(recovered.state, BackendJobAttemptState::Dispatched);
    let recovered_worker = recovered.worker.clone().expect("recovered Job Worker");
    let replay = api.dispatch_backend_job(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.attempt.worker, Some(recovered_worker));
}

#[tokio::test]
async fn backend_job_missing_worker_reaches_failure_without_fabricated_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-unknown-1".to_string(),
        purpose: "unknown_contract_check".to_string(),
        input_ref: "test://runner/unknown/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Return a structured check result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits::default(),
    };
    let reserved = api
        .store
        .reserve_backend_job(
            TEST_WORKSPACE_ID,
            &request,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();
    let missing_worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        "0199a3ae-ecdf-7eb2-883e-a5b7b5f04a0d",
    );
    api.store
        .upsert_worker_registry(&WorkerRegistryRecord {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            worker: missing_worker.clone(),
            display_name: "Missing Job Worker".to_string(),
            profile: Some("builtin:backend-job".to_string()),
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "2026-09-01T00:00:00Z".to_string(),
            updated_at: "2026-09-01T00:00:00Z".to_string(),
        })
        .unwrap();
    let (_, claimed) = api
        .store
        .claim_backend_job_attempt_dispatch(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reserved.attempt.attempt_id,
            "2026-09-01T00:00:00Z",
        )
        .unwrap();
    assert!(claimed);
    api.store
        .bind_backend_job_attempt_worker(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reserved.attempt.attempt_id,
            &missing_worker,
            Some("missing-runtime-run"),
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
        .unwrap();

    api.recover_backend_jobs().unwrap();
    let attempt = wait_for_attempt(
        &api,
        &request.job_id,
        &reserved.attempt.attempt_id,
        |attempt| attempt.state == BackendJobAttemptState::Failed,
    )
    .await;
    assert_eq!(attempt.failure_category.as_deref(), Some("timeout"));
    assert_ne!(
        attempt.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Completed)
    );
    assert!(
        api.require_backend_job_attempt_effects_ended(&attempt)
            .is_err()
    );
    assert!(api.retry_backend_job(&request.job_id).is_err());
    api.stop_backend_job_recovery().await;
}

#[tokio::test]
async fn backend_job_timeout_fences_late_structured_result() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let request = BackendJobRequest {
        job_id: "runner-timeout-1".to_string(),
        purpose: "timeout_contract_check".to_string(),
        input_ref: "test://runner/timeout/1".to_string(),
        input: serde_json::json!({"value": "immutable"}),
        instruction: "Wait for a structured result.".to_string(),
        profile: "builtin:backend-job".to_string(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: crate::backend_job::BackendJobLimits {
            timeout_seconds: 1,
            max_attempts: 1,
            ..Default::default()
        },
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.clone().unwrap();
    seed_worker_session_for_cleanup(temp.path(), &worker);
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    let attempt = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &dispatched.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(attempt.state, BackendJobAttemptState::Failed);
    assert_eq!(attempt.failure_category.as_deref(), Some("timeout"));
    wait_for_backend_job_worker_cleanup(
        &api,
        &request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(matches!(
        api.runtime.worker(&worker),
        Err(RuntimeRegistryError::UnknownWorker { .. })
    ));
    assert!(
        api.accept_backend_job_result(
            &worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id,
                input_digest: request.input_digest().unwrap(),
                result: serde_json::json!({"late": true}),
            },
        )
        .is_err()
    );
}
