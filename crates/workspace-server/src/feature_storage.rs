//! Server-owned SQLite storage for trusted Workspace Features.
//!
//! The server owns database placement, connection configuration, migrations, and
//! operational lifecycle. A Feature owns its schema and only receives a scoped
//! [`FeatureDatabase`] after trusted registration; Worker/model inputs never
//! select a path or receive a SQL surface.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::backup::Backup;
use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior, params};
use uuid::Uuid;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_TABLE: &str = "__yoi_feature_schema_migrations";
const SNAPSHOT_FORMAT: &str = "yoi-feature-storage-v1";
const SNAPSHOT_MANIFEST: &str = "manifest.txt";
const MAX_IDENTIFIER_BYTES: usize = 128;

pub type Result<T> = std::result::Result<T, FeatureStorageError>;
pub type FeatureMigrationApply = for<'connection> fn(&Transaction<'connection>) -> Result<()>;

#[derive(Debug, thiserror::Error)]
pub enum FeatureStorageError {
    #[error("invalid {kind} storage identifier `{value}`")]
    InvalidIdentifier { kind: &'static str, value: String },
    #[error("Feature `{0}` is not registered with this Server storage manager")]
    FeatureNotRegistered(String),
    #[error("Feature `{0}` is already registered with this Server storage manager")]
    FeatureAlreadyRegistered(String),
    #[error("invalid migration plan for Feature `{feature}`: {detail}")]
    InvalidMigrationPlan { feature: String, detail: String },
    #[error("Feature registration belongs to a different Server storage manager")]
    ForeignRegistration,
    #[error("Feature storage is shutting down")]
    ShuttingDown,
    #[error("Feature storage for Workspace `{0}` is undergoing maintenance")]
    WorkspaceMaintenance(String),
    #[error("Feature database is closed")]
    DatabaseClosed,
    #[error(
        "Feature `{feature}` database schema version {actual} is newer than supported version {supported}"
    )]
    UnsupportedSchema {
        feature: String,
        actual: u32,
        supported: u32,
    },
    #[error("Feature `{feature}` migration history is incompatible: {detail}")]
    MigrationHistory { feature: String, detail: String },
    #[error("Feature `{feature}` migration {version} (`{name}`) failed: {detail}")]
    MigrationFailed {
        feature: String,
        version: u32,
        name: String,
        detail: String,
    },
    #[error("Feature storage snapshot is invalid: {0}")]
    InvalidSnapshot(String),
    #[error("Feature storage destination already exists: {0}")]
    DestinationExists(String),
    #[error("Feature storage operation failed: {0}")]
    Operation(String),
    #[error("feature storage I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("feature storage SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl FeatureStorageError {
    /// Lets a Feature reject a domain operation while retaining transaction rollback.
    pub fn operation(message: impl Into<String>) -> Self {
        Self::Operation(message.into())
    }
}

#[derive(Clone, Copy)]
pub struct FeatureMigration {
    version: u32,
    name: &'static str,
    apply: FeatureMigrationApply,
}

impl FeatureMigration {
    pub const fn new(version: u32, name: &'static str, apply: FeatureMigrationApply) -> Self {
        Self {
            version,
            name,
            apply,
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[derive(Clone, Copy)]
pub struct FeatureRegistration {
    feature_id: &'static str,
    migrations: &'static [FeatureMigration],
}

impl FeatureRegistration {
    pub const fn new(feature_id: &'static str, migrations: &'static [FeatureMigration]) -> Self {
        Self {
            feature_id,
            migrations,
        }
    }

    pub fn feature_id(&self) -> &'static str {
        self.feature_id
    }
}

#[derive(Clone, Debug)]
pub struct RegisteredFeature {
    manager_id: Uuid,
    feature_id: String,
}

impl RegisteredFeature {
    pub fn feature_id(&self) -> &str {
        &self.feature_id
    }
}

#[derive(Clone)]
pub struct FeatureStorage {
    inner: Arc<FeatureStorageInner>,
}

struct FeatureStorageInner {
    id: Uuid,
    root: PathBuf,
    state: Mutex<FeatureStorageState>,
}

struct FeatureStorageState {
    running: bool,
    registrations: HashMap<String, &'static [FeatureMigration]>,
    connections: HashMap<(String, String), Weak<FeatureDatabaseInner>>,
    maintenance: HashSet<String>,
    deletion_fences: HashSet<String>,
}

impl FeatureStorage {
    /// Creates a lazy manager. No directories or databases are created until a
    /// registered Feature is opened, restored, or explicitly backed up.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(FeatureStorageInner {
                id: Uuid::now_v7(),
                root: root.into(),
                state: Mutex::new(FeatureStorageState {
                    running: true,
                    registrations: HashMap::new(),
                    connections: HashMap::new(),
                    maintenance: HashSet::new(),
                    deletion_fences: HashSet::new(),
                }),
            }),
        }
    }

    pub fn for_server_database(database_path: impl AsRef<Path>) -> Result<Self> {
        let database_path = database_path.as_ref();
        let parent = database_path.parent().ok_or_else(|| {
            FeatureStorageError::Operation(format!(
                "Server database path `{}` has no parent",
                database_path.display()
            ))
        })?;
        Ok(Self::new(parent.join("feature-storage")))
    }

    pub fn workspace(&self, workspace_id: &str) -> Result<WorkspaceFeatureStorage> {
        Ok(WorkspaceFeatureStorage {
            manager: self.clone(),
            workspace_id: validate_identifier("Workspace", workspace_id)?.to_string(),
        })
    }

    pub fn register(&self, registration: FeatureRegistration) -> Result<RegisteredFeature> {
        let feature_id = validate_identifier("Feature", registration.feature_id)?.to_string();
        validate_migrations(&feature_id, registration.migrations)?;
        let mut state = self.lock_state()?;
        ensure_running(&state)?;
        if state.registrations.contains_key(&feature_id) {
            return Err(FeatureStorageError::FeatureAlreadyRegistered(feature_id));
        }
        state
            .registrations
            .insert(feature_id.clone(), registration.migrations);
        Ok(RegisteredFeature {
            manager_id: self.inner.id,
            feature_id,
        })
    }

    /// Closes every managed connection after any operation currently holding a
    /// connection lock completes. New opens and operations then fail.
    pub fn shutdown(&self) -> Result<()> {
        let connections = {
            let mut state = self.lock_state()?;
            if !state.running {
                return Ok(());
            }
            state.running = false;
            let connections = state
                .connections
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>();
            for connection in &connections {
                connection.fence();
            }
            state.connections.clear();
            connections
        };
        for connection in connections {
            connection.close()?;
        }
        Ok(())
    }

    fn open(
        &self,
        workspace_id: &str,
        registration: &RegisteredFeature,
    ) -> Result<FeatureDatabase> {
        if registration.manager_id != self.inner.id {
            return Err(FeatureStorageError::ForeignRegistration);
        }
        let key = (workspace_id.to_string(), registration.feature_id.clone());
        let mut state = self.lock_state()?;
        ensure_running(&state)?;
        if state.maintenance.contains(workspace_id) || state.deletion_fences.contains(workspace_id)
        {
            return Err(FeatureStorageError::WorkspaceMaintenance(
                workspace_id.to_string(),
            ));
        }
        let migrations = state
            .registrations
            .get(&registration.feature_id)
            .copied()
            .ok_or_else(|| {
                FeatureStorageError::FeatureNotRegistered(registration.feature_id.clone())
            })?;
        if let Some(connection) = state.connections.get(&key).and_then(Weak::upgrade) {
            return Ok(FeatureDatabase { inner: connection });
        }

        let feature_dir = self.feature_dir(workspace_id);
        fs::create_dir_all(&feature_dir)?;
        let path = feature_dir.join(format!("{}.sqlite", registration.feature_id));
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        configure_connection(&connection)?;
        migrate(&connection, &registration.feature_id, migrations)?;
        let connection = Arc::new(FeatureDatabaseInner {
            workspace_id: workspace_id.to_string(),
            feature_id: registration.feature_id.clone(),
            accepting_operations: AtomicBool::new(true),
            connection: Mutex::new(Some(connection)),
        });
        state.connections.insert(key, Arc::downgrade(&connection));
        Ok(FeatureDatabase { inner: connection })
    }

    fn backup_workspace(&self, workspace_id: &str, destination: &Path) -> Result<()> {
        // Keep manager lifecycle and deletion/restore mutually exclusive for the
        // duration. Feature transactions use SQLite's online backup snapshot rules.
        let state = self.lock_state()?;
        ensure_running(&state)?;
        if state.maintenance.contains(workspace_id) || state.deletion_fences.contains(workspace_id)
        {
            return Err(FeatureStorageError::WorkspaceMaintenance(
                workspace_id.to_string(),
            ));
        }
        if destination.exists() {
            return Err(FeatureStorageError::DestinationExists(
                destination.display().to_string(),
            ));
        }
        let feature_ids = list_feature_databases(&self.feature_dir(workspace_id))?;
        let parent = destination.parent().ok_or_else(|| {
            FeatureStorageError::Operation(format!(
                "snapshot destination `{}` has no parent",
                destination.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        let temporary = temporary_sibling(destination, "backup")?;
        let result = (|| {
            let temporary_features = temporary.join("features");
            fs::create_dir_all(&temporary_features)?;
            for feature_id in &feature_ids {
                backup_database(
                    &self
                        .feature_dir(workspace_id)
                        .join(format!("{feature_id}.sqlite")),
                    &temporary_features.join(format!("{feature_id}.sqlite")),
                )?;
            }
            fs::write(
                temporary.join(SNAPSHOT_MANIFEST),
                snapshot_manifest(workspace_id, &feature_ids),
            )?;
            fs::rename(&temporary, destination)?;
            Ok(())
        })();
        drop(state);
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        result
    }

    fn restore_workspace(&self, workspace_id: &str, snapshot: &Path) -> Result<()> {
        let feature_ids = read_snapshot_manifest(snapshot, workspace_id)?;
        self.begin_maintenance(workspace_id, true)?;
        let workspace_dir = self.workspace_dir(workspace_id);
        let temporary = self
            .inner
            .root
            .join(format!(".restore-{workspace_id}-{}", Uuid::now_v7()));
        let result = (|| {
            if workspace_dir.exists() {
                return Err(FeatureStorageError::DestinationExists(
                    workspace_dir.display().to_string(),
                ));
            }
            let temporary_features = temporary.join("features");
            fs::create_dir_all(&temporary_features)?;
            for feature_id in &feature_ids {
                backup_database(
                    &snapshot
                        .join("features")
                        .join(format!("{feature_id}.sqlite")),
                    &temporary_features.join(format!("{feature_id}.sqlite")),
                )?;
            }
            fs::create_dir_all(&self.inner.root)?;
            fs::rename(&temporary, &workspace_dir)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        self.end_maintenance(workspace_id)?;
        result
    }

    fn delete_workspace(&self, workspace_id: &str) -> Result<()> {
        let connections = {
            let mut state = self.lock_state()?;
            ensure_running(&state)?;
            if state.maintenance.contains(workspace_id) {
                return Err(FeatureStorageError::WorkspaceMaintenance(
                    workspace_id.to_string(),
                ));
            }
            // The fence intentionally survives physical removal. Workspace deletion
            // is finalized in the Server DB immediately afterwards; keeping this
            // admission fence prevents a cached API/background task from recreating
            // Feature files in that gap. A retry is idempotently admitted.
            state.deletion_fences.insert(workspace_id.to_string());
            let connections = state
                .connections
                .iter()
                .filter(|((candidate, _), _)| candidate == workspace_id)
                .filter_map(|(_, connection)| connection.upgrade())
                .collect::<Vec<_>>();
            for connection in &connections {
                connection.fence();
            }
            state
                .connections
                .retain(|(candidate, _), _| candidate != workspace_id);
            connections
        };
        for connection in connections {
            connection.close()?;
        }
        match fs::remove_dir_all(self.workspace_dir(workspace_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn begin_maintenance(
        &self,
        workspace_id: &str,
        require_no_connections: bool,
    ) -> Result<Vec<Arc<FeatureDatabaseInner>>> {
        let mut state = self.lock_state()?;
        ensure_running(&state)?;
        if state.deletion_fences.contains(workspace_id)
            || !state.maintenance.insert(workspace_id.to_string())
        {
            return Err(FeatureStorageError::WorkspaceMaintenance(
                workspace_id.to_string(),
            ));
        }
        let connections = state
            .connections
            .iter()
            .filter(|((candidate, _), _)| candidate == workspace_id)
            .filter_map(|(_, connection)| connection.upgrade())
            .collect::<Vec<_>>();
        if require_no_connections && !connections.is_empty() {
            state.maintenance.remove(workspace_id);
            return Err(FeatureStorageError::Operation(format!(
                "cannot restore Workspace `{workspace_id}` while Feature databases are open"
            )));
        }
        state
            .connections
            .retain(|(candidate, _), _| candidate != workspace_id);
        Ok(connections)
    }

    fn end_maintenance(&self, workspace_id: &str) -> Result<()> {
        let mut state = self.lock_state()?;
        state.maintenance.remove(workspace_id);
        Ok(())
    }

    fn workspace_dir(&self, workspace_id: &str) -> PathBuf {
        self.inner.root.join(workspace_id)
    }

    fn feature_dir(&self, workspace_id: &str) -> PathBuf {
        self.workspace_dir(workspace_id).join("features")
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, FeatureStorageState>> {
        self.inner.state.lock().map_err(|_| {
            FeatureStorageError::Operation("Feature storage state lock was poisoned".to_string())
        })
    }
}

impl Drop for FeatureStorageInner {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.running = false;
            for connection in state.connections.values().filter_map(Weak::upgrade) {
                let _ = connection.close();
            }
            state.connections.clear();
        }
    }
}

#[derive(Clone)]
pub struct WorkspaceFeatureStorage {
    manager: FeatureStorage,
    workspace_id: String,
}

impl WorkspaceFeatureStorage {
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn register(&self, registration: FeatureRegistration) -> Result<RegisteredFeature> {
        self.manager.register(registration)
    }

    pub fn open(&self, registration: &RegisteredFeature) -> Result<FeatureDatabase> {
        self.manager.open(&self.workspace_id, registration)
    }

    /// Creates a consistent online SQLite snapshot of every Feature database,
    /// including data for Features that are currently disabled/unregistered.
    pub fn backup(&self, destination: impl AsRef<Path>) -> Result<()> {
        self.manager
            .backup_workspace(&self.workspace_id, destination.as_ref())
    }

    /// Restores a snapshot only into an absent Workspace storage directory and
    /// only while no Feature database for this Workspace is open.
    pub fn restore(&self, snapshot: impl AsRef<Path>) -> Result<()> {
        self.manager
            .restore_workspace(&self.workspace_id, snapshot.as_ref())
    }

    /// Closes this Workspace's Feature connections and removes only its scoped
    /// storage tree. This is the operation used by Workspace deletion.
    pub fn delete(&self) -> Result<()> {
        self.manager.delete_workspace(&self.workspace_id)
    }

    pub fn shutdown(&self) -> Result<()> {
        self.manager.shutdown()
    }
}

#[derive(Clone)]
pub struct FeatureDatabase {
    inner: Arc<FeatureDatabaseInner>,
}

struct FeatureDatabaseInner {
    workspace_id: String,
    feature_id: String,
    accepting_operations: AtomicBool,
    connection: Mutex<Option<Connection>>,
}

impl FeatureDatabase {
    pub fn workspace_id(&self) -> &str {
        &self.inner.workspace_id
    }

    pub fn feature_id(&self) -> &str {
        &self.inner.feature_id
    }

    /// Executes a Feature-owned read/query while serializing use of the managed
    /// connection. The connection never leaves trusted Server Rust code.
    pub fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        self.inner.ensure_accepting()?;
        let connection = self.inner.connection.lock().map_err(|_| {
            FeatureStorageError::Operation("Feature database lock was poisoned".to_string())
        })?;
        self.inner.ensure_accepting()?;
        let connection = connection
            .as_ref()
            .ok_or(FeatureStorageError::DatabaseClosed)?;
        operation(connection)
    }

    /// Executes a Feature-owned multi-table operation atomically. Returning an
    /// error rolls the transaction back; successful return commits it.
    pub fn transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.inner.ensure_accepting()?;
        let mut connection = self.inner.connection.lock().map_err(|_| {
            FeatureStorageError::Operation("Feature database lock was poisoned".to_string())
        })?;
        self.inner.ensure_accepting()?;
        let connection = connection
            .as_mut()
            .ok_or(FeatureStorageError::DatabaseClosed)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = operation(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!("SELECT COALESCE(MAX(version), 0) FROM {MIGRATION_TABLE}"),
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }
}

impl FeatureDatabaseInner {
    fn ensure_accepting(&self) -> Result<()> {
        if self.accepting_operations.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(FeatureStorageError::DatabaseClosed)
        }
    }

    fn fence(&self) {
        self.accepting_operations.store(false, Ordering::Release);
    }

    fn close(&self) -> Result<()> {
        self.fence();
        let mut connection = self.connection.lock().map_err(|_| {
            FeatureStorageError::Operation("Feature database lock was poisoned".to_string())
        })?;
        connection.take();
        Ok(())
    }
}

fn validate_identifier<'a>(kind: &'static str, value: &'a str) -> Result<&'a str> {
    let valid = !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => true,
            b'-' | b'_' => index > 0,
            _ => false,
        });
    if !valid {
        return Err(FeatureStorageError::InvalidIdentifier {
            kind,
            value: value.to_string(),
        });
    }
    Ok(value)
}

fn validate_migrations(feature: &str, migrations: &[FeatureMigration]) -> Result<()> {
    for (index, migration) in migrations.iter().enumerate() {
        let expected = (index + 1) as u32;
        if migration.version != expected {
            return Err(FeatureStorageError::InvalidMigrationPlan {
                feature: feature.to_string(),
                detail: format!(
                    "versions must be contiguous from 1; expected {expected}, found {}",
                    migration.version
                ),
            });
        }
        if migration.name.trim().is_empty() {
            return Err(FeatureStorageError::InvalidMigrationPlan {
                feature: feature.to_string(),
                detail: format!("migration {expected} has an empty name"),
            });
        }
    }
    Ok(())
}

fn ensure_running(state: &FeatureStorageState) -> Result<()> {
    if state.running {
        Ok(())
    } else {
        Err(FeatureStorageError::ShuttingDown)
    }
}

fn configure_connection(connection: &Connection) -> Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch(
        r#"
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
"#,
    )?;
    // Changing journal mode takes a database-wide lock and, unlike ordinary
    // statements, may report SQLITE_BUSY immediately while another opener is
    // making the same transition. Bound retries by the same common timeout.
    let attempts = (BUSY_TIMEOUT.as_millis() / 10) as usize;
    for attempt in 0..=attempts {
        match connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => {
                return Err(FeatureStorageError::Operation(format!(
                    "SQLite refused WAL journal mode and selected `{mode}`"
                )));
            }
            Err(error) if sqlite_is_busy(&error) && attempt < attempts => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("bounded WAL retry loop always returns")
}

fn sqlite_is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(error, _)
            if matches!(error.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn migrate(
    connection: &Connection,
    feature_id: &str,
    migrations: &[FeatureMigration],
) -> Result<()> {
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    transaction.execute_batch(&format!(
        r#"
CREATE TABLE IF NOT EXISTS {MIGRATION_TABLE} (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
"#
    ))?;
    let mut statement = transaction.prepare(&format!(
        "SELECT version, name FROM {MIGRATION_TABLE} ORDER BY version"
    ))?;
    let applied = statement
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);

    let supported = migrations.last().map_or(0, |migration| migration.version);
    if let Some((actual, _)) = applied.last()
        && *actual > supported
    {
        return Err(FeatureStorageError::UnsupportedSchema {
            feature: feature_id.to_string(),
            actual: *actual,
            supported,
        });
    }
    for (index, (version, name)) in applied.iter().enumerate() {
        let Some(expected) = migrations.get(index) else {
            return Err(FeatureStorageError::UnsupportedSchema {
                feature: feature_id.to_string(),
                actual: *version,
                supported,
            });
        };
        if *version != expected.version || name != expected.name {
            return Err(FeatureStorageError::MigrationHistory {
                feature: feature_id.to_string(),
                detail: format!(
                    "stored migration {version} (`{name}`) does not match expected {} (`{}`)",
                    expected.version, expected.name
                ),
            });
        }
    }

    for migration in migrations.iter().skip(applied.len()) {
        if let Err(error) = (migration.apply)(&transaction) {
            return Err(FeatureStorageError::MigrationFailed {
                feature: feature_id.to_string(),
                version: migration.version,
                name: migration.name.to_string(),
                detail: error.to_string(),
            });
        }
        transaction.execute(
            &format!("INSERT INTO {MIGRATION_TABLE} (version, name) VALUES (?1, ?2)"),
            params![migration.version, migration.name],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn backup_database(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() {
        return Err(FeatureStorageError::InvalidSnapshot(format!(
            "Feature database `{}` is missing",
            source.display()
        )));
    }
    let source = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(BUSY_TIMEOUT)?;
    verify_database(&source)?;
    let mut destination = Connection::open(destination)?;
    Backup::new(&source, &mut destination)?.run_to_completion(
        128,
        Duration::from_millis(2),
        None,
    )?;
    verify_database(&destination)?;
    Ok(())
}

fn verify_database(connection: &Connection) -> Result<()> {
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(FeatureStorageError::InvalidSnapshot(format!(
            "SQLite integrity_check failed: {integrity}"
        )));
    }
    let mut foreign_keys = connection.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        return Err(FeatureStorageError::InvalidSnapshot(
            "SQLite foreign_key_check reported a violation".to_string(),
        ));
    }
    Ok(())
}

fn list_feature_databases(feature_dir: &Path) -> Result<Vec<String>> {
    if !feature_dir.exists() {
        return Ok(Vec::new());
    }
    let mut feature_ids = Vec::new();
    for entry in fs::read_dir(feature_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sqlite") {
            continue;
        }
        let feature_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                FeatureStorageError::InvalidSnapshot(format!(
                    "Feature database filename `{}` is not UTF-8",
                    path.display()
                ))
            })?;
        validate_identifier("Feature", feature_id)?;
        feature_ids.push(feature_id.to_string());
    }
    feature_ids.sort();
    Ok(feature_ids)
}

fn snapshot_manifest(workspace_id: &str, feature_ids: &[String]) -> String {
    let mut manifest = format!("format={SNAPSHOT_FORMAT}\nworkspace={workspace_id}\n");
    for feature_id in feature_ids {
        manifest.push_str("feature=");
        manifest.push_str(feature_id);
        manifest.push('\n');
    }
    manifest
}

fn read_snapshot_manifest(snapshot: &Path, expected_workspace: &str) -> Result<Vec<String>> {
    let manifest = fs::read_to_string(snapshot.join(SNAPSHOT_MANIFEST)).map_err(|error| {
        FeatureStorageError::InvalidSnapshot(format!("cannot read manifest: {error}"))
    })?;
    let mut format = None;
    let mut workspace = None;
    let mut feature_ids = Vec::new();
    for line in manifest.lines() {
        if let Some(value) = line.strip_prefix("format=") {
            format = Some(value);
        } else if let Some(value) = line.strip_prefix("workspace=") {
            workspace = Some(value);
        } else if let Some(value) = line.strip_prefix("feature=") {
            validate_identifier("Feature", value)?;
            feature_ids.push(value.to_string());
        } else {
            return Err(FeatureStorageError::InvalidSnapshot(format!(
                "unknown manifest entry `{line}`"
            )));
        }
    }
    if format != Some(SNAPSHOT_FORMAT) {
        return Err(FeatureStorageError::InvalidSnapshot(
            "unsupported snapshot format".to_string(),
        ));
    }
    if workspace != Some(expected_workspace) {
        return Err(FeatureStorageError::InvalidSnapshot(format!(
            "snapshot Workspace `{}` does not match `{expected_workspace}`",
            workspace.unwrap_or("<missing>")
        )));
    }
    feature_ids.sort();
    if feature_ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(FeatureStorageError::InvalidSnapshot(
            "duplicate Feature entry".to_string(),
        ));
    }
    for feature_id in &feature_ids {
        let database = snapshot
            .join("features")
            .join(format!("{feature_id}.sqlite"));
        if !database.is_file() {
            return Err(FeatureStorageError::InvalidSnapshot(format!(
                "Feature database `{}` is missing",
                database.display()
            )));
        }
    }
    Ok(feature_ids)
}

fn temporary_sibling(destination: &Path, operation: &str) -> Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        FeatureStorageError::Operation(format!(
            "destination `{}` has no parent",
            destination.display()
        ))
    })?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            FeatureStorageError::Operation(format!(
                "destination `{}` has no UTF-8 filename",
                destination.display()
            ))
        })?;
    Ok(parent.join(format!(".{name}.{operation}-{}", Uuid::now_v7())))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rusqlite::OptionalExtension;

    use super::*;

    fn create_items(transaction: &Transaction<'_>) -> Result<()> {
        transaction.execute_batch(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE);",
        )?;
        Ok(())
    }

    fn add_audit(transaction: &Transaction<'_>) -> Result<()> {
        transaction.execute_batch(
            "CREATE TABLE audit (id INTEGER PRIMARY KEY, item_id INTEGER NOT NULL, note TEXT NOT NULL);",
        )?;
        Ok(())
    }

    const MIGRATIONS: &[FeatureMigration] = &[
        FeatureMigration::new(1, "create items", create_items),
        FeatureMigration::new(2, "add audit", add_audit),
    ];

    fn failing_migration(transaction: &Transaction<'_>) -> Result<()> {
        transaction.execute_batch("CREATE TABLE partial (id INTEGER PRIMARY KEY);")?;
        Err(FeatureStorageError::operation(
            "intentional migration failure",
        ))
    }

    const FAILING_MIGRATIONS: &[FeatureMigration] = &[
        FeatureMigration::new(1, "create items", create_items),
        FeatureMigration::new(2, "fail", failing_migration),
    ];

    fn registration(feature_id: &'static str) -> FeatureRegistration {
        FeatureRegistration::new(feature_id, MIGRATIONS)
    }

    fn insert(database: &FeatureDatabase, value: &str) -> Result<()> {
        database.transaction(|transaction| {
            transaction.execute("INSERT INTO items (value) VALUES (?1)", [value])?;
            Ok(())
        })
    }

    fn values(database: &FeatureDatabase) -> Result<Vec<String>> {
        database.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT value FROM items ORDER BY id")?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
    }

    #[test]
    fn open_migrate_transaction_rollback_and_reopen_persist() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("subject-memory")).unwrap();
        let database = workspace.open(&feature).unwrap();
        assert_eq!(database.schema_version().unwrap(), 2);
        insert(&database, "committed").unwrap();
        let error = database
            .transaction::<()>(|transaction| {
                transaction.execute("INSERT INTO items (value) VALUES ('rolled-back')", [])?;
                Err(FeatureStorageError::operation("reject candidate"))
            })
            .unwrap_err();
        assert!(error.to_string().contains("reject candidate"));
        assert_eq!(values(&database).unwrap(), ["committed"]);

        manager.shutdown().unwrap();
        assert!(matches!(
            values(&database).unwrap_err(),
            FeatureStorageError::DatabaseClosed
        ));

        let reopened = FeatureStorage::new(temp.path().join("storage"));
        let workspace = reopened.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("subject-memory")).unwrap();
        let database = workspace.open(&feature).unwrap();
        assert_eq!(values(&database).unwrap(), ["committed"]);
        assert_eq!(database.schema_version().unwrap(), 2);
    }

    #[test]
    fn workspace_and_feature_databases_are_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let feature_a = manager.register(registration("feature-a")).unwrap();
        let feature_b = manager.register(registration("feature-b")).unwrap();
        let workspace_a = manager.workspace("workspace-a").unwrap();
        let workspace_b = manager.workspace("workspace-b").unwrap();
        let a_a = workspace_a.open(&feature_a).unwrap();
        let a_b = workspace_a.open(&feature_b).unwrap();
        let b_a = workspace_b.open(&feature_a).unwrap();
        insert(&a_a, "workspace-a/feature-a").unwrap();
        insert(&a_b, "workspace-a/feature-b").unwrap();
        insert(&b_a, "workspace-b/feature-a").unwrap();
        assert_eq!(values(&a_a).unwrap(), ["workspace-a/feature-a"]);
        assert_eq!(values(&a_b).unwrap(), ["workspace-a/feature-b"]);
        assert_eq!(values(&b_a).unwrap(), ["workspace-b/feature-a"]);
        assert_eq!(a_a.schema_version().unwrap(), 2);
        assert_eq!(a_b.schema_version().unwrap(), 2);
        assert_eq!(b_a.schema_version().unwrap(), 2);
    }

    #[test]
    fn concurrent_open_migrates_once_and_reuses_connection() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let threads = (0..2)
            .map(|_| {
                let workspace = workspace.clone();
                let feature = feature.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    workspace.open(&feature).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let mut databases = Vec::new();
        for thread in threads {
            databases.push(thread.join().unwrap());
        }
        assert!(Arc::ptr_eq(&databases[0].inner, &databases[1].inner));
        assert_eq!(databases[0].schema_version().unwrap(), 2);
    }

    #[test]
    fn independent_managers_serialize_migration_and_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let first_manager = FeatureStorage::new(&root);
        let second_manager = FeatureStorage::new(&root);
        let barrier = Arc::new(Barrier::new(3));
        let open = |manager: FeatureStorage, barrier: Arc<Barrier>| {
            thread::spawn(move || {
                let workspace = manager.workspace("workspace-a").unwrap();
                let feature = workspace.register(registration("feature-a")).unwrap();
                barrier.wait();
                let database = workspace.open(&feature).unwrap();
                (manager, database)
            })
        };
        let first = open(first_manager, barrier.clone());
        let second = open(second_manager, barrier.clone());
        barrier.wait();
        let (first_manager, first_database) = first.join().unwrap();
        let (second_manager, second_database) = second.join().unwrap();
        assert_eq!(first_database.schema_version().unwrap(), 2);
        assert_eq!(second_database.schema_version().unwrap(), 2);

        let writers = (0..16)
            .map(|index| {
                let database = if index % 2 == 0 {
                    first_database.clone()
                } else {
                    second_database.clone()
                };
                thread::spawn(move || insert(&database, &format!("manager-value-{index}")))
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        assert_eq!(values(&first_database).unwrap().len(), 16);
        drop((first_manager, second_manager));
    }

    #[test]
    fn concurrent_writes_are_serialized_without_lost_updates() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        let database = workspace.open(&feature).unwrap();
        let threads = (0..8)
            .map(|index| {
                let database = database.clone();
                thread::spawn(move || insert(&database, &format!("value-{index}")))
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(values(&database).unwrap().len(), 8);
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_version() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace
            .register(FeatureRegistration::new("broken", FAILING_MIGRATIONS))
            .unwrap();
        let error = workspace.open(&feature).err().unwrap();
        assert!(matches!(error, FeatureStorageError::MigrationFailed { .. }));
        let path = temp
            .path()
            .join("storage/workspace-a/features/broken.sqlite");
        let connection = Connection::open(path).unwrap();
        let migration_table_exists: bool = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [MIGRATION_TABLE],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(!migration_table_exists);
        let partial_exists: bool = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'partial'",
                [],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(!partial_exists);
    }

    #[test]
    fn rejects_invalid_identifiers_and_newer_schema() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        assert!(manager.workspace("../escape").is_err());
        assert!(
            manager
                .register(FeatureRegistration::new("bad/name", MIGRATIONS))
                .is_err()
        );

        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        let database = workspace.open(&feature).unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    &format!("INSERT INTO {MIGRATION_TABLE} (version, name) VALUES (99, 'future')"),
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        drop(database);
        manager.shutdown().unwrap();

        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        assert!(matches!(
            workspace.open(&feature).err().unwrap(),
            FeatureStorageError::UnsupportedSchema { actual: 99, .. }
        ));
    }

    #[test]
    fn registration_is_lazy_and_disable_preserves_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let manager = FeatureStorage::new(&root);
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        assert!(!root.exists(), "registration must not create storage");
        let database = workspace.open(&feature).unwrap();
        insert(&database, "retained").unwrap();
        manager.shutdown().unwrap();

        let disabled = FeatureStorage::new(&root);
        assert!(root.join("workspace-a/features/feature-a.sqlite").is_file());
        disabled.shutdown().unwrap();

        let enabled = FeatureStorage::new(&root);
        let workspace = enabled.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        assert_eq!(
            values(&workspace.open(&feature).unwrap()).unwrap(),
            ["retained"]
        );
    }

    #[test]
    fn backup_restore_is_consistent_and_includes_disabled_features() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let snapshot = temp.path().join("snapshot");
        let manager = FeatureStorage::new(&root);
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        let database = workspace.open(&feature).unwrap();
        insert(&database, "snapshot-value").unwrap();
        workspace.backup(&snapshot).unwrap();
        insert(&database, "after-snapshot").unwrap();
        workspace.delete().unwrap();
        assert!(matches!(
            workspace.open(&feature).err().unwrap(),
            FeatureStorageError::WorkspaceMaintenance(_)
        ));
        drop(database);
        drop(workspace);
        drop(manager);

        let manager = FeatureStorage::new(&root);
        let workspace = manager.workspace("workspace-a").unwrap();
        workspace.restore(&snapshot).unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        assert_eq!(
            values(&workspace.open(&feature).unwrap()).unwrap(),
            ["snapshot-value"]
        );
    }

    #[test]
    fn workspace_delete_closes_handles_and_is_target_limited() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let feature = manager.register(registration("feature-a")).unwrap();
        let workspace_a = manager.workspace("workspace-a").unwrap();
        let workspace_b = manager.workspace("workspace-b").unwrap();
        let database_a = workspace_a.open(&feature).unwrap();
        let database_b = workspace_b.open(&feature).unwrap();
        insert(&database_a, "a").unwrap();
        insert(&database_b, "b").unwrap();
        workspace_a.delete().unwrap();
        assert!(matches!(
            values(&database_a).unwrap_err(),
            FeatureStorageError::DatabaseClosed
        ));
        assert_eq!(values(&database_b).unwrap(), ["b"]);
        assert!(!temp.path().join("storage/workspace-a").exists());
        assert!(temp.path().join("storage/workspace-b").exists());
    }

    #[test]
    fn shutdown_waits_for_operation_and_rejects_later_use() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("workspace-a").unwrap();
        let feature = workspace.register(registration("feature-a")).unwrap();
        let database = workspace.open(&feature).unwrap();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let operation = {
            let database = database.clone();
            let entered = entered.clone();
            let release = release.clone();
            thread::spawn(move || {
                database.with_connection(|_| {
                    entered.wait();
                    release.wait();
                    Ok(())
                })
            })
        };
        entered.wait();
        let shutdown = {
            let manager = manager.clone();
            thread::spawn(move || manager.shutdown())
        };
        while database.inner.accepting_operations.load(Ordering::Acquire) {
            thread::yield_now();
        }
        assert!(!shutdown.is_finished());
        assert!(matches!(
            values(&database).unwrap_err(),
            FeatureStorageError::DatabaseClosed
        ));
        release.wait();
        operation.join().unwrap().unwrap();
        shutdown.join().unwrap().unwrap();
        assert!(matches!(
            values(&database).unwrap_err(),
            FeatureStorageError::DatabaseClosed
        ));
        assert!(matches!(
            workspace.open(&feature).err().unwrap(),
            FeatureStorageError::ShuttingDown
        ));
    }
}
