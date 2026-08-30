use std::time::Duration;

use agen::llm_client::client::LlmClient;
use protocol::{Event, Method};
use session_store::{CombinedStore, FsStore, FsWorkerStore};
use thiserror::Error;
use tokio::sync::broadcast;
use worker::bootstrap::{WorkerBootstrap, WorkerBootstrapError, WorkerBootstrapLayout};
use worker::controller::WorkerControllerTransport;
use worker::{WorkerError, WorkerFilesystemAuthority, WorkerWorkspaceContext};

use crate::launch::ResolvedStandaloneLaunch;

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// One top-level Worker plus its process-owned local Workdir session.
///
/// The host deliberately exposes the existing typed Worker protocol rather
/// than owning an HTTP/WebSocket server or a second execution model.
pub struct StandaloneHost {
    handle: worker::WorkerHandle,
    shutdown: worker::controller::ShutdownReceiver,
    shutdown_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneStartupError {
    #[error("the standalone state store could not be opened")]
    StateStore,
    #[error("the resolved Worker configuration is invalid")]
    WorkerConfiguration,
    #[error("the configured model provider is unavailable")]
    ModelProvider,
    #[error("the fixed standalone feature composition could not be installed")]
    FeatureComposition,
    #[error("the in-process Worker controller could not start")]
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneRequestError {
    #[error("the standalone Worker is no longer accepting requests")]
    WorkerUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneShutdownError {
    #[error("the standalone Worker did not stop before the shutdown deadline")]
    DeadlineExceeded,
    #[error("the standalone Worker shutdown confirmation was lost")]
    ConfirmationLost,
}

impl StandaloneHost {
    pub async fn start(launch: ResolvedStandaloneLaunch) -> Result<Self, StandaloneStartupError> {
        Self::start_with_optional_model_client(launch, None).await
    }

    pub async fn start_with_model_client<C>(
        launch: ResolvedStandaloneLaunch,
        model_client: C,
    ) -> Result<Self, StandaloneStartupError>
    where
        C: LlmClient + 'static,
    {
        Self::start_with_optional_model_client(launch, Some(Box::new(model_client))).await
    }

    async fn start_with_optional_model_client(
        launch: ResolvedStandaloneLaunch,
        model_client: Option<Box<dyn LlmClient>>,
    ) -> Result<Self, StandaloneStartupError> {
        std::fs::create_dir_all(&launch.state_dir)
            .map_err(|_| StandaloneStartupError::StateStore)?;
        let session_store = FsStore::new(launch.state_dir.join("sessions"))
            .map_err(|_| StandaloneStartupError::StateStore)?;
        let worker_store = FsWorkerStore::new(launch.state_dir.join("workers"))
            .map_err(|_| StandaloneStartupError::StateStore)?;
        let store = CombinedStore::new(session_store, worker_store);
        let filesystem_authority =
            WorkerFilesystemAuthority::local(launch.cwd.clone(), launch.cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        let runtime_base = launch.state_dir.join("runtime");

        let mut bootstrap = WorkerBootstrap::new(
            launch.profile.manifest,
            store,
            launch.prompt_catalog,
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::Direct { runtime_base },
            WorkerControllerTransport::InProcess,
        );
        if let Some(model_client) = model_client {
            bootstrap = bootstrap.with_model_client(model_client);
        }
        let started = bootstrap.start().await.map_err(classify_startup_error)?;

        Ok(Self {
            handle: started.handle,
            shutdown: started.shutdown,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        })
    }

    pub async fn send(&self, method: Method) -> Result<(), StandaloneRequestError> {
        self.handle
            .send(method)
            .await
            .map_err(|_| StandaloneRequestError::WorkerUnavailable)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.handle.subscribe()
    }

    pub fn snapshot(&self) -> Event {
        self.handle.snapshot_event()
    }

    pub fn with_shutdown_timeout(mut self, shutdown_timeout: Duration) -> Self {
        self.shutdown_timeout = shutdown_timeout;
        self
    }

    pub async fn shutdown(self) -> Result<(), StandaloneShutdownError> {
        let StandaloneHost {
            handle,
            shutdown,
            shutdown_timeout,
        } = self;
        let _ = handle.send(Method::Shutdown).await;
        match tokio::time::timeout(shutdown_timeout, shutdown).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(StandaloneShutdownError::ConfirmationLost),
            Err(_) => Err(StandaloneShutdownError::DeadlineExceeded),
        }
    }
}

fn classify_startup_error(error: WorkerBootstrapError) -> StandaloneStartupError {
    match error {
        WorkerBootstrapError::Worker(WorkerError::Provider(_)) => {
            StandaloneStartupError::ModelProvider
        }
        WorkerBootstrapError::Worker(_) => StandaloneStartupError::WorkerConfiguration,
        WorkerBootstrapError::Controller { source, .. }
            if source.kind() == std::io::ErrorKind::Other =>
        {
            StandaloneStartupError::FeatureComposition
        }
        WorkerBootstrapError::Controller { .. } => StandaloneStartupError::Controller,
    }
}
