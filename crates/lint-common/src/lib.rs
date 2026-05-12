//! Shared record lint primitives for memory and workflow files.

mod frontmatter;
mod slug;

pub use frontmatter::{Frontmatter, split_frontmatter};
pub use slug::{Slug, is_valid_slug};

/// Common lint errors for Markdown record syntax shared by memory and workflow.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum RecordLintError {
    #[error("invalid slug `{0}`: must match ^[a-z0-9](?:[a-z0-9-]{{0,62}}[a-z0-9])?$")]
    InvalidSlug(String),

    #[error("malformed frontmatter: {0}")]
    MalformedFrontmatter(String),

    #[error("frontmatter is missing or document is empty")]
    MissingFrontmatter,
}
