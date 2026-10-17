//! Controller-owned quiescence with a structural persistence/admission fence.
//!
//! A reservation is acknowledged only by the actual idle Controller. Shared read
//! leases cover input enqueue, durable pending admission, and history writes; the
//! exclusive lease spans the Host's reconciliation and ownership release. No
//! Journal policy or catalog-status inference belongs here.

use session_store::StoreError;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, oneshot};

#[derive(Default)]
pub(crate) struct MutationAuthority {
    barrier: Arc<RwLock<()>>,
    disconnected: AtomicBool,
}
impl MutationAuthority {
    pub(crate) fn enter(&self) -> Result<OwnedRwLockReadGuard<()>, StoreError> {
        let lease = self
            .barrier
            .clone()
            .try_read_owned()
            .map_err(|_| denied())?;
        if self.disconnected.load(Ordering::Acquire) {
            return Err(denied());
        }
        Ok(lease)
    }
}
fn denied() -> StoreError {
    StoreError::Io(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "Subject release has fenced Worker admission and history mutation",
    ))
}

type Reply = oneshot::Sender<Result<SubjectReleaseReservation, String>>;
struct Requests {
    alive: bool,
    queue: VecDeque<Reply>,
}
pub(crate) struct ReleaseControl {
    pub(crate) authority: Arc<MutationAuthority>,
    requests: Mutex<Requests>,
    changed: Notify,
}
impl ReleaseControl {
    pub(crate) fn new(authority: Arc<MutationAuthority>) -> Arc<Self> {
        Arc::new(Self {
            authority,
            requests: Mutex::new(Requests {
                alive: true,
                queue: VecDeque::new(),
            }),
            changed: Notify::new(),
        })
    }
    pub(crate) async fn request(self: &Arc<Self>) -> Result<SubjectReleaseReservation, String> {
        let (reply, receive) = oneshot::channel();
        {
            let mut requests = self
                .requests
                .lock()
                .map_err(|_| "Controller release requests poisoned")?;
            if !requests.alive {
                return Err("Controller is unavailable".into());
            }
            requests.queue.push_back(reply);
        }
        self.changed.notify_one();
        receive
            .await
            .map_err(|_| "Controller closed before release acknowledgement".to_string())?
    }
    pub(crate) async fn notified(&self) {
        self.changed.notified().await;
    }
    pub(crate) fn respond(self: &Arc<Self>, check_quiescent: impl Fn() -> Result<(), String>) {
        let requests = {
            let mut state = self
                .requests
                .lock()
                .expect("Controller release requests poisoned");
            state.queue.drain(..).collect::<Vec<_>>()
        };
        for reply in requests {
            if reply.is_closed() {
                continue;
            }
            let result = self
                .authority
                .barrier
                .clone()
                .try_write_owned()
                .map_err(|_| "Controller admission or history mutation is active".to_string())
                .and_then(|exclusive| {
                    if self.authority.disconnected.load(Ordering::Acquire) {
                        return Err("Controller body is disconnected".into());
                    }
                    check_quiescent()?;
                    Ok(SubjectReleaseReservation {
                        control: self.clone(),
                        _exclusive: exclusive,
                        committed: false,
                    })
                });
            // If the waiter disappeared, dropping the unsent guard cancels the fence.
            let _ = reply.send(result);
        }
    }
    pub(crate) fn reject_busy(self: &Arc<Self>) {
        self.respond(|| Err("Controller is not quiescent Idle".into()));
    }
    pub(crate) fn close(&self) {
        let mut state = self
            .requests
            .lock()
            .expect("Controller release requests poisoned");
        state.alive = false;
        for reply in state.queue.drain(..) {
            let _ = reply.send(Err("Controller is unavailable".into()));
        }
    }
}

pub(crate) struct ControllerLifetime(pub(crate) Arc<ReleaseControl>);
impl Drop for ControllerLifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Exclusive permission to perform Host-side Subject handoff while the old body
/// cannot accept input or change source history. Dropping an uncommitted guard
/// cancels the reservation. Committing permanently revokes body admission; it
/// does not stop the Controller or disable read-only observation and shutdown.
pub struct SubjectReleaseReservation {
    control: Arc<ReleaseControl>,
    _exclusive: OwnedRwLockWriteGuard<()>,
    committed: bool,
}
impl SubjectReleaseReservation {
    pub fn commit_disconnect(&mut self) -> Result<(), String> {
        if self.committed {
            return Ok(());
        }
        let state = self
            .control
            .requests
            .lock()
            .map_err(|_| "Controller release requests poisoned")?;
        if !state.alive {
            return Err("Controller stopped before disconnect commit".into());
        }
        self.control
            .authority
            .disconnected
            .store(true, Ordering::Release);
        self.committed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Structural lease test: the Controller's grant path is production code;
    // the held reader represents an already-entered synchronous persistence call.
    #[tokio::test]
    async fn in_flight_mutation_rejects_release_without_waiting_and_retry_recovers() {
        let authority = Arc::new(MutationAuthority::default());
        let control = ReleaseControl::new(authority.clone());
        let (entered, entry) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let writer = tokio::spawn({
            let authority = authority.clone();
            async move {
                let _lease = authority.enter().unwrap();
                entered.send(()).unwrap();
                finished.await.unwrap();
            }
        });
        entry.await.unwrap();
        let request = tokio::spawn({
            let control = control.clone();
            async move { control.request().await }
        });
        control.notified().await;
        control.respond(|| Ok(()));
        assert!(request.await.unwrap().is_err());
        finish.send(()).unwrap();
        writer.await.unwrap();
        let request = tokio::spawn({
            let control = control.clone();
            async move { control.request().await }
        });
        control.notified().await;
        control.respond(|| Ok(()));
        let guard = request.await.unwrap().unwrap();
        assert!(authority.enter().is_err());
        drop(guard);
        assert!(authority.enter().is_ok());
    }
    #[tokio::test]
    async fn closed_controller_rejects_unanswered_request_and_guard_disconnect() {
        let control = ReleaseControl::new(Arc::default());
        let request = tokio::spawn({
            let control = control.clone();
            async move { control.request().await }
        });
        control.notified().await;
        control.respond(|| Ok(()));
        let mut guard = request.await.unwrap().unwrap();
        control.close();
        assert!(guard.commit_disconnect().is_err());
        assert!(control.request().await.is_err());
    }
}
