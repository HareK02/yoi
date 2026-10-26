//! Filesystem persistence for the Runtime catalog and Worker aggregates.
//!
//! Atomically publishes settled Worker status, execution binding, and pending lifecycle operations
//! as one Worker record. Session content and metadata projections use separate stores;
//! sockets and run paths do not substitute for durable identity.
//!
//! Legacy conversion copies the Runtime tree into staging under the OS owner lock, validates
//! Worker record/metadata/Manifest identity consistency, then syncs and switches trees.
//! Partially converted trees are not opened as normal authority. Duplicate identities, existing
//! targets, missing Workspace bindings, and metadata identity mismatches are rejected, not guessed away.
//! After interruption, staging/backup and validation determine which tree to publish;
//! writes do not continue into an incomplete tree.
//!
//! Directories without Worker records are archived, not adopted into the normal catalog.
//! Archive target collisions are rejected; data of unknown ownership is never overwritten or deleted.
//! Symlinks/special files are rejected except recognizable stopped sockets. Preserving historical
//! data and deciding whether execution hints can migrate are separate judgments.

use crate::catalog::{
    ConfigBundleRef, CreateWorkerRequest, LogicalWorkdirAttachment, ProfileSelector,
    WorkerRestoreIntent, WorkerStatus, WorkingDirectoryAttachmentClaim,
    WorkingDirectoryAttachmentRequest, WorkingDirectoryAttachmentStatus, WorkingDirectoryRequest,
};
use crate::config_bundle::ConfigBundle;
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::execution::WorkerLifecycleOperationId;
use crate::identity::{
    LegacyWorkerIdentityMapping, WorkerId, WorkerRef, legacy_worker_identity_mapping_digest,
};
use crate::management::{RuntimeBackendKind, RuntimeStatus};
use crate::profile_archive::ProfileSourceArchiveRef;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

#[cfg(feature = "test-support")]
#[path = "test_support.rs"]
pub mod test_support;

pub(crate) const SCHEMA_VERSION: u32 = 14;
const RESTORE_DECISION_SCHEMA_VERSION: u32 = 13;
const RESTORE_ADMISSION_SCHEMA_VERSION: u32 = 12;
// Frozen source schemas: do not move their meaning when adding a migration.
const SOURCE_SCHEMA_VERSION: u32 = 11;
const EXECUTION_SCHEMA_VERSION: u32 = 10;
const AGGREGATE_SCHEMA_VERSION: u32 = 9;
const PREVIOUS_SCHEMA_VERSION: u32 = 8;
const LEGACY_SCHEMA_VERSION: u32 = 7;
const OLDEST_SCHEMA_VERSION: u32 = 6;
const RUNTIME_FILE: &str = "runtime.json";
const WORKERS_DIR: &str = "workers";
const ORPHANED_WORKERS_DIR: &str = "orphaned-workers";
const WORKER_FILE: &str = "worker.json";
const WORKER_EXECUTION_FILE: &str = "execution.json";
const WORKER_METADATA_FILE: &str = "metadata.json";
const LEGACY_OBSERVATIONS_FILE: &str = "observations.jsonl";
const MAX_WORKER_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PERSISTED_WORKER_RECORDS: usize = 4096;

static NEXT_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum AtomicWriteFault {
    None = 0,
    BeforeRename = 1,
    AfterRename = 2,
}

impl AtomicWriteFault {
    fn take(slot: &AtomicU8) -> Self {
        match slot.swap(Self::None as u8, Ordering::AcqRel) {
            value if value == Self::BeforeRename as u8 => Self::BeforeRename,
            value if value == Self::AfterRename as u8 => Self::AfterRename,
            _ => Self::None,
        }
    }
}

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

#[derive(Debug)]
struct RuntimeStoreOwnerLock {
    file: File,
}

impl Drop for RuntimeStoreOwnerLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Filesystem persistence boundary for one Worker Runtime state.
///
/// Authority is the Workspace-owned typed Worker identity. Legacy pod paths, socket
/// paths, and session paths are deliberately not part of the layout or lookup API.
#[derive(Clone, Debug)]
pub struct FsRuntimeStore {
    root: PathBuf,
    _owner_lock: Option<Arc<RuntimeStoreOwnerLock>>,
    worker_write_fault: Arc<AtomicU8>,
    #[cfg(test)]
    worker_reconcile_sync_fault: Arc<std::sync::atomic::AtomicBool>,
}

impl PartialEq for FsRuntimeStore {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
    }
}

impl Eq for FsRuntimeStore {}

impl FsRuntimeStore {
    pub fn migration_plan(
        options: &FsRuntimeStoreOptions,
    ) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
        let _owner_lock = acquire_runtime_store_owner_lock(&options.root)?;
        plan_runtime_store_migration(&options.root, &options.runtime_id).map(|(plan, _)| plan)
    }

    pub fn migrate(
        options: &FsRuntimeStoreOptions,
    ) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
        let _owner_lock = acquire_runtime_store_owner_lock(&options.root)?;
        recover_runtime_store_migration(&options.root)?;
        migrate_runtime_store(&options.root, &options.runtime_id)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn fail_next_worker_write(&self, fault: AtomicWriteFault) {
        self.worker_write_fault
            .store(fault as u8, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_worker_reconcile_sync(&self) {
        self.worker_reconcile_sync_fault
            .store(true, Ordering::Release);
    }

    pub(crate) fn open_or_create(
        root: PathBuf,
        runtime_id: &str,
    ) -> Result<OpenedFsRuntimeStore, RuntimeError> {
        let parent = runtime_store_parent(&root)?;
        fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
            operation: "create runtime store parent",
            path: parent.to_path_buf(),
            source,
        })?;
        let owner_lock = acquire_runtime_store_owner_lock(&root)?;
        recover_runtime_store_migration(&root)?;
        let root_existed = root.exists();
        if root_existed && !root.is_dir() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "open runtime store",
                path: root,
                message: "runtime path exists but is not a directory".to_string(),
            });
        }

        if !root_existed {
            fs::create_dir_all(&root).map_err(|source| RuntimeError::StoreIo {
                operation: "create runtime store root",
                path: root.clone(),
                source,
            })?;
        }
        let snapshot_exists = root.join(RUNTIME_FILE).is_file();
        if root_existed && !snapshot_exists {
            validate_fresh_runtime_store_layout(&root)?;
        }

        let workers_dir = root.join(WORKERS_DIR);
        fs::create_dir_all(&workers_dir).map_err(|source| RuntimeError::StoreIo {
            operation: "create runtime store",
            path: workers_dir,
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

        if snapshot_exists {
            migrate_runtime_store(&root, runtime_id)?;
        }
        let store = Self {
            root,
            _owner_lock: Some(owner_lock),
            worker_write_fault: Arc::new(AtomicU8::new(AtomicWriteFault::None as u8)),
            #[cfg(test)]
            worker_reconcile_sync_fault: Default::default(),
        };
        let state = if snapshot_exists {
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

    pub(crate) fn write_worker_record(
        &self,
        worker: &PersistedWorkerRecord,
    ) -> Result<(), RuntimeError> {
        self.ensure_worker_ref(&worker.worker_ref)?;
        let worker_dir = self.worker_dir(&worker.worker_id);
        fs::create_dir_all(&worker_dir).map_err(|source| RuntimeError::StoreIo {
            operation: "create Worker store",
            path: worker_dir.clone(),
            source,
        })?;
        atomic_write_json_with_fault(
            &worker_dir.join(WORKER_FILE),
            &WorkerAggregateRecord::from_persisted(worker),
            "write Worker aggregate",
            AtomicWriteFault::take(&self.worker_write_fault),
        )?;
        remove_legacy_observations(&worker_dir);
        Ok(())
    }

    /// Read one canonical aggregate without migration, cleanup, or tolerant
    /// startup enumeration. Reconciliation must not interpret an unreadable
    /// record as absent or replace it with an older in-memory projection.
    #[cfg(test)]
    pub(crate) fn read_worker_record(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<PersistedWorkerRecord, RuntimeError> {
        self.read_worker_record_file(worker_ref)
            .map(|(record, _)| record)
    }

    /// Reobserve and sync canonical authority after an uncertain aggregate write.
    ///
    /// The caller must hold the Worker operation lock; this store retains the
    /// Runtime owner lock. Sync the same file that was validated, then its directory
    /// and the directories owning newly created Worker paths. Read equality alone
    /// cannot discharge a failed post-rename sync. Any error leaves the caller's
    /// write outcome unresolved; this never writes an older memory projection.
    /// A returned record proves neither a matching operation nor live activation:
    /// the lifecycle owner must still compare request/operation, Stop and receipt.
    pub(crate) fn read_durable_worker_record(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<PersistedWorkerRecord, RuntimeError> {
        self.read_durable_worker_record_with_sync(worker_ref, |file| {
            #[cfg(test)]
            if self
                .worker_reconcile_sync_fault
                .swap(false, Ordering::AcqRel)
            {
                return Err(std::io::Error::other(
                    "injected reconciliation sync failure",
                ));
            }
            file.sync_all()
        })
    }

    /// Compare the entire typed aggregate, including request owner, receipt,
    /// attachments and Stop, after durability is confirmed under owner locks.
    pub(crate) fn reconcile_worker_record(
        &self,
        expected: &PersistedWorkerRecord,
    ) -> Result<(PersistedWorkerRecord, bool), RuntimeError> {
        let observed = self.read_durable_worker_record(&expected.worker_ref)?;
        let document = |record: &PersistedWorkerRecord| {
            serde_json::to_value(WorkerAggregateRecord::from_persisted(record))
                .map_err(|error| RuntimeError::InvalidRequest(error.to_string()))
        };
        let matches = document(&observed)? == document(expected)?;
        Ok((observed, matches))
    }

    fn read_durable_worker_record_with_sync(
        &self,
        worker_ref: &WorkerRef,
        mut sync: impl FnMut(&File) -> std::io::Result<()>,
    ) -> Result<PersistedWorkerRecord, RuntimeError> {
        if self._owner_lock.is_none() {
            return Err(RuntimeError::InvalidRequest(
                "Worker aggregate reconciliation requires the Runtime store owner lock".to_string(),
            ));
        }
        let (record, file) = self.read_worker_record_file(worker_ref)?;
        let worker_dir = self.worker_dir(&worker_ref.worker_id);
        let path = worker_dir.join(WORKER_FILE);
        let unknown = |source| RuntimeError::StoreCommitOutcomeUnknown {
            operation: "sync reconciled Worker aggregate",
            path: path.clone(),
            source,
        };
        sync(&file).map_err(&unknown)?;
        for directory in [worker_dir, self.root.join(WORKERS_DIR), self.root.clone()] {
            let file = File::open(directory).map_err(&unknown)?;
            sync(&file).map_err(&unknown)?;
        }
        Ok(record)
    }

    fn read_worker_record_file(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<(PersistedWorkerRecord, File), RuntimeError> {
        let worker_dir = self.worker_dir(&worker_ref.worker_id);
        for directory in [&self.root, &self.root.join(WORKERS_DIR), &worker_dir] {
            let metadata = fs::symlink_metadata(directory).map_err(|source| {
                if source.kind() == std::io::ErrorKind::NotFound {
                    RuntimeError::StoreMissing {
                        operation: "reconcile Worker aggregate",
                        path: directory.to_path_buf(),
                    }
                } else {
                    runtime_io_error("reconcile Worker aggregate", directory, source)
                }
            })?;
            if !metadata.file_type().is_dir() {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "reconcile Worker aggregate",
                    path: directory.to_path_buf(),
                    message: "Worker aggregate directory is not a regular directory".to_string(),
                });
            }
        }
        let path = worker_dir.join(WORKER_FILE);
        let (aggregate, file): (WorkerAggregateRecord, File) =
            read_bounded_json_file(&path, "reconcile Worker aggregate")?;
        let record = aggregate.validate(&path)?;
        if record.worker_ref != *worker_ref || record.worker_id != worker_ref.worker_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "reconcile Worker aggregate",
                path,
                message: "Worker aggregate does not match the requested identity".to_string(),
            });
        }
        Ok((record, file))
    }

    pub(crate) fn delete_worker_record(&self, worker_id: &WorkerId) -> Result<(), RuntimeError> {
        self.delete_worker_record_with_intent(
            worker_id,
            session_store::retention::SessionDeletionIntent::Ordinary,
        )
    }

    pub(crate) fn delete_worker_record_with_intent(
        &self,
        worker_id: &WorkerId,
        intent: session_store::retention::SessionDeletionIntent,
    ) -> Result<(), RuntimeError> {
        let worker_dir = self.worker_dir(worker_id);
        if !worker_dir.exists() {
            return Ok(());
        }
        crate::retention::delete_worker_aggregate(&self.root, &worker_dir, intent)
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

        if worker_dirs.len() > MAX_PERSISTED_WORKER_RECORDS {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker identities",
                path: workers_dir,
                message: format!(
                    "Worker record count {} exceeds limit {MAX_PERSISTED_WORKER_RECORDS}",
                    worker_dirs.len()
                ),
            });
        }

        for entry in worker_dirs {
            let path = entry.path();
            let is_directory = entry
                .file_type()
                .map(|file_type| file_type.is_dir() && !file_type.is_symlink())
                .unwrap_or(false);
            if !is_directory {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    None,
                    "ignored invalid Worker store entry while loading Runtime store",
                );
                continue;
            }
            let aggregate_path = path.join(WORKER_FILE);
            let aggregate: WorkerAggregateRecord =
                match read_bounded_json(&aggregate_path, "read Worker aggregate") {
                    Ok(aggregate) => aggregate,
                    Err(_error) => {
                        record_worker_load_diagnostic(
                            &mut snapshot,
                            None,
                            "ignored corrupt Worker aggregate while loading Runtime store",
                        );
                        continue;
                    }
                };
            let worker_ref = aggregate.worker_ref.clone();
            let worker = match aggregate.validate(&aggregate_path) {
                Ok(worker) => worker,
                Err(_error) => {
                    record_worker_load_diagnostic(
                        &mut snapshot,
                        Some(worker_ref),
                        "ignored invalid Worker aggregate while loading Runtime store",
                    );
                    continue;
                }
            };
            remove_legacy_observations(&path);
            if workers.insert(worker.worker_id.clone(), worker).is_some() {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    None,
                    "ignored duplicate Worker identity while loading Runtime store",
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

fn validate_fresh_runtime_store_layout(root: &Path) -> Result<(), RuntimeError> {
    let entries = fs::read_dir(root).map_err(|source| RuntimeError::StoreIo {
        operation: "inspect uninitialized runtime store",
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| RuntimeError::StoreIo {
            operation: "inspect uninitialized runtime store",
            path: root.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| RuntimeError::StoreIo {
            operation: "inspect uninitialized runtime store entry",
            path: entry.path(),
            source,
        })?;
        if entry.file_name() == "auth.toml" && file_type.is_file() {
            continue;
        }
        return Err(RuntimeError::StoreCorrupt {
            operation: "open runtime store",
            path: root.to_path_buf(),
            message: format!(
                "unexpected persistence `{}` exists without a Runtime snapshot",
                entry.file_name().to_string_lossy()
            ),
        });
    }
    Ok(())
}

fn acquire_runtime_store_owner_lock(
    root: &Path,
) -> Result<Arc<RuntimeStoreOwnerLock>, RuntimeError> {
    let root_parent = runtime_store_parent(root)?;
    let parent = fs::canonicalize(root_parent).map_err(|source| RuntimeError::StoreIo {
        operation: "resolve runtime store owner lock parent",
        path: root_parent.to_path_buf(),
        source,
    })?;
    let name = root.file_name().ok_or_else(|| RuntimeError::StoreCorrupt {
        operation: "resolve runtime store owner lock",
        path: root.to_path_buf(),
        message: "runtime store root has no file name".to_string(),
    })?;
    let mut lock_name = std::ffi::OsString::from(".");
    lock_name.push(name);
    lock_name.push(".runtime-owner.lock");
    let lock_path = parent.join(lock_name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| RuntimeError::StoreIo {
            operation: "open runtime store owner lock",
            path: root.to_path_buf(),
            source,
        })?;
    match file.try_lock_exclusive() {
        Ok(true) => Ok(Arc::new(RuntimeStoreOwnerLock { file })),
        Ok(false) => Err(RuntimeError::RuntimeStoreAlreadyOpen {
            path: root.to_path_buf(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
            Err(RuntimeError::RuntimeStoreAlreadyOpen {
                path: root.to_path_buf(),
            })
        }
        Err(source) => Err(RuntimeError::StoreIo {
            operation: "acquire runtime store owner lock",
            path: root.to_path_buf(),
            source,
        }),
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
    pub(crate) subject_create_recovery_fences:
        BTreeMap<WorkerId, crate::runtime::SubjectCreateRecoveryFence>,
    pub(crate) subject_releases: Vec<(
        runtime_api::SubjectReleaseRequest,
        runtime_api::SubjectReleaseState,
    )>,
    pub(crate) display_name: Option<String>,
    pub(crate) status: RuntimeStatus,
    pub(crate) next_diagnostic_id: u64,
    pub(crate) workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    pub(crate) workspace_owners: BTreeMap<String, String>,
    pub(crate) diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWorkerExecutionBinding {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWorkerExecution {
    pub(crate) request: CreateWorkerRequest,
    pub(crate) binding: Option<PersistedWorkerExecutionBinding>,
    pub(crate) restore_intent: WorkerRestoreIntent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistedWorkerRestoreMode {
    Automatic,
    Explicit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWorkerRestoreOperation {
    pub(crate) operation_id: WorkerLifecycleOperationId,
    pub(crate) mode: PersistedWorkerRestoreMode,
    pub(crate) commit: crate::runtime::RestoreCommitFact,
    pub(crate) source_repair_possible: bool,
    pub(crate) request: CreateWorkerRequest,
    pub(crate) binding: Option<PersistedWorkerExecutionBinding>,
    pub(crate) last_settled_status: WorkerStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWorkerStopOperation {
    pub(crate) operation_id: WorkerLifecycleOperationId,
    /// Restore resources remain operation-owned until stop confirms cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_restore: Option<PersistedWorkerRestoreOperation>,
    /// Stop can reconcile an identity whose execution metadata is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request: Option<CreateWorkerRequest>,
    pub(crate) binding: Option<PersistedWorkerExecutionBinding>,
    pub(crate) restore_intent: WorkerRestoreIntent,
    pub(crate) last_settled_status: WorkerStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "intent", rename_all = "snake_case")]
pub(crate) enum PersistedWorkerLifecycleOperation {
    Restore(PersistedWorkerRestoreOperation),
    Stop(PersistedWorkerStopOperation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PersistedWorkerExecutionState {
    Available(PersistedWorkerExecution),
    ReconciliationRequired(PersistedWorkerLifecycleOperation),
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedWorkerRecord {
    pub(crate) restore_guard: crate::runtime::RestoreGuard,
    pub(crate) worker_ref: WorkerRef,
    pub(crate) worker_id: WorkerId,
    pub(crate) profile: ProfileSelector,
    pub(crate) display_name: Option<String>,
    pub(crate) profile_source: ProfileSourceArchiveRef,
    pub(crate) config_bundle: Option<ConfigBundleRef>,
    pub(crate) created_at_ms: Option<u64>,
    pub(crate) status: WorkerStatus,
    pub(crate) last_finished_submission_request_id: Option<String>,
    pub(crate) execution_state: PersistedWorkerExecutionState,
    pub(crate) workspace_id: Option<String>,
    pub(crate) workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
    pub(crate) logical_workdir_attachments: Vec<LogicalWorkdirAttachment>,
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
    pub archived_orphan_worker_count: usize,
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
    let staging = migration_sibling(root, "schema-v13-staging")?;
    let backup = migration_sibling(root, "pre-schema-v13-backup")?;
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
        // Runtime and Session schemas evolve independently. Probe only through
        // Session Store's non-mutating current-schema admission boundary; any
        // conversion still runs on the existing owner-locked staging tree.
        let migration_required = crate::retention::sessions_require_migration(root)?;
        let excluded_ephemeral_paths = if migration_required {
            runtime_tree_exclusions(root)?
        } else {
            Vec::new()
        };
        let archived_orphan_worker_count = if migration_required {
            orphaned_worker_directories(root)?.len()
        } else {
            0
        };
        let plan = FsRuntimeStoreMigrationPlan {
            current_schema_version,
            target_schema_version: SCHEMA_VERSION,
            migration_required,
            worker_count: 0,
            archived_orphan_worker_count,
            migrated_worker_aggregate_count: 0,
            migrated_diagnostic_worker_ref_count: 0,
            cleared_diagnostic_worker_ref_count: 0,
            mapping_digest: legacy_worker_identity_mapping_digest(&[]),
            mappings: Vec::new(),
            excluded_ephemeral_paths,
        };
        return Ok((plan, Vec::new()));
    }
    if !matches!(
        current_schema_version,
        OLDEST_SCHEMA_VERSION
            | LEGACY_SCHEMA_VERSION
            | PREVIOUS_SCHEMA_VERSION
            | AGGREGATE_SCHEMA_VERSION
            | EXECUTION_SCHEMA_VERSION
            | SOURCE_SCHEMA_VERSION
            | RESTORE_ADMISSION_SCHEMA_VERSION
            | RESTORE_DECISION_SCHEMA_VERSION
    ) {
        return Err(runtime_store_corrupt(
            &runtime_path,
            format!(
                "unsupported Runtime store schema version {schema_version}; expected {OLDEST_SCHEMA_VERSION}, {LEGACY_SCHEMA_VERSION}, {PREVIOUS_SCHEMA_VERSION}, {AGGREGATE_SCHEMA_VERSION}, {EXECUTION_SCHEMA_VERSION}, {SOURCE_SCHEMA_VERSION}, {RESTORE_ADMISSION_SCHEMA_VERSION}, {RESTORE_DECISION_SCHEMA_VERSION}, or {SCHEMA_VERSION}"
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
    let mut archived_orphan_worker_count = 0;
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
        if !snapshot_path
            .try_exists()
            .map_err(|source| RuntimeError::StoreIo {
                operation: "inspect Worker record",
                path: snapshot_path.clone(),
                source,
            })?
        {
            archived_orphan_worker_count += 1;
            continue;
        }
        let snapshot: serde_json::Value = read_json(&snapshot_path, "read Worker record")?;
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
                        "legacy Worker record is missing workspace_id; unscoped Workers require an explicit migration disposition"
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
        let snapshot: serde_json::Value = read_json(&snapshot_path, "read Worker record")?;
        let migrated = migrate_worker_record(
            snapshot,
            current_schema_version,
            worker.legacy_mapping.as_ref(),
            &snapshot_path,
        )?;
        if migrated.worker_id != worker.worker_id {
            return Err(runtime_store_corrupt(
                &snapshot_path,
                format!(
                    "Worker identity {} does not match directory identity {}",
                    migrated.worker_id, worker.worker_id
                ),
            ));
        }
        worker.workspace_id = worker.workspace_id.clone().or(migrated.workspace_id);

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
        archived_orphan_worker_count,
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

#[derive(Clone, Debug)]
struct MigratedWorkerDocuments {
    identity: serde_json::Value,
    execution: serde_json::Value,
}

fn migrate_schema_v6_worker_document_to_v7(
    mut document: serde_json::Value,
    identity_path: &Path,
) -> Result<serde_json::Value, RuntimeError> {
    let object = document.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(
            identity_path,
            "Worker identity must be an object".to_string(),
        )
    })?;
    let execution = object
        .get_mut("execution")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            runtime_store_corrupt(
                identity_path,
                "Worker execution must be an object".to_string(),
            )
        })?;
    let last_run_generation = execution
        .remove("last_run_generation")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            runtime_store_corrupt(
                identity_path,
                "Worker execution last_run_generation must be an unsigned integer".to_string(),
            )
        })?;
    if let Some(binding) = execution
        .get_mut("binding")
        .and_then(serde_json::Value::as_object_mut)
    {
        let binding_run_generation = binding
            .remove("run_generation")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                runtime_store_corrupt(
                    identity_path,
                    "Worker execution binding run_generation must be an unsigned integer"
                        .to_string(),
                )
            })?;
        if binding_run_generation != last_run_generation {
            return Err(runtime_store_corrupt(
                identity_path,
                format!(
                    "Worker execution binding run_generation {binding_run_generation} does not match last_run_generation {last_run_generation}"
                ),
            ));
        }
    }
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(LEGACY_SCHEMA_VERSION),
    );
    Ok(document)
}

fn migrate_worker_document(
    document: serde_json::Value,
    source_schema_version: u32,
    _mapping: Option<&LegacyWorkerIdentityMapping>,
    identity_path: &Path,
) -> Result<MigratedWorkerDocuments, RuntimeError> {
    let mut document = if source_schema_version == OLDEST_SCHEMA_VERSION {
        migrate_schema_v6_worker_document_to_v7(document, identity_path)?
    } else {
        document
    };
    if !matches!(
        source_schema_version,
        OLDEST_SCHEMA_VERSION | LEGACY_SCHEMA_VERSION
    ) {
        return Err(runtime_store_corrupt(
            identity_path,
            format!(
                "unsupported Worker identity schema {source_schema_version}; expected {OLDEST_SCHEMA_VERSION} or {LEGACY_SCHEMA_VERSION}"
            ),
        ));
    }
    let object = document.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(identity_path, "Worker record must be an object".to_string())
    })?;
    let mut execution = object
        .remove("execution")
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| {
            runtime_store_corrupt(
                identity_path,
                "Worker record execution must be an object".to_string(),
            )
        })?;
    let request_value = object.remove("request").ok_or_else(|| {
        runtime_store_corrupt(
            identity_path,
            "Worker record request must be present".to_string(),
        )
    })?;
    let request: CreateWorkerRequest =
        serde_json::from_value(request_value.clone()).map_err(|error| {
            runtime_store_corrupt(
                identity_path,
                format!("decode Worker execution request: {error}"),
            )
        })?;
    object.insert(
        "profile".to_string(),
        serde_json::to_value(&request.profile).expect("Profile selector serializes"),
    );
    object.insert(
        "display_name".to_string(),
        serde_json::to_value(&request.display_name).expect("display name serializes"),
    );
    object.insert(
        "profile_source".to_string(),
        serde_json::to_value(request.profile_source.reference())
            .expect("Profile source reference serializes"),
    );
    object.insert(
        "config_bundle".to_string(),
        serde_json::to_value(&request.config_bundle).expect("config bundle serializes"),
    );
    execution.insert("request".to_string(), request_value);
    execution.insert(
        "schema_version".to_string(),
        serde_json::Value::from(PREVIOUS_SCHEMA_VERSION),
    );
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(PREVIOUS_SCHEMA_VERSION),
    );
    let migrated = MigratedWorkerDocuments {
        identity: document,
        execution: serde_json::Value::Object(execution),
    };
    validate_migrated_worker_documents(&migrated, identity_path)?;
    Ok(migrated)
}

fn validate_migrated_worker_documents(
    documents: &MigratedWorkerDocuments,
    identity_path: &Path,
) -> Result<WorkerIdentityRecord, RuntimeError> {
    let identity: WorkerIdentityRecord = serde_json::from_value(documents.identity.clone())
        .map_err(|error| {
            runtime_store_corrupt(
                identity_path,
                format!("decode migrated Worker identity: {error}"),
            )
        })?;
    identity.validate_for_schema(identity_path, PREVIOUS_SCHEMA_VERSION)?;
    let execution_path = identity_path.with_file_name(WORKER_EXECUTION_FILE);
    let execution: WorkerExecutionRecord = serde_json::from_value(documents.execution.clone())
        .map_err(|error| {
            runtime_store_corrupt(
                &execution_path,
                format!("decode migrated Worker execution record: {error}"),
            )
        })?;
    execution.validate_for_schema(&identity, &execution_path, PREVIOUS_SCHEMA_VERSION)?;
    Ok(identity)
}

fn migrate_worker_record(
    mut identity_document: serde_json::Value,
    source_schema_version: u32,
    mapping: Option<&LegacyWorkerIdentityMapping>,
    identity_path: &Path,
) -> Result<WorkerAggregateRecord, RuntimeError> {
    if matches!(
        source_schema_version,
        EXECUTION_SCHEMA_VERSION
            | SOURCE_SCHEMA_VERSION
            | RESTORE_ADMISSION_SCHEMA_VERSION
            | RESTORE_DECISION_SCHEMA_VERSION
    ) {
        // Admission identity survives migration. Older pending records did not
        // distinguish a committed decision from preparation or failed cleanup.
        if identity_document
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(source_schema_version))
        {
            return Err(runtime_store_corrupt(
                identity_path,
                "Worker schema does not match Runtime migration source".into(),
            ));
        }
        if source_schema_version != RESTORE_DECISION_SCHEMA_VERSION {
            migrate_restore_commit_fact(&mut identity_document, identity_path)?;
        }
        migrate_restore_repair_fact(&mut identity_document, identity_path)?;
        identity_document["schema_version"] = serde_json::Value::from(SCHEMA_VERSION);
        let aggregate: WorkerAggregateRecord = serde_json::from_value(identity_document)
            .map_err(|error| runtime_store_corrupt(identity_path, error.to_string()))?;
        aggregate.clone().validate(identity_path)?;
        return Ok(aggregate);
    }
    if source_schema_version == AGGREGATE_SCHEMA_VERSION {
        migrate_v9_restore_guard(&mut identity_document, identity_path)?;
        remove_legacy_job_input_counter(&mut identity_document);
        migrate_legacy_request_evidence(&mut identity_document)
            .map_err(|message| runtime_store_corrupt(identity_path, message.into()))?;
        migrate_restore_commit_fact(&mut identity_document, identity_path)?;
        identity_document["schema_version"] = serde_json::Value::from(SCHEMA_VERSION);
        let aggregate: WorkerAggregateRecord =
            serde_json::from_value(identity_document).map_err(|error| {
                runtime_store_corrupt(
                    identity_path,
                    format!("decode migrated schema-v9 Worker aggregate: {error}"),
                )
            })?;
        aggregate.clone().validate(identity_path)?;
        return Ok(aggregate);
    }
    remove_legacy_job_input_counter(&mut identity_document);
    migrate_legacy_request_evidence(&mut identity_document)
        .map_err(|message| runtime_store_corrupt(identity_path, message.into()))?;
    if source_schema_version == PREVIOUS_SCHEMA_VERSION {
        let identity: WorkerIdentityRecord =
            serde_json::from_value(identity_document).map_err(|error| {
                runtime_store_corrupt(
                    identity_path,
                    format!("decode schema-v8 Worker identity: {error}"),
                )
            })?;
        identity.validate_for_schema(identity_path, PREVIOUS_SCHEMA_VERSION)?;
        let execution_path = identity_path.with_file_name(WORKER_EXECUTION_FILE);
        let mut execution_document: serde_json::Value =
            read_bounded_json(&execution_path, "read schema-v8 Worker execution record")?;
        remove_legacy_job_input_counter(&mut execution_document);
        migrate_legacy_request_evidence(&mut execution_document)
            .map_err(|message| runtime_store_corrupt(&execution_path, message.into()))?;
        let execution: WorkerExecutionRecord =
            serde_json::from_value(execution_document).map_err(|error| {
                runtime_store_corrupt(
                    &execution_path,
                    format!("decode migrated schema-v8 execution: {error}"),
                )
            })?;
        execution.validate_for_schema(&identity, &execution_path, PREVIOUS_SCHEMA_VERSION)?;
        return Ok(WorkerAggregateRecord::from_v8(identity, execution));
    }

    let documents = migrate_worker_document(
        identity_document,
        source_schema_version,
        mapping,
        identity_path,
    )?;
    let identity: WorkerIdentityRecord =
        serde_json::from_value(documents.identity).map_err(|error| {
            runtime_store_corrupt(
                identity_path,
                format!("decode validated schema-v8 Worker identity: {error}"),
            )
        })?;
    let execution_path = identity_path.with_file_name(WORKER_EXECUTION_FILE);
    let execution: WorkerExecutionRecord =
        serde_json::from_value(documents.execution).map_err(|error| {
            runtime_store_corrupt(
                &execution_path,
                format!("decode validated schema-v8 Worker execution record: {error}"),
            )
        })?;
    identity.validate_for_schema(identity_path, PREVIOUS_SCHEMA_VERSION)?;
    execution.validate_for_schema(&identity, &execution_path, PREVIOUS_SCHEMA_VERSION)?;
    Ok(WorkerAggregateRecord::from_v8(identity, execution))
}

// Missing commit evidence is not proof of rollback. This conversion applies
// only to versioned legacy aggregates, including Restore nested under Stop.
fn migrate_restore_commit_fact(
    document: &mut serde_json::Value,
    path: &Path,
) -> Result<(), RuntimeError> {
    let execution = &mut document["execution_state"];
    if execution["state"] != "reconciliation_required" {
        return Ok(());
    }
    let operation = &mut execution["execution"];
    let restore = match operation["operation"].as_str() {
        Some("restore") => Some(&mut operation["intent"]),
        Some("stop") => operation["intent"]
            .as_object_mut()
            .and_then(|stop| stop.get_mut("pending_restore"))
            .filter(|restore| !restore.is_null()),
        _ => None,
    };
    if let Some(restore) = restore {
        let restore = restore.as_object_mut().ok_or_else(|| {
            runtime_store_corrupt(path, "legacy pending Restore must be an object".into())
        })?;
        if restore
            .insert("commit".into(), serde_json::json!("legacy_unknown"))
            .is_some()
        {
            return Err(runtime_store_corrupt(
                path,
                "legacy Restore contains unexpected commit evidence".into(),
            ));
        }
    }
    Ok(())
}

// Earlier writers could repair another canonical store before commit without
// retaining that authority in this aggregate. Missing evidence is never false.
fn migrate_restore_repair_fact(
    document: &mut serde_json::Value,
    path: &Path,
) -> Result<(), RuntimeError> {
    let execution = &mut document["execution_state"];
    if execution["state"] != "reconciliation_required" {
        return Ok(());
    }
    let operation = &mut execution["execution"];
    let restore = match operation["operation"].as_str() {
        Some("restore") => Some(&mut operation["intent"]),
        Some("stop") => operation["intent"]
            .as_object_mut()
            .and_then(|stop| stop.get_mut("pending_restore"))
            .filter(|restore| !restore.is_null()),
        _ => None,
    };
    if let Some(restore) = restore {
        let restore = restore.as_object_mut().ok_or_else(|| {
            runtime_store_corrupt(path, "legacy pending Restore must be an object".into())
        })?;
        if restore
            .insert("source_repair_possible".into(), serde_json::json!(true))
            .is_some()
        {
            return Err(runtime_store_corrupt(
                path,
                "legacy Restore contains unexpected source repair evidence".into(),
            ));
        }
    }
    Ok(())
}

// Frozen schema-v9 conversion. Preserve the exact token and all Restore owners
// and receipts; generating a fresh token here would lose retry/admission authority.
fn migrate_v9_restore_guard(
    document: &mut serde_json::Value,
    path: &Path,
) -> Result<(), RuntimeError> {
    let Some(guard) = document.get_mut("restore_guard") else {
        return Ok(());
    };
    let guard = guard.as_object_mut().ok_or_else(|| {
        runtime_store_corrupt(path, "schema-v9 restore_guard must be an object".into())
    })?;
    let token = guard.remove("generation").ok_or_else(|| {
        runtime_store_corrupt(path, "schema-v9 restore_guard is missing generation".into())
    })?;
    if !token.as_str().is_some_and(|token| !token.is_empty())
        || guard.contains_key("observation_token")
    {
        return Err(runtime_store_corrupt(
            path,
            "schema-v9 Restore observation token is invalid or ambiguous".into(),
        ));
    }
    guard.insert("observation_token".into(), token);
    Ok(())
}

// Legacy Runtime Job bindings never held input JSON, so the old caller-selected
// counter cannot become a content digest. Retain Job/attempt identity and leave
// content identity unknown until the host supplies its immutable intent.
fn remove_legacy_job_input_counter(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(job) = object
                .get_mut("backend_job")
                .and_then(serde_json::Value::as_object_mut)
            {
                job.remove("input_revision");
            }
            for value in object.values_mut() {
                remove_legacy_job_input_counter(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_legacy_job_input_counter(value);
            }
        }
        _ => {}
    }
}

// Frozen schema 6..9 request migration. Preserve concrete source/config digests,
// contents, operation ownership and fingerprints. Unknown legacy SSH key identity
// stays empty and cannot pass the materializer's live-authority validation.
fn migrate_legacy_request_evidence(value: &mut serde_json::Value) -> Result<(), &'static str> {
    match value {
        serde_json::Value::Object(object) => {
            if object.contains_key("credential_id") && object.contains_key("credential_revision") {
                let counter = object.remove("credential_revision").expect("checked field");
                if !counter.as_u64().is_some_and(|value| value > 0) {
                    return Err("invalid legacy SSH credential evidence");
                }
                object
                    .entry("public_key_fingerprint")
                    .or_insert_with(|| serde_json::json!(""));
            }
            if object.contains_key("host_trust_id") && object.contains_key("host_trust_revision") {
                let counter = object.remove("host_trust_revision").expect("checked field");
                if !counter.as_u64().is_some_and(|value| value > 0) {
                    return Err("invalid legacy SSH host trust evidence");
                }
                object
                    .entry("host_key_fingerprint")
                    .or_insert_with(|| serde_json::json!(""));
            }
            if let Some(settings) = object
                .get_mut("memory_settings")
                .and_then(serde_json::Value::as_object_mut)
                && let Some(counter) = settings.remove("settings_revision")
                && !counter.as_u64().is_some_and(|value| value > 0)
            {
                return Err("invalid legacy Memory settings evidence");
            }
            if object.contains_key("source_fingerprint") {
                object.remove("source_revision");
            }
            if object.contains_key("config_projection_digest") {
                object.remove("config_revision");
            }
            if object.contains_key("resource_id") && object.contains_key("nonce") {
                object.remove("revision");
                object.remove("generation");
            }
            for value in object.values_mut() {
                migrate_legacy_request_evidence(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                migrate_legacy_request_evidence(value)?;
            }
        }
        _ => {}
    }
    Ok(())
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
        let mut manifest = manifest::read_persisted_worker_manifest_snapshot(snapshot.clone())
            .map_err(|error| {
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
        manifest.worker.name = expected_name.clone();
        *snapshot =
            manifest::write_persisted_worker_manifest_snapshot(&manifest).map_err(|error| {
                runtime_store_corrupt(
                    metadata_path,
                    format!("encode migrated Worker aggregate resolved manifest: {error}"),
                )
            })?;
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
        let manifest =
            manifest::read_persisted_worker_manifest_snapshot(snapshot).map_err(|error| {
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
    // Intermediate schema-11 writers may already have saved irreversible fences.
    // Default only a missing field; malformed or existing fences must not be erased.
    object
        .entry("subject_create_recovery_fences")
        .or_insert_with(|| serde_json::json!({}));
    if !matches!(
        source_schema_version,
        SOURCE_SCHEMA_VERSION | RESTORE_ADMISSION_SCHEMA_VERSION | RESTORE_DECISION_SCHEMA_VERSION
    ) {
        object.remove("workers");
        // This legacy cache is not Runtime lifecycle or restore authority. Bundles
        // are reacquired from Workspace content authority, never renumbered/rehash-pinned here.
        object.remove("config_bundles");
        object.remove("next_worker_sequence");
    }
    let snapshot: RuntimeSnapshot = serde_json::from_value(document.clone()).map_err(|error| {
        runtime_store_corrupt(
            runtime_path,
            format!("decode migrated Runtime snapshot: {error}"),
        )
    })?;
    snapshot.validate(runtime_path)?;
    Ok((document, counts))
}

fn runtime_store_parent(root: &Path) -> Result<&Path, RuntimeError> {
    let parent = root.parent().ok_or_else(|| {
        runtime_store_corrupt(
            root,
            "Runtime store root has no parent directory".to_string(),
        )
    })?;
    Ok(if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    })
}

fn migration_sibling(root: &Path, suffix: &str) -> Result<PathBuf, RuntimeError> {
    let parent = runtime_store_parent(root)?;
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            runtime_store_corrupt(root, "Runtime store root name is not UTF-8".to_string())
        })?;
    Ok(parent.join(format!(".{name}.{suffix}")))
}

fn migration_directory_exists(path: &Path) -> Result<bool, RuntimeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(RuntimeError::StoreIo {
                operation: "inspect Runtime migration artifact",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(runtime_store_corrupt(
            path,
            "Runtime migration artifact is not a regular directory".to_string(),
        ));
    }
    Ok(true)
}

fn validate_current_runtime_store(root: &Path) -> Result<(), RuntimeError> {
    let store = FsRuntimeStore {
        root: root.to_path_buf(),
        _owner_lock: None,
        worker_write_fault: Arc::new(AtomicU8::new(AtomicWriteFault::None as u8)),
        #[cfg(test)]
        worker_reconcile_sync_fault: Default::default(),
    };
    store.load_runtime_state().map(|_| ())
}

fn validate_completed_migration_staging(root: &Path) -> Result<(), RuntimeError> {
    let store = FsRuntimeStore {
        root: root.to_path_buf(),
        _owner_lock: None,
        worker_write_fault: Arc::new(AtomicU8::new(AtomicWriteFault::None as u8)),
        #[cfg(test)]
        worker_reconcile_sync_fault: Default::default(),
    };
    let state = store.load_runtime_state()?;
    if let Some(worker) = state.workers.values().find(|worker| {
        matches!(
            worker.execution_state,
            PersistedWorkerExecutionState::Unavailable
        )
    }) {
        return Err(runtime_store_corrupt(
            root,
            format!(
                "migrated Worker {} has unavailable execution metadata",
                worker.worker_id
            ),
        ));
    }
    if state
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "worker_record_unavailable")
    {
        return Err(runtime_store_corrupt(
            root,
            "migrated Runtime store contains unavailable Worker records".to_string(),
        ));
    }
    Ok(())
}

fn remove_runtime_migration_directory(path: &Path) -> Result<(), RuntimeError> {
    if !migration_directory_exists(path)? {
        return Ok(());
    }
    fs::remove_dir_all(path)
        .map_err(|source| runtime_io_error("remove Runtime migration artifact", path, source))
}

fn recover_runtime_store_migration(root: &Path) -> Result<(), RuntimeError> {
    for suffix in [
        "schema-v9-staging",
        "pre-schema-v9-backup",
        "schema-v10-staging",
        "pre-schema-v10-backup",
        "schema-v11-staging",
        "pre-schema-v11-backup",
        "schema-v12-staging",
        "pre-schema-v12-backup",
    ] {
        let earlier = migration_sibling(root, suffix)?;
        if earlier.exists() {
            return Err(runtime_store_corrupt(root, "unfinished earlier Runtime migration requires explicit recovery before schema-v13 startup".into()));
        }
    }
    let staging = migration_sibling(root, "schema-v13-staging")?;
    let backup = migration_sibling(root, "pre-schema-v13-backup")?;
    let root_exists = migration_directory_exists(root)?;
    let staging_exists = migration_directory_exists(&staging)?;
    let backup_exists = migration_directory_exists(&backup)?;
    let parent = runtime_store_parent(root)?;

    if root_exists {
        if staging_exists {
            remove_runtime_migration_directory(&staging)?;
            sync_directory(parent, "recover Runtime migration")?;
        }
        if backup_exists {
            validate_current_runtime_store(root)?;
            remove_runtime_migration_directory(&backup)?;
            sync_directory(parent, "recover Runtime migration")?;
        }
        return Ok(());
    }

    if staging_exists {
        match validate_completed_migration_staging(&staging) {
            Ok(()) => {
                fs::rename(&staging, root).map_err(|source| {
                    runtime_io_error("activate recovered Runtime migration", &staging, source)
                })?;
                sync_directory(parent, "recover Runtime migration")?;
                if backup_exists {
                    remove_runtime_migration_directory(&backup)?;
                    sync_directory(parent, "recover Runtime migration")?;
                }
                return Ok(());
            }
            Err(_staging_error) if backup_exists => {
                remove_runtime_migration_directory(&staging)?;
                fs::rename(&backup, root).map_err(|source| {
                    runtime_io_error("restore Runtime migration backup", &backup, source)
                })?;
                sync_directory(parent, "recover Runtime migration")?;
                return Ok(());
            }
            Err(staging_error) => return Err(staging_error),
        }
    }

    if backup_exists {
        fs::rename(&backup, root).map_err(|source| {
            runtime_io_error("restore Runtime migration backup", &backup, source)
        })?;
        sync_directory(parent, "recover Runtime migration")?;
    }
    Ok(())
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

fn sync_runtime_tree(path: &Path) -> Result<(), RuntimeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| runtime_io_error("sync Runtime migration tree", path, source))?;
    if metadata.file_type().is_file() {
        return File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|source| runtime_io_error("sync Runtime migration file", path, source));
    }
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(runtime_store_corrupt(
            path,
            "Runtime migration sync refuses symlinks and special files".to_string(),
        ));
    }
    let mut entries = fs::read_dir(path)
        .map_err(|source| runtime_io_error("read Runtime migration tree", path, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| runtime_io_error("read Runtime migration tree", path, source))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        sync_runtime_tree(&entry.path())?;
    }
    sync_directory(path, "sync Runtime migration directory")
}

fn migrate_runtime_store(
    root: &Path,
    runtime_id: &str,
) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
    let (plan, _) = plan_runtime_store_migration(root, runtime_id)?;
    if !plan.migration_required {
        return Ok(plan);
    }
    let staging = migration_sibling(root, "schema-v13-staging")?;
    let backup = migration_sibling(root, "pre-schema-v13-backup")?;
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
    if let Err(error) = validate_completed_migration_staging(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    sync_runtime_tree(&staging)?;
    let parent = runtime_store_parent(root)?;
    fs::rename(root, &backup)
        .map_err(|error| runtime_io_error("backup runtime store", root, error))?;
    sync_directory(parent, "backup runtime store")?;
    if let Err(error) = fs::rename(&staging, root) {
        let rollback = fs::rename(&backup, root)
            .and_then(|()| File::open(parent).and_then(|directory| directory.sync_all()));
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
    sync_directory(parent, "activate migrated runtime store")?;
    fs::remove_dir_all(&backup)
        .map_err(|error| runtime_io_error("remove runtime migration backup", &backup, error))?;
    sync_directory(parent, "remove runtime migration backup")?;
    debug_assert_eq!(plan.mapping_digest, staged_plan.mapping_digest);
    Ok(plan)
}

fn orphaned_worker_directories(
    root: &Path,
) -> Result<Vec<(std::ffi::OsString, PathBuf)>, RuntimeError> {
    let workers_dir = root.join(WORKERS_DIR);
    let mut orphaned = fs::read_dir(&workers_dir)
        .map_err(|source| runtime_io_error("read Worker records", &workers_dir, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| runtime_io_error("read Worker records", &workers_dir, source))?
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            (!path.join(WORKER_FILE).is_file()).then_some((entry.file_name(), path))
        })
        .collect::<Vec<_>>();
    orphaned.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(orphaned)
}

fn archive_orphaned_worker_directories(root: &Path) -> Result<usize, RuntimeError> {
    let orphaned = orphaned_worker_directories(root)?;
    if orphaned.is_empty() {
        return Ok(0);
    }

    let archive_dir = root.join(ORPHANED_WORKERS_DIR);
    fs::create_dir_all(&archive_dir).map_err(|source| {
        runtime_io_error("create orphaned Worker archive", &archive_dir, source)
    })?;
    for (name, source_dir) in &orphaned {
        let target_dir = archive_dir.join(name);
        if target_dir.exists() {
            return Err(runtime_store_corrupt(
                &target_dir,
                format!(
                    "orphaned Worker archive target already exists for {}",
                    source_dir.display()
                ),
            ));
        }
        fs::rename(source_dir, &target_dir).map_err(|source| {
            runtime_io_error("archive orphaned Worker directory", source_dir, source)
        })?;
    }
    Ok(orphaned.len())
}

fn migrate_runtime_store_in_place(
    root: &Path,
    runtime_id: &str,
) -> Result<FsRuntimeStoreMigrationPlan, RuntimeError> {
    let (plan, planned_workers) = plan_runtime_store_migration(root, runtime_id)?;
    if !plan.migration_required {
        return Ok(plan);
    }
    if plan.current_schema_version == SCHEMA_VERSION {
        // Session-only migration: do not reserialize Runtime/Worker identity,
        // Restore guards, recovery fences, or retained execution receipts.
        // As in a Runtime schema migration, retain orphan trees outside the
        // Worker catalog before validating the completed staging tree.
        let archived = archive_orphaned_worker_directories(root)?;
        if archived != plan.archived_orphan_worker_count {
            return Err(runtime_store_corrupt(
                root,
                "orphan count changed during Session migration".into(),
            ));
        }
        crate::retention::migrate_staged_sessions(root)?;
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
                    "decode Worker record {}: {error}",
                    source_snapshot_path.display()
                ),
            )
        })?;
        let aggregate = migrate_worker_record(
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
        let migrated_aggregate_path = migrated_dir.join(WORKER_FILE);
        atomic_write_json(
            &migrated_aggregate_path,
            &aggregate,
            "migrate Worker aggregate",
        )?;
        let legacy_execution_path = migrated_dir.join(WORKER_EXECUTION_FILE);
        match fs::remove_file(&legacy_execution_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(runtime_io_error(
                    "remove migrated Worker execution record",
                    &legacy_execution_path,
                    source,
                ));
            }
        }
        // Schema 11/12 metadata was validated above but needs no conversion.
        if let Some(metadata) = metadata
            && !matches!(
                plan.current_schema_version,
                SOURCE_SCHEMA_VERSION
                    | RESTORE_ADMISSION_SCHEMA_VERSION
                    | RESTORE_DECISION_SCHEMA_VERSION
            )
        {
            atomic_write_json(
                &migrated_dir.join(WORKER_METADATA_FILE),
                &metadata,
                "migrate Worker aggregate identity",
            )?;
        }
    }

    let archived_orphan_worker_count = archive_orphaned_worker_directories(root)?;
    if archived_orphan_worker_count != plan.archived_orphan_worker_count {
        return Err(runtime_store_corrupt(
            root,
            format!(
                "archived {archived_orphan_worker_count} orphaned Worker directories; migration plan expected {}",
                plan.archived_orphan_worker_count
            ),
        ));
    }
    // A Runtime schema bump says nothing about the retained Session schema.
    // Session Store owns versioned conversion, including v4 -> generic v5.
    crate::retention::migrate_staged_sessions(root)?;
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
    #[serde(default)]
    subject_create_recovery_fences: BTreeMap<WorkerId, crate::runtime::SubjectCreateRecoveryFence>,
    #[serde(default)]
    subject_releases: Vec<(
        runtime_api::SubjectReleaseRequest,
        runtime_api::SubjectReleaseState,
    )>,
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
        code: "worker_record_unavailable".to_string(),
        message: message.into(),
    });
}

impl RuntimeSnapshot {
    fn from_persisted(state: &PersistedRuntimeState) -> Self {
        Self {
            subject_create_recovery_fences: state.subject_create_recovery_fences.clone(),
            subject_releases: state.subject_releases.clone(),
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
        for (worker_id, fence) in &self.subject_create_recovery_fences {
            let request = &fence.request;
            let response_matches = fence.response.as_ref().is_none_or(|response| {
                response.worker_id == request.worker_id
                    && response.workspace_id == request.workspace_id
                    && response.create_fingerprint == request.create_fingerprint
                    && response.operation_id == request.operation_id
            });
            if *worker_id != request.worker_id
                || request.workspace_id.trim().is_empty()
                || request.create_fingerprint.trim().is_empty()
                || request.create_fingerprint.len() > 512
                || request.operation_id.trim().is_empty()
                || request.operation_id.len() > 512
                || self
                    .workspace_owners
                    .get(&request.workspace_id)
                    .is_none_or(|owner| owner.trim().is_empty())
                || !response_matches
            {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read Subject create recovery fence",
                    path: path.to_path_buf(),
                    message: "invalid recovery identity, receipt or Workspace owner".into(),
                });
            }
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
            subject_create_recovery_fences: self.subject_create_recovery_fences,
            subject_releases: self.subject_releases,
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
#[serde(deny_unknown_fields)]
struct WorkerAggregateRecord {
    #[serde(default)]
    restore_guard: crate::runtime::RestoreGuard,
    schema_version: u32,
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    profile: ProfileSelector,
    display_name: Option<String>,
    profile_source: ProfileSourceArchiveRef,
    config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<u64>,
    status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_finished_submission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    logical_workdir_attachments: Vec<LogicalWorkdirAttachment>,
    execution_state: WorkerExecutionStateRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", content = "execution", rename_all = "snake_case")]
enum WorkerExecutionStateRecord {
    Available(WorkerExecutionData),
    ReconciliationRequired(PersistedWorkerLifecycleOperation),
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerExecutionData {
    request: CreateWorkerRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<PersistedWorkerExecutionBinding>,
    restore_intent: WorkerRestoreIntent,
}

impl WorkerAggregateRecord {
    fn from_persisted(worker: &PersistedWorkerRecord) -> Self {
        let execution_state = match &worker.execution_state {
            PersistedWorkerExecutionState::Available(execution) => {
                WorkerExecutionStateRecord::Available(WorkerExecutionData {
                    request: execution.request.clone(),
                    binding: execution.binding.clone(),
                    restore_intent: execution.restore_intent,
                })
            }
            PersistedWorkerExecutionState::ReconciliationRequired(operation) => {
                WorkerExecutionStateRecord::ReconciliationRequired(operation.clone())
            }
            PersistedWorkerExecutionState::Unavailable => WorkerExecutionStateRecord::Unavailable,
        };
        Self {
            schema_version: SCHEMA_VERSION,
            restore_guard: worker.restore_guard.clone(),
            worker_ref: worker.worker_ref.clone(),
            worker_id: worker.worker_id,
            profile: worker.profile.clone(),
            display_name: worker.display_name.clone(),
            profile_source: worker.profile_source.clone(),
            config_bundle: worker.config_bundle.clone(),
            created_at_ms: worker.created_at_ms,
            status: worker.status,
            last_finished_submission_request_id: worker.last_finished_submission_request_id.clone(),
            workspace_id: worker.workspace_id.clone(),
            workdir_attachments: worker.workdir_attachments.clone(),
            logical_workdir_attachments: worker.logical_workdir_attachments.clone(),
            execution_state,
        }
    }

    fn from_v8(identity: WorkerIdentityRecord, execution: WorkerExecutionRecord) -> Self {
        let persisted = identity.into_persisted(PersistedWorkerExecutionState::Available(
            PersistedWorkerExecution {
                request: execution.request,
                binding: execution.binding,
                restore_intent: execution.restore_intent,
            },
        ));
        Self::from_persisted(&persisted)
    }

    fn validate(self, path: &Path) -> Result<PersistedWorkerRecord, RuntimeError> {
        let restore_guard = self.restore_guard;
        let last_finished_submission_request_id = self.last_finished_submission_request_id;
        let identity = WorkerIdentityRecord {
            schema_version: self.schema_version,
            worker_ref: self.worker_ref,
            worker_id: self.worker_id,
            profile: self.profile,
            display_name: self.display_name,
            profile_source: self.profile_source,
            config_bundle: self.config_bundle,
            created_at_ms: self.created_at_ms,
            status: self.status,
            workspace_id: self.workspace_id,
            workdir_attachments: self.workdir_attachments,
            logical_workdir_attachments: self.logical_workdir_attachments,
            legacy_working_directory: None,
        };
        identity.validate_for_schema(path, SCHEMA_VERSION)?;
        let execution_state = match self.execution_state {
            WorkerExecutionStateRecord::Available(execution) => {
                let execution = WorkerExecutionRecord {
                    schema_version: SCHEMA_VERSION,
                    request: execution.request,
                    binding: execution.binding,
                    restore_intent: execution.restore_intent,
                };
                PersistedWorkerExecutionState::Available(execution.validate_for_schema(
                    &identity,
                    path,
                    SCHEMA_VERSION,
                )?)
            }
            WorkerExecutionStateRecord::ReconciliationRequired(operation) => {
                let (request, binding, restore_intent, last_settled_status) = match &operation {
                    PersistedWorkerLifecycleOperation::Restore(restore) => {
                        if restore.mode == PersistedWorkerRestoreMode::Automatic
                            && !restore.last_settled_status.is_active()
                        {
                            return Err(RuntimeError::StoreCorrupt {
                                operation: "read Worker restore reconciliation",
                                path: path.to_path_buf(),
                                message: "automatic restore reconciliation requires an active settled Worker"
                                    .to_string(),
                            });
                        }
                        (
                            Some(&restore.request),
                            &restore.binding,
                            if restore.last_settled_status.is_active() {
                                WorkerRestoreIntent::Automatic
                            } else {
                                WorkerRestoreIntent::Explicit
                            },
                            restore.last_settled_status,
                        )
                    }
                    PersistedWorkerLifecycleOperation::Stop(stop) => {
                        if !stop.last_settled_status.is_active() && stop.pending_restore.is_none() {
                            return Err(RuntimeError::StoreCorrupt {
                                operation: "read Worker stop reconciliation",
                                path: path.to_path_buf(),
                                message: "stop reconciliation requires an active settled Worker"
                                    .to_string(),
                            });
                        }
                        if stop.request.is_none()
                            && (stop.binding.is_some() || stop.pending_restore.is_some())
                        {
                            return Err(RuntimeError::StoreCorrupt {
                                operation: "read Worker stop reconciliation",
                                path: path.to_path_buf(),
                                message: "metadata-free stop cannot carry restore authority"
                                    .to_string(),
                            });
                        }
                        (
                            stop.request.as_ref(),
                            &stop.binding,
                            stop.restore_intent,
                            stop.last_settled_status,
                        )
                    }
                };
                if identity.status != last_settled_status {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read Worker lifecycle reconciliation",
                        path: path.to_path_buf(),
                        message:
                            "Worker reconciliation status does not match the last settled status"
                                .to_string(),
                    });
                }
                if let Some(request) = request {
                    let mut validation_request = request.clone();
                    // Pending admission may supply a missing Workspace connection
                    // without applying it to the saved execution. Validate that
                    // exact owner's intent against identity; do not widen the
                    // Available-record rule or mutate the canonical request.
                    if validation_request.workspace_api.is_none()
                        && let PersistedWorkerLifecycleOperation::Restore(restore) = &operation
                        && let Some(api) = restore_guard
                            .pending_preparation(restore.operation_id)
                            .and_then(|preparation| preparation.workspace_api.as_ref())
                    {
                        validation_request.workspace_api =
                            Some(
                                serde_json::from_value(serde_json::to_value(api).map_err(
                                    |error| runtime_store_corrupt(path, error.to_string()),
                                )?)
                                .map_err(|error| runtime_store_corrupt(path, error.to_string()))?,
                            );
                    }
                    WorkerExecutionRecord {
                        schema_version: SCHEMA_VERSION,
                        request: validation_request,
                        binding: binding.clone(),
                        restore_intent,
                    }
                    .validate_for_schema(&identity, path, SCHEMA_VERSION)?;
                }
                PersistedWorkerExecutionState::ReconciliationRequired(operation)
            }
            WorkerExecutionStateRecord::Unavailable => PersistedWorkerExecutionState::Unavailable,
        };
        let mut persisted = identity.into_persisted(execution_state);
        persisted.restore_guard = restore_guard;
        persisted.last_finished_submission_request_id = last_finished_submission_request_id;
        Ok(persisted)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerIdentityRecord {
    schema_version: u32,
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    profile: ProfileSelector,
    display_name: Option<String>,
    profile_source: ProfileSourceArchiveRef,
    config_bundle: Option<ConfigBundleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<u64>,
    status: WorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workdir_attachments: Vec<WorkingDirectoryAttachmentStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    logical_workdir_attachments: Vec<LogicalWorkdirAttachment>,
    /// Schema-v8 compatibility input. New records serialize only `workdir_attachments`.
    #[serde(
        default,
        rename = "working_directory",
        skip_serializing_if = "Option::is_none"
    )]
    legacy_working_directory: Option<crate::catalog::WorkingDirectoryStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerExecutionRecord {
    schema_version: u32,
    request: CreateWorkerRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<PersistedWorkerExecutionBinding>,
    restore_intent: WorkerRestoreIntent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkerExecutionRecord {
    schema_version: u32,
    request: serde_json::Value,
    #[serde(default)]
    binding: Option<PersistedWorkerExecutionBinding>,
    restore_intent: WorkerRestoreIntent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWorkingDirectoryClaim {
    working_directory_id: String,
    #[serde(default)]
    relative_cwd: Option<String>,
}

impl<'de> Deserialize<'de> for WorkerExecutionRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawWorkerExecutionRecord::deserialize(deserializer)?;
        let mut request_value = raw.request;
        let request_object = request_value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("Worker create request must be an object"))?;
        let legacy_request = request_object.remove("working_directory_request");
        let legacy_claim = request_object.remove("working_directory");
        let mut request: CreateWorkerRequest =
            serde_json::from_value(request_value).map_err(serde::de::Error::custom)?;
        if request.workdir_attachment_requests.is_empty() && request.workdir_attachments.is_empty()
        {
            let alias = workdir::WorkdirAttachmentAlias::new("workdir")
                .expect("static compatibility alias is valid");
            if let Some(value) = legacy_request.filter(|value| !value.is_null()) {
                let working_directory: WorkingDirectoryRequest =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                request
                    .workdir_attachment_requests
                    .push(WorkingDirectoryAttachmentRequest {
                        alias,
                        working_directory,
                    });
            } else if let Some(value) = legacy_claim.filter(|value| !value.is_null()) {
                let claim: LegacyWorkingDirectoryClaim =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                request
                    .workdir_attachments
                    .push(WorkingDirectoryAttachmentClaim {
                        alias,
                        working_directory_id: claim.working_directory_id,
                        relative_cwd: claim.relative_cwd,
                        // The removed singular field represented a full local
                        // Workdir binding. Preserve that known legacy contract
                        // explicitly; missing capabilities on the current claim
                        // shape default to read-only instead.
                        capabilities: workdir::WorkdirSessionCapabilities::ALL,
                    });
            }
        }
        Ok(Self {
            schema_version: raw.schema_version,
            request,
            binding: raw.binding,
            restore_intent: raw.restore_intent,
        })
    }
}

impl WorkerIdentityRecord {
    fn validate_for_schema(&self, path: &Path, expected_schema: u32) -> Result<(), RuntimeError> {
        if self.schema_version != expected_schema {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker identity",
                path: path.to_path_buf(),
                message: format!(
                    "unsupported schema version {}, expected {}",
                    self.schema_version, expected_schema
                ),
            });
        }
        if self.worker_ref.worker_id != self.worker_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker identity",
                path: path.to_path_buf(),
                message: format!(
                    "worker_ref id {} does not match worker_id {}",
                    self.worker_ref.worker_id, self.worker_id
                ),
            });
        }
        let expected_name = self.worker_id.to_string();
        if path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some(expected_name.as_str())
        {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker identity",
                path: path.to_path_buf(),
                message: format!(
                    "Worker identity location does not match worker_id {}",
                    self.worker_id
                ),
            });
        }
        Ok(())
    }

    fn into_persisted(
        self,
        execution_state: PersistedWorkerExecutionState,
    ) -> PersistedWorkerRecord {
        let workdir_attachments = if self.workdir_attachments.is_empty() {
            self.legacy_working_directory
                .map(|working_directory| {
                    vec![WorkingDirectoryAttachmentStatus {
                        alias: workdir::WorkdirAttachmentAlias::new("workdir")
                            .expect("static compatibility alias is valid"),
                        working_directory,
                    }]
                })
                .unwrap_or_default()
        } else {
            self.workdir_attachments
        };
        PersistedWorkerRecord {
            restore_guard: crate::runtime::RestoreGuard::default(),
            worker_ref: self.worker_ref,
            worker_id: self.worker_id,
            profile: self.profile,
            display_name: self.display_name,
            profile_source: self.profile_source,
            config_bundle: self.config_bundle,
            created_at_ms: self.created_at_ms,
            status: self.status,
            last_finished_submission_request_id: None,
            execution_state,
            workspace_id: self.workspace_id,
            workdir_attachments,
            logical_workdir_attachments: self.logical_workdir_attachments,
        }
    }
}

impl WorkerExecutionRecord {
    fn validate_for_schema(
        &self,
        identity: &WorkerIdentityRecord,
        path: &Path,
        expected_schema: u32,
    ) -> Result<PersistedWorkerExecution, RuntimeError> {
        if self.schema_version != expected_schema {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker execution record",
                path: path.to_path_buf(),
                message: format!(
                    "unsupported schema version {}, expected {}",
                    self.schema_version, expected_schema
                ),
            });
        }
        let request_workspace_id = self
            .request
            .workspace_api
            .as_ref()
            .map(|workspace_api| workspace_api.workspace_id.as_str());
        if identity.workspace_id.as_deref() != request_workspace_id
            || self.request.worker_id != identity.worker_id
            || self.request.profile != identity.profile
            || self.request.display_name != identity.display_name
            || self.request.profile_source.reference() != identity.profile_source
            || self.request.config_bundle != identity.config_bundle
        {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker execution record",
                path: path.to_path_buf(),
                message: "Worker execution request does not match persisted Worker identity"
                    .to_string(),
            });
        }
        match (identity.status, self.restore_intent) {
            (status, WorkerRestoreIntent::Automatic) if status.is_active() => {
                if self.binding.is_none() {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read Worker execution record",
                        path: path.to_path_buf(),
                        message: "automatic restore intent requires an execution binding"
                            .to_string(),
                    });
                }
            }
            (WorkerStatus::Stopped, WorkerRestoreIntent::Explicit) => {}
            _ => {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read Worker execution record",
                    path: path.to_path_buf(),
                    message: format!(
                        "Worker status {:?} conflicts with restore intent {:?}",
                        identity.status, self.restore_intent
                    ),
                });
            }
        }
        Ok(PersistedWorkerExecution {
            request: self.request.clone(),
            binding: self.binding.clone(),
            restore_intent: self.restore_intent,
        })
    }
}

fn read_bounded_json<T>(path: &Path, operation: &'static str) -> Result<T, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    read_bounded_json_file(path, operation).map(|(value, _)| value)
}

// Keep the validated file open so reconciliation syncs the observed authority,
// not a separately reopened projection. The Runtime owner and Worker locks
// exclude cooperating writers throughout the read and sync.
fn read_bounded_json_file<T>(
    path: &Path,
    operation: &'static str,
) -> Result<(T, File), RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    let metadata = fs::symlink_metadata(path).map_err(|source| match source.kind() {
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
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: path.to_path_buf(),
            message: "record is not a regular file".to_string(),
        });
    }
    if metadata.len() > MAX_WORKER_RECORD_BYTES {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: path.to_path_buf(),
            message: format!(
                "record size {} exceeds limit {MAX_WORKER_RECORD_BYTES}",
                metadata.len()
            ),
        });
    }
    let mut file = File::open(path).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_WORKER_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_WORKER_RECORD_BYTES {
        return Err(RuntimeError::StoreCorrupt {
            operation,
            path: path.to_path_buf(),
            message: format!("record exceeds limit {MAX_WORKER_RECORD_BYTES} while reading"),
        });
    }
    let value = serde_json::from_slice(&bytes).map_err(|source| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;
    Ok((value, file))
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
    atomic_write_json_with_fault(path, value, operation, AtomicWriteFault::None)
}

fn atomic_write_json_with_fault<T>(
    path: &Path,
    value: &T,
    operation: &'static str,
    fault: AtomicWriteFault,
) -> Result<(), RuntimeError>
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
        if fault == AtomicWriteFault::BeforeRename {
            return Err(RuntimeError::StoreIo {
                operation,
                path: path.to_path_buf(),
                source: std::io::Error::other("injected failure before atomic rename"),
            });
        }
        fs::rename(&tmp_path, path).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
        if fault == AtomicWriteFault::AfterRename {
            return Err(RuntimeError::StoreCommitOutcomeUnknown {
                operation,
                path: path.to_path_buf(),
                source: std::io::Error::other("injected failure after atomic rename"),
            });
        }
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| RuntimeError::StoreCommitOutcomeUnknown {
                operation,
                path: path.to_path_buf(),
                source,
            })
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
mod tests {
    use super::*;

    fn write_empty_schema_v8_store(root: &Path) {
        fs::create_dir_all(root.join(WORKERS_DIR)).unwrap();
        fs::write(
            root.join(RUNTIME_FILE),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": PREVIOUS_SCHEMA_VERSION,
                "display_name": "test-runtime",
                "backend": "fs_store",
                "status": "running",
                "next_diagnostic_id": 1,
                "config_bundles": {},
                "workspace_owners": {},
                "assignments": [],
                "execution": [],
                "diagnostics": []
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn stored_schema_version(root: &Path) -> u64 {
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(RUNTIME_FILE)).unwrap()).unwrap();
        document["schema_version"].as_u64().unwrap()
    }

    #[test]
    fn schema_v11_migration_preserves_identity_history_and_recovery_fences_on_reopen() {
        // Cover the released schema (no field) and intermediate writers with
        // either a pending fence or a completed recovery receipt.
        for fence_state in ["absent", "pending", "completed"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runtime");
            let worker_id = WorkerId::now_v7();
            let fenced_worker_id = WorkerId::now_v7();
            let worker_dir = root.join(WORKERS_DIR).join(worker_id.to_string());
            fs::create_dir_all(worker_dir.join("session")).unwrap();
            let runtime_path = root.join(RUNTIME_FILE);
            let worker_path = worker_dir.join(WORKER_FILE);
            let metadata_path = worker_dir.join(WORKER_METADATA_FILE);
            let history_path = worker_dir.join("session/history.json");
            let history = b"[{\"role\":\"user\",\"content\":\"retained history\"}]\n";
            fs::write(&history_path, history).unwrap();
            let metadata = serde_json::to_vec(&serde_json::json!({
                "worker_name": runtime_worker_name(worker_id),
                "workspace_id": "workspace-a",
                "retained_extension": {"history": "must not be rewritten"}
            }))
            .unwrap();
            fs::write(&metadata_path, &metadata).unwrap();

            let mut runtime = serde_json::json!({
                "schema_version": 11,
                "display_name": "schema-11 Runtime",
                "backend": "fs_store",
                "status": "running",
                "next_diagnostic_id": 2,
                "config_bundles": {},
                "workspace_owners": {"workspace-a": "owner-a"},
                "subject_releases": [],
                "diagnostics": [{
                    "id": 1, "worker_ref": {"worker_id": worker_id},
                    "severity": "warning", "code": "retained_history",
                    "message": "preserve diagnostic identity"
                }]
            });
            let recovery_identity = serde_json::json!({
                "workspace_id": "workspace-a", "worker_id": fenced_worker_id,
                "create_fingerprint": "failed-create", "operation_id": "recover-a"
            });
            if fence_state != "absent" {
                runtime["subject_create_recovery_fences"] = serde_json::json!({
                    fenced_worker_id.to_string(): {
                        "request": recovery_identity,
                        "response": if fence_state == "completed" {
                            recovery_identity.clone()
                        } else {
                            serde_json::Value::Null
                        }
                    }
                });
            }
            atomic_write_json(&runtime_path, &runtime, "write schema-11 fixture").unwrap();
            let runtime_before = fs::read(&runtime_path).unwrap();

            let mut request = schema_v7_worker_document(worker_id)["request"].clone();
            request["workspace_api"] = serde_json::json!({
                "workspace_id": "workspace-a", "base_url": "https://workspace.invalid"
            });
            // Serialize defaults as a schema-11 writer would, without running
            // a migration or generating a fresh Restore guard for the fixture.
            let request: CreateWorkerRequest = serde_json::from_value(request).unwrap();
            let worker: WorkerAggregateRecord = serde_json::from_value(serde_json::json!({
                "schema_version": 11,
                "restore_guard": {
                    "observation_token": "retained-restore-token",
                    "active_request_id": null, "owners": {}
                },
                "worker_ref": {"worker_id": worker_id}, "worker_id": worker_id,
                "profile": request.profile,
                "display_name": null,
                "profile_source": request.profile_source.reference(),
                "config_bundle": null,
                "created_at_ms": 1234,
                "status": "running",
                "last_finished_submission_request_id": "finished-submission",
                "workspace_id": "workspace-a",
                "execution_state": {"state": "available", "execution": {
                    "request": request, "binding": {}, "restore_intent": "automatic"
                }}
            }))
            .unwrap();
            let mut expected_worker = serde_json::to_value(&worker).unwrap();
            atomic_write_json(&worker_path, &worker, "write schema-11 Worker").unwrap();
            let worker_before = fs::read(&worker_path).unwrap();
            let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-test");
            let plan = FsRuntimeStore::migration_plan(&options).unwrap();
            assert_eq!(plan.current_schema_version, 11);
            assert_eq!(plan.target_schema_version, SCHEMA_VERSION);
            assert!(plan.migration_required);
            assert_eq!(plan.worker_count, 1);
            assert_eq!(plan.migrated_worker_aggregate_count, 0);
            assert!(plan.mappings.is_empty());
            assert_eq!(fs::read(&runtime_path).unwrap(), runtime_before);
            assert_eq!(fs::read(&worker_path).unwrap(), worker_before);
            // The direct reader still rejects a previous schema: migration is
            // explicit, not permissive deserialization of a newer format.
            let snapshot: RuntimeSnapshot = serde_json::from_value(runtime.clone()).unwrap();
            assert!(snapshot.validate(&runtime_path).is_err());

            let opened = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap();
            let state = opened.state.as_ref().unwrap();
            let persisted_worker = state.workers.get(&worker_id).unwrap();
            expected_worker["schema_version"] = serde_json::json!(SCHEMA_VERSION);
            assert_eq!(
                serde_json::to_value(WorkerAggregateRecord::from_persisted(persisted_worker))
                    .unwrap(),
                expected_worker
            );
            runtime["schema_version"] = serde_json::json!(SCHEMA_VERSION);
            if fence_state == "absent" {
                runtime["subject_create_recovery_fences"] = serde_json::json!({});
            }
            assert_eq!(
                serde_json::to_value(&state.subject_create_recovery_fences).unwrap(),
                runtime["subject_create_recovery_fences"]
            );
            assert_eq!(
                read_json::<serde_json::Value>(&runtime_path, "read migrated Runtime").unwrap(),
                runtime
            );
            assert_eq!(
                read_json::<serde_json::Value>(&worker_path, "read migrated Worker").unwrap(),
                expected_worker
            );
            // Exercise the normal writer too: it must retain intermediate fences.
            opened.store.write_runtime_snapshot(state).unwrap();
            drop(opened);
            let runtime_once = fs::read(&runtime_path).unwrap();
            let worker_once = fs::read(&worker_path).unwrap();
            let reopened = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap();
            assert_eq!(
                serde_json::to_value(
                    &reopened
                        .state
                        .as_ref()
                        .unwrap()
                        .subject_create_recovery_fences
                )
                .unwrap(),
                runtime["subject_create_recovery_fences"]
            );
            drop(reopened);
            let plan = FsRuntimeStore::migrate(&options).unwrap();
            assert!(!plan.migration_required);
            assert_eq!(plan.current_schema_version, SCHEMA_VERSION);
            assert_eq!(plan.target_schema_version, SCHEMA_VERSION);
            assert!(
                !FsRuntimeStore::migration_plan(&options)
                    .unwrap()
                    .migration_required
            );
            assert_eq!(fs::read(&runtime_path).unwrap(), runtime_once);
            assert_eq!(fs::read(&worker_path).unwrap(), worker_once);
            assert_eq!(fs::read(&metadata_path).unwrap(), metadata);
            assert_eq!(fs::read(&history_path).unwrap(), history);
            assert!(
                !migration_sibling(&root, "schema-v13-staging")
                    .unwrap()
                    .exists()
            );
            assert!(
                !migration_sibling(&root, "pre-schema-v13-backup")
                    .unwrap()
                    .exists()
            );
        }
    }

    #[test]
    fn schema_v11_migration_rejects_invalid_fences_without_replacing_them() {
        for fences in [
            serde_json::Value::Null,
            serde_json::json!({"invalid-worker": {}}),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runtime");
            write_empty_schema_v8_store(&root);
            let path = root.join(RUNTIME_FILE);
            let mut runtime: serde_json::Value = read_json(&path, "read fixture").unwrap();
            runtime["schema_version"] = serde_json::json!(11);
            runtime["subject_create_recovery_fences"] = fences;
            atomic_write_json(&path, &runtime, "write invalid fence fixture").unwrap();
            let before = fs::read(&path).unwrap();
            assert!(FsRuntimeStore::open_or_create(root, "runtime-test").is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn schema_v10_startup_migrates_stopped_sessions_atomically() {
        for corrupt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runtime");
            write_empty_schema_v8_store(&root);
            let mut runtime: serde_json::Value =
                read_json(&root.join(RUNTIME_FILE), "test").unwrap();
            runtime["schema_version"] = serde_json::json!(10);
            atomic_write_json(&root.join(RUNTIME_FILE), &runtime, "test").unwrap();
            let worker_id = WorkerId::now_v7();
            let aggregate = root.join(WORKERS_DIR).join(worker_id.to_string());
            fs::create_dir_all(&aggregate).unwrap();
            let record = migrate_worker_record(
                schema_v7_worker_document(worker_id),
                7,
                None,
                &aggregate.join(WORKER_FILE),
            )
            .unwrap();
            let mut record = serde_json::to_value(record).unwrap();
            record["schema_version"] = serde_json::json!(10);
            atomic_write_json(&aggregate.join(WORKER_FILE), &record, "test").unwrap();
            let session = aggregate.join("session");
            fs::create_dir_all(session.join("segments")).unwrap();
            atomic_write_json(&session.join("session.json"), &serde_json::json!({"schema_version":3,"session_id":session_store::new_session_id()}), "test").unwrap();
            if corrupt {
                fs::write(
                    session
                        .join("segments")
                        .join(format!("{}.jsonl", session_store::new_segment_id())),
                    b"invalid\n",
                )
                .unwrap();
            }
            let before = fs::read(session.join("session.json")).unwrap();
            let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-test");
            assert!(
                FsRuntimeStore::migration_plan(&options)
                    .unwrap()
                    .migration_required
            );
            assert_eq!(fs::read(session.join("session.json")).unwrap(), before);
            let result = FsRuntimeStore::open_or_create(root.clone(), "runtime-test");
            if corrupt {
                assert!(result.is_err());
                assert_eq!(stored_schema_version(&root), 10);
                assert_eq!(fs::read(session.join("session.json")).unwrap(), before);
                let record: serde_json::Value =
                    read_json(&aggregate.join(WORKER_FILE), "test").unwrap();
                assert_eq!(record["schema_version"], 10);
            } else {
                drop(result.unwrap());
                assert_eq!(stored_schema_version(&root), u64::from(SCHEMA_VERSION));
                session_store::WorkerSessionStore::open_read_only(&session)
                    .expect("startup migration must admit the current Session schema");
                let once = fs::read(session.join("session.json")).unwrap();
                drop(FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap());
                assert_eq!(fs::read(session.join("session.json")).unwrap(), once);
            }
        }
    }

    #[test]
    fn auth_initialized_directory_without_snapshot_is_a_fresh_store() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("runtime");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("auth.toml"), b"fixture auth material").unwrap();

        let opened = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap();

        assert!(opened.state.is_none());
        assert!(root.join(WORKERS_DIR).is_dir());
        assert_eq!(
            fs::read(root.join("auth.toml")).unwrap(),
            b"fixture auth material"
        );
    }

    #[test]
    fn snapshotless_directory_rejects_non_auth_persistence() {
        for unexpected in ["events.jsonl", "unknown.json"] {
            let parent = tempfile::tempdir().unwrap();
            let root = parent.path().join("runtime");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("auth.toml"), b"fixture auth material").unwrap();
            fs::write(root.join(unexpected), b"must remain").unwrap();

            let error = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap_err();

            assert!(matches!(
                error,
                RuntimeError::StoreCorrupt { path, .. } if path == root
            ));
            assert_eq!(fs::read(root.join(unexpected)).unwrap(), b"must remain");
            assert!(!root.join(WORKERS_DIR).exists());
        }
    }

    #[test]
    fn orphan_archive_without_runtime_snapshot_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("runtime");
        fs::create_dir_all(root.join(ORPHANED_WORKERS_DIR).join("worker-a")).unwrap();

        let error = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::StoreCorrupt { path, .. } if path == root
        ));
        assert!(root.join(ORPHANED_WORKERS_DIR).join("worker-a").is_dir());
    }

    #[test]
    fn worker_records_without_runtime_snapshot_are_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("runtime");
        fs::create_dir_all(root.join(WORKERS_DIR).join("orphan")).unwrap();

        let error = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::StoreCorrupt { path, .. } if path == root
        ));
    }

    #[test]
    fn runtime_store_owner_lock_child_probe() {
        let Some(root) = std::env::var_os("YOI_TEST_RUNTIME_STORE_LOCK_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        if std::env::var_os("YOI_TEST_RUNTIME_STORE_LOCK_EXIT_WITHOUT_DROP").is_some() {
            let _store = FsRuntimeStore::open_or_create(root, "runtime-test").unwrap();
            std::process::exit(0);
        }
        assert!(matches!(
            FsRuntimeStore::open_or_create(root, "runtime-test").unwrap_err(),
            RuntimeError::RuntimeStoreAlreadyOpen { .. }
        ));
    }

    #[test]
    fn runtime_store_owner_lock_is_released_when_process_exits_without_drop() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("crashed-runtime-store");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("fs_store::tests::runtime_store_owner_lock_child_probe")
            .arg("--exact")
            .env("YOI_TEST_RUNTIME_STORE_LOCK_ROOT", &root)
            .env("YOI_TEST_RUNTIME_STORE_LOCK_EXIT_WITHOUT_DROP", "1")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "process-exit lock probe failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        acquire_runtime_store_owner_lock(&root).unwrap();
    }

    #[test]
    fn second_runtime_store_open_conflicts_before_store_mutation() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("runtime-store");
        let first = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap();
        let legacy_events = root.join("events.jsonl");
        fs::write(&legacy_events, b"must remain").unwrap();

        let error = FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::RuntimeStoreAlreadyOpen { path } if path == root
        ));
        assert_eq!(fs::read(&legacy_events).unwrap(), b"must remain");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("fs_store::tests::runtime_store_owner_lock_child_probe")
            .arg("--exact")
            .env("YOI_TEST_RUNTIME_STORE_LOCK_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "cross-process lock probe failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("owner.lock")
        }));

        let retained_clone = first.store.clone();
        drop(first);
        assert!(matches!(
            FsRuntimeStore::open_or_create(root.clone(), "runtime-test").unwrap_err(),
            RuntimeError::RuntimeStoreAlreadyOpen { .. }
        ));
        drop(retained_clone);
        fs::remove_dir_all(&root).unwrap();
        FsRuntimeStore::open_or_create(root, "runtime-test").unwrap();
    }

    #[test]
    fn schema_v7_migration_archives_orphan_worker_directories() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(RUNTIME_FILE),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": PREVIOUS_SCHEMA_VERSION,
                "display_name": null,
                "backend": "fs_store",
                "status": "running",
                "next_diagnostic_id": 1,
                "config_bundles": {},
                "workspace_owners": {},
                "diagnostics": []
            }))
            .unwrap(),
        )
        .unwrap();
        fs::create_dir_all(root.path().join(WORKERS_DIR).join("orphan").join("session")).unwrap();
        fs::write(
            root.path()
                .join(WORKERS_DIR)
                .join("orphan")
                .join("session")
                .join("history.json"),
            b"[]",
        )
        .unwrap();
        let (plan, _) = plan_runtime_store_migration(root.path(), "runtime-test").unwrap();

        assert!(plan.migration_required);
        assert_eq!(plan.current_schema_version, PREVIOUS_SCHEMA_VERSION);
        assert_eq!(plan.target_schema_version, SCHEMA_VERSION);
        assert_eq!(plan.worker_count, 0);
        assert_eq!(plan.archived_orphan_worker_count, 1);

        let opened =
            FsRuntimeStore::open_or_create(root.path().to_path_buf(), "runtime-test").unwrap();

        assert!(opened.state.unwrap().diagnostics.is_empty());
        assert!(!root.path().join(WORKERS_DIR).join("orphan").exists());
        assert!(
            root.path()
                .join(ORPHANED_WORKERS_DIR)
                .join("orphan")
                .join("session")
                .join("history.json")
                .is_file()
        );
    }

    fn schema_v7_worker_document(worker_id: WorkerId) -> serde_json::Value {
        serde_json::json!({
            "schema_version": LEGACY_SCHEMA_VERSION,
            "worker_ref": { "worker_id": worker_id },
            "worker_id": worker_id,
            "request": {
                "worker_id": worker_id,
                "create_fingerprint": "test-create",
                "profile": { "kind": "builtin", "value": "builtin:coder" },
                "profile_source": {
                    "kind": "workspace_config",
                    "archive": {
                        "id": "archive-1",
                        "digest": "sha256:archive",
                        "size_bytes": 0,
                        "source_graph": {
                            "source_count": 0,
                            "total_source_bytes": 0,
                            "entrypoints": {},
                            "import_count": 0
                        }
                    }
                }
            },
            "status": "running",
            "execution": {
                "binding": {},
                "restore_intent": "automatic"
            }
        })
    }

    fn schema_v6_worker_document(worker_id: WorkerId) -> serde_json::Value {
        let mut document = schema_v7_worker_document(worker_id);
        document["schema_version"] = serde_json::json!(OLDEST_SCHEMA_VERSION);
        document["execution"]["last_run_generation"] = serde_json::json!(7);
        document["execution"]["binding"] = serde_json::json!({ "run_generation": 7 });
        document
    }

    #[test]
    fn schema_v8_singular_workdir_fields_normalize_to_one_attachment() {
        let worker_id = WorkerId::now_v7();
        let workdir_status = serde_json::json!({
            "summary": {
                "working_directory_id": "wd-legacy",
                "repository_id": "repo",
                "materializer_kind": "runtime_git_clone",
                "status": "active"
            }
        });
        let identity: WorkerIdentityRecord = serde_json::from_value(serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "worker_ref": { "worker_id": worker_id },
            "worker_id": worker_id,
            "profile": { "kind": "builtin", "value": "builtin:coder" },
            "display_name": null,
            "profile_source": {
                "id": "archive-1",
                "digest": "sha256:archive",
                "size_bytes": 0,
                "source_graph": {
                    "source_count": 0,
                    "total_source_bytes": 0,
                    "entrypoints": {},
                    "import_count": 0
                }
            },
            "config_bundle": null,
            "status": "stopped",
            "working_directory": workdir_status
        }))
        .unwrap();
        let persisted = identity.into_persisted(PersistedWorkerExecutionState::Unavailable);
        assert!(persisted.logical_workdir_attachments.is_empty());
        assert_eq!(persisted.workdir_attachments.len(), 1);
        assert_eq!(persisted.workdir_attachments[0].alias.as_str(), "workdir");
        assert_eq!(
            persisted.workdir_attachments[0]
                .working_directory
                .summary
                .working_directory_id,
            "wd-legacy"
        );

        let mut request = schema_v7_worker_document(worker_id)["request"].clone();
        request["working_directory"] = serde_json::json!({
            "working_directory_id": "wd-legacy",
            "relative_cwd": "crates/yoi"
        });
        let execution: WorkerExecutionRecord = serde_json::from_value(serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "request": request,
            "binding": null,
            "restore_intent": "explicit"
        }))
        .unwrap();
        assert!(execution.request.workdir_attachment_requests.is_empty());
        assert_eq!(execution.request.workdir_attachments.len(), 1);
        let attachment = &execution.request.workdir_attachments[0];
        assert_eq!(attachment.alias.as_str(), "workdir");
        assert_eq!(attachment.working_directory_id, "wd-legacy");
        assert_eq!(attachment.relative_cwd.as_deref(), Some("crates/yoi"));
        assert_eq!(
            attachment.capabilities,
            workdir::WorkdirSessionCapabilities::ALL
        );
        let serialized = serde_json::to_value(execution).unwrap();
        assert!(serialized["request"].get("working_directory").is_none());
    }

    #[test]
    fn current_attachment_claim_without_capabilities_defaults_to_read_only() {
        let worker_id = WorkerId::now_v7();
        let mut request = schema_v7_worker_document(worker_id)["request"].clone();
        request["workdir_attachments"] = serde_json::json!([{
            "alias": "docs",
            "working_directory_id": "wd-docs"
        }]);
        let execution: WorkerExecutionRecord = serde_json::from_value(serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "request": request,
            "binding": {},
            "restore_intent": "automatic"
        }))
        .unwrap();

        assert_eq!(
            execution.request.workdir_attachments[0].capabilities,
            workdir::WorkdirSessionCapabilities::READ_ONLY
        );
        let serialized = serde_json::to_value(execution).unwrap();
        assert_eq!(
            serialized["request"]["workdir_attachments"][0]["capabilities"],
            serde_json::to_value(workdir::WorkdirSessionCapabilities::READ_ONLY).unwrap()
        );
    }

    #[test]
    fn worker_identity_round_trips_logical_workdir_attachments() {
        let worker_id = WorkerId::now_v7();
        let identity: WorkerIdentityRecord = serde_json::from_value(serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "worker_ref": { "worker_id": worker_id },
            "worker_id": worker_id,
            "profile": { "kind": "builtin", "value": "builtin:coder" },
            "display_name": null,
            "profile_source": {
                "id": "archive-1",
                "digest": "sha256:archive",
                "size_bytes": 0,
                "source_graph": {
                    "source_count": 0,
                    "total_source_bytes": 0,
                    "entrypoints": {},
                    "import_count": 0
                }
            },
            "config_bundle": null,
            "status": "stopped",
            "logical_workdir_attachments": [{
                "alias": "checkout",
                "working_directory_id": "remote-workdir"
            }]
        }))
        .unwrap();
        let persisted = identity.into_persisted(PersistedWorkerExecutionState::Unavailable);
        assert_eq!(
            persisted.logical_workdir_attachments,
            [LogicalWorkdirAttachment {
                alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
                working_directory_id: "remote-workdir".to_string(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            }]
        );
    }

    #[test]
    fn startup_recovers_staging_created_before_source_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let staging = migration_sibling(&root, "schema-v13-staging").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("partial"), b"partial").unwrap();
        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();

        store.store.load_runtime_state().unwrap();
        assert_eq!(stored_schema_version(&root), u64::from(SCHEMA_VERSION));
        assert!(!staging.exists());
    }

    #[test]
    fn startup_restores_backup_when_source_was_renamed_before_activation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let backup = migration_sibling(&root, "pre-schema-v13-backup").unwrap();
        fs::rename(&root, &backup).unwrap();
        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();

        store.store.load_runtime_state().unwrap();
        assert_eq!(stored_schema_version(&root), u64::from(SCHEMA_VERSION));
        assert!(!backup.exists());
    }

    #[test]
    fn startup_promotes_valid_staging_after_source_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let staging = migration_sibling(&root, "schema-v13-staging").unwrap();
        copy_runtime_tree(&root, &root, &staging).unwrap();
        migrate_runtime_store_in_place(&staging, "test-runtime").unwrap();
        let backup = migration_sibling(&root, "pre-schema-v13-backup").unwrap();
        fs::rename(&root, &backup).unwrap();
        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();

        store.store.load_runtime_state().unwrap();
        assert_eq!(stored_schema_version(&root), u64::from(SCHEMA_VERSION));
        assert!(!staging.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn startup_restores_backup_when_staging_is_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let staging = migration_sibling(&root, "schema-v13-staging").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join(RUNTIME_FILE), b"{\"schema_version\":8}").unwrap();
        let backup = migration_sibling(&root, "pre-schema-v13-backup").unwrap();
        fs::rename(&root, &backup).unwrap();

        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();

        store.store.load_runtime_state().unwrap();
        assert_eq!(stored_schema_version(&root), u64::from(SCHEMA_VERSION));
        assert!(!staging.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn startup_removes_backup_left_after_activation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        migrate_runtime_store_in_place(&root, "test-runtime").unwrap();
        let backup = migration_sibling(&root, "pre-schema-v13-backup").unwrap();
        write_empty_schema_v8_store(&backup);
        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();

        store.store.load_runtime_state().unwrap();
        assert!(!backup.exists());
    }

    #[test]
    fn earlier_migration_backup_cannot_be_mistaken_for_a_fresh_runtime() {
        for suffix in [
            "pre-schema-v9-backup",
            "pre-schema-v10-backup",
            "schema-v11-staging",
            "pre-schema-v11-backup",
            "schema-v12-staging",
            "pre-schema-v12-backup",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runtime-store");
            let backup = migration_sibling(&root, suffix).unwrap();
            write_empty_schema_v8_store(&backup);
            let error = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap_err();
            assert!(error.to_string().contains("explicit recovery"));
            assert!(!root.exists());
            assert!(backup.join(RUNTIME_FILE).exists());
        }
    }

    #[test]
    fn legacy_request_migration_preserves_content_and_never_invents_key_identity() {
        let mut document = serde_json::json!({"request": {
            "memory_settings": {"workspace_id":"workspace-a", "language":"English", "settings_revision":2},
            "repository": {"source_revision":1, "source_fingerprint":"source-content"},
            "materialization": {"operation_id":"op-a", "config_revision":2, "config_projection_digest":"projection-content", "ssh": {
                "host_trust_id":"host-a", "host_trust_revision":3,
                "credential_candidates":[{"credential_id":"credential-a", "credential_revision":4}]
            }}
        }});
        migrate_legacy_request_evidence(&mut document).unwrap();
        let request = &document["request"];
        assert_eq!(
            request["memory_settings"],
            serde_json::json!({"workspace_id": "workspace-a", "language": "English"})
        );
        assert_eq!(
            request["repository"]["source_fingerprint"],
            "source-content"
        );
        assert_eq!(
            request["materialization"]["config_projection_digest"],
            "projection-content"
        );
        assert_eq!(
            request["materialization"]["ssh"]["host_key_fingerprint"],
            ""
        );
        assert_eq!(
            request["materialization"]["ssh"]["credential_candidates"][0]["public_key_fingerprint"],
            ""
        );
    }

    #[test]
    fn schema_v9_restore_token_migrates_durably_without_rotating_admission_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let runtime_path = root.join(RUNTIME_FILE);
        let mut runtime: serde_json::Value = read_json(&runtime_path, "read test Runtime").unwrap();
        runtime["schema_version"] = serde_json::json!(AGGREGATE_SCHEMA_VERSION);
        fs::write(&runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
        let worker_id = WorkerId::now_v7();
        let path = root
            .join(WORKERS_DIR)
            .join(worker_id.to_string())
            .join(WORKER_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let aggregate = migrate_worker_record(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &path,
        )
        .unwrap();
        let mut legacy = serde_json::to_value(&aggregate).unwrap();
        let token = legacy["restore_guard"]["observation_token"].take();
        legacy["restore_guard"]
            .as_object_mut()
            .unwrap()
            .remove("observation_token");
        legacy["restore_guard"]["generation"] = token.clone();
        legacy["schema_version"] = serde_json::json!(AGGREGATE_SCHEMA_VERSION);
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let store = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        store.store.load_runtime_state().unwrap();
        drop(store);
        let migrated: serde_json::Value = read_json(&path, "read migrated Worker").unwrap();
        assert_eq!(migrated["schema_version"], SCHEMA_VERSION);
        assert_eq!(migrated["restore_guard"]["observation_token"], token);
        assert!(migrated["restore_guard"].get("generation").is_none());
        let store = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        store.store.load_runtime_state().unwrap();
        assert_eq!(
            read_json::<serde_json::Value>(&path, "read restarted Worker").unwrap(),
            migrated
        );
    }

    #[test]
    fn frozen_restore_guard_conversion_preserves_receipts_and_rejects_ambiguous_identity() {
        let path = Path::new("worker.json");
        let mut document = serde_json::json!({"restore_guard": {
            "generation": "opaque-admission-token", "active_request_id": "restore-a",
            "owners": {"restore-a": {"fingerprint": "intent", "receipt": {"accepted": true}}}
        }});
        let owners = document["restore_guard"]["owners"].clone();
        migrate_v9_restore_guard(&mut document, path).unwrap();
        assert_eq!(document["restore_guard"]["owners"], owners);
        assert_eq!(document["restore_guard"]["active_request_id"], "restore-a");
        assert_eq!(
            document["restore_guard"]["observation_token"],
            "opaque-admission-token"
        );
        let mut ambiguous =
            serde_json::json!({"restore_guard": {"generation": "old", "observation_token": "new"}});
        assert!(migrate_v9_restore_guard(&mut ambiguous, path).is_err());
    }

    #[test]
    fn bounded_worker_record_read_rejects_oversize_before_parse() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(WORKER_FILE);
        File::create(&path)
            .unwrap()
            .set_len(MAX_WORKER_RECORD_BYTES + 1)
            .unwrap();

        let error =
            read_bounded_json::<serde_json::Value>(&path, "read Worker identity").unwrap_err();

        assert!(matches!(error, RuntimeError::StoreCorrupt { .. }));
        assert!(error.to_string().contains("exceeds limit"));
    }

    #[test]
    fn worker_aggregate_write_distinguishes_pre_and_post_rename_failures() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let worker_id = WorkerId::now_v7();
        let aggregate_path = root
            .join(WORKERS_DIR)
            .join(worker_id.to_string())
            .join(WORKER_FILE);
        let mut worker = migrate_worker_record(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &aggregate_path,
        )
        .unwrap()
        .validate(&aggregate_path)
        .unwrap();

        opened
            .store
            .fail_next_worker_write(AtomicWriteFault::BeforeRename);
        let error = opened.store.write_worker_record(&worker).unwrap_err();
        assert!(matches!(error, RuntimeError::StoreIo { .. }));
        assert!(!aggregate_path.exists());
        assert!(opened.store.read_worker_record(&worker.worker_ref).is_err());

        worker.created_at_ms = Some(1);
        opened.store.write_worker_record(&worker).unwrap();
        worker.created_at_ms = Some(2);
        opened
            .store
            .fail_next_worker_write(AtomicWriteFault::AfterRename);
        let error = opened.store.write_worker_record(&worker).unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::StoreCommitOutcomeUnknown { .. }
        ));
        let committed: WorkerAggregateRecord =
            read_bounded_json(&aggregate_path, "read test Worker aggregate").unwrap();
        assert_eq!(committed.created_at_ms, Some(2));
        let before = fs::read(&aggregate_path).unwrap();
        assert_eq!(
            opened
                .store
                .read_worker_record(&worker.worker_ref)
                .unwrap()
                .created_at_ms,
            Some(2),
            "unknown write must be reconciled from the canonical record"
        );
        assert_eq!(fs::read(&aggregate_path).unwrap(), before);
        assert!(
            !aggregate_path
                .with_file_name(WORKER_EXECUTION_FILE)
                .exists()
        );
    }

    #[test]
    fn canonical_worker_read_rejects_unreadable_or_wrong_identity_without_repair() {
        let temp = tempfile::tempdir().unwrap();
        let opened =
            FsRuntimeStore::open_or_create(temp.path().join("runtime-store"), "test-runtime")
                .unwrap();
        let requested = WorkerRef::new(WorkerId::now_v7());
        let path = opened
            .store
            .worker_dir(&requested.worker_id)
            .join(WORKER_FILE);
        assert!(opened.store.read_worker_record(&requested).is_err());
        assert!(!path.exists());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let foreign = WorkerId::now_v7();
        let foreign_record = migrate_worker_record(
            schema_v7_worker_document(foreign),
            LEGACY_SCHEMA_VERSION,
            None,
            &path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(foreign.to_string())
                .join(WORKER_FILE),
        )
        .unwrap();
        let documents = [
            b"not-json".to_vec(),
            serde_json::to_vec(&foreign_record).unwrap(),
        ];
        for document in documents {
            fs::write(&path, &document).unwrap();
            assert!(opened.store.read_worker_record(&requested).is_err());
            assert_eq!(fs::read(&path).unwrap(), document);
        }
        File::create(&path)
            .unwrap()
            .set_len(MAX_WORKER_RECORD_BYTES + 1)
            .unwrap();
        assert!(opened.store.read_worker_record(&requested).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            MAX_WORKER_RECORD_BYTES + 1
        );
    }

    fn canonical_worker_store() -> (
        tempfile::TempDir,
        FsRuntimeStore,
        PersistedWorkerRecord,
        PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let opened =
            FsRuntimeStore::open_or_create(temp.path().join("runtime-store"), "test-runtime")
                .unwrap();
        let worker_id = WorkerId::now_v7();
        let path = opened.store.worker_dir(&worker_id).join(WORKER_FILE);
        let mut worker = migrate_worker_record(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &path,
        )
        .unwrap()
        .validate(&path)
        .unwrap();
        worker.created_at_ms = Some(1);
        opened.store.write_worker_record(&worker).unwrap();
        (temp, opened.store, worker, path)
    }

    #[test]
    fn legacy_pending_restore_migrates_as_unknown_and_preserves_stop_ownership() {
        use crate::runtime::RestoreCommitFact;
        for (source_schema, under_stop) in [
            RESTORE_ADMISSION_SCHEMA_VERSION,
            RESTORE_DECISION_SCHEMA_VERSION,
        ]
        .into_iter()
        .flat_map(|schema| [false, true].map(move |stop| (schema, stop)))
        {
            let (_temp, store, mut worker, path) = canonical_worker_store();
            let root = store.root.clone();
            let PersistedWorkerExecutionState::Available(execution) =
                worker.execution_state.clone()
            else {
                panic!("fixture execution unavailable")
            };
            let restore = PersistedWorkerRestoreOperation {
                operation_id: WorkerLifecycleOperationId::new(),
                mode: PersistedWorkerRestoreMode::Explicit,
                commit: RestoreCommitFact::Committed,
                source_repair_possible: false,
                request: execution.request.clone(),
                binding: execution.binding.clone(),
                last_settled_status: worker.status,
            };
            let operation = if under_stop {
                PersistedWorkerLifecycleOperation::Stop(PersistedWorkerStopOperation {
                    operation_id: WorkerLifecycleOperationId::new(),
                    pending_restore: Some(restore),
                    request: Some(execution.request),
                    binding: execution.binding,
                    restore_intent: execution.restore_intent,
                    last_settled_status: worker.status,
                })
            } else {
                PersistedWorkerLifecycleOperation::Restore(restore)
            };
            worker.execution_state =
                PersistedWorkerExecutionState::ReconciliationRequired(operation);
            let mut legacy =
                serde_json::to_value(WorkerAggregateRecord::from_persisted(&worker)).unwrap();
            legacy["schema_version"] = serde_json::json!(source_schema);
            let intent = &mut legacy["execution_state"]["execution"]["intent"];
            let restore = if under_stop {
                &mut intent["pending_restore"]
            } else {
                intent
            };
            if source_schema == RESTORE_ADMISSION_SCHEMA_VERSION {
                restore.as_object_mut().unwrap().remove("commit");
            }
            restore
                .as_object_mut()
                .unwrap()
                .remove("source_repair_possible");
            let expected_request = restore["request"].clone();
            let before_guard = legacy["restore_guard"].clone();
            fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
            write_empty_schema_v8_store(&root);
            let runtime_path = root.join(RUNTIME_FILE);
            let mut runtime: serde_json::Value = read_json(&runtime_path, "read fixture").unwrap();
            runtime["schema_version"] = serde_json::json!(source_schema);
            fs::write(&runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
            drop(store);

            let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
            let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
            let PersistedWorkerExecutionState::ReconciliationRequired(operation) =
                saved.execution_state
            else {
                panic!("migration discarded pending ownership")
            };
            let restored = match operation {
                PersistedWorkerLifecycleOperation::Restore(restore) => restore,
                PersistedWorkerLifecycleOperation::Stop(stop) => stop.pending_restore.unwrap(),
            };
            assert_eq!(
                restored.commit,
                if source_schema == RESTORE_DECISION_SCHEMA_VERSION {
                    RestoreCommitFact::Committed
                } else {
                    RestoreCommitFact::LegacyUnknown
                }
            );
            assert!(
                restored.source_repair_possible,
                "older source-repair effects must remain possible"
            );
            assert_eq!(
                serde_json::to_value(restored.request).unwrap(),
                expected_request
            );
            let disk: serde_json::Value = read_json(&path, "read migrated Worker").unwrap();
            assert_eq!(disk["restore_guard"], before_guard);
            assert_eq!(disk["worker_ref"], legacy["worker_ref"]);
            let settled = disk["execution_state"]["execution"]["intent"].clone();
            let legacy_intent = &legacy["execution_state"]["execution"]["intent"];
            if under_stop {
                assert_eq!(settled["operation_id"], legacy_intent["operation_id"]);
                assert_eq!(
                    settled["pending_restore"]["operation_id"],
                    legacy_intent["pending_restore"]["operation_id"]
                );
            } else {
                assert_eq!(settled["operation_id"], legacy_intent["operation_id"]);
            }
            drop(opened);
            let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
            assert!(
                reopened
                    .store
                    .read_worker_record(&worker.worker_ref)
                    .is_ok()
            );
        }
    }

    #[test]
    fn uncertain_worker_write_sync_failure_stays_unknown_until_sync_recovers() {
        // Real filesystem aggregate/rename/read/validation; substitute only a
        // failing file/directory sync syscall, never the reconciliation owner.
        let (_temp, store, mut worker, path) = canonical_worker_store();
        worker.created_at_ms = Some(2);
        store.fail_next_worker_write(AtomicWriteFault::AfterRename);
        assert!(matches!(
            store.write_worker_record(&worker),
            Err(RuntimeError::StoreCommitOutcomeUnknown { .. })
        ));
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            store
                .read_worker_record(&worker.worker_ref)
                .unwrap()
                .created_at_ms,
            Some(2)
        );
        // The readable replacement is not a durability confirmation. Inject
        // failure at the file and at each directory owning its canonical path.
        for fail_at in 0..4 {
            let mut sync_count = 0;
            let result = store.read_durable_worker_record_with_sync(&worker.worker_ref, |file| {
                let index = sync_count;
                sync_count += 1;
                if index == fail_at {
                    Err(std::io::Error::other(
                        "injected reconciliation sync failure",
                    ))
                } else {
                    file.sync_all()
                }
            });
            assert!(matches!(
                result,
                Err(RuntimeError::StoreCommitOutcomeUnknown { .. })
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        let reconciled = store
            .read_durable_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(reconciled.created_at_ms, Some(2));
        assert_eq!(reconciled.execution_state, worker.execution_state);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn pre_rename_failure_durable_reread_preserves_old_canonical_record() {
        let (_temp, store, mut worker, path) = canonical_worker_store();
        let bytes = fs::read(&path).unwrap();
        worker.created_at_ms = Some(2);
        store.fail_next_worker_write(AtomicWriteFault::BeforeRename);
        assert!(matches!(
            store.write_worker_record(&worker),
            Err(RuntimeError::StoreIo { .. })
        ));
        let reconciled = store
            .read_durable_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(reconciled.created_at_ms, Some(1));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn durable_worker_reread_rejects_invalid_authority_before_sync_without_repair() {
        let (_temp, store, worker, path) = canonical_worker_store();
        let bytes = fs::read(&path).unwrap();
        let foreign = WorkerId::now_v7();
        let foreign_record = migrate_worker_record(
            schema_v7_worker_document(foreign),
            LEGACY_SCHEMA_VERSION,
            None,
            &path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(foreign.to_string())
                .join(WORKER_FILE),
        )
        .unwrap();
        for document in [
            b"not-json".to_vec(),
            serde_json::to_vec(&foreign_record).unwrap(),
        ] {
            fs::write(&path, &document).unwrap();
            assert!(matches!(
                store.read_durable_worker_record_with_sync(&worker.worker_ref, |_| {
                    panic!("invalid authority must not reach durability confirmation")
                }),
                Err(RuntimeError::StoreCorrupt { .. })
            ));
            assert_eq!(fs::read(&path).unwrap(), document);
        }
        File::create(&path)
            .unwrap()
            .set_len(MAX_WORKER_RECORD_BYTES + 1)
            .unwrap();
        assert!(matches!(
            store.read_durable_worker_record_with_sync(&worker.worker_ref, |_| {
                panic!("oversized authority must not reach durability confirmation")
            }),
            Err(RuntimeError::StoreCorrupt { .. })
        ));
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            MAX_WORKER_RECORD_BYTES + 1
        );
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            store.read_durable_worker_record_with_sync(&worker.worker_ref, |_| {
                panic!("missing authority must not reach durability confirmation")
            }),
            Err(RuntimeError::StoreMissing { .. })
        ));
        assert!(!path.exists());
        fs::write(&path, &bytes).unwrap();
        assert!(store.read_durable_worker_record(&worker.worker_ref).is_ok());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn canonical_worker_reread_rejects_symlinked_worker_directory() {
        let (_temp, store, worker, path) = canonical_worker_store();
        let worker_dir = path.parent().unwrap();
        let retained_dir = worker_dir.with_extension("retained");
        fs::rename(worker_dir, &retained_dir).unwrap();
        std::os::unix::fs::symlink(&retained_dir, worker_dir).unwrap();
        let bytes = fs::read(retained_dir.join(WORKER_FILE)).unwrap();
        assert!(matches!(
            store.read_worker_record(&worker.worker_ref),
            Err(RuntimeError::StoreCorrupt { .. })
        ));
        assert!(matches!(
            store.read_durable_worker_record(&worker.worker_ref),
            Err(RuntimeError::StoreCorrupt { .. })
        ));
        assert_eq!(fs::read(retained_dir.join(WORKER_FILE)).unwrap(), bytes);
    }

    #[test]
    fn schema_v8_pair_migrates_to_one_worker_aggregate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let worker_id = WorkerId::now_v7();
        let worker_dir = root.join(WORKERS_DIR).join(worker_id.to_string());
        fs::create_dir_all(&worker_dir).unwrap();
        let identity_path = worker_dir.join(WORKER_FILE);
        let pair = migrate_worker_document(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &identity_path,
        )
        .unwrap();
        atomic_write_json(&identity_path, &pair.identity, "write schema-v8 identity").unwrap();
        atomic_write_json(
            &worker_dir.join(WORKER_EXECUTION_FILE),
            &pair.execution,
            "write schema-v8 execution",
        )
        .unwrap();

        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let state = opened.state.unwrap();
        let worker = state.workers.get(&worker_id).unwrap();
        assert!(matches!(
            worker.execution_state,
            PersistedWorkerExecutionState::Available(_)
        ));
        assert!(!worker_dir.join(WORKER_EXECUTION_FILE).exists());
        let aggregate: WorkerAggregateRecord =
            read_bounded_json(&identity_path, "read schema-v10 aggregate").unwrap();
        assert_eq!(aggregate.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn schema_v8_migration_fails_closed_without_execution_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime-store");
        write_empty_schema_v8_store(&root);
        let worker_id = WorkerId::now_v7();
        let worker_dir = root.join(WORKERS_DIR).join(worker_id.to_string());
        fs::create_dir_all(&worker_dir).unwrap();
        let identity_path = worker_dir.join(WORKER_FILE);
        let pair = migrate_worker_document(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &identity_path,
        )
        .unwrap();
        atomic_write_json(&identity_path, &pair.identity, "write schema-v8 identity").unwrap();

        let error = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap_err();
        assert!(matches!(error, RuntimeError::StoreMissing { .. }));
    }

    #[test]
    fn schema_v6_worker_migrates_through_v7_to_split_v8_records() {
        let worker_id = WorkerId::now_v7();
        let root = tempfile::tempdir().unwrap();
        let identity_path = root
            .path()
            .join(WORKERS_DIR)
            .join(worker_id.to_string())
            .join(WORKER_FILE);
        let migrated = migrate_worker_document(
            schema_v6_worker_document(worker_id),
            OLDEST_SCHEMA_VERSION,
            None,
            &identity_path,
        )
        .unwrap();

        assert_eq!(migrated.identity["schema_version"], PREVIOUS_SCHEMA_VERSION);
        assert!(migrated.identity.get("request").is_none());
        assert!(migrated.identity.get("execution").is_none());
        assert_eq!(
            migrated.execution["schema_version"],
            PREVIOUS_SCHEMA_VERSION
        );
        assert_eq!(
            migrated.execution["request"]["worker_id"],
            worker_id.to_string()
        );
        assert_eq!(migrated.execution["binding"], serde_json::json!({}));
        assert!(
            migrated.execution.get("last_run_generation").is_none(),
            "schema 7 removed the duplicated run generation authority"
        );
    }

    #[test]
    fn schema_v6_stopped_worker_preserves_explicit_restore_without_binding() {
        let worker_id = WorkerId::now_v7();
        let root = tempfile::tempdir().unwrap();
        let identity_path = root
            .path()
            .join(WORKERS_DIR)
            .join(worker_id.to_string())
            .join(WORKER_FILE);
        let mut source = schema_v6_worker_document(worker_id);
        source["status"] = serde_json::json!("stopped");
        source["execution"]["binding"] = serde_json::Value::Null;
        source["execution"]["restore_intent"] = serde_json::json!("explicit");

        let migrated =
            migrate_worker_document(source, OLDEST_SCHEMA_VERSION, None, &identity_path).unwrap();

        assert_eq!(migrated.identity["status"], "stopped");
        assert_eq!(migrated.execution["binding"], serde_json::Value::Null);
        assert_eq!(migrated.execution["restore_intent"], "explicit");
    }

    #[test]
    fn schema_v6_worker_rejects_mismatched_run_generation_authority() {
        let worker_id = WorkerId::now_v7();
        let mut source = schema_v6_worker_document(worker_id);
        source["execution"]["binding"]["run_generation"] = serde_json::json!(6);

        let error = migrate_worker_document(
            source,
            OLDEST_SCHEMA_VERSION,
            None,
            Path::new("worker.json"),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not match last_run_generation 7")
        );
    }

    #[test]
    fn schema_v6_runtime_store_is_migrated_in_place() {
        let root = tempfile::tempdir().unwrap();
        write_empty_schema_v8_store(root.path());
        let runtime_path = root.path().join(RUNTIME_FILE);
        let mut runtime_document: serde_json::Value =
            read_json(&runtime_path, "read test Runtime").unwrap();
        runtime_document["schema_version"] = serde_json::json!(OLDEST_SCHEMA_VERSION);
        atomic_write_json(&runtime_path, &runtime_document, "write test Runtime").unwrap();

        let worker_id = WorkerId::now_v7();
        let worker_dir = root.path().join(WORKERS_DIR).join(worker_id.to_string());
        fs::create_dir_all(&worker_dir).unwrap();
        atomic_write_json(
            &worker_dir.join(WORKER_FILE),
            &schema_v6_worker_document(worker_id.clone()),
            "write test Worker",
        )
        .unwrap();
        let orphan_history = root
            .path()
            .join(WORKERS_DIR)
            .join("legacy-orphan")
            .join("session")
            .join("history.json");
        fs::create_dir_all(orphan_history.parent().unwrap()).unwrap();
        fs::write(&orphan_history, b"[]").unwrap();

        let opened =
            FsRuntimeStore::open_or_create(root.path().to_path_buf(), "test-runtime").unwrap();

        assert_eq!(
            stored_schema_version(root.path()),
            u64::from(SCHEMA_VERSION)
        );
        assert!(
            opened
                .store
                .load_runtime_state()
                .unwrap()
                .workers
                .contains_key(&worker_id)
        );
        assert!(!worker_dir.join(WORKER_EXECUTION_FILE).exists());
        let aggregate: serde_json::Value = read_json(
            &worker_dir.join(WORKER_FILE),
            "read migrated Worker aggregate",
        )
        .unwrap();
        assert_eq!(aggregate["schema_version"], SCHEMA_VERSION);
        assert_eq!(aggregate["execution_state"]["state"], "available");
        assert_eq!(
            aggregate["execution_state"]["execution"]["binding"],
            serde_json::json!({})
        );
        assert!(
            root.path()
                .join(ORPHANED_WORKERS_DIR)
                .join("legacy-orphan")
                .join("session")
                .join("history.json")
                .is_file()
        );
    }

    #[test]
    fn schema_v7_worker_migration_splits_identity_from_execution_metadata() {
        let root = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::now_v7();
        let worker_dir = root.path().join(WORKERS_DIR).join(worker_id.to_string());
        fs::create_dir_all(&worker_dir).unwrap();
        let identity_path = worker_dir.join(WORKER_FILE);

        let migrated = migrate_worker_document(
            schema_v7_worker_document(worker_id),
            LEGACY_SCHEMA_VERSION,
            None,
            &identity_path,
        )
        .unwrap();

        assert_eq!(migrated.identity["schema_version"], PREVIOUS_SCHEMA_VERSION);
        assert_eq!(migrated.identity["status"], "running");
        assert!(migrated.identity.get("request").is_none());
        assert!(migrated.identity.get("execution").is_none());
        assert_eq!(
            migrated.execution["schema_version"],
            PREVIOUS_SCHEMA_VERSION
        );
        assert_eq!(
            migrated.execution["request"]["worker_id"],
            worker_id.to_string()
        );
        assert_eq!(migrated.execution["binding"], serde_json::json!({}));
        assert_eq!(migrated.execution["restore_intent"], "automatic");
    }

    #[test]
    fn schema_v7_worker_migration_rejects_invalid_execution_metadata() {
        let root = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::now_v7();
        let worker_dir = root.path().join(WORKERS_DIR).join(worker_id.to_string());
        fs::create_dir_all(&worker_dir).unwrap();
        let identity_path = worker_dir.join(WORKER_FILE);
        let mut source = schema_v7_worker_document(worker_id);
        source["execution"]["restore_intent"] = serde_json::json!(17);

        let error = migrate_worker_document(source, LEGACY_SCHEMA_VERSION, None, &identity_path)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("decode migrated Worker execution record")
        );
    }
}
