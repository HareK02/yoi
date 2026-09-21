//! Shared Workdir inventory domain contracts.
//!
//! These types are a neutral lower-level authority shared by Server and Runtime
//! transport projections. They intentionally do not expose provider/session
//! handles, host paths, Runtime URLs, or credentials.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryMaterializerKind {
    #[default]
    RuntimeGitClone,
    ClientHostedExternal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

impl std::fmt::Display for WorkingDirectoryStatusKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryOccupancy {
    pub runtime_id: String,
    pub worker_id: String,
    pub display_name: String,
    pub linked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectorySummary {
    pub working_directory_id: String,
    /// Optional human-facing label. Never used for routing or identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<RuntimeWorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkingDirectorySource {
    Repository { repository_key: String },
    ExternalGrant { grant_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    /// Optional human-facing label. Never used for attachment routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub source: WorkingDirectorySource,
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
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

impl WorkingDirectorySummary {
    pub fn is_workspace_managed(&self) -> bool {
        self.cleanup_target.is_some()
    }
}
pub use WorkingDirectoryMaterializerKind as MaterializerKind;

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
    fn working_directory_summary_preserves_server_projection_wire_shape() {
        let summary = WorkingDirectorySummary {
            working_directory_id: "wd-1".to_string(),
            display_name: Some("Checkout".to_string()),
            source: WorkingDirectorySource::Repository {
                repository_key: "repo-1".to_string(),
            },
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materializer_kind: WorkingDirectoryMaterializerKind::RuntimeGitClone,
            cleanup_target: None,
            status: WorkingDirectoryStatusKind::Active,
            cleanliness: None,
            occupied_by: None,
        };
        let value = serde_json::to_value(summary).unwrap();
        assert_eq!(value["working_directory_id"], "wd-1");
        assert_eq!(value["materializer_kind"], "runtime_git_clone");
        assert_eq!(value["status"], "active");
        assert!(value.get("cleanup_target").is_none());
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWorkdirSessionOperationRequest {
    /// Worker-local attachment alias selected by the calling tool router.
    pub target_workdir: String,
    pub operation: crate::http::WorkdirSessionOperation,
}
