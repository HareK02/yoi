use std::path::PathBuf;
use std::time::Duration;

use agen::llm_client::client::LlmClient;
use client::Client;
use client::transport::in_process::{Peer as InProcessPeer, Socket as InProcessSocket};
use protocol::stream::{decode_method, encode_event};
use protocol::{Event, Method, WorkerId};
use session_store::{
    CombinedStore, FsStore, FsWorkerStore, WorkerActiveSegmentRef, WorkerMetadataStore,
};
use thiserror::Error;
use worker::bootstrap::{WorkerBootstrap, WorkerBootstrapError, WorkerBootstrapLayout};
use worker::controller::WorkerControllerTransport;
use worker::ipc::protocol_session::{
    WorkerProtocolSessionStreams, dispatch_worker_protocol_method, live_log_entry_event,
    subscribe_worker_protocol_session,
};
use worker::{BootstrappedWorker, WorkerError, WorkerFilesystemAuthority, WorkerWorkspaceContext};

use crate::launch::ResolvedStandaloneLaunch;
use crate::store::{
    StaleLeasePolicy, StandaloneShutdownReason, StandaloneStoreError, StandaloneWorkerLease,
    StandaloneWorkerRecord, StandaloneWorkerStore,
};

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
type StandaloneBackingStore = CombinedStore<FsStore, FsWorkerStore>;

/// One client-owned top-level Worker and its standalone Worker authority.
///
/// The host deliberately exposes the existing typed Worker protocol rather than owning an
/// HTTP/WebSocket server or creating Runtime/Workspace/Ticket/Workdir domain records.
pub struct StandaloneHost {
    handle: worker::WorkerHandle,
    shutdown: Option<worker::controller::ShutdownReceiver>,
    shutdown_timeout: Duration,
    store: StandaloneWorkerStore,
    worker_store: FsWorkerStore,
    record: StandaloneWorkerRecord,
    lease: Option<StandaloneWorkerLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneStartupError {
    #[error("the standalone state store could not be opened or validated")]
    StateStore,
    #[error("the standalone Worker is already active")]
    WorkerActive,
    #[error("the standalone Worker lease cannot be observed safely; recovery is rejected")]
    LeaseLivenessUnknown,
    #[error("the standalone Worker working directory is unavailable or changed")]
    WorkingDirectoryUnavailable,
    #[error("the resolved Worker configuration or persisted history is invalid")]
    WorkerConfiguration,
    #[error("the configured model provider is unavailable")]
    ModelProvider,
    #[error("the fixed standalone feature composition could not be installed")]
    FeatureComposition,
    #[error("the in-process Worker controller could not start")]
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneShutdownError {
    #[error("the standalone Worker did not stop before the shutdown deadline")]
    DeadlineExceeded,
    #[error("the standalone Worker shutdown confirmation was lost")]
    ConfirmationLost,
    #[error("the standalone Worker final state could not be committed")]
    StateStore,
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
        let store =
            StandaloneWorkerStore::open(&launch.state_dir).map_err(classify_store_startup_error)?;
        let allocation = store
            .allocate(&launch.cwd, StaleLeasePolicy::Reject)
            .map_err(classify_store_startup_error)?;
        let worker_id = allocation.worker_id();

        // WorkerId is the stable identity. The current Worker store remains
        // name-keyed, so keep its derived storage key separate from the
        // user-facing profile name.
        let manifest = launch.profile.manifest.clone();
        let storage_key = format!("standalone-{worker_id}");
        let mut bootstrap_manifest = manifest.clone();
        bootstrap_manifest.worker.name = storage_key.clone();
        let (backing_store, worker_store) = match backing_store(&store, worker_id) {
            Ok(stores) => stores,
            Err(error) => {
                let _ = store.abandon_allocation(allocation);
                return Err(error);
            }
        };
        let filesystem_authority =
            WorkerFilesystemAuthority::local(launch.cwd.clone(), launch.cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        let runtime_base = store.runtime_dir(worker_id);

        let mut bootstrap = WorkerBootstrap::new(
            bootstrap_manifest,
            backing_store,
            launch.prompt_catalog,
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::Direct { runtime_base },
            WorkerControllerTransport::InProcess,
        );
        if let Some(model_client) = model_client {
            bootstrap = bootstrap.with_model_client(model_client);
        }
        let started = match bootstrap.start().await {
            Ok(started) => started,
            Err(error) => {
                let _ = store.abandon_allocation(allocation);
                return Err(classify_startup_error(error));
            }
        };
        let active = match active_pointer(&worker_store, &storage_key) {
            Ok(active) => active,
            Err(error) => {
                stop_started_worker(started).await;
                let _ = store.abandon_allocation(allocation);
                return Err(error);
            }
        };
        let record = match store.commit_created(
            &allocation,
            manifest,
            storage_key,
            active.session_id,
            active.segment_id,
        ) {
            Ok(record) => record,
            Err(_) => {
                stop_started_worker(started).await;
                let _ = store.abandon_allocation(allocation);
                return Err(StandaloneStartupError::StateStore);
            }
        };
        Ok(Self::from_started(
            started,
            store,
            worker_store,
            record,
            allocation.into_lease(),
        ))
    }

    pub async fn restore(
        state_dir: PathBuf,
        worker_id: WorkerId,
    ) -> Result<Self, StandaloneStartupError> {
        Self::restore_with_optional_model_client(state_dir, worker_id, None).await
    }

    pub async fn restore_with_model_client<C>(
        state_dir: PathBuf,
        worker_id: WorkerId,
        model_client: C,
    ) -> Result<Self, StandaloneStartupError>
    where
        C: LlmClient + 'static,
    {
        Self::restore_with_optional_model_client(state_dir, worker_id, Some(Box::new(model_client)))
            .await
    }

    async fn restore_with_optional_model_client(
        state_dir: PathBuf,
        worker_id: WorkerId,
        model_client: Option<Box<dyn LlmClient>>,
    ) -> Result<Self, StandaloneStartupError> {
        let store = StandaloneWorkerStore::open(state_dir).map_err(classify_store_startup_error)?;
        let record = store
            .load(worker_id)
            .map_err(classify_store_startup_error)?;
        record.cwd.verify().map_err(classify_store_startup_error)?;
        let lease = store
            .acquire_lease(worker_id, StaleLeasePolicy::Recover)
            .map_err(classify_store_startup_error)?;
        let (backing_store, worker_store) = backing_store(&store, worker_id)?;
        let storage_key = record.storage_key.clone();
        let mut manifest = record.manifest.clone();
        manifest.worker.name = storage_key.clone();
        let filesystem_authority = WorkerFilesystemAuthority::local(
            record.cwd.canonical_path.clone(),
            record.cwd.canonical_path.clone(),
        );
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        let runtime_base = store.runtime_dir(worker_id);

        let mut bootstrap = WorkerBootstrap::new(
            manifest,
            backing_store,
            worker::PromptCatalogSource::builtins_only(),
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::Direct { runtime_base },
            WorkerControllerTransport::InProcess,
        );
        if let Some(model_client) = model_client {
            bootstrap = bootstrap.with_model_client(model_client);
        }
        let prepared = bootstrap
            .prepare_restored(&storage_key)
            .await
            .map_err(classify_startup_error)?;
        let started = prepared.start().await.map_err(classify_startup_error)?;
        let active = match active_pointer(&worker_store, &storage_key) {
            Ok(active) => active,
            Err(error) => {
                stop_started_worker(started).await;
                return Err(error);
            }
        };
        let record =
            match store.update_active_pointer(&record, active.session_id, active.segment_id) {
                Ok(record) => record,
                Err(_) => {
                    stop_started_worker(started).await;
                    lease.retain();
                    return Err(StandaloneStartupError::StateStore);
                }
            };
        Ok(Self::from_started(
            started,
            store,
            worker_store,
            record,
            lease,
        ))
    }

    fn from_started(
        started: BootstrappedWorker,
        store: StandaloneWorkerStore,
        worker_store: FsWorkerStore,
        record: StandaloneWorkerRecord,
        lease: StandaloneWorkerLease,
    ) -> Self {
        Self {
            handle: started.handle,
            shutdown: Some(started.shutdown),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            store,
            worker_store,
            record,
            lease: Some(lease),
        }
    }

    #[must_use]
    pub fn worker_id(&self) -> WorkerId {
        self.record.worker_id
    }

    #[must_use]
    pub fn record(&self) -> &StandaloneWorkerRecord {
        &self.record
    }

    /// Open one complete client-side Worker protocol session.
    ///
    /// Working events, committed session entries, alert snapshots, and the
    /// initial history snapshot are merged behind the client boundary.
    pub fn connect(&self) -> Client<InProcessSocket> {
        let streams = subscribe_worker_protocol_session(&self.handle);
        let (socket, peer) = InProcessSocket::pair();
        tokio::spawn(run_protocol_session(self.handle.clone(), streams, peer));
        Client::new(socket)
    }

    pub fn with_shutdown_timeout(mut self, shutdown_timeout: Duration) -> Self {
        self.shutdown_timeout = shutdown_timeout;
        self
    }

    pub async fn shutdown(mut self) -> Result<(), StandaloneShutdownError> {
        let _ = self.handle.send(Method::Shutdown).await;
        let Some(shutdown) = self.shutdown.take() else {
            self.retain_lease();
            return Err(StandaloneShutdownError::ConfirmationLost);
        };
        match tokio::time::timeout(self.shutdown_timeout, shutdown).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                self.retain_lease();
                return Err(StandaloneShutdownError::ConfirmationLost);
            }
            Err(_) => {
                self.retain_lease();
                return Err(StandaloneShutdownError::DeadlineExceeded);
            }
        }
        let active = match active_pointer(&self.worker_store, &self.record.storage_key) {
            Ok(active) => active,
            Err(_) => {
                self.retain_lease();
                return Err(StandaloneShutdownError::StateStore);
            }
        };
        if self
            .store
            .mark_stopped(
                &self.record,
                active.session_id,
                active.segment_id,
                StandaloneShutdownReason::UserExit,
            )
            .is_err()
        {
            self.retain_lease();
            return Err(StandaloneShutdownError::StateStore);
        }
        if let Some(lease) = self.lease.take() {
            lease
                .release()
                .map_err(|_| StandaloneShutdownError::StateStore)?;
        }
        Ok(())
    }

    fn retain_lease(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.retain();
        }
    }
}

async fn run_protocol_session(
    handle: worker::WorkerHandle,
    streams: WorkerProtocolSessionStreams,
    mut peer: InProcessPeer,
) {
    let WorkerProtocolSessionStreams {
        snapshot_event,
        mut log_entries,
        alert_snapshot,
        mut events,
    } = streams;

    if !send_protocol_snapshot(&peer, alert_snapshot, snapshot_event).await {
        return;
    }

    loop {
        tokio::select! {
            message = peer.next() => {
                let Some(message) = message else {
                    return;
                };
                let Ok(method) = decode_method(&message) else {
                    return;
                };
                if let Some(event) = dispatch_worker_protocol_method(&handle, method).await
                    && !send_protocol_event(&peer, event).await
                {
                    return;
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if !send_protocol_event(&peer, event).await {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let replacement = subscribe_worker_protocol_session(&handle);
                        let WorkerProtocolSessionStreams {
                            snapshot_event,
                            log_entries: replacement_log_entries,
                            alert_snapshot,
                            events: replacement_events,
                        } = replacement;
                        log_entries = replacement_log_entries;
                        events = replacement_events;
                        if !send_protocol_snapshot(&peer, alert_snapshot, snapshot_event).await {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
            entry = log_entries.recv() => {
                match entry {
                    Ok(entry) => {
                        if let Some(event) = live_log_entry_event(entry)
                            && !send_protocol_event(&peer, event).await
                        {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let replacement = subscribe_worker_protocol_session(&handle);
                        let WorkerProtocolSessionStreams {
                            snapshot_event,
                            log_entries: replacement_log_entries,
                            alert_snapshot,
                            events: replacement_events,
                        } = replacement;
                        log_entries = replacement_log_entries;
                        events = replacement_events;
                        if !send_protocol_snapshot(&peer, alert_snapshot, snapshot_event).await {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

async fn send_protocol_snapshot(
    peer: &InProcessPeer,
    alert_snapshot: Vec<protocol::Alert>,
    snapshot_event: Event,
) -> bool {
    for alert in alert_snapshot {
        if !send_protocol_event(peer, Event::Alert(alert)).await {
            return false;
        }
    }
    send_protocol_event(peer, snapshot_event).await
}

async fn send_protocol_event(peer: &InProcessPeer, event: Event) -> bool {
    let Ok(message) = encode_event(&event) else {
        return false;
    };
    peer.send(message).await.is_ok()
}

fn backing_store(
    store: &StandaloneWorkerStore,
    worker_id: WorkerId,
) -> Result<(StandaloneBackingStore, FsWorkerStore), StandaloneStartupError> {
    let session_store = FsStore::new(store.sessions_dir(worker_id))
        .map_err(|_| StandaloneStartupError::StateStore)?;
    let worker_store = FsWorkerStore::new(store.worker_metadata_dir(worker_id))
        .map_err(|_| StandaloneStartupError::StateStore)?;
    Ok((
        CombinedStore::new(session_store, worker_store.clone()),
        worker_store,
    ))
}

fn active_pointer(
    worker_store: &FsWorkerStore,
    storage_key: &str,
) -> Result<WorkerActiveSegmentRef, StandaloneStartupError> {
    worker_store
        .read_by_name(storage_key)
        .map_err(|_| StandaloneStartupError::StateStore)?
        .and_then(|metadata| metadata.active)
        .ok_or(StandaloneStartupError::StateStore)
}

async fn stop_started_worker(started: BootstrappedWorker) {
    let _ = started.handle.send(Method::Shutdown).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), started.shutdown).await;
}

fn classify_store_startup_error(error: StandaloneStoreError) -> StandaloneStartupError {
    match error {
        StandaloneStoreError::WorkerLeased(_) => StandaloneStartupError::WorkerActive,
        StandaloneStoreError::LeaseLivenessUnknown(_) => {
            StandaloneStartupError::LeaseLivenessUnknown
        }
        StandaloneStoreError::CwdUnavailable(_)
        | StandaloneStoreError::CwdNotDirectory
        | StandaloneStoreError::CwdIdentityMismatch => {
            StandaloneStartupError::WorkingDirectoryUnavailable
        }
        _ => StandaloneStartupError::StateStore,
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
