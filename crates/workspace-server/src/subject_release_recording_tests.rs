//! SQLite component tests: lifecycle proofs are supplied at the storage boundary;
//! real Controller acknowledgement is covered by the Server release integration suite.
use super::*;

fn seed(conn: &Connection) -> Result<()> {
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('release-owner','user','release-owner','Owner','t','t');
        INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('release-workspace','W','active','t','t','release-owner');
        INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES('release-workspace','subjektiv:subject','runtime','worker',1,'t','t');
        INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation) VALUES('release-workspace','a','worker','runtime','fp','created','t','t','subjektiv:subject',1);")?;
    Ok(())
}
fn lease() -> WorkerSingletonLease {
    WorkerSingletonLease {
        worker: RuntimeWorkerRef::new("runtime", "worker"),
        key: "subjektiv:subject".into(),
        generation: 1,
    }
}
fn binding() -> JournalRemovalBinding {
    JournalRemovalBinding {
        subject_id: "subject".into(),
        writer_session_id: "session".into(),
        writer_binding_id: "binding".into(),
        database: feature_storage::DatabaseIdentity {
            scope: "workspace".into(),
            feature: "subjektiv".into(),
            physical: "physical".into(),
        },
    }
}
fn members() -> Vec<FrozenJournalRemovalMember> {
    vec![FrozenJournalRemovalMember {
        session_id: "session".into(),
        active_segment_id: None,
        materialized: false,
        end: session_store::capture::RecordingPosition {
            canonical_order: 0,
            byte_offset: 0,
        },
        descriptors: vec![],
    }]
}

#[test]
fn release_recording_preparation_cancel_reopen_and_completion_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
    store.with_conn(seed).unwrap();
    let lease = lease();
    let binding = binding();
    let members = members();
    let workspace = "release-workspace";
    store
        .reserve_subject_release(workspace, &lease, "binding", "cancel")
        .unwrap();
    let cancelled = store
        .prepare_release_journal_recording(workspace, &lease, "cancel", &binding, &members)
        .unwrap();
    assert!(!cancelled.released);
    assert!(
        store
            .list_pending_release_journal_recordings(workspace, None, 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .update_release_journal_recording(&cancelled, true, None)
            .is_err()
    );
    store
        .cancel_subject_release(workspace, &lease, "binding", "cancel")
        .unwrap();
    assert!(
        store
            .get_release_journal_recording(workspace, &lease.worker)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .prepare_release_journal_recording(workspace, &lease, "cancel", &binding, &members)
            .is_err()
    );
    store
        .reserve_subject_release(workspace, &lease, "binding", "release")
        .unwrap();
    let prepared = store
        .prepare_release_journal_recording(workspace, &lease, "release", &binding, &members)
        .unwrap();
    assert_eq!(
        store
            .prepare_release_journal_recording(workspace, &lease, "release", &binding, &members)
            .unwrap(),
        prepared
    );
    let mut different = binding.clone();
    different.database.physical = "other".into();
    assert!(
        store
            .prepare_release_journal_recording(workspace, &lease, "release", &different, &members)
            .is_err()
    );
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        store
            .get_release_journal_recording(workspace, &lease.worker)
            .unwrap(),
        Some(prepared.clone())
    );
    store
        .confirm_subject_release_proof(workspace, &lease, "binding", "release", false)
        .unwrap();
    store
        .release_subject_singleton(workspace, &lease, "binding", "release")
        .unwrap();
    assert!(
        store
            .update_release_journal_recording(&prepared, true, None)
            .is_err(),
        "stale pre-release observation cannot complete recording"
    );
    let released = store
        .get_release_journal_recording(workspace, &lease.worker)
        .unwrap()
        .unwrap();
    assert!(released.released && !released.recording.completed);
    assert_eq!(
        store
            .list_pending_release_journal_recordings(workspace, None, 100)
            .unwrap(),
        vec![released.clone()]
    );
    store
        .update_release_journal_recording(&released, false, Some(JournalRemovalCause::JobFailed))
        .unwrap();
    assert!(
        store
            .update_release_journal_recording(&released, true, None)
            .is_err()
    );
    let failed = store
        .get_release_journal_recording(workspace, &lease.worker)
        .unwrap()
        .unwrap();
    store
        .update_release_journal_recording(&failed, true, None)
        .unwrap();
    assert!(
        store
            .list_pending_release_journal_recordings(workspace, None, 100)
            .unwrap()
            .is_empty()
    );
    let completed = store
        .get_release_journal_recording(workspace, &lease.worker)
        .unwrap()
        .unwrap();
    assert!(
        store
            .update_release_journal_recording(&completed, false, None)
            .is_err()
    );
    store.with_conn(|conn| {
        assert!(conn.execute("UPDATE subject_worker_releases SET recording_json=json_remove(recording_json,'$.completed') WHERE operation_id='release'", []).is_err());
        assert!(conn.execute("UPDATE subject_worker_releases SET binding_id='different' WHERE operation_id='release'", []).is_err());
        assert!(conn.execute("UPDATE subject_worker_releases SET recording_json=json_set(recording_json,'$.members',json('[]')) WHERE operation_id='release'", []).is_err());
        Ok(())
    }).unwrap();
}

#[test]
fn release_recording_migration_preserves_old_receipt_and_rolls_back_failed_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    super::super::tests::prepare_retained_schema(&path, 134);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed(&conn).unwrap();
    conn.execute_batch("INSERT INTO subject_worker_releases(workspace_id,runtime_id,worker_id,singleton_key,singleton_generation,binding_id,operation_id,proof,completed) VALUES('release-workspace','runtime','worker','subjektiv:subject',1,'binding','old-complete','runtime_disconnected',1);
        CREATE TEMP TRIGGER fail_release_migration BEFORE INSERT ON main.__yoi_schema_migrations WHEN NEW.version=135 BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
    assert!(migrate(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 134);
    let columns: usize = conn.query_row("SELECT count(*) FROM pragma_table_info('subject_worker_releases') WHERE name='recording_json'", [], |r| r.get(0)).unwrap();
    assert_eq!(columns, 0);
    conn.execute_batch("DROP TRIGGER fail_release_migration")
        .unwrap();
    migrate(&conn).unwrap();
    let old = conn
        .query_row(
            "SELECT completed,proof,recording_json FROM subject_worker_releases",
            [],
            |r| {
                Ok((
                    r.get::<_, bool>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(old, (true, "runtime_disconnected".into(), None));
    let shape = |conn: &Connection| -> Result<Vec<(String, String)>> {
        Ok(conn.prepare("SELECT name,sql FROM sqlite_schema WHERE name IN ('subject_worker_releases','subject_release_recording_immutable') ORDER BY name")?
            .query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    let fresh = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    // ALTER TABLE preserves the baseline table text differently, so compare the
    // new trigger exactly and the column contract through SQLite introspection.
    let trigger = |conn: &Connection| -> Result<String> {
        Ok(conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='subject_release_recording_immutable'",
            [],
            |r| r.get(0),
        )?)
    };
    assert_eq!(fresh.with_conn(trigger).unwrap(), trigger(&conn).unwrap());
    assert_eq!(shape(&conn).unwrap().len(), 2);
    storage_formats::verify_history(&conn, 135).unwrap();
    drop(conn);
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert!(
        reopened
            .get_release_journal_recording("release-workspace", &lease().worker)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .prepare_release_journal_recording(
                "release-workspace",
                &lease(),
                "old-complete",
                &binding(),
                &members()
            )
            .is_err()
    );
    reopened
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get::<_, usize>(0)
                })?,
                0
            );
            Ok(())
        })
        .unwrap();
}
