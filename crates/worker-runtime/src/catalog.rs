use crate::identity::{RuntimeWorkerRef, WorkerId, WorkerRef};
use crate::interaction::WorkerInput;
use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveRef};
use serde::{Deserialize, Serialize};

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

/// Profile source material available to a Runtime during Worker creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSourceArchiveSource {
    /// Backend-internal embedded runtimes may receive already-built archive bytes.
    Embedded { archive: ProfileSourceArchive },
    /// Standalone runtimes resolve this immutable archive from the latest
    /// Workspace Config bundle before creating the Worker.
    WorkspaceConfig { archive: ProfileSourceArchiveRef },
}

impl ProfileSourceArchiveSource {
    pub fn reference(&self) -> ProfileSourceArchiveRef {
        match self {
            Self::Embedded { archive } => archive.reference.clone(),
            Self::WorkspaceConfig { archive } => archive.clone(),
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
    pub source: server_api::RepositorySource,
    pub source_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<RepositorySelector>,
}

pub use workdir::workspace::{
    MaterializerKind, RuntimeWorkingDirectoryCleanupTarget as WorkingDirectoryCleanupTarget,
    RuntimeWorkingDirectorySummary as WorkingDirectorySummary, WorkingDirectoryCurrentObservation,
    WorkingDirectoryOccupancy, WorkingDirectoryProvenance, WorkingDirectoryStatusKind,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensitiveString(String);

impl SensitiveString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SensitiveString {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl Default for SensitiveString {
    fn default() -> Self {
        Self(String::new())
    }
}

impl std::fmt::Debug for SensitiveString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositorySshCredentialCandidate {
    pub credential_id: String,
    pub public_key_fingerprint: String,
    #[serde(skip, default)]
    pub private_key: SensitiveString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositorySshMaterializationAccess {
    pub credential_candidates: Vec<RepositorySshCredentialCandidate>,
    pub host_trust_id: String,
    pub host_key_fingerprint: String,
    pub access: server_api::RepositoryAccessMode,
    pub expires_at_epoch_seconds: u64,
    pub repository_id: String,
    pub repository_source_fingerprint: String,
    pub repository_uri: String,
    pub secret_resource: crate::resource::BackendResourceHandle,
    #[serde(skip, default)]
    pub known_hosts_entry: SensitiveString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryMaterializationContext {
    pub workspace_id: String,
    pub runtime_id: String,
    pub operation_id: String,
    pub config_projection_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<RepositorySshMaterializationAccess>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryRepositoryAccessRequest {
    pub working_directory_id: String,
    pub materialization: RepositoryMaterializationContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryRequest {
    pub repository: WorkingDirectoryRepository,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub materializer: MaterializerKind,
    /// Backend-assigned stable Workdir id. Runtimes use this when present so the
    /// Backend can create canonical registry rows before materialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_workdir_id: Option<String>,
    /// Backend-authored, operation-scoped repository access and cache identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<RepositoryMaterializationContext>,
}

/// Backend-authorized request to freshly resolve one Repository provider ref.
///
/// Runtime executes this against the registered source itself rather than a Workdir
/// or Runtime cache. Secret material is fetched through `materialization` and never
/// appears in the result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRefObservationRequest {
    pub repository: WorkingDirectoryRepository,
    pub selector: String,
    /// When present, require the provider's commit graph to contain this exact commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_ancestor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<RepositoryMaterializationContext>,
}

/// Provider-neutral proof of one freshly observed Repository ref.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRefObservation {
    pub repository_id: String,
    pub source_fingerprint: String,
    pub selector: String,
    pub resolved_ref: String,
    /// Present only after checking the freshly fetched provider graph, not local refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contained_commit: Option<String>,
    pub observed_at_epoch_seconds: u64,
}

/// Fixed publication input. Each execution requires current authority; this is not a replay key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPublicationBinding {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub operation_id: String,
    pub repository_id: String,
    pub repository_source_fingerprint: String,
    pub target: String,
    pub source: String,
    pub approval_event_id: String,
    pub owner_operation_id: String,
    pub worker: crate::identity::RuntimeWorkerRef,
    pub working_directory_id: String,
    pub connection_id: String,
    pub alias: workdir::WorkdirAttachmentAlias,
    pub target_before: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPublicationRequest {
    pub binding: RepositoryPublicationBinding,
    pub repository: WorkingDirectoryRepository,
    pub materialization: RepositoryMaterializationContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepositoryPublicationOutcome {
    Unknown {},
    NotPublished {
        reason: String,
    },
    Published {
        target_after: String,
        observed_at_epoch_seconds: u64,
    },
}

/// Ephemeral execution result. Unknown never authorizes an automatic retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPublicationResult {
    pub binding: RepositoryPublicationBinding,
    pub outcome: RepositoryPublicationOutcome,
}

/// Workspace-authoritative alias-to-Workdir mapping. This carries no claim that
/// the Worker Runtime materialized or can bind the referenced Workdir locally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalWorkdirAttachment {
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory_id: String,
    #[serde(default = "default_workdir_session_capabilities")]
    pub capabilities: workdir::WorkdirSessionCapabilities,
}

fn default_workdir_session_capabilities() -> workdir::WorkdirSessionCapabilities {
    // Capability-less wire and persisted records fail closed instead of
    // silently regaining write or command authority.
    workdir::WorkdirSessionCapabilities::READ_ONLY
}

pub use runtime_api::WorkdirAttachmentProvider;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryAttachmentClaim {
    #[serde(default, skip_serializing_if = "WorkdirAttachmentProvider::is_runtime")]
    pub provider: WorkdirAttachmentProvider,
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_cwd: Option<String>,
    /// Backend-authoritative maximum capabilities for the attachment session.
    #[serde(default = "default_workdir_session_capabilities")]
    pub capabilities: workdir::WorkdirSessionCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryAttachmentRequest {
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory: WorkingDirectoryRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryAttachmentStatus {
    pub alias: workdir::WorkdirAttachmentAlias,
    pub working_directory: WorkingDirectoryStatus,
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

pub use worker::BackendJobExecutionBinding;

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
// Legacy attachment bits cannot attest a saved identity. In particular, reject
// old true records explicitly instead of silently defaulting to no connection.
#[serde(deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachment_requests: Vec<WorkingDirectoryAttachmentRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkingDirectoryAttachmentClaim>,
    /// Backend-only enablement of the optional Runtime-local peer provider.
    /// Scoped Backend observation is composed by the Worker Feature, independently
    /// of this saved flag, and rechecks operation use plus current target control.
    #[serde(default, skip_serializing_if = "is_false")]
    pub worker_observation_enabled: bool,
    /// Backend-authored, bounded peer session grants. Runtime revalidates each
    /// requested capture against this exact canonical `(runtime_id, worker_id)` set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_observation_grants: Vec<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_api: Option<WorkspaceApiRef>,
    /// Backend-authored immutable settings for a host-bound Subjektiv attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_settings: Option<manifest::SubjektivMemorySettingsSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_execution: Option<runtime_api::PluginExecutionSnapshot>,
    /// Durable Feature-owned initialization expectations.
    #[serde(default)]
    pub feature_initialization: protocol::feature_initialization::FeatureInitializations,
    /// Backend-only immutable Job/attempt capability. Never inferred from Profile policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_job: Option<BackendJobExecutionBinding>,
}

impl CreateWorkerRequest {
    pub(crate) fn workspace_workdir_attachments(&self) -> Vec<LogicalWorkdirAttachment> {
        self.workdir_attachments
            .iter()
            .filter(|claim| claim.provider == WorkdirAttachmentProvider::Workspace)
            .map(|claim| LogicalWorkdirAttachment {
                alias: claim.alias.clone(),
                working_directory_id: claim.working_directory_id.clone(),
                capabilities: claim.capabilities,
            })
            .collect()
    }
}

/// Last persisted Worker lifecycle status.
///
/// This is not proof that the current Runtime process holds a live execution handle. Run
/// termination details remain separate Worker protocol state; in particular, cancellation
/// returns a Worker to `Idle` and is not a lifecycle state of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Idle,
    Running,
    Paused,
    Stopped,
}

impl WorkerStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Idle | Self::Running | Self::Paused)
    }
}

pub use protocol::WorkerLifecycleState;
pub use runtime_api::WorkerObservationState;

/// Lightweight persisted Worker identity projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_finished_submission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_observation_token: Option<String>,
    pub worker_ref: WorkerRef,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub lifecycle: WorkerLifecycleState,
    #[serde(default)]
    pub observation: WorkerObservationState,
    /// Creation timestamp in Unix epoch milliseconds for records created on this schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    /// Whether the persisted execution metadata was valid when this identity was loaded.
    pub execution_metadata_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_state: Option<protocol::WorkerStateSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
    pub profile: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub profile_source: ProfileSourceArchiveRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_bundle: Option<ConfigBundleRef>,
}

/// Full persisted Worker identity and lifecycle detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_finished_submission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_observation_token: Option<String>,
    pub worker_ref: WorkerRef,
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub lifecycle: WorkerLifecycleState,
    #[serde(default)]
    pub observation: WorkerObservationState,
    /// Creation timestamp in Unix epoch milliseconds for records created on this schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    /// Whether the persisted execution metadata was valid when this identity was loaded.
    pub execution_metadata_available: bool,
    /// Whether this Runtime process reconstructed the Worker from durable execution state.
    /// This proves that work from the prior process incarnation is no longer running.
    #[serde(default)]
    pub execution_reconstructed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_state: Option<protocol::WorkerStateSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
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
    /// Exact endpoint command evidence; the snapshot is not current-state authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<protocol::WorkerCommandResult>,
    /// Historical typed stop receipt; never authorizes cleanup of current resources.
    #[serde(default)]
    pub replayed: bool,
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    pub lifecycle: WorkerLifecycleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_state: Option<protocol::WorkerStateSnapshot>,
}

impl WorkerLifecycleAck {
    pub(crate) fn historical_stop_receipt(&self) -> Self {
        Self {
            replayed: true,
            ..self.clone()
        }
    }
}

/// Retained stop evidence, independent of the Worker's current execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerStopOutcome {
    NotFound,
    Pending,
    Interrupted,
    Failed {
        cause: protocol::control_failure::RuntimeControlCause,
    },
    Stopped {
        ack: WorkerLifecycleAck,
    },
}

#[cfg(test)]
mod tests {
    use super::WorkspaceApiRef;

    #[test]
    fn stop_receipt_without_replay_marker_is_marked_only_when_returned_as_history() {
        let value = serde_json::json!({
            "worker_ref": {"worker_id": crate::identity::WorkerId::now_v7()}, "status":"stopped", "lifecycle":"stopped"
        });
        let receipt: super::WorkerLifecycleAck = serde_json::from_value(value).unwrap();
        assert!(!receipt.replayed);
        let returned = receipt.historical_stop_receipt();
        assert!(returned.replayed);
        assert!(
            !receipt.replayed,
            "the retained receipt must not be rewritten"
        );
        let wire = serde_json::to_value(&returned).unwrap();
        let external: runtime_api::WorkerLifecycleAck =
            serde_json::from_value(wire.clone()).unwrap();
        assert!(external.replayed);
        assert_eq!(serde_json::to_value(external).unwrap(), wire);
    }

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

#[cfg(test)]
pub(crate) fn test_subject_initialization(
    workspace_id: &str,
    subject_id: &str,
) -> protocol::feature_initialization::FeatureInitializations {
    protocol::feature_initialization::FeatureInitializations::new(vec![
        protocol::feature_initialization::FeatureInitialization::encode(
            "builtin:subjektiv",
            1,
            &serde_json::json!({
                "workspace_id": workspace_id, "subject_id": subject_id, "kind": "subject_body"
            }),
        )
        .unwrap(),
    ])
    .unwrap()
}
