use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use llm_worker::Item;
use llm_worker::llm_client::RequestConfig;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::state::Mutable;
use llm_worker::{ToolOutputLimits, Worker, WorkerError, WorkerResult};
use session_store::{
    EntryHash, Outcome, SessionId, SessionStartState, Store, StoreError, UsageRecord,
};
use tracing::{info, warn};

use manifest::{PodManifest, PodManifestConfig, ResolveError, Scope, ScopeError, WorkerManifest};

use crate::agents_md::read_agents_md;
use crate::compact_state::CompactState;
use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreRequestInfo, PreToolCall,
};
use crate::notification_buffer::NotificationBuffer;
use crate::notifier::Notifier;
use crate::pod_interceptor::PodInterceptor;
use crate::prompt_loader::PromptLoader;
use crate::runtime_dir;
use crate::scope_lock::{self, ScopeAllocationGuard, ScopeLockError};
use crate::system_prompt::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
use crate::usage_tracker::UsageTracker;
use protocol::{NotificationLevel, NotificationSource};
use async_trait::async_trait;
use llm_worker::interceptor::PreRequestAction;

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

const SUMMARY_SYSTEM_PROMPT: &str = "\
You are a context compaction assistant. Your job is to hand the next session a \
structured summary plus pointers to the files it actually needs.\n\n\
Tools you can call:\n\
- `read_file(file_path, offset?, limit?)` — inspect referenced files before deciding.\n\
- `mark_read_required(file_path, offset?, limit?)` — inject a file's contents into the \
  next session as an auto-read system message. Counts against `auto_read_budget`.\n\
- `add_reference(file_path)` — record a file path the next session should know about \
  without embedding its contents.\n\
- `write_summary(text)` — deliver the final structured summary. May be called multiple \
  times; only the last call is kept.\n\n\
Always finish by calling `write_summary`. Produce the summary in this exact format:\n\n\
## Completed Tasks\n\
### (task name)\n\
- what was done (use concrete type / file / function names)\n\
- gotchas or facts that came up\n\n\
## Active Task\n\
### (task name)\n\
- goal\n\
- current state (what is done / not done)\n\
- next step\n\n\
## Key Decisions\n\
- (decision) — (reason)\n\n\
## User Directives\n\
- \"verbatim user line\" — only include directives whose wording the next session \
  should not lose.\n\n\
## Current Work\n\
(2–3 lines on what was happening just before compaction).\n\n\
Keep code snippets and raw tool output OUT of the summary — that is what auto-read \
and references are for. Target 1000–2000 tokens.";

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
    notifier: Option<Notifier>,
    /// Queue of pending `Method::Notify` notifications awaiting
    /// injection into the next LLM request. Shared with the
    /// PodInterceptor installed in `ensure_interceptor_installed`.
    pending_notifications: NotificationBuffer,
    /// Scope allocation in the machine-wide lock file. `Some` for
    /// Pods built via `from_manifest` (production path); `None` for
    /// lower-level constructors (`Pod::new`, `Pod::restore`) that
    /// bypass the registry. Kept purely for its `Drop` impl, which
    /// releases the allocation when the Pod is dropped.
    #[allow(dead_code)]
    scope_allocation: Option<ScopeAllocationGuard>,
    /// Socket path of the spawning Pod. `Some` only for Pods built via
    /// `from_manifest_spawned`. Consumed by the controller to fire
    /// `Method::PodEvent` reports upward (turn end, error, shutdown,
    /// scope sub-delegation).
    callback_socket: Option<PathBuf>,
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
            notifier: None,
            pending_notifications: NotificationBuffer::new(),
            scope_allocation: None,
            callback_socket: None,
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

    /// Restore a Pod from a persisted session.
    pub async fn restore(
        session_id: SessionId,
        manifest: PodManifest,
        client: C,
        store: St,
        pwd: PathBuf,
        scope: Scope,
    ) -> Result<Self, PodError> {
        let state = session_store::restore(&store, session_id).await?;
        let mut worker = Worker::new(client);
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
        worker.set_history(state.history);
        worker.set_request_config(state.config);
        worker.set_turn_count(state.turn_count);
        worker.set_last_run_interrupted(state.last_run_interrupted);
        if anchored_on_summary {
            worker.set_cache_anchor(Some(0));
        }

        let mut pod = Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: state.head_hash,
            pwd,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            usage_history: Arc::new(Mutex::new(state.usage_history)),
            tracker: None,
            system_prompt_template: None,
            notifier: None,
            pending_notifications: NotificationBuffer::new(),
            scope_allocation: None,
            callback_socket: None,
        };
        pod.apply_prune_from_manifest();
        Ok(pod)
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
    pub fn attach_notifier(&mut self, notifier: Notifier) {
        self.notifier = Some(notifier);
    }

    fn notify(&self, level: NotificationLevel, source: NotificationSource, message: String) {
        if let Some(n) = self.notifier.as_ref() {
            n.notify(level, source, message);
        }
    }

    /// Push a `Method::Notify` entry onto the pending buffer.
    ///
    /// The notification will be injected as an `Item::system_message`
    /// into the next outgoing LLM request context (not into history).
    /// See [`NotificationBuffer`] for overflow behaviour.
    pub fn push_notification(&self, message: String) {
        self.pending_notifications.push(message);
    }

    /// Shared handle to the pending notification buffer.
    ///
    /// The Controller holds a clone so that `Method::Notify` arriving
    /// while `pod.run()` is in flight can still reach the interceptor.
    pub fn notification_buffer_handle(&self) -> NotificationBuffer {
        self.pending_notifications.clone()
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

            let usage_history_handle = compact_state
                .as_ref()
                .map(|_| self.usage_history.clone());

            let interceptor = PodInterceptor::new(
                registry,
                compact_state,
                usage_history_handle,
                self.pending_notifications.clone(),
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
        let notifier = self.notifier.clone();
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
            if let Some(n) = notifier.as_ref() {
                n.notify(
                    NotificationLevel::Warn,
                    NotificationSource::AgentsMd,
                    warning,
                );
            }
        }
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            cwd: &self.pwd,
            scope: &self.scope,
            tool_names,
            agents_md: agents_md_read.body,
        };
        let rendered = template
            .render(&ctx)
            .map_err(|source| PodError::SystemPromptRender { source })?;
        worker.set_system_prompt(rendered);
        Ok(())
    }

    /// Send user input and run until the LLM turn completes.
    ///
    /// If the between-turns compaction threshold is exceeded mid-run,
    /// the Worker is aborted, history is compacted, and execution resumes
    /// automatically.
    pub async fn run(&mut self, input: impl Into<String>) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized()?;
        self.ensure_session_head().await?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → run → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.run(input).await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
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
        session_store::ensure_head_or_fork(
            &self.store,
            &mut self.session_id,
            &mut self.head_hash,
            state,
        )
        .await?;
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

            match self.compact(retained).await {
                Ok(new_session_id) => {
                    info!(
                        new_session_id = %new_session_id,
                        "Compaction succeeded, resuming execution"
                    );
                    if let Some(ref state) = self.compact_state {
                        state.record_compact_success();
                    }
                    self.resume().await
                }
                Err(e) => {
                    warn!(error = %e, "Compaction failed during run");
                    self.notify(
                        NotificationLevel::Error,
                        NotificationSource::Compactor,
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
        match self.compact(retained).await {
            Ok(new_session_id) => {
                info!(
                    new_session_id = %new_session_id,
                    "Proactive post-run compaction succeeded"
                );
                state.record_compact_success();
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "Proactive post-run compaction failed");
                self.notify(
                    NotificationLevel::Warn,
                    NotificationSource::Compactor,
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
        let outcome = match result {
            Ok(WorkerResult::Finished) => Outcome::Finished,
            Ok(WorkerResult::Paused) => Outcome::Paused,
            Ok(WorkerResult::LimitReached) => Outcome::LimitReached,
            Ok(WorkerResult::Yielded) => Outcome::Yielded,
            Err(e) => Outcome::Error {
                message: e.to_string(),
            },
        };
        session_store::save_outcome(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            outcome,
            interrupted,
        )
        .await?;

        Ok(())
    }

    /// Compact the current session by summarising history via a
    /// disposable Worker, then replacing history with
    /// `[summary, ...recent_turns]` and creating a new session.
    ///
    /// The summary Worker uses:
    /// - `compaction.provider` from the manifest if configured, or
    /// - a clone of the main LlmClient via `clone_boxed()`.
    ///
    /// Returns the new session ID.
    pub async fn compact(&mut self, retained_tokens: u64) -> Result<SessionId, PodError> {
        use std::sync::atomic::{AtomicU64, Ordering};

        use crate::compact_worker::{
            CompactWorkerContext, CompactWorkerInterceptor, add_reference_tool,
            mark_read_required_tool, slice_lines, write_summary_tool,
        };

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
            .map(|c| (c.compact_auto_read_budget, c.compact_worker_max_input_tokens))
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
        let mut summary_worker = Worker::new(summary_client)
            .system_prompt(SUMMARY_SYSTEM_PROMPT)
            .temperature(0.0);
        summary_worker.set_max_tokens(4096);

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
        summary_worker
            .register_tool(mark_read_required_tool(scoped_fs.clone(), ctx.clone()));
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
            let _ = locked_worker
                .run(prompt)
                .await
                .map_err(PodError::Worker)?;
        }

        let final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
        let summary_text = final_ctx
            .summary
            .clone()
            .ok_or(PodError::CompactSummaryMissing)?;

        // Re-read each auto-read target through ScopedFs and render the
        // requested slice. Errors are logged and skipped rather than
        // aborting compaction — a missing / moved file should not fail
        // the whole compact.
        let mut auto_read_messages = Vec::new();
        for req in &final_ctx.read_required {
            match scoped_fs.read_bytes(&req.path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let body = slice_lines(&text, req.offset.unwrap_or(0), req.limit);
                    let range = match (req.offset, req.limit) {
                        (None, None) => String::new(),
                        (Some(off), None) => format!(":{}-", off + 1),
                        (None, Some(lim)) => format!(":1-{lim}"),
                        (Some(off), Some(lim)) => {
                            format!(":{}-{}", off + 1, off.saturating_add(lim))
                        }
                    };
                    auto_read_messages.push(Item::system_message(format!(
                        "[Auto-read file: {}{range}]\n{body}",
                        req.path.display()
                    )));
                }
                Err(e) => {
                    warn!(
                        path = %req.path.display(),
                        error = %e,
                        "auto-read target could not be read; skipping",
                    );
                }
            }
        }

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

        // Build new history: [summary, ...auto-read, references, ...retained].
        let mut new_history = Vec::with_capacity(
            1 + auto_read_messages.len() + reference_message.is_some() as usize
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

        Ok(new_session_id)
    }

    /// Build the LlmClient for the compactor Worker.
    ///
    /// Uses `compaction.provider` from manifest if set, otherwise clones
    /// the main client.
    fn build_compactor_client(&self) -> Result<Box<dyn LlmClient>, PodError> {
        if let Some(ref compaction) = self.manifest.compaction {
            if let Some(ref provider_config) = compaction.provider {
                let client = provider::build_client(provider_config)?;
                return Ok(client);
            }
        }
        let worker = self.worker.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
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
        let pwd = current_pwd()?;
        let scope = Scope::from_config(&manifest.scope).map_err(PodError::Scope)?;
        if !scope.is_readable(&pwd) {
            return Err(PodError::PwdOutsideScope { pwd });
        }

        // Register this Pod in the machine-wide scope-lock registry
        // before building anything else, so a spawn that conflicts on
        // scope fails fast (and without having paid for client setup).
        let socket_path = runtime_dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.pod.name)
            .join("sock");
        let scope_allocation = scope_lock::install_top_level(
            manifest.pod.name.clone(),
            std::process::id(),
            socket_path,
            scope.allow_rules(),
        )?;

        let client = provider::build_client(&manifest.provider)?;
        let mut worker = Worker::new(client);
        apply_worker_manifest(&mut worker, &manifest.worker);

        // Resolve the instruction reference and parse the resulting
        // template eagerly (syntax check only). Rendering is deferred
        // to `ensure_system_prompt_materialized` at first turn so
        // runtime values (date, tools, scope summary, ...) can be
        // injected.
        let system_prompt_template = Some(
            SystemPromptTemplate::parse(&manifest.worker.instruction, loader)
                .map_err(|source| PodError::InvalidSystemPromptTemplate { source })?,
        );

        // Session creation is deferred to the first run (see
        // `ensure_session_head`) so the SessionStart entry can capture
        // the rendered system prompt, not the raw template source.
        let session_id = session_store::new_session_id();
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
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            system_prompt_template,
            notifier: None,
            pending_notifications: NotificationBuffer::new(),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
        };
        pod.apply_prune_from_manifest();
        Ok(pod)
    }

    /// Build a Pod spawned by another Pod (sibling process).
    ///
    /// Behaves like [`Pod::from_manifest`] but claims the scope
    /// allocation that the spawner pre-registered via
    /// [`scope_lock::delegate_scope`], rather than installing a new
    /// top-level entry. `callback_socket` carries the spawner's
    /// Unix-socket path so the spawned Pod can send `Method::Notify`
    /// back to the spawner; it is stored but unused in the
    /// `spawn-pod-tool` ticket — the receiving side lands in the
    /// follow-up `pod-callback` ticket.
    pub async fn from_manifest_spawned(
        manifest: PodManifest,
        store: St,
        loader: PromptLoader,
        callback_socket: PathBuf,
    ) -> Result<Self, PodError> {
        let pwd = current_pwd()?;
        let scope = Scope::from_config(&manifest.scope).map_err(PodError::Scope)?;
        if !scope.is_readable(&pwd) {
            return Err(PodError::PwdOutsideScope { pwd });
        }

        let scope_allocation =
            scope_lock::adopt_allocation(manifest.pod.name.clone(), std::process::id())?;

        let client = provider::build_client(&manifest.provider)?;
        let mut worker = Worker::new(client);
        apply_worker_manifest(&mut worker, &manifest.worker);

        let system_prompt_template = Some(
            SystemPromptTemplate::parse(&manifest.worker.instruction, loader)
                .map_err(|source| PodError::InvalidSystemPromptTemplate { source })?,
        );

        let session_id = session_store::new_session_id();
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
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            system_prompt_template,
            notifier: None,
            pending_notifications: NotificationBuffer::new(),
            scope_allocation: Some(scope_allocation),
            callback_socket: Some(callback_socket),
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
    let mut config = RequestConfig::new();
    if let Some(max_tokens) = wm.max_tokens {
        config.max_tokens = Some(max_tokens);
    }
    if let Some(temperature) = wm.temperature {
        config.temperature = Some(temperature);
    }
    worker.set_request_config(config);
    worker.set_max_turns(wm.max_turns.map(|n| n.get()));
    worker.set_tool_output_limits(Some(ToolOutputLimits {
        default_max_bytes: wm.tool_output.default_max_bytes,
        per_tool: wm.tool_output.per_tool.clone(),
    }));
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
    out.push_str(
        "\n\nWhen you are done, call `write_summary` with the final 5-section text.",
    );
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
    cwd.canonicalize().map_err(|source| PodError::InvalidPwd {
        pwd: cwd,
        source,
    })
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
    fn keeps_user_and_assistant_messages() {
        let items = vec![
            Item::user_message("fix the bug"),
            Item::assistant_message("done"),
        ];
        let prompt = build_summary_prompt(&items);
        assert_eq!(prompt, "[User] fix the bug\n\n[Assistant] done");
    }
}
