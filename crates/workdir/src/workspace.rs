//! Shared Workspace-facing Workdir inventory transport contracts.
//!
//! These projections describe Workspace inventory and durable occupancy. They
//! intentionally do not expose provider/session handles, host paths, Runtime
//! URLs, or credentials. Runtime-local Workdir operation contracts live in
//! [`crate::http`].

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable Workspace identity for a Worker hosted by a Runtime.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkerRef {
    pub runtime_id: String,
    /// Runtime-owned opaque Worker id. Consumers must not assume a numeric id.
    pub worker_id: String,
}

impl RuntimeWorkerRef {
    pub fn new(runtime_id: impl Into<String>, worker_id: impl Into<String>) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializerKind {
    #[default]
    RuntimeGitCache,
    /// Legacy persisted value from the pre-cache local `git worktree` materializer.
    LocalGitWorktree,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryStatusKind {
    Active,
    CleanupPending,
    Corrupted,
    NotFound,
    Unknown,
}

impl WorkingDirectoryStatusKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::CleanupPending => "cleanup_pending",
            Self::Corrupted => "corrupted",
            Self::NotFound => "not_found",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for WorkingDirectoryStatusKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

/// Durable Workspace occupancy projection for one Workdir.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkingDirectoryOccupancy {
    #[serde(flatten)]
    pub worker: RuntimeWorkerRef,
    pub display_name: String,
    pub linked_at: String,
}

impl<'de> Deserialize<'de> for WorkingDirectoryOccupancy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            runtime_id: String,
            worker_id: String,
            display_name: String,
            linked_at: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            worker: RuntimeWorkerRef::new(wire.runtime_id, wire.worker_id),
            display_name: wire.display_name,
            linked_at: wire.linked_at,
        })
    }
}

/// Immutable materialization provenance retained by Workspace inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    pub materializer_kind: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
}

/// Latest provider-neutral observation attached to Workspace inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCurrentObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_epoch_seconds: Option<u64>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

impl WorkingDirectorySummary {
    /// Workspace-managed inventory rows carry explicit cleanup authority.
    pub fn is_workspace_managed(&self) -> bool {
        self.cleanup_target.is_some()
    }

    pub fn provenance(&self) -> WorkingDirectoryProvenance {
        WorkingDirectoryProvenance {
            creation_selector: self.creation_selector.clone(),
            creation_ref: self.creation_ref.clone(),
            creation_tree: self.creation_tree.clone(),
            materializer_kind: self.materializer_kind.clone(),
            cleanup_target: self.cleanup_target.clone(),
        }
    }

    pub fn current_observation(&self) -> WorkingDirectoryCurrentObservation {
        WorkingDirectoryCurrentObservation {
            current_selector: self.current_selector.clone(),
            current_ref: self.current_ref.clone(),
            current_tree: self.current_tree.clone(),
            observed_at_epoch_seconds: self.observed_at_epoch_seconds,
            status: self.status.clone(),
            cleanliness: self.cleanliness.clone(),
            primary_worker_id: self.primary_worker_id.clone(),
            occupied_by: self.occupied_by.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_directory_status_display_matches_wire_values() {
        for (status, expected) in [
            (WorkingDirectoryStatusKind::Active, "active"),
            (
                WorkingDirectoryStatusKind::CleanupPending,
                "cleanup_pending",
            ),
            (WorkingDirectoryStatusKind::Corrupted, "corrupted"),
            (WorkingDirectoryStatusKind::NotFound, "not_found"),
            (WorkingDirectoryStatusKind::Unknown, "unknown"),
        ] {
            assert_eq!(status.to_string(), expected);
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWorkdirSessionOperationRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session_fence: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegations: Vec<crate::WorkdirDelegationRequest>,
    pub operation: crate::http::WorkdirSessionOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWorkdirSessionFence {
    pub value: String,
}
