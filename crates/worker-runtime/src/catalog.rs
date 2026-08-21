use crate::identity::{RuntimeWorkerRef, WorkerId, WorkerRef};
use crate::interaction::WorkerInput;
use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveRef};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Profile selector boundary. This is a selector, not a resolved runtime config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProfileSelector {
    Builtin(String),
    Named(String),
}

/// Runtime fetch/caching metadata for a Backend-authored Decodal profile source archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSourceArchiveHttpRef {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub archive: ProfileSourceArchiveRef,
}

/// Profile source material available to a Runtime during Worker creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSourceArchiveSource {
    /// Backend-internal embedded runtimes may receive already-built archive bytes.
    Embedded { archive: ProfileSourceArchive },
    /// Standalone runtimes fetch/cache the tar archive over HTTP.
    Http {
        location: ProfileSourceArchiveHttpRef,
    },
}

impl ProfileSourceArchiveSource {
    pub fn reference(&self) -> ProfileSourceArchiveRef {
        match self {
            Self::Embedded { archive } => archive.reference.clone(),
            Self::Http { location } => location.archive.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleRef {
    pub id: String,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositorySelector(pub String);

impl From<&str> for RepositorySelector {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for RepositorySelector {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for RepositorySelector {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for RepositorySelector {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryRepository {
    pub id: String,
    pub provider: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<RepositorySelector>,
}

pub use workdir::workspace::{
    MaterializerKind, WorkingDirectoryCleanupTarget, WorkingDirectoryCurrentObservation,
    WorkingDirectoryOccupancy, WorkingDirectoryProvenance, WorkingDirectoryStatusKind,
    WorkingDirectorySummary,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryRequest {
    pub repository: WorkingDirectoryRepository,
    #[serde(default)]
    pub materializer: MaterializerKind,
    /// Backend-assigned stable Workdir id. Runtimes use this when present so the
    /// Backend can create canonical registry rows before materialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_workdir_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryClaim {
    pub working_directory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryStatus {
    pub summary: WorkingDirectorySummary,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceApiRef {
    pub workspace_id: String,
    pub base_url: String,
}

impl std::fmt::Debug for WorkspaceApiRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceApiRef")
            .field("workspace_id", &self.workspace_id)
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Canonical Runtime Worker creation request.
///
/// Browser/product launch semantics are resolved by a backend before this
/// request is built. The request contains only durable Runtime identity inputs:
/// a backend-decided profile selector, the Decodal profile source archive source
/// used to resolve that selector, optional initial user input committed as a
/// protocol observation event, and an optional Runtime-owned working directory
/// binding. Browser-facing status for materialized working directories is
/// summarized without exposing raw host paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkerRequest {
    /// Workspace-owned stable identity reserved before this request reaches a Runtime.
    pub worker_id: WorkerId,
    /// Canonical create-intent fingerprint bound to `worker_id` for retry recovery.
    pub create_fingerprint: String,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<WorkerInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_request: Option<WorkingDirectoryRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryClaim>,
    /// Backend-only feature enablement. Grants still define local Runtime peers;
    /// the Workspace provider reauthorizes its dynamic set per operation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub worker_observation_enabled: bool,
    /// Backend-authored, bounded peer session grants. Runtime revalidates each
    /// requested capture against this exact canonical `(runtime_id, worker_id)` set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_observation_grants: Vec<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_api: Option<WorkspaceApiRef>,
    /// Backend-authored immutable Workspace Memory settings snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_settings: Option<manifest::WorkspaceMemorySettingsSnapshot>,
}

/// Worker lifecycle status for the in-memory embedded runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Idle,
    Running,
    Paused,
    Stopped,
    Cancelled,
}

impl WorkerStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Idle | Self::Running | Self::Paused)
    }
}

/// Lightweight catalog row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSummary {
    pub worker_ref: WorkerRef,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryStatus>,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
}

/// Full Worker catalog/lifecycle detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDetail {
    pub worker_ref: WorkerRef,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryStatus>,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
}

/// Acknowledgement returned by stop/cancel lifecycle operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLifecycleAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
}

#[cfg(test)]
mod tests {
    use super::WorkspaceApiRef;

    #[test]
    fn workspace_api_ref_public_schema_contains_no_source_credentials_or_claim_choices() {
        let value = serde_json::to_value(WorkspaceApiRef {
            workspace_id: "workspace-a".to_string(),
            base_url: "https://server.invalid".to_string(),
        })
        .unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 2);
        assert!(object.contains_key("workspace_id"));
        assert!(object.contains_key("base_url"));
        for forbidden in [
            "runtime_id",
            "worker_id",
            "permission",
            "private_key",
            "bearer_token",
            "signing_handle",
        ] {
            assert!(!object.contains_key(forbidden), "unexpected {forbidden}");
        }
    }
}
