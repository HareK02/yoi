use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use workspace_api::{
    WorkspaceDeletionBlocker, WorkspaceDeletionBlockerKind, WorkspaceDeletionOperationResponse,
    WorkspaceDeletionPreflightResponse, WorkspaceDeletionRequest, WorkspaceDeletionResourceCounts,
    WorkspaceDeletionState,
};

use crate::store::{SqliteWorkspaceStore, WorkspaceRecord};
use crate::{Error, Result};

const MAX_OPERATION_ID_BYTES: usize = 128;
const CONFIRMATION_PREFIX: &str = "delete ";

#[derive(Debug, Clone)]
pub struct WorkspaceDeletionReservation {
    pub operation: WorkspaceDeletionOperationResponse,
    pub replay: bool,
}

pub trait WorkspaceDeletionStore: Send + Sync {
    fn workspace_deletion_preflight(
        &self,
        actor_account_id: &str,
        workspace_id: &str,
    ) -> Result<WorkspaceDeletionPreflightResponse>;

    fn reserve_workspace_deletion(
        &self,
        actor_account_id: &str,
        workspace_id: &str,
        request: &WorkspaceDeletionRequest,
    ) -> Result<WorkspaceDeletionReservation>;

    fn workspace_deletion_operation(
        &self,
        actor_account_id: &str,
        operation_id: &str,
    ) -> Result<Option<WorkspaceDeletionOperationResponse>>;

    fn release_workspace_assignments_for_deletion(&self, workspace_id: &str) -> Result<u64>;

    fn latest_worker_removal_operation_id(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<Option<String>>;

    fn update_workspace_deletion_operation(
        &self,
        operation_id: &str,
        state: WorkspaceDeletionState,
        child_operation_ids: &[String],
        blockers: &[WorkspaceDeletionBlocker],
        failure_category: Option<&str>,
    ) -> Result<WorkspaceDeletionOperationResponse>;

    fn finalize_workspace_deletion(
        &self,
        operation_id: &str,
    ) -> Result<WorkspaceDeletionOperationResponse>;
}

impl WorkspaceDeletionStore for SqliteWorkspaceStore {
    fn workspace_deletion_preflight(
        &self,
        actor_account_id: &str,
        workspace_id: &str,
    ) -> Result<WorkspaceDeletionPreflightResponse> {
        self.with_conn(|conn| {
            let workspace = owner_workspace(conn, actor_account_id, workspace_id)?;
            let resources = resource_counts(conn, workspace_id)?;
            let accessible: u64 = conn.query_row(
                "SELECT COUNT(*) FROM workspaces WHERE owner_account_id = ?1 AND state = 'active'",
                params![actor_account_id],
                |row| row.get(0),
            )?;
            let mut blockers = Vec::new();
            if accessible <= 1 {
                blockers.push(WorkspaceDeletionBlocker {
                    kind: WorkspaceDeletionBlockerKind::LastAccessibleWorkspace,
                    resource_kind: None,
                    resource_key: None,
                    message: "You cannot delete your last accessible Workspace.".to_string(),
                });
            }
            Ok(WorkspaceDeletionPreflightResponse {
                workspace_id: workspace.workspace_id,
                display_name: workspace.display_name,
                expected_revision: workspace.updated_at,
                can_delete: blockers.is_empty(),
                force_delete_dirty_workdirs_available: true,
                resources,
                blockers,
            })
        })
    }

    fn reserve_workspace_deletion(
        &self,
        actor_account_id: &str,
        workspace_id: &str,
        request: &WorkspaceDeletionRequest,
    ) -> Result<WorkspaceDeletionReservation> {
        validate_operation_id(&request.operation_id)?;
        self.with_transaction(|tx| {
            if let Some(existing) = read_operation(tx, &request.operation_id)? {
                if existing.actor_account_id != actor_account_id {
                    return Err(Error::WorkspacePermissionDenied(
                        "Workspace deletion operation is not owned by the current account".to_string(),
                    ));
                }
                let expected_fingerprint = request_fingerprint(actor_account_id, workspace_id, request);
                if existing.request_fingerprint != expected_fingerprint {
                    return Err(Error::WorkspaceConfigConflict(
                        "Workspace deletion operation_id was reused with different intent".to_string(),
                    ));
                }
                return Ok(WorkspaceDeletionReservation {
                    operation: existing.response,
                    replay: true,
                });
            }

            let workspace = owner_workspace(tx, actor_account_id, workspace_id)?;
            let expected_confirmation = format!("{CONFIRMATION_PREFIX}{}", workspace.display_name);
            if request.confirmation != expected_confirmation {
                return Err(Error::InvalidInput(
                    "confirmation must exactly match the displayed Workspace name".to_string(),
                ));
            }
            if workspace.updated_at != request.expected_revision {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace metadata changed; reload deletion impact before confirming".to_string(),
                ));
            }
            let accessible: u64 = tx.query_row(
                "SELECT COUNT(*) FROM workspaces WHERE owner_account_id = ?1 AND state = 'active'",
                params![actor_account_id],
                |row| row.get(0),
            )?;
            if accessible <= 1 {
                return Err(Error::WorkspaceConfigConflict(
                    "last_accessible_workspace: create or retain another accessible Workspace first"
                        .to_string(),
                ));
            }

            let resources = resource_counts(tx, workspace_id)?;
            let now = Utc::now().to_rfc3339();
            let fingerprint = request_fingerprint(actor_account_id, workspace_id, request);
            tx.execute(
                "INSERT INTO workspace_deletion_operations (
                    operation_id, request_fingerprint, workspace_id, workspace_display_name,
                    workspace_revision, owner_account_id, actor_account_id,
                    force_delete_dirty_workdirs, state, resource_counts_json,
                    child_operation_ids_json, blockers_json, failure_category,
                    created_at, updated_at, completed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', ?9, '[]', '[]', NULL, ?10, ?10, NULL)",
                params![
                    request.operation_id,
                    fingerprint,
                    workspace_id,
                    workspace.display_name,
                    request.expected_revision,
                    workspace.owner_account_id,
                    actor_account_id,
                    request.force_delete_dirty_workdirs as i64,
                    serde_json::to_string(&resources).map_err(|error| Error::Store(error.to_string()))?,
                    now,
                ],
            )?;
            let changed = tx.execute(
                "UPDATE workspaces SET state = 'deleting', updated_at = ?2
                 WHERE workspace_id = ?1 AND updated_at = ?3 AND state = 'active'",
                params![workspace_id, now, request.expected_revision],
            )?;
            if changed != 1 {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace lifecycle changed before deletion could be reserved".to_string(),
                ));
            }
            let operation = read_operation(tx, &request.operation_id)?
                .ok_or_else(|| Error::Store("reserved Workspace deletion operation disappeared".to_string()))?;
            Ok(WorkspaceDeletionReservation {
                operation: operation.response,
                replay: false,
            })
        })
    }

    fn workspace_deletion_operation(
        &self,
        actor_account_id: &str,
        operation_id: &str,
    ) -> Result<Option<WorkspaceDeletionOperationResponse>> {
        self.with_conn(|conn| {
            let Some(operation) = read_operation(conn, operation_id)? else {
                return Ok(None);
            };
            if operation.actor_account_id != actor_account_id {
                return Err(Error::WorkspacePermissionDenied(
                    "Workspace deletion operation is not owned by the current account".to_string(),
                ));
            }
            Ok(Some(operation.response))
        })
    }

    fn release_workspace_assignments_for_deletion(&self, workspace_id: &str) -> Result<u64> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "DELETE FROM ticket_current_worker_assignments WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            u64::try_from(changed)
                .map_err(|_| Error::Store("assignment deletion count overflow".to_string()))
        })
    }

    fn latest_worker_removal_operation_id(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<Option<String>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT operation_id FROM worker_removal_operations
                 WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3
                 ORDER BY created_at DESC LIMIT 1",
                params![workspace_id, runtime_id, worker_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    fn update_workspace_deletion_operation(
        &self,
        operation_id: &str,
        state: WorkspaceDeletionState,
        child_operation_ids: &[String],
        blockers: &[WorkspaceDeletionBlocker],
        failure_category: Option<&str>,
    ) -> Result<WorkspaceDeletionOperationResponse> {
        self.with_transaction(|tx| {
            let now = Utc::now().to_rfc3339();
            let completed_at =
                matches!(state, WorkspaceDeletionState::Succeeded).then_some(now.as_str());
            let changed = tx.execute(
                "UPDATE workspace_deletion_operations
                 SET state = ?2, child_operation_ids_json = ?3, blockers_json = ?4,
                     failure_category = ?5, updated_at = ?6, completed_at = ?7
                 WHERE operation_id = ?1",
                params![
                    operation_id,
                    deletion_state_label(state),
                    serde_json::to_string(child_operation_ids)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    serde_json::to_string(blockers)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    failure_category,
                    now,
                    completed_at,
                ],
            )?;
            if changed != 1 {
                return Err(Error::InvalidInput(
                    "Workspace deletion operation".to_string(),
                ));
            }
            Ok(read_operation(tx, operation_id)?
                .ok_or_else(|| {
                    Error::Store("Workspace deletion operation disappeared".to_string())
                })?
                .response)
        })
    }

    fn finalize_workspace_deletion(
        &self,
        operation_id: &str,
    ) -> Result<WorkspaceDeletionOperationResponse> {
        self.with_transaction(|tx| {
            tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
            let operation = read_operation(tx, operation_id)?
                .ok_or_else(|| Error::InvalidInput("Workspace deletion operation".to_string()))?;
            if operation.response.state == WorkspaceDeletionState::Succeeded {
                return Ok(operation.response);
            }
            let workspace_id = operation.response.workspace_id.clone();

            let mut scoped_tables = Vec::new();
            let mut statement = tx.prepare(
                "SELECT m.name
                 FROM sqlite_master m
                 WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'
                 ORDER BY m.name",
            )?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(statement);
            for table in names {
                if table == "workspaces" || table == "workspace_deletion_operations" {
                    continue;
                }
                let escaped = table.replace('"', "\"\"");
                let mut info = tx.prepare(&format!("PRAGMA table_info(\"{escaped}\")"))?;
                let columns = info
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<std::result::Result<BTreeSet<_>, _>>()?;
                if columns.contains("workspace_id") {
                    scoped_tables.push(escaped);
                }
            }
            for table in scoped_tables {
                tx.execute(
                    &format!("DELETE FROM \"{table}\" WHERE workspace_id = ?1"),
                    params![workspace_id],
                )?;
            }
            let deleted = tx.execute(
                "DELETE FROM workspaces WHERE workspace_id = ?1",
                params![workspace_id],
            )?;
            if deleted != 1 {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace disappeared before deletion finalized".to_string(),
                ));
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE workspace_deletion_operations
                 SET state = 'succeeded', blockers_json = '[]', failure_category = NULL,
                     updated_at = ?2, completed_at = ?2
                 WHERE operation_id = ?1",
                params![operation_id, now],
            )?;
            let fk_failures: u64 =
                tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })?;
            if fk_failures != 0 {
                return Err(Error::Store(
                    "foreign key check failed while finalizing Workspace deletion".to_string(),
                ));
            }
            Ok(read_operation(tx, operation_id)?
                .ok_or_else(|| {
                    Error::Store("completed Workspace deletion operation disappeared".to_string())
                })?
                .response)
        })
    }
}

#[derive(Debug)]
struct StoredOperation {
    request_fingerprint: String,
    actor_account_id: String,
    response: WorkspaceDeletionOperationResponse,
}

fn read_operation(
    conn: &rusqlite::Connection,
    operation_id: &str,
) -> Result<Option<StoredOperation>> {
    conn.query_row(
        "SELECT request_fingerprint, actor_account_id, workspace_id, workspace_display_name,
                state, force_delete_dirty_workdirs, resource_counts_json,
                child_operation_ids_json, blockers_json, failure_category,
                created_at, updated_at, completed_at
         FROM workspace_deletion_operations WHERE operation_id = ?1",
        params![operation_id],
        |row| {
            let state: String = row.get(4)?;
            let resource_counts_json: String = row.get(6)?;
            let child_operation_ids_json: String = row.get(7)?;
            let blockers_json: String = row.get(8)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                state,
                row.get::<_, bool>(5)?,
                resource_counts_json,
                child_operation_ids_json,
                blockers_json,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
            ))
        },
    )
    .optional()?
    .map(
        |(
            fingerprint,
            actor,
            workspace_id,
            display_name,
            state,
            force,
            resources,
            children,
            blockers,
            failure,
            created_at,
            updated_at,
            completed_at,
        )| {
            Ok(StoredOperation {
                request_fingerprint: fingerprint,
                actor_account_id: actor,
                response: WorkspaceDeletionOperationResponse {
                    operation_id: operation_id.to_string(),
                    workspace_id,
                    display_name,
                    state: parse_deletion_state(&state)?,
                    force_delete_dirty_workdirs: force,
                    resources: serde_json::from_str(&resources)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    child_operation_ids: serde_json::from_str(&children)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    blockers: serde_json::from_str(&blockers)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    failure_category: failure,
                    created_at,
                    updated_at,
                    completed_at,
                },
            })
        },
    )
    .transpose()
}

fn owner_workspace(
    conn: &rusqlite::Connection,
    actor_account_id: &str,
    workspace_id: &str,
) -> Result<WorkspaceRecord> {
    let workspace = conn
        .query_row(
            "SELECT workspace_id, owner_account_id, display_name, state, created_at, updated_at
             FROM workspaces WHERE workspace_id = ?1",
            params![workspace_id],
            |row| {
                Ok(WorkspaceRecord {
                    workspace_id: row.get(0)?,
                    owner_account_id: row.get(1)?,
                    display_name: row.get(2)?,
                    state: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::InvalidInput("Workspace".to_string()))?;
    if workspace.owner_account_id != actor_account_id {
        return Err(Error::WorkspacePermissionDenied(
            "Workspace owner permission is required".to_string(),
        ));
    }
    Ok(workspace)
}

fn resource_counts(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<WorkspaceDeletionResourceCounts> {
    Ok(WorkspaceDeletionResourceCounts {
        workers: table_count(conn, "worker_registry", workspace_id)?,
        workdirs: table_count(conn, "workdir_registry", workspace_id)?,
        repositories: table_count(conn, "repositories", workspace_id)?,
        runtime_bindings: table_count(conn, "workspace_runtime_bindings", workspace_id)?,
        secrets: table_count(conn, "server_secret_versions", workspace_id)?,
        artifacts: table_count(conn, "artifacts", workspace_id)?,
    })
}

fn table_count(conn: &rusqlite::Connection, table: &str, workspace_id: &str) -> Result<u64> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM \"{table}\" WHERE workspace_id = ?1"),
        params![workspace_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn validate_operation_id(operation_id: &str) -> Result<()> {
    if operation_id.is_empty()
        || operation_id.len() > MAX_OPERATION_ID_BYTES
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::InvalidInput("operation_id is invalid".to_string()));
    }
    Ok(())
}

fn request_fingerprint(
    actor_account_id: &str,
    workspace_id: &str,
    request: &WorkspaceDeletionRequest,
) -> String {
    let canonical = format!(
        "workspace-delete-v1\0{actor_account_id}\0{workspace_id}\0{}\0{}\0{}",
        request.expected_revision, request.confirmation, request.force_delete_dirty_workdirs
    );
    encode_hex(&Sha256::digest(canonical.as_bytes()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn deletion_state_label(state: WorkspaceDeletionState) -> &'static str {
    match state {
        WorkspaceDeletionState::Queued => "queued",
        WorkspaceDeletionState::Running => "running",
        WorkspaceDeletionState::Blocked => "blocked",
        WorkspaceDeletionState::Failed => "failed",
        WorkspaceDeletionState::Succeeded => "succeeded",
    }
}

fn parse_deletion_state(value: &str) -> Result<WorkspaceDeletionState> {
    match value {
        "queued" => Ok(WorkspaceDeletionState::Queued),
        "running" => Ok(WorkspaceDeletionState::Running),
        "blocked" => Ok(WorkspaceDeletionState::Blocked),
        "failed" => Ok(WorkspaceDeletionState::Failed),
        "succeeded" => Ok(WorkspaceDeletionState::Succeeded),
        other => Err(Error::Store(format!(
            "invalid Workspace deletion state `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> (SqliteWorkspaceStore, String, String) {
        let dir = tempdir().expect("tempdir");
        let store = SqliteWorkspaceStore::open(dir.path().join("server.db")).expect("store");
        store.with_conn(|conn| {
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO accounts (account_id, kind, handle, display_name, created_at, updated_at)
                 VALUES ('owner', 'user', 'owner', 'Owner', ?1, ?1)",
                params![now],
            )?;
            for (id, name) in [("workspace-a", "Alpha"), ("workspace-b", "Beta")] {
                conn.execute(
                    "INSERT INTO workspaces (workspace_id, owner_account_id, display_name, state, created_at, updated_at)
                     VALUES (?1, 'owner', ?2, 'active', ?3, ?3)",
                    params![id, name, now],
                )?;
            }
            Ok(())
        }).expect("fixtures");
        (store, "owner".to_string(), "workspace-a".to_string())
    }

    #[test]
    fn deletion_is_idempotent_and_removes_workspace_scoped_rows() {
        let (store, owner, workspace_id) = setup();
        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        let request = WorkspaceDeletionRequest {
            operation_id: "delete-workspace-a".to_string(),
            expected_revision: preflight.expected_revision,
            confirmation: "delete Alpha".to_string(),
            force_delete_dirty_workdirs: false,
        };
        let first = store
            .reserve_workspace_deletion(&owner, &workspace_id, &request)
            .expect("reserve");
        assert!(!first.replay);
        let replay = store
            .reserve_workspace_deletion(&owner, &workspace_id, &request)
            .expect("replay");
        assert!(replay.replay);
        let completed = store
            .finalize_workspace_deletion(&request.operation_id)
            .expect("finalize");
        assert_eq!(completed.state, WorkspaceDeletionState::Succeeded);
        let replayed = store
            .finalize_workspace_deletion(&request.operation_id)
            .expect("finalize replay");
        assert_eq!(completed, replayed);
        let workspace_count: u64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM workspaces WHERE workspace_id = ?1",
                    params![workspace_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("read");
        assert_eq!(workspace_count, 0);
    }

    #[test]
    fn owner_confirmation_and_revision_are_required_before_reservation() {
        let (store, owner, workspace_id) = setup();
        store.with_conn(|conn| {
            conn.execute(
                "INSERT INTO accounts (account_id, kind, handle, display_name, created_at, updated_at)
                 VALUES ('other', 'user', 'other', 'Other', '1', '1')",
                [],
            )?;
            Ok(())
        }).expect("other account");
        assert!(matches!(
            store.workspace_deletion_preflight("other", &workspace_id),
            Err(Error::WorkspacePermissionDenied(_))
        ));

        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        let mut request = WorkspaceDeletionRequest {
            operation_id: "delete-alpha-guarded".to_string(),
            expected_revision: "stale".to_string(),
            confirmation: "delete Alpha".to_string(),
            force_delete_dirty_workdirs: false,
        };
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        request.expected_revision = preflight.expected_revision;
        request.confirmation = "Alpha".to_string();
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn last_accessible_workspace_and_revision_conflicts_fail_closed() {
        let (store, owner, workspace_id) = setup();
        let other = "workspace-b";
        let preflight = store
            .workspace_deletion_preflight(&owner, other)
            .expect("preflight");
        store
            .finalize_workspace_deletion(
                &store
                    .reserve_workspace_deletion(
                        &owner,
                        other,
                        &WorkspaceDeletionRequest {
                            operation_id: "delete-beta".to_string(),
                            expected_revision: preflight.expected_revision,
                            confirmation: "delete Beta".to_string(),
                            force_delete_dirty_workdirs: false,
                        },
                    )
                    .expect("reserve")
                    .operation
                    .operation_id,
            )
            .expect("delete beta");
        let blocked = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("blocked");
        assert!(!blocked.can_delete);
        assert_eq!(
            blocked.blockers[0].kind,
            WorkspaceDeletionBlockerKind::LastAccessibleWorkspace
        );
    }
}
