use super::*;
use worker::job::JobResultSink;

fn request(id: &str) -> JobRequest {
    JobRequest {
        job_id: id.into(),
        purpose: "test".into(),
        input_revision: "r1".into(),
        input_ref: "fixture:input".into(),
        input: serde_json::json!({"data":42}),
        instruction: "Return a structured result".into(),
        profile: "builtin:job".into(),
        serialization_key: None,
        limits: job::JobLimits::default(),
    }
}
fn sink(jobs: &StandaloneJobs, id: &str, attempt: &str) -> BoundResultSink {
    BoundResultSink {
        store: jobs.store.clone(),
        job_id: id.into(),
        attempt_id: attempt.into(),
        input_revision: "r1".into(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
    }
}
fn result(id: &str, attempt: &str) -> JobResultSubmission {
    JobResultSubmission {
        job_id: id.into(),
        attempt_id: attempt.into(),
        input_revision: "r1".into(),
        result: serde_json::json!({"valid":true}),
    }
}

#[tokio::test]
async fn host_result_capability_cannot_cross_job_attempt_revision_or_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let first = jobs.request(request("first")).unwrap();
    let second = jobs.request(request("second")).unwrap();
    for snapshot in [&first, &second] {
        jobs.with_store(|store| {
            store.dispatch(&snapshot.request.job_id, &snapshot.attempt.attempt_id, 8)
        })
        .unwrap();
    }
    let bound = sink(&jobs, "first", &first.attempt.attempt_id);
    assert!(
        bound
            .submit(result("second", &second.attempt.attempt_id))
            .is_err()
    );
    assert!(bound.submit(result("first", "first:attempt:2")).is_err());
    let mut stale = result("first", &first.attempt.attempt_id);
    stale.input_revision = "r0".into();
    assert!(bound.submit(stale).is_err());
    let mut expired = sink(&jobs, "first", &first.attempt.attempt_id);
    expired.deadline = tokio::time::Instant::now() - Duration::from_secs(1);
    assert!(
        expired
            .submit(result("first", &first.attempt.attempt_id))
            .is_err()
    );
    assert!(jobs.get("first").unwrap().result.is_none());
    bound
        .submit(result("first", &first.attempt.attempt_id))
        .unwrap();
    // Accepted exact replay is still allowed after the deadline, never a new result.
    expired
        .submit(result("first", &first.attempt.attempt_id))
        .unwrap();
    let mut changed = result("first", &first.attempt.attempt_id);
    changed.result = serde_json::json!({"valid":false});
    assert!(expired.submit(changed).is_err());
    assert_eq!(jobs.get("second").unwrap().state, JobState::Pending);
    jobs.shutdown().await.unwrap();
    assert!(
        bound
            .submit(result("first", &first.attempt.attempt_id))
            .is_err()
    );
}

#[tokio::test]
async fn cleanup_cannot_reclassify_success_or_durable_cancellation_and_stale_handles_are_closed() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let cancelled = jobs.request(request("cancelled")).unwrap();
    jobs.cancel("cancelled").unwrap();
    finish_if_pending(
        &jobs.store,
        "cancelled",
        &cancelled.attempt.attempt_id,
        JobAttemptState::Unknown,
        "cleanup",
    )
    .unwrap();
    assert_eq!(jobs.get("cancelled").unwrap().state, JobState::Cancelled);
    let accepted = jobs.request(request("accepted")).unwrap();
    jobs.with_store(|store| store.dispatch("accepted", &accepted.attempt.attempt_id, 8))
        .unwrap();
    sink(&jobs, "accepted", &accepted.attempt.attempt_id)
        .submit(result("accepted", &accepted.attempt.attempt_id))
        .unwrap();
    finish_if_pending(
        &jobs.store,
        "accepted",
        &accepted.attempt.attempt_id,
        JobAttemptState::Failed,
        "model failed later",
    )
    .unwrap();
    assert_eq!(jobs.get("accepted").unwrap().state, JobState::Completed);
    jobs.shutdown().await.unwrap();
    assert!(jobs.store.lock().unwrap().is_none());
    for operation in [
        jobs.cancel("cancelled"),
        jobs.acknowledge("accepted"),
        jobs.retry("cancelled"),
        jobs.request(request("new")),
    ] {
        assert!(matches!(operation, Err(JobServiceError::Closed)));
    }
}
