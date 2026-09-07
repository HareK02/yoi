use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use workspace_api::{
    WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES, WORKSPACE_DELETION_MAX_BLOCKERS,
    WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS, WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES,
    WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES, WORKSPACE_DELETION_MAX_REVISION_BYTES,
    WorkspaceDeletionBlocker, WorkspaceDeletionBlockerKind, WorkspaceDeletionOperationResponse,
    WorkspaceDeletionPreflightResponse, WorkspaceDeletionRequest, WorkspaceDeletionResourceCounts,
    WorkspaceDeletionState,
};

use crate::store::{SqliteWorkspaceStore, WorkspaceRecord};
use crate::{Error, Result};

/// Explicit domain-owned purge inventory. The deletion operation tombstone is intentionally
/// excluded so retries and audit remain available after the Workspace row is gone.
const WORKSPACE_DELETION_PURGE_TABLES: &[&str] = &[
    "artifacts",
    "audit_events",
    "flow_source_revisions",
    "flow_sources",
    "memory_staging_records",
    "memory_staging_resolutions",
    "merge_request_review_grants",
    "merge_request_reviewer_child_sessions",
    "merge_request_thread_events",
    "merge_request_ticket_relations",
    "merge_requests",
    "objective_events",
    "objective_resources",
    "objective_ticket_links",
    "objectives",
    "repositories",
    "repository_secret_audit_events",
    "repository_secret_operations",
    "repository_ssh_credential_revisions",
    "repository_ssh_credentials",
    "repository_ssh_host_trust_revisions",
    "repository_ssh_host_trusts",
    "server_secret_versions",
    "ticket_assignment_operations",
    "ticket_assignment_ticket_tombstones",
    "ticket_assignment_worker_tombstones",
    "ticket_current_worker_assignments",
    "ticket_worker_assignment_events",
    "ticket_worker_assignments",
    "typed_ticket_artifacts",
    "typed_ticket_event_attributes",
    "typed_ticket_event_references",
    "typed_ticket_events",
    "typed_ticket_labels",
    "typed_ticket_orchestration_plans",
    "typed_ticket_raw_frontmatter",
    "typed_ticket_relations",
    "typed_ticket_risk_flags",
    "typed_tickets",
    "workdir_create_operations",
    "workdir_registry",
    "workdir_removal_operations",
    "worker_control_grants",
    "worker_create_reservations",
    "worker_diagnostics_archives",
    "worker_mutation_source_proof_jtis",
    "worker_orphan_diagnostics",
    "worker_registry",
    "worker_removal_operations",
    "worker_retention_audit_events",
    "worker_session_archives",
    "worker_tombstones",
    "worker_workdir_attachment_reservations",
    "worker_workdir_links",
    "workspace_config_entries",
    "workspace_config_tree_revisions",
    "workspace_config_trees",
    "workspace_create_operations",
    "workspace_memory_documents",
    "workspace_memory_settings",
    "workspace_resource_key_counters",
    "workspace_resource_keys",
    "workspace_runtime_binding_audit",
    "workspace_runtime_bindings",
    "workspace_runtime_verifications",
    "workspace_signing_identities",
    "workspace_signing_identity_audit",
    "workspace_signing_identity_provisioning_operations",
    "workspace_worker_retention_policies",
    "workspace_worker_retention_policy_revisions",
];

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

    fn workspace_deletion_operation_for_recovery(
        &self,
        operation_id: &str,
    ) -> Result<Option<WorkspaceDeletionOperationResponse>>;

    fn resumable_workspace_deletion_operation_ids(&self) -> Result<Vec<String>>;

    fn append_workspace_deletion_child_operation(
        &self,
        operation_id: &str,
        child_operation_id: &str,
    ) -> Result<WorkspaceDeletionOperationResponse>;

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
                "SELECT COUNT(*) FROM workspaces WHERE owner_account_id = ?1",
                params![actor_account_id],
                |row| row.get(0),
            )?;
            let mut blockers = workspace_database_blockers(conn, workspace_id)?;
            if accessible <= 1 {
                blockers.push(WorkspaceDeletionBlocker {
                    kind: WorkspaceDeletionBlockerKind::LastAccessibleWorkspace,
                    resource_kind: None,
                    resource_key: None,
                    message: "You cannot delete your last accessible Workspace.".to_string(),
                });
            }
            if resources.workers.saturating_add(resources.workdirs)
                > WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS as u64
            {
                blockers.push(WorkspaceDeletionBlocker {
                    kind: WorkspaceDeletionBlockerKind::CleanupUnavailable,
                    resource_kind: None,
                    resource_key: None,
                    message:
                        "Workspace cleanup exceeds the supported durable child-operation bound."
                            .to_string(),
                });
            }
            bound_workspace_deletion_blockers(&mut blockers);
            Ok(WorkspaceDeletionPreflightResponse {
                workspace_id: workspace.workspace_id,
                display_name: workspace.display_name,
                expected_revision: workspace.updated_at,
                can_delete: blockers.is_empty(),
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
        if request.expected_revision.len() > WORKSPACE_DELETION_MAX_REVISION_BYTES
            || request.confirmation.len() > workspace_api::WORKSPACE_DELETION_MAX_CONFIRMATION_BYTES
        {
            return Err(Error::InvalidInput(
                "Workspace deletion request exceeds bounded field limits".to_string(),
            ));
        }
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
            if request.confirmation != workspace.display_name {
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
                "SELECT COUNT(*) FROM workspaces WHERE owner_account_id = ?1",
                params![actor_account_id],
                |row| row.get(0),
            )?;
            if accessible <= 1 {
                return Err(Error::WorkspaceConfigConflict(
                    "last_accessible_workspace: create or retain another accessible Workspace first"
                        .to_string(),
                ));
            }
            if !workspace_database_blockers(tx, workspace_id)?.is_empty() {
                return Err(Error::WorkspaceConfigConflict(
                    "Workspace deletion preflight changed; reload current blockers".to_string(),
                ));
            }

            let resources = resource_counts(tx, workspace_id)?;
            let now = Utc::now().to_rfc3339();
            let fingerprint = request_fingerprint(actor_account_id, workspace_id, request);
            tx.execute(
                "INSERT INTO workspace_deletion_operations (
                    operation_id, request_fingerprint, workspace_id, workspace_display_name,
                    workspace_revision, owner_account_id, actor_account_id,
                    state, resource_counts_json,
                    child_operation_ids_json, blockers_json, failure_category,
                    created_at, updated_at, completed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, '[]', '[]', NULL, ?9, ?9, NULL)",
                params![
                    request.operation_id,
                    fingerprint,
                    workspace_id,
                    workspace.display_name,
                    request.expected_revision,
                    workspace.owner_account_id,
                    actor_account_id,
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

    fn workspace_deletion_operation_for_recovery(
        &self,
        operation_id: &str,
    ) -> Result<Option<WorkspaceDeletionOperationResponse>> {
        self.with_conn(|conn| Ok(read_operation(conn, operation_id)?.map(|stored| stored.response)))
    }

    fn resumable_workspace_deletion_operation_ids(&self) -> Result<Vec<String>> {
        self.with_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT operation_id FROM workspace_deletion_operations
                 WHERE state IN ('queued', 'running') ORDER BY created_at, operation_id",
            )?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
    }

    fn append_workspace_deletion_child_operation(
        &self,
        operation_id: &str,
        child_operation_id: &str,
    ) -> Result<WorkspaceDeletionOperationResponse> {
        validate_operation_id(child_operation_id)?;
        self.with_transaction(|tx| {
            let operation = read_operation(tx, operation_id)?
                .ok_or_else(|| Error::InvalidInput("Workspace deletion operation".to_string()))?
                .response;
            if !operation
                .child_operation_ids
                .iter()
                .any(|existing| existing == child_operation_id)
            {
                let mut child_operation_ids = operation.child_operation_ids;
                child_operation_ids.push(child_operation_id.to_string());
                validate_operation_projection(&child_operation_ids, &operation.blockers)?;
                let now = Utc::now().to_rfc3339();
                tx.execute(
                    "UPDATE workspace_deletion_operations
                     SET child_operation_ids_json = ?2, updated_at = ?3
                     WHERE operation_id = ?1",
                    params![
                        operation_id,
                        serde_json::to_string(&child_operation_ids)
                            .map_err(|error| Error::Store(error.to_string()))?,
                        now,
                    ],
                )?;
            }
            Ok(read_operation(tx, operation_id)?
                .ok_or_else(|| {
                    Error::Store("Workspace deletion operation disappeared".to_string())
                })?
                .response)
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
        validate_operation_projection(child_operation_ids, blockers)?;
        self.with_transaction(|tx| {
            let current = read_operation(tx, operation_id)?
                .ok_or_else(|| Error::InvalidInput("Workspace deletion operation".to_string()))?
                .response;
            let mut merged_child_operation_ids = current.child_operation_ids;
            for child_operation_id in child_operation_ids {
                if !merged_child_operation_ids
                    .iter()
                    .any(|existing| existing == child_operation_id)
                {
                    merged_child_operation_ids.push(child_operation_id.clone());
                }
            }
            validate_operation_projection(&merged_child_operation_ids, blockers)?;
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
                    serde_json::to_string(&merged_child_operation_ids)
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

            for table in WORKSPACE_DELETION_PURGE_TABLES {
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
                state, resource_counts_json, child_operation_ids_json,
                blockers_json, failure_category, created_at, updated_at, completed_at
         FROM workspace_deletion_operations WHERE operation_id = ?1",
        params![operation_id],
        |row| {
            let state: String = row.get(4)?;
            let resource_counts_json: String = row.get(5)?;
            let child_operation_ids_json: String = row.get(6)?;
            let blockers_json: String = row.get(7)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                state,
                resource_counts_json,
                child_operation_ids_json,
                blockers_json,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
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

pub(crate) fn bound_workspace_deletion_blockers(blockers: &mut Vec<WorkspaceDeletionBlocker>) {
    for blocker in blockers.iter_mut() {
        blocker.resource_kind = blocker
            .resource_kind
            .take()
            .map(|value| truncate_utf8(value, WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES));
        blocker.resource_key = blocker
            .resource_key
            .take()
            .map(|value| truncate_utf8(value, WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES));
        blocker.message = truncate_utf8(
            std::mem::take(&mut blocker.message),
            WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES,
        );
    }
    if blockers.len() > WORKSPACE_DELETION_MAX_BLOCKERS {
        blockers.truncate(WORKSPACE_DELETION_MAX_BLOCKERS - 1);
        blockers.push(WorkspaceDeletionBlocker {
            kind: WorkspaceDeletionBlockerKind::CleanupUnavailable,
            resource_kind: None,
            resource_key: None,
            message: "Additional deletion blockers exist; reduce Workspace resources and run preflight again."
                .to_string(),
        });
    }
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn workspace_database_blockers(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<Vec<WorkspaceDeletionBlocker>> {
    let mut blockers = Vec::new();
    for (sql, kind, resource_kind, message) in [
        (
            "SELECT workdir_id FROM worker_workdir_links WHERE workspace_id = ?1 AND unlinked_at IS NULL",
            WorkspaceDeletionBlockerKind::WorkdirRemovalBlocked,
            "workdir",
            "Release this active Worker–Workdir attachment before deleting the Workspace.",
        ),
        (
            "SELECT workdir_id FROM worker_workdir_attachment_reservations WHERE workspace_id = ?1",
            WorkspaceDeletionBlockerKind::WorkdirRemovalBlocked,
            "workdir",
            "Wait for or cancel this pending Workdir attachment reservation.",
        ),
        (
            "SELECT display_name FROM worker_registry WHERE workspace_id = ?1 AND retention_state = 'pinned'",
            WorkspaceDeletionBlockerKind::RetentionHold,
            "worker",
            "Remove this Worker retention pin before deleting the Workspace.",
        ),
        (
            "SELECT workdir_id FROM workdir_registry WHERE workspace_id = ?1 AND COALESCE(cleanliness, '') != 'clean'",
            WorkspaceDeletionBlockerKind::DirtyWorkdir,
            "workdir",
            "Clean this Workdir and refresh unknown cleanliness before deleting the Workspace.",
        ),
    ] {
        for resource_key in query_resource_keys(conn, sql, workspace_id)? {
            blockers.push(WorkspaceDeletionBlocker {
                kind,
                resource_kind: Some(resource_kind.to_string()),
                resource_key: Some(resource_key),
                message: message.to_string(),
            });
        }
    }

    for (sql, kind, resource_kind, message) in [
        (
            "SELECT COUNT(*) FROM ticket_current_worker_assignments WHERE workspace_id = ?1",
            WorkspaceDeletionBlockerKind::WorkerRemovalBlocked,
            "ticket",
            "Remove current Ticket assignments before deleting the Workspace.",
        ),
        (
            "SELECT COUNT(*)
             FROM worker_create_reservations reservation
             WHERE reservation.workspace_id = ?1
               AND (
                    reservation.state = 'reserved'
                    OR (
                        reservation.state = 'created'
                        AND NOT EXISTS (
                            SELECT 1 FROM worker_registry worker
                            WHERE worker.workspace_id = reservation.workspace_id
                              AND worker.runtime_id = reservation.runtime_id
                              AND worker.worker_id = reservation.worker_id
                        )
                    )
               )",
            WorkspaceDeletionBlockerKind::CleanupUnavailable,
            "worker",
            "Reconcile pending or incompletely finalized Worker creation reservations.",
        ),
        (
            "SELECT COUNT(*) FROM worker_removal_operations WHERE workspace_id = ?1 AND state IN ('planned', 'blocked', 'executing', 'failed', 'stale')",
            WorkspaceDeletionBlockerKind::CleanupUnavailable,
            "worker",
            "Resolve pending or failed Worker removal operations first.",
        ),
        (
            "SELECT COUNT(*) FROM workdir_removal_operations WHERE workspace_id = ?1 AND state IN ('pending', 'failed')",
            WorkspaceDeletionBlockerKind::CleanupUnavailable,
            "workdir",
            "Resolve pending or failed Workdir removal operations first.",
        ),
        (
            "SELECT COUNT(*) FROM workdir_create_operations WHERE workspace_id = ?1 AND state = 'pending'",
            WorkspaceDeletionBlockerKind::CleanupUnavailable,
            "workdir",
            "Wait for pending Workdir creation operations to finish.",
        ),
    ] {
        let count: u64 = conn.query_row(sql, params![workspace_id], |row| row.get(0))?;
        if count != 0 {
            blockers.push(WorkspaceDeletionBlocker {
                kind,
                resource_kind: Some(resource_kind.to_string()),
                resource_key: None,
                message: format!("{message} ({count})"),
            });
        }
    }
    Ok(blockers)
}

fn query_resource_keys(
    conn: &rusqlite::Connection,
    sql: &str,
    workspace_id: &str,
) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    statement
        .query_map(params![workspace_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
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

fn validate_operation_projection(
    child_operation_ids: &[String],
    blockers: &[WorkspaceDeletionBlocker],
) -> Result<()> {
    let invalid_child_ids = child_operation_ids.len() > WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS
        || child_operation_ids
            .iter()
            .any(|value| value.len() > WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES);
    let invalid_blockers =
        blockers.len() > WORKSPACE_DELETION_MAX_BLOCKERS
            || blockers.iter().any(|blocker| {
                blocker.message.len() > WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES
                    || blocker.resource_kind.as_ref().is_some_and(|value| {
                        value.len() > WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES
                    })
                    || blocker.resource_key.as_ref().is_some_and(|value| {
                        value.len() > WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES
                    })
            });
    if invalid_child_ids || invalid_blockers {
        return Err(Error::Store(
            "Workspace deletion operation projection exceeds bounded limits".to_string(),
        ));
    }
    Ok(())
}

fn validate_operation_id(operation_id: &str) -> Result<()> {
    if operation_id.is_empty()
        || operation_id.len() > WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES
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
        request.operation_id, request.expected_revision, request.confirmation
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
    use crate::store::ControlPlaneStore;
    use std::collections::BTreeSet;
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
    fn explicit_purge_inventory_covers_every_workspace_scoped_table() {
        let (store, _, _) = setup();
        store
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT name FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )?;
                let tables = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                drop(statement);
                let mut scoped = BTreeSet::new();
                for table in tables {
                    let escaped = table.replace('"', "\"\"");
                    let mut info = conn.prepare(&format!("PRAGMA table_info(\"{escaped}\")"))?;
                    let columns = info
                        .query_map([], |row| row.get::<_, String>(1))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    if columns.iter().any(|column| column == "workspace_id") {
                        scoped.insert(table);
                    }
                }
                let expected = WORKSPACE_DELETION_PURGE_TABLES
                    .iter()
                    .copied()
                    .chain(["workspace_deletion_operations", "workspaces"])
                    .map(str::to_string)
                    .collect::<BTreeSet<_>>();
                assert_eq!(scoped, expected);
                Ok(())
            })
            .expect("purge inventory");
    }

    #[test]
    fn deletion_is_idempotent_and_removes_workspace_scoped_rows() {
        let (store, owner, workspace_id) = setup();
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO worker_mutation_source_proof_jtis (
                        workspace_id, runtime_id, jti, expires_at, consumed_at
                     ) VALUES (?1, 'runtime-a', 'jti-a', 1, '1')",
                    params![workspace_id],
                )?;
                Ok(())
            })
            .expect("non-FK scoped audit fixture");
        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        let request = WorkspaceDeletionRequest {
            operation_id: "delete-workspace-a".to_string(),
            expected_revision: preflight.expected_revision,
            confirmation: "Alpha".to_string(),
        };
        let first = store
            .reserve_workspace_deletion(&owner, &workspace_id, &request)
            .expect("reserve");
        assert!(!first.replay);
        assert_eq!(
            store
                .resumable_workspace_deletion_operation_ids()
                .expect("resumable operations"),
            vec![request.operation_id.clone()]
        );
        let replay = store
            .reserve_workspace_deletion(&owner, &workspace_id, &request)
            .expect("replay");
        assert!(replay.replay);
        assert!(matches!(
            store.update_workspace_deletion_operation(
                &request.operation_id,
                WorkspaceDeletionState::Running,
                &vec!["child".to_string(); WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS + 1],
                &[],
                None,
            ),
            Err(Error::Store(_))
        ));
        let with_child = store
            .append_workspace_deletion_child_operation(&request.operation_id, "child-operation-1")
            .expect("append child operation");
        assert_eq!(
            with_child.child_operation_ids,
            vec!["child-operation-1".to_string()]
        );
        let duplicate = store
            .append_workspace_deletion_child_operation(&request.operation_id, "child-operation-1")
            .expect("append child operation replay");
        assert_eq!(
            duplicate.child_operation_ids,
            with_child.child_operation_ids
        );
        let stale_failure_update = store
            .update_workspace_deletion_operation(
                &request.operation_id,
                WorkspaceDeletionState::Blocked,
                &[],
                &[],
                Some("retryable_failure"),
            )
            .expect("stale failure update");
        assert_eq!(
            stale_failure_update.child_operation_ids,
            with_child.child_operation_ids
        );
        let completed = store
            .finalize_workspace_deletion(&request.operation_id)
            .expect("finalize");
        assert_eq!(completed.state, WorkspaceDeletionState::Succeeded);
        assert_eq!(
            completed.child_operation_ids,
            vec!["child-operation-1".to_string()]
        );
        let replayed = store
            .finalize_workspace_deletion(&request.operation_id)
            .expect("finalize replay");
        assert_eq!(completed, replayed);
        assert!(
            store
                .resumable_workspace_deletion_operation_ids()
                .expect("terminal operations")
                .is_empty()
        );
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
        let proof_count: u64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM worker_mutation_source_proof_jtis WHERE workspace_id = ?1",
                    params![workspace_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("scoped audit read");
        assert_eq!(proof_count, 0);
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
            confirmation: "Alpha".to_string(),
        };
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        let mut other_operation = request.clone();
        other_operation.operation_id = "delete-alpha-other-operation".to_string();
        assert_ne!(
            request_fingerprint(&owner, &workspace_id, &request),
            request_fingerprint(&owner, &workspace_id, &other_operation)
        );
        request.expected_revision = preflight.expected_revision;
        request.confirmation = "delete Alpha".to_string();
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn incomplete_worker_create_and_pending_workdir_create_block_without_orphans() {
        let (store, owner, workspace_id) = setup();
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO worker_create_reservations (
                        workspace_id, allocation_key, worker_id, runtime_id,
                        create_fingerprint, state, created_at, updated_at
                     ) VALUES (?1, 'allocation', 'worker-pending', 'runtime-a',
                               'fingerprint', 'created', '1', '1')",
                    params![workspace_id],
                )?;
                conn.execute(
                    "INSERT INTO workdir_create_operations (
                        workspace_id, operation_id, request_fingerprint, repository_id,
                        selector, requested_runtime_id, resolved_runtime_id, config_revision,
                        config_projection_digest, working_directory_id, state, created_at, updated_at
                     ) VALUES (
                        ?1, 'workdir-create', 'fingerprint', 'repository-pending',
                        'develop', 'runtime-a', 'runtime-a', 1,
                        'projection', 'workdir-pending', 'pending', '1', '1'
                     )",
                    params![workspace_id],
                )?;
                Ok(())
            })
            .expect("pending creation fixtures");
        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        assert!(!preflight.can_delete);
        assert!(
            preflight
                .blockers
                .iter()
                .any(|blocker| blocker.message.contains("Worker creation reservations"))
        );
        assert!(
            preflight
                .blockers
                .iter()
                .any(|blocker| blocker.message.contains("Workdir creation operations"))
        );
        let request = WorkspaceDeletionRequest {
            operation_id: "delete-with-pending-creates".to_string(),
            expected_revision: preflight.expected_revision,
            confirmation: "Alpha".to_string(),
        };
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        assert!(
            store
                .workspace_deletion_operation_for_recovery(&request.operation_id)
                .expect("operation lookup")
                .is_none()
        );

        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO worker_registry (
                        workspace_id, runtime_id, worker_id, display_name,
                        created_at, updated_at, retention_state
                     ) VALUES (?1, 'runtime-a', 'worker-pending', 'Created worker', '1', '1', 'normal')",
                    params![workspace_id],
                )?;
                conn.execute(
                    "DELETE FROM workdir_create_operations WHERE operation_id = 'workdir-create'",
                    [],
                )?;
                Ok(())
            })
            .expect("finalized worker creation");
        let reconciled = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("reconciled preflight");
        assert!(
            !reconciled
                .blockers
                .iter()
                .any(|blocker| blocker.message.contains("Worker creation reservations"))
        );
    }

    #[test]
    fn worker_registry_removal_terminalizes_created_reservation() {
        let (store, owner, workspace_id) = setup();
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO worker_create_reservations (
                        workspace_id, allocation_key, worker_id, runtime_id,
                        create_fingerprint, state, created_at, updated_at
                     ) VALUES (?1, 'allocation', 'worker-created', 'runtime-a',
                               'fingerprint', 'created', '1', '1')",
                    params![workspace_id],
                )?;
                conn.execute(
                    "INSERT INTO worker_registry (
                        workspace_id, runtime_id, worker_id, display_name,
                        created_at, updated_at, retention_state
                     ) VALUES (?1, 'runtime-a', 'worker-created', 'Created worker', '1', '1', 'normal')",
                    params![workspace_id],
                )?;
                Ok(())
            })
            .expect("created worker fixture");
        let before = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight before removal");
        assert!(
            !before
                .blockers
                .iter()
                .any(|blocker| blocker.message.contains("Worker creation reservations"))
        );

        assert!(
            store
                .delete_worker_registry(
                    &workspace_id,
                    &worker_runtime::identity::RuntimeWorkerRef {
                        runtime_id: "runtime-a".to_string(),
                        worker_id: "worker-created".to_string(),
                    },
                )
                .expect("remove worker registry")
        );
        let state: String = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT state FROM worker_create_reservations
                     WHERE workspace_id = ?1 AND allocation_key = 'allocation'",
                    params![workspace_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("reservation state");
        assert_eq!(state, "removed");
        let after = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight after removal");
        assert!(
            !after
                .blockers
                .iter()
                .any(|blocker| blocker.message.contains("Worker creation reservations"))
        );
    }

    #[test]
    fn pinned_worker_blocks_preflight_before_operation_reservation() {
        let (store, owner, workspace_id) = setup();
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO worker_registry (
                        workspace_id, runtime_id, worker_id, display_name,
                        created_at, updated_at, retention_state
                     ) VALUES (?1, 'runtime-a', 'worker-a', 'Pinned worker', '1', '1', 'pinned')",
                    params![workspace_id],
                )?;
                Ok(())
            })
            .expect("worker");
        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        assert!(!preflight.can_delete);
        assert!(preflight.blockers.iter().any(|blocker| {
            blocker.kind == WorkspaceDeletionBlockerKind::RetentionHold
                && blocker.resource_key.as_deref() == Some("Pinned worker")
        }));
        let request = WorkspaceDeletionRequest {
            operation_id: "delete-pinned".to_string(),
            expected_revision: preflight.expected_revision,
            confirmation: "Alpha".to_string(),
        };
        assert!(matches!(
            store.reserve_workspace_deletion(&owner, &workspace_id, &request),
            Err(Error::WorkspaceConfigConflict(_))
        ));
    }

    #[test]
    fn preflight_counts_complete_inventory_and_bounds_blocker_projection() {
        let (store, owner, workspace_id) = setup();
        store
            .with_conn(|conn| {
                conn.execute(
                    "WITH RECURSIVE seq(value) AS (
                        SELECT 1 UNION ALL SELECT value + 1 FROM seq WHERE value < 10001
                     )
                     INSERT INTO worker_registry (
                        workspace_id, runtime_id, worker_id, display_name,
                        created_at, updated_at, retention_state
                     )
                     SELECT ?1, 'runtime-a', 'worker-' || value, 'Pinned ' || value,
                            '1', '1', 'pinned'
                     FROM seq",
                    params![workspace_id],
                )?;
                Ok(())
            })
            .expect("worker inventory");
        let preflight = store
            .workspace_deletion_preflight(&owner, &workspace_id)
            .expect("preflight");
        assert_eq!(preflight.resources.workers, 10_001);
        assert_eq!(preflight.blockers.len(), WORKSPACE_DELETION_MAX_BLOCKERS);
        assert!(preflight.blockers.iter().any(|blocker| {
            blocker
                .message
                .contains("Additional deletion blockers exist")
        }));
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
                            confirmation: "Beta".to_string(),
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
