// Same store migration runner and retained-history fixture as the ordinary suite.
#[test]
fn plugin_launch_migration_preserves_reservations_and_rolls_back_failed_history_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    prepare_retained_schema(&path, 109);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    let before = stored_fixture_rows(&conn, "worker_create_reservations");
    conn.execute_batch("CREATE TRIGGER reject_launch_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=110 BEGIN SELECT RAISE(ABORT,'injected history failure'); END;").unwrap();
    assert!(migrate_plugin_launches_v109_to_v110(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 109);
    assert!(!table_exists(&conn, "worker_plugin_launches").unwrap());
    assert_eq!(
        stored_fixture_rows(&conn, "worker_create_reservations"),
        before
    );
    conn.execute_batch("DROP TRIGGER reject_launch_migration")
        .unwrap();
    migrate_plugin_launches_v109_to_v110(&conn).unwrap();
    crate::plugin_plan::verify_launch_schema(&conn).unwrap();
    assert_eq!(
        stored_fixture_rows(&conn, "worker_create_reservations"),
        before
    );
    assert!(conn.execute("INSERT INTO worker_plugin_launches(workspace_id,worker_id,runtime_id,plan_id) VALUES('missing','missing','runtime',NULL)", []).is_err());
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let ddl = |c: &Connection| {
        c.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='worker_plugin_launches'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    assert_eq!(ddl(&conn), fresh.with_conn(|conn| Ok(ddl(conn))).unwrap());
}
