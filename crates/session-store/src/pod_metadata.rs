//! Pod metadata persistence API.
//!
//! Pod metadata is a lightweight name-keyed pointer to the Session/Segment
//! currently active for a Pod. Conversation content remains in the segment log;
//! this metadata only records references needed by Pod-name resume/attach flows.

use crate::store::StoreError;
use crate::{SegmentId, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Active Session/Segment pointer for a Pod.
///
/// `segment_id` is optional so callers can persist a reserved Session before
/// the first Segment ID is known. Once a segment exists, callers should rewrite
/// the metadata with `Some(segment_id)`.
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

/// One delegated scope rule for a spawned child, kept local to
/// `session-store` so the persistence crate does not depend on manifest
/// scope types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodSpawnedScopeRule {
    pub target: PathBuf,
    pub permission: String,
    pub recursive: bool,
}

/// One child Pod spawned by this Pod and persisted with the spawner's
/// name-keyed Pod state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodSpawnedChild {
    pub pod_name: String,
    pub socket_path: PathBuf,
    pub scope_delegated: Vec<PodSpawnedScopeRule>,
    pub callback_address: PathBuf,
}

/// Persistent metadata for a Pod name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodMetadata {
    pub pod_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<PodActiveSegmentRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned_children: Vec<PodSpawnedChild>,
}

impl PodMetadata {
    /// Create Pod metadata for `pod_name`.
    pub fn new(pod_name: impl Into<String>, active: Option<PodActiveSegmentRef>) -> Self {
        Self {
            pod_name: pod_name.into(),
            active,
            spawned_children: Vec::new(),
        }
    }
}

/// Sync persistence backend for Pod metadata.
///
/// The key is the Pod name. Missing state is not an error: `read_by_name`
/// returns `Ok(None)` for Pods that have never persisted metadata or whose
/// metadata was deleted.
pub trait PodMetadataStore: Send + Sync {
    /// Create or replace metadata for its `pod_name` key.
    fn write(&self, metadata: &PodMetadata) -> Result<(), StoreError>;

    /// Read metadata by Pod name. Returns `None` when no metadata exists.
    fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, StoreError>;

    /// List persisted Pod metadata keys. Implementations return names only;
    /// callers can then read each item independently so a corrupt metadata
    /// file does not make the whole discovery result fail.
    fn list_names(&self) -> Result<Vec<String>, StoreError>;

    /// Return the metadata root directory when this backend is path-backed.
    fn root_dir(&self) -> Option<PathBuf> {
        None
    }

    /// Delete metadata by Pod name. Missing metadata is a successful no-op.
    fn delete_by_name(&self, pod_name: &str) -> Result<(), StoreError>;
}

pub(crate) fn validate_pod_name(pod_name: &str) -> Result<(), StoreError> {
    if pod_name.is_empty()
        || pod_name == "."
        || pod_name == ".."
        || pod_name.contains('/')
        || pod_name.contains('\0')
    {
        return Err(StoreError::InvalidPodName(pod_name.to_string()));
    }
    Ok(())
}
