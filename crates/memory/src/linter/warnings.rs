//! Soft warnings: low-importance large records, sources accumulation.
//!
//! Similar-slug warnings need the existing record set and are
//! integrated into the main linter pass when implemented; this file
//! covers per-write checks that only need the proposed content.

use crate::error::LintWarning;
use crate::linter::LintReport;
use crate::workspace::ClassifiedPath;

const LARGE_BODY_THRESHOLD: usize = 1500;
const SOURCES_OVERFLOW_THRESHOLD: usize = 10;

/// For kinds that don't carry a `sources` array (Summary), emit only
/// the body-size warning.
pub fn check_warnings_kindless(_cp: &ClassifiedPath, body: &str, _report: &mut LintReport) {
    let _ = body;
    // Summary intentionally has no warning band — the per-record
    // size:importance heuristic doesn't apply to a single rolling file.
}

/// For kinds with `sources` (Decisions / Requests / Knowledge), consult
/// both the body length and the sources count.
pub fn check_warnings_with_sources(body: &str, source_count: usize, report: &mut LintReport) {
    let chars = body.chars().count();
    if source_count <= 1 && chars >= LARGE_BODY_THRESHOLD {
        report.push_warning(LintWarning::LowImportanceLargeRecord { chars });
    }
    if source_count > SOURCES_OVERFLOW_THRESHOLD {
        report.push_warning(LintWarning::SourcesOverflow {
            count: source_count,
        });
    }
}
