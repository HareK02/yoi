//! Shared registry of Pods spawned by this Pod.
//!
//! `SpawnPod` writes here; the pod-comm tools (`SendToPod`,
//! `ReadPodOutput`, `StopPod`, `ListPods`) read and mutate the same
//! instance. Runtime write-through still materialises `spawned_pods.json`,
//! but durable state lives in the spawner's Pod metadata.
//!
//! `ReadPodOutput` additionally owns a per-spawned-pod cursor here so
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
    PodMetadataStore, PodReclaimedChild, PodSpawnedChild, PodSpawnedScopeRule, PodStoreError,
};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::warn;

use crate::runtime::dir::{RuntimeDir, SpawnedPodRecord};
use crate::runtime::pod_registry;

type RegistryStateWriter = Arc<dyn Fn(&[SpawnedPodRecord]) -> io::Result<()> + Send + Sync>;
type RegistryReclaimWriter = Arc<dyn Fn(&SpawnedPodRecord) -> io::Result<()> + Send + Sync>;

const RESTORE_REACHABILITY_TIMEOUT: Duration = Duration::from_millis(500);

pub struct SpawnedPodRegistry {
    records: Mutex<Vec<SpawnedPodRecord>>,
    cursors: Mutex<HashMap<String, usize>>,
    runtime_dir: Arc<RuntimeDir>,
    state_writer: Option<RegistryStateWriter>,
    reclaim_writer: Option<RegistryReclaimWriter>,
    parent_name: Option<String>,
    parent_scope: Option<SharedScope>,
}

pub struct SpawnedPodRegistryLoad {
    pub registry: Arc<SpawnedPodRegistry>,
    pub reclaimed_unreachable: bool,
}

impl SpawnedPodRegistry {
    pub fn new(runtime_dir: Arc<RuntimeDir>) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            runtime_dir,
            state_writer: None,
            reclaim_writer: None,
            parent_name: None,
            parent_scope: None,
        })
    }

    /// Build a registry from the spawner's durable Pod state, pruning child
    /// records whose socket path is already gone. The surviving list is
    /// written through to both `spawned_pods.json` and Pod state so runtime
    /// and durable views start aligned.
    pub async fn load_from_pod_state<St>(
        runtime_dir: Arc<RuntimeDir>,
        store: St,
        pod_name: String,
    ) -> io::Result<Arc<Self>>
    where
        St: PodMetadataStore + Clone + Send + Sync + 'static,
    {
        let loaded =
            Self::load_from_pod_state_with_reclaim(runtime_dir, store, pod_name, None).await?;
        Ok(loaded.registry)
    }

    pub async fn load_from_pod_state_with_reclaim<St>(
        runtime_dir: Arc<RuntimeDir>,
        store: St,
        pod_name: String,
        parent_scope: Option<SharedScope>,
    ) -> io::Result<SpawnedPodRegistryLoad>
    where
        St: PodMetadataStore + Clone + Send + Sync + 'static,
    {
        let metadata = store.read_by_name(&pod_name).map_err(store_error_to_io)?;
        let persisted_children = metadata
            .as_ref()
            .map(|m| m.spawned_children.clone())
            .unwrap_or_default();

        let mut records = Vec::with_capacity(persisted_children.len());
        let mut pruned_records = Vec::new();
        for child in &persisted_children {
            let record = match record_from_pod_state(child) {
                Ok(record) => record,
                Err(err) => {
                    warn!(
                        error = %err,
                        pod = %child.pod_name,
                        "dropping corrupt persisted spawned-pod record"
                    );
                    continue;
                }
            };
            if is_reachable(&record.socket_path).await {
                records.push(record);
            } else {
                warn!(
                    pod = %record.pod_name,
                    socket = %record.socket_path.display(),
                    "dropping unreachable persisted spawned-pod record"
                );
                pruned_records.push(record);
            }
        }

        runtime_dir.write_spawned_pods(&records).await?;
        let state_writer = pod_state_writer(store.clone(), pod_name.clone());
        let reclaim_writer = pod_state_reclaim_writer(store.clone(), pod_name.clone());
        if metadata.is_none() {
            state_writer(&records)?;
        }

        let mut reclaimed_unreachable = false;
        if !pruned_records.is_empty() {
            let reclaimed = pruned_records
                .iter()
                .map(|record| PodReclaimedChild {
                    pod_name: record.pod_name.clone(),
                    scope_delegated: record
                        .scope_delegated
                        .iter()
                        .map(|rule| PodSpawnedScopeRule {
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
                .reclaim_spawned_children(&pod_name, reclaimed)
                .map_err(store_error_to_io)?;
            reclaimed_unreachable = true;
        }
        if parent_scope.is_some() {
            for record in &pruned_records {
                reclaim_record(&pod_name, parent_scope.as_ref(), record)?;
            }
        }

        Ok(SpawnedPodRegistryLoad {
            registry: Arc::new(Self {
                records: Mutex::new(records),
                cursors: Mutex::new(HashMap::new()),
                runtime_dir,
                state_writer: Some(state_writer),
                reclaim_writer: Some(reclaim_writer),
                parent_name: Some(pod_name),
                parent_scope,
            }),
            reclaimed_unreachable,
        })
    }

    /// Append a new record and persist the full list. Returns an I/O
    /// error if either persisted write fails; the in-memory state is still
    /// updated in that case — the next successful write will reconcile.
    pub async fn add(&self, record: SpawnedPodRecord) -> io::Result<()> {
        let mut records = self.records.lock().await;
        records.push(record);
        self.persist_records(records.as_slice()).await
    }

    /// Look up a record by pod name. Cloned so callers can drop the lock.
    pub async fn get(&self, pod_name: &str) -> Option<SpawnedPodRecord> {
        self.records
            .lock()
            .await
            .iter()
            .find(|r| r.pod_name == pod_name)
            .cloned()
    }

    pub async fn list(&self) -> Vec<SpawnedPodRecord> {
        self.records.lock().await.clone()
    }

    /// Remove the record for `pod_name`, persist, clear its cursor, and
    /// reclaim any delegated Write scope owned by that child. Returns the
    /// removed record (if any).
    pub async fn remove(&self, pod_name: &str) -> io::Result<Option<SpawnedPodRecord>> {
        let removed = {
            let mut records = self.records.lock().await;
            let idx = records.iter().position(|r| r.pod_name == pod_name);
            let removed = idx.map(|i| records.remove(i));
            self.persist_records(records.as_slice()).await?;
            removed
        };
        self.cursors.lock().await.remove(pod_name);
        if let Some(record) = &removed {
            self.reclaim_record(record)?;
            if let Some(write_reclaim) = &self.reclaim_writer {
                write_reclaim(record)?;
            }
        }
        Ok(removed)
    }

    fn reclaim_record(&self, record: &SpawnedPodRecord) -> io::Result<()> {
        let Some(parent_name) = &self.parent_name else {
            release_child_allocation(&record.pod_name)?;
            return Ok(());
        };
        reclaim_record(parent_name, self.parent_scope.as_ref(), record)
    }

    /// Read-only cursor lookup. Returns 0 when no cursor has been set.
    pub async fn cursor(&self, pod_name: &str) -> usize {
        self.cursors
            .lock()
            .await
            .get(pod_name)
            .copied()
            .unwrap_or(0)
    }

    pub async fn set_cursor(&self, pod_name: &str, cursor: usize) {
        self.cursors
            .lock()
            .await
            .insert(pod_name.to_string(), cursor);
    }

    async fn persist_records(&self, records: &[SpawnedPodRecord]) -> io::Result<()> {
        self.runtime_dir.write_spawned_pods(records).await?;
        if let Some(write_state) = &self.state_writer {
            write_state(records)?;
        }
        Ok(())
    }
}

fn pod_state_writer<St>(store: St, pod_name: String) -> RegistryStateWriter
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move |records| {
        write_records_to_pod_state(&store, &pod_name, records).map_err(store_error_to_io)
    })
}

fn pod_state_reclaim_writer<St>(store: St, pod_name: String) -> RegistryReclaimWriter
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move |record| {
        let reclaimed = PodReclaimedChild {
            pod_name: record.pod_name.clone(),
            scope_delegated: record
                .scope_delegated
                .iter()
                .map(|rule| PodSpawnedScopeRule {
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
            .reclaim_spawned_children(&pod_name, vec![reclaimed])
            .map(|_| ())
            .map_err(store_error_to_io)
    })
}

fn reclaim_record(
    parent_name: &str,
    parent_scope: Option<&SharedScope>,
    record: &SpawnedPodRecord,
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
        &record.pod_name,
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

fn release_child_allocation(pod_name: &str) -> io::Result<()> {
    let lock_path = pod_registry::default_registry_path()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let mut guard = pod_registry::LockFileGuard::open(&lock_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    match pod_registry::release_pod(&mut guard, pod_name) {
        Ok(()) | Err(pod_registry::ScopeLockError::UnknownPod(_)) => Ok(()),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err)),
    }
}

fn write_records_to_pod_state<St>(
    store: &St,
    pod_name: &str,
    records: &[SpawnedPodRecord],
) -> Result<(), PodStoreError>
where
    St: PodMetadataStore,
{
    let children = records
        .iter()
        .map(record_to_pod_state)
        .collect::<Result<Vec<_>, _>>()?;
    store.set_spawned_children(pod_name, children)?;
    Ok(())
}

fn record_to_pod_state(record: &SpawnedPodRecord) -> Result<PodSpawnedChild, serde_json::Error> {
    Ok(PodSpawnedChild {
        pod_name: record.pod_name.clone(),
        socket_path: record.socket_path.clone(),
        scope_delegated: record
            .scope_delegated
            .iter()
            .map(|rule| PodSpawnedScopeRule {
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

fn record_from_pod_state(child: &PodSpawnedChild) -> Result<SpawnedPodRecord, serde_json::Error> {
    Ok(SpawnedPodRecord {
        pod_name: child.pod_name.clone(),
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

fn store_error_to_io(error: PodStoreError) -> io::Error {
    io::Error::other(error)
}

async fn is_reachable(socket: &Path) -> bool {
    tokio::time::timeout(RESTORE_REACHABILITY_TIMEOUT, UnixStream::connect(socket))
        .await
        .map(|result| result.is_ok())
        .unwrap_or(false)
}
