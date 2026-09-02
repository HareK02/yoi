//! Shared Workspace-facing Workdir inventory transport contracts.
//!
//! These projections describe Workspace inventory and durable occupancy. They
//! intentionally do not expose provider/session handles, host paths, Runtime
//! URLs, or credentials. Runtime-local Workdir operation contracts live in
//! [`crate::http`].

use serde::{Deserialize, Serialize};

pub use workspace_api::{
    RuntimeWorkingDirectoryCleanupTarget, RuntimeWorkingDirectorySummary,
    WorkingDirectoryCleanupTarget, WorkingDirectoryMaterializerKind as MaterializerKind,
    WorkingDirectoryOccupancy, WorkingDirectoryStatusKind, WorkingDirectorySummary,
};

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

    #[test]
    fn workspace_workdir_projection_reexports_workspace_api_authority() {
        assert_eq!(
            std::any::TypeId::of::<WorkingDirectorySummary>(),
            std::any::TypeId::of::<workspace_api::WorkingDirectorySummary>()
        );
        assert_eq!(
            std::any::TypeId::of::<WorkingDirectoryOccupancy>(),
            std::any::TypeId::of::<workspace_api::WorkingDirectoryOccupancy>()
        );
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
