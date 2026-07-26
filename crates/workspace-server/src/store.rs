use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

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
    pub runtime_id: String,
    pub runtime_worker_id: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRegistryRecord {
    pub workspace_id: String,
    pub workdir_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub selector: Option<String>,
    pub resolved_commit: Option<String>,
    pub materialization_status: String,
    pub cleanliness: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkdirLinkRecord {
    pub workspace_id: String,
    pub runtime_id: String,
    pub runtime_worker_id: u64,
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

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;
    fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>>;
    fn upsert_repository(&self, record: &RepositoryRecord) -> Result<()>;
    fn list_repositories(&self, workspace_id: &str) -> Result<Vec<RepositoryRecord>>;

    fn upsert_objective(&self, record: &ObjectiveRecord) -> Result<()>;
    fn list_objectives(&self, workspace_id: &str, limit: usize) -> Result<Vec<ObjectiveRecord>>;
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
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Option<WorkerRegistryRecord>>;
    fn list_worker_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkerRegistryRecord>>;
    fn update_worker_retention(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool>;
    fn delete_worker_registry(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<bool>;

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

    fn upsert_worker_workdir_link(&self, record: &WorkerWorkdirLinkRecord) -> Result<()>;
    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
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
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Store("sqlite connection lock poisoned".to_string()))?;
        f(&conn)
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
        })
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
                ON CONFLICT(repository_id) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
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
            conn.execute(
                r#"INSERT INTO worker_registry (
                    workspace_id, runtime_id, runtime_worker_id, display_name, profile,
                    retention_state, transcript_ref, session_ref, summary_ref,
                    diagnostics_ref, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(workspace_id, runtime_id, runtime_worker_id) DO UPDATE SET
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
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.runtime_id,
                    record.runtime_worker_id,
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
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Option<WorkerRegistryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                worker_registry_select_sql(
                    "WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3",
                )
                .as_str(),
                params![workspace_id, runtime_id, runtime_worker_id],
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
        runtime_id: &str,
        runtime_worker_id: u64,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE worker_registry
                   SET retention_state = ?4, updated_at = ?5
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3"#,
                params![
                    workspace_id,
                    runtime_id,
                    runtime_worker_id,
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
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "DELETE FROM worker_registry WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3",
                params![workspace_id, runtime_id, runtime_worker_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn upsert_workdir_registry(&self, record: &WorkdirRegistryRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workdir_registry (
                    workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
                    materialization_status, cleanliness, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(workspace_id, workdir_id) DO UPDATE SET
                    runtime_id = excluded.runtime_id,
                    repository_id = excluded.repository_id,
                    selector = excluded.selector,
                    resolved_commit = excluded.resolved_commit,
                    materialization_status = excluded.materialization_status,
                    cleanliness = excluded.cleanliness,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.workdir_id,
                    record.runtime_id,
                    record.repository_id,
                    record.selector,
                    record.resolved_commit,
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
            let changed = conn.execute(
                "DELETE FROM workdir_registry WHERE workspace_id = ?1 AND workdir_id = ?2",
                params![workspace_id, workdir_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn upsert_worker_workdir_link(&self, record: &WorkerWorkdirLinkRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO worker_workdir_links (
                    workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(workspace_id, runtime_id, runtime_worker_id, workdir_id, role) DO UPDATE SET
                    linked_at = excluded.linked_at,
                    unlinked_at = excluded.unlinked_at"#,
                params![
                    record.workspace_id,
                    record.runtime_id,
                    record.runtime_worker_id,
                    record.workdir_id,
                    record.role,
                    record.linked_at,
                    record.unlinked_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
                   FROM worker_workdir_links
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3 AND unlinked_at IS NULL
                   ORDER BY linked_at DESC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, runtime_id, runtime_worker_id],
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
                r#"SELECT workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
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
        runtime_id: row.get(1)?,
        runtime_worker_id: row.get(2)?,
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
        "SELECT workspace_id, runtime_id, runtime_worker_id, display_name, profile, \
         retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref, \
         created_at, updated_at FROM worker_registry {where_clause}"
    )
}

fn read_worker_registry_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRegistryRecord> {
    Ok(WorkerRegistryRecord {
        workspace_id: row.get(0)?,
        runtime_id: row.get(1)?,
        runtime_worker_id: row.get(2)?,
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

fn workdir_registry_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit, \
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
        selector: row.get(4)?,
        resolved_commit: row.get(5)?,
        materialization_status: row.get(6)?,
        cleanliness: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
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

fn configure_sqlite(conn: &Connection) -> Result<()> {
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

fn current_schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM __yoi_schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

fn apply_migrations(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current)
    {
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

CREATE TABLE IF NOT EXISTS tickets (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    priority TEXT,
    assignee_kind TEXT,
    assignee_key TEXT,
    assignee_display TEXT,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    resolution_event_id TEXT
);

CREATE TABLE IF NOT EXISTS ticket_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    event_id TEXT PRIMARY KEY,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    event_seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    activity_id TEXT,
    author_kind TEXT NOT NULL,
    author_key TEXT NOT NULL,
    author_display TEXT NOT NULL,
    author_source_kind TEXT,
    author_source_key TEXT,
    created_at TEXT NOT NULL,
    body_md TEXT,
    subject_kind TEXT,
    subject_id TEXT,
    previous_state TEXT,
    new_state TEXT,
    status TEXT,
    artifact_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    host_ref_kind TEXT,
    host_ref_key TEXT,
    host_display TEXT,
    repository_id TEXT,
    caused_by_event_id TEXT,
    UNIQUE (ticket_id, event_seq)
);

CREATE TABLE IF NOT EXISTS ticket_relations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    source_ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    target_ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_key TEXT NOT NULL,
    author_display TEXT NOT NULL,
    author_source_kind TEXT,
    author_source_key TEXT,
    note TEXT,
    PRIMARY KEY (source_ticket_id, target_ticket_id, kind)
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

CREATE TABLE IF NOT EXISTS ticket_targets (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    target_id TEXT NOT NULL,
    repository_id TEXT NOT NULL REFERENCES repositories(repository_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    intent TEXT NOT NULL,
    ref_selector TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (ticket_id, target_id)
);

CREATE TABLE IF NOT EXISTS ticket_target_paths (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (ticket_id, target_id, path),
    FOREIGN KEY (ticket_id, target_id) REFERENCES ticket_targets(ticket_id, target_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ticket_worker_links (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    worker_ref_kind TEXT NOT NULL,
    worker_ref_key TEXT NOT NULL,
    worker_display TEXT,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    activity_id TEXT,
    assigned_at TEXT,
    released_at TEXT,
    last_event_id TEXT,
    PRIMARY KEY (ticket_id, worker_ref_kind, worker_ref_key, role)
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

    #[tokio::test]
    async fn migrates_sqlite_and_preserves_workspace_record() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("control-plane.sqlite");
        let store = SqliteWorkspaceStore::open(&db).unwrap();

        assert_eq!(store.schema_version().await.unwrap(), 11);

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
        assert_eq!(reopened.schema_version().await.unwrap(), 11);
        assert_eq!(
            reopened.get_workspace("local-dev").await.unwrap(),
            Some(record)
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
            "tickets",
            "ticket_events",
            "ticket_relations",
            "objectives",
            "objective_ticket_links",
            "objective_resources",
            "memory_staging_records",
            "workspace_memory_documents",
            "memory_staging_resolutions",
            "repositories",
            "ticket_targets",
            "ticket_target_paths",
            "ticket_worker_links",
            "artifacts",
            "audit_events",
            "worker_registry",
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
            "ticket_events",
            [
                "workspace_id",
                "event_id",
                "ticket_id",
                "event_seq",
                "kind",
                "activity_id",
                "author_kind",
                "author_key",
                "author_display",
                "author_source_kind",
                "author_source_key",
                "created_at",
                "body_md",
                "subject_kind",
                "subject_id",
                "previous_state",
                "new_state",
                "status",
                "artifact_id",
                "worker_ref_kind",
                "worker_ref_key",
                "worker_display",
                "host_ref_kind",
                "host_ref_key",
                "host_display",
                "repository_id",
                "caused_by_event_id",
            ],
        );
        assert_columns(
            &conn,
            "worker_registry",
            [
                "workspace_id",
                "runtime_id",
                "runtime_worker_id",
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

        for table in ["workspaces", "repositories", "ticket_events", "artifacts"] {
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
        assert_eq!(store.schema_version().await.unwrap(), 11);

        store
            .with_conn(|conn| {
                let tables = table_names(conn);
                for expected in [
                    "workspaces",
                    "repositories",
                    "tickets",
                    "ticket_events",
                    "ticket_worker_links",
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
                for forbidden in ["runs", "hosts", "workers", "actors", "validation_results"] {
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

    #[tokio::test]
    async fn repository_records_round_trip() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 11);
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
            store.list_repositories("local-dev").unwrap(),
            vec![repository]
        );
        assert_eq!(
            store.list_repositories("other-workspace").unwrap(),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn memory_authority_records_round_trip_and_close_staging() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 11);
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

        let worker = WorkerRegistryRecord {
            workspace_id: "local-dev".to_string(),
            runtime_id: "embedded".to_string(),
            runtime_worker_id: 1,
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
            selector: Some("develop".to_string()),
            resolved_commit: Some("abcdef".to_string()),
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
            selector: Some("feature".to_string()),
            resolved_commit: Some("123456".to_string()),
            materialization_status: "present".to_string(),
            cleanliness: "unknown".to_string(),
            created_at: "3".to_string(),
            updated_at: "4".to_string(),
        };
        store.upsert_workdir_registry(&unmanaged_workdir).unwrap();

        let link = WorkerWorkdirLinkRecord {
            workspace_id: "local-dev".to_string(),
            runtime_id: worker.runtime_id.clone(),
            runtime_worker_id: worker.runtime_worker_id.clone(),
            workdir_id: workdir.workdir_id.clone(),
            role: "primary_cwd".to_string(),
            linked_at: "4".to_string(),
            unlinked_at: None,
        };
        store.upsert_worker_workdir_link(&link).unwrap();

        assert_eq!(
            store
                .get_worker_registry("local-dev", "embedded", 1)
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
                .list_worker_workdir_links("local-dev", "embedded", 1)
                .unwrap(),
            vec![link]
        );
    }

    #[tokio::test]
    async fn account_and_login_records_round_trip() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 11);
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
