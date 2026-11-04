use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::{
    LlmClient,
    error::ClientError,
    event::{Event as LlmEvent, StopReason},
    types::Request,
};
use async_trait::async_trait;
use futures::{Stream, stream};
use job::{JobLimits, JobRequest};
use standalone::jobs::{JobServiceError, JobState};
use standalone::{StandaloneHost, StandaloneLaunchConfig};

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<VecDeque<Vec<LlmEvent>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    started: Arc<tokio::sync::Notify>,
    stall: bool,
    fail: bool,
}
impl ScriptedClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            started: Arc::new(tokio::sync::Notify::new()),
            stall: false,
            fail: false,
        }
    }
    fn stall() -> Self {
        Self {
            stall: true,
            ..Self::new(Vec::new())
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
#[async_trait]
impl LlmClient for ScriptedClient {
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.requests.lock().unwrap().push(request);
        self.started.notify_one();
        if self.fail {
            return Err(ClientError::Config("scripted model failure".into()));
        }
        if self.stall {
            return Ok(Box::pin(stream::pending()));
        }
        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted response");
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}
fn prose() -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "done"),
        LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}
fn success() -> ScriptedClient {
    ScriptedClient::new(vec![
        vec![
            LlmEvent::tool_use_start(0, "result-1", "SubmitJobResult"),
            LlmEvent::tool_input_delta(0, r#"{"result":{"valid":true}}"#),
            LlmEvent::tool_use_stop(0),
        ],
        prose(),
    ])
}
fn request(id: &str) -> JobRequest {
    JobRequest {
        job_id: id.into(),
        purpose: "generic_test".into(),
        input_ref: "test:immutable".into(),
        input: serde_json::json!({"test_input":42}),
        instruction: "Return a structured validity result".into(),
        profile: "builtin:job".into(),
        serialization_key: None,
        limits: JobLimits::default(),
    }
}
async fn host(temp: &tempfile::TempDir) -> (StandaloneHost, ScriptedClient) {
    let launch = StandaloneLaunchConfig::new(
        temp.path(),
        temp.path().join("state"),
        manifest::ProfileSelector::Default,
        "job-host",
    )
    .resolve()
    .unwrap();
    let client = ScriptedClient::new(Vec::new());
    let host = StandaloneHost::start_with_model_client(launch, client.clone())
        .await
        .unwrap();
    (host, client)
}
async fn wait_started(client: &ScriptedClient) {
    tokio::time::timeout(Duration::from_secs(10), client.started.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn host_job_runs_without_dialog_and_result_survives_restore_and_ack() {
    let temp = tempfile::tempdir().unwrap();
    let (host, dialog) = host(&temp).await;
    let id = host.worker_id();
    let jobs = host.jobs();
    let intent = request("success");
    let reserved = jobs.request(intent.clone()).unwrap();
    assert_eq!(jobs.request(intent.clone()).unwrap(), reserved);
    let model = success();
    jobs.start_with_model_client("success", model.clone())
        .await
        .unwrap();
    let completed = jobs.wait("success").await.unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.result, Some(serde_json::json!({"valid":true})));
    assert!(!completed.acknowledged);
    assert!(completed.executions_released());
    assert_eq!(completed.executions.len(), 1);
    assert_eq!(
        completed.executions[0].run_id,
        worker::job::execution_run_id(&intent, &completed.attempt.attempt_id)
            .unwrap()
            .to_string()
    );
    assert_eq!(
        completed.executions[0].cleanup_receipt.as_deref(),
        Some(format!("owned-run:{}", completed.executions[0].run_id).as_str())
    );
    let neutral = completed.outcome();
    assert_eq!(neutral.state, job::JobState::Completed);
    assert_eq!(neutral.result, completed.result);
    assert_eq!(neutral.attempt_state, job::JobAttemptState::Completed);
    assert!(
        dialog.requests().is_empty(),
        "Job never runs on the interactive model/session"
    );
    assert_eq!(
        model.requests()[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["SubmitJobResult"]
    );
    assert_eq!(jobs.request(intent.clone()).unwrap(), completed);
    assert!(
        jobs.start_with_model_client("success", success())
            .await
            .is_err()
    );
    assert!(jobs.retry("success").is_err());
    let db = temp
        .path()
        .join("state")
        .join(id.to_string())
        .join("jobs.sqlite3");
    assert!(db.exists());
    host.shutdown().await.unwrap();
    assert!(matches!(
        jobs.request(request("after-shutdown")),
        Err(JobServiceError::Closed)
    ));
    assert!(matches!(
        jobs.cancel("success"),
        Err(JobServiceError::Closed)
    ));
    assert!(matches!(
        jobs.acknowledge("success"),
        Err(JobServiceError::Closed)
    ));
    assert!(matches!(
        jobs.start_with_model_client("success", success()).await,
        Err(JobServiceError::Closed)
    ));
    let restored = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        id,
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    let restored_jobs = restored.jobs();
    assert_eq!(restored_jobs.get("success").unwrap(), completed);
    assert!(restored_jobs.acknowledge("success").unwrap().acknowledged);
    restored.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_restore_resumes_reserved_result_job_on_same_attempt_without_client_action() {
    let temp = tempfile::tempdir().unwrap();
    let (host, dialog) = host(&temp).await;
    let worker_id = host.worker_id();
    let saved = host
        .jobs()
        .request(request("reserved-before-dispatch"))
        .unwrap();
    host.shutdown().await.unwrap();
    assert!(dialog.requests().is_empty());

    let model = success();
    let restored = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        worker_id,
        model.clone(),
    )
    .await
    .unwrap();
    // The production from_started dispatcher, not a test start/retry/probe call,
    // must supply the already-accepted immutable input to the result-only Job.
    wait_started(&model).await;
    let completed = tokio::time::timeout(
        Duration::from_secs(10),
        restored.jobs().wait("reserved-before-dispatch"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.attempt.attempt_id, saved.attempt.attempt_id);
    assert_eq!(completed.input_digest, saved.input_digest);
    assert_eq!(completed.request, saved.request);
    assert_eq!(completed.executions.len(), 1);
    assert!(completed.executions_released());
    assert_eq!(model.requests().len(), 2);
    restored.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_resource_limits_cancellation_and_wait_are_host_owned() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    let mut first = request("first");
    first.limits.max_concurrent_jobs = 1;
    jobs.request(first).unwrap();
    jobs.request(request("second")).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("first", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    // Waiting must not remove the Host-owned task/cancellation capability.
    let waiting_jobs = jobs.clone();
    let waiter = tokio::spawn(async move { waiting_jobs.wait("first").await });
    assert!(matches!(
        jobs.start_with_model_client("second", success()).await,
        Err(JobServiceError::Capacity)
    ));
    jobs.cancel("first").unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(jobs.retry("first").is_err());
    jobs.start_with_model_client("second", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("second").await.unwrap().state,
        JobState::Completed
    );
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn prose_failure_requires_explicit_bounded_new_attempt_and_exact_replay_does_not_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    let intent = request("prose");
    jobs.request(intent.clone()).unwrap();
    jobs.start_with_model_client("prose", ScriptedClient::new(vec![prose()]))
        .await
        .unwrap();
    let failed = jobs.wait("prose").await.unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert!(failed.result.is_none());
    assert_eq!(jobs.request(intent).unwrap(), failed);
    let retry = jobs.retry("prose").unwrap();
    assert_eq!(retry.attempt.number, 2);
    assert_ne!(retry.attempt.attempt_id, failed.attempt.attempt_id);
    jobs.start_with_model_client("prose", ScriptedClient::new(vec![prose()]))
        .await
        .unwrap();
    assert_eq!(jobs.wait("prose").await.unwrap().state, JobState::Failed);
    assert!(jobs.retry("prose").is_err());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeout_fails_without_automatic_retry_and_cleanup_allows_explicit_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    let mut intent = request("timeout");
    intent.limits.timeout_seconds = 1;
    let reserved = jobs.request(intent.clone()).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("timeout", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), jobs.wait("timeout"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.state, JobState::Failed);
    assert_eq!(
        outcome.attempt.state,
        standalone::jobs::JobAttemptState::Failed
    );
    assert_eq!(outcome.attempt.failure.as_deref(), Some("timeout"));
    assert_eq!(outcome.attempt.attempt_id, reserved.attempt.attempt_id);
    assert_eq!(outcome.request, intent);
    assert!(outcome.result.is_none());
    assert!(outcome.executions_released());
    assert_eq!(jobs.request(intent).unwrap(), outcome);
    assert_eq!(
        stalled.requests().len(),
        1,
        "timeout never reexecutes the model"
    );
    let retried = jobs.retry("timeout").unwrap();
    assert_eq!(retried.attempt.number, 2);
    assert_ne!(retried.attempt.attempt_id, outcome.attempt.attempt_id);
    jobs.start_with_model_client("timeout", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("timeout").await.unwrap().state,
        JobState::Completed
    );
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_preserves_reserved_intent_and_reopen_preserves_confirmed_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let id = host.worker_id();
    let jobs = host.jobs();
    let reserved = jobs.request(request("reserved")).unwrap();
    jobs.request(request("cancelled")).unwrap();
    let cancelled = jobs.cancel("cancelled").unwrap();
    let running = jobs.request(request("running")).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("running", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    host.shutdown().await.unwrap();
    assert!(matches!(jobs.get("running"), Err(JobServiceError::Closed)));
    let resume_model = success();
    let restored = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        id,
        resume_model.clone(),
    )
    .await
    .unwrap();
    wait_started(&resume_model).await;
    let jobs = restored.jobs();
    let resumed = jobs.wait("reserved").await.unwrap();
    assert_eq!(resumed.state, JobState::Completed);
    assert_eq!(resumed.attempt.attempt_id, reserved.attempt.attempt_id);
    assert_eq!(resumed.input_digest, reserved.input_digest);
    assert_eq!(resumed.request, reserved.request);
    assert!(jobs.pending().unwrap().is_empty());
    assert_eq!(jobs.get("cancelled").unwrap(), cancelled);
    let stopped = jobs.get("running").unwrap();
    assert_eq!(stopped.state, JobState::Failed);
    assert_eq!(
        stopped.attempt.state,
        standalone::jobs::JobAttemptState::Failed
    );
    assert_eq!(
        stopped.attempt.failure.as_deref(),
        Some("Host cancellation/shutdown")
    );
    assert_eq!(stopped.request, running.request);
    assert_eq!(stopped.attempt.attempt_id, running.attempt.attempt_id);
    assert!(stopped.executions_released());
    assert!(
        !stopped.version_stopped,
        "ordinary shutdown is not a version cutover"
    );
    let retry = jobs.retry("running").unwrap();
    assert_eq!(retry.attempt.number, 2);
    jobs.start_with_model_client("running", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("running").await.unwrap().state,
        JobState::Completed
    );
    assert_eq!(jobs.get("reserved").unwrap(), resumed);
    assert_eq!(resume_model.requests().len(), 2);
    restored.shutdown().await.unwrap();
}

#[tokio::test]
async fn profile_requirements_are_rejected_without_model_execution_or_implicit_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    for profile in [
        "builtin:coder",
        "builtin:backend-job",
        "builtin:does-not-exist",
    ] {
        let mut intent = request(profile);
        intent.profile = profile.into();
        jobs.request(intent).unwrap();
        let model = success();
        assert!(matches!(
            jobs.start_with_model_client(profile, model.clone()).await,
            Err(JobServiceError::Configuration(_))
        ));
        assert!(model.requests().is_empty());
        assert_eq!(jobs.get(profile).unwrap().state, JobState::Pending);
    }
    host.shutdown().await.unwrap();
}

// Only this test harness starts a subprocess: production Jobs remain in-process.
// The child exits deliberately without destructors to prove actual process-loss
// recovery, rather than treating a hand-edited SQLite fixture as that evidence.
#[tokio::test]
#[ignore = "entry point for process_loss_fails_same_attempt_without_claiming_execution_cleanup"]
async fn process_job_fixture() {
    let root = std::path::PathBuf::from(
        std::env::var_os("YOI_JOB_PROCESS_FIXTURE_DIR").expect("fixture root"),
    );
    let launch = StandaloneLaunchConfig::new(
        &root,
        root.join("state"),
        manifest::ProfileSelector::Default,
        "job-process-fixture",
    )
    .resolve()
    .unwrap();
    let host = StandaloneHost::start_with_model_client(launch, ScriptedClient::new(Vec::new()))
        .await
        .unwrap();
    std::fs::write(root.join("worker-id"), host.worker_id().to_string()).unwrap();
    let jobs = host.jobs();
    jobs.request(request("reserved")).unwrap();
    jobs.request(request("cancelled")).unwrap();
    jobs.cancel("cancelled").unwrap();
    jobs.request(request("accepted")).unwrap();
    jobs.start_with_model_client("accepted", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("accepted").await.unwrap().state,
        JobState::Completed
    );
    jobs.request(request("interrupted")).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("interrupted", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    std::fs::write(
        root.join("interrupted-snapshot.json"),
        serde_json::to_vec(&jobs.get("interrupted").unwrap()).unwrap(),
    )
    .unwrap();
    // Test-owned process only, never the operating Yoi Worker or dogfood daemon.
    std::process::exit(23);
}

#[tokio::test]
async fn process_loss_fails_same_attempt_without_claiming_execution_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_job_fixture", "--ignored", "--nocapture"])
        .env("YOI_JOB_PROCESS_FIXTURE_DIR", temp.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "fixture failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let worker_id = std::fs::read_to_string(temp.path().join("worker-id"))
        .unwrap()
        .parse()
        .unwrap();
    let recovery_model = success();
    let host = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        worker_id,
        recovery_model.clone(),
    )
    .await
    .unwrap();
    let jobs = host.jobs();
    let persisted: standalone::jobs::JobSnapshot = serde_json::from_slice(
        &std::fs::read(temp.path().join("interrupted-snapshot.json")).unwrap(),
    )
    .unwrap();
    let interrupted = jobs.get("interrupted").unwrap();
    assert_eq!(interrupted.request, persisted.request);
    assert_eq!(interrupted.input_digest, persisted.input_digest);
    assert_eq!(interrupted.attempt.attempt_id, persisted.attempt.attempt_id);
    assert_eq!(interrupted.attempt.number, persisted.attempt.number);
    assert_eq!(interrupted.executions, persisted.executions);
    assert_eq!(interrupted.result, persisted.result);
    assert_eq!(interrupted.state, JobState::Failed);
    assert_eq!(
        interrupted.attempt.state,
        standalone::jobs::JobAttemptState::Failed
    );
    assert!(
        interrupted
            .attempt
            .failure
            .as_deref()
            .unwrap()
            .starts_with("host_restart_execution_unconfirmed:")
    );
    assert_eq!(interrupted.executions.len(), 1);
    assert!(!interrupted.executions_released());
    assert!(interrupted.executions[0].cleanup_receipt.is_none());
    assert!(!interrupted.executions[0].writes_revoked);
    assert!(!interrupted.version_stopped);
    assert!(jobs.retry("interrupted").is_err());
    let replay = success();
    assert!(
        jobs.start_with_model_client("interrupted", replay.clone())
            .await
            .is_err()
    );
    assert!(replay.requests().is_empty());
    assert_eq!(jobs.wait("interrupted").await.unwrap(), interrupted);
    let completed = jobs.get("accepted").unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.result, Some(serde_json::json!({"valid":true})));
    assert!(!completed.acknowledged);
    assert_eq!(jobs.get("cancelled").unwrap().state, JobState::Cancelled);
    wait_started(&recovery_model).await;
    let reserved = jobs.wait("reserved").await.unwrap();
    assert_eq!(reserved.state, JobState::Completed);
    assert_eq!(reserved.attempt.number, 1);
    assert_eq!(recovery_model.requests().len(), 2);
    assert!(jobs.pending().unwrap().is_empty());
    assert_eq!(jobs.get("interrupted").unwrap(), interrupted);
    assert_eq!(
        host.shutdown().await,
        Err(standalone::StandaloneShutdownError::JobCleanup)
    );
}

#[tokio::test]
async fn ordinary_model_failure_preserves_cause_and_confirms_independent_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    jobs.request(request("model-failure")).unwrap();
    let client = ScriptedClient {
        fail: true,
        ..ScriptedClient::new(Vec::new())
    };
    jobs.start_with_model_client("model-failure", client.clone())
        .await
        .unwrap();
    let failure = tokio::time::timeout(Duration::from_secs(10), jobs.wait("model-failure"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(jobs.get("model-failure").unwrap(), failure);
    assert_eq!(failure.state, JobState::Failed);
    assert!(failure.result.is_none());
    assert!(
        failure
            .attempt
            .failure
            .as_deref()
            .unwrap()
            .contains("scripted model failure")
    );
    assert!(failure.executions_released());
    assert_eq!(failure.executions.len(), 1);
    let execution = &failure.executions[0];
    assert!(execution.writes_revoked);
    assert_eq!(
        execution.cleanup_receipt.as_deref(),
        Some(format!("owned-run:{}", execution.run_id).as_str())
    );
    assert_eq!(client.requests().len(), 1, "no automatic model retry");
    assert_eq!(
        jobs.with_execution_diagnostic("model-failure", &failure.attempt.attempt_id, |report| {
            assert_eq!(report.run_id.to_string(), execution.run_id);
            assert!(report.feature_shutdown_complete());
            assert!(report.worker_failure().is_some());
            assert!(report.result.is_err());
            !report.requires_worker_cleanup_reconciliation()
        }),
        Some(true)
    );
    assert_eq!(jobs.request(request("model-failure")).unwrap(), failure);
    let retried = jobs.retry("model-failure").unwrap();
    assert_eq!(retried.attempt.number, 2);
    assert_ne!(retried.attempt.attempt_id, failure.attempt.attempt_id);
    assert_eq!(retried.executions, failure.executions);
    let recovery = success();
    jobs.start_with_model_client("model-failure", recovery.clone())
        .await
        .unwrap();
    let completed = jobs.wait("model-failure").await.unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.executions.len(), 2);
    assert_eq!(completed.executions[0], failure.executions[0]);
    assert!(completed.executions_released());
    assert_eq!(recovery.requests().len(), 2);
    assert_eq!(client.requests().len(), 1);
    host.shutdown().await.unwrap();
}
