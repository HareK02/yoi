//! Foundational filesystem operation contracts and provider-side search.
//!
//! This crate deliberately has no dependency on Workdir identity, Runtime
//! transport, or LLM Tool implementations. Paths are logical and root-relative;
//! providers supply host roots and access policy.

mod checked;
pub use checked::{CheckedTarget, identity_validator};
mod descriptor_ignore;
mod glob;
#[cfg(test)]
mod ignore_tests;
mod local;
mod operation;
mod search;
pub mod text;
#[cfg(test)]
mod traversal_tests;
mod walk;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use glob::run_glob;
pub use local::{
    resolve_access_path, run_edit, run_list, run_read, run_read_bounded, run_read_bytes,
    run_read_bytes_bounded, run_stat, run_write,
};
pub use operation::*;
pub use search::run_grep;

/// Provider-side ceiling for directory/tree traversal, independent of result truncation.
pub const MAX_TRAVERSAL_ENTRIES: usize = 100_000;
/// Provider-side ceiling for aggregate retained result paths per operation.
pub const MAX_RESULT_PATH_BYTES: usize = 1024 * 1024;
/// Provider-side ceiling for aggregate grep source bytes per operation.
pub const MAX_GREP_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// Open `path` beneath `root` without following any symbolic link. Linux uses
/// `openat2` so resolution and open are one kernel-enforced operation. Other
/// platforms fail closed rather than silently weakening an External grant.
#[cfg(target_os = "linux")]
pub fn open_root_no_symlinks(root: &Path) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;

    let root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "root contains NUL"))?;
    // SAFETY: `root` is a valid C string and the returned descriptor is checked
    // before ownership is transferred to File.
    let fd: RawFd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a new owned descriptor returned by open.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
pub fn open_root_no_symlinks(_root: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "root-confined no-symlink open is unavailable on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub fn open_beneath_no_symlinks_at(
    root: &std::fs::File,
    relative: &Path,
) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;

    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    const RESOLVE_NO_XDEV: u64 = 0x01;
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;

    if relative.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path is outside provider root",
        ));
    }
    let relative = if relative.as_os_str().is_empty() {
        CString::new(".").expect("static path")
    } else {
        CString::new(relative.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
        })?
    };
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK) as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV,
    };
    // SAFETY: `how` and the path remain alive for the syscall duration, and the
    // borrowed root descriptor remains valid for the call.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            relative.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        ) as RawFd
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ELOOP) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "symbolic links are not permitted by this provider",
            ));
        }
        return Err(error);
    }
    // SAFETY: fd is a new owned descriptor returned by openat2.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
pub fn open_beneath_no_symlinks_at(
    _root: &std::fs::File,
    _relative: &Path,
) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "root-confined no-symlink open is unavailable on this platform",
    ))
}

#[cfg(target_os = "linux")]
pub fn open_beneath_no_symlinks(root: &Path, path: &Path) -> std::io::Result<std::fs::File> {
    let relative = path.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path is outside provider root",
        )
    })?;
    let root = open_root_no_symlinks(root)?;
    open_beneath_no_symlinks_at(&root, relative)
}

#[cfg(not(target_os = "linux"))]
pub fn open_beneath_no_symlinks(_root: &Path, _path: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "root-confined no-symlink open is unavailable on this platform",
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicWriteMode {
    CreateNew,
    Replace,
}

#[cfg(target_os = "linux")]
pub fn atomic_write_beneath_no_symlinks_at(
    root: &std::fs::File,
    relative: &Path,
    content: &[u8],
    mode: AtomicWriteMode,
    before_commit: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::ffi::{CString, OsString};
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;

    if relative.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path is outside provider root",
        ));
    }
    let mut components = relative.components().peekable();
    let mut parent = root.try_clone()?;
    let mut file_name = None::<OsString>;
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "path is outside provider root",
            ));
        };
        if components.peek().is_none() {
            file_name = Some(name.to_os_string());
            break;
        }
        let name = CString::new(name.as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
        })?;
        // SAFETY: `name` is a valid C string and `parent` remains open.
        let mut fd: RawFd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
            // SAFETY: same valid directory descriptor and component C string.
            let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) };
            if created < 0 {
                let create_error = std::io::Error::last_os_error();
                if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(create_error);
                }
            }
            // SAFETY: same as the first openat; a racing symlink is rejected by O_NOFOLLOW.
            fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: fd is a fresh descriptor returned by openat.
        parent = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    let file_name = file_name.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "file path is empty")
    })?;
    let parent_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let mut temporary = tempfile::NamedTempFile::new_in(&parent_path)?;
    temporary.write_all(content)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    before_commit()?;
    let destination = parent_path.join(file_name);
    match mode {
        AtomicWriteMode::CreateNew => temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?,
        AtomicWriteMode::Replace => temporary
            .persist(destination)
            .map_err(|error| error.error)?,
    };
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn atomic_write_beneath_no_symlinks_at(
    _root: &std::fs::File,
    _relative: &Path,
    _content: &[u8],
    _mode: AtomicWriteMode,
    _before_commit: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "root-confined no-symlink write is unavailable on this platform",
    ))
}

/// Keeps a descriptor-backed traversal path alive for the duration of a walk.
#[derive(Debug)]
pub struct FsTraversalRoot {
    path: PathBuf,
    logical_root: Option<PathBuf>,
    _directory: std::fs::File,
}

impl FsTraversalRoot {
    pub fn new(path: PathBuf, directory: std::fs::File) -> Self {
        Self {
            path,
            logical_root: None,
            _directory: directory,
        }
    }

    /// Map walked paths to this logical anchor rather than the provider root.
    pub fn new_at(path: PathBuf, directory: std::fs::File, logical_root: PathBuf) -> Self {
        Self {
            path,
            logical_root: Some(logical_root),
            _directory: directory,
        }
    }
    pub fn logical_root(&self) -> Option<&Path> {
        self.logical_root.as_deref()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Owned predicate used before walker descent. Paths are provider-logical;
/// true directories require enumeration authority, other entries require read.
pub type FsTraversalFilter = std::sync::Arc<dyn Fn(&Path, bool) -> bool + Send + Sync>;

/// Provider-owned access policy used by local filesystem operations.
pub trait FsAccessPolicy: Send + Sync {
    fn is_readable(&self, path: &Path) -> bool;
    fn is_writable(&self, path: &Path) -> bool;

    fn can_enumerate_directory(&self, logical: &Path, resolved: &Path) -> bool {
        self.is_readable_paths(logical, resolved)
    }
    /// Owned because the shared walker requires a 'static entry predicate.
    fn traversal_filter(&self) -> Option<FsTraversalFilter> {
        None
    }

    /// Cooperatively stop provider-owned filesystem work. Implementations may
    /// use this for per-operation cancellation and deadlines.
    fn check_cancelled(&self) -> std::io::Result<()> {
        Ok(())
    }

    /// Resolve a logical path for authorization. Descriptor-confined providers
    /// may retain the lexical path because the subsequent open is the authority.
    fn resolve_access_path(&self, logical: &Path) -> std::io::Result<PathBuf> {
        resolve_access_path(logical)
    }

    /// Return a descriptor-backed traversal root when pathname walking would
    /// violate provider confinement. The retained directory keeps procfs-style
    /// descriptor paths valid for the complete walk.
    fn open_traversal_root(
        &self,
        _logical: &Path,
        _resolved: &Path,
    ) -> std::io::Result<Option<FsTraversalRoot>> {
        Ok(None)
    }

    /// Open an already-authorized readable file. Capability providers override
    /// this to bind path resolution and open into one root-confined operation.
    fn open_read_file(&self, _logical: &Path, resolved: &Path) -> std::io::Result<std::fs::File> {
        std::fs::File::open(resolved)
    }

    /// Obtain metadata for an already-authorized path. Capability providers
    /// override this to prevent a path swap between authorization and stat.
    fn read_metadata(
        &self,
        _logical: &Path,
        resolved: &Path,
    ) -> std::io::Result<std::fs::Metadata> {
        std::fs::metadata(resolved)
    }

    /// Bound provider-owned mutation preimages and results before allocation or
    /// commit. `None` retains the provider's ordinary filesystem behavior.
    fn max_write_bytes(&self) -> Option<usize> {
        None
    }

    /// Bound provider-owned Edit result cardinality before mutation.
    fn max_edit_replacements(&self) -> Option<usize> {
        None
    }

    /// Atomically replace or create an already-authorized writable file.
    /// Capability providers override this to keep parent traversal descriptor-
    /// confined through the final rename.
    fn atomic_write_file(
        &self,
        _logical: &Path,
        resolved: &Path,
        content: &[u8],
        mode: AtomicWriteMode,
    ) -> std::io::Result<()> {
        use std::io::Write;

        let parent = resolved.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "file path has no parent")
        })?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(content)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        self.check_cancelled()?;
        match mode {
            AtomicWriteMode::CreateNew => temporary
                .persist_noclobber(resolved)
                .map_err(|error| error.error)?,
            AtomicWriteMode::Replace => temporary.persist(resolved).map_err(|error| error.error)?,
        };
        Ok(())
    }

    /// Open an already-authorized directory for bounded enumeration.
    /// Capability providers override this to bind traversal to a confined
    /// directory descriptor rather than reopening a mutable path.
    fn open_read_dir(&self, _logical: &Path, resolved: &Path) -> std::io::Result<std::fs::ReadDir> {
        std::fs::read_dir(resolved)
    }

    /// Authorize both the Workdir-visible path and its provider-resolved
    /// target. Implementations that do not distinguish symbolic-link identity
    /// retain resolved-target semantics through the defaults.
    fn is_readable_paths(&self, logical: &Path, resolved: &Path) -> bool {
        let _ = logical;
        self.is_readable(resolved)
    }

    fn is_writable_paths(&self, logical: &Path, resolved: &Path) -> bool {
        let _ = logical;
        self.is_writable(resolved)
    }
}

/// First symlink encountered while resolving a provider path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkInfo {
    pub link_path: PathBuf,
    pub target_path: PathBuf,
    pub resolved_path: PathBuf,
    pub target_exists: bool,
}

pub fn first_symlink(path: &Path) -> Option<SymlinkInfo> {
    if !path.is_absolute() {
        return None;
    }
    let mut current = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        current.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&current).ok()?;
        if !metadata.file_type().is_symlink() {
            continue;
        }
        let raw_target = std::fs::read_link(&current).ok()?;
        let target_path = if raw_target.is_absolute() {
            raw_target
        } else {
            current
                .parent()
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
            link_path: current,
            target_path,
            resolved_path,
            target_exists,
        });
    }
    None
}

pub fn direct_symlink(path: &Path) -> Option<SymlinkInfo> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata
        .file_type()
        .is_symlink()
        .then(|| first_symlink(path))
        .flatten()
}

#[derive(Debug, Error)]
pub enum FsError {
    #[error("invalid logical filesystem path: {0}")]
    InvalidPath(String),
    #[error("operation requires an absolute provider path, got {0}")]
    RelativePath(PathBuf),
    #[error("path is outside readable provider scope: {0}")]
    OutOfScope(PathBuf),
    #[error("path not found: {0}")]
    NotFound(PathBuf),
    #[error("broken symbolic link {link}: {target}")]
    BrokenSymlink {
        path: PathBuf,
        link: PathBuf,
        target: PathBuf,
    },
    #[error("symbolic-link target is outside {required_permission} scope: {path} -> {target}")]
    SymlinkOutOfScope {
        path: PathBuf,
        target: PathBuf,
        required_permission: &'static str,
    },
    #[error("symbolic-link directories are not traversed by {tool}: {path} -> {target}")]
    SymlinkDirectoryNotTraversed {
        tool: &'static str,
        path: PathBuf,
        target: PathBuf,
    },
    #[error("path is read-only: {0}")]
    ReadOnly(PathBuf),
    #[error("path is a directory: {0}")]
    IsDirectory(PathBuf),
    #[error("path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("symbolic-link target is a directory: {path} -> {target}")]
    SymlinkTargetIsDirectory { path: PathBuf, target: PathBuf },
    #[error("filesystem content conflict: {0}")]
    Conflict(String),
    #[error("invalid glob: {0}")]
    InvalidGlob(String),
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
    #[error("invalid filesystem operation argument: {0}")]
    InvalidArgument(String),
    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl FsError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RootAccess(PathBuf);

    impl FsAccessPolicy for RootAccess {
        fn is_readable(&self, path: &Path) -> bool {
            path.starts_with(&self.0)
        }

        fn is_writable(&self, path: &Path) -> bool {
            path.starts_with(&self.0)
        }
    }

    struct CreateRaceAccess {
        root: PathBuf,
        destination: PathBuf,
    }

    impl FsAccessPolicy for CreateRaceAccess {
        fn is_readable(&self, path: &Path) -> bool {
            path.starts_with(&self.root)
        }

        fn is_writable(&self, path: &Path) -> bool {
            path.starts_with(&self.root)
        }

        fn read_metadata(
            &self,
            _logical: &Path,
            _resolved: &Path,
        ) -> std::io::Result<std::fs::Metadata> {
            std::fs::write(&self.destination, b"created concurrently")?;
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "simulated absence before create race",
            ))
        }
    }

    fn grep_request(path: &str, pattern: &str) -> GrepRequest {
        GrepRequest {
            pattern: pattern.to_string(),
            path: FsPath::new(path).unwrap(),
            glob: None,
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::Content,
            limit: 10,
            offset: 0,
        }
    }

    #[test]
    fn bounded_byte_reads_preserve_binary_ranges_and_fence_versions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let original = b"\x89PNG\r\n\x1a\n\0line\n\xfftail";
        std::fs::write(root.join("image.bin"), original).unwrap();
        let access = RootAccess(root.clone());
        let limits = BoundedReadLimits::new(1024, 8).unwrap();

        let first = run_read_bytes_bounded(
            &root,
            ReadBytesRequest {
                path: FsPath::new("image.bin").unwrap(),
                offset: 0,
                max_bytes: 8,
                expected_hash: None,
            },
            &access,
            limits,
        )
        .unwrap();
        assert_eq!(first.bytes, original[..8]);
        assert_eq!(first.total_bytes, original.len() as u64);
        assert!(!first.eof);

        let second = run_read_bytes_bounded(
            &root,
            ReadBytesRequest {
                path: FsPath::new("image.bin").unwrap(),
                offset: 8,
                max_bytes: 8,
                expected_hash: Some(first.content_hash),
            },
            &access,
            limits,
        )
        .unwrap();
        assert_eq!(second.bytes, original[8..16]);
        assert_eq!(second.content_hash, first.content_hash);

        std::fs::write(root.join("image.bin"), b"different bytes long!").unwrap();
        let error = run_read_bytes_bounded(
            &root,
            ReadBytesRequest {
                path: FsPath::new("image.bin").unwrap(),
                offset: 16,
                max_bytes: 8,
                expected_hash: Some(first.content_hash),
            },
            &access,
            limits,
        )
        .expect_err("a later chunk must not cross file versions");
        assert!(matches!(error, FsError::Conflict(_)));
    }

    #[test]
    fn bounded_byte_reads_reject_request_and_source_limit_overflow() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("image.bin"), [0_u8; 17]).unwrap();
        let access = RootAccess(root.clone());
        let limits = BoundedReadLimits::new(16, 8).unwrap();

        let response_error = run_read_bytes_bounded(
            &root,
            ReadBytesRequest {
                path: FsPath::new("image.bin").unwrap(),
                offset: 0,
                max_bytes: 9,
                expected_hash: None,
            },
            &access,
            limits,
        )
        .expect_err("requests above the response ceiling must be rejected");
        assert!(
            matches!(response_error, FsError::InvalidArgument(message) if message.contains("8 bytes"))
        );

        let source_error = run_read_bytes_bounded(
            &root,
            ReadBytesRequest {
                path: FsPath::new("image.bin").unwrap(),
                offset: 0,
                max_bytes: 8,
                expected_hash: None,
            },
            &access,
            limits,
        )
        .expect_err("sources above the provider ceiling must be rejected");
        assert!(
            matches!(source_error, FsError::InvalidArgument(message) if message.contains("provider limit 16"))
        );
    }

    #[test]
    fn cancellation_stops_a_read_already_in_progress() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};

        struct CancellableAccess {
            root: PathBuf,
            checks: AtomicUsize,
            cancelled: AtomicBool,
            state: Mutex<()>,
            changed: Condvar,
        }

        impl FsAccessPolicy for CancellableAccess {
            fn is_readable(&self, path: &Path) -> bool {
                path.starts_with(&self.root)
            }

            fn is_writable(&self, _path: &Path) -> bool {
                false
            }

            fn check_cancelled(&self) -> std::io::Result<()> {
                if self.checks.fetch_add(1, Ordering::SeqCst) == 1 {
                    self.changed.notify_all();
                    let mut state = self.state.lock().unwrap();
                    while !self.cancelled.load(Ordering::Acquire) {
                        state = self.changed.wait(state).unwrap();
                    }
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cancelled",
                    ));
                }
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("large.txt"), vec![b'x'; 128 * 1024]).unwrap();
        let access = Arc::new(CancellableAccess {
            root: root.clone(),
            checks: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            state: Mutex::new(()),
            changed: Condvar::new(),
        });
        let task_access = access.clone();
        let task_root = root.clone();
        let task = std::thread::spawn(move || {
            run_read_bounded(
                &task_root,
                ReadRequest {
                    path: FsPath::new("large.txt").unwrap(),
                    offset: 0,
                    limit: usize::MAX,
                    max_bytes: 1024,
                },
                task_access.as_ref(),
                BoundedReadLimits::new(256 * 1024, 1024).unwrap(),
            )
        });

        let mut state = access.state.lock().unwrap();
        while access.checks.load(Ordering::SeqCst) < 2 {
            state = access.changed.wait(state).unwrap();
        }
        access.cancelled.store(true, Ordering::Release);
        access.changed.notify_all();
        drop(state);

        let error = task
            .join()
            .unwrap()
            .expect_err("read must stop on cancellation");
        assert!(
            matches!(error, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::Interrupted)
        );
    }

    #[test]
    fn logical_paths_reject_absolute_parent_and_backslash_forms() {
        assert!(FsPath::new("src/lib.rs").is_ok());
        assert!(FsPath::new("/tmp/file").is_err());
        assert!(FsPath::new_scoped("/tmp/file").is_ok());
        assert!(FsPath::new_scoped("/tmp/../secret").is_err());
        assert!(FsPath::new("../file").is_err());
        assert!(FsPath::new("src\\lib.rs").is_err());
    }

    #[test]
    fn deserialization_cannot_bypass_logical_path_validation() {
        assert!(serde_json::from_str::<FsPath>(r#""../secret""#).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn root_confined_open_rejects_a_symlink_swap_between_authorization_and_open() {
        use std::os::unix::fs::symlink;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct SwapAccess {
            root: PathBuf,
            selected: PathBuf,
            outside: PathBuf,
            swapped: AtomicBool,
        }
        impl FsAccessPolicy for SwapAccess {
            fn is_readable(&self, path: &Path) -> bool {
                path.starts_with(&self.root)
            }
            fn is_writable(&self, _path: &Path) -> bool {
                false
            }
            fn is_readable_paths(&self, _logical: &Path, _resolved: &Path) -> bool {
                if !self.swapped.swap(true, Ordering::SeqCst) {
                    std::fs::remove_file(&self.selected).unwrap();
                    symlink(&self.outside, &self.selected).unwrap();
                }
                true
            }
            fn open_read_file(
                &self,
                _logical: &Path,
                resolved: &Path,
            ) -> std::io::Result<std::fs::File> {
                open_beneath_no_symlinks(&self.root, resolved)
            }
        }

        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path().canonicalize().unwrap();
        let outside = outside_dir.path().join("secret.txt");
        let selected = root.join("selected.txt");
        std::fs::write(&selected, "safe").unwrap();
        std::fs::write(&outside, "secret").unwrap();
        let access = SwapAccess {
            root: root.clone(),
            selected,
            outside,
            swapped: AtomicBool::new(false),
        };

        let error = run_read_bounded(
            &root,
            ReadRequest {
                path: FsPath::new("selected.txt").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            },
            &access,
            BoundedReadLimits::EXTERNAL_DEFAULT,
        )
        .expect_err("openat2 must reject the swapped symbolic link");
        assert!(matches!(error, FsError::Io { .. }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn root_confined_stat_and_list_reject_swaps_after_authorization() {
        use std::os::unix::fs::symlink;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct SwapAccess {
            root: PathBuf,
            selected: PathBuf,
            outside: PathBuf,
            directory: bool,
            swapped: AtomicBool,
        }
        impl FsAccessPolicy for SwapAccess {
            fn is_readable(&self, path: &Path) -> bool {
                path.starts_with(&self.root)
            }
            fn is_writable(&self, _path: &Path) -> bool {
                false
            }
            fn is_readable_paths(&self, logical: &Path, _resolved: &Path) -> bool {
                if logical == self.selected && !self.swapped.swap(true, Ordering::SeqCst) {
                    if self.directory {
                        std::fs::remove_dir(&self.selected).unwrap();
                    } else {
                        std::fs::remove_file(&self.selected).unwrap();
                    }
                    symlink(&self.outside, &self.selected).unwrap();
                }
                true
            }
            fn read_metadata(
                &self,
                _logical: &Path,
                resolved: &Path,
            ) -> std::io::Result<std::fs::Metadata> {
                open_beneath_no_symlinks(&self.root, resolved)?.metadata()
            }
            fn open_read_dir(
                &self,
                _logical: &Path,
                resolved: &Path,
            ) -> std::io::Result<std::fs::ReadDir> {
                use std::os::fd::AsRawFd;
                let directory = open_beneath_no_symlinks(&self.root, resolved)?;
                std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            }
        }

        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path().canonicalize().unwrap();
        let selected_file = root.join("selected.txt");
        let outside_file = outside_dir.path().join("secret.txt");
        std::fs::write(&selected_file, "safe").unwrap();
        std::fs::write(&outside_file, "secret").unwrap();
        let stat_error = run_stat(
            &root,
            StatRequest {
                path: FsPath::new("selected.txt").unwrap(),
            },
            &SwapAccess {
                root: root.clone(),
                selected: selected_file,
                outside: outside_file,
                directory: false,
                swapped: AtomicBool::new(false),
            },
        )
        .expect_err("confined stat must reject a swapped symlink");
        assert!(matches!(stat_error, FsError::Io { .. }));

        let selected_dir = root.join("selected-dir");
        let outside_content = outside_dir.path().join("outside-dir");
        std::fs::create_dir(&selected_dir).unwrap();
        std::fs::create_dir(&outside_content).unwrap();
        std::fs::write(outside_content.join("secret.txt"), "secret").unwrap();
        let list_error = run_list(
            &root,
            ListRequest {
                path: FsPath::new("selected-dir").unwrap(),
                limit: 10,
            },
            &SwapAccess {
                root: root.clone(),
                selected: selected_dir,
                outside: outside_content,
                directory: true,
                swapped: AtomicBool::new(false),
            },
        )
        .expect_err("confined list must reject a swapped symlink");
        assert!(matches!(list_error, FsError::Io { .. }));
    }

    #[test]
    fn direct_operations_cover_stat_read_write_edit_and_list() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let access = RootAccess(root.clone());
        let path = FsPath::new("notes/item.txt").unwrap();

        let written = run_write(
            &root,
            WriteRequest {
                path: path.clone(),
                content: b"alpha\nbeta\n".to_vec(),
                expected_hash: None,
            },
            &access,
        )
        .unwrap();
        assert!(written.created);

        let read = run_read(
            &root,
            ReadRequest {
                path: path.clone(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            },
            &access,
        )
        .unwrap();
        assert_eq!(read.bytes, b"alpha\nbeta\n");

        let edited = run_edit(
            &root,
            EditRequest {
                path: path.clone(),
                old_string: "beta".to_string(),
                new_string: "gamma".to_string(),
                replace_all: false,
                expected_hash: read.content_hash,
            },
            &access,
        )
        .unwrap();
        assert_eq!(edited.replacements, 1);

        let stat = run_stat(&root, StatRequest { path: path.clone() }, &access).unwrap();
        assert_eq!(stat.kind, EntryKind::File);

        let listed = run_list(
            &root,
            ListRequest {
                path: FsPath::new("notes").unwrap(),
                limit: 10,
            },
            &access,
        )
        .unwrap();
        assert_eq!(listed.total_entries, 1);
        assert_eq!(listed.entries[0].path, path);
    }

    #[test]
    fn glob_and_grep_execute_as_bounded_provider_side_operations() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/a.rs"), "needle one\n").unwrap();
        std::fs::write(temp.path().join("src/b.rs"), "needle two\n").unwrap();
        std::fs::write(temp.path().join("src/c.txt"), "needle hidden\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        let glob = run_glob(
            &root,
            &root,
            GlobRequest {
                pattern: "**/*.rs".to_string(),
                path: FsPath::root(),
                limit: 1,
            },
            &readable,
        )
        .unwrap();
        assert_eq!(glob.paths, vec![FsPath::new("src/a.rs").unwrap()]);
        assert!(glob.truncated);

        let grep = run_grep(
            &root,
            root.clone(),
            GrepRequest {
                pattern: "needle".to_string(),
                path: FsPath::root(),
                glob: Some("**/*.rs".to_string()),
                output_mode: GrepOutputMode::Count,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                file_type: None,
                limit: 10,
                offset: 0,
            },
            &readable,
        )
        .unwrap();
        assert_eq!(grep.match_count, 2);
        assert_eq!(grep.matched_files, 2);
        assert!(!grep.output.contains("c.txt"));
    }

    #[test]
    fn grep_accepts_a_direct_file_without_searching_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected.txt");
        std::fs::write(&selected, "before\nneedle selected\nafter\n").unwrap();
        std::fs::write(temp.path().join("sibling.txt"), "needle sibling\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        let mut request = grep_request("selected.txt", "needle");
        request.before_context = 1;
        request.after_context = 1;
        let direct = run_grep(&root, selected, request, &readable).unwrap();

        assert_eq!(direct.match_count, 1);
        assert_eq!(direct.matched_files, 1);
        assert_eq!(
            direct.output,
            concat!(
                "selected.txt\n",
                "   1 │ before\n",
                " > 2 │ needle selected\n",
                "   3 │ after\n",
            )
        );
        assert!(!direct.output.contains("sibling"));

        let directory = run_grep(
            &root,
            root.clone(),
            GrepRequest {
                pattern: "needle".to_string(),
                path: FsPath::root(),
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: GrepOutputMode::Content,
                limit: 10,
                offset: 0,
            },
            &readable,
        )
        .unwrap();
        assert_eq!(directory.match_count, 2);
        assert_eq!(directory.matched_files, 2);
    }

    #[test]
    fn grep_direct_file_applies_glob_and_type_filters_for_every_output_mode() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let selected = nested.join("selected.rs");
        std::fs::write(&selected, "needle one\nneedle two\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        for mode in [
            GrepOutputMode::Content,
            GrepOutputMode::FilesWithMatches,
            GrepOutputMode::Count,
        ] {
            for (glob, file_type) in [(Some("other/*.rs"), None), (None, Some("python"))] {
                let mut request = grep_request("nested/selected.rs", "needle");
                request.output_mode = mode;
                request.glob = glob.map(str::to_string);
                request.file_type = file_type.map(str::to_string);

                let excluded = run_grep(&root, selected.clone(), request, &readable).unwrap();
                assert_eq!(excluded.output, "", "mode {mode:?}");
                assert_eq!(excluded.match_count, 0, "mode {mode:?}");
                assert_eq!(excluded.matched_files, 0, "mode {mode:?}");
                assert!(!excluded.truncated, "mode {mode:?}");
            }

            let mut request = grep_request("nested/selected.rs", "needle");
            request.output_mode = mode;
            request.glob = Some("nested/*.rs".to_string());
            request.file_type = Some("rust".to_string());
            let matched = run_grep(&root, selected.clone(), request, &readable).unwrap();

            match mode {
                GrepOutputMode::Content => {
                    assert_eq!(matched.match_count, 2);
                    assert_eq!(matched.matched_files, 1);
                    assert!(matched.output.starts_with("nested/selected.rs\n"));
                    assert!(matched.output.contains("> 1 │ needle one"));
                    assert!(matched.output.contains("> 2 │ needle two"));
                }
                GrepOutputMode::FilesWithMatches => {
                    assert_eq!(matched.match_count, 1);
                    assert_eq!(matched.matched_files, 1);
                    assert_eq!(matched.output, "nested/selected.rs\n");
                }
                GrepOutputMode::Count => {
                    assert_eq!(matched.match_count, 2);
                    assert_eq!(matched.matched_files, 1);
                    assert_eq!(matched.output, "nested/selected.rs:2\n");
                }
            }
            assert!(!matched.truncated, "mode {mode:?}");
        }
    }

    #[test]
    fn grep_direct_file_preserves_explicit_hidden_and_gitignored_behavior() {
        let temp = tempfile::tempdir().unwrap();
        let hidden = temp.path().join(".hidden.rs");
        let ignored = temp.path().join("ignored.rs");
        std::fs::write(&hidden, "needle hidden\n").unwrap();
        std::fs::write(&ignored, "needle ignored\n").unwrap();
        std::fs::write(temp.path().join(".gitignore"), "ignored.rs\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        for (path, expected) in [
            (".hidden.rs", "needle hidden"),
            ("ignored.rs", "needle ignored"),
        ] {
            let result = run_grep(
                &root,
                root.join(path),
                grep_request(path, "needle"),
                &readable,
            )
            .unwrap();
            assert_eq!(result.match_count, 1, "path {path}");
            assert!(result.output.contains(expected), "path {path}");
        }
    }

    #[test]
    fn grep_direct_file_preserves_case_multiline_and_bounds() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected.txt");
        std::fs::write(&selected, "NEEDLE first\nstart\nfinish\nneedle last\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        let mut case_request = grep_request("selected.txt", "needle");
        case_request.case_insensitive = true;
        case_request.offset = 1;
        case_request.limit = 1;
        let bounded = run_grep(&root, selected.clone(), case_request, &readable).unwrap();
        assert_eq!(bounded.match_count, 1);
        assert!(!bounded.output.contains("NEEDLE first"));
        assert!(bounded.output.contains("needle last"));
        assert!(bounded.truncated);

        let mut multiline_request = grep_request("selected.txt", "start\\nfinish");
        multiline_request.multiline = true;
        let multiline = run_grep(&root, selected, multiline_request, &readable).unwrap();
        assert_eq!(multiline.match_count, 1);
        assert!(multiline.output.contains("start\nfinish"));
    }

    #[test]
    fn grep_returns_not_found_for_a_missing_direct_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let missing = root.join("missing.txt");
        let readable = RootAccess(root.clone());

        let error = run_grep(
            &root,
            missing.clone(),
            grep_request("missing.txt", "needle"),
            &readable,
        )
        .unwrap_err();

        assert!(matches!(error, FsError::NotFound(path) if path == missing));
    }

    #[cfg(unix)]
    #[test]
    fn grep_traverses_a_direct_symlink_directory_and_rejects_a_broken_path() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());
        std::fs::create_dir(root.join("target-dir")).unwrap();
        std::fs::write(root.join("target-dir/nested.rs"), "needle nested\n").unwrap();
        std::fs::write(root.join("target-file.rs"), "needle file\n").unwrap();
        symlink(root.join("target-file.rs"), root.join("file-link.rs")).unwrap();
        symlink(root.join("target-dir"), root.join("directory-link")).unwrap();
        symlink(root.join("missing-target"), root.join("broken-link")).unwrap();

        let request = |path: &str| grep_request(path, "needle");

        let file_result = run_grep(
            &root,
            root.join("file-link.rs"),
            request("file-link.rs"),
            &readable,
        )
        .unwrap();
        assert_eq!(file_result.match_count, 1);
        assert!(file_result.output.starts_with("file-link.rs\n"));

        let directory_result = run_grep(
            &root,
            root.join("directory-link"),
            request("directory-link"),
            &readable,
        )
        .unwrap();
        assert_eq!(directory_result.match_count, 1);
        assert!(
            directory_result
                .output
                .starts_with("directory-link/nested.rs\n")
        );

        let glob_result = run_glob(
            &root,
            &root.join("directory-link"),
            GlobRequest {
                pattern: "**/*.rs".to_string(),
                path: FsPath::new("directory-link").unwrap(),
                limit: 10,
            },
            &readable,
        )
        .unwrap();
        assert_eq!(
            glob_result.paths,
            vec![FsPath::new("directory-link/nested.rs").unwrap()]
        );

        let broken_error = run_grep(
            &root,
            root.join("broken-link"),
            request("broken-link"),
            &readable,
        )
        .unwrap_err();
        assert!(matches!(
            broken_error,
            FsError::BrokenSymlink { path, .. } if path == root.join("broken-link")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn grep_rejects_a_direct_special_file_as_invalid_argument() {
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("grep.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        let error = run_grep(
            &root,
            socket,
            grep_request("grep.sock", "needle"),
            &readable,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FsError::InvalidArgument(message)
                if message.contains("must be a regular file or directory")
        ));
    }

    #[test]
    fn no_clobber_create_race_is_reported_as_conflict() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("race.txt");
        let root_path = root.path().canonicalize().unwrap();
        let access = CreateRaceAccess {
            root: root_path.clone(),
            destination: destination.clone(),
        };

        let error = run_write(
            &root_path,
            WriteRequest {
                path: FsPath::new("race.txt").unwrap(),
                content: b"worker content".to_vec(),
                expected_hash: None,
            },
            &access,
        )
        .unwrap_err();

        assert!(matches!(error, FsError::Conflict(path) if path == "race.txt"));
        assert_eq!(
            std::fs::read_to_string(destination).unwrap(),
            "created concurrently"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descriptor_confined_atomic_write_rejects_symlinks_and_survives_root_rename() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let selected = parent.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(outside.path(), selected.join("escape")).unwrap();
        let root = open_root_no_symlinks(&selected).unwrap();

        let error = atomic_write_beneath_no_symlinks_at(
            &root,
            Path::new("escape/secret.txt"),
            b"escaped",
            AtomicWriteMode::Replace,
            || Ok(()),
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::NotADirectory | std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "secret"
        );

        std::fs::rename(&selected, parent.path().join("approved-original")).unwrap();
        std::fs::create_dir(&selected).unwrap();
        atomic_write_beneath_no_symlinks_at(
            &root,
            Path::new("nested/created.txt"),
            b"approved",
            AtomicWriteMode::CreateNew,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.path().join("approved-original/nested/created.txt"))
                .unwrap(),
            "approved"
        );
        assert!(!selected.join("nested/created.txt").exists());
    }

    #[test]
    fn grep_content_groups_lines_by_file_and_marks_matches() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("first.txt"),
            "before\nneedle one\nafter\nomitted one\nomitted two\nbefore distant\nneedle distant\nafter distant\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("second.txt"), "needle two\n").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());

        let grep = run_grep(
            &root,
            root.clone(),
            GrepRequest {
                pattern: "needle".to_string(),
                path: FsPath::root(),
                glob: Some("*.txt".to_string()),
                output_mode: GrepOutputMode::Content,
                case_insensitive: false,
                before_context: 1,
                after_context: 1,
                multiline: false,
                file_type: None,
                limit: 20,
                offset: 0,
            },
            &readable,
        )
        .unwrap();

        assert_eq!(grep.match_count, 3);
        assert_eq!(grep.matched_files, 2);
        assert_eq!(
            grep.output,
            concat!(
                "first.txt\n",
                "   1 │ before\n",
                " > 2 │ needle one\n",
                "   3 │ after\n",
                "   …\n",
                "   6 │ before distant\n",
                " > 7 │ needle distant\n",
                "   8 │ after distant\n",
                "\n",
                "second.txt\n",
                " > 1 │ needle two\n",
            )
        );
        assert_eq!(grep.output.matches("first.txt").count(), 1);
        assert_eq!(grep.output.matches("second.txt").count(), 1);
    }
}
