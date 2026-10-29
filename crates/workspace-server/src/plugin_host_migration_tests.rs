// Retained migrations build the real predecessor; this is not a latest-schema
// fixture with its version number relabelled. Existing 107/108 history is frozen.
#[test]
fn plugin_receipt_tail_migrates_true_predecessor_reopens_and_matches_fresh_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 108);
    let conn = Connection::open(&path).unwrap();
    seed_migration_workspace(&conn);
    let history = workspace_schema_migration_history(&conn).unwrap();
    assert_eq!(history.last().unwrap().0, 108);
    assert!(!table_exists(&conn, "plugin_invocations").unwrap());
    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 108);
    assert_eq!(plan.migrations.iter().map(|step| step.version).collect::<Vec<_>>(), [109, 110]);
    assert_eq!(current_schema_version(&conn).unwrap(), 108);
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store.with_conn(|conn| {
        crate::plugin_host::verify_receipt_schema(conn)?;
        assert_eq!(current_schema_version(conn)?, 110);
        let upgraded = workspace_schema_migration_history(conn)?;
        assert_eq!(&upgraded[..history.len()], history.as_slice());
        assert_eq!(upgraded.iter().find(|(version, _)| *version == 109).unwrap().1, "Durable Plugin Host invocations and HTTPS effect receipts");
        assert_eq!(upgraded.last().unwrap().1, "Worker Plugin launch selection associations");
        assert_eq!(conn.query_row("SELECT count(*) FROM workspaces WHERE workspace_id='space'", [], |row| row.get::<_, i64>(0))?, 1);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| row.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
    drop(store);
    for _ in 0..2 {
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        reopened.with_conn(|conn| {
            crate::plugin_host::verify_receipt_schema(conn)?;
            assert_eq!(workspace_schema_migration_history(conn)?.iter().filter(|(v, _)| *v == 109).count(), 1);
            Ok(())
        }).unwrap();
    }
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    fresh.with_conn(crate::plugin_host::verify_receipt_schema).unwrap();
}

#[test]
fn plugin_receipt_tail_rolls_back_partial_ddl_and_retries_without_history_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 108);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    // A late-name collision fails after the first CREATEs. The owner transaction
    // must roll back all new objects and the migration receipt together.
    conn.execute_batch("CREATE TABLE plugin_effect_receipts(unrelated INTEGER)").unwrap();
    let history = workspace_schema_migration_history(&conn).unwrap();
    let before: String = conn.query_row("SELECT group_concat(sql,char(10)) FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |r| r.get(0)).unwrap();
    assert!(migrate_plugin_receipts_v108_to_v109(&conn).is_err());
    let after: String = conn.query_row("SELECT group_concat(sql,char(10)) FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
    assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
    assert!(!table_exists(&conn, "plugin_invocations").unwrap());
    conn.execute_batch("DROP TABLE plugin_effect_receipts").unwrap();
    migrate_plugin_receipts_v108_to_v109(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 109);
    assert!(migrate_plugin_receipts_v108_to_v109(&conn).is_err());
    crate::plugin_host::verify_receipt_schema(&conn).unwrap();
    conn.execute("UPDATE __yoi_schema_migrations SET name='wrong predecessor' WHERE version=108", []).unwrap();
    assert!(verify_schema_history(&conn, 109).is_err());
}
