//! Restore integration boundary: real SQLite, service admission, Internal Worker,
//! result sink and cleanup. Only model transport and the WorkerStopping barrier
//! are scripted. No process exit or absent map is used as shutdown evidence.
use super::*;

async fn await_capacity_owner(jobs: &StandaloneJobs, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if jobs
                .result_recovery_diagnostic(id)
                .is_some_and(|cause| cause.contains("capacity is exhausted"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn two_results() -> DomainTestClient {
    let client = DomainTestClient::success();
    client.scripts.lock().unwrap().extend(
        DomainTestClient::success()
            .scripts
            .lock()
            .unwrap()
            .drain(..),
    );
    client
}

#[tokio::test]
async fn restored_results_wait_for_domain_cleanup_then_resume_exact_attempts_without_clicks() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let prior = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    prior
        .request_granted(domain_request("domain-first"), domain_grant("domain-first"))
        .unwrap();
    let first = prior.request(request("result-first")).unwrap();
    let second = prior.request(request("result-second")).unwrap();
    drop(prior);
    let mut jobs = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    jobs.capacity = 1;
    let hook = ControlledShutdown {
        block: true,
        ..Default::default()
    };
    let _release_on_failure = ReleaseShutdownOnDrop(hook.release.clone());
    jobs.bind_domain(Arc::new(hook.clone())).unwrap();
    let model = two_results();
    jobs.bind_model_client(Box::new(model.clone()));
    let ids = jobs.reserved_result_attempts().unwrap();
    assert_eq!(ids.len(), 2);
    jobs.start_with_model_client("domain-first", DomainTestClient::success())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), hook.entered.notified())
        .await
        .unwrap();
    let domain = jobs.get("domain-first").unwrap();
    assert_eq!(domain.state, JobState::Completed);
    assert!(!domain.executions_released());
    let recovery_jobs = jobs.clone();
    let owner = tokio::spawn(async move { recovery_jobs.resume_reserved_result_jobs(ids).await });
    await_capacity_owner(&jobs, "result-first").await;
    await_capacity_owner(&jobs, "result-second").await;
    assert_eq!(jobs.get("result-first").unwrap(), first);
    assert_eq!(jobs.get("result-second").unwrap(), second);
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(!owner.is_finished());
    // A reservation accepted after restore capture is not silently swept in.
    let outside = jobs.request(request("outside-capture")).unwrap();
    hook.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), owner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(jobs.get("domain-first").unwrap().executions_released());
    for original in [first, second] {
        let saved = jobs.get(&original.request.job_id).unwrap();
        assert_eq!(saved.state, JobState::Completed);
        assert_eq!(saved.request, original.request);
        assert_eq!(saved.input_digest, original.input_digest);
        assert_eq!(saved.attempt.attempt_id, original.attempt.attempt_id);
        assert_eq!(saved.attempt.number, 1);
        assert_eq!(saved.executions.len(), 1);
        assert!(saved.executions_released());
        assert_eq!(saved.result, Some(json!({"valid":true})));
        assert!(
            jobs.result_recovery_diagnostic(&saved.request.job_id)
                .is_none()
        );
    }
    assert_eq!(model.requests.lock().unwrap().len(), 4);
    assert!(model.scripts.lock().unwrap().is_empty());
    assert_eq!(jobs.get("outside-capture").unwrap(), outside);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM job_intents", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM job_attempts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn restored_results_save_individual_configuration_and_store_rejections_then_continue() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let prior = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    let mut invalid = request("bad-profile");
    invalid.profile = "builtin:no-such-job-profile".into();
    let bad_profile = prior.request(invalid).unwrap();
    let bad_store = prior.request(request("store-rejection")).unwrap();
    let good = prior.request(request("later-result")).unwrap();
    drop(prior);
    let jobs = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_one_dispatch BEFORE UPDATE OF state ON job_attempts
        WHEN NEW.job_id='store-rejection' AND NEW.state='dispatched'
        BEGIN SELECT RAISE(FAIL, 'scripted individual dispatch rejection'); END;",
    )
    .unwrap();
    let model = DomainTestClient::success();
    jobs.bind_model_client(Box::new(model.clone()));
    tokio::time::timeout(
        Duration::from_secs(10),
        jobs.resume_reserved_result_jobs(jobs.reserved_result_attempts().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    for (original, expected) in [
        (bad_profile, "no-such-job-profile"),
        (bad_store, "scripted individual dispatch rejection"),
    ] {
        let saved = jobs.get(&original.request.job_id).unwrap();
        assert_eq!(saved.state, JobState::Failed);
        assert_eq!(saved.attempt.attempt_id, original.attempt.attempt_id);
        assert_eq!(saved.attempt.number, 1);
        assert_eq!(saved.request, original.request);
        assert!(saved.attempt.failure.as_deref().unwrap().contains(expected));
        assert!(
            saved
                .attempt
                .failure
                .as_deref()
                .unwrap()
                .starts_with("result_recovery_cannot_start:")
        );
        assert!(saved.start_rejected_by.is_some());
        assert!(
            saved.executions_released(),
            "explicit pre-dispatch rejection, not Failed alone"
        );
        assert!(
            saved.executions.is_empty(),
            "dispatch was rejected, not a cleanup receipt"
        );
    }
    let saved = jobs.get("later-result").unwrap();
    assert_eq!(saved.state, JobState::Completed);
    assert_eq!(saved.attempt.attempt_id, good.attempt.attempt_id);
    assert_eq!(saved.executions.len(), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM job_attempts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn restored_results_continue_after_wait_error_without_forging_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    let failed = jobs.request(request("wait-error")).unwrap();
    let later = jobs.request(request("later-result")).unwrap();
    let ids = jobs.reserved_result_attempts().unwrap();
    let release = Arc::new(Notify::new());
    let _release_on_failure = ReleaseShutdownOnDrop(release.clone());
    let bad_model = DomainTestClient {
        failure_release: Some(release.clone()),
        ..DomainTestClient::success()
    };
    bad_model.scripts.lock().unwrap().pop_back();
    jobs.start_with_model_client("wait-error", bad_model.clone())
        .await
        .unwrap();
    let model = DomainTestClient::success();
    jobs.bind_model_client(Box::new(model.clone()));
    let owner_jobs = jobs.clone();
    let owner = tokio::spawn(async move { owner_jobs.resume_reserved_result_jobs(ids).await });
    // Wait for the later model to enter, then observe its real cleanup. The first
    // owner still cannot finish; it is held before a known provider failure.
    await_model(&model).await;
    assert_eq!(
        await_job(&jobs, "later-result").await.state,
        JobState::Completed
    );
    assert_eq!(
        jobs.get("later-result").unwrap().attempt.attempt_id,
        later.attempt.attempt_id
    );
    assert!(!owner.is_finished());
    release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), owner)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let saved = jobs.get("wait-error").unwrap();
    assert_eq!(saved.state, JobState::Completed);
    assert_eq!(saved.attempt.attempt_id, failed.attempt.attempt_id);
    assert_eq!(saved.executions.len(), 1);
    assert!(!saved.executions_released());
    assert!(
        jobs.result_recovery_diagnostic("wait-error")
            .unwrap()
            .contains("cleanup")
    );
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(bad_model.requests.lock().unwrap().len(), 2);
    assert!(matches!(
        jobs.shutdown().await,
        Err(JobServiceError::Cleanup(_))
    ));
    assert!(jobs.store.lock().unwrap().is_some());
}
