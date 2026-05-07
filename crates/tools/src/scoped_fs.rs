//! Scope-aware filesystem primitive.
//!
//! `ScopedFs` is the write/read gate layered on top of a [`manifest::Scope`]
//! and a Pod's working directory. The scope decides which paths are
//! readable and writable; the pwd is carried alongside for convenience
//! (Glob/Grep default their search base to it).
//!
//! `ScopedFs` is cheap to clone (`Arc` inside) and carries no per-session
//! state — the read-before-edit policy lives separately in
//! [`crate::Tracker`].

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use manifest::{Scope, SharedScope};

use crate::error::ToolsError;

#[derive(Debug)]
struct ScopedFsInner {
    scope: SharedScope,
    pwd: PathBuf,
}

/// Scope-aware filesystem handle. Clone-cheap (`Arc` inside).
///
/// The wrapped [`SharedScope`] is shared with every clone of this
/// `ScopedFs` and with whoever else holds the same `SharedScope`
/// handle (typically the owning Pod). Mutations to that `SharedScope`
/// propagate atomically; the next permission check inside any
/// `ScopedFs` reads the new view.
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

/// First symlink encountered while resolving a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkInfo {
    /// The symlink path as it appears in the original path chain.
    pub link_path: PathBuf,
    /// The symlink target resolved relative to the symlink's parent when the
    /// link stores a relative target.
    pub target_path: PathBuf,
    /// Best-effort resolved form of the full requested path after replacing
    /// the symlink component with its target and rejoining any remaining tail.
    /// Existing targets are canonicalized; broken targets are left absolute.
    pub resolved_path: PathBuf,
    /// Whether the symlink target itself exists. A missing target is a broken
    /// symlink even when the symlink lives inside an allowed scope.
    pub target_exists: bool,
}

impl ScopedFs {
    /// Create a new [`ScopedFs`] wrapping `scope` and `pwd` in a fresh
    /// [`SharedScope`]. Use [`ScopedFs::with_shared_scope`] when you
    /// need the resulting `ScopedFs` to share scope state with another
    /// holder of the `SharedScope` (typically the Pod).
    pub fn new(scope: Scope, pwd: PathBuf) -> Self {
        Self::with_shared_scope(SharedScope::new(scope), pwd)
    }

    /// Build a [`ScopedFs`] over an existing [`SharedScope`]. The
    /// resulting handle and any future updates the caller pushes to
    /// `scope` are observed by every clone of this `ScopedFs`.
    pub fn with_shared_scope(scope: SharedScope, pwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(ScopedFsInner { scope, pwd }),
        }
    }

    /// Snapshot the current scope. Cheap; the returned `Arc<Scope>` is
    /// a coherent point-in-time view that subsequent mutations do not
    /// affect.
    pub fn scope(&self) -> Arc<Scope> {
        self.inner.scope.snapshot()
    }

    /// Shared scope handle backing this `ScopedFs`. Cloning it lets a
    /// caller (usually the Pod) hold the same view and push updates
    /// that are immediately reflected in subsequent permission checks.
    pub fn shared_scope(&self) -> &SharedScope {
        &self.inner.scope
    }

    /// The Pod's working directory. Glob/Grep default their search base
    /// to this path when callers omit an explicit `path` parameter.
    pub fn pwd(&self) -> &Path {
        &self.inner.pwd
    }

    // =========================================================================
    // Read — scope-checked against readability
    // =========================================================================

    /// Read the full contents of `path` as raw bytes.
    ///
    /// Follows symlinks. Rejects directories, relative paths, paths not
    /// readable by the scope, and missing files.
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, ToolsError> {
        if !path.is_absolute() {
            return Err(ToolsError::RelativePath(path.to_path_buf()));
        }
        let symlink = first_symlink(path);
        let scope = self.inner.scope.load();
        if !scope.is_readable(path) {
            return Err(symlink_out_of_scope_or_plain(
                path,
                symlink.as_ref(),
                "read",
                &scope,
            ));
        }
        if let Some(info) = symlink.as_ref() {
            if !info.target_exists {
                return Err(broken_symlink_error(path, info));
            }
        }
        let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolsError::NotFound(path.to_path_buf()),
            _ => ToolsError::io(path, e),
        })?;
        if meta.is_dir() {
            return Err(if let Some(info) = symlink.as_ref() {
                ToolsError::SymlinkTargetIsDirectory {
                    path: path.to_path_buf(),
                    target: info.resolved_path.clone(),
                }
            } else {
                ToolsError::IsDirectory(path.to_path_buf())
            });
        }
        std::fs::read(path).map_err(|e| ToolsError::io(path, e))
    }

    // =========================================================================
    // Write — scope-checked, atomic
    // =========================================================================

    /// Atomically write `content` to `path`, creating or overwriting it.
    ///
    /// - `path` must be absolute and writable under the scope.
    /// - Paths that are readable but not writable return [`ToolsError::ReadOnly`].
    /// - Paths outside the scope entirely return [`ToolsError::OutOfScope`].
    /// - Missing parent directories are created.
    /// - The actual write uses a sibling tempfile + `persist`, so the
    ///   target file transitions atomically between states.
    ///
    /// This method does **not** consult any read history. Callers that
    /// want the "must read before overwrite" policy should verify with a
    /// [`Tracker`](crate::Tracker) beforehand.
    pub fn write(&self, path: &Path, content: &[u8]) -> Result<WriteOutcome, ToolsError> {
        if !path.is_absolute() {
            return Err(ToolsError::RelativePath(path.to_path_buf()));
        }
        let symlink = first_symlink(path);
        let scope = self.inner.scope.load();
        if !scope.is_writable(path) {
            return Err(if scope.is_readable(path) {
                ToolsError::ReadOnly(path.to_path_buf())
            } else {
                symlink_out_of_scope_or_plain(path, symlink.as_ref(), "write", &scope)
            });
        }
        drop(scope);

        if let Some(info) = symlink.as_ref() {
            if !info.target_exists {
                return Err(broken_symlink_error(path, info));
            }
        }

        // Reject existing directory targets.
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                return Err(if let Some(info) = symlink.as_ref() {
                    ToolsError::SymlinkTargetIsDirectory {
                        path: path.to_path_buf(),
                        target: info.resolved_path.clone(),
                    }
                } else {
                    ToolsError::IsDirectory(path.to_path_buf())
                });
            }
            _ => {}
        }

        let existed = path.exists();
        let write_target = if existed {
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        };

        let parent = write_target.parent().ok_or_else(|| {
            ToolsError::InvalidArgument(format!(
                "path has no parent directory: {}",
                write_target.display()
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
        tmp.write_all(content)
            .map_err(|e| ToolsError::io(&write_target, e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| ToolsError::io(&write_target, e))?;
        tmp.persist(&write_target)
            .map_err(|e| ToolsError::io(&write_target, e.error))?;

        Ok(WriteOutcome {
            bytes_written: content.len(),
            created: !existed,
        })
    }
}

/// Return the first symlink component in `path`, if one exists.
///
/// The function only inspects existing path components. It intentionally uses
/// `symlink_metadata` so the symlink itself can be diagnosed before any later
/// `metadata` call follows it and collapses the reason into `NotFound` or
/// `OutOfScope`.
pub fn first_symlink(path: &Path) -> Option<SymlinkInfo> {
    if !path.is_absolute() {
        return None;
    }

    let mut cur = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        cur.push(component.as_os_str());
        let meta = std::fs::symlink_metadata(&cur).ok()?;
        if !meta.file_type().is_symlink() {
            continue;
        }

        let raw_target = std::fs::read_link(&cur).ok()?;
        let target_path = if raw_target.is_absolute() {
            raw_target
        } else {
            cur.parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(raw_target)
        };
        let target_exists = target_path.exists();
        let mut resolved_path = target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.clone());
        for remaining in components {
            resolved_path.push(remaining.as_os_str());
        }

        return Some(SymlinkInfo {
            link_path: cur,
            target_path,
            resolved_path,
            target_exists,
        });
    }

    None
}

pub fn direct_symlink(path: &Path) -> Option<SymlinkInfo> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        first_symlink(path)
    } else {
        None
    }
}

fn symlink_out_of_scope_or_plain(
    path: &Path,
    symlink: Option<&SymlinkInfo>,
    required_permission: &'static str,
    scope: &Scope,
) -> ToolsError {
    if let Some(info) = symlink {
        let link_parent_readable = info
            .link_path
            .parent()
            .map(|parent| scope.is_readable(parent))
            .unwrap_or(false);
        if info.target_exists && link_parent_readable {
            return ToolsError::SymlinkOutOfScope {
                path: path.to_path_buf(),
                target: info.resolved_path.clone(),
                required_permission,
            };
        }
    }
    ToolsError::OutOfScope(path.to_path_buf())
}

fn broken_symlink_error(path: &Path, info: &SymlinkInfo) -> ToolsError {
    ToolsError::BrokenSymlink {
        path: path.to_path_buf(),
        link: info.link_path.clone(),
        target: info.target_path.clone(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{Permission, ScopeConfig, ScopeRule};
    use std::fs;
    use tempfile::TempDir;

    fn make_fs(dir: &TempDir) -> ScopedFs {
        ScopedFs::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        )
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
    fn read_bytes_rejects_paths_outside_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("x.txt");
        fs::write(&outside_file, b"hi").unwrap();

        let scoped = make_fs(&dir);
        let err = scoped.read_bytes(&outside_file).unwrap_err();
        assert!(matches!(err, ToolsError::OutOfScope(_)));
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_reports_broken_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let link = dir.path().join("external-project");
        let target = dir.path().join("missing-target");
        symlink(&target, &link).unwrap();

        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                ToolsError::BrokenSymlink { ref path, link: ref err_link, target: ref err_target }
                    if path == &link && err_link == &link && err_target == &target
            ),
            "expected broken symlink diagnostic, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_reports_symlink_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                ToolsError::SymlinkOutOfScope { ref path, target: ref err_target, required_permission: "read" }
                    if path == &link && err_target == &target.canonicalize().unwrap()
            ),
            "expected symlink out-of-scope diagnostic, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_allows_symlink_file_when_target_is_inside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, b"visible").unwrap();
        let link = dir.path().join("link.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        assert_eq!(fs.read_bytes(&link).unwrap(), b"visible");
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_reports_symlink_to_directory_as_wrong_file_type() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let target_dir = dir.path().join("target-dir");
        fs::create_dir(&target_dir).unwrap();
        let link = dir.path().join("dir-link");
        symlink(&target_dir, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                ToolsError::SymlinkTargetIsDirectory { ref path, ref target }
                    if path == &link && target == &target_dir.canonicalize().unwrap()
            ),
            "expected symlink directory type diagnostic, got {err:?}"
        );
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

    #[cfg(unix)]
    #[test]
    fn write_existing_symlink_file_updates_in_scope_target() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let target = dir.path().join("target.txt");
        fs::write(&target, b"old").unwrap();
        let link = dir.path().join("link.txt");
        symlink(&target, &link).unwrap();

        let out = fs.write(&link, b"new").unwrap();
        assert!(!out.created);
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_reports_symlink_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.write(&link, b"new").unwrap_err();
        assert!(
            matches!(
                err,
                ToolsError::SymlinkOutOfScope { ref path, target: ref err_target, required_permission: "write" }
                    if path == &link && err_target == &target.canonicalize().unwrap()
            ),
            "expected write symlink out-of-scope diagnostic, got {err:?}"
        );
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
    fn write_rejects_readonly_path() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: vec![ScopeRule {
                target: sub.clone(),
                permission: Permission::Write,
                recursive: true,
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let scoped = ScopedFs::new(scope, dir.path().to_path_buf());
        let err = scoped.write(&sub.join("locked.txt"), b"x").unwrap_err();
        assert!(
            matches!(err, ToolsError::ReadOnly(_)),
            "expected ReadOnly, got {err:?}"
        );
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

    // -------------------------------------------------------------------------
    // Dynamic scope: SharedScope mutations propagate into ScopedFs decisions
    // -------------------------------------------------------------------------

    #[test]
    fn add_allow_rule_through_shared_scope_grows_readable_set() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let extra = TempDir::new().unwrap();
        let extra_file = extra.path().join("x.txt");
        fs::write(&extra_file, b"hi").unwrap();

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs = ScopedFs::with_shared_scope(shared.clone(), dir.path().to_path_buf());

        // Before: extra is out of scope.
        let err = fs.read_bytes(&extra_file).unwrap_err();
        assert!(matches!(err, ToolsError::OutOfScope(_)));

        // Push an allow(Read) rule.
        shared
            .update(|cur| {
                cur.with_added_allow_rules([ScopeRule {
                    target: extra.path().to_path_buf(),
                    permission: Permission::Read,
                    recursive: true,
                }])
            })
            .unwrap();

        // After: read goes through.
        assert_eq!(fs.read_bytes(&extra_file).unwrap(), b"hi");
        // But write still fails — allow only granted Read.
        let err = fs.write(&extra.path().join("y.txt"), b"x").unwrap_err();
        assert!(
            matches!(err, ToolsError::ReadOnly(_)),
            "expected ReadOnly, got {err:?}"
        );
    }

    #[test]
    fn revoke_write_through_shared_scope_blocks_subsequent_writes() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let target = sub.join("a.txt");

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs = ScopedFs::with_shared_scope(shared.clone(), dir.path().to_path_buf());

        // Write succeeds initially.
        fs.write(&target, b"first").unwrap();

        // Revoke Write on `sub` (push a deny(Write) rule).
        shared
            .update(|cur| {
                cur.with_added_deny_rules([ScopeRule {
                    target: sub.clone(),
                    permission: Permission::Write,
                    recursive: true,
                }])
            })
            .unwrap();

        // Subsequent write fails with ReadOnly — Read is preserved.
        let err = fs.write(&target, b"second").unwrap_err();
        assert!(
            matches!(err, ToolsError::ReadOnly(_)),
            "expected ReadOnly after revoke, got {err:?}"
        );
        // Read still works.
        assert_eq!(fs.read_bytes(&target).unwrap(), b"first");
    }

    #[test]
    fn shared_scope_changes_propagate_across_clones() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("a.txt");

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs1 = ScopedFs::with_shared_scope(shared.clone(), dir.path().to_path_buf());
        let fs2 = fs1.clone();

        // fs1 writes; both clones see the file.
        fs1.write(&target, b"hi").unwrap();
        assert_eq!(fs2.read_bytes(&target).unwrap(), b"hi");

        // Revoke write through the original handle.
        shared
            .update(|cur| {
                cur.with_added_deny_rules([ScopeRule {
                    target: dir.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                }])
            })
            .unwrap();

        // Both clones reject writes now — they share the same SharedScope.
        assert!(matches!(
            fs1.write(&target, b"x").unwrap_err(),
            ToolsError::ReadOnly(_)
        ));
        assert!(matches!(
            fs2.write(&target, b"x").unwrap_err(),
            ToolsError::ReadOnly(_)
        ));
    }
}
