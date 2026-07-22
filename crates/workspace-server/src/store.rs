use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const WORKSPACES_V0_COLUMNS: &[&str] = &[
    "workspace_id",
    "display_name",
    "state",
    "created_at",
    "updated_at",
];

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
    Migration {
        version: 3,
        name: "backend worker workdir registry schema",
        apply: create_worker_workdir_registry_tables,
    },
    Migration {
        version: 4,
        name: "remove durable worker lifecycle state",
        apply: remove_worker_registry_legacy_live_state_column,
    },
    Migration {
        version: 5,
        name: "use composite worker registry keys",
        apply: use_composite_worker_registry_keys,
    },
    Migration {
        version: 6,
        name: "add workdir runtime observation states",
        apply: add_workdir_runtime_observation_states,
    },
    Migration {
        version: 7,
        name: "remove workdir registry management kind",
        apply: remove_workdir_registry_management_kind_column,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRegistryRecord {
    pub workspace_id: String,
    pub runtime_id: String,
    pub runtime_worker_id: u64,
    pub display_name: String,
    pub profile: Option<String>,
    /// Retention state is explicit so `pinned` can be represented before prune exists.
    pub retention_state: String,
    pub transcript_ref: Option<String>,
    pub session_ref: Option<String>,
    pub summary_ref: Option<String>,
    pub diagnostics_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkdirRegistryRecord {
    pub workspace_id: String,
    pub workdir_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub selector: Option<String>,
    pub resolved_commit: Option<String>,
    pub materialization_status: String,
    pub cleanliness: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerWorkdirLinkRecord {
    pub workspace_id: String,
    pub runtime_id: String,
    pub runtime_worker_id: u64,
    pub workdir_id: String,
    pub role: String,
    pub linked_at: String,
    pub unlinked_at: Option<String>,
}

#[async_trait]
pub trait ControlPlaneStore: Send + Sync {
    async fn schema_version(&self) -> Result<i64>;
    async fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()>;
    async fn get_workspace(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>>;

    fn upsert_worker_registry(&self, record: &WorkerRegistryRecord) -> Result<()>;
    fn get_worker_registry(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Option<WorkerRegistryRecord>>;
    fn list_worker_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkerRegistryRecord>>;
    fn update_worker_retention(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool>;
    fn delete_worker_registry(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<bool>;

    fn upsert_workdir_registry(&self, record: &WorkdirRegistryRecord) -> Result<()>;
    fn get_workdir_registry(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Option<WorkdirRegistryRecord>>;
    fn list_workdir_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkdirRegistryRecord>>;
    fn delete_workdir_registry(&self, workspace_id: &str, workdir_id: &str) -> Result<bool>;

    fn upsert_worker_workdir_link(&self, record: &WorkerWorkdirLinkRecord) -> Result<()>;
    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>>;
    fn list_workdir_worker_links(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>>;
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

    fn upsert_worker_registry(&self, record: &WorkerRegistryRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO worker_registry (
                    workspace_id, runtime_id, runtime_worker_id, display_name, profile,
                    retention_state, transcript_ref, session_ref, summary_ref,
                    diagnostics_ref, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(workspace_id, runtime_id, runtime_worker_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    profile = excluded.profile,
                    retention_state = CASE
                        WHEN worker_registry.retention_state = 'pinned' AND excluded.retention_state = 'normal'
                        THEN worker_registry.retention_state
                        ELSE excluded.retention_state
                    END,
                    transcript_ref = excluded.transcript_ref,
                    session_ref = excluded.session_ref,
                    summary_ref = excluded.summary_ref,
                    diagnostics_ref = excluded.diagnostics_ref,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.runtime_id,
                    record.runtime_worker_id,
                    record.display_name,
                    record.profile,
                    record.retention_state,
                    record.transcript_ref,
                    record.session_ref,
                    record.summary_ref,
                    record.diagnostics_ref,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn get_worker_registry(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Option<WorkerRegistryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                worker_registry_select_sql(
                    "WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3",
                )
                .as_str(),
                params![workspace_id, runtime_id, runtime_worker_id],
                read_worker_registry_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_worker_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkerRegistryRecord>> {
        self.with_conn(|conn| {
            let sql = worker_registry_select_sql(
                "WHERE workspace_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
            );
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_worker_registry_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn update_worker_retention(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
        retention_state: &str,
        updated_at: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                r#"UPDATE worker_registry
                   SET retention_state = ?4, updated_at = ?5
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3"#,
                params![
                    workspace_id,
                    runtime_id,
                    runtime_worker_id,
                    retention_state,
                    updated_at
                ],
            )?;
            Ok(changed > 0)
        })
    }

    fn delete_worker_registry(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "DELETE FROM worker_registry WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3",
                params![workspace_id, runtime_id, runtime_worker_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn upsert_workdir_registry(&self, record: &WorkdirRegistryRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO workdir_registry (
                    workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
                    materialization_status, cleanliness, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(workspace_id, workdir_id) DO UPDATE SET
                    runtime_id = excluded.runtime_id,
                    repository_id = excluded.repository_id,
                    selector = excluded.selector,
                    resolved_commit = excluded.resolved_commit,
                    materialization_status = excluded.materialization_status,
                    cleanliness = excluded.cleanliness,
                    updated_at = excluded.updated_at"#,
                params![
                    record.workspace_id,
                    record.workdir_id,
                    record.runtime_id,
                    record.repository_id,
                    record.selector,
                    record.resolved_commit,
                    record.materialization_status,
                    record.cleanliness,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    fn get_workdir_registry(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Option<WorkdirRegistryRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                workdir_registry_select_sql("WHERE workspace_id = ?1 AND workdir_id = ?2").as_str(),
                params![workspace_id, workdir_id],
                read_workdir_registry_record,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    fn list_workdir_registry(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkdirRegistryRecord>> {
        self.with_conn(|conn| {
            let sql = workdir_registry_select_sql(
                "WHERE workspace_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
            );
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(
                params![workspace_id, limit as i64],
                read_workdir_registry_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn delete_workdir_registry(&self, workspace_id: &str, workdir_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "DELETE FROM workdir_registry WHERE workspace_id = ?1 AND workdir_id = ?2",
                params![workspace_id, workdir_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn upsert_worker_workdir_link(&self, record: &WorkerWorkdirLinkRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"INSERT INTO worker_workdir_links (
                    workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(workspace_id, runtime_id, runtime_worker_id, workdir_id, role) DO UPDATE SET
                    linked_at = excluded.linked_at,
                    unlinked_at = excluded.unlinked_at"#,
                params![
                    record.workspace_id,
                    record.runtime_id,
                    record.runtime_worker_id,
                    record.workdir_id,
                    record.role,
                    record.linked_at,
                    record.unlinked_at,
                ],
            )?;
            Ok(())
        })
    }

    fn list_worker_workdir_links(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        runtime_worker_id: u64,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
                   FROM worker_workdir_links
                   WHERE workspace_id = ?1 AND runtime_id = ?2 AND runtime_worker_id = ?3 AND unlinked_at IS NULL
                   ORDER BY linked_at DESC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, runtime_id, runtime_worker_id],
                read_worker_workdir_link_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    fn list_workdir_worker_links(
        &self,
        workspace_id: &str,
        workdir_id: &str,
    ) -> Result<Vec<WorkerWorkdirLinkRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"SELECT workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
                   FROM worker_workdir_links
                   WHERE workspace_id = ?1 AND workdir_id = ?2 AND unlinked_at IS NULL
                   ORDER BY linked_at DESC"#,
            )?;
            let rows = stmt.query_map(
                params![workspace_id, workdir_id],
                read_worker_workdir_link_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }
}

fn read_worker_workdir_link_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkerWorkdirLinkRecord> {
    Ok(WorkerWorkdirLinkRecord {
        workspace_id: row.get(0)?,
        runtime_id: row.get(1)?,
        runtime_worker_id: row.get(2)?,
        workdir_id: row.get(3)?,
        role: row.get(4)?,
        linked_at: row.get(5)?,
        unlinked_at: row.get(6)?,
    })
}

fn worker_registry_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT workspace_id, runtime_id, runtime_worker_id, display_name, profile, \
         retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref, \
         created_at, updated_at FROM worker_registry {where_clause}"
    )
}

fn read_worker_registry_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRegistryRecord> {
    Ok(WorkerRegistryRecord {
        workspace_id: row.get(0)?,
        runtime_id: row.get(1)?,
        runtime_worker_id: row.get(2)?,
        display_name: row.get(3)?,
        profile: row.get(4)?,
        retention_state: row.get(5)?,
        transcript_ref: row.get(6)?,
        session_ref: row.get(7)?,
        summary_ref: row.get(8)?,
        diagnostics_ref: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn workdir_registry_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit, \
         materialization_status, cleanliness, created_at, updated_at \
         FROM workdir_registry {where_clause}"
    )
}

fn read_workdir_registry_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkdirRegistryRecord> {
    Ok(WorkdirRegistryRecord {
        workspace_id: row.get(0)?,
        workdir_id: row.get(1)?,
        runtime_id: row.get(2)?,
        repository_id: row.get(3)?,
        selector: row.get(4)?,
        resolved_commit: row.get(5)?,
        materialization_status: row.get(6)?,
        cleanliness: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn create_worker_workdir_registry_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS worker_registry (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    runtime_worker_id INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    profile TEXT,
    retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
    transcript_ref TEXT,
    session_ref TEXT,
    summary_ref TEXT,
    diagnostics_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS workdir_registry (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    selector TEXT,
    resolved_commit TEXT,
    materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
    cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS worker_workdir_links (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    runtime_worker_id INTEGER NOT NULL,
    workdir_id TEXT NOT NULL,
    role TEXT NOT NULL,
    linked_at TEXT NOT NULL,
    unlinked_at TEXT,
    PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id, workdir_id, role),
    FOREIGN KEY (workspace_id, runtime_id, runtime_worker_id) REFERENCES worker_registry(workspace_id, runtime_id, runtime_worker_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, workdir_id) REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
    ON worker_registry(workspace_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
    ON workdir_registry(workspace_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_worker_workdir_links_worker
    ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id, linked_at DESC);
"#,
    )?;
    Ok(())
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

    let legacy_workspaces = preserve_noncanonical_workspaces(conn)?;
    create_schema_v0_tables(conn)?;
    if let Some(legacy_table) = legacy_workspaces {
        copy_legacy_workspaces(conn, &legacy_table)?;
    }
    Ok(())
}

fn preserve_noncanonical_workspaces(conn: &Connection) -> Result<Option<String>> {
    if !table_exists(conn, "workspaces")? {
        return Ok(None);
    }
    let columns = table_columns(conn, "workspaces")?;
    if columns
        .iter()
        .map(String::as_str)
        .eq(WORKSPACES_V0_COLUMNS.iter().copied())
    {
        return Ok(None);
    }
    let legacy_table = "legacy_workspaces";
    rename_legacy_table(conn, "workspaces", legacy_table)?;
    Ok(Some(legacy_table.to_string()))
}

fn copy_legacy_workspaces(conn: &Connection, legacy_table: &str) -> Result<()> {
    let columns = table_columns(conn, legacy_table)?;
    for required_column in ["workspace_id", "display_name", "created_at", "updated_at"] {
        if !columns.iter().any(|column| column == required_column) {
            return Err(Error::Store(format!(
                "cannot migrate legacy workspaces: `{legacy_table}` is missing `{required_column}`"
            )));
        }
    }
    let state_expr = if columns.iter().any(|column| column == "state") {
        "COALESCE(NULLIF(state, ''), 'active')"
    } else {
        "'active'"
    };
    conn.execute_batch(&format!(
        r#"INSERT OR IGNORE INTO workspaces (
            workspace_id, display_name, state, created_at, updated_at
        )
        SELECT workspace_id, display_name, {state_expr}, created_at, updated_at
        FROM {legacy_table};"#
    ))?;
    Ok(())
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

fn remove_worker_registry_legacy_live_state_column(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "worker_registry")?
        || !table_columns(conn, "worker_registry")?
            .iter()
            .any(|column| column == "lifecycle_state")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE worker_registry_v4 (
            workspace_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, worker_id),
            UNIQUE (workspace_id, runtime_id, runtime_worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT INTO worker_registry_v4 (
            workspace_id, worker_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        )
        SELECT
            workspace_id, worker_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        FROM worker_registry;
        DROP TABLE worker_registry;
        ALTER TABLE worker_registry_v4 RENAME TO worker_registry;
        CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
            ON worker_registry(workspace_id, updated_at DESC);
        "#,
    )?;
    Ok(())
}

fn use_composite_worker_registry_keys(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "worker_registry")?
        || !table_columns(conn, "worker_registry")?
            .iter()
            .any(|column| column == "worker_id")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE worker_registry_v5 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO worker_registry_v5 (
            workspace_id, runtime_id, runtime_worker_id, display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        )
        SELECT
            workspace_id,
            runtime_id,
            CASE
                WHEN typeof(runtime_worker_id) = 'integer' THEN runtime_worker_id
                WHEN runtime_worker_id GLOB 'worker-[0-9]*' THEN CAST(substr(runtime_worker_id, 8) AS INTEGER)
                WHEN runtime_worker_id GLOB '[0-9]*' THEN CAST(runtime_worker_id AS INTEGER)
                ELSE rowid
            END,
            display_name, profile,
            retention_state, transcript_ref, session_ref, summary_ref, diagnostics_ref,
            created_at, updated_at
        FROM worker_registry;

        CREATE TABLE worker_workdir_links_v5 (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            runtime_worker_id INTEGER NOT NULL,
            workdir_id TEXT NOT NULL,
            role TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            unlinked_at TEXT,
            PRIMARY KEY (workspace_id, runtime_id, runtime_worker_id, workdir_id, role),
            FOREIGN KEY (workspace_id, runtime_id, runtime_worker_id) REFERENCES worker_registry_v5(workspace_id, runtime_id, runtime_worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id) REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO worker_workdir_links_v5 (
            workspace_id, runtime_id, runtime_worker_id, workdir_id, role, linked_at, unlinked_at
        )
        SELECT
            links.workspace_id,
            registry.runtime_id,
            CASE
                WHEN typeof(registry.runtime_worker_id) = 'integer' THEN registry.runtime_worker_id
                WHEN registry.runtime_worker_id GLOB 'worker-[0-9]*' THEN CAST(substr(registry.runtime_worker_id, 8) AS INTEGER)
                WHEN registry.runtime_worker_id GLOB '[0-9]*' THEN CAST(registry.runtime_worker_id AS INTEGER)
                ELSE registry.rowid
            END,
            links.workdir_id,
            links.role,
            links.linked_at,
            links.unlinked_at
        FROM worker_workdir_links AS links
        JOIN worker_registry AS registry
          ON registry.workspace_id = links.workspace_id
         AND registry.worker_id = links.worker_id;

        DROP TABLE worker_workdir_links;
        DROP TABLE worker_registry;
        ALTER TABLE worker_registry_v5 RENAME TO worker_registry;
        ALTER TABLE worker_workdir_links_v5 RENAME TO worker_workdir_links;
        CREATE INDEX IF NOT EXISTS idx_worker_registry_workspace_updated
            ON worker_registry(workspace_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_worker_workdir_links_worker
            ON worker_workdir_links(workspace_id, runtime_id, runtime_worker_id, linked_at DESC);
        "#,
    )?;
    Ok(())
}

fn add_workdir_runtime_observation_states(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "workdir_registry")? {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE workdir_registry_v6 (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            selector TEXT,
            resolved_commit TEXT,
            materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
            cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO workdir_registry_v6 (
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        )
        SELECT
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            CASE materialization_status
                WHEN 'missing' THEN 'not_found'
                WHEN 'removed' THEN 'not_found'
                ELSE materialization_status
            END,
            cleanliness, created_at, updated_at
        FROM workdir_registry;
        DROP TABLE workdir_registry;
        ALTER TABLE workdir_registry_v6 RENAME TO workdir_registry;
        CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
            ON workdir_registry(workspace_id, updated_at DESC);
        "#,
    )?;
    Ok(())
}

fn remove_workdir_registry_management_kind_column(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "workdir_registry")?
        || !table_columns(conn, "workdir_registry")?
            .iter()
            .any(|column| column == "management_kind")
    {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TABLE workdir_registry_v7 (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            selector TEXT,
            resolved_commit TEXT,
            materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
            cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        INSERT OR REPLACE INTO workdir_registry_v7 (
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        )
        SELECT
            workspace_id, workdir_id, runtime_id, repository_id, selector, resolved_commit,
            materialization_status, cleanliness, created_at, updated_at
        FROM workdir_registry;
        DROP TABLE workdir_registry;
        ALTER TABLE workdir_registry_v7 RENAME TO workdir_registry;
        CREATE INDEX IF NOT EXISTS idx_workdir_registry_workspace_updated
            ON workdir_registry(workspace_id, updated_at DESC);
        "#,
    )?;
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

        assert_eq!(store.schema_version().await.unwrap(), 7);

        let record = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            display_name: "Yoi Dev".to_string(),
            state: "active".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&record).await.unwrap();

        let reopened = SqliteWorkspaceStore::open(&db).unwrap();
        assert_eq!(reopened.schema_version().await.unwrap(), 7);
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
            "worker_registry",
            "workdir_registry",
            "worker_workdir_links",
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
            "worker_registry",
            [
                "workspace_id",
                "runtime_id",
                "runtime_worker_id",
                "display_name",
                "profile",
                "retention_state",
                "transcript_ref",
                "session_ref",
                "summary_ref",
                "diagnostics_ref",
                "created_at",
                "updated_at",
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
            r#"INSERT INTO workspaces (
                workspace_id, display_name, local_root, record_authority, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                "legacy-workspace",
                "Legacy Workspace",
                "/tmp/legacy-workspace",
                "local_yoi_project_records",
                "2026-01-01T00:00:00Z",
                "2026-01-02T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (1, 'bootstrap workspace control plane')",
            [],
        )
        .unwrap();

        let store = SqliteWorkspaceStore::from_connection(conn).unwrap();
        assert_eq!(store.schema_version().await.unwrap(), 7);

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
                    "legacy_workspaces",
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
                assert_columns(
                    conn,
                    "workspaces",
                    [
                        "workspace_id",
                        "display_name",
                        "state",
                        "created_at",
                        "updated_at",
                    ],
                );
                let legacy_workspace_columns = table_columns(conn, "legacy_workspaces")?;
                assert!(
                    legacy_workspace_columns
                        .iter()
                        .any(|column| column == "local_root")
                );
                assert!(
                    legacy_workspace_columns
                        .iter()
                        .any(|column| column == "record_authority")
                );
                let artifact_columns = table_columns(conn, "artifacts")?;
                assert!(artifact_columns.iter().any(|column| column == "uri"));
                assert!(!artifact_columns.iter().any(|column| column == "run_id"));
                Ok(())
            })
            .unwrap();

        assert_eq!(
            store.get_workspace("legacy-workspace").await.unwrap(),
            Some(WorkspaceRecord {
                workspace_id: "legacy-workspace".to_string(),
                display_name: "Legacy Workspace".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
        );

        let new_record = WorkspaceRecord {
            workspace_id: "new-workspace".to_string(),
            display_name: "New Workspace".to_string(),
            state: "active".to_string(),
            created_at: "2026-02-01T00:00:00Z".to_string(),
            updated_at: "2026-02-01T00:00:00Z".to_string(),
        };
        store.upsert_workspace(&new_record).await.unwrap();
        assert_eq!(
            store.get_workspace("new-workspace").await.unwrap(),
            Some(new_record)
        );
    }

    #[tokio::test]
    async fn worker_workdir_registry_round_trips_and_preserves_pinned_retention() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db).unwrap();
        let workspace = WorkspaceRecord {
            workspace_id: "local-dev".to_string(),
            display_name: "Local Dev".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        store.upsert_workspace(&workspace).await.unwrap();

        let worker = WorkerRegistryRecord {
            workspace_id: "local-dev".to_string(),
            runtime_id: "embedded".to_string(),
            runtime_worker_id: 1,
            display_name: "Browser 1".to_string(),
            profile: Some("builtin:companion".to_string()),
            retention_state: "pinned".to_string(),
            transcript_ref: Some("runtime://embedded/workers/00000001/transcript".to_string()),
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "2".to_string(),
            updated_at: "2".to_string(),
        };
        store.upsert_worker_registry(&worker).unwrap();
        let mut runtime_sync_worker = worker.clone();
        runtime_sync_worker.retention_state = "normal".to_string();
        runtime_sync_worker.updated_at = "5".to_string();
        store.upsert_worker_registry(&runtime_sync_worker).unwrap();
        let mut expected_worker = worker.clone();
        expected_worker.updated_at = "5".to_string();

        let workdir = WorkdirRegistryRecord {
            workspace_id: "local-dev".to_string(),
            workdir_id: "0000019a00000000001".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            selector: Some("develop".to_string()),
            resolved_commit: Some("abcdef".to_string()),
            materialization_status: "not_found".to_string(),
            cleanliness: "clean".to_string(),
            created_at: "2".to_string(),
            updated_at: "3".to_string(),
        };
        store.upsert_workdir_registry(&workdir).unwrap();
        let unmanaged_workdir = WorkdirRegistryRecord {
            workspace_id: "local-dev".to_string(),
            workdir_id: "runtime-direct".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            selector: Some("feature".to_string()),
            resolved_commit: Some("123456".to_string()),
            materialization_status: "present".to_string(),
            cleanliness: "unknown".to_string(),
            created_at: "3".to_string(),
            updated_at: "4".to_string(),
        };
        store.upsert_workdir_registry(&unmanaged_workdir).unwrap();

        let link = WorkerWorkdirLinkRecord {
            workspace_id: "local-dev".to_string(),
            runtime_id: worker.runtime_id.clone(),
            runtime_worker_id: worker.runtime_worker_id.clone(),
            workdir_id: workdir.workdir_id.clone(),
            role: "primary_cwd".to_string(),
            linked_at: "4".to_string(),
            unlinked_at: None,
        };
        store.upsert_worker_workdir_link(&link).unwrap();

        assert_eq!(
            store
                .get_worker_registry("local-dev", "embedded", 1)
                .unwrap(),
            Some(expected_worker.clone())
        );
        assert_eq!(
            store
                .get_workdir_registry("local-dev", "0000019a00000000001")
                .unwrap(),
            Some(workdir.clone())
        );
        assert_eq!(
            store.list_workdir_registry("local-dev", 10).unwrap(),
            vec![unmanaged_workdir.clone(), workdir.clone()]
        );
        assert_eq!(
            store
                .list_worker_workdir_links("local-dev", "embedded", 1)
                .unwrap(),
            vec![link]
        );
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
