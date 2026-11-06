//! Producer-owned locations for additional retained Sessions of a Runtime Worker.

use crate::{SegmentId, SessionId, StoreError};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSessionOwner {
    pub workspace_id: String,
    pub worker_id: String,
}

/// Fixed Runtime layout, never a path supplied by a recording caller.
#[derive(Debug, Clone)]
pub struct WorkerSessionSources {
    runtime_root: PathBuf,
    owner: WorkerSessionOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSessionBinding {
    pub owner: WorkerSessionOwner,
    pub session_id: SessionId,
    pub active_segment_id: SegmentId,
}

impl WorkerSessionSources {
    pub fn new(
        runtime_root: impl Into<PathBuf>,
        owner: WorkerSessionOwner,
    ) -> Result<Self, StoreError> {
        if owner.workspace_id.trim().is_empty() || owner.worker_id.parse::<uuid::Uuid>().is_err() {
            return Err(std::io::Error::other("Invalid Worker Session owner").into());
        }
        Ok(Self {
            runtime_root: runtime_root.into(),
            owner,
        })
    }

    pub fn owner(&self) -> &WorkerSessionOwner {
        &self.owner
    }
    pub fn worker_root(&self) -> PathBuf {
        self.runtime_root
            .join("workers")
            .join(&self.owner.worker_id)
    }
    pub fn retained_root(&self) -> PathBuf {
        self.worker_root().join("retained-sessions")
    }
    pub fn freeze_path(&self) -> PathBuf {
        self.runtime_root
            .join("retention/removal-freezes")
            .join(format!("{}.json", self.owner.worker_id))
    }
    pub fn release_fence_path(&self) -> PathBuf {
        self.runtime_root
            .join("retention/subject-release-fences")
            .join(format!("{}.json", self.owner.worker_id))
    }
    pub fn lock_exclusive(&self) -> Result<File, StoreError> {
        let file = self.open_lock()?;
        file.lock()?;
        Ok(file)
    }
    pub fn lock_publication(&self) -> Result<File, StoreError> {
        let file = self.open_lock()?;
        file.lock_shared()?;
        for path in [self.freeze_path(), self.release_fence_path()] {
            match fs::metadata(path) {
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "Worker recording scope has fenced source publication",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(file)
    }
    fn open_lock(&self) -> Result<File, StoreError> {
        let dir = self.runtime_root.join("retention/source-locks");
        fs::create_dir_all(&dir)?;
        Ok(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("{}.lock", self.owner.worker_id)))?)
    }

    pub(crate) fn bind(
        &self,
        session_id: SessionId,
        active_segment_id: SegmentId,
    ) -> Result<(), StoreError> {
        let path = self
            .worker_root()
            .join("retained-session-bindings")
            .join(format!("{session_id}.json"));
        if let Some(binding) = self.read_binding(&path)? {
            if binding.owner != self.owner || binding.session_id != session_id {
                return Err(std::io::Error::other("Retained Session owner mismatch").into());
            }
            return Ok(());
        }
        crate::worker_session_store::atomic_write_json(
            &path,
            &RetainedSessionBinding {
                owner: self.owner.clone(),
                session_id,
                active_segment_id,
            },
        )
    }

    /// A producer explicitly selects the saved lineage; discovery never guesses
    /// an active pointer from filenames or modification times.
    pub(crate) fn select(
        &self,
        session_id: SessionId,
        active_segment_id: SegmentId,
    ) -> Result<(), StoreError> {
        let path = self
            .worker_root()
            .join("retained-session-bindings")
            .join(format!("{session_id}.json"));
        let mut binding = self
            .read_binding(&path)?
            .ok_or(StoreError::SessionSourceMissing(session_id))?;
        if binding.owner != self.owner || binding.session_id != session_id {
            return Err(std::io::Error::other("Retained Session owner mismatch").into());
        }
        binding.active_segment_id = active_segment_id;
        crate::worker_session_store::atomic_write_json(&path, &binding)
    }

    pub fn retained_bindings(&self) -> Result<Vec<RetainedSessionBinding>, StoreError> {
        let registry = self.worker_root().join("retained-session-bindings");
        let mut bindings = Vec::new();
        match fs::read_dir(&registry) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let session: SessionId = entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| {
                            std::io::Error::other("Unmapped retained Session binding")
                        })?;
                    let binding = self
                        .read_binding(&entry.path())?
                        .ok_or(StoreError::SessionSourceMissing(session))?;
                    if binding.owner != self.owner || binding.session_id != session {
                        return Err(
                            std::io::Error::other("Retained Session ownership mismatch").into()
                        );
                    }
                    bindings.push(binding);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        // Unregistered bytes are never silently omitted. A registered but missing
        // original also remains in the inventory and will fail its scoped read.
        match fs::read_dir(self.retained_root()) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let session: SessionId = entry
                        .file_name()
                        .to_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| {
                            std::io::Error::other("Unmapped retained Session identity")
                        })?;
                    if !entry.file_type()?.is_dir()
                        || !bindings.iter().any(|binding| binding.session_id == session)
                    {
                        return Err(StoreError::SessionSourceMissing(session));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        bindings.sort_by_key(|binding| binding.session_id);
        Ok(bindings)
    }

    fn read_binding(&self, path: &Path) -> Result<Option<RetainedSessionBinding>, StoreError> {
        match crate::read_without_atime(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
