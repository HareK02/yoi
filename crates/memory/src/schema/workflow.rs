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
    /// Workflows don't carry sources/created_at requirements in the
    /// plan doc; only `updated_at` is required at the schema level.
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    pub description: String,
    pub auto_invoke: bool,
    pub user_invocable: bool,
    #[serde(default)]
    pub requires: Vec<Slug>,
}

impl Frontmatter for WorkflowFrontmatter {
    const BODY_LIMIT: usize = 8000;

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at.unwrap_or(self.updated_at)
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}
