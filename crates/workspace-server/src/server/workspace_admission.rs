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
//! Leases cover HTTP responses, not the full lifetime of upgraded WS or background work.
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
