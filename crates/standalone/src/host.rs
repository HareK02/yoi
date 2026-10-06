use std::path::PathBuf;
use std::time::Duration;

use agen::llm_client::client::LlmClient;
use client::Client;
use client::transport::in_process::{Peer as InProcessPeer, Socket as InProcessSocket};
use manifest::ScopeRule;
use protocol::stream::{decode_method, encode_event};
use protocol::{Event, Method, WorkerId};
use session_store::{
    CombinedStore, FsStore, FsWorkerStore, WorkerActiveSegmentRef, WorkerMetadataStore,
};
use thiserror::Error;
use worker::bootstrap::{
    WorkerBootstrap, WorkerBootstrapError, WorkerBootstrapLayout, bash_output_dir_for_worker_id,
};
use worker::controller::WorkerControllerTransport;
use worker::ipc::protocol_session::{
    WorkerProtocolSessionStreams, dispatch_worker_protocol_method, live_log_entry_event,
    subscribe_worker_protocol_session,
};
use worker::runtime::worker_allocation::ScopeLockError;
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
    jobs: crate::jobs::StandaloneJobs,
    subject: Option<crate::subjektiv::SubjectConnection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StandaloneStartupError {
    #[error("the standalone state store could not be opened or validated")]
    StateStore,
    #[error("local Subject connection rejected: {0}")]
    Subject(String),
    #[error("the standalone Worker is already active")]
    WorkerActive,
    #[error("the standalone Worker lease cannot be observed safely; recovery is rejected")]
    LeaseLivenessUnknown,
    #[error("the standalone Worker working directory is unavailable or changed")]
    WorkingDirectoryUnavailable,
    #[error(
        "requested scope `{}` conflicts with worker allocation `{competitor}` rule `{}`",
        requested_rule.target.display(),
        competitor_rule.target.display()
    )]
    ScopeConflict {
        competitor: String,
        requested_rule: ScopeRule,
        competitor_rule: ScopeRule,
    },
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
    #[error("standalone Job execution cleanup or terminal persistence could not be confirmed")]
    JobCleanup,
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
        if launch.subject_id.is_some()
            && (!launch.profile.manifest.feature.subjektiv.profile.enabled
                || launch
                    .profile
                    .manifest
                    .feature
                    .subjektiv
                    .profile
                    .consolidation_tools
                || launch
                    .profile
                    .manifest
                    .feature
                    .subjektiv
                    .workspace_settings
                    .is_some())
        {
            return Err(StandaloneStartupError::Subject(
                "incompatible local Subject policy".into(),
            ));
        }
        let store =
            StandaloneWorkerStore::open(&launch.state_dir).map_err(classify_store_startup_error)?;
        let allocation = store
            .allocate(&launch.cwd, StaleLeasePolicy::Reject)
            .map_err(classify_store_startup_error)?;
        let worker_id = allocation.worker_id();
        let mut subject = match launch
            .subject_id
            .as_deref()
            .map(|id| {
                crate::subjektiv::StandaloneSubjects::open_existing(&launch.state_dir)?
                    .connect(id, worker_id)
            })
            .transpose()
        {
            Ok(subject) => subject,
            Err(error) => {
                let _ = store.abandon_allocation(allocation);
                return Err(StandaloneStartupError::Subject(error.to_string()));
            }
        };
        let jobs_path = subject
            .as_ref()
            .map(|s| s.jobs_path.clone())
            .unwrap_or_else(|| store.jobs_path(worker_id));
        let jobs = match crate::jobs::StandaloneJobs::open(&jobs_path, launch.cwd.clone()) {
            Ok(jobs) => jobs,
            Err(_) => {
                let _ = store.abandon_allocation(allocation);
                return Err(StandaloneStartupError::StateStore);
            }
        };

        if let Some(client) = &model_client {
            jobs.bind_model_client(client.clone_boxed());
        }
        if let Some(subject) = &subject {
            subject.host.bind_jobs(jobs.clone())?;
        }
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
        let bash_output_dir = bash_output_dir_for_worker_id(worker_id);

        let mut bootstrap = WorkerBootstrap::new(
            bootstrap_manifest,
            backing_store,
            launch.prompt_catalog,
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::Direct {
                runtime_base,
                bash_output_dir,
            },
            WorkerControllerTransport::InProcess,
        );
        if let Some(subject) = &subject {
            bootstrap = bootstrap.with_subjektiv_host(subject.host.clone());
        }
        if let Some(model_client) = model_client {
            bootstrap = bootstrap.with_model_client(model_client);
        }
        if let Some(subject) = &mut subject {
            subject.arm();
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
        let record = match store.commit_created_connected(
            &allocation,
            manifest,
            storage_key,
            active.session_id,
            active.segment_id,
            subject.as_ref().map(|s| s.binding.clone()),
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
            jobs,
            subject,
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
        let mut subject = record
            .subject
            .as_ref()
            .map(|binding| {
                if !record.manifest.feature.subjektiv.profile.enabled
                    || record
                        .manifest
                        .feature
                        .subjektiv
                        .profile
                        .consolidation_tools
                    || record
                        .manifest
                        .feature
                        .subjektiv
                        .workspace_settings
                        .is_some()
                {
                    return Err(crate::subjektiv::SubjectError::InvalidScope(
                        "persisted Subject policy changed".into(),
                    ));
                }
                let catalog = crate::subjektiv::StandaloneSubjects::open_existing(store.root())?;
                if catalog.scope_id() != binding.scope_id {
                    return Err(crate::subjektiv::SubjectError::InvalidScope(
                        "persisted local storage scope changed".into(),
                    ));
                }
                catalog.connect(&binding.subject_id, worker_id)
            })
            .transpose()
            .map_err(|e| StandaloneStartupError::Subject(e.to_string()))?;
        let jobs_path = subject
            .as_ref()
            .map(|s| s.jobs_path.clone())
            .unwrap_or_else(|| store.jobs_path(worker_id));
        let jobs = crate::jobs::StandaloneJobs::open(&jobs_path, record.cwd.canonical_path.clone())
            .map_err(|_| StandaloneStartupError::StateStore)?;
        if let Some(client) = &model_client {
            jobs.bind_model_client(client.clone_boxed());
        }
        if let Some(subject) = &subject {
            subject.host.bind_jobs(jobs.clone())?;
        }
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
        let bash_output_dir = bash_output_dir_for_worker_id(worker_id);

        let mut bootstrap = WorkerBootstrap::new(
            manifest,
            backing_store,
            worker::PromptCatalogSource::builtins_only(),
            workspace_context,
            filesystem_authority,
            WorkerBootstrapLayout::Direct {
                runtime_base,
                bash_output_dir,
            },
            WorkerControllerTransport::InProcess,
        );
        if let Some(subject) = &subject {
            bootstrap = bootstrap.with_subjektiv_host(subject.host.clone());
        }
        if let Some(model_client) = model_client {
            bootstrap = bootstrap.with_model_client(model_client);
        }
        let prepared = bootstrap
            .prepare_restored(&storage_key)
            .await
            .map_err(classify_startup_error)?;
        if let Some(subject) = &mut subject {
            subject.arm();
        }
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
            jobs,
            subject,
        ))
    }

    fn from_started(
        started: BootstrappedWorker,
        store: StandaloneWorkerStore,
        worker_store: FsWorkerStore,
        record: StandaloneWorkerRecord,
        lease: StandaloneWorkerLease,
        jobs: crate::jobs::StandaloneJobs,
        subject: Option<crate::subjektiv::SubjectConnection>,
    ) -> Self {
        if let Some(connection) = &subject {
            connection.host.recover_jobs();
        }
        Self {
            handle: started.handle,
            shutdown: Some(started.shutdown),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            store,
            worker_store,
            record,
            lease: Some(lease),
            jobs,
            subject,
        }
    }

    /// Explicit Host execution capability for local Features, separate from the
    /// interactive Worker protocol and its model-created SubWorkers.
    pub fn jobs(&self) -> crate::jobs::StandaloneJobs {
        self.jobs.clone()
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
        let jobs_result = self.jobs.shutdown().await;
        let command = protocol::WorkerCommandEnvelope::new(u64::MAX);
        let _ = self.handle.send(Method::Shutdown { command }).await;
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
        if jobs_result.is_err() {
            self.retain_lease();
            return Err(StandaloneShutdownError::JobCleanup);
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
        if let Some(subject) = &mut self.subject {
            if subject.close().is_err() {
                self.retain_lease();
                return Err(StandaloneShutdownError::StateStore);
            }
        }
        if let Some(lease) = self.lease.take() {
            lease
                .release()
                .map_err(|_| StandaloneShutdownError::StateStore)?;
        }
        Ok(())
    }

    fn retain_lease(&mut self) {
        if let Some(subject) = &mut self.subject {
            subject.retain();
        }
        if let Some(lease) = self.lease.take() {
            lease.retain();
        }
    }
}

impl Drop for StandaloneHost {
    fn drop(&mut self) {
        self.jobs.close();
        // A dropped Host cannot synchronously confirm either controller or Job
        // cleanup. Keep its lease until explicit shutdown or process recovery,
        // preventing another Host from reopening a still-live execution store.
        self.retain_lease();
    }
}

// The in-process channel is owned by the local operator's Host, not an
// arbitrary socket or Backend account. decode_method discards wire provenance;
// only this boundary can stamp accepted local human input.
fn authorize_local_input(method: protocol::Method) -> protocol::Method {
    use protocol::{AuthenticatedInputSource, Method};
    match method {
        Method::Submit {
            submission_request_id,
            input,
        }
        | Method::SubmitTracked {
            submission_request_id,
            input,
            ..
        } => Method::SubmitTracked {
            submission_request_id,
            input,
            source: AuthenticatedInputSource::LocalOperator,
        },
        Method::SubmitIfIdle {
            submission_request_id,
            input,
            ..
        } => Method::SubmitIfIdle {
            submission_request_id,
            input,
            source: AuthenticatedInputSource::LocalOperator,
        },
        Method::Notify {
            notification_request_id,
            message,
        }
        | Method::NotifyTracked {
            notification_request_id,
            message,
            ..
        } => Method::NotifyTracked {
            notification_request_id,
            message,
            source: AuthenticatedInputSource::LocalOperator,
        },
        method => method,
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
                let method = authorize_local_input(method);
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
    let command = protocol::WorkerCommandEnvelope::new(u64::MAX);
    let _ = started.handle.send(Method::Shutdown { command }).await;
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
        WorkerBootstrapError::Worker(WorkerError::ScopeLock(ScopeLockError::WriteConflict {
            competitor,
            rule,
            competitor_rule,
        })) => StandaloneStartupError::ScopeConflict {
            competitor,
            requested_rule: rule,
            competitor_rule,
        },
        WorkerBootstrapError::Worker(WorkerError::Provider(_)) => {
            StandaloneStartupError::ModelProvider
        }
        WorkerBootstrapError::Worker(WorkerError::SubjektivSessionAttribution {
            message, ..
        }) => StandaloneStartupError::Subject(message),
        WorkerBootstrapError::Worker(_) => StandaloneStartupError::WorkerConfiguration,
        WorkerBootstrapError::Controller { source, .. }
            if source.kind() == std::io::ErrorKind::Other =>
        {
            StandaloneStartupError::FeatureComposition
        }
        WorkerBootstrapError::Controller { .. } => StandaloneStartupError::Controller,
    }
}

#[cfg(test)]
mod local_input_tests {
    use super::*;
    use protocol::{AuthenticatedInputSource, Method};

    #[test]
    fn local_transport_stamps_operator_without_backend_account() {
        let source = AuthenticatedInputSource::Account {
            account_id: "forged-account".into(),
        };
        for method in [
            Method::submit_text("plain", "hello"),
            Method::SubmitTracked {
                submission_request_id: "tracked".into(),
                input: vec![],
                source: source.clone(),
            },
            Method::SubmitIfIdle {
                submission_request_id: "idle".into(),
                input: vec![],
                source: source.clone(),
            },
            Method::Notify {
                notification_request_id: "plain-notify".into(),
                message: "hello".into(),
            },
            Method::NotifyTracked {
                notification_request_id: "tracked-notify".into(),
                message: "hello".into(),
                source,
            },
        ] {
            let decoded: Method =
                serde_json::from_str(&serde_json::to_string(&method).unwrap()).unwrap();
            match authorize_local_input(decoded) {
                Method::SubmitTracked { source, .. }
                | Method::SubmitIfIdle { source, .. }
                | Method::NotifyTracked { source, .. } => {
                    assert_eq!(source, AuthenticatedInputSource::LocalOperator)
                }
                other => panic!("unexpected local input {other:?}"),
            }
        }
    }
}
