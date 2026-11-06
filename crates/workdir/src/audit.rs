//! Call-local audit receipts, independent of the operation's return value.
//!
//! The scope follows a future rather than a shared session or a thread. A caller
//! that exposes an operation result also exposes these receipts separately; audit
//! loss must not discard a command handle or authorize a mutation resend. This is
//! transport of existing receipts, not an audit ledger or a recovery mechanism.

pub mod failure;

use std::{cell::RefCell, future::Future};

use crate::http::WorkdirAuditFailure;

tokio::task_local! {
    static FAILURES: RefCell<Vec<WorkdirAuditFailure>>;
}

/// Capture receipts from this operation future. Spawned tasks and nested capture
/// scopes do not inherit the caller's receipt slot. Forward a nested result's
/// receipts explicitly when returning it across another boundary.
pub async fn capture<T>(future: impl Future<Output = T>) -> (T, Vec<WorkdirAuditFailure>) {
    FAILURES
        .scope(RefCell::new(Vec::new()), async {
            let result = future.await;
            let failures = FAILURES.with(|failures| failures.take());
            (result, failures)
        })
        .await
}

/// Retain a receipt for the current caller, if it requested capture. Producers
/// remain responsible for their immediate operational log, including cancellation.
pub fn notify(failure: WorkdirAuditFailure) {
    let _ = FAILURES.try_with(|failures| failures.borrow_mut().push(failure));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{WorkdirAuditFailureReason, WorkdirAuditOperationOutcome, WorkdirAuditStage};

    fn receipt(operation_id: &str) -> WorkdirAuditFailure {
        WorkdirAuditFailure {
            operation_id: operation_id.into(),
            stage: WorkdirAuditStage::ResponseReceived,
            operation_outcome: WorkdirAuditOperationOutcome::Succeeded,
            reason: WorkdirAuditFailureReason::AuditStoreUnavailable,
        }
    }

    #[tokio::test]
    async fn concurrent_and_nested_calls_do_not_consume_each_others_receipts() {
        let ((value, nested), outer) = capture(async {
            notify(receipt("outer"));
            let (left, right) = tokio::join!(
                capture(async {
                    notify(receipt("left"));
                    tokio::task::yield_now().await;
                    "left-result"
                }),
                capture(async {
                    tokio::task::yield_now().await;
                    notify(receipt("right"));
                    "right-result"
                })
            );
            assert_eq!(left, ("left-result", vec![receipt("left")]));
            assert_eq!(right, ("right-result", vec![receipt("right")]));
            ("original-success", (left.1, right.1))
        })
        .await;
        assert_eq!(value, "original-success");
        assert_eq!(outer, vec![receipt("outer")]);
        assert_eq!(nested, (vec![receipt("left")], vec![receipt("right")]));
        // A receipt outside a scope must not leak into a later call.
        notify(receipt("unscoped"));
        assert!(capture(async {}).await.1.is_empty());
    }
}
