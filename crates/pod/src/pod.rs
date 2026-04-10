use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::RequestConfig;
use llm_worker::Worker;
use llm_worker_persistence::{
    Session, SessionConfig, SessionError, SessionId, Store, StoreError,
};

use manifest::{PodManifest, Scope, WorkerManifest};

/// Pod identifier. UUID v7 (time-ordered).
pub type PodId = uuid::Uuid;

/// Generate a new Pod ID.
pub fn new_pod_id() -> PodId {
    uuid::Uuid::now_v7()
}

/// An independent agent execution unit.
///
/// Wraps a persistent [`Session`] with manifest metadata and an optional
/// directory scope. This is the primary abstraction in insomnia.
pub struct Pod<C: LlmClient, St: Store> {
    id: PodId,
    manifest: PodManifest,
    session: Session<C, St>,
    scope: Option<Scope>,
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
            id: new_pod_id(),
            manifest,
            session,
            scope,
        })
    }

    /// Restore a Pod from a persisted session.
    pub async fn restore(
        id: PodId,
        session_id: SessionId,
        manifest: PodManifest,
        client: C,
        store: St,
        scope: Option<Scope>,
    ) -> Result<Self, PodError> {
        let session = Session::restore(client, store, session_id, SessionConfig::default()).await?;
        Ok(Self {
            id,
            manifest,
            session,
            scope,
        })
    }

    /// The Pod's unique identifier.
    pub fn id(&self) -> PodId {
        self.id
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

    /// Send user input and run until the LLM turn completes.
    pub async fn run(&mut self, input: impl Into<String>) -> Result<PodRunResult, PodError> {
        let result = self.session.run(input).await?;
        Ok(result.into())
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<PodRunResult, PodError> {
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
    ) -> Result<Self, PodError> {
        let client = provider::build_client(&manifest.provider)?;
        let mut worker = Worker::new(client);
        apply_worker_manifest(&mut worker, &manifest.worker);
        let session = Session::new(worker, store, SessionConfig::default()).await?;
        Ok(Self {
            id: new_pod_id(),
            manifest,
            session,
            scope,
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
}

/// Result of a Pod run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodRunResult {
    /// The LLM finished its turn normally.
    Finished,
    /// The LLM paused (e.g. awaiting user confirmation via a hook).
    Paused,
}

impl From<llm_worker::WorkerResult> for PodRunResult {
    fn from(r: llm_worker::WorkerResult) -> Self {
        match r {
            llm_worker::WorkerResult::Finished => PodRunResult::Finished,
            llm_worker::WorkerResult::Paused => PodRunResult::Paused,
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
