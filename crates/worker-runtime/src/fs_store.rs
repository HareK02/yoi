use crate::catalog::{CreateWorkerRequest, WorkerStatus};
use crate::config_bundle::ConfigBundle;
use crate::diagnostics::RuntimeDiagnostic;
use crate::error::RuntimeError;
use crate::execution::WorkerExecutionStatus;
use crate::identity::{RuntimeId, WorkerId, WorkerRef};
use crate::management::{RuntimeBackendKind, RuntimeLimits, RuntimeStatus};
use crate::observation::{
    EventCursor, RuntimeEvent, RuntimeEventBatch, TranscriptEntry, TranscriptProjection,
    TranscriptQuery,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 1;
const RUNTIMES_DIR: &str = "runtimes";
const RUNTIME_FILE: &str = "runtime.json";
const EVENTS_FILE: &str = "events.jsonl";
const WORKERS_DIR: &str = "workers";
const WORKER_FILE: &str = "worker.json";
const TRANSCRIPT_FILE: &str = "transcript.jsonl";

static NEXT_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Options for constructing a filesystem-backed Runtime store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStoreOptions {
    /// Root directory containing all Runtime-scoped store data.
    pub root: PathBuf,
    pub runtime_id: Option<RuntimeId>,
    pub display_name: Option<String>,
    pub limits: RuntimeLimits,
}

impl FsRuntimeStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_id: None,
            display_name: None,
            limits: RuntimeLimits::default(),
        }
    }
}

/// Filesystem persistence boundary for Worker Runtime state.
///
/// Authority is the typed `runtime_id + worker_id` pair.  Those ids are encoded
/// into path components only after validation; legacy pod paths, socket paths,
/// and session paths are deliberately not part of the layout or lookup API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStore {
    root: PathBuf,
    runtime_id: RuntimeId,
    runtime_dir: PathBuf,
}

impl FsRuntimeStore {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_id(&self) -> &RuntimeId {
        &self.runtime_id
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Read persisted Runtime events directly from the event log with the same
    /// bounded cursor semantics as [`crate::Runtime::read_events`].
    pub fn read_events(
        &self,
        cursor: &EventCursor,
        limit: usize,
        max_limit: usize,
    ) -> Result<RuntimeEventBatch, RuntimeError> {
        if cursor.runtime_id != self.runtime_id {
            return Err(RuntimeError::WrongRuntimeCursor {
                expected_runtime_id: self.runtime_id.clone(),
                actual_runtime_id: cursor.runtime_id.clone(),
            });
        }
        if limit > max_limit {
            return Err(RuntimeError::LimitTooLarge {
                requested: limit,
                max: max_limit,
            });
        }

        let events = read_json_lines::<RuntimeEvent>(&self.events_path(), "read events")?;
        let mut selected = Vec::new();
        for event in events
            .iter()
            .filter(|event| event.id >= cursor.next_event_id)
            .take(limit)
        {
            selected.push(event.clone());
        }
        let next_event_id = selected
            .last()
            .map(|event| event.id + 1)
            .unwrap_or(cursor.next_event_id);
        let has_more = events.iter().any(|event| event.id >= next_event_id);

        Ok(RuntimeEventBatch {
            runtime_id: self.runtime_id.clone(),
            cursor: EventCursor {
                runtime_id: self.runtime_id.clone(),
                next_event_id,
            },
            events: selected,
            has_more,
        })
    }

    /// Read a persisted Worker transcript directly from its Worker-scoped log.
    pub fn read_transcript(
        &self,
        worker_ref: &WorkerRef,
        query: TranscriptQuery,
        max_limit: usize,
    ) -> Result<TranscriptProjection, RuntimeError> {
        self.ensure_worker_ref(worker_ref)?;
        if query.limit > max_limit {
            return Err(RuntimeError::LimitTooLarge {
                requested: query.limit,
                max: max_limit,
            });
        }

        let path = self.transcript_path(&worker_ref.worker_id);
        let entries = read_json_lines::<TranscriptEntry>(&path, "read transcript")?;
        let total_items = entries.len();
        let end = query.start.saturating_add(query.limit).min(total_items);
        let items = if query.start >= total_items {
            Vec::new()
        } else {
            entries[query.start..end].to_vec()
        };
        let next_start = (end < total_items).then_some(end);

        Ok(TranscriptProjection {
            worker_ref: worker_ref.clone(),
            start: query.start,
            limit: query.limit,
            total_items,
            items,
            next_start,
        })
    }

    pub(crate) fn open_or_create(
        root: PathBuf,
        runtime_id: RuntimeId,
    ) -> Result<OpenedFsRuntimeStore, RuntimeError> {
        fs::create_dir_all(root.join(RUNTIMES_DIR)).map_err(|source| RuntimeError::StoreIo {
            operation: "create store root",
            path: root.join(RUNTIMES_DIR),
            source,
        })?;

        let runtime_dir = runtime_dir(&root, &runtime_id);
        let existed = runtime_dir.exists();
        if existed && !runtime_dir.is_dir() {
            return Err(RuntimeError::StoreCorrupt {
                operation: "open runtime store",
                path: runtime_dir,
                message: "runtime path exists but is not a directory".to_string(),
            });
        }

        fs::create_dir_all(runtime_dir.join(WORKERS_DIR)).map_err(|source| {
            RuntimeError::StoreIo {
                operation: "create runtime store",
                path: runtime_dir.join(WORKERS_DIR),
                source,
            }
        })?;

        let store = Self {
            root,
            runtime_id,
            runtime_dir,
        };
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
        ensure_file_exists(&worker_dir.join(TRANSCRIPT_FILE), "create transcript log")
    }

    pub(crate) fn append_event(&self, event: &RuntimeEvent) -> Result<(), RuntimeError> {
        if let Some(worker_ref) = &event.worker_ref {
            self.ensure_worker_ref(worker_ref)?;
        }
        append_json_line(&self.events_path(), event, "append event")
    }

    pub(crate) fn append_transcript_entry(
        &self,
        entry: &TranscriptEntry,
    ) -> Result<(), RuntimeError> {
        self.ensure_worker_ref(&entry.worker_ref)?;
        append_json_line(
            &self.transcript_path(&entry.worker_ref.worker_id),
            entry,
            "append transcript",
        )
    }

    pub(crate) fn load_runtime_state(&self) -> Result<PersistedRuntimeState, RuntimeError> {
        let runtime_path = self.runtime_path();
        let snapshot: RuntimeSnapshot = read_json(&runtime_path, "read runtime snapshot")?;
        snapshot.validate(&self.runtime_id, &runtime_path)?;

        let events = read_json_lines::<RuntimeEvent>(&self.events_path(), "read events")?;
        let workers_dir = self.runtime_dir.join(WORKERS_DIR);
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
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read worker",
                    path,
                    message: "worker path exists but is not a directory".to_string(),
                });
            }
            let worker_snapshot_path = path.join(WORKER_FILE);
            let worker_snapshot: WorkerSnapshot =
                read_json(&worker_snapshot_path, "read worker snapshot")?;
            worker_snapshot.validate(&self.runtime_id, &worker_snapshot_path)?;
            let transcript =
                read_json_lines::<TranscriptEntry>(&path.join(TRANSCRIPT_FILE), "read transcript")?;
            for entry in &transcript {
                self.ensure_worker_ref(&entry.worker_ref)?;
                if entry.worker_ref.worker_id != worker_snapshot.worker_id {
                    return Err(RuntimeError::StoreCorrupt {
                        operation: "read transcript",
                        path: path.join(TRANSCRIPT_FILE),
                        message: format!(
                            "transcript entry belongs to worker {}, expected {}",
                            entry.worker_ref.worker_id, worker_snapshot.worker_id
                        ),
                    });
                }
            }
            let worker = worker_snapshot.into_persisted(transcript);
            if workers.insert(worker.worker_id.clone(), worker).is_some() {
                return Err(RuntimeError::StoreCorrupt {
                    operation: "read workers",
                    path: workers_dir.clone(),
                    message: "duplicate worker id in store".to_string(),
                });
            }
        }

        Ok(snapshot.into_persisted(events, workers))
    }

    fn ensure_worker_ref(&self, worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        if worker_ref.runtime_id == self.runtime_id {
            Ok(())
        } else {
            Err(RuntimeError::WrongRuntime {
                expected_runtime_id: self.runtime_id.clone(),
                actual_runtime_id: worker_ref.runtime_id.clone(),
                worker_id: worker_ref.worker_id.clone(),
            })
        }
    }

    fn runtime_path(&self) -> PathBuf {
        self.runtime_dir.join(RUNTIME_FILE)
    }

    fn events_path(&self) -> PathBuf {
        self.runtime_dir.join(EVENTS_FILE)
    }

    fn worker_dir(&self, worker_id: &WorkerId) -> PathBuf {
        self.runtime_dir
            .join(WORKERS_DIR)
            .join(encoded_component(worker_id.as_str()))
    }

    fn transcript_path(&self, worker_id: &WorkerId) -> PathBuf {
        self.worker_dir(worker_id).join(TRANSCRIPT_FILE)
    }
}

#[derive(Debug)]
pub(crate) struct OpenedFsRuntimeStore {
    pub(crate) store: FsRuntimeStore,
    pub(crate) state: Option<PersistedRuntimeState>,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedRuntimeState {
    pub(crate) runtime_id: RuntimeId,
    pub(crate) display_name: Option<String>,
    pub(crate) status: RuntimeStatus,
    pub(crate) limits: RuntimeLimits,
    pub(crate) next_worker_sequence: u64,
    pub(crate) next_event_id: u64,
    pub(crate) next_diagnostic_id: u64,
    pub(crate) workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    pub(crate) config_bundles: BTreeMap<String, ConfigBundle>,
    pub(crate) events: Vec<RuntimeEvent>,
    pub(crate) diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedWorkerRecord {
    pub(crate) worker_ref: WorkerRef,
    pub(crate) worker_id: WorkerId,
    pub(crate) status: WorkerStatus,
    pub(crate) request: CreateWorkerRequest,
    pub(crate) execution: WorkerExecutionStatus,
    pub(crate) transcript: Vec<TranscriptEntry>,
    pub(crate) next_transcript_sequence: u64,
    pub(crate) last_event_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeSnapshot {
    schema_version: u32,
    runtime_id: RuntimeId,
    display_name: Option<String>,
    backend: RuntimeBackendKind,
    status: RuntimeStatus,
    limits: RuntimeLimits,
    next_worker_sequence: u64,
    next_event_id: u64,
    next_diagnostic_id: u64,
    #[serde(default)]
    config_bundles: BTreeMap<String, ConfigBundle>,
    diagnostics: Vec<RuntimeDiagnostic>,
}

impl RuntimeSnapshot {
    fn from_persisted(state: &PersistedRuntimeState) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            runtime_id: state.runtime_id.clone(),
            display_name: state.display_name.clone(),
            backend: RuntimeBackendKind::FsStore,
            status: state.status,
            limits: state.limits.clone(),
            next_worker_sequence: state.next_worker_sequence,
            next_event_id: state.next_event_id,
            next_diagnostic_id: state.next_diagnostic_id,
            config_bundles: state.config_bundles.clone(),
            diagnostics: state.diagnostics.clone(),
        }
    }

    fn validate(&self, expected_runtime_id: &RuntimeId, path: &Path) -> Result<(), RuntimeError> {
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
        if &self.runtime_id != expected_runtime_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read runtime snapshot",
                path: path.to_path_buf(),
                message: format!(
                    "runtime snapshot id {} does not match requested runtime {}",
                    self.runtime_id, expected_runtime_id
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
        events: Vec<RuntimeEvent>,
        workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    ) -> PersistedRuntimeState {
        PersistedRuntimeState {
            runtime_id: self.runtime_id,
            display_name: self.display_name,
            status: self.status,
            limits: self.limits,
            next_worker_sequence: self.next_worker_sequence,
            next_event_id: self.next_event_id,
            next_diagnostic_id: self.next_diagnostic_id,
            workers,
            config_bundles: self.config_bundles,
            events,
            diagnostics: self.diagnostics,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkerSnapshot {
    schema_version: u32,
    worker_ref: WorkerRef,
    worker_id: WorkerId,
    status: WorkerStatus,
    request: CreateWorkerRequest,
    #[serde(default = "WorkerExecutionStatus::unconnected")]
    execution: WorkerExecutionStatus,
    next_transcript_sequence: u64,
    last_event_id: u64,
}

impl WorkerSnapshot {
    fn from_persisted(worker: &PersistedWorkerRecord) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            worker_ref: worker.worker_ref.clone(),
            worker_id: worker.worker_id.clone(),
            status: worker.status,
            request: worker.request.clone(),
            execution: worker.execution.clone(),
            next_transcript_sequence: worker.next_transcript_sequence,
            last_event_id: worker.last_event_id,
        }
    }

    fn validate(&self, expected_runtime_id: &RuntimeId, path: &Path) -> Result<(), RuntimeError> {
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
        if self.worker_ref.runtime_id != *expected_runtime_id {
            return Err(RuntimeError::StoreCorrupt {
                operation: "read worker snapshot",
                path: path.to_path_buf(),
                message: format!(
                    "worker belongs to runtime {}, expected {}",
                    self.worker_ref.runtime_id, expected_runtime_id
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

    fn into_persisted(self, transcript: Vec<TranscriptEntry>) -> PersistedWorkerRecord {
        PersistedWorkerRecord {
            worker_ref: self.worker_ref,
            worker_id: self.worker_id,
            status: self.status,
            request: self.request,
            execution: self.execution,
            transcript,
            next_transcript_sequence: self.next_transcript_sequence,
            last_event_id: self.last_event_id,
        }
    }
}

fn runtime_dir(root: &Path, runtime_id: &RuntimeId) -> PathBuf {
    root.join(RUNTIMES_DIR)
        .join(encoded_component(runtime_id.as_str()))
}

fn encoded_component(value: &str) -> String {
    let mut encoded = String::with_capacity(3 + value.len() * 2);
    encoded.push_str("id-");
    for byte in value.as_bytes() {
        encoded.push(hex_digit(byte >> 4));
        encoded.push(hex_digit(byte & 0x0f));
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => unreachable!("hex digit nybble is always <= 15"),
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

fn read_json_lines<T>(path: &Path, operation: &'static str) -> Result<Vec<T>, RuntimeError>
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
    let reader = BufReader::new(file);
    let mut items = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let item = serde_json::from_str(&line).map_err(|source| RuntimeError::StoreCorrupt {
            operation,
            path: path.to_path_buf(),
            message: format!("line {}: {source}", index + 1),
        })?;
        items.push(item);
    }
    Ok(items)
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

fn append_json_line<T>(path: &Path, value: &T, operation: &'static str) -> Result<(), RuntimeError>
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
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    serde_json::to_writer(&mut file, value).map_err(|source| RuntimeError::StoreCorrupt {
        operation,
        path: path.to_path_buf(),
        message: format!("serialize json: {source}"),
    })?;
    file.write_all(b"\n")
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

fn ensure_file_exists(path: &Path, operation: &'static str) -> Result<(), RuntimeError> {
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
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| RuntimeError::StoreIo {
            operation,
            path: path.to_path_buf(),
            source,
        })
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
