use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use flow::{CompiledFlowDefinition, FlowSourceKind, compile_flow_source};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use worker_runtime::identity::{RuntimeWorkerRef, WorkerId};

use crate::{Error, Result};

const WORKSPACES_V0_COLUMNS: &[&str] = &[
    "workspace_id",
    "display_name",
    "state",
    "created_at",
    "updated_at",
];

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "workspace db canonical schema v0 bootstrap",
        apply: create_schema_v0_tables,
    },
    Migration {
        version: 2,
        name: "align legacy workspace bootstrap with schema v0",
        apply: align_legacy_bootstrap_schema,
    },
    Migration {
        version: 3,
        name: "backend worker workdir registry schema",
        apply: create_worker_workdir_registry_tables,
    },
    Migration {
        version: 4,
        name: "remove durable worker lifecycle state",
        apply: remove_worker_registry_legacy_live_state_column,
    },
    Migration {
        version: 5,
        name: "use composite worker registry keys",
        apply: use_composite_worker_registry_keys,
    },
    Migration {
        version: 6,
        name: "add workdir runtime observation states",
        apply: add_workdir_runtime_observation_states,
    },
    Migration {
        version: 7,
        name: "remove workdir registry management kind",
        apply: remove_workdir_registry_management_kind_column,
    },
    Migration {
        version: 8,
        name: "account identity and login flow schema",
        apply: create_account_identity_tables,
    },
    Migration {
        version: 9,
        name: "webauthn challenge state",
        apply: add_webauthn_challenge_state,
    },
    Migration {
        version: 10,
        name: "sqlite objective authority and memory staging import",
        apply: create_objective_sqlite_authority_tables,
    },
    Migration {
        version: 11,
        name: "sqlite memory authority documents and staging resolutions",
        apply: create_memory_authority_tables,
    },
    Migration {
        version: 12,
        name: "trusted remote runtime registry",
        apply: create_trusted_runtime_registry_tables,
    },
    Migration {
        version: 13,
        name: "objective mutation audit events",
        apply: create_objective_event_tables,
    },
    Migration {
        version: 14,
        name: "remove unused control-plane Ticket tables",
        apply: remove_unused_control_plane_ticket_tables,
    },
    Migration {
        version: 15,
        name: "separate workdir creation evidence from current revision observation",
        apply: add_workdir_revision_observations,
    },
    Migration {
        version: 16,
        name: "ticket worker current assignment authority",
        apply: create_ticket_worker_assignment_tables,
    },
    Migration {
        version: 17,
        name: "worker workspace credentials and Ticket notification outbox",
        apply: create_ticket_notification_tables,
    },
    Migration {
        version: 18,
        name: "bidirectional idempotent Ticket Worker assignments",
        apply: strengthen_ticket_worker_assignments,
    },
    Migration {
        version: 19,
        name: "atomic Ticket notification identity credentials and cursors",
        apply: strengthen_ticket_notifications,
    },
    Migration {
        version: 20,
        name: "reconcile Workdir revision and crash safe Worker lifecycle reservations",
        apply: strengthen_ticket_assignment_lifecycle_reservations,
    },
    Migration {
        version: 21,
        name: "remove per-Worker Workspace credentials",
        apply: remove_worker_workspace_credentials,
    },
    Migration {
        version: 22,
        name: "drop Ticket notification outbox",
        apply: drop_ticket_notification_tables,
    },
    Migration {
        version: 23,
        name: "enforce exclusive active Worker Workdir attachments",
        apply: enforce_exclusive_worker_workdir_attachments,
    },
    Migration {
        version: 24,
        name: "create Worker Workdir attachment reservations",
        apply: create_worker_workdir_attachment_reservations,
    },
    Migration {
        version: 25,
        name: "create Flow source authority",
        apply: create_flow_source_authority,
    },
    Migration {
        version: 26,
        name: "remove Backend-owned Flow runtime authority",
        apply: remove_backend_flow_runtime_authority,
    },
    Migration {
        version: 27,
        name: "scope Repository identity and references by Workspace",
        apply: scope_repository_identity_by_workspace,
    },
    Migration {
        version: 28,
        name: "create Worker retention authority",
        apply: crate::retention::create_worker_retention_tables,
    },
    Migration {
        version: 29,
        name: "create Worker mutation source proof replay guard",
        apply: create_worker_mutation_source_proof_replay_guard,
    },
    Migration {
        version: 30,
        name: "create Workspace virtual config source authority",
        apply: create_workspace_config_source_authority,
    },
    Migration {
        version: 31,
        name: "materialize required main.dcdl Workspace config entrypoint",
        apply: materialize_main_config_entrypoint,
    },
    Migration {
        version: 32,
        name: "persist Workspace config schema contribution bundles",
        apply: persist_workspace_config_schema_bundles,
    },
    Migration {
        version: 33,
        name: "create durable Runtime Worker control grants",
        apply: create_worker_control_grant_authority,
    },
    Migration {
        version: 34,
        name: "create Worker control delegation operation authority",
        apply: create_worker_control_delegation_operation_authority,
    },
    Migration {
        version: 35,
        name: "remove Worker control delegation authority",
        apply: remove_worker_control_delegation_authority,
    },
    Migration {
        version: 36,
        name: "add Objective query indexes",
        apply: add_objective_query_indexes,
    },
    Migration {
        version: 37,
        name: "promote Workspace Worker UUIDv7 identity",
        apply: promote_workspace_worker_uuid_identity,
    },
];

struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    /// Account/namespace owner abstraction. `None` is allowed for legacy/local
    /// workspaces until a user account is bootstrapped.
    pub owner_account_id: Option<String>,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryRecord {
    pub workspace_id: String,
    pub repository_id: String,
    pub name: String,
    pub kind: String,
    pub provider: Option<String>,
    pub uri: String,
    pub default_ref: Option<String>,
    pub auth_ref_kind: Option<String>,
    pub auth_ref_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedRuntimeRecord {
    pub runtime_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketWorkerAssignmentRecord {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment_id: String,
    pub worker: RuntimeWorkerRef,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketWorkerAssignmentEventRecord {
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
    pub current: TicketWorkerAssignmentRecord,
    pub previous: Option<TicketWorkerAssignmentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRegistryRecord {
    pub workspace_id: String,
    pub workdir_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub creation_selector: Option<String>,
    pub creation_ref: Option<String>,
    pub current_selector: Option<String>,
    pub current_ref: Option<String>,
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

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;
    async fn get_trusted_runtime(&self, runtime_id: &str) -> Result<Option<TrustedRuntimeRecord>>;
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
        expected_worker_revision: &str,
        reason: &str,
    ) -> std::result::Result<
        Option<crate::retention::PreparedWorkerRemoval>,
        crate::retention::WorkerRetentionError,
    > {
        let _ = (workspace_id, worker, expected_worker_revision, reason);
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
    fn get_repository(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<Option<RepositoryRecord>>;
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
    fn create_device_login_flow(&self, record: &DeviceLoginFlowRecord) -> Result<()>;
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
    fn get_current_ticket_worker_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<TicketWorkerAssignmentRecord>>;
    fn set_current_ticket_worker_assignment(
        &self,
        record: &TicketWorkerAssignmentRecord,
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
    ) -> Result<Option<TicketWorkerAssignmentRecord>>;
    fn list_ticket_worker_assignment_events(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<TicketWorkerAssignmentEventRecord>>;

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
        apply_migrations(&conn)?;
        ticket::migrate_sqlite_ticket_schema(&conn)?;
        merge_request::migrate(&conn).map_err(|error| Error::Store(error.to_string()))?;
        validate_workspace_repository_references(&conn)?;
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

    pub(crate) fn reserve_worker_create(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        allocation_key: &str,
        create_fingerprint: &str,
    ) -> Result<WorkerId> {
        if allocation_key.trim().is_empty() || create_fingerprint.trim().is_empty() {
            return Err(Error::InvalidInput(
                "Worker create allocation key and fingerprint must be non-empty".to_string(),
            ));
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing = tx
                .query_row(
                    "SELECT worker_id, runtime_id, create_fingerprint \
                     FROM worker_create_reservations \
                     WHERE workspace_id = ?1 AND allocation_key = ?2",
                    params![workspace_id, allocation_key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((worker_id, reserved_runtime_id, reserved_fingerprint)) = existing {
                if reserved_runtime_id != runtime_id || reserved_fingerprint != create_fingerprint {
                    return Err(Error::InvalidInput(format!(
                        "Worker create allocation `{allocation_key}` was already used with different input"
                    )));
                }
                return worker_id.parse::<WorkerId>().map_err(|_| {
                    Error::Store(format!(
                        "Worker create allocation `{allocation_key}` has a non-UUIDv7 worker id"
                    ))
                });
            }

            let worker_id = WorkerId::now_v7();
            let now = chrono::Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO worker_create_reservations(\
                    workspace_id, allocation_key, worker_id, runtime_id, create_fingerprint,\
                    state, created_at, updated_at\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'reserved', ?6, ?6)",
                params![
                    workspace_id,
                    allocation_key,
                    worker_id.to_string(),
                    runtime_id,
                    create_fingerprint,
                    now
                ],
            )?;
            tx.commit()?;
            Ok(worker_id)
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
                    runtime_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(runtime_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    base_url = excluded.base_url,
                    public_key = excluded.public_key,
                    updated_at = excluded.updated_at,
                    revoked_at = excluded.revoked_at"#,
                params![
                    record.runtime_id,
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
                r#"SELECT runtime_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                   FROM trusted_runtime_records ORDER BY runtime_id ASC"#
            } else {
                r#"SELECT runtime_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
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

    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workspaces (
                    workspace_id, owner_account_id, display_name, state, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    owner_account_id = COALESCE(excluded.owner_account_id, workspaces.owner_account_id),
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

    async fn get_trusted_runtime(&self, runtime_id: &str) -> Result<Option<TrustedRuntimeRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT runtime_id, display_name, base_url, public_key, created_at, updated_at, revoked_at
                   FROM trusted_runtime_records WHERE runtime_id = ?1"#,
                params![runtime_id],
                read_trusted_runtime_record,
            )
            .optional()
            .map_err(Error::from)
        })
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
        expected_worker_revision: &str,
        reason: &str,
    ) -> std::result::Result<
        Option<crate::retention::PreparedWorkerRemoval>,
        crate::retention::WorkerRetentionError,
    > {
        SqliteWorkspaceStore::recover_worker_removal_execution(
            self,
            workspace_id,
            worker,
            expected_worker_revision,
            reason,
        )
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
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO repositories (
                    workspace_id, repository_id, name, kind, provider, uri, default_ref,
                    auth_ref_kind, auth_ref_key, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(workspace_id, repository_id) DO UPDATE SET
                    name = excluded.name,
                    kind = excluded.kind,
                    provider = excluded.provider,
                    uri = excluded.uri,
                    default_ref = excluded.default_ref,
                    auth_ref_kind = excluded.auth_ref_kind,
                    auth_ref_key = excluded.auth_ref_key,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.repository_id,
                    record.name,
                    record.kind,
                    record.provider,
                    record.uri,
                    record.default_ref,
                    record.auth_ref_kind,
                    record.auth_ref_key,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn get_repository(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<Option<RepositoryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, repository_id, name, kind, provider, uri, default_ref,
                          auth_ref_kind, auth_ref_key, created_at, updated_at
                   FROM repositories
                   WHERE workspace_id = ?1 AND repository_id = ?2"#,
                params![workspace_id, repository_id],
                read_repository_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_repositories(&self, workspace_id: &str) -> Result<Vec<RepositoryRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, repository_id, name, kind, provider, uri, default_ref,
                          auth_ref_kind, auth_ref_key, created_at, updated_at
                   FROM repositories
                   WHERE workspace_id = ?1
                   ORDER BY repository_id ASC"#,
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
            conn.execute(
                r#"INSERT INTO objectives (
                    workspace_id, objective_id, title, state, body_md, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(objective_id) DO UPDATE SET
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
                    ON CONFLICT(objective_id, ticket_id, kind) DO UPDATE SET
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
                ON CONFLICT(objective_id, resource_path) DO UPDATE SET
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

    fn create_device_login_flow(&self, record: &DeviceLoginFlowRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO device_login_flows (device_code, user_code, verification_uri, client_name, user_id, api_token_id, issued_access_token, created_at, expires_at, approved_at, consumed_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
                params![record.device_code, record.user_code, record.verification_uri, record.client_name, record.user_id, record.api_token_id, record.issued_access_token, record.created_at, record.expires_at, record.approved_at, record.consumed_at],
            )?;
            Ok(())
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
            let removal_blocks_upsert: bool = conn.query_row(
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
            conn.execute(
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

    fn get_current_ticket_worker_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<TicketWorkerAssignmentRecord>> {
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

    fn set_current_ticket_worker_assignment(
        &self,
        record: &TicketWorkerAssignmentRecord,
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
                       WHERE workspace_id = ?1 AND ticket_id = ?2"#,
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
    ) -> Result<Option<TicketWorkerAssignmentRecord>> {
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
                "DELETE FROM ticket_current_worker_assignments WHERE workspace_id = ?1 AND ticket_id = ?2",
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

    fn list_ticket_worker_assignment_events(
        &self,
        workspace_id: &str,
        ticket_id: &str,
        limit: usize,
    ) -> Result<Vec<TicketWorkerAssignmentEventRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, ticket_id, event_id, action, assignment_id,
                          previous_assignment_id, actor, created_at
                   FROM ticket_worker_assignment_events
                   WHERE workspace_id = ?1 AND ticket_id = ?2
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
                    creation_selector, creation_ref, current_selector, current_ref,
                    materialization_status, cleanliness, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(workspace_id, workdir_id) DO UPDATE SET
                    runtime_id = excluded.runtime_id,
                    repository_id = excluded.repository_id,
                    creation_selector = excluded.creation_selector,
                    creation_ref = excluded.creation_ref,
                    current_selector = excluded.current_selector,
                    current_ref = excluded.current_ref,
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
                    record.current_selector,
                    record.current_ref,
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

fn read_repository_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositoryRecord> {
    Ok(RepositoryRecord {
        workspace_id: row.get(0)?,
        repository_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        provider: row.get(4)?,
        uri: row.get(5)?,
        default_ref: row.get(6)?,
        auth_ref_kind: row.get(7)?,
        auth_ref_key: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
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
        display_name: row.get(1)?,
        base_url: row.get(2)?,
        public_key: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        revoked_at: row.get(6)?,
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

fn current_ticket_worker_assignment_select_sql() -> String {
    "SELECT a.workspace_id, a.ticket_id, a.assignment_id, a.runtime_id, a.worker_id, \
            a.assigned_by, a.assigned_at \
     FROM ticket_current_worker_assignments AS current \
     JOIN ticket_worker_assignments AS a \
       ON a.workspace_id = current.workspace_id \
      AND a.ticket_id = current.ticket_id \
      AND a.assignment_id = current.assignment_id \
     WHERE current.workspace_id = ?1 AND current.ticket_id = ?2"
        .to_owned()
}

fn read_ticket_worker_assignment_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TicketWorkerAssignmentRecord> {
    Ok(TicketWorkerAssignmentRecord {
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
) -> rusqlite::Result<TicketWorkerAssignmentEventRecord> {
    Ok(TicketWorkerAssignmentEventRecord {
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
    current: Option<&TicketWorkerAssignmentRecord>,
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
         creation_selector, creation_ref, current_selector, current_ref, \
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
        current_selector: row.get(6)?,
        current_ref: row.get(7)?,
        materialization_status: row.get(8)?,
        cleanliness: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn create_worker_workdir_registry_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS worker_registry (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    runtime_worker_id INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    profile TEXT,
    retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
    transcript_ref TEXT,
    session_ref TEXT,
    summary_ref TEXT,
    diagnostics_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS workdir_registry (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    selector TEXT,
    resolved_commit TEXT,
    materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
    cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS worker_workdir_links (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    runtime_worker_id INTEGER NOT NULL,
    workdir_id TEXT NOT NULL,
    role TEXT NOT NULL,
    linked_at TEXT NOT NULL,
    unlinked_at TEXT,
    PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id, workdir_id, role),
    FOREIGN KEY (workspace_id, runtime_id, runtime_worker_id) REFERENCES worker_registry(workspace_id, runtime_id, runtime_worker_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, workdir_id) REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
    ON worker_registry(workspace_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
    ON workdir_registry(workspace_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_worker_workdir_links_worker
    ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id, linked_at DESC);
"#,
    )?;
    Ok(())
}

fn add_webauthn_challenge_state(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "auth_challenges", "state_json")? {
        conn.execute_batch("ALTER TABLE auth_challenges ADD COLUMN state_json TEXT;")?;
    }
    Ok(())
}

fn create_objective_sqlite_authority_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
PRAGMA foreign_keys = OFF;
ALTER TABLE objective_ticket_links RENAME TO objective_ticket_links_old;
CREATE TABLE objective_ticket_links (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (objective_id, ticket_id, kind),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (objective_id) REFERENCES objectives(objective_id) ON DELETE CASCADE
);
INSERT OR IGNORE INTO objective_ticket_links (workspace_id, objective_id, ticket_id, kind, created_at)
    SELECT workspace_id, objective_id, ticket_id, kind, created_at FROM objective_ticket_links_old;
DROP TABLE objective_ticket_links_old;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS objective_resources (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    resource_path TEXT NOT NULL,
    body TEXT NOT NULL,
    media_type TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (objective_id, resource_path),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (objective_id) REFERENCES objectives(objective_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS memory_staging_records (
    workspace_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
"#,
    )?;
    Ok(())
}

fn create_memory_authority_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS workspace_memory_documents (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_staging_resolutions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id, resolved_at)
);
"#,
    )?;
    Ok(())
}

fn remove_unused_control_plane_ticket_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
DROP TABLE IF EXISTS ticket_target_paths;
DROP TABLE IF EXISTS ticket_worker_links;
DROP TABLE IF EXISTS ticket_targets;
DROP TABLE IF EXISTS ticket_relations;
DROP TABLE IF EXISTS ticket_events;
DROP TABLE IF EXISTS tickets;
"#,
    )?;
    Ok(())
}

fn add_workdir_revision_observations(conn: &Connection) -> Result<()> {
    if column_exists(conn, "workdir_registry", "selector")?
        && !column_exists(conn, "workdir_registry", "creation_selector")?
    {
        conn.execute_batch(
            "ALTER TABLE workdir_registry RENAME COLUMN selector TO creation_selector;",
        )?;
    }
    if column_exists(conn, "workdir_registry", "resolved_commit")?
        && !column_exists(conn, "workdir_registry", "creation_ref")?
    {
        conn.execute_batch(
            "ALTER TABLE workdir_registry RENAME COLUMN resolved_commit TO creation_ref;",
        )?;
    }
    if !column_exists(conn, "workdir_registry", "current_selector")? {
        conn.execute_batch("ALTER TABLE workdir_registry ADD COLUMN current_selector TEXT;")?;
    }
    if !column_exists(conn, "workdir_registry", "current_ref")? {
        conn.execute_batch("ALTER TABLE workdir_registry ADD COLUMN current_ref TEXT;")?;
    }
    Ok(())
}

fn create_ticket_worker_assignment_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS ticket_worker_assignments (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    assigned_by TEXT NOT NULL,
    assigned_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, assignment_id),
    UNIQUE (workspace_id, ticket_id, assignment_id)
);

CREATE TABLE IF NOT EXISTS ticket_current_worker_assignments (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id, assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id, ticket_id, assignment_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ticket_worker_assignment_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('assigned', 'reassigned', 'unassigned')),
    assignment_id TEXT,
    previous_assignment_id TEXT,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_ticket_assignments_worker
    ON ticket_worker_assignments(workspace_id, runtime_id, worker_id, assigned_at DESC);
CREATE INDEX IF NOT EXISTS idx_ticket_assignment_events_ticket
    ON ticket_worker_assignment_events(workspace_id, ticket_id, created_at DESC);
"#,
    )?;
    Ok(())
}

fn create_ticket_notification_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS worker_workspace_credentials (
    credential_id TEXT PRIMARY KEY,
    token TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    runtime_id TEXT NOT NULL,
    worker_id TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ticket_notification_outbox (
    notification_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    source_runtime_id TEXT NOT NULL,
    source_worker_id TEXT NOT NULL,
    previous_state TEXT NOT NULL,
    current_state TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ticket_notification_deliveries (
    notification_id TEXT NOT NULL REFERENCES ticket_notification_outbox(notification_id) ON DELETE CASCADE,
    recipient_runtime_id TEXT NOT NULL,
    recipient_worker_id TEXT NOT NULL,
    recipient_kind TEXT NOT NULL CHECK (recipient_kind IN ('assigned', 'orchestrator')),
    attempts INTEGER NOT NULL DEFAULT 0,
    delivered_at TEXT,
    last_error TEXT,
    PRIMARY KEY (notification_id, recipient_runtime_id, recipient_worker_id)
);

CREATE INDEX IF NOT EXISTS idx_ticket_notification_pending
    ON ticket_notification_deliveries(delivered_at, attempts);
"#,
    )?;
    Ok(())
}

fn strengthen_ticket_worker_assignments(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
ALTER TABLE ticket_current_worker_assignments RENAME TO ticket_current_worker_assignments_v16;

CREATE TABLE ticket_current_worker_assignments (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    UNIQUE (workspace_id, runtime_id, worker_id),
    FOREIGN KEY (workspace_id, ticket_id, assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id, ticket_id, assignment_id)
        ON DELETE CASCADE
);

INSERT INTO ticket_current_worker_assignments (
    workspace_id, ticket_id, assignment_id, runtime_id, worker_id, updated_at
)
SELECT current.workspace_id, current.ticket_id, current.assignment_id,
       assignment.runtime_id, assignment.worker_id, current.updated_at
FROM ticket_current_worker_assignments_v16 AS current
JOIN ticket_worker_assignments AS assignment
  ON assignment.workspace_id = current.workspace_id
 AND assignment.ticket_id = current.ticket_id
 AND assignment.assignment_id = current.assignment_id;

DROP TABLE ticket_current_worker_assignments_v16;

CREATE TABLE ticket_assignment_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('assign', 'reassign', 'unassign')),
    ticket_id TEXT NOT NULL,
    runtime_id TEXT,
    worker_id TEXT,
    assignment_id TEXT,
    expected_assignment_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_id)
);
"#,
    )?;
    Ok(())
}

fn strengthen_ticket_notifications(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
ALTER TABLE worker_workspace_credentials ADD COLUMN expires_at TEXT;
ALTER TABLE worker_workspace_credentials ADD COLUMN revoked_at TEXT;
ALTER TABLE ticket_notification_outbox ADD COLUMN event_kind TEXT NOT NULL DEFAULT 'comment';
ALTER TABLE ticket_notification_outbox ADD COLUMN source_operation_kind TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE ticket_notification_outbox ADD COLUMN source_actor_role TEXT NOT NULL DEFAULT 'worker';
ALTER TABLE ticket_notification_outbox ADD COLUMN source_assignment_id TEXT;

CREATE TABLE ticket_notification_cursors (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    last_event_index INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, runtime_id, worker_id)
);
"#,
    )?;
    Ok(())
}

fn strengthen_ticket_assignment_lifecycle_reservations(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "ticket_assignment_operations", "request_fingerprint")? {
        conn.execute_batch(
            "ALTER TABLE ticket_assignment_operations ADD COLUMN request_fingerprint TEXT;",
        )?;
    }
    // The Workdir and Ticket branches both used schema version 15 before
    // integration. A database that ran the Ticket branch through v19 still
    // needs the Workdir revision projection while already having the lifecycle
    // reservation column. Reconcile both shapes at the combined v20 boundary.
    add_workdir_revision_observations(conn)?;
    Ok(())
}

fn remove_worker_workspace_credentials(conn: &Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS worker_workspace_credentials;")?;
    Ok(())
}

fn drop_ticket_notification_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
DROP TABLE IF EXISTS ticket_notification_cursors;
DROP TABLE IF EXISTS ticket_notification_deliveries;
DROP TABLE IF EXISTS ticket_notification_outbox;
"#,
    )?;
    Ok(())
}

fn enforce_exclusive_worker_workdir_attachments(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE UNIQUE INDEX IF NOT EXISTS ux_worker_workdir_links_active_worker
    ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id)
    WHERE unlinked_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ux_worker_workdir_links_active_workdir
    ON worker_workdir_links(workspace_id, workdir_id)
    WHERE unlinked_at IS NULL;
"#,
    )?;
    Ok(())
}

fn create_worker_workdir_attachment_reservations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS worker_workdir_attachment_reservations (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    reserved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_worker_workdir_attachment_reservation_id
    ON worker_workdir_attachment_reservations(workspace_id, reservation_id);
"#,
    )?;
    Ok(())
}

fn create_objective_event_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS objective_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL REFERENCES objectives(objective_id) ON DELETE CASCADE,
    event_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    body_md TEXT,
    created_at TEXT NOT NULL
);
"#,
    )?;
    Ok(())
}

fn create_trusted_runtime_registry_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS trusted_runtime_records (
    runtime_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    public_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revoked_at TEXT
);
"#,
    )?;
    Ok(())
}

fn create_account_identity_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS accounts (
    account_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('user', 'organization')),
    handle TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (kind, handle)
);

CREATE TABLE IF NOT EXISTS users (
    user_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id) ON DELETE CASCADE,
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS passkey_credentials (
    credential_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    public_key_cose TEXT NOT NULL,
    transports_json TEXT,
    sign_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);

CREATE TABLE IF NOT EXISTS auth_challenges (
    challenge_id TEXT PRIMARY KEY,
    ceremony TEXT NOT NULL CHECK (ceremony IN ('passkey_registration', 'passkey_login')),
    challenge TEXT NOT NULL UNIQUE,
    user_id TEXT REFERENCES users(user_id) ON DELETE CASCADE,
    rp_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    state_json TEXT,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    consumed_at TEXT
);

CREATE TABLE IF NOT EXISTS browser_sessions (
    session_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS api_tokens (
    token_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    revoked_at TEXT,
    last_used_at TEXT
);

CREATE TABLE IF NOT EXISTS device_login_flows (
    device_code TEXT PRIMARY KEY,
    user_code TEXT NOT NULL UNIQUE,
    verification_uri TEXT NOT NULL,
    client_name TEXT,
    user_id TEXT REFERENCES users(user_id) ON DELETE SET NULL,
    api_token_id TEXT REFERENCES api_tokens(token_id) ON DELETE SET NULL,
    issued_access_token TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    approved_at TEXT,
    consumed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_browser_sessions_token_hash ON browser_sessions(token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_token_hash ON api_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_device_login_user_code ON device_login_flows(user_code);
"#,
    )?;
    if !column_exists(conn, "workspaces", "owner_account_id")? {
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN owner_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL;",
        )?;
    }
    Ok(())
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

fn create_flow_source_authority(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE flow_sources (
    workspace_id TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('builtin', 'workspace')),
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, flow_id),
    UNIQUE (workspace_id, source_kind, name),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE flow_source_revisions (
    workspace_id TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    content TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    definition_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, flow_id, revision),
    FOREIGN KEY (workspace_id, flow_id)
        REFERENCES flow_sources(workspace_id, flow_id) ON DELETE CASCADE
);
"#,
    )?;
    Ok(())
}

fn remove_backend_flow_runtime_authority(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
DROP TABLE IF EXISTS flow_events;
DROP TABLE IF EXISTS flow_transition_attempts;
DROP TABLE IF EXISTS flow_instances;
"#,
    )?;
    Ok(())
}

fn scope_repository_identity_by_workspace(conn: &Connection) -> Result<()> {
    validate_workspace_repository_references(conn)?;
    conn.execute_batch(
        r#"
CREATE TABLE repositories_v27 (
    workspace_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT,
    uri TEXT NOT NULL,
    default_ref TEXT,
    auth_ref_kind TEXT,
    auth_ref_key TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, repository_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
INSERT INTO repositories_v27 (
    workspace_id, repository_id, name, kind, provider, uri, default_ref,
    auth_ref_kind, auth_ref_key, created_at, updated_at
)
SELECT workspace_id, repository_id, name, kind, provider, uri, default_ref,
       auth_ref_kind, auth_ref_key, created_at, updated_at
FROM repositories;

CREATE TABLE artifacts_v27 (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    artifact_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    media_type TEXT,
    sha256 TEXT,
    size_bytes INTEGER,
    summary TEXT,
    created_at TEXT NOT NULL,
    created_by_kind TEXT NOT NULL,
    created_by_key TEXT NOT NULL,
    created_by_display TEXT NOT NULL,
    created_by_source_kind TEXT,
    created_by_source_key TEXT,
    ticket_id TEXT,
    objective_id TEXT,
    event_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    repository_id TEXT,
    source_kind TEXT,
    source_revision TEXT,
    FOREIGN KEY (workspace_id, repository_id)
        REFERENCES repositories_v27(workspace_id, repository_id)
);
INSERT INTO artifacts_v27 (
    workspace_id, artifact_id, kind, uri, media_type, sha256, size_bytes, summary,
    created_at, created_by_kind, created_by_key, created_by_display,
    created_by_source_kind, created_by_source_key, ticket_id, objective_id, event_id,
    worker_ref_kind, worker_ref_key, worker_display, repository_id, source_kind,
    source_revision
)
SELECT workspace_id, artifact_id, kind, uri, media_type, sha256, size_bytes, summary,
       created_at, created_by_kind, created_by_key, created_by_display,
       created_by_source_kind, created_by_source_key, ticket_id, objective_id, event_id,
       worker_ref_kind, worker_ref_key, worker_display, repository_id, source_kind,
       source_revision
FROM artifacts;

CREATE TABLE workdir_registry_v27 (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    creation_selector TEXT,
    creation_ref TEXT,
    materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
    cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    current_selector TEXT,
    current_ref TEXT,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, repository_id)
        REFERENCES repositories_v27(workspace_id, repository_id)
);
INSERT INTO workdir_registry_v27 (
    workspace_id, workdir_id, runtime_id, repository_id,
    creation_selector, creation_ref, current_selector, current_ref,
    materialization_status, cleanliness, created_at, updated_at
)
SELECT workspace_id, workdir_id, runtime_id, repository_id,
       creation_selector, creation_ref, current_selector, current_ref,
       materialization_status, cleanliness, created_at, updated_at
FROM workdir_registry;

CREATE TABLE worker_workdir_links_v27 (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    runtime_worker_id INTEGER NOT NULL,
    workdir_id TEXT NOT NULL,
    role TEXT NOT NULL,
    linked_at TEXT NOT NULL,
    unlinked_at TEXT,
    PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id, workdir_id, role),
    FOREIGN KEY (workspace_id, runtime_id, runtime_worker_id)
        REFERENCES worker_registry(workspace_id, runtime_id, runtime_worker_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, workdir_id)
        REFERENCES workdir_registry_v27(workspace_id, workdir_id) ON DELETE CASCADE
);
INSERT INTO worker_workdir_links_v27 (
    workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
)
SELECT workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
FROM worker_workdir_links;

CREATE TABLE worker_workdir_attachment_reservations_v27 (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    reserved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id, workdir_id)
        REFERENCES workdir_registry_v27(workspace_id, workdir_id) ON DELETE CASCADE
);
INSERT INTO worker_workdir_attachment_reservations_v27 (
    workspace_id, workdir_id, reservation_id, reserved_at
)
SELECT workspace_id, workdir_id, reservation_id, reserved_at
FROM worker_workdir_attachment_reservations;

DROP TABLE worker_workdir_links;
DROP TABLE worker_workdir_attachment_reservations;
DROP TABLE workdir_registry;
DROP TABLE artifacts;
DROP TABLE repositories;
ALTER TABLE repositories_v27 RENAME TO repositories;
ALTER TABLE artifacts_v27 RENAME TO artifacts;
ALTER TABLE workdir_registry_v27 RENAME TO workdir_registry;
ALTER TABLE worker_workdir_links_v27 RENAME TO worker_workdir_links;
ALTER TABLE worker_workdir_attachment_reservations_v27
    RENAME TO worker_workdir_attachment_reservations;

CREATE INDEX idx_workdir_registry_workspace_updated
    ON workdir_registry(workspace_id, updated_at DESC);
CREATE INDEX idx_worker_workdir_links_worker
    ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id, linked_at DESC);
CREATE UNIQUE INDEX ux_worker_workdir_links_active_worker
    ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id)
    WHERE unlinked_at IS NULL;
CREATE UNIQUE INDEX ux_worker_workdir_links_active_workdir
    ON worker_workdir_links(workspace_id, workdir_id)
    WHERE unlinked_at IS NULL;
CREATE UNIQUE INDEX ux_worker_workdir_attachment_reservation_id
    ON worker_workdir_attachment_reservations(workspace_id, reservation_id);
"#,
    )?;
    Ok(())
}

fn validate_workspace_repository_references(conn: &Connection) -> Result<()> {
    for (table, repository_nullable) in [
        ("workdir_registry", false),
        ("artifacts", true),
        // `typed_tickets` is owned and migrated by the Ticket component. The control-plane
        // migration may reject an already-invalid integrated reference, but must not rebuild
        // that component table or claim its schema authority.
        ("typed_tickets", true),
    ] {
        if !table_exists(conn, table)? || !column_exists(conn, table, "repository_id")? {
            continue;
        }
        let null_filter = if repository_nullable {
            "child.repository_id IS NOT NULL AND"
        } else {
            ""
        };
        let sql = format!(
            "SELECT child.workspace_id, child.repository_id FROM {table} AS child \
             WHERE {null_filter} NOT EXISTS (\
                 SELECT 1 FROM repositories AS repository \
                 WHERE repository.workspace_id = child.workspace_id \
                   AND repository.repository_id = child.repository_id\
             ) LIMIT 1"
        );
        let invalid = conn
            .query_row(&sql, [], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?;
        if let Some((workspace_id, repository_id)) = invalid {
            return Err(Error::Store(format!(
                "invalid Workspace-owned repository reference: {table} contains repository `{repository_id}` outside Workspace `{workspace_id}`"
            )));
        }
    }
    Ok(())
}

fn current_schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM __yoi_schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

fn create_workspace_config_source_authority(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS workspace_config_trees (
            workspace_id TEXT PRIMARY KEY,
            revision INTEGER NOT NULL CHECK (revision >= 0),
            tree_digest TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            entrypoints_json TEXT NOT NULL,
            decodal_version TEXT NOT NULL,
            import_policy_version INTEGER NOT NULL,
            toolchain_fingerprint TEXT NOT NULL,
            projection_digest TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS workspace_config_entries (
            workspace_id TEXT NOT NULL,
            path TEXT NOT NULL,
            content_type TEXT NOT NULL,
            content TEXT NOT NULL,
            content_digest TEXT NOT NULL,
            PRIMARY KEY (workspace_id, path),
            FOREIGN KEY (workspace_id) REFERENCES workspace_config_trees(workspace_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_workspace_config_entries_prefix
            ON workspace_config_entries(workspace_id, path);
        CREATE TABLE IF NOT EXISTS workspace_config_tree_revisions (
            workspace_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            tree_digest TEXT NOT NULL,
            toolchain_fingerprint TEXT NOT NULL,
            projection_digest TEXT NOT NULL,
            manifest_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, revision),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        "#,
    )?;
    Ok(())
}

pub(crate) fn persist_workspace_config_schema_bundles(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        ALTER TABLE workspace_config_trees
            ADD COLUMN schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}';
        ALTER TABLE workspace_config_tree_revisions
            ADD COLUMN schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}';
        "#,
    )?;
    let bundle = config_source::WorkspaceConfigSchemaBundle::empty();
    let bundle_json =
        serde_json::to_string(&bundle).map_err(|error| Error::Store(error.to_string()))?;
    let contract = config_source::ToolchainContract::with_schema_bundle(
        config_source::DEFAULT_SCHEMA_VERSION,
        vec![
            config_source::VirtualPath::parse(crate::config_source::MAIN_CONFIG_ENTRYPOINT)
                .map_err(|error| Error::Store(error.to_string()))?,
        ],
        config_source::DEFAULT_IMPORT_POLICY_VERSION,
        bundle,
    );
    conn.execute(
        "UPDATE workspace_config_trees
         SET schema_bundle_json = ?1,
             toolchain_fingerprint = ?2",
        params![bundle_json, contract.fingerprint],
    )?;
    conn.execute(
        "UPDATE workspace_config_tree_revisions
         SET schema_bundle_json = ?1,
             toolchain_fingerprint = ?2",
        params![bundle_json, contract.fingerprint],
    )?;
    Ok(())
}

fn create_worker_control_grant_authority(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE worker_control_grants (
            workspace_id TEXT NOT NULL,
            grant_id TEXT NOT NULL,
            controller_runtime_id TEXT NOT NULL,
            controller_worker_id INTEGER NOT NULL,
            subject_runtime_id TEXT NOT NULL,
            subject_worker_id INTEGER NOT NULL,
            relation TEXT NOT NULL,
            origin TEXT NOT NULL,
            permissions_json TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revoked_at TEXT,
            PRIMARY KEY (workspace_id, grant_id),
            UNIQUE (
                workspace_id,
                controller_runtime_id,
                controller_worker_id,
                operation_id
            ),
            FOREIGN KEY (workspace_id, controller_runtime_id, controller_worker_id)
                REFERENCES worker_registry (workspace_id, runtime_id, runtime_worker_id)
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, subject_runtime_id, subject_worker_id)
                REFERENCES worker_registry (workspace_id, runtime_id, runtime_worker_id)
                ON DELETE CASCADE
        );

        CREATE INDEX idx_worker_control_grants_controller_active
            ON worker_control_grants (
                workspace_id,
                controller_runtime_id,
                controller_worker_id,
                revoked_at,
                created_at
            );

        CREATE INDEX idx_worker_control_grants_subject_active
            ON worker_control_grants (
                workspace_id,
                subject_runtime_id,
                subject_worker_id,
                revoked_at
            );
        "#,
    )?;
    Ok(())
}

fn create_worker_control_delegation_operation_authority(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE worker_control_delegation_operations (
            workspace_id TEXT NOT NULL,
            source_controller_runtime_id TEXT NOT NULL,
            source_controller_worker_id INTEGER NOT NULL,
            source_grant_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            input_fingerprint TEXT NOT NULL,
            delegated_grant_id TEXT,
            created_at TEXT NOT NULL,
            completed_at TEXT,
            PRIMARY KEY (
                workspace_id,
                source_controller_runtime_id,
                source_controller_worker_id,
                operation_id
            ),
            FOREIGN KEY (workspace_id, source_controller_runtime_id, source_controller_worker_id)
                REFERENCES worker_registry (workspace_id, runtime_id, runtime_worker_id)
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, source_grant_id)
                REFERENCES worker_control_grants (workspace_id, grant_id)
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, delegated_grant_id)
                REFERENCES worker_control_grants (workspace_id, grant_id)
                ON DELETE SET NULL
        );
        "#,
    )?;
    Ok(())
}

fn add_objective_query_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS objectives_workspace_state_updated
            ON objectives(workspace_id, state, updated_at DESC, objective_id);
        CREATE INDEX IF NOT EXISTS objectives_workspace_updated
            ON objectives(workspace_id, updated_at DESC, objective_id);
        CREATE INDEX IF NOT EXISTS objectives_workspace_created
            ON objectives(workspace_id, created_at DESC, objective_id);
        CREATE INDEX IF NOT EXISTS objectives_workspace_title
            ON objectives(workspace_id, title COLLATE NOCASE, objective_id);
        CREATE INDEX IF NOT EXISTS objective_ticket_links_workspace_ticket_objective
            ON objective_ticket_links(workspace_id, ticket_id, objective_id);
        "#,
    )?;
    Ok(())
}

fn collect_legacy_text_worker_bindings(
    conn: &Connection,
    table: &str,
    bindings: &mut std::collections::BTreeSet<(String, String, u64)>,
) -> Result<()> {
    if !table_exists(conn, table)? {
        return Ok(());
    }
    let sql = format!(
        "SELECT workspace_id, runtime_id, worker_id FROM {table} WHERE worker_id IS NOT NULL"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (workspace_id, runtime_id, worker_id) = row?;
        let legacy_id = worker_id
            .strip_prefix("worker-")
            .unwrap_or(&worker_id)
            .parse::<u64>()
            .map_err(|_| {
                Error::InvalidInput(format!(
                    "{table} contains non-legacy Worker id `{worker_id}` before schema v37"
                ))
            })?;
        bindings.insert((workspace_id, runtime_id, legacy_id));
    }
    Ok(())
}

fn promote_workspace_worker_uuid_identity(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA defer_foreign_keys = ON;
        CREATE TEMP TABLE worker_identity_v37 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            worker_id TEXT NOT NULL UNIQUE,
            PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id)
        );
        "#,
    )?;

    let mut legacy_workers = std::collections::BTreeSet::new();
    {
        let mut statement = conn.prepare(
            "SELECT workspace_id, runtime_id, runtime_worker_id FROM worker_registry \
             ORDER BY workspace_id, runtime_id, runtime_worker_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (workspace_id, runtime_id, runtime_worker_id) = row?;
            legacy_workers.insert((
                workspace_id,
                runtime_id,
                u64::try_from(runtime_worker_id).map_err(|_| {
                    Error::InvalidInput(format!(
                        "legacy Runtime Worker id {runtime_worker_id} is negative"
                    ))
                })?,
            ));
        }
    }
    // v21/v22 removed the legacy per-Worker credential and Ticket notification
    // tables. Every Worker-reference table that still exists at the v36 boundary
    // participates in this map, including retention-only Workers no longer present
    // in worker_registry.
    for table in [
        "ticket_worker_assignments",
        "ticket_current_worker_assignments",
        "ticket_assignment_operations",
        "worker_removal_operations",
        "worker_session_archives",
        "worker_diagnostics_archives",
        "worker_tombstones",
        "worker_orphan_diagnostics",
    ] {
        collect_legacy_text_worker_bindings(conn, table, &mut legacy_workers)?;
    }

    for (workspace_id, runtime_id, runtime_worker_id) in legacy_workers {
        conn.execute(
            "INSERT INTO worker_identity_v37(\
                workspace_id, runtime_id, runtime_worker_id, worker_id\
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                workspace_id,
                runtime_id,
                runtime_worker_id,
                WorkerId::from_legacy_binding(&workspace_id, &runtime_id, runtime_worker_id)
                    .to_string()
            ],
        )?;
    }

    conn.execute_batch(
        r#"
        CREATE TABLE worker_registry_v37 (
            workspace_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT INTO worker_registry_v37(
            workspace_id, worker_id, runtime_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref,
            diagnostics_ref, created_at, updated_at
        )
        SELECT r.workspace_id, m.worker_id, r.runtime_id, r.display_name, r.profile,
               r.retention_state, r.transcript_ref, r.session_ref, r.summary_ref,
               r.diagnostics_ref, r.created_at, r.updated_at
        FROM worker_registry r
        JOIN worker_identity_v37 m
          ON m.workspace_id = r.workspace_id
         AND m.runtime_id = r.runtime_id
         AND m.runtime_worker_id = r.runtime_worker_id;

        CREATE TABLE worker_workdir_links_v37 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            role TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            unlinked_at TEXT,
            PRIMARY KEY (workspace_id, worker_id, workdir_id, role),
            FOREIGN KEY (workspace_id, worker_id)
                REFERENCES worker_registry_v37(workspace_id, worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id)
                REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
        INSERT INTO worker_workdir_links_v37(
            workspace_id, runtime_id, worker_id, workdir_id, role, linked_at, unlinked_at
        )
        SELECT l.workspace_id, l.runtime_id, m.worker_id, l.workdir_id, l.role, l.linked_at, l.unlinked_at
        FROM worker_workdir_links l
        JOIN worker_identity_v37 m
          ON m.workspace_id = l.workspace_id
         AND m.runtime_id = l.runtime_id
         AND m.runtime_worker_id = l.runtime_worker_id;

        CREATE TABLE worker_control_grants_v37 (
            grant_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            controller_runtime_id TEXT NOT NULL,
            controller_worker_id TEXT NOT NULL,
            subject_runtime_id TEXT NOT NULL,
            subject_worker_id TEXT NOT NULL,
            relation TEXT NOT NULL,
            origin TEXT NOT NULL,
            permissions_json TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revoked_at TEXT,
            PRIMARY KEY (workspace_id, grant_id),
            UNIQUE (workspace_id, controller_worker_id, operation_id),
            FOREIGN KEY (workspace_id, controller_worker_id)
                REFERENCES worker_registry_v37(workspace_id, worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, subject_worker_id)
                REFERENCES worker_registry_v37(workspace_id, worker_id) ON DELETE CASCADE
        );
        INSERT INTO worker_control_grants_v37(
            grant_id, workspace_id,
            controller_runtime_id, controller_worker_id,
            subject_runtime_id, subject_worker_id,
            relation, origin, permissions_json, operation_id,
            created_at, revoked_at
        )
        SELECT g.grant_id, g.workspace_id,
               g.controller_runtime_id, controller.worker_id,
               g.subject_runtime_id, subject.worker_id,
               g.relation, g.origin, g.permissions_json, g.operation_id,
               g.created_at, g.revoked_at
        FROM worker_control_grants g
        JOIN worker_identity_v37 controller
          ON controller.workspace_id = g.workspace_id
         AND controller.runtime_id = g.controller_runtime_id
         AND controller.runtime_worker_id = g.controller_worker_id
        JOIN worker_identity_v37 subject
          ON subject.workspace_id = g.workspace_id
         AND subject.runtime_id = g.subject_runtime_id
         AND subject.runtime_worker_id = g.subject_worker_id;

        UPDATE ticket_worker_assignments
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = ticket_worker_assignments.workspace_id
              AND m.runtime_id = ticket_worker_assignments.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = ticket_worker_assignments.worker_id
        );
        UPDATE ticket_current_worker_assignments
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = ticket_current_worker_assignments.workspace_id
              AND m.runtime_id = ticket_current_worker_assignments.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = ticket_current_worker_assignments.worker_id
        );
        UPDATE ticket_assignment_operations
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = ticket_assignment_operations.workspace_id
              AND m.runtime_id = ticket_assignment_operations.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = ticket_assignment_operations.worker_id
        )
        WHERE worker_id IS NOT NULL;

        UPDATE worker_removal_operations
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = worker_removal_operations.workspace_id
              AND m.runtime_id = worker_removal_operations.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = worker_removal_operations.worker_id
        );
        UPDATE worker_session_archives
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = worker_session_archives.workspace_id
              AND m.runtime_id = worker_session_archives.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = worker_session_archives.worker_id
        );
        UPDATE worker_diagnostics_archives
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = worker_diagnostics_archives.workspace_id
              AND m.runtime_id = worker_diagnostics_archives.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = worker_diagnostics_archives.worker_id
        );
        UPDATE worker_tombstones
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = worker_tombstones.workspace_id
              AND m.runtime_id = worker_tombstones.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = worker_tombstones.worker_id
        );
        UPDATE worker_orphan_diagnostics
        SET worker_id = (
            SELECT m.worker_id FROM worker_identity_v37 m
            WHERE m.workspace_id = worker_orphan_diagnostics.workspace_id
              AND m.runtime_id = worker_orphan_diagnostics.runtime_id
              AND CAST(m.runtime_worker_id AS TEXT) = worker_orphan_diagnostics.worker_id
        );

        DROP TABLE worker_control_grants;
        DROP TABLE worker_workdir_links;
        DROP TABLE worker_registry;
        ALTER TABLE worker_registry_v37 RENAME TO worker_registry;
        ALTER TABLE worker_workdir_links_v37 RENAME TO worker_workdir_links;
        ALTER TABLE worker_control_grants_v37 RENAME TO worker_control_grants;
        CREATE INDEX worker_registry_runtime
            ON worker_registry(workspace_id, runtime_id, worker_id);
        CREATE INDEX worker_workdir_links_workdir
            ON worker_workdir_links(workspace_id, workdir_id);
        CREATE UNIQUE INDEX worker_workdir_links_active_worker_unique
            ON worker_workdir_links(workspace_id, worker_id)
            WHERE unlinked_at IS NULL;
        CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
            ON worker_workdir_links(workspace_id, workdir_id)
            WHERE unlinked_at IS NULL;
        CREATE INDEX worker_control_grants_controller
            ON worker_control_grants(
                workspace_id, controller_worker_id, revoked_at
            );
        CREATE INDEX worker_control_grants_subject
            ON worker_control_grants(
                workspace_id, subject_worker_id, revoked_at
            );
        CREATE TABLE worker_create_reservations (
            workspace_id TEXT NOT NULL,
            allocation_key TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            create_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'created')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, allocation_key),
            UNIQUE (workspace_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        CREATE INDEX worker_create_reservations_worker
            ON worker_create_reservations(workspace_id, worker_id);
        DROP TABLE worker_identity_v37;
        "#,
    )?;
    Ok(())
}

fn remove_worker_control_delegation_authority(conn: &Connection) -> Result<()> {
    let mut statement =
        conn.prepare("SELECT workspace_id, grant_id, permissions_json FROM worker_control_grants")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut permission_updates = Vec::new();
    for row in rows {
        let (workspace_id, grant_id, permissions_json) = row?;
        let mut permissions: Vec<String> =
            serde_json::from_str(&permissions_json).map_err(|error| {
                Error::Store(format!(
                    "failed to decode Worker control grant `{grant_id}` permissions during delegation removal: {error}"
                ))
            })?;
        let previous_len = permissions.len();
        permissions
            .retain(|permission| !matches!(permission.as_str(), "share" | "transfer" | "revoke"));
        if permissions.len() != previous_len {
            permission_updates.push((
                workspace_id,
                grant_id,
                serde_json::to_string(&permissions).map_err(|error| {
                    Error::Store(format!(
                        "failed to encode Worker control grant permissions during delegation removal: {error}"
                    ))
                })?,
            ));
        }
    }
    drop(statement);

    for (workspace_id, grant_id, permissions_json) in permission_updates {
        conn.execute(
            "UPDATE worker_control_grants SET permissions_json = ?3 WHERE workspace_id = ?1 AND grant_id = ?2",
            params![workspace_id, grant_id, permissions_json],
        )?;
    }
    conn.execute(
        r#"UPDATE worker_control_grants
           SET revoked_at = COALESCE(revoked_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
           WHERE relation IN ('shared', 'transferred')"#,
        [],
    )?;
    conn.execute_batch("DROP TABLE IF EXISTS worker_control_delegation_operations;")?;
    Ok(())
}

fn create_worker_mutation_source_proof_replay_guard(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS worker_mutation_source_proof_jtis (
            runtime_id TEXT NOT NULL,
            jti TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            consumed_at TEXT NOT NULL,
            PRIMARY KEY (runtime_id, jti)
        );
        CREATE INDEX IF NOT EXISTS idx_worker_mutation_source_proof_jtis_expiry
            ON worker_mutation_source_proof_jtis(expires_at);
        "#,
    )?;
    Ok(())
}

pub(crate) fn materialize_main_config_entrypoint(conn: &Connection) -> Result<()> {
    let mut statement =
        conn.prepare("SELECT workspace_id, created_at FROM workspaces ORDER BY workspace_id")?;
    let workspaces = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);

    for (workspace_id, created_at) in workspaces {
        let existing = crate::config_source::load_state(conn, &workspace_id)?;
        let state = match existing {
            None => crate::config_source::initial_state()?,
            Some(existing) => {
                let main =
                    config_source::VirtualPath::parse(crate::config_source::MAIN_CONFIG_ENTRYPOINT)
                        .map_err(|error| Error::Store(error.to_string()))?;
                let snapshot = if existing.snapshot.entries.contains_key(&main) {
                    existing.snapshot
                } else {
                    existing
                        .snapshot
                        .apply(&[config_source::ConfigTreeChange::Create {
                            path: main.clone(),
                            content_type: config_source::ConfigContentType::Decodal,
                            content: crate::config_source::DEFAULT_MAIN_CONFIG_SOURCE.to_string(),
                        }])
                        .map_err(|error| Error::Store(error.to_string()))?
                };
                let contract = config_source::ToolchainContract::new(
                    config_source::DEFAULT_SCHEMA_VERSION,
                    vec![main],
                    config_source::DEFAULT_IMPORT_POLICY_VERSION,
                );
                let evaluation = config_source::SnapshotEnvironment::new(snapshot.clone())
                    .evaluate_contract(&contract)
                    .map_err(|diagnostics| {
                        Error::Store(format!(
                            "cannot materialize main.dcdl for Workspace {workspace_id}: {}",
                            serde_json::to_string(&diagnostics)
                                .unwrap_or_else(|_| "config evaluation failed".to_string())
                        ))
                    })?;
                crate::config_source::WorkspaceConfigState {
                    snapshot,
                    contract,
                    projection_digest: evaluation.projection_digest,
                }
            }
        };
        conn.execute(
            "DELETE FROM workspace_config_tree_revisions WHERE workspace_id = ?1",
            [&workspace_id],
        )?;
        conn.execute(
            "DELETE FROM workspace_config_entries WHERE workspace_id = ?1",
            [&workspace_id],
        )?;
        crate::config_source::insert_materialized_state(conn, &workspace_id, &state, &created_at)?;
    }
    Ok(())
}

pub(crate) fn apply_migrations_through(conn: &Connection, through_version: i64) -> Result<()> {
    let current = current_schema_version(conn)?;
    for migration in MIGRATIONS.iter().filter(|migration| {
        i64::from(migration.version) > current && i64::from(migration.version) <= through_version
    }) {
        let tx = conn.unchecked_transaction()?;
        (migration.apply)(&tx)?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![migration.version, migration.name],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn apply_migrations(conn: &Connection) -> Result<()> {
    apply_migrations_through(conn, i64::MAX)
}

fn align_legacy_bootstrap_schema(conn: &Connection) -> Result<()> {
    if table_exists(conn, "repositories")?
        && column_exists(conn, "repositories", "local_root")?
        && !column_exists(conn, "repositories", "uri")?
    {
        rename_legacy_table(conn, "repositories", "legacy_repositories")?;
    }
    if table_exists(conn, "runs")? {
        rename_legacy_table(conn, "runs", "legacy_runs")?;
    }
    if table_exists(conn, "artifacts")?
        && (column_exists(conn, "artifacts", "run_id")?
            || column_exists(conn, "artifacts", "path")?
            || !column_exists(conn, "artifacts", "uri")?)
    {
        rename_legacy_table(conn, "artifacts", "legacy_artifacts")?;
    }
    if table_exists(conn, "ticket_projections")? {
        rename_legacy_table(conn, "ticket_projections", "legacy_ticket_projections")?;
    }
    if table_exists(conn, "objective_projections")? {
        rename_legacy_table(
            conn,
            "objective_projections",
            "legacy_objective_projections",
        )?;
    }

    let legacy_workspaces = preserve_noncanonical_workspaces(conn)?;
    create_schema_v0_tables(conn)?;
    if let Some(legacy_table) = legacy_workspaces {
        copy_legacy_workspaces(conn, &legacy_table)?;
    }
    Ok(())
}

fn preserve_noncanonical_workspaces(conn: &Connection) -> Result<Option<String>> {
    if !table_exists(conn, "workspaces")? {
        return Ok(None);
    }
    let columns = table_columns(conn, "workspaces")?;
    if columns
        .iter()
        .map(String::as_str)
        .eq(WORKSPACES_V0_COLUMNS.iter().copied())
    {
        return Ok(None);
    }
    let legacy_table = "legacy_workspaces";
    rename_legacy_table(conn, "workspaces", legacy_table)?;
    Ok(Some(legacy_table.to_string()))
}

fn copy_legacy_workspaces(conn: &Connection, legacy_table: &str) -> Result<()> {
    let columns = table_columns(conn, legacy_table)?;
    for required_column in ["workspace_id", "display_name", "created_at", "updated_at"] {
        if !columns.iter().any(|column| column == required_column) {
            return Err(Error::Store(format!(
                "cannot migrate legacy workspaces: `{legacy_table}` is missing `{required_column}`"
            )));
        }
    }
    let state_expr = if columns.iter().any(|column| column == "state") {
        "COALESCE(NULLIF(state, ''), 'active')"
    } else {
        "'active'"
    };
    conn.execute_batch(&format!(
        r#"INSERT OR IGNORE INTO workspaces (
            workspace_id, display_name, state, created_at, updated_at
        )
        SELECT workspace_id, display_name, {state_expr}, created_at, updated_at
        FROM {legacy_table};"#
    ))?;
    Ok(())
}

fn rename_legacy_table(conn: &Connection, table_name: &str, legacy_name: &str) -> Result<()> {
    if table_exists(conn, legacy_name)? {
        return Err(Error::Store(format!(
            "cannot preserve legacy table `{table_name}` because `{legacy_name}` already exists"
        )));
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table_name} RENAME TO {legacy_name};"
    ))?;
    Ok(())
}

fn remove_worker_registry_legacy_live_state_column(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "worker_registry")?
        || !table_columns(conn, "worker_registry")?
            .iter()
            .any(|column| column == "lifecycle_state")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE worker_registry_v4 (
            workspace_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, worker_id),
            UNIQUE (workspace_id, runtime_id, runtime_worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT INTO worker_registry_v4 (
            workspace_id, worker_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        )
        SELECT
            workspace_id, worker_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        FROM worker_registry;
        DROP TABLE worker_registry;
        ALTER TABLE worker_registry_v4 RENAME TO worker_registry;
        CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
            ON worker_registry(workspace_id, updated_at DESC);
        "#,
    )?;
    Ok(())
}

fn use_composite_worker_registry_keys(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "worker_registry")?
        || !table_columns(conn, "worker_registry")?
            .iter()
            .any(|column| column == "worker_id")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE worker_registry_v5 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO worker_registry_v5 (
            workspace_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        )
        SELECT
            workspace_id,
            runtime_id,
            CASE
                WHEN typeof(runtime_worker_id) = 'integer' THEN runtime_worker_id
                WHEN runtime_worker_id GLOB 'worker-[0-9]*' THEN CAST(substr(runtime_worker_id, 8) AS INTEGER)
                WHEN runtime_worker_id GLOB '[0-9]*' THEN CAST(runtime_worker_id AS INTEGER)
                ELSE rowid
            END,
            display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        FROM worker_registry;

        CREATE TABLE worker_workdir_links_v5 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            workdir_id TEXT NOT NULL,
            role TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            unlinked_at TEXT,
            PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id, workdir_id, role),
            FOREIGN KEY (workspace_id, runtime_id, runtime_worker_id) REFERENCES worker_registry_v5(workspace_id, runtime_id, runtime_worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id) REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO worker_workdir_links_v5 (
            workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
        )
        SELECT
            links.workspace_id,
            registry.runtime_id,
            CASE
                WHEN typeof(registry.runtime_worker_id) = 'integer' THEN registry.runtime_worker_id
                WHEN registry.runtime_worker_id GLOB 'worker-[0-9]*' THEN CAST(substr(registry.runtime_worker_id, 8) AS INTEGER)
                WHEN registry.runtime_worker_id GLOB '[0-9]*' THEN CAST(registry.runtime_worker_id AS INTEGER)
                ELSE registry.rowid
            END,
            links.workdir_id,
            links.role,
            links.linked_at,
            links.unlinked_at
        FROM worker_workdir_links AS links
        JOIN worker_registry AS registry
          ON registry.workspace_id = links.workspace_id
         AND registry.worker_id = links.worker_id;

        DROP TABLE worker_workdir_links;
        DROP TABLE worker_registry;
        ALTER TABLE worker_registry_v5 RENAME TO worker_registry;
        ALTER TABLE worker_workdir_links_v5 RENAME TO worker_workdir_links;
        CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
            ON worker_registry(workspace_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_worker_workdir_links_worker
            ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id, linked_at DESC);
        "#,
    )?;
    Ok(())
}

fn add_workdir_runtime_observation_states(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "workdir_registry")? {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE workdir_registry_v6 (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            selector TEXT,
            resolved_commit TEXT,
            materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
            cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO workdir_registry_v6 (
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        )
        SELECT
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            CASE materialization_status
                WHEN 'missing' THEN 'not_found'
                WHEN 'removed' THEN 'not_found'
                ELSE materialization_status
            END,
            cleanliness, created_at, updated_at
        FROM workdir_registry;
        DROP TABLE workdir_registry;
        ALTER TABLE workdir_registry_v6 RENAME TO workdir_registry;
        CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
            ON workdir_registry(workspace_id, updated_at DESC);
        "#,
    )?;
    Ok(())
}

fn remove_workdir_registry_management_kind_column(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "workdir_registry")?
        || !table_columns(conn, "workdir_registry")?
            .iter()
            .any(|column| column == "management_kind")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE workdir_registry_v7 (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            selector TEXT,
            resolved_commit TEXT,
            materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
            cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO workdir_registry_v7 (
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        )
        SELECT
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        FROM workdir_registry;
        DROP TABLE workdir_registry;
        ALTER TABLE workdir_registry_v7 RENAME TO workdir_registry;
        CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
            ON workdir_registry(workspace_id, updated_at DESC);
        "#,
    )?;
    Ok(())
}

fn create_schema_v0_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objectives (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objective_ticket_links (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL REFERENCES objectives(objective_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (objective_id, ticket_id, kind)
);

CREATE TABLE IF NOT EXISTS objective_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL REFERENCES objectives(objective_id) ON DELETE CASCADE,
    event_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    body_md TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objective_resources (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL REFERENCES objectives(objective_id) ON DELETE CASCADE,
    resource_path TEXT NOT NULL,
    body TEXT NOT NULL,
    media_type TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (objective_id, resource_path)
);

CREATE TABLE IF NOT EXISTS memory_staging_records (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id)
);

CREATE TABLE IF NOT EXISTS workspace_memory_documents (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_staging_resolutions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id, resolved_at)
);

CREATE TABLE IF NOT EXISTS repositories (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    repository_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT,
    uri TEXT NOT NULL,
    default_ref TEXT,
    auth_ref_kind TEXT,
    auth_ref_key TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    artifact_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    media_type TEXT,
    sha256 TEXT,
    size_bytes INTEGER,
    summary TEXT,
    created_at TEXT NOT NULL,
    created_by_kind TEXT NOT NULL,
    created_by_key TEXT NOT NULL,
    created_by_display TEXT NOT NULL,
    created_by_source_kind TEXT,
    created_by_source_key TEXT,
    ticket_id TEXT,
    objective_id TEXT,
    event_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    repository_id TEXT,
    source_kind TEXT,
    source_revision TEXT
);

CREATE TABLE IF NOT EXISTS audit_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    audit_event_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_key TEXT NOT NULL,
    actor_display TEXT NOT NULL,
    actor_source_kind TEXT,
    actor_source_key TEXT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    outcome TEXT NOT NULL,
    request_id TEXT,
    summary TEXT
);
"#,
    )?;
    Ok(())
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
    use std::collections::BTreeSet;

    #[test]
    fn startup_composes_ticket_migrations_when_control_plane_is_current() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        apply_migrations(&conn).unwrap();
        assert!(!table_exists(&conn, "ticket_schema_migrations").unwrap());

        let store = SqliteWorkspaceStore::from_connection(conn).unwrap();
        store
            .with_conn(|conn| {
                ticket::verify_sqlite_ticket_schema(conn)?;
                let latest = conn.query_row(
                    "SELECT MAX(version) FROM ticket_schema_migrations",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                assert_eq!(latest, ticket::LATEST_SQLITE_TICKET_SCHEMA_VERSION);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn startup_fails_closed_when_current_ticket_schema_has_drifted() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        apply_migrations(&conn).unwrap();
        ticket::migrate_sqlite_ticket_schema(&conn).unwrap();
        conn.execute_batch("DROP TABLE typed_ticket_artifacts")
            .unwrap();

        let result = SqliteWorkspaceStore::from_connection(conn);
        let error = match result {
            Ok(_) => panic!("schema drift unexpectedly passed startup verification"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("typed_ticket_artifacts"));
    }

    #[test]
    fn removes_unused_control_plane_ticket_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE tickets (ticket_id TEXT PRIMARY KEY);
CREATE TABLE ticket_events (event_id TEXT PRIMARY KEY, ticket_id TEXT REFERENCES tickets(ticket_id));
CREATE TABLE ticket_relations (source_ticket_id TEXT, target_ticket_id TEXT);
CREATE TABLE ticket_targets (ticket_id TEXT, target_id TEXT, PRIMARY KEY (ticket_id, target_id));
CREATE TABLE ticket_target_paths (ticket_id TEXT, target_id TEXT, path TEXT);
CREATE TABLE ticket_worker_links (ticket_id TEXT, worker_ref_key TEXT);
"#,
        )
        .unwrap();
        remove_unused_control_plane_ticket_tables(&conn).unwrap();
        for table in [
            "tickets",
            "ticket_events",
            "ticket_relations",
            "ticket_targets",
            "ticket_target_paths",
            "ticket_worker_links",
            "ticket_notification_outbox",
            "ticket_notification_deliveries",
            "ticket_notification_cursors",
        ] {
            assert!(!table_exists(&conn, table).unwrap(), "{table} still exists");
        }
    }

    #[test]
    fn schema_v35_removes_worker_control_delegation_authority() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 34)
        {
            let tx = conn.unchecked_transaction().unwrap();
            (migration.apply)(&tx).unwrap();
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![migration.version, migration.name],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        conn.execute_batch(
            r#"
INSERT INTO workspaces (
    workspace_id, display_name, state, created_at, updated_at
) VALUES ('workspace-a', 'Workspace A', 'active', '1', '1');
INSERT INTO worker_registry (
    workspace_id, runtime_id, runtime_worker_id, display_name,
    retention_state, created_at, updated_at
) VALUES
    ('workspace-a', 'runtime-a', 1, 'Controller', 'normal', '1', '1'),
    ('workspace-a', 'runtime-a', 2, 'Spawned Worker', 'normal', '1', '1'),
    ('workspace-a', 'runtime-a', 3, 'Shared Worker', 'normal', '1', '1'),
    ('workspace-a', 'runtime-a', 4, 'Transferred Worker', 'normal', '1', '1');
INSERT INTO worker_control_grants (
    workspace_id, grant_id,
    controller_runtime_id, controller_worker_id,
    subject_runtime_id, subject_worker_id,
    relation, origin, permissions_json, operation_id, created_at, revoked_at
) VALUES
    ('workspace-a', 'spawned', 'runtime-a', 1, 'runtime-a', 2,
     'spawned', 'spawn', '["observe","share","transfer","revoke","stop"]', 'spawn-op', '1', NULL),
    ('workspace-a', 'shared', 'runtime-a', 1, 'runtime-a', 3,
     'shared', 'share', '["observe"]', 'share-op', '1', NULL),
    ('workspace-a', 'transferred', 'runtime-a', 1, 'runtime-a', 4,
     'transferred', 'transfer', '["observe"]', 'transfer-op', '1', NULL);
INSERT INTO ticket_worker_assignments (
    workspace_id, ticket_id, assignment_id, runtime_id, worker_id, assigned_by, assigned_at
) VALUES ('workspace-a', 'ticket-a', 'assignment-a', 'runtime-a', '1', 'test', '1');
INSERT INTO ticket_current_worker_assignments (
    workspace_id, ticket_id, assignment_id, runtime_id, worker_id, updated_at
) VALUES ('workspace-a', 'ticket-a', 'assignment-a', 'runtime-a', '1', '1');
INSERT INTO ticket_assignment_operations (
    workspace_id, operation_id, action, ticket_id, runtime_id, worker_id,
    assignment_id, request_fingerprint, created_at
) VALUES (
    'workspace-a', 'assignment-operation-a', 'assign', 'ticket-a',
    'runtime-a', '1', 'assignment-a', 'sha256:test', '1'
);
INSERT INTO worker_orphan_diagnostics (
    diagnostic_id, workspace_id, runtime_id, worker_id, category, detail, observed_at
) VALUES (
    'orphan-a', 'workspace-a', 'runtime-a', '9', 'runtime_aggregate_without_backend_registry',
    'legacy orphan', '1'
);
"#,
        )
        .unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 34);
        assert!(table_exists(&conn, "worker_control_delegation_operations").unwrap());

        apply_migrations(&conn).unwrap();

        assert_eq!(current_schema_version(&conn).unwrap(), 37);
        assert!(!table_exists(&conn, "worker_control_delegation_operations").unwrap());
        let controller_worker_id: String = conn
            .query_row(
                "SELECT worker_id FROM worker_registry WHERE display_name = 'Controller'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            controller_worker_id,
            WorkerId::from_legacy_binding("workspace-a", "runtime-a", 1).to_string()
        );
        let (grant_controller, grant_subject): (String, String) = conn
            .query_row(
                "SELECT controller_worker_id, subject_worker_id \
                 FROM worker_control_grants WHERE grant_id = 'spawned'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(grant_controller, controller_worker_id);
        assert_eq!(
            grant_subject,
            WorkerId::from_legacy_binding("workspace-a", "runtime-a", 2).to_string()
        );
        let expected_assignment_worker =
            WorkerId::from_legacy_binding("workspace-a", "runtime-a", 1).to_string();
        for table in [
            "ticket_worker_assignments",
            "ticket_current_worker_assignments",
            "ticket_assignment_operations",
        ] {
            let sql = format!("SELECT worker_id FROM {table} WHERE ticket_id = 'ticket-a'");
            let migrated_worker_id: String = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
            assert_eq!(
                migrated_worker_id, expected_assignment_worker,
                "{table} retained a legacy Worker id"
            );
        }
        let orphan_worker_id: String = conn
            .query_row(
                "SELECT worker_id FROM worker_orphan_diagnostics WHERE diagnostic_id = 'orphan-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            orphan_worker_id,
            WorkerId::from_legacy_binding("workspace-a", "runtime-a", 9).to_string()
        );
        let worker_registry_pk = {
            let mut statement = conn.prepare("PRAGMA table_info(worker_registry)").unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
                })
                .unwrap()
                .filter_map(|row| {
                    let (name, position) = row.unwrap();
                    (position > 0).then_some((position, name))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            worker_registry_pk,
            vec![
                (1, "workspace_id".to_string()),
                (2, "worker_id".to_string())
            ]
        );

        let (permissions_json, revoked_at): (String, Option<String>) = conn
            .query_row(
                "SELECT permissions_json, revoked_at FROM worker_control_grants WHERE grant_id = 'spawned'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&permissions_json).unwrap(),
            vec!["observe", "stop"]
        );
        assert!(revoked_at.is_none());
        for grant_id in ["shared", "transferred"] {
            let revoked_at: Option<String> = conn
                .query_row(
                    "SELECT revoked_at FROM worker_control_grants WHERE grant_id = ?1",
                    [grant_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(revoked_at.is_some(), "{grant_id} grant remained active");
        }
    }

    #[test]
    fn schema_v24_adds_attachment_reservations_to_already_applied_v23() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 23)
        {
            let tx = conn.unchecked_transaction().unwrap();
            (migration.apply)(&tx).unwrap();
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![migration.version, migration.name],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(current_schema_version(&conn).unwrap(), 23);
        assert!(!table_exists(&conn, "worker_workdir_attachment_reservations").unwrap());

        apply_migrations(&conn).unwrap();

        assert_eq!(current_schema_version(&conn).unwrap(), 37);
        assert!(table_exists(&conn, "worker_workdir_attachment_reservations").unwrap());
    }

    #[test]
    fn schema_v26_removes_legacy_backend_flow_runtime_tables() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 25)
        {
            let tx = conn.unchecked_transaction().unwrap();
            (migration.apply)(&tx).unwrap();
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![migration.version, migration.name],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        conn.execute_batch(
            r#"
CREATE TABLE flow_instances (instance_id TEXT PRIMARY KEY);
CREATE TABLE flow_transition_attempts (attempt_id TEXT PRIMARY KEY);
CREATE TABLE flow_events (event_id TEXT PRIMARY KEY);
"#,
        )
        .unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 25);

        apply_migrations(&conn).unwrap();

        assert_eq!(current_schema_version(&conn).unwrap(), 37);
        assert!(table_exists(&conn, "flow_sources").unwrap());
        assert!(table_exists(&conn, "flow_source_revisions").unwrap());
        assert!(!table_exists(&conn, "flow_instances").unwrap());
        assert!(!table_exists(&conn, "flow_transition_attempts").unwrap());
        assert!(!table_exists(&conn, "flow_events").unwrap());
    }

    #[test]
    fn schema_v27_upgrades_repository_identity_without_losing_workspace_owned_references() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 26)
        {
            let tx = conn.unchecked_transaction().unwrap();
            (migration.apply)(&tx).unwrap();
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![migration.version, migration.name],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        conn.execute_batch(
            r#"
INSERT INTO workspaces (
    workspace_id, display_name, state, created_at, updated_at
) VALUES
    ('workspace-a', 'Workspace A', 'active', '1', '1'),
    ('workspace-b', 'Workspace B', 'active', '1', '1');
INSERT INTO repositories (
    repository_id, workspace_id, name, kind, uri, created_at, updated_at
) VALUES ('main', 'workspace-a', 'Main', 'git', '/repo-a', '1', '1');
INSERT INTO artifacts (
    workspace_id, artifact_id, kind, uri, created_at,
    created_by_kind, created_by_key, created_by_display, repository_id
) VALUES (
    'workspace-a', 'artifact-1', 'report', 'artifact://1', '1',
    'worker', 'worker-1', 'Worker 1', 'main'
);
INSERT INTO worker_registry (
    workspace_id, runtime_id, runtime_worker_id, display_name,
    retention_state, created_at, updated_at
) VALUES ('workspace-a', 'runtime-a', 1, 'Worker 1', 'normal', '1', '1');
INSERT INTO workdir_registry (
    workspace_id, workdir_id, runtime_id, repository_id,
    creation_selector, creation_ref, materialization_status,
    cleanliness, created_at, updated_at, current_selector, current_ref
) VALUES (
    'workspace-a', 'workdir-1', 'runtime-a', 'main',
    'develop', 'abc', 'present', 'clean', '1', '1', 'develop', 'abc'
);
INSERT INTO worker_workdir_links (
    workspace_id, runtime_id, runtime_worker_id, workdir_id,
    role, linked_at, unlinked_at
) VALUES ('workspace-a', 'runtime-a', 1, 'workdir-1', 'attachment', '1', NULL);
INSERT INTO worker_workdir_attachment_reservations (
    workspace_id, workdir_id, reservation_id, reserved_at
) VALUES ('workspace-a', 'workdir-1', 'reservation-1', '1');
"#,
        )
        .unwrap();

        apply_migrations(&conn).unwrap();

        assert_eq!(current_schema_version(&conn).unwrap(), 37);
        let repositories_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'repositories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(repositories_sql.contains("PRIMARY KEY (workspace_id, repository_id)"));
        let preserved: (i64, i64, i64) = (
            conn.query_row("SELECT COUNT(*) FROM artifacts", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM worker_workdir_links", [], |row| {
                row.get(0)
            })
            .unwrap(),
            conn.query_row(
                "SELECT COUNT(*) FROM worker_workdir_attachment_reservations",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        );
        assert_eq!(preserved, (1, 1, 1));
        let foreign_key_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_violations, 0);
        conn.execute(
            r#"INSERT INTO repositories (
                workspace_id, repository_id, name, kind, uri, created_at, updated_at
            ) VALUES ('workspace-b', 'main', 'Other Main', 'git', '/repo-b', '2', '2')"#,
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repositories WHERE repository_id = 'main'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            2
        );
        assert!(
            conn.execute(
                r#"INSERT INTO workdir_registry (
                    workspace_id, workdir_id, runtime_id, repository_id,
                    materialization_status, cleanliness, created_at, updated_at
                ) VALUES ('workspace-b', 'invalid', 'runtime-b', 'missing',
                          'present', 'clean', '2', '2')"#,
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                r#"INSERT INTO artifacts (
                    workspace_id, artifact_id, kind, uri, created_at,
                    created_by_kind, created_by_key, created_by_display, repository_id
                ) VALUES ('workspace-b', 'invalid-artifact', 'report', 'artifact://invalid', '2',
                          'worker', 'worker-2', 'Worker 2', 'missing')"#,
                [],
            )
            .is_err()
        );
    }

    #[test]
    fn schema_v27_rejects_cross_workspace_legacy_repository_references() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 26)
        {
            let tx = conn.unchecked_transaction().unwrap();
            (migration.apply)(&tx).unwrap();
            tx.execute(
                "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
                params![migration.version, migration.name],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        conn.execute_batch(
            r#"
INSERT INTO workspaces (
    workspace_id, display_name, state, created_at, updated_at
) VALUES
    ('workspace-a', 'Workspace A', 'active', '1', '1'),
    ('workspace-b', 'Workspace B', 'active', '1', '1');
INSERT INTO repositories (
    repository_id, workspace_id, name, kind, uri, created_at, updated_at
) VALUES ('main', 'workspace-a', 'Main', 'git', '/repo-a', '1', '1');
INSERT INTO workdir_registry (
    workspace_id, workdir_id, runtime_id, repository_id,
    materialization_status, cleanliness, created_at, updated_at
) VALUES ('workspace-b', 'foreign-workdir', 'runtime-b', 'main',
          'present', 'clean', '1', '1');
"#,
        )
        .unwrap();

        let error = apply_migrations(&conn).unwrap_err();

        assert!(error.to_string().contains("workdir_registry"));
        assert!(error.to_string().contains("workspace-b"));
        assert_eq!(current_schema_version(&conn).unwrap(), 26);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM workdir_registry", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn startup_rejects_cross_workspace_ticket_repository_reference_without_claiming_ticket_schema()
     {
        let dir = tempfile::tempdir().unwrap();
        let database_path = dir.path().join("workspace.sqlite");
        let store = SqliteWorkspaceStore::open(&database_path).unwrap();
        for workspace_id in ["workspace-a", "workspace-b"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: workspace_id.to_string(),
                    owner_account_id: None,
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
                name: "Main".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                uri: "/repo-a".to_string(),
                default_ref: Some("HEAD".to_string()),
                auth_ref_kind: None,
                auth_ref_key: None,
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
        ticket::TicketBackend::create(&backend, input).unwrap();
        drop(backend);

        let error = match SqliteWorkspaceStore::open(&database_path) {
            Ok(_) => panic!("cross-Workspace Ticket repository reference must fail closed"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("typed_tickets"));
        assert!(error.to_string().contains("workspace-b"));
    }

    #[tokio::test]
    async fn migrates_sqlite_and_preserves_workspace_record() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("control-plane.sqlite");
        let store = SqliteWorkspaceStore::open(&db).unwrap();

        assert_eq!(store.schema_version().await.unwrap(), 37);
        assert!(
            !store
                .with_conn(|conn| table_exists(conn, "worker_workspace_credentials"))
                .unwrap()
        );
        let record = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: None,
            display_name: "Yoi Dev".to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&record).await.unwrap();

        let reopened = SqliteWorkspaceStore::open(&db).unwrap();
        assert_eq!(reopened.schema_version().await.unwrap(), 37);
        assert_eq!(
            reopened.get_workspace("local-dev").await.unwrap(),
            Some(record)
        );
    }

    #[tokio::test]
    async fn worker_create_reservation_allocates_uuid_before_runtime_and_replays_exact_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: None,
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "2026-08-06T00:00:00Z".to_string(),
                updated_at: "2026-08-06T00:00:00Z".to_string(),
            })
            .await
            .unwrap();

        let reserved = store
            .reserve_worker_create("workspace-a", "arcadia", "operation-1", "sha256:one")
            .unwrap();
        assert_eq!(
            reserved.as_uuid().get_version(),
            Some(uuid::Version::SortRand)
        );
        assert_eq!(
            store
                .reserve_worker_create("workspace-a", "arcadia", "operation-1", "sha256:one")
                .unwrap(),
            reserved
        );
        assert!(
            store
                .reserve_worker_create("workspace-a", "arcadia", "operation-1", "sha256:different")
                .is_err()
        );
        store
            .complete_worker_create_reservation("workspace-a", reserved)
            .unwrap();
        let state: String = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT state FROM worker_create_reservations \
                     WHERE workspace_id = 'workspace-a' AND worker_id = ?1",
                    [reserved.to_string()],
                    |row| row.get(0),
                )
                .map_err(Error::from)
            })
            .unwrap();
        assert_eq!(state, "created");
    }

    #[tokio::test]
    async fn workspace_flow_sources_keep_revisions_and_builtins_stay_resources() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: None,
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
                owner_account_id: None,
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "2026-07-32T00:00:00Z".to_string(),
                updated_at: "2026-07-32T00:00:00Z".to_string(),
            })
            .await
            .unwrap();

        let first = TicketWorkerAssignmentRecord {
            workspace_id: "workspace-a".to_string(),
            ticket_id: "ticket-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            worker: RuntimeWorkerRef::new("runtime-1", "worker-1"),
            assigned_by: "user-1".to_string(),
            assigned_at: "2026-07-32T00:00:01Z".to_string(),
        };
        let created = store
            .set_current_ticket_worker_assignment(&first, None, "event-1", "operation-1", false)
            .unwrap();
        assert_eq!(created.current, first);
        assert_eq!(created.previous, None);
        let retried = store
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
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
                .list_ticket_worker_assignment_events("workspace-a", "ticket-1", 10)
                .unwrap()
                .len(),
            1,
            "idempotent retry must not append another assignment event"
        );
        let implicit_reassign = store
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
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
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
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

        let second = TicketWorkerAssignmentRecord {
            assignment_id: "assignment-2".to_string(),
            worker: RuntimeWorkerRef::new("runtime-2", "worker-2"),
            assigned_by: "user-2".to_string(),
            assigned_at: "2026-07-32T00:00:02Z".to_string(),
            ..first.clone()
        };
        let replaced = store
            .set_current_ticket_worker_assignment(
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
            .set_current_ticket_worker_assignment(
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
                .get_current_ticket_worker_assignment("workspace-a", "ticket-1")
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
        let reserved_assignment = TicketWorkerAssignmentRecord {
            workspace_id: "workspace-a".to_string(),
            ticket_id: "ticket-3".to_string(),
            assignment_id: "assignment-3".to_string(),
            worker: RuntimeWorkerRef::new("runtime-3", "worker-3"),
            assigned_by: "runtime".to_string(),
            assigned_at: "2026-07-32T00:00:06Z".to_string(),
        };
        let completed_reservation = store
            .set_current_ticket_worker_assignment(
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
                .get_current_ticket_worker_assignment("workspace-a", "ticket-1")
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
                .get_current_ticket_worker_assignment("workspace-a", "ticket-3")
                .unwrap(),
            None
        );
        assert!(
            store
                .list_ticket_worker_assignment_events("workspace-a", "ticket-3", 10)
                .unwrap()
                .is_empty()
        );
        store
            .rollback_ticket_assignment_operation("workspace-a", "reserved-operation")
            .unwrap();

        let events = store
            .list_ticket_worker_assignment_events("workspace-a", "ticket-1", 10)
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

    #[test]
    fn fresh_schema_matches_workspace_db_v0_boundaries() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        apply_migrations(&conn).unwrap();

        let tables = table_names(&conn);
        for expected in [
            "workspaces",
            "objectives",
            "objective_ticket_links",
            "objective_resources",
            "memory_staging_records",
            "workspace_memory_documents",
            "memory_staging_resolutions",
            "repositories",
            "artifacts",
            "audit_events",
            "worker_registry",
            "worker_create_reservations",
            "ticket_worker_assignments",
            "ticket_current_worker_assignments",
            "ticket_worker_assignment_events",
            "workdir_registry",
            "worker_workdir_links",
            "accounts",
            "users",
            "passkey_credentials",
            "auth_challenges",
            "browser_sessions",
            "api_tokens",
            "device_login_flows",
        ] {
            assert!(
                tables.contains(expected),
                "missing expected v0 table {expected}"
            );
        }
        for forbidden in [
            "runs",
            "hosts",
            "workers",
            "actors",
            "validation_results",
            "ci_results",
            "tickets",
            "ticket_events",
            "ticket_relations",
            "ticket_targets",
            "ticket_target_paths",
            "ticket_worker_links",
            "ticket_notification_outbox",
            "ticket_notification_deliveries",
            "ticket_notification_cursors",
        ] {
            assert!(
                !tables.contains(forbidden),
                "fresh v0 schema must not create forbidden table {forbidden}"
            );
        }
        assert!(
            !tables.iter().any(|table| table.starts_with("legacy_")),
            "fresh v0 schema should not create legacy compatibility tables: {tables:?}"
        );

        assert_columns(
            &conn,
            "workspaces",
            [
                "workspace_id",
                "display_name",
                "state",
                "created_at",
                "updated_at",
                "owner_account_id",
            ],
        );
        assert_columns(
            &conn,
            "repositories",
            [
                "workspace_id",
                "repository_id",
                "name",
                "kind",
                "provider",
                "uri",
                "default_ref",
                "auth_ref_kind",
                "auth_ref_key",
                "created_at",
                "updated_at",
            ],
        );
        assert_columns(
            &conn,
            "worker_registry",
            [
                "workspace_id",
                "worker_id",
                "runtime_id",
                "display_name",
                "profile",
                "retention_state",
                "transcript_ref",
                "session_ref",
                "summary_ref",
                "diagnostics_ref",
                "created_at",
                "updated_at",
            ],
        );
        assert_columns(
            &conn,
            "workdir_registry",
            [
                "workspace_id",
                "workdir_id",
                "runtime_id",
                "repository_id",
                "creation_selector",
                "creation_ref",
                "materialization_status",
                "cleanliness",
                "created_at",
                "updated_at",
                "current_selector",
                "current_ref",
            ],
        );
        assert_columns(
            &conn,
            "artifacts",
            [
                "workspace_id",
                "artifact_id",
                "kind",
                "uri",
                "media_type",
                "sha256",
                "size_bytes",
                "summary",
                "created_at",
                "created_by_kind",
                "created_by_key",
                "created_by_display",
                "created_by_source_kind",
                "created_by_source_key",
                "ticket_id",
                "objective_id",
                "event_id",
                "worker_ref_kind",
                "worker_ref_key",
                "worker_display",
                "repository_id",
                "source_kind",
                "source_revision",
            ],
        );

        for table in ["workspaces", "repositories", "artifacts"] {
            let columns = table_columns(&conn, table).unwrap();
            for forbidden_column in [
                "payload",
                "payload_json",
                "metadata",
                "metadata_json",
                "diagnostics_json",
                "run_id",
                "local_root",
                "record_authority",
            ] {
                assert!(
                    !columns.iter().any(|column| column == forbidden_column),
                    "{table} must not contain obsolete/generic column {forbidden_column}"
                );
            }
        }
    }

    #[tokio::test]
    async fn upgrades_legacy_bootstrap_without_canonical_runs_table() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch(LEGACY_BOOTSTRAP_SQL).unwrap();
        conn.execute(
            r#"INSERT INTO workspaces (
                workspace_id, display_name, local_root, record_authority, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                "legacy-workspace",
                "Legacy Workspace",
                "/tmp/legacy-workspace",
                "local_yoi_project_records",
                "2026-01-01T00:00:00Z",
                "2026-01-02T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (1, 'bootstrap workspace control plane')",
            [],
        )
        .unwrap();

        let store = SqliteWorkspaceStore::from_connection(conn).unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 37);

        store
            .with_conn(|conn| {
                let tables = table_names(conn);
                for expected in [
                    "workspaces",
                    "repositories",
                    "artifacts",
                    "audit_events",
                    "workspace_memory_documents",
                    "memory_staging_records",
                    "memory_staging_resolutions",
                    "legacy_workspaces",
                    "legacy_repositories",
                    "legacy_runs",
                    "legacy_artifacts",
                    "legacy_ticket_projections",
                    "legacy_objective_projections",
                ] {
                    assert!(
                        tables.contains(expected),
                        "missing {expected} after upgrade"
                    );
                }
                for forbidden in [
                    "runs",
                    "hosts",
                    "workers",
                    "actors",
                    "validation_results",
                    "tickets",
                    "ticket_events",
                    "ticket_relations",
                    "ticket_targets",
                    "ticket_target_paths",
                    "ticket_worker_links",
                    "ticket_notification_outbox",
                    "ticket_notification_deliveries",
                    "ticket_notification_cursors",
                ] {
                    assert!(
                        !tables.contains(forbidden),
                        "upgraded schema must not retain forbidden canonical table {forbidden}"
                    );
                }
                assert_columns(
                    conn,
                    "workspaces",
                    [
                        "workspace_id",
                        "display_name",
                        "state",
                        "created_at",
                        "updated_at",
                        "owner_account_id",
                    ],
                );
                let legacy_workspace_columns = table_columns(conn, "legacy_workspaces")?;
                assert!(
                    legacy_workspace_columns
                        .iter()
                        .any(|column| column == "local_root")
                );
                assert!(
                    legacy_workspace_columns
                        .iter()
                        .any(|column| column == "record_authority")
                );
                let artifact_columns = table_columns(conn, "artifacts")?;
                assert!(artifact_columns.iter().any(|column| column == "uri"));
                assert!(!artifact_columns.iter().any(|column| column == "run_id"));
                Ok(())
            })
            .unwrap();

        assert_eq!(
            store.get_workspace("legacy-workspace").await.unwrap(),
            Some(WorkspaceRecord {
                workspace_id: "legacy-workspace".to_string(),
                owner_account_id: None,
                display_name: "Legacy Workspace".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
        );

        let new_record = WorkspaceRecord {
            workspace_id: "new-workspace".to_string(),
            owner_account_id: None,
            display_name: "New Workspace".to_string(),
            state: "active".to_string(),
            created_at: "2026-02-01T00:00:00Z".to_string(),
            updated_at: "2026-02-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&new_record).await.unwrap();
        assert_eq!(
            store.get_workspace("new-workspace").await.unwrap(),
            Some(new_record)
        );
    }

    #[test]
    fn workdir_revision_migration_preserves_creation_evidence_and_leaves_observation_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE workdir_registry (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    selector TEXT,
    resolved_commit TEXT,
    materialization_status TEXT NOT NULL,
    cleanliness TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id)
);
INSERT INTO workdir_registry (
    workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
    materialization_status, cleanliness, created_at, updated_at
) VALUES (
    'workspace', 'workdir', 'runtime', 'repository', 'develop', 'abcdef',
    'present', 'clean', '1', '2'
);
"#,
        )
        .unwrap();

        add_workdir_revision_observations(&conn).unwrap();

        let values = conn
            .query_row(
                "SELECT creation_selector, creation_ref, current_selector, current_ref FROM workdir_registry",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            values,
            (
                Some("develop".to_string()),
                Some("abcdef".to_string()),
                None,
                None,
            )
        );
    }

    #[test]
    fn combined_v20_migration_reconciles_ticket_branch_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE workdir_registry (
    selector TEXT,
    resolved_commit TEXT
);
CREATE TABLE ticket_assignment_operations (
    request_fingerprint TEXT
);
"#,
        )
        .unwrap();

        strengthen_ticket_assignment_lifecycle_reservations(&conn).unwrap();

        assert!(column_exists(&conn, "workdir_registry", "creation_selector").unwrap());
        assert!(column_exists(&conn, "workdir_registry", "creation_ref").unwrap());
        assert!(column_exists(&conn, "workdir_registry", "current_selector").unwrap());
        assert!(column_exists(&conn, "workdir_registry", "current_ref").unwrap());
        assert!(
            column_exists(&conn, "ticket_assignment_operations", "request_fingerprint").unwrap()
        );
    }

    #[tokio::test]
    async fn repository_records_round_trip() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 37);
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: None,
            display_name: "Local Dev".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workspace(&workspace).await.unwrap();

        let repository = RepositoryRecord {
            workspace_id: "local-dev".to_string(),
            repository_id: "main".to_string(),
            name: "Yoi".to_string(),
            kind: "git".to_string(),
            provider: Some("git".to_string()),
            uri: ".".to_string(),
            default_ref: Some("HEAD".to_string()),
            auth_ref_kind: None,
            auth_ref_key: None,
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
            owner_account_id: None,
            display_name: "Other Workspace".to_string(),
            state: "active".to_string(),
            created_at: "3".to_string(),
            updated_at: "3".to_string(),
        };
        store.upsert_workspace(&other_workspace).await.unwrap();
        let mut other_repository = repository.clone();
        other_repository.workspace_id = other_workspace.workspace_id.clone();
        other_repository.name = "Other Yoi".to_string();
        other_repository.uri = "/other/yoi".to_string();
        store.upsert_repository(&other_repository).unwrap();

        assert_eq!(
            store.get_repository("local-dev", "main").unwrap(),
            Some(repository)
        );
        assert_eq!(
            store.get_repository("other-workspace", "main").unwrap(),
            Some(other_repository)
        );
    }

    #[tokio::test]
    async fn memory_authority_records_round_trip_and_close_staging() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 37);
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            owner_account_id: None,
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
            owner_account_id: None,
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
                name: "Repository".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                uri: ".".to_string(),
                default_ref: Some("HEAD".to_string()),
                auth_ref_kind: None,
                auth_ref_key: None,
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
        let mut expected_worker = worker.clone();
        expected_worker.updated_at = "5".to_string();

        let workdir = WorkdirRegistryRecord {
            workspace_id: "local-dev".to_string(),
            workdir_id: "0000019a00000000001".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            creation_selector: Some("develop".to_string()),
            creation_ref: Some("abcdef".to_string()),
            current_selector: None,
            current_ref: Some("abcdef".to_string()),
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
            current_selector: Some("feature".to_string()),
            current_ref: Some("123456".to_string()),
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
                owner_account_id: None,
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
        assert_eq!(store.schema_version().await.unwrap(), 37);
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
            owner_account_id: Some(account.account_id.clone()),
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
        store.create_device_login_flow(&flow).unwrap();
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

    #[tokio::test]
    async fn worker_mutation_source_jti_replay_guard_survives_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let store = SqliteWorkspaceStore::open(&path).unwrap();
        store
            .upsert_trusted_runtime(&TrustedRuntimeRecord {
                runtime_id: "runtime-a".to_string(),
                display_name: "Runtime A".to_string(),
                base_url: "https://runtime.invalid".to_string(),
                public_key: "public-key".to_string(),
                created_at: "2026-08-11T00:00:00Z".to_string(),
                updated_at: "2026-08-11T00:00:00Z".to_string(),
                revoked_at: None,
            })
            .unwrap();
        assert!(
            store
                .consume_worker_mutation_source_jti(
                    "runtime-a",
                    "proof-1",
                    2_000,
                    1_000,
                    "2026-08-11T00:00:00Z",
                )
                .await
                .unwrap()
        );
        drop(store);

        let reopened = SqliteWorkspaceStore::open(&path).unwrap();
        assert!(
            !reopened
                .consume_worker_mutation_source_jti(
                    "runtime-a",
                    "proof-1",
                    2_000,
                    1_001,
                    "2026-08-11T00:00:01Z",
                )
                .await
                .unwrap()
        );
    }

    fn table_names(conn: &Connection) -> BTreeSet<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.collect::<rusqlite::Result<BTreeSet<_>>>().unwrap()
    }

    fn assert_columns<const N: usize>(conn: &Connection, table: &str, expected: [&str; N]) {
        let columns = table_columns(conn, table).unwrap();
        let expected = expected.map(str::to_string).to_vec();
        assert_eq!(columns, expected, "unexpected columns for {table}");
    }

    const LEGACY_BOOTSTRAP_SQL: &str = r#"
CREATE TABLE workspaces (
    workspace_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    local_root TEXT NOT NULL,
    record_authority TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE repositories (
    repository_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    local_root TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE ticket_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id)
);
CREATE TABLE objective_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id)
);
CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(run_id) ON DELETE SET NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    created_at TEXT NOT NULL
);
"#;
}
