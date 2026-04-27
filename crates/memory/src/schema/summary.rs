//! Summary frontmatter schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::Frontmatter;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SummaryFrontmatter {
    pub updated_at: DateTime<Utc>,
    /// `created_at` is optional for the summary because it's a
    /// long-lived single file rewritten in place.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// Optional pointer to the session-store entry range that drove the
    /// most recent rewrite.
    #[serde(default)]
    pub last_rewritten_from_range: Option<[u64; 2]>,
}

impl Frontmatter for SummaryFrontmatter {
    /// Summary holds always-on context, so it gets a larger body budget
    /// than per-record kinds (~5k tokens at the upper end).
    const BODY_LIMIT: usize = 20000;

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at.unwrap_or(self.updated_at)
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}
