// The real retained predecessor exercises both the catalog CHECK rebuild and
// new tables in one owner transaction; no schema-number relabeling is used.
#[test]
fn plugin_data_migration_preserves_references_and_matches_fresh_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 110);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    let digest = format!("sha256:{}", "a".repeat(64));
    conn.execute("INSERT INTO plugin_installations(workspace_id,installation_id,alias,package_id,last_operation) VALUES('space','installation','alias','package','install')", []).unwrap();
    conn.execute("INSERT INTO plugin_artifacts(workspace_id,installation_id,artifact_digest,package_digest,version,artifact_size,metadata_json) VALUES('space','installation',?1,?1,'1',1,'{}')", [&digest]).unwrap();
    conn.execute(
        "INSERT INTO plugin_references VALUES('space','config','config-owner','installation',?1)",
        [&digest],
    )
    .unwrap();
    let history = workspace_schema_migration_history(&conn).unwrap();
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT count(*) FROM plugin_references WHERE kind='config' AND owner='config-owner'", [], |r| r.get::<_, i64>(0))?, 1);
        conn.execute("INSERT INTO plugin_references VALUES('space','data','data-owner','installation',?1)", [&digest])?;
        assert!(conn.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='space'", []).is_err());
        assert!(conn.execute("INSERT INTO plugin_references VALUES('space','unknown','bad-owner','installation',?1)", [&digest]).is_err());
        assert_eq!(&workspace_schema_migration_history(conn)?[..history.len()], history.as_slice());
        let fresh = SqliteWorkspaceStore::in_memory_for_test()?;
        let objects = |connection: &Connection| -> Result<Vec<(String,String)>> {
            Ok(connection.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND (name LIKE 'plugin_data_%' OR name='plugin_references') ORDER BY name")?.query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
        };
        let expected = fresh.with_conn(objects)?;
        assert_eq!(objects(conn)?, expected);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_,i64>(0))?, 0);
        Ok(())
    }).unwrap();
    drop(store);
    SqliteWorkspaceStore::open_for_test(&path).unwrap();
}

#[test]
fn plugin_data_migration_late_failure_rolls_back_catalog_rebuild_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 110);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch("CREATE TABLE plugin_data_receipts(unrelated INTEGER)")
        .unwrap();
    let history = workspace_schema_migration_history(&conn).unwrap();
    let schema = |conn: &Connection| {
        conn.query_row("SELECT group_concat(sql,char(10)) FROM (SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name)", [], |r| r.get::<_,String>(0)).unwrap()
    };
    let before = schema(&conn);
    assert!(migrate_plugin_data_v110_to_v111(&conn).is_err());
    assert_eq!(schema(&conn), before);
    assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
    conn.execute_batch("DROP TABLE plugin_data_receipts")
        .unwrap();
    migrate_plugin_data_v110_to_v111(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 111);
    assert!(migrate_plugin_data_v110_to_v111(&conn).is_err());
}
