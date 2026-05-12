//! Requests frontmatter schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::{Frontmatter, SourceRef};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RequestFrontmatter {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sources: Vec<SourceRef>,
}

impl Frontmatter for RequestFrontmatter {
    const BODY_LIMIT: usize = 8000;

    fn created_at(&self) -> Option<DateTime<Utc>> {
        Some(self.created_at)
    }
    fn updated_at(&self) -> Option<DateTime<Utc>> {
        Some(self.updated_at)
    }
}
