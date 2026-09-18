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
    ControlPlaneStore, WorkerRegistryProjectionCommit, WorkerRegistryProjectionRecord,
};
use worker_runtime::identity::RuntimeWorkerRef;

const PROJECTION_EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub enum WorkerProjectionChange {
    Upsert(WorkerRegistryProjectionRecord),
    Removed(RuntimeWorkerRef),
}

#[derive(Debug, Clone)]
pub struct WorkerProjectionEvent {
    pub revision: u64,
    pub changes: Vec<WorkerProjectionChange>,
}

/// Owns the durable Runtime-to-Workspace Worker projection and its independent
/// low-frequency publication stream.
///
/// One Runtime subscription is kept alive per registered Runtime even when no
/// browser is connected. Browser subscriptions therefore never become the
/// authority for populating `worker_registry` observations.
pub struct WorkerProjectionService {
    workspace_id: String,
    store: Arc<dyn ControlPlaneStore>,
    events: broadcast::Sender<WorkerProjectionEvent>,
    connection_generation_epoch: u64,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl WorkerProjectionService {
    pub fn new(workspace_id: impl Into<String>, store: Arc<dyn ControlPlaneStore>) -> Arc<Self> {
        let (events, _) = broadcast::channel(PROJECTION_EVENT_CAPACITY);
        Arc::new(Self {
            workspace_id: workspace_id.into(),
            store,
            events,
            connection_generation_epoch: Utc::now().timestamp_nanos_opt().unwrap_or_default().max(0)
                as u64,
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

    pub fn subscribe(&self) -> broadcast::Receiver<WorkerProjectionEvent> {
        self.events.subscribe()
    }

    pub async fn attach_runtime(
        self: &Arc<Self>,
        broker: &RuntimeSubscriptionBroker,
        runtime_id: &str,
    ) -> Result<(), RuntimeSubscriptionBrokerError> {
        let mut subscription =
            broker.subscribe(runtime_id, EventSubscriptionSelector::RuntimeWorkers)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let Ok(Some(event)) = tokio::time::timeout_at(deadline, subscription.recv()).await
            else {
                break;
            };
            let initialized = matches!(event, BrokerSubscriptionEvent::Snapshot { .. });
            if let Err(error) = self.apply_broker_event(runtime_id, event).await {
                tracing::warn!(
                    runtime_id,
                    error = %error,
                    "failed to initialize durable Worker projection"
                );
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
                    .apply_broker_event(task_runtime_id.as_str(), event)
                    .await
                {
                    tracing::warn!(
                        runtime_id = %task_runtime_id,
                        error = %error,
                        "failed to update durable Worker projection"
                    );
                }
            }
        });
        if let Some(previous) = self
            .tasks
            .lock()
            .expect("Worker projection task lock poisoned")
            .insert(runtime_id, task)
        {
            previous.abort();
        }
        Ok(())
    }

    pub fn seed_observation(
        &self,
        runtime_id: &str,
        worker: &protocol::subscription::SubscriptionWorker,
        observed_at: &str,
    ) -> crate::Result<()> {
        let commit = self.store.apply_worker_registry_observation(
            self.workspace_id.as_str(),
            runtime_id,
            0,
            worker,
            observed_at,
        )?;
        self.publish_commit(commit)
    }

    pub fn publish_catalog_change(&self, worker: &RuntimeWorkerRef) -> crate::Result<()> {
        let commit = self
            .store
            .publish_worker_registry_catalog_change(self.workspace_id.as_str(), worker)?;
        self.publish_commit(commit)
    }

    pub fn publish_removed(&self, worker: RuntimeWorkerRef) -> crate::Result<()> {
        let commit = self
            .store
            .publish_worker_registry_removal(self.workspace_id.as_str(), &worker)?;
        if commit.changed_workers.is_empty() {
            return Ok(());
        }
        let _ = self.events.send(WorkerProjectionEvent {
            revision: commit.revision,
            changes: vec![WorkerProjectionChange::Removed(worker)],
        });
        Ok(())
    }

    async fn apply_broker_event(
        &self,
        runtime_id: &str,
        event: BrokerSubscriptionEvent,
    ) -> crate::Result<()> {
        let observed_at = Utc::now().to_rfc3339();
        let projection_generation = |connection_generation: u64| {
            self.connection_generation_epoch
                .saturating_add(connection_generation)
        };
        let commit = match event {
            BrokerSubscriptionEvent::Snapshot {
                connection_generation,
                snapshot_revision,
                snapshot: SubscriptionSnapshot::Workers { workers },
            } => self.store.reconcile_worker_registry_snapshot(
                self.workspace_id.as_str(),
                runtime_id,
                projection_generation(connection_generation),
                snapshot_revision,
                workers.as_slice(),
                observed_at.as_str(),
            )?,
            BrokerSubscriptionEvent::Snapshot { .. } => return Ok(()),
            BrokerSubscriptionEvent::Event {
                connection_generation,
                payload: SubscriptionEventPayload::WorkerUpserted { worker },
                ..
            } => self.store.apply_worker_registry_observation(
                self.workspace_id.as_str(),
                runtime_id,
                projection_generation(connection_generation),
                &worker,
                observed_at.as_str(),
            )?,
            BrokerSubscriptionEvent::Event {
                connection_generation,
                subject_revision,
                payload: SubscriptionEventPayload::WorkerRemoved { worker_id, .. },
            } => self.store.mark_worker_registry_observation_unavailable(
                self.workspace_id.as_str(),
                runtime_id,
                projection_generation(connection_generation),
                Some(worker_id.as_str()),
                Some(subject_revision),
                observed_at.as_str(),
            )?,
            BrokerSubscriptionEvent::Event { .. } => return Ok(()),
            BrokerSubscriptionEvent::Disconnected {
                connection_generation,
                ..
            }
            | BrokerSubscriptionEvent::Rejected {
                connection_generation,
                ..
            }
            | BrokerSubscriptionEvent::Closed {
                connection_generation,
                ..
            } => self.store.mark_worker_registry_observation_unavailable(
                self.workspace_id.as_str(),
                runtime_id,
                projection_generation(connection_generation),
                None,
                None,
                observed_at.as_str(),
            )?,
        };
        self.publish_commit(commit)
    }

    fn publish_commit(&self, commit: WorkerRegistryProjectionCommit) -> crate::Result<()> {
        if commit.changed_workers.is_empty() {
            return Ok(());
        }
        let changes = commit
            .changed_workers
            .iter()
            .filter_map(|worker| {
                match self
                    .store
                    .worker_registry_projection(self.workspace_id.as_str(), worker)
                {
                    Ok(Some(record)) => Some(WorkerProjectionChange::Upsert(record)),
                    Ok(None) => None,
                    Err(error) => {
                        tracing::warn!(
                            runtime_id = %worker.runtime_id,
                            worker_id = %worker.worker_id,
                            error = %error,
                            "failed to load committed Worker projection"
                        );
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        if !changes.is_empty() {
            let _ = self.events.send(WorkerProjectionEvent {
                revision: commit.revision,
                changes,
            });
        }
        Ok(())
    }
}

impl Drop for WorkerProjectionService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
