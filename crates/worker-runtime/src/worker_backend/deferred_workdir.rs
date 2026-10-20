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
            .map_err(|_| WorkdirError::Unavailable("Workdir session opener failed".into()))?
            .map_err(WorkdirError::Unavailable)?;
        if session.workdir().id() != self.workdir.id() {
            session.close().await?;
            return Err(WorkdirError::Unavailable(
                "Workdir session identity mismatch".into(),
            ));
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

    fn short_tempdir() -> tempfile::TempDir {
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target");
        std::fs::create_dir_all(&target).unwrap();
        tempfile::Builder::new()
            .prefix("d")
            .rand_bytes(3)
            .tempdir_in(target)
            .unwrap()
    }

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
    async fn router_cleanup_of_unopened_session_never_connects_or_follows_reused_alias() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let session = DeferredWorkdirSession::handle(
            "old",
            WorkdirSessionCapabilities::READ_ONLY,
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Err("offline".into())
            },
        );
        let router = WorkdirSessionRouter::new();
        let alias = WorkdirAttachmentAlias::new("w").unwrap();
        let routed = router.attach(alias.clone(), session.clone()).unwrap();
        assert!(routed.subscribe_command_events().is_some());
        assert!(routed.command_snapshot().is_empty());
        router.close_all().await.unwrap();
        router.close_all().await.unwrap();
        assert!(router.aliases().is_empty());
        assert!(matches!(
            session.read(read_request()).await,
            Err(WorkdirError::SessionClosed)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);

        let new_attempts = Arc::new(AtomicUsize::new(0));
        let new_observed = new_attempts.clone();
        let replacement = DeferredWorkdirSession::handle(
            "new",
            WorkdirSessionCapabilities::READ_ONLY,
            move || {
                new_observed.fetch_add(1, Ordering::SeqCst);
                Err("offline".into())
            },
        );
        let new_routed = router.attach(alias, replacement).unwrap();
        // A retained old connection cannot open the replacement provider.
        assert!(routed.read(read_request()).await.is_err());
        assert_eq!(new_attempts.load(Ordering::SeqCst), 0);
        assert!(matches!(
            new_routed.read(read_request()).await,
            Err(WorkdirError::Unavailable(_))
        ));
        assert_eq!(new_attempts.load(Ordering::SeqCst), 1);
        // Even after a failed open, cleanup must not try connecting again.
        router.close_all().await.unwrap();
        assert_eq!(new_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_open_retries_on_use_and_keeps_read_only_capabilities() {
        let temp = short_tempdir();
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
        assert!(matches!(
            session.read(read_request()).await,
            Err(WorkdirError::Unavailable(_))
        ));
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
}
