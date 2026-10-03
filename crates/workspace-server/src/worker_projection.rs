use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEventPayload, SubscriptionSnapshot,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::runtime_subscription::{
    BrokerSubscriptionEvent, RuntimeSubscriptionBroker, RuntimeSubscriptionBrokerError,
};
use crate::store::{
    ControlPlaneStore, WorkerCatalogChange, WorkerRegistryProjectionCommit,
    WorkerRegistryProjectionRecord,
};
use worker_runtime::identity::RuntimeWorkerRef;

const PROJECTION_EVENT_CAPACITY: usize = 256;

pub type WorkerProjectionChange = WorkerCatalogChange;

#[derive(Debug, Clone)]
pub struct WorkerProjectionEvent {
    pub changes: Vec<WorkerProjectionChange>,
}

/// An authoritative durable snapshot followed by changes committed after that snapshot.
pub struct WorkerProjectionSubscription {
    snapshot: Vec<WorkerRegistryProjectionRecord>,
    receiver: broadcast::Receiver<WorkerProjectionEvent>,
}

impl WorkerProjectionSubscription {
    pub fn take_snapshot(&mut self) -> Vec<WorkerRegistryProjectionRecord> {
        std::mem::take(&mut self.snapshot)
    }

    pub async fn recv(&mut self) -> Result<WorkerProjectionEvent, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<WorkerProjectionEvent, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Owns the durable Runtime-to-Workspace Worker projection and its independent
/// low-frequency publication stream.
///
/// The ordering gate covers both durable commits and subscription snapshot creation.
/// Every subscription therefore starts with one authoritative snapshot and receives
/// only commits that completed after it, in the same order in which they were stored.
pub struct WorkerProjectionService {
    workspace_id: String,
    store: Arc<dyn ControlPlaneStore>,
    events: broadcast::Sender<WorkerProjectionEvent>,
    ordering: Mutex<()>,
    attach_gate: tokio::sync::Mutex<()>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl WorkerProjectionService {
    pub fn new(workspace_id: impl Into<String>, store: Arc<dyn ControlPlaneStore>) -> Arc<Self> {
        let (events, _) = broadcast::channel(PROJECTION_EVENT_CAPACITY);
        Arc::new(Self {
            workspace_id: workspace_id.into(),
            store,
            events,
            ordering: Mutex::new(()),
            attach_gate: tokio::sync::Mutex::new(()),
            tasks: Mutex::new(HashMap::new()),
        })
    }

    pub fn shutdown(&self) {
        for (_, task) in self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .drain()
        {
            task.abort();
        }
    }

    pub fn subscribe_ordered(&self, limit: usize) -> crate::Result<WorkerProjectionSubscription> {
        let _ordering = self
            .ordering
            .lock()
            .expect("Worker projection ordering lock poisoned");
        let receiver = self.events.subscribe();
        let snapshot_limit = limit.saturating_add(1);
        let mut snapshot = self
            .store
            .worker_registry_projection_snapshot(self.workspace_id.as_str(), snapshot_limit)?;
        if snapshot.len() > limit {
            return Err(crate::Error::Store(format!(
                "authoritative Worker projection exceeds the configured snapshot limit of {limit} records"
            )));
        }
        snapshot.shrink_to_fit();
        Ok(WorkerProjectionSubscription { snapshot, receiver })
    }

    /// Serialize one durable projection mutation with its publication. The closure
    /// must return the exact projection payload captured by the same transaction.
    pub(crate) fn publish_ordered<T, E>(
        &self,
        mutation: impl FnOnce() -> std::result::Result<(T, WorkerRegistryProjectionCommit), E>,
    ) -> std::result::Result<T, E> {
        let _ordering = self
            .ordering
            .lock()
            .expect("Worker projection ordering lock poisoned");
        let (result, commit) = mutation()?;
        self.publish_commit(commit);
        Ok(result)
    }

    pub async fn attach_runtime(
        self: &Arc<Self>,
        broker: &RuntimeSubscriptionBroker,
        runtime_id: &str,
    ) -> Result<(), RuntimeSubscriptionBrokerError> {
        // Serialize replacement and wait for the old consumer to stop before the new
        // lifetime can apply its snapshot. Late notifications can therefore never
        // cross a Runtime ownership boundary.
        let _attach = self.attach_gate.lock().await;
        let previous = self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .remove(runtime_id);
        if let Some(previous) = previous {
            previous.abort();
            let _ = previous.await;
        }

        let mut subscription =
            broker.subscribe(runtime_id, EventSubscriptionSelector::RuntimeWorkers)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let Ok(Some(event)) = tokio::time::timeout_at(deadline, subscription.recv()).await
            else {
                break;
            };
            let initialized = matches!(event, BrokerSubscriptionEvent::Snapshot { .. });
            if let Err(error) = self.apply_broker_event(runtime_id, event) {
                tracing::warn!(
                    runtime_id,
                    error = %error,
                    "failed to initialize durable Worker projection; restarting from a fresh snapshot"
                );
                subscription.restart()?;
                continue;
            }
            if initialized {
                break;
            }
        }
        let service = Arc::downgrade(self);
        let runtime_id = runtime_id.to_string();
        let task_runtime_id = runtime_id.clone();
        let task = tokio::spawn(async move {
            while let Some(event) = subscription.recv().await {
                let Some(service) = service.upgrade() else {
                    return;
                };
                if let Err(error) = service.apply_broker_event(task_runtime_id.as_str(), event) {
                    tracing::warn!(
                        runtime_id = %task_runtime_id,
                        error = %error,
                        "failed to update durable Worker projection; restarting from a fresh snapshot"
                    );
                    if subscription.restart().is_err() {
                        return;
                    }
                }
            }
        });
        self.tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .insert(runtime_id, task);
        Ok(())
    }

    pub fn seed_observation(
        &self,
        runtime_id: &str,
        worker: &protocol::subscription::SubscriptionWorker,
        observed_at: &str,
    ) -> crate::Result<()> {
        self.publish_ordered(|| {
            self.store
                .apply_worker_registry_observation(
                    self.workspace_id.as_str(),
                    runtime_id,
                    worker,
                    observed_at,
                )
                .map(|commit| ((), commit))
        })
    }

    pub fn refresh(&self, worker: &RuntimeWorkerRef) -> crate::Result<()> {
        self.publish_ordered(|| {
            self.store
                .commit_worker_registry_projection_refresh(self.workspace_id.as_str(), worker)
                .map(|commit| ((), commit))
        })
    }

    fn apply_broker_event(
        &self,
        runtime_id: &str,
        event: BrokerSubscriptionEvent,
    ) -> crate::Result<()> {
        let observed_at = Utc::now().to_rfc3339();
        self.publish_ordered(|| {
            let commit = match event {
                BrokerSubscriptionEvent::Snapshot {
                    snapshot: SubscriptionSnapshot::Workers { workers },
                } => self.store.reconcile_worker_registry_snapshot(
                    self.workspace_id.as_str(),
                    runtime_id,
                    workers.as_slice(),
                    observed_at.as_str(),
                )?,
                BrokerSubscriptionEvent::Snapshot { .. } => {
                    return Ok((
                        (),
                        WorkerRegistryProjectionCommit {
                            changes: Vec::new(),
                        },
                    ));
                }
                BrokerSubscriptionEvent::Event {
                    payload: SubscriptionEventPayload::WorkerUpserted { worker },
                } => self.store.apply_worker_registry_observation(
                    self.workspace_id.as_str(),
                    runtime_id,
                    &worker,
                    observed_at.as_str(),
                )?,
                BrokerSubscriptionEvent::Event {
                    payload: SubscriptionEventPayload::WorkerRemoved { worker_id, .. },
                } => self.store.mark_worker_registry_observation_unavailable(
                    self.workspace_id.as_str(),
                    runtime_id,
                    Some(worker_id.as_str()),
                    observed_at.as_str(),
                )?,
                BrokerSubscriptionEvent::Event { .. } => {
                    return Ok((
                        (),
                        WorkerRegistryProjectionCommit {
                            changes: Vec::new(),
                        },
                    ));
                }
                BrokerSubscriptionEvent::Disconnected { .. }
                | BrokerSubscriptionEvent::Rejected { .. }
                | BrokerSubscriptionEvent::Closed { .. } => {
                    self.store.mark_worker_registry_observation_unavailable(
                        self.workspace_id.as_str(),
                        runtime_id,
                        None,
                        observed_at.as_str(),
                    )?
                }
            };
            Ok(((), commit))
        })
    }

    fn publish_commit(&self, commit: WorkerRegistryProjectionCommit) {
        if commit.changes.is_empty() {
            return;
        }
        let _ = self.events.send(WorkerProjectionEvent {
            changes: commit.changes,
        });
    }
}

impl Drop for WorkerProjectionService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use crate::store::{
        AccountRecord, ControlPlaneStore, SqliteWorkspaceStore, WorkerCatalogChange,
        WorkerRegistryRecord, WorkspaceRecord,
    };
    use worker_runtime::identity::RuntimeWorkerRef;

    use super::WorkerProjectionService;

    const WORKSPACE_ID: &str = "projection-ordering-test";

    async fn fixture() -> (
        Arc<SqliteWorkspaceStore>,
        Arc<WorkerProjectionService>,
        tempfile::TempDir,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteWorkspaceStore::open(temp.path().join("workspace.db")).unwrap());
        store
            .upsert_account(&AccountRecord {
                account_id: "owner".to_string(),
                kind: "user".to_string(),
                handle: "owner".to_string(),
                display_name: "Owner".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: WORKSPACE_ID.to_string(),
                owner_account_id: "owner".to_string(),
                display_name: "Projection ordering".to_string(),
                state: "active".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .await
            .unwrap();
        let service = WorkerProjectionService::new(WORKSPACE_ID, store.clone());
        (store, service, temp)
    }

    fn worker(runtime_id: &str, worker_id: &str) -> WorkerRegistryRecord {
        WorkerRegistryRecord {
            workspace_id: WORKSPACE_ID.to_string(),
            worker: RuntimeWorkerRef::new(runtime_id, worker_id),
            display_name: format!("{runtime_id}-{worker_id}"),
            profile: Some("builtin:coder".to_string()),
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn snapshot_registration_is_atomic_with_concurrent_publication() {
        let (store, service, _temp) = fixture().await;
        let barrier = Arc::new(Barrier::new(2));
        let subscriber_service = service.clone();
        let subscriber_barrier = barrier.clone();
        let subscriber = std::thread::spawn(move || {
            subscriber_barrier.wait();
            subscriber_service.subscribe_ordered(10).unwrap()
        });
        let publisher_service = service.clone();
        let publisher_barrier = barrier.clone();
        let publisher_store = store.clone();
        let record = worker("runtime-a", "worker-a");
        let publisher = std::thread::spawn(move || {
            publisher_barrier.wait();
            publisher_service
                .publish_ordered(|| {
                    publisher_store
                        .upsert_worker_registry(&record)
                        .map(|commit| ((), commit))
                })
                .unwrap();
        });

        let mut subscription = subscriber.join().unwrap();
        publisher.join().unwrap();
        let in_snapshot = subscription
            .take_snapshot()
            .iter()
            .any(|record| record.registry.worker.worker_id == "worker-a");
        let in_event = match subscription.try_recv() {
            Ok(event) => matches!(
                event.changes.as_slice(),
                [WorkerCatalogChange::Upsert(record)]
                    if record.registry.worker.worker_id == "worker-a"
            ),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => false,
            Err(error) => panic!("unexpected projection receive error: {error}"),
        };
        assert_ne!(
            in_snapshot, in_event,
            "the commit must cross the boundary exactly once"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_commits_are_published_in_durable_execution_order() {
        let (store, service, _temp) = fixture().await;
        let mut subscription = service.subscribe_ordered(10).unwrap();
        assert!(subscription.take_snapshot().is_empty());
        let barrier = Arc::new(Barrier::new(3));
        let durable_order = Arc::new(Mutex::new(Vec::new()));
        let mut publishers = Vec::new();
        for worker_id in ["worker-a", "worker-b"] {
            let service = service.clone();
            let store = store.clone();
            let barrier = barrier.clone();
            let durable_order = durable_order.clone();
            let record = worker("runtime-a", worker_id);
            publishers.push(std::thread::spawn(move || {
                barrier.wait();
                service
                    .publish_ordered(|| {
                        durable_order
                            .lock()
                            .unwrap()
                            .push(record.worker.worker_id.clone());
                        store
                            .upsert_worker_registry(&record)
                            .map(|commit| ((), commit))
                    })
                    .unwrap();
            }));
        }
        barrier.wait();
        for publisher in publishers {
            publisher.join().unwrap();
        }

        let published_order = [
            subscription.try_recv().unwrap(),
            subscription.try_recv().unwrap(),
        ]
        .into_iter()
        .map(|event| match event.changes.as_slice() {
            [WorkerCatalogChange::Upsert(record)] => record.registry.worker.worker_id.clone(),
            changes => panic!("unexpected projection changes: {changes:?}"),
        })
        .collect::<Vec<_>>();
        assert_eq!(published_order, *durable_order.lock().unwrap());
    }
}
