//! Shared Workspace HTTP resource contracts.
//!
//! This crate owns transport DTOs exposed by the Workspace Server and consumed
//! by Rust clients. Runtime-internal projections remain in their owning crates;
//! callers must explicitly construct these Workspace-authoritative resources.

use serde::{Deserialize, Serialize};
use workdir::workspace::WorkingDirectorySummary;

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
    pub owner_account_id: Option<String>,
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
    pub repository_id: String,
    pub name: String,
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
    pub id: String,
    pub display_name: String,
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
    pub repository_id: String,
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
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
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
pub struct WorkspaceWorkerDiscoveryItem {
    pub subject: WorkspaceWorkerSubject,
    pub resource_key: String,
    pub display_name: String,
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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
#[serde(deny_unknown_fields)]
pub struct RepositorySshCredential {
    pub credential_id: String,
    pub workspace_id: String,
    pub name: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    pub current_revision: u64,
    pub status: String,
    pub created_at: String,
    pub rotated_at: Option<String>,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[serde(deny_unknown_fields)]
pub struct RotateRepositorySshCredentialRequest {
    pub operation_id: String,
    pub expected_revision: u64,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshCredentialRequest {
    pub operation_id: String,
    pub expected_revision: u64,
}

/// Public metadata for an explicitly pinned SSH host key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostTrust {
    pub host_trust_id: String,
    pub workspace_id: String,
    pub hostname: String,
    pub port: u16,
    pub key_algorithm: String,
    pub host_key: String,
    pub fingerprint: String,
    pub current_revision: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PutRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub host_trust_id: String,
    pub hostname: String,
    pub port: u16,
    pub host_key: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositorySshAccessBinding {
    pub repository_id: String,
    pub credential_id: String,
    pub host_trust_id: String,
    pub access: RepositoryAccessMode,
}

/// Secret-free active Repository access projection consumed by later Runtime work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessProjection {
    pub workspace_id: String,
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

#[cfg(test)]
mod tests {
    use super::*;

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
                "id": "main",
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
        let parsed: RepositoryListResponse = serde_json::from_value(repositories.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), repositories);
    }

    #[test]
    fn repository_response_rejects_stale_field_aliases() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_id": "main",
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
        assert!(!output.contains("repository_id: string, display_name"));
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
}
