use std::path::PathBuf;

use agen::llm_client::client::LlmClient;
use session_store::{Store, WorkerMetadataStore};
use thiserror::Error;
use workdir::WorkdirSessionHandle;

use crate::PromptCatalogSource;
use crate::controller::{ShutdownReceiver, WorkerController, WorkerHandle};
pub use crate::worker::{PreparedRestoredWorker, RestoreCandidateWorkdirCleanup};
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
    model_client: Option<Box<dyn LlmClient>>,
    workdir_session: Option<WorkdirSessionHandle>,
    subjektiv_host: Option<std::sync::Arc<dyn crate::subjektiv::SubjektivHost>>,
}

/// A constructed Worker whose host-owned live bindings can still be installed
/// before Feature installation and controller exposure.
pub struct PreparedWorker<C: LlmClient, St: Store> {
    worker: Worker<C, St>,
    layout: WorkerBootstrapLayout,
}

/// Live controller returned only after Worker construction and feature
/// installation have completed successfully.
pub struct BootstrappedWorker {
    pub handle: WorkerHandle,
    pub shutdown: ShutdownReceiver,
    pub controller_task: tokio::task::JoinHandle<()>,
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
    pub fn new(
        manifest: WorkerManifest,
        store: St,
        prompt_catalog: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        layout: WorkerBootstrapLayout,
    ) -> Self {
        Self {
            manifest,
            store,
            prompt_catalog,
            workspace_context,
            filesystem_authority,
            layout,
            model_client: None,
            workdir_session: None,
            subjektiv_host: None,
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

    /// Bind an explicit, process-owned subject capability. Profile policy alone
    /// is inert; the connection is never persisted into the Worker manifest.
    pub fn with_subjektiv_host(
        mut self,
        host: std::sync::Arc<dyn crate::subjektiv::SubjektivHost>,
    ) -> Self {
        self.subjektiv_host = Some(host);
        self
    }

    /// Construct the Worker without exposing a controller handle. Runtime hosts
    /// use this seam to bind Workdir, observation, Flow, and other live services
    /// before [`PreparedWorker::start`] performs Feature installation.
    pub async fn prepare(
        self,
    ) -> Result<PreparedWorker<Box<dyn LlmClient>, St>, WorkerBootstrapError> {
        crate::feature::authorize_requested_manifest(
            self.workspace_context.client_handle().as_ref(),
            &self.manifest,
        )
        .map_err(|error| {
            WorkerBootstrapError::Worker(WorkerError::FeatureLifecycle(error.to_string()))
        })?;
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
            worker.bind_single_workdir_session(Some(workdir_session));
        }
        let explicit_subjektiv_host = self.subjektiv_host.is_some();
        worker.bind_subjektiv_host(self.subjektiv_host);
        if explicit_subjektiv_host
            && worker.manifest().feature.subjektiv.profile.enabled
            && !worker
                .manifest()
                .feature
                .subjektiv
                .profile
                .consolidation_tools
        {
            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    crate::SubjektivSessionAttributionLifecycle::NewSession,
                )
                .map_err(WorkerBootstrapError::Worker)?;
        }
        Ok(PreparedWorker::new(worker, self.layout))
    }

    /// Reconstruct a saved materialized Worker without allocation, attribution
    /// repair, Feature installation, or controller exposure. The caller owns the
    /// later controller layout and must activate before starting it.
    pub async fn prepare_restored_candidate(
        self,
        worker_name: &str,
    ) -> Result<PreparedRestoredWorker<St>, WorkerBootstrapError> {
        let mut candidate =
            Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
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
            candidate
                .worker_mut()
                .map_err(WorkerBootstrapError::Worker)?
                .bind_single_workdir_session(Some(workdir_session));
        }
        candidate
            .worker_mut()
            .map_err(WorkerBootstrapError::Worker)?
            .bind_subjektiv_host(self.subjektiv_host);
        Ok(candidate)
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
            worker.bind_single_workdir_session(Some(workdir_session));
        }
        let explicit_subjektiv_host = self.subjektiv_host.is_some();
        worker.bind_subjektiv_host(self.subjektiv_host);
        if explicit_subjektiv_host
            && worker.manifest().feature.subjektiv.profile.enabled
            && !worker
                .manifest()
                .feature
                .subjektiv
                .profile
                .consolidation_tools
        {
            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    crate::SubjektivSessionAttributionLifecycle::RestoredSession,
                )
                .map_err(WorkerBootstrapError::Worker)?;
        }
        Ok(PreparedWorker::new(worker, self.layout))
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
    pub fn new(worker: Worker<C, St>, layout: WorkerBootstrapLayout) -> Self {
        Self { worker, layout }
    }

    pub fn worker(&self) -> &Worker<C, St> {
        &self.worker
    }

    pub fn worker_mut(&mut self) -> &mut Worker<C, St> {
        &mut self.worker
    }

    pub async fn start(self) -> Result<BootstrappedWorker, WorkerBootstrapError> {
        start_worker_controller(self.worker, self.layout).await
    }
}

/// Start the shared direct/runtime-managed controller lifecycle for an already
/// constructed Worker. Restore paths use this after replaying durable state;
/// fresh hosts normally use [`WorkerBootstrap::start`].
pub async fn start_worker_controller<C, St>(
    worker: Worker<C, St>,
    layout: WorkerBootstrapLayout,
) -> Result<BootstrappedWorker, WorkerBootstrapError>
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    let cleanup_sessions = worker.workdir_sessions();
    let factory_owned_cleanup = worker.startup_feature_cleanup_handle().is_some();
    let controller = match layout {
        WorkerBootstrapLayout::Direct { runtime_base } => {
            WorkerController::spawn_owned(worker, &runtime_base).await
        }
        WorkerBootstrapLayout::RuntimeManagedRun { run_dir } => {
            WorkerController::spawn_runtime_managed_run_owned(worker, &run_dir).await
        }
    };

    match controller {
        Ok((handle, shutdown, controller_task)) => Ok(BootstrappedWorker {
            handle,
            shutdown,
            controller_task,
        }),
        Err(source) => {
            // Factory-owned task scopes must settle before sessions close.
            // Cleanup remains unconfirmed here; preserve the startup source.
            let cleanup_failed =
                factory_owned_cleanup || cleanup_sessions.close_all().await.is_err();
            Err(WorkerBootstrapError::Controller {
                source,
                cleanup_failed,
            })
        }
    }
}

/// Host bindings for one unpublished restored Controller. These do not expose
/// Worker execution or permit arbitrary mutation during preparation.
pub struct RestoredControllerBindings {
    pub backend_job: Option<crate::BackendJobExecutionBinding>,
    pub workdir_sessions: std::sync::Arc<workdir::WorkdirSessionRouter>,
    pub observation_provider:
        Option<std::sync::Arc<dyn crate::feature::contracts::WorkerObservationProvider>>,
    pub subjektiv_attached: bool,
}

/// The same real receiver, snapshot sources and paired subscription retained
/// privately across the Runtime's durable restore decision.
pub struct UnpublishedRestoredController<St: Store> {
    initialized: Option<crate::controller::InitializedWorkerController<Box<dyn LlmClient>, St>>,
    activation: Option<crate::worker::RestoredWorkerActivation>,
    streams: Option<crate::ipc::protocol_session::WorkerProtocolSessionStreams>,
    subjektiv_attached: bool,
    effects_applied: bool,
    activation_failed: bool,
    closing: bool,
}

pub struct ActivatedRestoredController {
    pub controller: BootstrappedWorker,
    pub streams: crate::ipc::protocol_session::WorkerProtocolSessionStreams,
}

impl<St> PreparedRestoredWorker<St>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    pub fn workspace_client_handle(
        &mut self,
    ) -> Result<std::sync::Arc<dyn crate::WorkspaceClient>, WorkerError> {
        Ok(self.worker_mut()?.workspace_client_handle())
    }

    pub fn retain_startup_feature_cleanup(
        &mut self,
    ) -> Result<crate::WorkerStartupFeatureCleanup, WorkerError> {
        Ok(self.worker_mut()?.retain_startup_feature_cleanup())
    }

    /// Initialize the real Controller without allocation, canonical restore
    /// writes, attribution changes, registry reclamation or input consumption.
    /// MCP discovery and Feature tasks belong to the retained startup cleanup.
    pub async fn prepare_runtime_controller(
        mut self,
        run_dir: &std::path::Path,
        bindings: RestoredControllerBindings,
    ) -> Result<UnpublishedRestoredController<St>, WorkerBootstrapError> {
        let worker = self.worker_mut().map_err(WorkerBootstrapError::Worker)?;
        if let Some(binding) = bindings.backend_job {
            worker.bind_backend_job(binding).map_err(|error| {
                WorkerBootstrapError::Worker(WorkerError::FeatureLifecycle(error.into()))
            })?;
        }
        worker
            .workdir_sessions()
            .close_all()
            .await
            .map_err(|error| {
                WorkerBootstrapError::Worker(WorkerError::FeatureLifecycle(error.to_string()))
            })?;
        worker.bind_workdir_sessions(bindings.workdir_sessions);
        worker.bind_worker_observation_provider(bindings.observation_provider);
        let flow_enabled = worker.manifest().feature.flow.enabled;
        if flow_enabled {
            let report = worker
                .install_runtime_flow_transition_feature()
                .map_err(|error| {
                    WorkerBootstrapError::Worker(WorkerError::FeatureLifecycle(error))
                })?;
            if report.reports.iter().any(|report| !report.installed) {
                return Err(WorkerBootstrapError::Worker(WorkerError::FeatureLifecycle(
                    "Flow transition installation refused".into(),
                )));
            }
        }
        let (worker, activation) = self
            .into_controller_parts()
            .map_err(WorkerBootstrapError::Worker)?;
        let initialized = WorkerController::initialize_restored(worker, run_dir)
            .await
            .map_err(|source| WorkerBootstrapError::Controller {
                source,
                cleanup_failed: true,
            })?;
        if flow_enabled {
            initialized.handle.shared_state.enable_flow_transition();
        }
        let streams = {
            let _guard = initialized.handle.protocol_snapshot_publish_guard();
            crate::ipc::protocol_session::subscribe_worker_protocol_session(&initialized.handle)
        };
        Ok(UnpublishedRestoredController {
            initialized: Some(initialized),
            activation: Some(activation),
            streams: Some(streams),
            subjektiv_attached: bindings.subjektiv_attached,
            effects_applied: false,
            activation_failed: false,
            closing: false,
        })
    }
}

impl<St> UnpublishedRestoredController<St>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    pub fn snapshot(&self) -> &protocol::Event {
        &self
            .streams
            .as_ref()
            .expect("unpublished restore streams")
            .snapshot_event
    }

    /// Only publication failures after confirmed effects can retry on this
    /// exact owner. Partial activation must be reconciled, never blindly rerun.
    pub fn can_retry_publication(&self) -> bool {
        self.initialized.is_some()
            && self.effects_applied
            && !self.activation_failed
            && !self.closing
    }

    /// Apply canonical restore effects after the host's durable decision, while
    /// retaining the same private Controller for subsequent publication.
    pub async fn apply_effects(&mut self) -> Result<(), WorkerBootstrapError> {
        let initialized = self
            .initialized
            .as_mut()
            .ok_or_else(|| WorkerBootstrapError::Worker(WorkerError::RestoreCandidateClosing))?;
        if self.activation_failed || self.closing {
            return Err(WorkerBootstrapError::Worker(
                WorkerError::RestoreCandidateClosing,
            ));
        }
        // Preparation's read-only authorization is not a publication grant.
        // Revalidate current Host authority after commit, before effects/input,
        // including a publication retry on this same private Controller.
        initialized
            .worker
            .validate_subjektiv_body_publication()
            .map_err(WorkerBootstrapError::Worker)?;
        if !self.effects_applied {
            self.activation_failed = true;
            self.activation
                .as_ref()
                .expect("restore activation")
                .activate(&mut initialized.worker)
                .await
                .map_err(WorkerBootstrapError::Worker)?;
            initialized
                .worker
                .finalize_subjektiv_session_attribution(
                    self.subjektiv_attached,
                    crate::SubjektivSessionAttributionLifecycle::RestoredSession,
                )
                .map_err(WorkerBootstrapError::Worker)?;
            self.effects_applied = true;
            self.activation_failed = false;
            self.activation = None;
        }
        Ok(())
    }

    pub fn allocation_cleanup(
        &self,
    ) -> Option<crate::runtime::worker_allocation::ScopeAllocationCleanup> {
        self.initialized
            .as_ref()
            .and_then(|initialized| initialized.worker.restore_allocation_cleanup_handle())
    }

    pub async fn activate(&mut self) -> Result<ActivatedRestoredController, WorkerBootstrapError> {
        self.apply_effects().await?;
        let initialized = self
            .initialized
            .as_mut()
            .expect("same initialized Controller");
        let publication = initialized.prepare_publication().await.map_err(|source| {
            WorkerBootstrapError::Controller {
                source,
                cleanup_failed: true,
            }
        })?;
        let initialized = self
            .initialized
            .take()
            .expect("same initialized Controller");
        let (handle, shutdown, controller_task) = initialized.start(publication);
        Ok(ActivatedRestoredController {
            controller: BootstrappedWorker {
                handle,
                shutdown,
                controller_task,
            },
            streams: self.streams.take().expect("same paired subscription"),
        })
    }

    pub async fn discard(&mut self) -> Result<(), WorkerError> {
        self.closing = true;
        if let Some(initialized) = self.initialized.as_mut() {
            initialized
                .worker
                .retain_startup_feature_cleanup()
                .shutdown()
                .await
                .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
            initialized
                .worker
                .workdir_sessions()
                .close_all()
                .await
                .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
        }
        self.streams = None;
        self.initialized = None;
        Ok(())
    }
}
