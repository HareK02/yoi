//! Agent Skills (`SKILL.md`) parser.
//!
//! Skills follow the [agentskills.io](https://agentskills.io/specification)
//! spec: a directory `<root>/<name>/` containing `SKILL.md` (YAML frontmatter
//! + Markdown body) and optional `scripts/` / `references/` / `assets/`
//! subdirectories. The body is procedural agent guidance; insomnia ingests
//! it as a Workflow so `/<name>` resolves to it just like an internal
//! Workflow.
//!
//! Parsing is intentionally lenient at the directory-scan level — one
//! malformed SKILL.md emits `tracing::warn!` and is skipped, leaving sibling
//! skills loadable. Internal Workflows (`.insomnia/workflow/<slug>.md`) keep
//! their hard-error semantics.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

use crate::error::LintError;
use crate::schema::split_frontmatter;
use crate::slug::Slug;
use crate::workflow::{WORKFLOW_DESCRIPTION_HARD_CAP, WorkflowRecord, WorkflowSource};

/// Filename within a skill directory carrying the frontmatter + body.
pub const SKILL_FILENAME: &str = "SKILL.md";

/// SKILL.md frontmatter as defined by the agent-skills spec.
///
/// Fields beyond `name` / `description` are accepted to be spec-compatible
/// but not used by insomnia today: `license`, `compatibility`, and
/// `metadata` are documentary, while `allowed-tools` is recognised and
/// emits a warning until [`permission-extension-point.md`] lands.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_yaml::Value>,
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<serde_yaml::Value>,
}

/// Validated skill record. Constructed by [`parse_skill_md`] and converted
/// to a `WorkflowRecord` by the caller via the `Skill → Workflow`
/// projection in [`crate::workflow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRecord {
    pub slug: Slug,
    pub description: String,
    pub body: String,
    /// The skill directory (parent of `SKILL.md`). Carried so callers can
    /// register `scripts/` / `references/` / `assets/` against the Pod's
    /// scope.
    pub dir: PathBuf,
    /// Path to the `SKILL.md` file itself. Used as the resolved path on
    /// the resulting `WorkflowRecord`.
    pub skill_md_path: PathBuf,
}

impl SkillRecord {
    /// Project this skill into a [`WorkflowRecord`]. Skill-sourced
    /// Workflows are advertised resident (`model_invokation: true`,
    /// matching the agentskills progressive-disclosure model), are
    /// invocable as `/<slug>`, and carry no `requires` since the SKILL
    /// spec has no Knowledge-dependency concept.
    pub fn into_workflow_record(self, source: WorkflowSource) -> WorkflowRecord {
        WorkflowRecord {
            slug: self.slug,
            description: self.description,
            model_invokation: true,
            user_invocable: true,
            requires: Vec::new(),
            body: self.body,
            path: self.skill_md_path,
            source,
        }
    }
}

#[derive(Debug, Error)]
pub enum SkillParseError {
    #[error("skill path has no parent directory: {}", .0.display())]
    NoParentDir(PathBuf),
    #[error("failed to read SKILL.md at {}: {source}", .path.display())]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("invalid frontmatter in {}: {source}", .path.display())]
    Frontmatter {
        path: PathBuf,
        #[source]
        source: LintError,
    },
    #[error(
        "SKILL.md `name` `{name}` does not match its directory name `{dir_name}` (at {})",
        .skill_md_path.display()
    )]
    NameDirMismatch {
        name: String,
        dir_name: String,
        skill_md_path: PathBuf,
    },
    #[error("SKILL.md `name` is not a valid slug at {}: {source}", .skill_md_path.display())]
    InvalidName {
        skill_md_path: PathBuf,
        #[source]
        source: LintError,
    },
    #[error("SKILL.md `description` must be non-empty (at {})", .skill_md_path.display())]
    DescriptionEmpty { skill_md_path: PathBuf },
    #[error(
        "SKILL.md `description` length {actual} exceeds limit {limit} (at {})",
        .skill_md_path.display()
    )]
    DescriptionTooLong {
        skill_md_path: PathBuf,
        actual: usize,
        limit: usize,
    },
}

/// Parse a single `SKILL.md`. The directory name is taken from the parent
/// of `skill_md_path` and validated against the frontmatter `name`.
pub fn parse_skill_md(skill_md_path: &Path) -> Result<SkillRecord, SkillParseError> {
    let dir = skill_md_path
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| SkillParseError::NoParentDir(skill_md_path.to_path_buf()))?;
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| SkillParseError::NoParentDir(skill_md_path.to_path_buf()))?;

    let raw =
        std::fs::read_to_string(skill_md_path).map_err(|source| SkillParseError::ReadFile {
            path: skill_md_path.to_path_buf(),
            source,
        })?;
    let (yaml, body) = split_frontmatter(&raw).map_err(|source| SkillParseError::Frontmatter {
        path: skill_md_path.to_path_buf(),
        source,
    })?;
    warn_unknown_skill_fields(skill_md_path, yaml);
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml).map_err(|err| SkillParseError::Frontmatter {
            path: skill_md_path.to_path_buf(),
            source: LintError::MalformedFrontmatter(err.to_string()),
        })?;

    if frontmatter.allowed_tools.is_some() {
        warn!(
            path = %skill_md_path.display(),
            "SKILL.md `allowed-tools` is recognised but not yet enforced; ignoring"
        );
    }

    let desc_chars = frontmatter.description.chars().count();
    if desc_chars == 0 {
        return Err(SkillParseError::DescriptionEmpty {
            skill_md_path: skill_md_path.to_path_buf(),
        });
    }
    if desc_chars > WORKFLOW_DESCRIPTION_HARD_CAP {
        return Err(SkillParseError::DescriptionTooLong {
            skill_md_path: skill_md_path.to_path_buf(),
            actual: desc_chars,
            limit: WORKFLOW_DESCRIPTION_HARD_CAP,
        });
    }

    if frontmatter.name != dir_name {
        return Err(SkillParseError::NameDirMismatch {
            name: frontmatter.name,
            dir_name,
            skill_md_path: skill_md_path.to_path_buf(),
        });
    }
    let slug = Slug::parse(frontmatter.name).map_err(|source| SkillParseError::InvalidName {
        skill_md_path: skill_md_path.to_path_buf(),
        source,
    })?;

    Ok(SkillRecord {
        slug,
        description: frontmatter.description,
        body: body.to_string(),
        dir,
        skill_md_path: skill_md_path.to_path_buf(),
    })
}

/// Scan a skills root for `<root>/<name>/SKILL.md`. Returns successfully
/// parsed skills; per-skill errors emit a `tracing::warn!` and are
/// skipped. A missing root is treated as zero skills, not an error —
/// callers can probe optional directories without pre-checking.
pub fn load_skills_from_dir(root: &Path) -> Vec<SkillRecord> {
    let entries = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            warn!(
                dir = %root.display(),
                error = %err,
                "failed to read skills directory; treating as empty"
            );
            return Vec::new();
        }
    };

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                warn!(
                    dir = %root.display(),
                    error = %err,
                    "skill directory entry read error; skipping"
                );
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join(SKILL_FILENAME);
        if skill_md.is_file() {
            paths.push(skill_md);
        }
    }
    paths.sort();

    let mut out = Vec::new();
    for path in paths {
        match parse_skill_md(&path) {
            Ok(record) => out.push(record),
            Err(err) => warn!(path = %path.display(), error = %err, "SKILL.md skipped"),
        }
    }
    out
}

fn warn_unknown_skill_fields(path: &Path, yaml: &str) {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return;
    };
    let Some(map) = value.as_mapping() else {
        return;
    };
    for key in map.keys().filter_map(|k| k.as_str()) {
        if !matches!(
            key,
            "name" | "description" | "license" | "compatibility" | "metadata" | "allowed-tools"
        ) {
            warn!(path = %path.display(), field = key, "unknown SKILL.md frontmatter field ignored");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_skill(root: &Path, name: &str, frontmatter: &str, body: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SKILL_FILENAME);
        std::fs::write(&path, format!("---\n{frontmatter}\n---\n{body}")).unwrap();
        path
    }

    #[test]
    fn parses_minimal_skill() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(
            dir.path(),
            "do-thing",
            "name: do-thing\ndescription: Do the thing",
            "Step 1\nStep 2\n",
        );
        let record = parse_skill_md(&path).unwrap();
        assert_eq!(record.slug.as_str(), "do-thing");
        assert_eq!(record.description, "Do the thing");
        assert_eq!(record.body, "Step 1\nStep 2\n");
        assert_eq!(record.dir, dir.path().join("do-thing"));
        assert_eq!(record.skill_md_path, path);
    }

    #[test]
    fn name_dir_mismatch_is_error() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(
            dir.path(),
            "actual-dir",
            "name: declared-name\ndescription: x",
            "body",
        );
        let err = parse_skill_md(&path).unwrap_err();
        assert!(matches!(err, SkillParseError::NameDirMismatch { .. }));
    }

    #[test]
    fn invalid_slug_name_is_error() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(
            dir.path(),
            "BAD-Caps",
            "name: BAD-Caps\ndescription: x",
            "body",
        );
        // Slug::parse rejects uppercase before the dir match check fires;
        // either way the parse is rejected.
        let err = parse_skill_md(&path).unwrap_err();
        assert!(matches!(
            err,
            SkillParseError::InvalidName { .. } | SkillParseError::NameDirMismatch { .. }
        ));
    }

    #[test]
    fn empty_description_is_error() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(dir.path(), "x", "name: x\ndescription: \"\"", "body");
        let err = parse_skill_md(&path).unwrap_err();
        assert!(matches!(err, SkillParseError::DescriptionEmpty { .. }));
    }

    #[test]
    fn description_at_cap_is_accepted() {
        let dir = TempDir::new().unwrap();
        let desc = "x".repeat(WORKFLOW_DESCRIPTION_HARD_CAP);
        let path = write_skill(
            dir.path(),
            "x",
            &format!("name: x\ndescription: {desc}"),
            "body",
        );
        let record = parse_skill_md(&path).unwrap();
        assert_eq!(record.description.chars().count(), WORKFLOW_DESCRIPTION_HARD_CAP);
    }

    #[test]
    fn description_over_cap_is_error() {
        let dir = TempDir::new().unwrap();
        let desc = "x".repeat(WORKFLOW_DESCRIPTION_HARD_CAP + 1);
        let path = write_skill(
            dir.path(),
            "x",
            &format!("name: x\ndescription: {desc}"),
            "body",
        );
        let err = parse_skill_md(&path).unwrap_err();
        assert!(matches!(err, SkillParseError::DescriptionTooLong { .. }));
    }

    #[test]
    fn missing_frontmatter_is_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("x").join(SKILL_FILENAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "no frontmatter at all\n").unwrap();
        let err = parse_skill_md(&path).unwrap_err();
        assert!(matches!(err, SkillParseError::Frontmatter { .. }));
    }

    #[test]
    fn extra_frontmatter_fields_are_kept() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(
            dir.path(),
            "x",
            "name: x\ndescription: ok\nlicense: MIT\ncompatibility: claude-4\n\
             metadata:\n  team: foo\nallowed-tools:\n  - Read",
            "body",
        );
        let record = parse_skill_md(&path).unwrap();
        assert_eq!(record.slug.as_str(), "x");
        // allowed-tools triggers a warn, but parse succeeds.
    }

    #[test]
    fn load_skills_from_dir_skips_broken_and_keeps_good() {
        let dir = TempDir::new().unwrap();
        write_skill(dir.path(), "good", "name: good\ndescription: ok", "body");
        // Mismatch — should be skipped, not abort the scan.
        write_skill(
            dir.path(),
            "bad-dir",
            "name: declared-different\ndescription: ok",
            "body",
        );
        // A bare file at the root (not a directory) is ignored.
        std::fs::write(dir.path().join("stray.md"), "not a skill").unwrap();

        let records = load_skills_from_dir(dir.path());
        let slugs: Vec<&str> = records.iter().map(|r| r.slug.as_str()).collect();
        assert_eq!(slugs, vec!["good"]);
    }

    #[test]
    fn load_skills_from_dir_missing_root_is_empty() {
        let dir = TempDir::new().unwrap();
        let records = load_skills_from_dir(&dir.path().join("does-not-exist"));
        assert!(records.is_empty());
    }

    #[test]
    fn into_workflow_record_uses_skill_defaults() {
        let dir = TempDir::new().unwrap();
        let path = write_skill(
            dir.path(),
            "x",
            "name: x\ndescription: Project X",
            "Steps\n",
        );
        let record = parse_skill_md(&path).unwrap();
        let wf = record.into_workflow_record(WorkflowSource::Skill {
            dir: dir.path().to_path_buf(),
        });
        assert_eq!(wf.slug.as_str(), "x");
        assert_eq!(wf.description, "Project X");
        assert!(wf.model_invokation);
        assert!(wf.user_invocable);
        assert!(wf.requires.is_empty());
        assert_eq!(wf.body, "Steps\n");
        assert!(matches!(wf.source, WorkflowSource::Skill { .. }));
    }

    #[test]
    fn load_skills_from_dir_orders_deterministically() {
        let dir = TempDir::new().unwrap();
        write_skill(dir.path(), "b", "name: b\ndescription: b", "");
        write_skill(dir.path(), "a", "name: a\ndescription: a", "");
        write_skill(dir.path(), "c", "name: c\ndescription: c", "");
        let records = load_skills_from_dir(dir.path());
        let slugs: Vec<&str> = records.iter().map(|r| r.slug.as_str()).collect();
        assert_eq!(slugs, vec!["a", "b", "c"]);
    }
}
