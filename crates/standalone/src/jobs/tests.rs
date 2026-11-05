#[path = "recovery_tests.rs"]
mod recovery_tests;

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
        storage_recovered: jobs.storage_recovered.clone(),
        revoked: Arc::new(AtomicBool::new(false)),
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
        if let Some(release) = &self.failure_release
            && self.scripts.lock().unwrap().is_empty()
        {
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
    fn callbacks_use_attempt_fence(&self) -> bool {
        true
    }

    fn grant(
        &self,
        snapshot: &JobSnapshot,
        fence: Arc<dyn JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, JobServiceError> {
        if snapshot.domain_grant != Some(domain_grant(&snapshot.request.job_id)) {
            return Err(JobServiceError::Configuration(
                "foreign or mismatched immutable domain grant".into(),
            ));
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

// The model and stopping hook are the only substituted boundaries. Dispatch, result
// persistence, the owned Internal Worker and service cleanup remain production code.
#[derive(Clone, Default)]
struct ControlledShutdown {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    block: bool,
    fail: bool,
}
impl worker::feature::FeatureModule for ControlledShutdown {
    fn descriptor(&self) -> worker::feature::FeatureDescriptor {
        worker::feature::FeatureDescriptor::builtin("job-cleanup-test", "Controlled shutdown")
            .with_hook(worker::feature::HookDeclaration::new(
                "stop",
                worker::feature::FeatureHookPoint::WorkerStopping,
            ))
    }
    fn install(
        &self,
        context: &mut worker::feature::FeatureInstallContext<'_>,
    ) -> Result<(), worker::feature::FeatureInstallError> {
        context.hooks().add_worker_stopping(
            "stop",
            worker::hook::HookExecutionPolicy::fail_closed(),
            self.clone(),
        )
    }
}
#[async_trait]
impl worker::hook::Hook<worker::hook::WorkerStopping> for ControlledShutdown {
    async fn call(
        &self,
        _: &worker::hook::WorkerStoppingContext,
    ) -> Result<(), worker::hook::HookError> {
        self.entered.notify_one();
        if self.block {
            self.release.notified().await;
        }
        if self.fail {
            Err(worker::hook::HookError::new(
                worker::hook::HookErrorCategory::Internal,
                "scripted Feature shutdown failure",
            ))
        } else {
            Ok(())
        }
    }
}
impl JobDomainProvider for ControlledShutdown {
    fn callbacks_use_attempt_fence(&self) -> bool {
        true
    }

    fn grant(
        &self,
        _: &JobSnapshot,
        _: Arc<dyn JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, JobServiceError> {
        Ok(worker::job::JobFeatureGrant {
            features: worker::feature::FeatureRegistryBuilder::new().with_module(self.clone()),
            satisfied_requirements: vec!["feature.subjektiv"],
            ..Default::default()
        })
    }
}
struct ReleaseShutdownOnDrop(Arc<tokio::sync::Notify>);
impl Drop for ReleaseShutdownOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[tokio::test]
async fn overdue_timeout_keeps_owned_future_and_capacity_until_feature_shutdown_returns() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    let hook = ControlledShutdown {
        block: true,
        ..Default::default()
    };
    let _release_on_failure = ReleaseShutdownOnDrop(hook.release.clone());
    jobs.bind_domain(Arc::new(hook.clone())).unwrap();
    let mut intent = domain_request("overdue");
    intent.limits.timeout_seconds = 1;
    intent.limits.max_concurrent_jobs = 1;
    intent.serialization_key = Some("owned-resource".into());
    let reserved = jobs
        .request_granted(intent.clone(), domain_grant("overdue"))
        .unwrap();
    jobs.request(request("next")).unwrap();
    let model = DomainTestClient {
        stall: true,
        ..Default::default()
    };
    jobs.start_with_model_client("overdue", model.clone())
        .await
        .unwrap();
    await_model(&model).await;
    let dispatched = jobs.get("overdue").unwrap();
    assert_eq!(dispatched.executions.len(), 1);
    let ownership = dispatched.executions[0].clone();
    assert_eq!(
        ownership.run_id,
        worker::job::execution_run_id(&intent, &reserved.attempt.attempt_id)
            .unwrap()
            .to_string()
    );
    assert!(!ownership.writes_revoked);
    assert!(ownership.cleanup_receipt.is_none());
    tokio::time::timeout(Duration::from_secs(5), hook.entered.notified())
        .await
        .unwrap();
    // Grace expiry is diagnostic only: a dropped wait must not detach the owner
    // or make the Journal dispatcher abandon the eventual settled result.
    assert!(
        tokio::time::timeout(Duration::from_secs(3), jobs.wait("overdue"))
            .await
            .is_err()
    );
    let mut completion = {
        let executions = jobs.executions.lock().unwrap();
        let execution = &executions.jobs["overdue"];
        assert!(!execution.task.is_finished());
        assert!(execution._owned.lock().unwrap().is_none());
        execution.complete.clone()
    };
    let failed = jobs.get("overdue").unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.attempt.failure.as_deref(), Some("timeout"));
    assert_eq!(failed.executions, vec![ownership.clone()]);
    assert!(jobs.reserve_quiescent_closure().is_err());
    assert!(jobs.retry("overdue").is_err());
    assert!(
        jobs.resume_granted("overdue", &failed.attempt.attempt_id)
            .is_err()
    );
    assert!(matches!(
        jobs.start_with_model_client("next", DomainTestClient::success())
            .await,
        Err(JobServiceError::Capacity)
    ));
    let mut conflicting = request("conflicting");
    conflicting.serialization_key = intent.serialization_key.clone();
    assert!(jobs.request(conflicting).is_err());
    hook.release.notify_one();
    tokio::time::timeout(
        Duration::from_secs(5),
        completion.wait_for(|state| matches!(state, Some(Ok(())))),
    )
    .await
    .unwrap()
    .unwrap();
    let cleaned = await_job(&jobs, "overdue").await;
    assert!(cleaned.executions_released());
    assert_eq!(cleaned.request, reserved.request);
    assert_eq!(cleaned.attempt, failed.attempt);
    assert_eq!(cleaned.executions[0].owner_id, ownership.owner_id);
    assert_eq!(cleaned.executions[0].run_id, ownership.run_id);
    assert_eq!(
        cleaned.executions[0].cleanup_receipt,
        Some(format!("owned-run:{}", ownership.run_id))
    );
    jobs.start_with_model_client("next", DomainTestClient::success())
        .await
        .unwrap();
    assert_eq!(await_job(&jobs, "next").await.state, JobState::Completed);
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn accepted_result_with_feature_shutdown_failure_retains_typed_owner_and_blocks_release() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    jobs.bind_domain(Arc::new(ControlledShutdown {
        fail: true,
        ..Default::default()
    }))
    .unwrap();
    jobs.request_granted(
        domain_request("cleanup-error"),
        domain_grant("cleanup-error"),
    )
    .unwrap();
    jobs.start_with_model_client("cleanup-error", DomainTestClient::success())
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), jobs.wait("cleanup-error"))
            .await
            .unwrap(),
        Err(JobServiceError::Cleanup(_))
    ));
    let accepted = jobs.get("cleanup-error").unwrap();
    assert_eq!(accepted.state, JobState::Completed);
    assert_eq!(accepted.result, Some(json!({"valid":true})));
    assert!(!accepted.executions_released());
    {
        let executions = jobs.executions.lock().unwrap();
        let owned = executions.jobs["cleanup-error"]._owned.lock().unwrap();
        let owned = owned.as_ref().unwrap().as_ref().unwrap();
        assert!(!owned.feature_shutdown_complete());
        assert!(!owned.feature_shutdown_failures().is_empty());
        assert_eq!(owned.run_id.to_string(), accepted.executions[0].run_id);
    }
    assert!(jobs.reserve_quiescent_closure().is_err());
    assert!(
        jobs.reclaim_input(
            "cleanup-error",
            &accepted.input_digest,
            JobInputRetirementReason::Completed
        )
        .is_err()
    );
    assert!(matches!(
        jobs.shutdown().await,
        Err(JobServiceError::Cleanup(_))
    ));
    assert!(jobs.store.lock().unwrap().is_some());
    assert_eq!(jobs.get("cleanup-error").unwrap(), accepted);
}

#[tokio::test]
async fn accepted_result_with_ordinary_worker_failure_confirms_independent_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    jobs.request(request("post-result-error")).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let model = DomainTestClient {
        failure_release: Some(release.clone()),
        ..DomainTestClient::success()
    };
    model.scripts.lock().unwrap().pop_back();
    release.notify_one();
    jobs.start_with_model_client("post-result-error", model)
        .await
        .unwrap();
    let mut completion = jobs.executions.lock().unwrap().jobs["post-result-error"]
        .complete
        .clone();
    completion.wait_for(Option::is_some).await.unwrap();
    {
        let executions = jobs.executions.lock().unwrap();
        let owned = executions.jobs["post-result-error"]._owned.lock().unwrap();
        let owned = owned.as_ref().unwrap().as_ref().unwrap();
        assert!(owned.feature_shutdown_complete());
        assert!(owned.worker_failure().is_some());
        assert!(!owned.requires_worker_cleanup_reconciliation());
        assert!(owned.result.is_ok());
    }
    let accepted = tokio::time::timeout(Duration::from_secs(5), jobs.wait("post-result-error"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.state, JobState::Completed);
    assert_eq!(accepted.result, Some(json!({"valid":true})));
    assert!(accepted.executions_released());
    assert_eq!(
        jobs.with_execution_diagnostic(
            "post-result-error",
            &accepted.attempt.attempt_id,
            |report| {
                assert_eq!(report.run_id.to_string(), accepted.executions[0].run_id);
                assert!(report.feature_shutdown_complete());
                assert!(!report.requires_worker_cleanup_reconciliation());
                assert!(report.result.is_ok());
                matches!(
                    report.worker_failure(),
                    Some(worker::WorkerError::Engine(_))
                )
            }
        ),
        Some(true),
        "typed ordinary failure survives execution-slot release"
    );
    assert!(
        jobs.with_execution_diagnostic("post-result-error", "foreign-attempt", |_| ())
            .is_none()
    );
    assert!(jobs.retry("post-result-error").is_err());
    assert!(jobs.reserve_quiescent_closure().is_ok());
    jobs.shutdown().await.unwrap();
    assert!(jobs.store.lock().unwrap().is_none());
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
    let mut intent = domain_request("fenced");
    intent.limits.timeout_seconds = 1;
    let reserved = jobs
        .request_granted(intent, domain_grant("fenced"))
        .unwrap();
    let first = DomainTestClient {
        stall: true,
        ..Default::default()
    };
    jobs.start_with_model_client("fenced", first.clone())
        .await
        .unwrap();
    await_model(&first).await;
    let old_fence = provider.0.lock().unwrap()[0].1.clone();
    old_fence.ensure_live().unwrap();
    let failed = await_job(&jobs, "fenced").await;
    assert_eq!(failed.state, JobState::Failed);
    assert!(failed.result.is_none());
    assert!(old_fence.ensure_live().is_err());
    assert!(failed.executions_released());
    assert!(
        jobs.retry("fenced").is_err(),
        "generic retry cannot bypass the domain"
    );
    jobs.reconcile_granted_attempt(
        "fenced",
        &failed.attempt.attempt_id,
        "test-domain-reconciled",
    )
    .unwrap();
    let retried = jobs
        .resume_granted("fenced", &failed.attempt.attempt_id)
        .unwrap();
    assert_eq!(retried.request, reserved.request);
    assert_eq!(retried.input_digest, reserved.input_digest);
    assert_eq!(retried.attempt.number, 2);
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
async fn host_restart_rejects_live_lease_then_fails_only_unconfirmed_dispatch_without_cleanup() {
    // Real Worker lease admission and Job SQLite; no Worker/process is spawned
    // for the interrupted rows. Releasing a lease is NOT execution drain proof.
    use crate::store::{StaleLeasePolicy, StandaloneStoreError, StandaloneWorkerStore};
    let temp = tempfile::tempdir().unwrap();
    let workers = StandaloneWorkerStore::open(&temp.path().join("state")).unwrap();
    let allocation = workers
        .allocate(temp.path(), StaleLeasePolicy::Reject)
        .unwrap();
    let worker = allocation.worker_id();
    let path = workers.jobs_path(worker);
    let lease = allocation.into_lease();
    let prior = StandaloneJobs::open_for_host(&path, temp.path().into(), &lease).unwrap();
    let reserved = prior.request(request("interrupted")).unwrap();
    let dispatched = prior
        .with_store_write(|store| store.dispatch("interrupted", &reserved.attempt.attempt_id, 8))
        .unwrap();
    let receipt = prior.request(request("accepted")).unwrap();
    prior
        .with_store_write(|store| store.dispatch("accepted", &receipt.attempt.attempt_id, 8))
        .unwrap();
    let accepted = prior
        .with_store_write(|store| store.accept(&result("accepted", &receipt.attempt.attempt_id)))
        .unwrap();
    let unstarted = prior.request(request("unstarted")).unwrap();
    assert!(matches!(
        workers.acquire_lease(worker, StaleLeasePolicy::Recover),
        Err(StandaloneStoreError::WorkerLeased(_))
    ));
    assert!(
        StandaloneJobs::open_for_host(&path, temp.path().into(), &lease).is_err(),
        "same live lease/empty local map is not a restart"
    );
    assert_eq!(prior.get("interrupted").unwrap(), dispatched);
    let old_sink = sink(&prior, "interrupted", &reserved.attempt.attempt_id);
    lease.release().unwrap();
    let replacement_lease = workers
        .acquire_lease(worker, StaleLeasePolicy::Recover)
        .unwrap();
    let reopened =
        StandaloneJobs::open_for_host(&path, temp.path().into(), &replacement_lease).unwrap();
    let failed = reopened.get("interrupted").unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.attempt.state, JobAttemptState::Failed);
    assert_eq!(failed.attempt.attempt_id, reserved.attempt.attempt_id);
    assert!(
        failed
            .attempt
            .failure
            .as_deref()
            .unwrap()
            .starts_with("host_restart_execution_unconfirmed:")
    );
    assert!(!failed.version_stopped);
    assert_eq!(failed.request, reserved.request);
    assert_eq!(failed.executions, dispatched.executions);
    assert!(!failed.executions_released());
    assert!(
        old_sink
            .submit(result("interrupted", &reserved.attempt.attempt_id))
            .is_err()
    );
    assert!(reopened.retry("interrupted").is_err());
    assert_eq!(reopened.get("accepted").unwrap(), accepted);
    assert_eq!(reopened.get("unstarted").unwrap(), unstarted);
    reopened.bind_model_client(Box::new(DomainTestClient::success()));
    tokio::time::timeout(
        Duration::from_secs(5),
        reopened.resume_reserved_result_jobs(reopened.reserved_result_attempts().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let resumed = reopened.get("unstarted").unwrap();
    assert_eq!(resumed.state, JobState::Completed);
    assert_eq!(resumed.attempt.attempt_id, unstarted.attempt.attempt_id);
    assert_eq!(resumed.attempt.number, 1);
    assert!(!resumed.version_stopped);
    assert_eq!(reopened.get("interrupted").unwrap(), failed);
    assert!(matches!(
        reopened.try_shutdown(),
        Err(JobServiceError::ShutdownIncomplete)
    ));
    // The fixture never had a real prior runner to join; do not forge its receipt.
}

struct UnconfirmedSink {
    bound: BoundResultSink,
    commit_before_error: bool,
}
impl JobResultSink for UnconfirmedSink {
    fn submit(&self, submission: JobResultSubmission) -> Result<(), String> {
        if self.commit_before_error {
            self.bound.submit(submission)?;
        }
        Err("injected result persistence/acknowledgement outage".into())
    }
}

#[tokio::test]
async fn sealed_non_journal_result_recovers_store_or_ack_loss_without_running_model_again() {
    // Real owned Worker, result Feature and temporary SQLite store; only the
    // model and sink acknowledgement boundary are substituted. No process death.
    for committed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let jobs =
            StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
        let intent = request("receipt");
        let reserved = jobs.request(intent.clone()).unwrap();
        let dispatched = jobs
            .with_store(|store| store.dispatch("receipt", &reserved.attempt.attempt_id, 8))
            .unwrap();
        let model = DomainTestClient::success();
        let prepared =
            worker::job::prepare_job(&intent, temp.path(), Some(Box::new(model.clone()))).unwrap();
        let owned = tokio::time::timeout(
            Duration::from_secs(5),
            prepared.run_owned(
                reserved.attempt.attempt_id.clone(),
                Arc::new(UnconfirmedSink {
                    bound: sink(&jobs, "receipt", &reserved.attempt.attempt_id),
                    commit_before_error: committed,
                }),
                |_| {},
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            owned.result,
            Err(worker::job::JobExecutionError::ResultUnconfirmed(_))
        ));
        assert!(owned.feature_shutdown_complete());
        assert_eq!(
            owned.unconfirmed_submission(),
            Some(&result("receipt", &reserved.attempt.attempt_id))
        );
        assert_eq!(
            jobs.get("receipt").unwrap().state,
            if committed {
                JobState::Completed
            } else {
                JobState::Pending
            }
        );
        let model_calls = model.requests.lock().unwrap().len();
        let owned = Ok(owned);
        recover_settlement(|| {
            settle_owned_execution(
                &jobs.store,
                "receipt",
                &reserved.attempt.attempt_id,
                &reserved.input_digest,
                &dispatched.executions[0],
                true,
                None,
                true,
                &owned,
            )
        })
        .await
        .unwrap();
        let saved = jobs.get("receipt").unwrap();
        assert_eq!(saved.state, JobState::Completed);
        assert_eq!(saved.result, Some(json!({"valid":true})));
        assert_eq!(saved.attempt.attempt_id, reserved.attempt.attempt_id);
        assert_eq!(saved.attempt.number, 1);
        assert!(saved.executions_released());
        assert_eq!(model.requests.lock().unwrap().len(), model_calls);
        jobs.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn stale_settlement_is_not_retried_and_cannot_fail_a_later_attempt() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    let reserved = jobs.request(request("stale")).unwrap();
    let dispatched = jobs
        .with_store(|store| store.dispatch("stale", &reserved.attempt.attempt_id, 8))
        .unwrap();
    let owner = dispatched.executions[0].clone();
    jobs.with_store(|store| {
        store.finish(
            "stale",
            &reserved.attempt.attempt_id,
            JobAttemptState::Failed,
            Some("first failure"),
        )?;
        store.confirm_execution_cleanup("stale", &owner, "fixture-owned-run-joined")
    })
    .unwrap();
    let next = jobs.retry("stale").unwrap();
    let mut calls = 0;
    let outcome = recover_settlement(|| {
        calls += 1;
        settle_owned_execution(
            &jobs.store,
            "stale",
            &reserved.attempt.attempt_id,
            &reserved.input_digest,
            &owner,
            true,
            None,
            false,
            &Err(worker::job::JobExecutionError::MissingResult),
        )
    })
    .await;
    assert!(matches!(
        outcome,
        Err(JobServiceError::Store(JobStoreError::AttemptMismatch))
    ));
    assert_eq!(calls, 1);
    assert_eq!(jobs.get("stale").unwrap(), next);
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_revocation_fences_callbacks_and_results_even_when_terminal_write_is_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    let reserved = jobs.request(request("revoked")).unwrap();
    let dispatched = jobs
        .with_store(|store| store.dispatch("revoked", &reserved.attempt.attempt_id, 8))
        .unwrap();
    let revoked = Arc::new(AtomicBool::new(false));
    let fence = BoundAttemptFence {
        revoked: revoked.clone(),
        effect_gate: Arc::new(Mutex::new(())),
        store: jobs.store.clone(),
        job_id: "revoked".into(),
        attempt_id: reserved.attempt.attempt_id.clone(),
        input_digest: reserved.input_digest.clone(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
    };
    let mut sink = sink(&jobs, "revoked", &reserved.attempt.attempt_id);
    sink.revoked = revoked.clone();
    assert!(fence.enter().is_ok());
    // Fault boundary: the cancellation's terminal write has not committed.
    // No DB terminal state or elapsed deadline is available as revocation proof.
    revoked.store(true, Ordering::Release);
    assert!(fence.enter().is_err());
    assert!(
        sink.submit(result("revoked", &reserved.attempt.attempt_id))
            .is_err()
    );
    assert_eq!(jobs.get("revoked").unwrap(), dispatched);
    assert!(!dispatched.executions_released());
    assert!(matches!(
        jobs.shutdown().await,
        Err(JobServiceError::ShutdownIncomplete)
    ));
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
    assert!(matches!(
        jobs.shutdown().await,
        Err(JobServiceError::ShutdownIncomplete)
    ));
    assert!(jobs.store.lock().unwrap().is_some());
    assert!(!jobs.get("first").unwrap().executions_released());
    assert!(!jobs.get("second").unwrap().executions_released());
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
        JobAttemptState::Failed,
        "cleanup",
    )
    .unwrap();
    assert_eq!(jobs.get("cancelled").unwrap().state, JobState::Cancelled);
    let accepted = jobs.request(request("accepted")).unwrap();
    jobs.start_with_model_client("accepted", DomainTestClient::success())
        .await
        .unwrap();
    let completed = await_job(&jobs, "accepted").await;
    assert!(completed.executions_released());
    let bound = sink(&jobs, "accepted", &accepted.attempt.attempt_id);
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
    // Repeated close uses the retained proof of this service's real shutdown,
    // not an absent store/map as a newly inferred completion.
    jobs.try_shutdown().unwrap();
    jobs.shutdown().await.unwrap();
    assert!(
        bound
            .submit(result("accepted", &accepted.attempt.attempt_id))
            .is_err()
    );
    for operation in [
        jobs.cancel("cancelled"),
        jobs.acknowledge("accepted"),
        jobs.retry("cancelled"),
        jobs.request(request("new")),
    ] {
        assert!(matches!(operation, Err(JobServiceError::Closed)));
    }
}

#[test]
fn absent_store_and_empty_execution_map_do_not_create_shutdown_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = StandaloneJobs::open(&temp.path().join("jobs.sqlite3"), temp.path().into()).unwrap();
    jobs.close();
    jobs.store.lock().unwrap().take();
    assert!(matches!(jobs.try_shutdown(), Err(JobServiceError::Closed)));
}

#[tokio::test]
async fn cancelled_shutdown_waiter_keeps_execution_barrier_and_failed_input_on_reopen() {
    // Actual generic runner + SQLite; only the provider stream is scripted/blocked.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let jobs = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    let reserved = jobs.request(request("cancel-shutdown-wait")).unwrap();
    let model = DomainTestClient {
        stall: true,
        ..Default::default()
    };
    jobs.start_with_model_client(&reserved.request.job_id, model.clone())
        .await
        .unwrap();
    await_model(&model).await;
    {
        let shutdown = jobs.shutdown();
        tokio::pin!(shutdown);
        assert!(futures::poll!(shutdown.as_mut()).is_pending());
        // Drop the caller's waiter while cancellation/cleanup is still pending.
    }
    assert!(matches!(
        jobs.try_shutdown(),
        Err(JobServiceError::ShutdownIncomplete)
    ));
    assert!(jobs.store.lock().unwrap().is_some());
    assert!(matches!(
        jobs.request(request("after-shutdown")),
        Err(JobServiceError::Closed)
    ));
    tokio::time::timeout(Duration::from_secs(5), jobs.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(jobs.store.lock().unwrap().is_none());
    let reopened = StandaloneJobs::open(&path, temp.path().into()).unwrap();
    let stopped = reopened.get(&reserved.request.job_id).unwrap();
    assert_eq!(stopped.state, JobState::Failed);
    assert_eq!(stopped.attempt.state, JobAttemptState::Failed);
    assert_eq!(
        stopped.attempt.failure.as_deref(),
        Some("Host cancellation/shutdown")
    );
    assert_eq!(stopped.input_disposition, JobInputDisposition::Available);
    assert_eq!(stopped.request, reserved.request);
    assert_eq!(stopped.input_digest, reserved.input_digest);
    assert!(stopped.executions_released());
    assert!(!stopped.version_stopped);
    let retry = reopened.retry(&stopped.request.job_id).unwrap();
    assert_eq!(retry.attempt.number, 2);
    assert_eq!(retry.request, reserved.request);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn input_retirement_service_fences_live_attempt_and_preserves_session_receipts() {
    // Production service + SQLite + attempt/result fences, without a model or Worker fixture.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("jobs.sqlite3");
    let jobs = StandaloneJobs::open(&path, temp.path().to_path_buf()).unwrap();
    let original = request("input-retirement");
    let grant = json!({"session_id":"session-a"});
    let reserved = jobs
        .request_granted(original.clone(), grant.clone())
        .unwrap();
    jobs.with_store(|store| store.dispatch("input-retirement", &reserved.attempt.attempt_id, 8))
        .unwrap();
    let fence = BoundAttemptFence {
        revoked: Arc::new(AtomicBool::new(false)),
        effect_gate: Arc::new(Mutex::new(())),
        store: jobs.store.clone(),
        job_id: "input-retirement".into(),
        attempt_id: reserved.attempt.attempt_id.clone(),
        input_digest: reserved.input_digest.clone(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
    };
    assert!(fence.ensure_live().is_ok());
    assert!(
        jobs.reclaim_input(
            "input-retirement",
            &reserved.input_digest,
            JobInputRetirementReason::Completed
        )
        .is_err()
    );
    let erased = jobs
        .reclaim_input(
            "input-retirement",
            &reserved.input_digest,
            JobInputRetirementReason::ExplicitErasure,
        )
        .unwrap();
    assert!(fence.ensure_live().is_err());
    assert!(
        sink(&jobs, "input-retirement", &reserved.attempt.attempt_id)
            .submit(result("input-retirement", &reserved.attempt.attempt_id))
            .is_err()
    );
    assert!(jobs.start("input-retirement").await.is_err());
    assert_eq!(
        jobs.list_session_snapshots("session-a", None, 100).unwrap(),
        vec![erased.clone()]
    );
    assert!(
        jobs.list_session_snapshots("session-b", None, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(jobs.request_granted(original, grant).unwrap(), erased);
    assert!(matches!(
        jobs.shutdown().await,
        Err(JobServiceError::ShutdownIncomplete)
    ));
    assert!(jobs.store.lock().unwrap().is_some());
    assert!(!jobs.get("input-retirement").unwrap().executions_released());
}
