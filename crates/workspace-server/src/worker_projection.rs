//! Ordered durable Worker projection, not Runtime execution or Ticket-handling authority.
//!
//! Commit and payload publication share actual bounded DB ownership and the snapshot ordering
//! gate, surviving waiter cancellation. Replacement/shutdown stops producers and drains accepted
//! projection jobs before a new Runtime snapshot can apply; cancelling a consumer alone is not
//! proof that SQL finished. Async resource views reuse the whole stored projection stage rather
//! than acquiring the shared connection on Tokio or reconstructing state from Runtime liveness.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use protocol::subscription::{
    EventSubscriptionSelector, SubscriptionEventPayload, SubscriptionSnapshot,
};
use tokio::sync::{Semaphore, broadcast};
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
    tasks: Mutex<HashMap<String, RuntimeProjectionLifetime>>,
}

struct RuntimeProjectionLifetime {
    consumer: Option<JoinHandle<()>>,
    // The permit belongs to the actual DB closure, not its cancellable async waiter.
    // Keep this owner in the map even if attachment initialization is cancelled.
    operations: Arc<Semaphore>,
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

    /// Stops consumers but does not claim that accepted database work has finished.
    /// Async shutdown callers must use `shutdown_async` to drain accepted operations.
    pub fn shutdown(&self) {
        for lifetime in self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .values()
        {
            if let Some(task) = &lifetime.consumer {
                task.abort();
            }
        }
    }

    pub async fn shutdown_async(&self) {
        let _attach = self.attach_gate.lock().await;
        self.shutdown();
        let runtime_ids = self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for runtime_id in runtime_ids {
            self.drain_runtime(&runtime_id).await;
        }
        self.tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .clear();
    }

    async fn drain_runtime(&self, runtime_id: &str) {
        let previous = self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .get_mut(runtime_id)
            .map(|lifetime| (lifetime.consumer.take(), lifetime.operations.clone()));
        if let Some((consumer, operations)) = previous {
            if let Some(consumer) = consumer {
                consumer.abort();
                let _ = consumer.await;
            }
            // Aborting the waiter cannot abort a queued/running synchronous job.
            // Cancellation here leaves the gate in the map for the next drain.
            let _drained = operations
                .acquire_owned()
                .await
                .expect("operation gate closed");
        }
    }

    // Caller holds attach_gate. Install ownership before any initialization await.
    async fn replace_runtime(&self, runtime_id: &str) -> Arc<Semaphore> {
        self.drain_runtime(runtime_id).await;
        let operations = Arc::new(Semaphore::new(1));
        self.tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .insert(
                runtime_id.to_string(),
                RuntimeProjectionLifetime {
                    consumer: None,
                    operations: operations.clone(),
                },
            );
        operations
    }

    pub async fn subscribe_ordered_async(
        self: &Arc<Self>,
        limit: usize,
    ) -> crate::Result<WorkerProjectionSubscription> {
        let service = self.clone();
        self.store
            .db_executor()
            .run(move || service.subscribe_ordered(limit))
            .await
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
        // Serialize replacement, stop admission by aborting the consumer, and then
        // drain ownership held by the actual SQL jobs before the new snapshot.
        let _attach = self.attach_gate.lock().await;
        let operations = self.replace_runtime(runtime_id).await;

        let mut subscription =
            broker.subscribe(runtime_id, EventSubscriptionSelector::RuntimeWorkers)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let Ok(Some(event)) = tokio::time::timeout_at(deadline, subscription.recv()).await
            else {
                break;
            };
            let initialized = matches!(event, BrokerSubscriptionEvent::Snapshot { .. });
            if let Err(error) = self
                .apply_broker_event_owned_async(runtime_id, event, operations.clone())
                .await
            {
                tracing::warn!(
                    runtime_id,
                    error = %error,
                    "failed to initialize durable Worker projection; restarting from a fresh snapshot"
                );
                if matches!(
                    error,
                    crate::Error::DbOperationOutcomeUnknown
                        | crate::Error::DbExecutorUnavailable
                        | crate::Error::DbCapacityExceeded
                ) {
                    // Do not automatically replay a possibly committed mutation or
                    // spin on executor overload; attachment must be retried explicitly.
                    return Err(RuntimeSubscriptionBrokerError::Closed);
                }
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
                if let Err(error) = service
                    .apply_broker_event_owned_async(
                        task_runtime_id.as_str(),
                        event,
                        operations.clone(),
                    )
                    .await
                {
                    tracing::warn!(
                        runtime_id = %task_runtime_id,
                        error = %error,
                        "failed to update durable Worker projection; restarting from a fresh snapshot"
                    );
                    if matches!(
                        error,
                        crate::Error::DbOperationOutcomeUnknown
                            | crate::Error::DbExecutorUnavailable
                            | crate::Error::DbCapacityExceeded
                    ) {
                        return;
                    }
                    if subscription.restart().is_err() {
                        return;
                    }
                }
            }
        });
        self.tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .get_mut(&runtime_id)
            .expect("Runtime lifetime installed before initialization")
            .consumer = Some(task);
        Ok(())
    }

    #[cfg(test)]
    async fn seed_observation_async(
        self: &Arc<Self>,
        runtime_id: &str,
        worker: &protocol::subscription::SubscriptionWorker,
        observed_at: &str,
    ) -> crate::Result<()> {
        let service = self.clone();
        let runtime_id = runtime_id.to_string();
        let worker = worker.clone();
        let observed_at = observed_at.to_string();
        self.store
            .db_executor()
            .run(move || service.seed_observation(&runtime_id, &worker, &observed_at))
            .await
    }

    pub async fn refresh_async(self: &Arc<Self>, worker: &RuntimeWorkerRef) -> crate::Result<()> {
        let service = self.clone();
        let worker = worker.clone();
        self.store
            .db_executor()
            .run(move || service.refresh(&worker))
            .await
    }

    async fn apply_broker_event_owned_async(
        self: &Arc<Self>,
        runtime_id: &str,
        event: BrokerSubscriptionEvent,
        operations: Arc<Semaphore>,
    ) -> crate::Result<()> {
        let operation = operations
            .acquire_owned()
            .await
            .expect("operation gate closed");
        let service = self.clone();
        let runtime_id = runtime_id.to_string();
        self.store
            .db_executor()
            .run(move || {
                // The permit must outlive SQL commit AND ordered publication, even
                // when the executor result receiver has been dropped by cancellation.
                let _operation = operation;
                service.apply_broker_event(&runtime_id, event)
            })
            .await
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
        let store = Arc::new(
            SqliteWorkspaceStore::open_for_test(temp.path().join("workspace.db"))
                .unwrap()
                .with_db_executor(crate::db_executor::DbExecutor::new(Default::default()).unwrap()),
        );
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

    fn observation(
        state: protocol::subscription::SubscriptionWorkerState,
    ) -> protocol::subscription::SubscriptionWorker {
        protocol::subscription::SubscriptionWorker {
            restore_observation_token: None,
            worker_id: protocol::subscription::SubscriptionWorkerId::new("worker-a").unwrap(),
            runtime_id: Some("runtime-a".to_string()),
            resource_key: None,
            availability: protocol::subscription::SubscriptionWorkerAvailability::Observed,
            worker_state: None,
            state,
            has_running_internal_workers: false,
            workspace_id: Some(WORKSPACE_ID.to_string()),
            display_name: None,
            profile: None,
            job: None,
            workdir_attachments: Vec::new(),
        }
    }

    // Real second-connection contention; the controller releases automatically on
    // failure, so a regression that blocks the Tokio thread cannot hang the suite.
    struct WriteLock {
        release: Option<std::sync::mpsc::Sender<()>>,
        controller: Option<std::thread::JoinHandle<()>>,
    }

    impl WriteLock {
        fn acquire(path: std::path::PathBuf) -> Self {
            let (release, released) = std::sync::mpsc::channel();
            let (locked, ready) = std::sync::mpsc::channel();
            let controller = std::thread::spawn(move || {
                let connection = rusqlite::Connection::open(path).unwrap();
                connection.execute_batch("BEGIN IMMEDIATE").unwrap();
                locked.send(()).unwrap();
                let _ = released.recv_timeout(std::time::Duration::from_secs(3));
                connection.execute_batch("ROLLBACK").unwrap();
            });
            ready.recv().unwrap();
            Self {
                release: Some(release),
                controller: Some(controller),
            }
        }

        fn release(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
        }
    }

    impl Drop for WriteLock {
        fn drop(&mut self) {
            self.release();
            if let Some(controller) = self.controller.take() {
                controller.join().unwrap();
            }
        }
    }

    async fn wait_for_sql(service: &WorkerProjectionService) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            // With the second connection holding the write lock, a held ordering
            // gate identifies the accepted synchronous mutation, not a fake future.
            while service.ordering.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("projection job did not enter synchronous SQL");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sqlite_write_contention_keeps_projection_timer_and_control_live_and_publishes_committed_payload()
     {
        use protocol::subscription::SubscriptionWorkerState;
        let (store, service, temp) = fixture().await;
        store
            .upsert_worker_registry(&worker("runtime-a", "worker-a"))
            .unwrap();
        let mut subscription = service.subscribe_ordered_async(10).await.unwrap();
        assert_eq!(subscription.take_snapshot().len(), 1);
        let mut lock = WriteLock::acquire(temp.path().join("workspace.db"));
        let publishing_service = service.clone();
        let publication = tokio::spawn(async move {
            publishing_service
                .seed_observation_async(
                    "runtime-a",
                    &observation(SubscriptionWorkerState::Idle),
                    "2",
                )
                .await
        });
        wait_for_sql(&service).await;
        let started = std::time::Instant::now();
        let (control, controlled) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            control.send("control progressed").unwrap();
        });
        let (_, control_result) = tokio::join!(
            tokio::time::sleep(std::time::Duration::from_millis(20)),
            controlled,
        );
        assert_eq!(control_result.unwrap(), "control progressed");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "SQLite blocked the current-thread executor"
        );
        assert!(matches!(
            subscription.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        lock.release();
        publication.await.unwrap().unwrap();
        let committed = subscription.recv().await.unwrap();
        let mut snapshot = service.subscribe_ordered_async(10).await.unwrap();
        let snapshot = snapshot.take_snapshot();
        assert_eq!(
            committed.changes,
            vec![WorkerCatalogChange::Upsert(snapshot[0].clone())]
        );
        assert_eq!(
            snapshot[0].observation.as_ref().unwrap().worker.state,
            SubscriptionWorkerState::Idle
        );
        service
            .seed_observation_async(
                "runtime-a",
                &observation(SubscriptionWorkerState::Running),
                "3",
            )
            .await
            .unwrap();
        // Delayed readers receive the first transaction's payload, not a re-query
        // of mutable authority after a later commit.
        let WorkerCatalogChange::Upsert(first) = &committed.changes[0] else {
            panic!("expected upsert");
        };
        assert_eq!(
            first.observation.as_ref().unwrap().worker.state,
            SubscriptionWorkerState::Idle
        );
        service
            .refresh_async(&RuntimeWorkerRef::new("runtime-a", "worker-a"))
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_runtime_consumer_and_replacement_drain_real_sql_before_new_snapshot() {
        use crate::runtime_subscription::BrokerSubscriptionEvent;
        use protocol::subscription::SubscriptionWorkerState;
        let (store, service, temp) = fixture().await;
        store
            .upsert_worker_registry(&worker("runtime-a", "worker-a"))
            .unwrap();
        let mut subscription = service.subscribe_ordered_async(10).await.unwrap();
        subscription.take_snapshot();
        let operations = service.replace_runtime("runtime-a").await;
        let old_operations = operations.clone();
        let mut lock = WriteLock::acquire(temp.path().join("workspace.db"));
        let old_service = service.clone();
        let consumer = tokio::spawn(async move {
            old_service
                .apply_broker_event_owned_async(
                    "runtime-a",
                    BrokerSubscriptionEvent::Event {
                        payload: protocol::subscription::SubscriptionEventPayload::WorkerUpserted {
                            worker: observation(SubscriptionWorkerState::Running),
                        },
                    },
                    operations,
                )
                .await
                .unwrap();
        });
        service
            .tasks
            .lock()
            .unwrap()
            .get_mut("runtime-a")
            .unwrap()
            .consumer = Some(consumer);
        wait_for_sql(&service).await;
        let replacing_service = service.clone();
        let mut replacement = tokio::spawn(async move {
            let _attach = replacing_service.attach_gate.lock().await;
            replacing_service.replace_runtime("runtime-a").await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut replacement)
                .await
                .is_err()
        );
        replacement.abort();
        assert!(replacement.await.unwrap_err().is_cancelled());
        assert_eq!(
            old_operations.available_permits(),
            0,
            "cancelled drain lost accepted SQL ownership"
        );
        // A second replacement must still observe the old gate after cancellation.
        let replacing_service = service.clone();
        let mut replacement = tokio::spawn(async move {
            let _attach = replacing_service.attach_gate.lock().await;
            let operations = replacing_service.replace_runtime("runtime-a").await;
            replacing_service
                .apply_broker_event_owned_async(
                    "runtime-a",
                    BrokerSubscriptionEvent::Snapshot {
                        snapshot: protocol::subscription::SubscriptionSnapshot::Workers {
                            workers: vec![observation(SubscriptionWorkerState::Idle)],
                        },
                    },
                    operations,
                )
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut replacement)
                .await
                .is_err()
        );
        assert!(matches!(
            subscription.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        lock.release();
        replacement.await.unwrap().unwrap();
        assert_eq!(old_operations.available_permits(), 1);
        let old_commit = subscription.recv().await.unwrap();
        let new_commit = subscription.recv().await.unwrap();
        for (commit, expected) in [
            (old_commit, SubscriptionWorkerState::Running),
            (new_commit, SubscriptionWorkerState::Idle),
        ] {
            let [WorkerCatalogChange::Upsert(record)] = commit.changes.as_slice() else {
                panic!("expected one committed upsert");
            };
            assert_eq!(record.observation.as_ref().unwrap().worker.state, expected);
        }
        let mut snapshot = service.subscribe_ordered_async(10).await.unwrap();
        assert_eq!(
            snapshot.take_snapshot()[0]
                .observation
                .as_ref()
                .unwrap()
                .worker
                .state,
            SubscriptionWorkerState::Idle
        );
        assert!(matches!(
            subscription.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        service.shutdown_async().await;
        assert!(service.tasks.lock().unwrap().is_empty());
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
