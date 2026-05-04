//! End-to-end coverage for the prune-projection metrics path.
//!
//! Drives a Pod with a scripted mock LLM client and a custom tool that
//! returns a long `ToolOutput.content`, then inspects the persisted
//! session log to verify:
//!
//! - `prune.skip { reason: "no_candidates" }` lands when the protected-turn
//!   window covers the entire history.
//! - `prune.fire` lands once enough turns + usage measurements exist for
//!   the projection to actually apply.
//! - The fire metric and the immediately-following `prune.post_request`
//!   metric share the same `correlation_id`, so cache_read / cache_write
//!   from the LlmUsage that triggered the projection can be joined back
//!   to the originating event.
//! - `prune.skip { reason: "below_min_savings" }` lands when candidates
//!   exist but their estimated savings are below the configured floor.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::Stream;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent, UsageEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use session_metrics::{DOMAIN, Metric, metrics_from_extensions};
use session_store::{
    EntryHash, FsStore, HashedEntry, LogEntry, SessionId, Store, StoreError, TraceEntry,
};

use pod::{Pod, PodManifest};

#[derive(Clone)]
struct MockClient {
    responses: Arc<Vec<Vec<LlmEvent>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for MockClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Config("mock client exhausted".into()));
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

/// Tool that returns a fixed `ToolOutput { summary, content: Some(big) }`.
/// `content` is long enough for prune savings to comfortably clear small
/// `min_savings` thresholds.
struct BigContentTool {
    summary: &'static str,
    content: String,
}

#[async_trait]
impl Tool for BigContentTool {
    async fn execute(&self, _input: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            summary: self.summary.into(),
            content: Some(self.content.clone()),
        })
    }
}

fn big_content_tool_definition(name: &'static str) -> ToolDefinition {
    Arc::new(move || {
        let summary = "tool result summary";
        let content = "x".repeat(2048);
        (
            ToolMeta::new(name)
                .description("test tool that returns a long content")
                .input_schema(serde_json::json!({"type": "object"})),
            Arc::new(BigContentTool { summary, content }) as Arc<dyn Tool>,
        )
    })
}

fn usage_event(input_total: u64, cache_read: u64, cache_write: u64, output: u64) -> LlmEvent {
    LlmEvent::Usage(UsageEvent {
        input_tokens: Some(input_total),
        output_tokens: Some(output),
        total_tokens: Some(input_total + output),
        cache_read_input_tokens: Some(cache_read),
        cache_creation_input_tokens: Some(cache_write),
    })
}

/// Tool-call response from the assistant: emits a `tool_use` block then a
/// usage event so usage_history gains a measurement on this turn.
fn tool_use_response(call_id: &str, tool_name: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::tool_use_start(0, call_id, tool_name),
        LlmEvent::tool_input_delta(0, "{}"),
        LlmEvent::tool_use_stop(0),
        usage_event(500, 0, 0, 10),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

/// Plain text response with explicit cache_read/cache_write so that
/// `prune.post_request` can carry meaningful values when this is the
/// LLM call that follows a `prune.fire` event.
fn text_response_with_cache(text: &str, cache_read: u64, cache_write: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, text),
        LlmEvent::text_block_stop(0, None),
        usage_event(800, cache_read, cache_write, 5),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

fn manifest_toml(prune_protected_turns: usize, prune_min_savings: u64) -> String {
    format!(
        r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[compaction]
prune_protected_turns = {prune_protected_turns}
prune_min_savings = {prune_min_savings}

[[scope.allow]]
target = "./"
permission = "write"
"#
    )
}

async fn make_pod(
    manifest_toml: String,
    client: MockClient,
    tool_name: &'static str,
) -> (
    Pod<MockClient, FsStore>,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let manifest = PodManifest::from_toml(&manifest_toml).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();

    let mut worker = Worker::new(client);
    worker.register_tool(big_content_tool_definition(tool_name));

    let pod = Pod::new(manifest, worker, store, pwd, scope).await.unwrap();
    (pod, store_tmp, pwd_tmp)
}

/// Drive Pod through enough runs to exercise both skip-no_candidates and
/// fire branches, then read the session log back and assert the metric
/// stream.
#[tokio::test]
async fn prune_metrics_emit_skip_then_fire_with_post_request_join() {
    // Run 1 (request 0): tool_use → triggers tool execution → request 1
    //   on the second iteration to produce the assistant reply.
    // Run 2 (request 2): plain assistant text. Prune evaluation here
    //   sees user1's tool_result outside the 1-protected-turn window and
    //   should fire.
    let client = MockClient::new(vec![
        tool_use_response("call-1", "big_tool"),
        text_response_with_cache("ok", 0, 200),
        text_response_with_cache("done", 1234, 50),
    ]);
    let (mut pod, _store_tmp, _pwd_tmp) = make_pod(manifest_toml(1, 1), client, "big_tool").await;
    let session_id = pod.session_id();
    // Cloning the store handle to read the session log back after the
    // runs complete — the Pod retains its own copy.
    let store = pod.store().clone();

    pod.run_text("first").await.unwrap();
    pod.run_text("second").await.unwrap();

    let state = session_store::restore(&store, session_id).await.unwrap();
    let metrics = metrics_from_extensions(&state.extensions);

    // Run 1 has 2 LLM iterations (tool loop), each evaluates prune with
    // only one user-message turn → 2x skip{no_candidates}.
    // Run 2 has 1 LLM iteration with enough turns → 1x fire +
    // 1x post_request paired by correlation_id.
    let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
    assert!(
        names.contains(&"prune.skip"),
        "expected prune.skip in {names:?}"
    );
    assert!(
        names.contains(&"prune.fire"),
        "expected prune.fire in {names:?}"
    );
    assert!(
        names.contains(&"prune.post_request"),
        "expected prune.post_request in {names:?}"
    );

    // All skips in run 1 must record reason=no_candidates.
    for m in metrics.iter().filter(|m| m.name == "prune.skip") {
        assert_eq!(
            m.dimensions.get("reason").map(String::as_str),
            Some("no_candidates"),
            "skip metric should be no_candidates here, got {m:?}"
        );
        assert!(m.correlation_id.is_none());
    }

    // The fire metric carries dimensions and correlation_id.
    let fire = metrics
        .iter()
        .find(|m| m.name == "prune.fire")
        .expect("prune.fire missing");
    assert!(
        fire.dimensions.contains_key("candidate_count"),
        "fire missing candidate_count: {fire:?}"
    );
    assert!(
        fire.dimensions.contains_key("border_turn"),
        "fire missing border_turn: {fire:?}"
    );
    assert!(fire.value.is_some(), "fire missing estimated_savings value");
    let fire_id = fire
        .correlation_id
        .as_ref()
        .expect("fire metric missing correlation_id");

    // Exactly one post_request metric should exist with the same id, and
    // its value/dimension should reflect the cache numbers from the
    // text_response_with_cache call (cache_read=1234, cache_write=50).
    let post = metrics
        .iter()
        .find(|m| m.name == "prune.post_request")
        .expect("prune.post_request missing");
    assert_eq!(post.correlation_id.as_ref(), Some(fire_id));
    assert_eq!(post.value, Some(1234.0));
    assert_eq!(
        post.dimensions
            .get("cache_write_tokens")
            .map(String::as_str),
        Some("50")
    );
    assert!(post.dimensions.contains_key("history_len"));
}

/// `min_savings` set high enough that candidates exist but the estimated
/// savings always fall short → the second run should record
/// `prune.skip { reason: "below_min_savings" }`.
#[tokio::test]
async fn prune_metrics_record_below_min_savings_skip() {
    let client = MockClient::new(vec![
        tool_use_response("call-1", "big_tool"),
        text_response_with_cache("ok", 0, 100),
        text_response_with_cache("done", 0, 0),
    ]);
    let (mut pod, _store_tmp, _pwd_tmp) =
        make_pod(manifest_toml(1, u64::MAX), client, "big_tool").await;
    let session_id = pod.session_id();
    let store = pod.store().clone();

    pod.run_text("first").await.unwrap();
    pod.run_text("second").await.unwrap();

    let state = session_store::restore(&store, session_id).await.unwrap();
    let metrics = metrics_from_extensions(&state.extensions);
    let below = metrics
        .iter()
        .find(|m| {
            m.name == "prune.skip"
                && m.dimensions.get("reason").map(String::as_str) == Some("below_min_savings")
        })
        .expect("expected prune.skip with reason=below_min_savings");
    assert!(
        below.dimensions.contains_key("candidate_count"),
        "below_min_savings skip should report candidate_count: {below:?}"
    );
    assert!(
        below.value.is_some(),
        "below_min_savings skip should report estimated savings as value: {below:?}"
    );
    // No prune.fire for this scenario.
    assert!(metrics.iter().all(|m| m.name != "prune.fire"));
    // No prune.post_request either (no fire to join with).
    assert!(metrics.iter().all(|m| m.name != "prune.post_request"));
}

/// `Store` wrapper that delegates to an inner `FsStore` for everything
/// except `LogEntry::Extension { domain: "metrics", .. }` appends, which
/// it rejects with an `Io` error. Lets us drive the `try_record_metric`
/// failure path without affecting any other persistence write.
#[derive(Clone)]
struct MetricFailingStore {
    inner: FsStore,
}

impl Store for MetricFailingStore {
    async fn append(&self, id: SessionId, entry: &HashedEntry) -> Result<(), StoreError> {
        if let LogEntry::Extension { domain, .. } = &entry.entry {
            if domain == DOMAIN {
                return Err(StoreError::Io(std::io::Error::other("synthetic failure")));
            }
        }
        self.inner.append(id, entry).await
    }
    async fn read_all(&self, id: SessionId) -> Result<Vec<HashedEntry>, StoreError> {
        self.inner.read_all(id).await
    }
    async fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        self.inner.list_sessions().await
    }
    async fn create_session(
        &self,
        id: SessionId,
        entries: &[HashedEntry],
    ) -> Result<(), StoreError> {
        self.inner.create_session(id, entries).await
    }
    async fn exists(&self, id: SessionId) -> Result<bool, StoreError> {
        self.inner.exists(id).await
    }
    async fn read_head_hash(&self, id: SessionId) -> Result<Option<EntryHash>, StoreError> {
        self.inner.read_head_hash(id).await
    }
    async fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> Result<(), StoreError> {
        self.inner.append_trace(id, entry).await
    }
}

/// Metric write failures are non-fatal: the run still completes, the
/// session log carries no metric entries (drops), but a `Warn` alert
/// fires on the alerter so the TUI surface picks it up.
#[tokio::test]
async fn metric_write_failure_emits_warn_alert_and_does_not_abort_run() {
    use protocol::{AlertLevel, AlertSource, Event};
    use tokio::sync::broadcast;

    let manifest_toml = manifest_toml(1, 1);
    let manifest = PodManifest::from_toml(&manifest_toml).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let inner = FsStore::new(store_tmp.path()).await.unwrap();
    let store = MetricFailingStore { inner };
    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();

    // Even with a tool registered, this run will only emit
    // `prune.skip { reason: "no_candidates" }` (one user message,
    // protected_turns=1 covers everything). That is enough to drive
    // the failure path: at least one metric attempts to write.
    let client = MockClient::new(vec![text_response_with_cache("hi", 0, 0)]);
    let worker = Worker::new(client);
    let mut pod = Pod::new(manifest, worker, store.clone(), pwd, scope)
        .await
        .unwrap();

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    let alerter = pod::Alerter::new(tx);
    pod.attach_alerter(alerter);

    let session_id = pod.session_id();
    // Run completes successfully despite metric failure.
    pod.run_text("hello").await.unwrap();

    // No metrics ended up in the log (writes were rejected).
    let state = session_store::restore(&store, session_id).await.unwrap();
    let metrics = metrics_from_extensions(&state.extensions);
    assert!(metrics.is_empty(), "metrics must drop on write failure");

    // The alerter saw at least one Warn from AlertSource::Pod.
    let mut saw_warn = false;
    while let Ok(ev) = rx.try_recv() {
        if let Event::Alert(a) = ev {
            if a.level == AlertLevel::Warn
                && a.source == AlertSource::Pod
                && a.message.contains("metric")
            {
                saw_warn = true;
                break;
            }
        }
    }
    assert!(saw_warn, "expected Warn/Pod alert about metric failure");
}

/// Sessions that have no metrics in the log restore cleanly: the
/// `RestoredState.extensions` simply contains no `metrics` domain
/// payloads, and `metrics_from_extensions` returns an empty Vec.
/// Backward-compatibility check for old logs predating this feature.
#[tokio::test]
async fn old_sessions_without_metrics_replay_cleanly() {
    // Manifest without any `[compaction]` section → prune (and therefore
    // the prune observer) is never installed, so no metrics get written.
    let manifest_toml = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(vec![text_response_with_cache("hi", 0, 0)]);
    let manifest = PodManifest::from_toml(manifest_toml).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();
    let worker = Worker::new(client);
    let mut pod = Pod::new(manifest, worker, store.clone(), pwd, scope)
        .await
        .unwrap();
    let session_id = pod.session_id();
    pod.run_text("hello").await.unwrap();

    let state = session_store::restore(&store, session_id).await.unwrap();
    let metrics = metrics_from_extensions(&state.extensions);
    assert!(
        metrics.is_empty(),
        "no metrics should be recorded: {metrics:?}"
    );
    // And no extension entries at all in the metrics domain.
    assert!(state.extensions.iter().all(|(d, _)| d != DOMAIN));

    // Smoke check that fold helper is robust on a sentinel Metric value:
    let m = Metric::now("smoke");
    assert_eq!(m.name, "smoke");
}
