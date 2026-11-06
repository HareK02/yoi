use super::*;
use runtime_api::startup::{
    CompensationOutcome, CreationAcceptance, DecodeStoredWorkerCreateFailure, ExecutionOutcome,
    FinalizeFacts, FinalizeStage, StartupBoundaryCause, StartupBoundaryReason, StartupCause,
    StartupOperationFacts, WorkerCreateFailure, decode_stored_worker_create_failure,
};
use rusqlite::types::Value;

const WORKSPACE: &str = "failure-workspace";
const RUNTIME: &str = "failure-runtime";

fn encoded_failure(compensation: CompensationOutcome) -> String {
    let failure = WorkerCreateFailure::from_cause(
        StartupCause::Boundary(StartupBoundaryCause {
            reason: StartupBoundaryReason::FinalizeFailed,
        }),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Accepted,
            execution: ExecutionOutcome::Accepted,
            finalize: Some(FinalizeFacts {
                stage: FinalizeStage::CreateReservation,
                compensation,
            }),
            backend_compensation: Some(compensation),
            correlation: None,
        },
    )
    .unwrap();
    failure.encode_storage().unwrap()
}

fn reserve(store: &SqliteWorkspaceStore) -> WorkerCreateReservation {
    store.with_conn(|conn| {
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
            VALUES('failure-owner','user','failure-owner','Owner','1','1');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
            VALUES('failure-workspace','failure-owner','Failure workspace','active','1','1');
            INSERT INTO workspace_memory_settings(workspace_id,language,created_at,updated_at)
            VALUES('failure-workspace','en','1','1');")?;
        Ok(())
    }).unwrap();
    store
        .reserve_worker_create(
            WORKSPACE,
            RUNTIME,
            "manual:startup-failure",
            "sha256:startup-failure",
            Some("subjektiv:failed-create"),
            &store.get_workspace_memory_settings(WORKSPACE).unwrap(),
        )
        .unwrap()
}

// Snapshot every non-result reservation field plus resource keys and leases, so
// a save cannot silently acquire lifecycle effects as new columns are added.
fn non_result_snapshot(store: &SqliteWorkspaceStore) -> Vec<Vec<Value>> {
    store.with_conn(|conn| {
        let columns = table_columns(conn, "worker_create_reservations")?
            .into_iter()
            .filter(|name| name != "startup_failure_json")
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(",");
        let queries = [
            format!("SELECT {columns} FROM worker_create_reservations ORDER BY workspace_id,allocation_key"),
            "SELECT * FROM workspace_resource_keys ORDER BY workspace_id,resource_kind,resource_id".into(),
            "SELECT * FROM workspace_resource_key_counters ORDER BY workspace_id,resource_kind".into(),
            "SELECT * FROM worker_singleton_owners ORDER BY workspace_id,singleton_key".into(),
        ];
        let mut values = Vec::new();
        for query in queries {
            let mut statement = conn.prepare(&query)?;
            let count = statement.column_count();
            values.extend(statement.query_map([], |row| {
                (0..count).map(|i| row.get::<_, Value>(i)).collect::<rusqlite::Result<Vec<_>>>()
            })?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(values)
    }).unwrap()
}

fn save(
    store: &SqliteWorkspaceStore,
    reservation: &WorkerCreateReservation,
    previous: Option<&str>,
    encoded: &str,
) -> Result<bool> {
    store.save_worker_create_failure(
        WORKSPACE,
        RUNTIME,
        reservation.worker_id,
        &reservation.create_fingerprint,
        previous,
        encoded,
    )
}

#[test]
fn worker_create_failure_survives_restart_and_compensation_without_changing_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("server.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let reservation = reserve(&store);
    assert_eq!(
        store
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .unwrap(),
        None
    );
    let original = non_result_snapshot(&store);
    let encoded = encoded_failure(CompensationOutcome::Unknown);
    assert!(save(&store, &reservation, None, &encoded).unwrap());
    assert_eq!(non_result_snapshot(&store), original);
    drop(store);

    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        store
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .unwrap()
            .as_deref(),
        Some(encoded.as_str())
    );
    store
        .fail_worker_create_reservation(
            WORKSPACE,
            RUNTIME,
            reservation.worker_id,
            &reservation.create_fingerprint,
        )
        .unwrap();
    assert_eq!(
        store
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .unwrap()
            .as_deref(),
        Some(encoded.as_str())
    );
    let removed = non_result_snapshot(&store);
    let compensated = encoded_failure(CompensationOutcome::Completed);
    assert!(save(&store, &reservation, Some(&encoded), &compensated).unwrap());
    assert_eq!(non_result_snapshot(&store), removed);
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let raw = store
        .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(raw, compensated);
    let DecodeStoredWorkerCreateFailure::Known(failure) = decode_stored_worker_create_failure(&raw)
    else {
        panic!("saved result must decode")
    };
    assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
    assert_eq!(
        failure.operation().finalize.unwrap().compensation,
        CompensationOutcome::Completed
    );
}

#[test]
fn worker_create_failure_identity_and_exact_previous_bytes_fence_every_write() {
    let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    let reservation = reserve(&store);
    let first = encoded_failure(CompensationOutcome::Unknown);
    let next = encoded_failure(CompensationOutcome::Completed);
    assert!(save(&store, &reservation, None, &first).unwrap());
    let original = non_result_snapshot(&store);
    for (workspace, runtime, worker, fingerprint, previous) in [
        (
            "other",
            RUNTIME,
            reservation.worker_id,
            reservation.create_fingerprint.as_str(),
            Some(first.as_str()),
        ),
        (
            WORKSPACE,
            "other",
            reservation.worker_id,
            reservation.create_fingerprint.as_str(),
            Some(first.as_str()),
        ),
        (
            WORKSPACE,
            RUNTIME,
            WorkerId::now_v7(),
            reservation.create_fingerprint.as_str(),
            Some(first.as_str()),
        ),
        (
            WORKSPACE,
            RUNTIME,
            reservation.worker_id,
            "wrong",
            Some(first.as_str()),
        ),
        (
            WORKSPACE,
            RUNTIME,
            reservation.worker_id,
            reservation.create_fingerprint.as_str(),
            None,
        ),
        (
            WORKSPACE,
            RUNTIME,
            reservation.worker_id,
            reservation.create_fingerprint.as_str(),
            Some(next.as_str()),
        ),
    ] {
        assert!(
            !store
                .save_worker_create_failure(
                    workspace,
                    runtime,
                    worker,
                    fingerprint,
                    previous,
                    &next
                )
                .unwrap()
        );
        assert_eq!(
            store
                .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
                .unwrap()
                .as_deref(),
            Some(first.as_str())
        );
    }
    let equivalent_but_different_bytes = format!("{first} ");
    assert!(
        !save(
            &store,
            &reservation,
            Some(&equivalent_but_different_bytes),
            &next
        )
        .unwrap()
    );
    for (workspace, runtime, worker) in [
        ("other", RUNTIME, reservation.worker_id),
        (WORKSPACE, "other", reservation.worker_id),
        (WORKSPACE, RUNTIME, WorkerId::now_v7()),
    ] {
        assert_eq!(
            store
                .worker_create_failure(workspace, runtime, worker)
                .unwrap(),
            None
        );
    }
    assert_eq!(non_result_snapshot(&store), original);
    assert!(save(&store, &reservation, Some(&first), &next).unwrap());
}

#[test]
fn worker_create_failure_failed_write_rolls_back_result_and_all_other_fields() {
    let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    let reservation = reserve(&store);
    let first = encoded_failure(CompensationOutcome::Unknown);
    let next = encoded_failure(CompensationOutcome::Completed);
    assert!(save(&store, &reservation, None, &first).unwrap());
    let original = non_result_snapshot(&store);
    store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER interrupt_startup_failure AFTER UPDATE OF startup_failure_json ON worker_create_reservations
            BEGIN UPDATE worker_create_reservations SET state='removed';
            SELECT RAISE(ABORT,'injected startup failure write interruption'); END;")?;
        Ok(())
    }).unwrap();
    assert!(save(&store, &reservation, Some(&first), &next).is_err());
    assert_eq!(non_result_snapshot(&store), original);
    assert_eq!(
        store
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .unwrap()
            .as_deref(),
        Some(first.as_str())
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER interrupt_startup_failure")?;
            Ok(())
        })
        .unwrap();
    assert!(save(&store, &reservation, Some(&first), &next).unwrap());
}

#[test]
fn worker_create_failure_rejects_unknown_corrupt_and_oversized_writes_but_reads_raw_history() {
    let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    let reservation = reserve(&store);
    let first = encoded_failure(CompensationOutcome::Unknown);
    assert!(save(&store, &reservation, None, &first).unwrap());
    let mut unknown: serde_json::Value = serde_json::from_str(&first).unwrap();
    unknown["version"] = serde_json::json!(65535);
    let unknown = serde_json::to_string(&unknown).unwrap();
    assert!(matches!(
        decode_stored_worker_create_failure(&unknown),
        DecodeStoredWorkerCreateFailure::Unsupported
    ));
    for invalid in [
        unknown.as_str(),
        "not JSON",
        &" ".repeat(WORKER_CREATE_FAILURE_MAX_BYTES + 1),
    ] {
        assert!(save(&store, &reservation, Some(&first), invalid).is_err());
        assert_eq!(
            store
                .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
                .unwrap()
                .as_deref(),
            Some(first.as_str())
        );
    }
    // Simulate a retained newer writer/corrupt row, without interpreting prose.
    for raw in [unknown.as_str(), "not JSON"] {
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_create_reservations SET startup_failure_json=?1",
                    [raw],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
                .unwrap()
                .as_deref(),
            Some(raw)
        );
    }
    assert!(matches!(
        decode_stored_worker_create_failure("not JSON"),
        DecodeStoredWorkerCreateFailure::Corrupt(_)
    ));
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA ignore_check_constraints=ON;")?;
            conn.execute(
                "UPDATE worker_create_reservations SET startup_failure_json=?1",
                ["x".repeat(WORKER_CREATE_FAILURE_MAX_BYTES + 1)],
            )?;
            conn.execute_batch("PRAGMA ignore_check_constraints=OFF;")?;
            Ok(())
        })
        .unwrap();
    assert!(
        store
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .is_err()
    );
}

fn result_column_signature(conn: &Connection) -> Vec<(String, String, bool, Option<String>)> {
    conn.prepare("SELECT name,type,\"notnull\",dflt_value FROM pragma_table_info('worker_create_reservations') ORDER BY name").unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

fn assert_result_constraints(conn: &Connection, worker: WorkerId) {
    for accepted in [
        "x".repeat(WORKER_CREATE_FAILURE_MAX_BYTES),
        "é".repeat(WORKER_CREATE_FAILURE_MAX_BYTES / 2),
    ] {
        conn.execute(
            "UPDATE worker_create_reservations SET startup_failure_json=?1 WHERE worker_id=?2",
            params![accepted, worker.to_string()],
        )
        .unwrap();
    }
    for rejected in [
        "x".repeat(WORKER_CREATE_FAILURE_MAX_BYTES + 1),
        "é".repeat(WORKER_CREATE_FAILURE_MAX_BYTES / 2 + 1),
    ] {
        assert!(
            conn.execute(
                "UPDATE worker_create_reservations SET startup_failure_json=?1 WHERE worker_id=?2",
                params![rejected, worker.to_string()]
            )
            .is_err()
        );
    }
    assert!(
        conn.execute(
            "UPDATE worker_create_reservations SET startup_failure_json=x'00' WHERE worker_id=?1",
            [worker.to_string()]
        )
        .is_err()
    );
    conn.execute(
        "UPDATE worker_create_reservations SET startup_failure_json=NULL WHERE worker_id=?1",
        [worker.to_string()],
    )
    .unwrap();
    verify_worker_create_failure_schema(conn).unwrap();
}

#[test]
fn worker_create_failure_oldest_upgrade_preserves_legacy_null_and_matches_fresh_constraints() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oldest.db");
    tests::prepare_schema_v50(&path, Some("workspace-a"));
    let legacy_worker = WorkerId::now_v7();
    let conn = Connection::open(&path).unwrap();
    conn.execute("INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at)
        VALUES('workspace-a','legacy-allocation',?1,'shared','legacy-fingerprint','reserved','1','1')", [legacy_worker.to_string()]).unwrap();
    drop(conn);
    let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        upgraded
            .worker_create_failure("workspace-a", "shared", legacy_worker)
            .unwrap(),
        None
    );
    upgraded.with_conn(|conn| {
        let saved: (String, String, String) = conn.query_row("SELECT create_fingerprint,state,created_at FROM worker_create_reservations WHERE worker_id=?1", [legacy_worker.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        assert_eq!(saved, ("legacy-fingerprint".into(), "reserved".into(), "1".into()));
        assert_result_constraints(conn, legacy_worker);
        Ok(())
    }).unwrap();
    let reservation = reserve(&upgraded);
    let encoded = encoded_failure(CompensationOutcome::Residual);
    assert!(save(&upgraded, &reservation, None, &encoded).unwrap());
    drop(upgraded);
    let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        upgraded
            .worker_create_failure(WORKSPACE, RUNTIME, reservation.worker_id)
            .unwrap()
            .as_deref(),
        Some(encoded.as_str())
    );
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let reservation = reserve(&fresh);
    fresh
        .with_conn(|conn| {
            assert_result_constraints(conn, reservation.worker_id);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        upgraded
            .with_conn(|conn| Ok(result_column_signature(conn)))
            .unwrap(),
        fresh
            .with_conn(|conn| Ok(result_column_signature(conn)))
            .unwrap()
    );
}

#[test]
fn worker_create_failure_migration_failure_rolls_back_column_and_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("upgrade.db");
    tests::prepare_retained_schema(&path, 123);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_startup_failure_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=124
        BEGIN SELECT RAISE(ABORT,'injected startup failure migration interruption'); END;").unwrap();
    assert!(migrate_worker_create_failure_to_v124(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 123);
    assert!(!column_exists(&conn, "worker_create_reservations", "startup_failure_json").unwrap());
    conn.execute_batch("DROP TRIGGER interrupt_startup_failure_migration")
        .unwrap();
    migrate_worker_create_failure_to_v124(&conn).unwrap();
    verify_worker_create_failure_schema(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 124);
    drop(conn);
    SqliteWorkspaceStore::open_for_test(&path).unwrap();
}
