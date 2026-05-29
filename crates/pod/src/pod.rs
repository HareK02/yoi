use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use llm_worker::Item;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::types::Role;
use llm_worker::state::Mutable;
use llm_worker::{ToolOutputLimits, UsageRecord, Worker, WorkerError, WorkerResult};
use session_store::{
    LogEntry, PodActiveSegmentRef, PodMetadata, PodMetadataStore, PodScopeSnapshot, SegmentId,
    SessionId, Store, StoreError, SystemItem, segment_log, to_logged,
};
use tracing::{info, warn};

use crate::segment_log_sink::SegmentLogSink;

use manifest::{
    Permission, PodManifest, PodManifestConfig, ResolveError, Scope, ScopeConfig, ScopeError,
    ScopeRule, SharedScope, WorkerManifest,
};

use crate::compact::state::CompactState;
use crate::compact::usage_tracker::UsageTracker;
use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreRequestInfo, PreToolCall,
};
use crate::ipc::alerter::Alerter;
use crate::ipc::interceptor::{PodInterceptor, TaskReminderState};
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::prompt::agents_md::read_agents_md;
use crate::prompt::catalog::{CatalogError, PromptCatalog};
use crate::prompt::loader::PromptLoader;
use crate::prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
use crate::runtime::dir;
use crate::runtime::pod_registry::{self, ScopeAllocationGuard, ScopeLockError};
use crate::workflow::WorkflowResolveError;
use async_trait::async_trait;
use llm_worker::interceptor::PreRequestAction;
use protocol::{
    AlertLevel, AlertSource, Event, RewindSummary, RewindTarget, RewindTargetId, Segment,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// `(SessionId, SegmentId)` pair the Pod is currently writing to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentLocation {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
}

type PodMetadataWriter = Arc<dyn Fn(PodMetadata) -> Result<(), StoreError> + Send + Sync>;

fn pod_metadata_writer_for_store<St>(store: &St) -> PodMetadataWriter
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    let store = store.clone();
    Arc::new(move |mut metadata| {
        if let Some(existing) = store.read_by_name(&metadata.pod_name)? {
            metadata.spawned_children = existing.spawned_children;
        }
        store.write(&metadata)
    })
}

/// Lock-free shared session/segment pointer.
///
/// Holds the current `(SessionId, SegmentId)` pair and the append tally
/// so that the Pod and every `LogWriterHandle` clone see a consistent
/// view through `Arc`-shared lock-free reads. The location is wrapped in
/// `ArcSwap` so fork (a rare, run-start-only event) can atomically swap
/// session_id + segment_id together without taking a mutex on the
/// append hot path. `entries_written` is an `AtomicUsize` bumped on
/// every successful append; the writer's tally is compared against the
/// store's on-disk count to detect concurrent writers in
/// `ensure_segment_head`.
pub struct SegmentState {
    location: ArcSwap<SegmentLocation>,
    entries_written: AtomicUsize,
}

impl SegmentState {
    pub fn new(session_id: SessionId, segment_id: SegmentId, entries_written: usize) -> Arc<Self> {
        Arc::new(Self {
            location: ArcSwap::from_pointee(SegmentLocation {
                session_id,
                segment_id,
            }),
            entries_written: AtomicUsize::new(entries_written),
        })
    }

    pub fn location(&self) -> SegmentLocation {
        **self.location.load()
    }

    pub fn session_id(&self) -> SessionId {
        self.location().session_id
    }

    pub fn segment_id(&self) -> SegmentId {
        self.location().segment_id
    }

    pub fn set_location(&self, loc: SegmentLocation) {
        self.location.store(Arc::new(loc));
    }

    pub fn entries_written(&self) -> usize {
        self.entries_written.load(Ordering::Acquire)
    }

    pub fn set_entries_written(&self, n: usize) {
        self.entries_written.store(n, Ordering::Release);
    }

    fn increment_entries(&self) {
        self.entries_written.fetch_add(1, Ordering::Release);
    }
}

struct EmptyTurnRollbackSnapshot {
    history_len: usize,
    user_segments_len: usize,
    entries_written: usize,
    sink_len: usize,
    pending_attachments: Vec<SystemItem>,
    usage_history_len: usize,
    ai_activity_count: usize,
    last_run_interrupted: bool,
}

fn is_ai_materialized_item(item: &Item) -> bool {
    match item {
        Item::Message { role, .. } => *role == Role::Assistant,
        Item::ToolCall { .. } | Item::ToolResult { .. } | Item::Reasoning { .. } => true,
    }
}

/// Cheap-cloneable bundle of (store + shared session pointer + sink)
/// handed to the worker callback and the interceptor so they can
/// commit `LogEntry` values directly without going through an mpsc
/// ferry. All fields are `Clone` (`store` per its `Clone` impl,
/// `state` and `sink` as `Arc` clones).
#[derive(Clone)]
pub struct LogWriterHandle<St: Clone> {
    pub store: St,
    pub state: Arc<SegmentState>,
    pub sink: SegmentLogSink,
}

impl<St> LogWriterHandle<St>
where
    St: Store + Clone,
{
    /// Append `entry` to the log: disk write → counter bump → in-memory
    /// mirror push → broadcast. The kernel orders concurrent `O_APPEND`
    /// writes for `< PIPE_BUF` lines, so no user-space serialization is
    /// needed across appenders.
    pub fn append_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        let loc = self.state.location();
        self.store.append(loc.session_id, loc.segment_id, &entry)?;
        self.state.increment_entries();
        self.sink.publish(entry);
        Ok(())
    }

    /// Append a debug trace record alongside the current segment log. Trace
    /// writes deliberately do not affect the segment entry counter or live
    /// replay sink because they are not conversation history.
    pub fn append_trace(&self, entry: &session_store::TraceEntry) -> Result<(), StoreError> {
        let loc = self.state.location();
        self.store
            .append_trace(loc.session_id, loc.segment_id, entry)
    }
}

/// Type-erased commit handle for the interceptor. Lets the
/// interceptor commit `SystemItem`s without being generic over the
/// concrete `Store` type.
pub trait SystemItemCommitter: Send + Sync {
    fn commit_system_item(&self, item: SystemItem);
}

impl<St> SystemItemCommitter for LogWriterHandle<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    fn commit_system_item(&self, item: SystemItem) {
        let entry = LogEntry::SystemItem {
            ts: segment_log::now_millis(),
            item,
        };
        if let Err(err) = self.append_entry(entry) {
            warn!(error = %err, "system item commit failed; dropping");
        }
    }
}

/// Pre-LLM-request hook that records `history.len()` at send time into a
/// shared `UsageTracker`. The on_usage callback later pairs this with the
/// aggregated UsageEvent to produce one `UsageRecord` per LLM call.
struct UsageTrackingHook {
    tracker: Arc<UsageTracker>,
}

#[async_trait]
impl Hook<PreLlmRequest> for UsageTrackingHook {
    async fn call(&self, info: &PreRequestInfo) -> PreRequestAction {
        self.tracker.note_request(info.item_count);
        PreRequestAction::Continue
    }
}

/// An independent agent execution unit.
///
/// Holds a [`Worker`] directly and persists session state via
/// `session-store` functions after each turn.
pub struct Pod<C: LlmClient, St: Store> {
    manifest: PodManifest,
    /// Always `Some` outside of `run()`/`resume()`.
    worker: Option<Worker<C, Mutable>>,
    store: St,
    /// Optional write-through hook for name-keyed Pod metadata. Production
    /// constructors install this from the same FsStore that owns the session
    /// logs; low-level `Pod::new` tests leave it absent.
    pod_metadata_writer: Option<PodMetadataWriter>,
    /// Shared session pointer. Source of truth for the Pod's current
    /// `segment_id` and append tally. `self.segment_id()` is a thin
    /// wrapper over `segment_state.segment_id()`.
    segment_state: Arc<SegmentState>,
    /// Absolute working directory of the Pod.
    pwd: PathBuf,
    /// Shared, atomically-swappable view of the Pod's resolved scope.
    /// Cloned out to `ScopedFs` instances (builtin tools, fs_view,
    /// compact worker) so scope updates propagate to every consumer
    /// at the next permission check.
    scope: SharedScope,
    hook_builder: HookRegistryBuilder,
    interceptor_installed: bool,
    /// Shared compaction state (present when threshold is configured).
    compact_state: Option<Arc<CompactState>>,
    /// Per-LLM-request Usage tracker. Always present after construction.
    /// Captures `(history_len, UsageEvent)` pairs during a run; drained
    /// in `persist_turn` and persisted as `LogEntry::LlmUsage` entries.
    usage_tracker: Arc<UsageTracker>,
    /// Sync-side buffer for `Metric` values queued from inside Worker
    /// callbacks (currently the prune observer). Drained in `persist_turn`
    /// and written via `session_metrics::record_metric` alongside
    /// `LogEntry::LlmUsage`. Always present after construction.
    metrics_tracker: Arc<crate::compact::metrics_tracker::MetricsTracker>,
    /// Cumulative Usage measurement timeline, one entry per LLM call.
    /// Restored from session log on `restore`, appended on each persist.
    /// Read by token-accounting APIs (`Pod::total_tokens`, etc.).
    ///
    /// Wrapped in `Arc<Mutex>` so that callbacks injected into the
    /// Worker (e.g. the savings estimator used by the prune projection)
    /// can share the same view via [`Pod::usage_history_handle`].
    usage_history: Arc<Mutex<Vec<UsageRecord>>>,
    /// Pod-lifetime file-operation tracker from the builtin `tools`
    /// crate. Populated by the Controller when it registers the builtin
    /// tools so that Pod-owned operations (e.g. compaction) can consult
    /// the recency of touched files.
    tracker: Option<tools::Tracker>,
    /// Pod-lifetime task store from the builtin `tools` crate. Shared by
    /// TaskCreate / TaskUpdate / TaskList / TaskGet and preserved across
    /// compaction by keeping the same handle while the Worker history is
    /// replaced. Restored Pods reconstruct it by replaying Task* tool calls.
    task_store: tools::TaskStore,
    /// Session-lifetime counters for active-Task reminder nudges.
    /// Restored Pods start these at zero; the only consequence is a delayed
    /// first reminder after resume.
    task_reminder_state: Arc<TaskReminderState>,
    /// Parsed system-prompt template awaiting first-turn materialisation.
    /// `Some` until `ensure_system_prompt_materialized` renders it once,
    /// then `None` forever — including after compaction.
    system_prompt_template: Option<SystemPromptTemplate>,
    /// User-facing notification sink attached by the Controller at
    /// spawn time. `None` in tests / direct `Pod::new` usage.
    alerter: Option<Alerter>,
    /// Broadcast sender for typed lifecycle `Event`s (compact progress,
    /// etc.). Attached by the Controller alongside `alerter`. Unlike
    /// notifications, events sent here are NOT replayed to clients that
    /// connect after the fact — they are fire-and-forget broadcasts.
    event_tx: Option<broadcast::Sender<Event>>,
    /// Monotonic counter incremented by worker event bridges when an
    /// assistant-side execution artifact becomes visible to clients before
    /// it is necessarily committed to history (e.g. streaming text deltas).
    /// `Pod::run` uses it to avoid rolling back a turn after the UI has
    /// already observed AI output.
    ai_activity_counter: Arc<AtomicUsize>,
    /// Queue of pending `Method::Notify` notifications awaiting
    /// injection into the next LLM request. Shared with the
    /// PodInterceptor installed in `ensure_interceptor_installed`.
    pending_notifies: NotifyBuffer,
    /// Submit-scoped stash for resolver-produced system messages
    /// (currently `@<path>` file content). `Pod::run` fills this
    /// before handing off to the worker; `PodInterceptor::on_prompt_submit`
    /// drains it and returns `ContinueWith` so the items land in
    /// history right after the user message that referenced them.
    pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
    /// Scope allocation in the machine-wide lock file. `Some` for
    /// Pods built via `from_manifest` / `from_manifest_spawned` /
    /// `restore_from_manifest` (production paths); `None` for the
    /// low-level `Pod::new` constructor used in tests, which bypasses
    /// the registry. Kept purely for its `Drop` impl, which releases
    /// the allocation when the Pod is dropped.
    #[allow(dead_code)]
    scope_allocation: Option<ScopeAllocationGuard>,
    /// Socket path of the spawning Pod. `Some` only for Pods built via
    /// `from_manifest_spawned`. Consumed by the controller to fire
    /// `Method::PodEvent` reports upward (turn end, error, shutdown,
    /// scope sub-delegation).
    callback_socket: Option<PathBuf>,
    /// Central catalog of Pod-level prompt strings (compaction system
    /// prompt, notification wrapper, interrupt notes, trailing system
    /// sections, ...). Built from the 4-layer overlay in
    /// [`Self::from_manifest`], or defaults to the builtin pack when a
    /// Pod is constructed through lower-level paths that have no loader.
    prompts: Arc<PromptCatalog>,
    /// Registry loaded from `<workspace>/.insomnia/workflow/*.md` when
    /// memory is enabled. Missing memory config keeps this empty.
    workflow_registry: workflow_crate::WorkflowRegistry,
    /// Memory workspace layout used by the workflow resolver to load required
    /// Knowledge records by exact slug.
    memory_layout: Option<memory::WorkspaceLayout>,
    /// When true (default), the system-prompt assembler may append the
    /// workspace memory summary (`memory/summary.md`). Internal disposable
    /// workers disable this so resident memory exposure is opt-in per Pod.
    inject_resident_summary: bool,
    /// When true (default), the system-prompt assembler may append resident
    /// Knowledge descriptions. This is intentionally independent from
    /// summary and workflow residency: each section has its own gate.
    inject_resident_knowledge: bool,
    /// When true (default), the system-prompt assembler may append resident
    /// Workflow descriptions. This is intentionally independent from
    /// summary and Knowledge residency: each section has its own gate.
    inject_resident_workflows: bool,
    /// Latest runtime scope snapshot queued by dynamic scope changes.
    /// Drained into the session log before the next turn result is
    /// persisted, so resume never silently reclaims delegated writes.
    pending_scope_snapshot: Arc<Mutex<Option<PodScopeSnapshot>>>,
    /// extract (memory.extract) reentry guard. `true` while an extract
    /// worker is running; subsequent triggers are skipped per spec
    /// (`docs/plan/memory.md` §Extract 並走防止). `Arc<AtomicBool>` so
    /// the flag survives across `try_post_run_extract` calls without a
    /// `&mut self` race.
    extract_in_flight: Arc<AtomicBool>,
    /// consolidation (memory.consolidation) in-process reentry guard. The
    /// staging-side `StagingLock` already provides cross-process
    /// exclusion, but this AtomicBool keeps a careless concurrent caller
    /// inside the same Pod from racing on the staging snapshot.
    consolidation_in_flight: Arc<AtomicBool>,
    /// Last completed extract boundary. `None` means no extract has
    /// run yet on this session — next extract starts from entry 0.
    /// Restored from `RestoredState.extensions` on `restore`, updated
    /// after each successful extract via `save_extension`.
    extract_pointer: Arc<Mutex<Option<memory::ExtractPointerPayload>>>,
    /// extract/consolidation memory job running outside the controller method loop.
    /// The task owns the extract/consolidate worker execution and is joined
    /// at shutdown. A single slot is enough: extract/consolidation implementations loop
    /// until thresholds fall below their trigger points, and concurrent
    /// triggers are coalesced by skipping when this handle is still active.
    memory_task: Option<JoinHandle<()>>,
    /// Typed user submissions in submit order. K-th entry corresponds to
    /// the K-th `Item::user_message` in `worker.history()` (modulo seed
    /// history loaded via `SegmentStart.history`, whose original segments
    /// are not preserved). Populated from log on `restore_from_manifest`,
    /// appended after `save_user_input` on each `run`. Pre-`Event::Snapshot`
    /// this fed `PodSharedState.user_segments`; the new wire format
    /// carries typed atoms via `LogEntry::UserInput { segments }` so
    /// this remains purely an in-memory tracker for compact alignment.
    user_segments: Vec<Vec<Segment>>,
    /// Pod-side session-log mirror + broadcast sink. Populated alongside
    /// every successful `session_store::append_entry` write so connected
    /// clients see a `(snapshot, live)` stream consistent with what's
    /// on disk.
    sink: SegmentLogSink,
    /// `true` once `wire_history_persistence` has installed the
    /// `Worker::on_history_append` callback that commits each appended
    /// item as a singular `LogEntry::AssistantItem` / `ToolResult`
    /// directly through the writer. Tests that drive `Pod::new` without
    /// going through the controller leave this `false`; `persist_turn`
    /// then walks the post-`history_before` slice inline so entries
    /// still land on disk.
    history_persistence_wired: bool,
    /// Type-erased commit handle wired by the controller (or by tests
    /// via `attach_log_writer`). The interceptor uses it to commit
    /// `SystemItem`s directly without being generic over `St`. `None`
    /// in low-level test paths that bypass the controller — those
    /// paths skip SystemItem disk commits but still see the rendered
    /// `Item::system_message` in worker history.
    log_writer: Option<Arc<dyn SystemItemCommitter>>,
}

impl<C: LlmClient + 'static, St: Store + 'static> Pod<C, St> {
    pub async fn wait_for_memory_jobs(&mut self) {
        if let Some(handle) = self.memory_task.take()
            && let Err(e) = handle.await
        {
            tracing::warn!(error = %e, "Post-run memory task join failed");
        }
    }
}

impl<C: LlmClient + Clone + 'static, St: Store + Clone + 'static> Pod<C, St> {
    fn clone_for_memory_task(&self) -> Self {
        // The cloned Pod's worker exists only as a snapshot for the memory
        // task: `run_extract_once` reads `worker.history()`, and the
        // extract/consolidate workers are built fresh inside their own
        // methods using `worker.client()` as fallback when no override
        // model is configured. system_prompt / request_config / cache_key
        // are unused on this path, so we deliberately skip copying them.
        let source_worker = self.worker.as_ref().expect("worker present");
        let mut worker = Worker::new(source_worker.client().clone());
        worker.set_history(source_worker.history().to_vec());
        Self {
            manifest: self.manifest.clone(),
            worker: Some(worker),
            store: self.store.clone(),
            pod_metadata_writer: None,
            segment_state: self.segment_state.clone(),
            pwd: self.pwd.clone(),
            scope: self.scope.clone(),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: self.usage_history.clone(),
            tracker: None,
            task_store: self.task_store.clone(),
            task_reminder_state: self.task_reminder_state.clone(),
            system_prompt_template: None,
            alerter: self.alerter.clone(),
            event_tx: self.event_tx.clone(),
            ai_activity_counter: self.ai_activity_counter.clone(),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            scope_allocation: None,
            callback_socket: None,
            prompts: self.prompts.clone(),
            workflow_registry: self.workflow_registry.clone(),
            memory_layout: self.memory_layout.clone(),
            inject_resident_summary: self.inject_resident_summary,
            inject_resident_knowledge: self.inject_resident_knowledge,
            inject_resident_workflows: self.inject_resident_workflows,
            pending_scope_snapshot: self.pending_scope_snapshot.clone(),
            extract_in_flight: self.extract_in_flight.clone(),
            consolidation_in_flight: self.consolidation_in_flight.clone(),
            extract_pointer: self.extract_pointer.clone(),
            memory_task: None,
            user_segments: self.user_segments.clone(),
            // The memory-task clone never appends to the session log
            // (it only reads `worker.history()`), so a fresh sink is
            // fine — nothing observes its broadcast.
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        }
    }

    /// Build a `LogWriterHandle` carrying everything the worker
    /// callback / interceptor needs to commit `LogEntry` values
    /// directly: store handle, the shared session pointer, and the
    /// broadcast sink. All three are cheap clones.
    pub fn log_writer_handle(&self) -> LogWriterHandle<St> {
        LogWriterHandle {
            store: self.store.clone(),
            state: self.segment_state.clone(),
            sink: self.sink.clone(),
        }
    }

    /// Attach a type-erased system-item commit handle. The controller
    /// calls this once during spawn so the interceptor can commit
    /// `SystemItem`s directly without owning a generic store handle.
    /// Idempotent: subsequent calls overwrite the previous handle.
    pub fn attach_log_writer(&mut self, writer: Arc<dyn SystemItemCommitter>) {
        self.log_writer = Some(writer);
    }

    /// Wire `Worker::on_history_append` to commit each appended item
    /// directly as a singular `LogEntry::AssistantItem` / `ToolResult`
    /// through the writer. The controller calls this once per spawned
    /// Pod after the worker is built; tests that drive `Pod::new` may
    /// opt in to the same wiring or leave it off (in which case
    /// `persist_turn`'s inline fallback writes entries at turn end).
    ///
    /// `user_message` items are skipped because they are committed
    /// up-front via `commit_entry(LogEntry::UserInput { segments })`.
    /// `role:system` items are committed as typed `LogEntry::SystemItem`
    /// entries by their producers (for example `PodInterceptor` and
    /// interrupted-turn prep) before they reach the worker's history, so this
    /// callback would otherwise double-write them.
    pub fn wire_history_persistence(&mut self) {
        let writer = self.log_writer_handle();
        self.worker_mut().on_history_append(move |item| {
            if item.is_user_message() {
                return;
            }
            if matches!(
                item,
                Item::Message {
                    role: llm_worker::Role::System,
                    ..
                }
            ) {
                return;
            }
            let entry = session_store::classify_history_item(item, segment_log::now_millis());
            if let Err(err) = writer.append_entry(entry) {
                warn!(error = %err, "history append commit failed; dropping");
            }
        });
        if self.manifest.session.record_event_trace {
            let writer = self.log_writer_handle();
            self.worker_mut()
                .on_stream_event(move |turn, llm_call, event| {
                    let entry = session_store::TraceEntry {
                        ts: segment_log::now_millis(),
                        turn,
                        llm_call: Some(llm_call),
                        payload: session_store::TracePayload::StreamEvent {
                            event: event.clone(),
                        },
                    };
                    if let Err(err) = writer.append_trace(&entry) {
                        warn!(error = %err, "stream event trace commit failed; dropping");
                    }
                });
            let writer = self.log_writer_handle();
            self.worker_mut()
                .on_lifecycle_trace(move |turn, llm_call, label, data| {
                    let entry = session_store::TraceEntry {
                        ts: segment_log::now_millis(),
                        turn,
                        llm_call: Some(llm_call),
                        payload: session_store::TracePayload::Lifecycle {
                            label: label.to_string(),
                            data: data.clone(),
                        },
                    };
                    if let Err(err) = writer.append_trace(&entry) {
                        warn!(error = %err, "lifecycle trace commit failed; dropping");
                    }
                });
        }
        self.history_persistence_wired = true;
    }

    pub fn spawn_post_run_memory_jobs(&mut self) {
        // Drop a finished prior handle so we can spawn a fresh task.
        // If the prior task is still running, coalesce by skipping —
        // extract/consolidation implementations re-evaluate thresholds on completion.
        self.cleanup_finished_memory_task();
        if self.memory_task.is_some() {
            return;
        }

        let mut pod = self.clone_for_memory_task();
        self.memory_task = Some(tokio::spawn(async move {
            if let Err(e) = pod.try_post_run_extract().await {
                tracing::warn!(error = %e, "Post-run memory extract task error");
            }
            if let Err(e) = pod.try_post_run_consolidate().await {
                tracing::warn!(error = %e, "Post-run memory consolidate task error");
            }
        }));
    }
}

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// Create a new Pod from a pre-built Worker and store.
    ///
    /// Callers must pre-resolve `pwd` (absolute) and build a [`Scope`]
    /// — typically via [`Scope::from_config`] when coming from a
    /// manifest, or [`Scope::writable`] in tests.
    ///
    /// Note: this constructor does **not** parse `manifest.worker.system_prompt`
    /// as a template. `Pod::from_manifest` is the production path for
    /// templated prompts; callers of `Pod::new` that want a template
    /// should parse it themselves and call [`set_system_prompt_template`].
    pub async fn new(
        manifest: PodManifest,
        worker: Worker<C>,
        store: St,
        pwd: PathBuf,
        scope: Scope,
    ) -> Result<Self, PodError> {
        // Segment creation is deferred to `ensure_segment_head` at first
        // run so a later-installed system-prompt template (see
        // `set_system_prompt_template`) can be captured by `SegmentStart`.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let prompts = PromptCatalog::builtins_only()?;
        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            pod_metadata_writer: None,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            pwd,
            scope: SharedScope::new(scope),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::<UsageRecord>::new())),
            tracker: None,
            task_store: tools::TaskStore::new(),
            task_reminder_state: Arc::new(TaskReminderState::new()),
            system_prompt_template: None,
            alerter: None,
            event_tx: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            scope_allocation: None,
            callback_socket: None,
            prompts,
            workflow_registry: workflow_crate::WorkflowRegistry::empty(),
            memory_layout: None,
            inject_resident_summary: true,
            inject_resident_knowledge: true,
            inject_resident_workflows: true,
            pending_scope_snapshot: Arc::new(Mutex::new(None)),
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        pod.apply_permissions_from_manifest();
        pod.apply_prune_from_manifest();
        Ok(pod)
    }

    /// Install a parsed system-prompt template that will be rendered
    /// exactly once, immediately before the first LLM turn. Mirrors the
    /// path used by `Pod::from_manifest` and is exposed for tests and
    /// other callers that build a Pod without going through a manifest.
    pub fn set_system_prompt_template(&mut self, template: SystemPromptTemplate) {
        self.system_prompt_template = Some(template);
    }

    /// Toggle all resident sections in the system prompt.
    ///
    /// Default `true`: normal Pods may expose each resident section according
    /// to its own gate and manifest settings. Internal disposable workers set
    /// this to `false` so summary, Knowledge, and Workflow residency are all
    /// suppressed while explicit tools remain available.
    pub fn set_resident_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
        self.inject_resident_knowledge = enabled;
        self.inject_resident_workflows = enabled;
    }

    /// Toggle `memory/summary.md` resident injection in the system prompt.
    pub fn set_resident_summary_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
    }

    /// Toggle resident Knowledge injection in the system prompt.
    pub fn set_resident_knowledge_injection(&mut self, enabled: bool) {
        self.inject_resident_knowledge = enabled;
    }

    /// Toggle resident Workflow injection in the system prompt.
    pub fn set_resident_workflow_injection(&mut self, enabled: bool) {
        self.inject_resident_workflows = enabled;
    }

    /// Shared handle to the prompt catalog. Cheap to clone (`Arc`).
    pub fn prompts(&self) -> &Arc<PromptCatalog> {
        &self.prompts
    }

    /// The current segment ID. Read lock-free from the shared session
    /// pointer so fork-time swaps are observed immediately.
    pub fn segment_id(&self) -> SegmentId {
        self.segment_state.segment_id()
    }

    /// The Session this Pod belongs to. Stable across compaction and
    /// auto-fork (both stay within the same Session); there is no
    /// Pod-level operation today that moves a running Pod to a different
    /// Session.
    pub fn session_id(&self) -> SessionId {
        self.segment_state.session_id()
    }

    /// The Pod's manifest.
    pub fn manifest(&self) -> &PodManifest {
        &self.manifest
    }

    /// The Pod's working directory.
    pub fn pwd(&self) -> &Path {
        &self.pwd
    }

    /// The Pod's directory scope, as a shared atomically-swappable
    /// handle. Clone it to share scope state with another consumer
    /// (e.g. a tool that needs to mutate scope dynamically).
    pub fn scope(&self) -> &SharedScope {
        &self.scope
    }

    /// Snapshot the current scope as an owned `Arc<Scope>`. Subsequent
    /// scope mutations do not affect the returned snapshot.
    pub fn scope_snapshot(&self) -> Arc<Scope> {
        self.scope.snapshot()
    }

    /// Apply `extra_allow` to the Pod's runtime scope. Future tool
    /// permission checks (read/write/glob/grep) reflect the broadened
    /// scope; in-flight tool calls keep the snapshot they captured at
    /// invocation time.
    pub fn add_scope_rules(
        &self,
        extra_allow: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<(), ScopeError> {
        let extra: Vec<ScopeRule> = extra_allow.into_iter().collect();
        self.scope
            .update(|cur| cur.with_added_allow_rules(extra.clone()))
    }

    /// Strip `revoke` rules from the Pod's runtime scope by adding
    /// matching deny rules. A `Permission::Write` revoke caps effective
    /// access at `Read` (mirroring the pod-registry `effective_write`
    /// semantics — Write is the only permission tracked across Pods).
    /// A `Permission::Read` revoke removes access entirely.
    pub fn revoke_scope_rules(
        &self,
        revoke: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<(), ScopeError> {
        let revoke: Vec<ScopeRule> = revoke.into_iter().collect();
        self.scope
            .update(|cur| cur.with_added_deny_rules(revoke.clone()))
    }

    /// Snapshot the current runtime scope in the session log. The entry
    /// is intentionally appended as soon as a session log exists: if the
    /// process later exits while children keep their allocations, resume
    /// can restore the narrowed scope instead of reclaiming delegated
    /// writes.
    pub fn persist_scope_snapshot(&mut self) -> Result<(), StoreError> {
        if self.segment_state.entries_written() == 0 {
            return Ok(());
        }
        let snapshot = {
            let scope = self.scope.snapshot();
            PodScopeSnapshot {
                allow: scope.allow_rules(),
                deny: scope.deny_rules(),
            }
        };
        let payload = serde_json::to_value(&snapshot).expect("PodScopeSnapshot is Serialize");
        self.commit_entry(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: session_store::POD_SCOPE_EXTENSION_DOMAIN.into(),
            payload,
        })
    }

    /// Append `entry` to the session log AND publish it through the
    /// broadcast sink. No user-space serialization is needed across
    /// concurrent appenders — the kernel orders `O_APPEND` writes for
    /// lines smaller than `PIPE_BUF`.
    pub(crate) fn commit_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        let loc = self.segment_state.location();
        self.store.append(loc.session_id, loc.segment_id, &entry)?;
        self.segment_state.increment_entries();
        self.sink.publish(entry);
        Ok(())
    }

    /// Cloneable sink handle. Exposed to the controller so the IPC
    /// layer can `subscribe_with_snapshot` and stream entries to
    /// clients without consulting any other state.
    pub fn sink(&self) -> SegmentLogSink {
        self.sink.clone()
    }

    /// Cloneable callback handed to dynamic-scope tools. It cannot append
    /// directly to the async store from a sync tool callback, so it records
    /// the latest snapshot and the controller flushes it after the tool
    /// turn completes.
    pub fn scope_change_sink(&self) -> Arc<dyn Fn(PodScopeSnapshot) + Send + Sync> {
        let pending = self.pending_scope_snapshot.clone();
        Arc::new(move |snapshot| {
            *pending.lock().expect("pending_scope_snapshot poisoned") = Some(snapshot);
        })
    }

    fn flush_pending_scope_snapshot(&mut self) -> Result<(), StoreError> {
        let snapshot = self
            .pending_scope_snapshot
            .lock()
            .expect("pending_scope_snapshot poisoned")
            .take();
        if let Some(snapshot) = snapshot {
            let payload = serde_json::to_value(&snapshot).expect("PodScopeSnapshot is Serialize");
            self.commit_entry(LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: session_store::POD_SCOPE_EXTENSION_DOMAIN.into(),
                payload,
            })?;
        }
        Ok(())
    }

    /// Direct access to the underlying Worker.
    pub fn worker(&self) -> &Worker<C, Mutable> {
        self.worker.as_ref().expect("worker taken during run")
    }

    /// Mutable access to the underlying Worker.
    ///
    /// Use this to register tools, hooks, or subscribers before calling
    /// [`run`](Self::run).
    pub fn worker_mut(&mut self) -> &mut Worker<C, Mutable> {
        self.worker.as_mut().expect("worker taken during run")
    }

    /// Reference to the store.
    pub fn store(&self) -> &St {
        &self.store
    }

    /// List user-submitted turns in newest-first order for the manual rewind picker.
    pub fn list_rewind_targets(&self) -> Result<(usize, Vec<RewindTarget>), RewindError> {
        let loc = self.segment_state.location();
        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        Ok((
            entries.len(),
            build_rewind_targets(loc.segment_id, &entries),
        ))
    }

    /// Truncate the current segment to just before a previously listed user input.
    pub fn rewind_to(
        &mut self,
        target: RewindTargetId,
        expected_head_entries: usize,
    ) -> Result<RewindAppliedState, RewindError> {
        let loc = self.segment_state.location();
        if target.segment_id != loc.segment_id {
            return Err(RewindError::Invalid(
                "rewind target belongs to a different segment".into(),
            ));
        }

        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        if entries.len() != expected_head_entries {
            return Err(RewindError::Invalid(format!(
                "session head changed since picker opened (expected {expected_head_entries}, current {})",
                entries.len()
            )));
        }

        let Some(LogEntry::UserInput { segments, .. }) = entries.get(target.user_input_entry_index)
        else {
            return Err(RewindError::Invalid(
                "rewind target is no longer a user message".into(),
            ));
        };
        let input = segments.clone();
        let truncate_entries = rewind_truncate_entries(&entries, target.user_input_entry_index);
        let retained = entries[..truncate_entries].to_vec();
        let tool_side_effect_warning = suffix_has_tool_side_effects(&entries[truncate_entries..]);
        let state = segment_log::collect_state(&retained);
        let extract_pointer = memory::extract::fold_pointer(&state.extensions);
        let task_store = tools::TaskStore::from_history(&state.history);
        let summary = RewindSummary {
            truncated_to_entries: truncate_entries,
            discarded_entries: entries.len().saturating_sub(truncate_entries),
            tool_side_effect_warning,
        };

        self.store
            .truncate(loc.session_id, loc.segment_id, truncate_entries)?;
        self.segment_state.set_entries_written(truncate_entries);
        self.sink.truncate_silent(truncate_entries);

        self.worker_mut().set_history(state.history);
        self.worker_mut().set_request_config(state.config);
        self.worker_mut().set_turn_count(state.turn_count);
        self.worker_mut()
            .set_last_run_interrupted(state.last_run_interrupted);
        self.user_segments = state.user_segments;
        *self.usage_history.lock().expect("usage_history poisoned") = state.usage_history;
        *self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned") = Vec::new();
        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = extract_pointer;
        self.task_store = task_store;

        Ok(RewindAppliedState {
            entries: retained,
            input,
            summary,
        })
    }

    fn pod_metadata(&self, active: Option<PodActiveSegmentRef>) -> PodMetadata {
        let mut metadata = PodMetadata::new(self.manifest.pod.name.clone(), active);
        if self.manifest.profile.is_some() {
            metadata.resolved_manifest_snapshot = serde_json::to_value(&self.manifest).ok();
        }
        metadata
    }

    fn write_pod_metadata_pending(&self) -> Result<(), StoreError> {
        let Some(writer) = &self.pod_metadata_writer else {
            return Ok(());
        };
        writer(self.pod_metadata(Some(PodActiveSegmentRef::pending_segment(
            self.session_id(),
        ))))
    }

    fn write_pod_metadata_active(&self, loc: SegmentLocation) -> Result<(), StoreError> {
        let Some(writer) = &self.pod_metadata_writer else {
            return Ok(());
        };
        writer(self.pod_metadata(Some(PodActiveSegmentRef::active_segment(
            loc.session_id,
            loc.segment_id,
        ))))
    }

    /// Enable name-keyed Pod metadata write-through for Pods built through
    /// the low-level constructor. High-level manifest constructors enable it
    /// automatically; this hook lets tests and custom embedders opt into the
    /// same persistence behavior without changing `Pod::new`'s minimal bounds.
    pub fn enable_pod_metadata_write_through(&mut self) -> Result<(), StoreError>
    where
        St: PodMetadataStore + Clone + Send + Sync + 'static,
    {
        self.pod_metadata_writer = Some(pod_metadata_writer_for_store(&self.store));
        self.write_pod_metadata_pending()
    }

    /// Current history items held by the underlying Worker.
    pub fn history(&self) -> &[Item] {
        self.worker().history()
    }

    /// Snapshot of the cumulative LLM Usage measurement timeline.
    ///
    /// One entry per LLM call. Restored on `restore` and appended in
    /// `persist_turn`. Used by token-accounting APIs in [`token_counter`].
    /// Returns a clone since the underlying vector is shared with hooks
    /// running on the Worker.
    pub fn usage_history(&self) -> Vec<UsageRecord> {
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clone()
    }

    /// Snapshot of the extract (memory.extract) boundary pointer.
    ///
    /// `None` means no extract has run yet on the current session — the
    /// next extract will start from entry 0. Updated by
    /// [`try_post_run_extract`](Self::try_post_run_extract) on success
    /// and reset by [`compact`](Self::compact) (the new compacted
    /// session has a fresh log with no `LogEntry::Extension` entries).
    /// Cheap clone via `Option<Clone>`.
    /// Snapshot of the typed user segments tracked alongside worker
    /// history. The K-th entry corresponds to the K-th `Item::user_message`
    /// derived from `LogEntry::UserInput` entries (post-compaction); seed
    /// history loaded via `SegmentStart.history` does not contribute,
    /// which is acceptable because the original segments are unrecoverable.
    pub fn user_segments(&self) -> &[Vec<Segment>] {
        &self.user_segments
    }

    pub fn extract_pointer(&self) -> Option<memory::ExtractPointerPayload> {
        self.extract_pointer
            .lock()
            .expect("extract_pointer poisoned")
            .clone()
    }

    /// Test/diagnostic handle to the consolidation in-flight guard. Production
    /// callers do not need this; tests use it to assert that the reentry
    /// guard skips an in-progress consolidation without losing data.
    #[doc(hidden)]
    pub fn consolidation_in_flight_handle(&self) -> Arc<AtomicBool> {
        self.consolidation_in_flight.clone()
    }

    /// Shared handle to the cumulative Usage history.
    ///
    /// Callbacks that need live access to the latest measurements (e.g.
    /// the savings estimator that `attach_prune` installs on the Worker)
    /// clone this `Arc` and read it at request time. The handle outlives
    /// any individual run.
    ///
    /// **Locking contract:** the inner `Mutex` is held only for a short
    /// clone (`lock().unwrap().clone()`) and released immediately.
    /// Callers must not hold the guard across `.await` points, I/O, or
    /// long computations — the guard is implicitly assumed to be
    /// non-contended at every Pod lifecycle event.
    pub fn usage_history_handle(&self) -> Arc<Mutex<Vec<UsageRecord>>> {
        self.usage_history.clone()
    }

    /// Handle to the per-LLM-request `UsageTracker`.
    ///
    /// Sibling modules (e.g. the prune observer) clone this `Arc` to stash
    /// per-request side state (e.g. a `correlation_id`) that pairs with
    /// the next `LlmUsage`.
    pub(crate) fn usage_tracker_handle(&self) -> Arc<UsageTracker> {
        self.usage_tracker.clone()
    }

    /// Handle to the synchronous `MetricsTracker` buffer.
    ///
    /// Worker callbacks (e.g. the prune observer) clone this `Arc` and
    /// `.push(metric)` into it; Pod drains it in `persist_turn` and
    /// writes each metric via `session_metrics::record_metric`.
    pub(crate) fn metrics_tracker_handle(
        &self,
    ) -> Arc<crate::compact::metrics_tracker::MetricsTracker> {
        self.metrics_tracker.clone()
    }

    /// Attach the session-scoped file-operation tracker from the builtin
    /// `tools` crate. Called by the Controller immediately after it
    /// registers the builtin tools on the Worker. Overwrites any
    /// previously attached tracker.
    pub fn attach_tracker(&mut self, tracker: tools::Tracker) {
        self.tracker = Some(tracker);
    }

    /// Attach the session-scoped TaskStore from the builtin `tools` crate.
    /// Called by the Controller before registering builtin tools so the Pod
    /// and Worker share one store.
    pub fn attach_task_store(&mut self, task_store: tools::TaskStore) {
        self.task_store = task_store;
    }

    /// Shared TaskStore handle.
    pub fn task_store(&self) -> tools::TaskStore {
        self.task_store.clone()
    }

    /// The attached session-scoped file-operation tracker, if any.
    pub fn tracker(&self) -> Option<&tools::Tracker> {
        self.tracker.as_ref()
    }

    /// Attach a user-facing notification sink.
    ///
    /// Called by the Controller immediately after spawning so that
    /// Pod-internal operations (compaction failures, AGENTS.md
    /// ingestion warnings) can surface messages to connected clients.
    pub fn attach_alerter(&mut self, alerter: Alerter) {
        self.alerter = Some(alerter);
    }

    /// Attach the broadcast sender used for typed lifecycle `Event`s.
    ///
    /// The Controller wires this alongside [`attach_alerter`] so that
    /// Pod-internal operations (currently: compaction) can surface
    /// progress to connected clients.
    pub fn attach_event_tx(&mut self, event_tx: broadcast::Sender<Event>) {
        self.event_tx = Some(event_tx);
    }

    /// Shared activity counter incremented by worker event bridges when any
    /// assistant-side output is surfaced before history persistence.
    pub fn ai_activity_counter(&self) -> Arc<AtomicUsize> {
        self.ai_activity_counter.clone()
    }

    fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        if let Some(n) = self.alerter.as_ref() {
            n.alert(level, source, message);
        }
    }

    /// Append a metric, swallowing errors so observability writes never
    /// fail the surrounding turn. On failure the head hash stays put
    /// (the entry is dropped) and a `Warn` alert + `tracing::warn!` are
    /// emitted so the failure isn't completely silent.
    fn try_record_metric(&mut self, metric: &session_metrics::Metric) {
        let payload = serde_json::to_value(metric).expect("Metric is Serialize");
        let entry = LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: session_metrics::DOMAIN.into(),
            payload,
        };
        if let Err(err) = self.commit_entry(entry) {
            warn!(name = %metric.name, error = %err, "failed to record session metric; dropping");
            self.alert(
                AlertLevel::Warn,
                AlertSource::Pod,
                format!("failed to record metric `{}`: {}", metric.name, err),
            );
        }
    }

    /// Broadcast a typed `Event` to connected clients. No-op when no
    /// `event_tx` is attached (tests / direct `Pod::new` usage) or when
    /// no clients are currently subscribed.
    fn send_event(&self, event: Event) {
        if let Some(tx) = self.event_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Push a `Method::Notify` (or rendered `Method::PodEvent`) entry
    /// onto the pending buffer.
    ///
    /// The notification will be appended to `worker.history` as an
    /// `Item::system_message` just before the next LLM request, via
    /// `PodInterceptor::pending_history_appends`. See [`NotifyBuffer`]
    /// for overflow behaviour and the lane-of-record rationale.
    pub fn push_notify(&self, message: String) {
        self.pending_notifies.push_notify(message);
    }

    /// Push a typed `PodEvent` entry onto the pending buffer.
    ///
    /// Same lifecycle as [`push_notify`](Self::push_notify) but
    /// preserves the typed `PodEvent` payload so the IPC layer can
    /// emit `SystemItem::PodEvent { event, body }` with structured
    /// data for clients.
    pub fn push_pod_event_notify(&self, event: protocol::PodEvent) {
        self.pending_notifies.push_pod_event(event);
    }

    /// Shared handle to the pending notification buffer.
    ///
    /// The Controller holds a clone so that `Method::Notify` arriving
    /// while `pod.run()` is in flight can still reach the interceptor.
    pub fn notify_buffer_handle(&self) -> NotifyBuffer {
        self.pending_notifies.clone()
    }

    /// Parent callback socket set by `from_manifest_spawned`.
    ///
    /// Consumed by the Controller to fire `Method::PodEvent` upward on
    /// lifecycle transitions. `None` for top-level Pods, in which case
    /// the Controller silently skips the send.
    pub fn callback_socket(&self) -> Option<&PathBuf> {
        self.callback_socket.as_ref()
    }

    // --- Hook registration ---

    fn assert_hooks_open(&self) {
        assert!(
            !self.interceptor_installed,
            "cannot add hooks after run() or resume() has been called"
        );
    }

    /// Register a hook that runs after receiving user input.
    pub fn add_on_prompt_submit_hook(&mut self, hook: impl Hook<OnPromptSubmit> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_prompt_submit(hook);
    }

    /// Register a hook that runs before each LLM request.
    pub fn add_pre_llm_request_hook(&mut self, hook: impl Hook<PreLlmRequest> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_pre_llm_request(hook);
    }

    /// Register a hook that runs before each tool call.
    pub fn add_pre_tool_call_hook(&mut self, hook: impl Hook<PreToolCall> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_pre_tool_call(hook);
    }

    /// Register a hook that runs after each tool call.
    pub fn add_post_tool_call_hook(&mut self, hook: impl Hook<PostToolCall> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_post_tool_call(hook);
    }

    /// Register a hook that runs at the end of a turn.
    pub fn add_on_turn_end_hook(&mut self, hook: impl Hook<OnTurnEnd> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_turn_end(hook);
    }

    /// Register a hook that runs when execution is aborted.
    pub fn add_on_abort_hook(&mut self, hook: impl Hook<OnAbort> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_abort(hook);
    }

    /// Install the hook-based interceptor on the Worker if not already done.
    ///
    /// When either compaction threshold (`threshold` or
    /// `request_threshold`) is configured in the manifest, allocates
    /// a shared [`CompactState`] and wires the interceptor to read current
    /// occupancy through the `UsageRecord` timeline.
    fn ensure_interceptor_installed(&mut self) {
        if !self.interceptor_installed {
            // Pre-LLM-request hook: record the item count at send time
            // so the on_usage callback can pair it with the measured
            // input_tokens.
            self.hook_builder.add_pre_llm_request(UsageTrackingHook {
                tracker: self.usage_tracker.clone(),
            });

            let builder = std::mem::take(&mut self.hook_builder);
            let registry = Arc::new(builder.build());

            let (post_run_threshold, request_threshold, retained) = self
                .manifest
                .compaction
                .as_ref()
                .map(|c| (c.threshold, c.request_threshold, c.retained_tokens))
                .unwrap_or((None, None, manifest::defaults::COMPACT_RETAINED_TOKENS));

            let tracker_for_usage = self.usage_tracker.clone();
            self.worker_mut().on_usage(move |event| {
                tracker_for_usage.record_usage(event);
            });

            let compact_state = if post_run_threshold.is_some() || request_threshold.is_some() {
                if let (Some(post), Some(req)) = (post_run_threshold, request_threshold) {
                    if post > req {
                        warn!(
                            post_run_threshold = post,
                            request_threshold = req,
                            "threshold > request_threshold; \
                             proactive check will never fire before the safety net"
                        );
                    }
                }
                let state = Arc::new(CompactState::new(
                    post_run_threshold,
                    request_threshold,
                    retained,
                ));
                self.compact_state = Some(state.clone());
                Some(state)
            } else {
                None
            };

            let usage_history_handle = compact_state.as_ref().map(|_| self.usage_history.clone());

            let interceptor = PodInterceptor::new(
                registry,
                compact_state,
                usage_history_handle,
                self.pending_notifies.clone(),
                self.pending_attachments.clone(),
                self.task_store.clone(),
                self.task_reminder_state.clone(),
                self.prompts.clone(),
                self.log_writer.clone(),
            )
            .with_usage_tracker(self.usage_tracker.clone());
            self.worker_mut().set_interceptor(interceptor);
            self.interceptor_installed = true;
        }
    }

    /// Render the manifest-supplied instruction template exactly once,
    /// just before the first LLM turn, append the fixed trailing
    /// section (scope summary + optional AGENTS.md), and hand the
    /// resulting string to the Worker via `set_system_prompt`.
    /// Subsequent invocations are no-ops: the template field is
    /// consumed with `Option::take()`, so the materialised value
    /// persists across all later turns and compaction.
    fn ensure_system_prompt_materialized(&mut self) -> Result<(), PodError> {
        let Some(template) = self.system_prompt_template.take() else {
            return Ok(());
        };
        let alerter = self.alerter.clone();
        let tool_names: Vec<String> = {
            let worker = self.worker.as_mut().expect("worker present");
            // Materialise any pending tool factories so the template sees the
            // full list of tool names. Redundant with the flush inside
            // `Worker::lock()`; safe because `flush_pending` is idempotent.
            worker.tool_server_handle().flush_pending();
            worker
                .tool_server_handle()
                .tool_definitions_sorted()
                .into_iter()
                .map(|d| d.name)
                .collect()
        };
        let agents_md_read = read_agents_md(&self.pwd);
        for warning in agents_md_read.warnings {
            if let Some(n) = alerter.as_ref() {
                n.alert(AlertLevel::Warn, AlertSource::AgentsMd, warning);
            }
        }
        // Resident-injection collection. Each resident section has its own
        // gate so summary, Knowledge, and Workflow residency remain
        // conceptually independent. Internal workers can still opt out of all
        // resident sections by flipping all three gates.
        // Owned values live for the duration of `render` below; the
        // context borrows from them.
        let memory_layout = self.memory_layout.as_ref();
        let inject_summary = self.inject_resident_summary
            && memory_layout.is_some()
            && self
                .manifest
                .memory
                .as_ref()
                .and_then(|m| m.inject_summary)
                .unwrap_or(true);
        let resident_summary: Option<String> = if inject_summary {
            memory_layout.and_then(memory::collect_resident_summary)
        } else {
            None
        };
        let inject_resident_knowledge = self.inject_resident_knowledge && memory_layout.is_some();
        let resident: Vec<memory::ResidentKnowledgeEntry> = if inject_resident_knowledge {
            memory_layout
                .map(memory::collect_resident_knowledge)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let resident_slice: Option<&[memory::ResidentKnowledgeEntry]> = if inject_resident_knowledge
        {
            Some(&resident)
        } else {
            None
        };
        let resident_workflows: Vec<workflow_crate::ResidentWorkflowEntry> =
            if self.inject_resident_workflows {
                self.workflow_registry.resident_entries()
            } else {
                Vec::new()
            };
        let resident_workflow_slice: Option<&[workflow_crate::ResidentWorkflowEntry]> =
            if self.inject_resident_workflows {
                Some(&resident_workflows)
            } else {
                None
            };
        let resident_exposure_snapshots =
            self.resident_exposure_snapshots(&resident, &resident_workflows);
        let worker_language = worker_language(&self.manifest.worker);
        let scope_snapshot = self.scope.snapshot();
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            cwd: &self.pwd,
            language: worker_language,
            scope: &scope_snapshot,
            tool_names,
            agents_md: agents_md_read.body,
            resident_summary: resident_summary.as_deref(),
            resident_knowledge: resident_slice,
            resident_workflows: resident_workflow_slice,
            prompts: &self.prompts,
        };
        let rendered = template
            .render(&ctx)
            .map_err(|source| PodError::SystemPromptRender { source })?;
        self.worker
            .as_mut()
            .expect("worker present")
            .set_system_prompt(rendered);
        self.append_resident_exposure_event(resident_exposure_snapshots);
        Ok(())
    }

    /// Convenience: run with a single `Segment::Text`.
    ///
    /// Equivalent to `run(vec![Segment::text(s)])`. The dumb-client
    /// counterpart of [`protocol::Method::run_text`]; primarily for
    /// tests and tools that have only a string in hand.
    pub async fn run_text(&mut self, s: impl Into<String>) -> Result<PodRunResult, PodError> {
        self.run(vec![Segment::text(s)]).await
    }

    /// Drop the prior memory_task handle if it has finished. Keep it if
    /// still running so callers can decide whether to wait or coalesce.
    fn cleanup_finished_memory_task(&mut self) {
        if self.memory_task.as_ref().is_some_and(|h| h.is_finished()) {
            self.memory_task = None;
        }
    }

    /// Wait for the in-flight memory task (if any) to finish. Used before
    /// compact rewrites history (extract reads the same history).
    async fn join_memory_task(&mut self) {
        if let Some(handle) = self.memory_task.take()
            && let Err(e) = handle.await
        {
            tracing::warn!(error = %e, "Memory task join failed");
        }
    }

    /// Whether `try_pre_run_compact` would actually compact. The same
    /// check is duplicated inside `try_pre_run_compact` itself for
    /// defensive reasons; this is the gate for joining the memory task
    /// before the compact runs.
    fn should_pre_run_compact(&self) -> bool {
        self.compact_state.as_ref().is_some_and(|s| {
            !s.is_disabled()
                && !s.just_compacted()
                && s.exceeds_post_run(self.total_tokens().tokens)
        })
    }

    /// Prelude shared by `run` / `run_for_notification` / `resume`.
    /// Wires up worker hooks, ensures the session is materialized on the
    /// store, and runs pre-run compact (joining any in-flight memory task
    /// first so extract sees a stable history range).
    async fn prepare_for_run(&mut self) -> Result<(), PodError> {
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized()?;
        self.cleanup_finished_memory_task();
        self.ensure_segment_head()?;
        if self.should_pre_run_compact() {
            self.join_memory_task().await;
        }
        self.try_pre_run_compact().await;
        Ok(())
    }

    fn capture_empty_turn_rollback_snapshot(&self) -> EmptyTurnRollbackSnapshot {
        let pending_attachments = self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned")
            .clone();
        let usage_history_len = self
            .usage_history
            .lock()
            .expect("usage_history poisoned")
            .len();
        EmptyTurnRollbackSnapshot {
            history_len: self.worker().history().len(),
            user_segments_len: self.user_segments.len(),
            entries_written: self.segment_state.entries_written(),
            sink_len: self.sink.len(),
            pending_attachments,
            usage_history_len,
            ai_activity_count: self.ai_activity_counter.load(Ordering::SeqCst),
            last_run_interrupted: self.worker().last_run_interrupted(),
        }
    }

    fn should_rollback_empty_turn(
        &self,
        result: &Result<WorkerResult, WorkerError>,
        snapshot: &EmptyTurnRollbackSnapshot,
    ) -> bool {
        if !matches!(result, Err(WorkerError::Cancelled)) {
            return false;
        }
        if self.ai_activity_counter.load(Ordering::SeqCst) != snapshot.ai_activity_count {
            return false;
        }
        !self.worker().history()[snapshot.history_len..]
            .iter()
            .any(is_ai_materialized_item)
    }

    fn rollback_empty_turn(
        &mut self,
        snapshot: EmptyTurnRollbackSnapshot,
    ) -> Result<(), StoreError> {
        self.worker_mut().truncate_history(snapshot.history_len);
        self.worker_mut()
            .set_last_run_interrupted(snapshot.last_run_interrupted);
        self.user_segments.truncate(snapshot.user_segments_len);
        *self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned") = snapshot.pending_attachments;
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .truncate(snapshot.usage_history_len);
        let _ = self.usage_tracker.drain();
        let _ = self.metrics_tracker.drain();

        let loc = self.segment_state.location();
        self.store
            .truncate(loc.session_id, loc.segment_id, snapshot.entries_written)?;
        self.segment_state
            .set_entries_written(snapshot.entries_written);
        self.sink.truncate_silent(snapshot.sink_len);
        Ok(())
    }

    /// Send user input and run until the LLM turn completes.
    ///
    /// `input` is a typed segment list (see [`protocol::Segment`]). The
    /// Pod flattens it into a single user-message string for the
    /// underlying Worker, expanding paste content inline, resolving file refs
    /// into adjacent attachments where possible, and surfacing alerts for
    /// unresolved refs / unsupported segment kinds.
    ///
    /// If the between-turns compaction threshold is exceeded mid-run,
    /// the Worker is aborted, history is compacted, and execution resumes
    /// automatically.
    pub async fn run(&mut self, input: Vec<Segment>) -> Result<PodRunResult, PodError> {
        // Validate workflow invocations up front so an invalid slug
        // never commits a UserInput entry, never triggers pre-run
        // compaction, and never half-applies interrupt prep when the
        // previous turn was interrupted. Read-only against
        // `workflow_registry`.
        self.validate_workflow_invocations(&input)?;

        // Paused→Run transition: if the previous turn was cut short,
        // any `Item::ToolCall` whose tool never produced a matching
        // `ToolResult` is closed with a synthetic one, and a short
        // system note explaining the interruption is appended — so the
        // next request is wire-valid (Anthropic) and the LLM knows
        // prior work was abandoned. Driven by the worker's own
        // `last_run_interrupted` flag; `Pod::resume` reuses the prior
        // context via a different entry point and never triggers this
        // path.
        if self.worker.as_ref().unwrap().last_run_interrupted() {
            self.apply_interrupt_prep()?;
        }

        self.prepare_for_run().await?;

        let rollback_snapshot = self.capture_empty_turn_rollback_snapshot();

        // IDLE → active marker. Commits first so the next UserInput entry
        // is contained inside this Invoke range. See `tickets/invoke-turn-llmcall-semantics.md`.
        self.commit_entry(LogEntry::Invoke {
            ts: segment_log::now_millis(),
            trigger: protocol::InvokeKind::UserSend,
        })?;

        // Persist the user input as typed segments before the worker
        // pushes its flattened copy into history. save_delta deliberately
        // skips the resulting `is_user_message()` item to avoid double-write.
        self.commit_entry(LogEntry::UserInput {
            ts: segment_log::now_millis(),
            segments: input.clone(),
        })?;
        self.user_segments.push(input.clone());

        // Resolve `@<path>` refs, `#<slug>` Knowledge refs, and `/<slug>`
        // workflow invocations to system messages stashed for the
        // PodInterceptor to attach right after the user message. File and
        // Knowledge failures are non-fatal alerts; explicit workflow invocation
        // failures abort before the Worker sees the turn.
        let mut attachments = self.resolve_file_refs(&input);
        attachments.extend(self.resolve_knowledge_refs(&input));
        attachments.extend(self.resolve_workflow_invocations(&input)?);
        if !attachments.is_empty() {
            *self
                .pending_attachments
                .lock()
                .expect("pending_attachments poisoned") = attachments;
        }

        let flattened = self.flatten_segments(&input);

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → run → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.run(flattened).await;
        self.worker = Some(locked.unlock());

        if self.should_rollback_empty_turn(&result, &rollback_snapshot) {
            self.rollback_empty_turn(rollback_snapshot)?;
            return Ok(PodRunResult::RolledBack);
        }

        self.handle_worker_result(result, history_before).await
    }

    /// Resolve every `Segment::FileRef` in `segments` to a `[File: <path>]`
    /// or shallow `[Dir: <path>]` system message via `PodFsView`. Resolution
    /// failures (out-of-scope, not-found, binary, I/O, unsupported symlink
    /// directory) surface as `AlertLevel::Warn` Alerts and are skipped — the
    /// unresolved placeholder stays in the flattened user message so the LLM
    /// still sees the intent.
    fn resolve_file_refs(&self, segments: &[Segment]) -> Vec<SystemItem> {
        let view = crate::fs_view::PodFsView::new(tools::ScopedFs::with_shared_scope(
            self.scope.clone(),
            self.pwd.clone(),
        ));
        let mut out = Vec::new();
        for seg in segments {
            let Segment::FileRef { path } = seg else {
                continue;
            };
            match view.resolve_file_ref(path, self.manifest.worker.file_upload.max_bytes) {
                Ok(item) => {
                    // `resolve_file_ref` returns an `Item::system_message`
                    // whose text already carries the `[File: <path>]` or
                    // `[Dir: <path>]` header (plus any truncation hint).
                    // Persist that body verbatim — it is what the LLM
                    // actually saw, so resume produces byte-identical
                    // history.
                    let body = item.as_text().unwrap_or_default().to_string();
                    out.push(SystemItem::FileAttachment {
                        path: path.clone(),
                        body,
                    });
                }
                Err(e) => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("file ref @{path} could not be resolved: {e}"),
                    );
                }
            }
        }
        out
    }

    fn resolve_knowledge_refs(&self, segments: &[Segment]) -> Vec<SystemItem> {
        let Some(layout) = self.memory_layout.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for seg in segments {
            let Segment::KnowledgeRef { slug } = seg else {
                continue;
            };
            let parsed = match memory::Slug::parse(slug.clone()) {
                Ok(slug) => slug,
                Err(e) => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("knowledge ref #{slug} has invalid slug: {e}"),
                    );
                    continue;
                }
            };
            let path = layout.knowledge_path(&parsed);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("knowledge ref #{slug} could not be read: {e}"),
                    );
                    continue;
                }
            };
            let raw = String::from_utf8_lossy(&bytes).into_owned();
            let body_text = match memory::schema::split_frontmatter(&raw) {
                Ok((_yaml, body)) => body,
                Err(e) => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("knowledge ref #{slug} has invalid frontmatter: {e}"),
                    );
                    continue;
                }
            };
            let snapshot = memory::snapshot_record_from_bytes(
                memory::workspace::RecordKind::Knowledge,
                slug.clone(),
                &bytes,
            );
            self.append_memory_use_event(memory::UsageSource::KnowledgeRef, vec![snapshot]);
            let body = format!("[Knowledge #{}]\n{}", slug, body_text.trim_end());
            out.push(SystemItem::Knowledge {
                slug: slug.clone(),
                body,
            });
        }
        out
    }

    fn resident_exposure_snapshots(
        &self,
        knowledge: &[memory::ResidentKnowledgeEntry],
        workflows: &[workflow_crate::ResidentWorkflowEntry],
    ) -> Vec<memory::UsageRecordSnapshot> {
        let Some(layout) = self.memory_layout.as_ref() else {
            return Vec::new();
        };
        let mut snapshots = Vec::new();
        for entry in knowledge {
            match memory::snapshot_record_from_layout(
                layout,
                memory::workspace::RecordKind::Knowledge,
                &entry.slug,
            ) {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(err) => {
                    warn!(knowledge = %entry.slug, error = %err, "failed to snapshot resident knowledge exposure")
                }
            }
        }
        for entry in workflows {
            match memory::snapshot_record_from_layout(
                layout,
                memory::workspace::RecordKind::Workflow,
                &entry.slug,
            ) {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(err) => {
                    warn!(workflow = %entry.slug, error = %err, "failed to snapshot resident workflow exposure")
                }
            }
        }
        snapshots
    }

    fn append_memory_use_event(
        &self,
        source: memory::UsageSource,
        records: Vec<memory::UsageRecordSnapshot>,
    ) {
        let Some(layout) = self.memory_layout.as_ref() else {
            return;
        };
        if let Err(err) =
            memory::append_use_event(layout, self.segment_id().to_string(), source, records)
        {
            warn!(error = %err, "failed to append memory usage event");
        }
    }

    fn append_resident_exposure_event(&self, records: Vec<memory::UsageRecordSnapshot>) {
        let Some(layout) = self.memory_layout.as_ref() else {
            return;
        };
        if let Err(err) =
            memory::append_resident_exposure_event(layout, self.segment_id().to_string(), records)
        {
            warn!(error = %err, "failed to append resident exposure event");
        }
    }

    fn resolve_workflow_invocations(
        &self,
        segments: &[Segment],
    ) -> Result<Vec<SystemItem>, WorkflowResolveError> {
        let Some(layout) = self.memory_layout.as_ref() else {
            if let Some(slug) = segments.iter().find_map(|seg| match seg {
                Segment::WorkflowInvoke { slug } => Some(slug.clone()),
                _ => None,
            }) {
                return Err(WorkflowResolveError::NotFound { slug });
            }
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for seg in segments {
            let Segment::WorkflowInvoke { slug } = seg else {
                continue;
            };
            let items = crate::workflow::resolve_workflow_invocation(
                &self.workflow_registry,
                layout,
                slug,
            )?;
            match memory::snapshot_record_from_layout(
                layout,
                memory::workspace::RecordKind::Workflow,
                slug,
            ) {
                Ok(snapshot) => {
                    self.append_memory_use_event(
                        memory::UsageSource::WorkflowInvoke,
                        vec![snapshot],
                    );
                }
                Err(err) => {
                    warn!(workflow = %slug, error = %err, "failed to snapshot workflow usage");
                }
            }
            // `resolve_workflow_invocation` returns Item::system_message
            // entries (potentially multiple — body + dependency knowledge
            // bodies). Persist each as a SystemItem::Workflow keyed on
            // the invocation slug.
            for item in items {
                let body = item.as_text().unwrap_or_default().to_string();
                out.push(SystemItem::Workflow {
                    slug: slug.clone(),
                    body,
                });
            }
        }
        Ok(out)
    }

    /// Stage the post-interruption cleanup at the front of worker
    /// history: close every unanswered `Item::ToolCall` with a synthetic
    /// `Item::ToolResult` (Anthropic wire-validity), then append a
    /// system note so the LLM understands the prior turn was cut
    /// short. Called from `Pod::run` when the worker's
    /// `last_run_interrupted` flag is set (i.e. the Pod just transitioned
    /// out of Paused via a new user input).
    fn apply_interrupt_prep(&mut self) -> Result<(), PodError> {
        let tool_result_summary = self
            .prompts()
            .interrupt_tool_result_summary()
            .map_err(PodError::from)?;
        let system_note = self
            .prompts()
            .interrupt_system_note()
            .map_err(PodError::from)?;

        let closures = crate::interrupt_prep::orphan_tool_result_closures(
            self.worker().history(),
            &tool_result_summary,
        );
        if !closures.is_empty() {
            self.worker_mut().append_history(closures);
        }
        self.commit_entry(LogEntry::SystemItem {
            ts: segment_log::now_millis(),
            item: SystemItem::Interrupt {
                body: system_note.clone(),
            },
        })?;
        self.worker_mut()
            .append_history(std::iter::once(llm_worker::Item::system_message(
                system_note,
            )));
        Ok(())
    }

    /// Validate explicit workflow invocations without reading dependency
    /// bodies. Called from `Pod::run` entry so an invalid slug aborts
    /// the turn before any session-log commit or interrupt-prep side
    /// effects; `pub` so completion / preview paths can also dry-check
    /// inputs.
    pub fn validate_workflow_invocations(
        &self,
        segments: &[Segment],
    ) -> Result<(), WorkflowResolveError> {
        for seg in segments {
            let Segment::WorkflowInvoke { slug } = seg else {
                continue;
            };
            let parsed = workflow_crate::Slug::parse(slug.clone())
                .map_err(|source| WorkflowResolveError::InvalidSlug(source.into()))?;
            let record = self
                .workflow_registry
                .get(&parsed)
                .ok_or_else(|| WorkflowResolveError::NotFound { slug: slug.clone() })?;
            if !record.user_invocable {
                return Err(WorkflowResolveError::NotUserInvocable { slug: slug.clone() });
            }
        }
        Ok(())
    }

    pub fn workflow_completions(&self) -> Vec<String> {
        self.workflow_registry.list_user_invocable("")
    }

    pub fn knowledge_completions(&self) -> Vec<String> {
        self.memory_layout
            .as_ref()
            .map(memory::list_knowledge_slugs)
            .unwrap_or_default()
    }

    /// Flatten a typed segment list into the single string the Worker
    /// receives as the user message, and emit user-facing alerts for
    /// segments that fall through to placeholder (knowledge / workflow
    /// refs without a resolver, or unknown variants from a newer client).
    /// `FileRef` is handled separately by `resolve_file_refs`. The text
    /// reconstruction itself comes from `Segment::flatten_to_text`,
    /// shared with replay paths that should not re-alert.
    fn flatten_segments(&self, segments: &[Segment]) -> String {
        for seg in segments {
            match seg {
                Segment::Text { .. } | Segment::Paste { .. } | Segment::FileRef { .. } => {}
                Segment::KnowledgeRef { slug } => {
                    if self.memory_layout.is_none() {
                        self.alert(
                            AlertLevel::Warn,
                            AlertSource::Pod,
                            format!(
                                "knowledge ref #{slug} cannot be resolved \
                                 because memory is disabled; passed to LLM as placeholder"
                            ),
                        );
                    }
                }
                Segment::WorkflowInvoke { .. } => {}
                Segment::Unknown => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        "received unknown segment kind from a newer client; \
                         passed to LLM as placeholder"
                            .into(),
                    );
                }
            }
        }
        Segment::flatten_to_text(segments)
    }

    /// Run a turn triggered by `Method::Notify` while the Pod is idle.
    ///
    /// Unlike [`run`](Self::run), no user message is appended to
    /// history. The `PodInterceptor::pre_llm_request` drains the
    /// pending-notification buffer and injects each entry as an
    /// `Item::system_message` into the per-request context, then the
    /// Worker's resume path issues the LLM request without a new
    /// user turn.
    pub async fn run_for_notification(
        &mut self,
        kind: protocol::InvokeKind,
    ) -> Result<PodRunResult, PodError> {
        debug_assert!(
            matches!(
                kind,
                protocol::InvokeKind::Notify
                    | protocol::InvokeKind::PodEvent
                    | protocol::InvokeKind::SystemReminder
                    | protocol::InvokeKind::Wakeup
            ),
            "run_for_notification expects a non-UserSend InvokeKind; got {kind:?}"
        );
        self.prepare_for_run().await?;

        // IDLE → active marker for the buffered notification / pod-event
        // drain. The trailing SystemItem entries (drained by the
        // PodInterceptor) carry the actual payload.
        self.commit_entry(LogEntry::Invoke {
            ts: segment_log::now_millis(),
            trigger: kind,
        })?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<PodRunResult, PodError> {
        self.prepare_for_run().await?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → resume → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Ensure the session exists and the writer's tally still matches
    /// the on-disk entry count.
    ///
    /// On the first call for a Pod built via `from_manifest`, the session
    /// has not been written to the store yet — this is when we append the
    /// initial `SegmentStart` entry, carrying the system prompt that
    /// `ensure_system_prompt_materialized` has just rendered. Subsequent
    /// calls fall through to entry-count comparison, which auto-forks
    /// when another writer has appended behind our back.
    fn ensure_segment_head(&mut self) -> Result<(), PodError> {
        let w = self.worker.as_ref().unwrap();
        let loc = self.segment_state.location();
        let entries_written = self.segment_state.entries_written();
        if entries_written == 0 {
            let initial = LogEntry::SegmentStart {
                ts: segment_log::now_millis(),
                session_id: loc.session_id,
                system_prompt: w.get_system_prompt().map(String::from),
                config: w.request_config().clone(),
                history: to_logged(w.history()),
                forked_from: None,
                compacted_from: None,
            };
            self.commit_entry(initial)?;
            self.persist_scope_snapshot()?;
            self.write_pod_metadata_active(loc)?;
            return Ok(());
        }
        // Check store count + auto-fork if it drifted.
        let store_count = self
            .store
            .read_entry_count(loc.session_id, loc.segment_id)
            .map_err(PodError::from)?;
        if store_count == entries_written {
            return Ok(());
        }
        // Auto-fork within the same Session: mint a fresh Segment and
        // switch to it. The source segment is left immutable (no terminal
        // marker is written back); the fork relationship is recorded
        // forward on the new segment's `forked_from`, with `at_turn_index`
        // = the writer's current turn (its in-memory history reflects
        // state up to that turn). The new SegmentStart replaces the mirror
        // and is broadcast through the sink so existing subscribers reset
        // their view.
        let fork_segment_id = session_store::new_segment_id();
        let entry = LogEntry::SegmentStart {
            ts: segment_log::now_millis(),
            session_id: loc.session_id,
            system_prompt: w.get_system_prompt().map(String::from),
            config: w.request_config().clone(),
            history: to_logged(w.history()),
            forked_from: Some(session_store::SegmentOrigin {
                segment_id: loc.segment_id,
                at_turn_index: w.turn_count(),
            }),
            compacted_from: None,
        };
        self.store
            .create_segment(loc.session_id, fork_segment_id, &[entry.clone()])
            .map_err(PodError::from)?;
        self.segment_state.set_location(SegmentLocation {
            session_id: loc.session_id,
            segment_id: fork_segment_id,
        });
        self.segment_state.set_entries_written(1);
        self.sink.reset_with_initial(entry);
        if self.scope_allocation.is_some() {
            pod_registry::update_segment(&self.manifest.pod.name, fork_segment_id)?;
        }
        self.write_pod_metadata_active(SegmentLocation {
            session_id: loc.session_id,
            segment_id: fork_segment_id,
        })?;
        Ok(())
    }

    /// Handle Worker result: always persist the turn first, then if
    /// `Yielded`, perform compaction and resume.
    ///
    /// Persisting before compaction ensures that if compact fails, the
    /// turn is fully recorded in the old session (interrupted, outcome
    /// `Yielded`), so restore remains consistent.
    async fn handle_worker_result(
        &mut self,
        result: Result<WorkerResult, WorkerError>,
        history_before: usize,
    ) -> Result<PodRunResult, PodError> {
        self.persist_turn(history_before, &result).await?;

        if matches!(result, Ok(WorkerResult::Yielded)) {
            return self.do_compact_and_resume().await;
        }

        if result.is_ok() {
            if let Some(ref state) = self.compact_state {
                state.set_just_compacted(false);
            }
        }
        result.map(PodRunResult::from).map_err(PodError::Worker)
    }

    /// Perform compaction after a `compact_needed` abort and resume execution.
    ///
    /// Uses `Box::pin` for the recursive `resume()` call to break the
    /// async layout cycle (`run → handle_worker_result → do_compact_and_resume → resume`).
    fn do_compact_and_resume(
        &mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PodRunResult, PodError>> + Send + '_>,
    > {
        Box::pin(async move {
            // Thrash detection: if we just compacted and hit the threshold again,
            // something is wrong.
            if let Some(ref state) = self.compact_state {
                if state.just_compacted() {
                    state.set_just_compacted(false);
                    return Err(PodError::CompactThrash);
                }
            }

            let retained = self
                .compact_state
                .as_ref()
                .map(|s| s.retained_tokens())
                .unwrap_or(manifest::defaults::COMPACT_RETAINED_TOKENS);

            self.send_event(Event::CompactStart);
            match self.compact(retained).await {
                Ok(new_segment_id) => {
                    info!(
                        new_segment_id = %new_segment_id,
                        "Compaction succeeded, resuming execution"
                    );
                    self.send_event(Event::CompactDone { new_segment_id });
                    if let Some(ref state) = self.compact_state {
                        state.record_compact_success();
                    }
                    self.resume().await
                }
                Err(e) => {
                    warn!(error = %e, "Compaction failed during run");
                    self.send_event(Event::CompactFailed {
                        error: e.to_string(),
                    });
                    self.alert(
                        AlertLevel::Error,
                        AlertSource::Compactor,
                        format!("mid-run compaction failed: {e}"),
                    );
                    if let Some(ref state) = self.compact_state {
                        state.record_compact_failure();
                    }
                    Err(e)
                }
            }
        })
    }

    /// Attempt proactive compaction at the beginning of a controller Run.
    ///
    /// This used to run in the controller's post-run path. Keeping it here
    /// preserves the ordering requirement that the next turn starts with a
    /// compacted history, without introducing a separate Busy controller state.
    /// Best-effort: failures are logged and surfaced, but do not abort the
    /// user turn that triggered the check.
    pub async fn try_pre_run_compact(&mut self) {
        let state = match self.compact_state.as_ref() {
            Some(s) if !s.is_disabled() && !s.just_compacted() => s.clone(),
            _ => return,
        };
        let current_tokens = self.total_tokens().tokens;
        if !state.exceeds_post_run(current_tokens) {
            return;
        }

        let retained = state.retained_tokens();
        self.send_event(Event::CompactStart);
        match self.compact(retained).await {
            Ok(new_segment_id) => {
                info!(
                    new_segment_id = %new_segment_id,
                    "Proactive pre-run compaction succeeded"
                );
                self.send_event(Event::CompactDone { new_segment_id });
                state.record_compact_success();
            }
            Err(e) => {
                warn!(error = %e, "Proactive pre-run compaction failed");
                self.send_event(Event::CompactFailed {
                    error: e.to_string(),
                });
                self.alert(
                    AlertLevel::Warn,
                    AlertSource::Compactor,
                    format!("pre-run compaction failed: {e}"),
                );
                state.record_compact_failure();
            }
        }
    }

    /// Run an explicit user-requested compaction between turns.
    ///
    /// The controller only calls this while Idle. Paused turns keep their
    /// interrupted Worker state intact and are intentionally rejected before
    /// this method is reached.
    pub async fn manual_compact(&mut self) -> Result<ManualCompactResult, PodError> {
        if self.manifest.compaction.is_none() {
            let message =
                "manual compact is unavailable because [compaction] is not configured".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        if self.history().is_empty() {
            let message = "manual compact skipped: no conversation history to compact".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        self.ensure_interceptor_installed();
        self.cleanup_finished_memory_task();
        self.ensure_segment_head()?;

        let state = self.compact_state.clone();
        if state.as_ref().is_some_and(|s| s.is_disabled()) {
            let message =
                "manual compact is disabled after repeated compaction failures".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        let retained = state
            .as_ref()
            .map(|s| s.retained_tokens())
            .or_else(|| self.manifest.compaction.as_ref().map(|c| c.retained_tokens))
            .unwrap_or(manifest::defaults::COMPACT_RETAINED_TOKENS);
        let current_tokens = self.total_tokens().tokens;
        let cut = self.split_for_retained(retained);
        if cut.index == 0 {
            let message = format!(
                "manual compact skipped: current context is within the retained tail ({current_tokens} <= {retained} tokens)"
            );
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        self.join_memory_task().await;
        self.send_event(Event::CompactStart);
        match self.compact(retained).await {
            Ok(new_segment_id) => {
                info!(new_segment_id = %new_segment_id, "Manual compaction succeeded");
                self.send_event(Event::CompactDone { new_segment_id });
                if let Some(ref state) = state {
                    state.record_compact_success();
                }
                Ok(ManualCompactResult::Compacted { new_segment_id })
            }
            Err(e) => {
                warn!(error = %e, "Manual compaction failed");
                self.send_event(Event::CompactFailed {
                    error: e.to_string(),
                });
                self.alert(
                    AlertLevel::Error,
                    AlertSource::Compactor,
                    format!("manual compaction failed: {e}"),
                );
                if let Some(ref state) = state {
                    state.record_compact_failure();
                }
                Err(e)
            }
        }
    }

    /// Persist delta + turn end + outcome after a run/resume.
    async fn persist_turn(
        &mut self,
        history_before: usize,
        result: &Result<WorkerResult, WorkerError>,
    ) -> Result<(), StoreError> {
        // Per-item commits for AssistantItem / ToolResult / SystemItem
        // entries are expected to have landed synchronously: the
        // worker `on_history_append` callback (wired by the controller
        // via `wire_history_persistence`) commits each appended item
        // directly through the writer, and the interceptor commits
        // SystemItem entries up-front in `on_prompt_submit` /
        // `pending_history_appends` before returning the matching
        // `Item::system_message`s.
        //
        // Low-level test paths that build `Pod::new` without wiring
        // the callback fall through this branch: they classify the
        // slice from `history_before` inline so the test's
        // `restore`-style assertions still see entries on disk.
        if !self.history_persistence_wired {
            let new_items: Vec<Item> = self.worker.as_ref().unwrap().history()[history_before..]
                .iter()
                .cloned()
                .collect();
            let ts = segment_log::now_millis();
            for item in &new_items {
                if item.is_user_message() {
                    continue;
                }
                if matches!(
                    item,
                    Item::Message {
                        role: llm_worker::Role::System,
                        ..
                    }
                ) {
                    continue;
                }
                let entry = session_store::classify_history_item(item, ts);
                self.commit_entry(entry)?;
            }
        }

        self.flush_pending_scope_snapshot()?;

        let turn_count = self.worker.as_ref().unwrap().turn_count();
        self.commit_entry(LogEntry::TurnEnd {
            ts: segment_log::now_millis(),
            turn_count,
        })?;

        // Flush any sync-buffered metrics from this run first
        // (currently `prune.fire` / `prune.skip` from the prune observer).
        // Ordered before LlmUsage so that a `prune.fire` and the
        // `prune.post_request` derived from the matching usage record
        // appear in the log close together.
        //
        // Metric writes are intentionally non-fatal: a failure here
        // surfaces as a `Warn` alert + `tracing::warn!` and the loop
        // continues. Metrics are observability data, not load-bearing
        // for run correctness, so a transient FS error must not poison
        // the turn record (`save_delta` / `save_turn_end` already landed
        // by this point, and `save_run_completed` still needs to land).
        let pending_metrics = self.metrics_tracker.drain();
        for metric in pending_metrics {
            self.try_record_metric(&metric);
        }

        // Persist any LLM Usage measurements collected during this run.
        // One LogEntry::LlmUsage per LLM call (the tool loop may have run
        // many calls within a single Pod::run). Each is also appended to
        // the in-memory `usage_history` so token-accounting APIs see it
        // before the next run. Records carrying a `correlation_id` (set
        // by an upstream observer such as the prune projection) also get
        // a paired `prune.post_request` metric so cache_read/write can be
        // joined back to the originating event.
        let usage_records = self.usage_tracker.drain();
        for recorded in usage_records {
            let crate::compact::usage_tracker::RecordedUsage {
                record,
                correlation_id,
            } = recorded;
            self.commit_entry(LogEntry::LlmUsage {
                ts: segment_log::now_millis(),
                history_len: record.history_len,
                input_total_tokens: record.input_total_tokens,
                cache_read_tokens: record.cache_read_tokens,
                cache_write_tokens: record.cache_write_tokens,
                output_tokens: record.output_tokens,
            })?;
            if let Some(id) = correlation_id {
                let metric = session_metrics::Metric::now("prune.post_request")
                    .with_correlation_id(&id)
                    .with_value(record.cache_read_tokens as f64)
                    .with_dimension("cache_write_tokens", record.cache_write_tokens.to_string())
                    .with_dimension("history_len", record.history_len.to_string());
                self.try_record_metric(&metric);
            }
            self.usage_history
                .lock()
                .expect("usage_history poisoned")
                .push(record);
        }

        let interrupted = self.worker.as_ref().unwrap().last_run_interrupted();
        match result {
            Ok(r) => {
                self.commit_entry(LogEntry::RunCompleted {
                    ts: segment_log::now_millis(),
                    interrupted,
                    result: r.clone(),
                })?;
            }
            Err(e) => {
                self.commit_entry(LogEntry::RunErrored {
                    ts: segment_log::now_millis(),
                    interrupted,
                    message: e.to_string(),
                })?;
            }
        }

        Ok(())
    }

    /// Compact the current session by summarising history via a
    /// disposable Worker, then replacing history with
    /// `[summary, ...recent_turns]` and creating a new session.
    ///
    /// The summary Worker uses:
    /// - `compaction.model` from the manifest if configured, or
    /// - a clone of the main LlmClient via `clone_boxed()`.
    ///
    /// Returns the new session ID.
    pub async fn compact(&mut self, retained_tokens: u64) -> Result<SegmentId, PodError> {
        use crate::compact::worker::{
            CompactWorkerContext, CompactWorkerInterceptor, add_reference_tool,
            mark_read_required_tool, read_session_items_tool, search_session_log_tool,
            write_summary_tool,
        };
        use crate::fs_view::PodFsView;

        // Decide the cut point by projecting the UsageRecord timeline onto
        // the current history: keep the tail whose estimated token count is
        // within `retained_tokens`. Item-granular, turn boundaries ignored.
        let cut = self.split_for_retained(retained_tokens);

        let worker = self.worker.as_ref().expect("worker taken during run");
        let history = worker.history();
        let retain_from = cut.index.min(history.len());
        let retained_items = history[retain_from..].to_vec();
        let items_to_summarise = history[..retain_from].to_vec();

        // Compaction-related knobs. Fall through to manifest defaults when
        // `[compaction]` is omitted entirely.
        let (
            auto_read_budget,
            worker_context_max_tokens,
            finish_warning_remaining_tokens,
            final_reserve_tokens,
            worker_max_turns,
            overview_target_tokens,
            overview_warning_tokens,
            overview_deadline_tokens,
            summary_target_tokens,
            summary_max_tokens,
            result_context_max_tokens,
        ) = self
            .manifest
            .compaction
            .as_ref()
            .map(|c| {
                (
                    c.auto_read_budget_tokens,
                    c.worker_context_max_tokens,
                    c.finish_warning_remaining_tokens,
                    c.final_reserve_tokens,
                    c.worker_max_turns,
                    c.overview_target_tokens,
                    c.overview_warning_tokens,
                    c.overview_deadline_tokens,
                    c.summary_target_tokens,
                    c.summary_max_tokens,
                    c.result_context_max_tokens,
                )
            })
            .unwrap_or((
                manifest::defaults::COMPACT_AUTO_READ_BUDGET,
                manifest::defaults::COMPACT_WORKER_MAX_INPUT_TOKENS,
                manifest::defaults::COMPACT_FINISH_WARNING_REMAINING_TOKENS,
                manifest::defaults::COMPACT_FINAL_RESERVE_TOKENS,
                manifest::defaults::COMPACT_WORKER_MAX_TURNS,
                manifest::defaults::COMPACT_OVERVIEW_TARGET_TOKENS,
                manifest::defaults::COMPACT_OVERVIEW_WARNING_TOKENS,
                manifest::defaults::COMPACT_OVERVIEW_DEADLINE_TOKENS,
                manifest::defaults::COMPACT_SUMMARY_TARGET_TOKENS,
                manifest::defaults::COMPACT_SUMMARY_MAX_TOKENS,
                manifest::defaults::COMPACT_RESULT_CONTEXT_MAX_TOKENS,
            ));

        // Default references: the N most-recently-touched files in the
        // session, surfaced so the compact worker can inspect them and
        // decide which (if any) the next session needs.
        let default_refs: Vec<PathBuf> = self
            .tracker
            .as_ref()
            .map(|t| t.recent_files(manifest::defaults::COMPACT_DEFAULT_REFERENCE_COUNT))
            .unwrap_or_default();

        // Input text fed to the compact worker. Includes the default
        // references, current TaskStore snapshot, and the (pruned)
        // conversation text.
        let task_snapshot_text = self.task_store.snapshot_text();
        let summary_input = build_summary_input(
            &items_to_summarise,
            &default_refs,
            Some(task_snapshot_text.as_str()),
            SummaryInputOptions {
                overview_target_tokens,
                overview_warning_tokens,
                overview_deadline_tokens,
                summary_target_tokens,
            },
        );
        if summary_input.warning_exceeded {
            self.alert(
                AlertLevel::Warn,
                AlertSource::Compactor,
                format!(
                    "compact overview is larger than expected (≈{} tokens; warning threshold {})",
                    summary_input.overview_tokens, overview_warning_tokens
                ),
            );
        }
        if summary_input.deadline_fallback_used {
            self.alert(
                AlertLevel::Warn,
                AlertSource::Compactor,
                format!(
                    "compact overview exceeded deadline ({} tokens); using coarse fallback",
                    overview_deadline_tokens
                ),
            );
        }

        // Worker-side state collected by the compact worker's tool calls.
        let ctx = Arc::new(std::sync::Mutex::new(CompactWorkerContext::with_budget(
            auto_read_budget,
        )));

        // Build an independent compact worker. Scope and pwd are shared
        // with the main Pod (reads go through the same policy) but the
        // Tracker is fresh — compact-time reads must not pollute the
        // main session's recency list, which feeds `default_refs` above.
        let scoped_fs = tools::ScopedFs::with_shared_scope(self.scope.clone(), self.pwd.clone());
        let summary_tracker = tools::Tracker::new();
        let summary_client: Box<dyn LlmClient> = self.build_compactor_client()?;
        let summary_system_prompt = self
            .prompts
            .compact_system()
            .map_err(PodError::PromptCatalog)?;
        let mut summary_worker = Worker::new(summary_client).system_prompt(summary_system_prompt);
        summary_worker.set_cache_key(Some(self.segment_id().to_string()));

        // Occupancy-based input-token meter + interceptor. The tracker pairs
        // each pre-request history length with the following UsageEvent, then
        // the interceptor projects current prompt occupancy with the same
        // UsageRecord counter used by the main Pod thresholds.
        let summary_usage_tracker = Arc::new(UsageTracker::new());
        {
            let tracker = summary_usage_tracker.clone();
            summary_worker.on_usage(move |event| {
                tracker.record_usage(event);
            });
        }
        let compactor_warning_cb = self.alerter.clone().map(|alerter| {
            Arc::new(move |message: String| {
                alerter.alert(AlertLevel::Warn, AlertSource::Compactor, message);
            }) as Arc<dyn Fn(String) + Send + Sync>
        });
        summary_worker.set_interceptor(CompactWorkerInterceptor::new(
            summary_usage_tracker,
            worker_context_max_tokens,
            finish_warning_remaining_tokens,
            final_reserve_tokens,
            compactor_warning_cb,
        ));
        summary_worker.set_max_turns(worker_max_turns);

        // Tools: read_file (shared scope, fresh tracker), bounded session
        // history exploration, and compact-specific tools that populate `ctx`.
        let compact_target_items = Arc::new(items_to_summarise.clone());
        summary_worker.register_tool(tools::read_tool(scoped_fs.clone(), summary_tracker));
        summary_worker.register_tool(search_session_log_tool(compact_target_items.clone()));
        summary_worker.register_tool(read_session_items_tool(compact_target_items));
        summary_worker.register_tool(mark_read_required_tool(scoped_fs.clone(), ctx.clone()));
        summary_worker.register_tool(add_reference_tool(ctx.clone()));
        summary_worker.register_tool(write_summary_tool(ctx.clone()));

        let out = summary_worker
            .run(summary_input.text)
            .await
            .map_err(PodError::Worker)?;
        let mut locked_worker = out.worker;

        // Guard: nudge the worker once more if the expected outputs
        // (summary, and any auto-read nominations when default refs
        // existed) were not produced on the first pass. `write_summary`
        // is idempotent-by-overwrite so a second call is safe.
        let nudge = {
            let snapshot = ctx.lock().expect("compact ctx poisoned").clone();
            if snapshot.summary.is_none() {
                Some(
                    "You have not called `write_summary` yet. Deliver the structured \
                     summary now (Completed Tasks / Active Task / Key Decisions / \
                     User Directives / Current Work) and nominate any files the next \
                     session needs with `mark_read_required`."
                        .to_string(),
                )
            } else if snapshot.read_required.is_empty() && !default_refs.is_empty() {
                Some(
                    "Summary received. If any of the referenced files are required \
                     for the next session to continue the task, call \
                     `mark_read_required` on them now. Otherwise reply briefly to \
                     close out."
                        .to_string(),
                )
            } else {
                None
            }
        };
        if let Some(prompt) = nudge {
            let _ = locked_worker.run(prompt).await.map_err(PodError::Worker)?;
        }

        let mut final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
        let mut summary_text = final_ctx
            .summary
            .clone()
            .ok_or(PodError::CompactSummaryMissing)?;
        let mut summary_tokens = estimate_text_tokens(summary_text.len());
        if summary_max_tokens > 0 && summary_tokens > summary_max_tokens {
            let prompt = format!(
                "Your `write_summary` output is too large (≈{summary_tokens} tokens; max \
                 {summary_max_tokens}). Rewrite it now with `write_summary`, preserving the \
                 same five sections but making it concise. Target ≈{summary_target_tokens} tokens."
            );
            let _ = locked_worker.run(prompt).await.map_err(PodError::Worker)?;
            final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
            summary_text = final_ctx
                .summary
                .clone()
                .ok_or(PodError::CompactSummaryMissing)?;
            summary_tokens = estimate_text_tokens(summary_text.len());
            if summary_tokens > summary_max_tokens {
                return Err(PodError::CompactSummaryTooLarge {
                    tokens: summary_tokens,
                    max: summary_max_tokens,
                });
            }
        }

        // Re-read each auto-read target via the Pod FS view. Errors are
        // logged and skipped inside `render_auto_read` rather than
        // aborting compaction — a missing / moved file should not fail
        // the whole compact.
        let auto_read_messages =
            PodFsView::new(scoped_fs.clone()).render_auto_read(&final_ctx.read_required);

        // Reference list as a single system message; omitted when empty.
        let reference_message = (!final_ctx.references.is_empty()).then(|| {
            let list = final_ctx
                .references
                .iter()
                .map(|p| format!("- {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Item::system_message(format!(
                "[Referenced files — read before compaction, contents not included]\n\
                 {list}\n\
                 Use read_file to access current contents if needed."
            ))
        });

        // Count surviving user_messages before consuming `retained_items`
        // — needed to align `self.user_segments` after the swap below.
        let retained_user_msgs = retained_items
            .iter()
            .filter(|i| i.is_user_message())
            .count();

        // Build new history: [summary, ...auto-read, references, ...retained, task snapshot, TaskList synthetic call/result].
        // The TaskStore snapshot trails the retained items so that, on resume,
        // `replay_history` walks any pre-compact Task* calls preserved verbatim
        // in retained_items first and the trailing snapshot's `replace_with`
        // is the final word — pre-compact `TaskCreate` calls cannot leak as
        // duplicate entries.
        let mut new_history = Vec::with_capacity(
            1 + auto_read_messages.len()
                + 3
                + reference_message.is_some() as usize
                + retained_items.len(),
        );
        let mut compact_introduced_system_messages =
            Vec::with_capacity(2 + auto_read_messages.len() + reference_message.is_some() as usize);
        let summary_message =
            Item::system_message(format!("[Compacted context summary]\n\n{summary_text}"));
        compact_introduced_system_messages.push(summary_message.clone());
        compact_introduced_system_messages.extend(auto_read_messages.iter().cloned());
        if let Some(msg) = reference_message.as_ref() {
            compact_introduced_system_messages.push(msg.clone());
        }
        let task_snapshot_message = Item::system_message(format!(
            "[Session TaskStore snapshot]\n\n{task_snapshot_text}\n\n\
             This is the complete session task list preserved across compaction. \
             The following TaskList tool result presents the same state through the tool lane."
        ));
        compact_introduced_system_messages.push(task_snapshot_message.clone());

        new_history.push(summary_message);
        new_history.extend(auto_read_messages);
        if let Some(msg) = reference_message {
            new_history.push(msg);
        }
        new_history.extend(retained_items);
        new_history.push(task_snapshot_message);
        new_history.push(Item::tool_call("compact-tasklist", "TaskList", "{}"));
        new_history.push(Item::tool_result_with_content(
            "compact-tasklist",
            tools::task::snapshot_overview(&self.task_store.list()),
            task_snapshot_text.clone(),
        ));
        let result_estimate = llm_worker::token_counter::total_tokens(&new_history, &[]);
        if result_context_max_tokens > 0 && result_estimate.tokens > result_context_max_tokens {
            return Err(PodError::CompactResultContextTooLarge {
                tokens: result_estimate.tokens,
                max: result_context_max_tokens,
            });
        }

        // Build the SegmentStart entry for the new compacted segment.
        // Inherits the source Segment's session_id so the compacted
        // lineage stays grouped under the same Session. Atomically
        // rotate: create on disk, swap location, reset the broadcast
        // sink so existing subscribers see the new `SegmentStart
        // { compacted_from }` and reset their view.
        let new_segment_id = session_store::new_segment_id();
        let old_loc = self.segment_state.location();
        let source_turn_count = self.worker.as_ref().unwrap().turn_count();
        let w = self.worker.as_ref().unwrap();
        let entry = LogEntry::SegmentStart {
            ts: segment_log::now_millis(),
            session_id: old_loc.session_id,
            system_prompt: w.get_system_prompt().map(String::from),
            config: w.request_config().clone(),
            history: to_logged(&new_history),
            forked_from: None,
            compacted_from: Some(session_store::SegmentOrigin {
                segment_id: old_loc.segment_id,
                at_turn_index: source_turn_count,
            }),
        };
        self.store
            .create_segment(old_loc.session_id, new_segment_id, &[entry.clone()])?;
        self.segment_state.set_location(SegmentLocation {
            session_id: old_loc.session_id,
            segment_id: new_segment_id,
        });
        self.segment_state.set_entries_written(1);
        let session_start = entry;
        // Broadcast the SegmentStart through the sink. This atomically
        // resets the mirror to `[SegmentStart]` so any subscriber
        // querying after this point sees the post-compaction prefix.
        self.sink.reset_with_initial(session_start);
        // Keep pods.json pointing at the live segment_id. Without this
        // a concurrent `restore_from_manifest(new_segment_id)` would
        // see no live writer and grab the session this Pod just moved
        // into, causing two writers to race on the same jsonl. Skipped
        // when no allocation is installed (e.g. compact under
        // `Pod::new` in tests).
        if self.scope_allocation.is_some() {
            pod_registry::update_segment(&self.manifest.pod.name, new_segment_id)?;
        }
        self.write_pod_metadata_active(SegmentLocation {
            session_id: old_loc.session_id,
            segment_id: new_segment_id,
        })?;
        // Align user_segments with the post-compaction history. Items
        // before `retain_from` (now folded into the summary) lose their
        // segments; only the user_messages surviving in retained_items
        // keep them. They are always the trailing K entries of
        // `self.user_segments` because submissions are appended in order.
        let drop_n = self.user_segments.len().saturating_sub(retained_user_msgs);
        if drop_n > 0 {
            self.user_segments.drain(..drop_n);
        }

        self.worker.as_mut().unwrap().set_history(new_history);
        // Compaction-introduced system messages are part of the new
        // SegmentStart's history (broadcast above) — clients derive
        // their blocks from `SegmentStart.history`. No per-item
        // broadcast is required.
        let _ = &compact_introduced_system_messages;
        let worker = self.worker.as_mut().unwrap();
        // Anchor the prompt cache at the summary item so that Anthropic
        // can place a durable `cache_control` breakpoint there — our
        // compact layout guarantees history[0] is the summary.
        worker.set_cache_anchor(Some(0));
        // Re-key the OpenAI Responses prompt cache namespace to the new
        // segment_id so post-compact turns share a key with extract /
        // consolidate workers running in the same session.
        worker.set_cache_key(Some(new_segment_id.to_string()));
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clear();
        self.persist_scope_snapshot()?;
        // Reset extract pointer alongside usage_history: the compacted
        // session has a fresh log with no `LogEntry::Extension` entries
        // yet, so a cold restore here would set extract_pointer to None
        // via fold_pointer. The in-memory pointer must match — otherwise
        // `tokens_added_since(old_history_len)` would treat the new
        // (shorter) history as if it had already been processed, and
        // extract would stop firing for the rest of the process's
        // lifetime.
        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = None;

        Ok(new_segment_id)
    }

    /// Build the LlmClient for the compactor Worker.
    ///
    /// Uses `compaction.model` from manifest if set, otherwise clones
    /// the main client.
    fn build_compactor_client(&self) -> Result<Box<dyn LlmClient>, PodError> {
        if let Some(ref compaction) = self.manifest.compaction {
            if let Some(ref model_config) = compaction.model {
                let client = provider::build_client(model_config)?;
                return Ok(client);
            }
        }
        let worker = self.worker.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }

    /// Build the LlmClient for the extract (memory.extract) Worker.
    ///
    /// Uses `memory.extract_model` from manifest if set, otherwise clones
    /// the main client.
    fn build_extractor_client(
        &self,
        memory_cfg: &manifest::MemoryConfig,
    ) -> Result<Box<dyn LlmClient>, PodError> {
        if let Some(ref m) = memory_cfg.extract_model {
            let client = provider::build_client(m)?;
            return Ok(client);
        }
        let worker = self.worker.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }

    /// pointer 以降に増えたプロンプト全長の推定。extract trigger が
    /// 閾値判定に使う。
    ///
    /// `total_tokens_at(now) - total_tokens_at(pointer)` の差分で、
    /// compact と同じ accounting (measured / interpolated / extrapolated)
    /// に乗る。`history_len_pointer == 0` は「未抽出」扱いで現プロンプト
    /// 全長そのものが返る。
    ///
    /// 素朴な `usage_history.input_total_tokens` の合計は使わない:
    /// `input_total_tokens` は **送信時の prompt prefix 全長** であって
    /// 増分ではないので、長い turn 内の連続 LLM call では super-set を
    /// 何度も足し込んでしまい実消費の数倍に膨らむ。
    fn tokens_added_since(&self, history_len_pointer: usize) -> u64 {
        let now = self.history().len();
        let total_now = self.total_tokens_at(now).tokens;
        let total_at_pointer = self.total_tokens_at(history_len_pointer).tokens;
        total_now.saturating_sub(total_at_pointer)
    }

    /// extract (memory.extract) post-run trigger.
    ///
    /// Called by the Controller before spawning the background memory task so
    /// the extract worker sees a stable session-log entry range while compact
    /// is deferred until the next turn starts. Best-effort: failures are
    /// logged but not propagated.
    ///
    /// Behaviour follows `docs/plan/memory.md` §Extract 並走防止:
    /// in-flight 中の trigger は skip し、完了時点で閾値再評価する
    /// (the loop below). Pending state is not retained — the
    /// re-evaluation happens naturally because the in-memory pointer
    /// has advanced.
    pub async fn try_post_run_extract(&mut self) -> Result<(), PodError> {
        let Some(memory_cfg) = self.manifest.memory.clone() else {
            return Ok(());
        };
        // `Some(0)` means disabled, same as `None`. Otherwise the
        // `tokens_since >= 0` comparison would fire on every post-run.
        let Some(threshold) = memory_cfg.extract_threshold.filter(|n| *n > 0) else {
            let layout = memory::WorkspaceLayout::resolve(&memory_cfg, &self.pwd);
            let model = memory_cfg
                .extract_model
                .as_ref()
                .unwrap_or(&self.manifest.model);
            WorkerAuditBase::new(
                memory::audit::AuditWorker::MemoryExtract,
                memory::audit::AuditTrigger::TokenThreshold,
                Some(model_audit_from_manifest(model)),
            )
            .emit(
                &layout,
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Skipped,
                "extract_threshold_disabled",
                None,
                None,
                None,
            );
            return Ok(());
        };

        loop {
            // CAS the in-flight flag. If another task is already running
            // an extract for this Pod, skip per spec.
            if self
                .extract_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                let layout = memory::WorkspaceLayout::resolve(&memory_cfg, &self.pwd);
                let model = memory_cfg
                    .extract_model
                    .as_ref()
                    .unwrap_or(&self.manifest.model);
                WorkerAuditBase::new(
                    memory::audit::AuditWorker::MemoryExtract,
                    memory::audit::AuditTrigger::TokenThreshold,
                    Some(model_audit_from_manifest(model)),
                )
                .emit(
                    &layout,
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "extract_already_in_flight",
                    None,
                    None,
                    None,
                );
                return Ok(());
            }
            let result = self.run_extract_once(&memory_cfg, threshold).await;
            self.extract_in_flight.store(false, Ordering::Release);

            match result {
                Ok(ExtractDecision::Skipped) => return Ok(()),
                Ok(ExtractDecision::Completed) => {
                    // Re-evaluate threshold against the newly advanced
                    // pointer. In the current synchronous architecture
                    // this normally exits via Skipped on the next pass,
                    // but the loop is forward-looking for the case
                    // where new activity piles up while extract runs.
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "extract failed");
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("memory extract failed: {e}"),
                    );
                    return Ok(());
                }
            }
        }
    }

    /// Single extract iteration: snapshot pointer, decide whether to
    /// fire, run the worker if so, persist results and the new pointer.
    async fn run_extract_once(
        &mut self,
        memory_cfg: &manifest::MemoryConfig,
        threshold: u64,
    ) -> Result<ExtractDecision, PodError> {
        use memory::extract;

        let layout = memory::WorkspaceLayout::resolve(memory_cfg, &self.pwd);
        let model = memory_cfg
            .extract_model
            .as_ref()
            .unwrap_or(&self.manifest.model);
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::AuditTrigger::TokenThreshold,
            Some(model_audit_from_manifest(model)),
        );
        let event_tx = self.event_tx.as_ref();

        let pointer_snapshot = self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned")
            .clone();
        let processed_history_len = pointer_snapshot
            .as_ref()
            .map(|p| p.processed_through_history_len)
            .unwrap_or(0);

        let tokens_since = self.tokens_added_since(processed_history_len);
        if tokens_since < threshold {
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                format!(
                    "token_threshold_not_reached tokens_since={tokens_since} threshold={threshold}"
                ),
                None,
                None,
                None,
            );
            return Ok(ExtractDecision::Skipped);
        }

        let current_history_len = self
            .worker
            .as_ref()
            .expect("worker present")
            .history()
            .len();
        if current_history_len <= processed_history_len {
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                "no_new_history_items",
                None,
                Some(memory::audit::ExtractAudit {
                    history_range: Some([processed_history_len as u64, current_history_len as u64]),
                    ..Default::default()
                }),
                None,
            );
            return Ok(ExtractDecision::Skipped);
        }

        // Read the session log to get the current entry count. This is
        // the boundary for the source.range end_entry. Called once per
        // extract, on a small local file.
        let entries_now = self
            .store
            .read_all(self.session_id(), self.segment_id())?
            .len();
        if entries_now == 0 {
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                "empty_segment_log",
                None,
                None,
                None,
            );
            return Ok(ExtractDecision::Skipped);
        }
        let end_entry = entries_now - 1;
        let start_entry = pointer_snapshot
            .as_ref()
            .map(|p| p.processed_through_entry + 1)
            .unwrap_or(0);
        if start_entry > end_entry {
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                "no_new_segment_entries",
                None,
                Some(memory::audit::ExtractAudit {
                    session_id: Some(self.session_id().to_string()),
                    segment_id: Some(self.segment_id().to_string()),
                    entry_range: Some([start_entry as u64, end_entry as u64]),
                    history_range: Some([processed_history_len as u64, current_history_len as u64]),
                    ..Default::default()
                }),
                None,
            );
            return Ok(ExtractDecision::Skipped);
        }

        let extract_audit_base = memory::audit::ExtractAudit {
            session_id: Some(self.session_id().to_string()),
            segment_id: Some(self.segment_id().to_string()),
            entry_range: Some([start_entry as u64, end_entry as u64]),
            history_range: Some([processed_history_len as u64, current_history_len as u64]),
            ..Default::default()
        };
        audit.emit(
            &layout,
            event_tx,
            memory::audit::WorkerLifecycleStatus::Started,
            format!("token_threshold_reached tokens_since={tokens_since} threshold={threshold}"),
            None,
            Some(extract_audit_base.clone()),
            None,
        );

        let items_to_extract = self.worker.as_ref().expect("worker present").history()
            [processed_history_len..current_history_len]
            .to_vec();

        let extract_worker_max_turns = memory_cfg
            .extract_worker_max_turns
            .or(manifest::defaults::MEMORY_EXTRACT_WORKER_MAX_TURNS);

        let client = match self.build_extractor_client(memory_cfg) {
            Ok(client) => client,
            Err(err) => {
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Failed,
                    format!("client_build_failed: {err}"),
                    None,
                    Some(extract_audit_base),
                    None,
                );
                return Err(err);
            }
        };
        let memory_language = memory_language(memory_cfg);
        let extract_system_prompt = match self.prompts.memory_extract_system(memory_language) {
            Ok(prompt) => prompt,
            Err(err) => {
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Failed,
                    format!("prompt_render_failed: {err}"),
                    None,
                    Some(extract_audit_base),
                    None,
                );
                return Err(PodError::PromptCatalog(err));
            }
        };
        let mut extract_worker = Worker::new(client).system_prompt(extract_system_prompt);
        extract_worker.set_cache_key(Some(self.segment_id().to_string()));

        extract_worker.set_max_turns(extract_worker_max_turns);

        let usage_capture = Arc::new(Mutex::new(None));
        let usage_capture_for_worker = usage_capture.clone();
        extract_worker.on_usage(move |event| {
            *usage_capture_for_worker
                .lock()
                .expect("memory extract usage capture poisoned") =
                Some(usage_audit_from_event(event));
        });

        let ctx = Arc::new(extract::ExtractWorkerContext::new());
        extract_worker.register_tool(extract::write_extracted_tool(ctx.clone()));

        let input_text = extract::build_extract_input(&items_to_extract);
        if let Err(err) = extract_worker.run(input_text).await {
            let usage = usage_capture
                .lock()
                .expect("memory extract usage capture poisoned")
                .clone();
            audit.emit(
                &layout,
                event_tx,
                lifecycle_status_for_worker_error(&err),
                format!("worker_failed: {err}"),
                usage,
                Some(extract_audit_base),
                None,
            );
            return Err(PodError::Worker(err));
        }

        let payload = ctx.take_payload().unwrap_or_else(|| {
            tracing::warn!(
                "extract worker did not call write_extracted; advancing pointer with empty payload"
            );
            extract::ExtractedPayload::default()
        });

        let source_segment_id = self.segment_state.segment_id();
        let staging_id = if payload.is_empty() {
            String::new()
        } else {
            let source = memory::schema::SourceRef {
                segment_id: source_segment_id.to_string(),
                range: [start_entry as u64, end_entry as u64],
            };
            let (id, _) = match extract::write_staging(&layout, source, payload) {
                Ok(result) => result,
                Err(err) => {
                    let usage = usage_capture
                        .lock()
                        .expect("memory extract usage capture poisoned")
                        .clone();
                    audit.emit(
                        &layout,
                        event_tx,
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("staging_write_failed: {err}"),
                        usage,
                        Some(extract_audit_base),
                        None,
                    );
                    return Err(PodError::ExtractStaging(err));
                }
            };
            id.to_string()
        };

        let pointer_payload = extract::ExtractPointerPayload {
            processed_through_entry: end_entry,
            processed_through_history_len: current_history_len,
            staging_id: staging_id.clone(),
        };
        let payload_value = serde_json::to_value(&pointer_payload)
            .expect("ExtractPointerPayload is always JSON-serializable");
        self.commit_entry(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: extract::EXTRACT_DOMAIN.into(),
            payload: payload_value,
        })?;

        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = Some(pointer_payload);

        let mut extract_audit = extract_audit_base;
        if !staging_id.is_empty() {
            extract_audit.staging_count = 1;
            extract_audit.staging_ids.push(staging_id.clone());
            extract_audit.staging_paths.push(
                layout
                    .staging_dir()
                    .join(format!("{staging_id}.json"))
                    .display()
                    .to_string(),
            );
        }
        let usage = usage_capture
            .lock()
            .expect("memory extract usage capture poisoned")
            .clone();
        let reason = if staging_id.is_empty() {
            "completed_no_staging_output"
        } else {
            "completed_staging_written"
        };
        audit.emit(
            &layout,
            event_tx,
            memory::audit::WorkerLifecycleStatus::Completed,
            reason,
            usage,
            Some(extract_audit),
            None,
        );

        Ok(ExtractDecision::Completed)
    }

    /// Build the LlmClient for the consolidation (memory.consolidation) Worker.
    ///
    /// Uses `memory.consolidation_model` from manifest if set, otherwise
    /// clones the main client. Mirrors [`build_extractor_client`].
    fn build_consolidator_client(
        &self,
        memory_cfg: &manifest::MemoryConfig,
    ) -> Result<Box<dyn LlmClient>, PodError> {
        if let Some(ref m) = memory_cfg.consolidation_model {
            let client = provider::build_client(m)?;
            return Ok(client);
        }
        let worker = self.worker.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }

    /// consolidation (memory.consolidation) trigger.
    ///
    /// Intended to run from a background memory task after extract may have
    /// added staging entries. Compact is deferred until the next turn starts,
    /// so consolidation no longer blocks the controller's post-run path.
    ///
    /// Behaviour follows `docs/plan/memory.md` §Consolidation / §並走防止:
    /// the staging-side `StagingLock` enforces cross-process exclusion;
    /// `consolidation_in_flight` keeps in-process callers honest. On
    /// success, the lock is released *with* consumed-id cleanup; on
    /// worker failure, only the lock file is unlinked so the staging
    /// entries remain for a future retry.
    pub async fn try_post_run_consolidate(&mut self) -> Result<(), PodError> {
        let Some(memory_cfg) = self.manifest.memory.clone() else {
            return Ok(());
        };
        // `Some(0)` collapses to `None` — staging count / bytes always
        // satisfies `>= 0`, which would fire consolidation on every post-run.
        // Treating zero as disabled lines up with `extract_threshold` and
        // matches the "no threshold ⇒ consolidation off" invariant in the
        // ticket's §Trigger.
        let files_threshold = memory_cfg.consolidation_threshold_files.filter(|n| *n > 0);
        let bytes_threshold = memory_cfg.consolidation_threshold_bytes.filter(|n| *n > 0);
        if files_threshold.is_none() && bytes_threshold.is_none() {
            let layout = memory::WorkspaceLayout::resolve(&memory_cfg, &self.pwd);
            let model = memory_cfg
                .consolidation_model
                .as_ref()
                .unwrap_or(&self.manifest.model);
            WorkerAuditBase::new(
                memory::audit::AuditWorker::MemoryConsolidation,
                memory::audit::AuditTrigger::StagingBacklog,
                Some(model_audit_from_manifest(model)),
            )
            .emit(
                &layout,
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Skipped,
                "consolidation_threshold_disabled",
                None,
                None,
                None,
            );
            return Ok(());
        }

        loop {
            if self
                .consolidation_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                let layout = memory::WorkspaceLayout::resolve(&memory_cfg, &self.pwd);
                let model = memory_cfg
                    .consolidation_model
                    .as_ref()
                    .unwrap_or(&self.manifest.model);
                WorkerAuditBase::new(
                    memory::audit::AuditWorker::MemoryConsolidation,
                    memory::audit::AuditTrigger::StagingBacklog,
                    Some(model_audit_from_manifest(model)),
                )
                .emit(
                    &layout,
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "consolidation_already_in_flight",
                    None,
                    None,
                    None,
                );
                return Ok(());
            }
            let result = self
                .run_consolidate_once(&memory_cfg, files_threshold, bytes_threshold)
                .await;
            self.consolidation_in_flight.store(false, Ordering::Release);

            match result {
                Ok(ConsolidateDecision::Skipped) => return Ok(()),
                Ok(ConsolidateDecision::Completed) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "consolidation failed");
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("memory consolidation failed: {e}"),
                    );
                    return Ok(());
                }
            }
        }
    }

    /// Single consolidation iteration: snapshot staging, decide whether to
    /// fire, run the worker if so, release the lock and clean up consumed
    /// IDs.
    async fn run_consolidate_once(
        &mut self,
        memory_cfg: &manifest::MemoryConfig,
        files_threshold: Option<usize>,
        bytes_threshold: Option<u64>,
    ) -> Result<ConsolidateDecision, PodError> {
        use memory::consolidate;

        let layout = memory::WorkspaceLayout::resolve(memory_cfg, &self.pwd);
        let model = memory_cfg
            .consolidation_model
            .as_ref()
            .unwrap_or(&self.manifest.model);
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::AuditTrigger::StagingBacklog,
            Some(model_audit_from_manifest(model)),
        );
        let event_tx = self.event_tx.as_ref();

        let staging_snapshot = consolidate::list_staging_entries_snapshot(&layout);
        let invalid_staging_count = staging_snapshot.invalid_count;
        let entries = staging_snapshot.entries;
        if entries.is_empty() {
            let reason = if invalid_staging_count == 0 {
                "no_staging_entries".to_string()
            } else {
                format!("no_valid_staging_entries invalid={invalid_staging_count}")
            };
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                reason,
                None,
                None,
                Some(memory::audit::ConsolidationAudit {
                    invalid_staging_count,
                    ..Default::default()
                }),
            );
            return Ok(ConsolidateDecision::Skipped);
        }

        let total_files = entries.len();
        let total_bytes: u64 = entries.iter().map(|e| e.bytes).sum();
        let consumed_ids: Vec<uuid::Uuid> = entries.iter().map(|e| e.id).collect();
        let base_consolidation = memory::audit::ConsolidationAudit {
            staging_count: total_files,
            invalid_staging_count,
            staging_bytes: total_bytes,
            consumed_staging_ids: consumed_ids.iter().map(ToString::to_string).collect(),
            operations: memory::audit::OperationCounts::default(),
        };
        let files_hit = files_threshold.is_some_and(|n| total_files >= n);
        let bytes_hit = bytes_threshold.is_some_and(|n| total_bytes >= n);
        if !files_hit && !bytes_hit {
            audit.emit(
                &layout,
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                format!(
                    "threshold_not_reached files={total_files} bytes={total_bytes} files_threshold={files_threshold:?} bytes_threshold={bytes_threshold:?}"
                ),
                None,
                None,
                Some(base_consolidation),
            );
            return Ok(ConsolidateDecision::Skipped);
        }

        let lock = match consolidate::StagingLock::acquire(
            &layout,
            std::process::id(),
            self.manifest.pod.name.clone(),
            consumed_ids,
        ) {
            Ok(l) => l,
            Err(memory::consolidate::LockError::InUse { .. }) => {
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "staging_lock_in_use",
                    None,
                    None,
                    Some(base_consolidation),
                );
                return Ok(ConsolidateDecision::Skipped);
            }
            Err(e) => {
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Failed,
                    format!("staging_lock_failed: {e}"),
                    None,
                    None,
                    Some(base_consolidation),
                );
                return Err(PodError::ConsolidationLock(e));
            }
        };

        audit.emit(
            &layout,
            event_tx,
            memory::audit::WorkerLifecycleStatus::Started,
            format!("staging_threshold_reached files={total_files} bytes={total_bytes}"),
            None,
            None,
            Some(base_consolidation.clone()),
        );

        let before_records = memory::audit::snapshot_records(&layout);

        let client = match self.build_consolidator_client(memory_cfg) {
            Ok(c) => c,
            Err(e) => {
                lock.release_only();
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Failed,
                    format!("client_build_failed: {e}"),
                    None,
                    None,
                    Some(base_consolidation),
                );
                return Err(e);
            }
        };
        let memory_language = memory_language(memory_cfg);
        let consolidation_system_prompt =
            match self.prompts.memory_consolidation_system(memory_language) {
                Ok(p) => p,
                Err(e) => {
                    lock.release_only();
                    audit.emit(
                        &layout,
                        event_tx,
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("prompt_render_failed: {e}"),
                        None,
                        None,
                        Some(base_consolidation),
                    );
                    return Err(PodError::PromptCatalog(e));
                }
            };
        let mut worker = Worker::new(client).system_prompt(consolidation_system_prompt);
        worker.set_cache_key(Some(self.segment_id().to_string()));

        let usage_capture = Arc::new(Mutex::new(None));
        let usage_capture_for_worker = usage_capture.clone();
        worker.on_usage(move |event| {
            *usage_capture_for_worker
                .lock()
                .expect("memory consolidation usage capture poisoned") =
                Some(usage_audit_from_event(event));
        });

        // Memory tools are self-contained — they bypass ScopedFs and write
        // directly under the workspace via WorkspaceLayout. Resident section
        // injection is a Pod-level concern; this disposable Worker is built
        // without it by construction, in keeping with `docs/plan/memory.md`
        // §Consolidation のKnowledgeアクセス (agent pulls knowledge through
        // the search tool instead of via system-prompt residency).
        let query_cfg = memory::tool::QueryConfig::from(memory_cfg);
        worker.register_tool(memory::tool::read_tool_with_usage(
            layout.clone(),
            self.segment_id().to_string(),
        ));
        worker.register_tool(memory::tool::write_tool(layout.clone()));
        worker.register_tool(memory::tool::edit_tool(layout.clone()));
        worker.register_tool(memory::tool::delete_tool(layout.clone()));
        worker.register_tool(memory::tool::memory_query_tool(layout.clone(), query_cfg));
        worker.register_tool(memory::tool::knowledge_query_tool(
            layout.clone(),
            query_cfg,
        ));

        let tidy = consolidate::collect_tidy_hints(&layout);
        let usage_report = match memory::build_usage_report(&layout) {
            Ok(report) => report,
            Err(err) => {
                warn!(error = %err, "failed to build memory usage report for consolidation");
                memory::UsageReport::empty()
            }
        };
        let input_text =
            consolidate::build_consolidate_input(&layout, &entries, &tidy, &usage_report);

        let run_result = worker.run(input_text).await;
        let usage = usage_capture
            .lock()
            .expect("memory consolidation usage capture poisoned")
            .clone();
        match run_result {
            Ok(_) => {
                lock.release_with_cleanup(&layout);
                let after_records = memory::audit::snapshot_records(&layout);
                let mut consolidation = base_consolidation;
                consolidation.operations =
                    memory::audit::operation_counts_from_snapshots(&before_records, &after_records);
                let reason = if consolidation.operations.total_record_changes() == 0 {
                    "completed_no_record_changes"
                } else {
                    "completed_record_changes"
                };
                audit.emit(
                    &layout,
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Completed,
                    reason,
                    usage,
                    None,
                    Some(consolidation),
                );
                Ok(ConsolidateDecision::Completed)
            }
            Err(e) => {
                lock.release_only();
                audit.emit(
                    &layout,
                    event_tx,
                    lifecycle_status_for_worker_error(&e),
                    format!("worker_failed: {e}"),
                    usage,
                    None,
                    Some(base_consolidation),
                );
                Err(PodError::Worker(e))
            }
        }
    }
}

fn lifecycle_status_for_worker_error(err: &WorkerError) -> memory::audit::WorkerLifecycleStatus {
    if matches!(err, WorkerError::Cancelled) {
        memory::audit::WorkerLifecycleStatus::Cancelled
    } else {
        memory::audit::WorkerLifecycleStatus::Failed
    }
}

fn usage_audit_from_event(
    event: &llm_worker::llm_client::event::UsageEvent,
) -> memory::audit::UsageAudit {
    memory::audit::UsageAudit {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        total_tokens: event.total_tokens,
        cache_read_input_tokens: event.cache_read_input_tokens,
        cache_creation_input_tokens: event.cache_creation_input_tokens,
    }
}

fn model_audit_from_manifest(model: &manifest::ModelManifest) -> memory::audit::ModelAudit {
    memory::audit::ModelAudit {
        ref_: model.ref_.clone(),
        scheme: model.scheme.map(|scheme| format!("{scheme:?}")),
        model_id: model.model_id.clone(),
    }
}

fn emit_memory_worker_event(
    event_tx: Option<&broadcast::Sender<Event>>,
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    status: memory::audit::WorkerLifecycleStatus,
    trigger: memory::audit::AuditTrigger,
    reason: &str,
) {
    let Some(event_tx) = event_tx else {
        return;
    };
    let message = format!("memory {} {}: {reason}", worker.label(), status.label());
    let _ = event_tx.send(Event::MemoryWorker(protocol::MemoryWorkerEvent {
        worker: worker.label().to_string(),
        status: status.label().to_string(),
        run_id: run_id.to_string(),
        trigger: trigger.label().to_string(),
        reason: reason.to_string(),
        message,
        timestamp_ms: segment_log::now_millis() as i64,
    }));
}

#[derive(Debug, Clone)]
struct WorkerAuditBase {
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    trigger: memory::audit::AuditTrigger,
    model: Option<memory::audit::ModelAudit>,
}

impl WorkerAuditBase {
    fn new(
        worker: memory::audit::AuditWorker,
        trigger: memory::audit::AuditTrigger,
        model: Option<memory::audit::ModelAudit>,
    ) -> Self {
        Self {
            run_id: uuid::Uuid::now_v7(),
            worker,
            trigger,
            model,
        }
    }

    fn emit(
        &self,
        layout: &memory::WorkspaceLayout,
        event_tx: Option<&broadcast::Sender<Event>>,
        status: memory::audit::WorkerLifecycleStatus,
        reason: impl Into<String>,
        usage: Option<memory::audit::UsageAudit>,
        extract: Option<memory::audit::ExtractAudit>,
        consolidation: Option<memory::audit::ConsolidationAudit>,
    ) {
        let reason = reason.into();
        let _ = memory::audit::append_worker_lifecycle(
            layout,
            memory::audit::WorkerLifecycleAudit {
                run_id: self.run_id,
                worker: self.worker,
                status,
                trigger: self.trigger,
                reason: reason.clone(),
                model: self.model.clone(),
                usage,
                extract,
                consolidation,
            },
        );
        if should_emit_memory_worker_event(self.worker, status, &reason) {
            emit_memory_worker_event(
                event_tx,
                self.run_id,
                self.worker,
                status,
                self.trigger,
                &reason,
            );
        }
    }
}

fn should_emit_memory_worker_event(
    worker: memory::audit::AuditWorker,
    status: memory::audit::WorkerLifecycleStatus,
    reason: &str,
) -> bool {
    if worker == memory::audit::AuditWorker::MemoryConsolidation
        && status == memory::audit::WorkerLifecycleStatus::Skipped
    {
        return !is_idle_consolidation_skip_reason(reason);
    }
    true
}

fn is_idle_consolidation_skip_reason(reason: &str) -> bool {
    reason == "no_staging_entries"
        || reason == "consolidation_threshold_disabled"
        || reason.starts_with("threshold_not_reached")
}

fn memory_language(cfg: &manifest::MemoryConfig) -> &str {
    cfg.language
        .as_deref()
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .unwrap_or(manifest::defaults::MEMORY_LANGUAGE)
}

fn worker_language(cfg: &manifest::WorkerManifest) -> &str {
    let language = cfg.language.trim();
    if language.is_empty() {
        manifest::defaults::WORKER_LANGUAGE
    } else {
        language
    }
}

/// Outcome of a single extract iteration. Internal to
/// `try_post_run_extract` / `run_extract_once`.
enum ExtractDecision {
    /// Threshold not reached, or no items to extract.
    Skipped,
    /// Extract ran and pointer advanced. Caller re-evaluates threshold.
    Completed,
}

/// Outcome of a single consolidation iteration. Internal to
/// `try_post_run_consolidate` / `run_consolidate_once`.
enum ConsolidateDecision {
    /// Either threshold not met, no staging, or another Pod holds the lock.
    Skipped,
    /// Consolidation ran. Caller re-evaluates threshold against any
    /// staging entries that arrived during the run (Coalesce).
    Completed,
}

impl<St> Pod<Box<dyn LlmClient>, St>
where
    St: Store + PodMetadataStore + Clone + Send + Sync + 'static,
{
    /// Create a Pod entirely from a validated manifest.
    ///
    /// The Pod's working directory is captured once here from the
    /// process's `std::env::current_dir()` — callers that want a
    /// different cwd must `cd` before constructing the Pod (e.g. the
    /// `SpawnPod` tool sets `Command::current_dir` on the child). The
    /// captured pwd is canonicalised and validated against
    /// `manifest.scope`.
    ///
    /// `loader` is installed into the system-prompt template
    /// environment so that `{% include "name" %}` /
    /// `{% import "name" %}` references resolve against the three-layer
    /// prompt asset library.
    pub async fn from_manifest(
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
    ) -> Result<Self, PodError> {
        let mut common = prepare_pod_common(&manifest, &loader, /* parse_template */ true)?;
        let skill_shadows = std::mem::take(&mut common.skill_shadows);

        // Segment creation is deferred to the first run (see
        // `ensure_segment_head`) so the SegmentStart entry can capture
        // the rendered system prompt, not the raw template source. The
        // session_id + segment_id are allocated here so the pod-registry
        // registration can record them from the start.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();

        // Register this Pod in the machine-wide pod-registry
        // before building anything else, so a spawn that conflicts on
        // scope fails fast.
        let socket_path = dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.pod.name)
            .join("sock");
        let scope_allocation = pod_registry::install_top_level(
            manifest.pod.name.clone(),
            std::process::id(),
            socket_path,
            common.scope.allow_rules(),
            segment_id,
        )?;

        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);
        worker.set_cache_key(Some(segment_id.to_string()));
        let pod_metadata_writer = Some(pod_metadata_writer_for_store(&store));

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            pod_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            pwd: common.pwd,
            scope: SharedScope::new(common.scope),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_store: tools::TaskStore::new(),
            task_reminder_state: Arc::new(TaskReminderState::new()),
            system_prompt_template: common.system_prompt_template,
            alerter: None,
            event_tx: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            prompts: common.prompts,
            workflow_registry: common.workflow_registry,
            memory_layout: common.memory_layout,
            inject_resident_summary: true,
            inject_resident_knowledge: true,
            inject_resident_workflows: true,
            pending_scope_snapshot: Arc::new(Mutex::new(None)),
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        pod.apply_permissions_from_manifest();
        pod.apply_prune_from_manifest();
        pod.write_pod_metadata_pending()?;
        drain_skill_shadows(&pod, skill_shadows);
        Ok(pod)
    }

    /// Build a Pod spawned by another Pod (sibling process).
    ///
    /// Behaves like [`Pod::from_manifest`] but claims the scope
    /// allocation that the spawner pre-registered via
    /// [`pod_registry::delegate_scope`], rather than installing a new
    /// top-level entry. `callback_socket` carries the spawner's
    /// Unix-socket path so the spawned Pod can send `Method::Notify`
    /// back to the spawner.
    pub async fn from_manifest_spawned(
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
        callback_socket: PathBuf,
    ) -> Result<Self, PodError> {
        let mut common = prepare_pod_common(&manifest, &loader, /* parse_template */ true)?;
        let skill_shadows = std::mem::take(&mut common.skill_shadows);

        // A spawned child starts its own conversation, so it mints a
        // fresh Session rather than joining the spawner's.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let scope_allocation = pod_registry::adopt_allocation(
            manifest.pod.name.clone(),
            std::process::id(),
            segment_id,
        )?;

        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);
        worker.set_cache_key(Some(segment_id.to_string()));
        let pod_metadata_writer = Some(pod_metadata_writer_for_store(&store));

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            pod_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            pwd: common.pwd,
            scope: SharedScope::new(common.scope),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_store: tools::TaskStore::new(),
            task_reminder_state: Arc::new(TaskReminderState::new()),
            system_prompt_template: common.system_prompt_template,
            alerter: None,
            event_tx: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: Some(callback_socket),
            prompts: common.prompts,
            workflow_registry: common.workflow_registry,
            memory_layout: common.memory_layout,
            inject_resident_summary: true,
            inject_resident_knowledge: true,
            inject_resident_workflows: true,
            pending_scope_snapshot: Arc::new(Mutex::new(None)),
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        pod.apply_permissions_from_manifest();
        pod.apply_prune_from_manifest();
        pod.write_pod_metadata_pending()?;
        drain_skill_shadows(&pod, skill_shadows);
        Ok(pod)
    }

    /// Restore a Pod by resolving its name-keyed metadata to an active
    /// `(SessionId, SegmentId)` and then using the normal session-log restore
    /// path. The metadata stores only the active pointer; lineage and origin
    /// remain authoritative in the session log.
    pub async fn restore_from_pod_metadata(
        pod_name: &str,
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
    ) -> Result<Self, PodError> {
        let metadata =
            store
                .read_by_name(pod_name)?
                .ok_or_else(|| PodError::PodMetadataMissing {
                    pod_name: pod_name.to_string(),
                })?;
        let active = metadata
            .active
            .ok_or_else(|| PodError::PodMetadataInactive {
                pod_name: pod_name.to_string(),
            })?;
        let segment_id = active
            .segment_id
            .ok_or_else(|| PodError::PodMetadataPending {
                pod_name: pod_name.to_string(),
                session_id: active.session_id,
            })?;
        let manifest = match metadata.resolved_manifest_snapshot {
            Some(snapshot) => serde_json::from_value(snapshot).map_err(|source| {
                PodError::PodMetadataManifestSnapshot {
                    pod_name: pod_name.to_string(),
                    source,
                }
            })?,
            None => manifest,
        };
        Self::restore_from_manifest(active.session_id, segment_id, manifest, store, loader).await
    }

    /// Restore a Pod from an existing session log.
    ///
    /// Resolves the manifest cascade exactly like [`Self::from_manifest`]
    /// (pwd / scope / pod-registry / client / prompt catalog), seeds a
    /// fresh Worker from the source session's `RestoredState`, and
    /// reuses the same `segment_id` so subsequent turns append to the
    /// source jsonl as a continuation of the same conversation.
    ///
    /// Concurrent writers are prevented by the pod-registry:
    /// the registration carries `segment_id`, and this constructor
    /// refuses to start when `pod_registry::lookup_segment` already finds
    /// a live Pod writing to `segment_id`. So there is no need to fork —
    /// resume is "the same session, a different process owning it".
    ///
    /// `system_prompt` is replayed verbatim from the session log —
    /// templates are not re-rendered on restore so a long-running
    /// session keeps a stable cache prefix even when the manifest's
    /// instruction template would render differently today.
    pub async fn restore_from_manifest(
        session_id: SessionId,
        segment_id: SegmentId,
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
    ) -> Result<Self, PodError> {
        // Read raw entries once so we can both reconstruct state and
        // seed the broadcast sink's mirror with the same prefix that
        // sits on disk.
        let raw_entries = store.read_all(session_id, segment_id)?;
        let state = session_store::collect_state(&raw_entries);
        if state.entries_count == 0 {
            return Err(PodError::SegmentEmpty { segment_id });
        }
        let mirror_entries: Vec<LogEntry> = raw_entries.clone();
        let scope_snapshot = state
            .pod_scope
            .clone()
            .ok_or(PodError::SegmentScopeMissing { segment_id })?;

        let mut common = prepare_pod_common_with_scope(
            &manifest,
            &loader,
            /* parse_template */ false,
            ScopeConfig {
                allow: scope_snapshot.allow,
                deny: scope_snapshot.deny,
            },
        )?;
        let skill_shadows = std::mem::take(&mut common.skill_shadows);

        // Atomic: register_pod inside install_top_level rejects when
        // another live allocation already holds `segment_id`. Wrapping
        // the lookup + install inside a single `LockFileGuard` is what
        // makes "no two live Pods write to the same session log"
        // actually structural rather than a hopeful pre-check.
        let socket_path = dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.pod.name)
            .join("sock");
        let scope_allocation = pod_registry::install_top_level_with_deny(
            manifest.pod.name.clone(),
            std::process::id(),
            socket_path,
            common.scope.allow_rules(),
            common.scope.deny_rules(),
            segment_id,
        )?;

        // Build the worker and apply the manifest defaults first, then
        // overwrite the pieces the session log is authoritative for.
        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);
        worker.set_cache_key(Some(segment_id.to_string()));
        if let Some(ref prompt) = state.system_prompt {
            worker.set_system_prompt(prompt);
        }
        // A leading `Role::System` item can only come from `compact`
        // (the Pod's one and only write path that prepends a summary at
        // history[0]). Restoring the anchor lets Anthropic re-use a
        // stable cache prefix for long-lived restored sessions.
        let anchored_on_summary = matches!(
            state.history.first(),
            Some(Item::Message {
                role: llm_worker::Role::System,
                ..
            })
        );
        worker.set_history(state.history.clone());
        worker.set_request_config(state.config.clone());
        worker.set_turn_count(state.turn_count);
        worker.set_last_run_interrupted(state.last_run_interrupted);
        if anchored_on_summary {
            worker.set_cache_anchor(Some(0));
        }

        let extract_pointer = memory::extract::fold_pointer(&state.extensions);
        let task_store = tools::TaskStore::from_history(&state.history);
        let pod_metadata_writer = Some(pod_metadata_writer_for_store(&store));

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            pod_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, state.entries_count),
            pwd: common.pwd,
            scope: SharedScope::new(common.scope),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(state.usage_history)),
            tracker: None,
            task_store,
            task_reminder_state: Arc::new(TaskReminderState::new()),
            // Restore replays the saved system_prompt verbatim — no
            // template re-render on resume.
            system_prompt_template: None,
            alerter: None,
            event_tx: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            prompts: common.prompts,
            workflow_registry: common.workflow_registry,
            memory_layout: common.memory_layout,
            inject_resident_summary: true,
            inject_resident_knowledge: true,
            inject_resident_workflows: true,
            pending_scope_snapshot: Arc::new(Mutex::new(None)),
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(extract_pointer)),
            memory_task: None,
            user_segments: state.user_segments,
            // Seed the mirror with the entries we just replayed so a
            // late-attaching client sees the full prefix without an
            // extra round trip.
            sink: SegmentLogSink::with_initial(mirror_entries),
            history_persistence_wired: false,
            log_writer: None,
        };
        pod.apply_permissions_from_manifest();
        pod.apply_prune_from_manifest();
        pod.write_pod_metadata_active(SegmentLocation {
            session_id,
            segment_id,
        })?;
        drain_skill_shadows(&pod, skill_shadows);
        Ok(pod)
    }

    /// Convenience: build a Pod from a single-layer TOML manifest string.
    ///
    /// Parses the TOML into a [`PodManifestConfig`], converts to a
    /// validated [`PodManifest`] via `TryFrom`, then delegates to
    /// [`Pod::from_manifest`]. Useful for tests, debugging, and any
    /// caller that wants to skip the cascade entirely.
    pub async fn from_manifest_toml(toml: &str, store: St) -> Result<Self, PodError> {
        let config = PodManifestConfig::from_toml(toml).map_err(PodError::ManifestParse)?;
        let manifest = PodManifest::try_from(config).map_err(PodError::ManifestResolve)?;
        Self::from_manifest(manifest, store, PromptLoader::builtins_only()).await
    }
}

/// Apply worker-level manifest settings to a Worker.
///
/// Note: `system_prompt` is intentionally not applied here. It is a
/// minijinja template that is parsed by `Pod::from_manifest` and
/// rendered once at first turn in `ensure_system_prompt_materialized`.
pub fn apply_worker_manifest<C: LlmClient>(worker: &mut Worker<C>, wm: &WorkerManifest) {
    worker.set_request_config(request_config_from_worker_manifest(wm));
    worker.set_max_turns(wm.max_turns.map(|n| n.get()));
    worker.set_tool_output_limits(Some(ToolOutputLimits {
        default_max_bytes: wm.tool_output.default_max_bytes,
        per_tool: wm.tool_output.per_tool.clone(),
    }));
}

fn request_config_from_worker_manifest(wm: &WorkerManifest) -> RequestConfig {
    let mut config = RequestConfig::new();
    if let Some(max_tokens) = wm.max_tokens {
        config.max_tokens = Some(max_tokens);
    }
    if let Some(temperature) = wm.temperature {
        config.temperature = Some(temperature);
    }
    if let Some(top_p) = wm.top_p {
        config.top_p = Some(top_p);
    }
    if let Some(top_k) = wm.top_k {
        config.top_k = Some(top_k);
    }
    config.stop_sequences = wm.stop_sequences.clone();
    config.reasoning = wm.reasoning.clone();
    config
}

/// Result of a Pod run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodRunResult {
    /// The LLM finished its turn normally.
    Finished,
    /// The LLM paused (e.g. awaiting user confirmation via a hook).
    Paused,
    /// The worker reached its configured max_turns limit.
    LimitReached,
    /// The submit-time user turn was rolled back because no AI output was materialized.
    RolledBack,
}

/// Result of a manual compaction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCompactResult {
    /// The history was compacted into a new segment.
    Compacted { new_segment_id: SegmentId },
    /// No compaction was run; the message has already been surfaced as an alert.
    Skipped { message: String },
}

impl From<WorkerResult> for PodRunResult {
    fn from(r: WorkerResult) -> Self {
        match r {
            WorkerResult::Finished => PodRunResult::Finished,
            WorkerResult::Paused => PodRunResult::Paused,
            WorkerResult::LimitReached => PodRunResult::LimitReached,
            // Yielded is internal to Pod: it's always caught by
            // handle_worker_result and never converted to PodRunResult.
            WorkerResult::Yielded => unreachable!("Yielded never converts to PodRunResult"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SummaryInputOptions {
    overview_target_tokens: u64,
    overview_warning_tokens: u64,
    overview_deadline_tokens: u64,
    summary_target_tokens: u64,
}

#[derive(Debug)]
struct SummaryInputBuild {
    text: String,
    overview_tokens: u64,
    warning_exceeded: bool,
    deadline_fallback_used: bool,
}

/// Build the compact worker's input: default-reference instructions,
/// the list of recently-touched files, task snapshot, and a bounded overview
/// rather than a prefix-wide transcript.
fn build_summary_input(
    items: &[Item],
    default_refs: &[PathBuf],
    task_snapshot: Option<&str>,
    options: SummaryInputOptions,
) -> SummaryInputBuild {
    let overview = build_summary_overview(
        items,
        options.overview_target_tokens,
        options.overview_deadline_tokens,
    );
    let overview_tokens = estimate_text_tokens(overview.len());
    let warning_exceeded =
        options.overview_warning_tokens > 0 && overview_tokens > options.overview_warning_tokens;
    let deadline_fallback_used =
        options.overview_deadline_tokens > 0 && overview_tokens > options.overview_deadline_tokens;
    let overview = if deadline_fallback_used {
        build_coarse_summary_overview(items, options.overview_deadline_tokens)
    } else {
        overview
    };
    let overview_tokens = estimate_text_tokens(overview.len());

    let mut out = String::new();
    out.push_str(&format!(
        "Summarise this session into a structured summary of about {} tokens and \
         nominate files the next session needs. The conversation below is a \
         bounded overview/index, not the full transcript. Use tools to inspect \
         current files when deciding auto-read/reference output.\n\n",
        options.summary_target_tokens
    ));
    if !default_refs.is_empty() {
        out.push_str(
            "These files were touched recently in this session. Use `read_file` \
             on them as needed, then call `mark_read_required` for any whose \
             contents the next session must have, and `add_reference` for files \
             it should know about by name only.\n\n## Referenced files\n",
        );
        for p in default_refs {
            out.push_str("- ");
            out.push_str(&p.display().to_string());
            out.push('\n');
        }
        out.push('\n');
    }
    if let Some(task_snapshot) = task_snapshot {
        out.push_str(
            "## Current Session TaskStore\n\
             This is the full current task list. Use it as source material for the \
             summary, especially active (pending/inprogress) tasks, but do not edit tasks \
             from the compact worker.\n",
        );
        out.push_str(task_snapshot);
        out.push_str("\n\n");
    }
    out.push_str("## Conversation overview/index\n");
    out.push_str(&overview);
    out.push_str("\n\nWhen you are done, call `write_summary` with the final 5-section text.");

    SummaryInputBuild {
        text: out,
        overview_tokens,
        warning_exceeded,
        deadline_fallback_used,
    }
}

fn build_summary_overview(items: &[Item], target_tokens: u64, deadline_tokens: u64) -> String {
    let target_bytes = token_budget_bytes(target_tokens).max(1024);
    let deadline_bytes = token_budget_bytes(deadline_tokens).max(target_bytes);
    let mut out = String::new();
    write_overview_header(items, &mut out);
    out.push_str("\n## Recent user/assistant/system messages\n");

    let mut selected = Vec::new();
    let mut omitted_messages = 0usize;
    for (idx, item) in items.iter().enumerate().rev() {
        let Some(entry) = message_overview_entry(idx, item, 2_000) else {
            continue;
        };
        let projected = out
            .len()
            .saturating_add(selected.iter().map(String::len).sum::<usize>())
            .saturating_add(entry.len())
            .saturating_add(2);
        if projected > target_bytes && !selected.is_empty() {
            omitted_messages += 1;
            continue;
        }
        selected.push(entry);
        if projected >= target_bytes {
            break;
        }
    }
    selected.reverse();
    for entry in selected {
        out.push_str(&entry);
        out.push_str("\n\n");
    }
    if omitted_messages > 0 {
        out.push_str(&format!(
            "[Overview omitted {omitted_messages} older message(s) to stay near target.]\n\n"
        ));
    }

    append_tool_index(items, &mut out, target_bytes, deadline_bytes);
    out
}

fn build_coarse_summary_overview(items: &[Item], deadline_tokens: u64) -> String {
    let deadline_bytes = token_budget_bytes(deadline_tokens).max(1024);
    let mut out = String::new();
    write_overview_header(items, &mut out);
    out.push_str("\n## Coarse recent message index\n");
    for (idx, item) in items.iter().enumerate().rev() {
        let Some(entry) = message_overview_entry(idx, item, 240) else {
            continue;
        };
        if out.len().saturating_add(entry.len()).saturating_add(2) > deadline_bytes {
            break;
        }
        out.push_str(&entry);
        out.push_str("\n\n");
    }
    out
}

fn write_overview_header(items: &[Item], out: &mut String) {
    let mut messages = 0usize;
    let mut tool_calls = 0usize;
    let mut tool_results = 0usize;
    let mut reasoning = 0usize;
    for item in items {
        match item {
            Item::Message { .. } => messages += 1,
            Item::ToolCall { .. } => tool_calls += 1,
            Item::ToolResult { .. } => tool_results += 1,
            Item::Reasoning { .. } => reasoning += 1,
        }
    }
    out.push_str(&format!(
        "Items summarized: {} total; {messages} message(s), {tool_calls} tool call(s), \
         {tool_results} tool result(s), {reasoning} reasoning item(s). Tool call \
         arguments, tool result full content, and reasoning bodies are omitted from \
         this initial input.\n",
        items.len()
    ));
}

fn append_tool_index(items: &[Item], out: &mut String, target_bytes: usize, deadline_bytes: usize) {
    let mut entries = Vec::new();
    for (idx, item) in items.iter().enumerate().rev() {
        match item {
            Item::ToolCall { name, .. } => entries.push(format!("[{idx} ToolCall] {name}")),
            Item::ToolResult { summary, .. } => entries.push(format!(
                "[{idx} ToolResult] {}",
                truncate_chars(summary, 240)
            )),
            _ => {}
        }
        if entries.len() >= 24 {
            break;
        }
    }
    if entries.is_empty() {
        return;
    }
    entries.reverse();
    out.push_str("## Recent tool index (content omitted)\n");
    for entry in entries {
        let projected = out.len().saturating_add(entry.len()).saturating_add(1);
        if projected > deadline_bytes || (projected > target_bytes && out.contains("ToolResult")) {
            out.push_str("[Additional tool index entries omitted.]\n");
            break;
        }
        out.push_str(&entry);
        out.push('\n');
    }
}

fn message_overview_entry(idx: usize, item: &Item, max_chars: usize) -> Option<String> {
    let Item::Message { role, content, .. } = item else {
        return None;
    };
    let role_label = match role {
        llm_worker::Role::User => "User",
        llm_worker::Role::Assistant => "Assistant",
        llm_worker::Role::System => "System",
    };
    let text: String = content
        .iter()
        .map(|p| p.as_text())
        .collect::<Vec<_>>()
        .join("");
    Some(format!(
        "[{idx} {role_label}] {}",
        truncate_chars(&text, max_chars)
    ))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("… [truncated]");
    out
}

fn estimate_text_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

fn token_budget_bytes(tokens: u64) -> usize {
    tokens.saturating_mul(4).min(usize::MAX as u64) as usize
}

/// Pod errors.
#[derive(Debug, thiserror::Error)]
pub enum RewindError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug)]
pub struct RewindAppliedState {
    pub entries: Vec<LogEntry>,
    pub input: Vec<Segment>,
    pub summary: RewindSummary,
}

fn build_rewind_targets(segment_id: uuid::Uuid, entries: &[LogEntry]) -> Vec<RewindTarget> {
    let head_entries = entries.len();
    let mut turn_index = 0usize;
    let mut targets = Vec::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        if let LogEntry::UserInput { segments, ts } = entry {
            turn_index += 1;
            let truncate_entries = rewind_truncate_entries(entries, entry_index);
            let tool_warning = suffix_has_tool_side_effects(&entries[truncate_entries..]);
            targets.push(RewindTarget {
                id: RewindTargetId {
                    segment_id,
                    user_input_entry_index: entry_index,
                },
                expected_head_entries: head_entries,
                truncate_entries,
                turn_index,
                timestamp_ms: Some(*ts),
                preview: preview_segments(segments),
                eligible: true,
                disabled_reason: None,
                warning: tool_warning.then(|| {
                    "history suffix will be discarded; tool side effects are not undone".into()
                }),
            });
        }
    }
    targets.reverse();
    targets
}

fn rewind_truncate_entries(entries: &[LogEntry], user_input_entry_index: usize) -> usize {
    if user_input_entry_index > 0
        && matches!(
            entries.get(user_input_entry_index - 1),
            Some(LogEntry::Invoke { .. })
        )
    {
        user_input_entry_index - 1
    } else {
        user_input_entry_index
    }
}

fn suffix_has_tool_side_effects(entries: &[LogEntry]) -> bool {
    entries.iter().any(|entry| match entry {
        LogEntry::ToolResult { .. } => true,
        LogEntry::AssistantItem { item, .. } => logged_item_is_tool_call(item),
        _ => false,
    })
}

fn logged_item_is_tool_call(item: &session_store::LoggedItem) -> bool {
    matches!(item, session_store::LoggedItem::ToolCall { .. })
}

fn preview_segments(segments: &[Segment]) -> String {
    let mut preview = String::new();
    for segment in segments {
        if !preview.is_empty() {
            preview.push(' ');
        }
        match segment {
            Segment::Text { content } => preview.push_str(content.trim()),
            Segment::Paste { content, .. } => preview.push_str(content.trim()),
            Segment::FileRef { path } => {
                preview.push('@');
                preview.push_str(path);
            }
            Segment::KnowledgeRef { slug } => {
                preview.push('#');
                preview.push_str(slug);
            }
            Segment::WorkflowInvoke { slug } => {
                preview.push('/');
                preview.push_str(slug);
            }
            Segment::Unknown => preview.push_str("[unknown input segment]"),
        }
    }
    let preview = preview.replace(['\n', '\r'], " ");
    let mut chars = preview.chars();
    let mut out: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum PodError {
    #[error(transparent)]
    Worker(#[from] WorkerError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Scope(ScopeError),

    #[error("pwd is not readable under the configured scope: {}", .pwd.display())]
    PwdOutsideScope { pwd: PathBuf },

    #[error("failed to resolve pwd {}: {source}", .pwd.display())]
    InvalidPwd {
        pwd: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse manifest TOML: {0}")]
    ManifestParse(#[source] toml::de::Error),

    #[error("failed to resolve manifest config: {0}")]
    ManifestResolve(#[source] ResolveError),

    #[error(transparent)]
    Provider(#[from] provider::ProviderError),

    #[error("compaction thrash: context still exceeds threshold immediately after compact")]
    CompactThrash,

    #[error("compact worker did not produce a summary (write_summary was never called)")]
    CompactSummaryMissing,

    #[error("compact summary too large: {tokens} tokens exceeds max {max}")]
    CompactSummaryTooLarge { tokens: u64, max: u64 },

    #[error("compacted result context too large: {tokens} tokens exceeds max {max}")]
    CompactResultContextTooLarge { tokens: u64, max: u64 },

    #[error("invalid system prompt template: {source}")]
    InvalidSystemPromptTemplate {
        #[source]
        source: SystemPromptError,
    },

    #[error("failed to render system prompt template: {source}")]
    SystemPromptRender {
        #[source]
        source: SystemPromptError,
    },

    #[error(transparent)]
    ScopeLock(#[from] ScopeLockError),

    #[error(transparent)]
    PromptCatalog(#[from] CatalogError),

    #[error("memory extract staging write failed: {0}")]
    ExtractStaging(#[source] memory::extract::StagingError),

    #[error("memory consolidation lock acquisition failed: {0}")]
    ConsolidationLock(#[source] memory::consolidate::LockError),

    #[error("workflow load failed: {0}")]
    WorkflowLoad(#[source] workflow_crate::WorkflowLoadError),

    #[error("workflow invocation failed: {0}")]
    WorkflowResolve(#[from] WorkflowResolveError),

    #[error("session {segment_id} has no entries to restore")]
    SegmentEmpty { segment_id: SegmentId },

    #[error(
        "session {segment_id} has no persisted scope snapshot; refusing resume without explicit scope"
    )]
    SegmentScopeMissing { segment_id: SegmentId },

    #[error("pod metadata for {pod_name} was not found")]
    PodMetadataMissing { pod_name: String },

    #[error("pod metadata for {pod_name} has no active session")]
    PodMetadataInactive { pod_name: String },

    #[error(
        "pod metadata for {pod_name} points to session {session_id} but no segment is materialized yet"
    )]
    PodMetadataPending {
        pod_name: String,
        session_id: SessionId,
    },

    #[error("pod metadata for {pod_name} contains an invalid resolved manifest snapshot: {source}")]
    PodMetadataManifestSnapshot {
        pod_name: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Bundle of resources that every high-level Pod constructor needs:
/// pwd, scope, an LLM client, the prompt catalog, and (optionally) a
/// parsed system-prompt template. Built once by [`prepare_pod_common`]
/// from the manifest cascade and then split into Pod fields.
struct PodCommon {
    pwd: PathBuf,
    scope: Scope,
    client: Box<dyn LlmClient>,
    prompts: Arc<PromptCatalog>,
    workflow_registry: workflow_crate::WorkflowRegistry,
    memory_layout: Option<memory::WorkspaceLayout>,
    system_prompt_template: Option<SystemPromptTemplate>,
    /// SKILL.md shadow events surfaced during workflow-registry build.
    /// The Pod constructor drains these into the notify buffer right
    /// after the Pod is materialised so the first LLM request observes
    /// any skill ↔ workflow collisions.
    skill_shadows: Vec<workflow_crate::ShadowedSkill>,
}

/// Resolve pwd / scope / LLM client / prompt catalog from a validated
/// manifest cascade. Used by `from_manifest`, `from_manifest_spawned`,
/// and `restore_from_manifest` so they share one definition of "what
/// pieces fall out of a manifest".
///
/// `parse_template` controls whether the manifest's instruction is
/// parsed as a system-prompt template. New Pods always parse so the
/// template is rendered at first turn; restored Pods skip parsing
/// because the saved session log replays a previously-rendered
/// `system_prompt` verbatim.
fn prepare_pod_common(
    manifest: &PodManifest,
    loader: &PromptLoader,
    parse_template: bool,
) -> Result<PodCommon, PodError> {
    let pwd = current_pwd()?;
    let scope = build_scope_with_memory(manifest, &pwd)?;
    prepare_pod_common_from_scope(manifest, loader, parse_template, pwd, scope)
}

fn prepare_pod_common_with_scope(
    manifest: &PodManifest,
    loader: &PromptLoader,
    parse_template: bool,
    scope_config: ScopeConfig,
) -> Result<PodCommon, PodError> {
    let pwd = current_pwd()?;
    let scope = Scope::from_config(&scope_config).map_err(PodError::Scope)?;
    prepare_pod_common_from_scope(manifest, loader, parse_template, pwd, scope)
}

fn prepare_pod_common_from_scope(
    manifest: &PodManifest,
    loader: &PromptLoader,
    parse_template: bool,
    pwd: PathBuf,
    scope: Scope,
) -> Result<PodCommon, PodError> {
    if !scope.is_readable(&pwd) {
        return Err(PodError::PwdOutsideScope { pwd });
    }

    let client = provider::build_client(&manifest.model)?;
    let prompts = PromptCatalog::load(loader, manifest.pod.prompt_pack.as_deref())?;
    let memory_layout = manifest
        .memory
        .as_ref()
        .map(|mem| memory::WorkspaceLayout::resolve(mem, &pwd));
    let mut workflow_registry = match memory_layout.as_ref() {
        Some(layout) => workflow_crate::load_workflows(layout).map_err(PodError::WorkflowLoad)?,
        None => workflow_crate::WorkflowRegistry::empty(),
    };
    let skill_shadows = ingest_skills(&mut workflow_registry, manifest);

    let system_prompt_template = if parse_template {
        Some(
            SystemPromptTemplate::parse(&manifest.worker.instruction, loader.clone())
                .map_err(|source| PodError::InvalidSystemPromptTemplate { source })?,
        )
    } else {
        None
    };

    Ok(PodCommon {
        pwd,
        scope,
        client,
        prompts,
        workflow_registry,
        memory_layout,
        system_prompt_template,
        skill_shadows,
    })
}

/// Ingest external SKILL.md sources into the workflow registry.
///
/// Skills come exclusively from the manifest's `[skills] directories`
/// list (resolved against the manifest base directory). Internal
/// Workflows already loaded via [`workflow_crate::load_workflows`] take priority
/// over skills sharing the same slug; collisions are surfaced as
/// [`workflow_crate::ShadowedSkill`] events that the caller pushes onto the
/// Pod's notification buffer.
fn ingest_skills(
    registry: &mut workflow_crate::WorkflowRegistry,
    manifest: &PodManifest,
) -> Vec<workflow_crate::ShadowedSkill> {
    let mut shadows = Vec::new();
    let Some(skills_cfg) = manifest.skills.as_ref() else {
        return shadows;
    };
    for dir in &skills_cfg.directories {
        for skill in workflow_crate::load_skills_from_dir(dir) {
            let source = workflow_crate::WorkflowSource::Skill { dir: dir.clone() };
            let record = skill.into_workflow_record(source);
            if let Some(shadow) = registry.merge_skill(record) {
                shadows.push(shadow);
            }
        }
    }
    shadows
}

/// Drain skill-ingest shadow events into the Pod's notify buffer so the
/// first LLM request renders them as system-message attachments.
fn drain_skill_shadows<C, S>(pod: &Pod<C, S>, shadows: Vec<workflow_crate::ShadowedSkill>)
where
    C: LlmClient,
    S: Store,
{
    for shadow in shadows {
        pod.push_notify(format!("[Skill shadowed] {}", shadow.message()));
    }
}

/// Build the Pod's runtime [`Scope`] from the manifest, layering the
/// memory subsystem's deny-write rules on top when `[memory]` is
/// present, and read-allow rules for any external Agent Skills
/// directories ingested. The deny rules cap generic CRUD tools so they
/// cannot touch `<workspace>/memory/` or `<workspace>/knowledge/` while
/// the memory tools (registered separately) bypass `ScopedFs` and write
/// through `std::fs` directly. Skill directories are added at
/// `Permission::Read` so the agent can `Read` `scripts/` / `references/`
/// / `assets/` referenced by the Workflow body.
fn build_scope_with_memory(manifest: &PodManifest, pwd: &Path) -> Result<Scope, PodError> {
    let mut scope_config = manifest.scope.clone();
    if let Some(mem) = manifest.memory.as_ref() {
        let layout = memory::WorkspaceLayout::resolve(mem, pwd);
        scope_config.deny.extend(memory::deny_write_rules(&layout));
        scope_config
            .deny
            .extend(workflow_crate::deny_write_rules(&layout));
    }
    scope_config.allow.extend(skill_dir_read_rules(manifest));
    Scope::from_config(&scope_config).map_err(PodError::Scope)
}

/// Allow-rules granting `Read` access to every skill directory the Pod
/// will ingest from the manifest's `[skills] directories`. Returned
/// rules are recursive so the entire skill bundle (`SKILL.md` +
/// `scripts/` + `references/` + `assets/`) is readable.
fn skill_dir_read_rules(manifest: &PodManifest) -> Vec<ScopeRule> {
    let Some(skills_cfg) = manifest.skills.as_ref() else {
        return Vec::new();
    };
    skills_cfg
        .directories
        .iter()
        .map(|dir| ScopeRule {
            target: dir.clone(),
            permission: Permission::Read,
            recursive: true,
        })
        .collect()
}

/// Snapshot the process's current working directory as the Pod's pwd,
/// canonicalising symlinks and any `.`/`..` components. The Pod keeps
/// this value for its lifetime; changes to the process-wide cwd after
/// construction do not affect scope checks or the system prompt.
fn current_pwd() -> Result<PathBuf, PodError> {
    let cwd = std::env::current_dir().map_err(|source| PodError::InvalidPwd {
        pwd: PathBuf::from("."),
        source,
    })?;
    cwd.canonicalize()
        .map_err(|source| PodError::InvalidPwd { pwd: cwd, source })
}

#[cfg(test)]
mod memory_worker_event_tests {
    use super::*;

    #[test]
    fn suppresses_idle_consolidation_skip_worker_events() {
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "no_staging_entries",
        ));
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "threshold_not_reached files=1 bytes=64 min_files=2 min_bytes=1048576",
        ));
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "consolidation_threshold_disabled",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "no_valid_staging_entries invalid=1",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Completed,
            "completed",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "threshold_not_reached files=1",
        ));
    }
}

#[cfg(test)]
mod build_summary_prompt_tests {
    use super::*;

    fn test_summary_input(items: &[Item]) -> String {
        build_summary_input(
            items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 512,
                overview_warning_tokens: 1024,
                overview_deadline_tokens: 2048,
                summary_target_tokens: 256,
            },
        )
        .text
    }

    #[test]
    fn strips_tool_call_arguments() {
        let items = vec![Item::tool_call_json(
            "call-1",
            "read_file",
            serde_json::json!({ "path": "src/main.rs" }),
        )];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 ToolCall] read_file"));
        assert!(!prompt.contains("src/main.rs"));
    }

    #[test]
    fn strips_tool_result_content() {
        let items = vec![Item::tool_result_with_content(
            "call-1",
            "read 3 lines",
            "fn main() { println!(\"hello\"); }",
        )];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 ToolResult] read 3 lines"));
        assert!(!prompt.contains("println"));
    }

    #[test]
    fn drops_reasoning_entirely() {
        let items = vec![
            Item::user_message("hi"),
            Item::reasoning("internal deliberation"),
            Item::assistant_message("hello"),
        ];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 User] hi"));
        assert!(prompt.contains("[2 Assistant] hello"));
        assert!(!prompt.contains("Reasoning"));
        assert!(!prompt.contains("deliberation"));
    }

    #[test]
    fn overview_warning_does_not_drop_input() {
        let items = vec![Item::user_message("x".repeat(4_000))];
        let built = build_summary_input(
            &items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 10,
                overview_warning_tokens: 100,
                overview_deadline_tokens: 2_000,
                summary_target_tokens: 256,
            },
        );
        assert!(built.warning_exceeded);
        assert!(!built.deadline_fallback_used);
        assert!(built.text.contains("[0 User]"));
    }

    #[test]
    fn overview_deadline_falls_back_to_coarse_index() {
        let items = vec![Item::user_message("x".repeat(4_000))];
        let built = build_summary_input(
            &items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 10,
                overview_warning_tokens: 10,
                overview_deadline_tokens: 100,
                summary_target_tokens: 256,
            },
        );
        assert!(built.deadline_fallback_used);
        assert!(built.text.contains("## Coarse recent message index"));
    }

    #[test]
    fn worker_manifest_generation_settings_become_request_config() {
        let manifest = WorkerManifest {
            instruction: "unused".into(),
            language: manifest::defaults::WORKER_LANGUAGE.into(),
            max_tokens: Some(1024),
            max_turns: None,
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: vec!["\n\n".into(), "</stop>".into()],
            reasoning: None,
            tool_output: manifest::ToolOutputLimits::default(),
            file_upload: manifest::FileUploadLimits::default(),
        };

        let config = request_config_from_worker_manifest(&manifest);

        assert_eq!(config.max_tokens, Some(1024));
        assert_eq!(config.temperature, Some(0.2));
        assert_eq!(config.top_p, Some(0.9));
        assert_eq!(config.top_k, Some(40));
        assert_eq!(config.stop_sequences, vec!["\n\n", "</stop>"]);
    }

    #[test]
    fn keeps_user_and_assistant_messages() {
        let items = vec![
            Item::user_message("fix the bug"),
            Item::assistant_message("done"),
        ];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 User] fix the bug"));
        assert!(prompt.contains("[1 Assistant] done"));
    }

    #[derive(Clone)]
    struct NoopClient;

    #[async_trait]
    impl LlmClient for NoopClient {
        async fn stream(
            &self,
            _request: llm_worker::llm_client::Request,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                llm_worker::llm_client::event::Event,
                                llm_worker::llm_client::ClientError,
                            >,
                        > + Send,
                >,
            >,
            llm_worker::llm_client::ClientError,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    fn text_segment(text: &str) -> Segment {
        Segment::Text {
            content: text.into(),
        }
    }

    async fn rewind_test_pod() -> (tempfile::TempDir, Pod<NoopClient, session_store::FsStore>) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest_with_skills(vec![]);
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let pwd = dir.path().join("workspace");
        std::fs::create_dir_all(&pwd).unwrap();
        let scope = Scope::writable(&pwd).unwrap();
        let mut pod = Pod::new(manifest, Worker::new(NoopClient), store, pwd, scope)
            .await
            .unwrap();
        pod.ensure_segment_head().unwrap();
        (dir, pod)
    }

    fn append_test_entry(pod: &Pod<NoopClient, session_store::FsStore>, entry: LogEntry) {
        let loc = pod.segment_state.location();
        pod.store
            .append(loc.session_id, loc.segment_id, &entry)
            .unwrap();
    }

    fn append_user_turn(pod: &Pod<NoopClient, session_store::FsStore>, ts: u64, text: &str) {
        append_test_entry(
            pod,
            LogEntry::Invoke {
                ts,
                trigger: protocol::InvokeKind::UserSend,
            },
        );
        append_test_entry(
            pod,
            LogEntry::UserInput {
                ts: ts + 1,
                segments: vec![text_segment(text)],
            },
        );
        append_test_entry(
            pod,
            LogEntry::TurnEnd {
                ts: ts + 2,
                turn_count: 1,
            },
        );
    }

    #[tokio::test]
    async fn rewind_target_listing_is_newest_first_and_warns_on_tool_suffix() {
        let (_dir, pod) = rewind_test_pod().await;
        append_user_turn(&pod, 10, "first message");
        append_user_turn(&pod, 20, "second message");
        append_test_entry(
            &pod,
            LogEntry::ToolResult {
                ts: 30,
                item: session_store::LoggedItem::ToolResult {
                    call_id: "call-1".into(),
                    summary: "wrote a file".into(),
                    content: None,
                    is_error: false,
                },
            },
        );

        let (head_entries, targets) = pod.list_rewind_targets().unwrap();
        let loc = pod.segment_state.location();

        assert_eq!(
            head_entries,
            pod.store
                .read_all(loc.session_id, loc.segment_id)
                .unwrap()
                .len()
        );
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].preview, "second message");
        assert_eq!(targets[1].preview, "first message");
        assert!(
            targets[0]
                .warning
                .as_ref()
                .unwrap()
                .contains("tool side effects")
        );
    }

    #[tokio::test]
    async fn rewind_apply_truncates_log_and_restores_selected_input() {
        let (_dir, mut pod) = rewind_test_pod().await;
        append_user_turn(&pod, 10, "first message");
        append_user_turn(&pod, 20, "second message");
        append_test_entry(
            &pod,
            LogEntry::ToolResult {
                ts: 30,
                item: session_store::LoggedItem::ToolResult {
                    call_id: "call-1".into(),
                    summary: "wrote a file".into(),
                    content: None,
                    is_error: false,
                },
            },
        );
        let (head_entries, targets) = pod.list_rewind_targets().unwrap();
        let expected_truncate_entries = targets[0].truncate_entries;
        let target = targets[0].id.clone();

        let applied = pod.rewind_to(target, head_entries).unwrap();

        assert_eq!(preview_segments(&applied.input), "second message");
        assert_eq!(
            applied.summary.truncated_to_entries,
            expected_truncate_entries
        );
        assert!(applied.summary.tool_side_effect_warning);
        let loc = pod.segment_state.location();
        assert_eq!(
            pod.store
                .read_all(loc.session_id, loc.segment_id)
                .unwrap()
                .len(),
            expected_truncate_entries
        );
        assert_eq!(pod.worker().history().len(), 1);
        assert_eq!(
            pod.worker().history()[0].as_text().unwrap(),
            "first message"
        );
    }

    #[tokio::test]
    async fn rewind_apply_rejects_stale_head() {
        let (_dir, mut pod) = rewind_test_pod().await;
        append_user_turn(&pod, 10, "first message");
        let (head_entries, targets) = pod.list_rewind_targets().unwrap();
        append_user_turn(&pod, 20, "newer message");

        let err = pod
            .rewind_to(targets[0].id.clone(), head_entries)
            .unwrap_err()
            .to_string();

        assert!(err.contains("session head changed"));
    }

    #[tokio::test]
    async fn apply_interrupt_prep_appends_via_callback_and_logs_independent_entries() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest_with_skills(vec![]);
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let pwd = dir.path().join("workspace");
        std::fs::create_dir_all(&pwd).unwrap();
        let scope = Scope::writable(&pwd).unwrap();
        let mut pod = Pod::new(manifest, Worker::new(NoopClient), store, pwd, scope)
            .await
            .unwrap();

        pod.ensure_segment_head().unwrap();
        pod.wire_history_persistence();
        pod.worker_mut()
            .set_history(vec![Item::tool_call("call-1", "Read", "{}")]);

        pod.apply_interrupt_prep().unwrap();

        let history = pod.worker().history();
        assert_eq!(history.len(), 3);
        assert!(matches!(history[1], Item::ToolResult { ref call_id, .. } if call_id == "call-1"));
        assert!(matches!(
            history[2],
            Item::Message {
                role: Role::System,
                ..
            }
        ));

        let interrupt_note = history[2].as_text().unwrap().to_string();
        let entries = pod
            .store
            .read_all(
                pod.segment_state.session_id(),
                pod.segment_state.segment_id(),
            )
            .unwrap();
        let tool_result_count = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    LogEntry::ToolResult {
                        item: session_store::LoggedItem::ToolResult { call_id, .. },
                        ..
                    } if call_id == "call-1"
                )
            })
            .count();
        let interrupt_system_count = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    LogEntry::SystemItem {
                        item: SystemItem::Interrupt { body },
                        ..
                    } if body == &interrupt_note
                )
            })
            .count();

        assert_eq!(tool_result_count, 1);
        assert_eq!(interrupt_system_count, 1);
    }

    #[derive(Clone, Copy)]
    struct ResidentInjectionGates {
        summary: bool,
        knowledge: bool,
        workflows: bool,
    }

    impl ResidentInjectionGates {
        fn all(enabled: bool) -> Self {
            Self {
                summary: enabled,
                knowledge: enabled,
                workflows: enabled,
            }
        }
    }

    async fn render_system_prompt_with_summary(
        summary_doc: Option<&str>,
        memory_config: Option<manifest::MemoryConfig>,
        resident_injection: bool,
    ) -> String {
        render_system_prompt_with_resident_sections(
            summary_doc,
            memory_config,
            ResidentInjectionGates::all(resident_injection),
            false,
            false,
        )
        .await
    }

    async fn render_system_prompt_with_resident_sections(
        summary_doc: Option<&str>,
        memory_config: Option<manifest::MemoryConfig>,
        gates: ResidentInjectionGates,
        include_knowledge: bool,
        include_workflow: bool,
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let pwd = dir.path().join("workspace");
        std::fs::create_dir_all(&pwd).unwrap();
        if let Some(doc) = summary_doc {
            std::fs::create_dir_all(pwd.join(".insomnia/memory")).unwrap();
            std::fs::write(pwd.join(".insomnia/memory/summary.md"), doc).unwrap();
        }
        if include_knowledge {
            std::fs::create_dir_all(pwd.join(".insomnia/knowledge")).unwrap();
            std::fs::write(
                pwd.join(".insomnia/knowledge/resident-policy.md"),
                knowledge_doc("knowledge resident desc"),
            )
            .unwrap();
        }
        if include_workflow {
            std::fs::create_dir_all(pwd.join(".insomnia/workflow")).unwrap();
            std::fs::write(
                pwd.join(".insomnia/workflow/resident-flow.md"),
                workflow_doc("workflow resident desc"),
            )
            .unwrap();
        }

        let mut manifest = minimal_manifest_with_skills(vec![]);
        manifest.memory = memory_config;
        let scope = Scope::writable(&pwd).unwrap();
        let mut pod = Pod::new(manifest, Worker::new(NoopClient), store, pwd.clone(), scope)
            .await
            .unwrap();
        pod.memory_layout = pod
            .manifest
            .memory
            .as_ref()
            .map(|mem| memory::WorkspaceLayout::resolve(mem, &pwd));
        if let Some(layout) = pod.memory_layout.as_ref() {
            pod.workflow_registry = workflow_crate::load_workflows(layout).unwrap();
        }
        if gates.summary == gates.knowledge && gates.summary == gates.workflows {
            pod.set_resident_injection(gates.summary);
        } else {
            pod.set_resident_summary_injection(gates.summary);
            pod.set_resident_knowledge_injection(gates.knowledge);
            pod.set_resident_workflow_injection(gates.workflows);
        }
        let template = SystemPromptTemplate::parse(
            "$insomnia/default",
            crate::prompt::loader::PromptLoader::builtins_only(),
        )
        .unwrap();
        pod.set_system_prompt_template(template);
        pod.ensure_system_prompt_materialized().unwrap();
        pod.worker().get_system_prompt().unwrap().to_string()
    }

    fn summary_doc(body: &str) -> String {
        format!("---\nupdated_at: 2026-01-01T00:00:00Z\n---\n{body}")
    }

    fn knowledge_doc(description: &str) -> String {
        format!(
            "---\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\nkind: policy\ndescription: \"{description}\"\nmodel_invokation: true\nuser_invocable: true\nlast_sources: []\n---\nbody\n",
        )
    }

    fn workflow_doc(description: &str) -> String {
        format!("---\ndescription: {description}\nmodel_invokation: true\n---\nbody\n")
    }

    #[tokio::test]
    async fn resident_summary_body_is_injected_without_frontmatter() {
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("summary body for resident prompt\n")),
            Some(manifest::MemoryConfig::default()),
            true,
        )
        .await;

        assert!(rendered.contains("## Resident memory summary"));
        assert!(rendered.contains("summary body for resident prompt"));
        assert!(!rendered.contains("updated_at: 2026-01-01T00:00:00Z"));
        assert!(!rendered.contains("---\nupdated_at"));
    }

    #[tokio::test]
    async fn resident_summary_injection_can_be_disabled_by_manifest() {
        let memory = manifest::MemoryConfig {
            inject_summary: Some(false),
            ..manifest::MemoryConfig::default()
        };
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("disabled summary body\n")),
            Some(memory),
            true,
        )
        .await;

        assert!(!rendered.contains("Resident memory summary"));
        assert!(!rendered.contains("disabled summary body"));
    }

    #[tokio::test]
    async fn resident_summary_is_absent_without_memory_config() {
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("memory-disabled summary body\n")),
            None,
            true,
        )
        .await;

        assert!(!rendered.contains("Resident memory summary"));
        assert!(!rendered.contains("memory-disabled summary body"));
    }

    #[tokio::test]
    async fn malformed_resident_summary_does_not_fail_render() {
        let rendered = render_system_prompt_with_summary(
            Some("---\nthis is not yaml: : :\n---\nbad summary body\n"),
            Some(manifest::MemoryConfig::default()),
            true,
        )
        .await;

        assert!(rendered.contains("## Working boundaries"));
        assert!(!rendered.contains("Resident memory summary"));
        assert!(!rendered.contains("bad summary body"));
    }

    #[tokio::test]
    async fn resident_summary_gate_false_omits_only_summary() {
        let prompt = render_system_prompt_with_resident_sections(
            Some(&summary_doc("resident summary marker")),
            Some(manifest::MemoryConfig::default()),
            ResidentInjectionGates {
                summary: false,
                knowledge: true,
                workflows: true,
            },
            true,
            true,
        )
        .await;

        assert!(!prompt.contains("Resident memory summary"));
        assert!(!prompt.contains("resident summary marker"));
        assert!(prompt.contains("Resident knowledge"));
        assert!(prompt.contains("knowledge resident desc"));
        assert!(prompt.contains("Resident workflows"));
        assert!(prompt.contains("workflow resident desc"));
    }

    #[tokio::test]
    async fn knowledge_and_workflow_gates_false_keep_resident_summary() {
        let prompt = render_system_prompt_with_resident_sections(
            Some(&summary_doc("resident summary marker")),
            Some(manifest::MemoryConfig::default()),
            ResidentInjectionGates {
                summary: true,
                knowledge: false,
                workflows: false,
            },
            true,
            true,
        )
        .await;

        assert!(prompt.contains("Resident memory summary"));
        assert!(prompt.contains("resident summary marker"));
        assert!(!prompt.contains("Resident knowledge"));
        assert!(!prompt.contains("knowledge resident desc"));
        assert!(!prompt.contains("Resident workflows"));
        assert!(!prompt.contains("workflow resident desc"));
    }

    #[tokio::test]
    async fn resident_injection_opt_out_omits_all_resident_sections() {
        let prompt = render_system_prompt_with_resident_sections(
            Some(&summary_doc("resident summary marker")),
            Some(manifest::MemoryConfig::default()),
            ResidentInjectionGates::all(false),
            true,
            true,
        )
        .await;

        assert!(!prompt.contains("Resident memory summary"));
        assert!(!prompt.contains("resident summary marker"));
        assert!(!prompt.contains("Resident knowledge"));
        assert!(!prompt.contains("knowledge resident desc"));
        assert!(!prompt.contains("Resident workflows"));
        assert!(!prompt.contains("workflow resident desc"));
    }

    fn minimal_manifest_with_skills(dirs: Vec<PathBuf>) -> PodManifest {
        // Construct the smallest possible PodManifest that resolves; only
        // the `skills` field matters for `skill_dir_read_rules`.
        let toml_str = r#"
[pod]
name = "x"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[worker]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#;
        let mut manifest = PodManifest::from_toml(toml_str).unwrap();
        if !dirs.is_empty() {
            manifest.skills = Some(manifest::SkillsConfig { directories: dirs });
        }
        manifest
    }

    #[test]
    fn skill_dir_read_rules_lists_workspace_skill_directories() {
        let manifest = minimal_manifest_with_skills(vec![
            PathBuf::from("/abs/skills-a"),
            PathBuf::from("/abs/skills-b"),
        ]);
        let rules = skill_dir_read_rules(&manifest);
        let workspace_rules: Vec<_> = rules
            .iter()
            .filter(|r| {
                r.target == PathBuf::from("/abs/skills-a")
                    || r.target == PathBuf::from("/abs/skills-b")
            })
            .collect();
        assert_eq!(workspace_rules.len(), 2);
        for rule in &workspace_rules {
            assert_eq!(rule.permission, Permission::Read);
            assert!(rule.recursive);
        }
    }

    #[test]
    fn skill_dir_read_rules_empty_when_skills_section_missing() {
        let manifest = minimal_manifest_with_skills(vec![]);
        let rules = skill_dir_read_rules(&manifest);
        assert!(rules.is_empty());
    }

    #[test]
    fn ingest_skills_returns_empty_when_skills_section_missing() {
        let manifest = minimal_manifest_with_skills(vec![]);
        let mut registry = workflow_crate::WorkflowRegistry::empty();
        let shadows = ingest_skills(&mut registry, &manifest);
        assert!(shadows.is_empty());
        assert!(registry.is_empty());
    }

    #[test]
    fn ingest_skills_loads_from_workspace_directories() {
        let dir = tempfile::tempdir().unwrap();
        let skills_root = dir.path().join("skills");
        std::fs::create_dir_all(skills_root.join("alpha")).unwrap();
        std::fs::write(
            skills_root.join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: Alpha skill\n---\nbody\n",
        )
        .unwrap();

        let manifest = minimal_manifest_with_skills(vec![skills_root.clone()]);
        let mut registry = workflow_crate::WorkflowRegistry::empty();
        let shadows = ingest_skills(&mut registry, &manifest);

        // workspace skill `alpha` should be registered (no collision).
        assert!(
            registry
                .get(&workflow_crate::Slug::parse("alpha").unwrap())
                .is_some()
        );
        // No workflow exists to shadow `alpha`, so no shadow event for it.
        assert!(shadows.iter().all(|s| s.slug.as_str() != "alpha"));
    }
}
