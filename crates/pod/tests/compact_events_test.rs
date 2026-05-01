//! Compact lifecycle `Event` broadcasting.
//!
//! Covers three paths:
//! - `try_post_run_compact` success → `CompactStart + CompactDone`
//! - `try_post_run_compact` failure → `CompactStart + CompactFailed`
//! - mid-turn `do_compact_and_resume` success → `CompactStart + CompactDone`
//!   (driven by `compact_request_threshold` → `PreRequestAction::Yield`)

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::Stream;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use protocol::Event;
use session_store::FsStore;
use tokio::sync::broadcast;

use pod::Pod;

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

fn single_text_events(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, text),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

/// `single_text_events` + a UsageEvent so the Pod's `usage_history`
/// picks up a measurement, which is how `pre_llm_request` decides
/// whether to yield mid-turn.
fn text_events_with_usage(text: &str, input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, text),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::usage(input_tokens, 1),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

fn write_summary_tool_use_events(call_id: &str, text: &str) -> Vec<LlmEvent> {
    let input = serde_json::json!({ "text": text }).to_string();
    vec![
        LlmEvent::tool_use_start(0, call_id, "write_summary"),
        LlmEvent::tool_input_delta(0, input),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

// A low compact_threshold guarantees `try_post_run_compact` will fire
// the first time we check after a run.
const POST_RUN_MANIFEST_TOML: &str = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[compaction]
compact_threshold = 1
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

// `compact_request_threshold` drives the PodInterceptor's mid-turn yield
// path. `compact_threshold` is left unset so the post-run check stays inert.
const MID_TURN_MANIFEST_TOML: &str = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[compaction]
compact_request_threshold = 100
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

async fn make_pod_with_manifest(
    manifest_toml: &str,
    client: MockClient,
) -> Pod<MockClient, FsStore> {
    let manifest = pod::PodManifest::from_toml(manifest_toml).unwrap();

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    std::mem::forget(store_tmp);

    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let worker = Worker::new(client);
    Pod::new(manifest, worker, store, pwd, scope).await.unwrap()
}

async fn make_pod(client: MockClient) -> Pod<MockClient, FsStore> {
    make_pod_with_manifest(POST_RUN_MANIFEST_TOML, client).await
}

/// Drain whatever events are already queued on `rx`. Non-blocking.
fn drain(rx: &mut broadcast::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push(ev),
            Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn post_run_compact_success_broadcasts_start_and_done() {
    // Responses: (1) first run returns short text, (2) compact worker
    // emits write_summary then closes (two LLM calls inside the compact
    // worker: one for write_summary, one that the compact loop consumes
    // as the final "I'm done" close response).
    let client = MockClient::new(vec![
        single_text_events("hi"),
        write_summary_tool_use_events("call-1", "summary"),
        single_text_events("done"),
    ]);
    let mut pod = make_pod(client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    pod.attach_event_tx(tx);

    pod.run_text("first").await.unwrap();
    // Drain run events so only compact events remain in `rx`.
    let _ = drain(&mut rx);

    pod.try_post_run_compact().await.unwrap();

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart => "start",
            Event::CompactDone { .. } => "done",
            Event::CompactFailed { .. } => "failed",
            _ => "other",
        })
        .collect();
    assert!(
        kinds.contains(&"start") && kinds.contains(&"done"),
        "expected CompactStart + CompactDone in {kinds:?}"
    );
    assert!(
        !kinds.contains(&"failed"),
        "unexpected CompactFailed in {kinds:?}"
    );

    // CompactDone carries the new session id.
    let new_id_in_event = events.iter().find_map(|e| match e {
        Event::CompactDone { new_session_id } => Some(*new_session_id),
        _ => None,
    });
    assert!(new_id_in_event.is_some(), "CompactDone missing");
    assert_eq!(new_id_in_event.unwrap(), pod.session_id());
}

#[tokio::test]
async fn mid_turn_compact_success_broadcasts_start_and_done() {
    // Path: `do_compact_and_resume` via PreRequestAction::Yield.
    //
    // Sequence of LLM calls the mock will serve:
    //   [0] first run completes with a UsageEvent(1000 > threshold=100) so
    //       the next run's pre_llm_request will yield.
    //   [1] compact worker emits `write_summary` tool call.
    //   [2] compact worker closes (its final "done" response).
    //   [3] resume() after compact makes one more LLM call.
    let client = MockClient::new(vec![
        text_events_with_usage("a", 1000),
        write_summary_tool_use_events("call-1", "summary"),
        single_text_events("done"),
        single_text_events("b"),
    ]);
    let mut pod = make_pod_with_manifest(MID_TURN_MANIFEST_TOML, client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    pod.attach_event_tx(tx);

    // First run populates usage_history above the request threshold.
    pod.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Second run: pre_llm_request yields immediately, Worker returns
    // Yielded, handle_worker_result routes into do_compact_and_resume.
    pod.run_text("second").await.unwrap();

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart => "start",
            Event::CompactDone { .. } => "done",
            Event::CompactFailed { .. } => "failed",
            _ => "other",
        })
        .collect();
    assert!(
        kinds.contains(&"start") && kinds.contains(&"done"),
        "expected CompactStart + CompactDone in {kinds:?}"
    );
    assert!(
        !kinds.contains(&"failed"),
        "unexpected CompactFailed in {kinds:?}"
    );

    let new_id_in_event = events.iter().find_map(|e| match e {
        Event::CompactDone { new_session_id } => Some(*new_session_id),
        _ => None,
    });
    assert_eq!(new_id_in_event, Some(pod.session_id()));
}

/// Regression: `Pod::compact()` must reset the in-memory
/// `extract_pointer` so Phase 1 keeps firing on the new compacted
/// session.
///
/// Without the reset, the pointer's `processed_through_history_len`
/// holds the old (typically large) item count, while the new compacted
/// session starts with a much shorter history (`[summary, ...]`).
/// `cumulative_input_tokens_since` would then filter every new
/// usage record out (their `history_len` is below the stale pointer)
/// and Phase 1 would never re-fire for the rest of the process.
const EXTRACT_PLUS_COMPACT_MANIFEST: &str = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[memory]
extract_threshold = 1

[compaction]
compact_threshold = 1
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

fn write_extracted_tool_use_events(call_id: &str) -> Vec<LlmEvent> {
    let input = serde_json::json!({
        "decisions": [],
        "discussions": [],
        "attempts": [],
        "requests": []
    })
    .to_string();
    vec![
        LlmEvent::tool_use_start(0, call_id, "write_extracted"),
        LlmEvent::tool_input_delta(0, input),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

#[tokio::test]
async fn compact_resets_extract_pointer_so_phase1_can_fire_again() {
    // Mock LLM responses, in call order:
    //   [0] first run with usage(1000) so extract threshold (=1) fires.
    //   [1] extract worker invokes write_extracted with empty payload.
    //   [2] extract worker closes after the tool result.
    //   [3] compact worker invokes write_summary.
    //   [4] compact worker closes after the tool result.
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        write_extracted_tool_use_events("ec1"),
        single_text_events("done"),
        write_summary_tool_use_events("sc1", "summary"),
        single_text_events("done"),
    ]);
    let mut pod = make_pod_with_manifest(EXTRACT_PLUS_COMPACT_MANIFEST, client).await;

    pod.run_text("first").await.unwrap();

    // Phase 1 fires; pointer becomes Some.
    pod.try_post_run_extract().await.unwrap();
    assert!(
        pod.extract_pointer().is_some(),
        "extract_pointer should be Some after a successful extract"
    );

    // Compact runs. Without the fix the in-memory pointer would still
    // reference the old session's history_len.
    pod.try_post_run_compact().await.unwrap();
    assert!(
        pod.extract_pointer().is_none(),
        "extract_pointer must be reset to None after compact (matches cold-restore on the new session)"
    );
}

/// `extract_threshold = 0` is treated as "disabled" — without this, a
/// raw `>=` comparison against `tokens_since` would fire Phase 1 on
/// every post-run regardless of activity. Mirrors the Phase 2
/// zero-threshold convention so users have a single way to opt out
/// without removing the `[memory]` section.
const EXTRACT_THRESHOLD_ZERO_MANIFEST: &str = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[memory]
extract_threshold = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

#[tokio::test]
async fn extract_threshold_zero_is_disabled() {
    // Mock provides exactly one response — the first run. If Phase 1
    // were treated as "fire on any change" because of `tokens_since >= 0`,
    // it would call into the extract worker and exhaust the mock.
    let client = MockClient::new(vec![text_events_with_usage("hi", 1000)]);
    let mut pod = make_pod_with_manifest(EXTRACT_THRESHOLD_ZERO_MANIFEST, client).await;

    pod.run_text("first").await.unwrap();
    pod.try_post_run_extract()
        .await
        .expect("extract_threshold=0 must skip silently, not fail");
    assert!(
        pod.extract_pointer().is_none(),
        "no extract should have run — pointer must remain None"
    );
}

#[tokio::test]
async fn post_run_compact_failure_broadcasts_start_and_failed() {
    // Only the first run has a response. Compaction will run the
    // compact worker which immediately exhausts the mock → failure.
    let client = MockClient::new(vec![single_text_events("hi")]);
    let mut pod = make_pod(client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    pod.attach_event_tx(tx);

    pod.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Best-effort: returns Ok(()) even on failure, but emits CompactFailed.
    pod.try_post_run_compact().await.unwrap();

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart => "start",
            Event::CompactDone { .. } => "done",
            Event::CompactFailed { .. } => "failed",
            _ => "other",
        })
        .collect();
    assert!(
        kinds.contains(&"start") && kinds.contains(&"failed"),
        "expected CompactStart + CompactFailed in {kinds:?}"
    );
    assert!(
        !kinds.contains(&"done"),
        "unexpected CompactDone in {kinds:?}"
    );
}
