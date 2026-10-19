//! Compact lifecycle `Event` broadcasting.
//!
//! Covers three paths:
//! - `try_pre_run_compact` success → `CompactStart + CompactDone`
//! - `try_pre_run_compact` failure → `CompactStart + CompactFailed`
//! - mid-turn `do_compact_and_resume` success → `CompactStart + CompactDone`
//!   (driven by `compact_request_threshold` → `PreRequestAction::Yield`)

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agen::Engine;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent, UsageEvent};
use agen::llm_client::types::Item;
use agen::llm_client::{ClientError, LlmClient, Request};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use protocol::{Event, Method, RunResult};
use session_store::{
    CombinedStore, FsStore, FsWorkerStore, LogEntry, Store, WorkerMetadata, WorkerMetadataStore,
    WorkerStoreError,
};
use tokio::sync::broadcast;

use worker::{Worker, WorkerController, WorkerError};

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
            system_input: None,
            entry_id: session_store::LoggedSessionHistoryEntryId::new(),
            origin: session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
        },
    }
}

type CallObserver = Arc<dyn Fn(usize) + Send + Sync>;

#[derive(Clone)]
struct MockClient {
    responses: Arc<Vec<Vec<LlmEvent>>>,
    call_count: Arc<AtomicUsize>,
    observer: Arc<Mutex<Option<CallObserver>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MockClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
            observer: Arc::new(Mutex::new(None)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn set_observer(&self, observer: impl Fn(usize) + Send + Sync + 'static) {
        *self.observer.lock().unwrap() = Some(Arc::new(observer));
    }
}

#[async_trait]
impl LlmClient for MockClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.requests.lock().unwrap().push(request);
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if let Some(observer) = self.observer.lock().unwrap().clone() {
            observer(count);
        }
        if count >= self.responses.len() {
            return Err(ClientError::Config("mock client exhausted".into()));
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

#[derive(Clone)]
struct PauseAfterUsageClient {
    responses: Arc<Vec<Vec<LlmEvent>>>,
    call_count: Arc<AtomicUsize>,
    hanging_call: usize,
}

impl PauseAfterUsageClient {
    fn new(responses: Vec<Vec<LlmEvent>>, hanging_call: usize) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
            hanging_call,
        }
    }
}

#[async_trait]
impl LlmClient for PauseAfterUsageClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        let events = self
            .responses
            .get(count)
            .cloned()
            .ok_or_else(|| ClientError::Config("pause mock client exhausted".into()))?;
        let finite = futures::stream::iter(events.into_iter().map(Ok));
        if count == self.hanging_call {
            Ok(Box::pin(finite.chain(futures::stream::pending())))
        } else {
            Ok(Box::pin(finite))
        }
    }
}

#[derive(Clone)]
struct GrowTool;

#[async_trait]
impl Tool for GrowTool {
    async fn execute(
        &self,
        _input: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            summary: "grew context".into(),
            content: Some("x".repeat(400_000)),
            attachments: Vec::new(),
        })
    }
}

fn grow_tool_definition() -> ToolDefinition {
    Arc::new(|| {
        (
            ToolMeta::new("grow")
                .description("test tool that grows request context")
                .input_schema(serde_json::json!({"type": "object"})),
            Arc::new(GrowTool) as Arc<dyn Tool>,
        )
    })
}

fn grow_tool_use_events(call_id: &str, input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::tool_use_start(0, call_id, "grow"),
        LlmEvent::tool_input_delta(0, "{}"),
        LlmEvent::tool_use_stop(0),
        LlmEvent::usage(input_tokens, 1),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
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

async fn make_worker_with_manifest_and_store<C>(
    manifest_toml: &str,
    client: C,
) -> (Worker<C, TestStore>, TestStore)
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

    let mut engine =
        Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(client);
    engine.register_tool(grow_tool_definition());
    let observed_store = store.clone();
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
    worker.attach_log_writer(Arc::new(worker.log_writer_handle()));
    (worker, observed_store)
}

async fn make_worker_with_manifest<C>(manifest_toml: &str, client: C) -> Worker<C, TestStore>
where
    C: LlmClient + Clone + Send + Sync + 'static,
{
    make_worker_with_manifest_and_store(manifest_toml, client)
        .await
        .0
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
async fn result_context_warning_preserves_auto_read_and_continues() {
    // The auto-read contents alone exceed the warning threshold in the middle
    // case. Disabled and larger thresholds must remain silent.
    for (threshold, expect_warning) in [(0, false), (1_000, true), (50_000, false)] {
        let mark_input = serde_json::json!({ "file_path": "context.txt" }).to_string();
        let client = MockClient::new(vec![
            single_text_events("retained response"),
            write_summary_tool_use_events("summary-1", "replacement summary"),
            vec![
                LlmEvent::tool_use_start(0, "mark-1", "mark_read_required"),
                LlmEvent::tool_input_delta(0, mark_input),
                LlmEvent::tool_use_stop(0),
                LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ],
            single_text_events("done"),
            text_events_with_usage("continued on replacement", 4_000),
        ]);
        let calls = Arc::clone(&client.call_count);
        let manifest = MANUAL_ONLY_MANIFEST_TOML.replace(
            "[compaction]",
            &format!("[compaction]\nresult_context_max_tokens = {threshold}"),
        );
        let (mut worker, store) = make_worker_with_manifest_and_store(&manifest, client).await;
        let file_contents = "load-bearing file contents\n".repeat(300);
        let file = worker
            .filesystem_authority()
            .as_local()
            .unwrap()
            .root
            .join("context.txt");
        std::fs::write(file, &file_contents).unwrap();
        let (tx, mut rx) = broadcast::channel::<Event>(64);
        worker.attach_alerter(worker::Alerter::new(tx));
        worker.run_text("retained input").await.unwrap();
        let source_segment = worker.segment_id();
        let _ = drain(&mut rx);

        let replacement_segment = worker.compact(100).await.unwrap();
        assert_ne!(replacement_segment, source_segment);
        assert_eq!(worker.segment_id(), replacement_segment);
        assert_eq!(
            WorkerMetadataStore::read_by_name(&store, "test-worker")
                .unwrap()
                .unwrap()
                .active
                .unwrap()
                .segment_id,
            Some(replacement_segment)
        );
        let system_texts = system_texts_in_sink_session_start(&worker);
        assert!(
            system_texts
                .iter()
                .any(|text| text.contains("replacement summary"))
        );
        assert!(
            system_texts
                .iter()
                .any(|text| text.contains(&file_contents))
        );
        let result_tokens = agen::token_counter::total_tokens(&worker.history(), &[]).tokens;
        assert!(result_tokens > 1_000 && result_tokens < 50_000);
        assert!(worker.history().iter().any(|item| {
            matches!(item, Item::Message { content, .. }
                if content.iter().any(|part| part.as_text().contains("retained response")))
        }));
        let warnings = drain(&mut rx)
            .into_iter()
            .filter(|event| {
                matches!(event, Event::Alert(alert)
                if alert.level == protocol::AlertLevel::Warn
                    && alert.source == protocol::AlertSource::Compactor
                    && alert.message.contains("compacted result context"))
            })
            .count();
        assert_eq!(warnings, usize::from(expect_warning));

        worker.run_text("continue").await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        assert_eq!(worker.segment_id(), replacement_segment);
        let metrics = session_metrics::read_session_metrics(&store, worker.session_id()).unwrap();
        let finish = metrics
            .iter()
            .find(|record| record.metric.name == "compact.finish")
            .unwrap();
        assert_eq!(finish.metric.dimensions["outcome"], "succeeded");
        let auto_read = metrics
            .iter()
            .find(|record| record.metric.name == "compact.auto_read_tokens")
            .unwrap();
        assert!(auto_read.metric.value.unwrap() > 1_000.0);
    }
}

#[tokio::test]
async fn result_context_warning_does_not_stop_automatic_resume() {
    let client = MockClient::new(vec![
        text_events_with_usage("seed", 100_000),
        write_summary_tool_use_events("summary-1", "replacement summary"),
        single_text_events("done"),
        text_events_with_usage("continued on replacement", 1_000),
    ]);
    let calls = Arc::clone(&client.call_count);
    let manifest = MID_TURN_MANIFEST_TOML.replace(
        "compact_request_threshold = 100",
        "compact_request_threshold = 50000\nresult_context_max_tokens = 1",
    );
    let (mut worker, store) = make_worker_with_manifest_and_store(&manifest, client).await;
    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_alerter(worker::Alerter::new(tx));
    worker.run_text("seed input").await.unwrap();
    let source_segment = worker.segment_id();

    worker.run_text("continue").await.unwrap();
    assert_ne!(worker.segment_id(), source_segment);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|event| matches!(event, Event::Alert(alert)
        if alert.level == protocol::AlertLevel::Warn
            && alert.source == protocol::AlertSource::Compactor
            && alert.message.contains("compacted result context")))
    );
    let entries = store
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert!(entries.iter().any(|entry| matches!(
        entry,
        LogEntry::RunResumed {
            source: protocol::RunResumeSource::Compaction,
            ..
        }
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        LogEntry::RunCompleted {
            result: agen::EngineResult::Finished,
            ..
        }
    )));
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LogEntry::RunErrored { .. }))
    );
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
    assert_eq!(finish.metric.dimensions["outcome"], "failed");
    assert_eq!(finish.metric.value, Some(1.0));
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
    assert_eq!(finish.metric.dimensions["outcome"], "failed");
    assert_eq!(finish.metric.value, Some(1.0));
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
    assert_eq!(starts[0].metric.dimensions["trigger"], "request_threshold");
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
    assert_eq!(finish.metric.dimensions["outcome"], "succeeded");
    assert_eq!(finish.metric.value, Some(1.0));
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
    assert_eq!(value("compact.input_tokens"), 150);
    assert_eq!(value("compact.cache_read_tokens"), 13);
    assert_eq!(value("compact.cache_creation_tokens"), 7);
    assert_eq!(value("compact.output_tokens"), 30);
    assert_eq!(value("compact.requests"), 2);
    assert!(value("compact.tool_calls") >= 1);
    assert!(value("compact.turns") >= 2);
    assert!(value("compact.duration_ms") <= u64::MAX);
    let cost = metrics
        .iter()
        .find(|record| record.metric.name == "compact.cost_usd")
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
    assert_eq!(start.metric.dimensions["trigger"], "manual");
    assert_eq!(start.metric.dimensions["threshold_policy"], "manual");
    let finish = metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "succeeded");
    assert_eq!(finish.metric.value, Some(1.0));
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
                && record.metric.dimensions["outcome"] == "failed"
        })
        .unwrap();
    assert_eq!(failure.segment_id, source_segment_id);
    assert_eq!(failure.metric.value, Some(1.0));
    assert_eq!(
        failure.metric.dimensions["failure_category"],
        "summary_missing"
    );
    let encoded = serde_json::to_string(&failure.metric).unwrap();
    assert!(!encoded.contains("missing summary"));

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    cancel_tx.send(true).unwrap();
    let error = worker
        .manual_compact_with_cancel(cancel_rx)
        .await
        .unwrap_err();
    assert!(
        matches!(error, worker::WorkerError::CompactCancelled),
        "unexpected cancellation error: {error:?}"
    );
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
    assert_eq!(cancelled.metric.value, Some(1.0));
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
    worker.try_pre_run_compact().await.unwrap();
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
    assert_eq!(start.metric.dimensions["trigger"], "pre_run");
    assert_eq!(start.metric.dimensions["threshold_policy"], "pre_run");
    let finish = metrics
        .iter()
        .find(|record| record.metric.name == "compact.finish")
        .unwrap();
    assert_eq!(finish.metric.dimensions["outcome"], "succeeded");
    assert_eq!(finish.metric.value, Some(1.0));
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
        text_events_with_usage("a", 100_000),
        write_summary_tool_use_events("call-1", "summary"),
        single_text_events("done"),
        text_events_with_usage("b", 50),
    ]);
    let manifest = MID_TURN_MANIFEST_TOML.replace(
        "compact_request_threshold = 100",
        "compact_request_threshold = 50000",
    );
    let mut worker = make_worker_with_manifest(&manifest, client).await;

    let (tx, mut rx) = broadcast::channel::<Event>(64);
    worker.attach_working_event_tx(tx);

    // First run populates usage_history above the request threshold.
    worker.run_text("first").await.unwrap();
    let _ = drain(&mut rx);

    // Second run: pre_llm_request yields immediately, Engine returns
    // Yielded, handle_worker_result routes into do_compact_and_resume.
    let source_segment = worker.segment_id();
    let store = worker.store().clone();
    worker.run_text("second").await.unwrap();
    let replacement_segment = worker.segment_id();
    assert_ne!(replacement_segment, source_segment);

    let source_entries = store.read_all(worker.session_id(), source_segment).unwrap();
    assert!(source_entries.iter().any(|entry| matches!(
        entry,
        LogEntry::RunYielded {
            reason: protocol::RunYieldReason::Compaction,
            ..
        }
    )));
    assert!(!source_entries.iter().any(|entry| matches!(
        entry,
        LogEntry::RunCompleted {
            result: agen::EngineResult::Yielded,
            ..
        }
    )));
    let replacement_entries = store
        .read_all(worker.session_id(), replacement_segment)
        .unwrap();
    let resumed = replacement_entries
        .iter()
        .position(|entry| {
            matches!(
                entry,
                LogEntry::RunResumed {
                    source: protocol::RunResumeSource::Compaction,
                    ..
                }
            )
        })
        .expect("compaction resume transition");
    let completed = replacement_entries
        .iter()
        .position(|entry| {
            matches!(
                entry,
                LogEntry::RunCompleted {
                    result: agen::EngineResult::Finished,
                    ..
                }
            )
        })
        .expect("logical run completion");
    assert!(resumed < completed);

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
    assert_eq!(start.metric.dimensions["trigger"], "request_threshold");
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
async fn completed_post_compact_usage_rearms_within_one_tool_loop() {
    let client = MockClient::new(vec![
        text_events_with_usage("seed", 100_000),
        write_summary_tool_use_events("compact-1", "first summary"),
        single_text_events("done"),
        grow_tool_use_events("grow-1", 1_000),
        write_summary_tool_use_events("compact-2", "second summary"),
        single_text_events("done"),
        text_events_with_usage("finished", 800),
    ]);
    let call_count = Arc::clone(&client.call_count);
    let observed_client = client.clone();
    let manifest = MID_TURN_MANIFEST_TOML.replace(
        "compact_request_threshold = 100",
        "compact_threshold = 80000\ncompact_request_threshold = 50000",
    );
    let (mut worker, store) = make_worker_with_manifest_and_store(&manifest, client).await;
    let session_id = worker.session_id();
    let durable_at_next_request = Arc::new(AtomicBool::new(false));
    let observed_barrier = Arc::clone(&durable_at_next_request);
    let observed_store = store.clone();
    observed_client.set_observer(move |call_index| {
        if call_index != 4 {
            return;
        }
        let usage_is_durable = observed_store
            .list_segments(session_id)
            .unwrap()
            .into_iter()
            .flat_map(|segment_id| observed_store.read_all(session_id, segment_id).unwrap())
            .any(|entry| {
                matches!(
                    entry,
                    LogEntry::LlmUsage {
                        input_total_tokens: 1_000,
                        ..
                    }
                )
            });
        assert!(
            usage_is_durable,
            "the first post-compact usage must be durable before the next provider request"
        );
        observed_barrier.store(true, Ordering::SeqCst);
    });

    worker.run_text("seed input").await.unwrap();
    worker
        .run_text("continue through two compactions")
        .await
        .expect("a successful post-compact request must rearm before the tool-loop threshold");

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        7,
        "both compactor cycles and both normal post-compact requests must run"
    );
    assert!(durable_at_next_request.load(Ordering::SeqCst));

    let metrics = session_metrics::read_session_metrics(&store, session_id).unwrap();
    let compact_starts = metrics
        .iter()
        .filter(|record| record.metric.name == "compact.start")
        .collect::<Vec<_>>();
    assert_eq!(compact_starts.len(), 2);
    assert!(compact_starts.iter().any(|record| {
        record.metric.dimensions.get("trigger").map(String::as_str) == Some("pre_run")
    }));
    assert!(compact_starts.iter().any(|record| {
        record.metric.dimensions.get("trigger").map(String::as_str) == Some("request_threshold")
    }));
    let compact_posts = metrics
        .iter()
        .filter(|record| record.metric.name == "compact.post_request")
        .collect::<Vec<_>>();
    assert_eq!(compact_posts.len(), 2);
    assert_ne!(
        compact_posts[0].metric.correlation_id,
        compact_posts[1].metric.correlation_id
    );

    let usage = store
        .list_segments(session_id)
        .unwrap()
        .into_iter()
        .flat_map(|segment_id| store.read_all(session_id, segment_id).unwrap())
        .filter_map(|entry| match entry {
            LogEntry::LlmUsage {
                input_total_tokens, ..
            } => Some(input_total_tokens),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(usage.iter().filter(|tokens| **tokens == 1_000).count(), 1);
    assert_eq!(usage.iter().filter(|tokens| **tokens == 800).count(), 1);

    let restored = session_store::restore(&store, session_id, worker.segment_id()).unwrap();
    assert_eq!(restored.usage_history.len(), 1);
    assert_eq!(restored.usage_history[0].input_total_tokens, 800);
}

#[tokio::test]
async fn post_compact_pause_bills_partial_without_rearming_or_consuming_correlation() {
    let client = PauseAfterUsageClient::new(
        vec![
            text_events_with_usage("seed", 100_000),
            write_summary_tool_use_events("manual-compact", "pause test summary"),
            single_text_events("done"),
            vec![
                LlmEvent::text_block_start(0),
                LlmEvent::text_delta(0, "partial"),
                LlmEvent::usage(90, 2),
            ],
            grow_tool_use_events("grow-after-resume", 80),
            write_summary_tool_use_events("automatic-compact", "rearmed summary"),
            single_text_events("done"),
            text_events_with_usage("finished", 50),
        ],
        3,
    );
    let call_count = Arc::clone(&client.call_count);
    let manifest = MID_TURN_MANIFEST_TOML.replace(
        "compact_request_threshold = 100",
        "compact_threshold = 80000\ncompact_request_threshold = 50000",
    );
    let (worker, store) = make_worker_with_manifest_and_store(&manifest, client).await;
    let session_id = worker.session_id();

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
        .unwrap();
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for seed")
                .expect("event"),
            Event::RunEnd {
                result: RunResult::Finished
            }
        ) {
            break;
        }
    }
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "pause the first post-compact request",
        ))
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while call_count.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("post-compact provider request should start");
    handle
        .send(Method::Pause {
            command: protocol::WorkerCommandEnvelope::new(1),
        })
        .await
        .unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for pause")
            .expect("event");
        if matches!(
            event,
            Event::RunEnd {
                result: RunResult::Paused
            }
        ) {
            break;
        }
    }

    let paused_usage = store
        .list_segments(session_id)
        .unwrap()
        .into_iter()
        .flat_map(|segment_id| store.read_all(session_id, segment_id).unwrap())
        .filter_map(|entry| match entry {
            LogEntry::LlmUsage {
                input_total_tokens, ..
            } => Some(input_total_tokens),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paused_usage.iter().filter(|tokens| **tokens == 90).count(),
        1
    );
    let paused_metrics = session_metrics::read_session_metrics(&store, session_id).unwrap();
    assert!(
        paused_metrics
            .iter()
            .all(|record| record.metric.name != "compact.post_request"),
        "partial Pause usage must not consume compact correlation or rearm"
    );

    handle
        .send(Method::Resume {
            command: protocol::WorkerCommandEnvelope::new(2),
        })
        .await
        .unwrap();
    loop {
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for resume completion")
                .expect("event"),
            Event::RunEnd {
                result: RunResult::Finished
            }
        ) {
            break;
        }
    }

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        8,
        "the completed resumed request must rearm and permit request-threshold compaction"
    );
    let metrics = session_metrics::read_session_metrics(&store, session_id).unwrap();
    let compact_starts = metrics
        .iter()
        .filter(|record| record.metric.name == "compact.start")
        .collect::<Vec<_>>();
    assert_eq!(compact_starts.len(), 2);
    let pre_run_correlation = compact_starts
        .iter()
        .find(|record| {
            record.metric.dimensions.get("trigger").map(String::as_str) == Some("pre_run")
        })
        .and_then(|record| record.metric.correlation_id.as_deref())
        .expect("pre-run automatic compact correlation");
    let pre_run_posts = metrics
        .iter()
        .filter(|record| {
            record.metric.name == "compact.post_request"
                && record.metric.correlation_id.as_deref() == Some(pre_run_correlation)
        })
        .collect::<Vec<_>>();
    assert_eq!(pre_run_posts.len(), 1);
    assert_eq!(
        pre_run_posts[0].metric.dimensions["input_total_tokens"],
        "80"
    );
    assert!(metrics.iter().all(|record| {
        record.metric.name != "compact.post_request"
            || record.metric.dimensions["input_total_tokens"] != "90"
    }));

    handle
        .send(Method::Shutdown {
            command: protocol::WorkerCommandEnvelope::new(3),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), shutdown_receiver)
        .await
        .expect("controller shutdown timeout")
        .expect("shutdown confirmation");
}

#[tokio::test]
async fn compacted_context_above_request_threshold_fails_before_provider_request() {
    let client = MockClient::new(vec![
        text_events_with_usage("seed", 1000),
        write_summary_tool_use_events("call-1", "still too large after compaction"),
        single_text_events("done"),
        single_text_events("must not be requested"),
    ]);
    let call_count = Arc::clone(&client.call_count);
    let mut worker = make_worker_with_manifest(MID_TURN_MANIFEST_TOML, client).await;
    worker.run_text("first").await.unwrap();

    let error = worker
        .run_text("second")
        .await
        .expect_err("unsafe compacted context must fail closed");

    assert!(
        matches!(
            error,
            WorkerError::RunFailureAlreadyDelivered { ref source }
                if matches!(**source, WorkerError::CompactThrash)
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        3,
        "the provider must receive only the seed and compaction requests"
    );
    let entries = worker
        .store()
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap();
    assert!(entries.iter().any(|entry| {
        matches!(
            entry,
            LogEntry::RunErrored {
                interrupted: false,
                failure: Some(protocol::RunFailureKind::Compaction),
                message,
                ..
            } if message.contains("mid-run compaction could not continue")
        )
    }));
}

#[tokio::test]
async fn controller_does_not_repeat_a_persisted_mid_run_compaction_failure() {
    let client = MockClient::new(vec![
        grow_tool_use_events("grow-before-compaction", 1_000),
        single_text_events("missing summary"),
        single_text_events("still missing summary"),
    ]);
    let (worker, store) = make_worker_with_manifest_and_store(MID_TURN_MANIFEST_TOML, client).await;
    let session_id = worker.session_id();
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
            "trigger failing compaction",
        ))
        .await
        .unwrap();
    let mut observed = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("event");
            let idle = matches!(
                event,
                Event::WorkerState {
                    snapshot: protocol::WorkerStateSnapshot {
                        state: protocol::WorkerState::Idle,
                        ..
                    }
                }
            );
            observed.push(event);
            if idle {
                break;
            }
        }
    })
    .await
    .expect("timeout waiting for failed compaction and Idle");
    tokio::task::yield_now().await;
    while let Ok(event) = rx.try_recv() {
        observed.push(event);
    }

    let delivered = handle
        .committed_entries()
        .into_iter()
        .filter(|entry| matches!(entry, LogEntry::RunErrored { .. }))
        .count();
    assert_eq!(delivered, 1, "the durable terminal is the only delivery");
    assert!(
        observed
            .iter()
            .all(|event| !matches!(event, Event::Error { .. })),
        "Controller must not add a generic Error after durable delivery"
    );
    assert!(
        observed.iter().all(|event| !matches!(
            event,
            Event::Alert(protocol::Alert {
                level: protocol::AlertLevel::Error,
                ..
            })
        )),
        "mid-run terminal failure must not add an Error Alert"
    );

    let terminal_count = store
        .list_segments(session_id)
        .unwrap()
        .into_iter()
        .flat_map(|segment_id| store.read_all(session_id, segment_id).unwrap())
        .filter(|entry| matches!(entry, LogEntry::RunErrored { .. }))
        .count();
    assert_eq!(terminal_count, 1);

    handle
        .send(Method::Shutdown {
            command: protocol::WorkerCommandEnvelope::new(1),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), shutdown_receiver)
        .await
        .expect("controller shutdown timeout")
        .expect("shutdown confirmation");
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
    worker.try_pre_run_compact().await.unwrap();

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
    let (worker, observed_store) =
        make_worker_with_manifest_and_store(POST_RUN_MANIFEST_TOML, BlockingCompactClient::new())
            .await;
    let source_session = worker.session_id();
    let source_segment = worker.segment_id();
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

    let compact = protocol::WorkerCommandEnvelope::new(1);
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

    let cancel = protocol::WorkerCommandEnvelope::new(2);
    handle
        .send(Method::Cancel { command: cancel })
        .await
        .expect("send compact cancel");
    let mut saw_interrupted = false;
    let mut saw_idle = false;
    let mut saw_cancel_error = false;
    while !(saw_interrupted && saw_idle) {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for compact cancellation")
            .expect("event")
        {
            Event::CompactionProgress { compaction: None } => {
                saw_interrupted = true;
            }
            Event::Error { .. }
            | Event::Alert(protocol::Alert {
                level: protocol::AlertLevel::Error,
                ..
            }) => saw_cancel_error = true,
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
    assert!(
        !saw_cancel_error,
        "intentional manual cancellation must not be reported as an Error"
    );

    let compact = protocol::WorkerCommandEnvelope::new(3);
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
    let completion_request_id = "subworker-completion-during-compact";
    handle
        .send(Method::NotifyTracked {
            notification_request_id: completion_request_id.into(),
            message: "subworker completed".into(),
            source: protocol::AuthenticatedInputSource::UntrustedWire,
        })
        .await
        .expect("send completion notification during compact");
    let shutdown = protocol::WorkerCommandEnvelope::new(4);
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

    let persisted = observed_store
        .read_all(source_session, source_segment)
        .expect("read source Segment after shutdown");
    assert!(persisted.iter().any(|entry| {
        matches!(
            entry,
            LogEntry::Extension { domain, payload, .. }
                if domain == "worker.pending_activations.v1"
                    && payload.to_string().contains(completion_request_id)
        )
    }));
}

#[tokio::test]
async fn controller_compact_method_publishes_progress_and_clear() {
    let client = MockClient::new(vec![
        text_events_with_usage("hi", 1000),
        write_summary_tool_use_events("manual-summary", "manual compact summary"),
        single_text_events("done"),
        single_text_events("follow-up"),
    ]);
    let worker = make_worker_with_manifest(MANUAL_ONLY_MANIFEST_TOML, client).await;
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

    let command = protocol::WorkerCommandEnvelope::new(1);
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
    let command = protocol::WorkerCommandEnvelope::new(2);
    let _ = handle.send(Method::Shutdown { command }).await;
}

const INITIAL_SUMMARY_HEADING: &str = "## Initial compaction summary (full text)\n";
const OVERVIEW_HEADING: &str = "## Conversation overview/index\n";
const SUMMARY_MARKER: &str = "[Compacted context summary]\n\n";

// These tests prove transport and durable replacement policy, not whether a live
// model preserves meaning. All write_summary outputs below are predetermined.
struct SummaryInputFixture {
    worker: Worker<MockClient, TestStore>,
    _store_dir: tempfile::TempDir,
}

impl SummaryInputFixture {
    async fn new(manifest_toml: &str, client: MockClient) -> Self {
        Self::with_manifest(
            worker::WorkerManifest::from_toml(manifest_toml).unwrap(),
            client,
        )
        .await
    }

    async fn with_journal(
        manifest_toml: &str,
        client: MockClient,
        host: Arc<CompactionJournalHost>,
    ) -> Self {
        let mut manifest = worker::WorkerManifest::from_toml(manifest_toml).unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let mut fixture = Self::with_manifest(manifest, client).await;
        fixture.worker.bind_subjektiv_host(Some(host));
        fixture
    }

    async fn with_manifest(manifest: worker::WorkerManifest, client: MockClient) -> Self {
        let store_dir = tempfile::tempdir().unwrap();
        let store = CombinedStore::new(
            FsStore::new(store_dir.path()).unwrap(),
            FsWorkerStore::new(store_dir.path().join("pods")).unwrap(),
        );
        let engine =
            Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                client,
            );
        let mut worker = Worker::new(
            manifest,
            engine,
            store,
            worker::WorkerWorkspaceContext::no_workspace(),
            worker::WorkerFilesystemAuthority::None,
            worker::Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        worker.attach_log_writer(Arc::new(worker.log_writer_handle()));
        Self {
            worker,
            _store_dir: store_dir,
        }
    }

    // Seed the persisted boundary, then exercise the public rewind API's shared
    // typed history restore path. No production client or global allocation is
    // needed. This is restoration coverage, not a cold Runtime restart test.
    async fn restore_seed(
        &mut self,
        history: Vec<session_store::LoggedHistoryEntry>,
        compacted: bool,
    ) {
        self.restore_seed_with_system(history, compacted, vec![])
            .await;
    }

    async fn restore_seed_with_system(
        &mut self,
        history: Vec<session_store::LoggedHistoryEntry>,
        compacted: bool,
        systems: Vec<session_store::LoggedSystemHistoryEntry>,
    ) {
        self.worker
            .run_text("fixture rewind boundary")
            .await
            .unwrap();
        let session_id = self.worker.session_id();
        let segment_id = self.worker.segment_id();
        let mut records = self
            .worker
            .store()
            .read_all(session_id, segment_id)
            .unwrap();
        let start = records
            .iter_mut()
            .find(|entry| matches!(entry, LogEntry::AnnotatedSegmentStart { .. }))
            .unwrap();
        match start {
            LogEntry::AnnotatedSegmentStart {
                history: seed,
                compacted_from,
                ..
            } => {
                *seed = history;
                *compacted_from = compacted.then(|| session_store::SegmentOrigin {
                    segment_id: session_store::new_segment_id(),
                    at_turn_index: 0,
                });
            }
            _ => unreachable!(),
        }
        records.splice(
            1..1,
            systems
                .into_iter()
                .map(|entry| LogEntry::AnnotatedSystemItem {
                    ts: 1,
                    entry,
                    extensions: vec![],
                }),
        );
        self.worker
            .store()
            .create_segment(session_id, segment_id, &records)
            .unwrap();
        let snapshot = session_store::public_snapshot::project_current_session_snapshot(&records);
        let target = snapshot
            .entries
            .iter()
            .rev()
            .find(|entry| {
                matches!(
                    entry.data,
                    protocol::SessionSnapshotEntryData::UserInput { .. }
                )
            })
            .expect("fixture rewind boundary must remain a public user input");
        self.worker
            .rewind_to(target.entry_id.clone())
            .await
            .unwrap();
    }
}

fn summary_entry(
    item: Item,
    origin: session_store::LoggedSessionHistoryOrigin,
) -> session_store::LoggedHistoryEntry {
    let mut entry = annotated(item);
    entry.metadata.origin = origin;
    entry
}

fn long_summary(label: &str) -> String {
    format!(
        "## Completed Tasks\n{label}: investigated the transport boundary.\n\n\
         ## Active Task\nKeep the original user's completion conditions.\n{}\n\
         ## Key Decisions\nDo not infer provenance from pasted markers.\n\n\
         ## User Directives\n{label}_LOAD_BEARING_SENTINEL_AT_END\n\n\
         ## Current Work\nContinue the requested implementation.",
        "Historical constraints must remain explicit, including the original acceptance criteria.\n"
            .repeat(70)
    )
}

fn recent_history(count: usize) -> Vec<session_store::LoggedHistoryEntry> {
    (0..count)
        .flat_map(|index| {
            [
                annotated(Item::user_message(format!(
                    "recent-user-{index}: {}",
                    "recent progress is not a replacement for the original directive. ".repeat(40)
                ))),
                annotated(Item::assistant_message(format!("recent-assistant-{index}"))),
            ]
        })
        .collect()
}

fn first_compactor_input(request: &Request) -> String {
    request
        .items
        .iter()
        .find_map(|item| match item {
            Item::Message {
                role: agen::Role::User,
                ..
            } => item
                .as_text()
                .filter(|text| text.contains(OVERVIEW_HEADING)),
            _ => None,
        })
        .expect("the first compactor request must carry its overview input")
        .to_string()
}

fn assert_full_initial_summary(input: &str, summary: &str) {
    let heading_at = input
        .find(INITIAL_SUMMARY_HEADING)
        .expect("full summary heading");
    let overview_at = input.find(OVERVIEW_HEADING).expect("overview heading");
    let summary_at = heading_at + INITIAL_SUMMARY_HEADING.len();
    assert!(summary_at < overview_at);
    assert!(
        input[summary_at..].starts_with(summary),
        "summary must be verbatim"
    );
    assert_eq!(
        input.matches(summary).count(),
        1,
        "summary must not be duplicated"
    );
    assert!(!input[overview_at..].contains(summary));
    let opening = summary.chars().take(100).collect::<String>();
    assert!(
        !input[overview_at..].contains(&opening),
        "overview must not carry even a clipped summary"
    );
    assert!(!input[overview_at..].contains("LOAD_BEARING_SENTINEL_AT_END"));
}

fn replacement_seed(
    worker: &Worker<MockClient, TestStore>,
) -> Vec<session_store::LoggedHistoryEntry> {
    worker
        .store()
        .read_all(worker.session_id(), worker.segment_id())
        .unwrap()
        .into_iter()
        .find_map(|entry| match entry {
            LogEntry::AnnotatedSegmentStart { history, .. } => Some(history),
            _ => None,
        })
        .expect("replacement SegmentStart")
}

#[tokio::test]
async fn full_initial_summary_survives_small_overview_budgets_and_both_retention_extremes() {
    use session_store::LoggedSessionHistoryOrigin::CompactionSummary;

    let inherited = long_summary("original");
    assert!(inherited.chars().count() > 4_000);
    for retained in [0, 1_000_000] {
        for (warning, deadline) in [(0, 0), (1, 0), (1, 1)] {
            let manifest = MANUAL_ONLY_MANIFEST_TOML.replace(
                "[compaction]",
                &format!(
                    "[compaction]\noverview_target_tokens = 1\noverview_warning_tokens = {warning}\noverview_deadline_tokens = {deadline}"
                ),
            );
            let client = MockClient::new(vec![
                single_text_events("fixture"),
                write_summary_tool_use_events("replace", "new summary"),
                single_text_events("done"),
            ]);
            let observed = client.clone();
            let mut fixture = SummaryInputFixture::new(&manifest, client).await;
            let mut seed = vec![summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}{inherited}")),
                CompactionSummary,
            )];
            let recent = recent_history(40);
            seed.extend(recent.clone());
            fixture.restore_seed(seed, true).await;
            let (tx, mut rx) = broadcast::channel::<Event>(64);
            fixture.worker.attach_alerter(worker::Alerter::new(tx));

            fixture.worker.compact(retained).await.unwrap();

            let requests = observed.requests();
            assert_eq!(requests.len(), 3);
            let input = first_compactor_input(&requests[1]);
            assert_full_initial_summary(&input, &inherited);
            let alerts = drain(&mut rx);
            let has_alert = |snippet: &str| {
                alerts.iter().any(|event| {
                    matches!(event, Event::Alert(alert)
                    if alert.source == protocol::AlertSource::Compactor
                        && alert.level == protocol::AlertLevel::Warn
                        && alert.message.contains(snippet))
                })
            };
            assert_eq!(has_alert("larger than expected"), warning != 0);
            assert_eq!(has_alert("using coarse fallback"), deadline != 0);
            let replacement = replacement_seed(&fixture.worker);
            assert_eq!(
                replacement
                    .iter()
                    .filter(|entry| entry.metadata.origin == CompactionSummary)
                    .count(),
                1
            );
            assert!(replacement.iter().all(|entry| {
                !Item::from(entry.item.clone())
                    .as_text()
                    .is_some_and(|text| text.contains(&inherited))
            }));
            for entry in &recent {
                assert_eq!(
                    replacement
                        .iter()
                        .any(|retained_entry| retained_entry == entry),
                    retained != 0,
                    "ordinary retained entries must keep their item and metadata"
                );
            }
            let metrics = session_metrics::read_session_metrics(
                fixture.worker.store(),
                fixture.worker.session_id(),
            )
            .unwrap();
            let overview_tokens = metrics
                .iter()
                .find(|record| record.metric.name == "compact.overview_tokens")
                .and_then(|record| record.metric.value)
                .unwrap();
            assert!(
                overview_tokens < inherited.len().div_ceil(4) as f64,
                "overview accounting must exclude the inherited full summary"
            );
        }
    }
}

#[tokio::test]
async fn pasted_markers_and_other_derived_context_remain_ordinary_overview_material() {
    use session_store::LoggedSessionHistoryOrigin::{
        CompactionSummary, DerivedSummary, LegacyUnknown,
    };

    for retained in [0, 1_000_000] {
        let client = MockClient::new(vec![
            single_text_events("fixture"),
            write_summary_tool_use_events("replace", "replacement"),
            single_text_events("done"),
        ]);
        let observed = client.clone();
        let mut fixture = SummaryInputFixture::new(MANUAL_ONLY_MANIFEST_TOML, client).await;
        let ordinary = vec![
            summary_entry(
                Item::system_message("[Session TaskStore snapshot]\nactive task"),
                DerivedSummary,
            ),
            summary_entry(
                Item::system_message("auto-read file source material"),
                DerivedSummary,
            ),
            summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}pasted system marker")),
                LegacyUnknown,
            ),
            summary_entry(
                Item::user_message(format!("{SUMMARY_MARKER}pasted user marker")),
                LegacyUnknown,
            ),
            summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}later derived marker")),
                DerivedSummary,
            ),
        ];
        fixture.restore_seed(ordinary.clone(), true).await;
        assert!(
            fixture
                .worker
                .session_history()
                .entries()
                .iter()
                .all(|entry| entry.annotation.origin != CompactionSummary)
        );

        fixture.worker.compact(retained).await.unwrap();

        let input = first_compactor_input(&observed.requests()[1]);
        assert!(!input.contains(INITIAL_SUMMARY_HEADING));
        let overview = input.split_once(OVERVIEW_HEADING).unwrap().1;
        let replacement = replacement_seed(&fixture.worker);
        for entry in &ordinary {
            let text = Item::from(entry.item.clone())
                .as_text()
                .unwrap()
                .to_string();
            assert_eq!(
                overview.contains(&text),
                retained == 0,
                "ordinary overview source: {text}"
            );
            assert_eq!(
                replacement
                    .iter()
                    .any(|retained_entry| retained_entry == entry),
                retained != 0,
                "other derived context and pasted markers must follow ordinary retention"
            );
        }
    }
}

#[tokio::test]
async fn restore_does_not_reinterpret_legacy_summary_markers() {
    use session_store::LoggedSessionHistoryOrigin::{
        CompactionSummary, DerivedSummary, LegacyUnknown,
    };

    let inherited = long_summary("legacy");
    // Each false case lacks one trusted construction condition; marker text
    // alone must never promote ordinary context into inherited Host material.
    for case in [
        "trusted",
        "no-lineage",
        "not-first",
        "unknown-origin",
        "user-role",
        "no-marker",
    ] {
        let client = MockClient::new(vec![
            single_text_events("fixture"),
            write_summary_tool_use_events("replace", "replacement"),
            single_text_events("done"),
        ]);
        let observed = client.clone();
        let mut fixture = SummaryInputFixture::new(MANUAL_ONLY_MANIFEST_TOML, client).await;
        let text = if case == "no-marker" {
            inherited.clone()
        } else {
            format!("{SUMMARY_MARKER}{inherited}")
        };
        let item = if case == "user-role" {
            Item::user_message(text)
        } else {
            Item::system_message(text)
        };
        let origin = if case == "unknown-origin" {
            LegacyUnknown
        } else {
            DerivedSummary
        };
        let candidate = summary_entry(item, origin);
        let candidate_id = candidate.metadata.entry_id.clone();
        let mut history = vec![candidate];
        if case == "not-first" {
            history.insert(
                0,
                annotated(Item::system_message("unrelated first system context")),
            );
        }
        history.extend(recent_history(20));
        fixture.restore_seed(history, case != "no-lineage").await;
        let restored = fixture
            .worker
            .session_history()
            .entries()
            .iter()
            .find(|entry| entry.annotation.entry_id == candidate_id)
            .unwrap();
        assert_ne!(restored.annotation.origin, CompactionSummary, "{case}");
        // Migration belongs to the persisted store, not this restore path.
        // Even a legacy Host seed must not be silently reinterpreted here.
        assert!(
            replacement_seed(&fixture.worker)
                .iter()
                .all(|entry| entry.metadata.origin != CompactionSummary)
        );

        fixture.worker.compact(0).await.unwrap();

        let input = first_compactor_input(&observed.requests()[1]);
        assert!(!input.contains(INITIAL_SUMMARY_HEADING), "{case}");
    }
}

#[tokio::test]
async fn three_chained_compactions_transport_each_complete_predecessor_without_retaining_old_summaries()
 {
    use session_store::LoggedSessionHistoryOrigin::CompactionSummary;

    let original_goal =
        "CHAIN_ORIGINAL_USER_GOAL: finish only after all explicit acceptance conditions hold";
    // Every predetermined output keeps the original directive. The assertions
    // prove that complete predecessors reach later requests, not that a model
    // chose to preserve this directive on its own.
    let summaries = (0..3)
        .map(|index| {
            format!(
                "{}\n{original_goal}",
                long_summary(&format!("generation-{index}"))
            )
        })
        .collect::<Vec<_>>();
    let initial = format!("{}\n{original_goal}", long_summary("initial"));
    let mut responses = vec![single_text_events("seed response")];
    for (index, summary) in summaries.iter().enumerate() {
        responses.push(single_text_events(
            "acknowledge new directive and temporary recovery",
        ));
        responses.push(write_summary_tool_use_events(
            &format!("summary-{index}"),
            summary,
        ));
        responses.push(single_text_events("done"));
    }
    let client = MockClient::new(responses);
    let observed = client.clone();
    let mut fixture = SummaryInputFixture::new(MANUAL_ONLY_MANIFEST_TOML, client).await;
    fixture
        .restore_seed(
            vec![summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}{initial}")),
                CompactionSummary,
            )],
            true,
        )
        .await;
    let original_session = fixture.worker.session_id();
    let mut predecessor = fixture.worker.segment_id();
    for (index, summary) in summaries.iter().enumerate() {
        let new_directive = format!(
            "NEW_USER_DIRECTIVE_{index}: do not merge. Temporarily recover the interrupted work and collect SubWorkers; this does not retract the original goal."
        );
        fixture.worker.run_text(&new_directive).await.unwrap();
        let first_request = observed.requests().len();
        fixture.worker.compact(0).await.unwrap();
        let input = first_compactor_input(&observed.requests()[first_request]);
        let prior = if index == 0 {
            &initial
        } else {
            &summaries[index - 1]
        };
        assert_full_initial_summary(&input, prior);
        assert!(input.contains(original_goal));
        assert!(input.contains(&new_directive));
        let seed = replacement_seed(&fixture.worker);
        let inherited = seed
            .iter()
            .filter(|entry| entry.metadata.origin == CompactionSummary)
            .collect::<Vec<_>>();
        assert_eq!(inherited.len(), 1);
        assert_eq!(
            Item::from(inherited[0].item.clone()).as_text().unwrap(),
            format!("{SUMMARY_MARKER}{summary}")
        );
        for old in &summaries[..index] {
            assert!(
                fixture
                    .worker
                    .history()
                    .iter()
                    .all(|item| !item.as_text().is_some_and(|text| text.contains(old)))
            );
        }
        let records = fixture
            .worker
            .store()
            .read_all(original_session, fixture.worker.segment_id())
            .unwrap();
        assert!(
            records
                .iter()
                .any(|entry| matches!(entry, LogEntry::AnnotatedSegmentStart {
            compacted_from: Some(origin), ..
        } if origin.segment_id == predecessor))
        );
        let restored = session_store::restore(
            fixture.worker.store(),
            original_session,
            fixture.worker.segment_id(),
        )
        .unwrap();
        assert!(restored.history.iter().any(|item| {
            item.as_text()
                .is_some_and(|text| text == format!("{SUMMARY_MARKER}{summary}"))
        }));
        predecessor = fixture.worker.segment_id();
    }
    assert_eq!(observed.requests().len(), 10);
}

#[tokio::test]
async fn inherited_summary_counts_against_hard_input_limit_and_failure_does_not_activate() {
    use session_store::LoggedSessionHistoryOrigin::CompactionSummary;

    let manifest = MANUAL_ONLY_MANIFEST_TOML.replace(
        "[compaction]",
        "[compaction]\nworker_context_max_tokens = 10000\nfinish_warning_remaining_tokens = 0\nfinal_reserve_tokens = 0\noverview_target_tokens = 1",
    );
    for include_summary in [false, true] {
        let client = MockClient::new(vec![
            single_text_events("fixture"),
            write_summary_tool_use_events("replace", "replacement"),
            single_text_events("done"),
        ]);
        let observed = client.clone();
        let mut fixture = SummaryInputFixture::new(&manifest, client).await;
        let summary =
            "Preserve the user's goal and every explicit completion condition.\n".repeat(3_000);
        assert!(
            agen::token_counter::total_tokens(&[Item::user_message(&summary)], &[]).tokens > 10_000
        );
        let mut history = Vec::new();
        if include_summary {
            history.push(summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}{summary}")),
                CompactionSummary,
            ));
        }
        history.extend(recent_history(20));
        fixture.restore_seed(history, true).await;
        let source = fixture.worker.segment_id();
        let source_history = fixture.worker.history();
        let result = fixture.worker.compact(0).await;
        if include_summary {
            let error = result.expect_err("mandatory input over hard cap must fail, not truncate");
            assert!(
                matches!(error, WorkerError::CompactCancelled)
                    || error.to_string().contains("input occupancy"),
                "unexpected hard-cap failure: {error:?}"
            );
            assert_eq!(
                observed.requests().len(),
                1,
                "no compactor request may cross the hard cap"
            );
            assert_eq!(fixture.worker.segment_id(), source);
            assert_eq!(fixture.worker.history(), source_history);
            assert_eq!(
                fixture
                    .worker
                    .store()
                    .read_by_name("test-worker")
                    .unwrap()
                    .unwrap()
                    .active
                    .unwrap()
                    .segment_id,
                Some(source)
            );
            assert_eq!(
                fixture
                    .worker
                    .store()
                    .list_segments(fixture.worker.session_id())
                    .unwrap()
                    .len(),
                1
            );
        } else {
            result.expect("the same bounded overview without inherited input fits the cap");
            assert_eq!(observed.requests().len(), 3);
            assert_ne!(fixture.worker.segment_id(), source);
        }
    }
}

#[tokio::test]
async fn summary_output_still_over_hard_max_after_rewrite_fails_without_activation() {
    let oversized = long_summary("oversized");
    let manifest = MANUAL_ONLY_MANIFEST_TOML.replace(
        "[compaction]",
        "[compaction]\nsummary_target_tokens = 50\nsummary_max_tokens = 100",
    );
    let client = MockClient::new(vec![
        single_text_events("seed"),
        write_summary_tool_use_events("first-output", &oversized),
        single_text_events("done"),
        write_summary_tool_use_events("rewrite-output", &oversized),
        single_text_events("done"),
    ]);
    let observed = client.clone();
    let mut fixture = SummaryInputFixture::new(&manifest, client).await;
    fixture.worker.run_text("user goal").await.unwrap();
    let source = fixture.worker.segment_id();
    let history = fixture.worker.history();

    let error = fixture.worker.compact(0).await.unwrap_err();

    assert!(matches!(
        error,
        WorkerError::CompactSummaryTooLarge { max: 100, .. }
    ));
    assert_eq!(fixture.worker.segment_id(), source);
    assert_eq!(fixture.worker.history(), history);
    assert_eq!(
        fixture
            .worker
            .store()
            .read_by_name("test-worker")
            .unwrap()
            .unwrap()
            .active
            .unwrap()
            .segment_id,
        Some(source)
    );
    assert_eq!(
        fixture
            .worker
            .store()
            .list_segments(fixture.worker.session_id())
            .unwrap()
            .len(),
        1
    );
    assert!(
        observed
            .requests()
            .iter()
            .any(|request| request.items.iter().any(|item| item
                .as_text()
                .is_some_and(|text| text.contains("Rewrite it now with `write_summary`"))))
    );
}

#[tokio::test]
async fn budget_warning_is_durable_and_keeps_full_summary_in_first_model_request() {
    use session_store::LoggedSessionHistoryOrigin::CompactionSummary;
    let manifest = MANUAL_ONLY_MANIFEST_TOML.replace(
        "[compaction]",
        "[compaction]\nworker_context_max_tokens = 10000\nfinish_warning_remaining_tokens = 9999\nfinal_reserve_tokens = 9999",
    );
    let client = MockClient::new(vec![
        single_text_events("fixture"),
        write_summary_tool_use_events("summary", "replacement"),
        single_text_events("done"),
    ]);
    let observed = client.clone();
    let mut fixture = SummaryInputFixture::new(&manifest, client).await;
    let inherited = long_summary("budget-warning");
    fixture
        .restore_seed(
            vec![summary_entry(
                Item::system_message(format!("{SUMMARY_MARKER}{inherited}")),
                CompactionSummary,
            )],
            true,
        )
        .await;
    let (tx, mut rx) = broadcast::channel::<Event>(128);
    fixture.worker.attach_alerter(worker::Alerter::new(tx));
    fixture.worker.compact(0).await.unwrap();
    let requests = observed.requests();
    assert_full_initial_summary(&first_compactor_input(&requests[1]), &inherited);
    let warning = requests[1]
        .items
        .iter()
        .filter_map(Item::as_text)
        .find(|text| text.starts_with("[Compact worker budget warning]"))
        .expect("normal internal Worker must preserve the budget interceptor warning");
    assert!(warning.contains("full initial summary and segment"));
    assert!(warning.contains("changes/retractions"));
    assert!(
        requests[2]
            .items
            .iter()
            .filter_map(Item::as_text)
            .any(|text| text == warning),
        "warning must remain in history, not be a transient request insertion"
    );
    assert!(
        drain(&mut rx)
            .iter()
            .any(|event| matches!(event, Event::Alert(alert)
        if alert.message.contains("compact worker context budget is low")))
    );
}

const COMPACTION_HOST_WORKSPACE_ID: &str = "journal-transport-host-workspace";

// Public compact/rewind -> real internal Worker, prompt catalog, and FsStore.
// Only the authenticated Host and LLM provider are replaced. This does not test
// Host authorization/selection or live-model semantic continuation quality.
struct CompactionJournalHost {
    granted: bool,
    unavailable: bool,
    snapshot: Mutex<subjektiv::journal::startup::JournalStartupSnapshot>,
    reads: Mutex<Vec<worker::subjektiv::SubjektivHostContext>>,
    startup_reads: AtomicUsize,
}

impl CompactionJournalHost {
    fn new(snapshot: subjektiv::journal::startup::JournalStartupSnapshot) -> Self {
        Self {
            granted: true,
            unavailable: false,
            snapshot: Mutex::new(snapshot),
            reads: Mutex::new(vec![]),
            startup_reads: AtomicUsize::new(0),
        }
    }
}

impl worker::subjektiv::SubjektivHost for CompactionJournalHost {
    fn settings(&self) -> worker::subjektiv::SubjektivHostSettings {
        worker::subjektiv::SubjektivHostSettings {
            language: "English".into(),
        }
    }

    fn workspace_id(&self) -> Option<String> {
        Some(COMPACTION_HOST_WORKSPACE_ID.into())
    }

    fn journal_compaction_granted(&self) -> bool {
        self.granted
    }

    fn journal_compaction_context(
        &self,
        context: &worker::subjektiv::SubjektivHostContext,
    ) -> Result<
        subjektiv::journal::startup::JournalStartupSnapshot,
        worker::subjektiv::SubjektivHostError,
    > {
        self.reads.lock().unwrap().push(context.clone());
        if self.unavailable {
            return Err(worker::subjektiv::SubjektivHostError::Unavailable(
                "injected Journal read failure".into(),
            ));
        }
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn journal_startup_granted(&self) -> bool {
        true
    }

    fn journal_startup_context(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
    ) -> Result<
        subjektiv::journal::startup::JournalStartupSnapshot,
        worker::subjektiv::SubjektivHostError,
    > {
        self.startup_reads.fetch_add(1, Ordering::SeqCst);
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "startup must not be read during compaction/rewind/normal turns".into(),
        ))
    }

    fn memory(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
        _: server_api::SubjektivMemoryBackendOperation,
    ) -> Result<server_api::SubjektivMemoryBackendResponse, worker::subjektiv::SubjektivHostError>
    {
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "not used".into(),
        ))
    }
    fn session(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
        _: server_api::SubjektivSessionBackendOperation,
    ) -> Result<server_api::SubjektivSessionBackendResponse, worker::subjektiv::SubjektivHostError>
    {
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "not used".into(),
        ))
    }
    fn stage_candidate(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
        _: memory::backend::MemoryStageCandidateOperation,
    ) -> Result<server_api::SubjektivStageCandidateResponse, worker::subjektiv::SubjektivHostError>
    {
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "not used".into(),
        ))
    }
    fn record_session(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
        _: bool,
    ) -> Result<server_api::SubjektivRecordSessionResponse, worker::subjektiv::SubjektivHostError>
    {
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "not used".into(),
        ))
    }
    fn request_consolidation(
        &self,
        _: &worker::subjektiv::SubjektivHostContext,
        _: memory::backend::MemoryConsolidateStagingOperation,
    ) -> Result<memory::backend::MemoryConsolidationOutput, worker::subjektiv::SubjektivHostError>
    {
        Err(worker::subjektiv::SubjektivHostError::Unavailable(
            "not used".into(),
        ))
    }
}

fn compaction_snapshot(generation: usize) -> subjektiv::journal::startup::JournalStartupSnapshot {
    use subjektiv::journal::startup::{JournalStartupSnapshot, StartupEntry, StartupJournal};
    JournalStartupSnapshot {
        subject_id: "compaction-subject".into(),
        omitted: true,
        journals: vec![StartupJournal {
            journal_id: format!("journal-reference-{generation}"),
            source_session_id: "current-source-session".into(),
            state_token: format!("state-token-{generation}"),
            process_order: 7 + generation as u64,
            process_byte: 23 + generation as u64,
            current_position: format!(
                "JOURNAL_POSITION_{generation}: work ongoing, result not verified\nExact JSON bytes: 日本語, \"quoted\", \\path"
            ),
            entries: vec![StartupEntry {
                entry_id: format!("fixed-event-reference-{generation}"),
                position: 2,
                state: subjektiv::journal::JournalEntryState::Fixed,
                title: "An event ended, not the entire conversation".into(),
                body: format!("JOURNAL_BODY_{generation}"),
                ongoing: "User instructions after the checkpoint are not recorded".into(),
                source_end: Some(subjektiv::journal::JournalSourcePointer {
                    segment_id: session_store::new_segment_id(),
                    entry_ref: "E3".into(),
                }),
                correction_refs: vec![format!("correction-reference-{generation}")],
                omitted: false,
            }],
            omitted: true,
        }],
    }
}

fn journal_body(worker: &Worker<MockClient, TestStore>) -> String {
    let seed = replacement_seed(worker);
    assert_eq!(
        seed[1].metadata.origin,
        session_store::LoggedSessionHistoryOrigin::JournalContext
    );
    let item = Item::from(seed[1].item.clone());
    assert!(matches!(
        item,
        Item::Message {
            role: agen::Role::System,
            ..
        }
    ));
    assert_eq!(
        seed.iter()
            .filter(|entry| entry.metadata.origin
                == session_store::LoggedSessionHistoryOrigin::JournalContext)
            .count(),
        1
    );
    item.as_text().unwrap().to_string()
}

fn journal_source_records(
    store: &TestStore,
    session: session_store::SessionId,
    segment: session_store::SegmentId,
) -> serde_json::Value {
    // Compaction appends telemetry; the source material and typed startup
    // records must remain equivalent, not rewritten or removed.
    let records = store
        .read_all(session, segment)
        .unwrap()
        .into_iter()
        .filter(
            |record| !matches!(record, LogEntry::Extension { domain, .. } if domain == "metrics"),
        )
        .collect::<Vec<_>>();
    serde_json::to_value(records).unwrap()
}

fn assert_frozen_journal_transport(input: &str, body: &str) {
    assert!(
        input.ends_with(body),
        "the actual compactor user input ends with the frozen reference"
    );
    assert_eq!(input.matches(body).count(), 1);
}

fn assert_journal_provenance(
    body: &str,
    expected_json: Option<&str>,
    catalog: &worker::PromptCatalog,
    workspace_id: Option<&str>,
) {
    use sha2::{Digest, Sha256};
    let projection = catalog.projection();
    assert!(body.contains(&format!(
        "Prompt provenance: `internal.subjektiv_journal_compaction`, source `{}`, projection `{}`.",
        projection.source_digest, projection.catalog_digest,
    )));
    if let Some(workspace_id) = workspace_id {
        assert!(body.contains(&format!("Host Workspace: `{workspace_id}`.")));
    } else {
        assert!(!body.contains("Host Workspace:"));
    }
    if let Some(expected_json) = expected_json {
        // Hash the actual rendered JSON, including escapes and UTF-8 bytes,
        // not a parsed/re-serialized or pretty-printed approximation.
        let rendered_json = body.lines().rfind(|line| !line.is_empty()).unwrap();
        assert_eq!(rendered_json.as_bytes(), expected_json.as_bytes());
        let expected_hash = Sha256::digest(rendered_json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let declared_hash = body
            .lines()
            .find_map(|line| {
                line.strip_prefix("Snapshot SHA-256 (the exact JSON bytes below): `")
                    .and_then(|value| value.strip_suffix("`."))
            })
            .expect("snapshot SHA-256 must be persisted in the same rendered body");
        assert_eq!(declared_hash, expected_hash);
    } else {
        assert!(
            !body.contains("Snapshot SHA-256"),
            "failed/disabled reads must not invent a snapshot hash"
        );
    }
}

#[tokio::test]
async fn journal_three_compactions_freeze_one_snapshot_preserve_full_summary_tail_tasks_and_source()
{
    use session_store::LoggedSessionHistoryOrigin::{
        CompactionSummary, DerivedSummary, JournalContext,
    };
    let initial = long_summary("journal-initial");
    let summaries = (0..3)
        .map(|index| long_summary(&format!("journal-next-{index}")))
        .collect::<Vec<_>>();
    let mut responses = vec![single_text_events("fixture")];
    for (index, summary) in summaries.iter().enumerate() {
        responses.push(single_text_events(&format!("TAIL_ASSISTANT_{index}")));
        responses.push(write_summary_tool_use_events(
            &format!("journal-summary-{index}"),
            summary,
        ));
        responses.push(single_text_events("done"));
    }
    responses.extend([
        single_text_events("normal after compact"),
        single_text_events("after rewind"),
    ]);
    let client = MockClient::new(responses);
    let observed = client.clone();
    let host = Arc::new(CompactionJournalHost::new(compaction_snapshot(0)));
    let mut fixture =
        SummaryInputFixture::with_journal(MANUAL_ONLY_MANIFEST_TOML, client, host.clone()).await;
    // A resolved nonempty source digest distinguishes provenance forwarding
    // from silently substituting the built-in catalog's empty source digest.
    let mut projection = fixture.worker.prompts().load_full().projection().clone();
    projection.source_digest = "a".repeat(64);
    let initial_catalog = Arc::new(worker::PromptCatalog::from_projection(projection).unwrap());
    fixture.worker.prompts().store(initial_catalog.clone());
    let legacy_startup = session_store::LoggedSystemHistoryEntry {
        item: session_store::SystemItem::JournalStartup {
            body: "OLD_TYPED_STARTUP_JOURNAL".into(),
            snapshot: serde_json::json!({"journals": []}),
            snapshot_sha256: "legacy-startup-digest".into(),
            prompt_provenance: session_store::PromptRenderProvenance {
                workspace_id: None,
                source_digest: "legacy-source".into(),
                projection_digest: "legacy-projection".into(),
                logical_name: "internal.subjektiv_journal_startup".into(),
            },
        },
        metadata: summary_entry(Item::system_message(""), DerivedSummary).metadata,
    };
    let legacy_id = legacy_startup.metadata.entry_id.clone();
    fixture
        .restore_seed_with_system(
            vec![
                summary_entry(
                    Item::system_message(format!("{SUMMARY_MARKER}{initial}")),
                    CompactionSummary,
                ),
                summary_entry(
                    Item::system_message("OLD_COMPACTION_JOURNAL"),
                    JournalContext,
                ),
                annotated(Item::tool_call(
                    "seed-task",
                    "TaskCreate",
                    r#"{"subject":"JOURNAL_TASK","description":"Keep the unfinished task"}"#,
                )),
                annotated(Item::tool_result("seed-task", "created")),
            ],
            true,
            vec![legacy_startup],
        )
        .await;
    assert_eq!(
        fixture
            .worker
            .session_history()
            .entries()
            .iter()
            .find(|entry| entry.annotation.entry_id == legacy_id)
            .unwrap()
            .annotation
            .origin,
        JournalContext
    );
    let session = fixture.worker.session_id();
    let original_segment = fixture.worker.segment_id();
    let mut historical_sources = Vec::new();
    let mut old_bodies = vec![
        "OLD_TYPED_STARTUP_JOURNAL".to_string(),
        "OLD_COMPACTION_JOURNAL".to_string(),
    ];
    for (index, retained) in [0, 1_000_000, 100].into_iter().enumerate() {
        let directive = format!(
            "UNRECORDED_USER_{index}: do not merge; original completion conditions still apply"
        );
        fixture.worker.run_text(&directive).await.unwrap();
        if index == 0 {
            assert_eq!(
                fixture
                    .worker
                    .session_history()
                    .entries()
                    .iter()
                    .find(|entry| entry.annotation.entry_id == legacy_id)
                    .unwrap()
                    .annotation
                    .origin,
                JournalContext,
                "legacy startup classification survives a normal turn"
            );
        }
        assert_eq!(
            host.reads.lock().unwrap().len(),
            index,
            "normal turns do not fetch compaction Journal"
        );
        let snapshot = compaction_snapshot(index);
        *host.snapshot.lock().unwrap() = snapshot.clone();
        let first_request = observed.requests().len();
        let updating_host = host.clone();
        observed.set_observer(move |call| {
            if call == first_request {
                // A background update at the provider boundary, after the
                // request has been captured but before generation completes.
                *updating_host.snapshot.lock().unwrap() = compaction_snapshot(100 + index);
            }
        });
        let source = fixture.worker.segment_id();
        let frozen_catalog = fixture.worker.prompts().load_full();
        historical_sources.push((
            source,
            journal_source_records(fixture.worker.store(), session, source),
        ));
        fixture.worker.compact(retained).await.unwrap();
        let input = first_compactor_input(&observed.requests()[first_request]);
        assert_full_initial_summary(
            &input,
            if index == 0 {
                &initial
            } else {
                &summaries[index - 1]
            },
        );
        let body = journal_body(&fixture.worker);
        assert_frozen_journal_transport(&input, &body);
        assert_journal_provenance(
            &body,
            Some(&serde_json::to_string(&snapshot).unwrap()),
            &frozen_catalog,
            Some(COMPACTION_HOST_WORKSPACE_ID),
        );
        assert!(
            body.contains(&serde_json::to_string(&snapshot).unwrap()),
            "contents, references, fixed source_end and partial processing checkpoint must match"
        );
        assert!(!body.contains(&format!("JOURNAL_POSITION_{}:", 100 + index)));
        assert_eq!(host.reads.lock().unwrap().len(), index + 1);
        let history = fixture.worker.history();
        let overview = input
            .split("## Previous Journal reference (historical, not automatically retained)")
            .next()
            .unwrap();
        for old in &old_bodies {
            assert!(
                !overview.contains(old),
                "superseded Journal is excluded from ordinary overview"
            );
            assert!(
                !body.contains(old),
                "frozen latest reference does not incorporate old injections"
            );
            assert!(
                history
                    .iter()
                    .all(|item| !item.as_text().is_some_and(|text| text.contains(old))),
                "superseded Journal is not retained"
            );
        }
        assert!(
            input.contains(&directive)
                || history
                    .iter()
                    .any(|item| item.as_text() == Some(directive.as_str())),
            "unrecorded user instruction must reach summary input or retained tail"
        );
        if retained == 1_000_000 {
            assert!(
                history
                    .iter()
                    .any(|item| item.as_text() == Some("TAIL_ASSISTANT_1"))
            );
        }
        let task = history
            .iter()
            .filter_map(Item::as_text)
            .find(|text| text.starts_with("[Session TaskStore snapshot]"))
            .unwrap();
        let json = task
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        let tasks: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            tasks["tasks"].as_array().unwrap().len(),
            1,
            "replayed TaskCreate must not duplicate across compactions"
        );
        assert_eq!(tasks["tasks"][0]["subject"], "JOURNAL_TASK");
        assert_eq!(tasks["tasks"][0]["status"], "pending");
        assert!(input.contains("JOURNAL_TASK"));
        old_bodies.push(body);
        for (source, before) in &historical_sources {
            assert_eq!(
                journal_source_records(fixture.worker.store(), session, *source),
                *before,
                "original segments must remain unchanged"
            );
        }
    }
    assert_ne!(original_segment, fixture.worker.segment_id());
    let reads = host.reads.lock().unwrap().clone();
    assert!(
        reads
            .iter()
            .all(|context| context.worker_id == "test-worker"
                && context.session_id == session.to_string())
    );
    // Reloaded prompt resources must not regenerate the persisted reference
    // or relabel its original source/projection on normal turns or restore.
    let mut templates = initial_catalog.projection().templates.clone();
    templates
        .get_mut("internal.subjektiv_journal_compaction")
        .unwrap()
        .push_str("\nRELOADED_PROMPT_MUST_NOT_REPLACE_FROZEN_BODY");
    let mut reloaded_projection =
        worker::EffectivePromptCatalog::new(templates, "test-schema", "test-toolchain").unwrap();
    reloaded_projection.source_digest = "b".repeat(64);
    let reloaded_catalog =
        Arc::new(worker::PromptCatalog::from_projection(reloaded_projection).unwrap());
    assert_ne!(
        initial_catalog.projection().catalog_digest,
        reloaded_catalog.projection().catalog_digest
    );
    fixture.worker.prompts().store(reloaded_catalog);
    fixture
        .worker
        .run_text("normal post-compaction turn")
        .await
        .unwrap();
    let records = fixture
        .worker
        .store()
        .read_all(session, fixture.worker.segment_id())
        .unwrap();
    let snapshot = session_store::public_snapshot::project_current_session_snapshot(&records);
    let target = snapshot
        .entries
        .iter()
        .rev()
        .find(|entry| {
            matches!(
                entry.data,
                protocol::SessionSnapshotEntryData::UserInput { .. }
            )
        })
        .unwrap();
    fixture
        .worker
        .rewind_to(target.entry_id.clone())
        .await
        .unwrap();
    let restored_body = journal_body(&fixture.worker);
    assert_eq!(restored_body, *old_bodies.last().unwrap());
    assert_journal_provenance(
        &restored_body,
        old_bodies
            .last()
            .unwrap()
            .lines()
            .rfind(|line| !line.is_empty()),
        &initial_catalog,
        Some(COMPACTION_HOST_WORKSPACE_ID),
    );
    fixture
        .worker
        .run_text("normal turn after typed history restore")
        .await
        .unwrap();
    assert_eq!(
        host.reads.lock().unwrap().len(),
        3,
        "rewind uses the shared typed restore path without fresh Journal acquisition"
    );
    assert_eq!(host.startup_reads.load(Ordering::SeqCst), 0);
    let normal = observed.requests().last().unwrap().clone();
    assert_eq!(
        normal
            .items
            .iter()
            .filter(|item| item.as_text() == Some(old_bodies.last().unwrap().as_str()))
            .count(),
        1
    );
}

#[tokio::test]
async fn journal_absent_disabled_unavailable_and_over_byte_budget_keep_conversation_compaction() {
    for case in [
        "absent",
        "disabled",
        "unavailable",
        "over-byte-budget",
        "no-host",
    ] {
        let mut snapshot = compaction_snapshot(0);
        if case == "absent" {
            snapshot.journals.clear();
        }
        if case == "over-byte-budget" {
            snapshot.journals[0].current_position =
                "x".repeat(subjektiv::journal::startup::STARTUP_MAX_BYTES);
        }
        let expected_json = (case == "absent").then(|| serde_json::to_string(&snapshot).unwrap());
        let mut host = CompactionJournalHost::new(snapshot);
        host.granted = case != "disabled";
        host.unavailable = case == "unavailable";
        let host = Arc::new(host);
        let client = MockClient::new(vec![
            single_text_events("seed"),
            write_summary_tool_use_events("fallback", "fallback summary"),
            single_text_events("done"),
        ]);
        let observed = client.clone();
        let mut fixture = if case == "no-host" {
            SummaryInputFixture::new(MANUAL_ONLY_MANIFEST_TOML, client).await
        } else {
            SummaryInputFixture::with_journal(MANUAL_ONLY_MANIFEST_TOML, client, host.clone()).await
        };
        let inherited = long_summary("fallback-predecessor");
        fixture
            .restore_seed(
                vec![
                    summary_entry(
                        Item::system_message(format!("{SUMMARY_MARKER}{inherited}")),
                        session_store::LoggedSessionHistoryOrigin::CompactionSummary,
                    ),
                    summary_entry(
                        Item::system_message("PREVIOUS_JOURNAL_DELEGATED_CONTEXT"),
                        session_store::LoggedSessionHistoryOrigin::JournalContext,
                    ),
                    annotated(Item::user_message(
                        "UNRECORDED_FALLBACK_GOAL: implementation still unfinished",
                    )),
                ],
                true,
            )
            .await;
        fixture.worker.compact(0).await.unwrap();
        let body = journal_body(&fixture.worker);
        let expected = match case {
            "no-host" | "disabled" => "disabled",
            "absent" => "absent",
            _ => "unavailable",
        };
        assert!(
            body.contains(&format!("Journal read status: {expected}.")),
            "{case}"
        );
        let input = first_compactor_input(&observed.requests()[1]);
        assert_frozen_journal_transport(&input, &body);
        assert_journal_provenance(
            &body,
            expected_json.as_deref(),
            &fixture.worker.prompts().load_full(),
            (case != "no-host").then_some(COMPACTION_HOST_WORKSPACE_ID),
        );
        assert_full_initial_summary(&input, &inherited);
        assert!(input.contains("UNRECORDED_FALLBACK_GOAL"), "{case}");
        assert!(input.contains("## Previous Journal reference (historical, not automatically retained)\nPREVIOUS_JOURNAL_DELEGATED_CONTEXT"));
        assert!(
            fixture.worker.history().iter().all(|item| !item
                .as_text()
                .is_some_and(|text| text.contains("PREVIOUS_JOURNAL_DELEGATED_CONTEXT"))),
            "fallback retains only the newly frozen reference, not an old snapshot as latest"
        );
        assert!(!body.contains("JOURNAL_POSITION_0"));
        assert!(!body.contains("journal-reference-0"));
        assert_eq!(
            host.reads.lock().unwrap().len(),
            usize::from(!matches!(case, "disabled" | "no-host")),
            "{case}"
        );
        assert!(
            fixture.worker.history()[0]
                .as_text()
                .unwrap()
                .contains("fallback summary")
        );
    }
}

#[tokio::test]
async fn journal_snapshot_counts_against_compactor_hard_input_cap_without_mutating_source() {
    let manifest = MANUAL_ONLY_MANIFEST_TOML.replace("[compaction]", "[compaction]\nworker_context_max_tokens = 7000\nfinish_warning_remaining_tokens = 0\nfinal_reserve_tokens = 0\noverview_target_tokens = 1");
    for large in [false, true] {
        let mut snapshot = compaction_snapshot(0);
        if large {
            snapshot.journals[0].current_position = " x".repeat(15_000);
        }
        assert!(
            serde_json::to_vec(&snapshot).unwrap().len()
                <= subjektiv::journal::startup::STARTUP_MAX_BYTES
        );
        if large {
            assert!(
                agen::token_counter::total_tokens(
                    &[Item::user_message(&snapshot.journals[0].current_position)],
                    &[]
                )
                .tokens
                    > 7_000
            );
        }
        let host = Arc::new(CompactionJournalHost::new(snapshot));
        let client = MockClient::new(vec![
            single_text_events("seed"),
            write_summary_tool_use_events("summary", "new summary"),
            single_text_events("done"),
        ]);
        let observed = client.clone();
        let mut fixture = SummaryInputFixture::with_journal(&manifest, client, host.clone()).await;
        fixture.worker.run_text("unfinished goal").await.unwrap();
        let session = fixture.worker.session_id();
        let source = fixture.worker.segment_id();
        let before = journal_source_records(fixture.worker.store(), session, source);
        let history = fixture.worker.history();
        let result = fixture.worker.compact(0).await;
        if large {
            let error = result.expect_err(
                "mandatory frozen Journal must not be truncated to evade hard input cap",
            );
            assert!(
                matches!(error, WorkerError::CompactCancelled)
                    || error.to_string().contains("input occupancy"),
                "{error:?}"
            );
            assert_eq!(
                observed.requests().len(),
                1,
                "over-budget input must not reach the provider"
            );
            assert_eq!(fixture.worker.segment_id(), source);
            assert_eq!(fixture.worker.history(), history);
            assert_eq!(
                fixture.worker.store().list_segments(session).unwrap(),
                vec![source]
            );
            assert_eq!(
                fixture
                    .worker
                    .store()
                    .read_by_name("test-worker")
                    .unwrap()
                    .unwrap()
                    .active
                    .unwrap()
                    .segment_id,
                Some(source)
            );
        } else {
            result.unwrap();
            assert_eq!(observed.requests().len(), 3);
            assert_ne!(fixture.worker.segment_id(), source);
        }
        assert_eq!(host.reads.lock().unwrap().len(), 1);
        assert_eq!(
            journal_source_records(fixture.worker.store(), session, source),
            before
        );
    }
}
