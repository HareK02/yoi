//! Ordered production migration 106 -> 107. A marker failure rolls back new DDL
//! without changing the already applied catalog migration or its saved rows.
use super::*;

#[test]
fn plugin_plan_migration_marker_failure_preserves_catalog_then_retries_and_matches_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained.db");
    tests::prepare_retained_schema(&path, 106);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    let before = workspace_schema_migration_history(&conn).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_plan_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=107 BEGIN SELECT RAISE(ABORT,'plan marker failure'); END;").unwrap();
    assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
    assert_eq!(workspace_schema_migration_history(&conn).unwrap(), before);
    assert!(!table_exists(&conn, "plugin_execution_plans").unwrap());
    crate::plugin_catalog::verify(&conn).unwrap();
    conn.execute_batch("DROP TRIGGER interrupt_plan_migration")
        .unwrap();
    drop(conn);
    let updated = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    updated
        .with_conn(|conn| {
            assert!(workspace_schema_migration_history(conn)?.starts_with(&before));
            crate::plugin_plan::verify_plan_schema(conn)?;
            assert_eq!(
                conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            conn.execute_batch("DROP TRIGGER plugin_plan_snapshot_immutable")?;
            Ok(())
        })
        .unwrap();
    drop(updated);
    assert!(
        matches!(SqliteWorkspaceStore::open_for_test(path),Err(Error::Store(message)) if message.contains("Plugin plan storage schema has drifted"))
    );
}
