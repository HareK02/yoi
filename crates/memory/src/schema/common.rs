//! Common frontmatter helpers and shared types.

use serde::{Deserialize, Serialize};

use crate::error::LintError;

pub use lint_common::Frontmatter;

/// Reference to a session-store entry range. Stored in `sources` /
/// `last_sources` arrays for traceability back to raw session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub segment_id: String,
    /// `[start_entry, end_entry]` inclusive range of session-store entry indices.
    pub range: [u64; 2],
}

/// Split a markdown document into `(yaml_frontmatter, body)`.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), LintError> {
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
            LintError::Record(RecordLintError::MissingFrontmatter)
        ));
    }

    #[test]
    fn no_closing_delim_errors() {
        let err = split_frontmatter("---\nfoo: 1\nno close\n").unwrap_err();
        assert!(matches!(
            err,
            LintError::Record(RecordLintError::MalformedFrontmatter(_))
        ));
    }

    #[test]
    fn handles_empty_body() {
        let doc = "---\nfoo: 1\n---\n";
        let (_, b) = split_frontmatter(doc).unwrap();
        assert_eq!(b, "");
    }
}
