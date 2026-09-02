//! Shared Workspace HTTP resource contracts.
//!
//! This crate owns transport DTOs exposed by the Workspace Server and consumed
//! by Rust clients. Runtime-internal projections remain in their owning crates;
//! callers must explicitly construct these Workspace-authoritative resources.

use serde::{Deserialize, Serialize};

pub const REPOSITORY_KEY_MIN_LEN: usize = 1;
pub const REPOSITORY_KEY_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKeyError {
    Length,
    Character,
    LeadingHyphen,
    TrailingHyphen,
}

impl std::fmt::Display for RepositoryKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Length => "must contain between 1 and 64 ASCII bytes",
            Self::Character => "must contain only lowercase ASCII letters, digits, and hyphens",
            Self::LeadingHyphen => "must not start with a hyphen",
            Self::TrailingHyphen => "must not end with a hyphen",
        })
    }
}

impl std::error::Error for RepositoryKeyError {}

/// Validate one immutable Workspace-scoped Repository key.
///
/// Keys are deliberately not normalized: callers must submit the exact canonical
/// lowercase ASCII spelling so idempotency and route identity cannot alias.
pub fn validate_repository_key(value: &str) -> Result<(), RepositoryKeyError> {
    let bytes = value.as_bytes();
    if !(REPOSITORY_KEY_MIN_LEN..=REPOSITORY_KEY_MAX_LEN).contains(&bytes.len()) {
        return Err(RepositoryKeyError::Length);
    }
    if bytes[0] == b'-' {
        return Err(RepositoryKeyError::LeadingHyphen);
    }
    if bytes[bytes.len() - 1] == b'-' {
        return Err(RepositoryKeyError::TrailingHyphen);
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(RepositoryKeyError::Character);
    }
    Ok(())
}

/// Provider-neutral classification of an authoritative Repository source.
///
/// Local paths remain distinct from network Git transports so callers cannot
/// accidentally treat an unmaterialized remote as a server-local filesystem path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositorySourceKind {
    LocalPath,
    File,
    Ssh,
    Http,
    Https,
    /// A legacy value that could not be classified during migration. It remains
    /// inspectable but every provider operation must fail closed.
    Invalid,
}

impl RepositorySourceKind {
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Ssh | Self::Http | Self::Https)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local_path",
            Self::File => "file",
            Self::Ssh => "ssh",
            Self::Http => "http",
            Self::Https => "https",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local_path" => Self::LocalPath,
            "file" => Self::File,
            "ssh" => Self::Ssh,
            "http" => Self::Http,
            "https" => Self::Https,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// Stable Repository source identity stored by Workspace authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RepositorySource {
    pub kind: RepositorySourceKind,
    /// Canonical source representation. This is an absolute local path for
    /// `local_path`, and a normalized URI/remote specification otherwise.
    pub uri: String,
}

/// Browser/user intent for registering one Repository in a Workspace.
///
/// The Server parses and canonicalizes `source`; callers cannot assert a
/// transport classification or supply credential material through this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRepositoryRequest {
    pub repository_key: String,
    pub source: String,
    #[serde(default)]
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateWorkspaceRepositoryResponse {
    pub workspace_id: String,
    pub repository_key: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryObservedStatus {
    Unverified,
    Ready,
    Invalid,
}

impl RepositoryObservedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Ready => "ready",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "unverified" => Self::Unverified,
            "ready" => Self::Ready,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// Public Workspace catalog item returned by `GET /api/workspaces`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    pub owner_account_id: String,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Public response returned by `GET /api/workspaces`.
///
/// The transparent newtype keeps the established top-level JSON array while making the
/// complete list response a named cross-crate and generated-TypeScript authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceCatalogListResponse(pub Vec<WorkspaceSummary>);

/// Public Repository record embedded in Workspace creation responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRepositoryRecord {
    pub workspace_id: String,
    pub repository_key: String,
    pub kind: String,
    pub provider: Option<String>,
    pub source: RepositorySource,
    pub default_ref: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    pub observed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response returned after atomically creating a Workspace and its first Repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateResponse {
    pub workspace: WorkspaceSummary,
    pub repository: WorkspaceRepositoryRecord,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub config_revision: u64,
    pub request_fingerprint: String,
    pub replayed: bool,
}

/// Browser authentication configuration exposed by the scoped Workspace summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum WorkspaceAuthConfig {
    Passkey {
        rp_id: String,
        origin: String,
        public_base_url: String,
        cookie_name: String,
    },
}

/// Backend-authoritative permissions for the current Workspace actor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspacePermissionSummary {
    pub manage_repositories: bool,
    pub manage_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPointState {
    pub status: String,
    pub note: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPoints {
    pub store: String,
    pub event_stream: WorkspaceExtensionPointState,
    pub host_worker_bridge: WorkspaceExtensionPointState,
    pub companion_console: WorkspaceExtensionPointState,
}

/// Scoped Workspace metadata and current-actor permission projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub record_authority: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub schema_version: i64,
    pub auth: WorkspaceAuthConfig,
    pub permissions: WorkspacePermissionSummary,
    pub extension_points: WorkspaceExtensionPoints,
}

/// Workspace identity metadata exposed by the current settings resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataSettingsResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub created_at: String,
    pub revision: String,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compare-and-swap update for Workspace identity display metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMetadataRequest {
    pub display_name: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataMutationResponse {
    pub workspace: WorkspaceMetadataSettingsResponse,
    pub diagnostics: Vec<Diagnostic>,
}

/// Read-only Profile catalog projected from one active Workspace config revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct ProfileSettingsResponse {
    pub workspace_id: String,
    pub registry_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub config_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkspaceProfileSummary>,
    pub sources: Vec<WorkspaceProfileSourceSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSummary {
    pub profile_id: String,
    pub selector: String,
    pub label: String,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub editable: bool,
    pub is_default: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSourceSummary {
    pub profile_source_id: String,
    pub display_path: String,
    pub kind: String,
    pub content_type: String,
    pub content_digest: String,
    pub provenance: WorkspaceProfileSourceProvenance,
    pub editable: bool,
    pub revision: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub size_bytes: u64,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceProfileSourceProvenance {
    ProjectProfileSourceTree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDiagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRemoteSummary {
    pub name: String,
    pub fetch_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRepositorySummary {
    pub status: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub remotes: Vec<GitRemoteSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySummary {
    pub repository_key: String,
    pub kind: String,
    pub provider: String,
    pub source: RepositorySource,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub record_authority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub git: Option<GitRepositorySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub diagnostics: Option<Vec<RepositoryDiagnostic>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitCommitSummary {
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryListResponse {
    pub workspace_id: String,
    pub items: Vec<RepositorySummary>,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDetailResponse {
    pub workspace_id: String,
    pub item: RepositorySummary,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryLogResponse {
    pub workspace_id: String,
    pub repository_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub limit: usize,
    pub items: Vec<GitCommitSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

pub const TICKET_RELATIONS_QUERY_PATH: &str = "/tickets/relations/search";
pub const TICKET_ORCHESTRATION_PLANS_QUERY_PATH: &str = "/tickets/orchestration-plans/search";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

/// Public Workdir materializer classification.
///
/// The value identifies stable materialization provenance without exposing a
/// provider path, Runtime handle, or session identity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryMaterializerKind {
    #[default]
    RuntimeGitCache,
    LocalGitWorktree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalRequest {
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryRemovalDisposition {
    Removed,
    Retained,
    AttentionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalResponse {
    pub working_directory_id: String,
    pub disposition: WorkingDirectoryRemovalDisposition,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
}

/// Durable Workspace occupancy projection for one Workdir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryOccupancy {
    pub runtime_id: String,
    pub worker_id: String,
    pub display_name: String,
    pub linked_at: String,
}

/// Runtime-internal Workdir cleanup authority. This transport intentionally
/// retains the Backend-generated Repository id and is never a Workspace public
/// projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

/// Runtime-internal Workdir inventory transport. Workspace REST and model-facing
/// surfaces must project this through [`WorkingDirectorySummary`] so the UUID is
/// replaced with `repository_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectorySummary {
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
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<RuntimeWorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

/// Public, provider-neutral Workdir inventory projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    pub repository_key: String,
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
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: WorkingDirectoryMaterializerKind,
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
}

/// Browser/Rust-client Workdir materialization request.
///
/// `runtime_id = None` requests Workspace default Runtime resolution and
/// `operation_id = Some(_)` fences exact replay. All four fields deliberately
/// preserve the Server's existing optionality.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    pub repository_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryListResponse {
    pub workspace_id: String,
    pub items: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryDetailResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct QueryPage {
    pub limit: usize,
    pub returned: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub sort: String,
    pub source_limit: Option<usize>,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveEventDetail {
    pub event_ref: String,
    pub kind: String,
    pub body: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveLinkedTicketSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveResourceSummary {
    pub path: String,
    pub media_type: Option<String>,
    pub bytes: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub summary: String,
    pub linked_tickets: Vec<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveDetail {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub revision: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub linked_tickets: Vec<String>,
    pub linked_ticket_summaries: Vec<ObjectiveLinkedTicketSummary>,
    pub resources: Vec<ObjectiveResourceSummary>,
    pub body: String,
    pub body_truncated: bool,
    pub events: Vec<ObjectiveEventDetail>,
    pub event_page: QueryPage,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveCreateRequest {
    pub title: String,
    #[serde(default)]
    pub body_md: String,
    #[serde(default = "default_objective_state")]
    pub state: String,
    #[serde(default)]
    pub linked_tickets: Vec<String>,
}

fn default_objective_state() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ObjectiveEditRequest {
    pub title: Option<String>,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveStateRequest {
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveLinkTicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceKind {
    EmbeddedWorkerRuntime,
    RemoteHttp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIdentityAuthority {
    RuntimeRegistryProjection,
    ServerRuntimeConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSourceSummary {
    pub kind: RuntimeSourceKind,
    pub status: RuntimeSourceStatus,
    pub identity_authority: RuntimeIdentityAuthority,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub source: RuntimeSourceSummary,
    #[serde(default)]
    pub host_ids: Vec<String>,
    pub worker_creation_available: bool,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeManagementSummary {
    pub built_in: bool,
    pub config_managed: bool,
    pub removable: bool,
    pub endpoint_configured: bool,
    pub token_ref_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRuntimeResource {
    #[serde(flatten)]
    pub runtime: RuntimeSummary,
    pub management: RuntimeManagementSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateRemoteRuntimeRequest {
    pub runtime_id: String,
    pub display_name: Option<String>,
    pub endpoint: String,
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConnectionTestResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub checked_at: String,
    pub state: String,
    pub protocol_version: Option<String>,
    pub compatibility_basis: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub health_result: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerImplementationSummary {
    pub kind: String,
    pub display_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCapabilitySummary {
    pub can_stop: bool,
    pub can_spawn_followup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceWorkerSubject {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
}

/// Bounded, model-safe projection used by privileged Workspace Worker discovery.
/// Runtime placement appears only in the typed subject required by Worker
/// control operations; provider and launch internals are intentionally omitted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceWorkerDiscoveryItem {
    pub subject: WorkspaceWorkerSubject,
    pub resource_key: String,
    pub display_name: String,
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Public lifecycle projection for the Workspace Companion endpoint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionLifecycleState {
    Idle,
    Running,
    Stopped,
}

/// Public outcome of a Companion message submission.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionMessageDisposition {
    Accepted,
    Rejected,
}

/// Public, bounded transport metadata for Companion status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTransportSummary {
    pub mode: String,
    pub available: bool,
}

/// Public Workspace Companion status response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionStatusResponse {
    pub state: CompanionLifecycleState,
    pub worker: Option<WorkspaceWorkerDiscoveryItem>,
    pub transport: CompanionTransportSummary,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Public Workspace Companion message request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageRequest {
    pub content: String,
}

/// Public Workspace Companion cancellation request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionCancelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Public Workspace Companion message response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageResponse {
    pub state: CompanionMessageDisposition,
    pub message: String,
}

/// User-visible role accepted in the public Companion transcript.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionTranscriptRole {
    User,
    Assistant,
}

/// One allowlisted, user-visible Companion transcript item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptItem {
    pub sequence: usize,
    pub role: CompanionTranscriptRole,
    pub content: String,
    pub created_at: String,
}

/// Bounded public Companion transcript projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptProjection {
    pub state: CompanionLifecycleState,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub next: Option<usize>,
    pub items: Vec<CompanionTranscriptItem>,
}

#[cfg(feature = "typescript")]
pub fn companion_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceWorkerSubject::decl(&config),
        WorkspaceWorkerDiscoveryItem::decl(&config),
        CompanionLifecycleState::decl(&config),
        CompanionMessageDisposition::decl(&config),
        CompanionTransportSummary::decl(&config),
        CompanionStatusResponse::decl(&config),
        CompanionMessageRequest::decl(&config),
        CompanionCancelRequest::decl(&config),
        CompanionMessageResponse::decl(&config),
        CompanionTranscriptRole::decl(&config),
        CompanionTranscriptItem::decl(&config),
        CompanionTranscriptProjection::decl(&config),
    ];

    format!(
        "// Generated by `cargo run -p workspace-api --features typescript --example generate_companion_api_types`.\n// Do not edit manually.\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWorkerDiscoveryPage {
    pub workers: Vec<WorkspaceWorkerDiscoveryItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Workspace-authoritative Worker projection.
///
/// `resource_key` is required here even though Runtime-internal Worker summaries
/// do not carry one. The Workspace Server must resolve it from Workspace
/// authority before constructing this response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub resource_key: String,
    pub host_id: String,
    #[serde(default)]
    pub display_name: String,
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    pub state: String,
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectorySummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationState {
    Accepted,
    Unsupported,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRestoreResult {
    pub state: WorkerOperationState,
    pub worker: Option<WorkerSummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRestoreResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub result: WorkerRestoreResult,
}

/// Workspace-owned Memory settings returned by the shared Server API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemorySettings {
    pub workspace_id: String,
    pub settings_revision: u64,
    pub language: String,
}

/// Compare-and-swap update for Workspace-owned Memory settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMemorySettingsRequest {
    pub expected_revision: u64,
    pub language: String,
}

/// Public metadata for one Workspace-scoped Repository SSH credential.
///
/// Secret references and secret material are deliberately not part of this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshCredential {
    pub credential_id: String,
    pub workspace_id: String,
    pub name: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub current_revision: u64,
    pub status: String,
    pub created_at: String,
    pub rotated_at: Option<String>,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateRepositorySshCredentialRequest {
    pub operation_id: String,
    pub credential_id: String,
    pub name: String,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RotateRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
}

/// Public metadata for an explicitly pinned SSH host key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostTrust {
    pub host_trust_id: String,
    pub workspace_id: String,
    pub hostname: String,
    pub port: u16,
    pub key_algorithm: String,
    pub host_key: String,
    pub fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub current_revision: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PutRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub host_trust_id: String,
    pub hostname: String,
    pub port: u16,
    pub host_key: String,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshHostTrustRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshAccessBinding {
    pub repository_key: String,
    pub credential_id: String,
    pub host_trust_id: String,
    pub access: RepositoryAccessMode,
}

/// Secret-free active Repository access projection consumed by later Runtime work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessProjection {
    pub workspace_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub config_revision: u64,
    pub projection_digest: String,
    pub bindings: Vec<RepositorySshAccessBinding>,
}

#[cfg(feature = "typescript")]
pub fn catalog_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        WorkspaceSummary::decl(&config),
        WorkspaceCatalogListResponse::decl(&config),
        WorkspaceRepositoryRecord::decl(&config),
        WorkspaceCreateResponse::decl(&config),
        WorkspaceAuthConfig::decl(&config),
        WorkspacePermissionSummary::decl(&config),
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceExtensionPointState::decl(&config),
        WorkspaceExtensionPoints::decl(&config),
        WorkspaceResponse::decl(&config),
        WorkspaceMetadataSettingsResponse::decl(&config),
        UpdateWorkspaceMetadataRequest::decl(&config),
        WorkspaceMetadataMutationResponse::decl(&config),
        ProfileSettingsResponse::decl(&config),
        WorkspaceProfileSummary::decl(&config),
        WorkspaceProfileSourceSummary::decl(&config),
        WorkspaceProfileSourceProvenance::decl(&config),
        RepositorySourceKind::decl(&config),
        RepositorySource::decl(&config),
        RepositoryObservedStatus::decl(&config),
        RepositoryDiagnostic::decl(&config),
        GitRemoteSummary::decl(&config),
        GitRepositorySummary::decl(&config),
        RepositorySummary::decl(&config),
        GitCommitSummary::decl(&config),
        RepositoryListResponse::decl(&config),
        RepositoryDetailResponse::decl(&config),
        RepositoryLogResponse::decl(&config),
    ]
    .map(|declaration| format!("export {declaration}"));

    format!(
        "// This file is generated by `cargo run -p workspace-api --features typescript --example generate_typescript | deno fmt -`.\n// Do not edit this file directly.\n\n{}\n",
        declarations.join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn repository_access_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        RepositorySshCredential::decl(&config),
        CreateRepositorySshCredentialRequest::decl(&config),
        RotateRepositorySshCredentialRequest::decl(&config),
        DeleteRepositorySshCredentialRequest::decl(&config),
        RepositorySshHostTrust::decl(&config),
        PutRepositorySshHostTrustRequest::decl(&config),
        DeleteRepositorySshHostTrustRequest::decl(&config),
        RepositoryAccessMode::decl(&config),
        RepositorySshAccessBinding::decl(&config),
        RepositoryAccessProjection::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn workdir_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkingDirectoryMaterializerKind::decl(&config),
        WorkingDirectoryStatusKind::decl(&config),
        WorkingDirectoryCleanupTarget::decl(&config),
        WorkingDirectoryOccupancy::decl(&config),
        WorkingDirectorySummary::decl(&config),
        WorkingDirectoryCreateRequest::decl(&config),
        WorkingDirectoryListResponse::decl(&config),
        WorkingDirectoryDetailResponse::decl(&config),
        WorkingDirectoryCreateResponse::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(all(test, feature = "typescript"))]
mod workdir_typescript_tests {
    #[test]
    fn generated_workdir_api_contract_is_current() {
        let expected = super::workdir_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/workdir-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Workdir API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                '\r' | '\n' | ' ' | '\t' => None,
                _ => Some(character),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_key_validation_is_canonical_and_bounded() {
        let max = "a".repeat(64);
        for valid in ["a", "main", "repo-42", max.as_str()] {
            assert_eq!(validate_repository_key(valid), Ok(()), "{valid}");
        }
        let too_long = "a".repeat(65);
        for invalid in [
            "",
            "-main",
            "main-",
            "Main",
            "main_repo",
            "main.repo",
            "日本語",
            too_long.as_str(),
        ] {
            assert!(validate_repository_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn workspace_and_repository_response_shapes_round_trip() {
        let workspace = serde_json::json!({
            "workspace_id": "workspace-test",
            "display_name": "Test",
            "record_authority": "workspace-control-plane",
            "schema_version": 46,
            "auth": {"Passkey": {
                "rp_id": "example.test",
                "origin": "https://example.test",
                "public_base_url": "https://example.test",
                "cookie_name": "yoi_session"
            }},
            "permissions": {
                "manage_repositories": true,
                "manage_secrets": true
            },
            "extension_points": {
                "store": "sqlite",
                "event_stream": {"status": "available", "note": "ready", "diagnostics": []},
                "host_worker_bridge": {"status": "available", "note": "ready", "diagnostics": []},
                "companion_console": {"status": "available", "note": "ready", "diagnostics": []}
            }
        });
        let parsed: WorkspaceResponse = serde_json::from_value(workspace.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), workspace);

        let catalog = serde_json::json!([{
            "workspace_id": "workspace-test",
            "owner_account_id": "user-test",
            "display_name": "Test",
            "state": "active",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }]);
        let parsed: WorkspaceCatalogListResponse = serde_json::from_value(catalog.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), catalog);

        let repositories = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });
        let parsed: RepositoryListResponse = serde_json::from_value(repositories.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), repositories);
    }

    #[test]
    fn repository_response_rejects_stale_field_aliases() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "display_name": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });

        assert!(serde_json::from_value::<RepositoryListResponse>(stale).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_catalog_typescript_keeps_public_wrappers_and_nullability() {
        let output = catalog_typescript();
        assert!(
            output.contains("export type WorkspaceCatalogListResponse = Array<WorkspaceSummary>")
        );
        assert!(output.contains("export type WorkspaceResponse ="));
        assert!(output.contains("permissions: WorkspacePermissionSummary"));
        assert!(output.contains("export type RepositoryListResponse ="));
        assert!(output.contains("items: Array<RepositorySummary>"));
        assert!(output.contains("observed_at?: string | null"));
        assert!(output.contains("export type WorkspaceMetadataSettingsResponse ="));
        assert!(output.contains("export type WorkspaceMetadataMutationResponse ="));
        assert!(output.contains("export type ProfileSettingsResponse ="));
        assert!(output.contains("config_revision?: number | null"));
        assert!(output.contains("provenance: WorkspaceProfileSourceProvenance"));
        assert!(output.contains(
            "export type WorkspaceProfileSourceProvenance = \"project_profile_source_tree\""
        ));
        assert!(!output.contains("repository_key: string, display_name"));
    }

    #[test]
    fn worker_resource_key_is_required() {
        let payload = serde_json::json!({
            "runtime_id": "arcadia",
            "worker_id": "worker-1",
            "host_id": "host",
            "display_name": "Coder",
            "label": "Coder",
            "workspace": {
                "visibility": "workspace",
                "identity": "workspace-test"
            },
            "state": "idle",
            "implementation": {"kind": "worker", "display_hint": "Coder"},
            "capabilities": {"can_stop": true, "can_spawn_followup": false}
        });

        assert!(serde_json::from_value::<WorkerSummary>(payload).is_err());
    }

    fn round_trip<T>(value: T)
    where
        T: std::fmt::Debug + PartialEq + Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_vec(&value).expect("fixture should serialize");
        let decoded: T = serde_json::from_slice(&encoded).expect("fixture should deserialize");
        assert_eq!(decoded, value);
    }

    #[test]
    fn workspace_metadata_and_profile_projection_fixtures_round_trip() {
        let diagnostic = Diagnostic {
            code: "profile_projection_warning".to_string(),
            severity: DiagnosticSeverity::Warning,
            message: "projected from the active config revision".to_string(),
        };
        let metadata = WorkspaceMetadataSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            display_name: "Test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            revision: "sha256:metadata".to_string(),
            source: "workspace-config".to_string(),
            diagnostics: vec![diagnostic.clone()],
        };
        round_trip(metadata.clone());
        round_trip(UpdateWorkspaceMetadataRequest {
            display_name: "Renamed".to_string(),
            revision: metadata.revision.clone(),
        });
        round_trip(WorkspaceMetadataMutationResponse {
            workspace: metadata,
            diagnostics: vec![],
        });

        round_trip(ProfileSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            registry_revision: "config-source:7:sha256:tree:sha256:projection".to_string(),
            config_revision: Some(7),
            tree_digest: Some("sha256:tree".to_string()),
            projection_digest: Some("sha256:projection".to_string()),
            default_profile: Some("workspace:coder".to_string()),
            profiles: vec![WorkspaceProfileSummary {
                profile_id: "workspace:coder".to_string(),
                selector: "workspace:coder".to_string(),
                label: "Coder".to_string(),
                source_kind: "project".to_string(),
                profile_source_id: Some("profile-source-1".to_string()),
                description: None,
                editable: true,
                is_default: true,
                diagnostics: vec![diagnostic.clone()],
            }],
            sources: vec![WorkspaceProfileSourceSummary {
                profile_source_id: "profile-source-1".to_string(),
                display_path: "profiles/coder.dcdl".to_string(),
                kind: "profile".to_string(),
                content_type: "text/x-decodal".to_string(),
                content_digest: "sha256:source".to_string(),
                provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
                editable: false,
                revision: "config-source:7".to_string(),
                size_bytes: 128,
                diagnostics: vec![],
            }],
            diagnostics: vec![diagnostic],
        });

        let absent_optional_fields = serde_json::json!({
            "workspace_id": "workspace-test",
            "registry_revision": "builtin",
            "profiles": [],
            "sources": [],
            "diagnostics": []
        });
        let decoded: ProfileSettingsResponse =
            serde_json::from_value(absent_optional_fields.clone()).unwrap();
        assert_eq!(decoded.config_revision, None);
        assert_eq!(decoded.tree_digest, None);
        assert_eq!(decoded.projection_digest, None);
        assert_eq!(decoded.default_profile, None);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            absent_optional_fields
        );
    }

    fn companion_worker() -> WorkspaceWorkerDiscoveryItem {
        WorkspaceWorkerDiscoveryItem {
            subject: WorkspaceWorkerSubject::RuntimeWorker {
                runtime_id: "arcadia".to_string(),
                worker_id: "worker-7".to_string(),
            },
            resource_key: "W-7".to_string(),
            display_name: "Companion".to_string(),
            profile: Some("builtin:companion".to_string()),
            status: Some("idle".to_string()),
        }
    }

    #[test]
    fn companion_status_fixtures_round_trip() {
        for state in [
            CompanionLifecycleState::Idle,
            CompanionLifecycleState::Running,
            CompanionLifecycleState::Stopped,
        ] {
            round_trip(CompanionStatusResponse {
                state,
                worker: Some(companion_worker()),
                transport: CompanionTransportSummary {
                    mode: "worker_runtime".to_string(),
                    available: state != CompanionLifecycleState::Stopped,
                },
                diagnostics: Vec::new(),
            });
        }
    }

    #[test]
    fn companion_message_fixtures_round_trip() {
        for state in [
            CompanionMessageDisposition::Accepted,
            CompanionMessageDisposition::Rejected,
        ] {
            round_trip(CompanionMessageResponse {
                state,
                message: if state == CompanionMessageDisposition::Accepted {
                    "accepted"
                } else {
                    "rejected"
                }
                .to_string(),
            });
        }
    }

    #[test]
    fn companion_transcript_fixture_round_trips() {
        round_trip(CompanionTranscriptProjection {
            state: CompanionLifecycleState::Idle,
            start: 0,
            limit: 2,
            total: 2,
            next: None,
            items: vec![
                CompanionTranscriptItem {
                    sequence: 1,
                    role: CompanionTranscriptRole::User,
                    content: "hello".to_string(),
                    created_at: "2026-08-31T00:00:00Z".to_string(),
                },
                CompanionTranscriptItem {
                    sequence: 2,
                    role: CompanionTranscriptRole::Assistant,
                    content: "hi".to_string(),
                    created_at: "2026-08-31T00:00:01Z".to_string(),
                },
            ],
        });
    }

    #[test]
    fn companion_transcript_rejects_system_and_private_fields() {
        let public_item = CompanionTranscriptItem {
            sequence: 1,
            role: CompanionTranscriptRole::Assistant,
            content: "visible".to_string(),
            created_at: "2026-08-31T00:00:00Z".to_string(),
        };
        let public_fields = serde_json::to_value(public_item)
            .expect("public transcript item should serialize")
            .as_object()
            .expect("public transcript item should be an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            public_fields,
            ["content", "created_at", "role", "sequence"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );

        let system_item = serde_json::json!({
            "sequence": 1,
            "role": "system",
            "content": "raw system prompt",
            "created_at": "2026-08-31T00:00:00Z"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(system_item).is_err());

        let private_item = serde_json::json!({
            "sequence": 1,
            "role": "assistant",
            "content": "visible",
            "created_at": "2026-08-31T00:00:00Z",
            "reasoning": "hidden",
            "credential": "secret",
            "provider_session_id": "session-private"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(private_item).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_companion_api_contract_is_current() {
        let expected = companion_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/companion-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize_typescript(&actual),
            normalize_typescript(&expected),
            "regenerate Companion API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_companion_api_types > web/workspace/src/lib/generated/companion-api.ts` and format the generated file",
        );
    }

    #[cfg(feature = "typescript")]
    fn normalize_typescript(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace(";}", "}")
    }

    #[test]
    fn workdir_create_request_preserves_optional_operation_fields() {
        let payload = serde_json::json!({"repository_key": "main"});
        let request = serde_json::from_value::<WorkingDirectoryCreateRequest>(payload)
            .expect("optional create fields may be absent");

        assert_eq!(request.runtime_id, None);
        assert_eq!(request.selector, None);
        assert_eq!(request.operation_id, None);

        let serialized = serde_json::to_value(request).expect("serialize create request");
        assert_eq!(serialized, serde_json::json!({"repository_key": "main"}));
    }

    #[test]
    fn workdir_create_request_rejects_stale_or_incomplete_json() {
        let stale = serde_json::json!({
            "repository_key": "main",
            "selector": "develop",
            "path": "/tmp/workdir"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(stale).is_err());

        let incomplete = serde_json::json!({
            "runtime_id": "arcadia",
            "operation_id": "operation-1"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(incomplete).is_err());
    }

    #[test]
    fn workdir_summary_omits_absent_optional_fields_on_the_wire() {
        let value = serde_json::to_value(WorkingDirectorySummary {
            working_directory_id: "workdir-1".into(),
            repository_key: "main".into(),
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materializer_kind: WorkingDirectoryMaterializerKind::RuntimeGitCache,
            cleanup_target: None,
            status: WorkingDirectoryStatusKind::Active,
            cleanliness: None,
            primary_worker_id: None,
            occupied_by: None,
        })
        .expect("serialize Workdir summary");
        let object = value.as_object().expect("Workdir summary object");

        for key in [
            "creation_selector",
            "creation_ref",
            "creation_tree",
            "current_selector",
            "current_ref",
            "current_tree",
            "observed_at_epoch_seconds",
            "cleanup_target",
            "cleanliness",
            "primary_worker_id",
            "occupied_by",
        ] {
            assert!(
                !object.contains_key(key),
                "absent field {key} must be omitted"
            );
        }
    }

    #[test]
    fn workdir_response_rejects_stale_occupancy_shape() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "working_directory_id": "workdir-1",
                "repository_key": "main",
                "materializer_kind": "runtime_git_cache",
                "status": "active",
                "occupied_by": {
                    "runtime_worker_id": "worker-1",
                    "display_name": "Coder",
                    "linked_at": "2026-01-01T00:00:00Z"
                }
            }],
            "diagnostics": []
        });

        assert!(serde_json::from_value::<WorkingDirectoryListResponse>(stale).is_err());
    }
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn generated_repository_access_contract_is_current() {
        let expected = super::repository_access_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/repository-access-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Repository Access TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts` and format the generated file",
        );
    }

    #[test]
    fn generated_repository_access_responses_remain_secret_free() {
        use ts_rs::TS;

        let config = ts_rs::Config::default();
        for declaration in [
            super::RepositorySshCredential::decl(&config),
            super::RepositorySshHostTrust::decl(&config),
            super::RepositoryAccessProjection::decl(&config),
        ] {
            for forbidden in ["private_key", "passphrase", "secret_ref"] {
                assert!(
                    !declaration.contains(forbidden),
                    "Repository Access response declaration must not expose `{forbidden}`"
                );
            }
        }
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect()
    }
}
