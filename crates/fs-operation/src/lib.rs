//! Foundational filesystem operation contracts and provider-side search.
//!
//! This crate deliberately has no dependency on Workdir identity, Runtime
//! transport, or LLM Tool implementations. Paths are logical and root-relative;
//! providers supply host roots and access policy.

mod glob;
mod local;
mod operation;
mod search;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use glob::run_glob;
pub use local::{run_edit, run_list, run_read, run_stat, run_write};
pub use operation::*;
pub use search::run_grep;

/// Provider-owned access policy used by local filesystem operations.
pub trait FsAccessPolicy: Send + Sync {
    fn is_readable(&self, path: &Path) -> bool;
    fn is_writable(&self, path: &Path) -> bool;
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
    fn logical_paths_reject_absolute_parent_and_backslash_forms() {
        assert!(FsPath::new("src/lib.rs").is_ok());
        assert!(FsPath::new("/tmp/file").is_err());
        assert!(FsPath::new("../file").is_err());
        assert!(FsPath::new("src\\lib.rs").is_err());
    }

    #[test]
    fn deserialization_cannot_bypass_logical_path_validation() {
        assert!(serde_json::from_str::<FsPath>(r#""../secret""#).is_err());
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
    fn grep_keeps_direct_symlink_directory_and_broken_path_guards() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let readable = RootAccess(root.clone());
        std::fs::create_dir(root.join("target-dir")).unwrap();
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

        let directory_error = run_grep(
            &root,
            root.join("directory-link"),
            request("directory-link"),
            &readable,
        )
        .unwrap_err();
        assert!(matches!(
            directory_error,
            FsError::SymlinkDirectoryNotTraversed { tool: "Grep", path, .. }
                if path == root.join("directory-link")
        ));

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
