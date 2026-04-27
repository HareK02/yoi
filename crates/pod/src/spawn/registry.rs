//! Shared registry of Pods spawned by this Pod.
//!
//! `SpawnPod` writes here; the pod-comm tools (`SendToPod`,
//! `ReadPodOutput`, `StopPod`, `ListPods`) read and mutate the same
//! instance. Persisted to `spawned_pods.json` in the spawner's runtime
//! dir so a restarted spawner rebuilds its view from disk (future work
//! — today only write-through is implemented).
//!
//! `ReadPodOutput` additionally owns a per-spawned-pod cursor here so
//! two consecutive reads yield only new assistant text. The cursor is
//! an item-index into the child's history; push-only history makes
//! index stable across reads.
//!
//! The registry stays in-memory only for this Pod's lifetime — cursors
//! intentionally do not persist.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::runtime::dir::{RuntimeDir, SpawnedPodRecord};

pub struct SpawnedPodRegistry {
    records: Mutex<Vec<SpawnedPodRecord>>,
    cursors: Mutex<HashMap<String, usize>>,
    runtime_dir: Arc<RuntimeDir>,
}

impl SpawnedPodRegistry {
    pub fn new(runtime_dir: Arc<RuntimeDir>) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            runtime_dir,
        })
    }

    /// Append a new record and persist the full list. Returns an I/O
    /// error if the persisted write fails; the in-memory state is still
    /// updated in that case — the next successful write will reconcile.
    pub async fn add(&self, record: SpawnedPodRecord) -> io::Result<()> {
        let mut records = self.records.lock().await;
        records.push(record);
        self.runtime_dir
            .write_spawned_pods(records.as_slice())
            .await
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

    /// Remove the record for `pod_name`, persist, and clear its cursor.
    /// Returns the removed record (if any).
    pub async fn remove(&self, pod_name: &str) -> io::Result<Option<SpawnedPodRecord>> {
        let removed = {
            let mut records = self.records.lock().await;
            let idx = records.iter().position(|r| r.pod_name == pod_name);
            let removed = idx.map(|i| records.remove(i));
            self.runtime_dir
                .write_spawned_pods(records.as_slice())
                .await?;
            removed
        };
        self.cursors.lock().await.remove(pod_name);
        Ok(removed)
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
}
