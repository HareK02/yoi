//! Frontmatter schemas for memory records.
//!
//! Each record kind has its own typed `*Frontmatter` struct. The linter
//! deserializes the YAML between the leading `---` markers into the
//! kind-appropriate struct; field-level errors are surfaced as
//! [`LintError::MissingField`] / [`LintError::InvalidField`].

mod common;
mod decision;
mod knowledge;
mod request;
mod summary;
mod workflow;

pub use common::{Frontmatter, SourceRef, split_frontmatter};
pub use decision::{DecisionFrontmatter, DecisionStatus};
pub use knowledge::{KNOWLEDGE_DESCRIPTION_HARD_CAP, KnowledgeFrontmatter};
pub use request::RequestFrontmatter;
pub use summary::SummaryFrontmatter;
pub use workflow::WorkflowFrontmatter;
