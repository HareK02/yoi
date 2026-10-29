//! Exercise production migration/open against retained SQLite fixtures, including
//! the oldest retained transition, marker failure rollback and real FK/CHECKs.
use super::*;

fn catalog_schema(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND tbl_name LIKE 'plugin_%' ORDER BY name").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
}
#[test]
fn plugin_catalog_migration_matches_fresh_schema_from_oldest_retained_and_105() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = SqliteWorkspaceStore::open_for_test(dir.path().join("fresh.db")).unwrap();
    let canonical = fresh.with_conn(|c| Ok(catalog_schema(c))).unwrap();
    for version in [50, 105] {
        let path = dir.path().join(format!("v{version}.db"));
        if version == 50 {
            tests::prepare_schema_v50(&path, Some("workspace-a"));
        } else {
            tests::prepare_retained_schema(&path, version);
        }
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        let before = workspace_schema_migration_history(&conn).unwrap();
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('plugin-owner','user','plugin-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('plugin-workspace','plugin-owner','Plugins','active','1','1');").unwrap();
        drop(conn);
        let migrated = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        migrated.with_conn(|c| {
            assert_eq!(catalog_schema(c),canonical);
            let after=workspace_schema_migration_history(c)?;
            assert!(after.starts_with(&before));
            // Plugin catalog remains migration 106 even when startup applies later steps.
            assert_eq!(after.iter().filter(|(version, _)| *version == 106).cloned().collect::<Vec<_>>(),
                vec![(106, crate::plugin_catalog::MIGRATION_NAME.into())]);
            assert_eq!(after.last().unwrap(), &(109, "Durable Plugin Host invocations and HTTPS effect receipts".into()));
            assert_eq!(current_schema_version(c)?, LATEST_SCHEMA_VERSION);
            assert_eq!(c.query_row("SELECT display_name FROM workspaces WHERE workspace_id='plugin-workspace'",[],|r|r.get::<_,String>(0))?,"Plugins");
            assert_eq!(c.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0))?,0);
            assert!(c.execute("INSERT INTO plugin_installations VALUES('absent','i','alias','p','operation',0)",[]).is_err());
            c.execute("INSERT INTO plugin_installations VALUES('plugin-workspace','i','alias','p','operation',0)",[])?;
            assert!(c.execute("INSERT INTO plugin_installations VALUES('plugin-workspace','other','alias','p','operation',0)",[]).is_err());
            assert!(c.execute("INSERT INTO plugin_installations VALUES('plugin-workspace','other','other','p','operation',2)",[]).is_err());
            assert!(c.execute("INSERT INTO plugin_references VALUES('plugin-workspace','active','owner','i','unknown')",[]).is_err());
            Ok(())
        }).unwrap();
    }
}
#[test]
fn plugin_catalog_migration_marker_failure_rolls_back_then_retries_without_changing_applied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained.db");
    tests::prepare_retained_schema(&path, 105);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    let before = workspace_schema_migration_history(&conn).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_plugin_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=106 BEGIN SELECT RAISE(ABORT,'injected plugin migration interruption'); END;").unwrap();
    assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 105);
    assert_eq!(workspace_schema_migration_history(&conn).unwrap(), before);
    assert!(catalog_schema(&conn).is_empty());
    conn.execute_batch("DROP TRIGGER interrupt_plugin_migration")
        .unwrap();
    let migrated = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    migrated
        .with_conn(|c| {
            assert_eq!(current_schema_version(c)?, LATEST_SCHEMA_VERSION);
            let mut expected = before.clone();
            expected.push((106, crate::plugin_catalog::MIGRATION_NAME.into()));
            expected.push((
                107,
                "Immutable Worker Plugin plans and retained pins".into(),
            ));
            expected.push((
                108,
                "Subject Ticket responsibility and current-body notification routing".into(),
            ));
            expected.push((
                109,
                "Durable Plugin Host invocations and HTTPS effect receipts".into(),
            ));
            assert_eq!(workspace_schema_migration_history(c)?, expected);
            crate::plugin_catalog::verify(c)?;
            Ok(())
        })
        .unwrap();
}
#[test]
fn plugin_catalog_startup_rejects_drifted_reference_fence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store
        .with_conn(|c| {
            c.execute_batch("DROP TRIGGER plugin_reference_live")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    assert!(
        matches!(SqliteWorkspaceStore::open_for_test(path),Err(Error::Store(message)) if message.contains("Plugin catalog storage schema has drifted"))
    );
}
