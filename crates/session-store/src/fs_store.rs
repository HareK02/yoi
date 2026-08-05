//! Filesystem-backed JSONL store.
//!
//! Layout:
//! - Segment log: `{root}/{session_id}/{segment_id}.jsonl`
//! - Event trace: `{root}/{session_id}/{segment_id}.trace.jsonl`
//!
//! The per-Session directory makes `list_segments(session_id)` an O(dir)
//! scan and gives the fork tree a visible grouping in the filesystem.
//!
//! Migration: this layout is incompatible with the pre-`session-grouping`
//! flat `{root}/{segment_id}.jsonl` form. Project policy is no
//! backward compatibility — discard `~/.yoi/sessions/` (or whatever
//! `root` resolved to) before running the new code. `list_sessions`
//! ignores top-level files outside session directories, so leftover
//! flat files do not corrupt new sessions, but they are no longer
//! enumerable by the picker.

use crate::event_trace::TraceEntry;
use crate::segment_log::LogEntry;
use crate::store::{Store, StoreError};
use crate::{SegmentId, SessionId};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Filesystem-backed JSONL store.
///
/// Each segment is stored as a single `.jsonl` file with one [`LogEntry`]
/// per line. A trailing line is committed only once its newline has been
/// written; readers ignore an unterminated tail and the next append removes it.
#[derive(Clone)]
pub struct FsStore {
    root: PathBuf,
    /// Serialises append repair + write + rollback across clones. A failed
    /// `write_all` may have extended the file, so rollback is safe only while
    /// no sibling writer can append behind it.
    append_lock: Arc<Mutex<()>>,
}

impl FsStore {
    /// Create a new `FsStore` rooted at the given directory.
    /// Creates the directory if it does not exist.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Return the filesystem root used by this store.
    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    /// Return the latest filesystem mtime under a Session directory.
    ///
    /// Missing Sessions return `Ok(None)`. This is intentionally Session-scoped
    /// so cleanup callers can apply age thresholds without reaching around the
    /// Session store's directory authority.
    pub fn session_modified_at(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SystemTime>, StoreError> {
        let session_dir = self.session_dir(session_id);
        let dir_metadata = match fs::metadata(&session_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut latest = Some(dir_metadata.modified()?);
        for entry in fs::read_dir(&session_dir)? {
            let entry = entry?;
            let modified = entry.metadata()?.modified()?;
            if latest.map(|current| modified > current).unwrap_or(true) {
                latest = Some(modified);
            }
        }
        Ok(latest)
    }

    /// Delete an entire Session directory owned by this Session store.
    ///
    /// Returns `Ok(true)` when a Session directory was removed and `Ok(false)`
    /// when it was already absent.
    pub fn delete_session(&self, session_id: SessionId) -> Result<bool, StoreError> {
        let session_dir = self.session_dir(session_id);
        match fs::remove_dir_all(&session_dir) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn session_dir(&self, session_id: SessionId) -> PathBuf {
        self.root.join(session_id.to_string())
    }

    fn log_path(&self, session_id: SessionId, segment_id: SegmentId) -> PathBuf {
        self.session_dir(session_id)
            .join(format!("{segment_id}.jsonl"))
    }

    fn trace_path(&self, session_id: SessionId, segment_id: SegmentId) -> PathBuf {
        self.session_dir(session_id)
            .join(format!("{segment_id}.trace.jsonl"))
    }

    fn append_line(&self, path: &Path, line: &str) -> Result<(), StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("session store append lock was poisoned"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(true)
            .open(path)?;
        let committed_len = Self::truncate_uncommitted_tail(&mut file)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line.as_bytes());
        record.push(b'\n');

        if let Err(write_error) = file.write_all(&record) {
            return match file.set_len(committed_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "session append failed ({write_error}) and partial-write rollback failed: {rollback_error}"
                    ),
                )
                .into()),
            };
        }
        Ok(())
    }

    /// Return only newline-terminated records. A process interruption or
    /// ENOSPC can leave the final UTF-8 code point / JSON object incomplete;
    /// without a newline that record never crossed the commit boundary.
    fn complete_jsonl_prefix(content: &[u8]) -> &[u8] {
        if content.last() == Some(&b'\n') {
            return content;
        }
        match content.iter().rposition(|byte| *byte == b'\n') {
            Some(index) => &content[..=index],
            None => &[],
        }
    }

    fn parse_jsonl<T: serde::de::DeserializeOwned>(content: &[u8]) -> Result<Vec<T>, StoreError> {
        let complete = Self::complete_jsonl_prefix(content);
        let content = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
            line: complete[..error.valid_up_to()]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
            message: error.to_string(),
        })?;
        let mut entries = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: T = serde_json::from_str(line).map_err(|e| StoreError::Corrupt {
                line: i + 1,
                message: e.to_string(),
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Remove a prior unterminated record and return the committed file size.
    /// Scans backwards in bounded chunks so repairing a large session does not
    /// require loading it into memory.
    fn truncate_uncommitted_tail(file: &mut fs::File) -> std::io::Result<u64> {
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
}

impl Store for FsStore {
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.log_path(session_id, segment_id), &line)
    }

    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        let path = self.log_path(session_id, segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        let content = fs::read(&path)?;
        Self::parse_jsonl(&content)
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut sessions = Vec::new();
        if !self.root.exists() {
            return Ok(sessions);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<SessionId>() {
                    sessions.push(id);
                }
            }
        }
        sessions.sort_by(|a, b| b.cmp(a));
        Ok(sessions)
    }

    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
        let dir = self.session_dir(session_id);
        let mut segments = Vec::new();
        if !dir.exists() {
            return Ok(segments);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            // Only match .jsonl files, not .trace.jsonl
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".jsonl") && !name.ends_with(".trace.jsonl") {
                let stem = name.trim_end_matches(".jsonl");
                if let Ok(id) = stem.parse::<SegmentId>() {
                    segments.push(id);
                }
            }
        }
        // UUID v7: lexicographic sort = chronological sort, newest first
        segments.sort_by(|a, b| b.cmp(a));
        Ok(segments)
    }

    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError> {
        if !self.root.exists() {
            return Ok(None);
        }
        let needle = format!("{segment_id}.jsonl");
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if entry.path().join(&needle).exists()
                && let Some(name) = entry.file_name().to_str()
                && let Ok(id) = name.parse::<SessionId>()
            {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError> {
        let path = self.log_path(session_id, segment_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = String::new();
        for entry in entries {
            content.push_str(&serde_json::to_string(entry)?);
            content.push('\n');
        }
        fs::write(&path, content.as_bytes())?;
        Ok(())
    }

    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
        Ok(self.log_path(session_id, segment_id).exists())
    }

    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError> {
        let path = self.log_path(session_id, segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        let content = fs::read(&path)?;
        let complete = Self::complete_jsonl_prefix(&content);
        let complete = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
            line: complete[..error.valid_up_to()]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
            message: error.to_string(),
        })?;
        Ok(complete.lines().filter(|l| !l.trim().is_empty()).count())
    }

    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.trace_path(session_id, segment_id), &line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_segment_id, new_session_id};

    #[test]
    fn delete_session_removes_session_directory_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let keep_session = new_session_id();
        let keep_segment = new_segment_id();
        let delete_session = new_session_id();
        let delete_segment = new_segment_id();
        store
            .create_segment(keep_session, keep_segment, &[])
            .unwrap();
        store
            .create_segment(delete_session, delete_segment, &[])
            .unwrap();

        assert!(store.delete_session(delete_session).unwrap());
        assert!(!store.exists(delete_session, delete_segment).unwrap());
        assert!(store.exists(keep_session, keep_segment).unwrap());
        assert!(!store.delete_session(delete_session).unwrap());
    }

    #[test]
    fn session_modified_at_is_store_scoped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();

        assert!(store.session_modified_at(session_id).unwrap().is_none());
        store.create_segment(session_id, segment_id, &[]).unwrap();
        assert!(store.session_modified_at(session_id).unwrap().is_some());
    }
}
