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
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<u64>,
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
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    pub materializer_kind: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<u64>,
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
            materializer_kind: self.materializer_kind.clone(),
            cleanup_target: self.cleanup_target.clone(),
        }
    }

    pub fn current_observation(&self) -> WorkingDirectoryCurrentObservation {
        WorkingDirectoryCurrentObservation {
            current_selector: self.current_selector.clone(),
            current_ref: self.current_ref.clone(),
            status: self.status.clone(),
            cleanliness: self.cleanliness.clone(),
            primary_worker_id: self.primary_worker_id,
            occupied_by: self.occupied_by.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryDiagnostic {
    pub code: String,
    pub severity: WorkingDirectoryDiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryListResponse {
    pub workspace_id: String,
    pub items: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<WorkingDirectoryDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryDetailResponse {
    pub workspace_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<WorkingDirectoryDiagnostic>,
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
    fn occupied_and_free_list_response_round_trips() {
        let response = WorkingDirectoryListResponse {
            workspace_id: "workspace".to_string(),
            items: vec![
                WorkingDirectorySummary {
                    working_directory_id: "occupied".to_string(),
                    repository_id: "repo".to_string(),
                    creation_selector: Some("develop".to_string()),
                    creation_ref: Some("abc123".to_string()),
                    current_selector: Some("work/ticket".to_string()),
                    current_ref: Some("def456".to_string()),
                    materializer_kind: MaterializerKind::LocalGitWorktree,
                    cleanup_target: Some(WorkingDirectoryCleanupTarget {
                        kind: "git_worktree".to_string(),
                        working_directory_id: "occupied".to_string(),
                        repository_id: "repo".to_string(),
                    }),
                    status: WorkingDirectoryStatusKind::Active,
                    cleanliness: Some("clean".to_string()),
                    primary_worker_id: None,
                    occupied_by: Some(WorkingDirectoryOccupancy {
                        worker: RuntimeWorkerRef::new("arcadia", "worker-opaque-64"),
                        display_name: "Coder".to_string(),
                        linked_at: "2026-08-12T00:00:00Z".to_string(),
                    }),
                },
                WorkingDirectorySummary {
                    working_directory_id: "free".to_string(),
                    repository_id: "repo".to_string(),
                    creation_selector: None,
                    creation_ref: None,
                    current_selector: None,
                    current_ref: Some("987fed".to_string()),
                    materializer_kind: MaterializerKind::LocalGitWorktree,
                    cleanup_target: None,
                    status: WorkingDirectoryStatusKind::Active,
                    cleanliness: Some("unknown".to_string()),
                    primary_worker_id: None,
                    occupied_by: None,
                },
            ],
            diagnostics: vec![WorkingDirectoryDiagnostic {
                code: "observed".to_string(),
                severity: WorkingDirectoryDiagnosticSeverity::Info,
                message: "inventory observed".to_string(),
            }],
        };

        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(
            encoded["items"][0]["occupied_by"]["worker_id"],
            "worker-opaque-64"
        );
        assert!(
            encoded["items"][0]["occupied_by"]
                .get("runtime_worker_id")
                .is_none()
        );
        assert!(encoded["items"][1].get("occupied_by").is_none());

        let mut stale = encoded.clone();
        stale["items"][0]["occupied_by"]["runtime_worker_id"] = serde_json::json!(64);
        assert!(serde_json::from_value::<WorkingDirectoryListResponse>(stale).is_err());

        let decoded: WorkingDirectoryListResponse = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, response);

        let detail = WorkingDirectoryDetailResponse {
            workspace_id: decoded.workspace_id.clone(),
            item: decoded.items[0].clone(),
            diagnostics: decoded.diagnostics.clone(),
        };
        let encoded = serde_json::to_value(&detail).unwrap();
        let decoded: WorkingDirectoryDetailResponse = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, detail);
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
