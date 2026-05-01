//! Workflow loader and registry.
//!
//! Workflows live under `<workspace>/.insomnia/memory/workflow/<slug>.md`.
//! They are human-authored Markdown documents with YAML frontmatter. The loader
//! is intentionally strict about malformed records because Pod startup should
//! fail rather than silently ignoring a broken procedural instruction.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use crate::error::LintError;
use crate::schema::{WorkflowFrontmatter, split_frontmatter};
use crate::slug::Slug;
use crate::workspace::WorkspaceLayout;

/// Hard cap on Workflow descriptions that are advertised resident.
/// Mirrors agent-skills and resident Knowledge descriptions.
pub const WORKFLOW_DESCRIPTION_HARD_CAP: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRecord {
    pub slug: Slug,
    pub description: String,
    pub model_invokation: bool,
    pub user_invocable: bool,
    pub requires: Vec<Slug>,
    /// Markdown body after the closing frontmatter delimiter.
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowRegistry {
    records: BTreeMap<Slug, WorkflowRecord>,
}

impl WorkflowRegistry {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, slug: &Slug) -> Option<&WorkflowRecord> {
        self.records.get(slug)
    }

    pub fn iter(&self) -> impl Iterator<Item = &WorkflowRecord> {
        self.records.values()
    }

    pub fn resident_entries(&self) -> Vec<ResidentWorkflowEntry> {
        self.records
            .values()
            .filter(|record| record.model_invokation)
            .map(|record| ResidentWorkflowEntry {
                slug: record.slug.to_string(),
                description: record.description.clone(),
            })
            .collect()
    }

    pub fn list_user_invocable(&self, prefix: &str) -> Vec<String> {
        self.records
            .values()
            .filter(|record| record.user_invocable && record.slug.as_str().starts_with(prefix))
            .map(|record| record.slug.to_string())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentWorkflowEntry {
    pub slug: String,
    pub description: String,
}

#[derive(Debug, Error)]
pub enum WorkflowLoadError {
    #[error("failed to read workflow directory {}: {source}", .dir.display())]
    ReadDir { dir: PathBuf, source: io::Error },
    #[error("failed to read workflow file {}: {source}", .path.display())]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("invalid workflow file name {}: {source}", .path.display())]
    InvalidSlug { path: PathBuf, source: LintError },
    #[error("invalid workflow frontmatter in {}: {source}", .path.display())]
    Frontmatter { path: PathBuf, source: LintError },
    #[error(
        "Workflow {} with model_invokation: true cannot have description longer than {limit} chars (got {actual})",
        .path.display()
    )]
    DescriptionTooLong {
        path: PathBuf,
        actual: usize,
        limit: usize,
    },
}

pub fn load_workflows(layout: &WorkspaceLayout) -> Result<WorkflowRegistry, WorkflowLoadError> {
    let dir = layout.workflow_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkflowRegistry::empty());
        }
        Err(source) => return Err(WorkflowLoadError::ReadDir { dir, source }),
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WorkflowLoadError::ReadDir {
            dir: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut records = BTreeMap::new();
    for path in paths {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let slug =
            Slug::parse(stem.to_string()).map_err(|source| WorkflowLoadError::InvalidSlug {
                path: path.clone(),
                source,
            })?;
        if records.contains_key(&slug) {
            warn!(slug = %slug, path = %path.display(), "duplicate workflow slug encountered; keeping first record");
            continue;
        }
        let raw = std::fs::read_to_string(&path).map_err(|source| WorkflowLoadError::ReadFile {
            path: path.clone(),
            source,
        })?;
        let (yaml, body) =
            split_frontmatter(&raw).map_err(|source| WorkflowLoadError::Frontmatter {
                path: path.clone(),
                source,
            })?;
        warn_unknown_workflow_fields(&path, yaml);
        let frontmatter: WorkflowFrontmatter =
            serde_yaml::from_str(yaml).map_err(|err| WorkflowLoadError::Frontmatter {
                path: path.clone(),
                source: map_serde_workflow_error(err),
            })?;
        if frontmatter.model_invokation
            && frontmatter.description.chars().count() > WORKFLOW_DESCRIPTION_HARD_CAP
        {
            return Err(WorkflowLoadError::DescriptionTooLong {
                path,
                actual: frontmatter.description.chars().count(),
                limit: WORKFLOW_DESCRIPTION_HARD_CAP,
            });
        }

        let record = WorkflowRecord {
            slug: slug.clone(),
            description: frontmatter.description,
            model_invokation: frontmatter.model_invokation,
            user_invocable: frontmatter.user_invocable,
            requires: frontmatter.requires,
            body: body.to_string(),
            path: path.clone(),
        };
        records.insert(slug.clone(), record);
    }

    Ok(WorkflowRegistry { records })
}

fn warn_unknown_workflow_fields(path: &Path, yaml: &str) {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return;
    };
    let Some(map) = value.as_mapping() else {
        return;
    };
    for key in map.keys().filter_map(|k| k.as_str()) {
        if !matches!(
            key,
            "description"
                | "model_invokation"
                | "user_invocable"
                | "requires"
                | "created_at"
                | "updated_at"
        ) {
            warn!(path = %path.display(), field = key, "unknown workflow frontmatter field ignored");
        }
    }
}

fn map_serde_workflow_error(err: serde_yaml::Error) -> LintError {
    let msg = err.to_string();
    if let Some(field) = parse_missing_field(&msg) {
        return LintError::MissingField(field);
    }
    LintError::MalformedFrontmatter(msg)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/memory/workflow")).unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    fn write_workflow(root: &Path, slug: &str, frontmatter: &str, body: &str) {
        let path = root
            .join(".insomnia/memory/workflow")
            .join(format!("{slug}.md"));
        std::fs::write(path, format!("---\n{frontmatter}\n---\n{body}")).unwrap();
    }

    #[test]
    fn missing_directory_loads_empty_registry() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let got = load_workflows(&layout).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn loads_valid_workflow_with_default_flags() {
        let (dir, layout) = setup();
        write_workflow(dir.path(), "do-thing", "description: Do thing", "Step 1");
        let got = load_workflows(&layout).unwrap();
        let slug = Slug::parse("do-thing").unwrap();
        let record = got.get(&slug).unwrap();
        assert_eq!(record.description, "Do thing");
        assert!(!record.model_invokation);
        assert!(record.user_invocable);
        assert!(record.requires.is_empty());
        assert_eq!(record.body, "Step 1");
    }

    #[test]
    fn model_invokation_uses_typo_field() {
        let (dir, layout) = setup();
        write_workflow(
            dir.path(),
            "auto",
            "description: Auto\nmodel_invokation: true\nuser_invocable: false",
            "Body",
        );
        let got = load_workflows(&layout).unwrap();
        assert_eq!(got.resident_entries()[0].slug, "auto");
        assert!(got.list_user_invocable("").is_empty());
    }

    #[test]
    fn invalid_filename_is_hard_error() {
        let (dir, layout) = setup();
        write_workflow(dir.path(), "Bad", "description: Bad", "Body");
        let err = load_workflows(&layout).unwrap_err();
        assert!(matches!(err, WorkflowLoadError::InvalidSlug { .. }));
    }

    #[test]
    fn missing_description_is_hard_error() {
        let (dir, layout) = setup();
        write_workflow(dir.path(), "bad", "model_invokation: false", "Body");
        let err = load_workflows(&layout).unwrap_err();
        assert!(matches!(err, WorkflowLoadError::Frontmatter { .. }));
    }

    #[test]
    fn resident_description_cap_is_enforced() {
        let (dir, layout) = setup();
        let desc = "x".repeat(WORKFLOW_DESCRIPTION_HARD_CAP + 1);
        write_workflow(
            dir.path(),
            "bad",
            &format!("description: {desc}\nmodel_invokation: true"),
            "Body",
        );
        let err = load_workflows(&layout).unwrap_err();
        assert!(matches!(err, WorkflowLoadError::DescriptionTooLong { .. }));
    }
}
