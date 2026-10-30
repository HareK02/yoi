#[test]
fn service_migration_preserves_history_has_no_backfill_and_matches_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 111);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    let before = workspace_schema_migration_history(&conn).unwrap();
    migrate_plugin_services_v111_to_v112(&conn).unwrap();
    assert_eq!(
        &workspace_schema_migration_history(&conn).unwrap()[..before.len()],
        before.as_slice()
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM typed_ticket_service_outbox",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let objects = |c: &Connection| -> Result<Vec<(String, String)>> {
        Ok(c.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND (name LIKE 'plugin_service_%' OR name LIKE 'typed_ticket_service_outbox%') ORDER BY name")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
    };
    assert_eq!(objects(&conn).unwrap(), fresh.with_conn(objects).unwrap());
    assert!(migrate_plugin_services_v111_to_v112(&conn).is_err());
}

#[test]
fn service_migration_late_failure_rolls_back_domain_and_consumer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 111);
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch("CREATE TABLE plugin_service_recovery_audit(unrelated INTEGER)")
        .unwrap();
    let schema = |c: &Connection| {
        c.query_row("SELECT group_concat(sql,char(10)) FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)",[],|r|r.get::<_,String>(0)).unwrap()
    };
    let before = schema(&conn);
    let history = workspace_schema_migration_history(&conn).unwrap();
    assert!(migrate_plugin_services_v111_to_v112(&conn).is_err());
    assert_eq!(schema(&conn), before);
    assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
    conn.execute_batch("DROP TABLE plugin_service_recovery_audit")
        .unwrap();
    migrate_plugin_services_v111_to_v112(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 112);
}
