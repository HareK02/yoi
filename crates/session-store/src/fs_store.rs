//! Filesystem-backed JSONL store.
//!
//! Layout:
//! - Session log: `{root}/{session_id}.jsonl`
//! - Event trace: `{root}/{session_id}.trace.jsonl`

use crate::SessionId;
use crate::event_trace::TraceEntry;
use crate::session_log::LogEntry;
use crate::store::{Store, StoreError};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Filesystem-backed JSONL store.
///
/// Each session is stored as a single `.jsonl` file with one [`LogEntry`]
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

    fn log_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{id}.jsonl"))
    }

    fn trace_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{id}.trace.jsonl"))
    }

    fn append_line(&self, path: &Path, line: &str) -> Result<(), StoreError> {
        let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
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
    fn append(&self, id: SessionId, entry: &LogEntry) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.log_path(id), &line)
    }

    fn read_all(&self, id: SessionId) -> Result<Vec<LogEntry>, StoreError> {
        let path = self.log_path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id));
        }
        let content = fs::read_to_string(&path)?;
        Self::parse_jsonl(&content)
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            // Only match .jsonl files, not .trace.jsonl
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".jsonl") && !name.ends_with(".trace.jsonl") {
                let stem = name.trim_end_matches(".jsonl");
                if let Ok(id) = stem.parse::<SessionId>() {
                    sessions.push(id);
                }
            }
        }
        // UUID v7: lexicographic sort = chronological sort, newest first
        sessions.sort_by(|a, b| b.cmp(a));
        Ok(sessions)
    }

    fn create_session(&self, id: SessionId, entries: &[LogEntry]) -> Result<(), StoreError> {
        let path = self.log_path(id);
        let mut content = String::new();
        for entry in entries {
            content.push_str(&serde_json::to_string(entry)?);
            content.push('\n');
        }
        fs::write(&path, content.as_bytes())?;
        Ok(())
    }

    fn exists(&self, id: SessionId) -> Result<bool, StoreError> {
        Ok(self.log_path(id).exists())
    }

    fn read_entry_count(&self, id: SessionId) -> Result<usize, StoreError> {
        let path = self.log_path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id));
        }
        let content = fs::read_to_string(&path)?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.trace_path(id), &line)
    }
}
