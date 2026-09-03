use crate::catalog::{
    CreateWorkerRequest, WorkerRestoreIntent, WorkerStatus, WorkingDirectoryStatus,
};
use crate::config_bundle::ConfigBundle;
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::identity::{
    LegacyWorkerIdentityMapping, WorkerId, WorkerRef, legacy_worker_identity_mapping_digest,
};
use crate::management::{RuntimeBackendKind, RuntimeStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 4;
const RUNTIME_FILE: &str = "runtime.json";
const WORKERS_DIR: &str = "workers";
const WORKER_FILE: &str = "worker.json";
const WORKER_METADATA_FILE: &str = "metadata.json";
const LEGACY_OBSERVATIONS_FILE: &str = "observations.jsonl";

static NEXT_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Options for constructing a filesystem-backed Runtime store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStoreOptions {
    /// Root directory containing this Runtime's store data.
    pub root: PathBuf,
    pub runtime_id: String,
    pub display_name: Option<String>,
}

impl FsRuntimeStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_id: "local".to_string(),
            display_name: None,
        }
    }
    pub fn with_runtime_id(mut self, runtime_id: impl Into<String>) -> Self {
        self.runtime_id = runtime_id.into();
        self
    }
}

/// Filesystem persistence boundary for one Worker Runtime state.
///
/// Authority is the Workspace-owned typed Worker identity. Legacy pod paths, socket
/// paths, and session paths are deliberately not part of the layout or lookup API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStore {
    root: PathBuf,
}

impl FsRuntimeStore {
    pub fn migration_plan(
        options: &FsRuntimeStoreOptions,
    ) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
        plan_runtime_store_migration(&options.root, &options.runtime_id).map(|(plan, _)| plan)
    }

    pub fn migrate(
        options: &FsRuntimeStoreOptions,
    ) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
        migrate_runtime_store(&options.root, &options.runtime_id)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.root
    }

    pub(crate) fn open_or_create(
        root: PathBuf,
        runtime_id: &str,
    ) -> Result<OpenedFsRuntimeStore, RuntimeError> {
        let existed = root.exists();
        if existed && !root.is_dir() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "open runtime store",
                path: root,
                message: "runtime path exists but is not a directory".to_string(),
            });
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

        if existed {
            migrate_runtime_store(&root, runtime_id)?;
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
    pub(crate) next_diagnostic_id: u64,
    pub(crate) workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    pub(crate) workspace_owners: BTreeMap<String, String>,
    pub(crate) diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedWorkerExecutionBinding {
    pub(crate) run_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedWorkerExecution {
    pub(crate) binding: Option<PersistedWorkerExecutionBinding>,
    pub(crate) restore_intent: WorkerRestoreIntent,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedWorkerRecord {
    pub(crate) worker_ref: WorkerRef,
    pub(crate) worker_id: WorkerId,
    pub(crate) request: CreateWorkerRequest,
    pub(crate) status: WorkerStatus,
    pub(crate) execution: PersistedWorkerExecution,
    pub(crate) workspace_id: Option<String>,
    pub(crate) working_directory: Option<WorkingDirectoryStatus>,
}

fn runtime_io_error(operation: &'static str, path: &Path, source: std::io::Error) -> RuntimeError {
    RuntimeError::StoreIo {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn runtime_store_corrupt(path: &Path, message: String) -> RuntimeError {
    RuntimeError::StoreCorrupt {
        operation: "migrate Worker identity",
        path: path.to_path_buf(),
        message,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRuntimeStoreMigrationPlan {
    pub current_schema_version: u32,
    pub target_schema_version: u32,
    pub migration_required: bool,
    pub worker_count: usize,
    pub migrated_worker_aggregate_count: usize,
    pub migrated_diagnostic_worker_ref_count: usize,
    pub cleared_diagnostic_worker_ref_count: usize,
    pub mapping_digest: String,
    pub mappings: Vec<LegacyWorkerIdentityMapping>,
    pub excluded_ephemeral_paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct PlannedRuntimeWorkerMigration {
    worker_id: WorkerId,
    source_dir: PathBuf,
    workspace_id: Option<String>,
    legacy_mapping: Option<LegacyWorkerIdentityMapping>,
}

fn plan_runtime_store_migration(
    root: &Path,
    runtime_id: &str,
) -> Result<
    (
        FsRuntimeStoreMigrationPlan,
        Vec<PlannedRuntimeWorkerMigration>,
    ),
    RuntimeError,
> {
    let runtime_path = root.join(RUNTIME_FILE);
    let bytes =
        fs::read(&runtime_path).map_err(|error| runtime_io_error("read", &runtime_path, error))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        runtime_store_corrupt(
            &runtime_path,
            format!("decode Runtime state {}: {error}", runtime_path.display()),
        )
    })?;
    let schema_version = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            runtime_store_corrupt(
                &runtime_path,
                "Runtime state is missing schema_version".to_string(),
            )
        })?;
    let current_schema_version = u32::try_from(schema_version).map_err(|_| {
        runtime_store_corrupt(
            &runtime_path,
            format!("Runtime store schema version {schema_version} is out of range"),
        )
    })?;
    let staging = migration_sibling(root, "schema-v4-staging")?;
    let backup = migration_sibling(root, "pre-schema-v4-backup")?;
    if staging.exists() || backup.exists() {
        return Err(runtime_store_corrupt(
            root,
            format!(
                "unfinished Runtime migration artifact exists (staging={}, backup={})",
                staging.display(),
                backup.display()
            ),
        ));
    }
    if current_schema_version == SCHEMA_VERSION {
        let plan = FsRuntimeStoreMigrationPlan {
            current_schema_version,
            target_schema_version: SCHEMA_VERSION,
            migration_required: false,
            worker_count: 0,
            migrated_worker_aggregate_count: 0,
            migrated_diagnostic_worker_ref_count: 0,
            cleared_diagnostic_worker_ref_count: 0,
            mapping_digest: legacy_worker_identity_mapping_digest(&[]),
            mappings: Vec::new(),
            excluded_ephemeral_paths: Vec::new(),
        };
        return Ok((plan, Vec::new()));
    }
    if current_schema_version != 3 {
        return Err(runtime_store_corrupt(
            &runtime_path,
            format!(
                "unsupported Runtime store schema version {schema_version}; expected 3 or {SCHEMA_VERSION}"
            ),
        ));
    }
    let excluded_ephemeral_paths = runtime_tree_exclusions(root)?;

    let workers_dir = root.join(WORKERS_DIR);
    let mut entries = fs::read_dir(&workers_dir)
        .map_err(|error| runtime_io_error("read workers", &workers_dir, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| runtime_io_error("read workers", &workers_dir, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut planned = Vec::with_capacity(entries.len());
    let mut target_ids = std::collections::BTreeSet::new();
    for entry in entries {
        let source_dir = entry.path();
        if !source_dir.is_dir() {
            return Err(runtime_store_corrupt(
                &source_dir,
                "workers directory contains a non-directory entry".to_string(),
            ));
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            runtime_store_corrupt(&source_dir, "Worker directory is not UTF-8".to_string())
        })?;
        let snapshot_path = source_dir.join(WORKER_FILE);
        let snapshot: serde_json::Value = read_json(&snapshot_path, "read Worker snapshot")?;
        let (worker_id, workspace_id, legacy_mapping) = if current_schema_version == 1 {
            let legacy_worker_id = name.parse::<u64>().map_err(|_| {
                runtime_store_corrupt(
                    &source_dir,
                    format!("legacy Worker directory name must be numeric, found {name}"),
                )
            })?;
            let workspace_id = snapshot
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .filter(|workspace_id| !workspace_id.is_empty())
                .ok_or_else(|| {
                    runtime_store_corrupt(
                        &snapshot_path,
                        "legacy Worker snapshot is missing workspace_id; unscoped Workers require an explicit migration disposition"
                            .to_string(),
                    )
                })?
                .to_string();
            let worker_id =
                WorkerId::from_legacy_binding(&workspace_id, runtime_id, legacy_worker_id);
            let mapping = LegacyWorkerIdentityMapping {
                workspace_id: workspace_id.clone(),
                runtime_id: runtime_id.to_string(),
                legacy_worker_id,
                worker_id,
            };
            (worker_id, Some(workspace_id), Some(mapping))
        } else {
            let worker_id = name.parse::<WorkerId>().map_err(|_| {
                runtime_store_corrupt(
                    &source_dir,
                    format!("pre-v4 Worker directory name must be a UUIDv7, found {name}"),
                )
            })?;
            (worker_id, None, None)
        };
        if !target_ids.insert(worker_id) {
            return Err(runtime_store_corrupt(
                &snapshot_path,
                format!("Worker identity maps to duplicate target {worker_id}"),
            ));
        }
        let target_dir = workers_dir.join(worker_id.to_string());
        if target_dir.exists() && target_dir != source_dir {
            return Err(runtime_store_corrupt(
                &target_dir,
                format!("target Worker directory {worker_id} already exists"),
            ));
        }
        planned.push(PlannedRuntimeWorkerMigration {
            worker_id,
            source_dir,
            workspace_id,
            legacy_mapping,
        });
    }
    let mappings = planned
        .iter()
        .filter_map(|worker| worker.legacy_mapping.clone())
        .collect::<Vec<_>>();
    let mut migrated_worker_aggregate_count = 0;
    for worker in &mut planned {
        let snapshot_path = worker.source_dir.join(WORKER_FILE);
        let snapshot: serde_json::Value = read_json(&snapshot_path, "read Worker snapshot")?;
        let migrated = migrate_worker_document(
            snapshot,
            current_schema_version,
            worker.legacy_mapping.as_ref(),
            &snapshot_path,
        )?;
        let snapshot = validate_migrated_worker_document(&migrated, &snapshot_path)?;
        if snapshot.worker_id != worker.worker_id {
            return Err(runtime_store_corrupt(
                &snapshot_path,
                format!(
                    "Worker snapshot id {} does not match directory identity {}",
                    snapshot.worker_id, worker.worker_id
                ),
            ));
        }
        worker.workspace_id = worker
            .workspace_id
            .clone()
            .or(snapshot.workspace_id)
            .or_else(|| {
                snapshot
                    .request
                    .workspace_api
                    .map(|workspace_api| workspace_api.workspace_id)
            });

        let metadata_path = worker.source_dir.join(WORKER_METADATA_FILE);
        if metadata_path.is_file() {
            let metadata: serde_json::Value =
                read_json(&metadata_path, "read Worker aggregate metadata")?;
            let (_, migrated) =
                migrate_worker_aggregate_document(metadata, worker, runtime_id, &metadata_path)?;
            migrated_worker_aggregate_count += usize::from(migrated);
        }
    }
    let (_, diagnostic_refs) =
        migrate_runtime_document(document, current_schema_version, &mappings, &runtime_path)?;
    let plan = FsRuntimeStoreMigrationPlan {
        current_schema_version,
        target_schema_version: SCHEMA_VERSION,
        migration_required: true,
        worker_count: planned.len(),
        migrated_worker_aggregate_count,
        migrated_diagnostic_worker_ref_count: diagnostic_refs.migrated,
        cleared_diagnostic_worker_ref_count: diagnostic_refs.cleared,
        mapping_digest: legacy_worker_identity_mapping_digest(&mappings),
        mappings,
        excluded_ephemeral_paths,
    };
    Ok((plan, planned))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DiagnosticWorkerRefMigrationCounts {
    migrated: usize,
    cleared: usize,
}

fn migrate_v1_worker_document(
    mut snapshot: serde_json::Value,
    mapping: &LegacyWorkerIdentityMapping,
    snapshot_path: &Path,
) -> Result<serde_json::Value, RuntimeError> {
    let worker_id_text = mapping.worker_id.to_string();
    let snapshot_object = snapshot.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(
            snapshot_path,
            "Worker snapshot must be an object".to_string(),
        )
    })?;
    snapshot_object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(SCHEMA_VERSION),
    );
    snapshot_object.insert(
        "worker_id".to_string(),
        serde_json::Value::String(worker_id_text.clone()),
    );
    snapshot_object
        .get_mut("worker_ref")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            runtime_store_corrupt(
                snapshot_path,
                "Worker snapshot worker_ref must be an object".to_string(),
            )
        })?
        .insert(
            "worker_id".to_string(),
            serde_json::Value::String(worker_id_text.clone()),
        );
    let request = snapshot_object
        .get_mut("request")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            runtime_store_corrupt(
                snapshot_path,
                "Worker snapshot request must be an object".to_string(),
            )
        })?;
    let fingerprint = request
        .remove("idempotency_fingerprint")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| {
            format!(
                "legacy:{}:{}:{}",
                mapping.workspace_id, mapping.runtime_id, mapping.legacy_worker_id
            )
        });
    request.remove("idempotency_key");
    request.insert(
        "worker_id".to_string(),
        serde_json::Value::String(worker_id_text),
    );
    request.insert(
        "create_fingerprint".to_string(),
        serde_json::Value::String(fingerprint),
    );
    Ok(snapshot)
}

fn migrate_worker_document(
    mut document: serde_json::Value,
    source_schema_version: u32,
    mapping: Option<&LegacyWorkerIdentityMapping>,
    snapshot_path: &Path,
) -> Result<serde_json::Value, RuntimeError> {
    if source_schema_version == 1 {
        document = migrate_v1_worker_document(
            document,
            mapping.ok_or_else(|| {
                runtime_store_corrupt(
                    snapshot_path,
                    "schema-v1 Worker migration is missing its identity mapping".to_string(),
                )
            })?,
            snapshot_path,
        )?;
    }
    let object = document.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(
            snapshot_path,
            "Worker snapshot must be an object".to_string(),
        )
    })?;
    let run_generation = object
        .remove("run_generation")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                runtime_store_corrupt(
                    snapshot_path,
                    "Worker snapshot run_generation must be an unsigned integer".to_string(),
                )
            })
        })
        .transpose()?
        .filter(|generation| *generation > 0);
    let legacy_execution = object.remove("execution");
    if !object.contains_key("working_directory") {
        if let Some(working_directory) = legacy_execution
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|execution| execution.get("working_directory"))
            .cloned()
        {
            object.insert("working_directory".to_string(), working_directory);
        }
    }
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(SCHEMA_VERSION),
    );
    object.insert(
        "status".to_string(),
        serde_json::Value::String("stopped".to_string()),
    );
    object.insert(
        "execution".to_string(),
        serde_json::json!({
            "binding": run_generation.map(|run_generation| {
                serde_json::json!({ "run_generation": run_generation })
            }),
            "restore_intent": "explicit",
        }),
    );
    Ok(document)
}

fn validate_migrated_worker_document(
    document: &serde_json::Value,
    snapshot_path: &Path,
) -> Result<WorkerSnapshot, RuntimeError> {
    let snapshot: WorkerSnapshot = serde_json::from_value(document.clone()).map_err(|error| {
        runtime_store_corrupt(
            snapshot_path,
            format!("decode migrated Worker snapshot: {error}"),
        )
    })?;
    snapshot.validate(snapshot_path)?;
    Ok(snapshot)
}

fn runtime_worker_name(worker_id: WorkerId) -> String {
    format!("worker-runtime-{worker_id}")
}

fn migrate_worker_aggregate_document(
    mut document: serde_json::Value,
    worker: &PlannedRuntimeWorkerMigration,
    runtime_id: &str,
    metadata_path: &Path,
) -> Result<(serde_json::Value, bool), RuntimeError> {
    let expected_name = runtime_worker_name(worker.worker_id);
    let metadata = document.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(
            metadata_path,
            "Worker aggregate metadata must be an object".to_string(),
        )
    })?;
    let actual_name = metadata
        .get("worker_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            runtime_store_corrupt(
                metadata_path,
                "Worker aggregate metadata is missing worker_name".to_string(),
            )
        })?
        .to_string();
    let migrated = actual_name != expected_name;
    if migrated {
        let legacy_worker_id = actual_name
            .strip_prefix("worker-runtime-")
            .and_then(|worker_id| worker_id.parse::<u64>().ok())
            .ok_or_else(|| {
                runtime_store_corrupt(
                    metadata_path,
                    format!(
                        "Worker aggregate identity {actual_name} is neither the expected UUID identity nor a legacy numeric identity"
                    ),
                )
            })?;
        let workspace_id = worker.workspace_id.as_deref().ok_or_else(|| {
            runtime_store_corrupt(
                metadata_path,
                "legacy Worker aggregate identity has no Workspace binding".to_string(),
            )
        })?;
        let mapped = WorkerId::from_legacy_binding(workspace_id, runtime_id, legacy_worker_id);
        if mapped != worker.worker_id {
            return Err(runtime_store_corrupt(
                metadata_path,
                format!(
                    "legacy Worker aggregate identity {actual_name} maps to {mapped}, expected {}",
                    worker.worker_id
                ),
            ));
        }
    }

    if let Some(snapshot) = metadata
        .get_mut("resolved_manifest_snapshot")
        .filter(|snapshot| !snapshot.is_null())
    {
        let manifest: manifest::WorkerManifest =
            serde_json::from_value(snapshot.clone()).map_err(|error| {
                runtime_store_corrupt(
                    metadata_path,
                    format!("decode Worker aggregate resolved manifest snapshot: {error}"),
                )
            })?;
        if manifest.worker.name != actual_name {
            return Err(runtime_store_corrupt(
                metadata_path,
                format!(
                    "Worker aggregate manifest identity {} does not match metadata identity {actual_name}",
                    manifest.worker.name
                ),
            ));
        }
        snapshot
            .as_object_mut()
            .and_then(|manifest| manifest.get_mut("worker"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                runtime_store_corrupt(
                    metadata_path,
                    "Worker aggregate resolved manifest is missing worker metadata".to_string(),
                )
            })?
            .insert(
                "name".to_string(),
                serde_json::Value::String(expected_name.clone()),
            );
    }
    metadata.insert(
        "worker_name".to_string(),
        serde_json::Value::String(expected_name.clone()),
    );

    let metadata: session_store::WorkerMetadata = serde_json::from_value(document.clone())
        .map_err(|error| {
            runtime_store_corrupt(
                metadata_path,
                format!("decode migrated Worker aggregate metadata: {error}"),
            )
        })?;
    if metadata.worker_name != expected_name {
        return Err(runtime_store_corrupt(
            metadata_path,
            "migrated Worker aggregate identity does not match its Worker UUID".to_string(),
        ));
    }
    if let Some(snapshot) = metadata.resolved_manifest_snapshot {
        let manifest: manifest::WorkerManifest =
            serde_json::from_value(snapshot).map_err(|error| {
                runtime_store_corrupt(
                    metadata_path,
                    format!("decode migrated Worker aggregate resolved manifest: {error}"),
                )
            })?;
        if manifest.worker.name != expected_name {
            return Err(runtime_store_corrupt(
                metadata_path,
                "migrated Worker aggregate manifest identity does not match its Worker UUID"
                    .to_string(),
            ));
        }
    }
    Ok((document, migrated))
}

fn migrate_runtime_document(
    mut document: serde_json::Value,
    source_schema_version: u32,
    mappings: &[LegacyWorkerIdentityMapping],
    runtime_path: &Path,
) -> Result<(serde_json::Value, DiagnosticWorkerRefMigrationCounts), RuntimeError> {
    let mapped_worker_ids = mappings
        .iter()
        .map(|mapping| (mapping.legacy_worker_id, mapping.worker_id))
        .collect::<BTreeMap<_, _>>();
    let mut counts = DiagnosticWorkerRefMigrationCounts::default();
    if source_schema_version == 1
        && let Some(diagnostics) = document.get_mut("diagnostics")
    {
        let diagnostics = diagnostics.as_array_mut().ok_or_else(|| {
            runtime_store_corrupt(
                runtime_path,
                "Runtime snapshot diagnostics must be an array".to_string(),
            )
        })?;
        for (index, diagnostic) in diagnostics.iter_mut().enumerate() {
            let diagnostic = diagnostic.as_object_mut().ok_or_else(|| {
                runtime_store_corrupt(
                    runtime_path,
                    format!("Runtime diagnostic {index} must be an object"),
                )
            })?;
            let Some(worker_ref) = diagnostic.get("worker_ref") else {
                continue;
            };
            if worker_ref.is_null() {
                continue;
            }
            let legacy_worker_id = worker_ref
                .as_object()
                .and_then(|worker_ref| worker_ref.get("worker_id"))
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    runtime_store_corrupt(
                        runtime_path,
                        format!(
                            "Runtime diagnostic {index} worker_ref.worker_id must be an unsigned legacy Worker id"
                        ),
                    )
                })?;
            if let Some(worker_id) = mapped_worker_ids.get(&legacy_worker_id) {
                diagnostic
                    .get_mut("worker_ref")
                    .and_then(serde_json::Value::as_object_mut)
                    .expect("validated diagnostic Worker reference")
                    .insert(
                        "worker_id".to_string(),
                        serde_json::Value::String(worker_id.to_string()),
                    );
                counts.migrated += 1;
            } else {
                // The diagnostic remains useful historical evidence, but a deleted
                // legacy Worker has no Workspace binding from which a stable UUID
                // can be reconstructed.
                diagnostic.remove("worker_ref");
                counts.cleared += 1;
            }
        }
    }

    let object = document.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(
            runtime_path,
            "Runtime snapshot must be an object".to_string(),
        )
    })?;
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(SCHEMA_VERSION),
    );
    object.remove("workers");
    object.remove("next_worker_sequence");
    let snapshot: RuntimeSnapshot = serde_json::from_value(document.clone()).map_err(|error| {
        runtime_store_corrupt(
            runtime_path,
            format!("decode migrated Runtime snapshot: {error}"),
        )
    })?;
    snapshot.validate(runtime_path)?;
    Ok((document, counts))
}

fn migration_sibling(root: &Path, suffix: &str) -> Result<PathBuf, RuntimeError> {
    let parent = root.parent().ok_or_else(|| {
        runtime_store_corrupt(
            root,
            "Runtime store root has no parent directory".to_string(),
        )
    })?;
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            runtime_store_corrupt(root, "Runtime store root name is not UTF-8".to_string())
        })?;
    Ok(parent.join(format!(".{name}.{suffix}")))
}

fn runtime_ephemeral_socket(root: &Path, path: &Path) -> Result<bool, RuntimeError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        runtime_store_corrupt(
            path,
            format!("Runtime migration path escaped root {}", root.display()),
        )
    })?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let known_path = components.len() == 5
        && components[0] == WORKERS_DIR
        && (components[1].parse::<u64>().is_ok() || WorkerId::parse(&components[1]).is_some())
        && components[2] == "runs"
        && components[3].parse::<u64>().is_ok()
        && components[4] == "worker.sock";
    if !known_path {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => Err(runtime_store_corrupt(
                path,
                "Runtime migration found an active Worker socket; stop the legacy Runtime and Worker before migrating"
                    .to_string(),
            )),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) => Ok(true),
            Err(error) => Err(runtime_store_corrupt(
                path,
                format!("Runtime migration could not verify Worker socket liveness: {error}"),
            )),
        }
    }
    #[cfg(not(unix))]
    Ok(false)
}

fn collect_runtime_tree_exclusions(
    root: &Path,
    source: &Path,
    excluded: &mut Vec<String>,
) -> Result<(), RuntimeError> {
    let entries = fs::read_dir(source)
        .map_err(|error| runtime_io_error("read migration source", source, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| runtime_io_error("read migration source", source, error))?;
        let source_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| runtime_io_error("inspect migration source", &source_path, error))?;
        if file_type.is_dir() {
            collect_runtime_tree_exclusions(root, &source_path, excluded)?;
        } else if file_type.is_file() {
        } else if runtime_ephemeral_socket(root, &source_path)? {
            excluded.push(
                source_path
                    .strip_prefix(root)
                    .expect("validated Runtime migration path")
                    .to_string_lossy()
                    .into_owned(),
            );
        } else {
            return Err(runtime_store_corrupt(
                &source_path,
                "Runtime migration refuses unknown symlinks and special files".to_string(),
            ));
        }
    }
    Ok(())
}

fn runtime_tree_exclusions(root: &Path) -> Result<Vec<String>, RuntimeError> {
    let mut excluded = Vec::new();
    collect_runtime_tree_exclusions(root, root, &mut excluded)?;
    excluded.sort();
    Ok(excluded)
}

fn copy_runtime_tree(root: &Path, source: &Path, target: &Path) -> Result<(), RuntimeError> {
    fs::create_dir(target)
        .map_err(|error| runtime_io_error("create migration staging", target, error))?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| runtime_io_error("read migration source", source, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| runtime_io_error("read migration source", source, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| runtime_io_error("inspect migration source", &source_path, error))?;
        if file_type.is_dir() {
            copy_runtime_tree(root, &source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path)
                .map_err(|error| runtime_io_error("copy migration source", &source_path, error))?;
        } else if runtime_ephemeral_socket(root, &source_path)? {
            continue;
        } else {
            return Err(runtime_store_corrupt(
                &source_path,
                "Runtime migration refuses unknown symlinks and special files".to_string(),
            ));
        }
    }
    Ok(())
}

fn migrate_runtime_store(
    root: &Path,
    runtime_id: &str,
) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
    let (plan, _) = plan_runtime_store_migration(root, runtime_id)?;
    if !plan.migration_required {
        return Ok(plan);
    }
    let staging = migration_sibling(root, "schema-v4-staging")?;
    let backup = migration_sibling(root, "pre-schema-v4-backup")?;
    if staging.exists() || backup.exists() {
        return Err(runtime_store_corrupt(
            root,
            format!(
                "unfinished Runtime migration artifact exists (staging={}, backup={}); recover or remove it before retrying",
                staging.display(),
                backup.display()
            ),
        ));
    }
    if let Err(error) = copy_runtime_tree(root, root, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let staged_plan = match migrate_runtime_store_in_place(&staging, runtime_id) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let staged_store = FsRuntimeStore {
        root: staging.clone(),
    };
    if let Err(error) = staged_store.load_runtime_state() {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::rename(root, &backup)
        .map_err(|error| runtime_io_error("backup runtime store", root, error))?;
    if let Err(error) = fs::rename(&staging, root) {
        let rollback = fs::rename(&backup, root);
        return match rollback {
            Ok(()) => Err(runtime_io_error(
                "activate migrated runtime store",
                &staging,
                error,
            )),
            Err(rollback_error) => Err(runtime_store_corrupt(
                root,
                format!(
                    "activate migrated Runtime store failed: {error}; rollback failed: {rollback_error}; backup remains at {}",
                    backup.display()
                ),
            )),
        };
    }
    fs::remove_dir_all(&backup)
        .map_err(|error| runtime_io_error("remove runtime migration backup", &backup, error))?;
    debug_assert_eq!(plan.mapping_digest, staged_plan.mapping_digest);
    Ok(plan)
}

fn migrate_runtime_store_in_place(
    root: &Path,
    runtime_id: &str,
) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
    let (plan, planned_workers) = plan_runtime_store_migration(root, runtime_id)?;
    if !plan.migration_required {
        return Ok(plan);
    }
    let runtime_path = root.join(RUNTIME_FILE);
    let bytes =
        fs::read(&runtime_path).map_err(|error| runtime_io_error("read", &runtime_path, error))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        runtime_store_corrupt(
            &runtime_path,
            format!("decode Runtime state {}: {error}", runtime_path.display()),
        )
    })?;

    for planned_worker in &planned_workers {
        let source_dir = &planned_worker.source_dir;
        let source_snapshot_path = source_dir.join(WORKER_FILE);
        let bytes = fs::read(&source_snapshot_path)
            .map_err(|error| runtime_io_error("read", &source_snapshot_path, error))?;
        let snapshot: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            runtime_store_corrupt(
                &source_snapshot_path,
                format!(
                    "decode Worker snapshot {}: {error}",
                    source_snapshot_path.display()
                ),
            )
        })?;
        let snapshot = migrate_worker_document(
            snapshot,
            plan.current_schema_version,
            planned_worker.legacy_mapping.as_ref(),
            &source_snapshot_path,
        )?;
        let metadata_path = source_dir.join(WORKER_METADATA_FILE);
        let metadata = if metadata_path.is_file() {
            let metadata: serde_json::Value =
                read_json(&metadata_path, "read Worker aggregate metadata")?;
            Some(
                migrate_worker_aggregate_document(
                    metadata,
                    planned_worker,
                    runtime_id,
                    &metadata_path,
                )?
                .0,
            )
        } else {
            None
        };

        let worker_id_text = planned_worker.worker_id.to_string();
        let migrated_dir = root.join("workers").join(&worker_id_text);
        if source_dir != &migrated_dir {
            fs::rename(source_dir, &migrated_dir)
                .map_err(|error| runtime_io_error("rename", source_dir, error))?;
        }
        let migrated_snapshot_path = migrated_dir.join(WORKER_FILE);
        atomic_write_json(
            &migrated_snapshot_path,
            &snapshot,
            "migrate Worker identity",
        )?;
        if let Some(metadata) = metadata {
            atomic_write_json(
                &migrated_dir.join(WORKER_METADATA_FILE),
                &metadata,
                "migrate Worker aggregate identity",
            )?;
        }
    }

    let (document, _) = migrate_runtime_document(
        document,
        plan.current_schema_version,
        &plan.mappings,
        &runtime_path,
    )?;
    atomic_write_json(
        &runtime_path,
        &document,
        "migrate Runtime Worker identities",
    )?;
    Ok(plan)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeSnapshot {
    schema_version: u32,
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    status: RuntimeStatus,
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
            next_diagnostic_id: state.next_diagnostic_id,
            config_bundles: BTreeMap::new(),
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
            next_diagnostic_id: self.next_diagnostic_id,
            workers,
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
    status: WorkerStatus,
    execution: PersistedWorkerExecution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_directory: Option<WorkingDirectoryStatus>,
}

impl WorkerSnapshot {
    fn from_persisted(worker: &PersistedWorkerRecord) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            worker_ref: worker.worker_ref.clone(),
            worker_id: worker.worker_id.clone(),
            request: worker.request.clone(),
            status: worker.status,
            execution: worker.execution.clone(),
            workspace_id: worker.workspace_id.clone(),
            working_directory: worker.working_directory.clone(),
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
        match (self.status, self.execution.restore_intent) {
            (status, WorkerRestoreIntent::Automatic) if status.is_active() => {
                let Some(binding) = self.execution.binding.as_ref() else {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read worker snapshot",
                        path: path.to_path_buf(),
                        message: "automatic restore intent requires an execution binding"
                            .to_string(),
                    });
                };
                if binding.run_generation == 0 {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read worker snapshot",
                        path: path.to_path_buf(),
                        message: "execution binding run_generation must be greater than zero"
                            .to_string(),
                    });
                }
            }
            (WorkerStatus::Stopped, WorkerRestoreIntent::Explicit) => {
                if self
                    .execution
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.run_generation == 0)
                {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read worker snapshot",
                        path: path.to_path_buf(),
                        message: "execution binding run_generation must be greater than zero"
                            .to_string(),
                    });
                }
            }
            _ => {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read worker snapshot",
                    path: path.to_path_buf(),
                    message: format!(
                        "worker status {:?} conflicts with restore intent {:?}",
                        self.status, self.execution.restore_intent
                    ),
                });
            }
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
            status: self.status,
            execution: self.execution,
            workspace_id,
            working_directory: self.working_directory,
        }
    }
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
