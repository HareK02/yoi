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
    pub repository_id: String,
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

    fn round_trip<T>(value: T)
    where
        T: std::fmt::Debug + PartialEq + Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_vec(&value).expect("fixture should serialize");
        let decoded: T = serde_json::from_slice(&encoded).expect("fixture should deserialize");
        assert_eq!(decoded, value);
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
