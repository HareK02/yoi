//! Parent-owned registry of direct Internal SubWorker sessions.
//!
//! `SubWorkerSpawn` inserts typed `InternalWorkerSessionHandle`s; List/Send/ReadOutput/Stop use
//! the same in-memory authority. Internal children are not persisted, restored, discovered as
//! Runtime Workers, or addressed through sockets. Restore consumes any legacy persisted process
//! child records only to reclaim their delegated scope and clear obsolete metadata.
//!
//! `SubWorkerReadOutput` owns a per-child, process-lifetime history cursor so consecutive reads
//! yield only new assistant text. Parent registry drop closes all session handles and synchronously
//! returns delegated Write deny rules to the parent scope.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use manifest::{Permission, ScopeRule, SharedScope};
use session_store::{
    WorkerMetadataStore, WorkerReclaimedChild, WorkerSpawnedChild, WorkerStoreError,
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::internal_worker::InternalWorkerSessionHandle;
use crate::runtime::dir::{RuntimeDir, SpawnedWorkerRecord};
use crate::runtime::worker_allocation;

#[derive(Clone)]
pub(crate) struct InternalSpawnedWorkerRecord {
    pub worker_name: String,
    pub scope_delegated: Vec<ScopeRule>,
    pub session: InternalWorkerSessionHandle,
}

pub struct SpawnedWorkerRegistry {
    internal_records: std::sync::Mutex<Vec<InternalSpawnedWorkerRecord>>,
    cursors: Mutex<HashMap<String, usize>>,
    parent_scope: Option<SharedScope>,
}

pub struct SpawnedWorkerRegistryLoad {
    pub registry: Arc<SpawnedWorkerRegistry>,
    /// True when obsolete process-child metadata was consumed and cleared.
    pub reclaimed_unreachable: bool,
}

impl SpawnedWorkerRegistry {
    /// Empty registry used by tests and non-spawning projections.
    pub fn new(_runtime_dir: Arc<RuntimeDir>) -> Arc<Self> {
        Arc::new(Self {
            internal_records: std::sync::Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            parent_scope: None,
        })
    }

    pub(crate) fn new_internal(_parent_name: String, parent_scope: SharedScope) -> Arc<Self> {
        Arc::new(Self {
            internal_records: std::sync::Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            parent_scope: Some(parent_scope),
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
                cursors: Mutex::new(HashMap::new()),
                parent_scope,
            }),
            reclaimed_unreachable: !persisted_children.is_empty(),
        })
    }

    pub(crate) fn add_internal(&self, record: InternalSpawnedWorkerRecord) -> io::Result<()> {
        let mut records = self
            .internal_records
            .lock()
            .map_err(|_| io::Error::other("internal spawned-worker registry lock poisoned"))?;
        if records
            .iter()
            .any(|existing| existing.worker_name == record.worker_name)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "spawned worker `{}` is already registered",
                    record.worker_name
                ),
            ));
        }
        records.push(record);
        Ok(())
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

    pub(crate) async fn remove_internal(
        &self,
        worker_name: &str,
    ) -> io::Result<Option<InternalSpawnedWorkerRecord>> {
        let removed = {
            let mut records = self
                .internal_records
                .lock()
                .map_err(|_| io::Error::other("internal spawned-worker registry lock poisoned"))?;
            records
                .iter()
                .position(|record| record.worker_name == worker_name)
                .map(|index| records.remove(index))
        };
        self.cursors.lock().await.remove(worker_name);
        if let (Some(record), Some(parent_scope)) = (&removed, &self.parent_scope) {
            parent_scope
                .update(|current| current.with_removed_deny_rules(delegated_write_rules(record)))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        }
        Ok(removed)
    }

    pub async fn cursor(&self, worker_name: &str) -> usize {
        *self.cursors.lock().await.get(worker_name).unwrap_or(&0)
    }

    pub async fn set_cursor(&self, worker_name: &str, value: usize) {
        self.cursors
            .lock()
            .await
            .insert(worker_name.to_owned(), value);
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

fn store_error_to_io(error: WorkerStoreError) -> io::Error {
    io::Error::other(error)
}
