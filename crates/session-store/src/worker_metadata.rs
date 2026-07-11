//! Durable Worker-name metadata/state persistence.
//!
//! This crate owns the name-keyed Worker state surface under a Worker-state root,
//! e.g. `{data_dir}/workers/{worker_name}/metadata.json`. Session JSONL replay stays
//! in `session-store`; Worker metadata may point at a `(SessionId, SegmentId)` but
//! does not own or replay session logs.
//!
//! `resolved_manifest_snapshot` is authority only for Worker-name restore before
//! loading the session log. Existing segment replay still uses `SegmentStart`
//! entries from `session-store`. `spawned_children` is durable current parent
//! Worker state for child registry/reclaim; child lifecycle messages shown to the
//! model remain session JSONL history. Socket and callback paths are last-known
//! runtime hints, not proof of liveness.

use crate::{SegmentId, SessionId};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Errors from Worker metadata persistence.
#[derive(Debug, thiserror::Error)]
pub enum WorkerStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid worker name: {0}")]
    InvalidWorkerName(String),
}

/// Active Session/Segment pointer for a Worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerActiveSegmentRef {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<SegmentId>,
}

impl WorkerActiveSegmentRef {
    /// Create a reference whose active Segment is not known yet.
    pub fn pending_segment(session_id: SessionId) -> Self {
        Self {
            session_id,
            segment_id: None,
        }
    }

    /// Create a fully resolved active Session/Segment reference.
    pub fn active_segment(session_id: SessionId, segment_id: SegmentId) -> Self {
        Self {
            session_id,
            segment_id: Some(segment_id),
        }
    }
}

/// One delegated scope rule for a spawned child, kept local to avoid depending
/// on manifest scope types in durable Worker state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSpawnedScopeRule {
    pub target: PathBuf,
    pub permission: String,
    pub recursive: bool,
}

/// One child Worker spawned by this Worker and persisted with the spawner's
/// name-keyed Worker state. Runtime paths are last-known hints only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSpawnedChild {
    pub worker_name: String,
    pub socket_path: PathBuf,
    pub scope_delegated: Vec<WorkerSpawnedScopeRule>,
    pub callback_address: PathBuf,
}

/// One child delegation that has been reclaimed. Kept as durable audit state so
/// restore can distinguish outstanding delegated scope from already-reclaimed
/// child state without consulting session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerReclaimedChild {
    pub worker_name: String,
    pub scope_delegated: Vec<WorkerSpawnedScopeRule>,
}

/// One peer Worker made visible by an explicit peer handshake.
///
/// Peer visibility is intentionally separate from spawned-child delegation: it
/// does not carry filesystem scope, callback ownership, output cursors, or
/// lifecycle-notification authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPeer {
    pub worker_name: String,
}

/// Persistent metadata for a Worker name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerMetadata {
    pub worker_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<WorkerActiveSegmentRef>,
    /// Legacy local path hint retained for host/runtime compatibility. It is not
    /// Worker workspace identity or authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    /// Path-free workspace identity supplied by the host/runtime boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned_children: Vec<WorkerSpawnedChild>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reclaimed_children: Vec<WorkerReclaimedChild>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<WorkerPeer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_manifest_snapshot: Option<serde_json::Value>,
}

impl WorkerMetadata {
    /// Create Worker metadata for `worker_name`.
    pub fn new(worker_name: impl Into<String>, active: Option<WorkerActiveSegmentRef>) -> Self {
        Self {
            worker_name: worker_name.into(),
            active,
            workspace_root: None,
            workspace_id: None,
            spawned_children: Vec::new(),
            reclaimed_children: Vec::new(),
            peers: Vec::new(),
            resolved_manifest_snapshot: None,
        }
    }

    pub fn with_workspace_root(mut self, workspace_root: PathBuf) -> Self {
        self.workspace_root = Some(workspace_root);
        self
    }

    pub fn with_workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }
}

/// Sync persistence backend for Worker metadata.
pub trait WorkerMetadataStore: Send + Sync {
    /// Create or replace metadata for its `worker_name` key.
    fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError>;

    /// Read metadata by Worker name. Returns `None` when no metadata exists.
    fn read_by_name(&self, worker_name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError>;

    /// List persisted Worker metadata keys.
    fn list_names(&self) -> Result<Vec<String>, WorkerStoreError>;

    /// Return the metadata root directory when this backend is path-backed.
    fn root_dir(&self) -> Option<PathBuf> {
        None
    }

    /// Delete metadata by Worker name. Missing metadata is a successful no-op.
    fn delete_by_name(&self, worker_name: &str) -> Result<(), WorkerStoreError>;

    /// Merge an update into one Worker's metadata, preserving unrelated fields.
    fn update_by_name<F>(
        &self,
        worker_name: &str,
        update: F,
    ) -> Result<WorkerMetadata, WorkerStoreError>
    where
        F: FnOnce(&mut WorkerMetadata),
    {
        let mut metadata = self
            .read_by_name(worker_name)?
            .unwrap_or_else(|| WorkerMetadata::new(worker_name, None));
        update(&mut metadata);
        metadata.worker_name = worker_name.to_string();
        self.write(&metadata)?;
        Ok(metadata)
    }

    /// Set the active pointer while preserving spawned children, workspace ownership, and manifest snapshot.
    fn set_active(
        &self,
        worker_name: &str,
        active: Option<WorkerActiveSegmentRef>,
        resolved_manifest_snapshot: Option<serde_json::Value>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.set_active_with_workspace_root(worker_name, active, resolved_manifest_snapshot, None)
    }

    /// Set the active pointer and workspace ownership while preserving unrelated fields.
    fn set_active_with_workspace_context(
        &self,
        worker_name: &str,
        active: Option<WorkerActiveSegmentRef>,
        resolved_manifest_snapshot: Option<serde_json::Value>,
        workspace_id: Option<String>,
        workspace_root: Option<PathBuf>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.active = active;
            metadata.resolved_manifest_snapshot = resolved_manifest_snapshot;
            if let Some(workspace_id) = workspace_id {
                metadata.workspace_id = Some(workspace_id);
            }
            if let Some(workspace_root) = workspace_root {
                metadata.workspace_root = Some(workspace_root);
            }
        })
    }

    /// Set the active pointer and path-free workspace identity while preserving unrelated fields.
    fn set_active_with_workspace_id(
        &self,
        worker_name: &str,
        active: Option<WorkerActiveSegmentRef>,
        resolved_manifest_snapshot: Option<serde_json::Value>,
        workspace_id: Option<String>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.active = active;
            metadata.resolved_manifest_snapshot = resolved_manifest_snapshot;
            if let Some(workspace_id) = workspace_id {
                metadata.workspace_id = Some(workspace_id);
            }
        })
    }

    /// Set the active pointer and legacy local workspace-root hint while preserving unrelated fields.
    fn set_active_with_workspace_root(
        &self,
        worker_name: &str,
        active: Option<WorkerActiveSegmentRef>,
        resolved_manifest_snapshot: Option<serde_json::Value>,
        workspace_root: Option<PathBuf>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.active = active;
            metadata.resolved_manifest_snapshot = resolved_manifest_snapshot;
            if let Some(workspace_root) = workspace_root {
                metadata.workspace_root = Some(workspace_root);
            }
        })
    }

    /// Set spawned-child registry state while preserving active pointer and manifest snapshot.
    fn set_spawned_children(
        &self,
        worker_name: &str,
        children: Vec<WorkerSpawnedChild>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.spawned_children = children;
        })
    }

    /// Set peer visibility state while preserving active pointer, child state,
    /// and manifest snapshot.
    fn set_peers(
        &self,
        worker_name: &str,
        peers: Vec<WorkerPeer>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.peers = peers;
        })
    }

    /// Add one peer if absent while preserving every other metadata field.
    fn add_peer(
        &self,
        worker_name: &str,
        peer_name: &str,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            if !metadata
                .peers
                .iter()
                .any(|peer| peer.worker_name == peer_name)
            {
                metadata.peers.push(WorkerPeer {
                    worker_name: peer_name.to_string(),
                });
                metadata
                    .peers
                    .sort_by(|a, b| a.worker_name.cmp(&b.worker_name));
            }
        })
    }

    /// Remove one peer while preserving every other metadata field.
    fn remove_peer(
        &self,
        worker_name: &str,
        peer_name: &str,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            metadata.peers.retain(|peer| peer.worker_name != peer_name);
        })
    }

    /// Remove reclaimed child delegations from the outstanding set and record
    /// them in durable reclaim history.
    fn reclaim_spawned_children(
        &self,
        worker_name: &str,
        reclaimed: Vec<WorkerReclaimedChild>,
    ) -> Result<WorkerMetadata, WorkerStoreError> {
        self.update_by_name(worker_name, |metadata| {
            for reclaimed_child in &reclaimed {
                metadata
                    .spawned_children
                    .retain(|child| child.worker_name != reclaimed_child.worker_name);
            }
            metadata.reclaimed_children.extend(reclaimed);
        })
    }
}

/// Filesystem-backed Worker metadata store.
#[derive(Clone)]
pub struct FsWorkerStore {
    root: PathBuf,
}

impl FsWorkerStore {
    /// Create a store rooted at the Worker-state directory, usually `{data_dir}/workers`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, WorkerStoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn worker_dir(&self, worker_name: &str) -> Result<PathBuf, WorkerStoreError> {
        validate_worker_name(worker_name)?;
        Ok(self.root.join(worker_name))
    }

    fn metadata_path(&self, worker_name: &str) -> Result<PathBuf, WorkerStoreError> {
        Ok(self.worker_dir(worker_name)?.join("metadata.json"))
    }
}

impl WorkerMetadataStore for FsWorkerStore {
    fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
        let path = self.metadata_path(&metadata.worker_name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_vec_pretty(metadata)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn read_by_name(&self, worker_name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
        let path = self.metadata_path(worker_name)?;
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(WorkerStoreError::Io(err)),
        };
        Ok(Some(serde_json::from_str(&content)?))
    }

    fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
        let mut names = Vec::new();
        if !self.root.exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if !entry.path().join("metadata.json").exists() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if validate_worker_name(&name).is_ok() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn delete_by_name(&self, worker_name: &str) -> Result<(), WorkerStoreError> {
        let path = self.metadata_path(worker_name)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(WorkerStoreError::Io(err)),
        }
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

pub fn validate_worker_name(worker_name: &str) -> Result<(), WorkerStoreError> {
    if worker_name.is_empty()
        || worker_name == "."
        || worker_name == ".."
        || worker_name.contains('/')
        || worker_name.contains('\0')
    {
        return Err(WorkerStoreError::InvalidWorkerName(worker_name.to_string()));
    }
    Ok(())
}

/// Convenience composition for callers that want one handle carrying separate
/// session-log and Worker-state roots.
#[derive(Clone)]
pub struct CombinedStore<S, W> {
    pub session_store: S,
    pub worker_metadata_store: W,
}

impl<S, W> CombinedStore<S, W> {
    pub fn new(session_store: S, worker_metadata_store: W) -> Self {
        Self {
            session_store,
            worker_metadata_store,
        }
    }
}

impl<S, W> crate::Store for CombinedStore<S, W>
where
    S: crate::Store,
    W: Send + Sync,
{
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &crate::LogEntry,
    ) -> Result<(), crate::StoreError> {
        self.session_store.append(session_id, segment_id, entry)
    }
    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<crate::LogEntry>, crate::StoreError> {
        self.session_store.read_all(session_id, segment_id)
    }
    fn list_sessions(&self) -> Result<Vec<SessionId>, crate::StoreError> {
        self.session_store.list_sessions()
    }
    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, crate::StoreError> {
        self.session_store.list_segments(session_id)
    }
    fn lookup_session_of(
        &self,
        segment_id: SegmentId,
    ) -> Result<Option<SessionId>, crate::StoreError> {
        self.session_store.lookup_session_of(segment_id)
    }
    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[crate::LogEntry],
    ) -> Result<(), crate::StoreError> {
        self.session_store
            .create_segment(session_id, segment_id, entries)
    }
    fn exists(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<bool, crate::StoreError> {
        self.session_store.exists(session_id, segment_id)
    }
    fn truncate(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries_len: usize,
    ) -> Result<(), crate::StoreError> {
        self.session_store
            .truncate(session_id, segment_id, entries_len)
    }
    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, crate::StoreError> {
        self.session_store.read_entry_count(session_id, segment_id)
    }
    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &crate::TraceEntry,
    ) -> Result<(), crate::StoreError> {
        self.session_store
            .append_trace(session_id, segment_id, entry)
    }
}

impl<S, W> WorkerMetadataStore for CombinedStore<S, W>
where
    S: Send + Sync,
    W: WorkerMetadataStore,
{
    fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
        self.worker_metadata_store.write(metadata)
    }
    fn read_by_name(&self, worker_name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
        self.worker_metadata_store.read_by_name(worker_name)
    }
    fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
        self.worker_metadata_store.list_names()
    }
    fn root_dir(&self) -> Option<PathBuf> {
        self.worker_metadata_store.root_dir()
    }
    fn delete_by_name(&self, worker_name: &str) -> Result<(), WorkerStoreError> {
        self.worker_metadata_store.delete_by_name(worker_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_metadata_manifest_snapshot_roundtrips() {
        let mut metadata = WorkerMetadata::new(
            "profile-worker",
            Some(WorkerActiveSegmentRef::pending_segment(
                crate::new_session_id(),
            )),
        );
        metadata.resolved_manifest_snapshot = Some(serde_json::json!({
            "worker": { "name": "profile-worker" },
            "profile": { "source": { "kind": "path", "path": "/profiles/coder.toml" } }
        }));

        let json = serde_json::to_string(&metadata).unwrap();
        let restored: WorkerMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, metadata);
    }

    #[test]
    fn fs_store_writes_under_worker_state_root_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_root = tmp.path().join("sessions");
        let worker_root = tmp.path().join("workers");
        fs::create_dir_all(&session_root).unwrap();
        let store = FsWorkerStore::new(&worker_root).unwrap();
        store
            .write(&WorkerMetadata::new(
                "agent",
                Some(WorkerActiveSegmentRef::pending_segment(
                    crate::new_session_id(),
                )),
            ))
            .unwrap();

        assert!(worker_root.join("agent/metadata.json").exists());
        assert!(!session_root.join("workers/agent/metadata.json").exists());
    }

    #[test]
    fn active_updates_preserve_children_and_manifest_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsWorkerStore::new(tmp.path()).unwrap();
        let mut metadata = WorkerMetadata::new("agent", None);
        metadata.spawned_children.push(WorkerSpawnedChild {
            worker_name: "child".into(),
            socket_path: std::path::Path::new("/tmp/child.sock").into(),
            scope_delegated: vec![],
            callback_address: std::path::Path::new("/tmp/parent.sock").into(),
        });
        metadata.resolved_manifest_snapshot = Some(serde_json::json!({"worker":{"name":"agent"}}));
        store.write(&metadata).unwrap();

        let snapshot = serde_json::json!({"worker":{"name":"updated"}});
        store
            .set_active(
                "agent",
                Some(WorkerActiveSegmentRef::active_segment(
                    crate::new_session_id(),
                    crate::new_segment_id(),
                )),
                Some(snapshot.clone()),
            )
            .unwrap();
        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert_eq!(restored.spawned_children.len(), 1);
        assert_eq!(restored.resolved_manifest_snapshot, Some(snapshot));
    }

    #[test]
    fn child_updates_preserve_active_and_manifest_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsWorkerStore::new(tmp.path()).unwrap();
        let active = WorkerActiveSegmentRef::active_segment(
            crate::new_session_id(),
            crate::new_segment_id(),
        );
        let snapshot = serde_json::json!({"worker":{"name":"agent"}});
        store
            .set_active("agent", Some(active.clone()), Some(snapshot.clone()))
            .unwrap();
        store
            .set_spawned_children(
                "agent",
                vec![WorkerSpawnedChild {
                    worker_name: "child".into(),
                    socket_path: std::path::Path::new("/tmp/child.sock").into(),
                    scope_delegated: vec![],
                    callback_address: std::path::Path::new("/tmp/parent.sock").into(),
                }],
            )
            .unwrap();
        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert_eq!(restored.active, Some(active));
        assert_eq!(restored.resolved_manifest_snapshot, Some(snapshot));
    }

    #[test]
    fn peer_updates_preserve_active_children_and_manifest_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsWorkerStore::new(tmp.path()).unwrap();
        let active = WorkerActiveSegmentRef::active_segment(
            crate::new_session_id(),
            crate::new_segment_id(),
        );
        let snapshot = serde_json::json!({"worker":{"name":"agent"}});
        store
            .set_active("agent", Some(active.clone()), Some(snapshot.clone()))
            .unwrap();
        store
            .set_spawned_children(
                "agent",
                vec![WorkerSpawnedChild {
                    worker_name: "child".into(),
                    socket_path: std::path::Path::new("/tmp/child.sock").into(),
                    scope_delegated: vec![],
                    callback_address: std::path::Path::new("/tmp/parent.sock").into(),
                }],
            )
            .unwrap();
        store.add_peer("agent", "peer-b").unwrap();
        store.add_peer("agent", "peer-a").unwrap();
        store.add_peer("agent", "peer-a").unwrap();

        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert_eq!(restored.active, Some(active));
        assert_eq!(restored.spawned_children.len(), 1);
        assert_eq!(restored.resolved_manifest_snapshot, Some(snapshot));
        assert_eq!(
            restored
                .peers
                .iter()
                .map(|peer| peer.worker_name.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-a", "peer-b"]
        );

        store.remove_peer("agent", "peer-a").unwrap();
        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert_eq!(restored.peers.len(), 1);
        assert_eq!(restored.peers[0].worker_name, "peer-b");
    }

    #[test]
    fn reclaim_children_removes_outstanding_and_records_history() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsWorkerStore::new(tmp.path()).unwrap();
        let scope = WorkerSpawnedScopeRule {
            target: std::path::Path::new("/tmp/delegated").into(),
            permission: "write".into(),
            recursive: true,
        };
        store
            .set_spawned_children(
                "agent",
                vec![WorkerSpawnedChild {
                    worker_name: "child".into(),
                    socket_path: std::path::Path::new("/tmp/child.sock").into(),
                    scope_delegated: vec![scope.clone()],
                    callback_address: std::path::Path::new("/tmp/parent.sock").into(),
                }],
            )
            .unwrap();

        store
            .reclaim_spawned_children(
                "agent",
                vec![WorkerReclaimedChild {
                    worker_name: "child".into(),
                    scope_delegated: vec![scope.clone()],
                }],
            )
            .unwrap();
        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert!(restored.spawned_children.is_empty());
        assert_eq!(restored.reclaimed_children.len(), 1);
        assert_eq!(restored.reclaimed_children[0].scope_delegated, vec![scope]);
    }
}
