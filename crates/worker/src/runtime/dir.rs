use std::io;
use std::path::{Path, PathBuf};

use manifest::{ScopeRule, paths};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::shared_state::WorkerSharedState;

/// One spawned-child record mirrored to `spawned_workers.json`.
///
/// Written by the spawner after registry changes so runtime-local tools
/// have a materialised snapshot. Durable restore uses Worker state metadata;
/// this file is not the authoritative source.
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

/// Manages the Worker's runtime directory on tmpfs.
///
/// ```text
/// <runtime_dir>/{worker_name}/
/// ├── pid
/// ├── status.json
/// ├── manifest.toml
/// ├── history.json
/// └── sock             (created by socket listener, not by RuntimeDir)
/// ```
///
/// `<runtime_dir>` is resolved via [`manifest::paths::runtime_dir`].
/// Files are written atomically (write tmp → rename).
/// The directory is removed on drop.
pub struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// Create the runtime directory and write the PID file.
    pub async fn create(base: &Path, worker_name: &str) -> Result<Self, io::Error> {
        let path = base.join(worker_name);
        fs::create_dir_all(&path).await?;

        let pid = std::process::id().to_string();
        fs::write(path.join("pid"), pid.as_bytes()).await?;

        Ok(Self { path })
    }

    /// Create in the default base directory resolved via
    /// [`manifest::paths::runtime_dir`].
    pub async fn create_default(worker_name: &str) -> Result<Self, io::Error> {
        let base = default_base()?;
        Self::create(&base, worker_name).await
    }

    /// Write status.json atomically.
    pub async fn write_status(&self, state: &WorkerSharedState) -> Result<(), io::Error> {
        let content = state.status_json();
        atomic_write(&self.path.join("status.json"), content.as_bytes()).await
    }

    /// Write manifest.toml (typically once at startup).
    pub async fn write_manifest(&self, toml: &str) -> Result<(), io::Error> {
        atomic_write(&self.path.join("manifest.toml"), toml.as_bytes()).await
    }

    /// Write `spawned_workers.json` atomically. The entries are the full
    /// set of spawned children known to this Worker — callers pass the
    /// replacement list, no incremental merge.
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

    /// Path where the Unix socket should be created. External callers
    /// that only know the worker name (e.g. the TUI's attach flow)
    /// predict the same path via [`manifest::paths::worker_socket_path`].
    pub fn socket_path(&self) -> PathBuf {
        self.path.join("sock")
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Atomic write: write to a temp file, then rename.
async fn atomic_write(target: &Path, content: &[u8]) -> Result<(), io::Error> {
    let tmp = target.with_extension("tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, target).await?;
    Ok(())
}

/// Resolve the default base directory for runtime data.
///
/// Thin wrapper over [`manifest::paths::runtime_dir`] that converts a
/// missing-env situation into an `io::Error`.
pub fn default_base() -> Result<PathBuf, io::Error> {
    paths::runtime_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve runtime directory (no YOI_HOME / \
             YOI_RUNTIME_DIR / XDG_RUNTIME_DIR / HOME)",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_state::WorkerSharedState;
    use protocol::WorkerStatus;

    fn test_state() -> WorkerSharedState {
        WorkerSharedState::new(
            "test-worker".into(),
            session_store::new_segment_id(),
            "[engine]\nname = \"test-worker\"".into(),
            protocol::Greeting {
                worker_name: "test-worker".into(),
                cwd: "/tmp".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 200_000,
                context_tokens: 0,
            },
        )
    }

    #[tokio::test]
    async fn creates_directory_and_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();

        assert!(rt.path().join("pid").exists());
        let pid = std::fs::read_to_string(rt.path().join("pid")).unwrap();
        assert_eq!(pid, std::process::id().to_string());
    }

    #[tokio::test]
    async fn write_status_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();
        let state = test_state();

        rt.write_status(&state).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("status.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["state"], "idle");
        assert_eq!(parsed["worker_name"], "test-worker");
    }

    #[tokio::test]
    async fn write_status_reflects_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();
        let state = test_state();

        state.set_status(WorkerStatus::Running);
        rt.write_status(&state).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("status.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["state"], "running");
    }

    #[tokio::test]
    async fn write_manifest_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();

        rt.write_manifest("[engine]\nname = \"test\"")
            .await
            .unwrap();

        let content = std::fs::read_to_string(rt.path().join("manifest.toml")).unwrap();
        assert_eq!(content, "[engine]\nname = \"test\"");
    }

    #[tokio::test]
    async fn write_spawned_workers_creates_file() {
        use manifest::{Permission, ScopeRule};
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();

        let records = vec![SpawnedWorkerRecord {
            worker_name: "child".into(),
            socket_path: "/run/yoi/child/sock".into(),
            scope_delegated: vec![ScopeRule {
                target: "/tmp/work".into(),
                permission: Permission::Write,
                recursive: true,
            }],
            callback_address: "/run/yoi/my-worker/sock".into(),
        }];
        rt.write_spawned_workers(&records).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("spawned_workers.json")).unwrap();
        let parsed: Vec<SpawnedWorkerRecord> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].worker_name, "child");
    }

    #[tokio::test]
    async fn socket_path() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();
        assert_eq!(rt.socket_path(), rt.path().join("sock"));
    }

    #[tokio::test]
    async fn drop_removes_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_path;
        {
            let rt = RuntimeDir::create(tmp.path(), "my-worker").await.unwrap();
            dir_path = rt.path().to_owned();
            assert!(dir_path.exists());
        }
        assert!(!dir_path.exists());
    }
}
