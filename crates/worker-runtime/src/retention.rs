//! Runtime-owned execution of Backend-resolved Worker retention dispositions.
//!
//! This boundary deliberately accepts only stable ids and resolved dispositions.
//! Host paths and provider handles never cross it.

use crate::error::RuntimeError;
use crate::identity::WorkerId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const ARCHIVE_SCHEMA_VERSION: u32 = 1;
const OPERATION_SCHEMA_VERSION: u32 = 1;
const RETENTION_LOCK: &str = ".worker-retention.lock";
static NEXT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDisposition {
    Archive,
    Purge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsDisposition {
    Purge,
    /// The Backend catalog owns the expiry. Runtime keeps only bounded stdout/stderr
    /// evidence and never mixes it into the Session archive.
    Retain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRetentionInventory {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: WorkerId,
    pub session_id: Option<String>,
    pub segment_ids: Vec<String>,
    pub session_bytes: u64,
    pub diagnostics_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeWorkerAggregateDiagnostic {
    worker_id: String,
    category: String,
    detail: String,
}

impl RuntimeWorkerAggregateDiagnostic {
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Opaque host-derived inventory snapshot. Callers can inspect but cannot
/// construct or alter the Runtime/Workspace scope used by reconciliation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkerRetentionInventorySnapshot {
    workspace_id: String,
    runtime_id: String,
    workers: Vec<WorkerRetentionInventory>,
    diagnostics: Vec<RuntimeWorkerAggregateDiagnostic>,
}

impl WorkerRetentionInventorySnapshot {
    pub(crate) fn new(
        workspace_id: String,
        runtime_id: String,
        workers: Vec<WorkerRetentionInventory>,
        diagnostics: Vec<RuntimeWorkerAggregateDiagnostic>,
    ) -> Self {
        Self {
            workspace_id,
            runtime_id,
            workers,
            diagnostics,
        }
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn workers(&self) -> &[WorkerRetentionInventory] {
        &self.workers
    }

    pub fn diagnostics(&self) -> &[RuntimeWorkerAggregateDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRetentionExecutionRequest {
    pub operation_id: String,
    pub input_fingerprint: String,
    pub archive_id: Option<String>,
    pub workspace_id: String,
    pub source_runtime_id: String,
    pub worker_id: WorkerId,
    pub source_created_at: String,
    pub removed_at: String,
    pub effective_profile: Option<String>,
    pub retention_class: Option<String>,
    pub policy_id: String,
    pub session_disposition: SessionDisposition,
    pub diagnostics_disposition: DiagnosticsDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSessionArchiveManifest {
    pub schema_version: u32,
    pub archive_id: String,
    pub workspace_id: String,
    pub source_runtime_id: String,
    pub source_worker_id: WorkerId,
    pub source_session_id: String,
    /// Older archives have no attested active lineage and cannot serve canonical scans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_active_segment_id: Option<session_store::SegmentId>,
    pub segment_ids: Vec<String>,
    pub source_created_at: String,
    pub removed_at: String,
    pub archived_at_unix_seconds: u64,
    pub effective_profile: Option<String>,
    pub retention_class: Option<String>,
    pub content_checksum_sha256: String,
    pub content_bytes: u64,
    pub content_file_count: u64,
    pub policy_id: String,
    pub operation_id: String,
    pub input_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRetentionExecutionResult {
    pub operation_id: String,
    pub input_fingerprint: String,
    pub worker_id: WorkerId,
    pub session_disposition: SessionDisposition,
    pub diagnostics_disposition: DiagnosticsDisposition,
    pub archive: Option<WorkerSessionArchiveManifest>,
    pub source_removed: bool,
    pub diagnostics_retained: bool,
}

pub(crate) trait WorkerRetentionProvider: Send + Sync {
    fn inventory(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: WorkerId,
    ) -> Result<WorkerRetentionInventory, RuntimeError>;

    fn execute(
        &self,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, RuntimeError>;
}

pub use crate::session_retention::runtime_session_retention;

/// Prove the legitimate pre-publication state, rather than interpreting a missing
/// source as empty. A committed active pointer or any source file defeats this proof.
pub(crate) fn unmaterialized_session(
    worker_dir: &Path,
    workspace_id: &str,
) -> Result<Option<session_store::SessionId>, RuntimeError> {
    let metadata_path = worker_dir.join("metadata.json");
    let bytes = match fs::read(&metadata_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RuntimeError::StoreIo {
                operation: "inspect pending Session",
                path: metadata_path,
                source,
            });
        }
    };
    let metadata: session_store::WorkerMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| RuntimeError::InvalidRequest("Pending Session metadata is invalid".into()))?;
    if metadata.workspace_id.as_deref() != Some(workspace_id) {
        return Ok(None);
    }
    let Some(active) = metadata.active.filter(|active| active.segment_id.is_none()) else {
        return Ok(None);
    };
    let root = worker_dir.join("session");
    if collect_files(&root, "inspect pending Session")?.is_empty() {
        Ok(Some(active.session_id))
    } else {
        Ok(None)
    }
}

pub(crate) fn aggregate_session_id(
    worker_dir: &Path,
) -> Result<Option<session_store::SessionId>, RuntimeError> {
    let session_path = worker_dir.join("session/session.json");
    let saved = match fs::read(&session_path) {
        Ok(bytes) => {
            let manifest: CanonicalSessionManifest =
                serde_json::from_slice(&bytes).map_err(|_| {
                    RuntimeError::InvalidRequest("Retained Session manifest is invalid".into())
                })?;
            Some(
                manifest
                    .session_id
                    .parse::<session_store::SessionId>()
                    .map_err(|_| {
                        RuntimeError::InvalidRequest("Retained Session identity is invalid".into())
                    })?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(RuntimeError::StoreIo {
                operation: "read retained Session identity",
                path: session_path,
                source,
            });
        }
    };
    let metadata_path = worker_dir.join("metadata.json");
    let active = match fs::read(&metadata_path) {
        Ok(bytes) => {
            let metadata: session_store::WorkerMetadata =
                serde_json::from_slice(&bytes).map_err(|_| {
                    RuntimeError::InvalidRequest("Retained Worker metadata is invalid".into())
                })?;
            metadata.active.map(|active| active.session_id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(RuntimeError::StoreIo {
                operation: "read retained Session pointer",
                path: metadata_path,
                source,
            });
        }
    };
    if let (Some(saved), Some(active)) = (saved, active) {
        if saved != active {
            return Err(RuntimeError::InvalidRequest(
                "Retained Session manifest and active pointer disagree".into(),
            ));
        }
    }
    if saved.is_none() && active.is_none() {
        let session_dir = worker_dir.join("session");
        if session_dir.is_dir()
            && !collect_files(&session_dir, "inspect uninitialized Session")?.is_empty()
        {
            return Err(RuntimeError::InvalidRequest(
                "Session material has no retained identity".into(),
            ));
        }
    }
    Ok(saved.or(active))
}

/// Called only while the common Session deletion lock is held. Include staged
/// archives from interrupted transfers: erasure must not leave a second original.
pub(crate) fn remove_archived_session_copies(
    runtime_root: &Path,
    session: session_store::SessionId,
) -> Result<(), session_store::StoreError> {
    let archives = runtime_root.join("archives/workers");
    let entries = match fs::read_dir(archives) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let session_root = entry.path().join("session");
        let owner = match fs::read(entry.path().join("manifest.json")) {
            Ok(bytes) => Some(serde_json::from_slice::<WorkerSessionArchiveManifest>(
                &bytes,
            )?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(owner) = owner {
            if owner.source_session_id == session.to_string() {
                erase_archived_diagnostics(runtime_root, &owner)?;
                match fs::remove_dir_all(&session_root) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
            continue;
        }
        let bytes = match fs::read(session_root.join("session.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let manifest: CanonicalSessionManifest = serde_json::from_slice(&bytes)?;
        if manifest.session_id == session.to_string() {
            fs::remove_dir_all(&session_root)?;
        }
    }
    Ok(())
}

/// Erasure includes diagnostic originals of the same archived removal operation.
/// The immutable removal receipt is historical evidence and is never rewritten.
/// Staging names are bound to that exact operation; unrelated archives are untouched.
fn erase_archived_diagnostics(
    runtime_root: &Path,
    archive: &WorkerSessionArchiveManifest,
) -> Result<(), session_store::StoreError> {
    let invalid = || {
        session_store::StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Archived diagnostics erasure identity mismatch",
        ))
    };
    if archive.schema_version != ARCHIVE_SCHEMA_VERSION {
        return Err(invalid());
    }
    validate_id("operation_id", &archive.operation_id).map_err(|_| invalid())?;
    let receipt_path = runtime_root
        .join("retention/operations")
        .join(format!("{}.json", archive.operation_id));
    if let Some(receipt) = read_operation_receipt(&receipt_path).map_err(|_| invalid())? {
        if receipt.request.workspace_id != archive.workspace_id
            || receipt.request.source_runtime_id != archive.source_runtime_id
            || receipt.request.worker_id != archive.source_worker_id
            || receipt.request.operation_id != archive.operation_id
            || receipt.request.input_fingerprint != archive.input_fingerprint
            || receipt.request.archive_id.as_ref() != Some(&archive.archive_id)
            || receipt.result.archive.as_ref() != Some(archive)
        {
            return Err(invalid());
        }
    }
    let parent = runtime_root.join("archives/diagnostics");
    let entries = match fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let staging_prefix = format!(".{}.tmp-", archive.operation_id);
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let staged = name.strip_prefix(&staging_prefix).is_some_and(|suffix| {
            suffix.split_once('-').is_some_and(|(pid, sequence)| {
                !pid.is_empty()
                    && !sequence.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
                    && sequence.bytes().all(|b| b.is_ascii_digit())
            })
        });
        if name != archive.operation_id && !staged {
            continue;
        }
        if !entry.file_type()?.is_dir() {
            return Err(invalid());
        }
        let manifest = match fs::read(entry.path().join("manifest.json")) {
            Ok(bytes) => Some(serde_json::from_slice::<DiagnosticsArchiveManifest>(
                &bytes,
            )?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && staged => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(manifest) = manifest {
            if manifest.schema_version != ARCHIVE_SCHEMA_VERSION
                || manifest.workspace_id != archive.workspace_id
                || manifest.source_runtime_id != archive.source_runtime_id
                || manifest.source_worker_id != archive.source_worker_id
                || manifest.operation_id != archive.operation_id
                || manifest.input_fingerprint != archive.input_fingerprint
            {
                return Err(invalid());
            }
        }
        fs::remove_dir_all(entry.path())?;
    }
    sync_directory(&parent, "erase archived Worker diagnostics").map_err(|_| invalid())?;
    Ok(())
}

pub(crate) fn retention_error(error: session_store::StoreError) -> RuntimeError {
    RuntimeError::InvalidRequest(format!("Session retention rejected operation: {error}"))
}

/// Direct Worker deletion cannot bypass pins. Purge policy is not erase intent.
pub(crate) fn delete_worker_aggregate(
    runtime_root: &Path,
    worker_dir: &Path,
    intent: session_store::retention::SessionDeletionIntent,
) -> Result<(), RuntimeError> {
    let session = aggregate_session_id(worker_dir)?;
    let retention = runtime_session_retention(runtime_root);
    // Ordinary policy never creates erase authority. A pre-existing permanent
    // tombstone is an irrevocable decision whose physical cleanup may be resumed.
    let intent = match session
        .map(|session| retention.ensure_live(session))
        .transpose()
    {
        Err(session_store::StoreError::SessionErased(_)) => {
            session_store::retention::SessionDeletionIntent::ErasePermanently
        }
        Err(error) => return Err(retention_error(error)),
        Ok(_) => intent,
    };
    let remove = || -> Result<(), session_store::StoreError> {
        if intent == session_store::retention::SessionDeletionIntent::ErasePermanently {
            if let Some(session) = session {
                remove_archived_session_copies(runtime_root, session)?;
            }
        }
        match fs::remove_dir_all(worker_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    };
    if let Some(session) = session {
        runtime_session_retention(runtime_root)
            .delete(session, intent, remove)
            .map_err(retention_error)
    } else {
        remove().map_err(retention_error)
    }
}

/// Filesystem provider for the canonical Runtime Worker aggregate.
#[derive(Clone, Debug)]
pub(crate) struct FsWorkerRetentionProvider {
    runtime_root: PathBuf,
}

impl FsWorkerRetentionProvider {
    pub(crate) fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub(crate) fn completed_for(
        &self,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<Option<WorkerRetentionExecutionResult>, RuntimeError> {
        let path = self.operation_path(&request.operation_id)?;
        let Some(receipt) = read_operation_receipt(&path)? else {
            return Ok(None);
        };
        validate_receipt_request(&receipt, request)?;
        Ok(receipt.result.source_removed.then_some(receipt.result))
    }

    pub(crate) fn snapshot(
        &self,
        workspace_id: &str,
        runtime_id: &str,
    ) -> Result<WorkerRetentionInventorySnapshot, RuntimeError> {
        fs::create_dir_all(&self.runtime_root).map_err(|source| RuntimeError::StoreIo {
            operation: "prepare Worker retention inventory",
            path: self.runtime_root.clone(),
            source,
        })?;
        let lock_path = self.runtime_root.join(RETENTION_LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| RuntimeError::StoreIo {
                operation: "lock Worker retention inventory",
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| RuntimeError::StoreIo {
            operation: "lock Worker retention inventory",
            path: lock_path,
            source,
        })?;
        let workers_root = self.runtime_root.join("workers");
        if !workers_root.is_dir() {
            return Ok(WorkerRetentionInventorySnapshot::new(
                workspace_id.to_string(),
                runtime_id.to_string(),
                Vec::new(),
                Vec::new(),
            ));
        }
        let mut entries = fs::read_dir(&workers_root)
            .map_err(|source| RuntimeError::StoreIo {
                operation: "scan Worker retention inventory",
                path: workers_root.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| RuntimeError::StoreIo {
                operation: "scan Worker retention inventory",
                path: workers_root.clone(),
                source,
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut workers = Vec::new();
        let mut diagnostics = Vec::new();
        for entry in entries {
            let raw_id = entry.file_name().to_string_lossy().to_string();
            let bounded_id = bounded_diagnostic_id(&raw_id);
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    diagnostics.push(runtime_aggregate_diagnostic(
                        &bounded_id,
                        "aggregate_metadata_unreadable",
                    ));
                    continue;
                }
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                diagnostics.push(runtime_aggregate_diagnostic(
                    &bounded_id,
                    "aggregate_entry_unsupported",
                ));
                continue;
            }
            let Ok(worker_id) = raw_id.parse::<WorkerId>() else {
                diagnostics.push(runtime_aggregate_diagnostic(
                    &bounded_id,
                    "aggregate_worker_id_invalid",
                ));
                continue;
            };
            let worker_dir = self.worker_dir(worker_id);
            let snapshot: WorkerAggregateSnapshot = match read_json(
                &worker_dir.join("worker.json"),
                "scan Worker retention inventory",
            ) {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    diagnostics.push(runtime_aggregate_diagnostic(
                        &bounded_id,
                        "aggregate_worker_record_corrupt",
                    ));
                    continue;
                }
            };
            if snapshot.workspace_id.as_deref() != Some(workspace_id) {
                diagnostics.push(runtime_aggregate_diagnostic(
                    &bounded_id,
                    "aggregate_workspace_mismatch",
                ));
                continue;
            }
            match self.inventory(workspace_id, runtime_id, worker_id) {
                Ok(item) => workers.push(item),
                Err(_) => diagnostics.push(runtime_aggregate_diagnostic(
                    &bounded_id,
                    "aggregate_session_inventory_failed",
                )),
            }
        }
        workers.sort_by_key(|item| item.worker_id);
        diagnostics.sort_by(|left, right| {
            (&left.worker_id, &left.category).cmp(&(&right.worker_id, &right.category))
        });
        Ok(WorkerRetentionInventorySnapshot::new(
            workspace_id.to_string(),
            runtime_id.to_string(),
            workers,
            diagnostics,
        ))
    }

    pub(crate) fn recover_after_source_removal(
        &self,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, RuntimeError> {
        if self.worker_dir(request.worker_id).exists() {
            return Err(RuntimeError::WorkerNotFound {
                worker_id: request.worker_id,
            });
        }
        self.execute(request)
    }

    fn worker_dir(&self, worker_id: WorkerId) -> PathBuf {
        self.runtime_root
            .join("workers")
            .join(worker_id.to_string())
    }

    fn operation_path(&self, operation_id: &str) -> Result<PathBuf, RuntimeError> {
        validate_id("operation_id", operation_id)?;
        Ok(self
            .runtime_root
            .join("retention")
            .join("operations")
            .join(format!("{operation_id}.json")))
    }

    fn archive_dir(&self, archive_id: &str) -> Result<PathBuf, RuntimeError> {
        validate_id("archive_id", archive_id)?;
        Ok(self
            .runtime_root
            .join("archives")
            .join("workers")
            .join(archive_id))
    }

    pub(crate) fn session_archive(
        &self,
        archive_id: &str,
    ) -> Result<(WorkerSessionArchiveManifest, PathBuf), RuntimeError> {
        let (manifest, session_root) = self.session_archive_identity(archive_id)?;
        let archive_dir = self.archive_dir(archive_id)?;
        let manifest = validate_existing_archive(&archive_dir, &manifest, false)?;
        Ok((manifest, session_root))
    }

    pub(crate) fn session_archive_identity(
        &self,
        archive_id: &str,
    ) -> Result<(WorkerSessionArchiveManifest, PathBuf), RuntimeError> {
        let archive_dir = self.archive_dir(archive_id)?;
        let manifest: WorkerSessionArchiveManifest = read_json(
            &archive_dir.join("manifest.json"),
            "read Worker Session archive",
        )?;
        if manifest.schema_version != ARCHIVE_SCHEMA_VERSION || manifest.archive_id != archive_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read Worker Session archive",
                path: archive_dir.join("manifest.json"),
                message: "archive manifest identity mismatch".to_string(),
            });
        }
        Ok((manifest, archive_dir.join("session")))
    }

    fn diagnostics_dir(&self, operation_id: &str) -> Result<PathBuf, RuntimeError> {
        validate_id("operation_id", operation_id)?;
        Ok(self
            .runtime_root
            .join("archives")
            .join("diagnostics")
            .join(operation_id))
    }
}

impl WorkerRetentionProvider for FsWorkerRetentionProvider {
    fn inventory(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: WorkerId,
    ) -> Result<WorkerRetentionInventory, RuntimeError> {
        let worker_dir = self.worker_dir(worker_id);
        if !worker_dir.is_dir() {
            return Err(RuntimeError::WorkerNotFound { worker_id });
        }
        let worker: WorkerAggregateSnapshot = read_json(
            &worker_dir.join("worker.json"),
            "inventory Worker retention",
        )?;
        if worker.workspace_id.as_deref() != Some(workspace_id) {
            return Err(RuntimeError::WorkerNotFound { worker_id });
        }
        let session_dir = worker_dir.join("session");
        let (session_id, segment_ids, session_bytes) =
            if unmaterialized_session(&worker_dir, workspace_id)?.is_some() {
                (None, Vec::new(), 0)
            } else if session_dir.is_dir() {
                let manifest: CanonicalSessionManifest = read_json(
                    &session_dir.join("session.json"),
                    "inventory Worker retention",
                )?;
                let files = collect_files(&session_dir, "inventory Worker retention")?;
                let mut segment_ids = BTreeSet::new();
                let mut bytes = 0_u64;
                for (relative, path) in files {
                    bytes = bytes.saturating_add(file_len(&path, "inventory Worker retention")?);
                    if let Some(name) = relative.file_name().and_then(|value| value.to_str()) {
                        let segment = name
                            .strip_suffix(".trace.jsonl")
                            .or_else(|| name.strip_suffix(".jsonl"));
                        if let Some(segment) = segment {
                            segment_ids.insert(segment.to_string());
                        }
                    }
                }
                (
                    Some(manifest.session_id),
                    segment_ids.into_iter().collect(),
                    bytes,
                )
            } else {
                (None, Vec::new(), 0)
            };
        let diagnostics_bytes = diagnostics_files(&worker_dir, "inventory Worker retention")?
            .into_iter()
            .try_fold(0_u64, |total, path| {
                file_len(&path, "inventory Worker retention").map(|size| total.saturating_add(size))
            })?;
        Ok(WorkerRetentionInventory {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            worker_id,
            session_id,
            segment_ids,
            session_bytes,
            diagnostics_bytes,
        })
    }

    fn execute(
        &self,
        request: &WorkerRetentionExecutionRequest,
    ) -> Result<WorkerRetentionExecutionResult, RuntimeError> {
        validate_request(request)?;
        fs::create_dir_all(&self.runtime_root).map_err(|source| RuntimeError::StoreIo {
            operation: "prepare Worker retention",
            path: self.runtime_root.clone(),
            source,
        })?;
        let lock_path = self.runtime_root.join(RETENTION_LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| RuntimeError::StoreIo {
                operation: "lock Worker retention",
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| RuntimeError::StoreIo {
            operation: "lock Worker retention",
            path: lock_path,
            source,
        })?;

        let pending = read_operation_receipt(&self.operation_path(&request.operation_id)?)?;
        if let Some(receipt) = &pending {
            validate_receipt_request(receipt, request)?;
            if receipt.result.source_removed {
                return Ok(receipt.result.clone());
            }
        }

        let worker_dir = self.worker_dir(request.worker_id);
        if !worker_dir.is_dir() {
            if let Some(mut receipt) = pending {
                // A prior attempt durably committed all disposition evidence and
                // removed the aggregate, then stopped before finalizing its receipt.
                receipt.result.source_removed = true;
                atomic_write_json(
                    &self.operation_path(&request.operation_id)?,
                    &receipt,
                    "recover Worker retention receipt",
                )?;
                return Ok(receipt.result);
            }
            return Err(RuntimeError::WorkerNotFound {
                worker_id: request.worker_id,
            });
        }
        let snapshot: WorkerAggregateSnapshot =
            read_json(&worker_dir.join("worker.json"), "execute Worker retention")?;
        if snapshot.workspace_id.as_deref() != Some(request.workspace_id.as_str()) {
            return Err(RuntimeError::WorkerNotFound {
                worker_id: request.worker_id,
            });
        }
        // Transfer keeps the same Session pin/fence namespace. Hold its lock
        // across verified archive publication and aggregate removal. A policy
        // purge instead uses ordinary deletion and must reject outstanding pins.
        let retention = runtime_session_retention(&self.runtime_root);
        let transfer_guard = if request.session_disposition == SessionDisposition::Archive {
            aggregate_session_id(&worker_dir)?
                .map(|session| retention.lock_live(session))
                .transpose()
                .map_err(retention_error)?
        } else {
            None
        };
        let archive = match request.session_disposition {
            SessionDisposition::Archive => {
                Some(commit_session_archive(self, request, &worker_dir)?)
            }
            SessionDisposition::Purge => None,
        };
        if let Some(manifest) = &archive {
            if let Some(active_segment) = manifest.source_active_segment_id {
                // We already hold the Session transfer lock. Do not reacquire it
                // through the external-retention constructor during verification.
                let reader =
                    session_store::public_material::SessionPublicMaterialReader::open_worker(
                        &self.archive_dir(&manifest.archive_id)?.join("session"),
                        active_segment,
                        Default::default(),
                    )
                    .map_err(|error| {
                        RuntimeError::InvalidRequest(format!(
                            "Archived canonical source is unreadable: {error}"
                        ))
                    })?;
                if reader.session_id().to_string() != manifest.source_session_id {
                    return Err(RuntimeError::InvalidRequest(
                        "Archived canonical source identity mismatch".into(),
                    ));
                }
            }
        }
        let diagnostics_retained = match request.diagnostics_disposition {
            DiagnosticsDisposition::Purge => false,
            DiagnosticsDisposition::Retain => {
                commit_diagnostics_archive(self, request, &worker_dir)?;
                true
            }
        };

        let mut result = WorkerRetentionExecutionResult {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            worker_id: request.worker_id,
            session_disposition: request.session_disposition,
            diagnostics_disposition: request.diagnostics_disposition,
            archive,
            source_removed: false,
            diagnostics_retained,
        };
        let mut receipt = RetentionOperationReceipt {
            schema_version: OPERATION_SCHEMA_VERSION,
            request: request.clone(),
            result: result.clone(),
        };
        // Pending receipt makes the delete/final-receipt crash window
        // recoverable without treating an uncommitted archive as completion.
        atomic_write_json(
            &self.operation_path(&request.operation_id)?,
            &receipt,
            "commit pending Worker retention receipt",
        )?;

        match request.session_disposition {
            SessionDisposition::Archive => {
                // commit_session_archive already compared all original bytes.
                // The transfer lock remains held until removal has completed.
                fs::remove_dir_all(&worker_dir).map_err(|source| RuntimeError::StoreIo {
                    operation: "remove retained Worker aggregate",
                    path: worker_dir.clone(),
                    source,
                })?;
            }
            SessionDisposition::Purge => delete_worker_aggregate(
                &self.runtime_root,
                &worker_dir,
                session_store::retention::SessionDeletionIntent::Ordinary,
            )?,
        }
        drop(transfer_guard);
        sync_directory(
            worker_dir.parent().unwrap_or(&self.runtime_root),
            "remove retained Worker aggregate",
        )?;

        result.source_removed = true;
        receipt.result = result.clone();
        atomic_write_json(
            &self.operation_path(&request.operation_id)?,
            &receipt,
            "commit Worker retention receipt",
        )?;
        Ok(result)
    }
}

#[derive(Deserialize)]
struct WorkerAggregateSnapshot {
    #[serde(default)]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct CanonicalSessionManifest {
    session_id: String,
}

#[derive(Serialize, Deserialize)]
struct RetentionOperationReceipt {
    schema_version: u32,
    request: WorkerRetentionExecutionRequest,
    result: WorkerRetentionExecutionResult,
}

fn read_operation_receipt(path: &Path) -> Result<Option<RetentionOperationReceipt>, RuntimeError> {
    if !path.is_file() {
        return Ok(None);
    }
    let receipt: RetentionOperationReceipt = read_json(path, "read Worker retention receipt")?;
    if receipt.schema_version != OPERATION_SCHEMA_VERSION {
        return Err(RuntimeError::StoreCorrupt {
            operation: "read Worker retention receipt",
            path: path.to_path_buf(),
            message: format!(
                "unsupported operation receipt schema {}",
                receipt.schema_version
            ),
        });
    }
    Ok(Some(receipt))
}

fn validate_receipt_request(
    receipt: &RetentionOperationReceipt,
    request: &WorkerRetentionExecutionRequest,
) -> Result<(), RuntimeError> {
    if &receipt.request != request
        || receipt.result.operation_id != request.operation_id
        || receipt.result.input_fingerprint != request.input_fingerprint
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "retention operation {} was already used with different input",
            request.operation_id
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DiagnosticsArchiveManifest {
    schema_version: u32,
    operation_id: String,
    workspace_id: String,
    source_runtime_id: String,
    source_worker_id: WorkerId,
    input_fingerprint: String,
    content_checksum_sha256: String,
    content_bytes: u64,
    content_file_count: u64,
}

fn runtime_aggregate_diagnostic(
    worker_id: &str,
    category: &str,
) -> RuntimeWorkerAggregateDiagnostic {
    RuntimeWorkerAggregateDiagnostic {
        worker_id: worker_id.to_string(),
        category: category.to_string(),
        detail: "Canonical Runtime Worker aggregate requires diagnostic reconciliation".to_string(),
    }
}

fn bounded_diagnostic_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn validate_request(request: &WorkerRetentionExecutionRequest) -> Result<(), RuntimeError> {
    validate_id("operation_id", &request.operation_id)?;
    validate_id("workspace_id", &request.workspace_id)?;
    validate_id("source_runtime_id", &request.source_runtime_id)?;
    validate_id("policy_id", &request.policy_id)?;
    if request.input_fingerprint.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "retention input fingerprint must not be empty".to_string(),
        ));
    }
    match (request.session_disposition, request.archive_id.as_deref()) {
        (SessionDisposition::Archive, Some(id)) => validate_id("archive_id", id),
        (SessionDisposition::Archive, None) => Err(RuntimeError::InvalidRequest(
            "archive disposition requires archive_id".to_string(),
        )),
        (SessionDisposition::Purge, None) => Ok(()),
        (SessionDisposition::Purge, Some(_)) => Err(RuntimeError::InvalidRequest(
            "purge disposition must not include archive_id".to_string(),
        )),
    }
}

fn validate_id(kind: &str, value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 160
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "invalid retention {kind}"
        )));
    }
    Ok(())
}

fn commit_session_archive(
    provider: &FsWorkerRetentionProvider,
    request: &WorkerRetentionExecutionRequest,
    worker_dir: &Path,
) -> Result<WorkerSessionArchiveManifest, RuntimeError> {
    let archive_id = request.archive_id.as_deref().ok_or_else(|| {
        RuntimeError::InvalidRequest("archive disposition requires archive_id".to_string())
    })?;
    let session_dir = worker_dir.join("session");
    let pending = unmaterialized_session(worker_dir, &request.workspace_id)?;
    let session: CanonicalSessionManifest = if let Some(session) = pending {
        CanonicalSessionManifest {
            session_id: session.to_string(),
        }
    } else {
        read_json(&session_dir.join("session.json"), "archive Worker Session")?
    };
    let source_files = collect_files(&session_dir, "archive Worker Session")?;
    let (checksum, bytes, count) = checksum_files(&source_files, "archive Worker Session")?;
    let mut segment_ids = BTreeSet::new();
    for (relative, _) in &source_files {
        if let Some(name) = relative.file_name().and_then(|name| name.to_str()) {
            if let Some(segment) = name
                .strip_suffix(".trace.jsonl")
                .or_else(|| name.strip_suffix(".jsonl"))
            {
                segment_ids.insert(segment.to_string());
            }
        }
    }
    let manifest = WorkerSessionArchiveManifest {
        schema_version: ARCHIVE_SCHEMA_VERSION,
        archive_id: archive_id.to_string(),
        workspace_id: request.workspace_id.clone(),
        source_runtime_id: request.source_runtime_id.clone(),
        source_worker_id: request.worker_id,
        source_active_segment_id: if worker_dir.join("metadata.json").is_file() {
            let metadata: session_store::WorkerMetadata = read_json(
                &worker_dir.join("metadata.json"),
                "archive active Session pointer",
            )?;
            let active = metadata.active.ok_or_else(|| {
                RuntimeError::InvalidRequest("Worker has no active Session pointer".into())
            })?;
            if active.session_id.to_string() != session.session_id
                || metadata.workspace_id.as_deref() != Some(&request.workspace_id)
            {
                return Err(RuntimeError::InvalidRequest(
                    "Archive Session pointer scope mismatch".into(),
                ));
            }
            active.segment_id
        } else {
            None
        },
        source_session_id: session.session_id,
        segment_ids: segment_ids.into_iter().collect(),
        source_created_at: request.source_created_at.clone(),
        removed_at: request.removed_at.clone(),
        archived_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        effective_profile: request.effective_profile.clone(),
        retention_class: request.retention_class.clone(),
        content_checksum_sha256: checksum,
        content_bytes: bytes,
        content_file_count: count,
        policy_id: request.policy_id.clone(),
        operation_id: request.operation_id.clone(),
        input_fingerprint: request.input_fingerprint.clone(),
    };
    let archive_dir = provider.archive_dir(archive_id)?;
    if archive_dir.exists() {
        return validate_existing_archive(&archive_dir, &manifest, true);
    }
    let parent = archive_dir
        .parent()
        .ok_or_else(|| RuntimeError::StoreCorrupt {
            operation: "archive Worker Session",
            path: archive_dir.clone(),
            message: "archive target has no parent".to_string(),
        })?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
        operation: "archive Worker Session",
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = temporary_path(&archive_dir);
    let result = (|| {
        fs::create_dir(&staging).map_err(|source| RuntimeError::StoreIo {
            operation: "archive Worker Session",
            path: staging.clone(),
            source,
        })?;
        // Persist source ownership before copying any original bytes. An
        // interrupted staging tree remains attributable to explicit erasure.
        atomic_write_json(
            &staging.join("manifest.json"),
            &manifest,
            "archive Worker Session",
        )?;
        let target_session = staging.join("session");
        fs::create_dir(&target_session).map_err(|source| RuntimeError::StoreIo {
            operation: "archive Worker Session",
            path: target_session.clone(),
            source,
        })?;
        copy_files(&source_files, &target_session, "archive Worker Session")?;
        sync_tree(&staging, "archive Worker Session")?;
        fs::rename(&staging, &archive_dir).map_err(|source| RuntimeError::StoreIo {
            operation: "archive Worker Session",
            path: archive_dir.clone(),
            source,
        })?;
        sync_directory(parent, "archive Worker Session")?;
        validate_existing_archive(&archive_dir, &manifest, true)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

// The public archive manifest is an immutable ingestion receipt also retained by
// Backend. Schema migration replaces the payload, not that receipt. This local
// storage attestation binds the new bytes to the exact verified ingestion receipt.
// It is published only with the entire staged Runtime tree, never by a reader.
#[derive(Serialize, Deserialize)]
struct MigratedArchiveStorage {
    schema_version: u32,
    ingestion: WorkerSessionArchiveManifest,
    checksum_sha256: String,
    bytes: u64,
    file_count: u64,
}

const ARCHIVE_STORAGE_FILE: &str = "session-storage.json";

fn archive_storage(
    archive_dir: &Path,
    ingestion: &WorkerSessionArchiveManifest,
) -> Result<Option<MigratedArchiveStorage>, RuntimeError> {
    let path = archive_dir.join(ARCHIVE_STORAGE_FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            let storage: MigratedArchiveStorage =
                serde_json::from_slice(&bytes).map_err(|error| RuntimeError::StoreCorrupt {
                    operation: "read migrated archive storage",
                    path: path.clone(),
                    message: error.to_string(),
                })?;
            if storage.schema_version != 1 || &storage.ingestion != ingestion {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read migrated archive storage",
                    path,
                    message: "archive storage attestation does not match ingestion receipt".into(),
                });
            }
            Ok(Some(storage))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RuntimeError::StoreIo {
            operation: "read migrated archive storage",
            path,
            source,
        }),
    }
}

/// Called only on the private Runtime migration staging tree under its owner
/// lock. Failure discards the whole tree, including manifests and attestations.
pub(crate) fn migrate_staged_sessions(root: &Path) -> Result<(), RuntimeError> {
    for directory in ["workers", "orphaned-workers", "archives/workers"] {
        let directory = root.join(directory);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(RuntimeError::StoreIo {
                    operation: "enumerate Session migration",
                    path: directory,
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| RuntimeError::StoreIo {
                operation: "enumerate Session migration",
                path: directory.clone(),
                source,
            })?;
            let aggregate = entry.path();
            let session = aggregate.join("session");
            if !session
                .join("session.json")
                .try_exists()
                .map_err(|source| RuntimeError::StoreIo {
                    operation: "inspect Session migration",
                    path: session.clone(),
                    source,
                })?
            {
                continue;
            }
            let identity: CanonicalSessionManifest = read_json(
                &session.join("session.json"),
                "read Session migration identity",
            )?;
            let session_id = identity.session_id.parse().map_err(|_| {
                RuntimeError::InvalidRequest("Session migration identity is invalid".into())
            })?;
            match runtime_session_retention(root).ensure_live(session_id) {
                Ok(()) => {}
                // Erasure fences remain authoritative even if physical removal
                // was interrupted. Do not inspect or migrate fenced originals.
                Err(session_store::StoreError::SessionErased(_)) => continue,
                Err(error) => return Err(retention_error(error)),
            }
            let archive = if directory == root.join("archives/workers") {
                let manifest: WorkerSessionArchiveManifest = read_json(
                    &aggregate.join("manifest.json"),
                    "read archive before Session migration",
                )?;
                Some(validate_existing_archive(&aggregate, &manifest, false)?)
            } else {
                None
            };
            session_store::WorkerSessionStore::new(&session).map_err(|error| {
                RuntimeError::StoreCorrupt {
                    operation: "migrate Session schema",
                    path: session.clone(),
                    message: error.to_string(),
                }
            })?;
            if let Some(ingestion) = archive {
                let (checksum_sha256, bytes, file_count) = checksum_files(
                    &collect_files(&session, "migrate archive storage")?,
                    "migrate archive storage",
                )?;
                atomic_write_json(
                    &aggregate.join(ARCHIVE_STORAGE_FILE),
                    &MigratedArchiveStorage {
                        schema_version: 1,
                        ingestion,
                        checksum_sha256,
                        bytes,
                        file_count,
                    },
                    "commit migrated archive storage",
                )?;
            }
        }
    }
    Ok(())
}

fn validate_existing_archive(
    archive_dir: &Path,
    expected: &WorkerSessionArchiveManifest,
    require_current_source: bool,
) -> Result<WorkerSessionArchiveManifest, RuntimeError> {
    let existing: WorkerSessionArchiveManifest = read_json(
        &archive_dir.join("manifest.json"),
        "verify Worker Session archive",
    )?;
    let storage = archive_storage(archive_dir, &existing)?;
    let mut comparable_expected = expected.clone();
    comparable_expected.archived_at_unix_seconds = existing.archived_at_unix_seconds;
    let mut current = existing.clone();
    if let Some(storage) = &storage {
        current.content_checksum_sha256 = storage.checksum_sha256.clone();
        current.content_bytes = storage.bytes;
        current.content_file_count = storage.file_count;
    }
    // Reads use the original receipt; a retry with a migrated live source uses
    // the current storage summary. All other identity/policy fields must agree.
    let expected_manifest = if require_current_source {
        &current
    } else {
        &existing
    };
    if expected_manifest != &comparable_expected {
        return Err(RuntimeError::StoreCorrupt {
            operation: "verify Worker Session archive",
            path: archive_dir.join("manifest.json"),
            message: "archive id collision or manifest mismatch".to_string(),
        });
    }
    let files = collect_files(
        &archive_dir.join("session"),
        "verify Worker Session archive",
    )?;
    let (checksum, bytes, count) = checksum_files(&files, "verify Worker Session archive")?;
    if checksum != current.content_checksum_sha256
        || bytes != current.content_bytes
        || count != current.content_file_count
    {
        return Err(RuntimeError::StoreCorrupt {
            operation: "verify Worker Session archive",
            path: archive_dir.to_path_buf(),
            message: "archive checksum or content summary mismatch".to_string(),
        });
    }
    Ok(existing)
}

fn commit_diagnostics_archive(
    provider: &FsWorkerRetentionProvider,
    request: &WorkerRetentionExecutionRequest,
    worker_dir: &Path,
) -> Result<(), RuntimeError> {
    let files = diagnostics_files(worker_dir, "archive Worker diagnostics")?;
    let target = provider.diagnostics_dir(&request.operation_id)?;
    if target.exists() {
        validate_existing_diagnostics_archive(&target, request)?;
        return Ok(());
    }
    let parent = target.parent().ok_or_else(|| RuntimeError::StoreCorrupt {
        operation: "archive Worker diagnostics",
        path: target.clone(),
        message: "diagnostics archive target has no parent".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
        operation: "archive Worker diagnostics",
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = temporary_path(&target);
    let result = (|| {
        fs::create_dir(&staging).map_err(|source| RuntimeError::StoreIo {
            operation: "archive Worker diagnostics",
            path: staging.clone(),
            source,
        })?;
        let source_files = files
            .iter()
            .map(|path| {
                let relative =
                    path.strip_prefix(worker_dir)
                        .map_err(|_| RuntimeError::StoreCorrupt {
                            operation: "archive Worker diagnostics",
                            path: path.clone(),
                            message: "diagnostics path escaped Worker aggregate".to_string(),
                        })?;
                Ok((relative.to_path_buf(), path.clone()))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        copy_files(&source_files, &staging, "archive Worker diagnostics")?;
        let (checksum, bytes, count) = checksum_files(&source_files, "archive Worker diagnostics")?;
        let manifest = DiagnosticsArchiveManifest {
            schema_version: ARCHIVE_SCHEMA_VERSION,
            operation_id: request.operation_id.clone(),
            workspace_id: request.workspace_id.clone(),
            source_runtime_id: request.source_runtime_id.clone(),
            source_worker_id: request.worker_id,
            input_fingerprint: request.input_fingerprint.clone(),
            content_checksum_sha256: checksum,
            content_bytes: bytes,
            content_file_count: count,
        };
        atomic_write_json(
            &staging.join("manifest.json"),
            &manifest,
            "archive Worker diagnostics",
        )?;
        sync_tree(&staging, "archive Worker diagnostics")?;
        fs::rename(&staging, &target).map_err(|source| RuntimeError::StoreIo {
            operation: "archive Worker diagnostics",
            path: target.clone(),
            source,
        })?;
        sync_directory(parent, "archive Worker diagnostics")?;
        validate_existing_diagnostics_archive(&target, request)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn validate_existing_diagnostics_archive(
    target: &Path,
    request: &WorkerRetentionExecutionRequest,
) -> Result<(), RuntimeError> {
    let manifest: DiagnosticsArchiveManifest = read_json(
        &target.join("manifest.json"),
        "verify Worker diagnostics archive",
    )?;
    if manifest.schema_version != ARCHIVE_SCHEMA_VERSION
        || manifest.operation_id != request.operation_id
        || manifest.workspace_id != request.workspace_id
        || manifest.source_runtime_id != request.source_runtime_id
        || manifest.source_worker_id != request.worker_id
        || manifest.input_fingerprint != request.input_fingerprint
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "diagnostics archive operation {} does not match the retention request",
            request.operation_id
        )));
    }
    let mut files = collect_files(target, "verify Worker diagnostics archive")?;
    files.retain(|(relative, _)| relative != Path::new("manifest.json"));
    let (checksum, bytes, count) = checksum_files(&files, "verify Worker diagnostics archive")?;
    if checksum != manifest.content_checksum_sha256
        || bytes != manifest.content_bytes
        || count != manifest.content_file_count
    {
        return Err(RuntimeError::StoreCorrupt {
            operation: "verify Worker diagnostics archive",
            path: target.to_path_buf(),
            message: "diagnostics archive checksum or content summary mismatch".to_string(),
        });
    }
    Ok(())
}

fn diagnostics_files(
    worker_dir: &Path,
    operation: &'static str,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let runs = worker_dir.join("runs");
    if !runs.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for (_, path) in collect_files(&runs, operation)? {
        let name = path.file_name().and_then(|name| name.to_str());
        if matches!(name, Some("worker.out.log" | "worker.err.log")) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn collect_files(
    root: &Path,
    operation: &'static str,
) -> Result<Vec<(PathBuf, PathBuf)>, RuntimeError> {
    fn visit(
        root: &Path,
        current: &Path,
        operation: &'static str,
        files: &mut Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), RuntimeError> {
        let mut entries = fs::read_dir(current)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: current.to_path_buf(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: current.to_path_buf(),
                source,
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| RuntimeError::StoreIo {
                operation,
                path: path.clone(),
                source,
            })?;
            if file_type.is_symlink() {
                return Err(RuntimeError::StoreCorrupt {
                    operation,
                    path,
                    message: "symlinks are not allowed in retained Worker evidence".to_string(),
                });
            }
            if file_type.is_dir() {
                visit(root, &path, operation, files)?;
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| RuntimeError::StoreCorrupt {
                        operation,
                        path: path.clone(),
                        message: "retention source escaped its aggregate root".to_string(),
                    })?
                    .to_path_buf();
                files.push((relative, path));
            } else {
                return Err(RuntimeError::StoreCorrupt {
                    operation,
                    path,
                    message: "unsupported retained Worker evidence entry".to_string(),
                });
            }
        }
        Ok(())
    }
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    visit(root, root, operation, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn checksum_files(
    files: &[(PathBuf, PathBuf)],
    operation: &'static str,
) -> Result<(String, u64, u64), RuntimeError> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    for (relative, path) in files {
        let relative = relative.to_string_lossy();
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        let mut file = File::open(path).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.clone(),
            source,
        })?;
        let length = file_len(path, operation)?;
        hasher.update(length.to_be_bytes());
        let mut buffer = [0_u8; 8192];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| RuntimeError::StoreIo {
                    operation,
                    path: path.clone(),
                    source,
                })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        total = total.saturating_add(length);
    }
    let digest = hasher.finalize();
    let checksum = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((checksum, total, files.len() as u64))
}

fn copy_files(
    files: &[(PathBuf, PathBuf)],
    target_root: &Path,
    operation: &'static str,
) -> Result<(), RuntimeError> {
    for (relative, source_path) in files {
        let target = target_root.join(relative);
        let parent = target.parent().ok_or_else(|| RuntimeError::StoreCorrupt {
            operation,
            path: target.clone(),
            message: "retention copy target has no parent".to_string(),
        })?;
        fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: parent.to_path_buf(),
            source,
        })?;
        let bytes = fs::read(source_path).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: source_path.clone(),
            source,
        })?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: target.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: target,
                source,
            })?;
    }
    Ok(())
}

fn atomic_write_json<T: Serialize>(
    path: &Path,
    value: &T,
    operation: &'static str,
) -> Result<(), RuntimeError> {
    let parent = path.parent().ok_or_else(|| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: "retention record has no parent".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::StoreIo {
        operation,
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: temporary.clone(),
                source,
            })?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|source| {
            RuntimeError::StoreCorrupt {
                operation,
                path: temporary.clone(),
                message: source.to_string(),
            }
        })?;
        file.write_all(b"\n")
            .and_then(|_| file.sync_all())
            .map_err(|source| RuntimeError::StoreIo {
                operation,
                path: temporary.clone(),
                source,
            })?;
        drop(file);
        fs::rename(&temporary, path).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
        sync_directory(parent, operation)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    operation: &'static str,
) -> Result<T, RuntimeError> {
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
    serde_json::from_reader(file).map_err(|source| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

fn sync_tree(path: &Path, operation: &'static str) -> Result<(), RuntimeError> {
    let mut directories = vec![path.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let current = directories[index].clone();
        index += 1;
        for entry in fs::read_dir(&current).map_err(|source| RuntimeError::StoreIo {
            operation,
            path: current.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| RuntimeError::StoreIo {
                operation,
                path: current.clone(),
                source,
            })?;
            if entry.path().is_dir() {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        sync_directory(&directory, operation)?;
    }
    Ok(())
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

fn file_len(path: &Path, operation: &'static str) -> Result<u64, RuntimeError> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

fn temporary_path(path: &Path) -> PathBuf {
    let sequence = NEXT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("retention");
    path.with_file_name(format!(".{name}.tmp-{}-{sequence}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn write_json(path: &Path, value: &impl Serialize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn source(root: &Path, worker_id: WorkerId) {
        let worker = root.join("workers").join(worker_id.to_string());
        write_json(
            &worker.join("worker.json"),
            &serde_json::json!({
                "workspace_id": "workspace-a"
            }),
        );
        write_json(
            &worker.join("session/session.json"),
            &serde_json::json!({"schema_version": 1, "session_id": "00000000-0000-4000-8000-000000000001"}),
        );
        fs::create_dir_all(worker.join("session/segments")).unwrap();
        fs::write(worker.join("session/segments/segment-a.jsonl"), b"one\n").unwrap();
        fs::create_dir_all(worker.join("runs/attempt-a")).unwrap();
        fs::write(
            worker.join("runs/attempt-a/worker.out.log"),
            b"diagnostic\n",
        )
        .unwrap();
        fs::write(worker.join("runs/attempt-a/worker.sock"), b"not retained").unwrap();
    }

    fn request(
        worker_id: WorkerId,
        disposition: SessionDisposition,
    ) -> WorkerRetentionExecutionRequest {
        WorkerRetentionExecutionRequest {
            operation_id: "operation-a".to_string(),
            input_fingerprint: "fingerprint-a".to_string(),
            archive_id: (disposition == SessionDisposition::Archive)
                .then(|| "archive-a".to_string()),
            workspace_id: "workspace-a".to_string(),
            source_runtime_id: "runtime-a".to_string(),
            worker_id,
            source_created_at: "2026-01-01T00:00:00Z".to_string(),
            removed_at: "2026-01-02T00:00:00Z".to_string(),
            effective_profile: Some("builtin:coder".to_string()),
            retention_class: None,
            policy_id: "policy-a".to_string(),
            session_disposition: disposition,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
        }
    }

    fn old_compacted_source(root: &Path, worker_id: WorkerId) -> session_store::SegmentId {
        use session_store::{
            LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
            LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
            Store,
        };
        let worker = root.join("workers").join(worker_id.to_string());
        write_json(
            &worker.join("worker.json"),
            &serde_json::json!({"workspace_id": "workspace-a"}),
        );
        let store = session_store::WorkerSessionStore::new(worker.join("session")).unwrap();
        let session_id = session_store::new_session_id();
        let parent = session_store::new_segment_id();
        let compact = session_store::new_segment_id();
        let original = LoggedHistoryEntry {
            item: LoggedItem::Message {
                role: LoggedRole::User,
                content: vec![LoggedContentPart::Text {
                    text: "request".into(),
                }],
            },
            metadata: LoggedSessionHistoryMetadata {
                system_input: None,
                entry_id: LoggedSessionHistoryEntryId("original".into()),
                origin: LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
            },
        };
        let summary = LoggedHistoryEntry {
            item: LoggedItem::Message {
                role: LoggedRole::System,
                content: vec![LoggedContentPart::Text {
                    text: "[Compacted context summary]\n\nsummary".into(),
                }],
            },
            metadata: LoggedSessionHistoryMetadata {
                system_input: None,
                entry_id: LoggedSessionHistoryEntryId("summary".into()),
                origin: LoggedSessionHistoryOrigin::DerivedSummary,
                derivation: Some(session_store::LoggedHistoryDerivation {
                    sources: vec![original.metadata.entry_id.clone()],
                }),
            },
        };
        for (id, history, compacted_from) in [
            (parent, vec![original], None),
            (
                compact,
                vec![summary],
                Some(session_store::SegmentOrigin {
                    segment_id: parent,
                    at_turn_index: 0,
                }),
            ),
        ] {
            store
                .create_segment(
                    session_id,
                    id,
                    &[LogEntry::AnnotatedSegmentStart {
                        ts: 1,
                        session_id,
                        system_prompt: None,
                        config: Default::default(),
                        history,
                        forked_from: None,
                        compacted_from,
                    }],
                )
                .unwrap();
        }
        let mut metadata = session_store::WorkerMetadata::new(
            worker_id.to_string(),
            Some(session_store::WorkerActiveSegmentRef::active_segment(
                session_id, compact,
            )),
        );
        metadata.workspace_id = Some("workspace-a".into());
        write_json(&worker.join("metadata.json"), &metadata);
        write_json(
            &worker.join("session/session.json"),
            &serde_json::json!({"schema_version": 3, "session_id": session_id}),
        );
        compact
    }

    #[test]
    fn session_migration_repairs_failed_archive_and_preserves_ingestion_receipt_on_retry() {
        let temp = tempfile::tempdir().unwrap();
        let worker = WorkerId::now_v7();
        let compact = old_compacted_source(temp.path(), worker);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let request = request(worker, SessionDisposition::Archive);
        assert!(
            provider
                .execute(&request)
                .unwrap_err()
                .to_string()
                .contains("Archived canonical source is unreadable")
        );
        let archive_dir = provider.archive_dir("archive-a").unwrap();
        let original_manifest = fs::read(archive_dir.join("manifest.json")).unwrap();
        // Production invokes this only in the private tree before atomic switch.
        migrate_staged_sessions(temp.path()).unwrap();
        assert_eq!(
            fs::read(archive_dir.join("manifest.json")).unwrap(),
            original_manifest
        );
        let (manifest, session_root) = provider.session_archive("archive-a").unwrap();
        let reader = session_store::public_material::SessionPublicMaterialReader::open_worker(
            &session_root,
            compact,
            Default::default(),
        )
        .unwrap();
        assert_eq!(reader.descriptors().len(), 1);
        let once = fs::read(archive_dir.join(ARCHIVE_STORAGE_FILE)).unwrap();
        migrate_staged_sessions(temp.path()).unwrap();
        assert_eq!(
            fs::read(archive_dir.join(ARCHIVE_STORAGE_FILE)).unwrap(),
            once
        );
        let result = provider.execute(&request).unwrap();
        assert!(result.source_removed);
        assert_eq!(result.archive.as_ref(), Some(&manifest));
        assert_eq!(provider.execute(&request).unwrap(), result);
        assert!(!provider.worker_dir(worker).exists());
        // Both historical receipt and rewritten payload remain verified.
        fs::write(
            session_root
                .join("segments")
                .join(format!("{compact}.jsonl")),
            b"tampered",
        )
        .unwrap();
        assert!(provider.session_archive("archive-a").is_err());
    }

    #[test]
    fn session_migration_updates_published_archive_without_changing_completed_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let worker = WorkerId::now_v7();
        let compact = old_compacted_source(temp.path(), worker);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let path = provider
            .worker_dir(worker)
            .join("session/segments")
            .join(format!("{compact}.jsonl"));
        let old = fs::read_to_string(&path).unwrap();
        fs::write(&path, old.replace("derived_summary", "compaction_summary")).unwrap();
        let request = request(worker, SessionDisposition::Archive);
        // Frozen schema-v3 completed-operation fixture: the old Runtime had
        // already accepted this payload and removed its live source.
        let archive =
            commit_session_archive(&provider, &request, &provider.worker_dir(worker)).unwrap();
        let completed = WorkerRetentionExecutionResult {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            worker_id: worker,
            session_disposition: SessionDisposition::Archive,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
            archive: Some(archive),
            source_removed: true,
            diagnostics_retained: false,
        };
        write_json(
            &provider.operation_path(&request.operation_id).unwrap(),
            &RetentionOperationReceipt {
                schema_version: OPERATION_SCHEMA_VERSION,
                request: request.clone(),
                result: completed.clone(),
            },
        );
        fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
        let receipt = fs::read(provider.operation_path(&request.operation_id).unwrap()).unwrap();
        migrate_staged_sessions(temp.path()).unwrap();
        let (manifest, root) = provider.session_archive("archive-a").unwrap();
        assert_eq!(completed.archive.as_ref(), Some(&manifest));
        assert_eq!(
            fs::read(provider.operation_path(&request.operation_id).unwrap()).unwrap(),
            receipt
        );
        assert_eq!(provider.execute(&request).unwrap(), completed);
        assert!(session_store::WorkerSessionStore::open_read_only(&root).is_ok());
        let storage_path = provider
            .archive_dir("archive-a")
            .unwrap()
            .join(ARCHIVE_STORAGE_FILE);
        let mut storage: MigratedArchiveStorage = read_json(&storage_path, "test").unwrap();
        storage.ingestion.archive_id = "another-archive".into();
        write_json(&storage_path, &storage);
        assert!(provider.session_archive("archive-a").is_err());
    }

    #[test]
    fn session_migration_rejects_corrupt_archive_before_replacing_its_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let worker = WorkerId::now_v7();
        let compact = old_compacted_source(temp.path(), worker);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let request = request(worker, SessionDisposition::Archive);
        assert!(provider.execute(&request).is_err());
        let archive = provider.archive_dir("archive-a").unwrap();
        let path = archive
            .join("session/segments")
            .join(format!("{compact}.jsonl"));
        fs::write(&path, b"corrupt\n").unwrap();
        assert!(migrate_staged_sessions(temp.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt\n");
        assert!(!archive.join(ARCHIVE_STORAGE_FILE).exists());
    }

    #[test]
    fn session_migration_does_not_read_or_upgrade_erased_originals() {
        let temp = tempfile::tempdir().unwrap();
        let worker = WorkerId::now_v7();
        let compact = old_compacted_source(temp.path(), worker);
        let session = temp
            .path()
            .join("workers")
            .join(worker.to_string())
            .join("session");
        let manifest = fs::read(session.join("session.json")).unwrap();
        let identity: CanonicalSessionManifest = serde_json::from_slice(&manifest).unwrap();
        let id = identity.session_id.parse().unwrap();
        let retention = runtime_session_retention(temp.path());
        retention
            .delete(
                id,
                session_store::retention::SessionDeletionIntent::ErasePermanently,
                || Ok(()),
            )
            .unwrap();
        let path = session.join("segments").join(format!("{compact}.jsonl"));
        fs::write(&path, b"unreadable fenced content\n").unwrap();
        migrate_staged_sessions(temp.path()).unwrap();
        assert_eq!(fs::read(session.join("session.json")).unwrap(), manifest);
        assert_eq!(fs::read(path).unwrap(), b"unreadable fenced content\n");
        assert!(matches!(
            retention.ensure_live(id),
            Err(session_store::StoreError::SessionErased(_))
        ));
    }

    #[test]
    fn archive_is_verified_before_source_removal_and_retry_converges() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(7);
        source(temp.path(), worker_id);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let request = request(worker_id, SessionDisposition::Archive);

        let first = provider.execute(&request).unwrap();
        assert!(first.source_removed);
        let archive = first.archive.as_ref().unwrap();
        assert_eq!(
            archive.source_session_id,
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(archive.segment_ids, vec!["segment-a"]);
        assert!(!temp.path().join(format!("workers/{worker_id}")).exists());
        assert!(
            temp.path()
                .join("archives/workers/archive-a/session/segments/segment-a.jsonl")
                .is_file()
        );
        assert!(!temp.path().join("archives/workers/archive-a/runs").exists());

        let retry = provider.execute(&request).unwrap();
        assert_eq!(retry, first);
        assert_eq!(
            fs::read_dir(temp.path().join("archives/workers"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn archive_failure_keeps_live_source_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(8);
        source(temp.path(), worker_id);
        let collision = temp.path().join("archives/workers/archive-a");
        fs::create_dir_all(&collision).unwrap();
        fs::write(collision.join("manifest.json"), b"not-json").unwrap();
        let provider = FsWorkerRetentionProvider::new(temp.path());

        assert!(
            provider
                .execute(&request(worker_id, SessionDisposition::Archive))
                .is_err()
        );
        assert!(
            temp.path()
                .join(format!("workers/{worker_id}/session"))
                .is_dir()
        );
        assert!(
            !temp
                .path()
                .join("retention/operations/operation-a.json")
                .exists()
        );
    }

    #[test]
    fn target_inventory_and_execute_reject_cross_workspace_aggregate() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(16);
        source(temp.path(), worker_id);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        assert!(matches!(
            provider.inventory("other-workspace", "runtime-a", worker_id),
            Err(RuntimeError::WorkerNotFound { .. })
        ));
        let mut request = request(worker_id, SessionDisposition::Purge);
        request.workspace_id = "other-workspace".to_string();
        assert!(matches!(
            provider.execute(&request),
            Err(RuntimeError::WorkerNotFound { .. })
        ));
        assert!(
            temp.path()
                .join(format!("workers/{worker_id}/session"))
                .is_dir()
        );
        assert!(
            !temp
                .path()
                .join("retention/operations/operation-a.json")
                .exists()
        );

        request.workspace_id = "workspace-a".to_string();
        provider.execute(&request).unwrap();
        request.workspace_id = "other-workspace".to_string();
        assert!(provider.execute(&request).is_err());
    }

    #[test]
    fn purge_removes_worker_aggregate() {
        let temp = tempfile::tempdir().unwrap();
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let worker_id = WorkerId::from_legacy_u64(9);
        source(temp.path(), worker_id);
        let mut current = request(worker_id, SessionDisposition::Purge);
        current.operation_id = "operation-current".to_string();
        current.input_fingerprint = "fingerprint-current".to_string();
        let result = provider.execute(&current).unwrap();
        assert!(result.archive.is_none());
        assert!(!temp.path().join(format!("workers/{worker_id}")).exists());
        assert!(
            temp.path()
                .join("retention/operations/operation-current.json")
                .is_file()
        );
    }

    #[test]
    fn pending_receipt_recovers_delete_to_receipt_crash_window() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(11);
        source(temp.path(), worker_id);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let request = request(worker_id, SessionDisposition::Archive);
        let completed = provider.execute(&request).unwrap();
        let receipt_path = temp.path().join("retention/operations/operation-a.json");
        let mut receipt: RetentionOperationReceipt =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        receipt.result.source_removed = false;
        fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

        let recovered = provider.execute(&request).unwrap();
        assert_eq!(recovered, completed);
        assert!(provider.completed_for(&request).unwrap().is_some());
    }

    #[test]
    fn provider_snapshot_scans_aggregate_storage_independent_of_runtime_catalog() {
        let temp = tempfile::tempdir().unwrap();
        source(temp.path(), WorkerId::from_legacy_u64(13));
        let other_worker = WorkerId::from_legacy_u64(14);
        source(temp.path(), other_worker);
        write_json(
            &temp
                .path()
                .join("workers")
                .join(other_worker.to_string())
                .join("worker.json"),
            &serde_json::json!({
                "workspace_id": "other-workspace"
            }),
        );
        fs::create_dir_all(temp.path().join("workers/not-a-worker")).unwrap();
        fs::write(
            temp.path().join("workers/not-a-worker/worker.json"),
            b"not-json",
        )
        .unwrap();
        let corrupt_worker = WorkerId::from_legacy_u64(15);
        let corrupt_worker_dir = temp.path().join("workers").join(corrupt_worker.to_string());
        fs::create_dir_all(&corrupt_worker_dir).unwrap();
        fs::write(corrupt_worker_dir.join("worker.json"), b"not-json").unwrap();
        let provider = FsWorkerRetentionProvider::new(temp.path());

        let snapshot = provider.snapshot("workspace-a", "runtime-a").unwrap();
        assert_eq!(snapshot.workers().len(), 1);
        assert_eq!(
            snapshot.workers()[0].worker_id,
            WorkerId::from_legacy_u64(13)
        );
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.worker_id() == other_worker.to_string()
                && diagnostic.category() == "aggregate_workspace_mismatch"
        }));
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.worker_id() == "not-a-worker"
                && diagnostic.category() == "aggregate_worker_id_invalid"
        }));
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.worker_id() == corrupt_worker.to_string()
                && diagnostic.category() == "aggregate_worker_record_corrupt"
        }));
    }

    #[test]
    fn diagnostics_retry_rejects_corrupt_existing_archive_before_source_delete() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(12);
        source(temp.path(), worker_id);
        let provider = FsWorkerRetentionProvider::new(temp.path());
        let mut request = request(worker_id, SessionDisposition::Archive);
        request.diagnostics_disposition = DiagnosticsDisposition::Retain;
        provider.execute(&request).unwrap();

        let receipt_path = temp.path().join("retention/operations/operation-a.json");
        let mut receipt: RetentionOperationReceipt =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        receipt.result.source_removed = false;
        fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
        source(temp.path(), worker_id);
        fs::write(
            temp.path()
                .join("archives/diagnostics/operation-a/runs/attempt-a/worker.out.log"),
            b"corrupt\n",
        )
        .unwrap();

        assert!(provider.execute(&request).is_err());
        assert!(
            temp.path()
                .join(format!("workers/{worker_id}/session"))
                .is_dir()
        );
        assert!(provider.completed_for(&request).unwrap().is_none());
    }

    #[test]
    fn concurrent_retry_produces_one_archive() {
        let temp = tempfile::tempdir().unwrap();
        let worker_id = WorkerId::from_legacy_u64(10);
        source(temp.path(), worker_id);
        let provider = Arc::new(FsWorkerRetentionProvider::new(temp.path()));
        let request = Arc::new(request(worker_id, SessionDisposition::Archive));
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let provider = provider.clone();
                let request = request.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    provider.execute(&request)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results[0], results[1]);
        assert_eq!(
            fs::read_dir(temp.path().join("archives/workers"))
                .unwrap()
                .count(),
            1
        );
    }
}
