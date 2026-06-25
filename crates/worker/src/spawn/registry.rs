//! Shared registry of Workers spawned by this Worker.
//!
//! `SpawnWorker` writes here; the worker-comm tools (`SendToWorker`,
//! `ReadWorkerOutput`, `StopWorker`) read and mutate the same instance. Discovery
//! tools consult this registry together with durable Worker state. Runtime
//! write-through still materialises `spawned_workers.json`, but durable state lives
//! in the spawner's Worker metadata.
//!
//! `ReadWorkerOutput` additionally owns a per-spawned-worker cursor here so
//! two consecutive reads yield only new assistant text. The cursor is
//! an item-index into the child's history; push-only history makes
//! index stable across reads.
//!
//! Cursors intentionally do not persist; a restored registry starts with
//! fresh read positions.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use manifest::{Permission, ScopeRule, SharedScope};
use pod_store::{
    WorkerMetadataStore, WorkerReclaimedChild, WorkerSpawnedChild, WorkerSpawnedScopeRule,
    WorkerStoreError,
};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::warn;

use crate::runtime::dir::{RuntimeDir, SpawnedWorkerRecord};
use crate::runtime::pod_registry;

type RegistryStateWriter = Arc<dyn Fn(&[SpawnedWorkerRecord]) -> io::Result<()> + Send + Sync>;
type RegistryReclaimWriter = Arc<dyn Fn(&SpawnedWorkerRecord) -> io::Result<()> + Send + Sync>;

const RESTORE_REACHABILITY_TIMEOUT: Duration = Duration::from_millis(500);
const REGISTRY_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SpawnedWorkerRegistry {
    records: Mutex<Vec<SpawnedWorkerRecord>>,
    cursors: Mutex<HashMap<String, usize>>,
    mutations: Mutex<()>,
    runtime_dir: Arc<RuntimeDir>,
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
            cursors: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
            runtime_dir,
            state_writer: None,
            reclaim_writer: None,
            parent_name: None,
            parent_scope: None,
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

        let mut records = Vec::with_capacity(persisted_children.len());
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
            if is_reachable(&record.socket_path).await {
                records.push(record);
            } else {
                warn!(
                    worker = %record.worker_name,
                    socket = %record.socket_path.display(),
                    "dropping unreachable persisted spawned-worker record"
                );
                pruned_records.push(record);
            }
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
                cursors: Mutex::new(HashMap::new()),
                mutations: Mutex::new(()),
                runtime_dir,
                state_writer: Some(state_writer),
                reclaim_writer: Some(reclaim_writer),
                parent_name: Some(worker_name),
                parent_scope,
            }),
            reclaimed_unreachable,
        })
    }

    /// Append a new record and persist the full list. Returns an I/O
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
        self.runtime_dir.write_spawned_workers(records).await?;
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

    let lock_path = pod_registry::default_registry_path()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let mut guard = pod_registry::LockFileGuard::open(&lock_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    pod_registry::reclaim_delegated_scope(
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
    let lock_path = pod_registry::default_registry_path()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let mut guard = pod_registry::LockFileGuard::open(&lock_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    match pod_registry::release_worker(&mut guard, worker_name) {
        Ok(()) | Err(pod_registry::ScopeLockError::UnknownWorker(_)) => Ok(()),
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

fn store_error_to_io(error: WorkerStoreError) -> io::Error {
    io::Error::other(error)
}

async fn is_reachable(socket: &Path) -> bool {
    tokio::time::timeout(RESTORE_REACHABILITY_TIMEOUT, UnixStream::connect(socket))
        .await
        .map(|result| result.is_ok())
        .unwrap_or(false)
}
