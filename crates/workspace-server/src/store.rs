use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "workspace db canonical schema v0 bootstrap",
        apply: create_schema_v0_tables,
    },
    Migration {
        version: 2,
        name: "align legacy workspace bootstrap with schema v0",
        apply: align_legacy_bootstrap_schema,
    },
];

struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;
}

#[derive(Clone)]
pub struct SqliteWorkspaceStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteWorkspaceStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn from_connection(conn: Connection) -> Result<Self> {
        configure_sqlite(&conn)?;
        apply_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Store("sqlite connection lock poisoned".to_string()))?;
        f(&conn)
    }
}

#[async_trait]
impl ControlPlaneStore for SqliteWorkspaceStore {
    async fn schema_version(&self) -> Result<i64> {
        self.with_conn(current_schema_version)
    }

    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workspaces (
                    workspace_id, display_name, state, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    state = excluded.state,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.display_name,
                    record.state,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                r#"SELECT workspace_id, display_name, state, created_at, updated_at
                   FROM workspaces WHERE workspace_id = ?1"#,
                params![workspace_id],
                |row| {
                    Ok(WorkspaceRecord {
                        workspace_id: row.get(0)?,
                        display_name: row.get(1)?,
                        state: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
        })
    }
}

fn configure_sqlite(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.execute_batch(
        r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS __yoi_schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
"#,
    )?;
    Ok(())
}

fn current_schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM __yoi_schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

fn apply_migrations(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current)
    {
        let tx = conn.unchecked_transaction()?;
        (migration.apply)(&tx)?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![migration.version, migration.name],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn align_legacy_bootstrap_schema(conn: &Connection) -> Result<()> {
    if table_exists(conn, "repositories")?
        && column_exists(conn, "repositories", "local_root")?
        && !column_exists(conn, "repositories", "uri")?
    {
        rename_legacy_table(conn, "repositories", "legacy_repositories")?;
    }
    if table_exists(conn, "runs")? {
        rename_legacy_table(conn, "runs", "legacy_runs")?;
    }
    if table_exists(conn, "artifacts")?
        && (column_exists(conn, "artifacts", "run_id")?
            || column_exists(conn, "artifacts", "path")?
            || !column_exists(conn, "artifacts", "uri")?)
    {
        rename_legacy_table(conn, "artifacts", "legacy_artifacts")?;
    }
    if table_exists(conn, "ticket_projections")? {
        rename_legacy_table(conn, "ticket_projections", "legacy_ticket_projections")?;
    }
    if table_exists(conn, "objective_projections")? {
        rename_legacy_table(
            conn,
            "objective_projections",
            "legacy_objective_projections",
        )?;
    }
    if table_exists(conn, "workspaces")? && !column_exists(conn, "workspaces", "state")? {
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN state TEXT NOT NULL DEFAULT 'active';",
        )?;
    }
    create_schema_v0_tables(conn)
}

fn rename_legacy_table(conn: &Connection, table_name: &str, legacy_name: &str) -> Result<()> {
    if table_exists(conn, legacy_name)? {
        return Err(Error::Store(format!(
            "cannot preserve legacy table `{table_name}` because `{legacy_name}` already exists"
        )));
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table_name} RENAME TO {legacy_name};"
    ))?;
    Ok(())
}

fn create_schema_v0_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tickets (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    priority TEXT,
    assignee_kind TEXT,
    assignee_key TEXT,
    assignee_display TEXT,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    resolution_event_id TEXT
);

CREATE TABLE IF NOT EXISTS ticket_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    event_id TEXT PRIMARY KEY,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    event_seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    activity_id TEXT,
    author_kind TEXT NOT NULL,
    author_key TEXT NOT NULL,
    author_display TEXT NOT NULL,
    author_source_kind TEXT,
    author_source_key TEXT,
    created_at TEXT NOT NULL,
    body_md TEXT,
    subject_kind TEXT,
    subject_id TEXT,
    previous_state TEXT,
    new_state TEXT,
    status TEXT,
    artifact_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    host_ref_kind TEXT,
    host_ref_key TEXT,
    host_display TEXT,
    repository_id TEXT,
    caused_by_event_id TEXT,
    UNIQUE (ticket_id, event_seq)
);

CREATE TABLE IF NOT EXISTS ticket_relations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    source_ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    target_ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    author_kind TEXT NOT NULL,
    author_key TEXT NOT NULL,
    author_display TEXT NOT NULL,
    author_source_kind TEXT,
    author_source_key TEXT,
    note TEXT,
    PRIMARY KEY (source_ticket_id, target_ticket_id, kind)
);

CREATE TABLE IF NOT EXISTS objectives (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objective_ticket_links (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL REFERENCES objectives(objective_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (objective_id, ticket_id, kind)
);

CREATE TABLE IF NOT EXISTS repositories (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    repository_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT,
    uri TEXT NOT NULL,
    default_ref TEXT,
    auth_ref_kind TEXT,
    auth_ref_key TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ticket_targets (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    target_id TEXT NOT NULL,
    repository_id TEXT NOT NULL REFERENCES repositories(repository_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    intent TEXT NOT NULL,
    ref_selector TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (ticket_id, target_id)
);

CREATE TABLE IF NOT EXISTS ticket_target_paths (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (ticket_id, target_id, path),
    FOREIGN KEY (ticket_id, target_id) REFERENCES ticket_targets(ticket_id, target_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ticket_worker_links (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
    worker_ref_kind TEXT NOT NULL,
    worker_ref_key TEXT NOT NULL,
    worker_display TEXT,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    activity_id TEXT,
    assigned_at TEXT,
    released_at TEXT,
    last_event_id TEXT,
    PRIMARY KEY (ticket_id, worker_ref_kind, worker_ref_key, role)
);

CREATE TABLE IF NOT EXISTS artifacts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    artifact_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    media_type TEXT,
    sha256 TEXT,
    size_bytes INTEGER,
    summary TEXT,
    created_at TEXT NOT NULL,
    created_by_kind TEXT NOT NULL,
    created_by_key TEXT NOT NULL,
    created_by_display TEXT NOT NULL,
    created_by_source_kind TEXT,
    created_by_source_key TEXT,
    ticket_id TEXT,
    objective_id TEXT,
    event_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    repository_id TEXT,
    source_kind TEXT,
    source_revision TEXT
);

CREATE TABLE IF NOT EXISTS audit_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    audit_event_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_key TEXT NOT NULL,
    actor_display TEXT NOT NULL,
    actor_source_kind TEXT,
    actor_source_key TEXT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    outcome TEXT NOT NULL,
    request_id TEXT,
    summary TEXT
);
"#,
    )?;
    Ok(())
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        params![table_name],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Error::from)
}

fn column_exists(conn: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    Ok(table_columns(conn, table_name)?
        .iter()
        .any(|column| column == column_name))
}

fn table_columns(conn: &Connection, table_name: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn migrates_sqlite_and_preserves_workspace_record() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("control-plane.sqlite");
        let store = SqliteWorkspaceStore::open(&db).unwrap();

        assert_eq!(store.schema_version().await.unwrap(), 2);

        let record = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            display_name: "Yoi Dev".to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&record).await.unwrap();

        let reopened = SqliteWorkspaceStore::open(&db).unwrap();
        assert_eq!(reopened.schema_version().await.unwrap(), 2);
        assert_eq!(
            reopened.get_workspace("local-dev").await.unwrap(),
            Some(record)
        );
    }

    #[test]
    fn fresh_schema_matches_workspace_db_v0_boundaries() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        apply_migrations(&conn).unwrap();

        let tables = table_names(&conn);
        for expected in [
            "workspaces",
            "tickets",
            "ticket_events",
            "ticket_relations",
            "objectives",
            "objective_ticket_links",
            "repositories",
            "ticket_targets",
            "ticket_target_paths",
            "ticket_worker_links",
            "artifacts",
            "audit_events",
        ] {
            assert!(
                tables.contains(expected),
                "missing expected v0 table {expected}"
            );
        }
        for forbidden in [
            "runs",
            "hosts",
            "workers",
            "actors",
            "validation_results",
            "ci_results",
        ] {
            assert!(
                !tables.contains(forbidden),
                "fresh v0 schema must not create forbidden table {forbidden}"
            );
        }
        assert!(
            !tables.iter().any(|table| table.starts_with("legacy_")),
            "fresh v0 schema should not create legacy compatibility tables: {tables:?}"
        );

        assert_columns(
            &conn,
            "workspaces",
            [
                "workspace_id",
                "display_name",
                "state",
                "created_at",
                "updated_at",
            ],
        );
        assert_columns(
            &conn,
            "repositories",
            [
                "workspace_id",
                "repository_id",
                "name",
                "kind",
                "provider",
                "uri",
                "default_ref",
                "auth_ref_kind",
                "auth_ref_key",
                "created_at",
                "updated_at",
            ],
        );
        assert_columns(
            &conn,
            "ticket_events",
            [
                "workspace_id",
                "event_id",
                "ticket_id",
                "event_seq",
                "kind",
                "activity_id",
                "author_kind",
                "author_key",
                "author_display",
                "author_source_kind",
                "author_source_key",
                "created_at",
                "body_md",
                "subject_kind",
                "subject_id",
                "previous_state",
                "new_state",
                "status",
                "artifact_id",
                "worker_ref_kind",
                "worker_ref_key",
                "worker_display",
                "host_ref_kind",
                "host_ref_key",
                "host_display",
                "repository_id",
                "caused_by_event_id",
            ],
        );
        assert_columns(
            &conn,
            "artifacts",
            [
                "workspace_id",
                "artifact_id",
                "kind",
                "uri",
                "media_type",
                "sha256",
                "size_bytes",
                "summary",
                "created_at",
                "created_by_kind",
                "created_by_key",
                "created_by_display",
                "created_by_source_kind",
                "created_by_source_key",
                "ticket_id",
                "objective_id",
                "event_id",
                "worker_ref_kind",
                "worker_ref_key",
                "worker_display",
                "repository_id",
                "source_kind",
                "source_revision",
            ],
        );

        for table in ["workspaces", "repositories", "ticket_events", "artifacts"] {
            let columns = table_columns(&conn, table).unwrap();
            for forbidden_column in [
                "payload",
                "payload_json",
                "metadata",
                "metadata_json",
                "diagnostics_json",
                "run_id",
                "local_root",
                "record_authority",
            ] {
                assert!(
                    !columns.iter().any(|column| column == forbidden_column),
                    "{table} must not contain obsolete/generic column {forbidden_column}"
                );
            }
        }
    }

    #[tokio::test]
    async fn upgrades_legacy_bootstrap_without_canonical_runs_table() {
        let conn = Connection::open_in_memory().unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch(LEGACY_BOOTSTRAP_SQL).unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (1, 'bootstrap workspace control plane')",
            [],
        )
        .unwrap();

        let store = SqliteWorkspaceStore::from_connection(conn).unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 2);

        store
            .with_conn(|conn| {
                let tables = table_names(conn);
                for expected in [
                    "workspaces",
                    "repositories",
                    "tickets",
                    "ticket_events",
                    "ticket_worker_links",
                    "artifacts",
                    "audit_events",
                    "legacy_repositories",
                    "legacy_runs",
                    "legacy_artifacts",
                    "legacy_ticket_projections",
                    "legacy_objective_projections",
                ] {
                    assert!(
                        tables.contains(expected),
                        "missing {expected} after upgrade"
                    );
                }
                for forbidden in ["runs", "hosts", "workers", "actors", "validation_results"] {
                    assert!(
                        !tables.contains(forbidden),
                        "upgraded schema must not retain forbidden canonical table {forbidden}"
                    );
                }
                let workspace_columns = table_columns(conn, "workspaces")?;
                assert!(workspace_columns.iter().any(|column| column == "state"));
                let artifact_columns = table_columns(conn, "artifacts")?;
                assert!(artifact_columns.iter().any(|column| column == "uri"));
                assert!(!artifact_columns.iter().any(|column| column == "run_id"));
                Ok(())
            })
            .unwrap();
    }

    fn table_names(conn: &Connection) -> BTreeSet<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.collect::<rusqlite::Result<BTreeSet<_>>>().unwrap()
    }

    fn assert_columns<const N: usize>(conn: &Connection, table: &str, expected: [&str; N]) {
        let columns = table_columns(conn, table).unwrap();
        let expected = expected.map(str::to_string).to_vec();
        assert_eq!(columns, expected, "unexpected columns for {table}");
    }

    const LEGACY_BOOTSTRAP_SQL: &str = r#"
CREATE TABLE workspaces (
    workspace_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    local_root TEXT NOT NULL,
    record_authority TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE repositories (
    repository_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    local_root TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE ticket_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id)
);
CREATE TABLE objective_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id)
);
CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(run_id) ON DELETE SET NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    created_at TEXT NOT NULL
);
"#;
}
