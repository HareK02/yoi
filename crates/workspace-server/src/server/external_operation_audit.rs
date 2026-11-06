//! Per-request audit notifications, independent of provider result classification.
//!
//! The scope follows the operation future, not a session-wide last-result slot: concurrent
//! requests cannot consume one another's diagnostics. Every failure is also emitted immediately
//! for direct session callers and requests cancelled before a response. This is notification,
//! not a replacement durable audit ledger or crash-recovery/effect-deduplication guarantee.

use workdir::http::WorkdirAuditFailure;

pub(super) use workdir::audit::capture;

pub(super) fn notify(failure: WorkdirAuditFailure, operation: &'static str) {
    tracing::error!(
        event = "external_workdir_audit_failed",
        operation,
        operation_id = %failure.operation_id,
        audit_stage = ?failure.stage,
        audit_reason = ?failure.reason,
        operation_outcome = ?failure.operation_outcome,
        "External Workdir audit persistence failed; operation result is independent; do not resend"
    );
    workdir::audit::notify(failure);
}
