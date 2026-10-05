#![cfg(target_os = "linux")]
use crate::*;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Access {
    root: PathBuf,
    denied: Option<PathBuf>,
    opened: Mutex<Vec<PathBuf>>,
    enumerated: Mutex<Vec<PathBuf>>,
    checks: AtomicUsize,
    cancel_at: usize,
}
impl Access {
    fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            denied: None,
            opened: Mutex::new(Vec::new()),
            enumerated: Mutex::new(Vec::new()),
            checks: AtomicUsize::new(0),
            cancel_at: usize::MAX,
        }
    }
}
impl FsAccessPolicy for Access {
    fn is_readable(&self, path: &Path) -> bool {
        path.starts_with(&self.root) && self.denied.as_ref().is_none_or(|denied| path != denied)
    }
    fn is_writable(&self, _: &Path) -> bool {
        false
    }
    fn resolve_access_path(&self, path: &Path) -> std::io::Result<PathBuf> {
        Ok(path.into())
    }
    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.checks.fetch_add(1, Ordering::Relaxed) >= self.cancel_at {
            Err(std::io::ErrorKind::Interrupted.into())
        } else {
            Ok(())
        }
    }
    fn open_read_file(&self, logical: &Path, resolved: &Path) -> std::io::Result<std::fs::File> {
        assert!(
            self.is_readable(logical),
            "denied ignore/source file opened"
        );
        self.opened.lock().unwrap().push(logical.into());
        open_beneath_no_symlinks(&self.root, resolved)
    }
    fn read_metadata(&self, _: &Path, resolved: &Path) -> std::io::Result<std::fs::Metadata> {
        open_beneath_no_symlinks(&self.root, resolved)?.metadata()
    }
    fn open_traversal_root(
        &self,
        _: &Path,
        resolved: &Path,
    ) -> std::io::Result<Option<FsTraversalRoot>> {
        use std::os::fd::AsRawFd;
        let directory = open_beneath_no_symlinks(&self.root, resolved)?;
        let walk = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        Ok(Some(FsTraversalRoot::new_at(
            walk,
            directory,
            resolved.into(),
        )))
    }
    fn open_read_dir(&self, logical: &Path, resolved: &Path) -> std::io::Result<std::fs::ReadDir> {
        use std::os::fd::AsRawFd;
        self.enumerated.lock().unwrap().push(logical.into());
        let directory = open_beneath_no_symlinks(&self.root, resolved)?;
        std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
    }
}
fn glob(root: &Path, access: &dyn FsAccessPolicy) -> Result<GlobResult, FsError> {
    run_glob(
        root,
        root,
        GlobRequest {
            path: FsPath::root(),
            pattern: "**/*.txt".into(),
            limit: 100,
        },
        access,
    )
}
#[test]
fn descriptor_glob_prunes_ignored_directories_before_enumeration_and_loading_rules() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("ignored/deep")).unwrap();
    std::fs::write(root.path().join(".ignore"), "ignored/\n").unwrap();
    // This rejected directory must never even attempt to load ignore files.
    std::fs::File::create(root.path().join("ignored/.ignore"))
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    std::fs::write(root.path().join("ignored/deep/secret.txt"), "secret").unwrap();
    std::fs::write(root.path().join("visible.txt"), "visible").unwrap();
    let access = Access::new(root.path());
    assert_eq!(
        glob(root.path(), &access).unwrap().paths,
        vec![FsPath::new("visible.txt").unwrap()]
    );
    assert_eq!(
        *access.enumerated.lock().unwrap(),
        vec![root.path().to_path_buf()]
    );
    assert!(
        !access
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.starts_with(root.path().join("ignored")))
    );
}
#[test]
fn descriptor_glob_denied_and_symlinked_ignore_files_cannot_change_selection() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("rules"), "visible.txt\n").unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("rules"), root.path().join(".gitignore"))
        .unwrap();
    std::fs::write(root.path().join(".ignore"), "visible.txt\n").unwrap();
    std::fs::write(root.path().join("visible.txt"), "visible").unwrap();
    let mut access = Access::new(root.path());
    access.denied = Some(root.path().join(".ignore"));
    assert_eq!(
        glob(root.path(), &access).unwrap().paths,
        vec![FsPath::new("visible.txt").unwrap()]
    );
    assert!(
        !access
            .opened
            .lock()
            .unwrap()
            .contains(&root.path().join(".ignore"))
    );
}
#[test]
fn descriptor_ignore_loading_checks_cancellation_without_retrying_interrupted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".ignore"), "ignored.txt\n".repeat(1000)).unwrap();
    let mut access = Access::new(root.path());
    // First check is before resolution, second at the first fill_buf.
    access.cancel_at = 1;
    let error = crate::descriptor_ignore::add_checked_ignore_file(
        &mut ignore::gitignore::GitignoreBuilder::new(root.path()),
        &root.path().join(".ignore"),
        &mut 1024_000,
        &access,
        true,
    )
    .unwrap_err();
    assert!(
        matches!(error, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::Interrupted)
    );
    assert_eq!(access.checks.load(Ordering::Relaxed), 2);
}
#[test]
fn descriptor_ignore_loading_bounds_file_line_and_aggregate_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".ignore");
    let access = Access::new(root.path());
    std::fs::File::create(&path)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        glob(root.path(), &access),
        Err(FsError::InvalidArgument(_))
    ));
    std::fs::write(&path, "x".repeat(16 * 1024 + 1)).unwrap();
    assert!(matches!(
        glob(root.path(), &access),
        Err(FsError::InvalidArgument(_))
    ));
    std::fs::write(&path, "ignored.txt\n").unwrap();
    let error = crate::descriptor_ignore::add_checked_ignore_file(
        &mut ignore::gitignore::GitignoreBuilder::new(root.path()),
        &path,
        &mut 3,
        &access,
        true,
    )
    .unwrap_err();
    assert!(
        matches!(error, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::InvalidData)
    );
}
#[test]
fn descriptor_ignore_ancestry_is_bounded_without_enumerating_a_tree() {
    let root = tempfile::tempdir().unwrap();
    let access = Access::new(root.path());
    let base = (0..128).fold(root.path().to_path_buf(), |path, _| path.join("d"));
    assert!(matches!(
        crate::descriptor_ignore::DescriptorIgnores::new(root.path(), &base, &access),
        Err(FsError::InvalidArgument(_))
    ));
    assert!(access.opened.lock().unwrap().is_empty());
}

struct Plain(PathBuf);
impl FsAccessPolicy for Plain {
    fn is_readable(&self, path: &Path) -> bool {
        path.starts_with(&self.0)
    }
    fn is_writable(&self, _: &Path) -> bool {
        false
    }
}
#[test]
fn descriptor_glob_matches_plain_git_markers_exclude_and_nested_repository_boundaries() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("nested/.git/info")).unwrap();
    std::fs::create_dir_all(root.path().join(".git/info")).unwrap();
    std::fs::write(
        root.path().join(".gitignore"),
        "*.outer.txt\n!include.txt\n",
    )
    .unwrap();
    std::fs::write(root.path().join(".ignore"), "always.txt\n").unwrap();
    std::fs::write(
        root.path().join(".git/info/exclude"),
        "excluded.txt\ninclude.txt\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("nested/.git/info/exclude"),
        "/excluded.txt\n",
    )
    .unwrap();
    for name in [
        "a.outer.txt",
        "excluded.txt",
        "include.txt",
        "always.txt",
        "nested/a.outer.txt",
        "nested/excluded.txt",
        "nested/always.txt",
    ] {
        std::fs::write(root.path().join(name), "data").unwrap();
    }
    let native = glob(root.path(), &Access::new(root.path())).unwrap();
    let ordinary = glob(root.path(), &Plain(root.path().into())).unwrap();
    assert_eq!(native, ordinary);
    assert_eq!(
        native.paths,
        vec![
            FsPath::new("include.txt").unwrap(),
            FsPath::new("nested/a.outer.txt").unwrap()
        ]
    );
}
#[test]
fn descriptor_glob_requires_authorized_git_marker_for_gitignore_not_for_ignore() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".gitignore"), "gitignored.txt\n").unwrap();
    std::fs::write(root.path().join(".ignore"), "ignored.txt\n").unwrap();
    for name in ["gitignored.txt", "ignored.txt", "visible.txt"] {
        std::fs::write(root.path().join(name), "data").unwrap();
    }
    let native = glob(root.path(), &Access::new(root.path())).unwrap();
    // Ambient ancestors may themselves be repositories (e.g. the harness's
    // TMPDIR). Their markers/ignore files are outside this provider root and
    // cannot activate Git rules or be inspected by descriptor search.
    assert_eq!(native.paths.len(), 2);
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let native = glob(root.path(), &Access::new(root.path())).unwrap();
    assert_eq!(
        native,
        glob(root.path(), &Plain(root.path().into())).unwrap()
    );
    assert_eq!(native.paths, vec![FsPath::new("visible.txt").unwrap()]);
    let mut denied = Access::new(root.path());
    denied.denied = Some(root.path().join(".git"));
    assert_eq!(glob(root.path(), &denied).unwrap().paths.len(), 2);
    std::fs::remove_dir(root.path().join(".git")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("exclude"), "visible.txt\n").unwrap();
    std::fs::write(
        root.path().join(".git"),
        format!("gitdir: {}\n", outside.path().display()),
    )
    .unwrap();
    // Real managed Workdirs use a .git file. Activate root .gitignore without
    // failing on .git/info/exclude or following external administration paths.
    let access = Access::new(root.path());
    assert_eq!(
        glob(root.path(), &access).unwrap().paths,
        vec![FsPath::new("visible.txt").unwrap()]
    );
    std::fs::write(root.path().join("visible.txt"), "needle\n").unwrap();
    let grep = run_grep(
        root.path(),
        root.path().into(),
        GrepRequest {
            path: FsPath::root(),
            pattern: "needle".into(),
            glob: None,
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            offset: 0,
            limit: 100,
        },
        &access,
    )
    .unwrap();
    assert_eq!(grep.paths, vec![FsPath::new("visible.txt").unwrap()]);
    assert!(
        !access
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.starts_with(outside.path()))
    );
}
