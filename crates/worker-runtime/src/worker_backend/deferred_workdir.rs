//! Restore retains attachment identity without opening a provider. Failed opens
//! are retryable at operation time; closing an unopened session never opens it.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, broadcast};
use workdir::*;

type OpenSession = dyn Fn() -> Result<WorkdirSessionHandle, String> + Send + Sync;

pub(super) struct DeferredWorkdirSession {
    workdir: Workdir,
    capabilities: WorkdirSessionCapabilities,
    open: Arc<OpenSession>,
    state: Mutex<State>,
    events: broadcast::Sender<CommandEvent>,
}

#[derive(Default)]
struct State {
    session: Option<WorkdirSessionHandle>,
    event_bridge: Option<tokio::task::JoinHandle<()>>,
    closed: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(task) = &self.event_bridge {
            task.abort();
        }
    }
}

impl std::fmt::Debug for DeferredWorkdirSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeferredWorkdirSession")
            .field("workdir", &self.workdir)
            .finish_non_exhaustive()
    }
}

impl DeferredWorkdirSession {
    pub(super) fn handle(
        workdir_id: &str,
        capabilities: WorkdirSessionCapabilities,
        open: impl Fn() -> Result<WorkdirSessionHandle, String> + Send + Sync + 'static,
    ) -> WorkdirSessionHandle {
        Arc::new(Self {
            workdir: Workdir::new(workdir_id),
            capabilities,
            open: Arc::new(open),
            state: Mutex::new(State::default()),
            events: broadcast::channel(1024).0,
        })
    }

    async fn session(&self) -> Result<WorkdirSessionHandle, WorkdirError> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(WorkdirError::SessionClosed);
        }
        if let Some(session) = &state.session {
            return Ok(session.clone());
        }
        let open = self.open.clone();
        let session = tokio::task::spawn_blocking(move || open())
            .await
            .map_err(|_| {
                WorkdirError::Unavailable("Workdir session opener failed".into())
                    .with_failure_reason(WorkdirFailureReason::SessionOpenTaskFailed)
            })?
            .map_err(|_| {
                // The opener's string contract cannot prove a more specific cause.
                WorkdirError::Unavailable("Workdir session could not be opened".into())
                    .with_failure_reason(WorkdirFailureReason::SessionOpenFailed)
            })?;
        if session.workdir().id() != self.workdir.id() {
            if let Err(error) = session.close().await {
                // Retain a failed-close owner for explicit close; never use it.
                state.closed = true;
                state.session = Some(session);
                tracing::warn!(
                    target: "worker_runtime::workdir",
                    event = "workdir_session_identity_cleanup_failed",
                    failure_reason = error.failure_reason().map(WorkdirFailureReason::as_str),
                    denial_reason = error.denial_reason().map(WorkdirDenialReason::as_str),
                    "Mismatched Workdir session cleanup failed"
                );
            }
            return Err(
                WorkdirError::Unavailable("Workdir session identity mismatch".into())
                    .with_failure_reason(WorkdirFailureReason::SessionIdentityMismatch),
            );
        }
        if let Some(mut source) = session.subscribe_command_events() {
            let events = self.events.clone();
            state.event_bridge = Some(tokio::spawn(async move {
                loop {
                    match source.recv().await {
                        Ok(event) => {
                            let _ = events.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }));
        }
        state.session = Some(session.clone());
        Ok(session)
    }
}

macro_rules! impl_deferred_session {
    ($( $method:ident($request:ident: $request_type:ty) -> $result:ty; )*) => {
        #[async_trait]
        impl WorkdirSession for DeferredWorkdirSession {
            fn workdir(&self) -> &Workdir {
                &self.workdir
            }

            fn capabilities(&self) -> WorkdirSessionCapabilities {
                self.capabilities
            }

            $(
                async fn $method(&self, $request: $request_type) -> Result<$result, WorkdirError> {
                    self.session().await?.$method($request).await
                }
            )*

            fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
                Some(self.events.subscribe())
            }

            fn command_snapshot(&self) -> Vec<CommandSnapshot> {
                self.state.try_lock().ok().and_then(|state| {
                    state.session.as_ref().map(|session| session.command_snapshot())
                }).unwrap_or_default()
            }

            async fn close(&self) -> Result<(), WorkdirError> {
                let mut state = self.state.lock().await;
                state.closed = true;
                if let Some(session) = &state.session {
                    session.close().await?;
                }
                if let Some(task) = state.event_bridge.take() {
                    task.abort();
                    let _ = task.await;
                }
                Ok(())
            }
        }
    };
}

impl_deferred_session! {
    authorize_scope_path(request: WorkdirScopeAuthorizationRequest) -> ();
    scope_rules_overlap(request: WorkdirScopeOverlapRequest) -> bool;
    checkout_search(request: CheckoutSearchRequest) -> CheckoutSearchResult;
    checkout_observe(path: WorkdirPath) -> CheckoutObservation;
    checkout_execute(request: CheckoutRequest) -> CheckoutResult;
    stat(request: StatRequest) -> StatResult;
    read(request: ReadRequest) -> ReadResult;
    read_bytes(request: ReadBytesRequest) -> ReadBytesResult;
    write(request: WriteRequest) -> WriteResult;
    edit(request: EditRequest) -> EditResult;
    list(request: ListRequest) -> ListResult;
    glob(request: GlobRequest) -> GlobResult;
    grep(request: GrepRequest) -> GrepResult;
    start_command(request: CommandRequest) -> CommandHandle;
    command_status(handle: CommandHandle) -> CommandStatus;
    command_output(request: CommandOutputRequest) -> CommandOutput;
    cancel_command(handle: CommandHandle) -> ();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn read_request() -> ReadRequest {
        ReadRequest {
            path: WorkdirPath::new("README.md").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        }
    }

    #[tokio::test]
    async fn unopened_session_close_does_not_connect_and_is_terminal() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let session = DeferredWorkdirSession::handle(
            "offline",
            WorkdirSessionCapabilities::READ_ONLY,
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Err("offline".into())
            },
        );
        assert_eq!(session.workdir().id().as_str(), "offline");
        assert!(session.subscribe_command_events().is_some());
        session.close().await.unwrap();
        session.close().await.unwrap();
        assert!(matches!(
            session.read(read_request()).await,
            Err(WorkdirError::SessionClosed)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_open_retries_on_use_and_keeps_read_only_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("README.md"), "restored").unwrap();
        let root = temp.path().to_path_buf();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let session = DeferredWorkdirSession::handle(
            "saved",
            WorkdirSessionCapabilities::READ_ONLY,
            move || {
                if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("offline".into());
                }
                let scope = super::super::runtime_attachment_scope(
                    &root,
                    WorkdirSessionCapabilities::READ_ONLY,
                )?;
                Ok(super::super::runtime_local_workdir_session(
                    "saved",
                    &root,
                    &root,
                    scope,
                    WorkdirSessionCapabilities::READ_ONLY,
                    Default::default(),
                    Vec::new(),
                ))
            },
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        let failed = session.read(read_request()).await.unwrap_err();
        assert!(matches!(
            failed.classification_source(),
            WorkdirError::Unavailable(_)
        ));
        assert_eq!(
            failed.failure_reason(),
            Some(WorkdirFailureReason::SessionOpenFailed)
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            session.read(read_request()).await.unwrap().bytes,
            b"restored"
        );
        assert_eq!(
            session.read(read_request()).await.unwrap().bytes,
            b"restored"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("README.md").unwrap(),
                    content: b"forbidden".to_vec(),
                    expected_hash: None
                })
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(temp.path().join("README.md")).unwrap(),
            b"restored"
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn opener_task_and_open_errors_keep_distinct_safe_causes() {
        for panic in [false, true] {
            let attempts = Arc::new(AtomicUsize::new(0));
            let observed = attempts.clone();
            let session = DeferredWorkdirSession::handle(
                "saved",
                WorkdirSessionCapabilities::READ_ONLY,
                move || {
                    observed.fetch_add(1, Ordering::SeqCst);
                    if panic {
                        panic!(
                            "/host/private bearer-secret command-secret proof-secret key-secret"
                        );
                    }
                    Err("/host/private bearer-secret command-secret proof-secret key-secret".into())
                },
            );
            let error = session.read(read_request()).await.unwrap_err();
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            let reason = if panic {
                WorkdirFailureReason::SessionOpenTaskFailed
            } else {
                WorkdirFailureReason::SessionOpenFailed
            };
            assert_eq!(error.failure_reason(), Some(reason));
            assert_eq!(reason.stage().as_str(), "session_open");
            let transport = workdir::http::WorkdirTransportError::from_workdir_error(&error);
            assert_eq!(transport.code.http_status(), 503);
            assert!(
                !serde_json::to_string(&transport)
                    .unwrap()
                    .contains("secret")
            );
            assert!(!format!("{error:?}").contains("secret"));
            session.close().await.unwrap();
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }

    #[derive(Debug)]
    struct MismatchedSession {
        workdir: Workdir,
        closes: Arc<AtomicUsize>,
        fail_close: bool,
    }

    macro_rules! mismatched_session {
        ($( $method:ident($request:ident: $request_type:ty) -> $result:ty; )*) => {
            #[async_trait]
            impl WorkdirSession for MismatchedSession {
                fn workdir(&self) -> &Workdir { &self.workdir }
                fn capabilities(&self) -> WorkdirSessionCapabilities { WorkdirSessionCapabilities::READ_ONLY }
                $(async fn $method(&self, _: $request_type) -> Result<$result, WorkdirError> {
                    panic!("mismatched session must never be used");
                })*
                async fn close(&self) -> Result<(), WorkdirError> {
                    let attempt = self.closes.fetch_add(1, Ordering::SeqCst);
                    if self.fail_close && attempt == 0 {
                        Err(WorkdirError::Transport("/host/private key-secret".into())
                            .with_failure_reason(WorkdirFailureReason::HttpTransfer))
                    } else { Ok(()) }
                }
            }
        };
    }
    mismatched_session! {
        stat(request: StatRequest) -> StatResult;
        read(request: ReadRequest) -> ReadResult;
        write(request: WriteRequest) -> WriteResult;
        edit(request: EditRequest) -> EditResult;
        list(request: ListRequest) -> ListResult;
        glob(request: GlobRequest) -> GlobResult;
        grep(request: GrepRequest) -> GrepResult;
        start_command(request: CommandRequest) -> CommandHandle;
        command_status(handle: CommandHandle) -> CommandStatus;
        command_output(request: CommandOutputRequest) -> CommandOutput;
        cancel_command(handle: CommandHandle) -> ();
    }

    #[tokio::test]
    async fn identity_mismatch_preserves_primary_cause_and_failed_close_owner() {
        for fail_close in [false, true] {
            let closes = Arc::new(AtomicUsize::new(0));
            let raw: WorkdirSessionHandle = Arc::new(MismatchedSession {
                workdir: Workdir::new("wrong-secret"),
                closes: closes.clone(),
                fail_close,
            });
            let attempts = Arc::new(AtomicUsize::new(0));
            let observed = attempts.clone();
            let session = DeferredWorkdirSession::handle(
                "saved",
                WorkdirSessionCapabilities::READ_ONLY,
                move || {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(raw.clone())
                },
            );
            let error = session.read(read_request()).await.unwrap_err();
            assert_eq!(
                error.failure_reason(),
                Some(WorkdirFailureReason::SessionIdentityMismatch)
            );
            assert!(!format!("{error:?}").contains("secret"));
            assert_eq!(closes.load(Ordering::SeqCst), 1);
            if fail_close {
                assert!(matches!(
                    session.read(read_request()).await,
                    Err(WorkdirError::SessionClosed)
                ));
                assert_eq!(attempts.load(Ordering::SeqCst), 1);
            }
            session.close().await.unwrap();
            assert_eq!(
                closes.load(Ordering::SeqCst),
                if fail_close { 2 } else { 1 }
            );
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }
}
