//! Common frontmatter helpers.

use chrono::{DateTime, Utc};

use crate::RecordLintError;

/// Trait record frontmatter types implement so linters can drive them uniformly.
pub trait Frontmatter: Sized {
    /// Hard upper bound on body chars (excluding the frontmatter block).
    const BODY_LIMIT: usize;

    fn created_at(&self) -> Option<DateTime<Utc>>;
    fn updated_at(&self) -> Option<DateTime<Utc>>;
}

const FRONTMATTER_DELIM: &str = "---";

/// Split a markdown document into `(yaml_frontmatter, body)`.
///
/// Expects the document to start with `---\n` and have a closing
/// `---\n` (or `---` at EOF) somewhere downstream. Trailing newline
/// after the closing delimiter is consumed.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), RecordLintError> {
    // The opening delimiter must be the very first line.
    let after_open = content
        .strip_prefix(FRONTMATTER_DELIM)
        .and_then(|s| s.strip_prefix('\n').or(Some(s)))
        .ok_or(RecordLintError::MissingFrontmatter)?;

    // Look for the closing `---` on its own line.
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
        RecordLintError::MalformedFrontmatter("missing closing `---` line".to_string())
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
        assert!(matches!(err, RecordLintError::MissingFrontmatter));
    }

    #[test]
    fn no_closing_delim_errors() {
        let err = split_frontmatter("---\nfoo: 1\nno close\n").unwrap_err();
        assert!(matches!(err, RecordLintError::MalformedFrontmatter(_)));
    }

    #[test]
    fn handles_empty_body() {
        let doc = "---\nfoo: 1\n---\n";
        let (_, b) = split_frontmatter(doc).unwrap();
        assert_eq!(b, "");
    }
}
