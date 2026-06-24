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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Filesystem-backed JSONL store.
///
/// Each segment is stored as a single `.jsonl` file with one [`LogEntry`]
/// per line. Writes use append mode for crash safety.
#[derive(Clone)]
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    /// Create a new `FsStore` rooted at the given directory.
    /// Creates the directory if it does not exist.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        // Append-mode write is the durability boundary; an explicit
        // `sync_all` here would multiply latency by ~10× for no gain
        // since the kernel already orders concurrent `O_APPEND` writes.
        Ok(())
    }

    fn parse_jsonl<T: serde::de::DeserializeOwned>(content: &str) -> Result<Vec<T>, StoreError> {
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
        let content = fs::read_to_string(&path)?;
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
        let content = fs::read_to_string(&path)?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
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
