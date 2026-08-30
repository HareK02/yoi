use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use manifest::WorkerManifest;
use serde::{Deserialize, Serialize};
use session_store::{SegmentId, SessionId};
use thiserror::Error;
use uuid::Uuid;

const RECORD_FILE: &str = "record.json";
const COMMIT_MARKER: &str = "commit.pending";
const LEASE_FILE: &str = "lease.json";
const LEASE_LOCK_FILE: &str = "lease.lock";
const SESSION_DIR: &str = "session";
const WORKER_DIR: &str = "worker";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StandaloneSessionId(Uuid);

impl StandaloneSessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub fn short(self) -> String {
        let simple = self.0.simple().to_string();
        simple[simple.len() - 12..].to_string()
    }
}

impl Default for StandaloneSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for StandaloneSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for StandaloneSessionId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandaloneCwdIdentity {
    pub canonical_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}

impl StandaloneCwdIdentity {
    pub fn capture(path: impl AsRef<Path>) -> Result<Self, StandaloneStoreError> {
        let canonical_path =
            fs::canonicalize(path).map_err(StandaloneStoreError::CwdUnavailable)?;
        let metadata =
            fs::metadata(&canonical_path).map_err(StandaloneStoreError::CwdUnavailable)?;
        if !metadata.is_dir() {
            return Err(StandaloneStoreError::CwdNotDirectory);
        }
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (Some(metadata.dev()), Some(metadata.ino()))
        };
        #[cfg(not(unix))]
        let (device, inode) = (None, None);
        Ok(Self {
            canonical_path,
            device,
            inode,
        })
    }

    pub fn verify(&self) -> Result<PathBuf, StandaloneStoreError> {
        let current = Self::capture(&self.canonical_path)?;
        if current != *self {
            return Err(StandaloneStoreError::CwdIdentityMismatch);
        }
        Ok(current.canonical_path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandaloneSessionStatus {
    Active,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandaloneShutdownReason {
    UserExit,
    StartupFailed,
    ControllerError,
    ProcessInterrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandaloneSessionRecord {
    pub schema_version: u32,
    pub revision: u64,
    pub session_id: StandaloneSessionId,
    pub worker_name: String,
    pub cwd: StandaloneCwdIdentity,
    pub manifest: WorkerManifest,
    pub active_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_segment_id: Option<SegmentId>,
    pub status: StandaloneSessionStatus,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_reason: Option<StandaloneShutdownReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandaloneListScope {
    CurrentCwd,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLeasePolicy {
    Reject,
    Recover,
}

#[derive(Debug, Clone)]
pub struct StandaloneSessionStore {
    root: PathBuf,
}

impl StandaloneSessionStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StandaloneStoreError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(StandaloneStoreError::Io)?;
        if !fs::metadata(&root)
            .map_err(StandaloneStoreError::Io)?
            .is_dir()
        {
            return Err(StandaloneStoreError::NotDirectory);
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn allocate(
        &self,
        cwd: impl AsRef<Path>,
        policy: StaleLeasePolicy,
    ) -> Result<StandaloneSessionAllocation, StandaloneStoreError> {
        let id = StandaloneSessionId::new();
        let cwd = StandaloneCwdIdentity::capture(cwd)?;
        let dir = self.session_dir(id);
        fs::create_dir(&dir).map_err(StandaloneStoreError::Io)?;
        fs::create_dir(dir.join(SESSION_DIR)).map_err(StandaloneStoreError::Io)?;
        fs::create_dir(dir.join(WORKER_DIR)).map_err(StandaloneStoreError::Io)?;
        let lease = self.acquire_lease(id, policy)?;
        Ok(StandaloneSessionAllocation { id, cwd, lease })
    }

    pub fn commit_created(
        &self,
        allocation: &StandaloneSessionAllocation,
        manifest: WorkerManifest,
        active_session_id: SessionId,
        active_segment_id: Option<SegmentId>,
    ) -> Result<StandaloneSessionRecord, StandaloneStoreError> {
        let now = now_unix_ms()?;
        let record = StandaloneSessionRecord {
            schema_version: SCHEMA_VERSION,
            revision: 1,
            session_id: allocation.id,
            worker_name: manifest.worker.name.clone(),
            cwd: allocation.cwd.clone(),
            manifest,
            active_session_id,
            active_segment_id,
            status: StandaloneSessionStatus::Active,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            shutdown_reason: None,
        };
        self.commit_record(None, &record)?;
        Ok(record)
    }

    pub fn load(
        &self,
        id: StandaloneSessionId,
    ) -> Result<StandaloneSessionRecord, StandaloneStoreError> {
        let dir = self.session_dir(id);
        if dir.join(COMMIT_MARKER).exists() {
            return Err(StandaloneStoreError::IncompleteCommit(id));
        }
        let bytes = fs::read(dir.join(RECORD_FILE)).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                StandaloneStoreError::SessionNotFound(id)
            } else {
                StandaloneStoreError::Io(error)
            }
        })?;
        let record: StandaloneSessionRecord = serde_json::from_slice(&bytes)
            .map_err(|source| StandaloneStoreError::CorruptRecord { id, source })?;
        if record.schema_version > SCHEMA_VERSION {
            return Err(StandaloneStoreError::NewerSchema {
                id,
                found: record.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        if record.schema_version != SCHEMA_VERSION || record.session_id != id {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        Ok(record)
    }

    pub fn list(
        &self,
        cwd: impl AsRef<Path>,
        scope: StandaloneListScope,
        limit: usize,
    ) -> Result<Vec<StandaloneSessionRecord>, StandaloneStoreError> {
        let current_cwd = (scope == StandaloneListScope::CurrentCwd)
            .then(|| StandaloneCwdIdentity::capture(cwd))
            .transpose()?;
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(StandaloneStoreError::Io)? {
            let entry = entry.map_err(StandaloneStoreError::Io)?;
            if !entry
                .file_type()
                .map_err(StandaloneStoreError::Io)?
                .is_dir()
            {
                continue;
            }
            let Ok(id) = entry.file_name().to_string_lossy().parse() else {
                continue;
            };
            let record = self.load(id)?;
            if current_cwd.as_ref().is_none_or(|cwd| &record.cwd == cwd) {
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .cmp(&left.updated_at_unix_ms)
                .then_with(|| {
                    right
                        .session_id
                        .to_string()
                        .cmp(&left.session_id.to_string())
                })
        });
        records.truncate(limit);
        Ok(records)
    }

    pub fn acquire_lease(
        &self,
        id: StandaloneSessionId,
        policy: StaleLeasePolicy,
    ) -> Result<StandaloneSessionLease, StandaloneStoreError> {
        let dir = self.session_dir(id);
        let path = dir.join(LEASE_FILE);
        let _guard = LeaseMutationGuard::acquire(&dir)?;
        let lease = LeaseRecord::current()?;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    serde_json::to_writer(&mut file, &lease).map_err(StandaloneStoreError::Json)?;
                    file.write_all(b"\n").map_err(StandaloneStoreError::Io)?;
                    file.sync_all().map_err(StandaloneStoreError::Io)?;
                    sync_directory(&dir)?;
                    return Ok(StandaloneSessionLease {
                        path,
                        lease_id: lease.lease_id,
                        released: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let existing = read_lease(&path, id)?;
                    match existing.liveness() {
                        LeaseLiveness::Live => {
                            return Err(StandaloneStoreError::SessionLeased(id));
                        }
                        LeaseLiveness::Unknown => {
                            return Err(StandaloneStoreError::LeaseLivenessUnknown(id));
                        }
                        LeaseLiveness::Stale => {}
                    }
                    if policy == StaleLeasePolicy::Reject {
                        return Err(StandaloneStoreError::StaleLease(id));
                    }
                    fs::remove_file(&path).map_err(StandaloneStoreError::Io)?;
                    sync_directory(&dir)?;
                }
                Err(error) => return Err(StandaloneStoreError::Io(error)),
            }
        }
    }

    pub fn update_active_pointer(
        &self,
        record: &StandaloneSessionRecord,
        active_session_id: SessionId,
        active_segment_id: Option<SegmentId>,
    ) -> Result<StandaloneSessionRecord, StandaloneStoreError> {
        let mut next = record.clone();
        next.revision = next.revision.saturating_add(1);
        next.updated_at_unix_ms = now_unix_ms()?;
        next.active_session_id = active_session_id;
        next.active_segment_id = active_segment_id;
        next.status = StandaloneSessionStatus::Active;
        next.shutdown_reason = None;
        self.commit_record(Some(record.revision), &next)?;
        Ok(next)
    }

    pub fn mark_stopped(
        &self,
        record: &StandaloneSessionRecord,
        active_session_id: SessionId,
        active_segment_id: Option<SegmentId>,
        reason: StandaloneShutdownReason,
    ) -> Result<StandaloneSessionRecord, StandaloneStoreError> {
        let mut next = record.clone();
        next.revision = next.revision.saturating_add(1);
        next.updated_at_unix_ms = now_unix_ms()?;
        next.active_session_id = active_session_id;
        next.active_segment_id = active_segment_id;
        next.status = StandaloneSessionStatus::Stopped;
        next.shutdown_reason = Some(reason);
        self.commit_record(Some(record.revision), &next)?;
        Ok(next)
    }

    pub fn delete(&self, id: StandaloneSessionId) -> Result<(), StandaloneStoreError> {
        let record = self.load(id)?;
        if record.status != StandaloneSessionStatus::Stopped {
            return Err(StandaloneStoreError::DeleteActive(id));
        }
        let session_dir = self.session_dir(id);
        let _guard = LeaseMutationGuard::acquire(&session_dir)?;
        let lease_path = session_dir.join(LEASE_FILE);
        if lease_path.exists() {
            let lease = read_lease(&lease_path, id)?;
            return Err(match lease.liveness() {
                LeaseLiveness::Live => StandaloneStoreError::SessionLeased(id),
                LeaseLiveness::Stale => StandaloneStoreError::StaleLease(id),
                LeaseLiveness::Unknown => StandaloneStoreError::LeaseLivenessUnknown(id),
            });
        }
        fs::remove_dir_all(self.session_dir(id)).map_err(StandaloneStoreError::Io)?;
        sync_directory(&self.root)
    }

    #[must_use]
    pub fn session_log_dir(&self, id: StandaloneSessionId) -> PathBuf {
        self.session_dir(id).join(SESSION_DIR)
    }

    #[must_use]
    pub fn worker_metadata_dir(&self, id: StandaloneSessionId) -> PathBuf {
        self.session_dir(id).join(WORKER_DIR)
    }

    #[must_use]
    pub(crate) fn runtime_dir(&self, id: StandaloneSessionId) -> PathBuf {
        self.session_dir(id).join("runtime")
    }

    pub(crate) fn abandon_allocation(
        &self,
        allocation: StandaloneSessionAllocation,
    ) -> Result<(), StandaloneStoreError> {
        let id = allocation.id;
        allocation.lease.release()?;
        fs::remove_dir_all(self.session_dir(id)).map_err(StandaloneStoreError::Io)?;
        sync_directory(&self.root)
    }

    fn commit_record(
        &self,
        expected_revision: Option<u64>,
        next: &StandaloneSessionRecord,
    ) -> Result<(), StandaloneStoreError> {
        let dir = self.session_dir(next.session_id);
        let marker = dir.join(COMMIT_MARKER);
        let mut marker_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    StandaloneStoreError::IncompleteCommit(next.session_id)
                } else {
                    StandaloneStoreError::Io(error)
                }
            })?;
        writeln!(marker_file, "{}", next.revision).map_err(StandaloneStoreError::Io)?;
        marker_file.sync_all().map_err(StandaloneStoreError::Io)?;
        sync_directory(&dir)?;

        if let Some(expected) = expected_revision {
            let current = self.load_record_while_committing(next.session_id)?;
            if current.revision != expected {
                let _ = fs::remove_file(&marker);
                return Err(StandaloneStoreError::RevisionConflict {
                    id: next.session_id,
                    expected,
                    found: current.revision,
                });
            }
        }

        let temporary = dir.join(format!("record.{}.tmp", Uuid::now_v7()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(StandaloneStoreError::Io)?;
            serde_json::to_writer_pretty(&mut file, next).map_err(StandaloneStoreError::Json)?;
            file.write_all(b"\n").map_err(StandaloneStoreError::Io)?;
            file.sync_all().map_err(StandaloneStoreError::Io)?;
            fs::rename(&temporary, dir.join(RECORD_FILE)).map_err(StandaloneStoreError::Io)?;
            sync_directory(&dir)?;
            fs::remove_file(&marker).map_err(StandaloneStoreError::Io)?;
            sync_directory(&dir)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn load_record_while_committing(
        &self,
        id: StandaloneSessionId,
    ) -> Result<StandaloneSessionRecord, StandaloneStoreError> {
        let bytes =
            fs::read(self.session_dir(id).join(RECORD_FILE)).map_err(StandaloneStoreError::Io)?;
        serde_json::from_slice(&bytes)
            .map_err(|source| StandaloneStoreError::CorruptRecord { id, source })
    }

    fn session_dir(&self, id: StandaloneSessionId) -> PathBuf {
        self.root.join(id.to_string())
    }
}

#[derive(Debug)]
pub struct StandaloneSessionAllocation {
    id: StandaloneSessionId,
    cwd: StandaloneCwdIdentity,
    lease: StandaloneSessionLease,
}

impl StandaloneSessionAllocation {
    #[must_use]
    pub fn id(&self) -> StandaloneSessionId {
        self.id
    }

    #[must_use]
    pub fn cwd(&self) -> &StandaloneCwdIdentity {
        &self.cwd
    }

    pub fn into_lease(self) -> StandaloneSessionLease {
        self.lease
    }
}

#[derive(Debug)]
pub struct StandaloneSessionLease {
    path: PathBuf,
    lease_id: Uuid,
    released: bool,
}

impl StandaloneSessionLease {
    pub fn release(mut self) -> Result<(), StandaloneStoreError> {
        self.release_inner()
    }

    pub(crate) fn retain(mut self) {
        self.released = true;
    }

    fn release_inner(&mut self) -> Result<(), StandaloneStoreError> {
        if self.released {
            return Ok(());
        }
        if self.path.exists() {
            let parent = self.path.parent().expect("lease parent");
            let _guard = LeaseMutationGuard::acquire(parent)?;
            let bytes = fs::read(&self.path).map_err(StandaloneStoreError::Io)?;
            let current: LeaseRecord =
                serde_json::from_slice(&bytes).map_err(StandaloneStoreError::Json)?;
            if current.lease_id != self.lease_id {
                return Err(StandaloneStoreError::LeaseOwnershipLost);
            }
            fs::remove_file(&self.path).map_err(StandaloneStoreError::Io)?;
            sync_directory(self.path.parent().expect("lease parent"))?;
        }
        self.released = true;
        Ok(())
    }
}

impl Drop for StandaloneSessionLease {
    fn drop(&mut self) {
        let _ = self.release_inner();
    }
}

struct LeaseMutationGuard {
    file: File,
}

impl LeaseMutationGuard {
    fn acquire(dir: &Path) -> Result<Self, StandaloneStoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LEASE_LOCK_FILE))
            .map_err(StandaloneStoreError::Io)?;
        file.lock_exclusive().map_err(StandaloneStoreError::Io)?;
        Ok(Self { file })
    }
}

impl Drop for LeaseMutationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LeaseRecord {
    lease_id: Uuid,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_marker: Option<u64>,
    acquired_at_unix_ms: u64,
}

impl LeaseRecord {
    fn current() -> Result<Self, StandaloneStoreError> {
        Ok(Self {
            lease_id: Uuid::now_v7(),
            pid: std::process::id(),
            process_start_marker: match observe_process(std::process::id()) {
                ProcessObservation::Running { start_marker } => Some(start_marker),
                ProcessObservation::Missing | ProcessObservation::Unobservable => None,
            },
            acquired_at_unix_ms: now_unix_ms()?,
        })
    }

    fn liveness(&self) -> LeaseLiveness {
        classify_lease_liveness(self.process_start_marker, observe_process(self.pid))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseLiveness {
    Live,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessObservation {
    Running { start_marker: u64 },
    Missing,
    Unobservable,
}

fn classify_lease_liveness(
    recorded_start_marker: Option<u64>,
    observation: ProcessObservation,
) -> LeaseLiveness {
    match (recorded_start_marker, observation) {
        (Some(recorded), ProcessObservation::Running { start_marker })
            if recorded == start_marker =>
        {
            LeaseLiveness::Live
        }
        (Some(_), ProcessObservation::Running { .. }) | (_, ProcessObservation::Missing) => {
            LeaseLiveness::Stale
        }
        (None, ProcessObservation::Running { .. }) | (_, ProcessObservation::Unobservable) => {
            LeaseLiveness::Unknown
        }
    }
}

fn read_lease(path: &Path, id: StandaloneSessionId) -> Result<LeaseRecord, StandaloneStoreError> {
    let bytes = fs::read(path).map_err(StandaloneStoreError::Io)?;
    serde_json::from_slice(&bytes)
        .map_err(|source| StandaloneStoreError::CorruptLease { id, source })
}

#[cfg(target_os = "linux")]
fn observe_process(pid: u32) -> ProcessObservation {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return if pid != std::process::id() && linux_proc_is_observable() {
                ProcessObservation::Missing
            } else {
                ProcessObservation::Unobservable
            };
        }
        Err(_) => return ProcessObservation::Unobservable,
    };
    parse_linux_process_start_marker(&stat)
        .map(|start_marker| ProcessObservation::Running { start_marker })
        .unwrap_or(ProcessObservation::Unobservable)
}

#[cfg(target_os = "linux")]
fn linux_proc_is_observable() -> bool {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| parse_linux_process_start_marker(&stat))
        .is_some()
}

#[cfg(target_os = "linux")]
fn parse_linux_process_start_marker(stat: &str) -> Option<u64> {
    let (_, tail) = stat.rsplit_once(") ")?;
    tail.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn observe_process(pid: u32) -> ProcessObservation {
    if pid == std::process::id() {
        ProcessObservation::Running { start_marker: 0 }
    } else {
        ProcessObservation::Unobservable
    }
}

fn now_unix_ms() -> Result<u64, StandaloneStoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StandaloneStoreError::Clock)?;
    u64::try_from(duration.as_millis()).map_err(|_| StandaloneStoreError::Clock)
}

fn sync_directory(path: &Path) -> Result<(), StandaloneStoreError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(StandaloneStoreError::Io)
}

#[derive(Debug, Error)]
pub enum StandaloneStoreError {
    #[error("standalone state path is not a directory")]
    NotDirectory,
    #[error("standalone cwd is unavailable")]
    CwdUnavailable(#[source] io::Error),
    #[error("standalone cwd is not a directory")]
    CwdNotDirectory,
    #[error("standalone cwd identity no longer matches the persisted session")]
    CwdIdentityMismatch,
    #[error("standalone session {0} was not found")]
    SessionNotFound(StandaloneSessionId),
    #[error("standalone session {0} has an incomplete metadata commit")]
    IncompleteCommit(StandaloneSessionId),
    #[error("standalone session {0} has invalid metadata")]
    InvalidRecord(StandaloneSessionId),
    #[error("standalone session {id} metadata is corrupt")]
    CorruptRecord {
        id: StandaloneSessionId,
        #[source]
        source: serde_json::Error,
    },
    #[error("standalone session {id} lease is corrupt")]
    CorruptLease {
        id: StandaloneSessionId,
        #[source]
        source: serde_json::Error,
    },
    #[error("standalone session {id} uses schema {found}, newer than supported schema {supported}")]
    NewerSchema {
        id: StandaloneSessionId,
        found: u32,
        supported: u32,
    },
    #[error("standalone session {0} is already active")]
    SessionLeased(StandaloneSessionId),
    #[error("standalone session {0} lease liveness cannot be proven; recovery is rejected")]
    LeaseLivenessUnknown(StandaloneSessionId),
    #[error("standalone session {0} has a stale lease; explicit recovery is required")]
    StaleLease(StandaloneSessionId),
    #[error("standalone session lease ownership changed")]
    LeaseOwnershipLost,
    #[error("standalone session {0} must be stopped before deletion")]
    DeleteActive(StandaloneSessionId),
    #[error(
        "standalone session {id} metadata revision changed (expected {expected}, found {found})"
    )]
    RevisionConflict {
        id: StandaloneSessionId,
        expected: u64,
        found: u64,
    },
    #[error("system clock is before the Unix epoch or out of range")]
    Clock,
    #[error("standalone metadata serialization failed")]
    Json(#[source] serde_json::Error),
    #[error("standalone state I/O failed")]
    Io(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::{LeaseLiveness, ProcessObservation, classify_lease_liveness};

    #[test]
    fn lease_liveness_requires_positive_live_or_stale_evidence() {
        assert_eq!(
            classify_lease_liveness(Some(41), ProcessObservation::Running { start_marker: 41 }),
            LeaseLiveness::Live
        );
        assert_eq!(
            classify_lease_liveness(Some(41), ProcessObservation::Running { start_marker: 42 }),
            LeaseLiveness::Stale
        );
        assert_eq!(
            classify_lease_liveness(Some(41), ProcessObservation::Missing),
            LeaseLiveness::Stale
        );
        assert_eq!(
            classify_lease_liveness(None, ProcessObservation::Running { start_marker: 41 }),
            LeaseLiveness::Unknown
        );
        assert_eq!(
            classify_lease_liveness(Some(41), ProcessObservation::Unobservable),
            LeaseLiveness::Unknown
        );
        assert_eq!(
            classify_lease_liveness(None, ProcessObservation::Unobservable),
            LeaseLiveness::Unknown
        );
    }
}
