use crate::catalog::{CreateWorkerRequest, WorkerStatus};
use crate::config_bundle::ConfigBundle;
use crate::diagnostics::{DiagnosticSeverity, RuntimeDiagnostic};
use crate::error::RuntimeError;
use crate::execution::WorkerExecutionStatus;
use crate::identity::{WorkerId, WorkerRef};
use crate::management::{RuntimeBackendKind, RuntimeLimits, RuntimeStatus};
#[cfg(feature = "ws-server")]
use crate::observation::WorkerObservationEvent;
use crate::observation::{EventCursor, RuntimeEvent, RuntimeEventBatch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 1;
const RUNTIME_FILE: &str = "runtime.json";
const EVENTS_FILE: &str = "events.jsonl";
const WORKERS_DIR: &str = "workers";
const LEGACY_RUNTIMES_DIR: &str = "runtimes";
const WORKER_FILE: &str = "worker.json";
#[cfg(feature = "ws-server")]
const OBSERVATIONS_FILE: &str = "observations.jsonl";

static NEXT_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Options for constructing a filesystem-backed Runtime store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsRuntimeStoreOptions {
    /// Root directory containing this Runtime's store data.
    pub root: PathBuf,
    pub display_name: Option<String>,
    pub limits: RuntimeLimits,
}

impl FsRuntimeStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            display_name: None,
            limits: RuntimeLimits::default(),
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

    /// Read persisted Runtime events directly from the event log with the same
    /// bounded cursor semantics as [`crate::Runtime::read_events`].
    pub fn read_events(
        &self,
        cursor: &EventCursor,
        limit: usize,
        max_limit: usize,
    ) -> Result<RuntimeEventBatch, RuntimeError> {
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
            cursor: EventCursor { next_event_id },
            events: selected,
            has_more,
        })
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
        #[cfg(feature = "ws-server")]
        ensure_file_exists(
            &worker_dir.join(OBSERVATIONS_FILE),
            "create observations log",
        )?;
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

    pub(crate) fn append_event(&self, event: &RuntimeEvent) -> Result<(), RuntimeError> {
        if let Some(worker_ref) = &event.worker_ref {
            self.ensure_worker_ref(worker_ref)?;
        }
        append_json_line(&self.events_path(), event, "append event")
    }

    #[cfg(feature = "ws-server")]
    pub(crate) fn append_worker_observation_event(
        &self,
        event: &WorkerObservationEvent,
    ) -> Result<(), RuntimeError> {
        self.ensure_worker_ref(&event.worker_ref)?;
        append_json_line(
            &self.observations_path(&event.worker_ref.worker_id),
            event,
            "append worker observation",
        )
    }

    pub(crate) fn load_runtime_state(&self) -> Result<PersistedRuntimeState, RuntimeError> {
        let runtime_path = self.runtime_path();
        let mut snapshot: RuntimeSnapshot = read_json(&runtime_path, "read runtime snapshot")?;
        snapshot.validate(&runtime_path)?;

        let events = read_json_lines::<RuntimeEvent>(&self.events_path(), "read events")?;
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

        #[cfg(feature = "ws-server")]
        let mut observation_events = Vec::new();

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
            #[cfg(feature = "ws-server")]
            let worker_observations = match read_json_lines::<WorkerObservationEvent>(
                &path.join(OBSERVATIONS_FILE),
                "read worker observations",
            ) {
                Ok(events) => events,
                Err(_error) => {
                    record_worker_load_diagnostic(
                        &mut snapshot,
                        Some(worker_snapshot.worker_ref.clone()),
                        "ignored worker with unreadable observations while loading runtime store",
                    );
                    continue;
                }
            };
            #[cfg(feature = "ws-server")]
            let mut observations_valid = true;
            #[cfg(feature = "ws-server")]
            for event in &worker_observations {
                if self.ensure_worker_ref(&event.worker_ref).is_err()
                    || event.worker_ref.worker_id != worker_snapshot.worker_id
                {
                    observations_valid = false;
                    break;
                }
            }
            #[cfg(feature = "ws-server")]
            if !observations_valid {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    Some(worker_snapshot.worker_ref.clone()),
                    "ignored worker with invalid observations while loading runtime store",
                );
                continue;
            }
            #[cfg(feature = "ws-server")]
            observation_events.extend(worker_observations);
            let worker = worker_snapshot.into_persisted();
            if workers.insert(worker.worker_id.clone(), worker).is_some() {
                record_worker_load_diagnostic(
                    &mut snapshot,
                    None,
                    "ignored duplicate worker snapshot while loading runtime store",
                );
            }
        }

        #[cfg(feature = "ws-server")]
        observation_events.sort_by_key(|event| event.sequence);

        Ok(snapshot.into_persisted(
            events,
            workers,
            #[cfg(feature = "ws-server")]
            observation_events,
        ))
    }

    fn ensure_worker_ref(&self, _worker_ref: &WorkerRef) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn runtime_path(&self) -> PathBuf {
        self.root.join(RUNTIME_FILE)
    }

    fn events_path(&self) -> PathBuf {
        self.root.join(EVENTS_FILE)
    }

    fn worker_dir(&self, worker_id: &WorkerId) -> PathBuf {
        self.root.join(WORKERS_DIR).join(worker_id.to_string())
    }

    #[cfg(feature = "ws-server")]
    fn observations_path(&self, worker_id: &WorkerId) -> PathBuf {
        self.worker_dir(worker_id).join(OBSERVATIONS_FILE)
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
    pub(crate) limits: RuntimeLimits,
    pub(crate) next_worker_sequence: u64,
    pub(crate) next_event_id: u64,
    pub(crate) next_diagnostic_id: u64,
    pub(crate) workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
    pub(crate) config_bundles: BTreeMap<String, ConfigBundle>,
    pub(crate) events: Vec<RuntimeEvent>,
    #[cfg(feature = "ws-server")]
    pub(crate) observation_events: Vec<WorkerObservationEvent>,
    pub(crate) diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedWorkerRecord {
    pub(crate) worker_ref: WorkerRef,
    pub(crate) worker_id: WorkerId,
    pub(crate) status: WorkerStatus,
    pub(crate) request: CreateWorkerRequest,
    pub(crate) execution: WorkerExecutionStatus,
    pub(crate) last_event_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeSnapshot {
    schema_version: u32,
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
            limits: state.limits.clone(),
            next_worker_sequence: state.next_worker_sequence,
            next_event_id: state.next_event_id,
            next_diagnostic_id: state.next_diagnostic_id,
            config_bundles: state.config_bundles.clone(),
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
        events: Vec<RuntimeEvent>,
        workers: BTreeMap<WorkerId, PersistedWorkerRecord>,
        #[cfg(feature = "ws-server")] observation_events: Vec<WorkerObservationEvent>,
    ) -> PersistedRuntimeState {
        PersistedRuntimeState {
            display_name: self.display_name,
            status: self.status,
            limits: self.limits,
            next_worker_sequence: self.next_worker_sequence,
            next_event_id: self.next_event_id,
            next_diagnostic_id: self.next_diagnostic_id,
            workers,
            config_bundles: self.config_bundles,
            events,
            #[cfg(feature = "ws-server")]
            observation_events,
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
    #[serde(default = "WorkerExecutionStatus::stopped")]
    execution: WorkerExecutionStatus,
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
            last_event_id: worker.last_event_id,
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
        PersistedWorkerRecord {
            worker_ref: self.worker_ref,
            worker_id: self.worker_id,
            status: self.status,
            request: self.request,
            execution: self.execution,
            last_event_id: self.last_event_id,
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
        &legacy_dir.join(EVENTS_FILE),
        &root.join(EVENTS_FILE),
        "migrate legacy runtime events",
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
