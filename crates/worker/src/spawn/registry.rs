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
use std::time::Duration;

use manifest::{Permission, ScopeRule, SharedScope};
use session_store::{
    WorkerMetadataStore, WorkerReclaimedChild, WorkerSpawnedChild, WorkerSpawnedScopeRule,
    WorkerStoreError,
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::internal_worker::InternalWorkerSessionHandle;
use crate::runtime::dir::{RuntimeDir, SpawnedWorkerRecord};
use crate::runtime::worker_allocation;

type RegistryStateWriter = Arc<dyn Fn(&[SpawnedWorkerRecord]) -> io::Result<()> + Send + Sync>;
type RegistryReclaimWriter = Arc<dyn Fn(&SpawnedWorkerRecord) -> io::Result<()> + Send + Sync>;

const REGISTRY_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(crate) struct InternalSpawnedWorkerRecord {
    pub worker_name: String,
    pub scope_delegated: Vec<ScopeRule>,
    pub session: InternalWorkerSessionHandle,
}

pub struct SpawnedWorkerRegistry {
    records: Mutex<Vec<SpawnedWorkerRecord>>,
    internal_records: std::sync::Mutex<Vec<InternalSpawnedWorkerRecord>>,
    cursors: Mutex<HashMap<String, usize>>,
    mutations: Mutex<()>,
    runtime_dir: Option<Arc<RuntimeDir>>,
    state_writer: Option<RegistryStateWriter>,
    reclaim_writer: Option<RegistryReclaimWriter>,
    parent_name: Option<String>,
    parent_scope: Option<SharedScope>,
}

pub struct SpawnedWorkerRegistryLoad {
    pub registry: Arc<SpawnedWorkerRegistry>,
    pub reclaimed_unreachable: bool,
}

impl SpawnedWorkerRegistry {
    pub fn new(runtime_dir: Arc<RuntimeDir>) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            internal_records: std::sync::Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
            runtime_dir: Some(runtime_dir),
            state_writer: None,
            reclaim_writer: None,
            parent_name: None,
            parent_scope: None,
        })
    }

    pub(crate) fn new_internal(parent_name: String, parent_scope: SharedScope) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            internal_records: std::sync::Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
            runtime_dir: None,
            state_writer: None,
            reclaim_writer: None,
            parent_name: Some(parent_name),
            parent_scope: Some(parent_scope),
        })
    }

    /// Build a registry from the spawner's durable Worker state, pruning child
    /// records whose socket path is already gone. The surviving list is
    /// written through to both `spawned_workers.json` and Worker state so runtime
    /// and durable views start aligned.
    pub async fn load_from_worker_state<St>(
        runtime_dir: Arc<RuntimeDir>,
        store: St,
        worker_name: String,
    ) -> io::Result<Arc<Self>>
    where
        St: WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let loaded =
            Self::load_from_worker_state_with_reclaim(runtime_dir, store, worker_name, None)
                .await?;
        Ok(loaded.registry)
    }

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
            .map(|m| m.spawned_children.clone())
            .unwrap_or_default();

        let records = Vec::with_capacity(persisted_children.len());
        let mut pruned_records = Vec::new();
        for child in &persisted_children {
            let record = match record_from_worker_state(child) {
                Ok(record) => record,
                Err(err) => {
                    warn!(
                        error = %err,
                        worker = %child.worker_name,
                        "dropping corrupt persisted spawned-worker record"
                    );
                    continue;
                }
            };
            warn!(
                worker = %record.worker_name,
                "reclaiming legacy persisted process Sub-worker during Internal session restore"
            );
            pruned_records.push(record);
        }

        runtime_dir.write_spawned_workers(&records).await?;
        let state_writer = worker_state_writer(store.clone(), worker_name.clone());
        let reclaim_writer = worker_state_reclaim_writer(store.clone(), worker_name.clone());
        if metadata.is_none() {
            state_writer(&records)?;
        }

        let mut reclaimed_unreachable = false;
        if !pruned_records.is_empty() {
            let reclaimed = pruned_records
                .iter()
                .map(|record| WorkerReclaimedChild {
                    worker_name: record.worker_name.clone(),
                    scope_delegated: record
                        .scope_delegated
                        .iter()
                        .map(|rule| WorkerSpawnedScopeRule {
                            target: rule.target.clone(),
                            permission: match rule.permission {
                                Permission::Read => "read".to_string(),
                                Permission::Write => "write".to_string(),
                            },
                            recursive: rule.recursive,
                        })
                        .collect(),
                })
                .collect();
            store
                .reclaim_spawned_children(&worker_name, reclaimed)
                .map_err(store_error_to_io)?;
            reclaimed_unreachable = true;
        }
        if parent_scope.is_some() {
            for record in &pruned_records {
                reclaim_record(&worker_name, parent_scope.as_ref(), record)?;
            }
        }

        Ok(SpawnedWorkerRegistryLoad {
            registry: Arc::new(Self {
                records: Mutex::new(records),
                internal_records: std::sync::Mutex::new(Vec::new()),
                cursors: Mutex::new(HashMap::new()),
                mutations: Mutex::new(()),
                runtime_dir: Some(runtime_dir),
                state_writer: Some(state_writer),
                reclaim_writer: Some(reclaim_writer),
                parent_name: Some(worker_name),
                parent_scope,
            }),
            reclaimed_unreachable,
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
        Ok(removed)
    }

    /// Append a new legacy process record and persist the full list.
    /// error if either persisted write fails; the in-memory state is still
    /// updated in that case — the next successful write will reconcile.
    pub async fn add(&self, record: SpawnedWorkerRecord) -> io::Result<()> {
        let _mutation = self.mutations.lock().await;
        let snapshot = {
            let mut records = self.records.lock().await;
            records.push(record);
            records.clone()
        };
        self.persist_records(&snapshot).await
    }

    /// Look up a record by worker name. Cloned so callers can drop the lock.
    pub async fn get(&self, worker_name: &str) -> Option<SpawnedWorkerRecord> {
        self.records
            .lock()
            .await
            .iter()
            .find(|r| r.worker_name == worker_name)
            .cloned()
    }

    pub async fn list(&self) -> Vec<SpawnedWorkerRecord> {
        self.records.lock().await.clone()
    }

    /// Remove the record for `worker_name`, persist, clear its cursor, and
    /// reclaim any delegated Write scope owned by that child. Returns the
    /// removed record (if any).
    pub async fn remove(&self, worker_name: &str) -> io::Result<Option<SpawnedWorkerRecord>> {
        let _mutation = self.mutations.lock().await;
        let (removed, snapshot) = {
            let mut records = self.records.lock().await;
            let idx = records.iter().position(|r| r.worker_name == worker_name);
            let removed = idx.map(|i| records.remove(i));
            let snapshot = records.clone();
            (removed, snapshot)
        };
        self.persist_records(&snapshot).await?;
        self.cursors.lock().await.remove(worker_name);
        if let Some(record) = &removed {
            self.reclaim_removed_record(record.clone()).await?;
        }
        Ok(removed)
    }

    async fn reclaim_removed_record(&self, record: SpawnedWorkerRecord) -> io::Result<()> {
        let parent_name = self.parent_name.clone();
        let parent_scope = self.parent_scope.clone();
        let reclaim_writer = self.reclaim_writer.clone();
        let worker_name = record.worker_name.clone();
        let reclaim = tokio::task::spawn_blocking(move || {
            reclaim_removed_record_blocking(parent_name, parent_scope, reclaim_writer, record)
        });
        tokio::time::timeout(REGISTRY_CLEANUP_TIMEOUT, reclaim)
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out reclaiming spawned worker `{worker_name}`"),
                )
            })?
            .map_err(|err| io::Error::other(format!("spawned-worker reclaim task failed: {err}")))?
    }

    /// Read-only cursor lookup. Returns 0 when no cursor has been set.
    pub async fn cursor(&self, worker_name: &str) -> usize {
        self.cursors
            .lock()
            .await
            .get(worker_name)
            .copied()
            .unwrap_or(0)
    }

    pub async fn set_cursor(&self, worker_name: &str, cursor: usize) {
        self.cursors
            .lock()
            .await
            .insert(worker_name.to_string(), cursor);
    }

    async fn persist_records(&self, records: &[SpawnedWorkerRecord]) -> io::Result<()> {
        if let Some(runtime_dir) = &self.runtime_dir {
            runtime_dir.write_spawned_workers(records).await?;
        }
        if let Some(write_state) = &self.state_writer {
            write_state(records)?;
        }
        Ok(())
    }
}

fn worker_state_writer<St>(store: St, worker_name: String) -> RegistryStateWriter
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move |records| {
        write_records_to_worker_state(&store, &worker_name, records).map_err(store_error_to_io)
    })
}

fn worker_state_reclaim_writer<St>(store: St, worker_name: String) -> RegistryReclaimWriter
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move |record| {
        let reclaimed = WorkerReclaimedChild {
            worker_name: record.worker_name.clone(),
            scope_delegated: record
                .scope_delegated
                .iter()
                .map(|rule| WorkerSpawnedScopeRule {
                    target: rule.target.clone(),
                    permission: match rule.permission {
                        Permission::Read => "read".to_string(),
                        Permission::Write => "write".to_string(),
                    },
                    recursive: rule.recursive,
                })
                .collect(),
        };
        store
            .reclaim_spawned_children(&worker_name, vec![reclaimed])
            .map(|_| ())
            .map_err(store_error_to_io)
    })
}

fn reclaim_removed_record_blocking(
    parent_name: Option<String>,
    parent_scope: Option<SharedScope>,
    reclaim_writer: Option<RegistryReclaimWriter>,
    record: SpawnedWorkerRecord,
) -> io::Result<()> {
    if let Some(parent_name) = parent_name {
        reclaim_record(&parent_name, parent_scope.as_ref(), &record)?;
    } else {
        release_child_allocation(&record.worker_name)?;
    }
    if let Some(write_reclaim) = reclaim_writer {
        write_reclaim(&record)?;
    }
    Ok(())
}

fn reclaim_record(
    parent_name: &str,
    parent_scope: Option<&SharedScope>,
    record: &SpawnedWorkerRecord,
) -> io::Result<()> {
    let write_rules = record
        .scope_delegated
        .iter()
        .filter(|rule| rule.permission == Permission::Write)
        .cloned()
        .collect::<Vec<_>>();

    let lock_path = worker_allocation::default_allocation_path()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let mut guard = worker_allocation::LockFileGuard::open(&lock_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    worker_allocation::reclaim_delegated_scope(
        &mut guard,
        parent_name,
        &record.worker_name,
        &record.scope_delegated,
    )
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    if let Some(scope) = parent_scope {
        scope
            .update(|current| current.with_removed_deny_rules(write_rules))
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    }

    Ok(())
}

fn release_child_allocation(worker_name: &str) -> io::Result<()> {
    let lock_path = worker_allocation::default_allocation_path()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let mut guard = worker_allocation::LockFileGuard::open(&lock_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    match worker_allocation::release_worker(&mut guard, worker_name) {
        Ok(()) | Err(worker_allocation::ScopeLockError::UnknownWorker(_)) => Ok(()),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err)),
    }
}

fn write_records_to_worker_state<St>(
    store: &St,
    worker_name: &str,
    records: &[SpawnedWorkerRecord],
) -> Result<(), WorkerStoreError>
where
    St: WorkerMetadataStore,
{
    let children = records
        .iter()
        .map(record_to_worker_state)
        .collect::<Result<Vec<_>, _>>()?;
    store.set_spawned_children(worker_name, children)?;
    Ok(())
}

fn record_to_worker_state(
    record: &SpawnedWorkerRecord,
) -> Result<WorkerSpawnedChild, serde_json::Error> {
    Ok(WorkerSpawnedChild {
        worker_name: record.worker_name.clone(),
        socket_path: record.socket_path.clone(),
        scope_delegated: record
            .scope_delegated
            .iter()
            .map(|rule| WorkerSpawnedScopeRule {
                target: rule.target.clone(),
                permission: match rule.permission {
                    Permission::Read => "read".to_string(),
                    Permission::Write => "write".to_string(),
                },
                recursive: rule.recursive,
            })
            .collect(),
        callback_address: record.callback_address.clone(),
    })
}

fn record_from_worker_state(
    child: &WorkerSpawnedChild,
) -> Result<SpawnedWorkerRecord, serde_json::Error> {
    Ok(SpawnedWorkerRecord {
        worker_name: child.worker_name.clone(),
        socket_path: child.socket_path.clone(),
        scope_delegated: child
            .scope_delegated
            .iter()
            .map(|rule| {
                Ok(ScopeRule {
                    target: rule.target.clone(),
                    permission: match rule.permission.as_str() {
                        "read" => Permission::Read,
                        "write" => Permission::Write,
                        other => {
                            return Err(serde_json::Error::io(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("invalid permission `{other}`"),
                            )));
                        }
                    },
                    recursive: rule.recursive,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        callback_address: child.callback_address.clone(),
    })
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
            .flat_map(|record| record.scope_delegated.iter())
            .filter(|rule| rule.permission == Permission::Write)
            .cloned()
            .collect::<Vec<_>>();
        let _ = parent_scope.update(|current| current.with_removed_deny_rules(write_rules));
    }
}

fn store_error_to_io(error: WorkerStoreError) -> io::Error {
    io::Error::other(error)
}
