//! Filesystem-backed JSONL store.
//!
//! Layout:
//! - Session log: `{root}/{session_id}.jsonl`
//! - Event trace: `{root}/{session_id}.trace.jsonl`

use crate::SessionId;
use crate::event_trace::TraceEntry;
use crate::session_log::{EntryHash, HashedEntry};
use crate::store::{Store, StoreError};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

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
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root).await?;
        Ok(Self { root })
    }

    fn log_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{id}.jsonl"))
    }

    fn trace_path(&self, id: SessionId) -> PathBuf {
        self.root.join(format!("{id}.trace.jsonl"))
    }

    async fn append_line(&self, path: &Path, line: &str) -> Result<(), StoreError> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
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
    async fn append(&self, id: SessionId, entry: &HashedEntry) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.log_path(id), &line).await
    }

    async fn read_all(&self, id: SessionId) -> Result<Vec<HashedEntry>, StoreError> {
        let path = self.log_path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id));
        }
        let content = fs::read_to_string(&path).await?;
        Self::parse_jsonl(&content)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut sessions = Vec::new();
        let mut dir = fs::read_dir(&self.root).await?;
        while let Some(entry) = dir.next_entry().await? {
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

    async fn create_session(
        &self,
        id: SessionId,
        entries: &[HashedEntry],
    ) -> Result<(), StoreError> {
        let path = self.log_path(id);
        let mut content = String::new();
        for entry in entries {
            content.push_str(&serde_json::to_string(entry)?);
            content.push('\n');
        }
        fs::write(&path, content.as_bytes()).await?;
        Ok(())
    }

    async fn exists(&self, id: SessionId) -> Result<bool, StoreError> {
        Ok(self.log_path(id).exists())
    }

    async fn read_head_hash(&self, id: SessionId) -> Result<Option<EntryHash>, StoreError> {
        let path = self.log_path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id));
        }
        let content = fs::read_to_string(&path).await?;
        let last_line = content.lines().rev().find(|l| !l.trim().is_empty());
        match last_line {
            Some(line) => {
                let entry: HashedEntry =
                    serde_json::from_str(line).map_err(|e| StoreError::Corrupt {
                        line: content.lines().count(),
                        message: e.to_string(),
                    })?;
                Ok(Some(entry.hash))
            }
            None => Ok(None),
        }
    }

    async fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> Result<(), StoreError> {
        let line = serde_json::to_string(entry)?;
        self.append_line(&self.trace_path(id), &line).await
    }
}
