// Uses the real Controller/session fixtures above; only model output and the
// deliberately lost Runtime response are substituted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_notification_real_worker_ack_loss_reopen_and_parallel_recovery_commits_once() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let target = spawn_ticket_check_source(&api, "fixed-job-result-target");
    let request = BackendJobRequest {
        job_id: "real-job-notification".into(),
        purpose: "result_notification_fixture".into(),
        input_ref: "test://job/result".into(),
        input: json!({"fixed":true}),
        instruction: "Return a result.".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: Some(target.clone()),
        limits: Default::default(),
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let submission = BackendJobResultSubmission {
        job_id: request.job_id.clone(),
        attempt_id: dispatched.attempt.attempt_id.clone(),
        input_digest: request.input_digest().unwrap(),
        result: json!({"result":"saved"}),
    };
    let accepted = api
        .store
        .accept_backend_job_result(TEST_WORKSPACE_ID, worker, &submission, TEST_CREATED_AT)
        .unwrap();
    let id = crate::backend_job::delivery_id(&request.job_id, &accepted.attempt.attempt_id);
    // Result commit without its wakeup. No old reserve record is necessary.
    assert!(
        api.store
            .job_notification_request(TEST_WORKSPACE_ID, &id)
            .unwrap()
            .is_none()
    );
    execution
        .lose_responses
        .store(true, std::sync::atomic::Ordering::SeqCst);
    notification_delivery::recover_job_notifications(&api);
    delivery_wait_idle(&api, &target, 1).await;
    let pinned = api
        .store
        .job_notification_request(TEST_WORKSPACE_ID, &id)
        .unwrap()
        .unwrap();
    let evidence = job_notification_evidence(&api, &id);
    assert!(!evidence.2);
    assert_eq!(evidence.3.as_deref(), Some("acceptance_unknown"));
    let diagnostics=delivery_http(&api,"GET",&format!("/api/w/{TEST_WORKSPACE_ID}/notification-diagnostics?job_id={}",request.job_id),None,StatusCode::OK).await;
    assert_eq!(diagnostics["items"][0]["stage"],"acceptance_unknown");
    assert_eq!(diagnostics["items"][0]["target_worker_id"],target.worker_id);
    assert_eq!(diagnostics["items"][0]["acceptance_outcome_unknown"],true);
    assert_eq!(delivery_receipts(&api, &target).len(), 1);
    // Reopen only Backend persistence, retaining the real Runtime receipt/session.
    let mut reopened = api.clone();
    reopened.store =
        Arc::new(SqliteWorkspaceStore::open_for_test(&api.config.database_path).unwrap());
    execution
        .lose_responses
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    std::thread::scope(|scope| {
        let joins = (0..4)
            .map(|_| {
                let api = reopened.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    notification_delivery::recover_job_notifications(&api);
                })
            })
            .collect::<Vec<_>>();
        for join in joins {
            join.join().unwrap();
        }
    });
    delivery_wait_idle(&reopened, &target, 1).await;
    assert!(job_notification_evidence(&reopened, &id).2);
    let diagnostics=delivery_http(&reopened,"GET",&format!("/api/w/{TEST_WORKSPACE_ID}/notification-diagnostics?job_id={}",request.job_id),None,StatusCode::OK).await;
    assert_eq!(diagnostics["items"][0]["stage"],"runtime_accepted");
    assert_eq!(diagnostics["items"][0]["target_worker_id"],target.worker_id);
    assert_eq!(diagnostics["items"][0]["acceptance_outcome_unknown"],false);
    assert_eq!(diagnostics["items"][0]["model_processing_confirmed"],false);
    assert_eq!(delivery_receipts(&reopened, &target).len(), 1);
    assert_eq!(delivery_commits(&reopened, &target), vec![pinned.0.clone()]);
    let attempts = execution.attempts.lock().unwrap();
    let retries = attempts
        .iter()
        .filter(|attempt| attempt.0 == id)
        .collect::<Vec<_>>();
    assert!(retries.len() >= 2);
    assert!(retries.iter().all(|attempt| attempt.1 == pinned.0));
    drop(attempts);
    assert!(
        reopened
            .store
            .accept_backend_job_result(TEST_WORKSPACE_ID, worker, &submission, TEST_CREATED_AT)
            .unwrap()
            .replayed
    );
    assert_eq!(
        reopened
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &request.job_id)
            .unwrap()
            .unwrap()
            .current_attempt,
        1
    );
    assert!(
        reopened
            .store
            .list_pending_job_notifications(TEST_WORKSPACE_ID, 100)
            .unwrap()
            .is_empty()
    );
    delivery_stop(&reopened, &target);
    delivery_stop(&reopened, worker);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_notification_stopped_fixed_recipient_is_not_replaced_or_restored() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let target = spawn_ticket_check_source(&api, "stopped-job-target");
    let request = BackendJobRequest {
        job_id: "stopped-fixed-job".into(),
        purpose: "result_notification_fixture".into(),
        input_ref: "test://job/stopped".into(),
        input: json!({"fixed":true}),
        instruction: "Return a result.".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: Some(target.clone()),
        limits: Default::default(),
    };
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let accepted = api
        .store
        .accept_backend_job_result(
            TEST_WORKSPACE_ID,
            worker,
            &BackendJobResultSubmission {
                job_id: request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id.clone(),
                input_digest: request.input_digest().unwrap(),
                result: json!({"saved":true}),
            },
            TEST_CREATED_AT,
        )
        .unwrap();
    let id = crate::backend_job::delivery_id(&request.job_id, &accepted.attempt.attempt_id);
    delivery_stop(&api, &target);
    let replacement = spawn_ticket_check_source(&api, "different-job-target");
    notification_delivery::recover_job_notifications(&api);
    let evidence = job_notification_evidence(&api, &id);
    assert!(!evidence.2);
    assert_eq!(evidence.3.as_deref(), Some("target_stopped"));
    assert!(
        !execution
            .attempts
            .lock()
            .unwrap()
            .iter()
            .any(|attempt| attempt.0 == id)
    );
    assert_eq!(
        api.store
            .job_notification_request(TEST_WORKSPACE_ID, &id)
            .unwrap()
            .unwrap()
            .1,
        target
    );
    assert_eq!(api.runtime.worker(&target).unwrap().state, "stopped");
    assert_eq!(
        api.store
            .get_backend_job(TEST_WORKSPACE_ID, &request.job_id)
            .unwrap()
            .unwrap()
            .result,
        Some(json!({"saved":true}))
    );
    delivery_stop(&api, &replacement);
    delivery_stop(&api, worker);
}
