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
pub struct RepositorySource {
    pub kind: RepositorySourceKind,
    /// Canonical source representation. This is an absolute local path for
    /// `local_path`, and a normalized URI/remote specification otherwise.
    pub uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

pub const TICKET_RELATIONS_QUERY_PATH: &str = "/tickets/relations/search";
pub const TICKET_ORCHESTRATION_PLANS_QUERY_PATH: &str = "/tickets/orchestration-plans/search";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
