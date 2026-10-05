#![cfg(target_os = "linux")]
use crate::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
struct Filtered {
    root: PathBuf,
    seen: Arc<Mutex<Vec<PathBuf>>>,
}
impl FsAccessPolicy for Filtered {
    fn is_readable(&self, path: &Path) -> bool {
        path.starts_with(&self.root) && !path.starts_with(self.root.join("search/denied"))
    }
    fn is_writable(&self, _: &Path) -> bool {
        false
    }
    fn open_traversal_root(
        &self,
        _: &Path,
        resolved: &Path,
    ) -> std::io::Result<Option<FsTraversalRoot>> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open(resolved)?;
        let walk = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        Ok(Some(FsTraversalRoot::new_at(
            walk,
            file,
            resolved.to_path_buf(),
        )))
    }
    fn traversal_filter(&self) -> Option<FsTraversalFilter> {
        let denied = self.root.join("search/denied");
        let seen = self.seen.clone();
        Some(Arc::new(move |path, _| {
            seen.lock().unwrap().push(path.to_path_buf());
            !path.starts_with(&denied)
        }))
    }
    fn open_read_file(&self, _: &Path, resolved: &Path) -> std::io::Result<std::fs::File> {
        assert!(
            !resolved.starts_with(self.root.join("search/denied")),
            "denied source opened"
        );
        std::fs::File::open(resolved)
    }
}
#[test]
fn descriptor_search_anchor_and_predicate_prune_before_denied_directory_descent() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("search/denied/deep")).unwrap();
    std::fs::write(root.path().join("search/allowed.txt"), "needle allowed").unwrap();
    std::fs::write(
        root.path().join("search/denied/deep/secret.txt"),
        "needle secret",
    )
    .unwrap();
    std::fs::write(root.path().join("outside.txt"), "needle outside").unwrap();
    let policy = Filtered {
        root: root.path().to_path_buf(),
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let glob = run_glob(
        root.path(),
        &root.path().join("search"),
        GlobRequest {
            path: FsPath::new("search").unwrap(),
            pattern: "**/*.txt".into(),
            limit: 100,
        },
        &policy,
    )
    .unwrap();
    assert_eq!(glob.paths, vec![FsPath::new("search/allowed.txt").unwrap()]);
    let grep = run_grep(
        root.path(),
        root.path().join("search"),
        GrepRequest {
            path: FsPath::new("search").unwrap(),
            pattern: "needle".into(),
            glob: None,
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::Content,
            offset: 0,
            limit: 100,
        },
        &policy,
    )
    .unwrap();
    assert_eq!(grep.paths, vec![FsPath::new("search/allowed.txt").unwrap()]);
    let seen = policy.seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|path| path == &root.path().join("search/denied"))
    );
    assert!(
        !seen
            .iter()
            .any(|path| path.starts_with(root.path().join("search/denied/deep")))
    );
    assert!(
        !seen
            .iter()
            .any(|path| path == &root.path().join("outside.txt"))
    );
}
