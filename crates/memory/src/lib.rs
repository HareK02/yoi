//! Memory subsystem: persistence layer for `memory/*` and `knowledge/*` records.
//!
//! Self-contained: provides its own Tool implementations (read/write/edit)
//! that target `<workspace>/memory/` and `<workspace>/knowledge/` only,
//! with a pre-write Linter built in. Generic CRUD tools (in the `tools`
//! crate) must not touch these directories — Pod is responsible for
//! denying them at the Scope level when memory is enabled.

pub mod consolidate;
pub mod error;
pub mod extract;
pub mod linter;
pub mod resident;
pub mod schema;
pub mod scope;
pub mod tool;
pub mod usage;
pub mod workspace;

pub use error::{LintError, LintWarning, MemoryError};
pub use extract::ExtractPointerPayload;
pub use lint_common::{RecordLintError, Slug, is_valid_slug};
pub use linter::{LintReport, Linter};
pub use resident::{ResidentKnowledgeEntry, collect_resident_knowledge, list_knowledge_slugs};
pub use scope::deny_write_rules;
pub use usage::{
    UsageEvent, UsageEventKind, UsageRecordSnapshot, UsageReport, UsageReportRecord, UsageSource,
    append_resident_exposure_event, append_usage_event, append_use_event, build_usage_report,
    snapshot_record_from_bytes, snapshot_record_from_layout,
};
pub use workspace::WorkspaceLayout;
