use std::io;
use std::path::{Path, PathBuf};

use manifest::ScopeRule;
use serde::{Deserialize, Serialize};
use tokio::fs;

/// Legacy process-child projection used only by persisted scope reclamation.
/// Socket fields retain the stored format; they are never connected to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnedWorkerRecord {
    /// Spawned Worker's identity.
    pub worker_name: String,
    /// Spawned Worker's Unix socket path.
    pub socket_path: PathBuf,
    /// Scope allow rules delegated to the spawned Worker.
    pub scope_delegated: Vec<ScopeRule>,
    /// Socket path the spawned Worker was told to use for callbacks
    /// (= this Worker's own socket when spawn happened).
    pub callback_address: PathBuf,
}

/// Artifact directory retained by an in-process Controller.
///
/// Worker liveness and configuration belong to the Host, not filesystem projections.
/// Persistent run directories and pre-existing data are never removed on drop.
pub struct RuntimeDir {
    path: PathBuf,
    preserve_on_drop: bool,
}

impl RuntimeDir {
    /// Create an ephemeral directory without PID, status, manifest or socket files.
    pub async fn create(base: &Path, worker_name: &str) -> Result<Self, io::Error> {
        Self::create_transient(base, worker_name).await
    }

    /// Preserve pre-existing directories rather than claiming their contents for cleanup.
    pub async fn create_transient(base: &Path, worker_name: &str) -> Result<Self, io::Error> {
        fs::create_dir_all(base).await?;
        let path = base.join(worker_name);
        let preserve_on_drop = match fs::create_dir(&path).await {
            Ok(()) => false,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => true,
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            preserve_on_drop,
        })
    }

    /// Create an exact, persistent generation-scoped Worker run directory.
    /// Existing directories are rejected so stale artifacts cannot be reused.
    pub async fn create_worker_run(path: &Path) -> Result<Self, io::Error> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("run path has no parent"))?;
        fs::create_dir_all(parent).await?;
        fs::create_dir(path).await?;
        fs::create_dir(path.join("artifacts")).await?;
        fs::create_dir(path.join("spawned")).await?;
        for log in ["worker.out.log", "worker.err.log"] {
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.join(log))
                .await?;
            file.sync_all().await?;
        }
        std::fs::File::open(path)?.sync_all()?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(Self {
            path: path.to_path_buf(),
            preserve_on_drop: true,
        })
    }

    /// Rewrite the legacy projection during persisted process-child reclamation.
    pub async fn write_spawned_workers(
        &self,
        records: &[SpawnedWorkerRecord],
    ) -> Result<(), io::Error> {
        let json = serde_json::to_vec_pretty(records).map_err(io::Error::other)?;
        atomic_write(&self.path.join("spawned_workers.json"), &json).await
    }

    /// Path to this Worker's runtime directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        if !self.preserve_on_drop {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Atomic write: write to a temp file, then rename.
async fn atomic_write(target: &Path, content: &[u8]) -> Result<(), io::Error> {
    let tmp = target.with_extension("tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, target).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transient_controller_directory_has_no_liveness_projections_and_is_released() {
        let root = tempfile::tempdir().unwrap();
        let path;
        {
            let runtime = RuntimeDir::create(root.path(), "worker").await.unwrap();
            path = runtime.path().to_owned();
            assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        }
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn preexisting_data_is_not_overwritten_or_removed_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("worker");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("pid"), "retained legacy data").unwrap();
        {
            let _runtime = RuntimeDir::create(root.path(), "worker").await.unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path.join("pid")).unwrap(),
            "retained legacy data"
        );
    }

    #[tokio::test]
    async fn persistent_run_preserves_logs_and_rejects_directory_reuse() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("runs/1");
        {
            let _runtime = RuntimeDir::create_worker_run(&path).await.unwrap();
            for name in ["artifacts", "spawned", "worker.out.log", "worker.err.log"] {
                assert!(path.join(name).exists(), "missing {name}");
            }
        }
        assert!(path.exists());
        let error = RuntimeDir::create_worker_run(&path).await.err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[tokio::test]
    async fn legacy_reclamation_projection_retains_its_stored_format() {
        let root = tempfile::tempdir().unwrap();
        let runtime = RuntimeDir::create(root.path(), "worker").await.unwrap();
        let record: SpawnedWorkerRecord = serde_json::from_value(serde_json::json!({
            "worker_name": "legacy-child", "socket_path": "/old/child/sock",
            "scope_delegated": [], "callback_address": "/old/parent/sock",
        }))
        .unwrap();
        runtime.write_spawned_workers(&[record]).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(runtime.path().join("spawned_workers.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value[0]["socket_path"], "/old/child/sock");
        assert_eq!(value[0]["callback_address"], "/old/parent/sock");
        runtime.write_spawned_workers(&[]).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(runtime.path().join("spawned_workers.json")).unwrap(),
            "[]"
        );
    }
}
