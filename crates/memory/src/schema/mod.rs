//! Typed frontmatter schemas for Memory documents.
//!
//! These structures and pure parsing helpers are shared by Workspace-backed
//! validation. They do not select or traverse a repository-local Memory tree.

mod common;
mod decision;
mod request;
mod summary;

pub(crate) use common::JsonSafeU64Pair;
pub use common::{
    EvidenceKind, EvidenceOrigin, EvidenceOriginKind, Frontmatter, SourceEvidenceRef, SourceRef,
    split_frontmatter,
};
pub use decision::{DecisionFrontmatter, DecisionStatus};
pub use request::RequestFrontmatter;
pub use summary::SummaryFrontmatter;
