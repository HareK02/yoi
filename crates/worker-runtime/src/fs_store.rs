use crate::catalog::{CreateWorkerRequest, WorkingDirectoryStatus};
use crate::config_bundle::ConfigBundle;
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::identity::{WorkerId, WorkerRef};
use crate::management::{RuntimeBackendKind, RuntimeStatus};
use serde::{Deserialize, Serialize};
use session_store::{FsStore, Store, TraceEntry, WorkerMetadata};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 1;
const RUNTIME_FILE: &str = "runtime.json";
const WORKERS_DIR: &str = "workers";
const LEGACY_RUNTIMES_DIR: &str = "runtimes";
const WORKER_FILE: &str = "worker.json";
const LEGACY_OBSERVATIONS_FILE: &str = "observations.jsonl";

static NEXT_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Options for constructing a filesystem-backed Runtime store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStoreOptions {
    /// Root directory containing this Runtime's store data.
    pub root: PathBuf,
    pub display_name: Option<String>,
}

impl FsRuntimeStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            display_name: None,
        }
    }
}

/// Filesystem persistence boundary for one Worker Runtime state.
///
/// Authority is Runtime-local typed Worker identity. Legacy pod paths, socket
/// paths, and session paths are deliberately not part of the layout or lookup API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStore {
    root: PathBuf,
}

impl FsRuntimeStore {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.root
    }

    /// Migrate legacy global Worker metadata/Session sources into canonical
    /// Runtime-owned Worker aggregates. Legacy roots are read-only and remain
    /// in place; only the canonical aggregate is used after this completes.
    pub fn migrate_legacy_worker_aggregates(
        runtime_root: impl Into<PathBuf>,
        legacy_session_root: impl AsRef<Path>,
        legacy_worker_metadata_root: impl AsRef<Path>,
    ) -> Result<(), RuntimeError> {
        let opened = Self::open_or_create(runtime_root.into())?;
        let Some(persisted) = opened.state else {
            return Ok(());
        };
        migrate_worker_aggregates(
            &opened.store.root,
            legacy_session_root.as_ref(),
            legacy_worker_metadata_root.as_ref(),
            persisted.workers.values(),
        )
    }

    pub(crate) fn open_or_create(root: PathBuf) -> Result<OpenedFsRuntimeStore, RuntimeError> {
        let existed = root.exists();
        if existed && !root.is_dir() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "open runtime store",
                path: root,
                message: "runtime path exists but is not a directory".to_string(),
            });
        }

        if existed {
            migrate_legacy_single_runtime_layout(&root)?;
        }

        fs::create_dir_all(root.join(WORKERS_DIR)).map_err(|source| RuntimeError::StoreIo {
            operation: "create runtime store",
            path: root.join(WORKERS_DIR),
            source,
        })?;
        let legacy_events = root.join("events.jsonl");
        match fs::remove_file(&legacy_events) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RuntimeError::StoreIo {
                    operation: "remove legacy runtime events",
                    path: legacy_events,
                    source,
                });
            }
        }

        let store = Self { root };
        let state = if existed {
            Some(store.load_runtime_state()?)
        } else {
            None
        };
        Ok(OpenedFsRuntimeStore { store, state })
    }

    pub(crate) fn write_runtime_snapshot(
        &self,
        state: &PersistedRuntimeState,
    ) -> Result<(), RuntimeError> {
        let snapshot = RuntimeSnapshot::from_persisted(state);
        atomic_write_json(&self.runtime_path(), &snapshot, "write runtime snapshot")
    }

    pub(crate) fn write_worker_snapshot(
        &self,
        worker: &PersistedWorkerRecord,
    ) -> Result<(), RuntimeError> {
        self.ensure_worker_ref(&worker.worker_ref)?;
        let worker_dir = self.worker_dir(&worker.worker_id);
        fs::create_dir_all(&worker_dir).map_err(|source| RuntimeError::StoreIo {
            operation: "create worker store",
            path: worker_dir.clone(),
            source,
        })?;
        atomic_write_json(
            &worker_dir.join(WORKER_FILE),
            &WorkerSnapshot::from_persisted(worker),
            "write worker snapshot",
        )?;
        remove_legacy_observations(&worker_dir);
        Ok(())
    }

    pub(crate) fn delete_worker_snapshot(&self, worker_id: &WorkerId) -> Result<(), RuntimeError> {
        let worker_dir = self.worker_dir(worker_id);
        if !worker_dir.exists() {
            return Ok(());
        }
        fs::remove_dir_all(&worker_dir).map_err(|source| RuntimeError::StoreIo {
            operation: "delete worker store",
            path: worker_dir,
            source,
        })
    }

    pub(crate) fn load_runtime_state(&self) -> Result<PersistedRuntimeState, RuntimeError> {
        let runtime_path = self.runtime_path();
        let mut snapshot: RuntimeSnapshot = read_json(&runtime_path, "read runtime snapshot")?;
        snapshot.validate(&runtime_path)?;

        let workers_dir = self.root.join(WORKERS_DIR);
        if !workers_dir.exists() {
            return Err(RuntimeError::StoreMissing {
                operation: "read workers",
                path: workers_dir,
            });
        }
        if !workers_dir.is_dir() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read workers",
                path: workers_dir,
                message: "workers path exists but is not a directory".to_string(),
            });
        }

        let mut workers = BTreeMap::new();
        let mut worker_dirs = fs::read_dir(&workers_dir)
            .map_err(|source| RuntimeError::StoreIo {
                operation: "read workers",
                path: workers_dir.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| RuntimeError::StoreIo {
                operation: "read workers",
                path: workers_dir.clone(),
                source,
            })?;
        worker_dirs.sort_by_key(|entry| entry.path());

        for entry in worker_dirs {
            let path = entry.path();
            if !path.is_dir() {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    None,
                    "ignored invalid worker store entry while loading runtime store",
                );
                continue;
            }
            let worker_snapshot_path = path.join(WORKER_FILE);
            let worker_snapshot: WorkerSnapshot =
                match read_json(&worker_snapshot_path, "read worker snapshot") {
                    Ok(snapshot) => snapshot,
                    Err(_error) => {
                        record_worker_load_diagnostic(
                            &mut snapshot,
                            None,
                            "ignored corrupt worker snapshot while loading runtime store",
                        );
                        continue;
                    }
                };
            if worker_snapshot.validate(&worker_snapshot_path).is_err() {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    Some(worker_snapshot.worker_ref.clone()),
                    "ignored invalid worker snapshot while loading runtime store",
                );
                continue;
            }
            remove_legacy_observations(&path);
            let worker = worker_snapshot.into_persisted();
            if workers.insert(worker.worker_id.clone(), worker).is_some() {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    None,
                    "ignored duplicate worker snapshot while loading runtime store",
                );
            }
        }

        Ok(snapshot.into_persisted(workers))
    }

    fn ensure_worker_ref(&self, _worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn runtime_path(&self) -> PathBuf {
        self.root.join(RUNTIME_FILE)
    }

    fn worker_dir(&self, worker_id: &WorkerId) -> PathBuf {
        self.root.join(WORKERS_DIR).join(worker_id.to_string())
    }
}

fn remove_legacy_observations(worker_dir: &Path) {
    let path = worker_dir.join(LEGACY_OBSERVATIONS_FILE);
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

#[derive(Debug)]
pub(crate) struct OpenedFsRuntimeStore {
    pub(crate) store: FsRuntimeStore,
    pub(crate) state: Option<PersistedRuntimeState>,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedRuntimeState {
    pub(crate) display_name: Option<String>,
    pub(crate) status: RuntimeStatus,
    pub(crate) next_worker_sequence: u64,
    pub(crate) next_diagnostic_id: u64,
    pub(crate) workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    pub(crate) workspace_owners: BTreeMap<String, String>,
    pub(crate) config_bundles: BTreeMap<String, ConfigBundle>,
    pub(crate) diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedWorkerRecord {
    pub(crate) worker_ref: WorkerRef,
    pub(crate) worker_id: WorkerId,
    pub(crate) request: CreateWorkerRequest,
    /// Last generation durably reserved for this Worker's execution.
    pub(crate) run_generation: u64,
    pub(crate) workspace_id: Option<String>,
    pub(crate) working_directory: Option<WorkingDirectoryStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeSnapshot {
    schema_version: u32,
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    status: RuntimeStatus,
    next_worker_sequence: u64,
    next_diagnostic_id: u64,
    #[serde(default)]
    config_bundles: BTreeMap<String, ConfigBundle>,
    #[serde(default)]
    workspace_owners: BTreeMap<String, String>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

fn record_worker_load_diagnostic(
    snapshot: &mut RuntimeSnapshot,
    worker_ref: Option<WorkerRef>,
    message: impl Into<String>,
) {
    let id = snapshot.next_diagnostic_id;
    snapshot.next_diagnostic_id = snapshot.next_diagnostic_id.saturating_add(1);
    snapshot.diagnostics.push(RuntimeDiagnostic {
        id,
        worker_ref,
        severity: DiagnosticSeverity::Warning,
        code: "worker_snapshot_ignored".to_string(),
        message: message.into(),
    });
}

impl RuntimeSnapshot {
    fn from_persisted(state: &PersistedRuntimeState) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            display_name: state.display_name.clone(),
            backend: RuntimeBackendKind::FsStore,
            status: state.status,
            next_worker_sequence: state.next_worker_sequence,
            next_diagnostic_id: state.next_diagnostic_id,
            config_bundles: state.config_bundles.clone(),
            workspace_owners: state.workspace_owners.clone(),
            diagnostics: state.diagnostics.clone(),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), RuntimeError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read runtime snapshot",
                path: path.to_path_buf(),
                message: format!(
                    "unsupported schema version {}, expected {}",
                    self.schema_version, SCHEMA_VERSION
                ),
            });
        }
        if self.backend != RuntimeBackendKind::FsStore {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read runtime snapshot",
                path: path.to_path_buf(),
                message: format!("runtime snapshot backend is {:?}", self.backend),
            });
        }
        Ok(())
    }

    fn into_persisted(
        self,
        workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    ) -> PersistedRuntimeState {
        PersistedRuntimeState {
            display_name: self.display_name,
            status: self.status,
            next_worker_sequence: self.next_worker_sequence,
            next_diagnostic_id: self.next_diagnostic_id,
            workers,
            config_bundles: self.config_bundles,
            workspace_owners: self.workspace_owners,
            diagnostics: self.diagnostics,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkerSnapshot {
    schema_version: u32,
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    request: CreateWorkerRequest,
    #[serde(default)]
    run_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_directory: Option<WorkingDirectoryStatus>,
    /// One-way migration input for schema-v1 snapshots. New snapshots never
    /// write the removed execution projection.
    #[serde(default, rename = "execution", skip_serializing)]
    legacy_execution: Option<LegacyWorkerExecutionProjection>,
}

#[derive(Clone, Debug, Deserialize)]
struct LegacyWorkerExecutionProjection {
    #[serde(default)]
    working_directory: Option<WorkingDirectoryStatus>,
}

impl WorkerSnapshot {
    fn from_persisted(worker: &PersistedWorkerRecord) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            worker_ref: worker.worker_ref.clone(),
            worker_id: worker.worker_id.clone(),
            request: worker.request.clone(),
            run_generation: worker.run_generation,
            workspace_id: worker.workspace_id.clone(),
            working_directory: worker.working_directory.clone(),
            legacy_execution: None,
        }
    }

    fn validate(&self, path: &Path) -> Result<(), RuntimeError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read worker snapshot",
                path: path.to_path_buf(),
                message: format!(
                    "unsupported schema version {}, expected {}",
                    self.schema_version, SCHEMA_VERSION
                ),
            });
        }
        if self.worker_ref.worker_id != self.worker_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read worker snapshot",
                path: path.to_path_buf(),
                message: format!(
                    "worker_ref id {} does not match worker_id {}",
                    self.worker_ref.worker_id, self.worker_id
                ),
            });
        }
        Ok(())
    }

    fn into_persisted(self) -> PersistedWorkerRecord {
        let workspace_id = self.workspace_id.or_else(|| {
            self.request
                .workspace_api
                .as_ref()
                .map(|workspace_api| workspace_api.workspace_id.clone())
        });
        PersistedWorkerRecord {
            worker_ref: self.worker_ref,
            worker_id: self.worker_id,
            request: self.request,
            run_generation: self.run_generation,
            workspace_id,
            working_directory: self.working_directory.or_else(|| {
                self.legacy_execution
                    .and_then(|execution| execution.working_directory)
            }),
        }
    }
}

fn migrate_legacy_single_runtime_layout(root: &Path) -> Result<(), RuntimeError> {
    if root.join(RUNTIME_FILE).exists() {
        return Ok(());
    }
    let legacy_root = root.join(LEGACY_RUNTIMES_DIR);
    if !legacy_root.is_dir() {
        return Ok(());
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(&legacy_root).map_err(|source| RuntimeError::StoreIo {
        operation: "read legacy runtime store root",
        path: legacy_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| RuntimeError::StoreIo {
            operation: "read legacy runtime store entry",
            path: legacy_root.clone(),
            source,
        })?;
        let path = entry.path();
        if path.join(RUNTIME_FILE).is_file() {
            candidates.push(path);
        }
    }
    if candidates.is_empty() {
        return Ok(());
    }
    if candidates.len() > 1 {
        return Err(RuntimeError::StoreCorrupt {
            operation: "migrate legacy runtime store",
            path: legacy_root,
            message: "multiple legacy runtime directories exist; choose a concrete fs root"
                .to_string(),
        });
    }

    let legacy_dir = candidates.remove(0);
    rename_if_exists(
        &legacy_dir.join(RUNTIME_FILE),
        &root.join(RUNTIME_FILE),
        "migrate legacy runtime snapshot",
    )?;
    rename_if_exists(
        &legacy_dir.join(WORKERS_DIR),
        &root.join(WORKERS_DIR),
        "migrate legacy runtime workers",
    )?;
    Ok(())
}

fn rename_if_exists(src: &Path, dst: &Path, operation: &'static str) -> Result<(), RuntimeError> {
    if !src.exists() {
        return Ok(());
    }
    if dst.exists() {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: dst.to_path_buf(),
            message: format!(
                "cannot migrate {} because destination already exists",
                src.display()
            ),
        });
    }
    fs::rename(src, dst).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: dst.to_path_buf(),
        source,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerAggregateMigrationManifest {
    schema_version: u32,
    complete: bool,
    workers: BTreeMap<String, WorkerAggregateMigrationCheckpoint>,
    #[serde(default)]
    diagnostics: Vec<WorkerAggregateMigrationDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkerAggregateMigrationDiagnostic {
    kind: String,
    source: String,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerAggregateMigrationCheckpoint {
    worker_name: String,
    session_id: Option<String>,
    state: String,
}

#[derive(Serialize, Deserialize)]
struct CanonicalSessionManifest {
    schema_version: u32,
    session_id: session_store::SessionId,
}

fn migrate_worker_aggregates<'a>(
    runtime_root: &Path,
    legacy_session_root: &Path,
    legacy_worker_metadata_root: &Path,
    workers: impl Iterator<Item = &'a PersistedWorkerRecord>,
) -> Result<(), RuntimeError> {
    if !legacy_session_root.exists() && !legacy_worker_metadata_root.exists() {
        return Ok(());
    }
    let operation = "migrate legacy Worker aggregates";
    fs::create_dir_all(runtime_root).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: runtime_root.to_path_buf(),
        source,
    })?;
    let lock_path = runtime_root.join(".worker-aggregate-v1.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: lock_path.clone(),
            source,
        })?;
    lock.lock().map_err(|source| RuntimeError::StoreIo {
        operation,
        path: lock_path,
        source,
    })?;

    let migration_dir = runtime_root.join("migrations");
    fs::create_dir_all(&migration_dir).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: migration_dir.clone(),
        source,
    })?;
    let manifest_path = migration_dir.join("worker-aggregate-v1.json");
    let mut manifest = if manifest_path.is_file() {
        let manifest: WorkerAggregateMigrationManifest = read_json(&manifest_path, operation)?;
        if manifest.schema_version != 1 {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path: manifest_path,
                message: format!(
                    "unsupported Worker aggregate migration schema {}",
                    manifest.schema_version
                ),
            });
        }
        manifest
    } else {
        WorkerAggregateMigrationManifest {
            schema_version: 1,
            complete: false,
            workers: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    };

    let workers = workers.cloned().collect::<Vec<_>>();
    let catalog_worker_ids = workers
        .iter()
        .map(|worker| worker.worker_ref.worker_id.to_string())
        .collect::<BTreeSet<_>>();
    manifest.complete = false;
    manifest.diagnostics = collect_legacy_orphan_diagnostics(
        legacy_session_root,
        legacy_worker_metadata_root,
        &workers,
        operation,
    )?;
    let shared_sessions = shared_legacy_session_references(legacy_worker_metadata_root, operation)?;
    if !shared_sessions.is_empty() {
        for (session_id, worker_ids) in &shared_sessions {
            manifest.diagnostics.push(WorkerAggregateMigrationDiagnostic {
                kind: "shared_session_reference".to_string(),
                source: legacy_session_root.join(session_id).display().to_string(),
                message: format!(
                    "legacy Session {session_id} is referenced by metadata sources {} and was not assigned",
                    worker_ids.join(", ")
                ),
            });
            for source_name in worker_ids {
                let Some(worker_id) = source_name.strip_prefix("worker-runtime-") else {
                    continue;
                };
                if !catalog_worker_ids.contains(worker_id) {
                    continue;
                }
                manifest.workers.insert(
                    worker_id.to_string(),
                    WorkerAggregateMigrationCheckpoint {
                        worker_name: source_name.clone(),
                        session_id: Some(session_id.clone()),
                        state: "shared_session_collision".to_string(),
                    },
                );
            }
        }
        atomic_write_json(&manifest_path, &manifest, operation)?;
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: manifest_path,
            message: "legacy Session ownership is ambiguous; see migration diagnostics".to_string(),
        });
    }
    atomic_write_json(&manifest_path, &manifest, operation)?;

    for worker in workers {
        let worker_id = worker.worker_ref.worker_id.to_string();
        let worker_name = format!("worker-runtime-{}", worker.worker_ref.worker_id);
        let legacy_metadata = legacy_worker_metadata_root
            .join(&worker_name)
            .join("metadata.json");
        let target_root = runtime_root.join(WORKERS_DIR).join(&worker_id);
        let target_metadata = target_root.join("metadata.json");

        if !legacy_metadata.is_file() {
            manifest.workers.insert(
                worker_id,
                WorkerAggregateMigrationCheckpoint {
                    worker_name,
                    session_id: None,
                    state: "no_legacy_metadata".to_string(),
                },
            );
            atomic_write_json(&manifest_path, &manifest, operation)?;
            continue;
        }

        let legacy_metadata_bytes =
            fs::read(&legacy_metadata).map_err(|source| RuntimeError::StoreIo {
                operation,
                path: legacy_metadata.clone(),
                source,
            })?;
        let metadata: WorkerMetadata =
            serde_json::from_slice(&legacy_metadata_bytes).map_err(|source| {
                RuntimeError::StoreCorrupt {
                    operation,
                    path: legacy_metadata.clone(),
                    message: source.to_string(),
                }
            })?;
        if metadata.worker_name != worker_name {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path: legacy_metadata,
                message: format!(
                    "legacy Worker metadata identity mismatch: expected `{worker_name}`, found `{}`",
                    metadata.worker_name
                ),
            });
        }

        let session_id = metadata.active.as_ref().map(|active| active.session_id);
        if let Some(session_id) = session_id {
            let legacy_session = legacy_session_root.join(session_id.to_string());
            if !legacy_session.is_dir() {
                return Err(RuntimeError::StoreCorrupt {
                    operation,
                    path: legacy_session,
                    message: format!(
                        "Worker `{worker_name}` references missing legacy Session {session_id}"
                    ),
                });
            }
            validate_legacy_session(legacy_session_root, session_id, operation)?;
            migrate_one_session(
                &legacy_session,
                &target_root.join("session"),
                session_id,
                operation,
            )?;
        }

        copy_atomic_or_validate(
            &legacy_metadata_bytes,
            &target_metadata,
            operation,
            "Worker metadata collision",
        )?;
        manifest.workers.insert(
            worker_id,
            WorkerAggregateMigrationCheckpoint {
                worker_name,
                session_id: session_id.map(|id| id.to_string()),
                state: "migrated".to_string(),
            },
        );
        atomic_write_json(&manifest_path, &manifest, operation)?;
    }

    manifest.complete = true;
    atomic_write_json(&manifest_path, &manifest, operation)
}

fn shared_legacy_session_references(
    legacy_worker_metadata_root: &Path,
    operation: &'static str,
) -> Result<BTreeMap<String, Vec<String>>, RuntimeError> {
    let mut owners = BTreeMap::<String, Vec<String>>::new();
    if !legacy_worker_metadata_root.is_dir() {
        return Ok(owners);
    }
    for entry in sorted_directory_entries(legacy_worker_metadata_root, operation)? {
        let metadata_path = entry.path().join("metadata.json");
        if !entry.path().is_dir() || !metadata_path.is_file() {
            continue;
        }
        let source_name = entry.file_name().to_string_lossy().into_owned();
        let metadata: WorkerMetadata = read_json(&metadata_path, operation)?;
        if let Some(active) = metadata.active {
            owners
                .entry(active.session_id.to_string())
                .or_default()
                .push(source_name);
        }
    }
    owners.retain(|_, source_names| source_names.len() > 1);
    Ok(owners)
}

fn collect_legacy_orphan_diagnostics(
    legacy_session_root: &Path,
    legacy_worker_metadata_root: &Path,
    workers: &[PersistedWorkerRecord],
    operation: &'static str,
) -> Result<Vec<WorkerAggregateMigrationDiagnostic>, RuntimeError> {
    let expected_worker_names = workers
        .iter()
        .map(|worker| format!("worker-runtime-{}", worker.worker_ref.worker_id))
        .collect::<BTreeSet<_>>();
    let mut referenced_sessions = BTreeSet::new();
    for worker_name in &expected_worker_names {
        let metadata_path = legacy_worker_metadata_root
            .join(worker_name)
            .join("metadata.json");
        if metadata_path.is_file() {
            let metadata: WorkerMetadata = read_json(&metadata_path, operation)?;
            if let Some(active) = metadata.active {
                referenced_sessions.insert(active.session_id.to_string());
            }
        }
    }

    let mut diagnostics = Vec::new();
    if legacy_worker_metadata_root.is_dir() {
        for entry in sorted_directory_entries(legacy_worker_metadata_root, operation)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir()
                && entry.path().join("metadata.json").is_file()
                && !expected_worker_names.contains(&name)
            {
                diagnostics.push(WorkerAggregateMigrationDiagnostic {
                    kind: "orphan_worker_metadata".to_string(),
                    source: entry.path().display().to_string(),
                    message:
                        "legacy Worker metadata has no Runtime catalog Worker and was left in place"
                            .to_string(),
                });
            }
        }
    }
    if legacy_session_root.is_dir() {
        for entry in sorted_directory_entries(legacy_session_root, operation)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !referenced_sessions.contains(&name) {
                diagnostics.push(WorkerAggregateMigrationDiagnostic {
                    kind: "orphan_session".to_string(),
                    source: entry.path().display().to_string(),
                    message: "legacy Session has no catalog-backed Worker reference and was left in place"
                        .to_string(),
                });
            }
        }
    }
    Ok(diagnostics)
}

fn validate_legacy_session(
    legacy_session_root: &Path,
    session_id: session_store::SessionId,
    operation: &'static str,
) -> Result<(), RuntimeError> {
    let legacy_session = legacy_session_root.join(session_id.to_string());
    for entry in sorted_directory_entries(&legacy_session, operation)? {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.clone(),
            source,
        })?;
        if !file_type.is_file() {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path,
                message: "legacy Session contains a non-file entry".to_string(),
            });
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(segment) = name.strip_suffix(".trace.jsonl") {
            segment
                .parse::<session_store::SegmentId>()
                .map_err(|error| RuntimeError::StoreCorrupt {
                    operation,
                    path: path.clone(),
                    message: format!("invalid trace segment filename: {error}"),
                })?;
            validate_trace_jsonl(&path, operation)?;
        } else if let Some(segment) = name.strip_suffix(".jsonl") {
            segment
                .parse::<session_store::SegmentId>()
                .map_err(|error| RuntimeError::StoreCorrupt {
                    operation,
                    path,
                    message: format!("invalid segment filename: {error}"),
                })?;
        } else {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path,
                message: "legacy Session contains an unknown file".to_string(),
            });
        }
    }
    let store = FsStore::new(legacy_session_root).map_err(|error| RuntimeError::StoreCorrupt {
        operation,
        path: legacy_session_root.to_path_buf(),
        message: error.to_string(),
    })?;
    for segment_id in
        store
            .list_segments(session_id)
            .map_err(|error| RuntimeError::StoreCorrupt {
                operation,
                path: legacy_session_root.join(session_id.to_string()),
                message: error.to_string(),
            })?
    {
        store
            .read_all(session_id, segment_id)
            .map_err(|error| RuntimeError::StoreCorrupt {
                operation,
                path: legacy_session_root
                    .join(session_id.to_string())
                    .join(format!("{segment_id}.jsonl")),
                message: error.to_string(),
            })?;
    }
    Ok(())
}

fn validate_trace_jsonl(path: &Path, operation: &'static str) -> Result<(), RuntimeError> {
    let bytes = fs::read(path).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    let complete = if bytes.last() == Some(&b'\n') {
        bytes.as_slice()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| &bytes[..=index])
            .unwrap_or(&[])
    };
    let text = std::str::from_utf8(complete).map_err(|error| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<TraceEntry>(line).map_err(|error| RuntimeError::StoreCorrupt {
            operation,
            path: path.to_path_buf(),
            message: format!("invalid trace JSONL at line {}: {error}", index + 1),
        })?;
    }
    Ok(())
}

fn migrate_one_session(
    legacy_session: &Path,
    target_session: &Path,
    session_id: session_store::SessionId,
    operation: &'static str,
) -> Result<(), RuntimeError> {
    if target_session.exists() {
        validate_existing_canonical_session(legacy_session, target_session, session_id, operation)?;
        return Ok(());
    }
    let target_root = target_session
        .parent()
        .ok_or_else(|| RuntimeError::StoreCorrupt {
            operation,
            path: target_session.to_path_buf(),
            message: "canonical Session path has no Worker aggregate parent".to_string(),
        })?;
    fs::create_dir_all(target_root).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: target_root.to_path_buf(),
        source,
    })?;
    let staging = target_root.join(format!(
        ".session.migrating-{}-{}",
        std::process::id(),
        NEXT_TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        fs::create_dir(&staging).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: staging.clone(),
            source,
        })?;
        fs::create_dir(staging.join("segments")).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: staging.join("segments"),
            source,
        })?;
        let session_manifest = CanonicalSessionManifest {
            schema_version: 1,
            session_id,
        };
        atomic_write_json(&staging.join("session.json"), &session_manifest, operation)?;
        for entry in sorted_directory_entries(legacy_session, operation)? {
            let file_type = entry.file_type().map_err(|source| RuntimeError::StoreIo {
                operation,
                path: entry.path(),
                source,
            })?;
            if !file_type.is_file() {
                return Err(RuntimeError::StoreCorrupt {
                    operation,
                    path: entry.path(),
                    message: "legacy Session contains a non-file entry".to_string(),
                });
            }
            let bytes = fs::read(entry.path()).map_err(|source| RuntimeError::StoreIo {
                operation,
                path: entry.path(),
                source,
            })?;
            copy_atomic_or_validate(
                &bytes,
                &staging.join("segments").join(entry.file_name()),
                operation,
                "staged Session file collision",
            )?;
        }
        sync_directory(&staging.join("segments"), operation)?;
        sync_directory(&staging, operation)?;
        fs::rename(&staging, target_session).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: target_session.to_path_buf(),
            source,
        })?;
        sync_directory(target_root, operation)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn validate_existing_canonical_session(
    legacy_session: &Path,
    target_session: &Path,
    session_id: session_store::SessionId,
    operation: &'static str,
) -> Result<(), RuntimeError> {
    if !target_session.is_dir() {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: target_session.to_path_buf(),
            message: "canonical Session path collision is not a directory".to_string(),
        });
    }
    let manifest_path = target_session.join("session.json");
    let manifest: CanonicalSessionManifest = read_json(&manifest_path, operation)?;
    if manifest.schema_version != 1 || manifest.session_id != session_id {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: manifest_path,
            message: format!(
                "canonical Session collision: expected Session {session_id}, found {}",
                manifest.session_id
            ),
        });
    }
    let source_entries = sorted_directory_entries(legacy_session, operation)?;
    let target_segments = target_session.join("segments");
    let target_entries = sorted_directory_entries(&target_segments, operation)?;
    let source_names = source_entries
        .iter()
        .map(|entry| entry.file_name())
        .collect::<BTreeSet<_>>();
    let target_names = target_entries
        .iter()
        .map(|entry| entry.file_name())
        .collect::<BTreeSet<_>>();
    if source_names != target_names {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: target_segments,
            message: format!(
                "canonical Session file set collision: legacy={source_names:?}, canonical={target_names:?}"
            ),
        });
    }
    for entry in source_entries {
        if !entry.path().is_file() {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path: entry.path(),
                message: "legacy Session contains a non-file entry".to_string(),
            });
        }
        let source = fs::read(entry.path()).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: entry.path(),
            source,
        })?;
        let target = target_session.join("segments").join(entry.file_name());
        if !target.is_file() {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path: target,
                message: "canonical Session entry is not a file".to_string(),
            });
        }
        let existing = fs::read(&target).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: target.clone(),
            source,
        })?;
        if source != existing {
            return Err(RuntimeError::StoreCorrupt {
                operation,
                path: target,
                message: "canonical Session file collision differs from legacy source".to_string(),
            });
        }
    }
    Ok(())
}

fn sorted_directory_entries(
    path: &Path,
    operation: &'static str,
) -> Result<Vec<fs::DirEntry>, RuntimeError> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn copy_atomic_or_validate(
    bytes: &[u8],
    target: &Path,
    operation: &'static str,
    collision: &'static str,
) -> Result<(), RuntimeError> {
    if target.exists() {
        let existing = fs::read(target).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: target.to_path_buf(),
            source,
        })?;
        if existing == bytes {
            return Ok(());
        }
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: target.to_path_buf(),
            message: collision.to_string(),
        });
    }
    let parent = target.parent().ok_or_else(|| RuntimeError::StoreCorrupt {
        operation,
        path: target.to_path_buf(),
        message: "migration target has no parent".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: parent.to_path_buf(),
        source,
    })?;
    let temp = tmp_path_for(target);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: temp.clone(),
                source,
            })?;
        file.write_all(bytes)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: temp.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| RuntimeError::StoreIo {
            operation,
            path: temp.clone(),
            source,
        })?;
        drop(file);
        fs::rename(&temp, target).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: target.to_path_buf(),
            source,
        })?;
        sync_directory(parent, operation)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn read_json<T>(path: &Path, operation: &'static str) -> Result<T, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    let file = File::open(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => RuntimeError::StoreMissing {
            operation,
            path: path.to_path_buf(),
        },
        _ => RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        },
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

fn atomic_write_json<T>(path: &Path, value: &T, operation: &'static str) -> Result<(), RuntimeError>
where
    T: Serialize,
{
    let parent = path.parent().ok_or_else(|| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: "path has no parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: parent.to_path_buf(),
        source,
    })?;

    let tmp_path = tmp_path_for(path);
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: tmp_path.clone(),
                source,
            })?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|source| {
            RuntimeError::StoreCorrupt {
                operation,
                path: tmp_path.clone(),
                message: format!("serialize json: {source}"),
            }
        })?;
        file.write_all(b"\n")
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: tmp_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| RuntimeError::StoreIo {
            operation,
            path: tmp_path.clone(),
            source,
        })?;
        drop(file);
        fs::rename(&tmp_path, path).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
        sync_directory(parent, operation)
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_result
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let sequence = NEXT_TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store");
    path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ))
}

fn sync_directory(path: &Path, operation: &'static str) -> Result<(), RuntimeError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod worker_aggregate_migration_tests {
    use super::*;
    use crate::catalog::{
        ConfigBundleRef, ProfileSelector, ProfileSourceArchiveHttpRef, ProfileSourceArchiveSource,
    };
    use crate::profile_archive::{ProfileSourceArchiveRef, ProfileSourceGraphSummary};
    use session_store::{
        FsWorkerStore, WorkerActiveSegmentRef, WorkerMetadataStore, new_segment_id, new_session_id,
    };
    use std::sync::{Arc, Barrier};

    fn request() -> CreateWorkerRequest {
        CreateWorkerRequest {
            idempotency_key: None,
            idempotency_fingerprint: None,
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            display_name: None,
            profile_source: ProfileSourceArchiveSource::Http {
                location: ProfileSourceArchiveHttpRef {
                    url: "http://127.0.0.1/profile-source.tar".to_string(),
                    etag: None,
                    archive: ProfileSourceArchiveRef {
                        id: "test-profile-source".to_string(),
                        digest: "test-digest".to_string(),
                        size_bytes: 0,
                        source_graph: ProfileSourceGraphSummary {
                            source_count: 0,
                            total_source_bytes: 0,
                            entrypoints: BTreeMap::new(),
                            import_count: 0,
                        },
                    },
                },
            },
            config_bundle: Some(ConfigBundleRef {
                id: "bundle".to_string(),
                digest: "digest".to_string(),
            }),
            initial_input: None,
            working_directory_request: None,
            working_directory: None,
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
        }
    }

    fn initialize_runtime(root: &Path, worker_ids: &[u64]) {
        let opened = FsRuntimeStore::open_or_create(root.to_path_buf()).unwrap();
        for id in worker_ids {
            let worker_id = WorkerId::new(*id);
            opened
                .store
                .write_worker_snapshot(&PersistedWorkerRecord {
                    worker_ref: WorkerRef::new(worker_id),
                    worker_id,
                    request: request(),
                    run_generation: 0,
                    workspace_id: None,
                    working_directory: None,
                })
                .unwrap();
        }
        let mut state = opened.state.unwrap_or(PersistedRuntimeState {
            display_name: None,
            status: RuntimeStatus::Running,
            next_worker_sequence: 1,
            next_diagnostic_id: 1,
            workers: BTreeMap::new(),
            workspace_owners: BTreeMap::new(),
            config_bundles: BTreeMap::new(),
            diagnostics: Vec::new(),
        });
        state.next_worker_sequence = worker_ids.iter().copied().max().unwrap_or(0) + 1;
        opened.store.write_runtime_snapshot(&state).unwrap();
    }

    fn create_legacy_worker(
        session_root: &Path,
        metadata_root: &Path,
        worker_id: u64,
    ) -> (session_store::SessionId, session_store::SegmentId) {
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        let sessions = FsStore::new(session_root).unwrap();
        sessions
            .create_segment(session_id, segment_id, &[])
            .unwrap();
        let metadata = FsWorkerStore::new(metadata_root).unwrap();
        metadata
            .write(&WorkerMetadata::new(
                format!("worker-runtime-{worker_id}"),
                Some(WorkerActiveSegmentRef {
                    session_id,
                    segment_id: Some(segment_id),
                }),
            ))
            .unwrap();
        (session_id, segment_id)
    }

    #[test]
    fn migration_preserves_legacy_ids_and_is_idempotent_for_mixed_layout() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1, 2]);
        let (session_id, segment_id) = create_legacy_worker(&sessions, &metadata, 1);

        // Worker 2 is already canonical and has no legacy source (mixed old/new).
        let aggregate2 = runtime.join("workers/2");
        fs::write(aggregate2.join("metadata.json"), b"canonical-new-worker\n").unwrap();

        FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata).unwrap();
        FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata).unwrap();

        let aggregate1 = runtime.join("workers/1");
        let manifest: CanonicalSessionManifest =
            read_json(&aggregate1.join("session/session.json"), "test").unwrap();
        assert_eq!(manifest.session_id, session_id);
        assert!(
            aggregate1
                .join(format!("session/segments/{segment_id}.jsonl"))
                .is_file()
        );
        assert!(aggregate1.join("metadata.json").is_file());
        assert_eq!(
            fs::read(aggregate2.join("metadata.json")).unwrap(),
            b"canonical-new-worker\n"
        );
        assert!(sessions.join(session_id.to_string()).is_dir());
        assert!(metadata.join("worker-runtime-1/metadata.json").is_file());
        let checkpoint: WorkerAggregateMigrationManifest =
            read_json(&runtime.join("migrations/worker-aggregate-v1.json"), "test").unwrap();
        assert!(checkpoint.complete);
        assert_eq!(checkpoint.workers["1"].state, "migrated");
        assert_eq!(checkpoint.workers["2"].state, "no_legacy_metadata");
    }

    #[test]
    fn partial_rerun_finishes_after_session_atomic_rename_before_metadata_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        let (session_id, _) = create_legacy_worker(&sessions, &metadata, 1);
        let source = sessions.join(session_id.to_string());
        let target = runtime.join("workers/1/session");
        migrate_one_session(&source, &target, session_id, "test partial migration").unwrap();
        assert!(!runtime.join("workers/1/metadata.json").exists());

        FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata).unwrap();
        assert!(runtime.join("workers/1/metadata.json").is_file());
        assert!(target.join("session.json").is_file());
    }

    #[test]
    fn collision_and_corruption_fail_without_overwriting_either_copy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        let (session_id, segment_id) = create_legacy_worker(&sessions, &metadata, 1);
        let source_path = sessions
            .join(session_id.to_string())
            .join(format!("{segment_id}.jsonl"));
        fs::write(&source_path, b"not-json\n").unwrap();
        let corrupt =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(
            corrupt
                .to_string()
                .contains("migrate legacy Worker aggregates")
        );
        assert!(!runtime.join("workers/1/session").exists());
        assert_eq!(fs::read(&source_path).unwrap(), b"not-json\n");

        fs::write(&source_path, b"").unwrap();
        fs::write(runtime.join("workers/1/metadata.json"), b"collision\n").unwrap();
        let collision =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(collision.to_string().contains("collision"));
        assert_eq!(
            fs::read(runtime.join("workers/1/metadata.json")).unwrap(),
            b"collision\n"
        );
        assert!(metadata.join("worker-runtime-1/metadata.json").is_file());
    }

    #[test]
    fn shared_session_reference_fails_closed_and_records_manifest_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1, 2]);
        let (session_id, segment_id) = create_legacy_worker(&sessions, &metadata, 1);
        FsWorkerStore::new(&metadata)
            .unwrap()
            .write(&WorkerMetadata::new(
                "worker-runtime-2",
                Some(WorkerActiveSegmentRef {
                    session_id,
                    segment_id: Some(segment_id),
                }),
            ))
            .unwrap();

        let error =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(error.to_string().contains("ownership is ambiguous"));
        assert!(!runtime.join("workers/1/session").exists());
        assert!(!runtime.join("workers/2/session").exists());
        let manifest: WorkerAggregateMigrationManifest =
            read_json(&runtime.join("migrations/worker-aggregate-v1.json"), "test").unwrap();
        assert_eq!(manifest.workers["1"].state, "shared_session_collision");
        assert_eq!(manifest.workers["2"].state, "shared_session_collision");
        assert!(
            manifest
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "shared_session_reference")
        );
    }

    #[test]
    fn orphan_metadata_sharing_catalog_session_prevents_assignment() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        let (session_id, segment_id) = create_legacy_worker(&sessions, &metadata, 1);
        FsWorkerStore::new(&metadata)
            .unwrap()
            .write(&WorkerMetadata::new(
                "worker-runtime-999",
                Some(WorkerActiveSegmentRef {
                    session_id,
                    segment_id: Some(segment_id),
                }),
            ))
            .unwrap();

        let error =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(error.to_string().contains("ownership is ambiguous"));
        assert!(!runtime.join("workers/1/session").exists());
        assert!(metadata.join("worker-runtime-999/metadata.json").is_file());
        let manifest: WorkerAggregateMigrationManifest =
            read_json(&runtime.join("migrations/worker-aggregate-v1.json"), "test").unwrap();
        assert_eq!(manifest.workers["1"].state, "shared_session_collision");
        assert!(!manifest.workers.contains_key("999"));
        assert!(manifest.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "orphan_worker_metadata"
                && diagnostic.source.contains("worker-runtime-999")
        }));
    }

    #[test]
    fn orphan_sources_are_preserved_and_reported_without_becoming_authority() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        create_legacy_worker(&sessions, &metadata, 1);
        let orphan_session = new_session_id();
        FsStore::new(&sessions)
            .unwrap()
            .create_segment(orphan_session, new_segment_id(), &[])
            .unwrap();
        FsWorkerStore::new(&metadata)
            .unwrap()
            .write(&WorkerMetadata::new("worker-runtime-999", None))
            .unwrap();

        FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata).unwrap();
        let manifest: WorkerAggregateMigrationManifest =
            read_json(&runtime.join("migrations/worker-aggregate-v1.json"), "test").unwrap();
        assert!(manifest.complete);
        assert!(manifest.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "orphan_worker_metadata"
                && diagnostic.source.contains("worker-runtime-999")
        }));
        assert!(manifest.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "orphan_session"
                && diagnostic.source.contains(&orphan_session.to_string())
        }));
        assert!(metadata.join("worker-runtime-999/metadata.json").is_file());
        assert!(sessions.join(orphan_session.to_string()).is_dir());
        assert!(!runtime.join("workers/999").exists());
    }

    #[test]
    fn trace_corruption_and_extra_canonical_history_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        let (session_id, segment_id) = create_legacy_worker(&sessions, &metadata, 1);
        let trace = sessions
            .join(session_id.to_string())
            .join(format!("{segment_id}.trace.jsonl"));
        fs::write(&trace, b"not-json\n").unwrap();
        let corrupt =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(corrupt.to_string().contains("invalid trace JSONL"));
        assert!(!runtime.join("workers/1/session").exists());

        fs::remove_file(trace).unwrap();
        FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata).unwrap();
        let extra = runtime.join("workers/1/session/segments/extra.jsonl");
        fs::write(&extra, b"").unwrap();
        let collision =
            FsRuntimeStore::migrate_legacy_worker_aggregates(&runtime, &sessions, &metadata)
                .unwrap_err();
        assert!(collision.to_string().contains("file set collision"));
        assert!(extra.is_file());
    }

    #[test]
    fn concurrent_startup_serializes_one_idempotent_migration() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let sessions = temp.path().join("legacy-sessions");
        let metadata = temp.path().join("legacy-metadata");
        initialize_runtime(&runtime, &[1]);
        let (session_id, _) = create_legacy_worker(&sessions, &metadata, 1);
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let runtime = runtime.clone();
            let sessions = sessions.clone();
            let metadata = metadata.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                FsRuntimeStore::migrate_legacy_worker_aggregates(runtime, sessions, metadata)
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        let manifest: WorkerAggregateMigrationManifest =
            read_json(&runtime.join("migrations/worker-aggregate-v1.json"), "test").unwrap();
        assert!(manifest.complete);
        assert_eq!(
            manifest.workers["1"].session_id.as_deref(),
            Some(session_id.to_string().as_str())
        );
    }
}
