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
