use std::path::PathBuf;
use std::sync::Arc;

use llm_worker::Item;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::RequestConfig;
use llm_worker::state::Mutable;
use llm_worker::{Worker, WorkerError, WorkerResult};
use session_store::{
    EntryHash, Outcome, SessionId, SessionStartState, Store, StoreError,
};
use tracing::{info, warn};

use manifest::{PodManifest, Scope, WorkerManifest};

use crate::compact_interceptor::CompactInterceptor;
use crate::compact_state::CompactState;
use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreToolCall,
};
use crate::hook_interceptor::HookInterceptor;

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
    scope: Option<Scope>,
    hook_builder: HookRegistryBuilder,
    interceptor_installed: bool,
    /// Directory containing the manifest file (needed for api_key_file resolution).
    manifest_dir: Option<PathBuf>,
    /// Shared compaction state (present when compact_threshold is configured).
    compact_state: Option<Arc<CompactState>>,
}

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// Create a new Pod from a pre-built Worker and store.
    pub async fn new(
        manifest: PodManifest,
        worker: Worker<C>,
        store: St,
        scope: Option<Scope>,
    ) -> Result<Self, PodError> {
        let state = SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        };
        let (session_id, head_hash) = session_store::create_session(&store, state).await?;
        Ok(Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: Some(head_hash),
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            manifest_dir: None,
            compact_state: None,
        })
    }

    /// Restore a Pod from a persisted session.
    pub async fn restore(
        session_id: SessionId,
        manifest: PodManifest,
        client: C,
        store: St,
        scope: Option<Scope>,
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

        Ok(Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: state.head_hash,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            manifest_dir: None,
            compact_state: None,
        })
    }

    /// The session ID used for persistence.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The Pod's manifest.
    pub fn manifest(&self) -> &PodManifest {
        &self.manifest
    }

    /// The Pod's directory scope, if any.
    pub fn scope(&self) -> Option<&Scope> {
        self.scope.as_ref()
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
            let builder = std::mem::take(&mut self.hook_builder);
            let registry = Arc::new(builder.build());
            let hook_interceptor = HookInterceptor::new(registry);

            let compact_threshold = self
                .manifest
                .compaction
                .as_ref()
                .and_then(|c| c.compact_threshold);

            if let Some(threshold) = compact_threshold {
                let retained = self
                    .manifest
                    .compaction
                    .as_ref()
                    .map(|c| c.compact_retained_turns)
                    .unwrap_or(2);

                let state = Arc::new(CompactState::new(threshold, retained));

                // Track input_tokens via on_usage callback.
                let state_for_usage = state.clone();
                self.worker_mut().on_usage(move |event| {
                    if let Some(tokens) = event.input_tokens {
                        state_for_usage.update_input_tokens(tokens);
                    }
                });

                let interceptor = CompactInterceptor::new(hook_interceptor, state.clone());
                self.worker_mut().set_interceptor(interceptor);
                self.compact_state = Some(state);
            } else {
                self.worker_mut().set_interceptor(hook_interceptor);
            }

            self.interceptor_installed = true;
        }
    }

    /// Send user input and run until the LLM turn completes.
    ///
    /// If the between-turns compaction threshold is exceeded mid-run,
    /// the Worker is aborted, history is compacted, and execution resumes
    /// automatically.
    pub async fn run(&mut self, input: impl Into<String>) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
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
        self.ensure_session_head().await?;

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → resume → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.handle_worker_result(result, history_before).await
    }

    /// Ensure session head exists (fork if needed).
    async fn ensure_session_head(&mut self) -> Result<(), PodError> {
        let w = self.worker.as_ref().unwrap();
        session_store::ensure_head_or_fork(
            &self.store,
            &mut self.session_id,
            &mut self.head_hash,
            SessionStartState {
                system_prompt: w.get_system_prompt(),
                config: w.request_config(),
                history: w.history(),
            },
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PodRunResult, PodError>> + Send + '_>>
    {
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
            Some(s) if !s.is_disabled() && s.exceeds_post_run() && !s.just_compacted() => {
                s.clone()
            }
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
        session_store::save_delta(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            new_items,
        )
        .await?;

        let turn_count = self.worker.as_ref().unwrap().turn_count();
        session_store::save_turn_end(
            &self.store,
            self.session_id,
            &mut self.head_hash,
            turn_count,
        )
        .await?;

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
    pub async fn compact(
        &mut self,
        retained_turns: usize,
    ) -> Result<SessionId, PodError> {
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

        let out = summary_worker.run(summary_prompt).await
            .map_err(PodError::Worker)?;
        let summary_text = out.worker
            .history()
            .iter()
            .filter_map(|item| {
                if item.is_assistant_message() { item.as_text().map(String::from) } else { None }
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
        let old_head_hash = self.head_hash.clone()
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

        // Swap in the new session state.
        self.session_id = new_session_id;
        self.head_hash = Some(new_head_hash);
        self.worker.as_mut().unwrap().set_history(new_history);

        Ok(new_session_id)
    }

    /// Build the LlmClient for the compactor Worker.
    ///
    /// Uses `compaction.provider` from manifest if set, otherwise clones
    /// the main client.
    fn build_compactor_client(&self) -> Result<Box<dyn LlmClient>, PodError> {
        if let Some(ref compaction) = self.manifest.compaction {
            if let Some(ref provider_config) = compaction.provider {
                let client = provider::build_client(
                    provider_config,
                    self.manifest_dir.as_deref().map(|p| p.as_ref()),
                )?;
                return Ok(client);
            }
        }
        let worker = self.worker.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }
}

impl<St: Store> Pod<Box<dyn LlmClient>, St> {
    /// Create a Pod entirely from a manifest.
    pub async fn from_manifest(
        manifest: PodManifest,
        store: St,
        scope: Option<Scope>,
        manifest_dir: Option<PathBuf>,
    ) -> Result<Self, PodError> {
        let client = provider::build_client(&manifest.provider, manifest_dir.as_deref())?;
        let mut worker = Worker::new(client);
        apply_worker_manifest(&mut worker, &manifest.worker);

        let state = SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        };
        let (session_id, head_hash) = session_store::create_session(&store, state).await?;
        Ok(Self {
            manifest,
            worker: Some(worker),
            store,
            session_id,
            head_hash: Some(head_hash),
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            manifest_dir,
            compact_state: None,
        })
    }

}

/// Apply worker-level manifest settings to a Worker.
pub fn apply_worker_manifest<C: LlmClient>(worker: &mut Worker<C>, wm: &WorkerManifest) {
    if let Some(ref prompt) = wm.system_prompt {
        worker.set_system_prompt(prompt);
    }
    let mut config = RequestConfig::new();
    if let Some(max_tokens) = wm.max_tokens {
        config.max_tokens = Some(max_tokens);
    }
    if let Some(temperature) = wm.temperature {
        config.temperature = Some(temperature);
    }
    worker.set_request_config(config);
    worker.set_max_turns(wm.max_turns.map(|n| n.get()));
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
                let text: String = content.iter().map(|p| p.as_text()).collect::<Vec<_>>().join("");
                lines.push(format!("[{role_label}] {text}"));
            }
            Item::ToolCall { name, arguments, .. } => {
                lines.push(format!("[ToolCall] {name}({arguments})"));
            }
            Item::ToolResult { summary, content, .. } => {
                match content {
                    Some(c) => lines.push(format!("[ToolResult] {summary}\n{c}")),
                    None => lines.push(format!("[ToolResult] {summary}")),
                }
            }
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

    #[error("scope violation: {path} is outside the allowed directory")]
    ScopeViolation { path: String },

    #[error(transparent)]
    Provider(#[from] provider::ProviderError),

    #[error("compaction thrash: context still exceeds threshold immediately after compact")]
    CompactThrash,
}
