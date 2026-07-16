//! Workspace-level path layout for the memory subsystem.
//!
//! `WorkspaceLayout` carries the root used by the memory subsystem.
//! All yoi-managed memory content lives under the conventional
//! `<root>/.yoi/` subdirectory — alongside workspace project records
//! generated durable memory. The trees inside it:
//!
//! - `<root>/.yoi/memory/summary.md`
//! - `<root>/.yoi/memory/decisions/<slug>.md`
//! - `<root>/.yoi/memory/requests/<slug>.md`
//! - `<root>/.yoi/memory/_staging/<id>.json`
//! - `<root>/.yoi/memory/_logs/current.log` (append-only audit log)
//!
//! `memory/` is reserved for session-derived / generated state;
//! `memory.workspace_root` pins this root explicitly. Without an explicit
//! root, resolution searches upward from the Worker pwd for a `.yoi/memory`
//! marker; `.yoi` project records alone are not a memory marker.

use std::path::{Path, PathBuf};

use crate::Slug;
use crate::error::LintError;
#[cfg(test)]
use lint_common::RecordLintError;

const YOI_DIR: &str = ".yoi";
const MEMORY_DIR: &str = "memory";
const SUMMARY_FILE: &str = "summary.md";
const DECISIONS_DIR: &str = "decisions";
const REQUESTS_DIR: &str = "requests";
const STAGING_DIR: &str = "_staging";
const USAGE_DIR: &str = "_usage";
const LOGS_DIR: &str = "_logs";
const USAGE_EVENTS_FILE: &str = "events.jsonl";
const AUDIT_CURRENT_LOG_FILE: &str = "current.log";

/// What kind of record a path under the memory tree represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Summary,
    Decision,
    Request,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Decision => "decision",
            Self::Request => "request",
        }
    }
}

/// A path classified into a kind and (where applicable) a slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedPath {
    pub kind: RecordKind,
    pub slug: Option<Slug>,
}

/// Workspace-rooted layout. Cheap to clone.
#[derive(Debug, Clone)]
pub struct WorkspaceLayout {
    root: PathBuf,
}

impl WorkspaceLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a layout from a `MemoryConfig`.
    ///
    /// An explicit `memory.workspace_root` is honored exactly. Without an
    /// explicit root, resolution searches `default_root` and its ancestors for
    /// the nearest `.yoi/memory` directory. This keeps child worktrees that
    /// contain `.yoi` project records such as tickets from
    /// becoming independent memory roots merely because they contain `.yoi`.
    ///
    /// If no memory marker exists, this falls back to `default_root` because
    /// existing call sites require a concrete layout. That fallback is a
    /// no-marker compatibility path, not a `.yoi` marker interpretation; it
    /// must not be used as evidence that `.yoi` alone enables repo-local
    /// memory.
    pub fn resolve(cfg: &manifest::MemoryConfig, default_root: &Path) -> Self {
        if let Some(root) = &cfg.workspace_root {
            return Self::new(root.clone());
        }

        let root =
            find_memory_marker_root(default_root).unwrap_or_else(|| default_root.to_path_buf());
        Self::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.yoi/`. The base of every other memory path.
    pub fn yoi_dir(&self) -> PathBuf {
        self.root.join(YOI_DIR)
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.yoi_dir().join(MEMORY_DIR)
    }

    pub fn summary_path(&self) -> PathBuf {
        self.memory_dir().join(SUMMARY_FILE)
    }

    pub fn decisions_dir(&self) -> PathBuf {
        self.memory_dir().join(DECISIONS_DIR)
    }

    pub fn requests_dir(&self) -> PathBuf {
        self.memory_dir().join(REQUESTS_DIR)
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.memory_dir().join(STAGING_DIR)
    }

    pub fn usage_dir(&self) -> PathBuf {
        self.memory_dir().join(USAGE_DIR)
    }

    pub fn usage_events_path(&self) -> PathBuf {
        self.usage_dir().join(USAGE_EVENTS_FILE)
    }

    pub fn audit_logs_dir(&self) -> PathBuf {
        self.memory_dir().join(LOGS_DIR)
    }

    /// Tail-friendly latest memory audit log path.
    ///
    /// Operators can inspect live memory worker and tool events with:
    /// `tail -f .yoi/memory/_logs/current.log`.
    pub fn audit_current_log_path(&self) -> PathBuf {
        self.audit_logs_dir().join(AUDIT_CURRENT_LOG_FILE)
    }

    pub fn decision_path(&self, slug: &Slug) -> PathBuf {
        self.decisions_dir().join(format!("{slug}.md"))
    }

    pub fn request_path(&self, slug: &Slug) -> PathBuf {
        self.requests_dir().join(format!("{slug}.md"))
    }

    /// Classify a path under the memory tree. Returns `None` if the
    /// path is not under `.yoi/memory/`
    /// of this workspace, or if it lives in
    /// `_staging/` / `_usage/` / `_logs/` (opaque subsystem-owned trees).
    ///
    /// On a conventional path that's *almost* a record but malformed
    /// (e.g. `.yoi/memory/decisions/Foo.md` with an invalid slug),
    /// returns `Err(LintError::Record(InvalidSlug) | InvalidPath)` so the caller
    /// can surface it as a write violation.
    pub fn classify(&self, path: &Path) -> Result<Option<ClassifiedPath>, LintError> {
        let memory = self.memory_dir();

        let rel = match path.strip_prefix(&memory) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        let mut comps = rel.components();
        let first = match comps.next() {
            Some(c) => c.as_os_str(),
            None => return Err(LintError::InvalidPath(path.to_path_buf())),
        };

        if first == SUMMARY_FILE {
            if comps.next().is_some() {
                return Err(LintError::InvalidPath(path.to_path_buf()));
            }
            return Ok(Some(ClassifiedPath {
                kind: RecordKind::Summary,
                slug: None,
            }));
        }
        if first == STAGING_DIR || first == USAGE_DIR || first == LOGS_DIR {
            // Linter opts out of subsystem-owned opaque trees.
            return Ok(None);
        }

        let kind = if first == DECISIONS_DIR {
            RecordKind::Decision
        } else if first == REQUESTS_DIR {
            RecordKind::Request
        } else {
            return Err(LintError::InvalidPath(path.to_path_buf()));
        };

        let rest: PathBuf = comps.collect();
        let cp = classify_kinded_md(&rest, kind, path)?;
        Ok(Some(cp))
    }
}

fn find_memory_marker_root(default_root: &Path) -> Option<PathBuf> {
    default_root
        .ancestors()
        .find(|ancestor| ancestor.join(YOI_DIR).join(MEMORY_DIR).is_dir())
        .map(Path::to_path_buf)
}

fn classify_kinded_md(
    rel: &Path,
    kind: RecordKind,
    full_path: &Path,
) -> Result<ClassifiedPath, LintError> {
    let mut comps = rel.components();
    let first = match comps.next() {
        Some(c) => c,
        None => return Err(LintError::InvalidPath(full_path.to_path_buf())),
    };
    if comps.next().is_some() {
        // Subdirectories under the record kind aren't allowed.
        return Err(LintError::InvalidPath(full_path.to_path_buf()));
    }
    let name = first.as_os_str();
    let s = name
        .to_str()
        .ok_or_else(|| LintError::InvalidPath(full_path.to_path_buf()))?;
    let stem = s
        .strip_suffix(".md")
        .ok_or_else(|| LintError::InvalidPath(full_path.to_path_buf()))?;
    let slug = Slug::parse(stem)?;
    Ok(ClassifiedPath {
        kind,
        slug: Some(slug),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn layout() -> WorkspaceLayout {
        WorkspaceLayout::new(PathBuf::from("/ws"))
    }

    #[test]
    fn classifies_summary() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/summary.md"))
            .unwrap()
            .unwrap();
        assert_eq!(cp.kind, RecordKind::Summary);
        assert!(cp.slug.is_none());
    }

    #[test]
    fn classifies_decision_with_slug() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/decisions/foo-bar.md"))
            .unwrap()
            .unwrap();
        assert_eq!(cp.kind, RecordKind::Decision);
        assert_eq!(cp.slug.unwrap().as_str(), "foo-bar");
    }

    #[test]
    fn staging_tree_is_opaque_to_classifier() {
        assert!(
            layout()
                .classify(&PathBuf::from("/ws/.yoi/memory/_staging/abc.json"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn usage_tree_is_opaque_to_classifier() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/_usage/events.jsonl"))
            .unwrap();
        assert!(cp.is_none());
    }

    #[test]
    fn logs_tree_is_opaque_to_classifier() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/_logs/current.log"))
            .unwrap();
        assert!(cp.is_none());
    }

    #[test]
    fn outside_returns_none() {
        assert!(
            layout()
                .classify(&PathBuf::from("/elsewhere/file.md"))
                .unwrap()
                .is_none()
        );
        assert!(
            layout()
                .classify(&PathBuf::from("/ws/src/main.rs"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalid_slug_rejected() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/decisions/Foo.md"))
            .unwrap_err();
        assert!(matches!(
            err,
            LintError::Record(RecordLintError::InvalidSlug(_))
        ));
    }

    #[test]
    fn nested_under_record_dir_rejected() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/decisions/sub/foo.md"))
            .unwrap_err();
        assert!(matches!(err, LintError::InvalidPath(_)));
    }

    #[test]
    fn unknown_top_level_dir_rejected() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.yoi/memory/something/foo.md"))
            .unwrap_err();
        assert!(matches!(err, LintError::InvalidPath(_)));
    }

    #[test]
    fn resolve_uses_workspace_root_when_set() {
        let cfg = manifest::MemoryConfig {
            workspace_root: Some(PathBuf::from("/explicit")),
            ..Default::default()
        };
        let layout = WorkspaceLayout::resolve(&cfg, Path::new("/fallback"));
        assert_eq!(layout.root(), Path::new("/explicit"));
    }

    #[test]
    fn resolve_selects_nearest_ancestor_memory_marker_when_workspace_root_missing() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let child = workspace.join(".worktree/child");
        std::fs::create_dir_all(workspace.join(".yoi/memory")).unwrap();
        std::fs::create_dir_all(&child).unwrap();

        let cfg = manifest::MemoryConfig::default();
        let layout = WorkspaceLayout::resolve(&cfg, &child);
        assert_eq!(layout.root(), workspace.as_path());
    }

    #[test]
    fn resolve_ignores_child_project_records_without_memory_marker() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let child = workspace.join(".worktree/child");
        std::fs::create_dir_all(workspace.join(".yoi/memory")).unwrap();
        std::fs::create_dir_all(child.join(".yoi/tickets")).unwrap();

        let cfg = manifest::MemoryConfig::default();
        let layout = WorkspaceLayout::resolve(&cfg, &child);
        assert_eq!(layout.root(), workspace.as_path());
    }

    #[test]
    fn yoi_project_records_alone_do_not_define_memory_marker_root() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let child = workspace.join("child");
        std::fs::create_dir_all(workspace.join(".yoi/tickets")).unwrap();
        std::fs::create_dir_all(&child).unwrap();

        assert_eq!(find_memory_marker_root(&child), None);

        let cfg = manifest::MemoryConfig::default();
        let layout = WorkspaceLayout::resolve(&cfg, &child);
        assert_eq!(layout.root(), child.as_path());
    }
}
