//! Admission gate coordinating Workspace deletion with HTTP handler lifetimes.
//!
//! Normal requests acquire leases immediately; independent handlers can run concurrently.
//! Resource owners handle Worker, Workdir, and SQLite exclusion; this gate does not serialize them.
//! Business-operation lock waits do not precede proof verification, avoiding expiry of short-lived
//! proofs while waiting for other requests.
//!
//! Deletion closes normal admission before draining existing handlers. Only Runtime callbacks
//! needed by active parent requests may acquire tracked leases; callback admission also closes
//! when the last normal lease leaves. This prevents deletion blocking callbacks awaited by parents
//! and callbacks indefinitely extending the drain. Callbacks still require authentication and
//! authorization.
//!
//! Drop releases leases and temporary deletion fences. Cancelling a deletion wait reopens normal
//! admission, but durable Workspace state blocks admission after deletion reservation succeeds.
//! Accepted bounded jobs retain the admitted request lease through execution and result cleanup,
//! even after the HTTP waiter is cancelled. Synchronous child submissions retain that source too;
//! detached tasks do not inherit it. Leases do not cover an upgraded WS's entire lifetime or all
//! background work. Only explicitly allowed callback routes may reenter deletion drain.
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};

#[derive(Default)]
struct AdmissionState {
    deleting: bool,
    requests: usize,
    callbacks: usize,
}

#[derive(Default)]
pub(super) struct WorkspaceAdmission {
    state: Mutex<AdmissionState>,
    changed: Notify,
    deletion: Arc<AsyncMutex<()>>,
}

impl WorkspaceAdmission {
    pub(super) fn admit(self: &Arc<Self>, callback: bool) -> Option<RequestLease> {
        let mut state = self.state.lock().expect("Workspace admission poisoned");
        if state.deleting && (!callback || state.requests == 0) {
            return None;
        }
        if callback {
            state.callbacks += 1;
        } else {
            state.requests += 1;
        }
        Some(RequestLease {
            gate: self.clone(),
            callback,
        })
    }

    // Observe parent cleanup without releasing a callback held by the test.
    // A reply can arrive before the DB/Host job drops its retained request lease.
    #[cfg(test)]
    pub(super) async fn wait_for_parent_requests_to_drain(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().expect("Workspace admission poisoned");
                assert!(
                    state.deleting,
                    "parent drain observation requires a deletion fence"
                );
                if state.requests == 0 {
                    return;
                }
            }
            changed.await;
        }
    }

    pub(super) async fn lock_deletion(self: &Arc<Self>) -> DeletionLease {
        let serial = self.deletion.clone().lock_owned().await;
        self.state
            .lock()
            .expect("Workspace admission poisoned")
            .deleting = true;
        // Construct the lease before awaiting: cancellation must reopen admission.
        let lease = DeletionLease {
            gate: self.clone(),
            _serial: serial,
        };
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().expect("Workspace admission poisoned");
                if state.requests == 0 && state.callbacks == 0 {
                    return lease;
                }
            }
            changed.await;
        }
    }
}

pub(super) struct RequestLease {
    gate: Arc<WorkspaceAdmission>,
    callback: bool,
}

impl crate::db_executor::DbOperationLifetime for RequestLease {
    fn retain(&self) -> Box<dyn Send> {
        // This is continuation of an already admitted request, not fresh admission.
        // Retain its drain responsibility before transferring work to the DB owner;
        // dropping/aborting the HTTP waiter must not allow deletion past live SQL.
        let mut state = self
            .gate
            .state
            .lock()
            .expect("Workspace admission poisoned");
        if self.callback {
            state.callbacks += 1;
        } else {
            state.requests += 1;
        }
        Box::new(RequestLease {
            gate: self.gate.clone(),
            callback: self.callback,
        })
    }
}

impl Drop for RequestLease {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .expect("Workspace admission poisoned");
        if self.callback {
            state.callbacks -= 1;
        } else {
            state.requests -= 1;
        }
        self.gate.changed.notify_waiters();
    }
}

pub(super) struct DeletionLease {
    gate: Arc<WorkspaceAdmission>,
    _serial: OwnedMutexGuard<()>,
}

impl Drop for DeletionLease {
    fn drop(&mut self) {
        self.gate
            .state
            .lock()
            .expect("Workspace admission poisoned")
            .deleting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_executor::DbOperationLifetime;

    #[tokio::test]
    async fn parent_drain_waits_for_retained_job_cleanup_without_waiting_for_callbacks() {
        let gate = Arc::new(WorkspaceAdmission::default());
        let parent = gate.admit(false).unwrap();
        let retained_job = parent.retain();
        let mut deletion = Box::pin(gate.lock_deletion());
        assert!(futures::poll!(&mut deletion).is_pending());
        let callback = gate.admit(true).unwrap();
        drop(parent);
        let mut parents_drained = Box::pin(gate.wait_for_parent_requests_to_drain());
        assert!(futures::poll!(&mut parents_drained).is_pending());
        assert!(
            gate.admit(true).is_some(),
            "retained parent may still need callbacks"
        );
        drop(retained_job);
        tokio::time::timeout(std::time::Duration::from_secs(5), parents_drained)
            .await
            .expect("parent cleanup did not notify the drain observer");
        assert!(gate.admit(true).is_none());
        assert!(gate.admit(false).is_none());
        assert!(
            futures::poll!(&mut deletion).is_pending(),
            "callback still owns its lease"
        );
        drop(callback);
        let deletion = tokio::time::timeout(std::time::Duration::from_secs(5), deletion)
            .await
            .expect("callback cleanup did not finish deletion drain");
        assert!(gate.admit(true).is_none());
        drop(deletion);
        assert!(gate.admit(false).is_some());
    }
}
