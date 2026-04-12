//! Scope-aware filesystem primitive.
//!
//! `ScopedFs` represents **only** the write-block boundary: it knows a
//! [`manifest::Scope`] and refuses writes outside of it. It carries no
//! per-session state and is cheap to clone (pod-lifetime, reusable across
//! sessions). The read-before-edit policy lives separately in
//! [`crate::ReadTracker`].
//!
//! Reads are unrestricted by design (see `tickets/builtin-tools.md`).

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use manifest::Scope;

use crate::error::ToolsError;

#[derive(Debug)]
struct ScopedFsInner {
    scope: Scope,
}

/// Scope-aware filesystem handle. Clone-cheap (`Arc` inside).
#[derive(Debug, Clone)]
pub struct ScopedFs {
    inner: Arc<ScopedFsInner>,
}

/// Outcome of a [`ScopedFs::write`] call.
#[derive(Debug, Clone, Copy)]
pub struct WriteOutcome {
    pub bytes_written: usize,
    pub created: bool,
}

impl ScopedFs {
    /// Create a new [`ScopedFs`] wrapping the given [`Scope`].
    pub fn new(scope: Scope) -> Self {
        Self {
            inner: Arc::new(ScopedFsInner { scope }),
        }
    }

    /// The underlying [`Scope`].
    pub fn scope(&self) -> &Scope {
        &self.inner.scope
    }

    // =========================================================================
    // Read — unrestricted
    // =========================================================================

    /// Read the full contents of `path` as raw bytes.
    ///
    /// Follows symlinks. Rejects directories, relative paths, and missing
    /// files. No scope check.
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, ToolsError> {
        if !path.is_absolute() {
            return Err(ToolsError::RelativePath(path.to_path_buf()));
        }
        let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolsError::NotFound(path.to_path_buf()),
            _ => ToolsError::io(path, e),
        })?;
        if meta.is_dir() {
            return Err(ToolsError::IsDirectory(path.to_path_buf()));
        }
        std::fs::read(path).map_err(|e| ToolsError::io(path, e))
    }

    // =========================================================================
    // Write — scope-checked, atomic
    // =========================================================================

    /// Atomically write `content` to `path`, creating or overwriting it.
    ///
    /// - `path` must be absolute and inside the scope (delegates to
    ///   [`Scope::contains`]).
    /// - Missing parent directories inside the scope are created.
    /// - The actual write uses a sibling tempfile + `persist`, so the
    ///   target file transitions atomically between states.
    ///
    /// This method does **not** consult any read history. Callers that
    /// want the "must read before overwrite" policy should verify with a
    /// [`ReadTracker`](crate::ReadTracker) beforehand.
    pub fn write(&self, path: &Path, content: &[u8]) -> Result<WriteOutcome, ToolsError> {
        if !path.is_absolute() {
            return Err(ToolsError::RelativePath(path.to_path_buf()));
        }
        if !self.inner.scope.contains(path) {
            return Err(ToolsError::OutOfScope(path.to_path_buf()));
        }

        // Reject existing directory targets.
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                return Err(ToolsError::IsDirectory(path.to_path_buf()));
            }
            _ => {}
        }

        let existed = path.exists();

        let parent = path.parent().ok_or_else(|| {
            ToolsError::InvalidArgument(format!(
                "path has no parent directory: {}",
                path.display()
            ))
        })?;
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| ToolsError::io(parent, e))?;
        }

        let tmp_parent: &Path = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let mut tmp = tempfile::NamedTempFile::new_in(tmp_parent)
            .map_err(|e| ToolsError::io(tmp_parent, e))?;
        tmp.write_all(content).map_err(|e| ToolsError::io(path, e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| ToolsError::io(path, e))?;
        tmp.persist(path)
            .map_err(|e| ToolsError::io(path, e.error))?;

        Ok(WriteOutcome {
            bytes_written: content.len(),
            created: !existed,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_fs(dir: &TempDir) -> ScopedFs {
        ScopedFs::new(Scope::new(dir.path()).unwrap())
    }

    // -------------------------------------------------------------------------
    // read_bytes
    // -------------------------------------------------------------------------

    #[test]
    fn read_bytes_returns_content() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(fs.read_bytes(&file).unwrap(), b"abc");
    }

    #[test]
    fn read_bytes_rejects_relative() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(Path::new("rel.txt")).unwrap_err();
        assert!(matches!(err, ToolsError::RelativePath(_)));
    }

    #[test]
    fn read_bytes_rejects_directory() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(dir.path()).unwrap_err();
        assert!(matches!(err, ToolsError::IsDirectory(_)));
    }

    #[test]
    fn read_bytes_rejects_missing() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(&dir.path().join("nope.txt")).unwrap_err();
        assert!(matches!(err, ToolsError::NotFound(_)));
    }

    #[test]
    fn read_bytes_allows_paths_outside_scope() {
        // Reads are unrestricted — scope only gates writes.
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("x.txt");
        fs::write(&outside_file, b"hi").unwrap();

        let scoped = make_fs(&dir);
        assert_eq!(scoped.read_bytes(&outside_file).unwrap(), b"hi");
    }

    // -------------------------------------------------------------------------
    // write
    // -------------------------------------------------------------------------

    #[test]
    fn write_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("new.txt");
        let out = fs.write(&file, b"hello").unwrap();
        assert!(out.created);
        assert_eq!(out.bytes_written, 5);
        assert_eq!(fs::read(&file).unwrap(), b"hello");
    }

    #[test]
    fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("a.txt");
        fs::write(&file, b"old").unwrap();
        let out = fs.write(&file, b"new").unwrap();
        assert!(!out.created);
        assert_eq!(fs::read(&file).unwrap(), b"new");
    }

    #[test]
    fn write_rejects_out_of_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(&outside.path().join("x"), b"x").unwrap_err();
        assert!(matches!(err, ToolsError::OutOfScope(_)));
    }

    #[test]
    fn write_rejects_relative_path() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(Path::new("rel.txt"), b"x").unwrap_err();
        assert!(matches!(err, ToolsError::RelativePath(_)));
    }

    #[test]
    fn write_creates_missing_parents_inside_scope() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let nested = dir.path().join("a/b/c/deep.txt");
        fs.write(&nested, b"x").unwrap();
        assert_eq!(fs::read(&nested).unwrap(), b"x");
    }

    #[test]
    fn write_rejects_directory_target() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(dir.path(), b"x").unwrap_err();
        assert!(matches!(err, ToolsError::IsDirectory(_)));
    }
}
