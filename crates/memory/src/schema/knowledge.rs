//! Knowledge frontmatter schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::{Frontmatter, SourceRef};

/// Hard cap on `description` length when `model_invokation: true`.
/// Mirrors the agent-skills 1024-char rule for description that lives
/// in resident system-prompt budget.
pub const KNOWLEDGE_DESCRIPTION_HARD_CAP: usize = 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KnowledgeFrontmatter {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub kind: String,
    pub description: String,
    pub model_invokation: bool,
    pub user_invocable: bool,
    pub last_sources: Vec<SourceRef>,
}

impl Frontmatter for KnowledgeFrontmatter {
    const BODY_LIMIT: usize = 8000;

    fn created_at(&self) -> Option<DateTime<Utc>> {
        Some(self.created_at)
    }
    fn updated_at(&self) -> Option<DateTime<Utc>> {
        Some(self.updated_at)
    }
}
