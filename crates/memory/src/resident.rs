//! Workspace memory resident-enumeration helpers.
//!
//! Surfaces used by the Worker system-prompt assembler:
//!
//! - [`collect_resident_summary`] — the body of
//!   `<workspace>/.yoi/memory/summary.md` when it parses as a summary
//!   record and has non-empty body.
//!
//! Files that fail to read or parse are skipped silently — the Linter
//! enforces shape on write, so a malformed file here means external
//! tampering and we'd rather degrade than panic.

use crate::schema::{SummaryFrontmatter, split_frontmatter};
use crate::workspace::WorkspaceLayout;

/// Read `<workspace>/.yoi/memory/summary.md` for resident prompt
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
        let path = dir.join(".yoi/memory/summary.md");
        let content = format!("---\nupdated_at: {n}\n---\n{body}", n = now());
        std::fs::write(path, content).unwrap();
    }

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".yoi/memory")).unwrap();
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
            dir.path().join(".yoi/memory/summary.md"),
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
}
