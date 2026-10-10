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
async fn timeout_is_unknown_not_prose_success_or_automatic_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    let mut intent = request("timeout");
    intent.limits.timeout_seconds = 1;
    jobs.request(intent).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("timeout", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), jobs.wait("timeout"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.state, JobState::Unknown);
    assert!(outcome.result.is_none());
    assert!(jobs.retry("timeout").is_err());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_keeps_reserved_results_and_fences_running_attempts_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let id = host.worker_id();
    let jobs = host.jobs();
    jobs.request(request("reserved")).unwrap();
    jobs.request(request("cancelled")).unwrap();
    jobs.cancel("cancelled").unwrap();
    jobs.request(request("running")).unwrap();
    let stalled = ScriptedClient::stall();
    jobs.start_with_model_client("running", stalled.clone())
        .await
        .unwrap();
    wait_started(&stalled).await;
    host.shutdown().await.unwrap();
    assert!(matches!(jobs.get("running"), Err(JobServiceError::Closed)));
    let restored = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        id,
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    let jobs = restored.jobs();
    assert_eq!(
        jobs.pending()
            .unwrap()
            .iter()
            .map(|snapshot| snapshot.request.job_id.as_str())
            .collect::<Vec<_>>(),
        ["reserved"]
    );
    assert_eq!(jobs.get("cancelled").unwrap().state, JobState::Cancelled);
    assert_eq!(jobs.get("running").unwrap().state, JobState::Unknown);
    assert!(jobs.retry("running").is_err());
    jobs.start_with_model_client("reserved", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("reserved").await.unwrap().state,
        JobState::Completed
    );
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
#[ignore = "entry point for process_loss_recovers_durable_jobs_without_replaying_unknown"]
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
    // Test-owned process only, never the operating Yoi Worker or dogfood daemon.
    std::process::exit(23);
}

#[tokio::test]
async fn process_loss_recovers_durable_jobs_without_replaying_unknown() {
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
    let host = StandaloneHost::restore_with_model_client(
        temp.path().join("state"),
        worker_id,
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    let jobs = host.jobs();
    assert_eq!(jobs.get("interrupted").unwrap().state, JobState::Unknown);
    assert!(jobs.retry("interrupted").is_err());
    let completed = jobs.get("accepted").unwrap();
    assert_eq!(completed.state, JobState::Completed);
    assert_eq!(completed.result, Some(serde_json::json!({"valid":true})));
    assert!(!completed.acknowledged);
    assert_eq!(jobs.get("cancelled").unwrap().state, JobState::Cancelled);
    assert_eq!(jobs.pending().unwrap().len(), 1);
    jobs.start_with_model_client("reserved", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("reserved").await.unwrap().state,
        JobState::Completed
    );
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn model_failure_is_durable_known_failure_with_explicit_successful_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (host, _) = host(&temp).await;
    let jobs = host.jobs();
    jobs.request(request("model-failure")).unwrap();
    let client = ScriptedClient {
        fail: true,
        ..ScriptedClient::new(Vec::new())
    };
    jobs.start_with_model_client("model-failure", client)
        .await
        .unwrap();
    let failure = jobs.wait("model-failure").await.unwrap();
    assert_eq!(failure.state, JobState::Failed);
    assert!(failure.result.is_none());
    assert!(
        failure
            .attempt
            .failure
            .unwrap()
            .contains("scripted model failure")
    );
    let next = jobs.retry("model-failure").unwrap();
    assert_eq!(next.attempt.number, 2);
    jobs.start_with_model_client("model-failure", success())
        .await
        .unwrap();
    assert_eq!(
        jobs.wait("model-failure").await.unwrap().state,
        JobState::Completed
    );
    host.shutdown().await.unwrap();
}
