//! Pre-write Linter for the memory subsystem.
//!
//! The linter is pure: given a [`WorkspaceLayout`], a target path, and
//! the proposed file content (raw bytes), it returns a [`LintReport`]
//! aggregating every applicable rule violation. The memory tool calls
//! this *before* committing to fs and surfaces a non-empty `errors`
//! collection back to the LLM as `ToolError::InvalidArgument`.
//!
//! Reference-integrity checks (`replaced_by` / `requires` existence,
//! cycle detection) walk the whole `memory/` and `knowledge/` trees
//! each call. No caching; the trees are expected to be small.

mod existing;
mod frontmatter;
mod references;
mod size;
mod warnings;

use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::{LintError, LintWarning};
use crate::schema::{
    DecisionFrontmatter, KnowledgeFrontmatter, RequestFrontmatter, SummaryFrontmatter,
    WorkflowFrontmatter, split_frontmatter,
};
use crate::workspace::{ClassifiedPath, RecordKind, WorkspaceLayout};

pub use existing::{ExistingRecords, scan_existing};

/// Aggregated linter result. `errors` empty ⇒ write proceeds.
#[derive(Debug, Default, Clone)]
pub struct LintReport {
    pub errors: Vec<LintError>,
    pub warnings: Vec<LintWarning>,
}

impl LintReport {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn extend_errors(&mut self, more: impl IntoIterator<Item = LintError>) {
        self.errors.extend(more);
    }

    pub fn push_error(&mut self, err: LintError) {
        self.errors.push(err);
    }

    pub fn push_warning(&mut self, w: LintWarning) {
        self.warnings.push(w);
    }
}

/// Operation context: is this a brand-new file or an update of an
/// existing one? Affects same-slug duplication check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Create,
    Update,
}

/// Stateless entry point holding the workspace layout. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Linter {
    layout: WorkspaceLayout,
}

impl Linter {
    pub fn new(layout: WorkspaceLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &WorkspaceLayout {
        &self.layout
    }

    /// Lint a proposed write to `path` with the given UTF-8 `content`.
    ///
    /// `mode` tells the linter whether the path already existed at the
    /// moment of write — Create triggers same-slug duplication checks,
    /// Update doesn't.
    pub fn lint(&self, path: &Path, content: &str, mode: WriteMode) -> LintReport {
        let mut report = LintReport::default();

        // 1. Path classification.
        let classified = match self.layout.classify(path) {
            Ok(Some(cp)) => cp,
            Ok(None) => {
                report.push_error(LintError::InvalidPath(path.to_path_buf()));
                return report;
            }
            Err(e) => {
                report.push_error(e);
                return report;
            }
        };

        // 2. Workflow paths are sub-Worker-forbidden at the tool layer.
        if classified.kind == RecordKind::Workflow {
            report.push_error(LintError::WorkflowWriteForbidden);
            return report;
        }

        // 3. Frontmatter parse + kind-specific structural checks +
        //    size limits. Reference-integrity needs the existing
        //    record set, fetched once below.
        let existing = match existing::scan_existing(&self.layout) {
            Ok(e) => e,
            Err(e) => {
                report.push_error(LintError::MalformedFrontmatter(format!(
                    "failed to scan existing records: {e}"
                )));
                return report;
            }
        };

        // Same-slug check on Create.
        if mode == WriteMode::Create {
            if let Some(slug) = &classified.slug {
                if existing.contains(classified.kind, slug) {
                    report.push_error(LintError::SlugAlreadyExists(slug.to_string()));
                }
            }
        }

        // Similar-slug clustering warning. Skipped for Summary (no slug).
        if let Some(slug) = &classified.slug {
            warnings::check_similar_slugs(slug, classified.kind, &existing, &mut report);
        }

        // Frontmatter parse dispatch by kind.
        match classified.kind {
            RecordKind::Decision => {
                self.check_decision(content, &classified, &existing, &mut report);
            }
            RecordKind::Request => {
                self.check_request(content, &classified, &mut report);
            }
            RecordKind::Knowledge => {
                self.check_knowledge(content, &classified, &mut report);
            }
            RecordKind::Summary => {
                self.check_kind::<SummaryFrontmatter>(content, &classified, &mut report);
            }
            RecordKind::Workflow => unreachable!("guarded above"),
        }

        report
    }

    fn check_kind<F>(&self, content: &str, cp: &ClassifiedPath, report: &mut LintReport)
    where
        F: DeserializeOwned + crate::schema::Frontmatter,
    {
        let parsed = match parse_frontmatter::<F>(content) {
            Ok(p) => p,
            Err(e) => {
                report.push_error(e);
                return;
            }
        };
        let body = parsed.body;
        size::check_body::<F>(body, report);
        warnings::check_warnings_kindless(cp, body, report);
        let _ = parsed.frontmatter; // discarded after structural checks
    }

    fn check_request(&self, content: &str, _cp: &ClassifiedPath, report: &mut LintReport) {
        let parsed = match parse_frontmatter::<RequestFrontmatter>(content) {
            Ok(p) => p,
            Err(e) => {
                report.push_error(e);
                return;
            }
        };
        size::check_body::<RequestFrontmatter>(parsed.body, report);
        warnings::check_warnings_with_sources(
            parsed.body,
            parsed.frontmatter.sources.len(),
            report,
        );
    }

    fn check_decision(
        &self,
        content: &str,
        cp: &ClassifiedPath,
        existing: &ExistingRecords,
        report: &mut LintReport,
    ) {
        let parsed = match parse_frontmatter::<DecisionFrontmatter>(content) {
            Ok(p) => p,
            Err(e) => {
                report.push_error(e);
                return;
            }
        };
        let fm = parsed.frontmatter;
        size::check_body::<DecisionFrontmatter>(parsed.body, report);

        // replaced_by structural rules.
        if let Some(target) = &fm.replaced_by {
            if let Some(self_slug) = &cp.slug {
                if target == self_slug {
                    report.push_error(LintError::ReplacedBySelf);
                }
            }
            references::check_replaced_by(cp.slug.as_ref(), target, existing, report);
        }

        warnings::check_warnings_with_sources(parsed.body, fm.sources.len(), report);
    }

    fn check_knowledge(&self, content: &str, cp: &ClassifiedPath, report: &mut LintReport) {
        let parsed = match parse_frontmatter::<KnowledgeFrontmatter>(content) {
            Ok(p) => p,
            Err(e) => {
                report.push_error(e);
                return;
            }
        };
        let fm = parsed.frontmatter;
        size::check_body::<KnowledgeFrontmatter>(parsed.body, report);

        if fm.model_invokation
            && fm.description.chars().count() > crate::schema::KNOWLEDGE_DESCRIPTION_HARD_CAP
        {
            report.push_error(LintError::DescriptionTooLong {
                actual: fm.description.chars().count(),
                limit: crate::schema::KNOWLEDGE_DESCRIPTION_HARD_CAP,
            });
        }

        warnings::check_warnings_with_sources(parsed.body, fm.last_sources.len(), report);
        let _ = cp;
    }
}

impl Linter {
    /// Workflow record validator exposed for human-edit paths
    /// (CLI / pre-commit). Not used by the memory tool, which rejects
    /// workflow writes outright.
    ///
    /// Verifies frontmatter shape, body size, and that every slug in
    /// `requires` points at an existing Knowledge record under the
    /// workspace's `knowledge/` directory.
    pub fn lint_workflow(&self, content: &str) -> LintReport {
        let mut report = LintReport::default();
        let parsed = match parse_frontmatter::<WorkflowFrontmatter>(content) {
            Ok(p) => p,
            Err(e) => {
                report.push_error(e);
                return report;
            }
        };
        size::check_body::<WorkflowFrontmatter>(parsed.body, &mut report);

        let existing = match existing::scan_existing(&self.layout) {
            Ok(e) => e,
            Err(e) => {
                report.push_error(LintError::MalformedFrontmatter(format!(
                    "failed to scan existing records: {e}"
                )));
                return report;
            }
        };
        for slug in &parsed.frontmatter.requires {
            if !existing.contains(crate::workspace::RecordKind::Knowledge, slug) {
                report.push_error(LintError::UnknownReference {
                    field: "requires",
                    kind: "knowledge",
                    slug: slug.to_string(),
                });
            }
        }
        report
    }
}

struct Parsed<'a, F> {
    frontmatter: F,
    body: &'a str,
}

fn parse_frontmatter<F: DeserializeOwned>(content: &str) -> Result<Parsed<'_, F>, LintError> {
    let (yaml, body) = split_frontmatter(content)?;
    let fm = frontmatter::deserialize_strict::<F>(yaml)?;
    Ok(Parsed {
        frontmatter: fm,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write(p: &std::path::Path, content: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    fn iso_now() -> String {
        Utc::now().to_rfc3339()
    }

    fn workspace() -> (TempDir, Linter) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let linter = Linter::new(layout);
        (dir, linter)
    }

    #[test]
    fn workflow_write_rejected() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/workflow/wf.md");
        let content = format!(
            "---\nupdated_at: {now}\ndescription: x\nauto_invoke: false\nuser_invocable: true\n---\nbody",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::WorkflowWriteForbidden))
        );
    }

    #[test]
    fn outside_memory_tree_rejected() {
        let (dir, linter) = workspace();
        let path = dir.path().join("src/main.rs");
        let report = linter.lint(&path, "ignored", WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::InvalidPath(_)))
        );
    }

    #[test]
    fn decision_with_unknown_replaced_by_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: replaced\nreplaced_by: ghost\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::UnknownReference { .. }))
        );
    }

    #[test]
    fn decision_replaced_by_self_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: replaced\nreplaced_by: foo\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Update);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::ReplacedBySelf))
        );
    }

    #[test]
    fn decision_replaced_by_existing_ok() {
        let (dir, linter) = workspace();
        // Pre-create the target.
        let target = dir.path().join(".insomnia/memory/decisions/bar.md");
        write(
            &target,
            &format!(
                "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: open\n---\nbar body\n",
                now = iso_now()
            ),
        );
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: replaced\nreplaced_by: bar\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(!report.has_errors(), "got errors: {:?}", report.errors);
    }

    #[test]
    fn missing_required_field_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        // Missing `status`.
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(report.errors.iter().any(|e| matches!(
            e,
            LintError::MissingField(_) | LintError::MalformedFrontmatter(_)
        )));
    }

    #[test]
    fn knowledge_long_description_with_model_invokation_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/knowledge/foo.md");
        let big_desc = "x".repeat(2000);
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nkind: rule\ndescription: {big_desc}\nmodel_invokation: true\nuser_invocable: true\nlast_sources: []\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::DescriptionTooLong { .. }))
        );
    }

    #[test]
    fn knowledge_long_description_without_model_invokation_ok() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/knowledge/foo.md");
        let big_desc = "x".repeat(2000);
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nkind: rule\ndescription: {big_desc}\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\nbody\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(!report.has_errors(), "got errors: {:?}", report.errors);
    }

    #[test]
    fn summary_path_accepted() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/summary.md");
        let content = format!(
            "---\nupdated_at: {now}\n---\nsummary body\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Update);
        assert!(!report.has_errors(), "got errors: {:?}", report.errors);
    }

    #[test]
    fn create_when_existing_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        write(
            &path,
            &format!(
                "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: open\n---\nold\n",
                now = iso_now()
            ),
        );
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: open\n---\nnew\n",
            now = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::SlugAlreadyExists(_)))
        );
    }

    #[test]
    fn workflow_lint_accepts_valid_record() {
        let (dir, linter) = workspace();
        // Place a Knowledge record that the workflow will reference.
        let kn = dir.path().join(".insomnia/knowledge/foo.md");
        write(
            &kn,
            &format!(
                "---\ncreated_at: {n}\nupdated_at: {n}\nkind: rule\ndescription: x\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\n",
                n = iso_now()
            ),
        );
        let wf = format!(
            "---\nupdated_at: {n}\ndescription: do thing\nauto_invoke: false\nuser_invocable: true\nrequires: [foo]\n---\nstep 1\n",
            n = iso_now()
        );
        let report = linter.lint_workflow(&wf);
        assert!(!report.has_errors(), "got errors: {:?}", report.errors);
    }

    #[test]
    fn workflow_lint_flags_unknown_requires() {
        let (_dir, linter) = workspace();
        let wf = format!(
            "---\nupdated_at: {n}\ndescription: x\nauto_invoke: false\nuser_invocable: true\nrequires: [missing-knowledge]\n---\n",
            n = iso_now()
        );
        let report = linter.lint_workflow(&wf);
        assert!(report.errors.iter().any(|e| matches!(
            e,
            LintError::UnknownReference {
                field: "requires",
                kind: "knowledge",
                ..
            }
        )));
    }

    #[test]
    fn workflow_lint_collects_multiple_unknown_requires() {
        let (_dir, linter) = workspace();
        let wf = format!(
            "---\nupdated_at: {n}\ndescription: x\nauto_invoke: false\nuser_invocable: true\nrequires: [a, b, c]\n---\n",
            n = iso_now()
        );
        let report = linter.lint_workflow(&wf);
        let unknown_count = report
            .errors
            .iter()
            .filter(|e| matches!(e, LintError::UnknownReference { .. }))
            .count();
        assert_eq!(unknown_count, 3);
    }

    #[test]
    fn similar_slugs_warns_on_cluster() {
        let (dir, linter) = workspace();
        // Two existing decisions within Levenshtein 2 of `db-pool`:
        //   `db-pol` (1 deletion), `db-pools` (1 insertion).
        for slug in ["db-pol", "db-pools"] {
            write(
                &dir.path()
                    .join(format!(".insomnia/memory/decisions/{slug}.md")),
                &format!(
                    "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n",
                    n = iso_now()
                ),
            );
        }
        let path = dir.path().join(".insomnia/memory/decisions/db-pool.md");
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\nbody\n",
            n = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        let warned = report
            .warnings
            .iter()
            .any(|w| matches!(w, LintWarning::SimilarSlugs(slugs) if slugs.len() >= 3));
        assert!(
            warned,
            "expected SimilarSlugs warning, got {:?}",
            report.warnings
        );
    }

    #[test]
    fn similar_slugs_silent_when_distant() {
        let (dir, linter) = workspace();
        for slug in ["alpha", "bravo"] {
            write(
                &dir.path()
                    .join(format!(".insomnia/memory/decisions/{slug}.md")),
                &format!(
                    "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n",
                    n = iso_now()
                ),
            );
        }
        let path = dir.path().join(".insomnia/memory/decisions/charlie.md");
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n",
            n = iso_now()
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| matches!(w, LintWarning::SimilarSlugs(_))),
            "unexpected SimilarSlugs warning: {:?}",
            report.warnings
        );
    }

    #[test]
    fn body_size_limit_errors() {
        let (dir, linter) = workspace();
        let path = dir.path().join(".insomnia/memory/decisions/foo.md");
        let big_body = "x".repeat(8001);
        let content = format!(
            "---\ncreated_at: {now}\nupdated_at: {now}\nsources: []\nstatus: open\n---\n{body}",
            now = iso_now(),
            body = big_body
        );
        let report = linter.lint(&path, &content, WriteMode::Create);
        assert!(
            report
                .errors
                .iter()
                .any(|e| matches!(e, LintError::BodyTooLong { .. }))
        );
        // Sanity: ensure path was treated as PathBuf consistently.
        let _ = PathBuf::from(path);
    }
}
