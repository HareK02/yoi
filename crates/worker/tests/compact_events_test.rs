//! Compact lifecycle `Event` broadcasting.
//!
//! Covers three paths:
//! - `try_pre_run_compact` success → `CompactStart + CompactDone`
//! - `try_pre_run_compact` failure → `CompactStart + CompactFailed`
//! - mid-turn `do_compact_and_resume` success → `CompactStart + CompactDone`
//!   (driven by `compact_request_threshold` → `PreRequestAction::Yield`)

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agen::Engine;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent, UsageEvent};
use agen::llm_client::types::Item;
use agen::llm_client::{ClientError, LlmClient, Request};
use async_trait::async_trait;
use futures::Stream;
use protocol::{Event, Method, RunResult};
use session_store::{
    CombinedStore, FsStore, FsWorkerStore, LogEntry, Store, WorkerMetadata, WorkerMetadataStore,
    WorkerStoreError,
};
use tokio::sync::broadcast;

use worker::{Worker, WorkerController};

type TestStore = CombinedStore<FsStore, FsWorkerStore>;

#[derive(Clone)]
struct FaultingWorkerMetadataStore {
    inner: FsWorkerStore,
    fail_next_update: Arc<AtomicBool>,
}

impl FaultingWorkerMetadataStore {
    fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            inner: FsWorkerStore::new(root).unwrap(),
            fail_next_update: Arc::new(AtomicBool::new(false)),
        }
    }

    fn arm_update_failure(&self) {
        self.fail_next_update.store(true, Ordering::SeqCst);
    }
}

impl WorkerMetadataStore for FaultingWorkerMetadataStore {
    fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
        let old_segment_id = self
            .inner
            .read_by_name(&metadata.worker_name)?
            .and_then(|current| current.active)
            .and_then(|active| active.segment_id);
        let new_segment_id = metadata
            .active
            .as_ref()
            .and_then(|active| active.segment_id);
        if old_segment_id != new_segment_id && self.fail_next_update.swap(false, Ordering::SeqCst) {
            return Err(WorkerStoreError::Io(std::io::Error::other(
                "injected active Segment commit failure",
            )));
        }
        self.inner.write(metadata)
    }

    fn read_by_name(&self, worker_name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
        self.inner.read_by_name(worker_name)
    }

    fn update_by_name<F>(
        &self,
        worker_name: &str,
        mutate: F,
    ) -> Result<WorkerMetadata, WorkerStoreError>
    where
        F: FnOnce(&mut WorkerMetadata),
    {
        let mut metadata = self
            .inner
            .read_by_name(worker_name)?
            .unwrap_or_else(|| WorkerMetadata::new(worker_name, None));
        let old_segment_id = metadata
            .active
            .as_ref()
            .and_then(|active| active.segment_id);
        mutate(&mut metadata);
        let new_segment_id = metadata
            .active
            .as_ref()
            .and_then(|active| active.segment_id);
        if old_segment_id != new_segment_id && self.fail_next_update.swap(false, Ordering::SeqCst) {
            return Err(WorkerStoreError::Io(std::io::Error::other(
                "injected active Segment commit failure",
            )));
        }
        self.inner.write(&metadata)?;
        Ok(metadata)
    }

    fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
        self.inner.list_names()
    }

    fn root_dir(&self) -> Option<std::path::PathBuf> {
        self.inner.root_dir()
    }

    fn delete_by_name(&self, worker_name: &str) -> Result<(), WorkerStoreError> {
        self.inner.delete_by_name(worker_name)
    }
}

type FaultingTestStore = CombinedStore<FsStore, FaultingWorkerMetadataStore>;

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

#[derive(Clone)]
struct BlockingCompactClient {
    calls: Arc<AtomicUsize>,
}

impl BlockingCompactClient {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for BlockingCompactClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok(Box::pin(futures::stream::iter(
                single_text_events("seed").into_iter().map(Ok),
            )))
        } else {
            Ok(Box::pin(futures::stream::pending()))
        }
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

fn write_summary_tool_use_events_with_usage(
    call_id: &str,
    text: &str,
    input_total: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> Vec<LlmEvent> {
    let mut events = write_summary_tool_use_events(call_id, text);
    events.insert(
        events.len() - 1,
        LlmEvent::Usage(UsageEvent {
            input_tokens: Some(input_total),
            output_tokens: Some(output),
            total_tokens: Some(input_total.saturating_add(output)),
            cache_read_input_tokens: Some(cache_read),
            cache_creation_input_tokens: Some(cache_write),
        }),
    );
    events
}

fn text_events_with_full_usage(
    text: &str,
    input_total: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> Vec<LlmEvent> {
    let mut events = single_text_events(text);
    events.insert(
        events.len() - 1,
        LlmEvent::Usage(UsageEvent {
            input_tokens: Some(input_total),
            output_tokens: Some(output),
            total_tokens: Some(input_total.saturating_add(output)),
            cache_read_input_tokens: Some(cache_read),
            cache_creation_input_tokens: Some(cache_write),
        }),
    );
    events
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

const MANUAL_ONLY_MANIFEST_TOML: &str = r#"
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[compaction]
compact_threshold = 1000000000
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

async fn make_worker_with_manifest<C>(manifest_toml: &str, client: C) -> Worker<C, TestStore>
where
    C: LlmClient + Clone + Send + Sync + 'static,
{
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

async fn make_faulting_worker(
    client: MockClient,
) -> (
    Worker<MockClient, FaultingTestStore>,
    FaultingWorkerMetadataStore,
    FsStore,
) {
    let manifest = worker::WorkerManifest::from_toml(MID_TURN_MANIFEST_TOML).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let segment_store = FsStore::new(store_tmp.path()).unwrap();
    let metadata_store = FaultingWorkerMetadataStore::new(store_tmp.path().join("pods"));
    let store = CombinedStore::new(segment_store.clone(), metadata_store.clone());
    std::mem::forget(store_tmp);

    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = worker::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let engine =
        Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(client);
    let mut worker = Worker::new(
        manifest,
        engine,
        store,
        worker::WorkerWorkspaceContext::local_filesystem(None),
        worker::WorkerFilesystemAuthority::local(pwd.clone(), pwd.clone()),
        scope,
    )
    .await
    .unwrap();
    worker.enable_worker_metadata_write_through().unwrap();
    (worker, metadata_store, segment_store)
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

#[tokio::test]
async fn active_segment_cas_rejects_stale_compaction_writer() {
    let client = MockClient::new(vec![
        single_text_events("seed response"),
        write_summary_tool_use_events("summary-1", "replacement summary"),
        single_text_events("done"),
    ]);
    let (mut worker, metadata_store, segment_store) = make_faulting_worker(client).await;
    worker.run_text("seed input").await.unwrap();
    let old_segment_id = worker.segment_id();
    let session_id = worker.session_id();
    let competing_segment_id = uuid::Uuid::now_v7();
    metadata_store
        .update_by_name("test-worker", |metadata| {
            metadata.active.as_mut().unwrap().segment_id = Some(competing_segment_id);
        })
        .unwrap();

    let error = worker.compact(0).await.unwrap_err();
    assert!(
        matches!(error, worker::WorkerError::CompactActiveSegmentChanged),
        "unexpected stale CAS error: {error:?}"
    );

    assert_eq!(worker.segment_id(), old_segment_id);
    let failure_metrics =
        session_metrics::read_segment_metrics(&segment_store, session_id, old_segment_id).unwrap();
    let finish = failure_metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "failure");
    assert_eq!(
        finish.metric.dimensions["failure_category"],
        "active_segment_commit"
    );
    assert!(!finish.metric.dimensions.contains_key("error"));
    assert_eq!(
        metadata_store
            .read_by_name("test-worker")
            .unwrap()
            .unwrap()
            .active
            .unwrap()
            .segment_id,
        Some(competing_segment_id)
    );
}

#[tokio::test]
async fn failed_active_segment_commit_keeps_live_and_durable_history_on_old_segment() {
    let client = MockClient::new(vec![
        single_text_events("seed response"),
        write_summary_tool_use_events("summary-1", "replacement summary"),
        single_text_events("continued on old segment"),
    ]);
    let (mut worker, metadata_store, segment_store) = make_faulting_worker(client).await;
    worker.run_text("seed input").await.unwrap();
    let old_segment_id = worker.segment_id();

    metadata_store.arm_update_failure();
    let error = worker.compact(0).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected active Segment commit failure")
    );

    assert_eq!(worker.segment_id(), old_segment_id);
    let failure_metrics =
        session_metrics::read_segment_metrics(&segment_store, worker.session_id(), old_segment_id)
            .unwrap();
    let start = failure_metrics
        .iter()
        .find(|record| record.metric.name == "compact.start")
        .unwrap();
    let finish = failure_metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "failure");
    assert_eq!(
        finish.metric.dimensions["failure_category"],
        "active_segment_commit"
    );
    assert_eq!(finish.metric.correlation_id, start.metric.correlation_id);
    assert!(
        !serde_json::to_string(&finish.metric)
            .unwrap()
            .contains("injected active Segment commit failure")
    );
    let metadata = metadata_store
        .read_by_name("test-worker")
        .unwrap()
        .expect("active Worker metadata should remain present");
    assert_eq!(
        metadata.active.and_then(|active| active.segment_id),
        Some(old_segment_id)
    );

    worker.run_text("continue input").await.unwrap();
    let active_records = segment_store
        .read_all(worker.session_id(), old_segment_id)
        .unwrap();
    assert!(
        format!("{active_records:?}").contains("continue input"),
        "the live Worker must continue appending to the old active Segment"
    );
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
        write_summary_tool_use_events_with_usage("call-1", "summary", 100, 10, 5, 20),
        text_events_with_full_usage("done", 50, 3, 2, 10),
        text_events_with_full_usage("after", 44, 4, 1, 6),
    ]);
    let mut worker = make_worker_with_manifest(MANUAL_ONLY_MANIFEST_TOML, client).await;

    let (tx, _rx_keep) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    worker.run_text("first").await.unwrap();
    let session_id = worker.session_id();
    let source_segment_id = worker.segment_id();
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

    worker.run_text("after compaction").await.unwrap();
    let metrics = session_metrics::read_session_metrics(worker.store(), session_id).unwrap();
    let starts = metrics
        .iter()
        .filter(|record| record.metric.name == "compact.start")
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].segment_id, source_segment_id);
    assert_eq!(starts[0].metric.dimensions["mode"], "automatic");
    assert_eq!(
        starts[0].metric.dimensions["threshold_policy"],
        "request_threshold"
    );
    let correlation_id = starts[0]
        .metric
        .correlation_id
        .as_deref()
        .expect("compact start must carry a correlation id");
    let finish = metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.segment_id, compacted_segment_id);
    assert_eq!(finish.metric.dimensions["outcome"], "success");
    assert_eq!(
        finish.metric.correlation_id.as_deref(),
        Some(correlation_id)
    );
    assert_eq!(
        finish.compacted_from.as_ref().unwrap().segment_id,
        starts[0].segment_id
    );

    let value = |name: &str| {
        metrics
            .iter()
            .find(|record| record.metric.name == name)
            .and_then(|record| record.metric.value)
            .unwrap() as u64
    };
    assert_eq!(value("compact.compactor.input_tokens"), 150);
    assert_eq!(value("compact.compactor.cache_read_tokens"), 13);
    assert_eq!(value("compact.compactor.cache_write_tokens"), 7);
    assert_eq!(value("compact.compactor.output_tokens"), 30);
    assert_eq!(value("compact.compactor.requests"), 2);
    assert!(value("compact.compactor.tool_calls") >= 1);
    assert!(value("compact.compactor.turns") >= 2);
    assert!(value("compact.duration_ms") <= u64::MAX);
    let cost = metrics
        .iter()
        .find(|record| record.metric.name == "compact.compactor.cost_usd")
        .unwrap();
    assert_eq!(cost.metric.value, None);
    assert_eq!(cost.metric.dimensions["status"], "unavailable");
    let post = metrics
        .iter()
        .find(|record| record.metric.name == "compact.post_request")
        .unwrap();
    assert_eq!(post.segment_id, compacted_segment_id);
    assert_eq!(post.metric.correlation_id.as_deref(), Some(correlation_id));
    assert_eq!(post.metric.dimensions["input_total_tokens"], "44");
    assert_eq!(post.metric.dimensions["cache_read_tokens"], "4");
    assert_eq!(post.metric.dimensions["cache_write_tokens"], "1");
    assert_eq!(post.metric.dimensions["output_tokens"], "6");
}

#[tokio::test]
async fn manual_compact_metrics_identify_manual_mode() {
    let client = MockClient::new(vec![
        single_text_events("seed response"),
        write_summary_tool_use_events("summary", "replacement summary"),
        single_text_events("done"),
    ]);
    let mut worker = make_worker_with_manifest(MANUAL_ONLY_MANIFEST_TOML, client).await;
    worker.run_text("seed input").await.unwrap();
    let session_id = worker.session_id();

    worker.manual_compact().await.unwrap();

    let metrics = session_metrics::read_session_metrics(worker.store(), session_id).unwrap();
    let start = metrics
        .iter()
        .find(|record| record.metric.name == "compact.start")
        .unwrap();
    assert_eq!(start.metric.dimensions["mode"], "manual");
    assert_eq!(start.metric.dimensions["threshold_policy"], "manual");
    let finish = metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "success");
    assert_eq!(finish.metric.correlation_id, start.metric.correlation_id);
}

#[tokio::test]
async fn compact_failure_and_cancellation_emit_bounded_categories() {
    let client = MockClient::new(vec![
        single_text_events("seed response"),
        single_text_events("missing summary"),
        single_text_events("still missing summary"),
    ]);
    let mut worker = make_worker_with_manifest(MANUAL_ONLY_MANIFEST_TOML, client).await;
    worker.run_text("seed input").await.unwrap();
    let session_id = worker.session_id();
    let source_segment_id = worker.segment_id();

    let error = worker.manual_compact().await.unwrap_err();
    assert!(matches!(error, worker::WorkerError::CompactSummaryMissing));
    let metrics = session_metrics::read_session_metrics(worker.store(), session_id).unwrap();
    let failure = metrics
        .iter()
        .find(|record| {
            record.metric.name == "compact.finish"
                && record.metric.dimensions["outcome"] == "failure"
        })
        .unwrap();
    assert_eq!(failure.segment_id, source_segment_id);
    assert_eq!(
        failure.metric.dimensions["failure_category"],
        "summary_missing"
    );
    let encoded = serde_json::to_string(&failure.metric).unwrap();
    assert!(!encoded.contains("missing summary"));

    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(true);
    let error = worker
        .manual_compact_with_cancel(cancel_rx)
        .await
        .unwrap_err();
    assert!(matches!(error, worker::WorkerError::CompactCancelled));
    let metrics = session_metrics::read_session_metrics(worker.store(), session_id).unwrap();
    let cancelled = metrics
        .iter()
        .filter(|record| {
            record.metric.name == "compact.finish"
                && record.metric.dimensions["outcome"] == "cancelled"
        })
        .last()
        .unwrap();
    assert_eq!(cancelled.segment_id, source_segment_id);
    assert_eq!(cancelled.metric.dimensions["failure_category"], "cancelled");
}

#[tokio::test]
async fn pre_run_compact_publishes_runtime_progress_phases() {
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
    let progress = events
        .iter()
        .filter_map(|event| match event {
            Event::CompactionProgress { compaction } => {
                Some(compaction.as_ref().map(|item| item.phase))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        progress,
        vec![
            Some(protocol::CompactionPhase::Preparing),
            Some(protocol::CompactionPhase::Summarizing),
            Some(protocol::CompactionPhase::Committing),
            None,
        ]
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        Event::CompactStart { .. } | Event::CompactDone { .. } | Event::CompactFailed { .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::InternalWorker { worker, .. }
            if matches!(&worker.kind, protocol::InternalWorkerKind::Service { kind } if kind == "compaction")
    )), "compactor activity must be projected through the parent stream");

    let active_entries = worker
        .store()
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert!(!active_entries.iter().any(|entry| matches!(
        entry,
        LogEntry::Extension { domain, .. } if domain == "yoi.compaction"
    )));
    let metrics = session_metrics::read_session_metrics(worker.store(), session_before).unwrap();
    let start = metrics
        .iter()
        .find(|record| record.metric.name == "compact.start")
        .unwrap();
    assert_eq!(start.segment_id, segment_before);
    assert_eq!(start.metric.dimensions["mode"], "automatic");
    assert_eq!(start.metric.dimensions["threshold_policy"], "pre_run");
    let finish = metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "success");
    assert_eq!(finish.metric.correlation_id, start.metric.correlation_id);
}

#[tokio::test]
async fn request_threshold_compact_publishes_runtime_progress() {
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
        text_events_with_usage("b", 50),
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
    assert!(events.iter().any(|event| matches!(
        event,
        Event::CompactionProgress { compaction: Some(progress) }
            if progress.phase == protocol::CompactionPhase::Committing
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::CompactionProgress { compaction: None }))
    );
    let metrics =
        session_metrics::read_session_metrics(worker.store(), worker.session_id()).unwrap();
    let start = metrics
        .iter()
        .find(|record| record.metric.name == "compact.start")
        .unwrap();
    assert_eq!(
        start.metric.dimensions["threshold_policy"],
        "request_threshold"
    );
    let correlation_id = start.metric.correlation_id.as_deref().unwrap();
    let post = metrics
        .iter()
        .find(|record| record.metric.name == "compact.post_request")
        .unwrap();
    assert_eq!(post.metric.correlation_id.as_deref(), Some(correlation_id));
}

#[tokio::test]
async fn pre_run_compact_failure_clears_runtime_progress() {
    // Only the first run has a response. Compaction will run the
    // compact worker which immediately exhausts the mock → failure.
    let client = MockClient::new(vec![single_text_events("hi")]);
    let mut worker = make_worker(client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    worker.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Best-effort: returns Ok(()) even on failure and clears runtime progress.
    worker.try_pre_run_compact().await;

    let events = drain(&mut rx);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::CompactionProgress { compaction: Some(progress) }
            if progress.phase == protocol::CompactionPhase::Preparing
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::CompactionProgress { compaction: None }))
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        Event::CompactStart { .. } | Event::CompactDone { .. } | Event::CompactFailed { .. }
    )));
}

#[tokio::test]
async fn manual_compact_cancel_clears_progress_before_returning_idle() {
    let worker =
        make_worker_with_manifest(POST_RUN_MANIFEST_TOML, BlockingCompactClient::new()).await;
    let runtime_tmp = tempfile::tempdir().unwrap();
    let bash_output_dir = runtime_tmp.path().join("bash-output");
    let (handle, shutdown_receiver) =
        WorkerController::spawn(worker, runtime_tmp.path(), &bash_output_dir)
            .await
            .unwrap();
    let mut rx = handle.subscribe();

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "seed history",
        ))
        .await
        .expect("send seed run");
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for seed run")
                .expect("event"),
            Event::RunEnd {
                result: RunResult::Finished
            }
        ) {
            break;
        }
    }

    let compact = protocol::WorkerCommandEnvelope::for_snapshot(1, &handle.shared_state.snapshot());
    handle
        .send(Method::Compact { command: compact })
        .await
        .expect("send compact");
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for compact start")
                .expect("event"),
            Event::CompactionProgress {
                compaction: Some(_)
            }
        ) {
            break;
        }
    }

    let cancel = protocol::WorkerCommandEnvelope::for_snapshot(2, &handle.shared_state.snapshot());
    handle
        .send(Method::Cancel { command: cancel })
        .await
        .expect("send compact cancel");
    let mut saw_interrupted = false;
    let mut saw_idle = false;
    while !(saw_interrupted && saw_idle) {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for compact cancellation")
            .expect("event")
        {
            Event::CompactionProgress { compaction: None } => {
                saw_interrupted = true;
            }
            Event::WorkerState { snapshot }
                if snapshot.catalog_status() == protocol::WorkerStatus::Idle =>
            {
                assert!(
                    saw_interrupted,
                    "Idle must follow durable Interrupted evidence"
                );
                saw_idle = true;
            }
            _ => {}
        }
    }

    let compact = protocol::WorkerCommandEnvelope::for_snapshot(3, &handle.shared_state.snapshot());
    handle
        .send(Method::Compact { command: compact })
        .await
        .expect("send second compact");
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for second compact start")
                .expect("event"),
            Event::CompactionProgress {
                compaction: Some(_)
            }
        ) {
            break;
        }
    }
    let shutdown =
        protocol::WorkerCommandEnvelope::for_snapshot(4, &handle.shared_state.snapshot());
    handle
        .send(Method::Shutdown { command: shutdown })
        .await
        .expect("send shutdown during compact");
    let mut interrupted_before_shutdown = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for shutdown")
            .expect("event")
        {
            Event::CompactionProgress { compaction: None } => {
                interrupted_before_shutdown = true;
            }
            Event::Shutdown => {
                assert!(
                    interrupted_before_shutdown,
                    "shutdown must await terminal compaction evidence"
                );
                break;
            }
            _ => {}
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), shutdown_receiver)
        .await
        .expect("controller shutdown timeout")
        .expect("shutdown confirmation");
}

#[tokio::test]
async fn controller_compact_method_publishes_progress_and_clear() {
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        write_summary_tool_use_events("manual-summary", "manual compact summary"),
        single_text_events("done"),
        single_text_events("follow-up"),
    ]);
    let worker = make_worker_with_manifest(POST_RUN_MANIFEST_TOML, client).await;
    let runtime_tmp = tempfile::tempdir().unwrap();
    let bash_output_dir = runtime_tmp.path().join("bash-output");
    let (handle, _shutdown) = WorkerController::spawn(worker, runtime_tmp.path(), &bash_output_dir)
        .await
        .unwrap();
    let mut rx = handle.subscribe();

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "seed history",
        ))
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

    let command = protocol::WorkerCommandEnvelope::for_snapshot(1, &handle.shared_state.snapshot());
    handle
        .send(Method::Compact { command })
        .await
        .expect("send compact");
    let mut saw_start = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for compact events")
            .expect("event")
        {
            Event::CompactionProgress {
                compaction: Some(_),
            } => saw_start = true,
            Event::CompactionProgress { compaction: None } => {
                break;
            }
            _ => {}
        }
    }

    assert!(saw_start, "manual compact should emit CompactStart");
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "run after compact",
        ))
        .await
        .expect("send follow-up run");
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for follow-up run")
            .expect("event")
        {
            Event::RunEnd {
                result: RunResult::Finished,
            } => break,
            _ => {}
        }
    }
    assert_eq!(
        handle.shared_state.catalog_status(),
        protocol::WorkerStatus::Idle,
        "successful manual compaction must release the execution fence"
    );
    let command = protocol::WorkerCommandEnvelope::for_snapshot(2, &handle.shared_state.snapshot());
    let _ = handle.send(Method::Shutdown { command }).await;
}
