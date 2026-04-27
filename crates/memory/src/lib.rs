//! Memory subsystem: persistence layer for `memory/*` and `knowledge/*` records.
//!
//! Self-contained: provides its own Tool implementations (read/write/edit)
//! that target `<workspace>/memory/` and `<workspace>/knowledge/` only,
//! with a pre-write Linter built in. Generic CRUD tools (in the `tools`
//! crate) must not touch these directories — Pod is responsible for
//! denying them at the Scope level when memory is enabled.

pub mod error;
pub mod linter;
pub mod resident;
pub mod schema;
pub mod scope;
pub mod slug;
pub mod tool;
pub mod workspace;

pub use error::{LintError, LintWarning, MemoryError};
pub use linter::{LintReport, Linter};
pub use resident::{ResidentKnowledgeEntry, collect_resident_knowledge};
pub use scope::deny_write_rules;
pub use slug::Slug;
pub use workspace::WorkspaceLayout;
