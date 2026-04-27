//! Body size limit checks.

use crate::error::LintError;
use crate::linter::LintReport;
use crate::schema::Frontmatter;

pub fn check_body<F: Frontmatter>(body: &str, report: &mut LintReport) {
    let chars = body.chars().count();
    if chars > F::BODY_LIMIT {
        report.push_error(LintError::BodyTooLong {
            actual: chars,
            limit: F::BODY_LIMIT,
        });
    }
}
