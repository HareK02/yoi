//! Walks `<workspace>/memory/{decisions,requests}/`,
//! `<workspace>/workflow/`, and `<workspace>/knowledge/` to collect
//! the slug set the linter needs for reference-integrity and
//! same-slug-duplication checks.
//!
//! No caching: each lint call walks fresh. Tree size is expected to
//! stay small (hundreds of files, not thousands).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use crate::schema::{
    DecisionFrontmatter, KnowledgeFrontmatter, RequestFrontmatter, WorkflowFrontmatter,
    split_frontmatter,
};
use crate::slug::Slug;
use crate::workspace::{RecordKind, WorkspaceLayout};

/// Snapshot of every record currently on disk under the workspace.
///
/// Carries enough metadata to answer:
/// - "does slug X of kind K exist?" (same-slug duplication, reference checks)
/// - "what is X's `replaced_by`?" (cycle detection)
/// - "what other slugs of kind K exist?" (similar-slug warning)
#[derive(Debug, Default, Clone)]
pub struct ExistingRecords {
    decisions: HashMap<Slug, DecisionMeta>,
    requests: HashSet<Slug>,
    knowledge: HashSet<Slug>,
    workflow: HashSet<Slug>,
}

#[derive(Debug, Clone)]
pub struct DecisionMeta {
    pub replaced_by: Option<Slug>,
}

impl ExistingRecords {
    pub fn contains(&self, kind: RecordKind, slug: &Slug) -> bool {
        match kind {
            RecordKind::Decision => self.decisions.contains_key(slug),
            RecordKind::Request => self.requests.contains(slug),
            RecordKind::Knowledge => self.knowledge.contains(slug),
            RecordKind::Workflow => self.workflow.contains(slug),
            RecordKind::Summary => false,
        }
    }

    pub fn decision(&self, slug: &Slug) -> Option<&DecisionMeta> {
        self.decisions.get(slug)
    }

    pub fn slugs(&self, kind: RecordKind) -> Vec<&Slug> {
        match kind {
            RecordKind::Decision => self.decisions.keys().collect(),
            RecordKind::Request => self.requests.iter().collect(),
            RecordKind::Knowledge => self.knowledge.iter().collect(),
            RecordKind::Workflow => self.workflow.iter().collect(),
            RecordKind::Summary => Vec::new(),
        }
    }
}

/// Walk the workspace and collect every record.
pub fn scan_existing(layout: &WorkspaceLayout) -> io::Result<ExistingRecords> {
    let mut out = ExistingRecords::default();

    scan_dir(&layout.decisions_dir(), |path, slug| {
        let meta = read_decision_meta(path);
        out.decisions.insert(slug, meta);
    })?;
    scan_dir(&layout.requests_dir(), |path, slug| {
        // Parse to validate but discard contents — only slug existence
        // matters for reference checks. Parse failure is silently
        // ignored: existing record corruption isn't this write's
        // responsibility to fix.
        let _ = parse_silent::<RequestFrontmatter>(path);
        out.requests.insert(slug);
    })?;
    scan_dir(&layout.knowledge_dir(), |path, slug| {
        let _ = parse_silent::<KnowledgeFrontmatter>(path);
        out.knowledge.insert(slug);
    })?;
    scan_dir(&layout.workflow_dir(), |path, slug| {
        let _ = parse_silent::<WorkflowFrontmatter>(path);
        out.workflow.insert(slug);
    })?;

    Ok(out)
}

fn scan_dir<F>(dir: &Path, mut visit: F) -> io::Result<()>
where
    F: FnMut(&Path, Slug),
{
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "md" {
            continue;
        }
        if let Ok(slug) = Slug::parse(stem) {
            visit(&path, slug);
        }
    }
    Ok(())
}

fn read_decision_meta(path: &Path) -> DecisionMeta {
    match parse_silent::<DecisionFrontmatter>(path) {
        Some(fm) => DecisionMeta {
            replaced_by: fm.replaced_by,
        },
        None => DecisionMeta { replaced_by: None },
    }
}

fn parse_silent<F>(path: &Path) -> Option<F>
where
    F: serde::de::DeserializeOwned,
{
    let content = std::fs::read_to_string(path).ok()?;
    let (yaml, _) = split_frontmatter(&content).ok()?;
    serde_yaml::from_str::<F>(yaml).ok()
}
