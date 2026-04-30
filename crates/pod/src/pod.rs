use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use llm_worker::Item;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::state::Mutable;
use llm_worker::{ToolOutputLimits, UsageRecord, Worker, WorkerError, WorkerResult};
use session_store::{EntryHash, SessionId, SessionStartState, Store, StoreError};
use tracing::{info, warn};

use manifest::{PodManifest, PodManifestConfig, ResolveError, Scope, ScopeError, WorkerManifest};

use crate::compact::state::CompactState;
use crate::compact::usage_tracker::UsageTracker;
use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreRequestInfo, PreToolCall,
};
use crate::ipc::alerter::Alerter;
use crate::ipc::interceptor::PodInterceptor;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::prompt::agents_md::read_agents_md;
use crate::prompt::catalog::{CatalogError, PromptCatalog};
use crate::prompt::loader::PromptLoader;
use crate::prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
use crate::runtime::dir;
use crate::runtime::pod_registry::{self, ScopeAllocationGuard, ScopeLockError};
use async_trait::async_trait;
use llm_worker::interceptor::PreRequestAction;
use protocol::{AlertLevel, AlertSource, Event, Segment};
use tokio::sync::broadcast;

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
    session_id: SessionId,
    head_hash: Option<EntryHash>,
    /// Absolute working directory of the Pod.
    pwd: PathBuf,
    /// Resolved scope — always present.
    scope: Scope,
    hook_builder: HookRegistryBuilder,
    interceptor_installed: bool,
    /// Shared compaction state (present when compact_threshold is configured).
    compact_state: Option<Arc<CompactState>>,
    /// Per-LLM-request Usage tracker. Always present after construction.
    /// Captures `(history_len, UsageEvent)` pairs during a run; drained
    /// in `persist_turn` and persisted as `LogEntry::LlmUsage` entries.
    usage_tracker: Arc<UsageTracker>,
    /// Cumulative Usage measurement timeline, one entry per LLM call.
    /// Restored from session log on `restore`, appended on each persist.
    /// Read by token-accounting APIs (`Pod::total_tokens`, etc.).
    ///
    /// Wrapped in `Arc<Mutex>` so that callbacks injected into the
    /// Worker (e.g. the savings estimator used by the prune projection)
    /// can share the same view via [`Pod::usage_history_handle`].
    usage_history: Arc<Mutex<Vec<UsageRecord>>>,
    /// Session-lifetime file-operation tracker from the builtin `tools`
    /// crate. Populated by the Controller when it registers the builtin
    /// tools so that Pod-owned operations (e.g. compaction) can consult
    /// the recency of touched files.
    tracker: Option<tools::Tracker>,
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
    /// Queue of pending `Method::Notify` notifications awaiting
    /// injection into the next LLM request. Shared with the
    /// PodInterceptor installed in `ensure_interceptor_installed`.
    pending_notifies: NotifyBuffer,
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
    /// When true (default), the system-prompt assembler walks
    /// `<workspace>/knowledge/*` and appends a `## Resident knowledge`
    /// section listing records with `model_invokation: true`.
    /// Phase 2 (consolidation) workers set this to false so the
    /// agentic worker pulls knowledge through the search tools instead.
    inject_resident_knowledge: bool,
    /// Phase 1 (memory.extract) reentry guard. `true` while an extract
    /// worker is running; subsequent triggers are skipped per spec
    /// (`docs/plan/memory.md` §Phase 1 並走防止). `Arc<AtomicBool>` so
    /// the flag survives across `try_post_run_extract` calls without a
    /// `&mut self` race.
    extract_in_flight: Arc<AtomicBool>,
    /// Last completed Phase 1 boundary. `None` means no extract has
    /// run yet on this session — next extract starts from entry 0.
    /// Restored from `RestoredState.extensions` on `restore`, updated
    /// after each successful extract via `save_extension`.
    extract_pointer: Mutex<Option<memory::ExtractPointerPayload>>,
    /// Typed user submissions in submit order. K-th entry corresponds to
    /// the K-th `Item::user_message` in `worker.history()` (modulo seed
    /// history loaded via `SessionStart.history`, whose original segments
    /// are not preserved). Populated from log on `restore_from_manifest`,
    /// appended after `save_user_input` on each `run`. Mirrored to
    /// `PodSharedState` by the controller for `Event::History` use.
    user_segments: Vec<Vec<Segment>>,
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
        // Session creation is deferred to `ensure_session_head` at first
        // run so a later-installed system-prompt template (see
        // `set_system_prompt_template`) can be captured by `SessionStart`.
        let session_id = session_store::new_session_id();
        let prompts = PromptCatalog::builtins_only()?;
        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: None,
            pwd,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::<UsageRecord>::new())),
            tracker: None,
            system_prompt_template: None,
            alerter: None,
            event_tx: None,
            pending_notifies: NotifyBuffer::new(),
            scope_allocation: None,
            callback_socket: None,
            prompts,
            inject_resident_knowledge: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Mutex::new(None),
            user_segments: Vec::new(),
        };
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

    /// Toggle the resident-knowledge section of the system prompt.
    ///
    /// Default `true`: when memory is enabled in the manifest, the
    /// assembler walks `<workspace>/knowledge/*` and lists records with
    /// `model_invokation: true`. Phase 2 (consolidation) workers and
    /// other agentic memory paths set this to `false` so the worker
    /// pulls knowledge through the search tools instead of riding on
    /// the resident system-prompt budget. Idempotent if called multiple
    /// times before the first turn; ineffective once the system prompt
    /// has been materialised.
    pub fn set_resident_knowledge_injection(&mut self, enabled: bool) {
        self.inject_resident_knowledge = enabled;
    }

    /// Shared handle to the prompt catalog. Cheap to clone (`Arc`).
    pub fn prompts(&self) -> &Arc<PromptCatalog> {
        &self.prompts
    }

    /// The session ID used for persistence.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The Pod's manifest.
    pub fn manifest(&self) -> &PodManifest {
        &self.manifest
    }

    /// The Pod's working directory.
    pub fn pwd(&self) -> &Path {
        &self.pwd
    }

    /// The Pod's directory scope.
    pub fn scope(&self) -> &Scope {
        &self.scope
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

    /// Snapshot of the Phase 1 (memory.extract) boundary pointer.
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
    /// history loaded via `SessionStart.history` does not contribute,
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

    /// Attach the session-scoped file-operation tracker from the builtin
    /// `tools` crate. Called by the Controller immediately after it
    /// registers the builtin tools on the Worker. Overwrites any
    /// previously attached tracker.
    pub fn attach_tracker(&mut self, tracker: tools::Tracker) {
        self.tracker = Some(tracker);
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

    fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        if let Some(n) = self.alerter.as_ref() {
            n.alert(level, source, message);
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

    /// Push a `Method::Notify` entry onto the pending buffer.
    ///
    /// The notification will be injected as an `Item::system_message`
    /// into the next outgoing LLM request context (not into history).
    /// See [`NotifyBuffer`] for overflow behaviour.
    pub fn push_notify(&self, message: String) {
        self.pending_notifies.push(message);
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
    /// When either compaction threshold (`compact_threshold` or
    /// `compact_request_threshold`) is configured in the manifest, allocates
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
                .map(|c| {
                    (
                        c.compact_threshold,
                        c.compact_request_threshold,
                        c.compact_retained_tokens,
                    )
                })
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
                            "compact_threshold > compact_request_threshold; \
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
                self.prompts.clone(),
            );
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
        let worker = self.worker.as_mut().expect("worker present");
        // Materialise any pending tool factories so the template sees the
        // full list of tool names. Redundant with the flush inside
        // `Worker::lock()`; safe because `flush_pending` is idempotent.
        worker.tool_server_handle().flush_pending();
        let tool_names: Vec<String> = worker
            .tool_server_handle()
            .tool_definitions_sorted()
            .into_iter()
            .map(|d| d.name)
            .collect();
        let agents_md_read = read_agents_md(&self.pwd);
        for warning in agents_md_read.warnings {
            if let Some(n) = alerter.as_ref() {
                n.alert(AlertLevel::Warn, AlertSource::AgentsMd, warning);
            }
        }
        // Resident-injection collection: only when memory is enabled in
        // the manifest AND this Pod opts in (Phase 2 workers opt out).
        // Owned `Vec` lives for the duration of `render` below; the
        // context borrows a slice into it.
        let resident: Vec<memory::ResidentKnowledgeEntry> = if self.inject_resident_knowledge {
            self.manifest
                .memory
                .as_ref()
                .map(|mem| {
                    let layout = memory::WorkspaceLayout::resolve(mem, &self.pwd);
                    memory::collect_resident_knowledge(&layout)
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let resident_slice: Option<&[memory::ResidentKnowledgeEntry]> =
            if self.inject_resident_knowledge && self.manifest.memory.is_some() {
                Some(&resident)
            } else {
                None
            };
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            cwd: &self.pwd,
            scope: &self.scope,
            tool_names,
            agents_md: agents_md_read.body,
            resident_knowledge: resident_slice,
            prompts: &self.prompts,
        };
        let rendered = template
            .render(&ctx)
            .map_err(|source| PodError::SystemPromptRender { source })?;
        worker.set_system_prompt(rendered);
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

    /// Send user input and run until the LLM turn completes.
    ///
    /// `input` is a typed segment list (see [`protocol::Segment`]). The
    /// Pod flattens it into a single user-message string for the
    /// underlying Worker, expanding paste content inline and surfacing
    /// alerts for any segment kind the current Pod has no resolver for
    /// (file refs, knowledge refs, workflow invocations, unknown
    /// variants from a newer client).
    ///
    /// If the between-turns compaction threshold is exceeded mid-run,
    /// the Worker is aborted, history is compacted, and execution resumes
    /// automatically.
    pub async fn run(&mut self, input: Vec<Segment>) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized()?;
        self.ensure_session_head().await?;

        // Persist the user input as typed segments before the worker
        // pushes its flattened copy into history. save_delta deliberately
        // skips the resulting `is_user_message()` item to avoid double-write.
        session_store::save_user_input(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            input.clone(),
        )
        .await?;
        self.user_segments.push(input.clone());

        let flattened = self.flatten_segments(&input);

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → run → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.run(flattened).await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Flatten a typed segment list into the single string the Worker
    /// receives as the user message, and emit user-facing alerts for
    /// segments that fall through to placeholder (file/knowledge/workflow
    /// refs without a resolver, or unknown variants from a newer client).
    /// The text reconstruction itself comes from `Segment::flatten_to_text`,
    /// shared with replay paths that should not re-alert.
    fn flatten_segments(&self, segments: &[Segment]) -> String {
        for seg in segments {
            match seg {
                Segment::Text { .. } | Segment::Paste { .. } => {}
                Segment::FileRef { path } => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!(
                            "file ref @{path} cannot be resolved \
                             (resolver not yet implemented); passed to LLM as placeholder"
                        ),
                    );
                }
                Segment::KnowledgeRef { slug } => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!(
                            "knowledge ref #{slug} cannot be resolved \
                             (resolver not yet implemented); passed to LLM as placeholder"
                        ),
                    );
                }
                Segment::WorkflowInvoke { slug } => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!(
                            "workflow /{slug} cannot be resolved \
                             (resolver not yet implemented); passed to LLM as placeholder"
                        ),
                    );
                }
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
    pub async fn run_for_notification(&mut self) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized()?;
        self.ensure_session_head().await?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized()?;
        self.ensure_session_head().await?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → resume → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Ensure the session exists and its head still matches ours.
    ///
    /// On the first call for a Pod built via `from_manifest`, the session
    /// has not been written to the store yet — this is when we append the
    /// initial `SessionStart` entry, carrying the system prompt that
    /// `ensure_system_prompt_materialized` has just rendered. Subsequent
    /// calls fall through to `ensure_head_or_fork`, which auto-forks when
    /// another writer has advanced the store head behind our back.
    async fn ensure_session_head(&mut self) -> Result<(), PodError> {
        let w = self.worker.as_ref().unwrap();
        let state = SessionStartState {
            system_prompt: w.get_system_prompt(),
            config: w.request_config(),
            history: w.history(),
        };
        if self.head_hash.is_none() {
            let hash =
                session_store::create_session_with_id(&self.store, self.session_id, state).await?;
            self.head_hash = Some(hash);
            return Ok(());
        }
        let prev_session_id = self.session_id;
        session_store::ensure_head_or_fork(
            &self.store,
            &mut self.session_id,
            &mut self.head_hash,
            state,
        )
        .await?;
        // ensure_head_or_fork mints a fresh session_id when it auto-
        // forks. Sync that to pods.json so a concurrent
        // restore_from_manifest can't see "no live writer" for the new
        // session and grab it.
        if self.session_id != prev_session_id && self.scope_allocation.is_some() {
            pod_registry::update_session(&self.manifest.pod.name, self.session_id)?;
        }
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
                Ok(new_session_id) => {
                    info!(
                        new_session_id = %new_session_id,
                        "Compaction succeeded, resuming execution"
                    );
                    self.send_event(Event::CompactDone { new_session_id });
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

    /// Attempt proactive compaction (called by Controller after run).
    ///
    /// Best-effort: failures are logged but do not propagate.
    pub async fn try_post_run_compact(&mut self) -> Result<(), PodError> {
        let state = match self.compact_state.as_ref() {
            Some(s) if !s.is_disabled() && !s.just_compacted() => s.clone(),
            _ => return Ok(()),
        };
        let current_tokens = self.total_tokens().tokens;
        if !state.exceeds_post_run(current_tokens) {
            return Ok(());
        }

        let retained = state.retained_tokens();
        self.send_event(Event::CompactStart);
        match self.compact(retained).await {
            Ok(new_session_id) => {
                info!(
                    new_session_id = %new_session_id,
                    "Proactive post-run compaction succeeded"
                );
                self.send_event(Event::CompactDone { new_session_id });
                state.record_compact_success();
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "Proactive post-run compaction failed");
                self.send_event(Event::CompactFailed {
                    error: e.to_string(),
                });
                self.alert(
                    AlertLevel::Warn,
                    AlertSource::Compactor,
                    format!("post-run compaction failed: {e}"),
                );
                state.record_compact_failure();
                Ok(())
            }
        }
    }

    /// Persist delta + turn end + outcome after a run/resume.
    async fn persist_turn(
        &mut self,
        history_before: usize,
        result: &Result<WorkerResult, WorkerError>,
    ) -> Result<(), StoreError> {
        // Use direct field access for split borrows (worker immutable,
        // head_hash mutable).
        let w = self.worker.as_ref().unwrap();
        let new_items = &w.history()[history_before..];
        session_store::save_delta(&self.store, self.session_id, &mut self.head_hash, new_items)
            .await?;

        let turn_count = self.worker.as_ref().unwrap().turn_count();
        session_store::save_turn_end(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            turn_count,
        )
        .await?;

        // Persist any LLM Usage measurements collected during this run.
        // One LogEntry::LlmUsage per LLM call (the tool loop may have run
        // many calls within a single Pod::run). Each is also appended to
        // the in-memory `usage_history` so token-accounting APIs see it
        // before the next run.
        let usage_records = self.usage_tracker.drain();
        for record in usage_records {
            session_store::save_usage(
                &self.store,
                self.session_id,
                &mut self.head_hash,
                record.history_len,
                record.input_total_tokens,
                record.cache_read_tokens,
                record.cache_write_tokens,
                record.output_tokens,
            )
            .await?;
            self.usage_history
                .lock()
                .expect("usage_history poisoned")
                .push(record);
        }

        let interrupted = self.worker.as_ref().unwrap().last_run_interrupted();
        match result {
            Ok(r) => {
                session_store::save_run_completed(
                    &self.store,
                    self.session_id,
                    &mut self.head_hash,
                    r.clone(),
                    interrupted,
                )
                .await?;
            }
            Err(e) => {
                session_store::save_run_errored(
                    &self.store,
                    self.session_id,
                    &mut self.head_hash,
                    e.to_string(),
                    interrupted,
                )
                .await?;
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
    pub async fn compact(&mut self, retained_tokens: u64) -> Result<SessionId, PodError> {
        use std::sync::atomic::{AtomicU64, Ordering};

        use crate::compact::worker::{
            CompactWorkerContext, CompactWorkerInterceptor, add_reference_tool,
            mark_read_required_tool, write_summary_tool,
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
        let (auto_read_budget, compact_worker_max_input_tokens) = self
            .manifest
            .compaction
            .as_ref()
            .map(|c| {
                (
                    c.compact_auto_read_budget,
                    c.compact_worker_max_input_tokens,
                )
            })
            .unwrap_or((
                manifest::defaults::COMPACT_AUTO_READ_BUDGET,
                manifest::defaults::COMPACT_WORKER_MAX_INPUT_TOKENS,
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
        // references and the (pruned) conversation text.
        let summary_input = build_summary_input(&items_to_summarise, &default_refs);

        // Worker-side state collected by the compact worker's tool calls.
        let ctx = Arc::new(std::sync::Mutex::new(CompactWorkerContext::with_budget(
            auto_read_budget,
        )));

        // Build an independent compact worker. Scope and pwd are shared
        // with the main Pod (reads go through the same policy) but the
        // Tracker is fresh — compact-time reads must not pollute the
        // main session's recency list, which feeds `default_refs` above.
        let scoped_fs = tools::ScopedFs::new(self.scope.clone(), self.pwd.clone());
        let summary_tracker = tools::Tracker::new();
        let summary_client: Box<dyn LlmClient> = self.build_compactor_client()?;
        let summary_system_prompt = self
            .prompts
            .compact_system()
            .map_err(PodError::PromptCatalog)?;
        let mut summary_worker = Worker::new(summary_client).system_prompt(summary_system_prompt);

        // Cumulative input-token meter + interceptor. The meter is bumped
        // from the on_usage callback and read on every pre_llm_request.
        let input_so_far = Arc::new(AtomicU64::new(0));
        {
            let acc = input_so_far.clone();
            summary_worker.on_usage(move |event| {
                if let Some(tokens) = event.input_tokens {
                    acc.fetch_add(tokens, Ordering::Relaxed);
                }
            });
        }
        summary_worker.set_interceptor(CompactWorkerInterceptor {
            input_so_far: input_so_far.clone(),
            max_input_tokens: compact_worker_max_input_tokens,
        });

        // Tools: read_file (shared scope, fresh tracker) + the three
        // compact-specific tools that populate `ctx`.
        summary_worker.register_tool(tools::read_tool(scoped_fs.clone(), summary_tracker));
        summary_worker.register_tool(mark_read_required_tool(scoped_fs.clone(), ctx.clone()));
        summary_worker.register_tool(add_reference_tool(ctx.clone()));
        summary_worker.register_tool(write_summary_tool(ctx.clone()));

        let out = summary_worker
            .run(summary_input)
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

        let final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
        let summary_text = final_ctx
            .summary
            .clone()
            .ok_or(PodError::CompactSummaryMissing)?;

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

        // Build new history: [summary, ...auto-read, references, ...retained].
        let mut new_history = Vec::with_capacity(
            1 + auto_read_messages.len()
                + reference_message.is_some() as usize
                + retained_items.len(),
        );
        new_history.push(Item::system_message(format!(
            "[Compacted context summary]\n\n{summary_text}"
        )));
        new_history.extend(auto_read_messages);
        if let Some(msg) = reference_message {
            new_history.push(msg);
        }
        new_history.extend(retained_items);

        // Persist as a new compacted session.
        let old_session_id = self.session_id;
        let old_head_hash = self
            .head_hash
            .clone()
            .expect("head_hash should be set after at least one entry");

        let w = self.worker.as_ref().unwrap();
        let state = SessionStartState {
            system_prompt: w.get_system_prompt(),
            config: w.request_config(),
            history: &new_history,
        };
        let (new_session_id, new_head_hash) = session_store::create_compacted_session(
            &self.store,
            state,
            old_session_id,
            old_head_hash,
        )
        .await?;

        // Swap in the new session state. usage_history belongs to the old
        // session — the new compacted session starts with no measurements
        // until its first LLM call.
        self.session_id = new_session_id;
        self.head_hash = Some(new_head_hash);
        // Keep pods.json pointing at the live session_id. Without this
        // a concurrent `restore_from_manifest(new_session_id)` would
        // see no live writer and grab the session this Pod just moved
        // into, causing two writers to race on the same jsonl. Skipped
        // when no allocation is installed (e.g. compact under
        // `Pod::new` in tests).
        if self.scope_allocation.is_some() {
            pod_registry::update_session(&self.manifest.pod.name, new_session_id)?;
        }
        // Align user_segments with the post-compaction history. Items
        // before `retain_from` (now folded into the summary) lose their
        // segments; only the user_messages surviving in retained_items
        // keep them. They are always the trailing K entries of
        // `self.user_segments` because submissions are appended in order.
        let drop_n = self.user_segments.len().saturating_sub(retained_user_msgs);
        if drop_n > 0 {
            self.user_segments.drain(..drop_n);
        }

        let worker = self.worker.as_mut().unwrap();
        worker.set_history(new_history);
        // Anchor the prompt cache at the summary item so that Anthropic
        // can place a durable `cache_control` breakpoint there — our
        // compact layout guarantees history[0] is the summary.
        worker.set_cache_anchor(Some(0));
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clear();
        // Reset Phase 1 pointer alongside usage_history: the compacted
        // session has a fresh log with no `LogEntry::Extension` entries
        // yet, so a cold restore here would set extract_pointer to None
        // via fold_pointer. The in-memory pointer must match — otherwise
        // `tokens_added_since(old_history_len)` would treat the new
        // (shorter) history as if it had already been processed, and
        // Phase 1 would stop firing for the rest of the process's
        // lifetime.
        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = None;

        Ok(new_session_id)
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

    /// Build the LlmClient for the Phase 1 (memory.extract) Worker.
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

    /// pointer 以降に増えたプロンプト全長の推定。Phase 1 trigger が
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

    /// Phase 1 (memory.extract) post-run trigger.
    ///
    /// Called by the Controller **before** [`try_post_run_compact`] so
    /// the extract worker sees a stable session-log entry range
    /// (compact rewrites history). Best-effort: failures are logged but
    /// not propagated.
    ///
    /// Behaviour follows `docs/plan/memory.md` §Phase 1 並走防止:
    /// in-flight 中の trigger は skip し、完了時点で閾値再評価する
    /// (the loop below). Pending state is not retained — the
    /// re-evaluation happens naturally because the in-memory pointer
    /// has advanced.
    pub async fn try_post_run_extract(&mut self) -> Result<(), PodError> {
        let Some(memory_cfg) = self.manifest.memory.clone() else {
            return Ok(());
        };
        let Some(threshold) = memory_cfg.extract_threshold else {
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
                    tracing::warn!(error = %e, "Phase 1 extract failed");
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Pod,
                        format!("memory Phase 1 extract failed: {e}"),
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
            return Ok(ExtractDecision::Skipped);
        }

        let current_history_len = self
            .worker
            .as_ref()
            .expect("worker present")
            .history()
            .len();
        if current_history_len <= processed_history_len {
            return Ok(ExtractDecision::Skipped);
        }

        // Read the session log to get the current entry count. This is
        // the boundary for the source.range end_entry. Called once per
        // extract, on a small local file.
        let entries_now = self.store.read_all(self.session_id).await?.len();
        if entries_now == 0 {
            return Ok(ExtractDecision::Skipped);
        }
        let end_entry = entries_now - 1;
        let start_entry = pointer_snapshot
            .as_ref()
            .map(|p| p.processed_through_entry + 1)
            .unwrap_or(0);
        if start_entry > end_entry {
            return Ok(ExtractDecision::Skipped);
        }

        let items_to_extract = self.worker.as_ref().expect("worker present").history()
            [processed_history_len..current_history_len]
            .to_vec();

        let layout = memory::WorkspaceLayout::resolve(memory_cfg, &self.pwd);
        let cap = memory_cfg
            .extract_worker_max_input_tokens
            .unwrap_or(manifest::defaults::MEMORY_EXTRACT_WORKER_MAX_INPUT_TOKENS);

        let client = self.build_extractor_client(memory_cfg)?;
        let mut extract_worker = Worker::new(client).system_prompt(extract::EXTRACT_SYSTEM_PROMPT);

        // Cumulative input-token meter + interceptor (mirror of
        // CompactWorkerInterceptor). Aborts the extract worker if its
        // own input usage crosses the cap.
        let input_so_far = Arc::new(std::sync::atomic::AtomicU64::new(0));
        {
            let acc = input_so_far.clone();
            extract_worker.on_usage(move |event| {
                if let Some(tokens) = event.input_tokens {
                    acc.fetch_add(tokens, Ordering::Relaxed);
                }
            });
        }
        extract_worker.set_interceptor(MemoryExtractWorkerInterceptor {
            input_so_far: input_so_far.clone(),
            max_input_tokens: cap,
        });

        let ctx = Arc::new(extract::ExtractWorkerContext::new());
        extract_worker.register_tool(extract::write_extracted_tool(ctx.clone()));

        let input_text = extract::build_extract_input(&items_to_extract);
        extract_worker
            .run(input_text)
            .await
            .map_err(PodError::Worker)?;

        let payload = ctx.take_payload().unwrap_or_else(|| {
            tracing::warn!(
                "Phase 1 extract worker did not call write_extracted; \
                 advancing pointer with empty payload"
            );
            extract::ExtractedPayload::default()
        });

        let staging_id = if payload.is_empty() {
            String::new()
        } else {
            let source = memory::schema::SourceRef {
                session_id: self.session_id.to_string(),
                range: [start_entry as u64, end_entry as u64],
            };
            let (id, _) = extract::write_staging(&layout, source, payload)
                .map_err(PodError::ExtractStaging)?;
            id.to_string()
        };

        let pointer_payload = extract::ExtractPointerPayload {
            processed_through_entry: end_entry,
            processed_through_history_len: current_history_len,
            staging_id,
        };
        let payload_value = serde_json::to_value(&pointer_payload)
            .expect("ExtractPointerPayload is always JSON-serializable");
        session_store::save_extension(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            extract::EXTRACT_DOMAIN,
            payload_value,
        )
        .await?;

        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = Some(pointer_payload);

        Ok(ExtractDecision::Completed)
    }
}

/// Outcome of a single Phase 1 extract iteration. Internal to
/// `try_post_run_extract` / `run_extract_once`.
enum ExtractDecision {
    /// Threshold not reached, or no items to extract.
    Skipped,
    /// Extract ran and pointer advanced. Caller re-evaluates threshold.
    Completed,
}

/// Pre-request interceptor for the Phase 1 extract worker. Aborts when
/// cumulative input tokens cross `max_input_tokens`. Mirror of
/// `compact::worker::CompactWorkerInterceptor`; kept separate so each
/// subsystem can tune its own message and budget.
struct MemoryExtractWorkerInterceptor {
    input_so_far: Arc<std::sync::atomic::AtomicU64>,
    max_input_tokens: u64,
}

#[async_trait]
impl llm_worker::interceptor::Interceptor for MemoryExtractWorkerInterceptor {
    async fn pre_llm_request(
        &self,
        _context: &mut Vec<Item>,
    ) -> llm_worker::interceptor::PreRequestAction {
        if self.input_so_far.load(Ordering::Relaxed) > self.max_input_tokens {
            return llm_worker::interceptor::PreRequestAction::Cancel(format!(
                "Phase 1 extract worker input exceeded {} tokens",
                self.max_input_tokens
            ));
        }
        llm_worker::interceptor::PreRequestAction::Continue
    }
}

impl<St: Store> Pod<Box<dyn LlmClient>, St> {
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
        let common = prepare_pod_common(&manifest, &loader, /* parse_template */ true)?;

        // Session creation is deferred to the first run (see
        // `ensure_session_head`) so the SessionStart entry can capture
        // the rendered system prompt, not the raw template source. The
        // session_id is allocated here so the pod-registry registration
        // can record it from the start.
        let session_id = session_store::new_session_id();

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
            session_id,
        )?;

        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: None,
            pwd: common.pwd,
            scope: common.scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            system_prompt_template: common.system_prompt_template,
            alerter: None,
            event_tx: None,
            pending_notifies: NotifyBuffer::new(),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            prompts: common.prompts,
            inject_resident_knowledge: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Mutex::new(None),
            user_segments: Vec::new(),
        };
        pod.apply_prune_from_manifest();
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
        let common = prepare_pod_common(&manifest, &loader, /* parse_template */ true)?;

        let session_id = session_store::new_session_id();
        let scope_allocation = pod_registry::adopt_allocation(
            manifest.pod.name.clone(),
            std::process::id(),
            session_id,
        )?;

        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: None,
            pwd: common.pwd,
            scope: common.scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            system_prompt_template: common.system_prompt_template,
            alerter: None,
            event_tx: None,
            pending_notifies: NotifyBuffer::new(),
            scope_allocation: Some(scope_allocation),
            callback_socket: Some(callback_socket),
            prompts: common.prompts,
            inject_resident_knowledge: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Mutex::new(None),
            user_segments: Vec::new(),
        };
        pod.apply_prune_from_manifest();
        Ok(pod)
    }

    /// Restore a Pod from an existing session log.
    ///
    /// Resolves the manifest cascade exactly like [`Self::from_manifest`]
    /// (pwd / scope / pod-registry / client / prompt catalog), seeds a
    /// fresh Worker from the source session's `RestoredState`, and
    /// reuses the same `session_id` so subsequent turns append to the
    /// source jsonl as a continuation of the same conversation.
    ///
    /// Concurrent writers are prevented by the pod-registry:
    /// the registration carries `session_id`, and this constructor
    /// refuses to start when `pod_registry::lookup_session` already finds
    /// a live Pod writing to `session_id`. So there is no need to fork —
    /// resume is "the same session, a different process owning it".
    ///
    /// `system_prompt` is replayed verbatim from the session log —
    /// templates are not re-rendered on restore so a long-running
    /// session keeps a stable cache prefix even when the manifest's
    /// instruction template would render differently today.
    pub async fn restore_from_manifest(
        session_id: SessionId,
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
    ) -> Result<Self, PodError> {
        let state = session_store::restore(&store, session_id).await?;
        if state.head_hash.is_none() {
            return Err(PodError::SessionEmpty { session_id });
        }

        let common = prepare_pod_common(&manifest, &loader, /* parse_template */ false)?;

        // Atomic: register_pod inside install_top_level rejects when
        // another live allocation already holds `session_id`. Wrapping
        // the lookup + install inside a single `LockFileGuard` is what
        // makes "no two live Pods write to the same session log"
        // actually structural rather than a hopeful pre-check.
        let socket_path = dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.pod.name)
            .join("sock");
        let scope_allocation = pod_registry::install_top_level(
            manifest.pod.name.clone(),
            std::process::id(),
            socket_path,
            common.scope.allow_rules(),
            session_id,
        )?;

        // Build the worker and apply the manifest defaults first, then
        // overwrite the pieces the session log is authoritative for.
        let mut worker = Worker::new(common.client);
        apply_worker_manifest(&mut worker, &manifest.worker);
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

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: state.head_hash,
            pwd: common.pwd,
            scope: common.scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            usage_history: Arc::new(Mutex::new(state.usage_history)),
            tracker: None,
            // Restore replays the saved system_prompt verbatim — no
            // template re-render on resume.
            system_prompt_template: None,
            alerter: None,
            event_tx: None,
            pending_notifies: NotifyBuffer::new(),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            prompts: common.prompts,
            inject_resident_knowledge: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Mutex::new(extract_pointer),
            user_segments: state.user_segments,
        };
        pod.apply_prune_from_manifest();
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

/// Build the compact worker's input: default-reference instructions,
/// the list of recently-touched files, and the pruned conversation
/// produced by [`build_summary_prompt`].
fn build_summary_input(items: &[Item], default_refs: &[PathBuf]) -> String {
    let mut out = String::new();
    out.push_str(
        "Summarise the conversation below into a structured summary and nominate \
         files the next session needs.\n\n",
    );
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
    out.push_str("## Conversation\n");
    out.push_str(&build_summary_prompt(items));
    out.push_str("\n\nWhen you are done, call `write_summary` with the final 5-section text.");
    out
}

/// Format conversation items into a text prompt for the summary Worker.
///
/// The summary should capture decisions and user intent, not recreate code.
/// File contents and tool IO belong in auto-read / references, not in the
/// summary input. So this strips:
/// - `ToolCall.arguments` (keep only the tool name)
/// - `ToolResult.content` (keep only the summary line)
/// - `Reasoning` entirely (intermediate thought, superseded by decisions)
fn build_summary_prompt(items: &[Item]) -> String {
    let mut lines = Vec::new();
    for item in items {
        match item {
            Item::Message { role, content, .. } => {
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
                lines.push(format!("[{role_label}] {text}"));
            }
            Item::ToolCall { name, .. } => {
                lines.push(format!("[ToolCall] {name}"));
            }
            Item::ToolResult { summary, .. } => {
                lines.push(format!("[ToolResult] {summary}"));
            }
            Item::Reasoning { .. } => {}
        }
    }
    lines.join("\n\n")
}

/// Pod errors.
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

    #[error("memory Phase 1 staging write failed: {0}")]
    ExtractStaging(#[source] memory::extract::StagingError),

    #[error("session {session_id} has no entries to restore")]
    SessionEmpty { session_id: SessionId },
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
    system_prompt_template: Option<SystemPromptTemplate>,
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
    if !scope.is_readable(&pwd) {
        return Err(PodError::PwdOutsideScope { pwd });
    }

    let client = provider::build_client(&manifest.model)?;
    let prompts = PromptCatalog::load(loader, manifest.pod.prompt_pack.as_deref())?;

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
        system_prompt_template,
    })
}

/// Build the Pod's runtime [`Scope`] from the manifest, layering the
/// memory subsystem's deny-write rules on top when `[memory]` is
/// present. The deny rules cap generic CRUD tools so they cannot
/// touch `<workspace>/memory/` or `<workspace>/knowledge/` while the
/// memory tools (registered separately) bypass `ScopedFs` and write
/// through `std::fs` directly.
fn build_scope_with_memory(manifest: &PodManifest, pwd: &Path) -> Result<Scope, PodError> {
    let mut scope_config = manifest.scope.clone();
    if let Some(mem) = manifest.memory.as_ref() {
        let layout = memory::WorkspaceLayout::resolve(mem, pwd);
        scope_config.deny.extend(memory::deny_write_rules(&layout));
    }
    Scope::from_config(&scope_config).map_err(PodError::Scope)
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
mod build_summary_prompt_tests {
    use super::*;

    #[test]
    fn strips_tool_call_arguments() {
        let items = vec![Item::tool_call_json(
            "call-1",
            "read_file",
            serde_json::json!({ "path": "src/main.rs" }),
        )];
        let prompt = build_summary_prompt(&items);
        assert_eq!(prompt, "[ToolCall] read_file");
        assert!(!prompt.contains("src/main.rs"));
    }

    #[test]
    fn strips_tool_result_content() {
        let items = vec![Item::tool_result_with_content(
            "call-1",
            "read 3 lines",
            "fn main() { println!(\"hello\"); }",
        )];
        let prompt = build_summary_prompt(&items);
        assert_eq!(prompt, "[ToolResult] read 3 lines");
        assert!(!prompt.contains("println"));
    }

    #[test]
    fn drops_reasoning_entirely() {
        let items = vec![
            Item::user_message("hi"),
            Item::reasoning("internal deliberation"),
            Item::assistant_message("hello"),
        ];
        let prompt = build_summary_prompt(&items);
        assert!(prompt.contains("[User] hi"));
        assert!(prompt.contains("[Assistant] hello"));
        assert!(!prompt.contains("Reasoning"));
        assert!(!prompt.contains("deliberation"));
    }

    #[test]
    fn worker_manifest_generation_settings_become_request_config() {
        let manifest = WorkerManifest {
            instruction: "unused".into(),
            max_tokens: Some(1024),
            max_turns: None,
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: vec!["\n\n".into(), "</stop>".into()],
            reasoning: None,
            tool_output: manifest::ToolOutputLimits::default(),
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
        let prompt = build_summary_prompt(&items);
        assert_eq!(prompt, "[User] fix the bug\n\n[Assistant] done");
    }
}
