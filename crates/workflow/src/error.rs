//! Errors raised by Workflow loading and linting.

use std::path::PathBuf;

use thiserror::Error;

/// A single Workflow linter violation.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WorkflowLintError {
    #[error("invalid slug `{0}`: must match ^[a-z0-9](?:[a-z0-9-]{{0,62}}[a-z0-9])?$")]
    InvalidSlug(String),

    #[error("malformed frontmatter: {0}")]
    MalformedFrontmatter(String),

    #[error("frontmatter is missing or document is empty")]
    MissingFrontmatter,

    #[error("missing required frontmatter field: `{0}`")]
    MissingField(&'static str),

    #[error(
        "Workflow with model_invokation: true cannot have description longer than {limit} chars (got {actual})"
    )]
    DescriptionTooLong { actual: usize, limit: usize },

    #[error("body exceeds the Workflow size limit: {actual} chars > {limit}")]
    BodyTooLong { actual: usize, limit: usize },

    #[error("`{field}` references unknown {kind} slug `{slug}`")]
    UnknownReference {
        field: &'static str,
        kind: &'static str,
        slug: String,
    },

    #[error("path is not a valid Workflow location: {}", .0.display())]
    InvalidPath(PathBuf),
}
