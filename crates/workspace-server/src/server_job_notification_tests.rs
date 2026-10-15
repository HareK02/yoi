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
async fn job_notification_checker_read_failure_reopen_recovers_same_source_to_real_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let target = spawn_ticket_check_source(&api, "checker-retry-target");
    let backend = browser_ticket_backend(&api).unwrap();
    let ticket_ref = backend.create(ticket::NewTicket::new("Writer-only scope in requirements")).unwrap();
    let ticket = backend.show(ticket_ref.id.into()).unwrap();
    let request = ticket_item_checker::request(
        &ticket,
        ticket_item_checker::TicketItemCheckEdit::Create,
        target.clone(),
        "Check saved Ticket content.".into(),
    ).unwrap();
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let submission = BackendJobResultSubmission {
        job_id: request.job_id.clone(),
        attempt_id: dispatched.attempt.attempt_id.clone(),
        input_digest: request.input_digest().unwrap(),
        result: json!({"findings":[{"category":"writer_scope","quote":"Writer-only scope","reason":"Writer scope is mixed into requirements.","suggestion":"Remove the writer-specific sentence."}]}),
    };
    let accepted = api.store.accept_backend_job_result(TEST_WORKSPACE_ID, worker, &submission, TEST_CREATED_AT).unwrap();
    let source = api.store.job_notification_source(TEST_WORKSPACE_ID, &request.job_id, &accepted.attempt.attempt_id).unwrap();
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let source_snapshot = || conn.query_row("SELECT sequence,facts_json FROM notification_sources WHERE workspace_id=?1 AND input_event_id=?2", rusqlite::params![TEST_WORKSPACE_ID,source.delivery_id], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap();
    let original_source = source_snapshot();
    // Fail exactly one checker read through the real adapter's error branch.
    // Its independent Ticket connection cannot open a directory as SQLite, while
    // the already-open result store remains available to persist failure evidence.
    let mut unavailable = api.clone();
    unavailable.config.database_path = dir.path().to_path_buf();
    notification_delivery::recover_job_notifications(&unavailable);
    let failures = || {
        let mut query = conn.prepare("SELECT sequence,source_sequence,code,recorded_at FROM notification_source_preparation_failures WHERE source_sequence=?1 ORDER BY sequence").unwrap();
        query.query_map([original_source.0], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    let failed_once = failures();
    assert_eq!(failed_once.len(), 1);
    assert_eq!(failed_once[0].2, "source_lookup_failed");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_decisions WHERE source_sequence=?1", [original_source.0], |r| r.get::<_,i64>(0)).unwrap(), 0);
    assert!(api.store.job_notification_request(TEST_WORKSPACE_ID, &source.delivery_id).unwrap().is_none());
    assert!(delivery_receipts(&api, &target).is_empty());
    assert!(!execution.attempts.lock().unwrap().iter().any(|attempt| attempt.0 == source.delivery_id));
    drop(unavailable);

    // Reopen Backend persistence and run the unchanged pending-source scan. It
    // rereads the real Ticket, obtains findings, then reaches the actual Runtime.
    let mut reopened = api.clone();
    reopened.store = Arc::new(SqliteWorkspaceStore::open_for_test(&api.config.database_path).unwrap());
    let pending = reopened.store.list_pending_job_notifications(TEST_WORKSPACE_ID, 100).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].delivery_id, source.delivery_id);
    assert_eq!(pending[0].job_id, source.job_id);
    assert_eq!(pending[0].attempt_id, source.attempt_id);
    notification_delivery::recover_job_notifications(&reopened);
    delivery_wait_idle(&reopened, &target, 1).await;
    assert_eq!(job_notification_evidence(&reopened, &source.delivery_id), ("deliver".into(), "result".into(), true, None));
    let pinned = reopened.store.job_notification_request(TEST_WORKSPACE_ID, &source.delivery_id).unwrap().unwrap();
    assert_eq!(pinned.1, target);
    assert!(pinned.0.contains("Quote: \"Writer-only scope\""));
    assert_eq!(delivery_commits(&reopened, &target), vec![pinned.0.clone()]);
    assert_eq!(delivery_receipts(&reopened, &target).len(), 1);
    assert!(execution.attempts.lock().unwrap().iter().any(|attempt| attempt.0 == source.delivery_id && attempt.1 == pinned.0));
    assert_eq!(source_snapshot(), original_source);
    assert_eq!(failures(), failed_once, "recovery must retain the original failure observation");
    let job = reopened.store.get_backend_job(TEST_WORKSPACE_ID, &request.job_id).unwrap().unwrap();
    assert_eq!(job.state, BackendJobState::Completed);
    assert_eq!(job.current_attempt, accepted.job.current_attempt);
    assert_eq!(job.result, accepted.job.result);
    assert_eq!(job.result_digest, accepted.job.result_digest);
    assert!(reopened.store.accept_backend_job_result(TEST_WORKSPACE_ID, worker, &submission, TEST_CREATED_AT).unwrap().replayed);
    notification_delivery::recover_job_notifications(&reopened);
    assert_eq!(delivery_receipts(&reopened, &target).len(), 1);
    assert!(reopened.store.list_pending_job_notifications(TEST_WORKSPACE_ID, 100).unwrap().is_empty());
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
