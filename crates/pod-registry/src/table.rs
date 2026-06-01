//! On-disk allocation table and the `flock`-protected guard.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use manifest::{ScopeRule, paths};
use serde::{Deserialize, Serialize};
use session_store::SegmentId;

/// On-disk representation of the allocation table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    #[serde(default)]
    pub allocations: Vec<Allocation>,
}

/// One Pod's scope allocation.
///
/// `scope_allow` is the full set of allow rules the Pod was granted.
/// Portions delegated out to child Pods are **not** subtracted in
/// storage — the effective write scope is derived on the fly by
/// removing rules owned by any Pod whose `delegated_from` points to
/// this one. Keeping the raw allow set makes reparenting (stale
/// reclaim) trivial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allocation {
    /// Pod name — also the identity used throughout orchestration.
    pub pod_name: String,
    /// Owning process. Checked with `kill(pid, 0)` for stale detection.
    pub pid: u32,
    /// Pod's Unix socket path.
    pub socket: PathBuf,
    /// Allow rules granted to this Pod (write + read).
    pub scope_allow: Vec<ScopeRule>,
    /// Deny rules that cap this Pod's effective scope. Normally empty for
    /// fresh allocations; restored Pods use this to avoid reclaiming
    /// previously delegated write regions.
    #[serde(default)]
    pub scope_deny: Vec<ScopeRule>,
    /// Name of the Pod that delegated scope to this one, or `None` for
    /// a top-level Pod started directly by a human.
    pub delegated_from: Option<String>,
    /// Segment ID this Pod is currently writing to. `None` means this
    /// is a pre-reservation made by a spawner via [`crate::delegate_scope`]
    /// before the child has come up; the child fills it in at
    /// [`crate::adopt_allocation`] time.
    #[serde(default)]
    pub segment_id: Option<SegmentId>,
}

impl LockFile {
    pub fn find(&self, pod_name: &str) -> Option<&Allocation> {
        self.allocations.iter().find(|a| a.pod_name == pod_name)
    }

    pub fn find_mut(&mut self, pod_name: &str) -> Option<&mut Allocation> {
        self.allocations.iter_mut().find(|a| a.pod_name == pod_name)
    }

    /// Find the allocation currently writing to `segment_id`. Skips
    /// pre-reservations whose `segment_id` is still `None`.
    pub fn find_by_segment(&self, segment_id: SegmentId) -> Option<&Allocation> {
        self.allocations
            .iter()
            .find(|a| a.segment_id == Some(segment_id))
    }
}

/// Default on-disk path: `<runtime_dir>/pods.json` resolved via
/// [`manifest::paths::pod_registry_path`]. Tests should point this
/// elsewhere by setting `YOI_HOME` or `YOI_RUNTIME_DIR` to a
/// tempdir.
pub fn default_registry_path() -> io::Result<PathBuf> {
    paths::pod_registry_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve pods.json path (no YOI_HOME / \
             YOI_RUNTIME_DIR / XDG_RUNTIME_DIR / HOME)",
        )
    })
}

/// RAII guard over an exclusively-locked lock file.
///
/// The file is kept open for the lifetime of the guard; `flock(LOCK_EX)`
/// is released automatically on drop. Mutations go through
/// [`LockFileGuard::data_mut`] and are committed with
/// [`LockFileGuard::save`] before dropping — callers who mutate but
/// never call `save` leave the table unchanged, which is the right
/// behaviour for error paths.
pub struct LockFileGuard {
    file: File,
    data: LockFile,
}

impl LockFileGuard {
    /// Open the lock file at `path` (creating it + parent dirs if
    /// needed), acquire an exclusive `flock`, then parse the contents.
    ///
    /// An empty file is treated as an empty allocation table.
    ///
    /// File is created with mode `0600` and its parent directory with
    /// mode `0700` so no other user on the machine can read the
    /// allocation table. Existing files/directories are left alone.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        FileExt::lock_exclusive(&file)?;
        let mut this = Self {
            file,
            data: LockFile::default(),
        };
        this.reload()?;
        Ok(this)
    }

    fn reload(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut buf = String::new();
        self.file.read_to_string(&mut buf)?;
        self.data = if buf.trim().is_empty() {
            LockFile::default()
        } else {
            serde_json::from_str(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("pods.json parse error: {e}"),
                )
            })?
        };
        Ok(())
    }

    pub fn data(&self) -> &LockFile {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut LockFile {
        &mut self.data
    }

    /// Serialise `self.data` back to the file (truncate + rewrite).
    pub fn save(&mut self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(&json)?;
        self.file.sync_data()?;
        Ok(())
    }
}

impl Drop for LockFileGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register_pod;
    use crate::test_util::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_empty_lock_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
        let guard = LockFileGuard::open(&path).unwrap();
        assert!(guard.data().allocations.is_empty());
        assert!(path.exists());
    }

    #[test]
    fn open_creates_file_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("yoi");
        let path = parent.join("pods.json");
        let _guard = LockFileGuard::open(&path).unwrap();
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "file mode = {file_mode:o}");
        let dir_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode = {dir_mode:o}");
    }

    #[test]
    fn save_and_reopen_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
        {
            let mut g = open_empty(&path);
            register_pod(
                &mut g,
                "a".into(),
                std::process::id(),
                sock("a"),
                vec![write_rule("/src", true)],
                sid(),
            )
            .unwrap();
        }
        let guard = LockFileGuard::open(&path).unwrap();
        assert_eq!(guard.data().allocations.len(), 1);
        assert_eq!(guard.data().allocations[0].pod_name, "a");
    }

    #[test]
    fn find_by_session_skips_none_placeholders() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
        let mut g = open_empty(&path);
        // Pre-reservation: delegate_scope leaves segment_id = None
        // until adopt_allocation rewrites it. find_by_segment must not
        // match those placeholders, otherwise a freshly-spawning child
        // would shadow itself before it has even chosen a session.
        register_pod(
            &mut g,
            "parent".into(),
            std::process::id(),
            sock("parent"),
            vec![write_rule("/p", true)],
            sid(),
        )
        .unwrap();
        crate::delegate_scope(
            &mut g,
            "parent",
            "child".into(),
            std::process::id(),
            sock("child"),
            vec![write_rule("/p/sub", true)],
        )
        .unwrap();

        let target_session = sid();
        // The placeholder allocation has segment_id = None and must
        // not be returned for any lookup.
        assert!(g.data().find_by_segment(target_session).is_none());

        // After adopt-style rewrite, the same allocation is now found.
        g.data_mut().find_mut("child").unwrap().segment_id = Some(target_session);
        let found = g.data().find_by_segment(target_session).unwrap();
        assert_eq!(found.pod_name, "child");
    }
}
