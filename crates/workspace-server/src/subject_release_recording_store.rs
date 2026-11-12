//! The existing release intent retains recording responsibility independently of
//! body occupancy. Prepared scope is not dispatch authority until disconnect and
//! singleton release commit; cancelled preparations never enter recovery.
use super::*;

#[cfg(test)]
#[path = "subject_release_recording_tests.rs"]
mod tests;

pub(super) const MIGRATION_NAME: &str = "Retained Journal scope on Subject release intents";
pub(super) const SCHEMA_SQL: &str = r#"
ALTER TABLE subject_worker_releases ADD COLUMN recording_json TEXT
    CHECK(recording_json IS NULL OR json_valid(recording_json));
CREATE TRIGGER subject_release_recording_immutable BEFORE UPDATE ON subject_worker_releases
WHEN OLD.recording_json IS NOT NULL AND (
    NEW.recording_json IS NULL OR
    NEW.workspace_id != OLD.workspace_id OR NEW.runtime_id != OLD.runtime_id OR
    NEW.worker_id != OLD.worker_id OR NEW.operation_id != OLD.operation_id OR
    NEW.singleton_key != OLD.singleton_key OR NEW.singleton_generation != OLD.singleton_generation OR
    NEW.binding_id != OLD.binding_id OR
    json_extract(NEW.recording_json,'$.binding') IS NOT json_extract(OLD.recording_json,'$.binding') OR
    json_extract(NEW.recording_json,'$.members') IS NOT json_extract(OLD.recording_json,'$.members') OR
    (json_extract(OLD.recording_json,'$.completed')=1 AND json_extract(NEW.recording_json,'$.completed') IS NOT 1)
) BEGIN SELECT RAISE(ABORT,'Subject release recording identity is immutable'); END;
"#;

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 134 {
        return Err(Error::Store("Release recording requires schema 134".into()));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_SQL)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(135,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseJournalRecording {
    pub binding: JournalRemovalBinding,
    pub members: Vec<FrozenJournalRemovalMember>,
    pub completed: bool,
    pub cause: Option<JournalRemovalCause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseJournalRecordingProgress {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub operation_id: String,
    pub released: bool,
    pub recording: ReleaseJournalRecording,
}

fn read(
    conn: &Connection,
    workspace: &str,
    operation: &str,
) -> Result<Option<ReleaseJournalRecordingProgress>> {
    let row = conn.query_row(
        "SELECT runtime_id,worker_id,completed,recording_json FROM subject_worker_releases WHERE workspace_id=?1 AND operation_id=?2 AND cancelled=0 AND recording_json IS NOT NULL",
        params![workspace, operation],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?,r.get::<_,String>(3)?)),
    ).optional()?;
    row.map(|(runtime, worker, released, json)| {
        Ok(ReleaseJournalRecordingProgress {
            workspace_id: workspace.into(),
            worker: RuntimeWorkerRef::new(runtime, worker),
            operation_id: operation.into(),
            released,
            recording: serde_json::from_str(&json).map_err(|e| Error::Store(e.to_string()))?,
        })
    })
    .transpose()
}

impl SqliteWorkspaceStore {
    pub(crate) fn prepare_release_journal_recording(
        &self,
        workspace: &str,
        lease: &WorkerSingletonLease,
        operation: &str,
        binding: &JournalRemovalBinding,
        members: &[FrozenJournalRemovalMember],
    ) -> Result<ReleaseJournalRecordingProgress> {
        if members.is_empty()
            || !members
                .iter()
                .any(|m| m.session_id == binding.writer_session_id)
        {
            return Err(Error::InvalidInput(
                "Release recording scope is absent".into(),
            ));
        }
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM subject_worker_releases WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND singleton_key=?4 AND singleton_generation=?5 AND binding_id=?6 AND operation_id=?7 AND cancelled=0)",
                params![workspace,lease.worker.runtime_id,lease.worker.worker_id,lease.key,lease.generation as i64,binding.writer_binding_id,operation],|r|r.get(0))?;
            if !active { return Err(Error::RepositoryConflict("Release recording intent changed".into())); }
            if let Some(saved) = read(&tx,workspace,operation)? {
                if saved.recording.binding != *binding || saved.recording.members != members {
                    return Err(Error::RepositoryConflict("Release recording scope changed".into()));
                }
                return Ok(saved);
            }
            let metadata = ReleaseJournalRecording {
                binding: binding.clone(), members: members.into(), completed: false, cause: None,
            };
            let changed = tx.execute("UPDATE subject_worker_releases SET recording_json=?3 WHERE workspace_id=?1 AND operation_id=?2 AND recording_json IS NULL AND cancelled=0 AND completed=0",
                params![workspace,operation,serde_json::to_string(&metadata).map_err(|e|Error::Store(e.to_string()))?])?;
            if changed != 1 { return Err(Error::RepositoryConflict("Cannot add recording responsibility to a completed release".into())); }
            let result = read(&tx,workspace,operation)?.ok_or_else(|| Error::Store("Release recording save disappeared".into()))?;
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn get_release_journal_recording(
        &self,
        workspace: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<ReleaseJournalRecordingProgress>> {
        self.with_conn(|conn| {
            let operation: Option<String> = conn.query_row("SELECT operation_id FROM subject_worker_releases WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND cancelled=0 AND recording_json IS NOT NULL",
                params![workspace,worker.runtime_id,worker.worker_id],|r|r.get(0)).optional()?;
            operation.map(|id|read(conn,workspace,&id)).transpose().map(Option::flatten)
        })
    }

    pub(crate) fn list_pending_release_journal_recordings(
        &self,
        workspace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ReleaseJournalRecordingProgress>> {
        self.with_conn(|conn| {
            let ids = conn.prepare("SELECT operation_id FROM subject_worker_releases WHERE workspace_id=?1 AND cancelled=0 AND completed=1 AND json_extract(recording_json,'$.completed')=0 AND (?2 IS NULL OR operation_id>?2) ORDER BY operation_id LIMIT ?3")?
                .query_map(params![workspace,after,limit.min(100) as i64],|r|r.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter().map(|id| read(conn,workspace,&id)?.ok_or_else(||Error::Store("Release recording disappeared".into()))).collect()
        })
    }

    pub(crate) fn update_release_journal_recording(
        &self,
        expected: &ReleaseJournalRecordingProgress,
        completed: bool,
        cause: Option<JournalRemovalCause>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if read(&tx,&expected.workspace_id,&expected.operation_id)?.as_ref() != Some(expected) || !expected.released {
                return Err(Error::RepositoryConflict("Release recording changed or body not released".into()));
            }
            let mut next = expected.recording.clone();
            next.completed = completed;
            next.cause = cause;
            tx.execute("UPDATE subject_worker_releases SET recording_json=?3 WHERE workspace_id=?1 AND operation_id=?2",
                params![expected.workspace_id,expected.operation_id,serde_json::to_string(&next).map_err(|e|Error::Store(e.to_string()))?])?;
            tx.commit()?;
            Ok(())
        })
    }
}
