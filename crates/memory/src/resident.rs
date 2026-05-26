//! Workspace memory resident-enumeration helpers.
//!
//! Surfaces used by the Pod system-prompt assembler:
//!
//! - [`collect_resident_knowledge`] — resident-injection candidates
//!   (`model_invokation: true`) returned as `(slug, description)` pairs.
//! - [`collect_resident_summary`] — the body of
//!   `<workspace>/.insomnia/memory/summary.md` when it parses as a summary
//!   record and has non-empty body.
//! - [`list_knowledge_slugs`] — every slug whose file parses, regardless
//!   of `model_invokation`. Used by the Pod IPC layer to answer TUI `#`
//!   completion (`model_invokation` is a resident-injection flag, not a
//!   user-visibility flag).
//!
//! Files that fail to read or parse are skipped silently — the Linter
//! enforces shape on write, so a malformed file here means external
//! tampering and we'd rather degrade than panic.

use crate::schema::{KnowledgeFrontmatter, SummaryFrontmatter, split_frontmatter};
use crate::workspace::WorkspaceLayout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentKnowledgeEntry {
    pub slug: String,
    pub description: String,
}

/// Walk `<workspace>/.insomnia/knowledge/*.md` and return entries whose
/// frontmatter has `model_invokation: true`, sorted by slug. A missing
/// directory yields an empty vec.
pub fn collect_resident_knowledge(layout: &WorkspaceLayout) -> Vec<ResidentKnowledgeEntry> {
    let mut out: Vec<ResidentKnowledgeEntry> = Vec::new();
    walk_knowledge(layout, |slug, fm| {
        if fm.model_invokation {
            out.push(ResidentKnowledgeEntry {
                slug,
                description: fm.description,
            });
        }
    });
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// Read `<workspace>/.insomnia/memory/summary.md` for resident prompt
/// injection. Returns only the markdown body (frontmatter stripped), and
/// degrades to `None` for missing, unreadable, malformed, or empty records.
pub fn collect_resident_summary(layout: &WorkspaceLayout) -> Option<String> {
    let raw = std::fs::read_to_string(layout.summary_path()).ok()?;
    let (yaml, body) = split_frontmatter(&raw).ok()?;
    let _fm: SummaryFrontmatter = serde_yaml::from_str(yaml).ok()?;
    let body = body.trim_matches(&['\n', '\r'][..]);
    if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Walk `<workspace>/knowledge/*.md` and return every slug whose
/// frontmatter parses, sorted ascending. Does not filter on
/// `model_invokation`. A missing `knowledge/` directory yields an empty
/// vec.
pub fn list_knowledge_slugs(layout: &WorkspaceLayout) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_knowledge(layout, |slug, _fm| out.push(slug));
    out.sort();
    out
}

fn walk_knowledge(layout: &WorkspaceLayout, mut visit: impl FnMut(String, KnowledgeFrontmatter)) {
    let dir = layout.knowledge_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let slug = match name.strip_suffix(".md") {
            Some(s) => s.to_string(),
            None => continue,
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (yaml, _body) = match split_frontmatter(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let fm: KnowledgeFrontmatter = match serde_yaml::from_str(yaml) {
            Ok(f) => f,
            Err(_) => continue,
        };
        visit(slug, fm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::Path;
    use tempfile::TempDir;

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn write_summary(dir: &Path, body: &str) {
        let path = dir.join(".insomnia/memory/summary.md");
        let content = format!("---\nupdated_at: {n}\n---\n{body}", n = now());
        std::fs::write(path, content).unwrap();
    }

    fn write_knowledge(
        dir: &Path,
        slug: &str,
        description: &str,
        model_invokation: bool,
        body: &str,
    ) {
        let path = dir.join(".insomnia/knowledge").join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nkind: policy\ndescription: \"{description}\"\nmodel_invokation: {flag}\nuser_invocable: true\nlast_sources: []\n---\n{body}",
            n = now(),
            flag = model_invokation,
        );
        std::fs::write(path, content).unwrap();
    }

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/knowledge")).unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/memory")).unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    #[test]
    fn missing_summary_returns_none() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        assert!(collect_resident_summary(&layout).is_none());
    }

    #[test]
    fn summary_returns_body_without_frontmatter() {
        let (dir, layout) = setup();
        write_summary(dir.path(), "remember this\n");

        let got = collect_resident_summary(&layout).unwrap();
        assert_eq!(got, "remember this");
        assert!(!got.contains("updated_at"));
        assert!(!got.contains("---"));
    }

    #[test]
    fn malformed_summary_returns_none() {
        let (dir, layout) = setup();
        std::fs::write(
            dir.path().join(".insomnia/memory/summary.md"),
            "---\nthis is not yaml: : :\n---\nbody\n",
        )
        .unwrap();

        assert!(collect_resident_summary(&layout).is_none());
    }

    #[test]
    fn empty_summary_body_returns_none() {
        let (dir, layout) = setup();
        write_summary(dir.path(), "   \n");
        assert!(collect_resident_summary(&layout).is_none());
    }

    #[test]
    fn missing_knowledge_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        // No knowledge/ directory at all.
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        assert!(collect_resident_knowledge(&layout).is_empty());
    }

    #[test]
    fn picks_only_model_invokation_true() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "alpha", "alpha desc", true, "body\n");
        write_knowledge(dir.path(), "beta", "beta desc", false, "body\n");
        write_knowledge(dir.path(), "gamma", "gamma desc", true, "body\n");

        let got = collect_resident_knowledge(&layout);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].slug, "alpha");
        assert_eq!(got[0].description, "alpha desc");
        assert_eq!(got[1].slug, "gamma");
        assert_eq!(got[1].description, "gamma desc");
    }

    #[test]
    fn entries_are_sorted_by_slug() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "zeta", "z", true, "");
        write_knowledge(dir.path(), "alpha", "a", true, "");
        write_knowledge(dir.path(), "mu", "m", true, "");

        let got = collect_resident_knowledge(&layout);
        let slugs: Vec<&str> = got.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn malformed_frontmatter_is_skipped() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "good", "ok", true, "");
        // Garbage in frontmatter — must be skipped, not panic.
        std::fs::write(
            dir.path().join(".insomnia/knowledge/bad.md"),
            "---\nthis is not yaml: : :\n---\nbody\n",
        )
        .unwrap();

        let got = collect_resident_knowledge(&layout);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "good");
    }

    #[test]
    fn non_md_files_ignored() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "good", "ok", true, "");
        std::fs::write(
            dir.path().join(".insomnia/knowledge/note.txt"),
            "not markdown\n",
        )
        .unwrap();

        let got = collect_resident_knowledge(&layout);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn list_slugs_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        assert!(list_knowledge_slugs(&layout).is_empty());
    }

    #[test]
    fn list_slugs_returns_all_regardless_of_model_invokation() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "alpha", "a", true, "");
        write_knowledge(dir.path(), "beta", "b", false, "");
        write_knowledge(dir.path(), "gamma", "g", true, "");

        let got = list_knowledge_slugs(&layout);
        assert_eq!(got, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn list_slugs_skips_malformed_and_non_md() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "good", "ok", true, "");
        std::fs::write(
            dir.path().join(".insomnia/knowledge/bad.md"),
            "---\nthis is not yaml: : :\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".insomnia/knowledge/note.txt"),
            "not markdown\n",
        )
        .unwrap();

        let got = list_knowledge_slugs(&layout);
        assert_eq!(got, vec!["good"]);
    }
}
