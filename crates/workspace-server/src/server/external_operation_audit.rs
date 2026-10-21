//! Per-request audit notifications, independent of provider result classification.
//!
//! The scope follows the operation future, not a session-wide last-result slot: concurrent
//! requests cannot consume one another's diagnostics. Every failure is also emitted immediately
//! for direct session callers and requests cancelled before a response. This is notification,
//! not a replacement durable audit ledger or crash-recovery/effect-deduplication guarantee.

use std::{cell::RefCell, future::Future};
use workdir::http::WorkdirAuditFailure;

tokio::task_local! {
    static FAILURES: RefCell<Vec<WorkdirAuditFailure>>;
}

pub(super) async fn capture<T>(future: impl Future<Output = T>) -> (T, Vec<WorkdirAuditFailure>) {
    FAILURES
        .scope(RefCell::new(Vec::new()), async {
            let result = future.await;
            let failures = FAILURES.with(|failures| failures.take());
            (result, failures)
        })
        .await
}

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
    let _ = FAILURES.try_with(|failures| failures.borrow_mut().push(failure));
}
