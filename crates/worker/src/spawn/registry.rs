//! Parent-owned registry of direct Internal Worker sessions.
//!
//! `SubWorkerSpawn` inserts controllable SubWorker handles, while host services such as
//! compaction insert parent-visible service handles without joining the model-facing
//! List/Send/Stop surface. Internal children are not persisted, restored, discovered as
//! Runtime Workers, or addressed through sockets. Restore consumes any legacy persisted process
//! child records only to reclaim their delegated scope and clear obsolete metadata.
//! Parent registry drop closes all session handles and synchronously returns delegated Write deny
//! rules to the parent scope.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use manifest::{Permission, ScopeRule, SharedScope};
use protocol::{Event, InternalWorkerKind, InternalWorkerRef, InternalWorkerSnapshot};
use session_store::{
    LoggedItem, WorkerMetadataStore, WorkerReclaimedChild, WorkerSpawnedChild, WorkerStoreError,
};
use tokio::sync::broadcast;
use tracing::warn;
use workdir::WorkdirDelegation;

use crate::internal_worker::{InternalWorkerSessionHandle, InternalWorkerVisibility};
use crate::runtime::dir::{RuntimeDir, SpawnedWorkerRecord};
use crate::runtime::worker_allocation;

const STOP_SUMMARY_TOOL_LIMIT: usize = 16;
const STOP_SUMMARY_TOOL_NAME_LIMIT: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SubWorkerFinalOutcome {
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubWorkerToolCount {
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubWorkerChangeStat {
    pub added: u64,
    pub deleted: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubWorkerStopSummary {
    pub session_id: String,
    pub display_name: String,
    pub outcome: SubWorkerFinalOutcome,
    pub elapsed_ms: u64,
    pub tool_counts: Vec<SubWorkerToolCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_stat: Option<SubWorkerChangeStat>,
}

#[derive(Clone)]
pub(crate) struct InternalSpawnedWorkerRecord {
    pub worker_name: String,
    pub scope_delegated: Vec<ScopeRule>,
    pub workdir_delegation: Arc<WorkdirDelegation>,
    #[cfg(test)]
    pub installed_tools: Arc<[String]>,
    pub session: InternalWorkerSessionHandle,
    change_tracker: Option<tools::Tracker>,
    started_at: Instant,
    stop_lock: Arc<tokio::sync::Mutex<()>>,
    scope_reclaimed: Arc<AtomicBool>,
    protocol_revision: Arc<AtomicU64>,
    protocol_emit_lock: Arc<Mutex<()>>,
    protocol_terminal: Arc<AtomicBool>,
    forwarding_started: Arc<AtomicBool>,
}

impl InternalSpawnedWorkerRecord {
    pub(crate) fn new(
        worker_name: String,
        scope_delegated: Vec<ScopeRule>,
        workdir_delegation: WorkdirDelegation,
        #[cfg(test)] installed_tools: Vec<String>,
        session: InternalWorkerSessionHandle,
        change_tracker: Option<tools::Tracker>,
    ) -> Self {
        Self {
            worker_name,
            scope_delegated,
            workdir_delegation: Arc::new(workdir_delegation),
            #[cfg(test)]
            installed_tools: installed_tools.into(),
            session,
            change_tracker,
            started_at: Instant::now(),
            stop_lock: Arc::new(tokio::sync::Mutex::new(())),
            scope_reclaimed: Arc::new(AtomicBool::new(false)),
            protocol_revision: Arc::new(AtomicU64::new(0)),
            protocol_emit_lock: Arc::new(Mutex::new(())),
            protocol_terminal: Arc::new(AtomicBool::new(false)),
            forwarding_started: Arc::new(AtomicBool::new(false)),
        }
    }

    fn stop_summary(&self) -> SubWorkerStopSummary {
        let mut counts = BTreeMap::<String, u64>::new();
        for entry in self.session.entries() {
            if let session_store::LogEntry::AssistantItem {
                item: LoggedItem::ToolCall { name, .. },
                ..
            } = entry
            {
                let count = counts.entry(bounded_tool_name(&name)).or_default();
                *count = count.saturating_add(1);
            }
        }
        let mut tool_counts = counts
            .into_iter()
            .map(|(name, count)| SubWorkerToolCount { name, count })
            .collect::<Vec<_>>();
        tool_counts.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.name.cmp(&right.name))
        });
        tool_counts.truncate(STOP_SUMMARY_TOOL_LIMIT);

        let change_stat = self.change_tracker.as_ref().and_then(|tracker| {
            let stat = tracker.change_stat();
            (stat.added > 0 || stat.deleted > 0).then(|| SubWorkerChangeStat {
                added: stat.added,
                deleted: stat.deleted,
                source: "tracked_write_edit_tools".to_string(),
            })
        });

        SubWorkerStopSummary {
            session_id: self.session.session_id_string(),
            display_name: self.worker_name.clone(),
            outcome: SubWorkerFinalOutcome::Done,
            elapsed_ms: self
                .started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            tool_counts,
            change_stat,
        }
    }

    fn claim_scope_reclaim(&self) -> bool {
        !self.scope_reclaimed.swap(true, Ordering::AcqRel)
    }

    fn restore_scope_reclaim(&self) {
        self.scope_reclaimed.store(false, Ordering::Release);
    }

    fn protocol_ref(&self, parent_session_id: Option<String>) -> InternalWorkerRef {
        InternalWorkerRef {
            session_id: self.session.session_id_string(),
            name: self.worker_name.clone(),
            parent_session_id,
            kind: InternalWorkerKind::SubWorker,
        }
    }

    fn protocol_revision(&self) -> u64 {
        self.protocol_revision.load(Ordering::Acquire)
    }
}

/// Parent-visible service Internal Worker. Unlike a SubWorker this record has no
/// delegated scope, model-facing control name, or stop-summary authority.
#[derive(Clone)]
pub(crate) struct InternalServiceWorkerRecord {
    pub service_kind: String,
    pub display_name: String,
    pub session: InternalWorkerSessionHandle,
    protocol_revision: Arc<AtomicU64>,
    protocol_emit_lock: Arc<Mutex<()>>,
    protocol_terminal: Arc<AtomicBool>,
    forwarding_started: Arc<AtomicBool>,
}

impl InternalServiceWorkerRecord {
    pub(crate) fn new(
        service_kind: impl Into<String>,
        display_name: impl Into<String>,
        session: InternalWorkerSessionHandle,
    ) -> Self {
        Self {
            service_kind: service_kind.into(),
            display_name: display_name.into(),
            session,
            protocol_revision: Arc::new(AtomicU64::new(0)),
            protocol_emit_lock: Arc::new(Mutex::new(())),
            protocol_terminal: Arc::new(AtomicBool::new(false)),
            forwarding_started: Arc::new(AtomicBool::new(false)),
        }
    }

    fn protocol_ref(&self, parent_session_id: Option<String>) -> InternalWorkerRef {
        InternalWorkerRef {
            session_id: self.session.session_id_string(),
            name: self.display_name.clone(),
            parent_session_id,
            kind: InternalWorkerKind::Service {
                kind: self.service_kind.clone(),
            },
        }
    }

    fn protocol_revision(&self) -> u64 {
        self.protocol_revision.load(Ordering::Acquire)
    }
}

pub(crate) struct InternalSpawnReservation {
    registry: Arc<SpawnedWorkerRegistry>,
    worker_name: String,
    committed: bool,
}

impl InternalSpawnReservation {
    pub(crate) fn commit(mut self, record: InternalSpawnedWorkerRecord) -> io::Result<()> {
        if record.worker_name != self.worker_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "internal SubWorker reservation name does not match record name",
            ));
        }
        self.registry
            .internal_records
            .lock()
            .map_err(|_| io::Error::other("internal spawned-worker registry lock poisoned"))?
            .push(record.clone());
        self.registry.start_protocol_forwarding(record);
        self.committed = true;
        Ok(())
    }
}

impl Drop for InternalSpawnReservation {
    fn drop(&mut self) {
        if !self.committed {
            if let Ok(mut names) = self.registry.internal_names.lock() {
                names.remove(&self.worker_name);
            }
        }
    }
}

pub struct SpawnedWorkerRegistry {
    internal_records: std::sync::Mutex<Vec<InternalSpawnedWorkerRecord>>,
    service_records: std::sync::Mutex<Vec<InternalServiceWorkerRecord>>,
    internal_names: std::sync::Mutex<HashSet<String>>,
    parent_scope: Option<SharedScope>,
    parent_protocol: Mutex<Option<(broadcast::Sender<Event>, String)>>,
}

pub struct SpawnedWorkerRegistryLoad {
    pub registry: Arc<SpawnedWorkerRegistry>,
    /// True when obsolete process-child metadata was consumed and cleared.
    pub reclaimed_unreachable: bool,
}

impl SpawnedWorkerRegistry {
    pub(crate) fn new_for_internal_services() -> Arc<Self> {
        Arc::new(Self {
            internal_records: std::sync::Mutex::new(Vec::new()),
            service_records: std::sync::Mutex::new(Vec::new()),
            internal_names: std::sync::Mutex::new(HashSet::new()),
            parent_scope: None,
            parent_protocol: Mutex::new(None),
        })
    }

    /// Empty registry used by tests and non-spawning projections.
    pub fn new(_runtime_dir: Arc<RuntimeDir>) -> Arc<Self> {
        Arc::new(Self {
            internal_records: std::sync::Mutex::new(Vec::new()),
            service_records: std::sync::Mutex::new(Vec::new()),
            internal_names: std::sync::Mutex::new(HashSet::new()),
            parent_scope: None,
            parent_protocol: Mutex::new(None),
        })
    }

    pub(crate) fn new_internal(_parent_name: String, parent_scope: SharedScope) -> Arc<Self> {
        Arc::new(Self {
            internal_records: std::sync::Mutex::new(Vec::new()),
            service_records: std::sync::Mutex::new(Vec::new()),
            internal_names: std::sync::Mutex::new(HashSet::new()),
            parent_scope: Some(parent_scope),
            parent_protocol: Mutex::new(None),
        })
    }

    pub async fn load_from_worker_state<St>(
        runtime_dir: Arc<RuntimeDir>,
        store: St,
        worker_name: String,
    ) -> io::Result<Arc<Self>>
    where
        St: WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        Ok(
            Self::load_from_worker_state_with_reclaim(runtime_dir, store, worker_name, None)
                .await?
                .registry,
        )
    }

    /// Clear obsolete process-child state instead of attempting socket reconnection.
    pub async fn load_from_worker_state_with_reclaim<St>(
        runtime_dir: Arc<RuntimeDir>,
        store: St,
        worker_name: String,
        parent_scope: Option<SharedScope>,
    ) -> io::Result<SpawnedWorkerRegistryLoad>
    where
        St: WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let metadata = store
            .read_by_name(&worker_name)
            .map_err(store_error_to_io)?;
        let persisted_children = metadata
            .as_ref()
            .map(|metadata| metadata.spawned_children.clone())
            .unwrap_or_default();
        let mut valid_records = Vec::new();
        for child in &persisted_children {
            match record_from_worker_state(child) {
                Ok(record) => {
                    warn!(
                        worker = %record.worker_name,
                        "reclaiming legacy persisted process SubWorker during Internal session restore"
                    );
                    valid_records.push(record);
                }
                Err(error) => warn!(
                    error = %error,
                    worker = %child.worker_name,
                    "clearing corrupt legacy persisted process SubWorker record"
                ),
            }
        }

        // Runtime projection is migration input only; the normal Internal registry is never
        // materialized into spawned_workers.json.
        let legacy_projection_exists = runtime_dir.path().join("spawned_workers.json").exists();
        if !persisted_children.is_empty() || legacy_projection_exists {
            runtime_dir.write_spawned_workers(&[]).await?;
        }
        if !persisted_children.is_empty() {
            let reclaimed = persisted_children
                .iter()
                .map(reclaimed_child_from_metadata)
                .collect();
            store
                .reclaim_spawned_children(&worker_name, reclaimed)
                .map_err(store_error_to_io)?;
        }
        for record in &valid_records {
            reclaim_record(&worker_name, parent_scope.as_ref(), record)?;
        }

        Ok(SpawnedWorkerRegistryLoad {
            registry: Arc::new(Self {
                internal_records: std::sync::Mutex::new(Vec::new()),
                service_records: std::sync::Mutex::new(Vec::new()),
                internal_names: std::sync::Mutex::new(HashSet::new()),
                parent_scope,
                parent_protocol: Mutex::new(None),
            }),
            reclaimed_unreachable: !persisted_children.is_empty(),
        })
    }

    pub(crate) fn reserve_internal_name(
        self: &Arc<Self>,
        worker_name: String,
    ) -> io::Result<InternalSpawnReservation> {
        let mut names = self
            .internal_names
            .lock()
            .map_err(|_| io::Error::other("internal SubWorker name registry lock poisoned"))?;
        if !names.insert(worker_name.clone()) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("spawned worker `{worker_name}` is already registered"),
            ));
        }
        drop(names);
        Ok(InternalSpawnReservation {
            registry: Arc::clone(self),
            worker_name,
            committed: false,
        })
    }

    pub(crate) fn attach_parent_protocol(
        &self,
        event_tx: broadcast::Sender<Event>,
        parent_session_id: String,
    ) {
        *self.parent_protocol.lock().unwrap() = Some((event_tx, parent_session_id));
        for record in self.internal_records.lock().unwrap().clone() {
            self.start_protocol_forwarding(record);
        }
        for record in self.service_records.lock().unwrap().clone() {
            self.start_service_protocol_forwarding(record);
        }
    }

    /// Register a parent-visible service Internal Worker before its first turn.
    pub(crate) fn attach_service(
        &self,
        record: InternalServiceWorkerRecord,
    ) -> io::Result<InternalWorkerRef> {
        let parent_session_id = self
            .parent_protocol
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, id)| id.clone());
        let worker_ref = record.protocol_ref(parent_session_id);
        let session_id = record.session.session_id_string();
        let mut records = self
            .service_records
            .lock()
            .map_err(|_| io::Error::other("internal service-worker registry lock poisoned"))?;
        if records
            .iter()
            .any(|candidate| candidate.session.session_id_string() == session_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "internal service Worker is already registered",
            ));
        }
        records.push(record.clone());
        drop(records);
        self.start_service_protocol_forwarding(record);
        Ok(worker_ref)
    }

    /// Stop and remove one parent-owned service Worker. This is host-only and is
    /// intentionally separate from the SubWorker control surface.
    pub(crate) async fn stop_service(&self, session_id: &str) -> io::Result<bool> {
        let record = self
            .service_records
            .lock()
            .map_err(|_| io::Error::other("internal service-worker registry lock poisoned"))?
            .iter()
            .find(|record| record.session.session_id_string() == session_id)
            .cloned();
        let Some(record) = record else {
            return Ok(false);
        };
        record
            .session
            .stop()
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.remove_service(session_id)
    }

    /// Remove one service Worker and emit the terminal projection fence.
    pub(crate) fn remove_service(&self, session_id: &str) -> io::Result<bool> {
        let removed = {
            let mut records = self
                .service_records
                .lock()
                .map_err(|_| io::Error::other("internal service-worker registry lock poisoned"))?;
            records
                .iter()
                .position(|record| record.session.session_id_string() == session_id)
                .map(|index| records.remove(index))
        };
        if let Some(record) = removed {
            self.publish_service_removal(&record);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn start_service_protocol_forwarding(&self, record: InternalServiceWorkerRecord) {
        if record.session.visibility() != InternalWorkerVisibility::ParentClient
            || record.forwarding_started.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Some((parent_tx, parent_session_id)) = self.parent_protocol.lock().unwrap().clone()
        else {
            record.forwarding_started.store(false, Ordering::Release);
            return;
        };
        let worker = record.protocol_ref(Some(parent_session_id));
        let protocol_revision = record.protocol_revision.clone();
        let protocol_emit_lock = record.protocol_emit_lock.clone();
        let protocol_terminal = record.protocol_terminal.clone();
        let mut child_rx = record.session.subscribe_events();
        tokio::spawn(async move {
            loop {
                match child_rx.recv().await {
                    Ok(event) => {
                        let shutdown = matches!(event, Event::Shutdown);
                        let _emit_guard = protocol_emit_lock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if protocol_terminal.load(Ordering::Acquire) {
                            break;
                        }
                        let revision = protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
                        let _ = parent_tx.send(Event::InternalWorker {
                            worker: worker.clone(),
                            revision,
                            event: Box::new(event),
                        });
                        if shutdown {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let _emit_guard = protocol_emit_lock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if protocol_terminal.load(Ordering::Acquire) {
                            break;
                        }
                        let revision = protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
                        let _ = parent_tx.send(Event::InternalWorker {
                            worker: worker.clone(),
                            revision,
                            event: Box::new(Event::Error {
                                code: protocol::ErrorCode::Internal,
                                message: format!(
                                    "internal Worker output lagged by {skipped} events; reconnect to resynchronize"
                                ),
                            }),
                        });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn start_protocol_forwarding(&self, record: InternalSpawnedWorkerRecord) {
        if record.session.visibility() != InternalWorkerVisibility::ParentClient
            || record.forwarding_started.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Some((parent_tx, parent_session_id)) = self.parent_protocol.lock().unwrap().clone()
        else {
            record.forwarding_started.store(false, Ordering::Release);
            return;
        };
        let worker = record.protocol_ref(Some(parent_session_id));
        let protocol_revision = record.protocol_revision.clone();
        let protocol_emit_lock = record.protocol_emit_lock.clone();
        let protocol_terminal = record.protocol_terminal.clone();
        let mut child_rx = record.session.subscribe_events();
        tokio::spawn(async move {
            loop {
                match child_rx.recv().await {
                    Ok(event) => {
                        let shutdown = matches!(event, Event::Shutdown);
                        let _emit_guard = protocol_emit_lock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if protocol_terminal.load(Ordering::Acquire) {
                            break;
                        }
                        let revision = protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
                        let _ = parent_tx.send(Event::InternalWorker {
                            worker: worker.clone(),
                            revision,
                            event: Box::new(event),
                        });
                        if shutdown {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let _emit_guard = protocol_emit_lock
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if protocol_terminal.load(Ordering::Acquire) {
                            break;
                        }
                        let revision = protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
                        let _ = parent_tx.send(Event::InternalWorker {
                            worker: worker.clone(),
                            revision,
                            event: Box::new(Event::Error {
                                code: protocol::ErrorCode::Internal,
                                message: format!(
                                    "internal Worker output lagged by {skipped} events; reconnect to resynchronize"
                                ),
                            }),
                        });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub(crate) fn internal_worker_snapshots(&self) -> Vec<InternalWorkerSnapshot> {
        let parent_session_id = self
            .parent_protocol
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, id)| id.clone());
        let mut snapshots = self
            .internal_records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.session.visibility() == InternalWorkerVisibility::ParentClient)
            .map(|record| {
                internal_worker_snapshot(
                    record.protocol_ref(parent_session_id.clone()),
                    record.protocol_revision(),
                    &record.session,
                )
            })
            .collect::<Vec<_>>();
        snapshots.extend(
            self.service_records
                .lock()
                .unwrap()
                .iter()
                .filter(|record| {
                    record.session.visibility() == InternalWorkerVisibility::ParentClient
                })
                .map(|record| {
                    internal_worker_snapshot(
                        record.protocol_ref(parent_session_id.clone()),
                        record.protocol_revision(),
                        &record.session,
                    )
                }),
        );
        snapshots
    }

    pub(crate) fn get_internal(&self, worker_name: &str) -> Option<InternalSpawnedWorkerRecord> {
        self.internal_records
            .lock()
            .ok()?
            .iter()
            .find(|record| record.worker_name == worker_name)
            .cloned()
    }

    pub(crate) fn list_internal(&self) -> Vec<InternalSpawnedWorkerRecord> {
        self.internal_records
            .lock()
            .map(|records| records.clone())
            .unwrap_or_default()
    }

    pub(crate) fn reclaim_internal_scope(&self, worker_name: &str) -> io::Result<bool> {
        let record = self.get_internal(worker_name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "internal SubWorker not found")
        })?;
        self.reclaim_record_scope(&record)
    }

    fn reclaim_record_scope(&self, record: &InternalSpawnedWorkerRecord) -> io::Result<bool> {
        if !record.claim_scope_reclaim() {
            return Ok(false);
        }
        record.workdir_delegation.release();
        let result = if let Some(parent_scope) = &self.parent_scope {
            parent_scope
                .update(|current| current.with_removed_deny_rules(delegated_write_rules(record)))
                .map(|_| true)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        } else {
            Ok(true)
        };
        if result.is_err() {
            record.restore_scope_reclaim();
        }
        result
    }

    /// Stop one direct Internal SubWorker and discard its registry/scope state.
    ///
    /// The child actor must acknowledge its stop before the registry is removed.
    /// After scope reclamation and removal, `InternalWorkerRemoved` is published
    /// exactly once as the parent-stream terminal fence. Callers only receive
    /// `Done` after all authoritative cleanup succeeds.
    pub(crate) async fn remove_internal(
        &self,
        worker_name: &str,
    ) -> io::Result<Option<SubWorkerStopSummary>> {
        let Some(record) = self.get_internal(worker_name) else {
            return Ok(None);
        };
        let _stop_guard = record.stop_lock.lock().await;
        let still_registered = self.get_internal(worker_name).is_some_and(|current| {
            current.session.session_id_string() == record.session.session_id_string()
        });
        if !still_registered {
            return Ok(None);
        }

        record
            .session
            .stop()
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        let summary = record.stop_summary();
        self.reclaim_record_scope(&record)?;
        let removed =
            {
                let mut records = self.internal_records.lock().map_err(|_| {
                    io::Error::other("internal spawned-worker registry lock poisoned")
                })?;
                let mut names = self.internal_names.lock().map_err(|_| {
                    io::Error::other("internal SubWorker name registry lock poisoned")
                })?;
                let removed = records
                    .iter()
                    .position(|candidate| {
                        candidate.worker_name == worker_name
                            && candidate.session.session_id_string()
                                == record.session.session_id_string()
                    })
                    .map(|index| records.remove(index));
                if removed.is_some() {
                    names.remove(worker_name);
                }
                removed
            };
        if removed.is_some() {
            self.publish_internal_removal(&record);
        }
        Ok(removed.map(|_| summary))
    }

    fn publish_internal_removal(&self, record: &InternalSpawnedWorkerRecord) {
        if record.session.visibility() != InternalWorkerVisibility::ParentClient {
            return;
        }
        let Some((parent_tx, parent_session_id)) = self.parent_protocol.lock().unwrap().clone()
        else {
            return;
        };
        let _emit_guard = record
            .protocol_emit_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        record.protocol_terminal.store(true, Ordering::Release);
        let revision = record.protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = parent_tx.send(Event::InternalWorkerRemoved {
            worker: record.protocol_ref(Some(parent_session_id)),
            revision,
        });
    }

    fn publish_service_removal(&self, record: &InternalServiceWorkerRecord) {
        if record.session.visibility() != InternalWorkerVisibility::ParentClient {
            return;
        }
        let Some((parent_tx, parent_session_id)) = self.parent_protocol.lock().unwrap().clone()
        else {
            return;
        };
        let _emit_guard = record
            .protocol_emit_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        record.protocol_terminal.store(true, Ordering::Release);
        let revision = record.protocol_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = parent_tx.send(Event::InternalWorkerRemoved {
            worker: record.protocol_ref(Some(parent_session_id)),
            revision,
        });
    }
}

fn internal_worker_snapshot(
    worker: InternalWorkerRef,
    revision: u64,
    session: &InternalWorkerSessionHandle,
) -> InternalWorkerSnapshot {
    let snapshot = session.protocol_snapshot();
    InternalWorkerSnapshot {
        worker,
        revision,
        session: snapshot.session,
        status: snapshot.status,
        error: snapshot.error,
        in_flight: snapshot.in_flight,
        internal_workers: snapshot.internal_workers,
    }
}

impl Drop for SpawnedWorkerRegistry {
    fn drop(&mut self) {
        let Some(parent_scope) = &self.parent_scope else {
            return;
        };
        let Ok(records) = self.internal_records.lock() else {
            return;
        };
        let write_rules = records
            .iter()
            .filter(|record| !record.scope_reclaimed.load(Ordering::Acquire))
            .flat_map(delegated_write_rules)
            .collect::<Vec<_>>();
        let _ = parent_scope.update(|current| current.with_removed_deny_rules(write_rules));
    }
}

fn delegated_write_rules(record: &InternalSpawnedWorkerRecord) -> Vec<ScopeRule> {
    record
        .scope_delegated
        .iter()
        .filter(|rule| rule.permission == Permission::Write)
        .cloned()
        .collect()
}

fn reclaimed_child_from_metadata(child: &WorkerSpawnedChild) -> WorkerReclaimedChild {
    WorkerReclaimedChild {
        worker_name: child.worker_name.clone(),
        scope_delegated: child.scope_delegated.clone(),
    }
}

fn reclaim_record(
    parent_name: &str,
    parent_scope: Option<&SharedScope>,
    record: &SpawnedWorkerRecord,
) -> io::Result<()> {
    if let Ok(path) = worker_allocation::default_allocation_path() {
        if let Ok(mut guard) = worker_allocation::LockFileGuard::open(&path) {
            match worker_allocation::reclaim_delegated_scope(
                &mut guard,
                parent_name,
                &record.worker_name,
                &record.scope_delegated,
            ) {
                Ok(()) | Err(worker_allocation::ScopeLockError::UnknownWorker(_)) => {}
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }
    if let Some(parent_scope) = parent_scope {
        let write_rules = record
            .scope_delegated
            .iter()
            .filter(|rule| rule.permission == Permission::Write)
            .cloned()
            .collect::<Vec<_>>();
        parent_scope
            .update(|current| current.with_removed_deny_rules(write_rules))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    }
    Ok(())
}

fn record_from_worker_state(child: &WorkerSpawnedChild) -> io::Result<SpawnedWorkerRecord> {
    let scope_delegated = child
        .scope_delegated
        .iter()
        .map(|rule| {
            let permission = match rule.permission.as_str() {
                "read" => Permission::Read,
                "write" => Permission::Write,
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported spawned-worker permission `{other}`"),
                    ));
                }
            };
            Ok(ScopeRule {
                target: rule.target.clone(),
                permission,
                recursive: rule.recursive,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(SpawnedWorkerRecord {
        worker_name: child.worker_name.clone(),
        socket_path: child.socket_path.clone(),
        scope_delegated,
        callback_address: child.callback_address.clone(),
    })
}

fn bounded_tool_name(name: &str) -> String {
    let mut bounded = name
        .chars()
        .take(STOP_SUMMARY_TOOL_NAME_LIMIT)
        .collect::<String>();
    if name.chars().count() > STOP_SUMMARY_TOOL_NAME_LIMIT {
        bounded.push('…');
    }
    bounded
}

fn store_error_to_io(error: WorkerStoreError) -> io::Error {
    io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use manifest::{Scope, ScopeConfig};
    use session_store::LogEntry;

    use super::*;
    use crate::internal_worker::{InternalWorkerSessionStatus, test_internal_worker_session};

    fn registry() -> Arc<SpawnedWorkerRegistry> {
        let scope = Scope::from_config(&ScopeConfig {
            allow: vec![ScopeRule {
                target: std::path::PathBuf::from("/tmp"),
                permission: Permission::Read,
                recursive: true,
            }],
            deny: Vec::new(),
        })
        .unwrap();
        SpawnedWorkerRegistry::new_internal("parent".into(), SharedScope::new(scope))
    }

    async fn record(
        name: &str,
        visibility: InternalWorkerVisibility,
    ) -> (InternalSpawnedWorkerRecord, broadcast::Sender<Event>) {
        let (session, sender) = test_internal_worker_session(visibility);
        let root = std::path::PathBuf::from("/tmp");
        let scope = Scope::from_config(&ScopeConfig {
            allow: vec![ScopeRule {
                target: root.clone(),
                permission: Permission::Read,
                recursive: true,
            }],
            deny: Vec::new(),
        })
        .unwrap();
        let source = workdir::delegation_capable_session(Arc::new(
            workdir::LocalWorkdirSession::materialized_bound(
                workdir::Workdir::new("registry-test"),
                root.clone(),
                root,
                SharedScope::new(scope),
                workdir::WorkdirSessionCapabilities::ALL,
            ),
        ));
        let delegation = source
            .delegate(workdir::WorkdirDelegationRequest {
                rules: vec![workdir::WorkdirDelegationRule {
                    target: workdir::WorkdirPath::new("").unwrap(),
                    permission: workdir::WorkdirDelegationPermission::Read,
                    recursive: true,
                }],
                cwd: workdir::WorkdirPath::new("").unwrap(),
            })
            .await
            .unwrap();
        (
            InternalSpawnedWorkerRecord::new(
                name.into(),
                Vec::new(),
                delegation,
                Vec::new(),
                session,
                None,
            ),
            sender,
        )
    }

    #[tokio::test]
    async fn visible_internal_output_is_wrapped_after_registry_insertion() {
        let registry = registry();
        let (parent_tx, mut parent_rx) = broadcast::channel(16);
        registry.attach_parent_protocol(parent_tx, "parent-session".into());
        let (record, child_tx) = record("research", InternalWorkerVisibility::ParentClient).await;
        registry
            .internal_records
            .lock()
            .unwrap()
            .push(record.clone());
        registry.start_protocol_forwarding(record.clone());

        child_tx
            .send(Event::TextDone {
                text: "answer".into(),
            })
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), parent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            Event::InternalWorker { worker, revision: 1, event }
                if worker.name == "research"
                    && worker.parent_session_id.as_deref() == Some("parent-session")
                    && matches!(*event, Event::TextDone { ref text } if text == "answer")
        ));
        record.session.publish_test_entry(LogEntry::UserInput {
            ts: 1,
            segments: vec![protocol::Segment::text("question")],
            extensions: Vec::new(),
        });
        let committed = tokio::time::timeout(Duration::from_secs(1), parent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            committed,
            Event::InternalWorker { revision: 2, event, .. }
                if matches!(*event, Event::UserMessage { .. })
        ));
        let snapshots = registry.internal_worker_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].revision, 2);
        assert_eq!(snapshots[0].session.entries.len(), 1);

        record.session.emit_test_text_delta("partial");
        let streamed = tokio::time::timeout(Duration::from_secs(1), parent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            streamed,
            Event::InternalWorker { revision: 3, event, .. }
                if matches!(*event, Event::TextDelta { ref text } if text == "partial")
        ));
        let snapshots = registry.internal_worker_snapshots();
        assert_eq!(snapshots[0].revision, 3);
        assert_eq!(snapshots[0].in_flight.blocks.len(), 1);
    }

    #[tokio::test]
    async fn service_worker_is_parent_visible_but_not_subworker_controllable() {
        let registry = registry();
        let (parent_tx, mut parent_rx) = broadcast::channel(16);
        registry.attach_parent_protocol(parent_tx, "parent-session".into());
        let (session, child_tx) =
            test_internal_worker_session(InternalWorkerVisibility::ParentClient);
        let session_id = session.session_id_string();

        let worker_ref = registry
            .attach_service(InternalServiceWorkerRecord::new(
                "compaction",
                "Compaction",
                session,
            ))
            .unwrap();
        assert!(matches!(
            worker_ref.kind,
            InternalWorkerKind::Service { ref kind } if kind == "compaction"
        ));
        assert!(registry.list_internal().is_empty());

        child_tx
            .send(Event::TextDone {
                text: "summary candidate".into(),
            })
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), parent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            Event::InternalWorker { worker, revision: 1, event }
                if worker.session_id == session_id
                    && matches!(worker.kind, InternalWorkerKind::Service { ref kind } if kind == "compaction")
                    && matches!(*event, Event::TextDone { ref text } if text == "summary candidate")
        ));
        let snapshots = registry.internal_worker_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(matches!(
            snapshots[0].worker.kind,
            InternalWorkerKind::Service { ref kind } if kind == "compaction"
        ));
        assert!(registry.remove_service(&session_id).unwrap());
        let removed = tokio::time::timeout(Duration::from_secs(1), parent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            removed,
            Event::InternalWorkerRemoved { worker, revision: 2 }
                if worker.session_id == session_id
        ));
        assert!(registry.internal_worker_snapshots().is_empty());
    }

    #[tokio::test]
    async fn service_private_internal_output_is_never_disclosed() {
        let registry = registry();
        let (parent_tx, mut parent_rx) = broadcast::channel(16);
        registry.attach_parent_protocol(parent_tx, "parent-session".into());
        let (record, child_tx) =
            record("memory-helper", InternalWorkerVisibility::ServicePrivate).await;
        registry
            .internal_records
            .lock()
            .unwrap()
            .push(record.clone());
        registry.start_protocol_forwarding(record);
        let _ = child_tx.send(Event::TextDone {
            text: "private".into(),
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(50), parent_rx.recv())
                .await
                .is_err()
        );
        assert!(registry.internal_worker_snapshots().is_empty());
    }

    fn install_record(registry: &SpawnedWorkerRegistry, record: InternalSpawnedWorkerRecord) {
        registry
            .internal_names
            .lock()
            .unwrap()
            .insert(record.worker_name.clone());
        registry.internal_records.lock().unwrap().push(record);
    }

    #[tokio::test]
    async fn stop_removes_internal_worker_and_returns_bounded_summary() {
        let registry = registry();
        let (parent_tx, mut parent_rx) = broadcast::channel(32);
        registry.attach_parent_protocol(parent_tx, "parent-session".into());
        let tracker = tools::Tracker::new();
        tracker.record_change(12, 4);
        let (mut record, _events) = record("child", InternalWorkerVisibility::ParentClient).await;
        record.change_tracker = Some(tracker);
        for (index, name) in ["Read", "Read", "Grep"].into_iter().enumerate() {
            record.session.publish_test_entry(LogEntry::AssistantItem {
                ts: index as u64,
                item: LoggedItem::ToolCall {
                    call_id: format!("call-{index}"),
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            });
        }
        registry.start_protocol_forwarding(record.clone());
        install_record(&registry, record);

        let summary = registry.remove_internal("child").await.unwrap().unwrap();

        assert_eq!(summary.display_name, "child");
        assert_eq!(summary.outcome, SubWorkerFinalOutcome::Done);
        assert_eq!(
            summary.tool_counts,
            vec![
                SubWorkerToolCount {
                    name: "Read".to_string(),
                    count: 2,
                },
                SubWorkerToolCount {
                    name: "Grep".to_string(),
                    count: 1,
                },
            ]
        );
        assert_eq!(
            summary.change_stat,
            Some(SubWorkerChangeStat {
                added: 12,
                deleted: 4,
                source: "tracked_write_edit_tools".to_string(),
            })
        );
        assert!(registry.get_internal("child").is_none());
        let terminal_revision = loop {
            if let Event::InternalWorkerRemoved { worker, revision } =
                parent_rx.recv().await.unwrap()
            {
                assert_eq!(worker.session_id, summary.session_id);
                assert!(revision > 0);
                break revision;
            }
        };
        assert!(registry.remove_internal("child").await.unwrap().is_none());
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(20), parent_rx.recv()).await
        {
            assert!(!matches!(event, Event::InternalWorkerRemoved { .. }));
            if let Event::InternalWorker { revision, .. } = event {
                assert!(revision > terminal_revision);
            }
        }
    }

    #[tokio::test]
    async fn running_worker_is_stopped_before_removal() {
        let registry = registry();
        let (record, _events) = record("running", InternalWorkerVisibility::ParentClient).await;
        record
            .session
            .force_status(InternalWorkerSessionStatus::Running);
        install_record(&registry, record);

        let summary = registry.remove_internal("running").await.unwrap().unwrap();

        assert_eq!(summary.outcome, SubWorkerFinalOutcome::Done);
        assert!(registry.get_internal("running").is_none());
    }

    #[tokio::test]
    async fn stop_failure_keeps_registry_and_emits_no_removal() {
        let registry = registry();
        let (parent_tx, mut parent_rx) = broadcast::channel(8);
        registry.attach_parent_protocol(parent_tx, "parent-session".into());
        let (record, _events) = record("child", InternalWorkerVisibility::ParentClient).await;
        record.session.force_stop_failure();
        install_record(&registry, record);

        let error = registry.remove_internal("child").await.unwrap_err();

        assert!(error.to_string().contains("unavailable"));
        assert!(registry.get_internal("child").is_some());
        assert!(matches!(
            parent_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn read_only_summary_omits_unavailable_change_stat() {
        let tracker = tools::Tracker::new();
        let (mut record, _events) = record("reader", InternalWorkerVisibility::ParentClient).await;
        record.change_tracker = Some(tracker);

        assert_eq!(record.stop_summary().change_stat, None);
    }
}
