// Included in store::tests. Intentionally preserves the v64/v78 physical column
// order, which differs from the latest baseline and must never be copied via '*'.
fn workspace_config_v78_fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(r#"
        PRAGMA foreign_keys=ON;
        CREATE TABLE __yoi_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL);
        INSERT INTO __yoi_schema_migrations VALUES(78,'workspace schema baseline');
        CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);
        INSERT INTO workspaces VALUES('ws');
        CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));
        INSERT INTO repositories VALUES('ws','repo');
        CREATE TABLE external_workdir_grants(workspace_id TEXT,grant_id TEXT,PRIMARY KEY(workspace_id,grant_id));
        INSERT INTO external_workdir_grants VALUES('ws','external-grant');
        CREATE TABLE workdir_registry(
            workspace_id TEXT NOT NULL,workdir_id TEXT NOT NULL,display_name TEXT,source_kind TEXT NOT NULL,
            runtime_id TEXT,repository_id TEXT,external_grant_id TEXT,creation_selector TEXT,creation_ref TEXT,
            creation_tree TEXT,current_selector TEXT,current_ref TEXT,current_tree TEXT,observed_at_epoch_seconds INTEGER,
            materialization_status TEXT NOT NULL,cleanliness TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,
            PRIMARY KEY(workspace_id,workdir_id));
        INSERT INTO workdir_registry VALUES('ws','repo-wd','Repository','repository','runtime','repo',NULL,'develop','a','tree-a','work','b','tree-b',123,'present','clean','created','updated');
        INSERT INTO workdir_registry VALUES('ws','external-wd','External','external_grant',NULL,NULL,'external-grant',NULL,NULL,NULL,NULL,NULL,NULL,NULL,'present','unknown','created-e','updated-e');
        CREATE INDEX registry_display_test ON workdir_registry(display_name);
        CREATE TABLE registry_updates(value TEXT);
        CREATE TRIGGER registry_update_test AFTER UPDATE ON workdir_registry BEGIN INSERT INTO registry_updates VALUES(NEW.display_name); END;
        CREATE TABLE dependent_links(workspace_id TEXT,workdir_id TEXT,connection_id TEXT,
            FOREIGN KEY(workspace_id,workdir_id) REFERENCES workdir_registry(workspace_id,workdir_id));
        INSERT INTO dependent_links VALUES('ws','repo-wd','durable-connection');
    "#).unwrap();
    conn
}

#[test]
fn workspace_config_migration_78_to_79_preserves_sources_refs_dependencies_and_constraints() {
    let conn = workspace_config_v78_fixture();
    migrate_workspace_config_v78_to_v79(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 79);
    assert!(
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
            .unwrap()
    );
    let row = conn
        .query_row(
            &workdir_registry_select_sql("WHERE workdir_id='repo-wd'"),
            [],
            read_workdir_registry_record,
        )
        .unwrap();
    assert_eq!(row.creation_tree.as_deref(), Some("tree-a"));
    assert_eq!(row.current_tree.as_deref(), Some("tree-b"));
    assert_eq!(row.current_selector.as_deref(), Some("work"));
    assert_eq!(row.cleanliness, "clean");
    assert_eq!(row.created_at, "created");
    let external = conn
        .query_row(
            &workdir_registry_select_sql("WHERE workdir_id='external-wd'"),
            [],
            read_workdir_registry_record,
        )
        .unwrap();
    assert!(
        matches!(external.source,WorkdirRegistrySource::ExternalGrant{ref grant_id} if grant_id=="external-grant")
    );
    assert_eq!(
        conn.query_row("SELECT connection_id FROM dependent_links", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "durable-connection"
    );
    assert!(
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='registry_display_test')",
            [],
            |row| row.get::<_, bool>(0)
        )
        .unwrap()
    );
    conn.execute(
        "UPDATE workdir_registry SET display_name='Updated' WHERE workdir_id='repo-wd'",
        [],
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM registry_updates", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "Updated"
    );
    conn.execute("INSERT INTO workspace_config_grants VALUES('ws','config-grant','runtime','worker','config-wd','read_only',0,'owner','now')",[]).unwrap();
    conn.execute("INSERT INTO workdir_registry(workspace_id,workdir_id,source_kind,workspace_config_grant_id,materialization_status,cleanliness,created_at,updated_at) VALUES('ws','config-wd','workspace_config','config-grant','present','clean','now','now')",[]).unwrap();
    let logical = conn
        .query_row(
            &workdir_registry_select_sql("WHERE workdir_id='config-wd'"),
            [],
            read_workdir_registry_record,
        )
        .unwrap();
    assert!(
        matches!(logical.source,WorkdirRegistrySource::WorkspaceConfig{ref grant_id} if grant_id=="config-grant")
    );
    assert!(conn.execute("INSERT INTO workspace_config_grants VALUES('ws','invalid-grant','r','w','invalid-wd','read_write_command',0,'owner','now')",[]).is_err());
    assert!(conn.execute("INSERT INTO workspace_config_grants VALUES('ws','duplicate','runtime','worker','other-wd','read_write',0,'owner','now')",[]).is_err());
    assert!(conn.execute("INSERT INTO workdir_registry(workspace_id,workdir_id,source_kind,workspace_config_grant_id,runtime_id,materialization_status,cleanliness,created_at,updated_at) VALUES('ws','invalid-wd','workspace_config','config-grant','os-runtime','present','clean','now','now')",[]).is_err());
    let mut check = conn.prepare("PRAGMA foreign_key_check").unwrap();
    assert!(check.query([]).unwrap().next().unwrap().is_none());
    assert!(
        migrate_workspace_config_v78_to_v79(&conn).is_err(),
        "wrong-version rerun must not silently mutate data"
    );
}

#[test]
fn workspace_config_migration_rolls_back_and_restores_foreign_keys_on_invalid_legacy_data() {
    let conn = workspace_config_v78_fixture();
    conn.execute(
        "UPDATE workdir_registry SET repository_id='missing' WHERE workdir_id='repo-wd'",
        [],
    )
    .unwrap();
    assert!(migrate_workspace_config_v78_to_v79(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 78);
    assert!(!column_exists(&conn, "workdir_registry", "workspace_config_grant_id").unwrap());
    assert!(!table_exists(&conn, "workspace_config_grants").unwrap());
    assert!(
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        conn.query_row(
            "SELECT repository_id FROM workdir_registry WHERE workdir_id='repo-wd'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "missing"
    );
    assert_eq!(
        conn.query_row("SELECT connection_id FROM dependent_links", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "durable-connection"
    );
}
