//! Workflow records, loading, Agent Skill ingestion, and human-edit linting.

mod error;
mod linter;
mod schema;
mod scope;
mod skill;
mod slug;
mod workflow;

pub use error::WorkflowLintError;
pub use linter::{WorkflowLintReport, WorkflowLinter};
pub use schema::{WorkflowFrontmatter, split_frontmatter};
pub use scope::deny_write_rules;
pub use skill::{
    SKILL_FILENAME, SkillParseError, SkillRecord, load_skills_from_dir, parse_skill_md,
};
pub use slug::{Slug, is_valid_slug};
pub use workflow::{
    ResidentWorkflowEntry, ShadowedSkill, WORKFLOW_DESCRIPTION_HARD_CAP, WorkflowLoadError,
    WorkflowRecord, WorkflowRegistry, WorkflowSource, load_workflows,
};
