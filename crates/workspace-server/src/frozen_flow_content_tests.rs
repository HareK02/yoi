use super::*;

fn legacy_flow_source(instructions: &str) -> String {
    format!(
        r#"{{
            schema_version = 1;
            name = "flow";
            initial = "work";
            states = {{
                work = {{
                    instructions = "{instructions}";
                    transitions = {{ done = {{ target = "done"; condition = "Finished."; }}; }};
                }};
                done = {{ instructions = ""; terminal = true; }};
            }};
        }}"#
    )
}

fn legacy_flow_definition(content: &str) -> CompiledFlowDefinition {
    let mut definition = compile_flow_source(content).unwrap();
    // Model a saved definition from a previous compiler. Migration must preserve
    // its runtime-supplied meaning, not replace it with today's compiler output.
    for state in definition.states.values_mut() {
        for transition in &mut state.transitions {
            if transition.synthetic {
                transition.condition = "Frozen compiler cancellation condition.".into();
            }
        }
    }
    definition
}

// Frozen schema-84 fixture: old ordinals are migration input only.
fn legacy_flow_database(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys=ON;
        CREATE TABLE __yoi_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL);
        INSERT INTO __yoi_schema_migrations VALUES(84,'workspace schema baseline');
        CREATE TABLE flow_sources(workspace_id TEXT,flow_id TEXT,content TEXT,content_digest TEXT,
            revision INTEGER,PRIMARY KEY(workspace_id,flow_id));
        CREATE TABLE flow_source_revisions(workspace_id TEXT,flow_id TEXT,revision INTEGER,
            content TEXT,content_digest TEXT,definition_json TEXT,created_at TEXT,
            PRIMARY KEY(workspace_id,flow_id,revision));
    "#,
    )
    .unwrap();
    let alpha = legacy_flow_source("alpha");
    let beta = legacy_flow_source("beta");
    let alpha_definition = legacy_flow_definition(&alpha);
    let beta_definition = legacy_flow_definition(&beta);
    conn.execute(
        "INSERT INTO flow_sources VALUES('space','flow',?1,?2,3)",
        params![alpha, alpha_definition.content_digest],
    )
    .unwrap();
    for (legacy_counter, content, definition, created_at) in [
        (1, &alpha, &alpha_definition, "first"),
        (2, &beta, &beta_definition, "second"),
        (3, &alpha, &alpha_definition, "third"),
    ] {
        conn.execute(
            "INSERT INTO flow_source_revisions VALUES('space','flow',?1,?2,?3,?4,?5)",
            params![
                legacy_counter,
                content,
                definition.content_digest,
                serde_json::to_string_pretty(definition).unwrap(),
                created_at
            ],
        )
        .unwrap();
    }
    conn
}

#[test]
fn flow_content_migration_preserves_current_and_historical_content_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flows.db");
    let conn = legacy_flow_database(&path);
    let expected: Vec<(String, String, String, String)> = conn
        .prepare("SELECT content_digest,content,definition_json,created_at FROM flow_source_revisions WHERE revision IN (1,2) ORDER BY content_digest")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for (_, content, definition_json, _) in &expected {
        let saved: CompiledFlowDefinition = serde_json::from_str(definition_json).unwrap();
        assert_ne!(saved, compile_flow_source(content).unwrap());
    }
    migrate_flow_content_v84_to_v85(&conn).unwrap();
    drop(conn);
    let conn = Connection::open(path).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 85);
    let rows: Vec<(String, String, String, String)> = conn
        .prepare("SELECT content_digest,content,definition_json,created_at FROM flow_source_contents ORDER BY content_digest")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows, expected);
    let current: String = conn
        .query_row("SELECT c.content FROM flow_sources s JOIN flow_source_contents c USING(workspace_id,flow_id,content_digest)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(current, legacy_flow_source("alpha"));
    assert!(!table_exists(&conn, "flow_source_revisions").unwrap());
    let failures: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(failures, 0);
}

// Snapshot all fixture data and schema so rejection proves durable rollback,
// including the corrupted input (which migration must not silently repair).
fn legacy_flow_snapshot(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "SELECT name,sql FROM sqlite_schema ORDER BY name",
        "SELECT * FROM __yoi_schema_migrations ORDER BY version",
        "SELECT * FROM flow_sources ORDER BY workspace_id,flow_id",
        "SELECT * FROM flow_source_revisions ORDER BY workspace_id,flow_id,revision",
    ]
    .into_iter()
    .map(|sql| {
        let mut statement = conn.prepare(sql).unwrap();
        let count = statement.column_count();
        statement
            .query_map([], |row| (0..count).map(|column| row.get(column)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    })
    .collect()
}

fn assert_flow_migration_rolls_back(conn: Connection, path: &Path, expected_error: &str) {
    let before = legacy_flow_snapshot(&conn);
    let error = migrate_flow_content_v84_to_v85(&conn).unwrap_err();
    assert!(error.to_string().contains(expected_error), "{error}");
    assert_eq!(legacy_flow_snapshot(&conn), before);
    drop(conn);
    let reopened = Connection::open(path).unwrap();
    assert_eq!(current_schema_version(&reopened).unwrap(), 84);
    assert!(table_exists(&reopened, "flow_source_revisions").unwrap());
    assert!(!table_exists(&reopened, "flow_source_contents").unwrap());
    assert_eq!(legacy_flow_snapshot(&reopened), before);
}

#[test]
fn flow_content_migration_missing_current_content_rolls_back_data_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flows.db");
    let conn = legacy_flow_database(&path);
    conn.execute(
        "DELETE FROM flow_source_revisions WHERE revision IN (1,3)",
        [],
    )
    .unwrap();
    assert_flow_migration_rolls_back(conn, &path, "missing current content");
}

#[test]
fn flow_content_migration_single_historical_wrong_source_digest_rolls_back_data_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flows.db");
    let conn = legacy_flow_database(&path);
    let mut definition = legacy_flow_definition(&legacy_flow_source("beta"));
    let wrong_digest = format!("sha256:{}", "0".repeat(64));
    definition.content_digest = wrong_digest.clone();
    conn.execute(
        "UPDATE flow_source_revisions SET content_digest=?1,definition_json=?2 WHERE revision=2",
        params![wrong_digest, serde_json::to_string(&definition).unwrap()],
    )
    .unwrap();
    assert_flow_migration_rolls_back(conn, &path, "source digest mismatch");
}

#[test]
fn flow_content_migration_invalid_historical_definition_json_rolls_back_data_and_marker() {
    for invalid_definition in ["{", "{}"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flows.db");
        let conn = legacy_flow_database(&path);
        conn.execute(
            "UPDATE flow_source_revisions SET definition_json=?1 WHERE revision=2",
            [invalid_definition],
        )
        .unwrap();
        assert_flow_migration_rolls_back(conn, &path, "invalid stored definition");
    }
}

#[test]
fn flow_content_migration_historical_definition_digest_mismatch_rolls_back_data_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flows.db");
    let conn = legacy_flow_database(&path);
    let mut definition = legacy_flow_definition(&legacy_flow_source("beta"));
    definition.content_digest = format!("sha256:{}", "0".repeat(64));
    conn.execute(
        "UPDATE flow_source_revisions SET definition_json=?1 WHERE revision=2",
        [serde_json::to_string(&definition).unwrap()],
    )
    .unwrap();
    assert_flow_migration_rolls_back(conn, &path, "definition digest mismatch");
}

#[test]
fn flow_content_migration_inconsistent_saved_definitions_rolls_back_without_data_loss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flows.db");
    let conn = legacy_flow_database(&path);
    let mut definition = legacy_flow_definition(&legacy_flow_source("alpha"));
    definition
        .states
        .get_mut(&flow::StateId::new("work").unwrap())
        .unwrap()
        .instructions = "Different saved meaning.".into();
    conn.execute(
        "UPDATE flow_source_revisions SET definition_json=?1 WHERE revision=3",
        [serde_json::to_string_pretty(&definition).unwrap()],
    )
    .unwrap();
    assert_flow_migration_rolls_back(conn, &path, "inconsistent content identity");
}
