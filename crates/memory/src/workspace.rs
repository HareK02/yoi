//! Workspace-level path layout for the memory subsystem.
//!
//! `WorkspaceLayout` carries the workspace root (typically the Pod's
//! pwd). All insomnia-managed content lives under the conventional
//! `<root>/.insomnia/` subdirectory — the same place that holds
//! `manifest.toml` and `prompts/`. The trees inside it:
//!
//! - `<root>/.insomnia/workflow/<slug>.md`
//! - `<root>/.insomnia/knowledge/<slug>.md`
//! - `<root>/.insomnia/memory/summary.md`
//! - `<root>/.insomnia/memory/decisions/<slug>.md`
//! - `<root>/.insomnia/memory/requests/<slug>.md`
//! - `<root>/.insomnia/memory/_staging/<id>.json`
//!
//! `memory/` is reserved for session-derived / generated state;
//! Workflows are human-managed and live one level up under
//! `.insomnia/workflow/`.
//!
//! Configuring `[memory]` with an empty body is therefore sufficient
//! for any workspace that already uses the `.insomnia/` convention; no
//! `workspace_root` override is needed.

use std::path::{Path, PathBuf};

use crate::error::LintError;
use crate::slug::Slug;

const INSOMNIA_DIR: &str = ".insomnia";
const MEMORY_DIR: &str = "memory";
const KNOWLEDGE_DIR: &str = "knowledge";
const WORKFLOW_DIR: &str = "workflow";
const SUMMARY_FILE: &str = "summary.md";
const DECISIONS_DIR: &str = "decisions";
const REQUESTS_DIR: &str = "requests";
const STAGING_DIR: &str = "_staging";
const USAGE_DIR: &str = "_usage";
const USAGE_EVENTS_FILE: &str = "events.jsonl";

/// What kind of record a path under the memory tree represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Summary,
    Decision,
    Request,
    Workflow,
    Knowledge,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Decision => "decision",
            Self::Request => "request",
            Self::Workflow => "workflow",
            Self::Knowledge => "knowledge",
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

    /// Resolve a layout from a `MemoryConfig`, falling back to
    /// `default_root` (typically the Pod's pwd) when the manifest does
    /// not pin `workspace_root` explicitly. Single source of truth for
    /// the `workspace_root.unwrap_or(pwd)` convention used across the
    /// codebase (controller wiring, scope-deny build, system-prompt
    /// resident-injection).
    pub fn resolve(cfg: &manifest::MemoryConfig, default_root: &Path) -> Self {
        let root = cfg
            .workspace_root
            .clone()
            .unwrap_or_else(|| default_root.to_path_buf());
        Self::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.insomnia/`. The base of every other memory path.
    pub fn insomnia_dir(&self) -> PathBuf {
        self.root.join(INSOMNIA_DIR)
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.insomnia_dir().join(MEMORY_DIR)
    }

    pub fn knowledge_dir(&self) -> PathBuf {
        self.insomnia_dir().join(KNOWLEDGE_DIR)
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

    /// Workflow directory: `<root>/.insomnia/workflow/`.
    pub fn workflow_dir(&self) -> PathBuf {
        self.insomnia_dir().join(WORKFLOW_DIR)
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

    pub fn decision_path(&self, slug: &Slug) -> PathBuf {
        self.decisions_dir().join(format!("{slug}.md"))
    }

    pub fn request_path(&self, slug: &Slug) -> PathBuf {
        self.requests_dir().join(format!("{slug}.md"))
    }

    pub fn workflow_path(&self, slug: &Slug) -> PathBuf {
        self.workflow_dir().join(format!("{slug}.md"))
    }

    pub fn knowledge_path(&self, slug: &Slug) -> PathBuf {
        self.knowledge_dir().join(format!("{slug}.md"))
    }

    /// Classify a path under the memory tree. Returns `None` if the
    /// path is not under `.insomnia/memory/` or `.insomnia/knowledge/`
    /// of this workspace, or if it lives in
    /// `_staging/` / `_usage/` (opaque subsystem-owned trees).
    ///
    /// On a conventional path that's *almost* a record but malformed
    /// (e.g. `.insomnia/memory/decisions/Foo.md` with an invalid slug),
    /// returns `Err(LintError::InvalidSlug | InvalidPath)` so the caller
    /// can surface it as a write violation.
    pub fn classify(&self, path: &Path) -> Result<Option<ClassifiedPath>, LintError> {
        let memory = self.memory_dir();
        let knowledge = self.knowledge_dir();

        if let Ok(rel) = path.strip_prefix(&knowledge) {
            return Ok(Some(classify_kinded_md(rel, RecordKind::Knowledge, path)?));
        }
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
        if first == STAGING_DIR || first == USAGE_DIR {
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

    fn layout() -> WorkspaceLayout {
        WorkspaceLayout::new(PathBuf::from("/ws"))
    }

    #[test]
    fn classifies_summary() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/summary.md"))
            .unwrap()
            .unwrap();
        assert_eq!(cp.kind, RecordKind::Summary);
        assert!(cp.slug.is_none());
    }

    #[test]
    fn classifies_decision_with_slug() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/decisions/foo-bar.md"))
            .unwrap()
            .unwrap();
        assert_eq!(cp.kind, RecordKind::Decision);
        assert_eq!(cp.slug.unwrap().as_str(), "foo-bar");
    }

    #[test]
    fn classifies_knowledge() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.insomnia/knowledge/x.md"))
            .unwrap()
            .unwrap();
        assert_eq!(cp.kind, RecordKind::Knowledge);
    }

    #[test]
    fn workflow_under_memory_is_invalid_path() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/workflow/wf.md"))
            .unwrap_err();
        assert!(matches!(err, LintError::InvalidPath(_)));
    }

    #[test]
    fn staging_returns_none() {
        assert!(
            layout()
                .classify(&PathBuf::from("/ws/.insomnia/memory/_staging/abc.json"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn usage_tree_is_opaque_to_classifier() {
        let cp = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/_usage/events.jsonl"))
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
            .classify(&PathBuf::from("/ws/.insomnia/memory/decisions/Foo.md"))
            .unwrap_err();
        assert!(matches!(err, LintError::InvalidSlug(_)));
    }

    #[test]
    fn nested_under_record_dir_rejected() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/decisions/sub/foo.md"))
            .unwrap_err();
        assert!(matches!(err, LintError::InvalidPath(_)));
    }

    #[test]
    fn unknown_top_level_dir_rejected() {
        let err = layout()
            .classify(&PathBuf::from("/ws/.insomnia/memory/something/foo.md"))
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
    fn resolve_falls_back_to_default_when_workspace_root_missing() {
        let cfg = manifest::MemoryConfig::default();
        let layout = WorkspaceLayout::resolve(&cfg, Path::new("/fallback"));
        assert_eq!(layout.root(), Path::new("/fallback"));
    }
}
