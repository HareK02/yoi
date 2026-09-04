use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use flow::{CompiledFlowDefinition, FlowSourceKind, compile_flow_source};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use worker_runtime::identity::{RuntimeWorkerRef, WorkerId};
use workspace_api::{RepositoryObservedStatus, RepositorySource};

use crate::{Error, Result};

const LATEST_SCHEMA_VERSION: i64 = 50;

const MIGRATIONS: &[Migration] = &[Migration {
    version: LATEST_SCHEMA_VERSION,
    name: "workspace schema baseline",
    apply: create_latest_workspace_schema,
}];

struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    /// Existing user Account that owns this Workspace. Owner transfer is a separate
    /// audited domain operation; ordinary upserts must preserve this identity.
    pub owner_account_id: String,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMemorySettingsRecord {
    pub workspace_id: String,
    pub settings_revision: u64,
    pub language: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCreateReservation {
    pub worker_id: WorkerId,
    pub create_fingerprint: String,
    pub memory_settings: manifest::WorkspaceMemorySettingsSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRecord {
    pub workspace_id: String,
    pub repository_id: String,
    pub repository_key: String,
    pub kind: String,
    pub provider: Option<String>,
    pub source: RepositorySource,
    pub default_ref: Option<String>,
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    pub observed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryInsertOutcome {
    Created,
    Existing(RepositoryRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBootstrapRecord {
    pub operation_key: String,
    pub request_fingerprint: String,
    pub workspace: WorkspaceRecord,
    pub repository: RepositoryRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBootstrapResult {
    pub workspace: WorkspaceRecord,
    pub repository: RepositoryRecord,
    pub config_revision: u64,
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedRuntimeRecord {
    pub runtime_id: String,
    pub workspace_id: Option<String>,
    pub display_name: String,
    pub base_url: String,
    pub public_key: String,
    pub created_at: String,
    pub updated_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountRecord {
    pub account_id: String,
    pub kind: String,
    pub handle: String,
    pub display_name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRecord {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyCredentialRecord {
    pub credential_id: String,
    pub user_id: String,
    pub public_key_cose: String,
    pub transports_json: Option<String>,
    pub sign_count: u64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthChallengeRecord {
    pub challenge_id: String,
    pub ceremony: String,
    pub challenge: String,
    pub user_id: Option<String>,
    pub rp_id: String,
    pub origin: String,
    pub state_json: Option<String>,
    pub expires_at: String,
    pub created_at: String,
    pub consumed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSessionRecord {
    pub session_id: String,
    pub token_hash: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiTokenRecord {
    pub token_id: String,
    pub token_hash: String,
    pub user_id: String,
    pub label: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceLoginFlowRecord {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub client_name: Option<String>,
    pub user_id: Option<String>,
    pub api_token_id: Option<String>,
    pub issued_access_token: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub approved_at: Option<String>,
    pub consumed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRegistryRecord {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub display_name: String,
    pub profile: Option<String>,
    /// Retention state is explicit so `pinned` can be represented before prune exists.
    pub retention_state: String,
    pub transcript_ref: Option<String>,
    pub session_ref: Option<String>,
    pub summary_ref: Option<String>,
    pub diagnostics_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Durable authority describing which Runtime Worker another Runtime Worker may
/// discover and control. Revoked grants remain as audit evidence but are never
/// returned by active-grant queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerControlGrantRecord {
    pub workspace_id: String,
    pub grant_id: String,
    pub controller: RuntimeWorkerRef,
    pub subject: RuntimeWorkerRef,
    pub relation: String,
    pub origin: String,
    pub permissions: Vec<String>,
    pub operation_id: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TicketAssignmentRole {
    Orchestrator,
    Coder,
    Owner,
    Contributor,
}

impl TicketAssignmentRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Coder => "coder",
            Self::Owner => "owner",
            Self::Contributor => "contributor",
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "orchestrator" => Ok(Self::Orchestrator),
            "coder" => Ok(Self::Coder),
            "owner" => Ok(Self::Owner),
            "contributor" => Ok(Self::Contributor),
            _ => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unknown Ticket assignment role `{value}`").into(),
            )),
        }
    }

    pub fn is_singleton(self) -> bool {
        matches!(self, Self::Orchestrator | Self::Coder)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketAssignmentPrincipal {
    User {
        account_id: String,
    },
    Worker {
        runtime_id: String,
        worker_id: String,
    },
    WorkspaceAgent {
        agent_key: String,
    },
}

impl TicketAssignmentPrincipal {
    fn kind(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Worker { .. } => "worker",
            Self::WorkspaceAgent { .. } => "workspace_agent",
        }
    }

    pub fn worker(&self) -> Option<RuntimeWorkerRef> {
        match self {
            Self::Worker {
                runtime_id,
                worker_id,
            } => Some(RuntimeWorkerRef::new(runtime_id.clone(), worker_id.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketRoleAssignmentRecord {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment_id: String,
    pub role: TicketAssignmentRole,
    pub principal: TicketAssignmentPrincipal,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketCoderAssignmentRecord {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment_id: String,
    pub worker: RuntimeWorkerRef,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketCoderAssignmentEventRecord {
    pub workspace_id: String,
    pub ticket_id: String,
    pub event_id: String,
    pub action: String,
    pub assignment_id: Option<String>,
    pub previous_assignment_id: Option<String>,
    pub actor: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketWorkerAssignmentUpdate {
    pub current: TicketCoderAssignmentRecord,
    pub previous: Option<TicketCoderAssignmentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirCreateOperationRecord {
    pub workspace_id: String,
    pub operation_id: String,
    pub request_fingerprint: String,
    pub repository_id: String,
    pub selector: Option<String>,
    pub requested_runtime_id: Option<String>,
    pub resolved_runtime_id: String,
    pub config_revision: u64,
    pub config_projection_digest: String,
    pub source_kind: Option<String>,
    pub source_uri: Option<String>,
    pub source_revision: Option<u64>,
    pub source_fingerprint: Option<String>,
    pub credential_id: Option<String>,
    pub credential_revision: Option<u64>,
    pub host_trust_id: Option<String>,
    pub host_trust_revision: Option<u64>,
    pub repository_access_mode: Option<String>,
    pub cache_generation: u64,
    pub working_directory_id: String,
    pub state: String,
    pub failure: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRegistryRecord {
    pub workspace_id: String,
    pub workdir_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub creation_selector: Option<String>,
    pub creation_ref: Option<String>,
    pub creation_tree: Option<String>,
    pub current_selector: Option<String>,
    pub current_ref: Option<String>,
    pub current_tree: Option<String>,
    pub observed_at_epoch_seconds: Option<u64>,
    pub materialization_status: String,
    pub cleanliness: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkdirLinkRecord {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub workdir_id: String,
    pub role: String,
    pub linked_at: String,
    pub unlinked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveRecord {
    pub workspace_id: String,
    pub objective_id: String,
    pub title: String,
    pub state: String,
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveTicketLinkRecord {
    pub workspace_id: String,
    pub objective_id: String,
    pub ticket_id: String,
    pub kind: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveEventRecord {
    pub workspace_id: String,
    pub objective_id: String,
    pub event_id: String,
    pub kind: String,
    pub body_md: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveResourceRecord {
    pub workspace_id: String,
    pub objective_id: String,
    pub resource_path: String,
    pub body: String,
    pub media_type: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDocumentRecord {
    pub workspace_id: String,
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingRecord {
    pub workspace_id: String,
    pub candidate_id: String,
    pub raw_json: String,
    pub source_path: Option<String>,
    pub imported_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryStagingResolutionRecord {
    pub workspace_id: String,
    pub candidate_id: String,
    pub action: String,
    pub reason: String,
    pub affected_refs_json: String,
    pub staging_raw_json: String,
    pub source_path: Option<String>,
    pub imported_at: String,
    pub resolved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowSourceRecord {
    pub workspace_id: String,
    pub flow_id: String,
    pub source_kind: FlowSourceKind,
    pub name: String,
    pub path: String,
    pub content: String,
    pub content_digest: String,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowSourceRevisionRecord {
    pub workspace_id: String,
    pub flow_id: String,
    pub revision: u64,
    pub content: String,
    pub content_digest: String,
    pub definition: CompiledFlowDefinition,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceResourceKind {
    Ticket,
    Objective,
    Worker,
}

impl WorkspaceResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Objective => "objective",
            Self::Worker => "worker",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Ticket => "T",
            Self::Objective => "O",
            Self::Worker => "W",
        }
    }
}

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    fn resource_key(
        &self,
        workspace_id: &str,
        kind: WorkspaceResourceKind,
        resource_id: &str,
    ) -> Result<Option<String>>;
    fn resolve_resource_reference(
        &self,
        workspace_id: &str,
        kind: WorkspaceResourceKind,
        reference: &str,
    ) -> Result<Option<String>>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;
    fn create_workspace_bootstrap(
        &self,
        record: &WorkspaceBootstrapRecord,
    ) -> Result<WorkspaceBootstrapResult>;
    async fn get_trusted_runtime(&self, runtime_id: &str) -> Result<Option<TrustedRuntimeRecord>>;
    async fn upsert_trusted_runtime_record(&self, record: &TrustedRuntimeRecord) -> Result<()>;
    async fn consume_worker_mutation_source_jti(
        &self,
        runtime_id: &str,
        jti: &str,
        expires_at: u64,
        now_seconds: u64,
        consumed_at: &str,
    ) -> Result<bool>;
    fn plan_worker_removal(
        &self,
        request: &crate::retention::WorkerRemovalPlanRequest,
        inventory: &worker_runtime::retention::WorkerRetentionInventory,
    ) -> std::result::Result<
        crate::retention::WorkerRemovalPlan,
        crate::retention::WorkerRetentionError,
    > {
        let _ = (request, inventory);
        Err(crate::retention::WorkerRetentionError::Invalid(
            "Worker retention authority is unavailable".to_string(),
        ))
    }
    fn prepare_worker_removal_execution(
        &self,
        workspace_id: &str,
        plan_id: &str,
        input_fingerprint: &str,
    ) -> std::result::Result<
        crate::retention::PreparedWorkerRemoval,
        crate::retention::WorkerRetentionError,
    > {
        let _ = (workspace_id, plan_id, input_fingerprint);
        Err(crate::retention::WorkerRetentionError::Invalid(
            "Worker retention authority is unavailable".to_string(),
        ))
    }
    fn recover_worker_removal_execution(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> std::result::Result<
        Option<crate::retention::PreparedWorkerRemoval>,
        crate::retention::WorkerRetentionError,
    > {
        let _ = (workspace_id, worker);
        Ok(None)
    }
    fn fail_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        input_fingerprint: &str,
        category: &str,
    ) -> std::result::Result<(), crate::retention::WorkerRetentionError> {
        let _ = (workspace_id, operation_id, input_fingerprint, category);
        Err(crate::retention::WorkerRetentionError::Invalid(
            "Worker retention authority is unavailable".to_string(),
        ))
    }
    fn commit_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        input_fingerprint: &str,
        result: &worker_runtime::retention::WorkerRetentionExecutionResult,
    ) -> std::result::Result<
        crate::retention::WorkerRemovalPlan,
        crate::retention::WorkerRetentionError,
    > {
        let _ = (workspace_id, operation_id, input_fingerprint, result);
        Err(crate::retention::WorkerRetentionError::Invalid(
            "Worker retention authority is unavailable".to_string(),
        ))
    }
    fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>>;
    fn upsert_repository(&self, record: &RepositoryRecord) -> Result<()>;
    fn insert_repository(&self, record: &RepositoryRecord) -> Result<RepositoryInsertOutcome>;
    fn get_repository(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<Option<RepositoryRecord>>;
    fn get_repository_by_key(
        &self,
        workspace_id: &str,
        repository_key: &str,
    ) -> Result<Option<RepositoryRecord>> {
        Ok(self
            .list_repositories(workspace_id)?
            .into_iter()
            .find(|repository| repository.repository_key == repository_key))
    }
    fn list_repositories(&self, workspace_id: &str) -> Result<Vec<RepositoryRecord>>;

    fn put_flow_source_for_kind(
        &self,
        workspace_id: &str,
        source_kind: FlowSourceKind,
        path: &str,
        content: &str,
        now: &str,
    ) -> Result<FlowSourceRecord>;
    fn get_flow_source_by_name(
        &self,
        workspace_id: &str,
        source_kind: FlowSourceKind,
        name: &str,
    ) -> Result<Option<FlowSourceRecord>>;
    fn list_flow_sources(&self, workspace_id: &str) -> Result<Vec<FlowSourceRecord>>;
    fn get_flow_source(
        &self,
        workspace_id: &str,
        flow_id: &str,
    ) -> Result<Option<FlowSourceRecord>>;
    fn get_flow_source_revision(
        &self,
        workspace_id: &str,
        flow_id: &str,
        revision: u64,
    ) -> Result<Option<FlowSourceRevisionRecord>>;

    fn upsert_objective(&self, record: &ObjectiveRecord) -> Result<()>;
    fn list_objectives(&self, workspace_id: &str, limit: usize) -> Result<Vec<ObjectiveRecord>>;
    fn list_objectives_for_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<ObjectiveRecord>>;
    fn get_objective(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Option<ObjectiveRecord>>;
    fn replace_objective_ticket_links(
        &self,
        workspace_id: &str,
        objective_id: &str,
        links: &[ObjectiveTicketLinkRecord],
    ) -> Result<()>;
    fn list_objective_ticket_links(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveTicketLinkRecord>>;
    fn insert_objective_event(&self, record: &ObjectiveEventRecord) -> Result<()>;
    fn list_objective_events(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveEventRecord>>;
    fn upsert_objective_resource(&self, record: &ObjectiveResourceRecord) -> Result<()>;
    fn list_objective_resources(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveResourceRecord>>;
    fn ensure_memory_document(
        &self,
        workspace_id: &str,
        default_body_md: &str,
        now: &str,
    ) -> Result<MemoryDocumentRecord>;
    fn get_memory_document(&self, workspace_id: &str) -> Result<Option<MemoryDocumentRecord>>;
    fn upsert_memory_document(&self, record: &MemoryDocumentRecord) -> Result<()>;
    fn upsert_memory_staging_record(&self, record: &MemoryStagingRecord) -> Result<()>;
    fn list_memory_staging_records(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryStagingRecord>>;
    fn get_memory_staging_record(
        &self,
        workspace_id: &str,
        candidate_id: &str,
    ) -> Result<Option<MemoryStagingRecord>>;
    fn delete_memory_staging_record(&self, workspace_id: &str, candidate_id: &str) -> Result<bool>;
    fn count_memory_staging_records(&self, workspace_id: &str) -> Result<usize>;
    fn insert_memory_staging_resolution(
        &self,
        record: &MemoryStagingResolutionRecord,
    ) -> Result<()>;
    fn list_memory_staging_resolutions(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryStagingResolutionRecord>>;

    fn upsert_account(&self, record: &AccountRecord) -> Result<()>;
    fn get_account(&self, account_id: &str) -> Result<Option<AccountRecord>>;
    fn get_account_by_handle(&self, kind: &str, handle: &str) -> Result<Option<AccountRecord>>;
    fn upsert_user(&self, record: &UserRecord) -> Result<()>;
    fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>>;
    fn get_user_by_handle(&self, handle: &str) -> Result<Option<UserRecord>>;
    fn any_user(&self) -> Result<Option<UserRecord>>;
    fn upsert_passkey_credential(&self, record: &PasskeyCredentialRecord) -> Result<()>;
    fn get_passkey_credential(
        &self,
        credential_id: &str,
    ) -> Result<Option<PasskeyCredentialRecord>>;
    fn list_passkey_credentials_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<PasskeyCredentialRecord>>;
    fn put_auth_challenge(&self, record: &AuthChallengeRecord) -> Result<()>;
    fn consume_auth_challenge(
        &self,
        challenge: &str,
        ceremony: &str,
        consumed_at: &str,
    ) -> Result<Option<AuthChallengeRecord>>;
    fn consume_auth_challenge_by_id(
        &self,
        challenge_id: &str,
        ceremony: &str,
        consumed_at: &str,
    ) -> Result<Option<AuthChallengeRecord>>;
    fn create_browser_session(&self, record: &BrowserSessionRecord) -> Result<()>;
    fn resolve_browser_session(&self, token_hash: &str) -> Result<Option<BrowserSessionRecord>>;
    fn revoke_browser_session(&self, token_hash: &str, revoked_at: &str) -> Result<bool>;
    fn create_api_token(&self, record: &ApiTokenRecord) -> Result<()>;
    fn resolve_api_token(&self, token_hash: &str) -> Result<Option<ApiTokenRecord>>;
    fn mark_api_token_used(&self, token_hash: &str, used_at: &str) -> Result<()>;
    fn try_create_device_login_flow(&self, record: &DeviceLoginFlowRecord) -> Result<bool>;
    fn get_device_login_flow_by_user_code(
        &self,
        user_code: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>>;
    fn get_device_login_flow_by_device_code(
        &self,
        device_code: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>>;
    fn approve_device_login_flow(
        &self,
        device_code: &str,
        user_id: &str,
        api_token_id: &str,
        issued_access_token: &str,
        approved_at: &str,
    ) -> Result<bool>;
    fn consume_device_login_token(
        &self,
        device_code: &str,
        consumed_at: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>>;

    fn upsert_worker_registry(&self, record: &WorkerRegistryRecord) -> Result<()>;
    fn get_worker_registry(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<WorkerRegistryRecord>>;
    fn has_active_worker_create_reservation(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<bool>;
    fn list_worker_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkerRegistryRecord>>;
    fn update_worker_retention(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool>;
    fn delete_worker_registry(&self, workspace_id: &str, worker: &RuntimeWorkerRef)
    -> Result<bool>;

    fn create_worker_control_grant(
        &self,
        record: &WorkerControlGrantRecord,
    ) -> Result<WorkerControlGrantRecord>;
    fn get_worker_control_grant(
        &self,
        workspace_id: &str,
        grant_id: &str,
    ) -> Result<Option<WorkerControlGrantRecord>>;
    fn get_worker_control_grant_by_operation(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        operation_id: &str,
    ) -> Result<Option<WorkerControlGrantRecord>>;
    fn get_active_worker_control_grant(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        subject: &RuntimeWorkerRef,
    ) -> Result<Option<WorkerControlGrantRecord>>;
    fn list_active_worker_control_grants(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        limit: usize,
    ) -> Result<Vec<WorkerControlGrantRecord>>;
    fn revoke_worker_control_grant(
        &self,
        workspace_id: &str,
        grant_id: &str,
        revoked_at: &str,
    ) -> Result<bool>;
    fn get_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<TicketAssignmentOperationRecord>>;
    fn reserve_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
        ticket_id: &str,
        runtime_id: &str,
        worker_id: Option<&str>,
        request_fingerprint: &str,
        created_at: &str,
    ) -> Result<()>;
    fn bind_ticket_assignment_operation_worker(
        &self,
        workspace_id: &str,
        operation_id: &str,
        worker_id: &str,
    ) -> Result<()>;
    fn rollback_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<()>;
    fn list_current_ticket_role_assignments(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Vec<TicketRoleAssignmentRecord>>;
    fn get_current_ticket_role_assignment_for_worker(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<TicketRoleAssignmentRecord>>;
    fn get_current_ticket_role_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        role: TicketAssignmentRole,
    ) -> Result<Option<TicketRoleAssignmentRecord>>;
    fn set_current_ticket_role_assignment(
        &self,
        record: &TicketRoleAssignmentRecord,
        expected_assignment_id: Option<&str>,
        event_id: &str,
        operation_id: &str,
        allow_reassign: bool,
    ) -> Result<TicketRoleAssignmentRecord>;
    fn start_ready_ticket_with_coder_assignment(
        &self,
        record: &TicketRoleAssignmentRecord,
        event_id: &str,
        operation_id: &str,
    ) -> Result<TicketRoleAssignmentRecord>;
    fn clear_current_ticket_role_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        role: TicketAssignmentRole,
        assignment_id: &str,
        event_id: &str,
        operation_id: &str,
        actor: &str,
        occurred_at: &str,
        reason: Option<&str>,
    ) -> Result<bool>;
    fn cancel_current_ticket_coder_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        assignment_id: &str,
        assignment_event_id: &str,
        state_event_id: &str,
        operation_id: &str,
        actor: &str,
        occurred_at: &str,
        reason: &str,
    ) -> Result<bool>;
    fn get_current_ticket_coder_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<TicketCoderAssignmentRecord>>;
    fn set_current_ticket_coder_assignment(
        &self,
        record: &TicketCoderAssignmentRecord,
        expected_assignment_id: Option<&str>,
        event_id: &str,
        operation_id: &str,
        allow_reassign: bool,
    ) -> Result<TicketWorkerAssignmentUpdate>;
    fn clear_current_ticket_worker_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        expected_assignment_id: Option<&str>,
        operation_id: &str,
        event_id: &str,
        actor: &str,
        created_at: &str,
    ) -> Result<Option<TicketCoderAssignmentRecord>>;
    fn list_ticket_coder_assignment_events(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<TicketCoderAssignmentEventRecord>>;

    fn upsert_workdir_registry(&self, record: &WorkdirRegistryRecord) -> Result<()>;
    fn get_workdir_registry(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Option<WorkdirRegistryRecord>>;
    fn list_workdir_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkdirRegistryRecord>>;
    fn delete_workdir_registry(&self, workspace_id: &str, workdir_id: &str) -> Result<bool>;

    fn reserve_worker_workdir_attachment(
        &self,
        workspace_id: &str,
        workdir_id: &str,
        reservation_id: &str,
        reserved_at: &str,
    ) -> Result<()>;
    fn release_worker_workdir_attachment_reservation(
        &self,
        workspace_id: &str,
        workdir_id: &str,
        reservation_id: &str,
    ) -> Result<()>;
    fn finalize_reserved_worker_workdir_attachment(
        &self,
        record: &WorkerWorkdirLinkRecord,
        reservation_id: &str,
    ) -> Result<WorkerWorkdirLinkRecord>;
    fn attach_worker_workdir(
        &self,
        record: &WorkerWorkdirLinkRecord,
    ) -> Result<WorkerWorkdirLinkRecord>;
    fn detach_worker_workdir(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
        expected_workdir_id: Option<&str>,
        unlinked_at: &str,
    ) -> Result<Option<WorkerWorkdirLinkRecord>>;
    fn worker_workdir_link_history_exists(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<bool>;
    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>>;
    fn list_workdir_worker_links(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>>;
}

#[derive(Clone)]
pub struct SqliteWorkspaceStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteWorkspaceStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn from_connection(conn: Connection) -> Result<Self> {
        configure_sqlite(&conn)?;
        ticket::migrate_sqlite_ticket_schema(&conn)
            .map_err(|error| Error::Store(format!("Ticket schema verification failed: {error}")))?;
        merge_request::migrate(&conn).map_err(|error| {
            Error::Store(format!("Merge Request schema verification failed: {error}"))
        })?;
        apply_migrations(&conn)
            .map_err(|error| Error::Store(format!("workspace schema migration failed: {error}")))?;
        validate_workspace_resource_references(&conn)?;
        verify_workspace_resource_constraints(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Store("sqlite connection lock poisoned".to_string()))?;
        f(&conn)
    }

    pub(crate) fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| Error::Store("sqlite connection lock poisoned".to_string()))?;
        f(&mut conn)
    }

    pub(crate) fn get_workspace_memory_settings(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceMemorySettingsRecord> {
        let record = self.with_conn(|conn| {
            conn.query_row(
                "SELECT workspace_id, settings_revision, language, created_at, updated_at \
                 FROM workspace_memory_settings WHERE workspace_id = ?1",
                params![workspace_id],
                |row| {
                    let revision = row.get::<_, i64>(1)?;
                    Ok(WorkspaceMemorySettingsRecord {
                        workspace_id: row.get(0)?,
                        settings_revision: revision
                            .try_into()
                            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, revision))?,
                        language: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::Store("Workspace Memory settings are missing".to_string()))
        })?;
        validate_workspace_memory_settings_record(&record, workspace_id)?;
        Ok(record)
    }

    pub(crate) fn update_workspace_memory_settings(
        &self,
        workspace_id: &str,
        expected_revision: u64,
        language: &str,
    ) -> Result<WorkspaceMemorySettingsRecord> {
        let language = normalize_workspace_memory_language(language)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = tx
                .query_row(
                    "SELECT workspace_id, settings_revision, language, created_at, updated_at \
                     FROM workspace_memory_settings WHERE workspace_id = ?1",
                    params![workspace_id],
                    |row| {
                        let revision = row.get::<_, i64>(1)?;
                        Ok(WorkspaceMemorySettingsRecord {
                            workspace_id: row.get(0)?,
                            settings_revision: revision.try_into().map_err(|_| {
                                rusqlite::Error::IntegralValueOutOfRange(1, revision)
                            })?,
                            language: row.get(2)?,
                            created_at: row.get(3)?,
                            updated_at: row.get(4)?,
                        })
                    },
                )
                .optional()?
                .ok_or_else(|| Error::Store("Workspace Memory settings are missing".to_string()))?;
            validate_workspace_memory_settings_record(&current, workspace_id)?;
            let current_revision = current.settings_revision;
            if current_revision != expected_revision {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "Workspace Memory settings revision changed: expected {expected_revision}, current {current_revision}"
                )));
            }
            if current.language == language {
                tx.commit()?;
                return Ok(current);
            }
            let next_revision = current_revision.checked_add(1).ok_or_else(|| {
                Error::InvalidInput("Workspace Memory settings revision overflow".to_string())
            })?;
            let now = chrono::Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE workspace_memory_settings \
                 SET settings_revision = ?2, language = ?3, updated_at = ?4 \
                 WHERE workspace_id = ?1",
                params![workspace_id, next_revision as i64, language, now],
            )?;
            let record = tx.query_row(
                "SELECT workspace_id, settings_revision, language, created_at, updated_at \
                 FROM workspace_memory_settings WHERE workspace_id = ?1",
                params![workspace_id],
                |row| {
                    let revision = row.get::<_, i64>(1)?;
                    Ok(WorkspaceMemorySettingsRecord {
                        workspace_id: row.get(0)?,
                        settings_revision: revision as u64,
                        language: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub(crate) fn reserve_worker_create(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        allocation_key: &str,
        request_fingerprint: &str,
        current_memory_settings: &WorkspaceMemorySettingsRecord,
    ) -> Result<WorkerCreateReservation> {
        if allocation_key.trim().is_empty() || request_fingerprint.trim().is_empty() {
            return Err(Error::InvalidInput(
                "Worker create allocation key and fingerprint must be non-empty".to_string(),
            ));
        }
        validate_workspace_memory_settings_record(current_memory_settings, workspace_id)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = tx
                .query_row(
                    "SELECT worker_id, runtime_id, request_fingerprint, create_fingerprint, \
                            memory_settings_revision, memory_language \
                     FROM worker_create_reservations \
                     WHERE workspace_id = ?1 AND allocation_key = ?2",
                    params![workspace_id, allocation_key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((worker_id, reserved_runtime_id, stored_request_fingerprint, create_fingerprint, revision, language)) = existing {
                if reserved_runtime_id != runtime_id
                    || stored_request_fingerprint.as_deref() != Some(request_fingerprint)
                {
                    return Err(Error::InvalidInput(format!(
                        "Worker create allocation {allocation_key} was already used with different input"
                    )));
                }
                let revision = revision.ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "Worker create allocation {allocation_key} has no persisted Memory settings snapshot"
                    ))
                })?;
                let language = language.ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "Worker create allocation {allocation_key} has no persisted Memory language"
                    ))
                })?;
                let worker_id = worker_id.parse::<WorkerId>().map_err(|_| {
                    Error::Store(format!(
                        "Worker create allocation {allocation_key} has a non-UUIDv7 worker id"
                    ))
                })?;
                let snapshot = manifest::WorkspaceMemorySettingsSnapshot {
                    workspace_id: workspace_id.to_string(),
                    settings_revision: revision.try_into().map_err(|_| {
                        rusqlite::Error::IntegralValueOutOfRange(4, revision)
                    })?,
                    language,
                };
                validate_workspace_memory_settings_snapshot(&snapshot, workspace_id)?;
                return Ok(WorkerCreateReservation {
                    worker_id,
                    create_fingerprint,
                    memory_settings: snapshot,
                });
            }

            let (authoritative_revision, authoritative_language) = tx
                .query_row(
                    "SELECT settings_revision, language FROM workspace_memory_settings WHERE workspace_id = ?1",
                    params![workspace_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .ok_or_else(|| Error::Store("Workspace Memory settings are missing".to_string()))?;
            let authoritative_revision: u64 = authoritative_revision.try_into().map_err(|_| {
                rusqlite::Error::IntegralValueOutOfRange(0, authoritative_revision)
            })?;
            if authoritative_revision != current_memory_settings.settings_revision
                || authoritative_language != current_memory_settings.language
            {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace Memory settings changed while the Worker create reservation was being accepted"
                        .to_string(),
                ));
            }

            let snapshot = manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: workspace_id.to_string(),
                settings_revision: authoritative_revision,
                language: authoritative_language,
            };
            validate_workspace_memory_settings_snapshot(&snapshot, workspace_id)?;
            let create_fingerprint =
                bound_worker_create_fingerprint(request_fingerprint, &snapshot);
            let worker_id = WorkerId::now_v7();
            let now = chrono::Utc::now().to_rfc3339();
            allocate_resource_key(
                &tx,
                workspace_id,
                WorkspaceResourceKind::Worker,
                &worker_id.to_string(),
                &now,
            )?;
            tx.execute(
                "INSERT INTO worker_create_reservations(\
                    workspace_id, allocation_key, worker_id, runtime_id, create_fingerprint,\
                    state, created_at, updated_at, request_fingerprint,\
                    memory_settings_revision, memory_language\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'reserved', ?6, ?6, ?7, ?8, ?9)",
                params![
                    workspace_id,
                    allocation_key,
                    worker_id.to_string(),
                    runtime_id,
                    create_fingerprint,
                    now,
                    request_fingerprint,
                    snapshot.settings_revision as i64,
                    snapshot.language,
                ],
            )?;
            tx.commit()?;
            Ok(WorkerCreateReservation {
                worker_id,
                create_fingerprint,
                memory_settings: snapshot,
            })
        })
    }

    pub(crate) fn complete_worker_create_reservation(
        &self,
        workspace_id: &str,
        worker_id: WorkerId,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE worker_create_reservations \
                 SET state = 'created', updated_at = ?3 \
                 WHERE workspace_id = ?1 AND worker_id = ?2",
                params![
                    workspace_id,
                    worker_id.to_string(),
                    chrono::Utc::now().to_rfc3339()
                ],
            )?;
            if changed != 1 {
                return Err(Error::Store(format!(
                    "Worker create reservation {} was not found",
                    worker_id
                )));
            }
            Ok(())
        })
    }

    fn materialize_workspace_config(&self, workspace_id: &str, created_at: &str) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if crate::config_source::load_state(&tx, workspace_id)?.is_none() {
                let state = crate::config_source::initial_state()?;
                crate::config_source::insert_materialized_state(
                    &tx,
                    workspace_id,
                    &state,
                    created_at,
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn upsert_trusted_runtime(&self, record: &TrustedRuntimeRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO trusted_runtime_records (
                    runtime_id, workspace_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(runtime_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    base_url = excluded.base_url,
                    public_key = excluded.public_key,
                    updated_at = excluded.updated_at,
                    revoked_at = excluded.revoked_at"#,
                params![
                    record.runtime_id,
                    record.workspace_id,
                    record.display_name,
                    record.base_url,
                    record.public_key,
                    record.created_at,
                    record.updated_at,
                    record.revoked_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_trusted_runtimes(
        &self,
        include_revoked: bool,
    ) -> Result<Vec<TrustedRuntimeRecord>> {
        self.with_conn(|conn| {
            let sql = if include_revoked {
                r#"SELECT runtime_id, workspace_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                   FROM trusted_runtime_records ORDER BY runtime_id ASC"#
            } else {
                r#"SELECT runtime_id, workspace_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                   FROM trusted_runtime_records WHERE revoked_at IS NULL ORDER BY runtime_id ASC"#
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], read_trusted_runtime_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn revoke_trusted_runtime(&self, runtime_id: &str, revoked_at: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE trusted_runtime_records
                   SET revoked_at = ?2, updated_at = ?2
                   WHERE runtime_id = ?1 AND revoked_at IS NULL"#,
                params![runtime_id, revoked_at],
            )?;
            Ok(changed > 0)
        })
    }
}

#[async_trait]
impl ControlPlaneStore for SqliteWorkspaceStore {
    async fn schema_version(&self) -> Result<i64> {
        self.with_conn(current_schema_version)
    }

    fn resource_key(
        &self,
        workspace_id: &str,
        kind: WorkspaceResourceKind,
        resource_id: &str,
    ) -> Result<Option<String>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT resource_key FROM workspace_resource_keys
                 WHERE workspace_id = ?1 AND resource_kind = ?2 AND resource_id = ?3",
                params![workspace_id, kind.as_str(), resource_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn resolve_resource_reference(
        &self,
        workspace_id: &str,
        kind: WorkspaceResourceKind,
        reference: &str,
    ) -> Result<Option<String>> {
        self.with_conn(|conn| {
            if let Some(resource_id) = conn
                .query_row(
                    "SELECT resource_id FROM workspace_resource_keys
                     WHERE workspace_id = ?1 AND resource_kind = ?2 AND resource_key = ?3",
                    params![workspace_id, kind.as_str(), reference],
                    |row| row.get(0),
                )
                .optional()?
            {
                return Ok(Some(resource_id));
            }
            let exists = match kind {
                WorkspaceResourceKind::Ticket => conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2)",
                    params![workspace_id, reference],
                    |row| row.get::<_, i64>(0),
                )?,
                WorkspaceResourceKind::Objective => conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM objectives WHERE workspace_id = ?1 AND objective_id = ?2)",
                    params![workspace_id, reference],
                    |row| row.get::<_, i64>(0),
                )?,
                WorkspaceResourceKind::Worker => conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_registry WHERE workspace_id = ?1 AND worker_id = ?2)",
                    params![workspace_id, reference],
                    |row| row.get::<_, i64>(0),
                )?,
            };
            Ok((exists != 0).then(|| reference.to_string()))
        })
    }

    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            #[cfg(test)]
            if record.owner_account_id == "owner-account" {
                // Unrelated store tests use this explicit fixture identity. Keep the
                // production owner contract strict while giving those fixtures a real
                // User Account row instead of reviving ownerless Workspace setup.
                tx.execute(
                    "INSERT OR IGNORE INTO accounts (
                         account_id, kind, handle, display_name, created_at, updated_at
                     ) VALUES (?1, 'user', 'owner-account', 'Owner Account', ?2, ?2)",
                    params![record.owner_account_id.as_str(), record.created_at.as_str()],
                )?;
            }
            let owner_kind = tx
                .query_row(
                    "SELECT kind FROM accounts WHERE account_id = ?1",
                    params![record.owner_account_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if owner_kind.as_deref() != Some("user") {
                return Err(Error::Store(format!(
                    "Workspace owner `{}` must reference an existing User Account",
                    record.owner_account_id
                )));
            }
            let current_owner = tx
                .query_row(
                    "SELECT owner_account_id FROM workspaces WHERE workspace_id = ?1",
                    params![record.workspace_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if current_owner
                .as_deref()
                .is_some_and(|owner| owner != record.owner_account_id)
            {
                return Err(Error::Store(format!(
                    "Workspace `{}` owner is immutable through upsert",
                    record.workspace_id
                )));
            }
            tx.execute(
                r#"INSERT INTO workspaces (
                    workspace_id, owner_account_id, display_name, state, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    state = excluded.state,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.owner_account_id,
                    record.display_name,
                    record.state,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            tx.execute(
                r#"INSERT OR IGNORE INTO workspace_memory_settings (
                    workspace_id, settings_revision, language, created_at, updated_at
                ) VALUES (?1, 1, 'English', ?2, ?3)"#,
                params![record.workspace_id, record.created_at, record.updated_at],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        self.materialize_workspace_config(&record.workspace_id, &record.created_at)
    }

    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, owner_account_id, display_name, state, created_at, updated_at
                   FROM workspaces WHERE workspace_id = ?1"#,
                params![workspace_id],
                read_workspace_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn create_workspace_bootstrap(
        &self,
        record: &WorkspaceBootstrapRecord,
    ) -> Result<WorkspaceBootstrapResult> {
        validate_repository_record_identity(&record.repository)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let owner_kind = tx
                .query_row(
                    "SELECT kind FROM accounts WHERE account_id = ?1",
                    params![record.workspace.owner_account_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if owner_kind.as_deref() != Some("user") {
                return Err(Error::Store(format!(
                    "Workspace owner `{}` must reference an existing User Account",
                    record.workspace.owner_account_id
                )));
            }
            if let Some((fingerprint, workspace_id)) = tx
                .query_row(
                    "SELECT request_fingerprint, workspace_id FROM workspace_create_operations WHERE operation_key = ?1",
                    params![record.operation_key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
            {
                if fingerprint != record.request_fingerprint {
                    return Err(Error::WorkspaceConfigConflict(
                        "Workspace create operation key was already used with different input"
                            .to_string(),
                    ));
                }
                let workspace = tx.query_row(
                    r#"SELECT workspace_id, owner_account_id, display_name, state, created_at, updated_at
                       FROM workspaces WHERE workspace_id = ?1"#,
                    params![workspace_id],
                    read_workspace_record,
                )?;
                let repository = tx.query_row(
                    r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                              source_kind, source_uri, default_ref, source_revision,
                              source_fingerprint, observed_status, observed_at, created_at, updated_at
                       FROM repositories WHERE workspace_id = ?1 AND repository_key = ?2"#,
                    params![workspace.workspace_id, record.repository.repository_key],
                    read_repository_record,
                )?;
                let config_revision = crate::config_source::load_state(&tx, &workspace.workspace_id)?
                    .ok_or_else(|| Error::Store("Workspace config is missing".to_string()))?
                    .snapshot
                    .revision;
                tx.commit()?;
                return Ok(WorkspaceBootstrapResult {
                    workspace,
                    repository,
                    config_revision,
                    replayed: true,
                });
            }

            if tx
                .query_row(
                    "SELECT 1 FROM workspaces WHERE workspace_id = ?1",
                    params![record.workspace.workspace_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some()
            {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace identity already exists".to_string(),
                ));
            } else {
                tx.execute(
                    r#"INSERT INTO workspaces (
                        workspace_id, owner_account_id, display_name, state, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                    params![
                        record.workspace.workspace_id,
                        record.workspace.owner_account_id,
                        record.workspace.display_name,
                        record.workspace.state,
                        record.workspace.created_at,
                        record.workspace.updated_at,
                    ],
                )?;
                tx.execute(
                    r#"INSERT INTO workspace_memory_settings (
                        workspace_id, settings_revision, language, created_at, updated_at
                    ) VALUES (?1, 1, 'English', ?2, ?3)"#,
                    params![
                        record.workspace.workspace_id,
                        record.workspace.created_at,
                        record.workspace.updated_at,
                    ],
                )?;
                tx.execute(
                    r#"INSERT INTO repositories (
                        workspace_id, repository_id, repository_key, kind, provider, uri,
                        source_kind, source_uri, default_ref, source_revision,
                        source_fingerprint, observed_status, observed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                    params![
                        record.repository.workspace_id,
                        record.repository.repository_id,
                        record.repository.repository_key,
                        record.repository.kind,
                        record.repository.provider,
                        record.repository.source.uri,
                        record.repository.source.kind.as_str(),
                        record.repository.source.uri,
                        record.repository.default_ref,
                        record.repository.source_revision,
                        record.repository.source_fingerprint,
                        record.repository.observed_status.as_str(),
                        record.repository.observed_at,
                        record.repository.created_at,
                        record.repository.updated_at,
                    ],
                )?;
            }
            if crate::config_source::load_state(&tx, &record.workspace.workspace_id)?.is_none() {
                let state = crate::config_source::initial_state()?;
                crate::config_source::insert_materialized_state(
                    &tx,
                    &record.workspace.workspace_id,
                    &state,
                    &record.workspace.created_at,
                )?;
            }
            for resource_kind in ["ticket", "objective", "worker"] {
                tx.execute(
                    r#"INSERT OR IGNORE INTO workspace_resource_key_counters (
                        workspace_id, resource_kind, next_sequence
                    ) VALUES (?1, ?2, 1)"#,
                    params![record.workspace.workspace_id, resource_kind],
                )?;
            }
            let config_revision = crate::config_source::load_state(
                &tx,
                &record.workspace.workspace_id,
            )?
            .ok_or_else(|| Error::Store("Workspace config is missing".to_string()))?
            .snapshot
            .revision;
            tx.execute(
                r#"INSERT INTO workspace_create_operations (
                    operation_key, request_fingerprint, workspace_id, created_at
                ) VALUES (?1, ?2, ?3, ?4)"#,
                params![
                    record.operation_key,
                    record.request_fingerprint,
                    record.workspace.workspace_id,
                    record.workspace.created_at,
                ],
            )?;
            tx.commit()?;
            Ok(WorkspaceBootstrapResult {
                workspace: record.workspace.clone(),
                repository: record.repository.clone(),
                config_revision,
                replayed: false,
            })
        })
    }

    async fn get_trusted_runtime(&self, runtime_id: &str) -> Result<Option<TrustedRuntimeRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT runtime_id, workspace_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                   FROM trusted_runtime_records WHERE runtime_id = ?1"#,
                params![runtime_id],
                read_trusted_runtime_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    async fn upsert_trusted_runtime_record(&self, record: &TrustedRuntimeRecord) -> Result<()> {
        SqliteWorkspaceStore::upsert_trusted_runtime(self, record)
    }

    async fn consume_worker_mutation_source_jti(
        &self,
        runtime_id: &str,
        jti: &str,
        expires_at: u64,
        now_seconds: u64,
        consumed_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM worker_mutation_source_proof_jtis WHERE expires_at < ?1",
                params![now_seconds],
            )?;
            let inserted = transaction.execute(
                r#"INSERT OR IGNORE INTO worker_mutation_source_proof_jtis (
                    runtime_id, jti, expires_at, consumed_at
                ) VALUES (?1, ?2, ?3, ?4)"#,
                params![runtime_id, jti, expires_at, consumed_at],
            )?;
            transaction.commit()?;
            Ok(inserted == 1)
        })
    }

    fn plan_worker_removal(
        &self,
        request: &crate::retention::WorkerRemovalPlanRequest,
        inventory: &worker_runtime::retention::WorkerRetentionInventory,
    ) -> std::result::Result<
        crate::retention::WorkerRemovalPlan,
        crate::retention::WorkerRetentionError,
    > {
        SqliteWorkspaceStore::plan_worker_removal(self, request, inventory)
    }

    fn prepare_worker_removal_execution(
        &self,
        workspace_id: &str,
        plan_id: &str,
        input_fingerprint: &str,
    ) -> std::result::Result<
        crate::retention::PreparedWorkerRemoval,
        crate::retention::WorkerRetentionError,
    > {
        SqliteWorkspaceStore::prepare_worker_removal_execution(
            self,
            workspace_id,
            plan_id,
            input_fingerprint,
        )
    }

    fn recover_worker_removal_execution(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> std::result::Result<
        Option<crate::retention::PreparedWorkerRemoval>,
        crate::retention::WorkerRetentionError,
    > {
        SqliteWorkspaceStore::recover_worker_removal_execution(self, workspace_id, worker)
    }

    fn fail_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        input_fingerprint: &str,
        category: &str,
    ) -> std::result::Result<(), crate::retention::WorkerRetentionError> {
        SqliteWorkspaceStore::fail_worker_removal(
            self,
            workspace_id,
            operation_id,
            input_fingerprint,
            category,
        )
    }

    fn commit_worker_removal(
        &self,
        workspace_id: &str,
        operation_id: &str,
        input_fingerprint: &str,
        result: &worker_runtime::retention::WorkerRetentionExecutionResult,
    ) -> std::result::Result<
        crate::retention::WorkerRemovalPlan,
        crate::retention::WorkerRetentionError,
    > {
        SqliteWorkspaceStore::commit_worker_removal(
            self,
            workspace_id,
            operation_id,
            input_fingerprint,
            result,
        )
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, owner_account_id, display_name, state, created_at, updated_at
                   FROM workspaces
                   ORDER BY workspace_id ASC"#,
            )?;
            let rows = stmt.query_map([], read_workspace_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn upsert_repository(&self, record: &RepositoryRecord) -> Result<()> {
        validate_repository_record_identity(record)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = tx
                .query_row(
                    r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                              source_kind, source_uri, default_ref, source_revision,
                              source_fingerprint, observed_status, observed_at, created_at, updated_at
                       FROM repositories
                       WHERE workspace_id = ?1 AND repository_key = ?2"#,
                    params![record.workspace_id, record.repository_key],
                    read_repository_record,
                )
                .optional()?;
            if let Some(existing) = existing {
                if !repository_registration_intent_matches(&existing, record) {
                    return Err(Error::WorkspaceConfigConflict(format!(
                        "Repository key `{}` already exists with different registration intent",
                        record.repository_key
                    )));
                }
                tx.execute(
                    r#"UPDATE repositories
                       SET observed_status = ?3,
                           observed_at = ?4,
                           updated_at = ?5
                       WHERE workspace_id = ?1 AND repository_id = ?2"#,
                    params![
                        record.workspace_id,
                        existing.repository_id,
                        record.observed_status.as_str(),
                        record.observed_at,
                        record.updated_at,
                    ],
                )?;
            } else {
                tx.execute(
                    r#"INSERT INTO repositories (
                        workspace_id, repository_id, repository_key, kind, provider, uri,
                        source_kind, source_uri, default_ref, source_revision,
                        source_fingerprint, observed_status, observed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                    params![
                        record.workspace_id,
                        record.repository_id,
                        record.repository_key,
                        record.kind,
                        record.provider,
                        record.source.uri,
                        record.source.kind.as_str(),
                        record.source.uri,
                        record.default_ref,
                        record.source_revision,
                        record.source_fingerprint,
                        record.observed_status.as_str(),
                        record.observed_at,
                        record.created_at,
                        record.updated_at,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    fn insert_repository(&self, record: &RepositoryRecord) -> Result<RepositoryInsertOutcome> {
        validate_repository_record_identity(record)?;
        self.with_conn_mut(|conn| {
            let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = transaction
                .query_row(
                    r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                              source_kind, source_uri, default_ref, source_revision,
                              source_fingerprint, observed_status, observed_at, created_at, updated_at
                       FROM repositories
                       WHERE workspace_id = ?1 AND repository_key = ?2"#,
                    params![record.workspace_id, record.repository_key],
                    read_repository_record,
                )
                .optional()?;
            if let Some(existing) = existing {
                transaction.commit()?;
                return Ok(RepositoryInsertOutcome::Existing(existing));
            }

            transaction.execute(
                r#"INSERT INTO repositories (
                    workspace_id, repository_id, repository_key, kind, provider, uri,
                    source_kind, source_uri, default_ref, source_revision,
                    source_fingerprint, observed_status, observed_at, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                params![
                    record.workspace_id,
                    record.repository_id,
                    record.repository_key,
                    record.kind,
                    record.provider,
                    record.source.uri,
                    record.source.kind.as_str(),
                    record.source.uri,
                    record.default_ref,
                    record.source_revision,
                    record.source_fingerprint,
                    record.observed_status.as_str(),
                    record.observed_at,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            transaction.commit()?;
            Ok(RepositoryInsertOutcome::Created)
        })
    }

    fn get_repository(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<Option<RepositoryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                          source_kind, source_uri, default_ref, source_revision,
                          source_fingerprint, observed_status, observed_at, created_at, updated_at
                   FROM repositories
                   WHERE workspace_id = ?1 AND repository_id = ?2"#,
                params![workspace_id, repository_id],
                read_repository_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_repository_by_key(
        &self,
        workspace_id: &str,
        repository_key: &str,
    ) -> Result<Option<RepositoryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                          source_kind, source_uri, default_ref, source_revision,
                          source_fingerprint, observed_status, observed_at, created_at, updated_at
                   FROM repositories
                   WHERE workspace_id = ?1 AND repository_key = ?2"#,
                params![workspace_id, repository_key],
                read_repository_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_repositories(&self, workspace_id: &str) -> Result<Vec<RepositoryRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, repository_id, repository_key, kind, provider,
                          source_kind, source_uri, default_ref, source_revision,
                          source_fingerprint, observed_status, observed_at, created_at, updated_at
                   FROM repositories
                   WHERE workspace_id = ?1
                   ORDER BY repository_key ASC"#,
            )?;
            let rows = stmt.query_map(params![workspace_id], read_repository_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn put_flow_source_for_kind(
        &self,
        workspace_id: &str,
        source_kind: FlowSourceKind,
        path: &str,
        content: &str,
        now: &str,
    ) -> Result<FlowSourceRecord> {
        if source_kind != FlowSourceKind::Workspace {
            return Err(Error::Store(
                "built-in Flow sources are resource authority and cannot be written to Workspace DB"
                    .to_string(),
            ));
        }
        let name = flow_source_name(path)?;
        let definition = compile_flow_source(content).map_err(|error| {
            Error::Store(format!(
                "compile Flow source {path:?}: {:?}",
                error.diagnostics
            ))
        })?;
        if definition.name != name {
            return Err(Error::Store(format!(
                "Flow source name {:?} does not match path slug {name:?}",
                definition.name
            )));
        }
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let existing = tx
                .query_row(
                    r#"SELECT workspace_id, flow_id, source_kind, name, path, content,
                              content_digest, revision, created_at, updated_at
                       FROM flow_sources
                       WHERE workspace_id = ?1 AND source_kind = ?2 AND name = ?3"#,
                    params![workspace_id, source_kind.as_str(), name],
                    read_flow_source_record,
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing.content_digest == definition.content_digest {
                    tx.commit()?;
                    return Ok(existing);
                }
                let revision = existing
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| Error::Store("Flow source revision overflowed".to_string()))?;
                let definition_json = serde_json::to_string(&definition)
                    .map_err(|error| Error::Store(error.to_string()))?;
                tx.execute(
                    r#"INSERT INTO flow_source_revisions (
                           workspace_id, flow_id, revision, content, content_digest,
                           definition_json, created_at
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                    params![
                        workspace_id,
                        existing.flow_id,
                        revision,
                        content,
                        definition.content_digest,
                        definition_json,
                        now
                    ],
                )?;
                tx.execute(
                    r#"UPDATE flow_sources
                       SET path = ?4, content = ?5, content_digest = ?6,
                           revision = ?7, updated_at = ?8
                       WHERE workspace_id = ?1 AND source_kind = ?2 AND name = ?3"#,
                    params![
                        workspace_id,
                        source_kind.as_str(),
                        name,
                        path,
                        content,
                        definition.content_digest,
                        revision,
                        now
                    ],
                )?;
                tx.commit()?;
                return Ok(FlowSourceRecord {
                    revision,
                    path: path.to_string(),
                    content: content.to_string(),
                    content_digest: definition.content_digest,
                    updated_at: now.to_string(),
                    ..existing
                });
            }

            let flow_id = Uuid::now_v7().to_string();
            let definition_json = serde_json::to_string(&definition)
                .map_err(|error| Error::Store(error.to_string()))?;
            tx.execute(
                r#"INSERT INTO flow_sources (
                       workspace_id, flow_id, source_kind, name, path, content,
                       content_digest, revision, created_at, updated_at
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)"#,
                params![
                    workspace_id,
                    flow_id,
                    source_kind.as_str(),
                    name,
                    path,
                    content,
                    definition.content_digest,
                    now
                ],
            )?;
            tx.execute(
                r#"INSERT INTO flow_source_revisions (
                       workspace_id, flow_id, revision, content, content_digest,
                       definition_json, created_at
                   ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6)"#,
                params![
                    workspace_id,
                    flow_id,
                    content,
                    definition.content_digest,
                    definition_json,
                    now
                ],
            )?;
            tx.commit()?;
            Ok(FlowSourceRecord {
                workspace_id: workspace_id.to_string(),
                flow_id,
                source_kind,
                name,
                path: path.to_string(),
                content: content.to_string(),
                content_digest: definition.content_digest,
                revision: 1,
                created_at: now.to_string(),
                updated_at: now.to_string(),
            })
        })
    }

    fn get_flow_source_by_name(
        &self,
        workspace_id: &str,
        source_kind: FlowSourceKind,
        name: &str,
    ) -> Result<Option<FlowSourceRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, flow_id, source_kind, name, path, content,
                          content_digest, revision, created_at, updated_at
                   FROM flow_sources
                   WHERE workspace_id = ?1 AND source_kind = ?2 AND name = ?3"#,
                params![workspace_id, source_kind.as_str(), name],
                read_flow_source_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_flow_sources(&self, workspace_id: &str) -> Result<Vec<FlowSourceRecord>> {
        self.with_conn(|conn| {
            let mut statement = conn.prepare(
                r#"SELECT workspace_id, flow_id, source_kind, name, path, content,
                          content_digest, revision, created_at, updated_at
                   FROM flow_sources WHERE workspace_id = ?1
                   ORDER BY source_kind ASC, name ASC"#,
            )?;
            let rows = statement.query_map(params![workspace_id], read_flow_source_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn get_flow_source(
        &self,
        workspace_id: &str,
        flow_id: &str,
    ) -> Result<Option<FlowSourceRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, flow_id, source_kind, name, path, content,
                          content_digest, revision, created_at, updated_at
                   FROM flow_sources WHERE workspace_id = ?1 AND flow_id = ?2"#,
                params![workspace_id, flow_id],
                read_flow_source_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_flow_source_revision(
        &self,
        workspace_id: &str,
        flow_id: &str,
        revision: u64,
    ) -> Result<Option<FlowSourceRevisionRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, flow_id, revision, content, content_digest,
                          definition_json, created_at
                   FROM flow_source_revisions
                   WHERE workspace_id = ?1 AND flow_id = ?2 AND revision = ?3"#,
                params![workspace_id, flow_id, revision],
                read_flow_source_revision_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn upsert_objective(&self, record: &ObjectiveRecord) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            allocate_resource_key(
                &tx,
                &record.workspace_id,
                WorkspaceResourceKind::Objective,
                &record.objective_id,
                &record.created_at,
            )?;
            tx.execute(
                r#"INSERT INTO objectives (
                    workspace_id, objective_id, title, state, body_md, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(workspace_id, objective_id) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
                    title = excluded.title,
                    state = excluded.state,
                    body_md = excluded.body_md,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.objective_id,
                    record.title,
                    record.state,
                    record.body_md,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn list_objectives(&self, workspace_id: &str, limit: usize) -> Result<Vec<ObjectiveRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, objective_id, title, state, body_md, created_at, updated_at
                   FROM objectives
                   WHERE workspace_id = ?1
                   ORDER BY updated_at DESC, objective_id ASC
                   LIMIT ?2"#,
            )?;
            let rows =
                stmt.query_map(params![workspace_id, limit as i64], read_objective_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn list_objectives_for_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<ObjectiveRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT o.workspace_id, o.objective_id, o.title, o.state, o.body_md,
                          o.created_at, o.updated_at
                   FROM objectives AS o
                   INNER JOIN objective_ticket_links AS l
                     ON l.workspace_id = o.workspace_id
                    AND l.objective_id = o.objective_id
                   WHERE o.workspace_id = ?1 AND l.ticket_id = ?2
                   ORDER BY o.updated_at DESC, o.objective_id ASC
                   LIMIT ?3"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, ticket_id, limit as i64],
                read_objective_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn get_objective(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Option<ObjectiveRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, objective_id, title, state, body_md, created_at, updated_at
                   FROM objectives
                   WHERE workspace_id = ?1 AND objective_id = ?2"#,
                params![workspace_id, objective_id],
                read_objective_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn replace_objective_ticket_links(
        &self,
        workspace_id: &str,
        objective_id: &str,
        links: &[ObjectiveTicketLinkRecord],
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM objective_ticket_links WHERE workspace_id = ?1 AND objective_id = ?2",
                params![workspace_id, objective_id],
            )?;
            for link in links {
                conn.execute(
                    r#"INSERT INTO objective_ticket_links (
                        workspace_id, objective_id, ticket_id, kind, created_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(workspace_id, objective_id, ticket_id, kind) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        created_at = excluded.created_at"#,
                    params![
                        link.workspace_id,
                        link.objective_id,
                        link.ticket_id,
                        link.kind,
                        link.created_at,
                    ],
                )?;
            }
            Ok(())
        })
    }

    fn list_objective_ticket_links(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveTicketLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, objective_id, ticket_id, kind, created_at
                   FROM objective_ticket_links
                   WHERE workspace_id = ?1 AND objective_id = ?2
                   ORDER BY ticket_id ASC, kind ASC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, objective_id],
                read_objective_ticket_link_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn insert_objective_event(&self, record: &ObjectiveEventRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO objective_events (
                    workspace_id, objective_id, event_id, kind, body_md, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    record.workspace_id,
                    record.objective_id,
                    record.event_id,
                    record.kind,
                    record.body_md,
                    record.created_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_objective_events(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveEventRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, objective_id, event_id, kind, body_md, created_at
                   FROM objective_events
                   WHERE workspace_id = ?1 AND objective_id = ?2
                   ORDER BY created_at ASC, event_id ASC"#,
            )?;
            let rows = stmt.query_map(params![workspace_id, objective_id], |row| {
                Ok(ObjectiveEventRecord {
                    workspace_id: row.get(0)?,
                    objective_id: row.get(1)?,
                    event_id: row.get(2)?,
                    kind: row.get(3)?,
                    body_md: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn upsert_objective_resource(&self, record: &ObjectiveResourceRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO objective_resources (
                    workspace_id, objective_id, resource_path, body, media_type, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(workspace_id, objective_id, resource_path) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
                    body = excluded.body,
                    media_type = excluded.media_type,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.objective_id,
                    record.resource_path,
                    record.body,
                    record.media_type,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_objective_resources(
        &self,
        workspace_id: &str,
        objective_id: &str,
    ) -> Result<Vec<ObjectiveResourceRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, objective_id, resource_path, body, media_type, created_at, updated_at
                   FROM objective_resources
                   WHERE workspace_id = ?1 AND objective_id = ?2
                   ORDER BY resource_path ASC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, objective_id],
                read_objective_resource_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn ensure_memory_document(
        &self,
        workspace_id: &str,
        default_body_md: &str,
        now: &str,
    ) -> Result<MemoryDocumentRecord> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT OR IGNORE INTO workspace_memory_documents (
                    workspace_id, body_md, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?3)"#,
                params![workspace_id, default_body_md, now],
            )?;
            conn.query_row(
                r#"SELECT workspace_id, body_md, created_at, updated_at
                   FROM workspace_memory_documents
                   WHERE workspace_id = ?1"#,
                params![workspace_id],
                read_memory_document_record,
            )
            .map_err(Error::from)
        })
    }

    fn get_memory_document(&self, workspace_id: &str) -> Result<Option<MemoryDocumentRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, body_md, created_at, updated_at
                   FROM workspace_memory_documents
                   WHERE workspace_id = ?1"#,
                params![workspace_id],
                read_memory_document_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn upsert_memory_document(&self, record: &MemoryDocumentRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workspace_memory_documents (
                    workspace_id, body_md, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    body_md = excluded.body_md,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.body_md,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn upsert_memory_staging_record(&self, record: &MemoryStagingRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO memory_staging_records (
                    workspace_id, candidate_id, raw_json, source_path, imported_at
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(workspace_id, candidate_id) DO UPDATE SET
                    raw_json = excluded.raw_json,
                    source_path = excluded.source_path,
                    imported_at = excluded.imported_at"#,
                params![
                    record.workspace_id,
                    record.candidate_id,
                    record.raw_json,
                    record.source_path,
                    record.imported_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_memory_staging_records(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryStagingRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, candidate_id, raw_json, source_path, imported_at
                   FROM memory_staging_records
                   WHERE workspace_id = ?1
                   ORDER BY imported_at DESC, candidate_id ASC
                   LIMIT ?2"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_memory_staging_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn get_memory_staging_record(
        &self,
        workspace_id: &str,
        candidate_id: &str,
    ) -> Result<Option<MemoryStagingRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, candidate_id, raw_json, source_path, imported_at
                   FROM memory_staging_records
                   WHERE workspace_id = ?1 AND candidate_id = ?2"#,
                params![workspace_id, candidate_id],
                read_memory_staging_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn delete_memory_staging_record(&self, workspace_id: &str, candidate_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "DELETE FROM memory_staging_records WHERE workspace_id = ?1 AND candidate_id = ?2",
                params![workspace_id, candidate_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn count_memory_staging_records(&self, workspace_id: &str) -> Result<usize> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_staging_records WHERE workspace_id = ?1",
                params![workspace_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count as usize)
            .map_err(Error::from)
        })
    }

    fn insert_memory_staging_resolution(
        &self,
        record: &MemoryStagingResolutionRecord,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO memory_staging_resolutions (
                    workspace_id, candidate_id, action, reason, affected_refs_json,
                    staging_raw_json, source_path, imported_at, resolved_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    record.workspace_id,
                    record.candidate_id,
                    record.action,
                    record.reason,
                    record.affected_refs_json,
                    record.staging_raw_json,
                    record.source_path,
                    record.imported_at,
                    record.resolved_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_memory_staging_resolutions(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryStagingResolutionRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, candidate_id, action, reason, affected_refs_json,
                          staging_raw_json, source_path, imported_at, resolved_at
                   FROM memory_staging_resolutions
                   WHERE workspace_id = ?1
                   ORDER BY resolved_at DESC, candidate_id ASC
                   LIMIT ?2"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_memory_staging_resolution_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn upsert_account(&self, record: &AccountRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO accounts (account_id, kind, handle, display_name, created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(account_id) DO UPDATE SET
                       handle = excluded.handle,
                       display_name = excluded.display_name,
                       updated_at = excluded.updated_at"#,
                params![record.account_id, record.kind, record.handle, record.display_name, record.created_at, record.updated_at],
            )?;
            Ok(())
        })
    }

    fn get_account(&self, account_id: &str) -> Result<Option<AccountRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                account_select_sql("WHERE account_id = ?1").as_str(),
                params![account_id],
                read_account_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_account_by_handle(&self, kind: &str, handle: &str) -> Result<Option<AccountRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                account_select_sql("WHERE kind = ?1 AND handle = ?2").as_str(),
                params![kind, handle],
                read_account_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn upsert_user(&self, record: &UserRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO users (user_id, account_id, handle, display_name, created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(user_id) DO UPDATE SET
                       handle = excluded.handle,
                       display_name = excluded.display_name,
                       updated_at = excluded.updated_at"#,
                params![record.user_id, record.account_id, record.handle, record.display_name, record.created_at, record.updated_at],
            )?;
            Ok(())
        })
    }

    fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                user_select_sql("WHERE user_id = ?1").as_str(),
                params![user_id],
                read_user_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_user_by_handle(&self, handle: &str) -> Result<Option<UserRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                user_select_sql("WHERE handle = ?1").as_str(),
                params![handle],
                read_user_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn any_user(&self) -> Result<Option<UserRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                user_select_sql("ORDER BY created_at ASC LIMIT 1").as_str(),
                [],
                read_user_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn upsert_passkey_credential(&self, record: &PasskeyCredentialRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO passkey_credentials (credential_id, user_id, public_key_cose, transports_json, sign_count, created_at, last_used_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                   ON CONFLICT(credential_id) DO UPDATE SET
                       public_key_cose = excluded.public_key_cose,
                       transports_json = excluded.transports_json,
                       sign_count = excluded.sign_count,
                       last_used_at = excluded.last_used_at"#,
                params![record.credential_id, record.user_id, record.public_key_cose, record.transports_json, record.sign_count, record.created_at, record.last_used_at],
            )?;
            Ok(())
        })
    }

    fn get_passkey_credential(
        &self,
        credential_id: &str,
    ) -> Result<Option<PasskeyCredentialRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                passkey_select_sql("WHERE credential_id = ?1").as_str(),
                params![credential_id],
                read_passkey_credential_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_passkey_credentials_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<PasskeyCredentialRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                passkey_select_sql("WHERE user_id = ?1 ORDER BY created_at ASC").as_str(),
            )?;
            let rows = stmt.query_map(params![user_id], read_passkey_credential_record)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn put_auth_challenge(&self, record: &AuthChallengeRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO auth_challenges (challenge_id, ceremony, challenge, user_id, rp_id, origin, state_json, expires_at, created_at, consumed_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
                params![record.challenge_id, record.ceremony, record.challenge, record.user_id, record.rp_id, record.origin, record.state_json, record.expires_at, record.created_at, record.consumed_at],
            )?;
            Ok(())
        })
    }

    fn consume_auth_challenge(
        &self,
        challenge: &str,
        ceremony: &str,
        consumed_at: &str,
    ) -> Result<Option<AuthChallengeRecord>> {
        self.with_conn(|conn| {
            let record = conn
                .query_row(
                    auth_challenge_select_sql(
                        "WHERE challenge = ?1 AND ceremony = ?2 AND consumed_at IS NULL",
                    )
                    .as_str(),
                    params![challenge, ceremony],
                    read_auth_challenge_record,
                )
                .optional()?;
            if let Some(record) = record.as_ref() {
                conn.execute(
                    "UPDATE auth_challenges SET consumed_at = ?2 WHERE challenge_id = ?1",
                    params![record.challenge_id, consumed_at],
                )?;
            }
            Ok(record)
        })
    }

    fn consume_auth_challenge_by_id(
        &self,
        challenge_id: &str,
        ceremony: &str,
        consumed_at: &str,
    ) -> Result<Option<AuthChallengeRecord>> {
        self.with_conn(|conn| {
            let record = conn
                .query_row(
                    auth_challenge_select_sql(
                        "WHERE challenge_id = ?1 AND ceremony = ?2 AND consumed_at IS NULL",
                    )
                    .as_str(),
                    params![challenge_id, ceremony],
                    read_auth_challenge_record,
                )
                .optional()?;
            if let Some(record) = record.as_ref() {
                conn.execute(
                    "UPDATE auth_challenges SET consumed_at = ?2 WHERE challenge_id = ?1",
                    params![record.challenge_id, consumed_at],
                )?;
            }
            Ok(record)
        })
    }

    fn create_browser_session(&self, record: &BrowserSessionRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO browser_sessions (session_id, token_hash, user_id, created_at, expires_at, revoked_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![record.session_id, record.token_hash, record.user_id, record.created_at, record.expires_at, record.revoked_at],
            )?;
            Ok(())
        })
    }

    fn resolve_browser_session(&self, token_hash: &str) -> Result<Option<BrowserSessionRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                browser_session_select_sql("WHERE token_hash = ?1 AND revoked_at IS NULL").as_str(),
                params![token_hash],
                read_browser_session_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn revoke_browser_session(&self, token_hash: &str, revoked_at: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE browser_sessions SET revoked_at = ?2 WHERE token_hash = ?1 AND revoked_at IS NULL",
                params![token_hash, revoked_at],
            )?;
            Ok(changed > 0)
        })
    }

    fn create_api_token(&self, record: &ApiTokenRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO api_tokens (token_id, token_hash, user_id, label, created_at, expires_at, revoked_at, last_used_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![record.token_id, record.token_hash, record.user_id, record.label, record.created_at, record.expires_at, record.revoked_at, record.last_used_at],
            )?;
            Ok(())
        })
    }

    fn resolve_api_token(&self, token_hash: &str) -> Result<Option<ApiTokenRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                api_token_select_sql("WHERE token_hash = ?1 AND revoked_at IS NULL").as_str(),
                params![token_hash],
                read_api_token_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn mark_api_token_used(&self, token_hash: &str, used_at: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?2 WHERE token_hash = ?1",
                params![token_hash, used_at],
            )?;
            Ok(())
        })
    }

    fn try_create_device_login_flow(&self, record: &DeviceLoginFlowRecord) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"INSERT INTO device_login_flows (device_code, user_code, verification_uri, client_name, user_id, api_token_id, issued_access_token, created_at, expires_at, approved_at, consumed_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                   ON CONFLICT(user_code) DO NOTHING"#,
                params![record.device_code, record.user_code, record.verification_uri, record.client_name, record.user_id, record.api_token_id, record.issued_access_token, record.created_at, record.expires_at, record.approved_at, record.consumed_at],
            )?;
            Ok(changed == 1)
        })
    }

    fn get_device_login_flow_by_user_code(
        &self,
        user_code: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                device_login_select_sql("WHERE user_code = ?1").as_str(),
                params![user_code],
                read_device_login_flow_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_device_login_flow_by_device_code(
        &self,
        device_code: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                device_login_select_sql("WHERE device_code = ?1").as_str(),
                params![device_code],
                read_device_login_flow_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn approve_device_login_flow(
        &self,
        device_code: &str,
        user_id: &str,
        api_token_id: &str,
        issued_access_token: &str,
        approved_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE device_login_flows
                   SET user_id = ?2, api_token_id = ?3, issued_access_token = ?4, approved_at = ?5
                   WHERE device_code = ?1 AND approved_at IS NULL AND consumed_at IS NULL"#,
                params![
                    device_code,
                    user_id,
                    api_token_id,
                    issued_access_token,
                    approved_at
                ],
            )?;
            Ok(changed > 0)
        })
    }

    fn consume_device_login_token(
        &self,
        device_code: &str,
        consumed_at: &str,
    ) -> Result<Option<DeviceLoginFlowRecord>> {
        self.with_conn(|conn| {
            let record = conn.query_row(device_login_select_sql("WHERE device_code = ?1 AND approved_at IS NOT NULL AND consumed_at IS NULL").as_str(), params![device_code], read_device_login_flow_record).optional()?;
            if record.is_some() {
                conn.execute("UPDATE device_login_flows SET consumed_at = ?2, issued_access_token = NULL WHERE device_code = ?1", params![device_code, consumed_at])?;
            }
            Ok(record)
        })
    }

    fn upsert_worker_registry(&self, record: &WorkerRegistryRecord) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let removal_blocks_upsert: bool = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM worker_removal_operations
                    WHERE workspace_id = ?1 AND runtime_id = ?2
                      AND worker_id = ?3
                      AND state IN ('executing', 'failed', 'succeeded')
                )",
                params![
                    record.workspace_id,
                    record.worker.runtime_id,
                    record.worker.worker_id
                ],
                |row| row.get(0),
            )?;
            if removal_blocks_upsert {
                return Ok(());
            }
            tx.execute(
                r#"INSERT INTO worker_registry (
                    workspace_id, runtime_id, worker_id, display_name, profile,
                    retention_state, transcript_ref, session_ref, summary_ref,
                    diagnostics_ref, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(workspace_id, worker_id) DO UPDATE SET
                    runtime_id = excluded.runtime_id,
                    display_name = excluded.display_name,
                    profile = excluded.profile,
                    retention_state = CASE
                        WHEN worker_registry.retention_state = 'pinned' AND excluded.retention_state = 'normal'
                        THEN worker_registry.retention_state
                        ELSE excluded.retention_state
                    END,
                    transcript_ref = excluded.transcript_ref,
                    session_ref = excluded.session_ref,
                    summary_ref = excluded.summary_ref,
                    diagnostics_ref = excluded.diagnostics_ref,
                    updated_at = excluded.updated_at
                WHERE NOT EXISTS (
                    SELECT 1 FROM worker_removal_operations retention
                    WHERE retention.workspace_id = excluded.workspace_id
                      AND retention.runtime_id = excluded.runtime_id
                      AND retention.worker_id = excluded.worker_id
                      AND retention.state IN ('executing', 'failed', 'succeeded')
                )"#,
                params![
                    record.workspace_id,
                    record.worker.runtime_id,
                    record.worker.worker_id,
                    record.display_name,
                    record.profile,
                    record.retention_state,
                    record.transcript_ref,
                    record.session_ref,
                    record.summary_ref,
                    record.diagnostics_ref,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            allocate_resource_key(
                &tx,
                &record.workspace_id,
                WorkspaceResourceKind::Worker,
                &record.worker.worker_id,
                &record.created_at,
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn get_worker_registry(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<WorkerRegistryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                worker_registry_select_sql(
                    "WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3",
                )
                .as_str(),
                params![workspace_id, worker.runtime_id, worker.worker_id],
                read_worker_registry_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn has_active_worker_create_reservation(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT EXISTS(
                     SELECT 1
                     FROM worker_create_reservations
                     WHERE workspace_id = ?1
                       AND runtime_id = ?2
                       AND worker_id = ?3
                       AND state = 'reserved'
                   )"#,
                params![workspace_id, worker.runtime_id, worker.worker_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(Error::from)
        })
    }

    fn list_worker_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkerRegistryRecord>> {
        self.with_conn(|conn| {
            let sql = worker_registry_select_sql(
                "WHERE workspace_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
            );
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_worker_registry_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn update_worker_retention(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE worker_registry
                   SET retention_state = ?4, updated_at = ?5
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                     AND NOT EXISTS (
                       SELECT 1 FROM worker_removal_operations retention
                       WHERE retention.workspace_id = ?1 AND retention.runtime_id = ?2
                         AND retention.worker_id = ?3
                         AND retention.state IN ('executing', 'failed')
                     )"#,
                params![
                    workspace_id,
                    worker.runtime_id,
                    worker.worker_id,
                    retention_state,
                    updated_at
                ],
            )?;
            Ok(changed > 0)
        })
    }

    fn delete_worker_registry(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                r#"UPDATE worker_workdir_links
                   SET unlinked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND unlinked_at IS NULL"#,
                params![workspace_id, worker.runtime_id, worker.worker_id],
            )?;
            let changed = tx.execute(
                "DELETE FROM worker_registry WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3",
                params![workspace_id, worker.runtime_id, worker.worker_id],
            )?;
            tx.commit()?;
            Ok(changed > 0)
        })
    }

    fn create_worker_control_grant(
        &self,
        record: &WorkerControlGrantRecord,
    ) -> Result<WorkerControlGrantRecord> {
        self.with_conn(|conn| {
            let permissions_json = serde_json::to_string(&record.permissions)
                .map_err(|error| Error::Store(error.to_string()))?;
            conn.execute(
                r#"INSERT INTO worker_control_grants (
                    workspace_id, grant_id,
                    controller_runtime_id, controller_worker_id,
                    subject_runtime_id, subject_worker_id,
                    relation, origin, permissions_json, operation_id, created_at, revoked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT (
                    workspace_id,
                    controller_worker_id,
                    operation_id
                ) DO NOTHING"#,
                params![
                    record.workspace_id,
                    record.grant_id,
                    record.controller.runtime_id,
                    record.controller.worker_id,
                    record.subject.runtime_id,
                    record.subject.worker_id,
                    record.relation,
                    record.origin,
                    permissions_json,
                    record.operation_id,
                    record.created_at,
                    record.revoked_at,
                ],
            )?;

            let persisted = read_worker_control_grant_by_operation(
                conn,
                record.workspace_id.as_str(),
                &record.controller,
                record.operation_id.as_str(),
            )?
            .ok_or_else(|| Error::Store("worker control grant was not persisted".to_string()))?;
            if persisted.subject != record.subject
                || persisted.relation != record.relation
                || persisted.origin != record.origin
                || persisted.permissions != record.permissions
            {
                return Err(Error::InvalidInput(format!(
                    "worker control operation `{}` was already used with different input",
                    record.operation_id
                )));
            }
            Ok(persisted)
        })
    }

    fn get_worker_control_grant(
        &self,
        workspace_id: &str,
        grant_id: &str,
    ) -> Result<Option<WorkerControlGrantRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, grant_id,
                          controller_runtime_id, controller_worker_id,
                          subject_runtime_id, subject_worker_id,
                          relation, origin, permissions_json, operation_id, created_at, revoked_at
                   FROM worker_control_grants
                   WHERE workspace_id = ?1 AND grant_id = ?2"#,
                params![workspace_id, grant_id],
                read_worker_control_grant_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn get_worker_control_grant_by_operation(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        operation_id: &str,
    ) -> Result<Option<WorkerControlGrantRecord>> {
        self.with_conn(|conn| {
            read_worker_control_grant_by_operation(conn, workspace_id, controller, operation_id)
        })
    }

    fn get_active_worker_control_grant(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        subject: &RuntimeWorkerRef,
    ) -> Result<Option<WorkerControlGrantRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, grant_id,
                          controller_runtime_id, controller_worker_id,
                          subject_runtime_id, subject_worker_id,
                          relation, origin, permissions_json, operation_id, created_at, revoked_at
                   FROM worker_control_grants
                   WHERE workspace_id = ?1
                     AND controller_runtime_id = ?2 AND controller_worker_id = ?3
                     AND subject_runtime_id = ?4 AND subject_worker_id = ?5
                     AND revoked_at IS NULL
                   ORDER BY created_at DESC
                   LIMIT 1"#,
                params![
                    workspace_id,
                    controller.runtime_id,
                    controller.worker_id,
                    subject.runtime_id,
                    subject.worker_id,
                ],
                read_worker_control_grant_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_active_worker_control_grants(
        &self,
        workspace_id: &str,
        controller: &RuntimeWorkerRef,
        limit: usize,
    ) -> Result<Vec<WorkerControlGrantRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, grant_id,
                          controller_runtime_id, controller_worker_id,
                          subject_runtime_id, subject_worker_id,
                          relation, origin, permissions_json, operation_id, created_at, revoked_at
                   FROM worker_control_grants
                   WHERE workspace_id = ?1
                     AND controller_runtime_id = ?2 AND controller_worker_id = ?3
                     AND revoked_at IS NULL
                   ORDER BY created_at ASC, grant_id ASC
                   LIMIT ?4"#,
            )?;
            let rows = stmt.query_map(
                params![
                    workspace_id,
                    controller.runtime_id,
                    controller.worker_id,
                    limit as i64,
                ],
                read_worker_control_grant_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn revoke_worker_control_grant(
        &self,
        workspace_id: &str,
        grant_id: &str,
        revoked_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE worker_control_grants
                   SET revoked_at = ?3
                   WHERE workspace_id = ?1 AND grant_id = ?2 AND revoked_at IS NULL"#,
                params![workspace_id, grant_id, revoked_at],
            )?;
            Ok(changed > 0)
        })
    }

    fn get_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<TicketAssignmentOperationRecord>> {
        self.with_conn(|conn| read_assignment_operation(conn, workspace_id, operation_id))
    }

    fn reserve_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
        ticket_id: &str,
        runtime_id: &str,
        worker_id: Option<&str>,
        request_fingerprint: &str,
        created_at: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let inserted = conn.execute(
                r#"INSERT OR IGNORE INTO ticket_assignment_operations (
                    workspace_id, operation_id, action, ticket_id, runtime_id, worker_id,
                    assignment_id, expected_assignment_id, created_at, request_fingerprint
                ) VALUES (?1, ?2, 'assign', ?3, ?4, ?5, NULL, NULL, ?6, ?7)"#,
                params![
                    workspace_id,
                    operation_id,
                    ticket_id,
                    runtime_id,
                    worker_id,
                    created_at,
                    request_fingerprint,
                ],
            )?;
            if inserted > 0 {
                return Ok(());
            }
            let existing = read_assignment_operation(conn, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::TicketAssignmentConflict(format!(
                        "assignment operation {operation_id} could not be reserved"
                    ))
                })?;
            if existing.action == "assign"
                && existing.ticket_id == ticket_id
                && existing.runtime_id.as_deref() == Some(runtime_id)
                && (worker_id.is_none()
                    || existing
                        .worker
                        .as_ref()
                        .map(|worker| worker.worker_id.as_str())
                        == worker_id)
                && existing.expected_assignment_id.is_none()
                && existing.request_fingerprint.as_deref() == Some(request_fingerprint)
            {
                Ok(())
            } else {
                Err(Error::TicketAssignmentConflict(format!(
                    "assignment operation {operation_id} was already used with different input"
                )))
            }
        })
    }

    fn bind_ticket_assignment_operation_worker(
        &self,
        workspace_id: &str,
        operation_id: &str,
        worker_id: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let updated = conn.execute(
                r#"UPDATE ticket_assignment_operations
                   SET worker_id = ?3
                   WHERE workspace_id = ?1 AND operation_id = ?2
                     AND assignment_id IS NULL AND (worker_id IS NULL OR worker_id = ?3)"#,
                params![workspace_id, operation_id, worker_id],
            )?;
            if updated == 1 {
                return Ok(());
            }
            Err(Error::TicketAssignmentConflict(format!(
                "assignment operation {operation_id} cannot bind Worker {worker_id}"
            )))
        })
    }

    fn rollback_ticket_assignment_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let operation = read_assignment_operation(&transaction, workspace_id, operation_id)?;
            let Some(operation) = operation else {
                transaction.commit()?;
                return Ok(());
            };
            if operation.action != "assign" {
                return Err(Error::TicketAssignmentConflict(format!(
                    "Ticket operation `{operation_id}` is `{}`, expected `assign`",
                    operation.action
                )));
            }

            transaction.execute(
                "DELETE FROM ticket_assignment_operations WHERE workspace_id = ?1 AND operation_id = ?2",
                params![workspace_id, operation_id],
            )?;
            if let Some(assignment_id) = operation.assignment_id {
                transaction.execute(
                    "DELETE FROM ticket_worker_assignment_events WHERE workspace_id = ?1 AND ticket_id = ?2 AND assignment_id = ?3",
                    params![workspace_id, operation.ticket_id, assignment_id],
                )?;
                transaction.execute(
                    "DELETE FROM ticket_worker_assignments WHERE workspace_id = ?1 AND ticket_id = ?2 AND assignment_id = ?3",
                    params![workspace_id, operation.ticket_id, assignment_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    fn list_current_ticket_role_assignments(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Vec<TicketRoleAssignmentRecord>> {
        self.with_conn(|conn| {
            let sql = ticket_role_assignment_select_sql(
                "WHERE current.workspace_id = ?1 AND current.ticket_id = ?2 \
             ORDER BY current.role, a.assigned_at, a.assignment_id",
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params![workspace_id, ticket_id],
                read_ticket_role_assignment_record,
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    fn get_current_ticket_role_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        role: TicketAssignmentRole,
    ) -> Result<Option<TicketRoleAssignmentRecord>> {
        self.with_conn(|conn| {
            let sql = ticket_role_assignment_select_sql(
                "WHERE current.workspace_id = ?1 AND current.ticket_id = ?2 AND current.role = ?3 \
             ORDER BY a.assigned_at, a.assignment_id LIMIT 1",
            );
            Ok(conn
                .query_row(
                    &sql,
                    params![workspace_id, ticket_id, role.as_str()],
                    read_ticket_role_assignment_record,
                )
                .optional()?)
        })
    }

    fn get_current_ticket_role_assignment_for_worker(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<TicketRoleAssignmentRecord>> {
        self.with_conn(|conn| {
            let sql = ticket_role_assignment_select_sql(
                "WHERE current.workspace_id = ?1 \
                   AND current.principal_kind = 'worker' \
                   AND current.runtime_id = ?2 AND current.worker_id = ?3 \
                 ORDER BY a.assigned_at, a.assignment_id LIMIT 1",
            );
            Ok(conn
                .query_row(
                    &sql,
                    params![workspace_id, worker.runtime_id, worker.worker_id],
                    read_ticket_role_assignment_record,
                )
                .optional()?)
        })
    }

    fn set_current_ticket_role_assignment(
        &self,
        record: &TicketRoleAssignmentRecord,
        expected_assignment_id: Option<&str>,
        event_id: &str,
        operation_id: &str,
        allow_reassign: bool,
    ) -> Result<TicketRoleAssignmentRecord> {
        validate_ticket_assignment_role_principal(record.role, &record.principal)?;
        let principal_json = serde_json::to_string(&record.principal).map_err(|error| {
            Error::Store(format!("serialize Ticket assignment principal: {error}"))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(b"ticket-role-assignment:set:v1\0");
        for value in [
            record.workspace_id.as_str(),
            record.ticket_id.as_str(),
            record.role.as_str(),
            principal_json.as_str(),
            record.assigned_by.as_str(),
            expected_assignment_id.unwrap_or(""),
            if allow_reassign { "reassign" } else { "assign" },
        ] {
            hasher.update(value.as_bytes());
            hasher.update([0]);
        }
        let request_fingerprint = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (principal_id, runtime_id, worker_id) = match &record.principal {
            TicketAssignmentPrincipal::User { account_id } => {
                (Some(account_id.as_str()), None, None)
            }
            TicketAssignmentPrincipal::Worker {
                runtime_id,
                worker_id,
            } => (None, Some(runtime_id.as_str()), Some(worker_id.as_str())),
            TicketAssignmentPrincipal::WorkspaceAgent { agent_key } => {
                (Some(agent_key.as_str()), None, None)
            }
        };
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(worker) = record.principal.worker() {
                ensure_worker_assignment_available(&tx, &record.workspace_id, &worker)?;
            }

            let existing_operation: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT request_fingerprint, assignment_id
                       FROM ticket_assignment_operations
                      WHERE workspace_id = ?1 AND operation_id = ?2",
                    params![record.workspace_id, operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((persisted_fingerprint, assignment_id)) = existing_operation {
                if persisted_fingerprint != request_fingerprint {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "operation `{operation_id}` was already used for different Ticket assignment input"
                    )));
                }
                let assignment_id = assignment_id.ok_or_else(|| {
                    Error::Store(format!(
                        "Ticket role assignment operation `{operation_id}` is missing assignment identity"
                    ))
                })?;
                let persisted = read_ticket_role_assignment_by_id(&tx, &record.workspace_id, &assignment_id)?
                    .ok_or_else(|| {
                        Error::TicketAssignmentConflict(format!(
                            "assignment `{assignment_id}` recorded by operation `{operation_id}` no longer exists"
                        ))
                    })?;
                tx.commit()?;
                return Ok(persisted);
            }

            let current_sql = ticket_role_assignment_select_sql(
                "WHERE current.workspace_id = ?1 AND current.ticket_id = ?2 AND current.role = ?3 \
                 ORDER BY a.assigned_at, a.assignment_id",
            );
            let mut current_stmt = tx.prepare(&current_sql)?;
            let current = current_stmt
                .query_map(
                    params![record.workspace_id, record.ticket_id, record.role.as_str()],
                    read_ticket_role_assignment_record,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(current_stmt);

            let replaced = if let Some(expected_assignment_id) = expected_assignment_id {
                let Some(expected) = current
                    .iter()
                    .find(|assignment| assignment.assignment_id == expected_assignment_id)
                else {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "Ticket `{}` role `{}` expected assignment `{expected_assignment_id}`, current is {:?}",
                        record.ticket_id,
                        record.role.as_str(),
                        current.first().map(|value| value.assignment_id.as_str())
                    )));
                };
                if !allow_reassign {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "Ticket `{}` role `{}` is already assigned as `{}`",
                        record.ticket_id,
                        record.role.as_str(),
                        expected.assignment_id
                    )));
                }
                Some(expected.clone())
            } else if record.role.is_singleton() && !current.is_empty() {
                return Err(Error::TicketAssignmentConflict(format!(
                    "Ticket `{}` already has an active `{}` assignment `{}`",
                    record.ticket_id,
                    record.role.as_str(),
                    current[0].assignment_id
                )));
            } else {
                None
            };

            tx.execute(
                "INSERT INTO ticket_assignment_operations (
                     workspace_id, operation_id, action, ticket_id, role, principal_kind,
                     principal_id, runtime_id, worker_id, assignment_id,
                     expected_assignment_id, created_at, request_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    record.workspace_id,
                    operation_id,
                    if replaced.is_some() { "reassign" } else { "assign" },
                    record.ticket_id,
                    record.role.as_str(),
                    record.principal.kind(),
                    principal_id,
                    runtime_id,
                    worker_id,
                    record.assignment_id,
                    expected_assignment_id,
                    record.assigned_at,
                    request_fingerprint,
                ],
            )?;
            tx.execute(
                "INSERT INTO ticket_worker_assignments (
                     workspace_id, ticket_id, assignment_id, role, principal_kind,
                     principal_id, runtime_id, worker_id, assigned_by, assigned_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.workspace_id,
                    record.ticket_id,
                    record.assignment_id,
                    record.role.as_str(),
                    record.principal.kind(),
                    principal_id,
                    runtime_id,
                    worker_id,
                    record.assigned_by,
                    record.assigned_at,
                ],
            )?;
            if let Some(previous) = &replaced {
                tx.execute(
                    "DELETE FROM ticket_current_worker_assignments
                      WHERE workspace_id = ?1 AND ticket_id = ?2 AND role = ?3 AND assignment_id = ?4",
                    params![
                        record.workspace_id,
                        record.ticket_id,
                        record.role.as_str(),
                        previous.assignment_id,
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO ticket_current_worker_assignments (
                     workspace_id, ticket_id, role, assignment_id, principal_kind,
                     principal_id, runtime_id, worker_id, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.workspace_id,
                    record.ticket_id,
                    record.role.as_str(),
                    record.assignment_id,
                    record.principal.kind(),
                    principal_id,
                    runtime_id,
                    worker_id,
                    record.assigned_at,
                ],
            )?;
            tx.execute(
                "INSERT INTO ticket_worker_assignment_events (
                     workspace_id, ticket_id, role, event_id, action, assignment_id,
                     previous_assignment_id, actor, created_at, operation_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.workspace_id,
                    record.ticket_id,
                    record.role.as_str(),
                    event_id,
                    if replaced.is_some() { "reassigned" } else { "assigned" },
                    record.assignment_id,
                    replaced.as_ref().map(|value| value.assignment_id.as_str()),
                    record.assigned_by,
                    record.assigned_at,
                    operation_id,
                ],
            )?;
                tx.commit()?;
            Ok(record.clone())
        })
    }

    fn start_ready_ticket_with_coder_assignment(
        &self,
        record: &TicketRoleAssignmentRecord,
        event_id: &str,
        operation_id: &str,
    ) -> Result<TicketRoleAssignmentRecord> {
        if record.role != TicketAssignmentRole::Coder
            || !matches!(
                record.principal,
                TicketAssignmentPrincipal::Worker { .. } | TicketAssignmentPrincipal::User { .. }
            )
        {
            return Err(Error::TicketAssignmentConflict(
                "manual Ticket start requires a Coder user or Worker principal".to_string(),
            ));
        }
        let (principal_id, runtime_id, worker_id) = match &record.principal {
            TicketAssignmentPrincipal::User { account_id } => {
                (Some(account_id.as_str()), None, None)
            }
            TicketAssignmentPrincipal::Worker {
                runtime_id,
                worker_id,
            } => (None, Some(runtime_id.as_str()), Some(worker_id.as_str())),
            TicketAssignmentPrincipal::WorkspaceAgent { .. } => {
                return Err(Error::TicketAssignmentConflict(
                    "Workspace agent principal cannot occupy the Coder role".to_string(),
                ));
            }
        };
        let principal_json = serde_json::to_string(&record.principal).map_err(|error| {
            Error::Store(format!("serialize Ticket assignment principal: {error}"))
        })?;
        let mut hasher = Sha256::new();
        for value in [
            "ticket-role-assignment:manual-start:v1",
            record.workspace_id.as_str(),
            record.ticket_id.as_str(),
            principal_json.as_str(),
            record.assigned_by.as_str(),
        ] {
            hasher.update(value.as_bytes());
            hasher.update([0]);
        }
        let fingerprint = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(worker) = record.principal.worker() {
                ensure_worker_assignment_available(&tx, &record.workspace_id, &worker)?;
            }
            let existing_operation: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT request_fingerprint, assignment_id FROM ticket_assignment_operations
                      WHERE workspace_id = ?1 AND operation_id = ?2",
                    params![record.workspace_id, operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((persisted, assignment_id)) = existing_operation {
                if persisted != fingerprint {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "operation `{operation_id}` was already used for different manual Coder assignment input"
                    )));
                }
                let assignment_id = assignment_id.ok_or_else(|| {
                    Error::TicketAssignmentConflict(format!(
                        "manual Coder operation `{operation_id}` has no result"
                    ))
                })?;
                return read_ticket_role_assignment_by_id(&tx, &record.workspace_id, &assignment_id)?
                    .ok_or_else(|| Error::TicketAssignmentConflict(format!(
                        "manual Coder assignment `{assignment_id}` no longer exists"
                    )));
            }

            let (state, repository_id, ref_selector): (String, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT workflow_state, repository_id, ref_selector FROM typed_tickets
                      WHERE workspace_id = ?1 AND ticket_id = ?2",
                    params![record.workspace_id, record.ticket_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            if state != "ready" {
                return Err(Error::TicketAssignmentConflict(format!(
                    "manual Coder assignment requires ready Ticket; current state is `{state}`"
                )));
            }
            if repository_id.as_deref().is_none_or(str::is_empty)
                || ref_selector.as_deref().is_none_or(str::is_empty)
            {
                return Err(Error::TicketAssignmentConflict(
                    "manual Coder assignment requires a valid repository/ref target".to_string(),
                ));
            }
            let unresolved_blockers: i64 = tx.query_row(
                "SELECT COUNT(*)
                   FROM typed_ticket_relations AS relation
                   JOIN typed_tickets AS related
                     ON related.workspace_id = relation.workspace_id
                    AND related.ticket_id = CASE
                        WHEN relation.ticket_id = ?2 THEN relation.target
                        ELSE relation.ticket_id
                    END
                  WHERE relation.workspace_id = ?1
                    AND (
                        (relation.ticket_id = ?2 AND relation.kind = 'depends_on')
                        OR (relation.target = ?2 AND relation.kind = 'blocks')
                    )
                    AND related.workflow_state NOT IN ('done', 'closed')",
                params![record.workspace_id, record.ticket_id],
                |row| row.get(0),
            )?;
            if unresolved_blockers != 0 {
                return Err(Error::TicketAssignmentConflict(
                    "manual Coder assignment is blocked by unresolved Ticket relations".to_string(),
                ));
            }
            let conflicting: i64 = tx.query_row(
                "SELECT COUNT(*) FROM ticket_current_worker_assignments
                  WHERE workspace_id = ?1 AND ticket_id = ?2
                    AND role IN ('orchestrator', 'coder')",
                params![record.workspace_id, record.ticket_id],
                |row| row.get(0),
            )?;
            if conflicting != 0 {
                return Err(Error::TicketAssignmentConflict(
                    "manual Coder assignment requires no active Orchestrator or Coder assignment"
                        .to_string(),
                ));
            }

            tx.execute(
                "INSERT INTO ticket_assignment_operations (
                     workspace_id, operation_id, action, ticket_id, role, principal_kind,
                     principal_id, runtime_id, worker_id, assignment_id, created_at,
                     request_fingerprint
                 ) VALUES (?1, ?2, 'assign', ?3, 'coder', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![record.workspace_id, operation_id, record.ticket_id,
                    record.principal.kind(), principal_id, runtime_id, worker_id,
                    record.assignment_id, record.assigned_at, fingerprint],
            )?;
            tx.execute(
                "INSERT INTO ticket_worker_assignments (
                     workspace_id, ticket_id, assignment_id, role, principal_kind,
                     principal_id, runtime_id, worker_id, assigned_by, assigned_at
                 ) VALUES (?1, ?2, ?3, 'coder', ?4, ?5, ?6, ?7, ?8, ?9)",
                params![record.workspace_id, record.ticket_id, record.assignment_id,
                    record.principal.kind(), principal_id, runtime_id, worker_id,
                    record.assigned_by, record.assigned_at],
            )?;
            tx.execute(
                "INSERT INTO ticket_current_worker_assignments (
                     workspace_id, ticket_id, role, assignment_id, principal_kind,
                     principal_id, runtime_id, worker_id, updated_at
                 ) VALUES (?1, ?2, 'coder', ?3, ?4, ?5, ?6, ?7, ?8)",
                params![record.workspace_id, record.ticket_id, record.assignment_id,
                    record.principal.kind(), principal_id, runtime_id, worker_id,
                    record.assigned_at],
            )?;
            tx.execute(
                "INSERT INTO ticket_worker_assignment_events (
                     workspace_id, ticket_id, role, event_id, action, assignment_id,
                     actor, created_at, operation_id
                 ) VALUES (?1, ?2, 'coder', ?3, 'assigned', ?4, ?5, ?6, ?7)",
                params![record.workspace_id, record.ticket_id, event_id, record.assignment_id,
                    record.assigned_by, record.assigned_at, operation_id],
            )?;
            let event_index: i64 = tx.query_row(
                "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events
                  WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![record.workspace_id, record.ticket_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO typed_ticket_events (
                     workspace_id, ticket_id, event_index, kind, author, at,
                     from_state, to_state, reason, state_field, heading, body
                 ) VALUES (?1, ?2, ?3, 'state_changed', ?4, ?5, 'ready', 'inprogress',
                           'manual Coder assignment accepted', 'state', 'State changed', '')",
                params![record.workspace_id, record.ticket_id, event_index,
                    record.assigned_by, record.assigned_at],
            )?;
            for (key, value) in [
                ("event_id", event_id),
                ("assignment_id", record.assignment_id.as_str()),
                ("assignment_role", "coder"),
                ("operation_id", operation_id),
                ("request_fingerprint", fingerprint.as_str()),
            ] {
                tx.execute(
                    "INSERT INTO typed_ticket_event_attributes (
                         workspace_id, ticket_id, event_index, key, value
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![record.workspace_id, record.ticket_id, event_index, key, value],
                )?;
            }
            let updated = tx.execute(
                "UPDATE typed_tickets SET workflow_state = 'inprogress',
                         workflow_state_explicit = 1, updated_at = ?3
                  WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = 'ready'",
                params![record.workspace_id, record.ticket_id, record.assigned_at],
            )?;
            if updated != 1 {
                return Err(Error::TicketAssignmentConflict(
                    "Ticket state changed during manual Coder assignment".to_string(),
                ));
            }
            tx.commit()?;
            Ok(record.clone())
        })
    }

    fn clear_current_ticket_role_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        role: TicketAssignmentRole,
        assignment_id: &str,
        event_id: &str,
        operation_id: &str,
        actor: &str,
        occurred_at: &str,
        reason: Option<&str>,
    ) -> Result<bool> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cleared = clear_current_ticket_role_assignment_in_tx(
                &tx,
                workspace_id,
                ticket_id,
                role,
                assignment_id,
                event_id,
                operation_id,
                actor,
                occurred_at,
                reason,
                "ticket-role-assignment:clear:v1",
            )?;
            tx.commit()?;
            Ok(cleared)
        })
    }

    fn cancel_current_ticket_coder_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        assignment_id: &str,
        assignment_event_id: &str,
        state_event_id: &str,
        operation_id: &str,
        actor: &str,
        occurred_at: &str,
        reason: &str,
    ) -> Result<bool> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let state: String = tx.query_row(
                "SELECT workflow_state FROM typed_tickets
                  WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![workspace_id, ticket_id],
                |row| row.get(0),
            )?;
            if !matches!(state.as_str(), "inprogress" | "ready") {
                return Err(Error::TicketAssignmentConflict(format!(
                    "implementation cancellation requires an inprogress Ticket; current state is `{state}`"
                )));
            }
            let cleared = clear_current_ticket_role_assignment_in_tx(
                &tx,
                workspace_id,
                ticket_id,
                TicketAssignmentRole::Coder,
                assignment_id,
                assignment_event_id,
                operation_id,
                actor,
                occurred_at,
                Some(reason),
                "ticket-role-assignment:cancel-implementation:v1",
            )?;
            if !cleared {
                return Ok(false);
            }
            if state == "inprogress" {
                let event_index: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events
                      WHERE workspace_id = ?1 AND ticket_id = ?2",
                    params![workspace_id, ticket_id],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO typed_ticket_events (
                         workspace_id, ticket_id, event_index, kind, author, at,
                         from_state, to_state, reason, state_field, heading, body
                     ) VALUES (?1, ?2, ?3, 'state_changed', ?4, ?5,
                               'inprogress', 'ready', ?6, 'state',
                               'Implementation cancelled', '')",
                    params![workspace_id, ticket_id, event_index, actor, occurred_at, reason],
                )?;
                for (key, value) in [
                    ("event_id", state_event_id),
                    ("assignment_id", assignment_id),
                    ("assignment_role", "coder"),
                    ("operation_id", operation_id),
                ] {
                    tx.execute(
                        "INSERT INTO typed_ticket_event_attributes (
                             workspace_id, ticket_id, event_index, key, value
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![workspace_id, ticket_id, event_index, key, value],
                    )?;
                }
                let updated = tx.execute(
                    "UPDATE typed_tickets SET workflow_state = 'ready',
                             workflow_state_explicit = 1,
                             queued_by = NULL, queued_at = NULL, updated_at = ?3
                      WHERE workspace_id = ?1 AND ticket_id = ?2
                        AND workflow_state = 'inprogress'",
                    params![workspace_id, ticket_id, occurred_at],
                )?;
                if updated != 1 {
                    return Err(Error::TicketAssignmentConflict(
                        "Ticket state changed during implementation cancellation".to_string(),
                    ));
                }
            }
            tx.commit()?;
            Ok(true)
        })
    }

    fn get_current_ticket_coder_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<TicketCoderAssignmentRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                current_ticket_worker_assignment_select_sql().as_str(),
                params![workspace_id, ticket_id],
                read_ticket_worker_assignment_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn set_current_ticket_coder_assignment(
        &self,
        record: &TicketCoderAssignmentRecord,
        expected_assignment_id: Option<&str>,
        event_id: &str,
        operation_id: &str,
        allow_reassign: bool,
    ) -> Result<TicketWorkerAssignmentUpdate> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let removal_blocks_assignment: bool = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM worker_removal_operations
                    WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                      AND state IN ('executing', 'failed', 'succeeded')
                    UNION ALL
                    SELECT 1 FROM worker_tombstones
                    WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                )",
                params![
                    record.workspace_id,
                    record.worker.runtime_id,
                    record.worker.worker_id
                ],
                |row| row.get(0),
            )?;
            if removal_blocks_assignment {
                return Err(Error::TicketAssignmentConflict(format!(
                    "Worker {}/{} is being retained or has been removed",
                    record.worker.runtime_id, record.worker.worker_id
                )));
            }
            let mut reserved_operation = false;
            if let Some(existing) =
                read_assignment_operation(&tx, &record.workspace_id, operation_id)?
            {
                if existing.action != if allow_reassign { "reassign" } else { "assign" }
                    || existing.ticket_id != record.ticket_id
                    || existing.worker.as_ref() != Some(&record.worker)
                    || existing.expected_assignment_id.as_deref() != expected_assignment_id
                {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "assignment operation {operation_id} was already used with different input"
                    )));
                }
                if let Some(assignment_id) = existing.assignment_id {
                    let current = tx.query_row(
                        r#"SELECT workspace_id, ticket_id, assignment_id, runtime_id, worker_id,
                                  assigned_by, assigned_at
                           FROM ticket_worker_assignments
                           WHERE workspace_id = ?1 AND ticket_id = ?2 AND assignment_id = ?3"#,
                        params![record.workspace_id, record.ticket_id, assignment_id],
                        read_ticket_worker_assignment_record,
                    )?;
                    let previous = if existing.action == "reassign" {
                        existing
                            .expected_assignment_id
                            .as_deref()
                            .map(|previous_assignment_id| {
                                tx.query_row(
                                    r#"SELECT workspace_id, ticket_id, assignment_id, runtime_id, worker_id,
                                              assigned_by, assigned_at
                                       FROM ticket_worker_assignments
                                       WHERE workspace_id = ?1 AND ticket_id = ?2 AND assignment_id = ?3"#,
                                    params![
                                        record.workspace_id,
                                        record.ticket_id,
                                        previous_assignment_id,
                                    ],
                                    read_ticket_worker_assignment_record,
                                )
                            })
                            .transpose()?
                    } else {
                        None
                    };
                    tx.commit()?;
                    return Ok(TicketWorkerAssignmentUpdate { current, previous });
                }
                reserved_operation = true;
            }
            let previous = tx
                .query_row(
                    current_ticket_worker_assignment_select_sql().as_str(),
                    params![record.workspace_id, record.ticket_id],
                    read_ticket_worker_assignment_record,
                )
                .optional()?;
            if previous.is_some() && !allow_reassign {
                return Err(Error::TicketAssignmentConflict(format!(
                    "Ticket {} is already assigned; use the explicit reassign operation",
                    record.ticket_id
                )));
            }
            if allow_reassign {
                let expected_assignment_id = expected_assignment_id.ok_or_else(|| {
                    Error::TicketAssignmentConflict(
                        "reassign requires expected_assignment_id".to_string(),
                    )
                })?;
                require_expected_ticket_assignment(
                    record.ticket_id.as_str(),
                    previous.as_ref(),
                    Some(expected_assignment_id),
                )?;
            } else if expected_assignment_id.is_some() {
                return Err(Error::TicketAssignmentConflict(
                    "assign does not accept expected_assignment_id".to_string(),
                ));
            }
            tx.execute(
                r#"INSERT INTO ticket_worker_assignments (
                    workspace_id, ticket_id, assignment_id, runtime_id, worker_id,
                    assigned_by, assigned_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![
                    record.workspace_id,
                    record.ticket_id,
                    record.assignment_id,
                    record.worker.runtime_id,
                    record.worker.worker_id,
                    record.assigned_by,
                    record.assigned_at,
                ],
            )?;
            let current_write = if allow_reassign {
                tx.execute(
                    r#"UPDATE ticket_current_worker_assignments
                       SET assignment_id = ?3, runtime_id = ?4, worker_id = ?5, updated_at = ?6
                       WHERE workspace_id = ?1 AND ticket_id = ?2 AND role = 'coder'"#,
                    params![
                        record.workspace_id,
                        record.ticket_id,
                        record.assignment_id,
                        record.worker.runtime_id,
                        record.worker.worker_id,
                        record.assigned_at,
                    ],
                )
            } else {
                tx.execute(
                    r#"INSERT INTO ticket_current_worker_assignments (
                        workspace_id, ticket_id, assignment_id, runtime_id, worker_id, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                    params![
                        record.workspace_id,
                        record.ticket_id,
                        record.assignment_id,
                        record.worker.runtime_id,
                        record.worker.worker_id,
                        record.assigned_at,
                    ],
                )
            };
            if let Err(error) = current_write {
                return Err(map_assignment_constraint(
                    error,
                    &record.ticket_id,
                    &record.worker.worker_id,
                ));
            }
            tx.execute(
                r#"INSERT INTO ticket_worker_assignment_events (
                    workspace_id, ticket_id, event_id, action, assignment_id,
                    previous_assignment_id, actor, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![
                    record.workspace_id,
                    record.ticket_id,
                    event_id,
                    if previous.is_some() {
                        "reassigned"
                    } else {
                        "assigned"
                    },
                    record.assignment_id,
                    previous
                        .as_ref()
                        .map(|assignment| assignment.assignment_id.as_str()),
                    record.assigned_by,
                    record.assigned_at,
                ],
            )?;
            if reserved_operation {
                let updated = tx.execute(
                    r#"UPDATE ticket_assignment_operations
                       SET assignment_id = ?3
                       WHERE workspace_id = ?1 AND operation_id = ?2 AND assignment_id IS NULL"#,
                    params![record.workspace_id, operation_id, record.assignment_id],
                )?;
                if updated != 1 {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "assignment operation {operation_id} reservation was not current"
                    )));
                }
            } else {
                tx.execute(
                    r#"INSERT INTO ticket_assignment_operations (
                        workspace_id, operation_id, action, ticket_id, runtime_id, worker_id,
                        assignment_id, expected_assignment_id, created_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                    params![
                        record.workspace_id,
                        operation_id,
                        if allow_reassign { "reassign" } else { "assign" },
                        record.ticket_id,
                        record.worker.runtime_id,
                        record.worker.worker_id,
                        record.assignment_id,
                        expected_assignment_id,
                        record.assigned_at,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(TicketWorkerAssignmentUpdate {
                current: record.clone(),
                previous,
            })
        })
    }

    fn clear_current_ticket_worker_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        expected_assignment_id: Option<&str>,
        operation_id: &str,
        event_id: &str,
        actor: &str,
        created_at: &str,
    ) -> Result<Option<TicketCoderAssignmentRecord>> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            if let Some(existing) = read_assignment_operation(&tx, workspace_id, operation_id)? {
                if existing.action != "unassign"
                    || existing.ticket_id != ticket_id
                    || existing.expected_assignment_id.as_deref() != expected_assignment_id
                {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "assignment operation {operation_id} was already used with different input"
                    )));
                }
                let assignment = existing
                    .assignment_id
                    .map(|assignment_id| {
                        tx.query_row(
                            r#"SELECT workspace_id, ticket_id, assignment_id, runtime_id, worker_id,
                                      assigned_by, assigned_at
                               FROM ticket_worker_assignments
                               WHERE workspace_id = ?1 AND ticket_id = ?2 AND assignment_id = ?3"#,
                            params![workspace_id, ticket_id, assignment_id],
                            read_ticket_worker_assignment_record,
                        )
                    })
                    .transpose()?;
                tx.commit()?;
                return Ok(assignment);
            }
            let previous = tx
                .query_row(
                    current_ticket_worker_assignment_select_sql().as_str(),
                    params![workspace_id, ticket_id],
                    read_ticket_worker_assignment_record,
                )
                .optional()?;
            require_expected_ticket_assignment(ticket_id, previous.as_ref(), expected_assignment_id)?;
            let Some(previous) = previous else {
                tx.commit()?;
                return Ok(None);
            };
            tx.execute(
                "DELETE FROM ticket_current_worker_assignments WHERE workspace_id = ?1 AND ticket_id = ?2 AND role = 'coder'",
                params![workspace_id, ticket_id],
            )?;
            tx.execute(
                r#"INSERT INTO ticket_worker_assignment_events (
                    workspace_id, ticket_id, event_id, action, assignment_id,
                    previous_assignment_id, actor, created_at
                ) VALUES (?1, ?2, ?3, 'unassigned', NULL, ?4, ?5, ?6)"#,
                params![
                    workspace_id,
                    ticket_id,
                    event_id,
                    previous.assignment_id,
                    actor,
                    created_at,
                ],
            )?;
            tx.execute(
                r#"INSERT INTO ticket_assignment_operations (
                    workspace_id, operation_id, action, ticket_id, runtime_id, worker_id,
                    assignment_id, expected_assignment_id, created_at
                ) VALUES (?1, ?2, 'unassign', ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![
                    workspace_id,
                    operation_id,
                    ticket_id,
                    previous.worker.runtime_id,
                    previous.worker.worker_id,
                    previous.assignment_id,
                    expected_assignment_id,
                    created_at,
                ],
            )?;
            tx.commit()?;
            Ok(Some(previous))
        })
    }

    fn list_ticket_coder_assignment_events(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<TicketCoderAssignmentEventRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, ticket_id, event_id, action, assignment_id,
                          previous_assignment_id, actor, created_at
                   FROM ticket_worker_assignment_events
                   WHERE workspace_id = ?1 AND ticket_id = ?2 AND role = 'coder'
                   ORDER BY created_at DESC, event_id DESC
                   LIMIT ?3"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, ticket_id, limit as i64],
                read_ticket_worker_assignment_event_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn upsert_workdir_registry(&self, record: &WorkdirRegistryRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workdir_registry (
                    workspace_id, workdir_id, runtime_id, repository_id,
                    creation_selector, creation_ref, creation_tree,
                    current_selector, current_ref, current_tree, observed_at_epoch_seconds,
                    materialization_status, cleanliness, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(workspace_id, workdir_id) DO UPDATE SET
                    runtime_id = excluded.runtime_id,
                    repository_id = excluded.repository_id,
                    creation_selector = excluded.creation_selector,
                    creation_ref = excluded.creation_ref,
                    creation_tree = excluded.creation_tree,
                    current_selector = excluded.current_selector,
                    current_ref = excluded.current_ref,
                    current_tree = excluded.current_tree,
                    observed_at_epoch_seconds = excluded.observed_at_epoch_seconds,
                    materialization_status = excluded.materialization_status,
                    cleanliness = excluded.cleanliness,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.workdir_id,
                    record.runtime_id,
                    record.repository_id,
                    record.creation_selector,
                    record.creation_ref,
                    record.creation_tree,
                    record.current_selector,
                    record.current_ref,
                    record.current_tree,
                    record.observed_at_epoch_seconds.map(|value| value as i64),
                    record.materialization_status,
                    record.cleanliness,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn get_workdir_registry(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Option<WorkdirRegistryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                workdir_registry_select_sql("WHERE workspace_id = ?1 AND workdir_id = ?2").as_str(),
                params![workspace_id, workdir_id],
                read_workdir_registry_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_workdir_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkdirRegistryRecord>> {
        self.with_conn(|conn| {
            let sql = workdir_registry_select_sql(
                "WHERE workspace_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
            );
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_workdir_registry_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn delete_workdir_registry(&self, workspace_id: &str, workdir_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let blocked: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_links
                    WHERE workspace_id = ?1 AND workdir_id = ?2 AND unlinked_at IS NULL
                    UNION ALL
                    SELECT 1 FROM worker_workdir_attachment_reservations
                    WHERE workspace_id = ?1 AND workdir_id = ?2
                )"#,
                params![workspace_id, workdir_id],
                |row| row.get(0),
            )?;
            if blocked {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {workdir_id} has an active or pending Worker attachment"
                )));
            }
            let changed = tx.execute(
                "DELETE FROM workdir_registry WHERE workspace_id = ?1 AND workdir_id = ?2",
                params![workspace_id, workdir_id],
            )?;
            tx.commit()?;
            Ok(changed > 0)
        })
    }

    fn reserve_worker_workdir_attachment(
        &self,
        workspace_id: &str,
        workdir_id: &str,
        reservation_id: &str,
        reserved_at: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let registered: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM workdir_registry
                    WHERE workspace_id = ?1 AND workdir_id = ?2
                )"#,
                params![workspace_id, workdir_id],
                |row| row.get(0),
            )?;
            if !registered {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {workdir_id} is not registered in Workspace {workspace_id}"
                )));
            }
            let removal_pending: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM workdir_removal_operations
                    WHERE workspace_id = ?1 AND workdir_id = ?2 AND state = 'pending'
                )"#,
                params![workspace_id, workdir_id],
                |row| row.get(0),
            )?;
            if removal_pending {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {workdir_id} has a pending durable removal operation"
                )));
            }
            let occupied: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_links
                    WHERE workspace_id = ?1 AND workdir_id = ?2 AND unlinked_at IS NULL
                )"#,
                params![workspace_id, workdir_id],
                |row| row.get(0),
            )?;
            if occupied {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {workdir_id} already has an active attachment"
                )));
            }
            tx.execute(
                r#"INSERT INTO worker_workdir_attachment_reservations (
                    workspace_id, workdir_id, reservation_id, reserved_at
                ) VALUES (?1, ?2, ?3, ?4)"#,
                params![workspace_id, workdir_id, reservation_id, reserved_at],
            )
            .map_err(|error| {
                if matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY || code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
                {
                    Error::WorkdirAttachmentConflict(format!(
                        "Workdir {workdir_id} already has a pending attachment reservation"
                    ))
                } else {
                    error.into()
                }
            })?;
            tx.commit()?;
            Ok(())
        })
    }

    fn release_worker_workdir_attachment_reservation(
        &self,
        workspace_id: &str,
        workdir_id: &str,
        reservation_id: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"DELETE FROM worker_workdir_attachment_reservations
                   WHERE workspace_id = ?1 AND workdir_id = ?2 AND reservation_id = ?3"#,
                params![workspace_id, workdir_id, reservation_id],
            )?;
            Ok(())
        })
    }

    fn finalize_reserved_worker_workdir_attachment(
        &self,
        record: &WorkerWorkdirLinkRecord,
        reservation_id: &str,
    ) -> Result<WorkerWorkdirLinkRecord> {
        if record.role != "attachment" || record.unlinked_at.is_some() {
            return Err(Error::WorkdirAttachmentConflict(
                "reserved attachment finalization requires an active canonical attachment"
                    .to_string(),
            ));
        }
        self.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let owns_reservation: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_attachment_reservations
                    WHERE workspace_id = ?1 AND workdir_id = ?2 AND reservation_id = ?3
                )"#,
                params![record.workspace_id, record.workdir_id, reservation_id],
                |row| row.get(0),
            )?;
            if !owns_reservation {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {} attachment reservation is missing or owned by another spawn",
                    record.workdir_id
                )));
            }
            tx.execute(
                r#"INSERT INTO worker_workdir_links (
                    workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
                ON CONFLICT(workspace_id, worker_id, workdir_id, role) DO UPDATE SET
                    linked_at = excluded.linked_at,
                    unlinked_at = NULL"#,
                params![
                    record.workspace_id,
                    record.worker.runtime_id,
                    record.worker.worker_id,
                    record.workdir_id,
                    record.role,
                    record.linked_at,
                ],
            )
            .map_err(|error| {
                if matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
                {
                    Error::WorkdirAttachmentConflict(
                        "Worker or Workdir acquired another active attachment".to_string(),
                    )
                } else {
                    error.into()
                }
            })?;
            tx.execute(
                r#"DELETE FROM worker_workdir_attachment_reservations
                   WHERE workspace_id = ?1 AND workdir_id = ?2 AND reservation_id = ?3"#,
                params![record.workspace_id, record.workdir_id, reservation_id],
            )?;
            tx.commit()?;
            Ok(record.clone())
        })
    }

    fn attach_worker_workdir(
        &self,
        record: &WorkerWorkdirLinkRecord,
    ) -> Result<WorkerWorkdirLinkRecord> {
        if record.role != "attachment" {
            return Err(Error::WorkdirAttachmentConflict(format!(
                "unsupported Workdir attachment role `{}`",
                record.role
            )));
        }
        if record.unlinked_at.is_some() {
            return Err(Error::WorkdirAttachmentConflict(
                "a new attachment cannot already be unlinked".to_string(),
            ));
        }
        self.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let registered: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM workdir_registry
                    WHERE workspace_id = ?1 AND workdir_id = ?2
                )"#,
                params![record.workspace_id, record.workdir_id],
                |row| row.get(0),
            )?;
            if !registered {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {} is not registered in Workspace {}",
                    record.workdir_id, record.workspace_id
                )));
            }
            let reserved: bool = tx.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_attachment_reservations
                    WHERE workspace_id = ?1 AND workdir_id = ?2
                )"#,
                params![record.workspace_id, record.workdir_id],
                |row| row.get(0),
            )?;
            if reserved {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {} has a pending attachment reservation",
                    record.workdir_id
                )));
            }
            let active_for_worker = tx
                .query_row(
                    r#"SELECT workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                       FROM worker_workdir_links
                       WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND unlinked_at IS NULL"#,
                    params![
                        record.workspace_id,
                        record.worker.runtime_id,
                        record.worker.worker_id,
                    ],
                    read_worker_workdir_link_record,
                )
                .optional()?;
            if let Some(active) = active_for_worker {
                if active.workdir_id == record.workdir_id {
                    tx.commit()?;
                    return Ok(active);
                }
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Worker {}:{} is already attached to Workdir {}",
                    record.worker.runtime_id, record.worker.worker_id, active.workdir_id
                )));
            }
            let active_for_workdir = tx
                .query_row(
                    r#"SELECT workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                       FROM worker_workdir_links
                       WHERE workspace_id = ?1 AND workdir_id = ?2 AND unlinked_at IS NULL"#,
                    params![record.workspace_id, record.workdir_id],
                    read_worker_workdir_link_record,
                )
                .optional()?;
            if let Some(active) = active_for_workdir {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {} is already attached to Worker {}:{}",
                    record.workdir_id, active.worker.runtime_id, active.worker.worker_id
                )));
            }
            let write = tx.execute(
                r#"INSERT INTO worker_workdir_links (
                    workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
                ON CONFLICT(workspace_id, worker_id, workdir_id, role) DO UPDATE SET
                    linked_at = excluded.linked_at,
                    unlinked_at = NULL"#,
                params![
                    record.workspace_id,
                    record.worker.runtime_id,
                    record.worker.worker_id,
                    record.workdir_id,
                    record.role,
                    record.linked_at,
                ],
            );
            if let Err(error) = write {
                if matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
                {
                    return Err(Error::WorkdirAttachmentConflict(
                        "Worker or Workdir acquired another active attachment".to_string(),
                    ));
                }
                return Err(error.into());
            }
            tx.commit()?;
            Ok(record.clone())
        })
    }

    fn detach_worker_workdir(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
        expected_workdir_id: Option<&str>,
        unlinked_at: &str,
    ) -> Result<Option<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let active = tx
                .query_row(
                    r#"SELECT workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                       FROM worker_workdir_links
                       WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND unlinked_at IS NULL"#,
                    params![workspace_id, worker.runtime_id, worker.worker_id],
                    read_worker_workdir_link_record,
                )
                .optional()?;
            let Some(active) = active else {
                tx.commit()?;
                return Ok(None);
            };
            if let Some(expected_workdir_id) = expected_workdir_id {
                if active.workdir_id != expected_workdir_id {
                    return Err(Error::WorkdirAttachmentConflict(format!(
                        "Worker {}:{} is attached to Workdir {}, not {expected_workdir_id}",
                        worker.runtime_id, worker.worker_id, active.workdir_id
                    )));
                }
            }
            let changed = tx.execute(
                r#"UPDATE worker_workdir_links
                   SET unlinked_at = ?4
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND unlinked_at IS NULL"#,
                params![workspace_id, worker.runtime_id, worker.worker_id, unlinked_at],
            )?;
            if changed != 1 {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Worker {}:{} attachment changed during detach",
                    worker.runtime_id, worker.worker_id
                )));
            }
            tx.commit()?;
            Ok(Some(WorkerWorkdirLinkRecord {
                unlinked_at: Some(unlinked_at.to_string()),
                ..active
            }))
        })
    }

    fn worker_workdir_link_history_exists(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let exists = conn.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_links
                    WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                )"#,
                params![workspace_id, worker.runtime_id, worker.worker_id],
                |row| row.get(0),
            )?;
            Ok(exists)
        })
    }

    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                   FROM worker_workdir_links
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND unlinked_at IS NULL
                   ORDER BY linked_at DESC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, worker.runtime_id, worker.worker_id],
                read_worker_workdir_link_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn list_workdir_worker_links(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
                   FROM worker_workdir_links
                   WHERE workspace_id = ?1 AND workdir_id = ?2 AND unlinked_at IS NULL
                   ORDER BY linked_at DESC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, workdir_id],
                read_worker_workdir_link_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }
}

fn flow_source_name(path: &str) -> Result<String> {
    let file_name = path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Store("Flow source path has no file name".to_string()))?;
    let name = file_name
        .strip_suffix(".dcdl")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Store("Flow source path must end in .dcdl".to_string()))?;
    flow::FlowSelector::builtin(name)
        .map_err(|error| Error::Store(format!("invalid Flow source slug: {error}")))?;
    Ok(name.to_string())
}

fn read_flow_source_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FlowSourceRecord> {
    let source_kind = match row.get::<_, String>(2)?.as_str() {
        "builtin" => FlowSourceKind::Builtin,
        "workspace" => FlowSourceKind::Workspace,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("invalid Flow source kind {other:?}").into(),
            ));
        }
    };
    let revision = row.get::<_, i64>(7)?;
    Ok(FlowSourceRecord {
        workspace_id: row.get(0)?,
        flow_id: row.get(1)?,
        source_kind,
        name: row.get(3)?,
        path: row.get(4)?,
        content: row.get(5)?,
        content_digest: row.get(6)?,
        revision: u64::try_from(revision).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn read_flow_source_revision_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<FlowSourceRevisionRecord> {
    let revision = row.get::<_, i64>(2)?;
    let definition_json = row.get::<_, String>(5)?;
    let definition = serde_json::from_str(&definition_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(FlowSourceRevisionRecord {
        workspace_id: row.get(0)?,
        flow_id: row.get(1)?,
        revision: u64::try_from(revision).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        content: row.get(3)?,
        content_digest: row.get(4)?,
        definition,
        created_at: row.get(6)?,
    })
}

fn read_workspace_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceRecord> {
    Ok(WorkspaceRecord {
        workspace_id: row.get(0)?,
        owner_account_id: row.get(1)?,
        display_name: row.get(2)?,
        state: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn repository_registration_intent_matches(
    existing: &RepositoryRecord,
    requested: &RepositoryRecord,
) -> bool {
    existing.workspace_id == requested.workspace_id
        && existing.repository_key == requested.repository_key
        && existing.kind == requested.kind
        && existing.provider == requested.provider
        && existing.source == requested.source
        && existing.default_ref == requested.default_ref
        && existing.source_revision == requested.source_revision
        && existing.source_fingerprint == requested.source_fingerprint
}

fn validate_repository_record_identity(record: &RepositoryRecord) -> Result<()> {
    workspace_api::validate_repository_key(&record.repository_key)
        .map_err(|error| Error::InvalidInput(format!("invalid Repository key: {error}")))?;
    Ok(())
}

fn read_repository_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositoryRecord> {
    let source_kind_value = row.get::<_, String>(5)?;
    let source_kind = workspace_api::RepositorySourceKind::parse(&source_kind_value)
        .unwrap_or(workspace_api::RepositorySourceKind::Invalid);
    let source_revision = row.get::<_, u64>(8)?;
    let source_fingerprint = row.get::<_, String>(9)?;
    let mut source = RepositorySource {
        kind: source_kind,
        uri: row.get(6)?,
    };
    let observed_status_value = row.get::<_, String>(10)?;
    let mut observed_status = RepositoryObservedStatus::parse(&observed_status_value)
        .unwrap_or(RepositoryObservedStatus::Invalid);
    if source_revision == 0
        || crate::repository_source::repository_source_fingerprint(&source) != source_fingerprint
    {
        source.kind = workspace_api::RepositorySourceKind::Invalid;
        observed_status = RepositoryObservedStatus::Invalid;
    }
    Ok(RepositoryRecord {
        workspace_id: row.get(0)?,
        repository_id: row.get(1)?,
        repository_key: row.get(2)?,
        kind: row.get(3)?,
        provider: row.get(4)?,
        source,
        default_ref: row.get(7)?,
        source_revision,
        source_fingerprint,
        observed_status,
        observed_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn account_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT account_id, kind, handle, display_name, created_at, updated_at FROM accounts {where_clause}"
    )
}

fn read_trusted_runtime_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedRuntimeRecord> {
    Ok(TrustedRuntimeRecord {
        runtime_id: row.get(0)?,
        workspace_id: row.get(1)?,
        display_name: row.get(2)?,
        base_url: row.get(3)?,
        public_key: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        revoked_at: row.get(7)?,
    })
}

fn read_account_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        account_id: row.get(0)?,
        kind: row.get(1)?,
        handle: row.get(2)?,
        display_name: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn user_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT user_id, account_id, handle, display_name, created_at, updated_at FROM users {where_clause}"
    )
}

fn read_user_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        user_id: row.get(0)?,
        account_id: row.get(1)?,
        handle: row.get(2)?,
        display_name: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn passkey_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT credential_id, user_id, public_key_cose, transports_json, sign_count, created_at, last_used_at FROM passkey_credentials {where_clause}"
    )
}

fn read_passkey_credential_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PasskeyCredentialRecord> {
    Ok(PasskeyCredentialRecord {
        credential_id: row.get(0)?,
        user_id: row.get(1)?,
        public_key_cose: row.get(2)?,
        transports_json: row.get(3)?,
        sign_count: row.get(4)?,
        created_at: row.get(5)?,
        last_used_at: row.get(6)?,
    })
}

fn auth_challenge_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT challenge_id, ceremony, challenge, user_id, rp_id, origin, state_json, expires_at, created_at, consumed_at FROM auth_challenges {where_clause}"
    )
}

fn read_auth_challenge_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthChallengeRecord> {
    Ok(AuthChallengeRecord {
        challenge_id: row.get(0)?,
        ceremony: row.get(1)?,
        challenge: row.get(2)?,
        user_id: row.get(3)?,
        rp_id: row.get(4)?,
        origin: row.get(5)?,
        state_json: row.get(6)?,
        expires_at: row.get(7)?,
        created_at: row.get(8)?,
        consumed_at: row.get(9)?,
    })
}

fn browser_session_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT session_id, token_hash, user_id, created_at, expires_at, revoked_at FROM browser_sessions {where_clause}"
    )
}

fn read_browser_session_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrowserSessionRecord> {
    Ok(BrowserSessionRecord {
        session_id: row.get(0)?,
        token_hash: row.get(1)?,
        user_id: row.get(2)?,
        created_at: row.get(3)?,
        expires_at: row.get(4)?,
        revoked_at: row.get(5)?,
    })
}

fn api_token_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT token_id, token_hash, user_id, label, created_at, expires_at, revoked_at, last_used_at FROM api_tokens {where_clause}"
    )
}

fn read_api_token_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiTokenRecord> {
    Ok(ApiTokenRecord {
        token_id: row.get(0)?,
        token_hash: row.get(1)?,
        user_id: row.get(2)?,
        label: row.get(3)?,
        created_at: row.get(4)?,
        expires_at: row.get(5)?,
        revoked_at: row.get(6)?,
        last_used_at: row.get(7)?,
    })
}

fn device_login_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT device_code, user_code, verification_uri, client_name, user_id, api_token_id, issued_access_token, created_at, expires_at, approved_at, consumed_at FROM device_login_flows {where_clause}"
    )
}

fn read_device_login_flow_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DeviceLoginFlowRecord> {
    Ok(DeviceLoginFlowRecord {
        device_code: row.get(0)?,
        user_code: row.get(1)?,
        verification_uri: row.get(2)?,
        client_name: row.get(3)?,
        user_id: row.get(4)?,
        api_token_id: row.get(5)?,
        issued_access_token: row.get(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
        approved_at: row.get(9)?,
        consumed_at: row.get(10)?,
    })
}

fn read_worker_workdir_link_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkerWorkdirLinkRecord> {
    Ok(WorkerWorkdirLinkRecord {
        workspace_id: row.get(0)?,
        worker: RuntimeWorkerRef::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?),
        workdir_id: row.get(3)?,
        role: row.get(4)?,
        linked_at: row.get(5)?,
        unlinked_at: row.get(6)?,
    })
}

fn read_objective_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveRecord> {
    Ok(ObjectiveRecord {
        workspace_id: row.get(0)?,
        objective_id: row.get(1)?,
        title: row.get(2)?,
        state: row.get(3)?,
        body_md: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn read_objective_ticket_link_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ObjectiveTicketLinkRecord> {
    Ok(ObjectiveTicketLinkRecord {
        workspace_id: row.get(0)?,
        objective_id: row.get(1)?,
        ticket_id: row.get(2)?,
        kind: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn read_objective_resource_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ObjectiveResourceRecord> {
    Ok(ObjectiveResourceRecord {
        workspace_id: row.get(0)?,
        objective_id: row.get(1)?,
        resource_path: row.get(2)?,
        body: row.get(3)?,
        media_type: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn read_memory_document_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryDocumentRecord> {
    Ok(MemoryDocumentRecord {
        workspace_id: row.get(0)?,
        body_md: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn read_memory_staging_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryStagingRecord> {
    Ok(MemoryStagingRecord {
        workspace_id: row.get(0)?,
        candidate_id: row.get(1)?,
        raw_json: row.get(2)?,
        source_path: row.get(3)?,
        imported_at: row.get(4)?,
    })
}

fn read_memory_staging_resolution_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MemoryStagingResolutionRecord> {
    Ok(MemoryStagingResolutionRecord {
        workspace_id: row.get(0)?,
        candidate_id: row.get(1)?,
        action: row.get(2)?,
        reason: row.get(3)?,
        affected_refs_json: row.get(4)?,
        staging_raw_json: row.get(5)?,
        source_path: row.get(6)?,
        imported_at: row.get(7)?,
        resolved_at: row.get(8)?,
    })
}

fn worker_registry_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT workspace_id, runtime_id, worker_id, display_name, profile, \
         retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref, \
         created_at, updated_at FROM worker_registry {where_clause}"
    )
}

fn read_worker_registry_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRegistryRecord> {
    Ok(WorkerRegistryRecord {
        workspace_id: row.get(0)?,
        worker: RuntimeWorkerRef::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?),
        display_name: row.get(3)?,
        profile: row.get(4)?,
        retention_state: row.get(5)?,
        transcript_ref: row.get(6)?,
        session_ref: row.get(7)?,
        summary_ref: row.get(8)?,
        diagnostics_ref: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn read_worker_control_grant_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkerControlGrantRecord> {
    let permissions_json: String = row.get(8)?;
    let permissions = serde_json::from_str(&permissions_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(WorkerControlGrantRecord {
        workspace_id: row.get(0)?,
        grant_id: row.get(1)?,
        controller: RuntimeWorkerRef::new(row.get::<_, String>(2)?, row.get::<_, String>(3)?),
        subject: RuntimeWorkerRef::new(row.get::<_, String>(4)?, row.get::<_, String>(5)?),
        relation: row.get(6)?,
        origin: row.get(7)?,
        permissions,
        operation_id: row.get(9)?,
        created_at: row.get(10)?,
        revoked_at: row.get(11)?,
    })
}

fn read_worker_control_grant_by_operation(
    conn: &Connection,
    workspace_id: &str,
    controller: &RuntimeWorkerRef,
    operation_id: &str,
) -> Result<Option<WorkerControlGrantRecord>> {
    conn.query_row(
        r#"SELECT workspace_id, grant_id,
                  controller_runtime_id, controller_worker_id,
                  subject_runtime_id, subject_worker_id,
                  relation, origin, permissions_json, operation_id, created_at, revoked_at
           FROM worker_control_grants
           WHERE workspace_id = ?1
             AND controller_runtime_id = ?2 AND controller_worker_id = ?3
             AND operation_id = ?4"#,
        params![
            workspace_id,
            controller.runtime_id,
            controller.worker_id,
            operation_id,
        ],
        read_worker_control_grant_record,
    )
    .optional()
    .map_err(Error::from)
}

fn ensure_worker_assignment_available(
    conn: &Connection,
    workspace_id: &str,
    worker: &RuntimeWorkerRef,
) -> Result<()> {
    let removal_blocks_assignment: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM worker_removal_operations
            WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
              AND state IN ('executing', 'failed', 'succeeded')
            UNION ALL
            SELECT 1 FROM worker_tombstones
            WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
        )",
        params![workspace_id, worker.runtime_id, worker.worker_id],
        |row| row.get(0),
    )?;
    if removal_blocks_assignment {
        return Err(Error::TicketAssignmentConflict(format!(
            "Worker {}/{} is being retained or has been removed",
            worker.runtime_id, worker.worker_id
        )));
    }
    Ok(())
}

fn read_ticket_role_assignment_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TicketRoleAssignmentRecord> {
    let role = TicketAssignmentRole::from_db(row.get::<_, String>(3)?.as_str())?;
    let principal_kind: String = row.get(4)?;
    let principal_id: Option<String> = row.get(5)?;
    let runtime_id: Option<String> = row.get(6)?;
    let worker_id: Option<String> = row.get(7)?;
    let principal = match principal_kind.as_str() {
        "user" => TicketAssignmentPrincipal::User {
            account_id: principal_id.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
        },
        "worker" => TicketAssignmentPrincipal::Worker {
            runtime_id: runtime_id.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
            worker_id: worker_id.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
        },
        "workspace_agent" => TicketAssignmentPrincipal::WorkspaceAgent {
            agent_key: principal_id.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
        },
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                format!("unknown Ticket assignment principal kind `{principal_kind}`").into(),
            ));
        }
    };
    Ok(TicketRoleAssignmentRecord {
        workspace_id: row.get(0)?,
        ticket_id: row.get(1)?,
        assignment_id: row.get(2)?,
        role,
        principal,
        assigned_by: row.get(8)?,
        assigned_at: row.get(9)?,
    })
}

fn read_ticket_role_assignment_by_id(
    conn: &Connection,
    workspace_id: &str,
    assignment_id: &str,
) -> Result<Option<TicketRoleAssignmentRecord>> {
    Ok(conn
        .query_row(
            "SELECT workspace_id, ticket_id, assignment_id, role, principal_kind,
                    principal_id, runtime_id, worker_id, assigned_by, assigned_at
               FROM ticket_worker_assignments
              WHERE workspace_id = ?1 AND assignment_id = ?2",
            params![workspace_id, assignment_id],
            read_ticket_role_assignment_record,
        )
        .optional()?)
}

fn ticket_role_assignment_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT a.workspace_id, a.ticket_id, a.assignment_id, a.role, \
                a.principal_kind, a.principal_id, a.runtime_id, a.worker_id, \
                a.assigned_by, a.assigned_at \
         FROM ticket_current_worker_assignments AS current \
         JOIN ticket_worker_assignments AS a \
           ON a.workspace_id = current.workspace_id \
          AND a.ticket_id = current.ticket_id \
          AND a.role = current.role \
          AND a.assignment_id = current.assignment_id \
         {where_clause}"
    )
}

fn validate_ticket_assignment_role_principal(
    role: TicketAssignmentRole,
    principal: &TicketAssignmentPrincipal,
) -> Result<()> {
    let valid = match role {
        TicketAssignmentRole::Orchestrator => matches!(
            principal,
            TicketAssignmentPrincipal::WorkspaceAgent { agent_key }
                if agent_key == "workspace-orchestrator"
        ),
        TicketAssignmentRole::Coder => {
            matches!(principal, TicketAssignmentPrincipal::Worker { .. })
        }
        TicketAssignmentRole::Owner => matches!(principal, TicketAssignmentPrincipal::User { .. }),
        TicketAssignmentRole::Contributor => true,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::TicketAssignmentConflict(format!(
            "principal kind `{}` is not valid for Ticket role `{}`",
            principal.kind(),
            role.as_str()
        )))
    }
}

#[allow(clippy::too_many_arguments)]
fn clear_current_ticket_role_assignment_in_tx(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    ticket_id: &str,
    role: TicketAssignmentRole,
    assignment_id: &str,
    event_id: &str,
    operation_id: &str,
    actor: &str,
    occurred_at: &str,
    reason: Option<&str>,
    fingerprint_domain: &str,
) -> Result<bool> {
    let current = read_ticket_role_assignment_by_id(tx, workspace_id, assignment_id)?;
    let Some(current) = current.filter(|value| value.ticket_id == ticket_id && value.role == role)
    else {
        return Ok(false);
    };
    let principal_json = serde_json::to_string(&current.principal)
        .map_err(|error| Error::Store(format!("serialize Ticket assignment principal: {error}")))?;
    let mut hasher = Sha256::new();
    for value in [
        fingerprint_domain,
        workspace_id,
        ticket_id,
        role.as_str(),
        assignment_id,
        principal_json.as_str(),
        actor,
        reason.unwrap_or(""),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    let fingerprint = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (principal_id, runtime_id, worker_id) = match &current.principal {
        TicketAssignmentPrincipal::User { account_id } => (Some(account_id.as_str()), None, None),
        TicketAssignmentPrincipal::Worker {
            runtime_id,
            worker_id,
        } => (None, Some(runtime_id.as_str()), Some(worker_id.as_str())),
        TicketAssignmentPrincipal::WorkspaceAgent { agent_key } => {
            (Some(agent_key.as_str()), None, None)
        }
    };
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO ticket_assignment_operations (
             workspace_id, operation_id, action, ticket_id, role, principal_kind,
             principal_id, runtime_id, worker_id, assignment_id,
             expected_assignment_id, created_at, request_fingerprint
         ) VALUES (?1, ?2, 'unassign', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11)",
        params![
            workspace_id,
            operation_id,
            ticket_id,
            role.as_str(),
            current.principal.kind(),
            principal_id,
            runtime_id,
            worker_id,
            assignment_id,
            occurred_at,
            fingerprint,
        ],
    )?;
    if inserted == 0 {
        let persisted: String = tx.query_row(
            "SELECT request_fingerprint FROM ticket_assignment_operations
              WHERE workspace_id = ?1 AND operation_id = ?2",
            params![workspace_id, operation_id],
            |row| row.get(0),
        )?;
        if persisted != fingerprint {
            return Err(Error::TicketAssignmentConflict(format!(
                "operation `{operation_id}` was already used for different Ticket assignment input"
            )));
        }
        return Ok(true);
    }
    let deleted = tx.execute(
        "DELETE FROM ticket_current_worker_assignments
          WHERE workspace_id = ?1 AND ticket_id = ?2 AND role = ?3 AND assignment_id = ?4",
        params![workspace_id, ticket_id, role.as_str(), assignment_id],
    )?;
    if deleted != 0 {
        tx.execute(
            "INSERT INTO ticket_worker_assignment_events (
                 workspace_id, ticket_id, role, event_id, action, assignment_id,
                 previous_assignment_id, actor, created_at, operation_id, reason
             ) VALUES (?1, ?2, ?3, ?4, 'unassigned', NULL, ?5, ?6, ?7, ?8, ?9)",
            params![
                workspace_id,
                ticket_id,
                role.as_str(),
                event_id,
                assignment_id,
                actor,
                occurred_at,
                operation_id,
                reason,
            ],
        )?;
    }
    Ok(deleted != 0)
}

fn current_ticket_worker_assignment_select_sql() -> String {
    "SELECT a.workspace_id, a.ticket_id, a.assignment_id, a.runtime_id, a.worker_id, \
            a.assigned_by, a.assigned_at \
     FROM ticket_current_worker_assignments AS current \
     JOIN ticket_worker_assignments AS a \
       ON a.workspace_id = current.workspace_id \
      AND a.ticket_id = current.ticket_id \
      AND a.role = current.role \
      AND a.assignment_id = current.assignment_id \
     WHERE current.workspace_id = ?1 AND current.ticket_id = ?2 AND current.role = 'coder'"
        .to_owned()
}

fn read_ticket_worker_assignment_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TicketCoderAssignmentRecord> {
    Ok(TicketCoderAssignmentRecord {
        workspace_id: row.get(0)?,
        ticket_id: row.get(1)?,
        assignment_id: row.get(2)?,
        worker: RuntimeWorkerRef::new(row.get::<_, String>(3)?, row.get::<_, String>(4)?),
        assigned_by: row.get(5)?,
        assigned_at: row.get(6)?,
    })
}

fn read_ticket_worker_assignment_event_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TicketCoderAssignmentEventRecord> {
    Ok(TicketCoderAssignmentEventRecord {
        workspace_id: row.get(0)?,
        ticket_id: row.get(1)?,
        event_id: row.get(2)?,
        action: row.get(3)?,
        assignment_id: row.get(4)?,
        previous_assignment_id: row.get(5)?,
        actor: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketAssignmentOperationRecord {
    pub action: String,
    pub ticket_id: String,
    pub runtime_id: Option<String>,
    pub worker: Option<RuntimeWorkerRef>,
    pub assignment_id: Option<String>,
    pub expected_assignment_id: Option<String>,
    pub request_fingerprint: Option<String>,
}

fn read_assignment_operation(
    conn: &Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<TicketAssignmentOperationRecord>> {
    conn.query_row(
        r#"SELECT action, ticket_id, runtime_id, worker_id, assignment_id, expected_assignment_id,
                  request_fingerprint
           FROM ticket_assignment_operations
           WHERE workspace_id = ?1 AND operation_id = ?2"#,
        params![workspace_id, operation_id],
        |row| {
            let runtime_id: Option<String> = row.get(2)?;
            let worker_id: Option<String> = row.get(3)?;
            Ok(TicketAssignmentOperationRecord {
                action: row.get(0)?,
                ticket_id: row.get(1)?,
                runtime_id: runtime_id.clone(),
                worker: runtime_id
                    .zip(worker_id)
                    .map(|(runtime_id, worker_id)| RuntimeWorkerRef::new(runtime_id, worker_id)),
                assignment_id: row.get(4)?,
                expected_assignment_id: row.get(5)?,
                request_fingerprint: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}

fn map_assignment_constraint(error: rusqlite::Error, ticket_id: &str, worker_id: &str) -> Error {
    if matches!(error, rusqlite::Error::SqliteFailure(_, _)) {
        Error::TicketAssignmentConflict(format!(
            "Ticket {ticket_id} or Worker {worker_id} already has a current assignment"
        ))
    } else {
        Error::Sqlite(error)
    }
}

fn require_expected_ticket_assignment(
    ticket_id: &str,
    current: Option<&TicketCoderAssignmentRecord>,
    expected_assignment_id: Option<&str>,
) -> Result<()> {
    let Some(expected_assignment_id) = expected_assignment_id else {
        return Ok(());
    };
    if current.map(|assignment| assignment.assignment_id.as_str()) == Some(expected_assignment_id) {
        return Ok(());
    }
    Err(Error::TicketAssignmentConflict(format!(
        "Ticket {ticket_id} is no longer assigned to {expected_assignment_id}"
    )))
}

fn workdir_registry_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT workspace_id, workdir_id, runtime_id, repository_id, \
         creation_selector, creation_ref, creation_tree, \
         current_selector, current_ref, current_tree, observed_at_epoch_seconds, \
         materialization_status, cleanliness, created_at, updated_at \
         FROM workdir_registry {where_clause}"
    )
}

fn read_workdir_registry_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkdirRegistryRecord> {
    Ok(WorkdirRegistryRecord {
        workspace_id: row.get(0)?,
        workdir_id: row.get(1)?,
        runtime_id: row.get(2)?,
        repository_id: row.get(3)?,
        creation_selector: row.get(4)?,
        creation_ref: row.get(5)?,
        creation_tree: row.get(6)?,
        current_selector: row.get(7)?,
        current_ref: row.get(8)?,
        current_tree: row.get(9)?,
        observed_at_epoch_seconds: row.get::<_, Option<i64>>(10)?.map(|value| value as u64),
        materialization_status: row.get(11)?,
        cleanliness: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

pub(crate) fn configure_sqlite(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.execute_batch(
        r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS __yoi_schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
"#,
    )?;
    Ok(())
}

fn create_latest_workspace_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("latest_schema.sql"))?;
    Ok(())
}

fn verify_workspace_resource_constraints(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? < 39 {
        return Ok(());
    }
    for trigger in [
        "ticket_assignment_ticket_parent_tombstone",
        "ticket_assignment_worker_parent_tombstone_delete",
        "ticket_assignment_worker_parent_tombstone_move",
        "ticket_worker_assignments_validate_insert",
        "ticket_worker_assignments_validate_update",
        "ticket_worker_assignment_events_validate_insert",
        "ticket_assignment_operations_validate_insert",
    ] {
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1)",
            [trigger],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(Error::Store(format!(
                "Workspace resource constraint trigger `{trigger}` is missing"
            )));
        }
    }
    Ok(())
}

fn validate_workspace_resource_references(conn: &Connection) -> Result<()> {
    let diagnostics = workspace_resource_reference_diagnostics(conn)?;
    if diagnostics.is_empty() {
        return Ok(());
    }
    Err(Error::Store(format!(
        "Workspace resource foreign-key preflight failed:\n- {}",
        diagnostics.join("\n- ")
    )))
}

fn workspace_resource_reference_diagnostics(conn: &Connection) -> Result<Vec<String>> {
    let mut diagnostics = Vec::new();
    let current_assignment_reference_sql =
        if column_exists(conn, "ticket_current_worker_assignments", "role")?
            && column_exists(conn, "ticket_worker_assignments", "role")?
        {
            "SELECT current.workspace_id || '/' || current.assignment_id \
             FROM ticket_current_worker_assignments AS current \
             WHERE NOT EXISTS (SELECT 1 FROM ticket_worker_assignments AS assignment \
                 WHERE assignment.workspace_id = current.workspace_id \
                   AND assignment.ticket_id = current.ticket_id \
                   AND assignment.role = current.role \
                   AND assignment.assignment_id = current.assignment_id) LIMIT 100"
        } else {
            "SELECT current.workspace_id || '/' || current.assignment_id \
             FROM ticket_current_worker_assignments AS current \
             WHERE NOT EXISTS (SELECT 1 FROM ticket_worker_assignments AS assignment \
                 WHERE assignment.workspace_id = current.workspace_id \
                   AND assignment.ticket_id = current.ticket_id \
                   AND assignment.assignment_id = current.assignment_id) LIMIT 100"
        };
    for (table, repository_nullable) in [
        ("workdir_registry", false),
        ("artifacts", true),
        ("typed_tickets", true),
        ("merge_requests", false),
    ] {
        if !table_exists(conn, table)? || !column_exists(conn, table, "repository_id")? {
            continue;
        }
        let null_filter = if repository_nullable {
            "child.repository_id IS NOT NULL AND"
        } else {
            ""
        };
        collect_reference_diagnostics(
            conn,
            &format!(
                "SELECT child.workspace_id || '/' || child.repository_id FROM {table} AS child \
                 WHERE {null_filter} NOT EXISTS (\
                     SELECT 1 FROM repositories AS parent \
                     WHERE parent.workspace_id = child.workspace_id \
                       AND parent.repository_id = child.repository_id\
                 ) LIMIT 100"
            ),
            &format!("{table}.repository_id"),
            &mut diagnostics,
        )?;
    }

    for (label, sql) in [
        (
            "typed_ticket_relations.target",
            "SELECT relation.workspace_id || '/' || relation.ticket_id || ' -> ' || relation.target \
             FROM typed_ticket_relations AS relation \
             WHERE NOT EXISTS (SELECT 1 FROM typed_tickets AS target \
                 WHERE target.workspace_id = relation.workspace_id \
                   AND target.ticket_id = relation.target) LIMIT 100",
        ),
        (
            "objective_events.objective_id",
            "SELECT child.workspace_id || '/' || child.event_id || ' -> ' || child.objective_id \
             FROM objective_events AS child \
             WHERE NOT EXISTS (SELECT 1 FROM objectives AS parent \
                 WHERE parent.workspace_id = child.workspace_id \
                   AND parent.objective_id = child.objective_id) LIMIT 100",
        ),
        (
            "objective_resources.objective_id",
            "SELECT child.workspace_id || '/' || child.resource_path || ' -> ' || child.objective_id \
             FROM objective_resources AS child \
             WHERE NOT EXISTS (SELECT 1 FROM objectives AS parent \
                 WHERE parent.workspace_id = child.workspace_id \
                   AND parent.objective_id = child.objective_id) LIMIT 100",
        ),
        (
            "objective_ticket_links.objective_id",
            "SELECT link.workspace_id || '/' || link.objective_id || ' -> ' || link.ticket_id \
             FROM objective_ticket_links AS link \
             WHERE NOT EXISTS (SELECT 1 FROM objectives AS objective \
                 WHERE objective.workspace_id = link.workspace_id \
                   AND objective.objective_id = link.objective_id) LIMIT 100",
        ),
        (
            "objective_ticket_links.ticket_id",
            "SELECT link.workspace_id || '/' || link.objective_id || ' -> ' || link.ticket_id \
             FROM objective_ticket_links AS link \
             WHERE NOT EXISTS (SELECT 1 FROM typed_tickets AS ticket \
                 WHERE ticket.workspace_id = link.workspace_id \
                   AND ticket.ticket_id = link.ticket_id) LIMIT 100",
        ),
        (
            "ticket_current_worker_assignments.assignment_id",
            current_assignment_reference_sql,
        ),
        (
            "ticket_worker_assignment_events.assignment_id",
            "SELECT event.workspace_id || '/' || event.event_id \
             FROM ticket_worker_assignment_events AS event \
             WHERE (event.assignment_id IS NOT NULL AND NOT EXISTS (\
                       SELECT 1 FROM ticket_worker_assignments AS assignment \
                       WHERE assignment.workspace_id = event.workspace_id \
                         AND assignment.ticket_id = event.ticket_id \
                         AND assignment.assignment_id = event.assignment_id)) \
                OR (event.previous_assignment_id IS NOT NULL AND NOT EXISTS (\
                       SELECT 1 FROM ticket_worker_assignments AS assignment \
                       WHERE assignment.workspace_id = event.workspace_id \
                         AND assignment.ticket_id = event.ticket_id \
                         AND assignment.assignment_id = event.previous_assignment_id)) LIMIT 100",
        ),
        (
            "artifacts.ticket_id",
            "SELECT artifact.workspace_id || '/' || artifact.artifact_id || ' -> ' || artifact.ticket_id \
             FROM artifacts AS artifact WHERE artifact.ticket_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM typed_tickets AS ticket \
                 WHERE ticket.workspace_id = artifact.workspace_id \
                   AND ticket.ticket_id = artifact.ticket_id) LIMIT 100",
        ),
        (
            "artifacts.worker_ref",
            "SELECT artifact.workspace_id || '/' || artifact.artifact_id \
             FROM artifacts AS artifact \
             WHERE (artifact.worker_ref_kind IS NULL) != (artifact.worker_ref_key IS NULL) LIMIT 100",
        ),
        (
            "artifacts.objective_id",
            "SELECT artifact.workspace_id || '/' || artifact.artifact_id || ' -> ' || artifact.objective_id \
             FROM artifacts AS artifact WHERE artifact.objective_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM objectives AS objective \
                 WHERE objective.workspace_id = artifact.workspace_id \
                   AND objective.objective_id = artifact.objective_id) LIMIT 100",
        ),
    ] {
        let Some(table) = label.split('.').next() else {
            continue;
        };
        if !table_exists(conn, table)? {
            continue;
        }
        if sql.contains("typed_tickets") && !table_exists(conn, "typed_tickets")? {
            continue;
        }
        if sql.contains("worker.worker_id") && !column_exists(conn, "worker_registry", "worker_id")?
        {
            continue;
        }
        if sql.contains("assignment.worker_id")
            && !column_exists(conn, "ticket_worker_assignments", "worker_id")?
        {
            continue;
        }
        if sql.contains("current.worker_id")
            && !column_exists(conn, "ticket_current_worker_assignments", "worker_id")?
        {
            continue;
        }
        collect_reference_diagnostics(conn, sql, label, &mut diagnostics)?;
    }

    // Assignment and operation rows are historical soft references. The current schema records
    // an explicit tombstone before a live Ticket or Worker parent is deleted or moved. Missing
    // parents without that authority are corruption and remain startup-blocking.
    if table_exists(conn, "ticket_worker_assignments")? && table_exists(conn, "typed_tickets")? {
        let tombstone_filter = if table_exists(conn, "ticket_assignment_ticket_tombstones")? {
            "AND NOT EXISTS (SELECT 1 FROM ticket_assignment_ticket_tombstones AS tombstone \
             WHERE tombstone.workspace_id = assignment.workspace_id \
               AND tombstone.ticket_id = assignment.ticket_id)"
        } else {
            ""
        };
        collect_reference_diagnostics(
            conn,
            &format!(
                "SELECT assignment.workspace_id || '/' || assignment.assignment_id || ' -> ' || assignment.ticket_id \
                 FROM ticket_worker_assignments AS assignment \
                 WHERE NOT EXISTS (SELECT 1 FROM typed_tickets AS ticket \
                     WHERE ticket.workspace_id = assignment.workspace_id \
                       AND ticket.ticket_id = assignment.ticket_id) \
                 {tombstone_filter} LIMIT 100"
            ),
            "ticket_worker_assignments.ticket_id",
            &mut diagnostics,
        )?;
    }
    if table_exists(conn, "ticket_worker_assignments")?
        && table_exists(conn, "worker_registry")?
        && column_exists(conn, "ticket_worker_assignments", "worker_id")?
        && column_exists(conn, "worker_registry", "worker_id")?
    {
        collect_assignment_worker_reference_diagnostics(conn, &mut diagnostics)?;
    }
    if table_exists(conn, "ticket_assignment_operations")? && table_exists(conn, "typed_tickets")? {
        let tombstone_filter = if table_exists(conn, "ticket_assignment_ticket_tombstones")? {
            "AND NOT EXISTS (SELECT 1 FROM ticket_assignment_ticket_tombstones AS tombstone \
             WHERE tombstone.workspace_id = operation.workspace_id \
               AND tombstone.ticket_id = operation.ticket_id)"
        } else {
            ""
        };
        collect_reference_diagnostics(
            conn,
            &format!(
                "SELECT operation.workspace_id || '/' || operation.operation_id || ' -> ' || operation.ticket_id \
                 FROM ticket_assignment_operations AS operation \
                 WHERE NOT EXISTS (SELECT 1 FROM typed_tickets AS ticket \
                     WHERE ticket.workspace_id = operation.workspace_id \
                       AND ticket.ticket_id = operation.ticket_id) \
                 {tombstone_filter} LIMIT 100"
            ),
            "ticket_assignment_operations.ticket_id",
            &mut diagnostics,
        )?;
    }
    Ok(diagnostics)
}

fn collect_assignment_worker_reference_diagnostics(
    conn: &Connection,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    let has_assignment_tombstones = table_exists(conn, "ticket_assignment_worker_tombstones")?;
    let tombstone_filter = if has_assignment_tombstones {
        "AND NOT EXISTS (SELECT 1 FROM ticket_assignment_worker_tombstones AS tombstone \
         WHERE tombstone.workspace_id = assignment.workspace_id \
           AND tombstone.runtime_id = assignment.runtime_id \
           AND tombstone.worker_id = assignment.worker_id)"
    } else {
        ""
    };
    let worker_principal_filter =
        if column_exists(conn, "ticket_worker_assignments", "principal_kind")? {
            "assignment.principal_kind = 'worker' AND "
        } else {
            ""
        };
    let sql = format!(
        "SELECT assignment.workspace_id, assignment.assignment_id, \
                assignment.runtime_id, assignment.worker_id \
         FROM ticket_worker_assignments AS assignment \
         WHERE {worker_principal_filter}NOT EXISTS (SELECT 1 FROM worker_registry AS worker \
             WHERE worker.workspace_id = assignment.workspace_id \
               AND worker.runtime_id = assignment.runtime_id \
               AND worker.worker_id = assignment.worker_id) \
         {tombstone_filter}"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut worker_diagnostic_count = 0;
    for row in rows {
        let (workspace_id, assignment_id, runtime_id, worker_id) = row?;
        diagnostics.push(format!(
            "ticket_worker_assignments.worker_id: \
             {workspace_id}/{assignment_id} -> {runtime_id}/{worker_id}"
        ));
        worker_diagnostic_count += 1;
        if worker_diagnostic_count == 100 {
            break;
        }
    }
    Ok(())
}

fn collect_reference_diagnostics(
    conn: &Connection,
    sql: &str,
    label: &str,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        diagnostics.push(format!("{label}: {}", row?));
    }
    Ok(())
}

fn validate_workspace_memory_settings_snapshot(
    snapshot: &manifest::WorkspaceMemorySettingsSnapshot,
    expected_workspace_id: &str,
) -> Result<()> {
    if snapshot.workspace_id != expected_workspace_id
        || snapshot.settings_revision == 0
        || !manifest::is_normalized_workspace_memory_language(&snapshot.language)
    {
        return Err(Error::Store(
            "Workspace Memory settings are corrupt or belong to another Workspace".to_string(),
        ));
    }
    Ok(())
}

fn validate_workspace_memory_settings_record(
    record: &WorkspaceMemorySettingsRecord,
    expected_workspace_id: &str,
) -> Result<()> {
    validate_workspace_memory_settings_snapshot(
        &manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: record.workspace_id.clone(),
            settings_revision: record.settings_revision,
            language: record.language.clone(),
        },
        expected_workspace_id,
    )
}

fn normalize_workspace_memory_language(language: &str) -> Result<String> {
    let language = language.trim();
    if !manifest::is_normalized_workspace_memory_language(language) {
        return Err(Error::InvalidInput(format!(
            "Workspace Memory language must be a non-empty UTF-8 string of at most {} characters without control characters",
            manifest::MAX_WORKSPACE_MEMORY_LANGUAGE_CHARS
        )));
    }
    Ok(language.to_string())
}

fn bound_worker_create_fingerprint(
    request_fingerprint: &str,
    snapshot: &manifest::WorkspaceMemorySettingsSnapshot,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"workspace-worker-create-v2\0");
    digest.update(request_fingerprint.as_bytes());
    digest.update(b"\0");
    digest.update(snapshot.workspace_id.as_bytes());
    digest.update(b"\0");
    digest.update(snapshot.settings_revision.to_be_bytes());
    digest.update(b"\0");
    digest.update(snapshot.language.as_bytes());
    let encoded = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{encoded}")
}

fn current_schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM __yoi_schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

fn allocate_resource_key(
    conn: &Connection,
    workspace_id: &str,
    kind: WorkspaceResourceKind,
    resource_id: &str,
    allocated_at: &str,
) -> Result<String> {
    if let Some(existing) = conn
        .query_row(
            "SELECT resource_key FROM workspace_resource_keys
             WHERE workspace_id = ?1 AND resource_kind = ?2 AND resource_id = ?3",
            params![workspace_id, kind.as_str(), resource_id],
            |row| row.get(0),
        )
        .optional()?
    {
        return Ok(existing);
    }
    conn.execute(
        "INSERT OR IGNORE INTO workspace_resource_key_counters
         (workspace_id, resource_kind, next_sequence) VALUES (?1, ?2, 1)",
        params![workspace_id, kind.as_str()],
    )?;
    let sequence: i64 = conn.query_row(
        "SELECT next_sequence FROM workspace_resource_key_counters
         WHERE workspace_id = ?1 AND resource_kind = ?2",
        params![workspace_id, kind.as_str()],
        |row| row.get(0),
    )?;
    conn.execute(
        "UPDATE workspace_resource_key_counters SET next_sequence = ?3
         WHERE workspace_id = ?1 AND resource_kind = ?2",
        params![workspace_id, kind.as_str(), sequence + 1],
    )?;
    let resource_key = format!("{}-{sequence}", kind.prefix());
    conn.execute(
        "INSERT INTO workspace_resource_keys
         (workspace_id, resource_kind, resource_id, sequence, resource_key, allocated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            workspace_id,
            kind.as_str(),
            resource_id,
            sequence,
            resource_key,
            allocated_at
        ],
    )?;
    Ok(resource_key)
}

fn verify_baseline_history(conn: &Connection) -> Result<()> {
    let rows = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(version), 0), COALESCE(MAX(name), '') \
         FROM __yoi_schema_migrations",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let baseline = &MIGRATIONS[0];
    if rows != (1, baseline.version, baseline.name.to_string()) {
        return Err(Error::Store(format!(
            "database migration history is not the canonical schema baseline: expected only version {} ({:?}), found {} row(s) ending at version {} ({:?})",
            baseline.version, baseline.name, rows.0, rows.1, rows.2
        )));
    }
    Ok(())
}

fn apply_migrations(conn: &Connection) -> Result<()> {
    let baseline = &MIGRATIONS[0];
    let current = current_schema_version(conn)?;
    match current {
        0 => {
            let tx = conn.unchecked_transaction()?;
            (baseline.apply)(&tx)?;
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![baseline.version, baseline.name],
            )?;
            tx.commit()?;
            Ok(())
        }
        version if version == baseline.version => verify_baseline_history(conn),
        version if version > baseline.version => Err(Error::Store(format!(
            "database schema version {version} is newer than this server supports ({}); refusing to serve with an older binary",
            baseline.version
        ))),
        version => Err(Error::Store(format!(
            "database schema version {version} predates the canonical baseline ({}); migrate its data manually before starting this server",
            baseline.version
        ))),
    }
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        params![table_name],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Error::from)
}

fn column_exists(conn: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    Ok(table_columns(conn, table_name)?
        .iter()
        .any(|column| column == column_name))
}

fn table_columns(conn: &Connection, table_name: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_database_uses_only_the_canonical_workspace_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let store = SqliteWorkspaceStore::open(&path).unwrap();
        store
            .with_conn(|conn| {
                let migrations = conn
                    .prepare("SELECT version, name FROM __yoi_schema_migrations ORDER BY version")?
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                assert_eq!(
                    migrations,
                    vec![(
                        LATEST_SCHEMA_VERSION,
                        "workspace schema baseline".to_string()
                    )]
                );
                ticket::verify_sqlite_ticket_schema(conn)?;
                merge_request::migrate(conn)?;
                let foreign_key_failures: i64 =
                    conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(foreign_key_failures, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn startup_rejects_prebaseline_workspace_history() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (49, 'legacy')",
            [],
        )
        .unwrap();
        let error = apply_migrations(&conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("predates the canonical baseline")
        );
    }

    #[test]
    fn startup_rejects_noncanonical_history_at_the_baseline_version() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (1, 'legacy')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![LATEST_SCHEMA_VERSION, "workspace schema baseline"],
        )
        .unwrap();
        let error = apply_migrations(&conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("migration history is not the canonical schema baseline")
        );
    }

    #[test]
    fn startup_fails_closed_when_current_ticket_schema_has_drifted() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        ticket::migrate_sqlite_ticket_schema(&conn).unwrap();
        merge_request::migrate(&conn).unwrap();
        apply_migrations(&conn).unwrap();
        conn.execute_batch("DROP TABLE typed_ticket_artifacts")
            .unwrap();

        let result = SqliteWorkspaceStore::from_connection(conn);
        let error = match result {
            Ok(_) => panic!("schema drift unexpectedly passed startup verification"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("typed_ticket_artifacts"));
    }

    #[tokio::test]
    async fn workspace_schema_rejects_cross_workspace_ticket_repository_reference_at_write_time() {
        let dir = tempfile::tempdir().unwrap();
        let database_path = dir.path().join("workspace.sqlite");
        let store = SqliteWorkspaceStore::open(&database_path).unwrap();
        for workspace_id in ["workspace-a", "workspace-b"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: workspace_id.to_string(),
                    owner_account_id: "owner-account".to_string(),
                    display_name: workspace_id.to_string(),
                    state: "active".to_string(),
                    created_at: "1".to_string(),
                    updated_at: "1".to_string(),
                })
                .await
                .unwrap();
        }
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: "workspace-a".to_string(),
                repository_id: "main".to_string(),
                repository_key: "main".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: "/repo-a".to_string(),
                },
                default_ref: Some("HEAD".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        drop(store);

        let backend = ticket::SqliteTicketBackend::open_verified(
            database_path.clone(),
            "workspace-b".to_string(),
        )
        .unwrap();
        let mut input = ticket::NewTicket::new("Foreign repository");
        input.repository_id = Some("main".to_string());
        let error = ticket::TicketBackend::create(&backend, input).unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY constraint failed"),
            "{error}"
        );
        drop(backend);

        SqliteWorkspaceStore::open(&database_path).unwrap();
    }

    #[tokio::test]
    async fn migrates_sqlite_and_preserves_workspace_record() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("control-plane.sqlite");
        let store = SqliteWorkspaceStore::open(&db).unwrap();

        assert_eq!(store.schema_version().await.unwrap(), 50);
        assert!(
            !store
                .with_conn(|conn| table_exists(conn, "worker_workspace_credentials"))
                .unwrap()
        );
        let record = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Yoi Dev".to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&record).await.unwrap();

        let reopened = SqliteWorkspaceStore::open(&db).unwrap();
        assert_eq!(reopened.schema_version().await.unwrap(), 50);
        assert_eq!(
            reopened.get_workspace("local-dev").await.unwrap(),
            Some(record)
        );
    }

    #[tokio::test]
    async fn objective_creation_allocates_and_resolves_workspace_resource_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".into(),
                owner_account_id: "owner-account".to_string(),
                display_name: "Workspace A".into(),
                state: "active".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .await
            .unwrap();
        store
            .upsert_objective(&ObjectiveRecord {
                workspace_id: "workspace-a".into(),
                objective_id: "objective-internal".into(),
                title: "Ship it".into(),
                state: "active".into(),
                body_md: String::new(),
                created_at: "2".into(),
                updated_at: "2".into(),
            })
            .unwrap();
        assert_eq!(
            store
                .resource_key(
                    "workspace-a",
                    WorkspaceResourceKind::Objective,
                    "objective-internal"
                )
                .unwrap()
                .as_deref(),
            Some("O-1")
        );
        assert_eq!(
            store
                .resolve_resource_reference("workspace-a", WorkspaceResourceKind::Objective, "O-1")
                .unwrap()
                .as_deref(),
            Some("objective-internal")
        );
    }

    #[tokio::test]
    async fn worker_create_reservation_allocates_uuid_before_runtime_and_replays_exact_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: "owner-account".to_string(),
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "2026-08-06T00:00:00Z".to_string(),
                updated_at: "2026-08-06T00:00:00Z".to_string(),
            })
            .await
            .unwrap();

        let memory_settings = store.get_workspace_memory_settings("workspace-a").unwrap();
        assert_eq!(memory_settings.settings_revision, 1);
        assert_eq!(memory_settings.language, "English");
        let reserved = store
            .reserve_worker_create(
                "workspace-a",
                "arcadia",
                "operation-1",
                "sha256:one",
                &memory_settings,
            )
            .unwrap();
        assert_eq!(
            reserved.worker_id.as_uuid().get_version(),
            Some(uuid::Version::SortRand)
        );
        assert_eq!(reserved.memory_settings.settings_revision, 1);
        assert_eq!(reserved.memory_settings.language, "English");
        let reserved_worker = RuntimeWorkerRef::new("arcadia", reserved.worker_id.to_string());
        assert!(
            store
                .has_active_worker_create_reservation("workspace-a", &reserved_worker)
                .unwrap()
        );
        let unchanged_memory_settings = store
            .update_workspace_memory_settings("workspace-a", 1, " English ")
            .unwrap();
        assert_eq!(unchanged_memory_settings, memory_settings);
        let updated_memory_settings = store
            .update_workspace_memory_settings("workspace-a", 1, " Français ")
            .unwrap();
        assert_eq!(updated_memory_settings.settings_revision, 2);
        assert_eq!(updated_memory_settings.language, "Français");
        assert_eq!(
            store
                .reserve_worker_create(
                    "workspace-a",
                    "arcadia",
                    "operation-1",
                    "sha256:one",
                    &updated_memory_settings,
                )
                .unwrap(),
            reserved
        );
        assert!(
            store
                .reserve_worker_create(
                    "workspace-a",
                    "arcadia",
                    "operation-1",
                    "sha256:different",
                    &updated_memory_settings,
                )
                .is_err()
        );
        assert_eq!(
            store
                .resource_key(
                    "workspace-a",
                    WorkspaceResourceKind::Worker,
                    &reserved.worker_id.to_string()
                )
                .unwrap()
                .as_deref(),
            Some("W-1")
        );
        assert_eq!(
            store
                .resolve_resource_reference("workspace-a", WorkspaceResourceKind::Worker, "W-1")
                .unwrap(),
            Some(reserved.worker_id.to_string())
        );
        let second = store
            .reserve_worker_create(
                "workspace-a",
                "arcadia",
                "operation-2",
                "sha256:two",
                &updated_memory_settings,
            )
            .unwrap();
        assert_eq!(second.memory_settings.settings_revision, 2);
        assert_eq!(
            store
                .resource_key(
                    "workspace-a",
                    WorkspaceResourceKind::Worker,
                    &second.worker_id.to_string()
                )
                .unwrap()
                .as_deref(),
            Some("W-2")
        );
        store
            .complete_worker_create_reservation("workspace-a", reserved.worker_id)
            .unwrap();
        assert!(
            !store
                .has_active_worker_create_reservation("workspace-a", &reserved_worker)
                .unwrap()
        );
        let state: String = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT state FROM worker_create_reservations \
                     WHERE workspace_id = 'workspace-a' AND worker_id = ?1",
                    [reserved.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(Error::from)
            })
            .unwrap();
        assert_eq!(state, "created");

        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspace_memory_settings SET language = ' English ' \
                     WHERE workspace_id = 'workspace-a'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.get_workspace_memory_settings("workspace-a").is_err());
        assert!(
            store
                .update_workspace_memory_settings("workspace-a", 2, "Spanish")
                .is_err()
        );
        let mut corrupt = updated_memory_settings.clone();
        corrupt.language = " English ".to_string();
        assert!(
            store
                .reserve_worker_create(
                    "workspace-a",
                    "arcadia",
                    "operation-corrupt",
                    "sha256:corrupt",
                    &corrupt,
                )
                .is_err()
        );

        store
            .with_conn(|conn| {
                conn.execute(
                    "DELETE FROM workspace_memory_settings WHERE workspace_id = 'workspace-a'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.get_workspace_memory_settings("workspace-a").is_err());
    }

    #[tokio::test]
    async fn workspace_flow_sources_keep_revisions_and_builtins_stay_resources() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: "owner-account".to_string(),
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "2026-08-06T00:00:00Z".to_string(),
                updated_at: "2026-08-06T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        let workspace_source = r#"{
            schema_version = 1;
            name = "coder-review";
            initial = "work";
            states = {
                work = {
                    instructions = "Workspace revision one.";
                    transitions = { done = { target = "done"; condition = "Done."; }; };
                };
                done = { instructions = ""; terminal = true; };
            };
        }"#;
        let workspace = store
            .put_flow_source_for_kind(
                "workspace-a",
                FlowSourceKind::Workspace,
                "flows/coder-review.dcdl",
                workspace_source,
                "2026-08-06T00:00:01Z",
            )
            .unwrap();
        let builtin = flow::builtin_flow_source("coder-review").unwrap();
        assert_eq!(builtin.slug, workspace.name);
        assert!(
            store
                .put_flow_source_for_kind(
                    "workspace-a",
                    FlowSourceKind::Builtin,
                    builtin.path,
                    builtin.content,
                    "2026-08-06T00:00:02Z",
                )
                .is_err()
        );
        assert_eq!(
            store.list_flow_sources("workspace-a").unwrap(),
            vec![workspace.clone()]
        );

        let revision_two =
            workspace_source.replace("Workspace revision one.", "Workspace revision two.");
        let updated = store
            .put_flow_source_for_kind(
                "workspace-a",
                FlowSourceKind::Workspace,
                "flows/coder-review.dcdl",
                &revision_two,
                "2026-08-06T00:00:03Z",
            )
            .unwrap();
        assert_eq!(updated.flow_id, workspace.flow_id);
        assert_eq!(updated.revision, 2);
        let pinned = store
            .get_flow_source_revision("workspace-a", &workspace.flow_id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(pinned.content, workspace_source);
        assert_eq!(pinned.definition.name, "coder-review");
    }

    #[tokio::test]
    async fn ticket_worker_assignment_replaces_current_and_preserves_audit_history() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("server.db");
        let store = SqliteWorkspaceStore::open(&db).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: "owner-account".to_string(),
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "2026-07-32T00:00:00Z".to_string(),
                updated_at: "2026-07-32T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch(
                    r#"
INSERT INTO typed_tickets (
    workspace_id, ticket_id, slug, title, status, kind, priority, body,
    workflow_state, workflow_state_explicit
) VALUES
    ('workspace-a', 'ticket-1', 'ticket-1', 'Ticket 1', 'open', 'task', 'normal', '', 'planning', 1),
    ('workspace-a', 'ticket-2', 'ticket-2', 'Ticket 2', 'open', 'task', 'normal', '', 'planning', 1),
    ('workspace-a', 'ticket-3', 'ticket-3', 'Ticket 3', 'open', 'task', 'normal', '', 'planning', 1);
INSERT INTO worker_registry (
    workspace_id, runtime_id, worker_id, display_name, retention_state, created_at, updated_at
) VALUES
    ('workspace-a', 'runtime-1', 'worker-1', 'Worker 1', 'normal', '1', '1'),
    ('workspace-a', 'runtime-1', 'worker-other', 'Other Worker', 'normal', '1', '1'),
    ('workspace-a', 'runtime-2', 'worker-2', 'Worker 2', 'normal', '1', '1'),
    ('workspace-a', 'runtime-3', 'worker-3', 'Worker 3', 'normal', '1', '1');
"#,
                )?;
                Ok(())
            })
            .unwrap();

        let first = TicketCoderAssignmentRecord {
            workspace_id: "workspace-a".to_string(),
            ticket_id: "ticket-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            worker: RuntimeWorkerRef::new("runtime-1", "worker-1"),
            assigned_by: "user-1".to_string(),
            assigned_at: "2026-07-32T00:00:01Z".to_string(),
        };
        let created = store
            .set_current_ticket_coder_assignment(&first, None, "event-1", "operation-1", false)
            .unwrap();
        assert_eq!(created.current, first);
        assert_eq!(created.previous, None);
        let retried = store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    assignment_id: "ignored-retry-assignment".to_string(),
                    ..first.clone()
                },
                None,
                "ignored-retry-event",
                "operation-1",
                false,
            )
            .unwrap();
        assert_eq!(retried.current, first);
        assert_eq!(
            store
                .list_ticket_coder_assignment_events("workspace-a", "ticket-1", 10)
                .unwrap()
                .len(),
            1,
            "idempotent retry must not append another assignment event"
        );
        let implicit_reassign = store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    assignment_id: "implicit-reassign".to_string(),
                    worker: RuntimeWorkerRef::new("runtime-1", "worker-other"),
                    ..first.clone()
                },
                None,
                "implicit-event",
                "implicit-operation",
                false,
            )
            .unwrap_err();
        assert!(matches!(
            implicit_reassign,
            Error::TicketAssignmentConflict(_)
        ));
        let worker_conflict = store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    ticket_id: "ticket-2".to_string(),
                    assignment_id: "worker-conflict".to_string(),
                    ..first.clone()
                },
                None,
                "worker-conflict-event",
                "worker-conflict-operation",
                false,
            )
            .unwrap_err();
        assert!(matches!(
            worker_conflict,
            Error::TicketAssignmentConflict(_)
        ));

        let second = TicketCoderAssignmentRecord {
            assignment_id: "assignment-2".to_string(),
            worker: RuntimeWorkerRef::new("runtime-2", "worker-2"),
            assigned_by: "user-2".to_string(),
            assigned_at: "2026-07-32T00:00:02Z".to_string(),
            ..first.clone()
        };
        let replaced = store
            .set_current_ticket_coder_assignment(
                &second,
                Some("assignment-1"),
                "event-2",
                "operation-2",
                true,
            )
            .unwrap();
        assert_eq!(replaced.current, second);
        assert_eq!(replaced.previous, Some(first.clone()));
        let replayed_reassignment = store
            .set_current_ticket_coder_assignment(
                &second,
                Some("assignment-1"),
                "ignored-reassign-event",
                "operation-2",
                true,
            )
            .unwrap();
        assert_eq!(replayed_reassignment, replaced);
        assert_eq!(
            store
                .get_current_ticket_coder_assignment("workspace-a", "ticket-1")
                .unwrap(),
            Some(second.clone())
        );

        let stale = store
            .clear_current_ticket_worker_assignment(
                "workspace-a",
                "ticket-1",
                Some("assignment-1"),
                "unassign-operation-stale",
                "event-stale",
                "user-1",
                "2026-07-32T00:00:03Z",
            )
            .unwrap_err();
        assert!(matches!(stale, Error::TicketAssignmentConflict(_)));

        let cleared = store
            .clear_current_ticket_worker_assignment(
                "workspace-a",
                "ticket-1",
                Some("assignment-2"),
                "unassign-operation-2",
                "event-3",
                "user-2",
                "2026-07-32T00:00:03Z",
            )
            .unwrap();
        assert_eq!(cleared, Some(second.clone()));
        let retried_clear = store
            .clear_current_ticket_worker_assignment(
                "workspace-a",
                "ticket-1",
                Some("assignment-2"),
                "unassign-operation-2",
                "ignored-clear-event",
                "user-2",
                "2026-07-32T00:00:04Z",
            )
            .unwrap();
        assert_eq!(retried_clear, Some(second));
        store
            .reserve_ticket_assignment_operation(
                "workspace-a",
                "reserved-operation",
                "ticket-3",
                "runtime-3",
                None,
                "sha256:reserved",
                "2026-07-32T00:00:05Z",
            )
            .unwrap();
        drop(store);
        let store = SqliteWorkspaceStore::open(&db).unwrap();
        let pending = store
            .get_ticket_assignment_operation("workspace-a", "reserved-operation")
            .unwrap()
            .unwrap();
        assert_eq!(pending.worker, None);
        assert_eq!(
            pending.request_fingerprint.as_deref(),
            Some("sha256:reserved")
        );
        store
            .bind_ticket_assignment_operation_worker(
                "workspace-a",
                "reserved-operation",
                "worker-3",
            )
            .unwrap();
        let reserved_assignment = TicketCoderAssignmentRecord {
            workspace_id: "workspace-a".to_string(),
            ticket_id: "ticket-3".to_string(),
            assignment_id: "assignment-3".to_string(),
            worker: RuntimeWorkerRef::new("runtime-3", "worker-3"),
            assigned_by: "runtime".to_string(),
            assigned_at: "2026-07-32T00:00:06Z".to_string(),
        };
        let completed_reservation = store
            .set_current_ticket_coder_assignment(
                &reserved_assignment,
                None,
                "reserved-event",
                "reserved-operation",
                false,
            )
            .unwrap();
        assert_eq!(completed_reservation.current, reserved_assignment);
        assert_eq!(
            store
                .get_ticket_assignment_operation("workspace-a", "reserved-operation")
                .unwrap()
                .and_then(|operation| operation.assignment_id),
            Some("assignment-3".to_string())
        );
        assert_eq!(
            store
                .get_current_ticket_coder_assignment("workspace-a", "ticket-1")
                .unwrap(),
            None
        );
        store
            .rollback_ticket_assignment_operation("workspace-a", "reserved-operation")
            .unwrap();
        assert_eq!(
            store
                .get_ticket_assignment_operation("workspace-a", "reserved-operation")
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .get_current_ticket_coder_assignment("workspace-a", "ticket-3")
                .unwrap(),
            None
        );
        assert!(
            store
                .list_ticket_coder_assignment_events("workspace-a", "ticket-3", 10)
                .unwrap()
                .is_empty()
        );
        store
            .rollback_ticket_assignment_operation("workspace-a", "reserved-operation")
            .unwrap();

        let events = store
            .list_ticket_coder_assignment_events("workspace-a", "ticket-1", 10)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.action.as_str())
                .collect::<Vec<_>>(),
            vec!["unassigned", "reassigned", "assigned"]
        );
        assert_eq!(events[1].assignment_id.as_deref(), Some("assignment-2"));
        assert_eq!(
            events[1].previous_assignment_id.as_deref(),
            Some("assignment-1")
        );
    }

    #[tokio::test]
    async fn role_assignment_routes_and_manual_start_are_state_fenced_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db_path).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-role".to_string(),
                display_name: "Role Workspace".to_string(),
                state: "active".to_string(),
                owner_account_id: "owner-account".to_string(),
                created_at: "2026-09-01T00:00:00Z".to_string(),
                updated_at: "2026-09-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: "workspace-role".to_string(),
                repository_id: "main".to_string(),
                repository_key: "main".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: RepositorySource {
                    kind: workspace_api::RepositorySourceKind::File,
                    uri: "file:///tmp/main".to_string(),
                },
                default_ref: Some("develop".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "2026-09-01T00:00:00Z".to_string(),
                updated_at: "2026-09-01T00:00:00Z".to_string(),
            })
            .unwrap();
        let worker = WorkerRegistryRecord {
            workspace_id: "workspace-role".to_string(),
            worker: RuntimeWorkerRef::new("runtime-role", "worker-role"),
            display_name: "Coder".to_string(),
            profile: Some("builtin:coder".to_string()),
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "2026-09-01T00:00:00Z".to_string(),
            updated_at: "2026-09-01T00:00:00Z".to_string(),
        };
        store.upsert_worker_registry(&worker).unwrap();
        let backend =
            ticket::SqliteTicketBackend::open_verified(&db_path, "workspace-role").unwrap();
        let mut input = ticket::NewTicket::new("Role assignment");
        input.body = ticket::MarkdownText::new("test");
        input.workflow_state = Some(ticket::TicketWorkflowState::Ready);
        input.repository_id = Some("main".to_string());
        input.ref_selector = Some("develop".to_string());
        let ticket = ticket::TicketBackend::create(&backend, input).unwrap();

        let orchestrator = TicketRoleAssignmentRecord {
            workspace_id: "workspace-role".to_string(),
            ticket_id: ticket.id.clone(),
            assignment_id: "orchestrator-1".to_string(),
            role: TicketAssignmentRole::Orchestrator,
            principal: TicketAssignmentPrincipal::WorkspaceAgent {
                agent_key: "workspace-orchestrator".to_string(),
            },
            assigned_by: "user".to_string(),
            assigned_at: "2026-09-01T00:00:00Z".to_string(),
        };
        store
            .set_current_ticket_role_assignment(
                &orchestrator,
                None,
                "event-orchestrator",
                "op-orchestrator",
                false,
            )
            .unwrap();
        assert_eq!(
            store
                .list_current_ticket_role_assignments("workspace-role", &ticket.id)
                .unwrap(),
            vec![orchestrator.clone()]
        );

        let replacement = TicketRoleAssignmentRecord {
            assignment_id: "orchestrator-2".to_string(),
            assigned_at: "2026-09-01T00:00:30Z".to_string(),
            ..orchestrator.clone()
        };
        store
            .set_current_ticket_role_assignment(
                &replacement,
                Some("orchestrator-1"),
                "event-orchestrator-reassign",
                "op-orchestrator-reassign",
                true,
            )
            .unwrap();
        assert_eq!(
            store
                .list_current_ticket_role_assignments("workspace-role", &ticket.id)
                .unwrap(),
            vec![replacement]
        );

        let coder = TicketRoleAssignmentRecord {
            workspace_id: "workspace-role".to_string(),
            ticket_id: ticket.id.clone(),
            assignment_id: "coder-manual-1".to_string(),
            role: TicketAssignmentRole::Coder,
            principal: TicketAssignmentPrincipal::Worker {
                runtime_id: "runtime-role".to_string(),
                worker_id: "worker-role".to_string(),
            },
            assigned_by: "user".to_string(),
            assigned_at: "2026-09-01T00:01:00Z".to_string(),
        };
        assert!(
            store
                .start_ready_ticket_with_coder_assignment(
                    &coder,
                    "event-coder-conflict",
                    "op-coder-conflict",
                )
                .is_err()
        );
        assert_eq!(
            ticket::TicketBackend::show(&backend, ticket.id.clone().into())
                .unwrap()
                .meta
                .workflow_state,
            ticket::TicketWorkflowState::Ready
        );
        assert!(
            store
                .clear_current_ticket_role_assignment(
                    "workspace-role",
                    &ticket.id,
                    TicketAssignmentRole::Orchestrator,
                    "orchestrator-2",
                    "event-clear-orchestrator",
                    "op-clear-orchestrator",
                    "user",
                    "2026-09-01T00:02:00Z",
                    Some("manual start"),
                )
                .unwrap()
        );
        let started = store
            .start_ready_ticket_with_coder_assignment(&coder, "event-coder", "op-coder")
            .unwrap();
        assert_eq!(started, coder);
        let replay_input = TicketRoleAssignmentRecord {
            assignment_id: "coder-regenerated-result".to_string(),
            assigned_at: "2026-09-01T00:01:59Z".to_string(),
            ..coder.clone()
        };
        let replayed = store
            .start_ready_ticket_with_coder_assignment(
                &replay_input,
                "event-coder-regenerated",
                "op-coder",
            )
            .unwrap();
        assert_eq!(replayed, coder);
        let ticket = ticket::TicketBackend::show(&backend, ticket.id.clone().into()).unwrap();
        assert_eq!(
            ticket.meta.workflow_state,
            ticket::TicketWorkflowState::InProgress
        );
        assert_eq!(
            ticket
                .events
                .last()
                .and_then(|event| event.attributes.get("assignment_id"))
                .map(String::as_str),
            Some("coder-manual-1")
        );
        assert_eq!(
            store
                .get_current_ticket_role_assignment(
                    "workspace-role",
                    &ticket.meta.id,
                    TicketAssignmentRole::Coder,
                )
                .unwrap(),
            Some(coder.clone())
        );
        assert!(
            store
                .delete_worker_registry("workspace-role", &worker.worker)
                .is_err(),
            "active role assignment must prevent Worker removal"
        );
        assert!(
            store
                .cancel_current_ticket_coder_assignment(
                    "workspace-role",
                    &ticket.meta.id,
                    "coder-manual-1",
                    "event-cancel-coder",
                    "event-cancel-state",
                    "op-cancel-coder",
                    "user",
                    "2026-09-01T00:03:00Z",
                    "implementation needs to be redone",
                )
                .unwrap()
        );
        assert!(
            store
                .cancel_current_ticket_coder_assignment(
                    "workspace-role",
                    &ticket.meta.id,
                    "coder-manual-1",
                    "event-cancel-coder-replay",
                    "event-cancel-state-replay",
                    "op-cancel-coder",
                    "user",
                    "2026-09-01T00:03:30Z",
                    "implementation needs to be redone",
                )
                .unwrap(),
            "same operation must be idempotent after the assignment is cleared"
        );
        let cancelled_ticket =
            ticket::TicketBackend::show(&backend, ticket.meta.id.clone().into()).unwrap();
        assert_eq!(
            cancelled_ticket.meta.workflow_state,
            ticket::TicketWorkflowState::Ready
        );
        assert_eq!(
            cancelled_ticket
                .events
                .last()
                .and_then(|event| event.attributes.get("assignment_id"))
                .map(String::as_str),
            Some("coder-manual-1")
        );
        assert!(
            store
                .get_current_ticket_role_assignment(
                    "workspace-role",
                    &ticket.meta.id,
                    TicketAssignmentRole::Coder,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .delete_worker_registry("workspace-role", &worker.worker)
                .unwrap()
        );
        let removed_worker_assignment = TicketRoleAssignmentRecord {
            assignment_id: "contributor-removed-worker".to_string(),
            role: TicketAssignmentRole::Contributor,
            assigned_at: "2026-09-01T00:04:00Z".to_string(),
            ..coder
        };
        assert!(
            store
                .set_current_ticket_role_assignment(
                    &removed_worker_assignment,
                    None,
                    "event-removed-worker",
                    "op-removed-worker",
                    false,
                )
                .is_err(),
            "removed/tombstoned Worker cannot become a new role principal"
        );
    }

    #[test]
    fn server_refuses_a_database_from_a_newer_schema_generation() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (51, 'future')",
            [],
        )
        .unwrap();

        let error = apply_migrations(&conn).unwrap_err().to_string();
        assert!(error.contains("schema version 51 is newer"), "{error}");
        assert!(error.contains("refusing to serve"), "{error}");
    }

    #[test]
    fn startup_rejects_missing_workspace_resource_constraint_trigger() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let store = SqliteWorkspaceStore::open(&path).unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER ticket_worker_assignments_validate_insert")?;
                Ok(())
            })
            .unwrap();
        drop(store);

        let error = match SqliteWorkspaceStore::open(&path) {
            Ok(_) => panic!("missing assignment constraint trigger must fail closed"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("ticket_worker_assignments_validate_insert"),
            "{error}"
        );
    }

    #[test]
    fn startup_accepts_retained_assignment_history_after_parent_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let store = SqliteWorkspaceStore::open(&path).unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch(
                    r#"
INSERT INTO accounts (account_id, kind, handle, display_name, created_at, updated_at)
VALUES ('owner-account', 'user', 'owner-account', 'Owner Account', '2026-01-01', '2026-01-01');
INSERT INTO workspaces (workspace_id, owner_account_id, display_name, state, created_at, updated_at)
VALUES ('workspace-a', 'owner-account', 'A', 'active', '2026-01-01', '2026-01-01');
INSERT INTO typed_tickets (
    workspace_id, ticket_id, slug, title, status, kind, priority, body,
    workflow_state, workflow_state_explicit
) VALUES ('workspace-a', 'ticket-a', 'ticket-a', 'A', 'open', 'task', 'normal', '', 'planning', 1);
INSERT INTO worker_registry (
    workspace_id, runtime_id, worker_id, display_name, retention_state, created_at, updated_at
) VALUES (
    'workspace-a', 'runtime-a', '00000000-0000-7000-8000-000000000001',
    'Worker A', 'normal', '2026-01-01', '2026-01-01'
);
INSERT INTO ticket_worker_assignments (
    workspace_id, ticket_id, assignment_id, runtime_id, worker_id, assigned_by, assigned_at
) VALUES (
    'workspace-a', 'ticket-a', 'assignment-a', 'runtime-a',
    '00000000-0000-7000-8000-000000000001', 'tester', '2026-01-01'
);
DELETE FROM worker_registry
WHERE workspace_id = 'workspace-a' AND worker_id = '00000000-0000-7000-8000-000000000001';
DELETE FROM typed_tickets
WHERE workspace_id = 'workspace-a' AND ticket_id = 'ticket-a';
INSERT INTO workspaces (workspace_id, owner_account_id, display_name, state, created_at, updated_at)
VALUES ('workspace-b', 'owner-account', 'B', 'active', '2026-01-01', '2026-01-01');
INSERT INTO typed_tickets (
    workspace_id, ticket_id, slug, title, status, kind, priority, body,
    workflow_state, workflow_state_explicit
) VALUES ('workspace-b', 'ticket-a', 'ticket-a-b', 'B', 'open', 'task', 'normal', '', 'planning', 1);
INSERT INTO worker_registry (
    workspace_id, runtime_id, worker_id, display_name, retention_state, created_at, updated_at
) VALUES (
    'workspace-b', 'runtime-b', '00000000-0000-7000-8000-000000000001',
    'Worker B', 'normal', '2026-01-01', '2026-01-01'
);
"#,
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);

        let reopened = SqliteWorkspaceStore::open(&path).unwrap();
        reopened
            .with_conn(|conn| {
                let retained: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM ticket_worker_assignments WHERE workspace_id = 'workspace-a'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(retained, 1);
                let ticket_tombstones: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM ticket_assignment_ticket_tombstones WHERE workspace_id = 'workspace-a' AND ticket_id = 'ticket-a'",
                    [],
                    |row| row.get(0),
                )?;
                let worker_tombstones: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM ticket_assignment_worker_tombstones WHERE workspace_id = 'workspace-a' AND runtime_id = 'runtime-a' AND worker_id = '00000000-0000-7000-8000-000000000001'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(ticket_tombstones, 1);
                assert_eq!(worker_tombstones, 1);
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn workspace_bootstrap_validates_key_and_rejects_duplicate_workspace() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store
            .upsert_account(&AccountRecord {
                account_id: "owner-account".to_string(),
                kind: "user".to_string(),
                handle: "owner".to_string(),
                display_name: "Owner".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        let workspace = WorkspaceRecord {
            workspace_id: "workspace-invalid-key".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Invalid key".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        let repository = RepositoryRecord {
            workspace_id: workspace.workspace_id.clone(),
            repository_id: Uuid::now_v7().to_string(),
            repository_key: "Invalid_Key".to_string(),
            kind: "git".to_string(),
            provider: Some("git".to_string()),
            source: workspace_api::RepositorySource {
                kind: workspace_api::RepositorySourceKind::LocalPath,
                uri: "/repo".to_string(),
            },
            default_ref: Some("develop".to_string()),
            source_revision: 1,
            source_fingerprint: "sha256:test".to_string(),
            observed_status: RepositoryObservedStatus::Unverified,
            observed_at: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };

        let error = store
            .create_workspace_bootstrap(&WorkspaceBootstrapRecord {
                operation_key: "invalid-key".to_string(),
                request_fingerprint: "sha256:invalid-key".to_string(),
                workspace: workspace.clone(),
                repository,
            })
            .unwrap_err()
            .to_string();

        assert!(error.contains("invalid Repository key"), "{error}");
        assert!(
            store
                .get_workspace(&workspace.workspace_id)
                .await
                .unwrap()
                .is_none()
        );

        let valid_repository = RepositoryRecord {
            workspace_id: workspace.workspace_id.clone(),
            repository_id: Uuid::now_v7().to_string(),
            repository_key: "main".to_string(),
            kind: "git".to_string(),
            provider: Some("git".to_string()),
            source: workspace_api::RepositorySource {
                kind: workspace_api::RepositorySourceKind::LocalPath,
                uri: "/repo".to_string(),
            },
            default_ref: Some("develop".to_string()),
            source_revision: 1,
            source_fingerprint: "sha256:test".to_string(),
            observed_status: RepositoryObservedStatus::Unverified,
            observed_at: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        let first = WorkspaceBootstrapRecord {
            operation_key: "create-workspace".to_string(),
            request_fingerprint: "sha256:create-workspace".to_string(),
            workspace,
            repository: valid_repository,
        };
        assert!(!store.create_workspace_bootstrap(&first).unwrap().replayed);
        let mut duplicate = first;
        duplicate.operation_key = "duplicate-workspace".to_string();
        duplicate.repository.repository_id = Uuid::now_v7().to_string();
        let error = store
            .create_workspace_bootstrap(&duplicate)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Workspace identity already exists"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn repository_records_round_trip() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 50);
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Local Dev".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workspace(&workspace).await.unwrap();

        let repository = RepositoryRecord {
            workspace_id: "local-dev".to_string(),
            repository_id: "main".to_string(),
            repository_key: "yoi".to_string(),
            kind: "git".to_string(),
            provider: Some("git".to_string()),
            source: RepositorySource {
                kind: workspace_api::RepositorySourceKind::LocalPath,
                uri: "/repo".to_string(),
            },
            default_ref: Some("HEAD".to_string()),
            source_revision: 1,
            source_fingerprint: crate::repository_source::repository_source_fingerprint(
                &RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: "/repo".to_string(),
                },
            ),
            observed_status: RepositoryObservedStatus::Unverified,
            observed_at: None,
            created_at: "2".to_string(),
            updated_at: "2".to_string(),
        };
        store.upsert_repository(&repository).unwrap();
        assert_eq!(
            store.get_repository("local-dev", "main").unwrap(),
            Some(repository.clone())
        );
        assert_eq!(
            store.list_repositories("local-dev").unwrap(),
            vec![repository.clone()]
        );
        assert_eq!(
            store.list_repositories("other-workspace").unwrap(),
            Vec::new()
        );

        let other_workspace = WorkspaceRecord {
            workspace_id: "other-workspace".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Other Workspace".to_string(),
            state: "active".to_string(),
            created_at: "3".to_string(),
            updated_at: "3".to_string(),
        };
        store.upsert_workspace(&other_workspace).await.unwrap();
        let mut other_repository = repository.clone();
        other_repository.workspace_id = other_workspace.workspace_id.clone();
        other_repository.repository_id = "other-main".to_string();
        other_repository.repository_key = "other-yoi".to_string();
        other_repository.source.uri = "/other/yoi".to_string();
        other_repository.source_fingerprint =
            crate::repository_source::repository_source_fingerprint(&other_repository.source);
        store.upsert_repository(&other_repository).unwrap();

        assert_eq!(
            store.get_repository("local-dev", "main").unwrap(),
            Some(repository)
        );
        assert_eq!(
            store
                .get_repository("other-workspace", "other-main")
                .unwrap(),
            Some(other_repository)
        );
    }

    #[tokio::test]
    async fn memory_authority_records_round_trip_and_close_staging() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 50);
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Local Dev".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workspace(&workspace).await.unwrap();

        let document = store
            .ensure_memory_document("local-dev", "# Memory\n", "2")
            .unwrap();
        assert_eq!(document.body_md, "# Memory\n");
        let updated = MemoryDocumentRecord {
            workspace_id: "local-dev".to_string(),
            body_md: "# Memory\n\n- fact\n".to_string(),
            created_at: document.created_at.clone(),
            updated_at: "3".to_string(),
        };
        store.upsert_memory_document(&updated).unwrap();
        assert_eq!(
            store.get_memory_document("local-dev").unwrap(),
            Some(updated)
        );

        let staging = MemoryStagingRecord {
            workspace_id: "local-dev".to_string(),
            candidate_id: "candidate-a".to_string(),
            raw_json: r#"{"claim":"candidate"}"#.to_string(),
            source_path: Some("memory/_staging/candidate-a.json".to_string()),
            imported_at: "4".to_string(),
        };
        store.upsert_memory_staging_record(&staging).unwrap();
        assert_eq!(
            store
                .get_memory_staging_record("local-dev", "candidate-a")
                .unwrap(),
            Some(staging.clone())
        );
        assert_eq!(
            store.list_memory_staging_records("local-dev", 10).unwrap(),
            vec![staging.clone()]
        );

        let resolution = MemoryStagingResolutionRecord {
            workspace_id: "local-dev".to_string(),
            candidate_id: staging.candidate_id.clone(),
            action: "apply".to_string(),
            reason: "accepted".to_string(),
            affected_refs_json: r#"["summary"]"#.to_string(),
            staging_raw_json: staging.raw_json.clone(),
            source_path: staging.source_path.clone(),
            imported_at: staging.imported_at.clone(),
            resolved_at: "5".to_string(),
        };
        store.insert_memory_staging_resolution(&resolution).unwrap();
        assert!(
            store
                .delete_memory_staging_record("local-dev", "candidate-a")
                .unwrap()
        );
        assert_eq!(store.count_memory_staging_records("local-dev").unwrap(), 0);
        assert_eq!(
            store
                .list_memory_staging_resolutions("local-dev", 10)
                .unwrap(),
            vec![resolution]
        );
    }

    #[tokio::test]
    async fn worker_workdir_registry_round_trips_and_preserves_pinned_retention() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db).unwrap();
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: "owner-account".to_string(),
            display_name: "Local Dev".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workspace(&workspace).await.unwrap();
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: workspace.workspace_id.clone(),
                repository_id: "repo".to_string(),
                repository_key: "repository".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: "/repo".to_string(),
                },
                default_ref: Some("HEAD".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();

        let worker = WorkerRegistryRecord {
            workspace_id: "local-dev".to_string(),
            worker: RuntimeWorkerRef::new("embedded", "1"),
            display_name: "Browser 1".to_string(),
            profile: Some("builtin:companion".to_string()),
            retention_state: "pinned".to_string(),
            transcript_ref: Some("runtime://embedded/workers/00000001/transcript".to_string()),
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "2".to_string(),
            updated_at: "2".to_string(),
        };
        store.upsert_worker_registry(&worker).unwrap();
        let mut runtime_sync_worker = worker.clone();
        runtime_sync_worker.retention_state = "normal".to_string();
        runtime_sync_worker.updated_at = "5".to_string();
        store.upsert_worker_registry(&runtime_sync_worker).unwrap();
        assert_eq!(
            store
                .resource_key(
                    "local-dev",
                    WorkspaceResourceKind::Worker,
                    &worker.worker.worker_id,
                )
                .unwrap()
                .as_deref(),
            Some("W-1")
        );
        let mut expected_worker = worker.clone();
        expected_worker.updated_at = "5".to_string();

        let workdir = WorkdirRegistryRecord {
            workspace_id: "local-dev".to_string(),
            workdir_id: "0000019a00000000001".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            creation_selector: Some("develop".to_string()),
            creation_ref: Some("abcdef".to_string()),
            creation_tree: Some("tree-creation".to_string()),
            current_selector: None,
            current_ref: Some("abcdef".to_string()),
            current_tree: Some("tree-current".to_string()),
            observed_at_epoch_seconds: Some(1_777_777_777),
            materialization_status: "not_found".to_string(),
            cleanliness: "clean".to_string(),
            created_at: "2".to_string(),
            updated_at: "3".to_string(),
        };
        store.upsert_workdir_registry(&workdir).unwrap();
        let unmanaged_workdir = WorkdirRegistryRecord {
            workspace_id: "local-dev".to_string(),
            workdir_id: "runtime-direct".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            creation_selector: Some("feature".to_string()),
            creation_ref: Some("123456".to_string()),
            creation_tree: None,
            current_selector: Some("feature".to_string()),
            current_ref: Some("123456".to_string()),
            current_tree: None,
            observed_at_epoch_seconds: None,
            materialization_status: "present".to_string(),
            cleanliness: "unknown".to_string(),
            created_at: "3".to_string(),
            updated_at: "4".to_string(),
        };
        store.upsert_workdir_registry(&unmanaged_workdir).unwrap();

        let link = WorkerWorkdirLinkRecord {
            workspace_id: "local-dev".to_string(),
            worker: worker.worker.clone(),
            workdir_id: workdir.workdir_id.clone(),
            role: "attachment".to_string(),
            linked_at: "4".to_string(),
            unlinked_at: None,
        };
        store
            .reserve_worker_workdir_attachment("local-dev", &workdir.workdir_id, "spawn-1", "4")
            .unwrap();
        assert!(matches!(
            store.reserve_worker_workdir_attachment(
                "local-dev",
                &workdir.workdir_id,
                "spawn-2",
                "4"
            ),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
        assert!(matches!(
            store.delete_workdir_registry("local-dev", &workdir.workdir_id),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
        assert!(matches!(
            store.attach_worker_workdir(&link),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
        assert_eq!(
            store
                .finalize_reserved_worker_workdir_attachment(&link, "spawn-1")
                .unwrap(),
            link
        );
        assert_eq!(store.attach_worker_workdir(&link).unwrap(), link);

        assert_eq!(
            store
                .get_worker_registry("local-dev", &worker.worker)
                .unwrap(),
            Some(expected_worker.clone())
        );
        assert_eq!(
            store
                .get_workdir_registry("local-dev", "0000019a00000000001")
                .unwrap(),
            Some(workdir.clone())
        );
        assert_eq!(
            store.list_workdir_registry("local-dev", 10).unwrap(),
            vec![unmanaged_workdir.clone(), workdir.clone()]
        );
        assert_eq!(
            store
                .list_worker_workdir_links("local-dev", &worker.worker)
                .unwrap(),
            vec![link.clone()]
        );

        let worker_conflict = WorkerWorkdirLinkRecord {
            workdir_id: unmanaged_workdir.workdir_id.clone(),
            linked_at: "5".to_string(),
            ..link.clone()
        };
        assert!(matches!(
            store.attach_worker_workdir(&worker_conflict),
            Err(Error::WorkdirAttachmentConflict(_))
        ));

        let second_worker = WorkerRegistryRecord {
            worker: RuntimeWorkerRef::new("embedded", "2"),
            display_name: "Browser 2".to_string(),
            created_at: "5".to_string(),
            updated_at: "5".to_string(),
            ..worker.clone()
        };
        store.upsert_worker_registry(&second_worker).unwrap();
        let workdir_conflict = WorkerWorkdirLinkRecord {
            worker: second_worker.worker.clone(),
            linked_at: "5".to_string(),
            ..link.clone()
        };
        assert!(matches!(
            store.attach_worker_workdir(&workdir_conflict),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
        assert!(matches!(
            store.detach_worker_workdir("local-dev", &worker.worker, Some("wrong-workdir"), "6",),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
        let detached = store
            .detach_worker_workdir("local-dev", &worker.worker, Some(&workdir.workdir_id), "6")
            .unwrap()
            .unwrap();
        assert_eq!(detached.unlinked_at.as_deref(), Some("6"));
        assert!(
            store
                .worker_workdir_link_history_exists("local-dev", &worker.worker)
                .unwrap()
        );
        assert_eq!(
            store.attach_worker_workdir(&workdir_conflict).unwrap(),
            workdir_conflict
        );
    }

    #[tokio::test]
    async fn worker_control_grants_are_idempotent_scoped_and_support_internal_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("control-grants.db");
        let store = SqliteWorkspaceStore::open(&database).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-control".to_string(),
                owner_account_id: "owner-account".to_string(),
                display_name: "Control grants".to_string(),
                state: "active".to_string(),
                created_at: "2026-07-27T00:00:00Z".to_string(),
                updated_at: "2026-07-27T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        let worker_record = |worker_id: &str, display_name: &str| WorkerRegistryRecord {
            workspace_id: "workspace-control".to_string(),
            worker: RuntimeWorkerRef::new("runtime-a", worker_id),
            display_name: display_name.to_string(),
            profile: None,
            retention_state: "normal".to_string(),
            transcript_ref: None,
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "2026-07-27T00:00:00Z".to_string(),
            updated_at: "2026-07-27T00:00:00Z".to_string(),
        };
        let controller_record = worker_record("1", "Controller");
        let subject_record = worker_record("2", "Subject");
        store.upsert_worker_registry(&controller_record).unwrap();
        store.upsert_worker_registry(&subject_record).unwrap();

        let grant = WorkerControlGrantRecord {
            workspace_id: "workspace-control".to_string(),
            grant_id: "grant-1".to_string(),
            controller: controller_record.worker.clone(),
            subject: subject_record.worker.clone(),
            relation: "spawned".to_string(),
            origin: "worker_spawn".to_string(),
            permissions: vec![
                "observe".to_string(),
                "send_input".to_string(),
                "stop".to_string(),
            ],
            operation_id: "spawn-op-1".to_string(),
            created_at: "2026-07-27T00:00:01Z".to_string(),
            revoked_at: None,
        };
        assert_eq!(store.create_worker_control_grant(&grant).unwrap(), grant);
        assert_eq!(store.create_worker_control_grant(&grant).unwrap(), grant);
        assert_eq!(
            store
                .list_active_worker_control_grants(
                    "workspace-control",
                    &controller_record.worker,
                    10,
                )
                .unwrap(),
            vec![grant.clone()]
        );
        assert_eq!(
            store
                .get_active_worker_control_grant(
                    "workspace-control",
                    &controller_record.worker,
                    &subject_record.worker,
                )
                .unwrap(),
            Some(grant.clone())
        );

        drop(store);
        let store = SqliteWorkspaceStore::open(&database).unwrap();
        assert_eq!(
            store
                .list_active_worker_control_grants(
                    "workspace-control",
                    &controller_record.worker,
                    10,
                )
                .unwrap(),
            vec![grant.clone()],
            "known Runtime Worker grants survive Backend restart"
        );

        let conflicting_replay = WorkerControlGrantRecord {
            subject: controller_record.worker.clone(),
            ..grant.clone()
        };
        assert!(matches!(
            store.create_worker_control_grant(&conflicting_replay),
            Err(Error::InvalidInput(_))
        ));
        assert!(
            store
                .revoke_worker_control_grant(
                    "workspace-control",
                    &grant.grant_id,
                    "2026-07-27T00:00:02Z",
                )
                .unwrap()
        );
        assert!(
            store
                .list_active_worker_control_grants(
                    "workspace-control",
                    &controller_record.worker,
                    10,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .delete_worker_registry("workspace-control", &subject_record.worker)
                .unwrap()
        );
        assert!(
            store
                .get_worker_control_grant("workspace-control", &grant.grant_id)
                .unwrap()
                .is_none(),
            "deleting a subject Worker cascades its durable control grants"
        );
    }

    #[tokio::test]
    async fn account_and_login_records_round_trip() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 50);
        let now = "2026-07-22T00:00:00Z".to_string();
        let account = AccountRecord {
            account_id: "acct-user-alice".to_string(),
            kind: "user".to_string(),
            handle: "alice".to_string(),
            display_name: "Alice".to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        store.upsert_account(&account).unwrap();
        let user = UserRecord {
            user_id: "user-alice".to_string(),
            account_id: account.account_id.clone(),
            handle: account.handle.clone(),
            display_name: account.display_name.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        store.upsert_user(&user).unwrap();
        assert_eq!(
            store.get_account_by_handle("user", "alice").unwrap(),
            Some(account.clone())
        );
        assert_eq!(
            store.get_user_by_handle("alice").unwrap(),
            Some(user.clone())
        );

        let workspace = WorkspaceRecord {
            workspace_id: "workspace".to_string(),
            owner_account_id: account.account_id.clone(),
            display_name: "Workspace".to_string(),
            state: "active".to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        store.upsert_workspace(&workspace).await.unwrap();
        assert_eq!(
            store.get_workspace("workspace").await.unwrap(),
            Some(workspace)
        );

        let passkey = PasskeyCredentialRecord {
            credential_id: "cred-1".to_string(),
            user_id: user.user_id.clone(),
            public_key_cose: "public-key".to_string(),
            transports_json: Some("[\\\"internal\\\"]".to_string()),
            sign_count: 0,
            created_at: now.clone(),
            last_used_at: None,
        };
        store.upsert_passkey_credential(&passkey).unwrap();
        assert_eq!(
            store.get_passkey_credential("cred-1").unwrap(),
            Some(passkey.clone())
        );
        assert_eq!(
            store
                .list_passkey_credentials_for_user(&user.user_id)
                .unwrap(),
            vec![passkey]
        );

        let challenge = AuthChallengeRecord {
            challenge_id: "challenge-id".to_string(),
            ceremony: "passkey_login".to_string(),
            challenge: "challenge".to_string(),
            user_id: Some(user.user_id.clone()),
            rp_id: "127.0.0.1".to_string(),
            origin: "http://127.0.0.1:8787".to_string(),
            state_json: None,
            expires_at: "2026-07-22T00:05:00Z".to_string(),
            created_at: now.clone(),
            consumed_at: None,
        };
        store.put_auth_challenge(&challenge).unwrap();
        assert_eq!(
            store
                .consume_auth_challenge("challenge", "passkey_login", "2026-07-22T00:01:00Z")
                .unwrap(),
            Some(challenge)
        );
        assert_eq!(
            store
                .consume_auth_challenge("challenge", "passkey_login", "2026-07-22T00:01:00Z")
                .unwrap(),
            None
        );

        let browser_session = BrowserSessionRecord {
            session_id: "session-id".to_string(),
            token_hash: "session-hash".to_string(),
            user_id: user.user_id.clone(),
            created_at: now.clone(),
            expires_at: "2026-07-22T12:00:00Z".to_string(),
            revoked_at: None,
        };
        store.create_browser_session(&browser_session).unwrap();
        assert_eq!(
            store.resolve_browser_session("session-hash").unwrap(),
            Some(browser_session)
        );
        assert!(
            store
                .revoke_browser_session("session-hash", "2026-07-22T00:02:00Z")
                .unwrap()
        );
        assert_eq!(store.resolve_browser_session("session-hash").unwrap(), None);
        assert!(
            !store
                .revoke_browser_session("session-hash", "2026-07-22T00:03:00Z")
                .unwrap()
        );

        let api_token = ApiTokenRecord {
            token_id: "token-id".to_string(),
            token_hash: "hash".to_string(),
            user_id: user.user_id.clone(),
            label: "cli".to_string(),
            created_at: now.clone(),
            expires_at: None,
            revoked_at: None,
            last_used_at: None,
        };
        store.create_api_token(&api_token).unwrap();
        assert_eq!(
            store.resolve_api_token("hash").unwrap(),
            Some(api_token.clone())
        );
        store
            .mark_api_token_used("hash", "2026-07-22T00:02:00Z")
            .unwrap();
        assert_eq!(
            store
                .resolve_api_token("hash")
                .unwrap()
                .unwrap()
                .last_used_at,
            Some("2026-07-22T00:02:00Z".to_string())
        );

        let flow = DeviceLoginFlowRecord {
            device_code: "device".to_string(),
            user_code: "USER-CODE".to_string(),
            verification_uri: "http://127.0.0.1:8787/login/device".to_string(),
            client_name: Some("yoi".to_string()),
            user_id: None,
            api_token_id: None,
            issued_access_token: None,
            created_at: now.clone(),
            expires_at: "2026-07-22T00:10:00Z".to_string(),
            approved_at: None,
            consumed_at: None,
        };
        assert!(store.try_create_device_login_flow(&flow).unwrap());
        let conflicting_flow = DeviceLoginFlowRecord {
            device_code: "device-2".into(),
            user_code: flow.user_code.clone(),
            verification_uri: flow.verification_uri.clone(),
            client_name: Some("other-cli".into()),
            user_id: None,
            api_token_id: None,
            issued_access_token: None,
            created_at: "2026-08-22T00:00:01Z".into(),
            expires_at: "2026-08-22T00:10:01Z".into(),
            approved_at: None,
            consumed_at: None,
        };
        assert!(
            !store
                .try_create_device_login_flow(&conflicting_flow)
                .expect("user code collision")
        );
        assert!(
            store
                .get_device_login_flow_by_device_code("device-2")
                .expect("read conflicting flow")
                .is_none()
        );
        let fresh_flow = DeviceLoginFlowRecord {
            device_code: "device-3".into(),
            user_code: "DCBA-5678".into(),
            verification_uri: flow.verification_uri.clone(),
            client_name: Some("retry-cli".into()),
            user_id: None,
            api_token_id: None,
            issued_access_token: None,
            created_at: "2026-08-22T00:00:02Z".into(),
            expires_at: "2026-08-22T00:10:02Z".into(),
            approved_at: None,
            consumed_at: None,
        };
        assert!(
            store
                .try_create_device_login_flow(&fresh_flow)
                .expect("fresh user code")
        );
        let device_code_conflict = DeviceLoginFlowRecord {
            device_code: flow.device_code.clone(),
            user_code: "ABCD-9999".into(),
            verification_uri: flow.verification_uri.clone(),
            client_name: Some("invalid-cli".into()),
            user_id: None,
            api_token_id: None,
            issued_access_token: None,
            created_at: "2026-08-22T00:00:03Z".into(),
            expires_at: "2026-08-22T00:10:03Z".into(),
            approved_at: None,
            consumed_at: None,
        };
        assert!(
            store
                .try_create_device_login_flow(&device_code_conflict)
                .is_err()
        );
        store
            .approve_device_login_flow(
                "device",
                &user.user_id,
                "token-id",
                "access",
                "2026-07-22T00:03:00Z",
            )
            .unwrap();
        let approved = store
            .consume_device_login_token("device", "2026-07-22T00:04:00Z")
            .unwrap()
            .unwrap();
        assert_eq!(approved.issued_access_token, Some("access".to_string()));
        assert_eq!(
            store
                .consume_device_login_token("device", "2026-07-22T00:05:00Z")
                .unwrap(),
            None
        );
    }
}
