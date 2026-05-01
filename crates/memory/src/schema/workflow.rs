//! Workflow frontmatter schema.
//!
//! NOTE: Workflows are written by humans, not by the memory tool. The
//! linter only validates frontmatter when invoked directly (e.g. by a
//! future CLI / pre-commit hook). The memory write/edit tool rejects
//! `memory/workflow/` paths outright via [`LintError::WorkflowWriteForbidden`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::Frontmatter;
use crate::slug::Slug;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowFrontmatter {
    /// Workflows do not require timestamps in the MVP. Human-authored files
    /// may carry them; when absent the linter uses Unix epoch as a neutral
    /// placeholder for the shared `Frontmatter` trait.
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    pub description: String,
    #[serde(default)]
    pub model_invokation: bool,
    #[serde(default = "default_user_invocable")]
    pub user_invocable: bool,
    #[serde(default)]
    pub requires: Vec<Slug>,
}

fn default_user_invocable() -> bool {
    true
}

fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch timestamp is valid")
}

impl Frontmatter for WorkflowFrontmatter {
    const BODY_LIMIT: usize = 8000;

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at.or(self.updated_at).unwrap_or_else(epoch)
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at.unwrap_or_else(epoch)
    }
}
