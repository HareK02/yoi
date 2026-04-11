use std::path::PathBuf;
use std::sync::Arc;

use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::RequestConfig;
use llm_worker::state::Mutable;
use llm_worker::{Worker, WorkerError, WorkerResult};
use session_store::{
    EntryHash, Outcome, SessionId, SessionStartState, Store, StoreError,
};

use manifest::{PodManifest, Scope, WorkerManifest};

use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreToolCall,
};
use crate::hook_interceptor::HookInterceptor;

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
    fn ensure_interceptor_installed(&mut self) {
        if !self.interceptor_installed {
            let builder = std::mem::take(&mut self.hook_builder);
            let registry = Arc::new(builder.build());
            let interceptor = HookInterceptor::new(registry);
            self.worker_mut().set_interceptor(interceptor);
            self.interceptor_installed = true;
        }
    }

    /// Send user input and run until the LLM turn completes.
    pub async fn run(&mut self, input: impl Into<String>) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();

        // Split borrow: access worker field directly to allow concurrent
        // mutable borrows on session_id / head_hash.
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

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → run → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.run(input).await;
        self.worker = Some(locked.unlock());

        self.persist_turn(history_before, &result).await?;
        result.map(PodRunResult::from).map_err(PodError::Worker)
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();

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

        let history_before = self.worker.as_ref().unwrap().history().len();

        // lock → resume → unlock
        let worker = self.worker.take().expect("worker taken during run");
        let mut locked = worker.lock();
        let result = locked.resume().await;
        self.worker = Some(locked.unlock());

        self.persist_turn(history_before, &result).await?;
        result.map(PodRunResult::from).map_err(PodError::Worker)
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
        }
    }
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
}
