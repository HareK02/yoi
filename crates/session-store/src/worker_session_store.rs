//! Filesystem store for the canonical `1 Worker = 1 Session` aggregate.
//!
//! Layout under one Worker aggregate:
//! - `session/session.json` — immutable Session identity
//! - `session/segments/<segment_id>.jsonl`
//! - `session/segments/<segment_id>.trace.jsonl`
//!
//! Unlike [`crate::FsStore`], this store cannot enumerate or switch between
//! arbitrary Sessions. The first segment materializes the sole Session identity;
//! every later operation must use that same ID.

use crate::event_trace::TraceEntry;
use crate::paste_artifact::{read_from_dir, write_to_dir};
use crate::segment_log::LogEntry;
use crate::store::{Store, StoreError};
use crate::uploaded_file::{
    bind_uploaded_file, clear_uploaded_file_binding, delete_uncommitted_uploaded_files,
    delete_uploaded_file, finalize_uploaded_file_binding, list_uploaded_file_refs,
    pin_uploaded_file, read_uploaded_file, read_uploaded_file_by_id, reconcile_uploaded_file_pins,
    release_uploaded_file_pin, uploaded_file_has_pending_owner, write_uploaded_file,
};
use crate::{
    PasteArtifactLimits, SegmentId, SessionId, UploadedFileLimits, UploadedFileUploadContext,
};
use protocol::{PasteArtifactRef, UploadedFileRef};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const SESSION_SCHEMA_VERSION: u32 = 3;
const PREVIOUS_SESSION_SCHEMA_VERSION: u32 = 2;
const LEGACY_SESSION_SCHEMA_VERSION: u32 = 1;
const SESSION_FILE: &str = "session.json";
const SEGMENTS_DIR: &str = "segments";
const PASTE_ARTIFACTS_DIR: &str = "artifacts/paste";

#[derive(Clone)]
pub struct WorkerSessionStore {
    root: PathBuf,
    session_id: Arc<Mutex<Option<SessionId>>>,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionManifest {
    schema_version: u32,
    session_id: SessionId,
}

const RETAINED_BACKWARD_READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct RetainedLogRecord {
    pub start_offset: u64,
    pub end_offset: u64,
    pub entry: LogEntry,
}

pub(crate) struct RetainedSegmentReader {
    file: File,
    session_id: SessionId,
    segment_id: SegmentId,
    read_end: u64,
    buffer_start: u64,
    buffer: Vec<u8>,
}

impl RetainedSegmentReader {
    pub fn previous_record(
        &mut self,
        max_additional_bytes: u64,
    ) -> Result<(Option<RetainedLogRecord>, u64), StoreError> {
        let mut read_bytes = 0_u64;
        loop {
            if !self.buffer.is_empty() {
                if self.buffer.last() != Some(&b'\n') {
                    return Err(StoreError::Corrupt {
                        line: 0,
                        message: "retained Segment cursor is not at a record boundary".to_string(),
                    });
                }
                let prior_newline = self.buffer[..self.buffer.len() - 1]
                    .iter()
                    .rposition(|byte| *byte == b'\n');
                if let Some(newline) =
                    prior_newline.or((self.buffer_start == 0).then_some(usize::MAX))
                {
                    let record_start_in_buffer = if newline == usize::MAX {
                        0
                    } else {
                        newline + 1
                    };
                    let record_start = self.buffer_start + record_start_in_buffer as u64;
                    let record_end = self.buffer_start + self.buffer.len() as u64;
                    let record =
                        self.buffer[record_start_in_buffer..self.buffer.len() - 1].to_vec();
                    self.buffer.truncate(record_start_in_buffer);
                    self.read_end = record_start;
                    if record.is_empty() {
                        return Err(StoreError::Corrupt {
                            line: 0,
                            message: "empty retained Segment log record".to_string(),
                        });
                    }
                    let mut entry: LogEntry = serde_json::from_slice(&record)?;
                    entry.ensure_legacy_session_entry_id(
                        self.session_id,
                        self.segment_id,
                        record_start,
                    );
                    return Ok((
                        Some(RetainedLogRecord {
                            start_offset: record_start,
                            end_offset: record_end,
                            entry,
                        }),
                        read_bytes,
                    ));
                }
            } else if self.read_end == 0 {
                return Ok((None, read_bytes));
            }

            if self.buffer_start == 0 && !self.buffer.is_empty() {
                return Err(StoreError::Corrupt {
                    line: 0,
                    message: "retained Segment log does not end at a record boundary".to_string(),
                });
            }
            if read_bytes >= max_additional_bytes {
                return Err(StoreError::ReadLimitExceeded);
            }
            let end = if self.buffer.is_empty() {
                self.read_end
            } else {
                self.buffer_start
            };
            let available = max_additional_bytes - read_bytes;
            let chunk_len = end
                .min(RETAINED_BACKWARD_READ_CHUNK_BYTES as u64)
                .min(available);
            if chunk_len == 0 {
                return Err(StoreError::ReadLimitExceeded);
            }
            let start = end - chunk_len;
            self.file.seek(SeekFrom::Start(start))?;
            let mut chunk = vec![0_u8; chunk_len as usize];
            self.file.read_exact(&mut chunk)?;
            read_bytes += chunk_len;
            chunk.extend_from_slice(&self.buffer);
            self.buffer = chunk;
            self.buffer_start = start;
        }
    }
}

impl WorkerSessionStore {
    /// Open the Session store rooted at `<worker-aggregate>/session`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(root.join(SEGMENTS_DIR))?;
        let session_id = match fs::read(root.join(SESSION_FILE)) {
            Ok(bytes) => {
                let mut manifest: SessionManifest = serde_json::from_slice(&bytes)?;
                match manifest.schema_version {
                    SESSION_SCHEMA_VERSION => {
                        validate_canonical_segment_logs(&root)?;
                    }
                    PREVIOUS_SESSION_SCHEMA_VERSION | LEGACY_SESSION_SCHEMA_VERSION => {
                        migrate_segment_logs_to_v3(
                            &root,
                            manifest.session_id,
                            manifest.schema_version,
                        )?;
                        manifest.schema_version = SESSION_SCHEMA_VERSION;
                        atomic_write_json(&root.join(SESSION_FILE), &manifest)?;
                    }
                    version => {
                        return Err(StoreError::Corrupt {
                            line: 0,
                            message: format!(
                                "unsupported Worker Session schema version {version}, expected {SESSION_SCHEMA_VERSION}"
                            ),
                        });
                    }
                }
                Some(manifest.session_id)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            root,
            session_id: Arc::new(Mutex::new(session_id)),
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Open an already-retained Session without creating directories or migrating
    /// persisted data. Observation paths must never mutate retained state.
    pub fn open_read_only(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let bytes = crate::read_without_atime(&root.join(SESSION_FILE))?;
        let manifest: SessionManifest = serde_json::from_slice(&bytes)?;
        if manifest.schema_version != SESSION_SCHEMA_VERSION {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker Session schema version {} requires migration; expected {}",
                    manifest.schema_version, SESSION_SCHEMA_VERSION
                ),
            });
        }
        Ok(Self {
            root,
            session_id: Arc::new(Mutex::new(Some(manifest.session_id))),
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn read_all_read_only(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        self.read_all_read_only_bounded(session_id, segment_id, u64::MAX)
            .map(|(entries, _)| entries)
    }

    /// Read one retained Segment under an actual byte bound without touching
    /// access time. The file descriptor read, rather than a preceding stat, is
    /// the limit authority so concurrent appends cannot evade accounting.
    pub fn read_all_read_only_bounded(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        max_bytes: u64,
    ) -> Result<(Vec<LogEntry>, u64), StoreError> {
        let retained_session_id = self.session_id.lock().map_err(|_| StoreError::Corrupt {
            line: 0,
            message: "Worker Session identity lock poisoned".to_string(),
        })?;
        if *retained_session_id != Some(session_id) {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "active Worker Session identity does not match retained manifest"
                    .to_string(),
            });
        }
        let path = self.log_path(segment_id);
        let bytes = crate::read_without_atime_bounded(&path, max_bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(segment_id)
            } else {
                StoreError::Io(error)
            }
        })?;
        let byte_len = bytes.len() as u64;
        if byte_len > max_bytes {
            return Err(StoreError::ReadLimitExceeded);
        }
        Ok((
            parse_session_log_jsonl(&bytes, session_id, segment_id)?,
            byte_len,
        ))
    }

    pub(crate) fn read_first_log_record_read_only_bounded(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        max_bytes: u64,
    ) -> Result<(Option<RetainedLogRecord>, u64, u64), StoreError> {
        let record_start = 0;
        self.validate_retained_session(session_id)?;
        let file = open_retained_file(&self.log_path(segment_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(segment_id)
            } else {
                StoreError::Io(error)
            }
        })?;
        let file_len = file.metadata()?.len();
        if file_len == 0 {
            return Ok((None, 0, 0));
        }
        let mut reader = BufReader::new(file.take(max_bytes.saturating_add(1)));
        let mut record = Vec::new();
        let read_bytes = reader.read_until(b'\n', &mut record)? as u64;
        if read_bytes > max_bytes || record.last() != Some(&b'\n') {
            return Err(StoreError::ReadLimitExceeded);
        }
        record.pop();
        if record.is_empty() {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "empty retained Segment log record".to_string(),
            });
        }
        let mut entry: LogEntry = serde_json::from_slice(&record)?;
        entry.ensure_legacy_session_entry_id(session_id, segment_id, record_start);
        Ok((
            Some(RetainedLogRecord {
                start_offset: 0,
                end_offset: read_bytes,
                entry,
            }),
            read_bytes,
            file_len,
        ))
    }

    pub(crate) fn read_next_log_record_read_only_bounded(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        start_offset: u64,
        max_bytes: u64,
    ) -> Result<(Option<RetainedLogRecord>, u64), StoreError> {
        let record_start = start_offset;
        self.validate_retained_session(session_id)?;
        let mut file = open_retained_file(&self.log_path(segment_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(segment_id)
            } else {
                StoreError::Io(error)
            }
        })?;
        let file_len = file.metadata()?.len();
        if start_offset >= file_len {
            return Ok((None, 0));
        }
        file.seek(SeekFrom::Start(start_offset))?;
        let mut reader = BufReader::new(file.take(max_bytes.saturating_add(1)));
        let mut record = Vec::new();
        let read_bytes = reader.read_until(b'\n', &mut record)? as u64;
        if read_bytes > max_bytes || record.last() != Some(&b'\n') {
            return Err(StoreError::ReadLimitExceeded);
        }
        record.pop();
        if record.is_empty() {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "empty retained Segment log record".to_string(),
            });
        }
        let mut entry: LogEntry = serde_json::from_slice(&record)?;
        entry.ensure_legacy_session_entry_id(session_id, segment_id, record_start);
        Ok((
            Some(RetainedLogRecord {
                start_offset,
                end_offset: start_offset + read_bytes,
                entry,
            }),
            read_bytes,
        ))
    }

    pub(crate) fn read_log_record_range_read_only_bounded(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        start_offset: u64,
        end_offset: u64,
        max_bytes: u64,
    ) -> Result<RetainedLogRecord, StoreError> {
        let record_start = start_offset;
        self.validate_retained_session(session_id)?;
        let record_len =
            end_offset
                .checked_sub(start_offset)
                .ok_or_else(|| StoreError::Corrupt {
                    line: 0,
                    message: "retained Segment record range is reversed".to_string(),
                })?;
        if record_len == 0 || record_len > max_bytes {
            return Err(StoreError::ReadLimitExceeded);
        }
        let mut file = open_retained_file(&self.log_path(segment_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(segment_id)
            } else {
                StoreError::Io(error)
            }
        })?;
        if end_offset > file.metadata()?.len() {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "retained Segment record range exceeds the committed log".to_string(),
            });
        }
        file.seek(SeekFrom::Start(start_offset))?;
        let mut record = vec![0_u8; record_len as usize];
        file.read_exact(&mut record)?;
        if record.last() != Some(&b'\n') || record[..record.len() - 1].contains(&b'\n') {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "retained Segment cursor does not identify one record".to_string(),
            });
        }
        record.pop();
        let mut entry: LogEntry = serde_json::from_slice(&record)?;
        entry.ensure_legacy_session_entry_id(session_id, segment_id, record_start);
        Ok(RetainedLogRecord {
            start_offset,
            end_offset,
            entry,
        })
    }

    pub(crate) fn open_retained_segment_reader(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        before_offset: u64,
    ) -> Result<RetainedSegmentReader, StoreError> {
        self.validate_retained_session(session_id)?;
        let mut file = open_retained_file(&self.log_path(segment_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(segment_id)
            } else {
                StoreError::Io(error)
            }
        })?;
        let file_len = file.metadata()?.len();
        if before_offset > file_len {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "retained Segment cursor is beyond the committed log".to_string(),
            });
        }
        if before_offset > 0 {
            file.seek(SeekFrom::Start(before_offset - 1))?;
            let mut boundary = [0_u8; 1];
            file.read_exact(&mut boundary)?;
            if boundary[0] != b'\n' {
                return Err(StoreError::Corrupt {
                    line: 0,
                    message: "retained Segment cursor is not at a record boundary".to_string(),
                });
            }
        }
        Ok(RetainedSegmentReader {
            file,
            session_id,
            segment_id,
            read_end: before_offset,
            buffer_start: before_offset,
            buffer: Vec::new(),
        })
    }

    fn validate_retained_session(&self, requested: SessionId) -> Result<(), StoreError> {
        let retained_session_id = self.session_id.lock().map_err(|_| StoreError::Corrupt {
            line: 0,
            message: "Worker Session identity lock poisoned".to_string(),
        })?;
        if *retained_session_id != Some(requested) {
            return Err(StoreError::Corrupt {
                line: 0,
                message: "active Worker Session identity does not match retained manifest"
                    .to_string(),
            });
        }
        Ok(())
    }

    /// Enumerate canonical Segment logs in ascending identity order without
    /// creating or migrating state. Unlike the general [`Store`] listing, this
    /// observation boundary rejects malformed names and non-regular files so a
    /// persisted Segment cannot be silently omitted from a public index.
    pub(crate) fn list_segments_read_only(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<SegmentId>, StoreError> {
        self.validate_retained_session(session_id)?;
        segment_log_paths(&self.root).map(|paths| {
            paths
                .into_iter()
                .map(|(segment_id, _)| segment_id)
                .collect()
        })
    }

    pub(crate) fn segment_log_observation(
        &self,
        segment_id: SegmentId,
    ) -> Result<(u64, u128), StoreError> {
        let metadata = fs::metadata(self.log_path(segment_id))?;
        let modified = metadata
            .modified()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| StoreError::Corrupt {
                line: 0,
                message: "retained Segment modification time predates the Unix epoch".to_string(),
            })?
            .as_nanos();
        Ok((metadata.len(), modified))
    }

    pub fn segment_log_len(&self, segment_id: SegmentId) -> Result<u64, StoreError> {
        Ok(fs::metadata(self.log_path(segment_id))?.len())
    }

    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    pub fn session_id(&self) -> Result<Option<SessionId>, StoreError> {
        self.session_id
            .lock()
            .map(|session_id| *session_id)
            .map_err(|_| std::io::Error::other("Worker Session identity lock was poisoned").into())
    }

    pub fn session_modified_at(&self) -> Result<Option<SystemTime>, StoreError> {
        let metadata = match fs::metadata(&self.root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut latest = Some(metadata.modified()?);
        for entry in fs::read_dir(self.root.join(SEGMENTS_DIR))? {
            let modified = entry?.metadata()?.modified()?;
            if latest.map(|current| modified > current).unwrap_or(true) {
                latest = Some(modified);
            }
        }
        Ok(latest)
    }

    fn ensure_session(&self, requested: SessionId, materialize: bool) -> Result<(), StoreError> {
        let mut session_id = self
            .session_id
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session identity lock was poisoned"))?;
        match *session_id {
            Some(existing) if existing == requested => Ok(()),
            Some(existing) => Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker aggregate owns Session {existing}; cannot attach or switch to Session {requested}"
                ),
            }),
            None if !materialize => Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker aggregate has no materialized Session; requested Session {requested}"
                ),
            }),
            None => {
                let manifest = SessionManifest {
                    schema_version: SESSION_SCHEMA_VERSION,
                    session_id: requested,
                };
                atomic_write_json(&self.root.join(SESSION_FILE), &manifest)?;
                *session_id = Some(requested);
                Ok(())
            }
        }
    }

    fn log_path(&self, segment_id: SegmentId) -> PathBuf {
        self.root
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"))
    }

    fn trace_path(&self, segment_id: SegmentId) -> PathBuf {
        self.root
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.trace.jsonl"))
    }

    fn uploaded_file_is_referenced(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<bool, StoreError> {
        fn segments_contain(segments: &[protocol::Segment], artifact_id: &str) -> bool {
            segments.iter().any(|segment| {
                matches!(
                    segment,
                    protocol::Segment::UploadedFile { file }
                        if file.artifact_id == artifact_id
                )
            })
        }

        for segment_id in self.list_segments(session_id)? {
            for entry in self.read_all(session_id, segment_id)? {
                let referenced = match entry {
                    LogEntry::AnnotatedUserInput { segments, .. } => {
                        segments_contain(&segments, artifact_id)
                    }
                    LogEntry::InputSegmentsCheckpoint { user_segments, .. } => user_segments
                        .iter()
                        .any(|segments| segments_contain(segments, artifact_id)),
                    _ => false,
                };
                if referenced {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn append_log_entry(&self, path: &Path, entry: &LogEntry) -> Result<(), StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(true)
            .open(path)?;
        let committed_len = truncate_uncommitted_tail(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        let mut existing = Vec::new();
        file.read_to_end(&mut existing)?;
        parse_jsonl::<LogEntry>(&existing)?;
        let line = serde_json::to_string(entry)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line.as_bytes());
        record.push(b'\n');
        if let Err(write_error) = file.write_all(&record) {
            return match file.set_len(committed_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "session append failed ({write_error}) and rollback failed: {rollback_error}"
                    ),
                )
                .into()),
            };
        }
        Ok(())
    }

    fn append_line(&self, path: &Path, line: &str) -> Result<(), StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(true)
            .open(path)?;
        let committed_len = truncate_uncommitted_tail(&mut file)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line.as_bytes());
        record.push(b'\n');
        if let Err(write_error) = file.write_all(&record) {
            return match file.set_len(committed_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "session append failed ({write_error}) and rollback failed: {rollback_error}"
                    ),
                )
                .into()),
            };
        }
        Ok(())
    }
}

impl Store for WorkerSessionStore {
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        self.append_log_entry(&self.log_path(segment_id), entry)
    }

    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        self.ensure_session(session_id, false)?;
        let path = self.log_path(segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        parse_session_log_jsonl(&fs::read(path)?, session_id, segment_id)
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        Ok(self.session_id()?.into_iter().collect())
    }

    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
        self.ensure_session(session_id, false)?;
        let mut segments: Vec<SegmentId> = Vec::new();
        for entry in fs::read_dir(self.root.join(SEGMENTS_DIR))? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.ends_with(".jsonl")
                && !name.ends_with(".trace.jsonl")
                && let Ok(segment_id) = name.trim_end_matches(".jsonl").parse()
            {
                segments.push(segment_id);
            }
        }
        segments.sort_by(|left, right| right.cmp(left));
        Ok(segments)
    }

    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError> {
        let session_id = self.session_id()?;
        Ok(session_id.filter(|_| self.log_path(segment_id).exists()))
    }

    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        let mut content = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut content, entry)?;
            content.push(b'\n');
        }
        atomic_write_bytes(&self.log_path(segment_id), &content)?;
        Ok(())
    }

    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
        self.ensure_session(session_id, false)?;
        Ok(self.log_path(segment_id).exists())
    }

    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError> {
        self.ensure_session(session_id, false)?;
        let path = self.log_path(segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        let content = fs::read(path)?;
        let complete = complete_jsonl_prefix(&content);
        let complete = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
            line: complete[..error.valid_up_to()]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
            message: error.to_string(),
        })?;
        Ok(complete
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    fn write_paste_artifact(
        &self,
        session_id: SessionId,
        source_entry_id: &str,
        content: &str,
        limits: PasteArtifactLimits,
    ) -> Result<PasteArtifactRef, StoreError> {
        self.ensure_session(session_id, true)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        write_to_dir(
            &self.root.join(PASTE_ARTIFACTS_DIR),
            source_entry_id,
            content,
            limits,
        )
    }

    fn read_paste_artifact(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<(PasteArtifactRef, String), StoreError> {
        self.ensure_session(session_id, false)?;
        read_from_dir(&self.root.join(PASTE_ARTIFACTS_DIR), artifact_id)
    }

    fn write_uploaded_file(
        &self,
        session_id: SessionId,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        limits: UploadedFileLimits,
    ) -> Result<UploadedFileRef, StoreError> {
        self.ensure_session(session_id, true)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        write_uploaded_file(
            &self.root.join(PASTE_ARTIFACTS_DIR),
            file_name,
            media_type,
            content,
            None,
            limits,
        )
    }

    fn write_uploaded_file_with_context(
        &self,
        session_id: SessionId,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        context: &UploadedFileUploadContext,
        limits: UploadedFileLimits,
    ) -> Result<UploadedFileRef, StoreError> {
        self.ensure_session(session_id, true)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        write_uploaded_file(
            &self.root.join(PASTE_ARTIFACTS_DIR),
            file_name,
            media_type,
            content,
            Some(context),
            limits,
        )
    }

    fn read_uploaded_file(
        &self,
        session_id: SessionId,
        reference: &UploadedFileRef,
    ) -> Result<Vec<u8>, StoreError> {
        self.ensure_session(session_id, false)?;
        read_uploaded_file(&self.root.join(PASTE_ARTIFACTS_DIR), reference)
    }

    fn read_uploaded_file_by_id(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<(UploadedFileRef, Vec<u8>), StoreError> {
        self.ensure_session(session_id, false)?;
        read_uploaded_file_by_id(&self.root.join(PASTE_ARTIFACTS_DIR), artifact_id)
    }

    fn bind_uploaded_file(
        &self,
        session_id: SessionId,
        reference: &UploadedFileRef,
        source_entry_id: &str,
    ) -> Result<UploadedFileRef, StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let dir = self.root.join(PASTE_ARTIFACTS_DIR);
        match bind_uploaded_file(&dir, reference, source_entry_id) {
            Err(StoreError::ArtifactAlreadyCommitted) => {
                let (stored, _) = read_uploaded_file_by_id(&dir, &reference.artifact_id)?;
                let previous_source = stored
                    .source_entry_id
                    .ok_or(StoreError::ArtifactIntegrityMismatch)?;
                if self.uploaded_file_is_referenced(session_id, &reference.artifact_id)? {
                    return Err(StoreError::ArtifactAlreadyCommitted);
                }
                clear_uploaded_file_binding(&dir, &reference.artifact_id, &previous_source)?;
                bind_uploaded_file(&dir, reference, source_entry_id)
            }
            result => result,
        }
    }

    fn pin_uploaded_file(
        &self,
        session_id: SessionId,
        reference: &UploadedFileRef,
        owner_id: &str,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        pin_uploaded_file(&self.root.join(PASTE_ARTIFACTS_DIR), reference, owner_id)
    }

    fn release_uploaded_file_pin(
        &self,
        session_id: SessionId,
        artifact_id: &str,
        owner_id: &str,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        release_uploaded_file_pin(&self.root.join(PASTE_ARTIFACTS_DIR), artifact_id, owner_id)
    }

    fn finalize_uploaded_file_binding(
        &self,
        session_id: SessionId,
        artifact_id: &str,
        source_entry_id: &str,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        finalize_uploaded_file_binding(
            &self.root.join(PASTE_ARTIFACTS_DIR),
            artifact_id,
            source_entry_id,
        )
    }

    fn reconcile_uploaded_file_pins(
        &self,
        session_id: SessionId,
        live_owner_ids: &[String],
    ) -> Result<u64, StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        reconcile_uploaded_file_pins(&self.root.join(PASTE_ARTIFACTS_DIR), live_owner_ids)
    }

    fn delete_uploaded_file(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<bool, StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        delete_uploaded_file(&self.root.join(PASTE_ARTIFACTS_DIR), artifact_id)
    }

    fn delete_uncommitted_uploaded_files(&self, session_id: SessionId) -> Result<u64, StoreError> {
        self.ensure_session(session_id, false)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let dir = self.root.join(PASTE_ARTIFACTS_DIR);
        let mut removed = delete_uncommitted_uploaded_files(&dir)?;
        for reference in list_uploaded_file_refs(&dir)? {
            let Some(source_entry_id) = reference.source_entry_id.as_deref() else {
                continue;
            };
            if self.uploaded_file_is_referenced(session_id, &reference.artifact_id)? {
                finalize_uploaded_file_binding(&dir, &reference.artifact_id, source_entry_id)?;
                continue;
            }
            if uploaded_file_has_pending_owner(&dir, &reference.artifact_id)? {
                continue;
            }
            clear_uploaded_file_binding(&dir, &reference.artifact_id, source_entry_id)?;
            if delete_uploaded_file(&dir, &reference.artifact_id)? {
                removed = removed
                    .checked_add(1)
                    .ok_or(StoreError::ArtifactQuotaExceeded)?;
            }
        }
        Ok(removed)
    }

    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        self.append_line(&self.trace_path(segment_id), &serde_json::to_string(entry)?)
    }
}

fn open_retained_file(path: &Path) -> std::io::Result<File> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "read-only retained observation requires no-atime file reads",
        ))
    }
}

fn segment_log_paths(root: &Path) -> Result<Vec<(SegmentId, PathBuf)>, StoreError> {
    let segments = root.join(SEGMENTS_DIR);
    if !segments.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(&segments)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!("non-UTF-8 Worker Session segment path: {}", path.display()),
            });
        };
        if name.ends_with(".trace.jsonl") || name.starts_with('.') {
            continue;
        }
        if !name.ends_with(".jsonl") {
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker Session segment is not a regular file: {}",
                    path.display()
                ),
            });
        }
        let segment_id =
            name.trim_end_matches(".jsonl")
                .parse()
                .map_err(|_| StoreError::Corrupt {
                    line: 0,
                    message: format!("invalid Worker Session segment name: {name}"),
                })?;
        paths.push((segment_id, path));
    }
    paths.sort_by_key(|(segment_id, _)| *segment_id);
    Ok(paths)
}

fn migrate_segment_logs_to_v3(
    root: &Path,
    session_id: SessionId,
    source_schema_version: u32,
) -> Result<(), StoreError> {
    struct MigrationPlan {
        path: PathBuf,
        source: Vec<u8>,
        output: Vec<u8>,
    }

    // Phase 1 is strictly read-only. Every segment must parse and canonicalize
    // successfully before the first authoritative byte is replaced.
    let mut plans = Vec::new();
    for (segment_id, path) in segment_log_paths(root)? {
        let source = fs::read(&path)?;
        let canonical = parse_legacy_jsonl(source_schema_version, session_id, segment_id, &source)
            .map_err(|error| StoreError::Corrupt {
                line: 0,
                message: format!(
                    "cannot migrate Worker Session log {}: {error}",
                    path.display()
                ),
            })?;
        let mut output = Vec::new();
        for entry in canonical {
            serde_json::to_writer(&mut output, &entry)?;
            output.push(b'\n');
        }
        plans.push(MigrationPlan {
            path,
            source,
            output,
        });
    }

    // Fence the complete preflight snapshot before starting phase 2. Session
    // open is the exclusive restore boundary; this additionally fails closed
    // if an unexpected writer raced the preflight.
    for plan in &plans {
        if fs::read(&plan.path)? != plan.source {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker Session segment changed during migration: {}",
                    plan.path.display()
                ),
            });
        }
    }

    for plan in plans {
        atomic_write_bytes(&plan.path, &plan.output)?;
    }
    Ok(())
}

fn validate_canonical_segment_logs(root: &Path) -> Result<(), StoreError> {
    for (_, path) in segment_log_paths(root)? {
        let _: Vec<LogEntry> = parse_jsonl(&fs::read(&path)?)?;
    }
    Ok(())
}

fn parse_legacy_jsonl(
    schema_version: u32,
    session_id: SessionId,
    segment_id: SegmentId,
    bytes: &[u8],
) -> Result<Vec<LogEntry>, serde_json::Error> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line_index, line)| {
            crate::legacy_session_log::decode_entry(
                schema_version,
                line,
                session_id,
                segment_id,
                line_index,
            )
        })
        .collect()
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write_bytes(path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Worker Session path has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session"),
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let result = (|| -> Result<(), StoreError> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn complete_jsonl_prefix(content: &[u8]) -> &[u8] {
    if content.last() == Some(&b'\n') {
        return content;
    }
    content
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| &content[..=index])
        .unwrap_or(&[])
}

fn parse_session_log_jsonl(
    content: &[u8],
    session_id: SessionId,
    segment_id: SegmentId,
) -> Result<Vec<LogEntry>, StoreError> {
    let complete = complete_jsonl_prefix(content);
    let content = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
        line: complete[..error.valid_up_to()]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1,
        message: error.to_string(),
    })?;
    let mut entries = Vec::new();
    let mut record_start = 0_u64;
    for (index, record) in content.split_inclusive('\n').enumerate() {
        let line = record.strip_suffix('\n').unwrap_or(record);
        if !line.trim().is_empty() {
            let mut entry: LogEntry =
                serde_json::from_str(line).map_err(|error| StoreError::Corrupt {
                    line: index + 1,
                    message: error.to_string(),
                })?;
            entry.ensure_legacy_session_entry_id(session_id, segment_id, record_start);
            entries.push(entry);
        }
        record_start = record_start.saturating_add(record.len() as u64);
    }
    Ok(entries)
}

fn parse_jsonl<T: serde::de::DeserializeOwned>(content: &[u8]) -> Result<Vec<T>, StoreError> {
    let complete = complete_jsonl_prefix(content);
    let content = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
        line: complete[..error.valid_up_to()]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1,
        message: error.to_string(),
    })?;
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| StoreError::Corrupt {
                line: index + 1,
                message: error.to_string(),
            })
        })
        .collect()
}

fn truncate_uncommitted_tail(file: &mut File) -> std::io::Result<u64> {
    const SCAN_BYTES: usize = 8 * 1024;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(len);
    }
    let mut end = len;
    let mut buffer = [0_u8; SCAN_BYTES];
    while end > 0 {
        let start = end.saturating_sub(SCAN_BYTES as u64);
        let chunk_len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..chunk_len])?;
        if let Some(index) = buffer[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            let committed_len = start + index as u64 + 1;
            file.set_len(committed_len)?;
            return Ok(committed_len);
        }
        end = start;
    }
    file.set_len(0)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LoggedHistoryEntry, LoggedItem, LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata,
        LoggedSessionHistoryOrigin, Store, new_segment_id, new_session_id,
    };

    fn annotated(item: agen::Item) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item: LoggedItem::from(item),
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId::new(),
                origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        }
    }

    #[test]
    fn canonical_combined_store_supports_attachment_uploads() {
        let root = tempfile::tempdir().unwrap();
        let session = WorkerSessionStore::new(root.path().join("session")).unwrap();
        let store = crate::CombinedStore::new(session.clone(), ());
        let session_id = new_session_id();
        store
            .create_segment(session_id, new_segment_id(), &[])
            .unwrap();
        let context = crate::UploadedFileUploadContext {
            upload_id: "upload-1".into(),
            principal_id: "account-1".into(),
            workspace_id: "workspace-1".into(),
            runtime_id: "runtime-1".into(),
            worker_id: "worker-1".into(),
        };
        let file = store
            .write_uploaded_file_with_context(
                session_id,
                "image.png",
                "image/png",
                b"\x89PNG\r\n\x1a\n",
                &context,
                crate::UploadedFileLimits::default(),
            )
            .expect("canonical Runtime store must support attachment upload");
        assert_eq!(
            store.read_uploaded_file(session_id, &file).unwrap(),
            b"\x89PNG\r\n\x1a\n"
        );
        assert_eq!(
            session.read_uploaded_file(session_id, &file).unwrap(),
            b"\x89PNG\r\n\x1a\n"
        );
        assert_eq!(
            store
                .write_uploaded_file_with_context(
                    session_id,
                    "image.png",
                    "image/png",
                    b"\x89PNG\r\n\x1a\n",
                    &context,
                    crate::UploadedFileLimits::default(),
                )
                .unwrap(),
            file
        );
        assert!(
            store
                .write_uploaded_file_with_context(
                    session_id,
                    "image.png",
                    "image/png",
                    b"\x89PNG\r\n\x1a\n",
                    &crate::UploadedFileUploadContext {
                        principal_id: "different-account".into(),
                        ..context
                    },
                    crate::UploadedFileLimits::default(),
                )
                .is_err(),
            "CombinedStore must not discard authenticated upload context"
        );
        let reopened = WorkerSessionStore::new(root.path().join("session")).unwrap();
        assert_eq!(
            reopened
                .read_uploaded_file_by_id(session_id, &file.artifact_id)
                .unwrap()
                .0,
            file
        );
        assert!(matches!(
            store.read_uploaded_file(new_session_id(), &file),
            Err(StoreError::Corrupt { .. })
        ));
        assert!(
            store
                .delete_uploaded_file(session_id, &file.artifact_id)
                .unwrap()
        );
    }

    #[test]
    fn canonical_attachment_pins_binding_and_cleanup_survive_reopen() {
        let root = tempfile::tempdir().unwrap();
        let store = crate::CombinedStore::new(WorkerSessionStore::new(root.path()).unwrap(), ());
        let id = new_session_id();
        let limits = UploadedFileLimits::default();
        store.create_segment(id, new_segment_id(), &[]).unwrap();
        let file = store
            .write_uploaded_file(id, "pending.txt", "text/plain", b"pending", limits)
            .unwrap();
        store.pin_uploaded_file(id, &file, "submit-1").unwrap();
        assert!(store.delete_uploaded_file(id, &file.artifact_id).is_err());
        drop(store);
        let store = crate::CombinedStore::new(WorkerSessionStore::new(root.path()).unwrap(), ());
        assert_eq!(
            store
                .reconcile_uploaded_file_pins(id, &["submit-1".into()])
                .unwrap(),
            0
        );
        assert_eq!(store.delete_uncommitted_uploaded_files(id).unwrap(), 0);
        let bound = store.bind_uploaded_file(id, &file, "entry-1").unwrap();
        store
            .create_segment(
                id,
                new_segment_id(),
                &[LogEntry::InputSegmentsCheckpoint {
                    ts: 1,
                    user_segments: vec![vec![protocol::Segment::UploadedFile {
                        file: bound.clone(),
                    }]],
                }],
            )
            .unwrap();
        store
            .finalize_uploaded_file_binding(id, &file.artifact_id, "entry-1")
            .unwrap();
        assert_eq!(store.delete_uncommitted_uploaded_files(id).unwrap(), 0);
        assert_eq!(store.read_uploaded_file(id, &bound).unwrap(), b"pending");
        assert!(
            store
                .bind_uploaded_file(id, &file, "different-entry")
                .is_err()
        );
        assert!(store.delete_uploaded_file(id, &file.artifact_id).is_err());

        let cancelled = store
            .write_uploaded_file(id, "cancelled.txt", "text/plain", b"cancelled", limits)
            .unwrap();
        store.pin_uploaded_file(id, &cancelled, "submit-2").unwrap();
        store
            .release_uploaded_file_pin(id, &cancelled.artifact_id, "submit-2")
            .unwrap();
        assert_eq!(store.delete_uncommitted_uploaded_files(id).unwrap(), 1);
        let orphan = store
            .write_uploaded_file(id, "orphan.txt", "text/plain", b"orphan", limits)
            .unwrap();
        store.pin_uploaded_file(id, &orphan, "lost-owner").unwrap();
        assert_eq!(store.reconcile_uploaded_file_pins(id, &[]).unwrap(), 1);
        assert_eq!(store.delete_uncommitted_uploaded_files(id).unwrap(), 1);

        let retry = store
            .write_uploaded_file(id, "retry.txt", "text/plain", b"retry", limits)
            .unwrap();
        store
            .bind_uploaded_file(id, &retry, "failed-entry")
            .unwrap();
        store.bind_uploaded_file(id, &retry, "retry-entry").unwrap();
        assert_eq!(store.delete_uncommitted_uploaded_files(id).unwrap(), 1);
        assert_eq!(store.read_uploaded_file(id, &bound).unwrap(), b"pending");
    }

    #[test]
    fn canonical_attachment_limits_integrity_and_session_fences_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let store = crate::CombinedStore::new(WorkerSessionStore::new(root.path()).unwrap(), ());
        let id = new_session_id();
        let limits = UploadedFileLimits {
            max_file_bytes: 8,
            max_session_bytes: 8,
        };
        store.create_segment(id, new_segment_id(), &[]).unwrap();
        assert!(matches!(
            store.write_uploaded_file(id, "../escape", "text/plain", b"x", limits),
            Err(StoreError::InvalidUploadedFileName)
        ));
        assert!(matches!(
            store.write_uploaded_file(id, "image.png", "image/png", b"invalid", limits),
            Err(StoreError::ArtifactIntegrityMismatch)
        ));
        assert!(matches!(
            store.write_uploaded_file(id, "large.txt", "text/plain", b"123456789", limits),
            Err(StoreError::ArtifactTooLarge)
        ));
        let file = store
            .write_uploaded_file(id, "notes.txt", "text/plain", b"1234", limits)
            .unwrap();
        let mut forged = file.clone();
        forged.sha256 = "invalid".into();
        assert!(matches!(
            store.read_uploaded_file(id, &forged),
            Err(StoreError::ArtifactIntegrityMismatch)
        ));
        let other = new_session_id();
        assert!(
            store
                .write_uploaded_file(other, "other.txt", "text/plain", b"x", limits)
                .is_err()
        );
        assert!(store.pin_uploaded_file(other, &file, "owner").is_err());
        assert!(
            store
                .delete_uploaded_file(other, &file.artifact_id)
                .is_err()
        );
        store
            .write_paste_artifact(id, "paste-entry", "1234", PasteArtifactLimits::default())
            .unwrap();
        assert!(matches!(
            store.write_uploaded_file(id, "overflow.txt", "text/plain", b"x", limits),
            Err(StoreError::ArtifactQuotaExceeded)
        ));
    }

    #[test]
    fn canonical_layout_and_single_session_invariant() {
        let root = tempfile::tempdir().unwrap();
        let store = WorkerSessionStore::new(root.path().join("session")).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        store.create_segment(session_id, segment_id, &[]).unwrap();

        assert!(root.path().join("session/session.json").is_file());
        assert!(
            root.path()
                .join(format!("session/segments/{segment_id}.jsonl"))
                .is_file()
        );
        assert_eq!(store.list_sessions().unwrap(), vec![session_id]);

        let other = new_session_id();
        let error = store
            .create_segment(other, new_segment_id(), &[])
            .unwrap_err();
        assert!(error.to_string().contains("cannot attach or switch"));
        assert_eq!(store.list_sessions().unwrap(), vec![session_id]);
    }

    #[test]
    fn worker_session_store_keeps_paste_artifacts_inside_retention_root() {
        let root = tempfile::tempdir().unwrap();
        let store = WorkerSessionStore::new(root.path().join("session")).unwrap();
        let session_id = new_session_id();
        store
            .create_segment(session_id, new_segment_id(), &[])
            .unwrap();
        let content = "large paste body\n終端\n";
        let reference = store
            .write_paste_artifact(
                session_id,
                "entry-1",
                content,
                PasteArtifactLimits::default(),
            )
            .unwrap();

        assert!(
            root.path()
                .join(format!(
                    "session/{PASTE_ARTIFACTS_DIR}/{}.json",
                    reference.artifact_id
                ))
                .is_file()
        );
        assert_eq!(
            store
                .read_paste_artifact(session_id, &reference.artifact_id)
                .unwrap()
                .1,
            content
        );
        assert!(matches!(
            store.read_paste_artifact(new_session_id(), &reference.artifact_id),
            Err(StoreError::Corrupt { .. })
        ));
    }

    #[test]
    fn schema_v1_logs_are_rewritten_and_promoted_to_v3() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        WorkerSessionStore::new(root.path())
            .unwrap()
            .create_segment(session_id, segment_id, &[])
            .unwrap();
        let manifest_path = root.path().join(SESSION_FILE);
        let mut manifest: SessionManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.schema_version = LEGACY_SESSION_SCHEMA_VERSION;
        atomic_write_json(&manifest_path, &manifest).unwrap();

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(reopened.session_id().unwrap(), Some(session_id));
        let migrated: SessionManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(migrated.schema_version, SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn schema_v1_migration_rejects_corrupt_log_before_v3_manifest_update() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let manifest = SessionManifest {
            schema_version: LEGACY_SESSION_SCHEMA_VERSION,
            session_id,
        };
        atomic_write_json(&root.path().join(SESSION_FILE), &manifest).unwrap();
        fs::create_dir_all(root.path().join(SEGMENTS_DIR)).unwrap();
        fs::write(
            root.path().join(SEGMENTS_DIR).join("broken.jsonl"),
            "{not-json}\n",
        )
        .unwrap();

        let error = match WorkerSessionStore::new(root.path()) {
            Ok(_) => panic!("corrupt legacy Session log must reject migration"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::Corrupt { .. }));
        let persisted: SessionManifest =
            serde_json::from_slice(&fs::read(root.path().join(SESSION_FILE)).unwrap()).unwrap();
        assert_eq!(persisted.schema_version, LEGACY_SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn schema_v2_migration_rewrites_legacy_records_with_stable_unknown_provenance() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        fs::create_dir_all(root.path().join(SEGMENTS_DIR)).unwrap();
        atomic_write_json(
            &root.path().join(SESSION_FILE),
            &SessionManifest {
                schema_version: PREVIOUS_SESSION_SCHEMA_VERSION,
                session_id,
            },
        )
        .unwrap();
        let source = vec![
            serde_json::json!({
                "kind": "segment_start",
                "ts": 1,
                "session_id": session_id,
                "system_prompt": null,
                "config": agen::llm_client::RequestConfig::default(),
                "history": [LoggedItem::from(agen::Item::assistant_message("prior"))],
                "forked_from": null,
                "compacted_from": null
            }),
            serde_json::json!({
                "kind": "user_input",
                "ts": 2,
                "segments": [{ "kind": "text", "content": "hello" }],
                "extensions": []
            }),
            serde_json::json!({
                "kind": "assistant_item",
                "ts": 3,
                "item": LoggedItem::from(agen::Item::assistant_message("reply"))
            }),
        ];
        let path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"));
        let mut bytes = Vec::new();
        for entry in source {
            serde_json::to_writer(&mut bytes, &entry).unwrap();
            bytes.push(b'\n');
        }
        fs::write(&path, bytes).unwrap();

        let store = WorkerSessionStore::new(root.path()).unwrap();
        let first = store.read_all(session_id, segment_id).unwrap();
        assert!(matches!(first[0], LogEntry::AnnotatedSegmentStart { .. }));
        assert!(matches!(first[1], LogEntry::AnnotatedUserInput { .. }));
        assert!(matches!(first[2], LogEntry::AnnotatedAssistantItem { .. }));
        let first_bytes = fs::read(&path).unwrap();
        drop(store);

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first_bytes);
        let snapshot = crate::public_snapshot::project_current_session_snapshot(
            &reopened.read_all(session_id, segment_id).unwrap(),
        );
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|entry| entry.timestamp)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(snapshot.entries.iter().all(|entry| {
            entry.provenance == protocol::SessionEntryProvenance::LegacyUnknown
                && entry.entry_id.len() <= 64
        }));
    }

    #[test]
    fn schema_v2_preflight_keeps_earlier_segments_unchanged_when_later_is_corrupt() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let valid_segment = uuid::Uuid::from_u128(1);
        let corrupt_segment = uuid::Uuid::from_u128(2);
        fs::create_dir_all(root.path().join(SEGMENTS_DIR)).unwrap();
        atomic_write_json(
            &root.path().join(SESSION_FILE),
            &SessionManifest {
                schema_version: PREVIOUS_SESSION_SCHEMA_VERSION,
                session_id,
            },
        )
        .unwrap();
        let manifest_before = fs::read(root.path().join(SESSION_FILE)).unwrap();

        let valid_path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{valid_segment}.jsonl"));
        let valid_entry = serde_json::json!({
            "kind": "segment_start",
            "ts": 1,
            "session_id": session_id,
            "system_prompt": null,
            "config": agen::llm_client::RequestConfig::default(),
            "history": [LoggedItem::from(agen::Item::assistant_message("prior"))],
            "forked_from": null,
            "compacted_from": null
        });
        let mut valid_bytes = serde_json::to_vec(&valid_entry).unwrap();
        valid_bytes.push(b'\n');
        fs::write(&valid_path, &valid_bytes).unwrap();
        let corrupt_path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{corrupt_segment}.jsonl"));
        fs::write(&corrupt_path, b"{not-json}\n").unwrap();
        let corrupt_before = fs::read(&corrupt_path).unwrap();

        let error = match WorkerSessionStore::new(root.path()) {
            Ok(_) => panic!("later corrupt segment must fail migration preflight"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::Corrupt { .. }));
        assert_eq!(fs::read(&valid_path).unwrap(), valid_bytes);
        assert_eq!(fs::read(&corrupt_path).unwrap(), corrupt_before);
        assert_eq!(
            fs::read(root.path().join(SESSION_FILE)).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn current_jsonl_requires_annotations_across_append_rewrite_and_reopen() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        let store = WorkerSessionStore::new(root.path()).unwrap();
        store
            .create_segment(
                session_id,
                segment_id,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: agen::llm_client::RequestConfig::default(),
                    history: vec![annotated(agen::Item::user_message("seed"))],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: annotated(agen::Item::assistant_message("reply")),
                },
            )
            .unwrap();

        let before_rewrite = store.read_all(session_id, segment_id).unwrap();
        store
            .create_segment(session_id, segment_id, &before_rewrite)
            .unwrap();
        drop(store);

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        let restored = reopened.read_all(session_id, segment_id).unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&before_rewrite).unwrap()
        );
        for entry in &restored {
            match entry {
                LogEntry::AnnotatedSegmentStart { history, .. } => assert!(history.iter().all(
                    |entry| !entry.metadata.entry_id.0.is_empty()
                        && matches!(
                            entry.metadata.origin,
                            LoggedSessionHistoryOrigin::LegacyUnknown
                        )
                )),
                LogEntry::AnnotatedAssistantItem { entry, .. } => {
                    assert!(!entry.metadata.entry_id.0.is_empty());
                    assert!(matches!(
                        entry.metadata.origin,
                        LoggedSessionHistoryOrigin::LegacyUnknown
                    ));
                }
                _ => {}
            }
        }

        let log = fs::read_to_string(reopened.log_path(segment_id)).unwrap();
        for line in log.lines() {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            let kind = value["kind"].as_str().unwrap();
            assert!(
                !matches!(
                    kind,
                    "segment_start"
                        | "user_input"
                        | "assistant_item"
                        | "tool_result"
                        | "system_item"
                ),
                "current-schema JSONL contains legacy history record: {kind}"
            );
        }
    }

    #[test]
    fn schema_v3_rejects_legacy_records_and_new_writes_are_canonical() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        let store = WorkerSessionStore::new(root.path()).unwrap();
        store
            .create_segment(
                session_id,
                segment_id,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: agen::llm_client::RequestConfig::default(),
                    history: vec![annotated(agen::Item::assistant_message("seed"))],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::AnnotatedUserInput {
                    ts: 2,
                    segments: vec![protocol::Segment::Text {
                        content: "new".into(),
                    }],
                    history: vec![annotated(agen::Item::user_message("new"))],
                    extensions: Vec::new(),
                },
            )
            .unwrap();
        let entries = store.read_all(session_id, segment_id).unwrap();
        assert!(matches!(entries[0], LogEntry::AnnotatedSegmentStart { .. }));
        assert!(matches!(entries[1], LogEntry::AnnotatedUserInput { .. }));
        drop(store);

        let path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"));
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "kind": "system_item",
                "ts": 3,
                "item": { "kind": "legacy_ignored", "slug": "legacy" }
            }),
        )
        .unwrap();
        file.write_all(b"\n").unwrap();
        let error = match WorkerSessionStore::new(root.path()) {
            Ok(_) => panic!("schema v3 must reject a legacy history record"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::Corrupt { .. }));
    }

    #[test]
    fn reopen_preserves_session_and_segment_ids() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        WorkerSessionStore::new(root.path())
            .unwrap()
            .create_segment(session_id, segment_id, &[])
            .unwrap();

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(reopened.session_id().unwrap(), Some(session_id));
        assert!(reopened.exists(session_id, segment_id).unwrap());
    }
}
