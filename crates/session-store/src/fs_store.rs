//! Filesystem-backed JSONL store.
//!
//! Layout:
//! - Segment log: `{root}/{session_id}/{segment_id}.jsonl`
//! - Event trace: `{root}/{session_id}/{segment_id}.trace.jsonl`
//! - Pod metadata: `{root}/pods/{pod_name}/metadata.json`
//!
//! The per-Session directory makes `list_segments(session_id)` an O(dir)
//! scan and gives the fork tree a visible grouping in the filesystem.
//!
//! Migration: this layout is incompatible with the pre-`session-grouping`
//! flat `{root}/{segment_id}.jsonl` form. Project policy is no
//! backward compatibility — discard `~/.insomnia/sessions/` (or whatever
//! `root` resolved to) before running the new code. `list_sessions`
//! ignores top-level files outside session directories, so leftover
//! flat files do not corrupt new sessions, but they are no longer
//! enumerable by the picker.

use crate::event_trace::TraceEntry;
use crate::pod_metadata::{PodMetadata, PodMetadataStore, validate_pod_name};
use crate::segment_log::LogEntry;
use crate::store::{Store, StoreError};
use crate::{SegmentId, SessionId};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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

    fn pods_dir(&self) -> PathBuf {
        self.root.join("pods")
    }

    fn pod_dir(&self, pod_name: &str) -> Result<PathBuf, StoreError> {
        validate_pod_name(pod_name)?;
        Ok(self.pods_dir().join(pod_name))
    }

    fn pod_metadata_path(&self, pod_name: &str) -> Result<PathBuf, StoreError> {
        Ok(self.pod_dir(pod_name)?.join("metadata.json"))
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

impl PodMetadataStore for FsStore {
    fn write(&self, metadata: &PodMetadata) -> Result<(), StoreError> {
        let path = self.pod_metadata_path(&metadata.pod_name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_vec_pretty(metadata)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, StoreError> {
        let path = self.pod_metadata_path(pod_name)?;
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(StoreError::Io(err)),
        };
        Ok(Some(serde_json::from_str(&content)?))
    }

    fn list_names(&self) -> Result<Vec<String>, StoreError> {
        let dir = self.pods_dir();
        let mut names = Vec::new();
        if !dir.exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if !entry.path().join("metadata.json").exists() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if validate_pod_name(&name).is_ok() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn delete_by_name(&self, pod_name: &str) -> Result<(), StoreError> {
        let path = self.pod_metadata_path(pod_name)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(StoreError::Io(err)),
        }
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
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
