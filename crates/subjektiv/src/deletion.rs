//! Explicit Subject erasure is a two-phase Host operation. Beginning commits a permanent,
//! content-free fence before the Host stops/removes Workers and purges Jobs and execution copies.
//! Source Worker/Session originals remain under their separate archive lifecycle.
//! Failure leaves that fence and the cleanup inventory intact; the Host retries
//! the same operation ID. Finishing asserts that external cleanup succeeded and atomically erases
//! this store's content. This store cannot verify external Runtime/Job/Session cleanup itself.
//!
//! Tombstones contain no behavior, permission vectors, Memory, Journal or Session text. Worker
//! identities remain permanently fenced so removal of a grant cannot become non-Subject fallback
//! authority. Existing Journal Session erasure markers are retained to reject delayed admission;
//! they are content-free and are not a second copy of the deleted material.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectDeletion {
    pub subject_id: String,
    pub operation_id: String,
    pub completed: bool,
    pub started_at: String,
}

/// Host-only cleanup identity, never Job input, output, profile or authorization material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionSubjectJob {
    pub job_id: String,
    pub attempt_id: String,
}

// All content tables are enumerated deliberately. Indirect tables are erased before their
// owner rows, both for FK checks and so their fencing predicates remain resolvable.
const INDIRECT: &[(&str, &str)] = &[
    (
        "journal_generation_input_deliveries",
        "generation_id IN (SELECT g.id FROM journal_generations g JOIN journals j ON j.id=g.journal_id WHERE j.subject_id={subject})",
    ),
    (
        "legacy_record_archive",
        "json_extract(row_json, '$.subject_id')={subject}",
    ),
    (
        "journal_generation_deliveries",
        "generation_id IN (SELECT g.id FROM journal_generations g JOIN journals j ON j.id=g.journal_id WHERE j.subject_id={subject})",
    ),
    (
        "journal_operations",
        "journal_id IN (SELECT id FROM journals WHERE subject_id={subject})",
    ),
    (
        "journal_generations",
        "journal_id IN (SELECT id FROM journals WHERE subject_id={subject})",
    ),
    (
        "journal_sources",
        "journal_id IN (SELECT id FROM journals WHERE subject_id={subject})",
    ),
    (
        "journal_session_bindings",
        "binding_id IN (SELECT binding_id FROM journal_writer_bindings WHERE subject_id={subject})",
    ),
];
const DIRECT: &[&str] = &[
    "memory_journal_links",
    "journal_corrections",
    "journal_entries",
    "journals",
    "journal_writer_bindings",
    "subject_session_attributions",
    "worker_execution_grants",
    "subject_permission_receipts",
    "subject_permissions",
    "subject_permission_changes",
    "candidate_decision_receipts",
    "surface_generation_state",
    "surface_generation_runs",
    "surface_snapshot_refs",
    "surface_snapshot_seals",
    "surface_snapshots",
    "staging_resolution_targets",
    "staging_resolution_seals",
    "staging_resolutions",
    "memory_change_candidates",
    "memory_change_derivations",
    "memory_change_seals",
    "memory_changes",
    "memory_records",
    "staging_records",
    "subjects",
];

fn owner_expression(table: &str, row: &str) -> String {
    match table {
        "legacy_record_archive" => format!("json_extract({row}.row_json, '$.subject_id')"),
        "journal_generation_input_deliveries" | "journal_generation_deliveries" => format!(
            "(SELECT j.subject_id FROM journal_generations g JOIN journals j ON j.id=g.journal_id WHERE g.id={row}.generation_id)"
        ),
        "journal_operations" | "journal_generations" | "journal_sources" => {
            format!("(SELECT subject_id FROM journals WHERE id={row}.journal_id)")
        }
        "journal_session_bindings" => format!(
            "(SELECT subject_id FROM journal_writer_bindings WHERE binding_id={row}.binding_id)"
        ),
        _ => format!("{row}.subject_id"),
    }
}

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(
        "CREATE TABLE subject_deletions (
        subject_id TEXT PRIMARY KEY,
        operation_id TEXT NOT NULL UNIQUE,
        completed INTEGER NOT NULL CHECK(completed IN (0,1)),
        started_at TEXT NOT NULL
    );
    CREATE TABLE subject_deleted_workers (
        subject_id TEXT NOT NULL REFERENCES subject_deletions(subject_id) ON DELETE RESTRICT,
        runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL,
        PRIMARY KEY(subject_id,runtime_id,worker_id)
    );
    CREATE TABLE subject_deleted_sessions (
        subject_id TEXT NOT NULL REFERENCES subject_deletions(subject_id) ON DELETE RESTRICT,
        session_id TEXT NOT NULL UNIQUE,
        PRIMARY KEY(subject_id,session_id)
    );
    CREATE TRIGGER subject_deleted_sessions_retained BEFORE DELETE ON subject_deleted_sessions
        BEGIN SELECT RAISE(ABORT, 'deleted Session fences are permanent'); END;
    CREATE TRIGGER subject_deleted_sessions_immutable BEFORE UPDATE ON subject_deleted_sessions
        BEGIN SELECT RAISE(ABORT, 'deleted Session fences are immutable'); END;
    CREATE INDEX subject_deleted_worker_identity ON subject_deleted_workers(runtime_id,worker_id);
    CREATE TRIGGER subject_deletions_retained BEFORE DELETE ON subject_deletions
        BEGIN SELECT RAISE(ABORT, 'Subject deletion fences are permanent'); END;
    CREATE TRIGGER subject_deletions_identity BEFORE UPDATE ON subject_deletions
        WHEN OLD.subject_id<>NEW.subject_id OR OLD.operation_id<>NEW.operation_id
            OR OLD.started_at<>NEW.started_at OR OLD.completed>NEW.completed
        BEGIN SELECT RAISE(ABORT, 'Subject deletion identity is permanent'); END;
    CREATE TRIGGER subject_deleted_workers_retained BEFORE DELETE ON subject_deleted_workers
        BEGIN SELECT RAISE(ABORT, 'deleted Worker fences are permanent'); END;
    CREATE TRIGGER subject_deleted_workers_immutable BEFORE UPDATE ON subject_deleted_workers
        BEGIN SELECT RAISE(ABORT, 'deleted Worker fences are immutable'); END;",
    )?;

    // Preserve each existing history-retention condition, adding only the explicit Subject
    // erasure exception. Frozen earlier migrations are not edited. Normal history deletion
    // (including another Subject's history) remains forbidden.
    let triggers = tx
        .prepare("SELECT name,tbl_name,sql FROM sqlite_master WHERE type='trigger'")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (name, table, sql) in triggers {
        if !DIRECT.contains(&table.as_str()) && !INDIRECT.iter().any(|(t, _)| *t == table) {
            continue;
        }
        let upper = sql.to_ascii_uppercase();
        if !upper.contains("BEFORE DELETE") {
            continue;
        }
        let begin = upper.find("BEGIN").expect("trigger body");
        let (header, body) = sql.split_at(begin);
        let exception = format!(
            "NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id={})",
            owner_expression(&table, "OLD")
        );
        let header = if let Some(when) = header.to_ascii_uppercase().find("WHEN") {
            format!(
                "{} WHEN ({}) AND {exception} ",
                &header[..when],
                header[when + 4..].trim()
            )
        } else {
            format!("{header} WHEN {exception} ")
        };
        tx.execute_batch(&format!("DROP TRIGGER \"{name}\"; {header}{body}"))?;
    }
    for table in DIRECT
        .iter()
        .copied()
        .chain(INDIRECT.iter().map(|(t, _)| *t))
        .chain(["journal_erased_sessions"])
    {
        for action in ["INSERT", "UPDATE"] {
            let new_owner = owner_expression(table, "NEW");
            let predicate = if action == "UPDATE" {
                format!(
                    "subject_id={new_owner} OR subject_id={}",
                    owner_expression(table, "OLD")
                )
            } else {
                format!("subject_id={new_owner}")
            };
            tx.execute_batch(&format!(
                "CREATE TRIGGER deletion_fence_{table}_{action} BEFORE {action} ON {table}
                WHEN EXISTS(SELECT 1 FROM subject_deletions WHERE {predicate})
                BEGIN SELECT RAISE(ABORT, 'Subject deletion has begun'); END;"
            ))?;
        }
    }
    // Identity reuse must not reconnect purged content or turn a deleted child into
    // an unscoped Worker. Check both old and new on ownership-changing UPDATEs.
    for table in [
        "subject_session_attributions",
        "journal_writer_bindings",
        "worker_execution_grants",
    ] {
        for action in ["INSERT", "UPDATE"] {
            let rows = if action == "UPDATE" {
                vec!["OLD", "NEW"]
            } else {
                vec!["NEW"]
            };
            let mut predicates = Vec::new();
            for row in rows {
                predicates.push(format!("EXISTS(SELECT 1 FROM subject_deleted_workers WHERE runtime_id={row}.runtime_id AND worker_id={row}.worker_id)"));
                if table == "subject_session_attributions" {
                    predicates.push(format!("EXISTS(SELECT 1 FROM subject_deleted_sessions WHERE session_id={row}.session_id)"));
                }
                if table == "worker_execution_grants" {
                    predicates.push(format!("EXISTS(SELECT 1 FROM subject_deleted_workers WHERE runtime_id=json_extract({row}.record_json,'$.origin_runtime_id') AND worker_id=json_extract({row}.record_json,'$.origin_worker_id'))"));
                }
            }
            tx.execute_batch(&format!("CREATE TRIGGER deletion_identity_{table}_{action} BEFORE {action} ON {table} WHEN {} BEGIN SELECT RAISE(ABORT, 'deleted Subject execution identity'); END;", predicates.join(" OR ")))?;
        }
    }
    Ok(())
}

pub(crate) fn is_deleting(connection: &Connection, subject: &str) -> Result<bool> {
    // Older migration fixtures deliberately use current Rust reads before upgrade. Production
    // opens only registered current stores and must fail closed if the fence table is absent.
    #[cfg(test)]
    if !connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='subject_deletions' AND type='table')", [], |r|r.get::<_,bool>(0))? {
        return Ok(false);
    }
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=?1)",
        [subject],
        |r| r.get::<_, bool>(0),
    )?)
}

pub(crate) fn require_not_deleting(connection: &Connection, subject: &str) -> Result<()> {
    if is_deleting(connection, subject)? {
        return Err(SubjektivError::SubjectNotFound(subject.into()));
    }
    Ok(())
}
pub(crate) fn require_live_worker(
    connection: &Connection,
    runtime: &str,
    worker: &str,
) -> Result<()> {
    #[cfg(test)]
    if !connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='subject_deleted_workers' AND type='table')", [], |r|r.get::<_,bool>(0))? { return Ok(()); }
    if connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_deleted_workers WHERE runtime_id=?1 AND worker_id=?2)",
        params![runtime, worker],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(SubjektivError::AuthorizationDenied(
            "Worker belongs to a deleted Subject".into(),
        ));
    }
    Ok(())
}

pub(crate) fn deleted_session_subject(
    connection: &Connection,
    session: &str,
) -> Result<Option<String>> {
    #[cfg(test)]
    if !connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='subject_deleted_sessions' AND type='table')", [], |r|r.get::<_,bool>(0))? { return Ok(None); }
    Ok(connection
        .query_row(
            "SELECT subject_id FROM subject_deleted_sessions WHERE session_id=?1",
            [session],
            |r| r.get(0),
        )
        .optional()?)
}

fn deletion(connection: &Connection, subject: &str) -> Result<Option<SubjectDeletion>> {
    Ok(connection.query_row("SELECT subject_id,operation_id,completed,started_at FROM subject_deletions WHERE subject_id=?1", [subject], |r|Ok(SubjectDeletion { subject_id:r.get(0)?, operation_id:r.get(1)?, completed:r.get(2)?, started_at:r.get(3)? })).optional()?)
}
fn require_deletion(connection: &Connection, subject: &str) -> Result<SubjectDeletion> {
    deletion(connection, subject)?.ok_or_else(|| SubjektivError::SubjectNotFound(subject.into()))
}

impl SubjektivStore {
    /// Commits the read/write/authority fence, even for legacy-retired Subjects. Operation IDs
    /// are unique within this store's scope; neither another key nor another Subject may reuse
    /// a deletion. Exact retries return the saved current result, including completed deletions.
    pub fn begin_subject_deletion(
        &self,
        subject: &str,
        operation: &str,
    ) -> Result<SubjectDeletion> {
        validate_label("subject id", subject)?;
        validate_label("deletion operation id", operation)?;
        self.database.try_transaction(|tx| {
            if let Some(saved) = deletion(tx,subject)? {
                return if saved.operation_id == operation { Ok(saved) } else { Err(SubjektivError::SubjectDeletionConflict(operation.into())) };
            }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE operation_id=?1)",[operation],|r|r.get::<_,bool>(0))? {
                return Err(SubjektivError::SubjectDeletionConflict(operation.into()));
            }
            require_subject(tx,subject)?;
            tx.execute("INSERT INTO subject_deletions(subject_id,operation_id,completed,started_at) VALUES(?1,?2,0,?3)",params![subject,operation,now()])?;
            // Capture bodies, historical attribution, delegated children, and grant origins.
            // This transaction also fences delayed grants/attributions, so no new identity can
            // appear after the inventory is frozen. Runtime create reservations remain Host-owned.
            tx.execute("INSERT INTO subject_deleted_workers(subject_id,runtime_id,worker_id)
                SELECT subject_id,runtime_id,worker_id FROM subject_session_attributions WHERE subject_id=?1
                UNION SELECT subject_id,runtime_id,worker_id FROM journal_writer_bindings WHERE subject_id=?1
                UNION SELECT subject_id,runtime_id,worker_id FROM worker_execution_grants WHERE subject_id=?1
                UNION SELECT subject_id,json_extract(record_json,'$.origin_runtime_id'),json_extract(record_json,'$.origin_worker_id') FROM worker_execution_grants WHERE subject_id=?1",[subject])?;
            tx.execute("INSERT INTO subject_deleted_sessions(subject_id,session_id)
                SELECT subject_id,session_id FROM subject_session_attributions WHERE subject_id=?1
                UNION SELECT subject_id,source_session_id FROM journals WHERE subject_id=?1
                UNION SELECT subject_id,session_id FROM journal_erased_sessions WHERE subject_id=?1", [subject])?;
            require_deletion(tx,subject)
        })
    }

    /// Host-only, content-free cleanup inventory. Usable after begin, including exact finish
    /// retries. Tombstones survive finish to prevent non-Subject permission fallback.
    pub fn deletion_subject_workers(&self, subject: &str) -> Result<Vec<(String, String)>> {
        self.database.try_with_connection(|tx| {
            require_deletion(tx,subject)?;
            Ok(tx.prepare("SELECT runtime_id,worker_id FROM subject_deleted_workers WHERE subject_id=?1 ORDER BY runtime_id,worker_id")?.query_map([subject],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })
    }

    /// Host-only Session identities; retained source rows are never exposed through this path.
    /// The Host must erase originals/archives before finish removes attribution.
    pub fn deletion_subject_sessions(&self, subject: &str) -> Result<Vec<String>> {
        self.database.try_with_connection(|tx| {
            require_deletion(tx,subject)?;
            Ok(tx.prepare("SELECT session_id FROM subject_deleted_sessions WHERE subject_id=?1 ORDER BY session_id")?.query_map([subject],|r|r.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })
    }

    /// Host-only Job cleanup inventory. Job storage owns inputs, outcomes and Worker archives;
    /// this store retains only these identities until Host cleanup has succeeded.
    pub fn deletion_subject_jobs(&self, subject: &str) -> Result<Vec<DeletionSubjectJob>> {
        self.database.try_with_connection(|tx| {
            require_deletion(tx,subject)?;
            Ok(tx.prepare("SELECT job_id,attempt_id FROM surface_generation_runs WHERE subject_id=?1 AND job_id IS NOT NULL AND attempt_id IS NOT NULL UNION SELECT g.job_id,g.attempt_id FROM journal_generations g JOIN journals j ON j.id=g.journal_id WHERE j.subject_id=?1 ORDER BY 1,2")?.query_map([subject],|r|Ok(DeletionSubjectJob {job_id:r.get(0)?,attempt_id:r.get(1)?}))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })
    }

    /// Host-only recovery state; this never exposes Subject content or authority.
    pub fn subject_deletion(&self, subject: &str) -> Result<Option<SubjectDeletion>> {
        self.database
            .try_with_connection(|tx| deletion(tx, subject))
    }

    /// Durable unfinished operations, including after Host restart. Completed tombstones
    /// remain queryable individually but are not returned as pending work.
    pub fn pending_subject_deletions(&self) -> Result<Vec<SubjectDeletion>> {
        self.database.try_with_connection(|tx| {
            Ok(tx.prepare("SELECT subject_id,operation_id,completed,started_at FROM subject_deletions WHERE completed=0 ORDER BY started_at,subject_id")?
                .query_map([], |r| Ok(SubjectDeletion { subject_id:r.get(0)?, operation_id:r.get(1)?, completed:r.get(2)?, started_at:r.get(3)? }))?
                .collect::<std::result::Result<Vec<_>,_>>()?)
        })
    }

    pub fn worker_subject_deleted(&self, runtime: &str, worker: &str) -> Result<bool> {
        self.database.try_with_connection(|tx| Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM subject_deleted_workers WHERE runtime_id=?1 AND worker_id=?2)",params![runtime,worker],|r|r.get(0))?))
    }

    /// Host confirmation that external cleanup is complete. All Subject content and completed
    /// receipt commit together. Deferred FKs allow cyclic current/history references to disappear
    /// without weakening FK enforcement outside this transaction. Failure rolls back all erasure.
    pub fn finish_subject_deletion(
        &self,
        subject: &str,
        operation: &str,
    ) -> Result<SubjectDeletion> {
        self.database.try_transaction(|tx| {
            let saved = require_deletion(tx, subject)?;
            if saved.operation_id != operation {
                return Err(SubjektivError::SubjectDeletionConflict(operation.into()));
            }
            if saved.completed {
                return Ok(saved);
            }
            tx.execute_batch("PRAGMA defer_foreign_keys=ON;")?;
            for (table, predicate) in INDIRECT {
                tx.execute(
                    &format!(
                        "DELETE FROM {table} WHERE {}",
                        predicate.replace("{subject}", "?1")
                    ),
                    [subject],
                )?;
            }
            for table in DIRECT {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE subject_id=?1"),
                    [subject],
                )?;
            }
            tx.execute(
                "UPDATE subject_deletions SET completed=1 WHERE subject_id=?1",
                [subject],
            )?;
            require_deletion(tx, subject)
        })
    }
}

#[cfg(test)]
mod tests;
