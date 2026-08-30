use std::path::PathBuf;

use agen::llm_client::client::LlmClient;
use session_store::{Store, WorkerMetadataStore};
use thiserror::Error;
use workdir::WorkdirSessionHandle;

use crate::PromptCatalogSource;
use crate::controller::{
    ShutdownReceiver, WorkerController, WorkerControllerTransport, WorkerHandle,
};
use crate::worker::{Worker, WorkerError, WorkerFilesystemAuthority, WorkerWorkspaceContext};
use manifest::WorkerManifest;

/// Filesystem layout used by a Worker controller started through the reusable
/// bootstrap boundary.
#[derive(Debug, Clone)]
pub enum WorkerBootstrapLayout {
    /// A direct Worker rooted below the supplied runtime base directory.
    Direct { runtime_base: PathBuf },
    /// A runtime-managed Worker with an exact persisted run directory.
    RuntimeManagedRun { run_dir: PathBuf },
}

/// Construction and controller inputs that are stable for one Worker launch.
pub struct WorkerBootstrap<St> {
    manifest: WorkerManifest,
    store: St,
    prompt_catalog: PromptCatalogSource,
    workspace_context: WorkerWorkspaceContext,
    filesystem_authority: WorkerFilesystemAuthority,
    layout: WorkerBootstrapLayout,
    transport: WorkerControllerTransport,
    model_client: Option<Box<dyn LlmClient>>,
    workdir_session: Option<WorkdirSessionHandle>,
}

/// A constructed Worker whose host-owned live bindings can still be installed
/// before Feature installation and controller exposure.
pub struct PreparedWorker<C: LlmClient, St: Store> {
    worker: Worker<C, St>,
    layout: WorkerBootstrapLayout,
    transport: WorkerControllerTransport,
}

/// Live controller returned only after Worker construction and feature
/// installation have completed successfully.
pub struct BootstrappedWorker {
    pub handle: WorkerHandle,
    pub shutdown: ShutdownReceiver,
}

#[derive(Debug, Error)]
pub enum WorkerBootstrapError {
    #[error("worker construction failed")]
    Worker(#[source] WorkerError),
    #[error("worker controller startup failed")]
    Controller {
        #[source]
        source: std::io::Error,
        cleanup_failed: bool,
    },
}

impl<St> WorkerBootstrap<St>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        manifest: WorkerManifest,
        store: St,
        prompt_catalog: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        layout: WorkerBootstrapLayout,
        transport: WorkerControllerTransport,
    ) -> Self {
        Self {
            manifest,
            store,
            prompt_catalog,
            workspace_context,
            filesystem_authority,
            layout,
            transport,
            model_client: None,
            workdir_session: None,
        }
    }

    /// Inject a process-owned model client. This is primarily useful for
    /// embedded hosts and deterministic tests that must not start an external
    /// model server.
    pub fn with_model_client<C>(mut self, model_client: C) -> Self
    where
        C: LlmClient + 'static,
    {
        self.model_client = Some(Box::new(model_client));
        self
    }

    /// Bind an already materialized Workdir session instead of asking the
    /// Worker to derive one from filesystem authority.
    pub fn with_workdir_session(mut self, workdir_session: WorkdirSessionHandle) -> Self {
        self.workdir_session = Some(workdir_session);
        self
    }

    /// Construct the Worker without exposing a controller handle. Runtime hosts
    /// use this seam to bind Workdir, observation, Flow, and other live services
    /// before [`PreparedWorker::start`] performs Feature installation.
    pub async fn prepare(
        self,
    ) -> Result<PreparedWorker<Box<dyn LlmClient>, St>, WorkerBootstrapError> {
        let mut worker = Worker::from_manifest_with_context_and_model_client(
            self.manifest,
            self.store,
            self.prompt_catalog,
            self.workspace_context,
            self.filesystem_authority,
            self.model_client,
        )
        .await
        .map_err(WorkerBootstrapError::Worker)?;

        if let Some(workdir_session) = self.workdir_session {
            worker.bind_workdir_session(Some(workdir_session));
        }
        Ok(PreparedWorker::new(worker, self.layout, self.transport))
    }

    pub async fn prepare_restored(
        self,
        worker_name: &str,
    ) -> Result<PreparedWorker<Box<dyn LlmClient>, St>, WorkerBootstrapError> {
        let mut worker =
            Worker::restore_pending_from_worker_metadata_with_context_and_model_client(
                worker_name,
                self.manifest,
                self.store,
                self.prompt_catalog,
                self.workspace_context,
                self.filesystem_authority,
                self.model_client,
            )
            .await
            .map_err(WorkerBootstrapError::Worker)?;

        if let Some(workdir_session) = self.workdir_session {
            worker.bind_workdir_session(Some(workdir_session));
        }
        Ok(PreparedWorker::new(worker, self.layout, self.transport))
    }

    pub async fn start(self) -> Result<BootstrappedWorker, WorkerBootstrapError> {
        self.prepare().await?.start().await
    }
}

impl<C, St> PreparedWorker<C, St>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    /// Wrap a restored Worker in the same pre-exposure lifecycle used by fresh
    /// bootstraps.
    pub fn new(
        worker: Worker<C, St>,
        layout: WorkerBootstrapLayout,
        transport: WorkerControllerTransport,
    ) -> Self {
        Self {
            worker,
            layout,
            transport,
        }
    }

    pub fn worker(&self) -> &Worker<C, St> {
        &self.worker
    }

    pub fn worker_mut(&mut self) -> &mut Worker<C, St> {
        &mut self.worker
    }

    pub async fn start(self) -> Result<BootstrappedWorker, WorkerBootstrapError> {
        start_worker_controller(self.worker, self.layout, self.transport).await
    }
}

/// Start the shared direct/runtime-managed controller lifecycle for an already
/// constructed Worker. Restore paths use this after replaying durable state;
/// fresh hosts normally use [`WorkerBootstrap::start`].
pub async fn start_worker_controller<C, St>(
    worker: Worker<C, St>,
    layout: WorkerBootstrapLayout,
    transport: WorkerControllerTransport,
) -> Result<BootstrappedWorker, WorkerBootstrapError>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    let cleanup_session = worker.workdir_session().cloned();
    let controller = match layout {
        WorkerBootstrapLayout::Direct { runtime_base } => {
            WorkerController::spawn_with_transport(worker, &runtime_base, transport).await
        }
        WorkerBootstrapLayout::RuntimeManagedRun { run_dir } => {
            WorkerController::spawn_runtime_managed_run_with_transport(worker, &run_dir, transport)
                .await
        }
    };

    match controller {
        Ok((handle, shutdown)) => Ok(BootstrappedWorker { handle, shutdown }),
        Err(source) => {
            let cleanup_failed = match cleanup_session {
                Some(session) => session.close().await.is_err(),
                None => false,
            };
            Err(WorkerBootstrapError::Controller {
                source,
                cleanup_failed,
            })
        }
    }
}
