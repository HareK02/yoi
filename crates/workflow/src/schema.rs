//! Workflow frontmatter schema and frontmatter splitting helpers.

use chrono::{DateTime, Utc};
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

fn default_user_invocable() -> bool {
    true
}

const FRONTMATTER_DELIM: &str = "---";

/// Split a markdown document into `(yaml_frontmatter, body)`.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), WorkflowLintError> {
    let after_open = content
        .strip_prefix(FRONTMATTER_DELIM)
        .and_then(|s| s.strip_prefix('\n').or(Some(s)))
        .ok_or(WorkflowLintError::MissingFrontmatter)?;

    let mut yaml_end = None;
    let mut byte_offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == FRONTMATTER_DELIM {
            yaml_end = Some((byte_offset, byte_offset + line.len()));
            break;
        }
        byte_offset += line.len();
    }

    let (yaml_end_excl, body_start) = yaml_end.ok_or_else(|| {
        WorkflowLintError::MalformedFrontmatter("missing closing `---` line".to_string())
    })?;

    let yaml = &after_open[..yaml_end_excl];
    let body = &after_open[body_start..];
    Ok((yaml, body))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(err, WorkflowLintError::MissingFrontmatter));
    }

    #[test]
    fn no_closing_delim_errors() {
        let err = split_frontmatter("---\nfoo: 1\nno close\n").unwrap_err();
        assert!(matches!(err, WorkflowLintError::MalformedFrontmatter(_)));
    }

    #[test]
    fn handles_empty_body() {
        let doc = "---\nfoo: 1\n---\n";
        let (_, b) = split_frontmatter(doc).unwrap();
        assert_eq!(b, "");
    }
}
