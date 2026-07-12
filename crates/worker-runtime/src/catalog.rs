use crate::execution::WorkerExecutionStatus;
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::interaction::WorkerInput;
use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveRef};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Profile selector boundary. This is a selector, not a resolved runtime config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProfileSelector {
    RuntimeDefault,
    Builtin(String),
    Named(String),
}

impl Default for ProfileSelector {
    fn default() -> Self {
        Self::RuntimeDefault
    }
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializerKind {
    #[default]
    LocalGitWorktree,
}

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
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryStatusKind {
    Active,
    Removed,
    CleanupPending,
    Corrupted,
    NotFound,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_selector: Option<String>,
    pub materializer_kind: MaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    /// Backend projection metadata. Runtimes leave this absent; Workspace Browser
    /// APIs fill it with `backend_managed` or `runtime_unmanaged`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryStatus {
    pub summary: WorkingDirectorySummary,
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
    pub profile: ProfileSelector,
    pub profile_source: ProfileSourceArchiveSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<WorkerInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_request: Option<WorkingDirectoryRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectoryClaim>,
}

/// Worker lifecycle status for the in-memory embedded runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Running,
    Stopped,
    Cancelled,
}

impl WorkerStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Lightweight catalog row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSummary {
    pub worker_ref: WorkerRef,
    pub runtime_id: RuntimeId,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub execution: WorkerExecutionStatus,
    pub profile: ProfileSelector,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    pub last_event_id: u64,
}

/// Full Worker catalog/lifecycle detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDetail {
    pub worker_ref: WorkerRef,
    pub runtime_id: RuntimeId,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub execution: WorkerExecutionStatus,
    pub profile: ProfileSelector,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
    pub last_event_id: u64,
}

/// Acknowledgement returned by stop/cancel lifecycle operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLifecycleAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    pub event_id: u64,
}
