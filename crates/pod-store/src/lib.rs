//! Durable Pod-name metadata/state persistence.
//!
//! This crate owns the name-keyed Pod state surface under a Pod-state root,
//! e.g. `{data_dir}/pods/{pod_name}/metadata.json`. Session JSONL replay stays
//! in `session-store`; Pod metadata may point at a `(SessionId, SegmentId)` but
//! does not own or replay session logs.
//!
//! `resolved_manifest_snapshot` is authority only for Pod-name restore before
//! loading the session log. Existing segment replay still uses `SegmentStart`
//! entries from `session-store`. `spawned_children` is durable current parent
//! Pod state for child registry/reclaim; child lifecycle messages shown to the
//! model remain session JSONL history. Socket and callback paths are last-known
//! runtime hints, not proof of liveness.

use serde::{Deserialize, Serialize};
use session_store::{SegmentId, SessionId};
use std::fs;
use std::path::PathBuf;

/// Errors from Pod metadata persistence.
#[derive(Debug, thiserror::Error)]
pub enum PodStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid pod name: {0}")]
    InvalidPodName(String),
}

/// Active Session/Segment pointer for a Pod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodActiveSegmentRef {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<SegmentId>,
}

impl PodActiveSegmentRef {
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
/// on manifest scope types in durable Pod state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodSpawnedScopeRule {
    pub target: PathBuf,
    pub permission: String,
    pub recursive: bool,
}

/// One child Pod spawned by this Pod and persisted with the spawner's
/// name-keyed Pod state. Runtime paths are last-known hints only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodSpawnedChild {
    pub pod_name: String,
    pub socket_path: PathBuf,
    pub scope_delegated: Vec<PodSpawnedScopeRule>,
    pub callback_address: PathBuf,
}

/// One child delegation that has been reclaimed. Kept as durable audit state so
/// restore can distinguish outstanding delegated scope from already-reclaimed
/// child state without consulting session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodReclaimedChild {
    pub pod_name: String,
    pub scope_delegated: Vec<PodSpawnedScopeRule>,
}

/// One peer Pod made visible by an explicit peer handshake.
///
/// Peer visibility is intentionally separate from spawned-child delegation: it
/// does not carry filesystem scope, callback ownership, output cursors, or
/// lifecycle-notification authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodPeer {
    pub pod_name: String,
}

/// Persistent metadata for a Pod name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodMetadata {
    pub pod_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<PodActiveSegmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned_children: Vec<PodSpawnedChild>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reclaimed_children: Vec<PodReclaimedChild>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PodPeer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_manifest_snapshot: Option<serde_json::Value>,
}

impl PodMetadata {
    /// Create Pod metadata for `pod_name`.
    pub fn new(pod_name: impl Into<String>, active: Option<PodActiveSegmentRef>) -> Self {
        Self {
            pod_name: pod_name.into(),
            active,
            workspace_root: None,
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
}

/// Sync persistence backend for Pod metadata.
pub trait PodMetadataStore: Send + Sync {
    /// Create or replace metadata for its `pod_name` key.
    fn write(&self, metadata: &PodMetadata) -> Result<(), PodStoreError>;

    /// Read metadata by Pod name. Returns `None` when no metadata exists.
    fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, PodStoreError>;

    /// List persisted Pod metadata keys.
    fn list_names(&self) -> Result<Vec<String>, PodStoreError>;

    /// Return the metadata root directory when this backend is path-backed.
    fn root_dir(&self) -> Option<PathBuf> {
        None
    }

    /// Delete metadata by Pod name. Missing metadata is a successful no-op.
    fn delete_by_name(&self, pod_name: &str) -> Result<(), PodStoreError>;

    /// Merge an update into one Pod's metadata, preserving unrelated fields.
    fn update_by_name<F>(&self, pod_name: &str, update: F) -> Result<PodMetadata, PodStoreError>
    where
        F: FnOnce(&mut PodMetadata),
    {
        let mut metadata = self
            .read_by_name(pod_name)?
            .unwrap_or_else(|| PodMetadata::new(pod_name, None));
        update(&mut metadata);
        metadata.pod_name = pod_name.to_string();
        self.write(&metadata)?;
        Ok(metadata)
    }

    /// Set the active pointer while preserving spawned children and manifest snapshot.
    fn set_active(
        &self,
        pod_name: &str,
        active: Option<PodActiveSegmentRef>,
        resolved_manifest_snapshot: Option<serde_json::Value>,
    ) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            metadata.active = active;
            metadata.resolved_manifest_snapshot = resolved_manifest_snapshot;
        })
    }

    /// Set spawned-child registry state while preserving active pointer and manifest snapshot.
    fn set_spawned_children(
        &self,
        pod_name: &str,
        children: Vec<PodSpawnedChild>,
    ) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            metadata.spawned_children = children;
        })
    }

    /// Set peer visibility state while preserving active pointer, child state,
    /// and manifest snapshot.
    fn set_peers(&self, pod_name: &str, peers: Vec<PodPeer>) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            metadata.peers = peers;
        })
    }

    /// Add one peer if absent while preserving every other metadata field.
    fn add_peer(&self, pod_name: &str, peer_name: &str) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            if !metadata.peers.iter().any(|peer| peer.pod_name == peer_name) {
                metadata.peers.push(PodPeer {
                    pod_name: peer_name.to_string(),
                });
                metadata.peers.sort_by(|a, b| a.pod_name.cmp(&b.pod_name));
            }
        })
    }

    /// Remove one peer while preserving every other metadata field.
    fn remove_peer(&self, pod_name: &str, peer_name: &str) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            metadata.peers.retain(|peer| peer.pod_name != peer_name);
        })
    }

    /// Remove reclaimed child delegations from the outstanding set and record
    /// them in durable reclaim history.
    fn reclaim_spawned_children(
        &self,
        pod_name: &str,
        reclaimed: Vec<PodReclaimedChild>,
    ) -> Result<PodMetadata, PodStoreError> {
        self.update_by_name(pod_name, |metadata| {
            for reclaimed_child in &reclaimed {
                metadata
                    .spawned_children
                    .retain(|child| child.pod_name != reclaimed_child.pod_name);
            }
            metadata.reclaimed_children.extend(reclaimed);
        })
    }
}

/// Filesystem-backed Pod metadata store.
#[derive(Clone)]
pub struct FsPodStore {
    root: PathBuf,
}

impl FsPodStore {
    /// Create a store rooted at the Pod-state directory, usually `{data_dir}/pods`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, PodStoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn pod_dir(&self, pod_name: &str) -> Result<PathBuf, PodStoreError> {
        validate_pod_name(pod_name)?;
        Ok(self.root.join(pod_name))
    }

    fn metadata_path(&self, pod_name: &str) -> Result<PathBuf, PodStoreError> {
        Ok(self.pod_dir(pod_name)?.join("metadata.json"))
    }
}

impl PodMetadataStore for FsPodStore {
    fn write(&self, metadata: &PodMetadata) -> Result<(), PodStoreError> {
        let path = self.metadata_path(&metadata.pod_name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_vec_pretty(metadata)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, PodStoreError> {
        let path = self.metadata_path(pod_name)?;
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(PodStoreError::Io(err)),
        };
        Ok(Some(serde_json::from_str(&content)?))
    }

    fn list_names(&self) -> Result<Vec<String>, PodStoreError> {
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
            if validate_pod_name(&name).is_ok() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn delete_by_name(&self, pod_name: &str) -> Result<(), PodStoreError> {
        let path = self.metadata_path(pod_name)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(PodStoreError::Io(err)),
        }
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

pub fn validate_pod_name(pod_name: &str) -> Result<(), PodStoreError> {
    if pod_name.is_empty()
        || pod_name == "."
        || pod_name == ".."
        || pod_name.contains('/')
        || pod_name.contains('\0')
    {
        return Err(PodStoreError::InvalidPodName(pod_name.to_string()));
    }
    Ok(())
}

/// Convenience composition for callers that want one handle carrying separate
/// session-log and Pod-state roots.
#[derive(Clone)]
pub struct CombinedStore<S, P> {
    pub session_store: S,
    pub pod_store: P,
}

impl<S, P> CombinedStore<S, P> {
    pub fn new(session_store: S, pod_store: P) -> Self {
        Self {
            session_store,
            pod_store,
        }
    }
}

impl<S, P> session_store::Store for CombinedStore<S, P>
where
    S: session_store::Store,
    P: Send + Sync,
{
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &session_store::LogEntry,
    ) -> Result<(), session_store::StoreError> {
        self.session_store.append(session_id, segment_id, entry)
    }
    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<session_store::LogEntry>, session_store::StoreError> {
        self.session_store.read_all(session_id, segment_id)
    }
    fn list_sessions(&self) -> Result<Vec<SessionId>, session_store::StoreError> {
        self.session_store.list_sessions()
    }
    fn list_segments(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<SegmentId>, session_store::StoreError> {
        self.session_store.list_segments(session_id)
    }
    fn lookup_session_of(
        &self,
        segment_id: SegmentId,
    ) -> Result<Option<SessionId>, session_store::StoreError> {
        self.session_store.lookup_session_of(segment_id)
    }
    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[session_store::LogEntry],
    ) -> Result<(), session_store::StoreError> {
        self.session_store
            .create_segment(session_id, segment_id, entries)
    }
    fn exists(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<bool, session_store::StoreError> {
        self.session_store.exists(session_id, segment_id)
    }
    fn truncate(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries_len: usize,
    ) -> Result<(), session_store::StoreError> {
        self.session_store
            .truncate(session_id, segment_id, entries_len)
    }
    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, session_store::StoreError> {
        self.session_store.read_entry_count(session_id, segment_id)
    }
    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &session_store::TraceEntry,
    ) -> Result<(), session_store::StoreError> {
        self.session_store
            .append_trace(session_id, segment_id, entry)
    }
}

impl<S, P> PodMetadataStore for CombinedStore<S, P>
where
    S: Send + Sync,
    P: PodMetadataStore,
{
    fn write(&self, metadata: &PodMetadata) -> Result<(), PodStoreError> {
        self.pod_store.write(metadata)
    }
    fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, PodStoreError> {
        self.pod_store.read_by_name(pod_name)
    }
    fn list_names(&self) -> Result<Vec<String>, PodStoreError> {
        self.pod_store.list_names()
    }
    fn root_dir(&self) -> Option<PathBuf> {
        self.pod_store.root_dir()
    }
    fn delete_by_name(&self, pod_name: &str) -> Result<(), PodStoreError> {
        self.pod_store.delete_by_name(pod_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_metadata_manifest_snapshot_roundtrips() {
        let mut metadata = PodMetadata::new(
            "profile-pod",
            Some(PodActiveSegmentRef::pending_segment(
                session_store::new_session_id(),
            )),
        );
        metadata.resolved_manifest_snapshot = Some(serde_json::json!({
            "pod": { "name": "profile-pod" },
            "profile": { "source": { "kind": "path", "path": "/profiles/coder.lua" } }
        }));

        let json = serde_json::to_string(&metadata).unwrap();
        let restored: PodMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, metadata);
    }

    #[test]
    fn fs_store_writes_under_pod_state_root_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_root = tmp.path().join("sessions");
        let pod_root = tmp.path().join("pods");
        fs::create_dir_all(&session_root).unwrap();
        let store = FsPodStore::new(&pod_root).unwrap();
        store
            .write(&PodMetadata::new(
                "agent",
                Some(PodActiveSegmentRef::pending_segment(
                    session_store::new_session_id(),
                )),
            ))
            .unwrap();

        assert!(pod_root.join("agent/metadata.json").exists());
        assert!(!session_root.join("pods/agent/metadata.json").exists());
    }

    #[test]
    fn active_updates_preserve_children_and_manifest_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsPodStore::new(tmp.path()).unwrap();
        let mut metadata = PodMetadata::new("agent", None);
        metadata.spawned_children.push(PodSpawnedChild {
            pod_name: "child".into(),
            socket_path: std::path::Path::new("/tmp/child.sock").into(),
            scope_delegated: vec![],
            callback_address: std::path::Path::new("/tmp/parent.sock").into(),
        });
        metadata.resolved_manifest_snapshot = Some(serde_json::json!({"pod":{"name":"agent"}}));
        store.write(&metadata).unwrap();

        let snapshot = serde_json::json!({"pod":{"name":"updated"}});
        store
            .set_active(
                "agent",
                Some(PodActiveSegmentRef::active_segment(
                    session_store::new_session_id(),
                    session_store::new_segment_id(),
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
        let store = FsPodStore::new(tmp.path()).unwrap();
        let active = PodActiveSegmentRef::active_segment(
            session_store::new_session_id(),
            session_store::new_segment_id(),
        );
        let snapshot = serde_json::json!({"pod":{"name":"agent"}});
        store
            .set_active("agent", Some(active.clone()), Some(snapshot.clone()))
            .unwrap();
        store
            .set_spawned_children(
                "agent",
                vec![PodSpawnedChild {
                    pod_name: "child".into(),
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
        let store = FsPodStore::new(tmp.path()).unwrap();
        let active = PodActiveSegmentRef::active_segment(
            session_store::new_session_id(),
            session_store::new_segment_id(),
        );
        let snapshot = serde_json::json!({"pod":{"name":"agent"}});
        store
            .set_active("agent", Some(active.clone()), Some(snapshot.clone()))
            .unwrap();
        store
            .set_spawned_children(
                "agent",
                vec![PodSpawnedChild {
                    pod_name: "child".into(),
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
                .map(|peer| peer.pod_name.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-a", "peer-b"]
        );

        store.remove_peer("agent", "peer-a").unwrap();
        let restored = store.read_by_name("agent").unwrap().unwrap();
        assert_eq!(restored.peers.len(), 1);
        assert_eq!(restored.peers[0].pod_name, "peer-b");
    }

    #[test]
    fn reclaim_children_removes_outstanding_and_records_history() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsPodStore::new(tmp.path()).unwrap();
        let scope = PodSpawnedScopeRule {
            target: std::path::Path::new("/tmp/delegated").into(),
            permission: "write".into(),
            recursive: true,
        };
        store
            .set_spawned_children(
                "agent",
                vec![PodSpawnedChild {
                    pod_name: "child".into(),
                    socket_path: std::path::Path::new("/tmp/child.sock").into(),
                    scope_delegated: vec![scope.clone()],
                    callback_address: std::path::Path::new("/tmp/parent.sock").into(),
                }],
            )
            .unwrap();

        store
            .reclaim_spawned_children(
                "agent",
                vec![PodReclaimedChild {
                    pod_name: "child".into(),
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
