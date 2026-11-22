//! One-time input handoff on the existing release intent. Frozen originals stay
//! retained until finite Host Job admission, not until recording completion.
//! Prepared scope cannot be admitted until disconnect/singleton release commit;
//! cancelled preparations never enter recovery. Legacy JSON is import input only.
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
pub(crate) struct SubjectReleaseInputHandoff {
    pub binding: WorkerInputHandoffBinding,
    pub members: Vec<WorkerInputHandoffMember>,
    #[serde(rename = "completed")]
    pub legacy_completed: bool,
    pub cause: Option<WorkerInputHandoffCause>,
    #[serde(default)]
    pub handoff_status: InputHandoffStatus,
    /// Admission and release of the original Session pins are separate checkpoints.
    #[serde(default)]
    pub original_pins_released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubjectReleaseInputHandoffProgress {
    pub workspace_id: String,
    pub worker: RuntimeWorkerRef,
    pub operation_id: String,
    pub released: bool,
    pub handoff: SubjectReleaseInputHandoff,
}

fn read(
    conn: &Connection,
    workspace: &str,
    operation: &str,
) -> Result<Option<SubjectReleaseInputHandoffProgress>> {
    let row = conn.query_row(
        "SELECT runtime_id,worker_id,completed,recording_json FROM subject_worker_releases WHERE workspace_id=?1 AND operation_id=?2 AND cancelled=0 AND recording_json IS NOT NULL",
        params![workspace, operation],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?,r.get::<_,String>(3)?)),
    ).optional()?;
    row.map(|(runtime, worker, released, json)| {
        Ok(SubjectReleaseInputHandoffProgress {
            workspace_id: workspace.into(),
            worker: RuntimeWorkerRef::new(runtime, worker),
            operation_id: operation.into(),
            released,
            handoff: serde_json::from_str(&json).map_err(|e| Error::Store(e.to_string()))?,
        })
    })
    .transpose()
}

impl SqliteWorkspaceStore {
    pub(crate) fn prepare_subject_release_input_handoff(
        &self,
        workspace: &str,
        lease: &WorkerSingletonLease,
        operation: &str,
        binding: &WorkerInputHandoffBinding,
        members: &[WorkerInputHandoffMember],
    ) -> Result<SubjectReleaseInputHandoffProgress> {
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
                if saved.handoff.binding != *binding || saved.handoff.members != members {
                    return Err(Error::RepositoryConflict("Release recording scope changed".into()));
                }
                return Ok(saved);
            }
            let metadata = SubjectReleaseInputHandoff {
                binding: binding.clone(), members: members.into(), legacy_completed: false, cause: None, handoff_status: InputHandoffStatus::Pending, original_pins_released: false,
            };
            let changed = tx.execute("UPDATE subject_worker_releases SET recording_json=?3 WHERE workspace_id=?1 AND operation_id=?2 AND recording_json IS NULL AND cancelled=0 AND completed=0",
                params![workspace,operation,serde_json::to_string(&metadata).map_err(|e|Error::Store(e.to_string()))?])?;
            if changed != 1 { return Err(Error::RepositoryConflict("Cannot add recording responsibility to a completed release".into())); }
            let result = read(&tx,workspace,operation)?.ok_or_else(|| Error::Store("Release recording save disappeared".into()))?;
            tx.commit()?;
            Ok(result)
        })
    }

    pub(crate) fn get_subject_release_input_handoff(
        &self,
        workspace: &str,
        worker: &RuntimeWorkerRef,
    ) -> Result<Option<SubjectReleaseInputHandoffProgress>> {
        self.with_conn(|conn| {
            let operation: Option<String> = conn.query_row("SELECT operation_id FROM subject_worker_releases WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND cancelled=0 AND recording_json IS NOT NULL",
                params![workspace,worker.runtime_id,worker.worker_id],|r|r.get(0)).optional()?;
            operation.map(|id|read(conn,workspace,&id)).transpose().map(Option::flatten)
        })
    }

    pub(crate) fn list_pending_subject_release_input_handoffs(
        &self,
        workspace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SubjectReleaseInputHandoffProgress>> {
        if !(1..=100).contains(&limit) {
            return Err(Error::InvalidInput(
                "Input handoff page limit must be 1..=100".into(),
            ));
        }
        self.with_conn(|conn| {
            let ids = conn.prepare("SELECT operation_id FROM subject_worker_releases WHERE workspace_id=?1 AND cancelled=0 AND completed=1 AND recording_json IS NOT NULL AND COALESCE(json_extract(recording_json,'$.handoff_status'),'pending')='pending' AND (?2 IS NULL OR operation_id>?2) ORDER BY operation_id LIMIT ?3")?
                .query_map(params![workspace,after,limit.min(100) as i64],|r|r.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter().map(|id| read(conn,workspace,&id)?.ok_or_else(||Error::Store("Release recording disappeared".into()))).collect()
        })
    }

    pub(crate) fn list_pending_release_original_pin_cleanup(
        &self,
        workspace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SubjectReleaseInputHandoffProgress>> {
        if !(1..=100).contains(&limit) {
            return Err(Error::InvalidInput(
                "Pin cleanup page limit must be 1..=100".into(),
            ));
        }
        self.with_conn(|conn| {
            let ids = conn.prepare("SELECT operation_id FROM subject_worker_releases WHERE workspace_id=?1 AND cancelled=0 AND completed=1 AND json_extract(recording_json,'$.handoff_status')='admitted' AND COALESCE(json_extract(recording_json,'$.original_pins_released'),0)=0 AND (?2 IS NULL OR operation_id>?2) ORDER BY operation_id LIMIT ?3")?
                .query_map(params![workspace,after,limit as i64],|r|r.get::<_,String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter().map(|id| read(conn,workspace,&id)?.ok_or_else(||Error::Store("Release pin cleanup disappeared".into()))).collect()
        })
    }

    pub(crate) fn complete_release_original_pin_cleanup(
        &self,
        expected: &SubjectReleaseInputHandoffProgress,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if read(&tx,&expected.workspace_id,&expected.operation_id)?.as_ref() != Some(expected)
                || !expected.released || expected.handoff.handoff_status != InputHandoffStatus::Admitted {
                return Err(Error::RepositoryConflict("Release pin cleanup owner changed".into()));
            }
            tx.execute("UPDATE subject_worker_releases SET recording_json=json_set(recording_json,'$.original_pins_released',json('true')) WHERE workspace_id=?1 AND operation_id=?2",
                params![expected.workspace_id,expected.operation_id])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub(crate) fn update_subject_release_input_handoff(
        &self,
        expected: &SubjectReleaseInputHandoffProgress,
        status: InputHandoffStatus,
        cause: Option<WorkerInputHandoffCause>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if read(&tx,&expected.workspace_id,&expected.operation_id)?.as_ref() != Some(expected) || !expected.released {
                return Err(Error::RepositoryConflict("Release recording changed or body not released".into()));
            }
            worker_input_handoff_store::validate_handoff_transition(expected.handoff.handoff_status, status, cause)?;
            // Preserve the frozen binding/member JSON representation checked by
            // the immutable-scope trigger, including legacy object key order.
            tx.execute("UPDATE subject_worker_releases SET recording_json=json_set(recording_json,'$.handoff_status',json(?3),'$.cause',json(?4)) WHERE workspace_id=?1 AND operation_id=?2",
                params![expected.workspace_id,expected.operation_id,
                    serde_json::to_string(&status).map_err(|e|Error::Store(e.to_string()))?,
                    serde_json::to_string(&cause).map_err(|e|Error::Store(e.to_string()))?])?;
            tx.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod input_handoff_tests {
    use super::*;

    #[test]
    fn admitted_release_retains_original_pin_cleanup_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('pin-owner','user','pin-owner','Owner','t','t');
                INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('pin-w','W','active','t','t','pin-owner');
                INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('pin-w','a','worker','runtime','fp','created','t','t','subjektiv:subject',1);")?;
            let legacy = serde_json::json!({
                "binding":{"subject_id":"subject","writer_session_id":"session","writer_binding_id":"binding",
                    "database":{"scope":"workspace","feature":"subjektiv","physical":"physical"}},
                "members":[], "completed":false, "cause":null,
            });
            conn.execute("INSERT INTO subject_worker_releases(workspace_id,runtime_id,worker_id,singleton_key,singleton_generation,binding_id,operation_id,proof,completed,recording_json) VALUES('pin-w','runtime','worker','subjektiv:subject',1,'binding','release','runtime_disconnected',1,?1)", [legacy.to_string()])?;
            Ok(())
        }).unwrap();
        let pending = store
            .list_pending_subject_release_input_handoffs("pin-w", None, 100)
            .unwrap()
            .pop()
            .unwrap();
        store
            .update_subject_release_input_handoff(&pending, InputHandoffStatus::Admitted, None)
            .unwrap();
        drop(store);
        // Crash after admission, or after only some Session unpins, must never
        // readmit recording and must retain the entire idempotent cleanup scope.
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert!(
            store
                .list_pending_subject_release_input_handoffs("pin-w", None, 100)
                .unwrap()
                .is_empty()
        );
        let cleanup = store
            .list_pending_release_original_pin_cleanup("pin-w", None, 100)
            .unwrap();
        assert_eq!(cleanup.len(), 1);
        assert!(!cleanup[0].handoff.original_pins_released);
        assert_eq!(cleanup[0].handoff.binding, pending.handoff.binding);
        assert!(
            store
                .complete_release_original_pin_cleanup(&pending)
                .is_err()
        );
        drop(store);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let cleanup = store
            .list_pending_release_original_pin_cleanup("pin-w", None, 100)
            .unwrap();
        assert_eq!(cleanup.len(), 1);
        store
            .complete_release_original_pin_cleanup(&cleanup[0])
            .unwrap();
        drop(store);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert!(
            store
                .list_pending_release_original_pin_cleanup("pin-w", None, 100)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .list_pending_subject_release_input_handoffs("pin-w", None, 100)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completed_legacy_release_requires_one_time_finite_job_admission() {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        store.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('handoff-owner','user','handoff-owner','Owner','t','t');
                INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('handoff-w','W','active','t','t','handoff-owner');
                INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('handoff-w','a','worker','runtime','fp','created','t','t','subjektiv:subject',1);")?;
            let legacy = serde_json::json!({
                "binding":{"subject_id":"subject","writer_session_id":"session","writer_binding_id":"binding",
                    "database":{"scope":"workspace","feature":"subjektiv","physical":"physical"}},
                "members":[], "completed":true, "cause":null,
            });
            conn.execute("INSERT INTO subject_worker_releases(workspace_id,runtime_id,worker_id,singleton_key,singleton_generation,binding_id,operation_id,proof,completed,recording_json) VALUES('handoff-w','runtime','worker','subjektiv:subject',1,'binding','legacy','runtime_disconnected',1,?1)", [legacy.to_string()])?;
            Ok(())
        }).unwrap();
        let page = store
            .list_pending_subject_release_input_handoffs("handoff-w", None, 100)
            .unwrap();
        assert_eq!(page.len(), 1);
        let legacy = &page[0];
        assert!(legacy.handoff.legacy_completed);
        assert_eq!(legacy.handoff.handoff_status, InputHandoffStatus::Pending);
        store
            .update_subject_release_input_handoff(legacy, InputHandoffStatus::SubjectDeleted, None)
            .unwrap();
        assert!(
            store
                .list_pending_subject_release_input_handoffs("handoff-w", None, 100)
                .unwrap()
                .is_empty()
        );
        let terminal = store
            .get_subject_release_input_handoff("handoff-w", &legacy.worker)
            .unwrap()
            .unwrap();
        assert!(terminal.handoff.legacy_completed);
        assert_eq!(
            terminal.handoff.handoff_status,
            InputHandoffStatus::SubjectDeleted
        );
        assert!(
            store
                .update_subject_release_input_handoff(&terminal, InputHandoffStatus::Pending, None)
                .is_err()
        );
        assert!(
            store
                .update_subject_release_input_handoff(legacy, InputHandoffStatus::Admitted, None)
                .is_err()
        );
    }
}
