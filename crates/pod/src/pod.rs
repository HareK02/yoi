use std::path::PathBuf;
use std::sync::Arc;

use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::RequestConfig;
use llm_worker::Worker;
use llm_worker_persistence::{
    Session, SessionConfig, SessionError, SessionId, Store, StoreError,
};

use manifest::{PodManifest, Scope, WorkerManifest};

use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreToolCall,
};
use crate::hook_interceptor::HookInterceptor;

/// An independent agent execution unit.
///
/// Wraps a persistent [`Session`] with manifest metadata and an optional
/// directory scope. This is the primary abstraction in insomnia.
pub struct Pod<C: LlmClient, St: Store> {
    manifest: PodManifest,
    session: Session<C, St>,
    scope: Option<Scope>,
    hook_builder: HookRegistryBuilder,
    interceptor_installed: bool,
}

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// Create a new Pod from a pre-built Worker and store.
    ///
    /// The caller is responsible for constructing the `LlmClient` from the
    /// manifest's provider config. This keeps Pod free of provider-specific
    /// dependencies.
    pub async fn new(
        manifest: PodManifest,
        worker: Worker<C>,
        store: St,
        scope: Option<Scope>,
    ) -> Result<Self, PodError> {
        let session = Session::new(worker, store, SessionConfig::default()).await?;
        Ok(Self {
            manifest,
            session,
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
        let session = Session::restore(client, store, session_id, SessionConfig::default()).await?;
        Ok(Self {
            manifest,
            session,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
        })
    }

    /// The session ID used for persistence.
    pub fn session_id(&self) -> SessionId {
        self.session.session_id()
    }

    /// The Pod's manifest.
    pub fn manifest(&self) -> &PodManifest {
        &self.manifest
    }

    /// The Pod's directory scope, if any.
    pub fn scope(&self) -> Option<&Scope> {
        self.scope.as_ref()
    }

    /// Direct access to the underlying session.
    ///
    /// Use this to register tools, hooks, or subscribers on the worker
    /// before calling [`run`](Self::run).
    pub fn session_mut(&mut self) -> &mut Session<C, St> {
        &mut self.session
    }

    // --- Hook registration ---
    //
    // Hooks must be registered before the first call to `run()` or `resume()`.
    // Attempting to add a hook after execution has started will panic.

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
            self.session.worker_mut().set_interceptor(interceptor);
            self.interceptor_installed = true;
        }
    }

    /// Send user input and run until the LLM turn completes.
    pub async fn run(&mut self, input: impl Into<String>) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        let result = self.session.run(input).await?;
        Ok(result.into())
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<PodRunResult, PodError> {
        self.ensure_interceptor_installed();
        let result = self.session.resume().await?;
        Ok(result.into())
    }
}

impl<St: Store> Pod<Box<dyn LlmClient>, St> {
    /// Create a Pod entirely from a manifest.
    ///
    /// Builds the LLM client from the provider config, applies worker
    /// settings, and creates a new persistent session.
    pub async fn from_manifest(
        manifest: PodManifest,
        store: St,
        scope: Option<Scope>,
        manifest_dir: Option<PathBuf>,
    ) -> Result<Self, PodError> {
        let client = provider::build_client(&manifest.provider, manifest_dir.as_deref())?;
        let mut worker = Worker::new(client);
        apply_worker_manifest(&mut worker, &manifest.worker);
        let session = Session::new(worker, store, SessionConfig::default()).await?;
        Ok(Self {
            manifest,
            session,
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

impl From<llm_worker::WorkerResult> for PodRunResult {
    fn from(r: llm_worker::WorkerResult) -> Self {
        match r {
            llm_worker::WorkerResult::Finished => PodRunResult::Finished,
            llm_worker::WorkerResult::Paused => PodRunResult::Paused,
            llm_worker::WorkerResult::LimitReached => PodRunResult::LimitReached,
        }
    }
}

/// Pod errors.
#[derive(Debug, thiserror::Error)]
pub enum PodError {
    #[error(transparent)]
    Session(#[from] SessionError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("scope violation: {path} is outside the allowed directory")]
    ScopeViolation { path: String },

    #[error(transparent)]
    Provider(#[from] provider::ProviderError),
}
