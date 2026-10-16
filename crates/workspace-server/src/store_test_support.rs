//! Ordinary store/API fixtures copy a verified empty schema into independent real DBs.
//! Each case owns its connections and executors, with production SQLite settings.
//! Additional connections within fresh API setup reuse its verified schema. Migration,
//! fresh-creation, drift detection and persisted-case reopen tests use uncached constructors.

use super::*;

impl SqliteWorkspaceStore {
    pub(crate) fn open_api_fixture(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            // Restart tests must reopen their persisted database, not reset it.
            return Self::open_for_test(path);
        }
        Self::from_fixture_connection(Connection::open(path)?)
    }

    pub(crate) fn in_memory_fixture() -> Result<Self> {
        Self::from_fixture_connection(Connection::open_in_memory()?)
    }

    // Only the fresh API fixture may use this after copying its verified template.
    // Open a distinct connection without replacing the DB or sharing a transaction.
    pub(crate) fn connect_prepared_fixture(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_sqlite(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_executor: crate::db_executor::DbExecutor::new(
                crate::db_executor::DbExecutorLimits::default(),
            )?,
        })
    }

    fn from_fixture_connection(mut conn: Connection) -> Result<Self> {
        static TEMPLATE: std::sync::OnceLock<Mutex<Option<Connection>>> =
            std::sync::OnceLock::new();
        {
            // Only initialization/copy is serialized, never a test's execution.
            let mut template = TEMPLATE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|_| Error::Store("API fixture template lock poisoned".into()))?;
            if template.is_none() {
                let source = Connection::open_in_memory()?;
                prepare_connection(&source)?;
                *template = Some(source);
            }
            Backup::new(template.as_ref().unwrap(), &mut conn)?.run_to_completion(
                128,
                Duration::from_millis(1),
                None,
            )?;
        }
        // Backup copies the already verified schema, not connection-local PRAGMAs.
        // Existing DBs above still pass through full production startup checks.
        configure_sqlite(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_executor: crate::db_executor::DbExecutor::new(
                crate::db_executor::DbExecutorLimits::default(),
            )?,
        })
    }
}

#[test]
fn prepared_fixture_connections_observe_only_committed_writes_and_keep_foreign_keys() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("connections.db");
    assert!(SqliteWorkspaceStore::connect_prepared_fixture(&path).is_err());
    assert!(!path.exists());
    let writer = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
    let reader = SqliteWorkspaceStore::connect_prepared_fixture(&path).unwrap();
    writer.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1')", [])?;
        reader.with_conn(|other| {
            assert!(other.is_autocommit());
            assert_eq!(other.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get::<_, i64>(0))?, 0);
            Ok(())
        })?;
        tx.commit()?;
        Ok(())
    }).unwrap();
    reader.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get::<_, i64>(0))?, 1);
        assert_eq!(conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?, "wal");
        assert!(conn.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('invalid','missing','Invalid','active','1','1')", []).is_err());
        Ok(())
    }).unwrap();
}

#[test]
fn memory_fixture_copies_fresh_schema_and_isolates_data_and_ddl() {
    fn schema(store: &SqliteWorkspaceStore) -> Vec<(String, String, Option<String>)> {
        store
            .with_conn(|conn| {
                let mut statement =
                    conn.prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY type, name")?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap()
    }
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let first = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    assert_eq!(schema(&first), schema(&fresh));
    first.with_conn(|conn| {
        assert_eq!(conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?, 1);
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1'); DROP TRIGGER ticket_assignment_ticket_parent_tombstone;")?;
        Ok(())
    }).unwrap();
    let second = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    assert_eq!(schema(&second), schema(&fresh));
    second.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get::<_, i64>(0))?, 0);
        assert!(conn.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('invalid','missing','Invalid','active','1','1')", []).is_err());
        Ok(())
    }).unwrap();
}

#[test]
fn api_fixture_reopen_rejects_schema_drift_instead_of_replacing_the_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("drift.db");
    let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER ticket_assignment_ticket_parent_tombstone;")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let error = SqliteWorkspaceStore::open_api_fixture(&path)
        .err()
        .expect("reopen must verify existing schema");
    assert!(
        error
            .to_string()
            .contains("ticket_assignment_ticket_parent_tombstone")
    );
    let conn = Connection::open(path).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name='ticket_assignment_ticket_parent_tombstone'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn api_fixture_databases_isolate_writes_and_reopen_persisted_rows() {
    let temp = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        let handles: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|name| {
                let path = temp.path().join(format!("{name}.db"));
                scope.spawn(move || {
                    let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
                    store.with_conn(|conn| {
                        assert_eq!(conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?, 1);
                        assert_eq!(conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?, "wal");
                        conn.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner',?1,'1','1')", [name])?;
                        assert!(conn.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('invalid','missing','Invalid','active','1','1')", []).is_err());
                        Ok(())
                    }).unwrap();
                    drop(store);
                    let reopened = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
                    reopened.with_conn(|conn| {
                        assert_eq!(conn.query_row("SELECT display_name FROM accounts WHERE account_id='owner'", [], |row| row.get::<_, String>(0))?, name);
                        Ok(())
                    }).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    let pristine = SqliteWorkspaceStore::open_api_fixture(temp.path().join("pristine.db")).unwrap();
    pristine
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}
