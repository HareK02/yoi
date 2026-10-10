// These fixtures use actual schema-88 column names as input, not as current APIs.
fn old_evidence_database(path: &Path) -> Connection {
    prepare_retained_schema(path, 88);
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    for (index, source) in [Some("immutable-source-commit"), None]
        .into_iter()
        .enumerate()
    {
        conn.execute("INSERT INTO artifacts(workspace_id,artifact_id,kind,uri,created_at,created_by_kind,created_by_key,created_by_display,source_revision)
            VALUES('space',?1,'file','artifact://saved','created','user','owner','Owner',?2)",params![format!("artifact-{index}"),source]).unwrap();
    }
    for (operation, result, expected) in [
        ("old-update", 7, Some(6)),
        ("old-create", 1, None),
        ("old-zero", 0, Some(0)),
    ] {
        conn.execute("INSERT INTO repository_secret_operations VALUES('space',?1,'saved-request-digest','credential','deleted-key',?1,'created')",[operation]).unwrap();
        conn.execute("INSERT INTO repository_secret_legacy_receipts VALUES('space',?1,'saved-request-digest','credential','deleted-key',?2,'created','credential_deleted',?3,'prior-operation','key-fingerprint')",params![operation,result,expected]).unwrap();
    }
    conn
}

fn evidence_rows(conn: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
        .unwrap();
    let count = stmt.column_count();
    stmt.query_map([], |row| {
        (0..count)
            .map(|i| row.get(i))
            .collect::<rusqlite::Result<Vec<_>>>()
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

#[test]
fn evidence_column_cutover_preserves_all_values_and_current_schema_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let conn = old_evidence_database(&path);
    let artifacts = evidence_rows(&conn, "artifacts");
    let receipts = evidence_rows(&conn, "repository_secret_legacy_receipts");
    drop(conn);
    let upgraded = SqliteWorkspaceStore::open(&path).unwrap();
    upgraded
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, 89);
            assert_eq!(evidence_rows(conn, "artifacts"), artifacts);
            assert_eq!(
                evidence_rows(conn, "repository_secret_legacy_receipts"),
                receipts
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                0
            );
            for (table, column) in [
                ("artifacts", "source_reference"),
                ("repository_secret_legacy_receipts", "legacy_result_counter"),
                (
                    "repository_secret_legacy_receipts",
                    "legacy_expected_counter",
                ),
            ] {
                assert!(column_exists(conn, table, column)?);
            }
            Ok(())
        })
        .unwrap();
    for store in [upgraded, SqliteWorkspaceStore::in_memory().unwrap()] {
        store.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'")?;
            let objects = stmt.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for (name,sql) in objects {
                assert!(!name.to_ascii_lowercase().contains("revision") && !sql.to_ascii_lowercase().contains("revision"),"old name in current schema: {name}");
            }
            Ok(())
        }).unwrap();
    }
}

#[test]
fn evidence_column_cutover_rolls_back_every_rename_before_retry() {
    let dir = tempfile::tempdir().unwrap();
    let conn = old_evidence_database(&dir.path().join("server.db"));
    conn.execute_batch("CREATE TRIGGER reject_schema_89 BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=89 BEGIN SELECT RAISE(ABORT,'reject schema 89'); END;").unwrap();
    let schema = evidence_rows(&conn, "sqlite_schema");
    let receipts = evidence_rows(&conn, "repository_secret_legacy_receipts");
    let artifacts = evidence_rows(&conn, "artifacts");
    let history = evidence_rows(&conn, "__yoi_schema_migrations");
    let pragmas = |conn: &Connection| {
        (
            conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                .unwrap(),
            conn.pragma_query_value(None, "legacy_alter_table", |row| row.get::<_, i64>(0))
                .unwrap(),
        )
    };
    let before = pragmas(&conn);
    assert!(
        migrate_evidence_column_names_v88_to_v89(&conn)
            .unwrap_err()
            .to_string()
            .contains("reject schema 89")
    );
    assert_eq!(current_schema_version(&conn).unwrap(), 88);
    assert_eq!(evidence_rows(&conn, "sqlite_schema"), schema);
    assert_eq!(
        evidence_rows(&conn, "repository_secret_legacy_receipts"),
        receipts
    );
    assert_eq!(evidence_rows(&conn, "artifacts"), artifacts);
    assert_eq!(evidence_rows(&conn, "__yoi_schema_migrations"), history);
    assert_eq!(pragmas(&conn), before);
    conn.execute_batch("DROP TRIGGER reject_schema_89").unwrap();
    migrate_evidence_column_names_v88_to_v89(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 89);
}
