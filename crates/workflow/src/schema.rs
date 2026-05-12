//! Workflow frontmatter schema and frontmatter splitting helpers.

use chrono::{DateTime, Utc};
use lint_common::Frontmatter;
use serde::{Deserialize, Serialize};

use crate::{Slug, WorkflowLintError};

pub const WORKFLOW_BODY_LIMIT: usize = 8000;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowFrontmatter {
    /// Workflows do not require timestamps in the MVP. Human-authored files
    /// may carry them.
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

impl Frontmatter for WorkflowFrontmatter {
    const BODY_LIMIT: usize = WORKFLOW_BODY_LIMIT;

    fn created_at(&self) -> Option<DateTime<Utc>> {
        self.created_at
    }

    fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.updated_at
    }
}

fn default_user_invocable() -> bool {
    true
}

/// Split a markdown document into `(yaml_frontmatter, body)`.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), WorkflowLintError> {
    lint_common::split_frontmatter(content).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lint_common::RecordLintError;

    #[test]
    fn splits_simple() {
        let doc = "---\nfoo: 1\n---\nbody here\n";
        let (y, b) = split_frontmatter(doc).unwrap();
        assert_eq!(y, "foo: 1\n");
        assert_eq!(b, "body here\n");
    }

    #[test]
    fn no_leading_delim_errors() {
        let err = split_frontmatter("hello").unwrap_err();
        assert!(matches!(
            err,
            WorkflowLintError::Record(RecordLintError::MissingFrontmatter)
        ));
    }

    #[test]
    fn no_closing_delim_errors() {
        let err = split_frontmatter("---\nfoo: 1\nno close\n").unwrap_err();
        assert!(matches!(
            err,
            WorkflowLintError::Record(RecordLintError::MalformedFrontmatter(_))
        ));
    }

    #[test]
    fn handles_empty_body() {
        let doc = "---\nfoo: 1\n---\n";
        let (_, b) = split_frontmatter(doc).unwrap();
        assert_eq!(b, "");
    }
}
