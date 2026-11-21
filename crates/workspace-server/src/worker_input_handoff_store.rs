//! Frozen input handoff pinned to the existing Worker removal intent. It retains
//! originals only until finite Host Jobs accept their bytes (or Subject deletion
//! fences admission). It never owns recording, checkpoint recovery or Job completion.
//! Physical removal phases are observations, not authority to release a writer.

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
pub struct WorkerInputHandoffMember {
    pub session_id: String,
    pub active_segment_id: Option<session_store::SegmentId>,
    pub materialized: bool,
    pub end: RecordingPosition,
    pub descriptors: Vec<session_store::public_material::PublicMaterialDescriptor>,
}

impl From<&runtime_api::WorkerRemovalSessionMember> for WorkerInputHandoffMember {
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
pub struct WorkerInputHandoffBinding {
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
pub enum WorkerInputHandoffPhase {
    AwaitingQuiescence,
    Recording,
    AwaitingEffectsEnd,
    Released,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputHandoffCause {
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
pub struct WorkerInputHandoffProgress {
    pub plan_id: String,
    pub operation_id: String,
    pub input_fingerprint: String,
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub metadata: WorkerInputHandoffMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerInputHandoffMetadata {
    pub binding: WorkerInputHandoffBinding,
    pub phase: WorkerInputHandoffPhase,
    pub cause: Option<WorkerInputHandoffCause>,
    /// None means the source is not yet frozen; Some(empty) is a confirmed empty scope.
    pub members: Option<Vec<WorkerInputHandoffMember>>,
    /// Absent until a verified archive owns the complete frozen originals.
    #[serde(rename = "retained_recording")]
    pub released_input_handoff: Option<ReleasedInputHandoff>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasedInputHandoff {
    pub archive_id: String,
    /// Historical recording completion, retained only for persisted compatibility.
    /// Even a completed legacy child must be imported into a finite owning Job.
    #[serde(rename = "completed")]
    pub legacy_completed: bool,
    pub cause: Option<WorkerInputHandoffCause>,
    #[serde(default)]
    pub handoff_status: InputHandoffStatus,
}

/// Only admission is tracked here. No state depends on recording Job completion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputHandoffStatus {
    #[default]
    Pending,
    Admitted,
    SubjectDeleted,
}

impl InputHandoffStatus {
    pub fn terminal(self) -> bool {
        self != Self::Pending
    }
}

pub(super) fn validate_handoff_transition(
    current: InputHandoffStatus,
    next: InputHandoffStatus,
    cause: Option<WorkerInputHandoffCause>,
) -> Result<()> {
    if (current.terminal() && current != next) || (next.terminal() && cause.is_some()) {
        return Err(invalid(
            "Terminal input handoff cannot be reopened or carry a failure",
        ));
    }
    Ok(())
}

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}

fn validate_binding(binding: &WorkerInputHandoffBinding) -> Result<()> {
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
    validate_plan(conn, plan)
}

fn validate_plan(conn: &Connection, plan: &crate::retention::WorkerRemovalPlan) -> Result<()> {
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

fn load(conn: &Connection, operation_id: &str) -> Result<Option<WorkerInputHandoffProgress>> {
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
            Ok(WorkerInputHandoffProgress {
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

fn save(
    conn: &Connection,
    operation_id: &str,
    metadata: &WorkerInputHandoffMetadata,
) -> Result<()> {
    let json =
        serde_json::to_string(metadata).map_err(|_| invalid("Invalid Journal removal metadata"))?;
    conn.execute(
        "UPDATE worker_removal_operations SET journal_removal_json=?1,updated_at=?2 WHERE operation_id=?3",
        params![json, chrono::Utc::now().to_rfc3339(), operation_id],
    )?;
    Ok(())
}

impl SqliteWorkspaceStore {
    /// Read retained recording progress without reconstructing an executable
    /// request from Worker metadata which may have been intentionally purged.
    pub fn get_worker_input_handoff_for_plan(
        &self,
        plan: &crate::retention::WorkerRemovalPlan,
    ) -> Result<Option<WorkerInputHandoffProgress>> {
        self.with_conn(|conn| {
            validate_plan(conn, plan)?;
            load(conn, &plan.operation_id)
        })
    }

    pub fn get_worker_input_handoff(
        &self,
        prepared: &PreparedWorkerRemoval,
    ) -> Result<Option<WorkerInputHandoffProgress>> {
        self.with_conn(|conn| {
            validate_prepared(conn, prepared)?;
            load(conn, &prepared.plan.operation_id)
        })
    }

    pub fn init_worker_input_handoff(
        &self,
        prepared: &PreparedWorkerRemoval,
        binding: &WorkerInputHandoffBinding,
    ) -> Result<WorkerInputHandoffProgress> {
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
                &WorkerInputHandoffMetadata {
                    binding: binding.clone(),
                    phase: WorkerInputHandoffPhase::AwaitingQuiescence,
                    cause: None,
                    members: None,
                    released_input_handoff: None,
                },
            )?;
            let current = load(&tx, &prepared.plan.operation_id)?.expect("saved removal intent");
            tx.commit()?;
            Ok(current)
        })
    }

    /// Bind once after Runtime has fenced input and confirmed quiescence. This
    /// stores only Session identities and ends; it does not attest Runtime quiescence.
    pub fn freeze_worker_input_handoff(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &WorkerInputHandoffProgress,
        members: &[WorkerInputHandoffMember],
    ) -> Result<WorkerInputHandoffProgress> {
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
        self.change_worker_input_handoff(prepared, expected, |metadata| {
            if let Some(frozen) = &metadata.members {
                if frozen != members {
                    return Err(invalid("Journal removal frozen scope changed"));
                }
                return Ok(());
            }
            if metadata.phase != WorkerInputHandoffPhase::AwaitingQuiescence {
                return Err(invalid("Journal removal is not awaiting quiescence"));
            }
            metadata.members = Some(members.to_vec());
            metadata.phase = WorkerInputHandoffPhase::Recording;
            metadata.cause = None;
            Ok(())
        })
    }

    /// Save source responsibility before releasing the producer. Replaying the same
    /// archive is harmless; a different archive cannot replace a retained source.
    pub fn retain_released_input_handoff(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &WorkerInputHandoffProgress,
        archive_id: &str,
    ) -> Result<WorkerInputHandoffProgress> {
        if archive_id.is_empty()
            || prepared.runtime_request.archive_id.as_deref() != Some(archive_id)
        {
            return Err(invalid("Journal archive differs from removal intent"));
        }
        self.change_worker_input_handoff(prepared, expected, |metadata| {
            if metadata.members.as_ref().is_none_or(Vec::is_empty) {
                return Err(invalid("Recording handoff has no frozen originals"));
            }
            if let Some(retained) = &metadata.released_input_handoff {
                if retained.archive_id != archive_id {
                    return Err(invalid("Recording archive identity changed"));
                }
            } else {
                metadata.released_input_handoff = Some(ReleasedInputHandoff {
                    archive_id: archive_id.into(),
                    legacy_completed: false,
                    cause: None,
                    handoff_status: InputHandoffStatus::Pending,
                });
            }
            Ok(())
        })
    }

    pub fn list_pending_released_input_handoffs(
        &self,
        workspace_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkerInputHandoffProgress>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("Recording page limit must be 1..=100"));
        }
        self.with_conn(|conn| {
            let ids = conn.prepare("SELECT operation_id FROM worker_removal_operations WHERE workspace_id=?1 AND json_type(journal_removal_json,'$.retained_recording')='object' AND COALESCE(json_extract(journal_removal_json,'$.retained_recording.handoff_status'),'pending')='pending' AND (?2 IS NULL OR operation_id>?2) ORDER BY operation_id LIMIT ?3")?
                .query_map(params![workspace_id,after,limit as i64], |r|r.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter().map(|id| load(conn,id)?.ok_or_else(|| invalid("Recording responsibility disappeared"))).collect()
        })
    }

    /// One-time admission receipt, including after physical removal completes.
    /// Legacy completed flags are never admission evidence. A terminal receipt
    /// removes this handoff from recovery without waiting for a Job checkpoint.
    pub fn update_released_input_handoff(
        &self,
        expected: &WorkerInputHandoffProgress,
        status: InputHandoffStatus,
        cause: Option<WorkerInputHandoffCause>,
    ) -> Result<WorkerInputHandoffProgress> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut current = load(&tx, &expected.operation_id)?
                .ok_or_else(|| invalid("Recording handoff missing"))?;
            if &current != expected {
                return Err(invalid("Recording progress CAS conflict"));
            }
            let recording = current
                .metadata
                .released_input_handoff
                .as_mut()
                .ok_or_else(|| invalid("Recording handoff missing"))?;
            validate_handoff_transition(recording.handoff_status, status, cause)?;
            recording.handoff_status = status;
            recording.cause = cause;
            save(&tx, &current.operation_id, &current.metadata)?;
            tx.commit()?;
            Ok(current)
        })
    }

    /// CAS progress only. The Server must read domain checkpoints, execution-end
    /// evidence and guarded writer ownership before recording release. Completed
    /// cannot be supplied here: it is derived from the canonical removal commit.
    pub fn update_worker_input_handoff_progress(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &WorkerInputHandoffProgress,
        phase: WorkerInputHandoffPhase,
        cause: Option<WorkerInputHandoffCause>,
    ) -> Result<WorkerInputHandoffProgress> {
        use WorkerInputHandoffPhase::*;
        self.change_worker_input_handoff(prepared, expected, |metadata| {
            let allowed = matches!(
                (metadata.phase, phase),
                (AwaitingQuiescence, AwaitingQuiescence)
                    | (Recording, Recording | AwaitingEffectsEnd | Released)
                    | (
                        AwaitingEffectsEnd,
                        AwaitingEffectsEnd | Recording | Released
                    )
                    | (Released, Released)
            );
            if !allowed
                || (phase == Released
                    && (cause.is_some() || metadata.released_input_handoff.is_none()))
            {
                return Err(invalid("Invalid Journal removal phase transition"));
            }
            metadata.phase = phase;
            metadata.cause = cause;
            Ok(())
        })
    }

    fn change_worker_input_handoff(
        &self,
        prepared: &PreparedWorkerRemoval,
        expected: &WorkerInputHandoffProgress,
        update: impl FnOnce(&mut WorkerInputHandoffMetadata) -> Result<()>,
    ) -> Result<WorkerInputHandoffProgress> {
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

    pub fn list_pending_worker_input_handoffs(
        &self,
        workspace_id: &str,
        after_operation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkerInputHandoffProgress>> {
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
        && progress.metadata.phase == WorkerInputHandoffPhase::Released
    {
        progress.metadata.cause = Some(WorkerInputHandoffCause::ExecutionUnconfirmed);
        save(conn, operation_id, &progress.metadata)?;
    }
    Ok(())
}

/// Physical completion remains the existing Runtime receipt/catalog transaction,
/// not a caller-supplied Journal completion assertion.
pub(crate) fn complete(conn: &Connection, operation_id: &str) -> Result<()> {
    if let Some(mut progress) = load(conn, operation_id)? {
        if progress.metadata.phase != WorkerInputHandoffPhase::Released {
            return Err(invalid("Journal writer has not been released"));
        }
        progress.metadata.phase = WorkerInputHandoffPhase::Completed;
        progress.metadata.cause = None;
        save(conn, operation_id, &progress.metadata)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_completed_recordings_are_import_input_not_admission_receipts() {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('a','user','a','A','t','t');
                INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('w','W','active','t','t','a');")?;
            for (id, completed, status) in [
                ("legacy-complete", true, None),
                ("legacy-unsubmitted", false, None),
                ("already-admitted", false, Some("admitted")),
                ("deleted", true, Some("subject_deleted")),
            ] {
                let mut json = serde_json::json!({
                    "binding": {"subject_id":"subject","writer_session_id":"session","writer_binding_id":"binding",
                        "database":{"scope":"workspace","feature":"subjektiv","physical":"physical"}},
                    "phase":"completed", "cause":null, "members":[],
                    "retained_recording":{"archive_id":"archive","completed":completed,"cause":null},
                });
                if let Some(status) = status { json["retained_recording"]["handoff_status"] = status.into(); }
                conn.execute("INSERT INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,diagnostics_disposition,blockers_json,state,reason,created_at,updated_at,journal_removal_json)
                    VALUES(?1,?1,'fp','w','r','worker','observed','policy','digest','archive','tombstone','forever','purge','[]','succeeded','cleanup','t','t',?2)", params![id,json.to_string()])?;
            }
            Ok(())
        }).unwrap();
        let page = store
            .list_pending_released_input_handoffs("w", None, 100)
            .unwrap();
        assert_eq!(
            page.iter()
                .map(|p| p.operation_id.as_str())
                .collect::<Vec<_>>(),
            ["legacy-complete", "legacy-unsubmitted"]
        );
        let legacy = &page[0];
        let retained = legacy.metadata.released_input_handoff.as_ref().unwrap();
        assert!(retained.legacy_completed);
        assert_eq!(retained.handoff_status, InputHandoffStatus::Pending);
        let terminal = store
            .update_released_input_handoff(legacy, InputHandoffStatus::SubjectDeleted, None)
            .unwrap();
        assert!(
            terminal
                .metadata
                .released_input_handoff
                .as_ref()
                .unwrap()
                .legacy_completed
        );
        assert!(
            store
                .update_released_input_handoff(&terminal, InputHandoffStatus::Pending, None)
                .is_err()
        );
        assert!(
            store
                .update_released_input_handoff(legacy, InputHandoffStatus::Admitted, None)
                .is_err(),
            "stale migration snapshot must be fenced"
        );
        assert_eq!(
            store
                .list_pending_released_input_handoffs("w", None, 100)
                .unwrap()
                .len(),
            1
        );
        let persisted = serde_json::to_value(&terminal.metadata).unwrap();
        assert_eq!(persisted["retained_recording"]["completed"], true);
        assert_eq!(
            persisted["retained_recording"]["handoff_status"],
            "subject_deleted"
        );
    }

    #[test]
    fn terminal_handoff_cannot_become_a_recording_retry() {
        for status in [
            InputHandoffStatus::Admitted,
            InputHandoffStatus::SubjectDeleted,
        ] {
            assert!(validate_handoff_transition(status, status, None).is_ok());
            assert!(
                validate_handoff_transition(status, InputHandoffStatus::Pending, None).is_err()
            );
            assert!(
                validate_handoff_transition(
                    status,
                    status,
                    Some(WorkerInputHandoffCause::JobPending)
                )
                .is_err()
            );
        }
    }

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
        let receipts = |conn: &Connection| {
            conn.prepare(
                "SELECT version,name,applied_at FROM __yoi_schema_migrations ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        let old_receipts = receipts(&conn);
        let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(plan.current_schema_version, 123);
        assert_eq!(plan.target_schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(
            plan.migrations
                .iter()
                .map(|step| (step.version, step.name.as_str()))
                .collect::<Vec<_>>(),
            MIGRATIONS
                .iter()
                .filter(|step| step.version > 123)
                .map(|step| (step.version, step.name))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            receipts(&conn),
            old_receipts,
            "preflight must retain receipt bytes"
        );
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            123,
            "preflight must not change source"
        );
        assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
        let final_receipts = receipts(&conn);
        assert_eq!(&final_receipts[..old_receipts.len()], old_receipts);
        assert_eq!(
            final_receipts.len(),
            old_receipts.len() + plan.migrations.len()
        );
        assert_eq!(
            final_receipts[old_receipts.len()..]
                .iter()
                .map(|(version, name, _)| (*version, name.as_str()))
                .collect::<Vec<_>>(),
            plan.migrations
                .iter()
                .map(|step| (step.version, step.name.as_str()))
                .collect::<Vec<_>>()
        );
        assert!(
            final_receipts[old_receipts.len()..]
                .iter()
                .all(|(_, _, at)| !at.is_empty())
        );
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
        reopened
            .with_conn(|conn| {
                assert_eq!(
                    receipts(conn),
                    final_receipts,
                    "reopen must not append or rewrite receipts"
                );
                assert_eq!(shape(conn)?, expected);
                Ok(())
            })
            .unwrap();
        assert!(
            reopened
                .list_pending_worker_input_handoffs("w", None, 100)
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
