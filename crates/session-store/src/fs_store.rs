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
use crate::paste_artifact::{read_from_dir, write_to_dir};
use crate::segment_log::LogEntry;
use crate::store::{Store, StoreError};
use crate::uploaded_file::{
    bind_uploaded_file, delete_uploaded_file, read_uploaded_file, read_uploaded_file_by_id,
    write_uploaded_file,
};
use crate::{PasteArtifactLimits, SegmentId, SessionId, UploadedFileLimits};
use protocol::{PasteArtifactRef, UploadedFileRef};
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

    fn paste_artifact_dir(&self, session_id: SessionId) -> PathBuf {
        self.session_dir(session_id).join("artifacts").join("paste")
    }

    #[cfg(test)]
    fn paste_artifact_path(&self, session_id: SessionId, artifact_id: &str) -> PathBuf {
        self.paste_artifact_dir(session_id)
            .join(format!("{artifact_id}.json"))
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

    fn write_paste_artifact(
        &self,
        session_id: SessionId,
        source_entry_id: &str,
        content: &str,
        limits: PasteArtifactLimits,
    ) -> Result<PasteArtifactRef, StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("session store append lock was poisoned"))?;
        write_to_dir(
            &self.paste_artifact_dir(session_id),
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
        read_from_dir(&self.paste_artifact_dir(session_id), artifact_id)
    }

    fn write_uploaded_file(
        &self,
        session_id: SessionId,
        file_name: &str,
        media_type: &str,
        content: &[u8],
        limits: UploadedFileLimits,
    ) -> Result<UploadedFileRef, StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("session store append lock was poisoned"))?;
        write_uploaded_file(
            &self.paste_artifact_dir(session_id),
            file_name,
            media_type,
            content,
            limits,
        )
    }

    fn read_uploaded_file(
        &self,
        session_id: SessionId,
        reference: &UploadedFileRef,
    ) -> Result<Vec<u8>, StoreError> {
        read_uploaded_file(&self.paste_artifact_dir(session_id), reference)
    }

    fn read_uploaded_file_by_id(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<(UploadedFileRef, Vec<u8>), StoreError> {
        read_uploaded_file_by_id(&self.paste_artifact_dir(session_id), artifact_id)
    }

    fn bind_uploaded_file(
        &self,
        session_id: SessionId,
        reference: &UploadedFileRef,
        source_entry_id: &str,
    ) -> Result<UploadedFileRef, StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("session store append lock was poisoned"))?;
        bind_uploaded_file(
            &self.paste_artifact_dir(session_id),
            reference,
            source_entry_id,
        )
    }

    fn delete_uploaded_file(
        &self,
        session_id: SessionId,
        artifact_id: &str,
    ) -> Result<bool, StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("session store append lock was poisoned"))?;
        delete_uploaded_file(&self.paste_artifact_dir(session_id), artifact_id)
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

    #[test]
    fn paste_artifacts_are_atomic_integrity_checked_and_session_scoped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let owner = new_session_id();
        let other = new_session_id();
        let content = "αβγ\nsecond line\n";
        let reference = store
            .write_paste_artifact(owner, "entry-1", content, PasteArtifactLimits::default())
            .unwrap();

        assert_eq!(reference.byte_len, content.len() as u64);
        assert!(reference.created_at_ms > 0);
        assert_eq!(
            reference.media_type,
            protocol::PasteArtifactMediaType::TextPlainUtf8
        );
        assert_eq!(
            reference.availability,
            protocol::PasteArtifactAvailability::Available
        );
        assert_eq!(reference.char_count, content.chars().count() as u64);
        assert_eq!(reference.source_entry_id, "entry-1");
        assert_eq!(
            store
                .read_paste_artifact(owner, &reference.artifact_id)
                .unwrap()
                .1,
            content
        );
        assert!(matches!(
            store.read_paste_artifact(other, &reference.artifact_id),
            Err(StoreError::PasteArtifactNotFound(_))
        ));
        assert!(
            self::fs::read_dir(store.paste_artifact_dir(owner))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
        let very_large = "z".repeat(1024 * 1024);
        let very_large_ref = store
            .write_paste_artifact(
                owner,
                "entry-2",
                &very_large,
                PasteArtifactLimits::default(),
            )
            .unwrap();
        assert_eq!(
            store
                .read_paste_artifact(owner, &very_large_ref.artifact_id)
                .unwrap()
                .1,
            very_large
        );
    }

    #[test]
    fn concurrent_paste_writes_atomically_enforce_aggregate_caps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = new_session_id();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let limits = PasteArtifactLimits {
            max_artifact_bytes: 4,
            max_session_bytes: 8,
            max_session_artifacts: 1,
        };
        let mut handles = Vec::new();
        for entry_id in ["entry-1", "entry-2"] {
            let root = tmp.path().to_path_buf();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let store = FsStore::new(root).unwrap();
                barrier.wait();
                store.write_paste_artifact(session_id, entry_id, "1234", limits)
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::PasteArtifactLimit(_))))
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_dir(
                FsStore::new(tmp.path())
                    .unwrap()
                    .paste_artifact_dir(session_id)
            )
            .unwrap()
            .filter_map(Result::ok)
            .filter(
                |entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            )
            .count(),
            1
        );
    }

    #[test]
    fn uploaded_files_are_session_scoped_integrity_checked_and_removable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let owner = new_session_id();
        let other = new_session_id();
        let limits = UploadedFileLimits {
            max_file_bytes: 16,
            max_session_bytes: 16,
        };
        let reference = store
            .write_uploaded_file(owner, "notes.txt", "text/plain", b"hello", limits)
            .unwrap();

        assert_eq!(reference.file_name, "notes.txt");
        assert_eq!(reference.media_type, "text/plain");
        assert_eq!(reference.byte_len, 5);
        assert_eq!(reference.source_entry_id, None);
        assert_eq!(
            store.read_uploaded_file(owner, &reference).unwrap(),
            b"hello"
        );
        assert!(store.read_uploaded_file(other, &reference).is_err());

        let mut forged = reference.clone();
        forged.file_name = "other.txt".to_string();
        assert!(matches!(
            store.read_uploaded_file(owner, &forged),
            Err(StoreError::ArtifactIntegrityMismatch)
        ));
        assert!(
            store
                .delete_uploaded_file(owner, &reference.artifact_id)
                .unwrap()
        );
        assert!(
            !store
                .delete_uploaded_file(owner, &reference.artifact_id)
                .unwrap()
        );
        assert!(store.read_uploaded_file(owner, &reference).is_err());
    }

    #[test]
    fn uploaded_file_validation_and_shared_quota_fail_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let session_id = new_session_id();
        let limits = UploadedFileLimits {
            max_file_bytes: 8,
            max_session_bytes: 8,
        };
        assert!(matches!(
            store.write_uploaded_file(session_id, "../secret", "text/plain", b"x", limits),
            Err(StoreError::InvalidUploadedFileName)
        ));
        assert!(matches!(
            store.write_uploaded_file(session_id, "notes.txt", "not a type", b"x", limits),
            Err(StoreError::InvalidUploadedFileMediaType)
        ));
        store
            .write_paste_artifact(
                session_id,
                "entry-1",
                "1234",
                PasteArtifactLimits {
                    max_artifact_bytes: 8,
                    max_session_bytes: 8,
                    max_session_artifacts: 4,
                },
            )
            .unwrap();
        assert!(matches!(
            store.write_uploaded_file(session_id, "notes.txt", "text/plain", b"56789", limits),
            Err(StoreError::ArtifactQuotaExceeded)
        ));
    }

    #[test]
    fn paste_artifact_limits_and_corruption_fail_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = FsStore::new(tmp.path()).unwrap();
        let session_id = new_session_id();
        let limits = PasteArtifactLimits {
            max_artifact_bytes: 5,
            max_session_bytes: 8,
            max_session_artifacts: 2,
        };
        let first = store
            .write_paste_artifact(session_id, "entry-1", "1234", limits)
            .unwrap();
        assert!(matches!(
            store.write_paste_artifact(session_id, "entry-2", "56789", limits),
            Err(StoreError::PasteArtifactLimit(_))
        ));
        assert!(matches!(
            store.write_paste_artifact(session_id, "entry-2", "5678", limits),
            Ok(_)
        ));
        std::fs::write(
            store.paste_artifact_path(session_id, &first.artifact_id),
            b"{}",
        )
        .unwrap();
        assert!(matches!(
            store.read_paste_artifact(session_id, &first.artifact_id),
            Err(StoreError::Serde(_)) | Err(StoreError::PasteArtifactIntegrity(_))
        ));
    }
}
