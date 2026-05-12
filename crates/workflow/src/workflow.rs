//! Workflow loader and registry.
//!
//! Workflows live under `<workspace>/.insomnia/workflow/<slug>.md`. They are
//! human-authored Markdown documents with YAML frontmatter. The loader is
//! intentionally strict about malformed records because Pod startup should
//! fail rather than silently ignoring a broken procedural instruction.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use crate::schema::{WorkflowFrontmatter, split_frontmatter};
use lint_common::RecordLintError;
use memory::WorkspaceLayout;

use crate::{Slug, WorkflowLintError};

/// Hard cap on Workflow descriptions that are advertised resident.
/// Mirrors agent-skills and resident Knowledge descriptions.
pub const WORKFLOW_DESCRIPTION_HARD_CAP: usize = 1024;

/// Origin of a [`WorkflowRecord`]. Used to break ties when the same slug
/// is provided by multiple sources: workspace-authored Workflows always
/// win over external skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowSource {
    /// `<workspace>/.insomnia/workflow/<slug>.md`. Authored in-tree by
    /// the project.
    WorkspaceWorkflow,
    /// SKILL.md ingested from a `[skills] directories` entry in the
    /// manifest. `dir` is the skills root that contained
    /// `<slug>/SKILL.md`.
    Skill { dir: PathBuf },
}

impl WorkflowSource {
    /// Human-readable label used in shadow-notification messages.
    pub fn label(&self) -> &'static str {
        match self {
            Self::WorkspaceWorkflow => "workspace workflow",
            Self::Skill { .. } => "skill",
        }
    }
}

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
    /// Where this record was loaded from. Determines shadowing priority
    /// when [`WorkflowRegistry::merge_skill`] encounters a slug
    /// collision.
    pub source: WorkflowSource,
}

/// Returned by [`WorkflowRegistry::merge_skill`] when an incoming skill is
/// shadowed by an existing record (either an internal Workflow or a
/// higher-priority skill). Carries enough context for a `Notification` to
/// explain which side won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedSkill {
    pub slug: Slug,
    pub kept_source: WorkflowSource,
    pub kept_path: PathBuf,
    pub shadowed_source: WorkflowSource,
    pub shadowed_path: PathBuf,
}

impl ShadowedSkill {
    /// One-line message for `Notification` payloads.
    pub fn message(&self) -> String {
        format!(
            "skill /{slug} from {shadowed_label} ({shadowed_path}) was shadowed by existing {kept_label} ({kept_path})",
            slug = self.slug,
            shadowed_label = self.shadowed_source.label(),
            shadowed_path = self.shadowed_path.display(),
            kept_label = self.kept_source.label(),
            kept_path = self.kept_path.display(),
        )
    }
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

    /// Insert a skill-derived record. If an existing record (internal
    /// Workflow or earlier-fed skill) already owns the slug, the
    /// incoming record is dropped and a [`ShadowedSkill`] describing the
    /// collision is returned. Callers feed records in priority order
    /// (highest first); the registry is "first-insert wins" and does
    /// not re-rank.
    pub fn merge_skill(&mut self, record: WorkflowRecord) -> Option<ShadowedSkill> {
        if let Some(existing) = self.records.get(&record.slug) {
            return Some(ShadowedSkill {
                slug: record.slug.clone(),
                kept_source: existing.source.clone(),
                kept_path: existing.path.clone(),
                shadowed_source: record.source,
                shadowed_path: record.path,
            });
        }
        self.records.insert(record.slug.clone(), record);
        None
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
    InvalidSlug {
        path: PathBuf,
        source: WorkflowLintError,
    },
    #[error("invalid workflow frontmatter in {}: {source}", .path.display())]
    Frontmatter {
        path: PathBuf,
        source: WorkflowLintError,
    },
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
                source: source.into(),
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
            source: WorkflowSource::WorkspaceWorkflow,
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

fn map_serde_workflow_error(err: serde_yaml::Error) -> WorkflowLintError {
    let msg = err.to_string();
    if let Some(field) = parse_missing_field(&msg) {
        return WorkflowLintError::MissingField(field);
    }
    WorkflowLintError::Record(RecordLintError::MalformedFrontmatter(msg))
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
        std::fs::create_dir_all(dir.path().join(".insomnia/workflow")).unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    fn write_workflow(root: &Path, slug: &str, frontmatter: &str, body: &str) {
        let path = root.join(".insomnia/workflow").join(format!("{slug}.md"));
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
    fn workflow_under_memory_is_ignored() {
        // The legacy `.insomnia/memory/workflow/` location is no longer
        // a Workflow source. Files placed there must be ignored (the
        // loader is rooted at `.insomnia/workflow/` only).
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let legacy = dir.path().join(".insomnia/memory/workflow");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("ghost.md"),
            "---\ndescription: ghost\n---\nbody\n",
        )
        .unwrap();
        let got = load_workflows(&layout).unwrap();
        assert!(got.is_empty());
    }

    fn skill_record(slug: &str, path: &Path) -> WorkflowRecord {
        WorkflowRecord {
            slug: Slug::parse(slug).unwrap(),
            description: format!("desc {slug}"),
            model_invokation: true,
            user_invocable: true,
            requires: Vec::new(),
            body: format!("body for {slug}"),
            path: path.to_path_buf(),
            source: WorkflowSource::Skill {
                dir: path.parent().unwrap().parent().unwrap().to_path_buf(),
            },
        }
    }

    #[test]
    fn merge_skill_inserts_when_no_collision() {
        let mut reg = WorkflowRegistry::empty();
        let path = std::path::PathBuf::from("/tmp/skills/x/SKILL.md");
        let shadow = reg.merge_skill(skill_record("x", &path));
        assert!(shadow.is_none());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn merge_skill_shadows_existing_workflow() {
        let (dir, layout) = setup();
        write_workflow(
            dir.path(),
            "shared",
            "description: Internal",
            "internal body",
        );
        let mut reg = load_workflows(&layout).unwrap();
        let skill_path = dir
            .path()
            .join("user-skills")
            .join("shared")
            .join("SKILL.md");
        std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        std::fs::write(&skill_path, "ignored").unwrap();
        let incoming = WorkflowRecord {
            slug: Slug::parse("shared").unwrap(),
            description: "From skill".into(),
            model_invokation: true,
            user_invocable: true,
            requires: Vec::new(),
            body: "skill body".into(),
            path: skill_path.clone(),
            source: WorkflowSource::Skill {
                dir: dir.path().join("user-skills"),
            },
        };
        let shadow = reg.merge_skill(incoming).expect("expected shadow");
        assert_eq!(shadow.slug.as_str(), "shared");
        assert!(matches!(
            shadow.kept_source,
            WorkflowSource::WorkspaceWorkflow
        ));
        assert!(matches!(
            shadow.shadowed_source,
            WorkflowSource::Skill { .. }
        ));
        // The kept record is still the workspace workflow.
        let kept = reg.get(&Slug::parse("shared").unwrap()).unwrap();
        assert!(matches!(kept.source, WorkflowSource::WorkspaceWorkflow));
        assert_eq!(kept.body, "internal body");
    }

    #[test]
    fn merge_skill_first_fed_wins_on_collision() {
        let mut reg = WorkflowRegistry::empty();
        let first_path = std::path::PathBuf::from("/a/skills/x/SKILL.md");
        let second_path = std::path::PathBuf::from("/b/skills/x/SKILL.md");
        let first = WorkflowRecord {
            slug: Slug::parse("x").unwrap(),
            description: "first".into(),
            model_invokation: true,
            user_invocable: true,
            requires: Vec::new(),
            body: "first body".into(),
            path: first_path.clone(),
            source: WorkflowSource::Skill {
                dir: std::path::PathBuf::from("/a/skills"),
            },
        };
        let second = WorkflowRecord {
            slug: Slug::parse("x").unwrap(),
            description: "second".into(),
            model_invokation: true,
            user_invocable: true,
            requires: Vec::new(),
            body: "second body".into(),
            path: second_path.clone(),
            source: WorkflowSource::Skill {
                dir: std::path::PathBuf::from("/b/skills"),
            },
        };
        // Caller is responsible for feeding in priority order; the
        // registry just keeps whichever arrives first.
        assert!(reg.merge_skill(first).is_none());
        let shadow = reg
            .merge_skill(second)
            .expect("later-fed skill must shadow");
        assert_eq!(shadow.kept_path, first_path);
        assert!(matches!(shadow.kept_source, WorkflowSource::Skill { .. }));
    }

    #[test]
    fn shadow_message_is_human_readable() {
        let s = ShadowedSkill {
            slug: Slug::parse("x").unwrap(),
            kept_source: WorkflowSource::WorkspaceWorkflow,
            kept_path: std::path::PathBuf::from("/ws/.insomnia/workflow/x.md"),
            shadowed_source: WorkflowSource::Skill {
                dir: std::path::PathBuf::from("/skills"),
            },
            shadowed_path: std::path::PathBuf::from("/skills/x/SKILL.md"),
        };
        let msg = s.message();
        assert!(msg.contains("/x"));
        assert!(msg.contains("workspace workflow"));
        assert!(msg.contains("skill"));
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
