use super::*;
use agen::llm_client::{ClientError, Request, event::Event};
use async_trait::async_trait;
use futures::Stream;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::pin::Pin;
use worker::job::JobResultSink;

fn request(id: &str) -> JobRequest {
    JobRequest {
        job_id: id.into(),
        purpose: "test".into(),
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
        input_digest: request(id).input_digest().unwrap(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
    }
}
fn result(id: &str, attempt: &str) -> JobResultSubmission {
    JobResultSubmission {
        job_id: id.into(),
        attempt_id: attempt.into(),
        input_digest: request(id).input_digest().unwrap(),
        result: serde_json::json!({"valid":true}),
    }
}

#[derive(Clone, Default)]
struct DomainTestClient {
    scripts: Arc<Mutex<VecDeque<Vec<Event>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    entered: Arc<tokio::sync::Notify>,
    stall: bool,
    failure_release: Option<Arc<tokio::sync::Notify>>,
}
impl DomainTestClient {
    fn success() -> Self {
        Self {
            scripts: Arc::new(Mutex::new(VecDeque::from([
                vec![
                    Event::tool_use_start(0, "result-1", "SubmitJobResult"),
                    Event::tool_input_delta(0, r#"{"result":{"valid":true}}"#),
                    Event::tool_use_stop(0),
                ],
                vec![
                    Event::text_block_start(0),
                    Event::text_delta(0, "done"),
                    Event::text_block_stop(0, None),
                ],
            ]))),
            ..Default::default()
        }
    }
}
#[async_trait]
impl LlmClient for DomainTestClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        self.requests.lock().unwrap().push(request);
        self.entered.notify_one();
        if let Some(release) = &self.failure_release {
            release.notified().await;
            return Err(ClientError::Config(
                "known scripted provider failure".into(),
            ));
        }
        if self.stall {
            return Ok(Box::pin(futures::stream::pending()));
        }
        let events = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model turn");
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

type CapturedDomainAttempt = (JobSnapshot, Arc<dyn JobAttemptFence>);
#[derive(Default)]
struct RecordingDomainProvider(Mutex<Vec<CapturedDomainAttempt>>);
fn domain_grant(id: &str) -> Value {
    json!({"consumer":"test-domain", "job_id":id, "input_digest":request(id).input_digest().unwrap()})
}
fn domain_request(id: &str) -> JobRequest {
    let mut intent = request(id);
    intent.profile = "builtin:standalone-subjektiv-consolidation".into();
    intent
}
impl JobDomainProvider for RecordingDomainProvider {
    fn grant(
        &self,
        snapshot: &JobSnapshot,
        fence: Arc<dyn JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, String> {
        if snapshot.domain_grant != Some(domain_grant(&snapshot.request.job_id)) {
            return Err("foreign or mismatched immutable domain grant".into());
        }
        self.0.lock().unwrap().push((snapshot.clone(), fence));
        Ok(worker::job::JobFeatureGrant {
            satisfied_requirements: vec!["feature.subjektiv"],
            ..Default::default()
        })
    }
}
async fn await_model(client: &DomainTestClient) {
    tokio::time::timeout(Duration::from_secs(2), client.entered.notified())
        .await
        .unwrap();
}
async fn await_job(jobs: &StandaloneJobs, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(3), jobs.wait(id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn pending_domain_grants_survive_service_reopen_and_remain_immutable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let jobs = StandaloneJobs::open(&path, temp.path().to_path_buf()).unwrap();
    let intent = domain_request("granted");
    let grant = domain_grant("granted");
    let reserved = jobs.request_granted(intent.clone(), grant.clone()).unwrap();
    let root = jobs.request(request("root")).unwrap();
    assert_eq!(reserved.domain_grant, Some(grant.clone()));
    assert!(root.domain_grant.is_none());
    assert_eq!(
        jobs.request_granted(intent.clone(), grant.clone()).unwrap(),
        reserved
    );
    assert!(matches!(
        jobs.request(intent.clone()),
        Err(JobServiceError::Store(JobStoreError::IntentConflict(_)))
    ));
    assert!(matches!(
        jobs.request_granted(intent.clone(), domain_grant("foreign")),
        Err(JobServiceError::Store(JobStoreError::IntentConflict(_)))
    ));
    assert!(matches!(
        jobs.request_granted(request("root"), grant.clone()),
        Err(JobServiceError::Store(JobStoreError::IntentConflict(_)))
    ));
    assert_eq!(jobs.pending().unwrap().len(), 2);
    jobs.shutdown().await.unwrap();

    let reopened = StandaloneJobs::open(&path, temp.path().to_path_buf()).unwrap();
    let pending = reopened.pending().unwrap();
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&reserved));
    assert!(pending.contains(&root));
    assert_eq!(reopened.get("granted").unwrap(), reserved);
    assert_eq!(
        reopened
            .request_granted(intent.clone(), grant.clone())
            .unwrap(),
        reserved
    );
    assert!(matches!(
        reopened.request_granted(intent, json!({"different":true})),
        Err(JobServiceError::Store(JobStoreError::IntentConflict(_)))
    ));
    let provider = Arc::new(RecordingDomainProvider::default());
    reopened.bind_domain(provider.clone()).unwrap();
    reopened
        .start_with_model_client("granted", DomainTestClient::success())
        .await
        .unwrap();
    let completed = await_job(&reopened, "granted").await;
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.domain_grant, Some(grant));
    assert_eq!(completed.result, Some(json!({"valid":true})));
    let captured = provider.0.lock().unwrap()[0].0.clone();
    assert_eq!(
        captured, reserved,
        "provider receives the persisted immutable intent and grant"
    );
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn persisted_grant_requires_provider_and_missing_grant_cannot_be_derived_from_profile() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let granted = jobs
        .request_granted(domain_request("granted"), domain_grant("granted"))
        .unwrap();
    let client = DomainTestClient::success();
    let error = jobs
        .start_with_model_client("granted", client.clone())
        .await
        .unwrap_err();
    assert!(matches!(error, JobServiceError::Configuration(_)));
    assert!(
        error
            .to_string()
            .contains("unavailable explicit domain capability"),
        "{error}"
    );
    assert_eq!(jobs.get("granted").unwrap(), granted);
    assert!(client.requests.lock().unwrap().is_empty());

    let provider = Arc::new(RecordingDomainProvider::default());
    jobs.bind_domain(provider.clone()).unwrap();
    assert!(matches!(
        jobs.bind_domain(provider.clone()),
        Err(JobServiceError::Configuration(_))
    ));
    let ungranted = jobs.request(domain_request("ungranted")).unwrap();
    let error = jobs
        .start_with_model_client("ungranted", client.clone())
        .await
        .unwrap_err();
    assert!(matches!(error, JobServiceError::Configuration(_)));
    assert!(error.to_string().contains("feature.subjektiv"), "{error}");
    assert_eq!(jobs.get("ungranted").unwrap(), ungranted);
    assert!(
        provider.0.lock().unwrap().is_empty(),
        "bound provider is not a profile-derived grant"
    );
    assert!(client.requests.lock().unwrap().is_empty());
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn foreign_job_digest_and_consumer_grants_are_refused_before_model_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let provider = Arc::new(RecordingDomainProvider::default());
    jobs.bind_domain(provider.clone()).unwrap();
    for (id, grant) in [
        ("foreign-job", domain_grant("another-job")),
        (
            "foreign-digest",
            json!({"consumer":"test-domain","job_id":"foreign-digest","input_digest":"r0"}),
        ),
        (
            "foreign-consumer",
            json!({"consumer":"another-domain","job_id":"foreign-consumer","input_digest":request("foreign-consumer").input_digest().unwrap()}),
        ),
    ] {
        let reserved = jobs.request_granted(domain_request(id), grant).unwrap();
        let client = DomainTestClient::success();
        let error = jobs
            .start_with_model_client(id, client.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, JobServiceError::Configuration(_)));
        assert!(
            error.to_string().contains("foreign or mismatched"),
            "{error}"
        );
        assert_eq!(jobs.get(id).unwrap(), reserved);
        assert!(client.requests.lock().unwrap().is_empty());
    }
    assert!(provider.0.lock().unwrap().is_empty());
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn domain_fence_is_live_only_for_current_dispatched_attempt_and_revoked_on_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let provider = Arc::new(RecordingDomainProvider::default());
    jobs.bind_domain(provider.clone()).unwrap();
    let reserved = jobs
        .request_granted(domain_request("fenced"), domain_grant("fenced"))
        .unwrap();
    let failure_release = Arc::new(tokio::sync::Notify::new());
    let first = DomainTestClient {
        failure_release: Some(failure_release.clone()),
        ..Default::default()
    };
    jobs.start_with_model_client("fenced", first.clone())
        .await
        .unwrap();
    await_model(&first).await;
    let old_fence = provider.0.lock().unwrap()[0].1.clone();
    old_fence.ensure_live().unwrap();
    failure_release.notify_one();
    let failed = await_job(&jobs, "fenced").await;
    assert_eq!(failed.state, JobState::Failed);
    assert!(failed.result.is_none());
    assert!(old_fence.ensure_live().is_err());
    // Only known failure permits a retry; cancellation remains terminal.
    let retried = jobs.retry("fenced").unwrap();
    assert_ne!(retried.attempt.attempt_id, reserved.attempt.attempt_id);
    assert_eq!(retried.domain_grant, reserved.domain_grant);
    assert!(old_fence.ensure_live().is_err());
    let second = DomainTestClient {
        stall: true,
        ..Default::default()
    };
    jobs.start_with_model_client("fenced", second.clone())
        .await
        .unwrap();
    await_model(&second).await;
    let current_fence = provider.0.lock().unwrap()[1].1.clone();
    current_fence.ensure_live().unwrap();
    assert!(
        old_fence.ensure_live().is_err(),
        "a new attempt must not revive a stale capability"
    );
    let cancelled = jobs.cancel("fenced").unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(
        current_fence.ensure_live().is_err(),
        "durable cancellation revokes current domain operations immediately"
    );
    assert!(old_fence.ensure_live().is_err());
    let cleaned = await_job(&jobs, "fenced").await;
    assert_eq!(cleaned.state, JobState::Cancelled);
    assert!(cleaned.result.is_none());
    assert!(
        jobs.retry("fenced").is_err(),
        "cancelled intent is not a known retryable failure"
    );
    jobs.shutdown().await.unwrap();
    assert!(old_fence.ensure_live().is_err());
    assert!(current_fence.ensure_live().is_err());
}

#[tokio::test]
async fn bound_domain_does_not_change_root_generic_result_only_execution() {
    let temp = tempfile::tempdir().unwrap();
    let jobs =
        StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().to_path_buf()).unwrap();
    let provider = Arc::new(RecordingDomainProvider::default());
    jobs.bind_domain(provider.clone()).unwrap();
    let root = jobs.request(request("root")).unwrap();
    assert!(root.domain_grant.is_none());
    let client = DomainTestClient::success();
    jobs.start_with_model_client("root", client.clone())
        .await
        .unwrap();
    let completed = await_job(&jobs, "root").await;
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.result, Some(json!({"valid":true})));
    assert!(completed.domain_grant.is_none());
    assert!(provider.0.lock().unwrap().is_empty());
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for sent in requests.iter() {
        assert_eq!(
            sent.tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["SubmitJobResult"]
        );
        assert_eq!(sent.config.max_tokens, Some(worker::job::MAX_JOB_TOKENS));
    }
    drop(requests);
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_result_capability_cannot_cross_job_attempt_digest_or_deadline() {
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
    stale.input_digest = "r0".into();
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
