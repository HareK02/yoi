use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "bootstrap workspace control plane",
    sql: r#"
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    local_root TEXT NOT NULL,
    record_authority TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS repositories (
    repository_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    local_root TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Projection tables are intentionally empty in this bootstrap: `.yoi/tickets`
-- and `.yoi/objectives` remain canonical, but the tables reserve a future
-- projection/cache seam without migrating authority.
CREATE TABLE IF NOT EXISTS ticket_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id)
);

CREATE TABLE IF NOT EXISTS objective_projections (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    objective_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id)
);

CREATE TABLE IF NOT EXISTS runners (
    runner_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    status TEXT NOT NULL,
    last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS runs (
    run_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(run_id) ON DELETE SET NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    created_at TEXT NOT NULL
);
"#,
}];

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub display_name: String,
    pub local_root: PathBuf,
    pub record_authority: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub workspace_id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerSummary {
    pub runner_id: String,
    pub workspace_id: String,
    pub label: String,
    pub status: String,
    pub last_seen_at: Option<String>,
}

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;
    async fn list_runs(&self, workspace_id: &str, limit: usize) -> Result<Vec<RunSummary>>;
    async fn list_runners(&self, workspace_id: &str, limit: usize) -> Result<Vec<RunnerSummary>>;
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
                    workspace_id, display_name, local_root, record_authority, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    local_root = excluded.local_root,
                    record_authority = excluded.record_authority,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.display_name,
                    record.local_root.to_string_lossy(),
                    record.record_authority,
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
                r#"SELECT workspace_id, display_name, local_root, record_authority, created_at, updated_at
                   FROM workspaces WHERE workspace_id = ?1"#,
                params![workspace_id],
                |row| {
                    Ok(WorkspaceRecord {
                        workspace_id: row.get(0)?,
                        display_name: row.get(1)?,
                        local_root: PathBuf::from(row.get::<_, String>(2)?),
                        record_authority: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
        })
    }

    async fn list_runs(&self, workspace_id: &str, limit: usize) -> Result<Vec<RunSummary>> {
        self.with_conn(|conn| {
            let limit = limit.min(200) as i64;
            let mut stmt = conn.prepare(
                r#"SELECT run_id, workspace_id, subject_kind, subject_id, status, created_at, updated_at
                   FROM runs WHERE workspace_id = ?1 ORDER BY updated_at DESC, run_id DESC LIMIT ?2"#,
            )?;
            let rows = stmt.query_map(params![workspace_id, limit], |row| {
                Ok(RunSummary {
                    run_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    subject_kind: row.get(2)?,
                    subject_id: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Error::from)
        })
    }

    async fn list_runners(&self, workspace_id: &str, limit: usize) -> Result<Vec<RunnerSummary>> {
        self.with_conn(|conn| {
            let limit = limit.min(200) as i64;
            let mut stmt = conn.prepare(
                r#"SELECT runner_id, workspace_id, label, status, last_seen_at
                   FROM runners WHERE workspace_id = ?1 ORDER BY runner_id ASC LIMIT ?2"#,
            )?;
            let rows = stmt.query_map(params![workspace_id, limit], |row| {
                Ok(RunnerSummary {
                    runner_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    label: row.get(2)?,
                    status: row.get(3)?,
                    last_seen_at: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
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
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![migration.version, migration.name],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_sqlite_and_preserves_workspace_record() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("control-plane.sqlite");
        let store = SqliteWorkspaceStore::open(&db).unwrap();

        assert_eq!(store.schema_version().await.unwrap(), 1);

        let record = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            display_name: "Yoi Dev".to_string(),
            local_root: dir.path().to_path_buf(),
            record_authority: "local_yoi_project_records".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&record).await.unwrap();

        let reopened = SqliteWorkspaceStore::open(&db).unwrap();
        assert_eq!(reopened.schema_version().await.unwrap(), 1);
        assert_eq!(
            reopened.get_workspace("local-dev").await.unwrap(),
            Some(record)
        );
        assert!(
            reopened
                .list_runs("local-dev", 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .list_runners("local-dev", 20)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
