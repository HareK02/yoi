//! Memory subsystem support shared by Worker and Workspace authority paths.
//!
//! Normal runtime Memory access is mediated through Workspace-backed backend
//! operations. Generic filesystem tools must not touch local memory paths; Worker
//! is responsible for denying them at the Scope level when memory is enabled.

pub mod audit;
pub mod backend;
pub mod consolidate;
pub mod error;
pub mod extract;
pub mod linter;
pub mod resident;
pub mod schema;
pub mod scope;
pub mod usage;
pub mod workspace;

pub use error::{LintError, LintWarning, MemoryError};
pub use extract::ExtractPointerPayload;
pub use lint_common::{RecordLintError, Slug, is_valid_slug};
pub use linter::{LintReport, Linter};
pub use resident::collect_resident_summary;
pub use scope::deny_write_rules;
pub use usage::{
    UsageEvent, UsageEventKind, UsageRecordSnapshot, UsageReport, UsageReportRecord, UsageSource,
    append_resident_exposure_event, append_usage_event, append_use_event, build_usage_report,
    snapshot_record_from_bytes, snapshot_record_from_layout,
};
pub use workspace::WorkspaceLayout;
