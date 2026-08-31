//! Compact lifecycle `Event` broadcasting.
//!
//! Covers three paths:
//! - `try_pre_run_compact` success → `CompactStart + CompactDone`
//! - `try_pre_run_compact` failure → `CompactStart + CompactFailed`
//! - mid-turn `do_compact_and_resume` success → `CompactStart + CompactDone`
//!   (driven by `compact_request_threshold` → `PreRequestAction::Yield`)

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agen::Engine;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::types::Item;
use agen::llm_client::{ClientError, LlmClient, Request};
use async_trait::async_trait;
use futures::Stream;
use protocol::{Event, Method, RunResult};
use session_store::{CombinedStore, FsWorkerStore, WorkerMetadataStore};
use session_store::{FsStore, LogEntry, Store};
use tokio::sync::broadcast;

use worker::{Worker, WorkerController};

type TestStore = CombinedStore<FsStore, FsWorkerStore>;

fn annotated(item: Item) -> session_store::LoggedHistoryEntry {
    session_store::LoggedHistoryEntry {
        item: session_store::LoggedItem::from(item),
        metadata: session_store::LoggedSessionHistoryMetadata {
            entry_id: session_store::LoggedSessionHistoryEntryId::new(),
            origin: session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
        },
    }
}

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

/// `single_text_events` + a UsageEvent so the Worker's `usage_history`
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

// A low compact_threshold guarantees `try_pre_run_compact` will fire
// the first time we check after a run.
const POST_RUN_MANIFEST_TOML: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[compaction]
compact_threshold = 1
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

// `compact_request_threshold` drives the WorkerInterceptor's mid-turn yield
// path. `compact_threshold` is left unset so the post-run check stays inert.
const MID_TURN_MANIFEST_TOML: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[compaction]
compact_request_threshold = 100
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

async fn make_worker_with_manifest(
    manifest_toml: &str,
    client: MockClient,
) -> Worker<MockClient, TestStore> {
    let manifest = worker::WorkerManifest::from_toml(manifest_toml).unwrap();

    let store_tmp = tempfile::tempdir().unwrap();
    let store = CombinedStore::new(
        FsStore::new(store_tmp.path()).unwrap(),
        FsWorkerStore::new(store_tmp.path().join("pods")).unwrap(),
    );
    std::mem::forget(store_tmp);

    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = worker::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let worker =
        Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(client);
    let mut worker = Worker::new(
        manifest,
        worker,
        store,
        worker::WorkerWorkspaceContext::local_filesystem(None),
        worker::WorkerFilesystemAuthority::local(pwd.clone(), pwd.clone()),
        scope,
    )
    .await
    .unwrap();
    worker.enable_worker_metadata_write_through().unwrap();
    worker
}

async fn make_worker(client: MockClient) -> Worker<MockClient, TestStore> {
    make_worker_with_manifest(POST_RUN_MANIFEST_TOML, client).await
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

/// Collect every system-message text that the post-compaction
/// `SegmentStart.history` carries, by reading the sink mirror directly.
fn system_texts_in_sink_session_start(
    worker: &worker::Worker<
        impl agen::llm_client::client::LlmClient + Clone + 'static,
        impl session_store::Store + Clone + 'static,
    >,
) -> Vec<String> {
    let (entries, _rx) = worker.sink().subscribe_with_snapshot();
    for entry in entries.into_iter().rev() {
        let history = match entry {
            session_store::LogEntry::AnnotatedSegmentStart { history, .. } => history
                .into_iter()
                .map(|entry| entry.item)
                .collect::<Vec<_>>(),
            _ => continue,
        };
        return history
            .into_iter()
            .filter_map(|logged| {
                let item: Item = logged.into();
                match item {
                    Item::Message {
                        role: agen::Role::System,
                        content,
                        ..
                    } => Some(
                        content
                            .iter()
                            .map(|p| p.as_text().to_owned())
                            .collect::<Vec<_>>()
                            .join(""),
                    ),
                    _ => None,
                }
            })
            .collect();
    }
    Vec::new()
}

/// Worker metadata starts with a reserved Session and no Segment, then becomes
/// active once the first SegmentStart is materialized by `run`.
#[tokio::test]
async fn worker_metadata_moves_from_pending_to_active_on_first_run() {
    let client = MockClient::new(vec![single_text_events("hi")]);
    let mut worker = make_worker(client).await;
    let store = worker.store().clone();
    let session_id = worker.session_id();
    let initial_segment_id = worker.segment_id();

    let pending = store
        .read_by_name("test-worker")
        .unwrap()
        .expect("metadata should be initialized at Worker construction");
    assert_eq!(pending.worker_name, "test-worker");
    let pending_active = pending.active.expect("active session pointer missing");
    assert_eq!(pending_active.session_id, session_id);
    assert_eq!(pending_active.segment_id, None);

    worker.run_text("first").await.unwrap();

    let resolved = store
        .read_by_name("test-worker")
        .unwrap()
        .expect("metadata should still exist after first run");
    let active = resolved.active.expect("active session pointer missing");
    assert_eq!(active.session_id, session_id);
    assert_eq!(active.segment_id, Some(initial_segment_id));
}

/// Live auto-fork: when another writer extends the segment behind the
/// Worker's back, the next run's `ensure_segment_head` detects the
/// entry-count drift and branches into a fresh segment **within the same
/// Session**. The source segment is left immutable (no terminal marker
/// written back); the new segment records its parentage forward via
/// `SegmentStart.forked_from`.
#[tokio::test]
async fn concurrent_writer_drift_auto_forks_with_forked_from() {
    // No compaction: keep run → run deterministic so each run consumes
    // exactly one mock response and ensure_segment_head is the only fork
    // trigger.
    const NO_COMPACT_MANIFEST_TOML: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(vec![
        single_text_events("first"),
        single_text_events("second"),
    ]);
    let mut worker = make_worker_with_manifest(NO_COMPACT_MANIFEST_TOML, client).await;

    worker.run_text("first").await.unwrap();

    let store = worker.store().clone();
    let session_id = worker.session_id();
    let source_segment_id = worker.segment_id();
    let source_len_before = store.read_all(session_id, source_segment_id).unwrap().len();

    // Simulate a foreign writer appending to the same segment. This bumps
    // the on-disk entry count past the Worker's own append tally without
    // updating the Worker's `entries_written`.
    session_store::save_user_input(
        &store,
        session_id,
        source_segment_id,
        vec![protocol::Segment::text("interloper")],
        vec![annotated(Item::user_message("interloper"))],
    )
    .unwrap();

    // Next run triggers ensure_segment_head, which sees the drift.
    worker.run_text("second").await.unwrap();

    // The Worker moved to a new segment in the same Session.
    let new_segment_id = worker.segment_id();
    assert_ne!(new_segment_id, source_segment_id);
    assert_eq!(
        worker.session_id(),
        session_id,
        "auto-fork stays in-Session"
    );
    let metadata = store
        .read_by_name("test-worker")
        .unwrap()
        .expect("metadata should exist after auto-fork");
    let active = metadata.active.expect("active session pointer missing");
    assert_eq!(active.session_id, session_id);
    assert_eq!(active.segment_id, Some(new_segment_id));

    // New segment records forked_from pointing at the source.
    let new_entries = store.read_all(session_id, new_segment_id).unwrap();
    match &new_entries[0] {
        LogEntry::AnnotatedSegmentStart {
            session_id: seg_session,
            forked_from: Some(origin),
            ..
        } => {
            assert_eq!(*seg_session, session_id);
            assert_eq!(origin.segment_id, source_segment_id);
        }
        other => panic!("expected SegmentStart with forked_from, got {other:?}"),
    }

    // Source segment is unchanged except for the foreign append — the
    // auto-fork wrote no terminal marker back into it.
    let source_after = store.read_all(session_id, source_segment_id).unwrap();
    assert_eq!(source_after.len(), source_len_before + 1);
    assert!(matches!(
        source_after.last(),
        Some(LogEntry::AnnotatedUserInput { .. })
    ));
}

#[tokio::test]
async fn compact_emits_session_start_carrying_summary_and_task_snapshot() {
    let client = MockClient::new(vec![
        single_text_events("hi"),
        write_summary_tool_use_events("call-1", "summary"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker(client).await;

    let (tx, _rx_keep) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    worker.run_text("first").await.unwrap();
    let session_id = worker.session_id();
    worker.compact(10_000).await.unwrap();
    let compacted_segment_id = worker.segment_id();
    let metadata = worker
        .store()
        .read_by_name("test-worker")
        .unwrap()
        .expect("metadata should exist after compaction");
    let active = metadata.active.expect("active session pointer missing");
    assert_eq!(active.session_id, session_id);
    assert_eq!(active.segment_id, Some(compacted_segment_id));

    let system_texts = system_texts_in_sink_session_start(&worker);
    // The post-compaction `SegmentStart.history` carries the new system
    // messages introduced by the compactor. Clients re-seed their view
    // from this entry alone, so it is the load-bearing payload.
    assert!(
        system_texts
            .iter()
            .any(|text| text.starts_with("[Compacted context summary]")),
        "summary system message missing from {system_texts:?}"
    );
    assert!(
        system_texts
            .iter()
            .any(|text| text.starts_with("[Session TaskStore snapshot]")),
        "task snapshot system message missing from {system_texts:?}"
    );
}

#[tokio::test]
async fn pre_run_compact_success_broadcasts_start_and_done() {
    // Responses: (1) first run returns short text, (2) compact worker
    // emits write_summary then closes (two LLM calls inside the compact
    // worker: one for write_summary, one that the compact loop consumes
    // as the final "I'm done" close response).
    let client = MockClient::new(vec![
        single_text_events("hi"),
        write_summary_tool_use_events("call-1", "summary"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker(client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    worker.run_text("first").await.unwrap();
    // Drain run events so only compact events remain in `rx`.
    let _ = drain(&mut rx);

    let session_before = worker.session_id();
    let segment_before = worker.segment_id();
    worker.try_pre_run_compact().await;
    assert_eq!(worker.session_id(), session_before);
    assert_ne!(worker.segment_id(), segment_before);

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart { .. } => "start",
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
    let starts = events
        .iter()
        .filter_map(|event| match event {
            Event::CompactStart { lifecycle } => Some(lifecycle),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts.len(),
        2,
        "start and Internal Worker binding revisions"
    );
    assert_eq!(starts[0].compaction_id, starts[1].compaction_id);
    assert_eq!(starts[0].revision, 1);
    assert!(starts[0].internal_worker.is_none());
    assert_eq!(starts[1].revision, 2);
    assert!(matches!(
        starts[1].internal_worker.as_ref().map(|worker| &worker.kind),
        Some(protocol::InternalWorkerKind::Service { kind }) if kind == "compaction"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::InternalWorker { worker, .. }
            if matches!(&worker.kind, protocol::InternalWorkerKind::Service { kind } if kind == "compaction")
    )), "compactor activity must be projected through the parent stream");
    let completed = events
        .iter()
        .find_map(|event| match event {
            Event::CompactDone { lifecycle } => Some(lifecycle),
            _ => None,
        })
        .expect("completed lifecycle");
    assert_eq!(completed.compaction_id, starts[0].compaction_id);
    assert_eq!(completed.revision, 3);
    assert_eq!(completed.summary.as_deref(), Some("summary"));
    assert_eq!(completed.state, protocol::CompactionLifecycleState::Done);
    let done_index = events
        .iter()
        .position(|event| matches!(event, Event::CompactDone { .. }))
        .expect("done event");
    let removed_index = events
        .iter()
        .position(|event| matches!(event, Event::InternalWorkerRemoved { .. }))
        .expect("terminal compactor session must be released");
    assert!(
        done_index < removed_index,
        "terminal lifecycle precedes release fence"
    );

    // CompactDone carries the new Segment ID; the Session ID is unchanged.
    let new_id_in_event = events.iter().find_map(|e| match e {
        Event::CompactDone { lifecycle } => lifecycle
            .new_segment_id
            .as_deref()
            .and_then(|value| uuid::Uuid::parse_str(value).ok()),
        _ => None,
    });
    assert!(new_id_in_event.is_some(), "CompactDone missing");
    assert_eq!(new_id_in_event.unwrap(), worker.segment_id());
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
    let mut worker = make_worker_with_manifest(MID_TURN_MANIFEST_TOML, client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    // First run populates usage_history above the request threshold.
    worker.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Second run: pre_llm_request yields immediately, Engine returns
    // Yielded, handle_worker_result routes into do_compact_and_resume.
    worker.run_text("second").await.unwrap();

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart { .. } => "start",
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
        Event::CompactDone { lifecycle } => lifecycle
            .new_segment_id
            .as_deref()
            .and_then(|value| uuid::Uuid::parse_str(value).ok()),
        _ => None,
    });
    assert_eq!(new_id_in_event, Some(worker.segment_id()));
}

/// Regression: `Worker::compact()` must reset the in-memory
/// `extract_pointer` so extract keeps firing on the new compacted
/// session.
///
/// Without the reset, the pointer's `processed_through_history_len`
/// holds the old (typically large) item count, while the new compacted
/// session starts with a much shorter history (`[summary, ...]`).
/// `cumulative_input_tokens_since` would then filter every new
/// usage record out (their `history_len` is below the stale pointer)
/// and extract would never re-fire for the rest of the process.
const EXTRACT_PLUS_COMPACT_MANIFEST: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[memory]
workspace_id = "test-workspace"
settings_revision = 1
language = "English"
extract_threshold = 1

[compaction]
compact_threshold = 1
compact_retained_tokens = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

fn finish_memory_extraction_tool_use_events(call_id: &str) -> Vec<LlmEvent> {
    let input = serde_json::json!({
        "staged_count": 0,
        "no_candidates_reason": "test run has no durable candidates"
    })
    .to_string();
    vec![
        LlmEvent::tool_use_start(0, call_id, "FinishMemoryExtraction"),
        LlmEvent::tool_input_delta(0, input),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

#[tokio::test]
async fn compact_resets_extract_pointer_so_extract_can_fire_again() {
    // Mock LLM responses, in call order:
    //   [0] first run with usage(1000) so extract threshold (=1) fires.
    //   [1] extract worker invokes FinishMemoryExtraction with empty output.
    //   [2] extract worker closes after the tool result.
    //   [3] compact worker invokes write_summary.
    //   [4] compact worker closes after the tool result.
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        finish_memory_extraction_tool_use_events("ec1"),
        single_text_events("done"),
        write_summary_tool_use_events("sc1", "summary"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker_with_manifest(EXTRACT_PLUS_COMPACT_MANIFEST, client).await;

    worker.run_text("first").await.unwrap();

    // extract fires; pointer becomes Some.
    worker.try_post_run_extract().await.unwrap();
    assert!(
        worker.extract_pointer().is_some(),
        "extract_pointer should be Some after a successful extract"
    );

    // Compact runs. Without the fix the in-memory pointer would still
    // reference the old Segment's history_len.
    worker.try_pre_run_compact().await;
    assert!(
        worker.extract_pointer().is_none(),
        "extract_pointer must be reset to None after compact (matches cold-restore on the new Segment)"
    );
}

/// `extract_threshold = 0` is treated as "disabled" — without this, a
/// raw `>=` comparison against `tokens_since` would fire extract on
/// every post-run regardless of activity. Mirrors the consolidation
/// zero-threshold convention so users have a single way to opt out
/// without removing the `[memory]` section.
const EXTRACT_THRESHOLD_ZERO_MANIFEST: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[memory]
extract_threshold = 0

[[scope.allow]]
target = "./"
permission = "write"
"#;

#[tokio::test]
async fn extract_threshold_zero_is_disabled() {
    // Mock provides exactly one response — the first run. If extract
    // were treated as "fire on any change" because of `tokens_since >= 0`,
    // it would call into the extract worker and exhaust the mock.
    let client = MockClient::new(vec![text_events_with_usage("hi", 1000)]);
    let mut worker = make_worker_with_manifest(EXTRACT_THRESHOLD_ZERO_MANIFEST, client).await;

    worker.run_text("first").await.unwrap();
    worker
        .try_post_run_extract()
        .await
        .expect("extract_threshold=0 must skip silently, not fail");
    assert!(
        worker.extract_pointer().is_none(),
        "no extract should have run — pointer must remain None"
    );
}

#[tokio::test]
async fn pre_run_compact_failure_broadcasts_start_and_failed() {
    // Only the first run has a response. Compaction will run the
    // compact worker which immediately exhausts the mock → failure.
    let client = MockClient::new(vec![single_text_events("hi")]);
    let mut worker = make_worker(client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    worker.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Best-effort: returns Ok(()) even on failure, but emits CompactFailed.
    worker.try_pre_run_compact().await;

    let events = drain(&mut rx);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::CompactStart { .. } => "start",
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

// ---------------------------------------------------------------------------
// Detached post-run memory jobs (`spawn_post_run_memory_jobs` /
// `wait_for_memory_jobs`). Covers the detach round-trip and the structural
// invariant that the cloned memory-task Worker shares `SegmentState` with the
// source Worker, so that `save_extension` from the background extract does not
// leave the next turn's `save_user_input` looking at a stale session pointer.

const EXTRACT_NO_COMPACT_MANIFEST: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[memory]
workspace_id = "test-workspace"
settings_revision = 1
language = "English"
extract_threshold = 1

[[scope.allow]]
target = "./"
permission = "write"
"#;

#[tokio::test]
async fn extract_large_unprocessed_range_does_not_abort_on_input_occupancy() {
    let client = MockClient::new(vec![
        text_events_with_usage("recorded", 1000),
        finish_memory_extraction_tool_use_events("ec-large"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker_with_manifest(EXTRACT_NO_COMPACT_MANIFEST, client).await;

    let large_request = format!("remember this large slice: {}", "x ".repeat(200_000));
    worker.run_text(&large_request).await.unwrap();

    worker.try_post_run_extract().await.expect(
        "large unprocessed extract ranges must reach the extract worker, not abort locally",
    );
    assert!(
        worker.extract_pointer().is_some(),
        "successful extract should advance the pointer even when the input range is large"
    );
}

#[tokio::test]
async fn spawn_and_wait_drives_extract_to_completion() {
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        finish_memory_extraction_tool_use_events("ec1"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker_with_manifest(EXTRACT_NO_COMPACT_MANIFEST, client).await;

    worker.run_text("first").await.unwrap();
    assert!(
        worker.extract_pointer().is_none(),
        "extract has not run yet — pointer must be None"
    );

    worker.spawn_post_run_memory_jobs();
    worker.wait_for_memory_jobs().await;

    assert!(
        worker.extract_pointer().is_some(),
        "spawn + wait must complete extract; pointer should be set"
    );
}

#[tokio::test]
async fn detached_extract_does_not_fork_session_log() {
    // Source worker and the cloned memory-task worker share `SegmentState` via
    // `Arc<_>`. The detached extract advances the entry tally through
    // `save_extension`; the next `run` must see that same tally so
    // `ensure_head_or_fork` does not spawn a new session.
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        finish_memory_extraction_tool_use_events("ec1"),
        single_text_events("done"),
        text_events_with_usage("ok", 1000),
    ]);
    let mut worker = make_worker_with_manifest(EXTRACT_NO_COMPACT_MANIFEST, client).await;

    worker.run_text("first").await.unwrap();
    let session_before = worker.segment_id();

    worker.spawn_post_run_memory_jobs();
    worker.wait_for_memory_jobs().await;

    worker.run_text("second").await.unwrap();
    let session_after = worker.segment_id();

    assert_eq!(
        session_before, session_after,
        "detached extract's save_extension and the next turn's save_user_input \
         must share the entry tally through SegmentState — a fork here means the \
         clone carried its own counter"
    );
}

#[tokio::test]
async fn controller_compact_method_emits_start_and_done() {
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        write_summary_tool_use_events("manual-summary", "manual compact summary"),
        single_text_events("done"),
    ]);
    let worker = make_worker_with_manifest(POST_RUN_MANIFEST_TOML, client).await;
    let runtime_tmp = tempfile::tempdir().unwrap();
    let (handle, _shutdown) = WorkerController::spawn(worker, runtime_tmp.path())
        .await
        .unwrap();
    let mut rx = handle.subscribe();

    handle
        .send(Method::run_text("seed history"))
        .await
        .expect("send run");
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for run end")
            .expect("event")
        {
            Event::RunEnd {
                result: RunResult::Finished,
            } => break,
            _ => {}
        }
    }

    handle.send(Method::Compact).await.expect("send compact");
    let mut saw_start = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for compact events")
            .expect("event")
        {
            Event::CompactStart { .. } => saw_start = true,
            Event::CompactDone { .. } => {
                break;
            }
            Event::CompactFailed { lifecycle } => panic!(
                "manual compact failed: {}",
                lifecycle.error.as_deref().unwrap_or("unknown error")
            ),
            _ => {}
        }
    }

    assert!(saw_start, "manual compact should emit CompactStart");
    let _ = handle.send(Method::Shutdown).await;
}
