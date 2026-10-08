use fs_operation::{
    EntryKind, FsAccessPolicy, FsError, FsPath, ListCursor, ListRequest, ListResult,
    MAX_RESULT_PATH_BYTES, run_list,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Access {
    root: PathBuf,
    checks: AtomicUsize,
    cancel_at: usize,
    enumerate: bool,
}
impl Access {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            checks: AtomicUsize::new(0),
            cancel_at: usize::MAX,
            enumerate: true,
        }
    }
}
impl FsAccessPolicy for Access {
    fn is_readable(&self, path: &Path) -> bool {
        path.starts_with(&self.root) && path != self.root.join("denied")
    }
    fn is_writable(&self, _: &Path) -> bool {
        false
    }
    fn can_enumerate_directory(&self, _: &Path, _: &Path) -> bool {
        self.enumerate
    }
    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.checks.fetch_add(1, Ordering::Relaxed) >= self.cancel_at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        Ok(())
    }
}
fn path(value: &str) -> FsPath {
    FsPath::new(value).unwrap()
}
fn page(root: &Path, limit: usize, after: Option<ListCursor>) -> ListResult {
    run_list(
        root,
        ListRequest {
            path: FsPath::root(),
            limit,
            after,
        },
        &Access::new(root),
    )
    .unwrap()
}
fn names(result: &ListResult) -> Vec<&str> {
    result
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect()
}

#[test]
fn list_pages_order_directories_first_then_paths_without_duplicates() {
    let root = tempfile::tempdir().unwrap();
    for name in ["z", "b", "a"] {
        std::fs::write(root.path().join(name), b"xx").unwrap();
    }
    for name in ["z-dir", "a-dir"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    let full = page(root.path(), usize::MAX, None);
    assert_eq!(names(&full), ["a-dir", "z-dir", "a", "b", "z"]);
    let mut after = None;
    let mut entries = Vec::new();
    loop {
        let result = page(root.path(), 2, after);
        assert!(result.entries.len() <= 2);
        assert_eq!(result.total_entries, 5);
        assert_eq!(result.total_bytes, full.total_bytes);
        assert_eq!(result.truncated, result.next_after.is_some());
        after = result.next_after;
        entries.extend(result.entries);
        if after.is_none() {
            break;
        }
    }
    assert_eq!(entries, full.entries);
    let zero = page(root.path(), 0, None);
    assert!(zero.entries.is_empty());
    assert!(zero.truncated);
    assert!(zero.next_after.is_none());
    assert_eq!(zero.total_entries, 5);
    let exhausted = page(
        root.path(),
        2,
        Some(ListCursor {
            kind: EntryKind::File,
            path: path("z"),
        }),
    );
    assert!(exhausted.entries.is_empty());
    assert!(!exhausted.truncated);
    assert!(exhausted.next_after.is_none());
    assert_eq!(exhausted.total_entries, 5);
}

#[test]
fn list_live_cursor_survives_removal_and_only_observes_new_keys_after_it() {
    let root = tempfile::tempdir().unwrap();
    for name in ["b", "d"] {
        std::fs::write(root.path().join(name), b"x").unwrap();
    }
    let first = page(root.path(), 1, None);
    assert_eq!(names(&first), ["b"]);
    std::fs::remove_file(root.path().join("b")).unwrap();
    for name in ["a", "c"] {
        std::fs::write(root.path().join(name), b"xx").unwrap();
    }
    // All directories sort before a file cursor, even when created later.
    std::fs::create_dir(root.path().join("z-directory")).unwrap();
    let second = page(root.path(), 10, first.next_after);
    assert_eq!(names(&second), ["c", "d"]);
    assert_eq!(second.total_entries, 4);
    assert!(!second.truncated);
    assert!(second.next_after.is_none());
    std::fs::remove_file(root.path().join("d")).unwrap();
    std::fs::rename(root.path().join("a"), root.path().join("e")).unwrap();
    let moved = page(
        root.path(),
        10,
        Some(ListCursor {
            kind: EntryKind::File,
            path: path("d"),
        }),
    );
    assert_eq!(names(&moved), ["e"]);
}

#[test]
fn list_path_byte_bound_returns_a_continuation_instead_of_losing_the_tail() {
    let root = tempfile::tempdir().unwrap();
    let long_count = MAX_RESULT_PATH_BYTES / (240 + 64) + 10;
    // A shorter late key must not leap over a longer evicted earlier key.
    let count = long_count + 50;
    let expected: Vec<_> = (0..count)
        .map(|n| format!("{n:05}{}", "x".repeat(if n < long_count { 235 } else { 1 })))
        .collect();
    for name in expected.iter().rev() {
        std::fs::write(root.path().join(name), b"x").unwrap();
    }
    let first = page(root.path(), usize::MAX, None);
    assert!(first.truncated);
    assert_eq!(first.total_entries, count);
    assert_eq!(first.total_bytes, count as u64);
    let path_bytes: usize = first
        .entries
        .iter()
        .map(|entry| entry.path.as_str().len() + 64)
        .sum::<usize>()
        + first.next_after.as_ref().unwrap().path.as_str().len()
        + 64;
    assert!(path_bytes <= MAX_RESULT_PATH_BYTES);
    let next = page(root.path(), usize::MAX, first.next_after.clone());
    assert!(!next.truncated);
    let got: Vec<_> = names(&first).into_iter().chain(names(&next)).collect();
    assert_eq!(got, expected.iter().map(String::as_str).collect::<Vec<_>>());
    let small = page(root.path(), 1, None);
    assert_eq!(names(&small), [expected[0].as_str()]);
    assert_eq!(small.total_entries, count);
}

#[test]
fn list_keeps_ignored_and_hidden_entries_but_respects_read_and_enumeration_authority() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".gitignore"), "ignored\n").unwrap();
    for name in ["ignored", ".hidden", "denied"] {
        std::fs::write(root.path().join(name), b"x").unwrap();
    }
    let first = page(root.path(), 2, None);
    assert_eq!(names(&first), [".gitignore", ".hidden"]);
    let second = page(root.path(), 2, first.next_after);
    assert_eq!(names(&second), ["ignored"]);
    assert_eq!(second.total_entries, 3);
    let mut access = Access::new(root.path());
    access.enumerate = false;
    let result = run_list(
        root.path(),
        ListRequest {
            path: FsPath::root(),
            limit: 1,
            after: None,
        },
        &access,
    )
    .unwrap();
    assert!(result.entries.is_empty());
    assert!(!result.truncated);
    assert!(result.next_after.is_none());
}

#[test]
fn list_cancellation_is_checked_even_after_the_page_is_full() {
    let root = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(root.path().join(name), b"x").unwrap();
    }
    let mut access = Access::new(root.path());
    access.cancel_at = 3;
    let error = run_list(
        root.path(),
        ListRequest {
            path: FsPath::root(),
            limit: 1,
            after: None,
        },
        &access,
    )
    .unwrap_err();
    assert!(
        matches!(error, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::Interrupted)
    );
}

#[test]
fn list_rejects_cursor_outside_the_requested_directory() {
    let root = tempfile::tempdir().unwrap();
    for value in ["", "other/a"] {
        let error = run_list(
            root.path(),
            ListRequest {
                path: FsPath::root(),
                limit: 1,
                after: Some(ListCursor {
                    kind: EntryKind::File,
                    path: path(value),
                }),
            },
            &Access::new(root.path()),
        )
        .unwrap_err();
        assert!(matches!(error, FsError::InvalidArgument(_)));
    }
}
