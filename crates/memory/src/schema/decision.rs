//! Decisions frontmatter schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::{Frontmatter, SourceRef};
use crate::slug::Slug;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Open,
    Resolved,
    Replaced,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DecisionFrontmatter {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sources: Vec<SourceRef>,
    pub status: DecisionStatus,
    #[serde(default)]
    pub replaced_by: Option<Slug>,
}

impl Frontmatter for DecisionFrontmatter {
    const BODY_LIMIT: usize = 8000;

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}
