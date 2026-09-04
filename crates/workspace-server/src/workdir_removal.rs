//! Durable, retryable Backend authority for persistent Workdir removal.
//!
//! Runtime cleanup is an external side effect, so callers reserve one immutable
//! operation before invoking the provider. The final registry deletion and
//! operation completion are committed atomically. If a process stops after the
//! provider side effect but before that transaction, recovery re-observes the
//! provider and converges through the same operation.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::WorkdirRegistryRecord;
use crate::{Error, Result, SqliteWorkspaceStore};

const MAX_REASON_BYTES: usize = 500;
const MAX_ACTOR_BYTES: usize = 200;
const MAX_FAILURE_CATEGORY_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirRemovalOperationState {
    Pending,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirRemovalDisposition {
    Removed,
    Retained,
    AttentionRequired,
}

impl WorkdirRemovalDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Retained => "retained",
            Self::AttentionRequired => "attention_required",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRemovalAttemptOwner {
    pub process_id: u32,
    pub process_start_marker: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkdirRemovalIntent {
    pub operation_id: String,
    pub request_fingerprint: String,
    pub workspace_id: String,
    pub working_directory_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub materialization_fingerprint: String,
    pub source_actor: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRemovalOperation {
    pub operation_id: String,
    pub request_fingerprint: String,
    pub workspace_id: String,
    pub working_directory_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub materialization_fingerprint: String,
    pub source_actor: String,
    pub reason: String,
    pub state: WorkdirRemovalOperationState,
    pub attempt_count: u64,
    pub retryable: bool,
    pub disposition: Option<WorkdirRemovalDisposition>,
    pub failure_category: Option<String>,
    pub attempt_owner: Option<WorkdirRemovalAttemptOwner>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkdirRemovalGuard {
    pub category: &'static str,
    pub detail: &'static str,
}

pub fn workdir_materialization_fingerprint(record: &WorkdirRegistryRecord) -> String {
    let bytes = serde_json::to_vec(&serde_json::json!([
        record.workspace_id,
        record.workdir_id,
        record.runtime_id,
        record.repository_id,
        record.creation_selector,
        record.creation_ref,
        record.creation_tree,
    ]))
    .expect("Workdir materialization identity is serializable");
    hex_sha256(&bytes)
}

pub fn workdir_removal_intent(
    record: &WorkdirRegistryRecord,
    source_actor: &str,
    reason: &str,
) -> Result<WorkdirRemovalIntent> {
    validate_bounded("source actor", source_actor, MAX_ACTOR_BYTES)?;
    validate_bounded("reason", reason, MAX_REASON_BYTES)?;
    let materialization_fingerprint = workdir_materialization_fingerprint(record);
    let fingerprint_bytes = serde_json::to_vec(&serde_json::json!([
        record.workspace_id,
        record.workdir_id,
        record.runtime_id,
        record.repository_id,
        materialization_fingerprint,
        source_actor,
        reason,
    ]))
    .map_err(|error| Error::Store(format!("Workdir removal fingerprint failed: {error}")))?;
    let request_fingerprint = hex_sha256(&fingerprint_bytes);
    Ok(WorkdirRemovalIntent {
        operation_id: format!("wdr_{}", &request_fingerprint[..32]),
        request_fingerprint,
        workspace_id: record.workspace_id.clone(),
        working_directory_id: record.workdir_id.clone(),
        runtime_id: record.runtime_id.clone(),
        repository_id: record.repository_id.clone(),
        materialization_fingerprint,
        source_actor: source_actor.to_string(),
        reason: reason.to_string(),
    })
}

impl SqliteWorkspaceStore {
    pub fn reserve_workdir_removal_operation(
        &self,
        intent: &WorkdirRemovalIntent,
    ) -> Result<WorkdirRemovalOperation> {
        validate_intent(intent)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = load_operation(&tx, &intent.workspace_id, &intent.operation_id)? {
                if existing.request_fingerprint != intent.request_fingerprint
                    || existing.working_directory_id != intent.working_directory_id
                    || existing.runtime_id != intent.runtime_id
                    || existing.repository_id != intent.repository_id
                    || existing.materialization_fingerprint != intent.materialization_fingerprint
                    || existing.source_actor != intent.source_actor
                    || existing.reason != intent.reason
                {
                    return Err(Error::WorkdirAttachmentConflict(format!(
                        "Workdir removal operation `{}` was reused with different intent",
                        intent.operation_id
                    )));
                }
                tx.commit()?;
                return Ok(existing);
            }
            let pending_operation: Option<String> = tx
                .query_row(
                    "SELECT operation_id FROM workdir_removal_operations WHERE workspace_id=?1 AND workdir_id=?2 AND state='pending' LIMIT 1",
                    params![intent.workspace_id, intent.working_directory_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(pending_operation) = pending_operation {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir removal operation `{pending_operation}` is already pending"
                )));
            }
            let current = load_workdir_record(&tx, &intent.workspace_id, &intent.working_directory_id)?
                .ok_or_else(|| Error::InvalidInput(format!(
                    "Unknown Workdir `{}`",
                    intent.working_directory_id
                )))?;
            require_matching_materialization(intent, &current)?;
            let repository_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM repositories WHERE workspace_id=?1 AND repository_id=?2)",
                params![intent.workspace_id, intent.repository_id],
                |row| row.get(0),
            )?;
            if !repository_exists {
                return Err(Error::RegistryInconsistency(
                    "Workdir Repository authority is missing".to_string(),
                ));
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                r#"INSERT INTO workdir_removal_operations (
                    workspace_id, operation_id, request_fingerprint, workdir_id, runtime_id,
                    repository_id, materialization_fingerprint, source_actor, reason, state,
                    attempt_count, retryable, disposition, failure_category,
                    created_at, updated_at, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', 0, 1, NULL, NULL, ?10, ?10, NULL)"#,
                params![
                    intent.workspace_id,
                    intent.operation_id,
                    intent.request_fingerprint,
                    intent.working_directory_id,
                    intent.runtime_id,
                    intent.repository_id,
                    intent.materialization_fingerprint,
                    intent.source_actor,
                    intent.reason,
                    now,
                ],
            )?;
            let operation = load_operation(&tx, &intent.workspace_id, &intent.operation_id)?
                .ok_or_else(|| Error::Store("reserved Workdir removal operation is missing".to_string()))?;
            tx.commit()?;
            Ok(operation)
        })
    }

    pub fn begin_workdir_removal_attempt(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        owner: WorkdirRemovalAttemptOwner,
    ) -> Result<WorkdirRemovalOperation> {
        self.begin_workdir_removal_attempt_inner(
            workspace_id,
            operation_id,
            request_fingerprint,
            owner,
            None,
        )
    }

    pub fn reclaim_workdir_removal_attempt_for_recovery(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        owner: WorkdirRemovalAttemptOwner,
        expected_prior_owner: Option<WorkdirRemovalAttemptOwner>,
        expected_attempt_count: u64,
    ) -> Result<WorkdirRemovalOperation> {
        self.begin_workdir_removal_attempt_inner(
            workspace_id,
            operation_id,
            request_fingerprint,
            owner,
            Some((expected_prior_owner, expected_attempt_count)),
        )
    }

    fn begin_workdir_removal_attempt_inner(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        owner: WorkdirRemovalAttemptOwner,
        recovery_expected: Option<(Option<WorkdirRemovalAttemptOwner>, u64)>,
    ) -> Result<WorkdirRemovalOperation> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let operation =
                require_operation(&tx, workspace_id, operation_id, request_fingerprint)?;
            if operation.state == WorkdirRemovalOperationState::Completed {
                tx.commit()?;
                return Ok(operation);
            }
            if operation.state == WorkdirRemovalOperationState::Failed && !operation.retryable {
                return Err(Error::InvalidInput(format!(
                    "Workdir removal operation `{operation_id}` is not retryable"
                )));
            }
            if let Some((expected_owner, expected_attempt_count)) = recovery_expected {
                if operation.state != WorkdirRemovalOperationState::Pending
                    || operation.attempt_owner != expected_owner
                    || operation.attempt_count != expected_attempt_count
                {
                    return Err(Error::WorkdirAttachmentConflict(format!(
                        "Workdir removal operation `{operation_id}` changed after orphan proof"
                    )));
                }
            } else if operation.state == WorkdirRemovalOperationState::Pending
                && operation.attempt_count > 0
            {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir removal operation `{operation_id}` already has an active attempt"
                )));
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE workdir_removal_operations SET state='pending', attempt_count=attempt_count+1, retryable=1, failure_category=NULL, disposition=NULL, attempt_owner_pid=?1, attempt_owner_start_marker=?2, updated_at=?3, completed_at=NULL WHERE workspace_id=?4 AND operation_id=?5 AND request_fingerprint=?6",
                params![
                    owner.process_id,
                    i64::try_from(owner.process_start_marker).map_err(|_| {
                        Error::InvalidInput(
                            "process start marker is out of SQLite range".to_string(),
                        )
                    })?,
                    now,
                    workspace_id,
                    operation_id,
                    request_fingerprint,
                ],
            )?;
            let operation =
                require_operation(&tx, workspace_id, operation_id, request_fingerprint)?;
            tx.commit()?;
            Ok(operation)
        })
    }

    pub fn workdir_removal_guards(
        &self,
        operation: &WorkdirRemovalOperation,
    ) -> Result<Vec<WorkdirRemovalGuard>> {
        self.with_conn(|conn| {
            let current = load_workdir_record(
                conn,
                &operation.workspace_id,
                &operation.working_directory_id,
            )?
            .ok_or_else(|| Error::InvalidInput(format!(
                "Unknown Workdir `{}`",
                operation.working_directory_id
            )))?;
            require_operation_materialization(operation, &current)?;
            let repository_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM repositories WHERE workspace_id=?1 AND repository_id=?2)",
                params![operation.workspace_id, operation.repository_id],
                |row| row.get(0),
            )?;
            if !repository_exists {
                return Err(Error::RegistryInconsistency(
                    "Workdir Repository authority is missing".to_string(),
                ));
            }
            let active_attachment: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM worker_workdir_links WHERE workspace_id=?1 AND workdir_id=?2 AND unlinked_at IS NULL)",
                params![operation.workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            let pending_attachment: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM worker_workdir_attachment_reservations WHERE workspace_id=?1 AND workdir_id=?2)",
                params![operation.workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            let current_assignment: bool = conn.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_links AS link
                    JOIN ticket_current_worker_assignments AS current
                      ON current.workspace_id=link.workspace_id
                    JOIN ticket_worker_assignments AS assignment
                      ON assignment.workspace_id=current.workspace_id
                     AND assignment.ticket_id=current.ticket_id
                     AND assignment.role=current.role
                     AND assignment.assignment_id=current.assignment_id
                     AND assignment.runtime_id=link.runtime_id
                     AND assignment.worker_id=link.worker_id
                    WHERE link.workspace_id=?1 AND link.workdir_id=?2 AND link.unlinked_at IS NULL
                )"#,
                params![operation.workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            let retention_hold: bool = conn.query_row(
                r#"SELECT EXISTS(
                    SELECT 1 FROM worker_workdir_links AS link
                    JOIN worker_registry AS worker
                      ON worker.workspace_id=link.workspace_id
                     AND worker.runtime_id=link.runtime_id
                     AND worker.worker_id=link.worker_id
                    WHERE link.workspace_id=?1 AND link.workdir_id=?2
                      AND link.unlinked_at IS NULL AND worker.retention_state='pinned'
                )"#,
                params![operation.workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            let pending_materialization: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM workdir_create_operations WHERE workspace_id=?1 AND working_directory_id=?2 AND state='pending')",
                params![operation.workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            let mut guards = Vec::new();
            if active_attachment {
                guards.push(WorkdirRemovalGuard {
                    category: "active_attachment",
                    detail: "Workdir has an active Worker attachment",
                });
            }
            if pending_attachment {
                guards.push(WorkdirRemovalGuard {
                    category: "pending_attachment",
                    detail: "Workdir has a pending Worker attachment reservation",
                });
            }
            if current_assignment {
                guards.push(WorkdirRemovalGuard {
                    category: "current_assignment",
                    detail: "Workdir is bound to a Worker with a current Ticket assignment",
                });
            }
            if retention_hold {
                guards.push(WorkdirRemovalGuard {
                    category: "retention_hold",
                    detail: "Workdir is bound to a retained Worker",
                });
            }
            if pending_materialization {
                guards.push(WorkdirRemovalGuard {
                    category: "materialization_pending",
                    detail: "Workdir materialization is still pending",
                });
            }
            Ok(guards)
        })
    }

    pub fn complete_workdir_removal_retained(
        &self,
        operation: &WorkdirRemovalOperation,
        disposition: WorkdirRemovalDisposition,
        category: &str,
    ) -> Result<WorkdirRemovalOperation> {
        validate_failure_category(category)?;
        if disposition == WorkdirRemovalDisposition::Removed {
            return Err(Error::InvalidInput(
                "retained completion cannot use removed disposition".to_string(),
            ));
        }
        self.finish_workdir_removal_operation(operation, disposition, false, Some(category), false)
    }

    pub fn fail_workdir_removal_operation(
        &self,
        operation: &WorkdirRemovalOperation,
        category: &str,
        retryable: bool,
    ) -> Result<WorkdirRemovalOperation> {
        validate_failure_category(category)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = require_operation(
                &tx,
                &operation.workspace_id,
                &operation.operation_id,
                &operation.request_fingerprint,
            )?;
            if current.state == WorkdirRemovalOperationState::Completed {
                tx.commit()?;
                return Ok(current);
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE workdir_removal_operations SET state='failed', retryable=?1, disposition=?2, failure_category=?3, attempt_owner_pid=NULL, attempt_owner_start_marker=NULL, updated_at=?4 WHERE workspace_id=?5 AND operation_id=?6 AND request_fingerprint=?7",
                params![
                    retryable,
                    WorkdirRemovalDisposition::AttentionRequired.as_str(),
                    category,
                    now,
                    operation.workspace_id,
                    operation.operation_id,
                    operation.request_fingerprint,
                ],
            )?;
            let updated = require_operation(
                &tx,
                &operation.workspace_id,
                &operation.operation_id,
                &operation.request_fingerprint,
            )?;
            tx.commit()?;
            Ok(updated)
        })
    }

    pub fn commit_workdir_removal_removed(
        &self,
        operation: &WorkdirRemovalOperation,
    ) -> Result<WorkdirRemovalOperation> {
        self.finish_workdir_removal_operation(
            operation,
            WorkdirRemovalDisposition::Removed,
            false,
            None,
            true,
        )
    }

    fn finish_workdir_removal_operation(
        &self,
        operation: &WorkdirRemovalOperation,
        disposition: WorkdirRemovalDisposition,
        retryable: bool,
        category: Option<&str>,
        delete_registry: bool,
    ) -> Result<WorkdirRemovalOperation> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = require_operation(
                &tx,
                &operation.workspace_id,
                &operation.operation_id,
                &operation.request_fingerprint,
            )?;
            if current.state == WorkdirRemovalOperationState::Completed {
                tx.commit()?;
                return Ok(current);
            }
            if delete_registry {
                let registry = load_workdir_record(
                    &tx,
                    &operation.workspace_id,
                    &operation.working_directory_id,
                )?
                .ok_or_else(|| Error::RegistryInconsistency(
                    "Workdir registry row disappeared before durable removal commit".to_string(),
                ))?;
                require_operation_materialization(operation, &registry)?;
                require_no_removal_blockers(
                    &tx,
                    &operation.workspace_id,
                    &operation.working_directory_id,
                )?;
                let deleted = tx.execute(
                    "DELETE FROM workdir_registry WHERE workspace_id=?1 AND workdir_id=?2",
                    params![operation.workspace_id, operation.working_directory_id],
                )?;
                if deleted != 1 {
                    return Err(Error::RegistryInconsistency(
                        "Workdir registry deletion did not remove exactly one row".to_string(),
                    ));
                }
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE workdir_removal_operations SET state='completed', retryable=?1, disposition=?2, failure_category=?3, attempt_owner_pid=NULL, attempt_owner_start_marker=NULL, updated_at=?4, completed_at=?4 WHERE workspace_id=?5 AND operation_id=?6 AND request_fingerprint=?7",
                params![
                    retryable,
                    disposition.as_str(),
                    category,
                    now,
                    operation.workspace_id,
                    operation.operation_id,
                    operation.request_fingerprint,
                ],
            )?;
            let updated = require_operation(
                &tx,
                &operation.workspace_id,
                &operation.operation_id,
                &operation.request_fingerprint,
            )?;
            tx.commit()?;
            Ok(updated)
        })
    }

    pub fn find_workdir_removal_operation_by_intent(
        &self,
        workspace_id: &str,
        working_directory_id: &str,
        source_actor: &str,
        reason: &str,
    ) -> Result<Option<WorkdirRemovalOperation>> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "{} WHERE workspace_id=?1 AND workdir_id=?2 AND source_actor=?3 AND reason=?4 ORDER BY created_at DESC, operation_id DESC LIMIT 1",
                    operation_select_sql()
                ),
                params![workspace_id, working_directory_id, source_actor, reason],
                read_operation,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn recoverable_workdir_removal_operations(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkdirRemovalOperation>> {
        self.with_conn(|conn| {
            let mut statement = conn.prepare(
                &format!(
                    "{} WHERE workspace_id=?1 AND (state='pending' OR (state='failed' AND retryable=1)) ORDER BY updated_at ASC, operation_id ASC LIMIT ?2",
                    operation_select_sql()
                ),
            )?;
            let rows = statement.query_map(params![workspace_id, limit as i64], read_operation)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    pub fn get_workdir_removal_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<WorkdirRemovalOperation>> {
        self.with_conn(|conn| load_operation(conn, workspace_id, operation_id))
    }
}

fn require_no_removal_blockers(
    conn: &Connection,
    workspace_id: &str,
    workdir_id: &str,
) -> Result<()> {
    let blocked: bool = conn.query_row(
        r#"SELECT EXISTS(
            SELECT 1 FROM worker_workdir_links
             WHERE workspace_id=?1 AND workdir_id=?2 AND unlinked_at IS NULL
            UNION ALL
            SELECT 1 FROM worker_workdir_attachment_reservations
             WHERE workspace_id=?1 AND workdir_id=?2
            UNION ALL
            SELECT 1 FROM workdir_create_operations
             WHERE workspace_id=?1 AND working_directory_id=?2 AND state='pending'
        )"#,
        params![workspace_id, workdir_id],
        |row| row.get(0),
    )?;
    if blocked {
        return Err(Error::WorkdirAttachmentConflict(format!(
            "Workdir {workdir_id} acquired active attachment or materialization authority during removal"
        )));
    }
    Ok(())
}

fn validate_intent(intent: &WorkdirRemovalIntent) -> Result<()> {
    for (label, value) in [
        ("operation id", intent.operation_id.as_str()),
        ("request fingerprint", intent.request_fingerprint.as_str()),
        ("Workspace id", intent.workspace_id.as_str()),
        ("Workdir id", intent.working_directory_id.as_str()),
        ("Runtime id", intent.runtime_id.as_str()),
        ("Repository id", intent.repository_id.as_str()),
        (
            "materialization fingerprint",
            intent.materialization_fingerprint.as_str(),
        ),
    ] {
        validate_bounded(label, value, 256)?;
    }
    validate_bounded("source actor", &intent.source_actor, MAX_ACTOR_BYTES)?;
    validate_bounded("reason", &intent.reason, MAX_REASON_BYTES)
}

fn validate_bounded(label: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(format!(
            "{label} must be non-empty and at most {max} bytes"
        )));
    }
    Ok(())
}

fn validate_failure_category(category: &str) -> Result<()> {
    if category.is_empty()
        || category.len() > MAX_FAILURE_CATEGORY_BYTES
        || !category
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(Error::InvalidInput(
            "Workdir removal failure category is invalid".to_string(),
        ));
    }
    Ok(())
}

fn require_matching_materialization(
    intent: &WorkdirRemovalIntent,
    record: &WorkdirRegistryRecord,
) -> Result<()> {
    if intent.workspace_id != record.workspace_id
        || intent.working_directory_id != record.workdir_id
        || intent.runtime_id != record.runtime_id
        || intent.repository_id != record.repository_id
        || intent.materialization_fingerprint != workdir_materialization_fingerprint(record)
    {
        return Err(Error::WorkdirAttachmentConflict(
            "Workdir authority changed before removal reservation".to_string(),
        ));
    }
    Ok(())
}

fn require_operation_materialization(
    operation: &WorkdirRemovalOperation,
    record: &WorkdirRegistryRecord,
) -> Result<()> {
    if operation.workspace_id != record.workspace_id
        || operation.working_directory_id != record.workdir_id
        || operation.runtime_id != record.runtime_id
        || operation.repository_id != record.repository_id
        || operation.materialization_fingerprint != workdir_materialization_fingerprint(record)
    {
        return Err(Error::WorkdirAttachmentConflict(
            "Workdir authority changed after removal reservation".to_string(),
        ));
    }
    Ok(())
}

fn load_workdir_record(
    conn: &Connection,
    workspace_id: &str,
    workdir_id: &str,
) -> Result<Option<WorkdirRegistryRecord>> {
    conn.query_row(
        r#"SELECT workspace_id, workdir_id, runtime_id, repository_id,
                  creation_selector, creation_ref, creation_tree,
                  current_selector, current_ref, current_tree, observed_at_epoch_seconds,
                  materialization_status, cleanliness, created_at, updated_at
           FROM workdir_registry WHERE workspace_id=?1 AND workdir_id=?2"#,
        params![workspace_id, workdir_id],
        |row| {
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
        },
    )
    .optional()
    .map_err(Error::from)
}

fn require_operation(
    conn: &Connection,
    workspace_id: &str,
    operation_id: &str,
    request_fingerprint: &str,
) -> Result<WorkdirRemovalOperation> {
    let operation = load_operation(conn, workspace_id, operation_id)?.ok_or_else(|| {
        Error::InvalidInput(format!(
            "Unknown Workdir removal operation `{operation_id}`"
        ))
    })?;
    if operation.request_fingerprint != request_fingerprint {
        return Err(Error::WorkdirAttachmentConflict(format!(
            "Workdir removal operation `{operation_id}` fingerprint mismatch"
        )));
    }
    Ok(operation)
}

fn load_operation(
    conn: &Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<WorkdirRemovalOperation>> {
    conn.query_row(
        &format!(
            "{} WHERE workspace_id=?1 AND operation_id=?2",
            operation_select_sql()
        ),
        params![workspace_id, operation_id],
        read_operation,
    )
    .optional()
    .map_err(Error::from)
}

fn operation_select_sql() -> &'static str {
    r#"SELECT operation_id, request_fingerprint, workspace_id, workdir_id, runtime_id,
              repository_id, materialization_fingerprint, source_actor, reason, state,
              attempt_count, retryable, disposition, failure_category,
              attempt_owner_pid, attempt_owner_start_marker,
              created_at, updated_at, completed_at
       FROM workdir_removal_operations"#
}

fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkdirRemovalOperation> {
    let state = parse_state(&row.get::<_, String>(9)?)?;
    let disposition = row
        .get::<_, Option<String>>(12)?
        .map(|value| parse_disposition(&value))
        .transpose()?;
    let attempt_count = row.get::<_, i64>(10)?;
    let attempt_owner_pid = row.get::<_, Option<i64>>(14)?;
    let attempt_owner_start_marker = row.get::<_, Option<i64>>(15)?;
    let attempt_owner = match (attempt_owner_pid, attempt_owner_start_marker) {
        (None, None) => None,
        (Some(process_id), Some(process_start_marker)) => Some(WorkdirRemovalAttemptOwner {
            process_id: process_id
                .try_into()
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(14, process_id))?,
            process_start_marker: process_start_marker
                .try_into()
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(15, process_start_marker))?,
        }),
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                14,
                rusqlite::types::Type::Integer,
                "incomplete Workdir removal attempt owner".into(),
            ));
        }
    };
    Ok(WorkdirRemovalOperation {
        operation_id: row.get(0)?,
        request_fingerprint: row.get(1)?,
        workspace_id: row.get(2)?,
        working_directory_id: row.get(3)?,
        runtime_id: row.get(4)?,
        repository_id: row.get(5)?,
        materialization_fingerprint: row.get(6)?,
        source_actor: row.get(7)?,
        reason: row.get(8)?,
        state,
        attempt_count: attempt_count
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, attempt_count))?,
        retryable: row.get(11)?,
        disposition,
        failure_category: row.get(13)?,
        attempt_owner,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        completed_at: row.get(18)?,
    })
}

fn parse_state(value: &str) -> rusqlite::Result<WorkdirRemovalOperationState> {
    match value {
        "pending" => Ok(WorkdirRemovalOperationState::Pending),
        "failed" => Ok(WorkdirRemovalOperationState::Failed),
        "completed" => Ok(WorkdirRemovalOperationState::Completed),
        _ => Err(invalid_enum(9, value)),
    }
}

fn parse_disposition(value: &str) -> rusqlite::Result<WorkdirRemovalDisposition> {
    match value {
        "removed" => Ok(WorkdirRemovalDisposition::Removed),
        "retained" => Ok(WorkdirRemovalDisposition::Retained),
        "attention_required" => Ok(WorkdirRemovalDisposition::AttentionRequired),
        _ => Err(invalid_enum(12, value)),
    }
}

fn invalid_enum(column: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("invalid Workdir removal value `{value}`").into(),
    )
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AccountRecord, ControlPlaneStore, RepositoryRecord, WorkspaceRecord};
    use workspace_api::{RepositoryObservedStatus, RepositorySource, RepositorySourceKind};

    fn attempt_owner() -> WorkdirRemovalAttemptOwner {
        WorkdirRemovalAttemptOwner {
            process_id: 100,
            process_start_marker: 200,
        }
    }

    async fn seeded_store() -> (SqliteWorkspaceStore, WorkdirRegistryRecord) {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store
            .upsert_account(&AccountRecord {
                account_id: "account-a".to_string(),
                kind: "user".to_string(),
                handle: "owner".to_string(),
                display_name: "Owner".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-a".to_string(),
                owner_account_id: "account-a".to_string(),
                display_name: "Workspace A".to_string(),
                state: "active".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .await
            .unwrap();
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: "workspace-a".to_string(),
                repository_id: "repository-a".to_string(),
                repository_key: "repository-a".to_string(),
                kind: "git".to_string(),
                provider: Some("local".to_string()),
                source: RepositorySource {
                    kind: RepositorySourceKind::LocalPath,
                    uri: "/repository-a".to_string(),
                },
                default_ref: Some("develop".to_string()),
                source_revision: 1,
                source_fingerprint: "source-a".to_string(),
                observed_status: RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        let workdir = WorkdirRegistryRecord {
            workspace_id: "workspace-a".to_string(),
            workdir_id: "workdir-a".to_string(),
            runtime_id: "runtime-a".to_string(),
            repository_id: "repository-a".to_string(),
            creation_selector: Some("refs/heads/develop".to_string()),
            creation_ref: Some("abc".to_string()),
            creation_tree: Some("tree-a".to_string()),
            current_selector: Some("refs/heads/work".to_string()),
            current_ref: Some("def".to_string()),
            current_tree: Some("tree-b".to_string()),
            observed_at_epoch_seconds: Some(1),
            materialization_status: "present".to_string(),
            cleanliness: "clean".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workdir_registry(&workdir).unwrap();
        (store, workdir)
    }

    #[tokio::test]
    async fn exact_replay_reuses_operation_and_conflicting_intent_fails() {
        let (store, workdir) = seeded_store().await;
        let intent =
            workdir_removal_intent(&workdir, "worker:W-1", "remove stale Workdir").unwrap();
        let first = store.reserve_workdir_removal_operation(&intent).unwrap();
        let replay = store.reserve_workdir_removal_operation(&intent).unwrap();
        assert_eq!(replay, first);

        let mut conflict = intent.clone();
        conflict.reason = "different intent".to_string();
        conflict.request_fingerprint = "f".repeat(64);
        let error = store
            .reserve_workdir_removal_operation(&conflict)
            .unwrap_err();
        assert!(matches!(error, Error::WorkdirAttachmentConflict(_)));
    }

    #[tokio::test]
    async fn failed_attempt_is_retryable_and_completed_retry_replays() {
        let (store, workdir) = seeded_store().await;
        let intent =
            workdir_removal_intent(&workdir, "workspace-api", "remove clean Workdir").unwrap();
        let reserved = store.reserve_workdir_removal_operation(&intent).unwrap();
        let first = store
            .begin_workdir_removal_attempt(
                &reserved.workspace_id,
                &reserved.operation_id,
                &reserved.request_fingerprint,
                attempt_owner(),
            )
            .unwrap();
        assert_eq!(first.attempt_count, 1);
        let failed = store
            .fail_workdir_removal_operation(&first, "runtime_unavailable", true)
            .unwrap();
        assert_eq!(failed.state, WorkdirRemovalOperationState::Failed);
        let retry = store
            .begin_workdir_removal_attempt(
                &failed.workspace_id,
                &failed.operation_id,
                &failed.request_fingerprint,
                attempt_owner(),
            )
            .unwrap();
        assert_eq!(retry.attempt_count, 2);
        let completed = store.commit_workdir_removal_removed(&retry).unwrap();
        assert_eq!(
            completed.disposition,
            Some(WorkdirRemovalDisposition::Removed)
        );
        assert!(
            store
                .get_workdir_registry("workspace-a", "workdir-a")
                .unwrap()
                .is_none()
        );
        let replay = store
            .begin_workdir_removal_attempt(
                &completed.workspace_id,
                &completed.operation_id,
                &completed.request_fingerprint,
                attempt_owner(),
            )
            .unwrap();
        assert_eq!(replay, completed);
    }

    #[tokio::test]
    async fn concurrent_claims_invoke_the_simulated_provider_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let (store, workdir) = seeded_store().await;
        let intent =
            workdir_removal_intent(&workdir, "workspace-api", "remove clean Workdir").unwrap();
        let operation = store.reserve_workdir_removal_operation(&intent).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let mut callers = Vec::new();
        for _ in 0..2 {
            let store = store.clone();
            let operation = operation.clone();
            let barrier = barrier.clone();
            let provider_calls = provider_calls.clone();
            callers.push(std::thread::spawn(move || {
                barrier.wait();
                let claim = store.begin_workdir_removal_attempt(
                    &operation.workspace_id,
                    &operation.operation_id,
                    &operation.request_fingerprint,
                    attempt_owner(),
                );
                if claim.is_ok() {
                    provider_calls.fetch_add(1, Ordering::SeqCst);
                }
                claim
            }));
        }
        barrier.wait();
        let results = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(Error::WorkdirAttachmentConflict(_))))
                .count(),
            1
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 1);

        let claimed = results.into_iter().find_map(|result| result.ok()).unwrap();
        let completed = store.commit_workdir_removal_removed(&claimed).unwrap();
        assert_eq!(
            completed.disposition,
            Some(WorkdirRemovalDisposition::Removed)
        );
    }

    #[tokio::test]
    async fn failed_workdir_create_retry_cannot_start_after_removal_claim() {
        use crate::store::WorkdirCreateOperationRecord;

        let (store, workdir) = seeded_store().await;
        let create = WorkdirCreateOperationRecord {
            workspace_id: "workspace-a".to_string(),
            operation_id: "create-a".to_string(),
            request_fingerprint: "create-fingerprint".to_string(),
            repository_id: "repository-a".to_string(),
            selector: Some("develop".to_string()),
            requested_runtime_id: Some("runtime-a".to_string()),
            resolved_runtime_id: "runtime-a".to_string(),
            config_revision: 1,
            config_projection_digest: "projection-a".to_string(),
            source_kind: Some("local_path".to_string()),
            source_uri: Some("/repository-a".to_string()),
            source_revision: Some(1),
            source_fingerprint: Some("source-a".to_string()),
            credential_id: None,
            credential_revision: None,
            host_trust_id: None,
            host_trust_revision: None,
            repository_access_mode: None,
            cache_generation: 0,
            working_directory_id: "workdir-a".to_string(),
            state: "pending".to_string(),
            failure: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.reserve_workdir_create_operation(&create).unwrap();
        store
            .finish_workdir_create_operation(
                "workspace-a",
                "create-a",
                "create-fingerprint",
                false,
                Some("provider failed"),
                "2",
            )
            .unwrap();
        let intent =
            workdir_removal_intent(&workdir, "workspace-api", "remove clean Workdir").unwrap();
        store.reserve_workdir_removal_operation(&intent).unwrap();

        let error = store
            .begin_failed_workdir_create_retry("workspace-a", "create-a", "create-fingerprint", "3")
            .unwrap_err();
        assert!(matches!(error, Error::WorkdirAttachmentConflict(_)));
        let create = store
            .load_workdir_create_operation("workspace-a", "create-a")
            .unwrap()
            .unwrap();
        assert_eq!(create.state, "failed");
    }

    #[tokio::test]
    async fn pending_removal_fences_new_attachment_and_retry_rereads_live_reservation() {
        let (store, workdir) = seeded_store().await;
        let intent =
            workdir_removal_intent(&workdir, "workspace-api", "remove clean Workdir").unwrap();
        let pending = store.reserve_workdir_removal_operation(&intent).unwrap();

        let error = store
            .reserve_worker_workdir_attachment("workspace-a", "workdir-a", "reservation-a", "2")
            .unwrap_err();
        assert!(matches!(error, Error::WorkdirAttachmentConflict(_)));

        let failed = store
            .fail_workdir_removal_operation(&pending, "provider_unavailable", true)
            .unwrap();
        store
            .reserve_worker_workdir_attachment("workspace-a", "workdir-a", "reservation-b", "3")
            .unwrap();
        let retry = store
            .begin_workdir_removal_attempt(
                &failed.workspace_id,
                &failed.operation_id,
                &failed.request_fingerprint,
                attempt_owner(),
            )
            .unwrap();
        let guards = store.workdir_removal_guards(&retry).unwrap();
        assert!(
            guards
                .iter()
                .any(|guard| guard.category == "pending_attachment")
        );
        let retained = store
            .complete_workdir_removal_retained(
                &retry,
                WorkdirRemovalDisposition::Retained,
                "blocked_by_live_authority",
            )
            .unwrap();
        assert_eq!(
            retained.disposition,
            Some(WorkdirRemovalDisposition::Retained)
        );
    }

    #[tokio::test]
    async fn retained_completion_keeps_registry_and_is_auditable() {
        let (store, workdir) = seeded_store().await;
        let intent =
            workdir_removal_intent(&workdir, "workspace-api", "inspect dirty Workdir").unwrap();
        let operation = store.reserve_workdir_removal_operation(&intent).unwrap();
        let retained = store
            .complete_workdir_removal_retained(
                &operation,
                WorkdirRemovalDisposition::Retained,
                "dirty_or_unknown",
            )
            .unwrap();
        assert_eq!(retained.state, WorkdirRemovalOperationState::Completed);
        assert_eq!(
            retained.failure_category.as_deref(),
            Some("dirty_or_unknown")
        );
        assert!(
            store
                .get_workdir_registry("workspace-a", "workdir-a")
                .unwrap()
                .is_some()
        );
    }
}
