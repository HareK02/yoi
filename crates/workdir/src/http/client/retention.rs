//! The Runtime connection retains unconfirmed remote sessions, not their users.
//! Last-user Drop starts best-effort teardown. Failed teardown remains owned by
//! this connection and ordinary Workdir cleanup retries it; live handles are never
//! swept. Open runs independently of its waiter so cancellation cannot lose its
//! recovery identity or turn an unknown response into a fresh open.

use super::*;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};

/// Exact Backend attachment lifetime, distinct from Runtime-local admission ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWorkdirSessionOwner {
    pub worker_runtime_id: String,
    pub worker_id: String,
    pub attachment_connection_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct RemoteWorkdirSessionRegistry {
    sessions: Arc<Mutex<HashMap<WorkdirSessionId, TrackedSession>>>,
}

#[derive(Debug)]
struct TrackedSession {
    close: Arc<RemoteSessionClose>,
    lifetime: Weak<RemoteSessionLifetime>,
    opening: Option<tokio::sync::watch::Receiver<bool>>,
}

pub(super) struct RemoteSessionClose {
    client: Client,
    base_url: Url,
    authorization: Arc<dyn WorkdirHttpAuthorization>,
    pub(super) session_id: WorkdirSessionId,
    workdir_id: WorkdirId,
    owner: Option<RemoteWorkdirSessionOwner>,
    pub(super) closed: AtomicBool,
    pub(super) dispatched: AtomicBool,
    close_lock: tokio::sync::Mutex<()>,
    registry: Weak<Mutex<HashMap<WorkdirSessionId, TrackedSession>>>,
}

impl std::fmt::Debug for RemoteSessionClose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSessionClose")
            .field("session_id", &self.session_id)
            .field("workdir_id", &self.workdir_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct RemoteSessionLifetime(Arc<RemoteSessionClose>);

impl Drop for RemoteSessionLifetime {
    fn drop(&mut self) {
        let close = self.0.clone();
        if !close.dispatched.load(Ordering::Acquire) {
            close.forget();
        } else if !close.closed.load(Ordering::Acquire) {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Err(error) = close.close().await {
                        tracing::warn!(session_id = close.session_id.as_str(),
                            workdir_id = close.workdir_id.as_str(), reason = ?error.failure_reason(),
                            "abandoned remote Workdir session close remains unconfirmed");
                    }
                });
            }
        }
    }
}

impl RemoteSessionClose {
    fn forget(&self) {
        if let Some(registry) = self.registry.upgrade() {
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.session_id);
        }
    }

    pub(super) async fn close(&self) -> Result<(), WorkdirError> {
        let _guard = self.close_lock.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let url = endpoint(
            &self.base_url,
            &["v1", "workdir-sessions", self.session_id.as_str()],
        )?;
        let token = self
            .authorization
            .bearer_token("DELETE".into(), url.path().into(), Vec::new())
            .await?;
        let response = self
            .client
            .delete(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(http_unavailable)?;
        if !response.status().is_success() {
            return Err(decode_error(response).await);
        }
        self.closed.store(true, Ordering::Release);
        if let Some(registry) = self.registry.upgrade() {
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.session_id);
        }
        tracing::debug!(
            session_id = self.session_id.as_str(),
            workdir_id = self.workdir_id.as_str(),
            "remote Workdir session close confirmed"
        );
        Ok(())
    }
}

impl RemoteWorkdirSessionRegistry {
    pub fn is_empty(&self) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
    // Shared production setup: retention must precede both authorization and HTTP
    // dispatch. Keeping it separate also lets boundary tests seed an open session
    // without duplicating the ownership and Drop implementation.
    pub(super) fn retain(
        &self,
        client: Client,
        base_url: Url,
        authorization: Arc<dyn WorkdirHttpAuthorization>,
        workdir_id: WorkdirId,
        request: &OpenWorkdirSessionRequest,
        owner: Option<RemoteWorkdirSessionOwner>,
    ) -> Result<(Arc<RemoteSessionClose>, Arc<RemoteSessionLifetime>), WorkdirError> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions.contains_key(&request.session_id) {
            return Err(WorkdirError::Conflict(
                "Workdir session identity is already retained".into(),
            ));
        }
        let close = Arc::new(RemoteSessionClose {
            client: client.clone(),
            base_url: base_url.clone(),
            authorization: authorization.clone(),
            session_id: request.session_id.clone(),
            workdir_id: workdir_id.clone(),
            owner,
            closed: AtomicBool::new(false),
            dispatched: AtomicBool::new(false),
            close_lock: tokio::sync::Mutex::new(()),
            registry: Arc::downgrade(&self.sessions),
        });
        let lifetime = Arc::new(RemoteSessionLifetime(close.clone()));
        sessions.insert(
            request.session_id.clone(),
            TrackedSession {
                close: close.clone(),
                lifetime: Arc::downgrade(&lifetime),
                opening: None,
            },
        );
        Ok((close, lifetime))
    }

    pub async fn open_with_authorization(
        &self,
        client: Client,
        base_url: Url,
        authorization: Arc<dyn WorkdirHttpAuthorization>,
        workdir_id: WorkdirId,
        request: OpenWorkdirSessionRequest,
        timeout: std::time::Duration,
        owner: Option<RemoteWorkdirSessionOwner>,
    ) -> Result<RemoteWorkdirSession, WorkdirError> {
        let (close, lifetime) = self.retain(
            client.clone(),
            base_url.clone(),
            authorization.clone(),
            workdir_id.clone(),
            &request,
            owner,
        )?;
        tracing::debug!(session_id = request.session_id.as_str(), workdir_id = workdir_id.as_str(),
            owner_worker_id = ?request.owner_worker_id, "remote Workdir session open retained");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (opened, opening) = tokio::sync::watch::channel(false);
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&request.session_id)
            .expect("prepared open retained")
            .opening = Some(opening);
        // The connection's recovery registry survives a cancelled Backend waiter.
        let registry = self.clone();
        let task = async move {
            let result = tokio::time::timeout(timeout, RemoteWorkdirSession::open_retained(
                client,
                base_url,
                authorization,
                workdir_id,
                request,
                close,
                lifetime,
            )).await.unwrap_or_else(|_| Err(WorkdirError::OutcomeUnknown(
                "Workdir session open is unconfirmed; its identity remains retained for teardown".into())));
            let _ = sender.send(result);
            opened.send_replace(true);
            drop(registry);
        };
        tokio::spawn(tracing::Instrument::in_current_span(task));
        receiver
            .await
            .map_err(|_| WorkdirError::Unavailable("Workdir session open task terminated".into()))?
    }

    /// Terminal Detach/Remove owns every prepared session for this exact lifetime,
    /// including an open whose response never reached the Backend attachment cache.
    pub async fn close_owned(
        &self,
        worker_runtime_id: &str,
        worker_id: &str,
        connection_id: Option<&str>,
    ) -> Result<(), WorkdirError> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|session| {
                session.close.owner.as_ref().is_some_and(|owner| {
                    owner.worker_runtime_id == worker_runtime_id
                        && owner.worker_id == worker_id
                        && connection_id.is_none_or(|id| id == owner.attachment_connection_id)
                })
            })
            .map(|session| (session.close.clone(), session.opening.clone()))
            .collect::<Vec<_>>();
        for (close, opening) in sessions {
            if let Some(mut completion) = opening {
                while !*completion.borrow_and_update() {
                    if completion.changed().await.is_err() {
                        break;
                    }
                }
            }
            close.close().await?;
        }
        Ok(())
    }

    /// Retry only sessions with no remaining user, before ordinary provider cleanup.
    pub async fn close_abandoned(&self, workdir_id: &str) -> Result<(), WorkdirError> {
        // A cancelled waiter's independent open still owns resources. Join its
        // bounded completion before deciding whether the result has a live user.
        let opening = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|session| session.close.workdir_id.as_str() == workdir_id)
            .filter_map(|session| session.opening.clone())
            .collect::<Vec<_>>();
        for mut completion in opening {
            while !*completion.borrow_and_update() {
                if completion.changed().await.is_err() {
                    break;
                }
            }
        }
        let abandoned = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|session| {
                // Do not temporarily upgrade while holding the registry mutex:
                // dropping that temporary could run the final lifetime's Drop,
                // which forgets an undispatched open using the same mutex.
                session.close.workdir_id.as_str() == workdir_id
                    && session.lifetime.strong_count() == 0
                    && session.close.dispatched.load(Ordering::Acquire)
            })
            .map(|session| session.close.clone())
            .collect::<Vec<_>>();
        for close in abandoned {
            close.close().await?;
        }
        Ok(())
    }
}
