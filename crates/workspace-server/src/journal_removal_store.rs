//! Journal scope pinned to the existing Worker removal intent. Checkpoints and
//! execution-end evidence stay in their owning domains; these phases are progress
//! observations, never authority to release a writer or remove a Runtime Worker.

use super::*;
use crate::retention::PreparedWorkerRemoval;
use feature_storage::DatabaseIdentity;
use session_store::capture::RecordingPosition;

pub(super) const MIGRATION_NAME: &str = "Journal frozen scope on Worker removal intents";
pub(super) const SCHEMA_SQL: &str = r#"
ALTER TABLE worker_removal_operations ADD COLUMN journal_removal_json TEXT
    CHECK(journal_removal_json IS NULL OR (
        json_valid(journal_removal_json)
        AND json_extract(journal_removal_json,'$.phase') IN
            ('awaiting_quiescence','recording','awaiting_effects_end','released','completed')
    ));
CREATE UNIQUE INDEX worker_removal_journal_active
    ON worker_removal_operations(workspace_id,runtime_id,worker_id)
    WHERE journal_removal_json IS NOT NULL AND state IN ('planned','executing','failed');
CREATE TRIGGER worker_removal_journal_immutable
BEFORE UPDATE ON worker_removal_operations
WHEN OLD.journal_removal_json IS NOT NULL AND (
    NEW.journal_removal_json IS NULL
    OR NEW.operation_id!=OLD.operation_id OR NEW.plan_id!=OLD.plan_id
    OR NEW.input_fingerprint!=OLD.input_fingerprint
    OR NEW.workspace_id!=OLD.workspace_id OR NEW.runtime_id!=OLD.runtime_id
    OR NEW.worker_id!=OLD.worker_id
    OR json_extract(NEW.journal_removal_json,'$.binding')
        IS NOT json_extract(OLD.journal_removal_json,'$.binding')
    OR (json_type(OLD.journal_removal_json,'$.members')='array'
        AND json_extract(NEW.journal_removal_json,'$.members')
            IS NOT json_extract(OLD.journal_removal_json,'$.members'))
)
BEGIN SELECT RAISE(ABORT,'Journal removal identity is immutable'); END;
"#;

/// One observed intent, including a held recovery error. Keeping the original row
/// identity lets discovery advance its cursor without authorizing an ambiguous owner.
#[derive(Debug)]
pub struct PendingWorkerRemovalIntent {
    pub operation_id: String,
    pub worker: RuntimeWorkerRef,
    pub preparation: Result<PreparedWorkerRemoval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenJournalRemovalMember {
    pub session_id: String,
    pub active_segment_id: Option<session_store::SegmentId>,
    pub materialized: bool,
    pub end: RecordingPosition,
    pub descriptors: Vec<session_store::public_material::PublicMaterialDescriptor>,
}

impl From<&runtime_api::WorkerRemovalSessionMember> for FrozenJournalRemovalMember {
    fn from(member: &runtime_api::WorkerRemovalSessionMember) -> Self {
        Self {
            session_id: member.session_id.to_string(),
            active_segment_id: member.active_segment_id,
            materialized: member.materialized,
            end: member.end,
            descriptors: member.descriptors.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRemovalBinding {
    pub subject_id: String,
    pub writer_session_id: String,
    pub writer_binding_id: String,
    #[serde(with = "database_identity::Identity")]
    pub database: DatabaseIdentity,
}

mod database_identity {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(remote = "DatabaseIdentity", deny_unknown_fields)]
    pub struct Identity {
        pub scope: String,
        pub feature: String,
        pub physical: String,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalRemovalPhase {
    AwaitingQuiescence,
    Recording,
    AwaitingEffectsEnd,
    Released,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalRemovalCause {
    RecordingDisabled,
    SourceUnavailable,
    OwnershipUnknown,
    JobPending,
    JobFailed,
    ExecutionUnconfirmed,
    StorageUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRemovalProgress {
    pub plan_id: String,
    pub operation_id: String,
    pub input_fingerprint: String,
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub metadata: JournalRemovalMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRemovalMetadata {
    pub binding: JournalRemovalBinding,
    pub phase: JournalRemovalPhase,
    pub cause: Option<JournalRemovalCause>,
    /// None means the source is not yet frozen; Some(empty) is a confirmed empty scope.
    pub members: Option<Vec<FrozenJournalRemovalMember>>,
}

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}

fn validate_binding(binding: &JournalRemovalBinding) -> Result<()> {
    for value in [
        &binding.subject_id,
        &binding.writer_session_id,
        &binding.writer_binding_id,
        &binding.database.scope,
        &binding.database.feature,
        &binding.database.physical,
    ] {
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(invalid("Invalid Journal removal binding identifier"));
        }
    }
    Ok(())
}

fn validate_prepared(conn: &Connection, prepared: &PreparedWorkerRemoval) -> Result<()> {
    let plan = &prepared.plan;
    let request = &prepared.runtime_request;
    if request.operation_id != plan.operation_id
        || request.input_fingerprint != plan.input_fingerprint
        || request.workspace_id != plan.workspace_id
        || request.source_runtime_id != plan.worker.runtime_id
        || request.worker_id.to_string() != plan.worker.worker_id
    {
        return Err(invalid("Journal removal prepared scope mismatch"));
    }
    let exact: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_removal_operations
         WHERE plan_id=?1 AND operation_id=?2 AND input_fingerprint=?3
           AND workspace_id=?4 AND runtime_id=?5 AND worker_id=?6)",
        params![
            plan.plan_id,
            plan.operation_id,
            plan.input_fingerprint,
            plan.workspace_id,
            plan.worker.runtime_id,
            plan.worker.worker_id
        ],
        |row| row.get(0),
    )?;
    if !exact {
        return Err(invalid("Journal removal does not match existing plan"));
    }
    Ok(())
}

fn load(conn: &Connection, operation_id: &str) -> Result<Option<JournalRemovalProgress>> {
    let row = conn.query_row(
        "SELECT plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,journal_removal_json
         FROM worker_removal_operations WHERE operation_id=?1 AND journal_removal_json IS NOT NULL",
        [operation_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
            row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?,
            row.get::<_, String>(5)?)),
    ).optional()?;
    row.map(
        |(plan_id, input_fingerprint, workspace_id, runtime_id, worker_id, json)| {
            let metadata = serde_json::from_str(&json)
                .map_err(|_| Error::Store("Invalid Journal removal metadata".into()))?;
            Ok(JournalRemovalProgress {
                plan_id,
                operation_id: operation_id.into(),
                input_fingerprint,
                workspace_id,
                worker: RuntimeWorkerRef {
                    runtime_id,
                    worker_id,
                },
                metadata,
            })
        },
    )
    .transpose()
}

fn save(conn: &Connection, operation_id: &str, metadata: &JournalRemovalMetadata) -> Result<()> {
    let json =
        serde_json::to_string(metadata).map_err(|_| invalid("Invalid Journal removal metadata"))?;
    conn.execute(
        "UPDATE worker_removal_operations SET journal_removal_json=?1,updated_at=?2 WHERE operation_id=?3",
        params![json, chrono::Utc::now().to_rfc3339(), operation_id],
    )?;
    Ok(())
}

impl SqliteWorkspaceStore {
    pub fn get_journal_removal(
        &self,
        prepared: &PreparedWorkerRemoval,
    ) -> Result<Option<JournalRemovalProgress>> {
        self.with_conn(|conn| {
            validate_prepared(conn, prepared)?;
            load(conn, &prepared.plan.operation_id)
        })
    }

    pub fn init_journal_removal(
        &self,
        prepared: &PreparedWorkerRemoval,
        binding: &JournalRemovalBinding,
    ) -> Result<JournalRemovalProgress> {
        validate_binding(binding)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            validate_prepared(&tx, prepared)?;
            if let Some(current) = load(&tx, &prepared.plan.operation_id)? {
                if &current.metadata.binding != binding {
                    return Err(invalid("Journal removal binding changed"));
                }
                tx.commit()?;
                return Ok(current);
            }
            let executable: bool = tx.query_row(
                "SELECT state='executing' FROM worker_removal_operations WHERE operation_id=?1",
                [&prepared.plan.operation_id],
                |row| row.get(0),
            )?;
            if !executable {
                return Err(invalid(
                    "Journal removal requires an executing prepared intent",
                ));
            }
            save(
                &tx,
                &prepared.plan.operation_id,
                &JournalRemovalMetadata {
                    binding: binding.clone(),
                    phase: JournalRemovalPhase::AwaitingQuiescence,
                    cause: None,
                    members: None,
                },
            )?;
            let current = load(&tx, &prepared.plan.operation_id)?.expect("saved removal intent");
            tx.commit()?;
            Ok(current)
        })
    }

    /// Bind once after Runtime has fenced input and confirmed quiescence. This
    /// stores only Session identities and ends; it does not attest Runtime quiescence.
    pub fn freeze_journal_removal(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &JournalRemovalProgress,
        members: &[FrozenJournalRemovalMember],
    ) -> Result<JournalRemovalProgress> {
        let mut seen = HashSet::new();
        for member in members {
            if member.session_id.is_empty()
                || member.session_id.len() > 512
                || member.session_id.chars().any(char::is_control)
                || !seen.insert(&member.session_id)
            {
                return Err(invalid("Invalid or duplicate frozen Session identity"));
            }
        }
        self.change_journal_removal(prepared, expected, |metadata| {
            if let Some(frozen) = &metadata.members {
                if frozen != members {
                    return Err(invalid("Journal removal frozen scope changed"));
                }
                return Ok(());
            }
            if metadata.phase != JournalRemovalPhase::AwaitingQuiescence {
                return Err(invalid("Journal removal is not awaiting quiescence"));
            }
            metadata.members = Some(members.to_vec());
            metadata.phase = JournalRemovalPhase::Recording;
            metadata.cause = None;
            Ok(())
        })
    }

    /// CAS progress only. The Server must read domain checkpoints, execution-end
    /// evidence and guarded writer ownership before recording release. Completed
    /// cannot be supplied here: it is derived from the canonical removal commit.
    pub fn update_journal_removal_progress(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &JournalRemovalProgress,
        phase: JournalRemovalPhase,
        cause: Option<JournalRemovalCause>,
    ) -> Result<JournalRemovalProgress> {
        use JournalRemovalPhase::*;
        self.change_journal_removal(prepared, expected, |metadata| {
            let allowed = matches!(
                (metadata.phase, phase),
                (AwaitingQuiescence, AwaitingQuiescence)
                    | (Recording, Recording | AwaitingEffectsEnd)
                    | (
                        AwaitingEffectsEnd,
                        AwaitingEffectsEnd | Recording | Released
                    )
                    | (Released, Released)
            );
            if !allowed || (phase == Released && cause.is_some()) {
                return Err(invalid("Invalid Journal removal phase transition"));
            }
            metadata.phase = phase;
            metadata.cause = cause;
            Ok(())
        })
    }

    fn change_journal_removal(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &JournalRemovalProgress,
        update: impl FnOnce(&mut JournalRemovalMetadata) -> Result<()>,
    ) -> Result<JournalRemovalProgress> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            validate_prepared(&tx, prepared)?;
            let mut current = load(&tx, &prepared.plan.operation_id)?
                .ok_or_else(|| invalid("Journal removal is not initialized"))?;
            if &current != expected {
                return Err(invalid("Journal removal progress CAS conflict"));
            }
            let active: bool = tx.query_row(
                "SELECT state IN ('executing','failed') FROM worker_removal_operations WHERE operation_id=?1",
                [&current.operation_id], |row| row.get(0),
            )?;
            if !active {
                return Err(invalid("Journal removal intent is no longer active"));
            }
            update(&mut current.metadata)?;
            save(&tx, &current.operation_id, &current.metadata)?;
            tx.commit()?;
            Ok(current)
        })
    }

    /// Read-only continuation discovery, including intent saved before Journal
    /// binding initialization. A prepared snapshot is not execution authority:
    /// continuation must still begin/revalidate the existing plan and its blockers.
    pub fn list_pending_worker_removal_intents(
        &self,
        workspace_id: &str,
        after_operation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PendingWorkerRemovalIntent>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("Worker removal page limit must be 1..=100"));
        }
        let candidates = self.with_conn(|conn| {
            conn.prepare(
                "SELECT removal.operation_id,removal.input_fingerprint,removal.runtime_id,removal.worker_id,
                    (SELECT COUNT(*) FROM worker_removal_operations other
                     WHERE other.workspace_id=removal.workspace_id
                       AND other.runtime_id=removal.runtime_id AND other.worker_id=removal.worker_id
                       AND other.state IN ('planned','executing','failed'))
                 FROM worker_removal_operations removal
                 WHERE removal.workspace_id=?1 AND removal.state IN ('planned','executing','failed')
                   AND (?2 IS NULL OR removal.operation_id>?2)
                   AND EXISTS(SELECT 1 FROM worker_create_reservations lease
                       WHERE lease.workspace_id=removal.workspace_id
                         AND lease.runtime_id=removal.runtime_id AND lease.worker_id=removal.worker_id
                         AND substr(lease.singleton_key,1,10)='subjektiv:'
                         AND length(lease.singleton_key)>10 AND lease.singleton_generation>0
                         AND lease.state!='removed')
                 ORDER BY removal.operation_id LIMIT ?3",
            )?.query_map(params![workspace_id, after_operation_id, limit as i64], |row| {
                Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,
                    RuntimeWorkerRef { runtime_id: row.get(2)?, worker_id: row.get(3)? }, row.get::<_,u64>(4)?))
            })?.collect::<rusqlite::Result<Vec<_>>>().map_err(Error::from)
        })?;
        Ok(candidates
            .into_iter()
            .map(|(operation_id, fingerprint, worker, count)| {
                let preparation = (|| {
                    if count != 1 {
                        return Err(invalid("Ambiguous pending Worker removal intents"));
                    }
                    let prepared = self
                        .recover_worker_removal_execution(workspace_id, &worker)
                        .map_err(|_| invalid("Pending Worker removal intent cannot be recovered"))?
                        .ok_or_else(|| invalid("Pending Worker removal intent is stale"))?;
                    if prepared.plan.operation_id != operation_id
                        || prepared.plan.input_fingerprint != fingerprint
                    {
                        return Err(invalid(
                            "Pending Worker removal intent differs from canonical recovery",
                        ));
                    }
                    Ok(prepared)
                })();
                PendingWorkerRemovalIntent {
                    operation_id,
                    worker,
                    preparation,
                }
            })
            .collect())
    }

    pub fn list_pending_journal_removals(
        &self,
        workspace_id: &str,
        after_operation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<JournalRemovalProgress>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("Journal removal page limit must be 1..=100"));
        }
        self.with_conn(|conn| {
            let ids = conn
                .prepare(
                    "SELECT operation_id FROM worker_removal_operations WHERE workspace_id=?1
                 AND journal_removal_json IS NOT NULL AND state IN ('planned','executing','failed')
                 AND (?2 IS NULL OR operation_id>?2) ORDER BY operation_id LIMIT ?3",
                )?
                .query_map(
                    params![workspace_id, after_operation_id, limit as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter()
                .map(|id| {
                    load(conn, id)?
                        .ok_or_else(|| Error::Store("Journal removal disappeared".into()))
                })
                .collect()
        })
    }
}

#[cfg(test)]
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 123 {
        return Err(Error::Store(
            "Journal removal migration requires schema 123".into(),
        ));
    }
    storage_formats::verify_history(conn, 123)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_SQL)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(124,?1)",
        [MIGRATION_NAME],
    )?;
    storage_formats::verify_history(&tx, 124)?;
    tx.commit()?;
    Ok(())
}

// Canonical and retained failure histories have actually installed all three
// extensions at 126. The published Journal-124 branch instead jumps directly
// to 127 in one transaction, retaining its original receipt and frozen values.
pub(super) const RETAINED_BRIDGE_NAME: &str =
    "Startup Job and Service causes after retained Journal removal";

pub(super) fn migrate_to_v127(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 126 {
        return Err(Error::Store(
            "Journal removal migration requires schema 126".into(),
        ));
    }
    storage_formats::verify_history(conn, 126)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_SQL)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(127,?1)",
        [MIGRATION_NAME],
    )?;
    storage_formats::verify_history(&tx, 127)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn migrate_retained_to_v127(conn: &Connection) -> Result<()> {
    storage_formats::verify_retained_journal_history(conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("worker_create_failure_schema.sql"))?;
    tx.execute_batch(backend_job_recovery_store::CAUSE_SCHEMA)?;
    tx.execute_batch(crate::plugin_service::FAILURE_MIGRATION_SQL)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(127,?1)",
        [RETAINED_BRIDGE_NAME],
    )?;
    storage_formats::verify_history(&tx, 127)?;
    tx.commit()?;
    Ok(())
}

/// Called inside the existing fingerprint-fenced failure transaction. Released
/// is retained as an observation, not success: a subsequent physical failure must
/// remain visible without rewriting frozen scope or re-opening writer authority.
pub(crate) fn record_physical_failure(conn: &Connection, operation_id: &str) -> Result<()> {
    if let Some(mut progress) = load(conn, operation_id)?
        && progress.metadata.phase == JournalRemovalPhase::Released
    {
        progress.metadata.cause = Some(JournalRemovalCause::ExecutionUnconfirmed);
        save(conn, operation_id, &progress.metadata)?;
    }
    Ok(())
}

/// Physical completion remains the existing Runtime receipt/catalog transaction,
/// not a caller-supplied Journal completion assertion.
pub(crate) fn complete(conn: &Connection, operation_id: &str) -> Result<()> {
    if let Some(mut progress) = load(conn, operation_id)? {
        if progress.metadata.phase != JournalRemovalPhase::Released {
            return Err(invalid("Journal writer has not been released"));
        }
        progress.metadata.phase = JournalRemovalPhase::Completed;
        progress.metadata.cause = None;
        save(conn, operation_id, &progress.metadata)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(conn: &Connection) -> Result<Vec<(String, String)>> {
        Ok(conn
            .prepare(
                "SELECT name,sql FROM sqlite_schema WHERE name IN
             ('worker_removal_journal_active','worker_removal_journal_immutable') ORDER BY name",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    #[test]
    fn journal_removal_migration_preserves_old_intent_and_matches_fresh_schema() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        super::super::tests::prepare_retained_schema(&path, 123);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('a','user','a','A','t','t');
            INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('w','W','active','t','t','a');
            INSERT INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,diagnostics_disposition,blockers_json,state,reason,failure_category,created_at,updated_at)
            VALUES('op','plan','fp','w','r','worker','observed','policy','digest','archive','tombstone','forever','purge','[]','failed','cleanup','execution_unconfirmed','t','t');").unwrap();
        let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(plan.current_schema_version, 123);
        assert_eq!(
            plan.migrations
                .iter()
                .map(|step| step.version)
                .collect::<Vec<_>>(),
            vec![124, 125, 126, 127, 128, 129, 130]
        );
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            123,
            "preflight must not change source"
        );
        SqliteWorkspaceStore::migrate_database(&path).unwrap();
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            super::LATEST_SCHEMA_VERSION
        );
        let saved = conn.query_row(
            "SELECT plan_id,input_fingerprint,state,failure_category,journal_removal_json FROM worker_removal_operations WHERE operation_id='op'", [],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?)),
        ).unwrap();
        assert_eq!(
            saved,
            (
                "plan".into(),
                "fp".into(),
                "failed".into(),
                "execution_unconfirmed".into(),
                None
            )
        );
        let fresh = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        let expected = fresh.with_conn(shape).unwrap();
        assert_eq!(shape(&conn).unwrap(), expected);
        assert_eq!(expected.len(), 2);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        drop(conn);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert!(
            reopened
                .list_pending_journal_removals("w", None, 100)
                .unwrap()
                .is_empty(),
            "migration must not invent Journal intent or evidence"
        );
    }

    #[test]
    fn journal_removal_migration_rolls_back_ddl_and_receipt_then_resumes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        super::super::tests::prepare_retained_schema(&path, 123);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_journal_migration_receipt BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=124 BEGIN SELECT RAISE(ABORT,'injected Journal migration receipt failure'); END;",
        )
        .unwrap();
        assert!(
            migrate(&conn)
                .unwrap_err()
                .to_string()
                .contains("injected Journal migration receipt failure")
        );
        assert_eq!(current_schema_version(&conn).unwrap(), 123);
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(worker_removal_operations)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(!columns.iter().any(|name| name == "journal_removal_json"));
        assert_eq!(
            shape(&conn).unwrap().len(),
            0,
            "partial DDL must roll back with the failed receipt"
        );
        conn.execute_batch("DROP TRIGGER reject_journal_migration_receipt;")
            .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 124);
        assert_eq!(shape(&conn).unwrap().len(), 2);
    }
}
