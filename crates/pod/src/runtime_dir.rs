use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use crate::shared_state::PodSharedState;

/// Manages the Pod's runtime directory on tmpfs.
///
/// ```text
/// $XDG_RUNTIME_DIR/insomnia/{pod_name}/
/// ├── pid
/// ├── status.json
/// ├── manifest.toml
/// ├── history.json
/// └── sock             (created by socket listener, not by RuntimeDir)
/// ```
///
/// Files are written atomically (write tmp → rename).
/// The directory is removed on drop.
pub struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// Create the runtime directory and write the PID file.
    pub async fn create(base: &Path, pod_name: &str) -> Result<Self, io::Error> {
        let path = base.join(pod_name);
        fs::create_dir_all(&path).await?;

        let pid = std::process::id().to_string();
        fs::write(path.join("pid"), pid.as_bytes()).await?;

        Ok(Self { path })
    }

    /// Create in the default base directory.
    ///
    /// Uses `$XDG_RUNTIME_DIR/insomnia/` if available,
    /// otherwise falls back to `~/.insomnia/run/`.
    pub async fn create_default(pod_name: &str) -> Result<Self, io::Error> {
        let base = default_base()?;
        Self::create(&base, pod_name).await
    }

    /// Write status.json atomically.
    pub async fn write_status(&self, state: &PodSharedState) -> Result<(), io::Error> {
        let content = state.status_json();
        atomic_write(&self.path.join("status.json"), content.as_bytes()).await
    }

    /// Write manifest.toml (typically once at startup).
    pub async fn write_manifest(&self, toml: &str) -> Result<(), io::Error> {
        atomic_write(&self.path.join("manifest.toml"), toml.as_bytes()).await
    }

    /// Write history.json atomically.
    pub async fn write_history(&self, state: &PodSharedState) -> Result<(), io::Error> {
        let content = state.history_json();
        atomic_write(&self.path.join("history.json"), content.as_bytes()).await
    }

    /// Path to this Pod's runtime directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path where the Unix socket should be created.
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
fn default_base() -> Result<PathBuf, io::Error> {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(PathBuf::from(runtime_dir).join("insomnia"))
    } else if let Ok(home) = std::env::var("HOME") {
        Ok(PathBuf::from(home).join(".insomnia").join("run"))
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_RUNTIME_DIR nor HOME is set",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_state::{PodSharedState, PodStatus};

    fn test_state() -> PodSharedState {
        PodSharedState::new(
            "test-pod".into(),
            session_store::new_session_id(),
            "[pod]\nname = \"test-pod\"".into(),
        )
    }

    #[tokio::test]
    async fn creates_directory_and_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();

        assert!(rt.path().join("pid").exists());
        let pid = std::fs::read_to_string(rt.path().join("pid")).unwrap();
        assert_eq!(pid, std::process::id().to_string());
    }

    #[tokio::test]
    async fn write_status_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();
        let state = test_state();

        rt.write_status(&state).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("status.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["state"], "idle");
        assert_eq!(parsed["pod_name"], "test-pod");
    }

    #[tokio::test]
    async fn write_status_reflects_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();
        let state = test_state();

        state.set_status(PodStatus::Running);
        rt.write_status(&state).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("status.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["state"], "running");
    }

    #[tokio::test]
    async fn write_manifest_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();

        rt.write_manifest("[pod]\nname = \"test\"").await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("manifest.toml")).unwrap();
        assert_eq!(content, "[pod]\nname = \"test\"");
    }

    #[tokio::test]
    async fn write_history_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();
        let state = test_state();

        rt.write_history(&state).await.unwrap();

        let content = std::fs::read_to_string(rt.path().join("history.json")).unwrap();
        assert_eq!(content, "[]");
    }

    #[tokio::test]
    async fn socket_path() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();
        assert_eq!(rt.socket_path(), rt.path().join("sock"));
    }

    #[tokio::test]
    async fn drop_removes_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_path;
        {
            let rt = RuntimeDir::create(tmp.path(), "my-pod").await.unwrap();
            dir_path = rt.path().to_owned();
            assert!(dir_path.exists());
        }
        assert!(!dir_path.exists());
    }
}
