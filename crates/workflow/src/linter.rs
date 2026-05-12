//! Human-edit linter for Workflow files.

use std::collections::HashSet;

use memory::WorkspaceLayout;

use crate::{Slug, WorkflowLintError};
use lint_common::RecordLintError;
use serde::de::DeserializeOwned;

use crate::schema::{WORKFLOW_BODY_LIMIT, WorkflowFrontmatter, split_frontmatter};
use crate::workflow::WORKFLOW_DESCRIPTION_HARD_CAP;

#[derive(Debug, Default, Clone)]
pub struct WorkflowLintReport {
    pub errors: Vec<WorkflowLintError>,
}

impl WorkflowLintReport {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn push_error(&mut self, err: WorkflowLintError) {
        self.errors.push(err);
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowLinter {
    layout: WorkspaceLayout,
}

impl WorkflowLinter {
    pub fn new(layout: WorkspaceLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &WorkspaceLayout {
        &self.layout
    }

    /// Validate a human-authored Workflow document.
    ///
    /// Verifies frontmatter shape, body size, resident description size, and
    /// that every `requires` slug points at an existing Knowledge record.
    pub fn lint(&self, content: &str) -> WorkflowLintReport {
        let mut report = WorkflowLintReport::default();
        let parsed = match parse_frontmatter::<WorkflowFrontmatter>(content) {
            Ok(parsed) => parsed,
            Err(err) => {
                report.push_error(err);
                return report;
            }
        };

        let body_chars = parsed.body.chars().count();
        if body_chars > WORKFLOW_BODY_LIMIT {
            report.push_error(WorkflowLintError::BodyTooLong {
                actual: body_chars,
                limit: WORKFLOW_BODY_LIMIT,
            });
        }

        if parsed.frontmatter.model_invokation {
            let actual = parsed.frontmatter.description.chars().count();
            if actual > WORKFLOW_DESCRIPTION_HARD_CAP {
                report.push_error(WorkflowLintError::DescriptionTooLong {
                    actual,
                    limit: WORKFLOW_DESCRIPTION_HARD_CAP,
                });
            }
        }

        let knowledge = match scan_knowledge_slugs(&self.layout) {
            Ok(knowledge) => knowledge,
            Err(err) => {
                report.push_error(WorkflowLintError::Record(
                    RecordLintError::MalformedFrontmatter(format!(
                        "failed to scan existing Knowledge records: {err}"
                    )),
                ));
                return report;
            }
        };

        for slug in &parsed.frontmatter.requires {
            if !knowledge.contains(slug) {
                report.push_error(WorkflowLintError::UnknownReference {
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

fn parse_frontmatter<F: DeserializeOwned>(
    content: &str,
) -> Result<Parsed<'_, F>, WorkflowLintError> {
    let (yaml, body) = split_frontmatter(content)?;
    let frontmatter = serde_yaml::from_str::<F>(yaml).map_err(|err| {
        let msg = err.to_string();
        if let Some(field) = parse_missing_field(&msg) {
            WorkflowLintError::MissingField(field)
        } else {
            WorkflowLintError::Record(RecordLintError::MalformedFrontmatter(msg))
        }
    })?;
    Ok(Parsed { frontmatter, body })
}

fn parse_missing_field(msg: &str) -> Option<&'static str> {
    let needle = "missing field `";
    let start = msg.find(needle)? + needle.len();
    let end = msg[start..].find('`')? + start;
    match &msg[start..end] {
        "description" => Some("description"),
        "model_invokation" => Some("model_invokation"),
        "user_invocable" => Some("user_invocable"),
        "requires" => Some("requires"),
        _ => None,
    }
}

fn scan_knowledge_slugs(layout: &WorkspaceLayout) -> std::io::Result<HashSet<Slug>> {
    let mut out = HashSet::new();
    let entries = match std::fs::read_dir(layout.knowledge_dir()) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(slug) = Slug::parse(stem) {
            out.insert(slug);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn workspace() -> (TempDir, WorkflowLinter) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, WorkflowLinter::new(layout))
    }

    #[test]
    fn workflow_lint_accepts_valid_file() {
        let (dir, linter) = workspace();
        write(
            &dir.path().join(".insomnia/knowledge/policy.md"),
            "---\ndescription: p\n---\nbody",
        );
        let wf = "---\ndescription: run\nrequires: [policy]\n---\nbody";
        let report = linter.lint(wf);
        assert!(!report.has_errors(), "{:?}", report.errors);
    }

    #[test]
    fn workflow_lint_rejects_missing_required_knowledge() {
        let (_dir, linter) = workspace();
        let wf = "---\ndescription: run\nrequires: [ghost]\n---\nbody";
        let report = linter.lint(wf);
        assert!(report.errors.iter().any(|err| matches!(
            err,
            WorkflowLintError::UnknownReference { field: "requires", kind: "knowledge", slug }
                if slug == "ghost"
        )));
    }

    #[test]
    fn workflow_lint_enforces_resident_description_cap() {
        let (_dir, linter) = workspace();
        let desc = "x".repeat(WORKFLOW_DESCRIPTION_HARD_CAP + 1);
        let wf = format!("---\ndescription: {desc}\nmodel_invokation: true\n---\nbody");
        let report = linter.lint(&wf);
        assert!(
            report
                .errors
                .iter()
                .any(|err| matches!(err, WorkflowLintError::DescriptionTooLong { .. }))
        );
    }

    #[test]
    fn workflow_lint_enforces_body_limit() {
        let (_dir, linter) = workspace();
        let body = "x".repeat(WORKFLOW_BODY_LIMIT + 1);
        let wf = format!("---\ndescription: run\n---\n{body}");
        let report = linter.lint(&wf);
        assert!(
            report
                .errors
                .iter()
                .any(|err| matches!(err, WorkflowLintError::BodyTooLong { .. }))
        );
    }
}
