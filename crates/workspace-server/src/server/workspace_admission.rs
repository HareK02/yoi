//! Workspace lifecycle admission, not operation serialization.
//!
//! Resource owners retain their own locks/transactions. Deletion first stops
//! ordinary admission, then drains admitted requests. Synchronous Runtime
//! callbacks remain admissible while a parent request is still running; once
//! the last ordinary request leaves, callback admission also closes. This
//! avoids both callback/deletion cycles and an unbounded stream of new work
//! starving deletion. All leases, including the deletion fence, are RAII.
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
