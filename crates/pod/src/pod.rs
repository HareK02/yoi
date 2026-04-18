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
use crate::notifier::Notifier;
use crate::pod_interceptor::PodInterceptor;
use crate::prompt_loader::PromptLoader;
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
You are a context compaction assistant. \
Summarise the conversation below into a structured summary. \
Preserve concrete details: file paths, function names, error messages, decisions made. \
Use the following format:\n\n\
## Original Task\n\
(the user's original request)\n\n\
## Completed Work\n\
- (what was done, with specifics)\n\n\
## Key Discoveries\n\
- (facts, constraints, errors found)\n\n\
## Current State\n\
- (files changed, remaining work)";

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
        worker.set_history(state.history);
        worker.set_request_config(state.config);
        worker.set_turn_count(state.turn_count);
        worker.set_last_run_interrupted(state.last_run_interrupted);

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
    /// When `compact_threshold` is configured in the manifest, wraps the
    /// `HookInterceptor` in a [`CompactInterceptor`] and registers an
    /// `on_usage` callback to track `input_tokens`.
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

            let compact_threshold = self
                .manifest
                .compaction
                .as_ref()
                .and_then(|c| c.compact_threshold);

            let tracker_for_usage = self.usage_tracker.clone();

            let compact_state = if let Some(threshold) = compact_threshold {
                let retained = self
                    .manifest
                    .compaction
                    .as_ref()
                    .map(|c| c.compact_retained_turns)
                    .unwrap_or(2);

                let state = Arc::new(CompactState::new(threshold, retained));
                let state_for_usage = state.clone();
                self.worker_mut().on_usage(move |event| {
                    if let Some(tokens) = event.input_tokens {
                        state_for_usage.update_input_tokens(tokens);
                    }
                    tracker_for_usage.record_usage(event);
                });
                self.compact_state = Some(state.clone());
                Some(state)
            } else {
                self.worker_mut().on_usage(move |event| {
                    tracker_for_usage.record_usage(event);
                });
                None
            };

            let interceptor = PodInterceptor::new(registry, compact_state);
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
                .map(|s| s.retained_turns())
                .unwrap_or(2);

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
            Some(s) if !s.is_disabled() && s.exceeds_post_run() && !s.just_compacted() => s.clone(),
            _ => return Ok(()),
        };

        let retained = state.retained_turns();
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
    pub async fn compact(&mut self, retained_turns: usize) -> Result<SessionId, PodError> {
        let worker = self.worker.as_ref().expect("worker taken during run");
        let history = worker.history();

        // Identify turn boundaries (user message positions).
        let turn_starts: Vec<usize> = history
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_user_message())
            .map(|(i, _)| i)
            .collect();

        // Items to retain: everything from `retained_turns` turns ago onward.
        let retain_from = if turn_starts.len() > retained_turns {
            turn_starts[turn_starts.len() - retained_turns]
        } else {
            0
        };
        let retained_items = history[retain_from..].to_vec();
        let items_to_summarise = &history[..retain_from];

        // Build summary prompt.
        let summary_prompt = build_summary_prompt(items_to_summarise);

        // Create a disposable summary Worker.
        let summary_client: Box<dyn LlmClient> = self.build_compactor_client()?;
        let mut summary_worker = Worker::new(summary_client)
            .system_prompt(SUMMARY_SYSTEM_PROMPT)
            .temperature(0.0);
        summary_worker.set_max_tokens(2048);

        let out = summary_worker
            .run(summary_prompt)
            .await
            .map_err(PodError::Worker)?;
        let summary_text = out
            .worker
            .history()
            .iter()
            .filter_map(|item| {
                if item.is_assistant_message() {
                    item.as_text().map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Build new history: [summary as user message, ...retained].
        let mut new_history = Vec::with_capacity(retained_items.len() + 1);
        new_history.push(Item::system_message(format!(
            "[Compacted context summary]\n\n{summary_text}"
        )));
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
        self.worker.as_mut().unwrap().set_history(new_history);
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
    /// `manifest.pod.pwd` must already be an absolute path (the cascade
    /// layer — `PodManifestConfig` → `PodManifest` — is the sole place
    /// where path normalisation happens). The Pod builds its [`Scope`]
    /// from `manifest.scope`, canonicalizes the pwd, and validates that
    /// the resolved pwd is readable under that scope.
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
        let pwd = resolve_pwd(&manifest.pod.pwd)?;
        let scope = Scope::from_config(&manifest.scope, &pwd).map_err(PodError::Scope)?;
        if !scope.is_readable(&pwd) {
            return Err(PodError::PwdOutsideScope { pwd });
        }

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

/// Format conversation items into a text prompt for the summary Worker.
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
            Item::ToolCall {
                name, arguments, ..
            } => {
                lines.push(format!("[ToolCall] {name}({arguments})"));
            }
            Item::ToolResult {
                summary, content, ..
            } => match content {
                Some(c) => lines.push(format!("[ToolResult] {summary}\n{c}")),
                None => lines.push(format!("[ToolResult] {summary}")),
            },
            Item::Reasoning { text, .. } => {
                lines.push(format!("[Reasoning] {text}"));
            }
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

    #[error("pwd must be absolute: {}", .0.display())]
    PwdNotAbsolute(PathBuf),

    #[error("failed to parse manifest TOML: {0}")]
    ManifestParse(#[source] toml::de::Error),

    #[error("failed to resolve manifest config: {0}")]
    ManifestResolve(#[source] ResolveError),

    #[error(transparent)]
    Provider(#[from] provider::ProviderError),

    #[error("compaction thrash: context still exceeds threshold immediately after compact")]
    CompactThrash,

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
}

/// Canonicalize an absolute pwd (resolves symlinks and any `.`/`..`
/// components). Relative inputs are rejected — the cascade layer is
/// the sole source of path normalisation and must hand off an absolute
/// path.
fn resolve_pwd(pwd: &Path) -> Result<PathBuf, PodError> {
    if !pwd.is_absolute() {
        return Err(PodError::PwdNotAbsolute(pwd.to_path_buf()));
    }
    pwd.canonicalize().map_err(|source| PodError::InvalidPwd {
        pwd: pwd.to_path_buf(),
        source,
    })
}
