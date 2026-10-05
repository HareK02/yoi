//! Deterministic attachment races at provider observation boundaries.
//!
//! These tests deliberately avoid spawned tasks, sleeps, and detach attempts
//! inside a routed operation (which is busy and cannot be detached). Deadline
//! tests use the provider's configurable deadline and a permanently pending
//! observation, without Tokio's test-util clock or the production 30-second wait.
use super::*;

use std::future::pending;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use workdir::{
    CheckoutRequest, CheckoutResult, CommandHandle, CommandOutput, CommandOutputRequest,
    CommandRequest, CommandStatus, EditRequest, EditResult, GlobRequest, GlobResult, GrepRequest,
    GrepResult, ListResult, LocalWorkdirSession, ReadBytesRequest, ReadBytesResult, ReadRequest,
    ReadResult, StatRequest, StatResult, Workdir, WorkdirAttachmentAlias, WorkdirError,
    WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionHandle, WriteRequest, WriteResult,
};

type ObserveHook = Box<dyn FnOnce(&WorkdirPath) + Send>;

/// Shared local provider with a one-shot, synchronous observation boundary.
/// Take the callback out of the lock before invoking it: it may call the router.
struct HookedSession {
    local: LocalWorkdirSession,
    observe_hook: Mutex<Option<ObserveHook>>,
    stall_path: Mutex<Option<WorkdirPath>>,
    stalled_observations: AtomicUsize,
}

impl std::fmt::Debug for HookedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookedSession")
            .field("local", &self.local)
            .field("stall_path", &self.stall_path)
            .field("stalled_observations", &self.stalled_observations)
            .finish_non_exhaustive()
    }
}

impl HookedSession {
    fn new(dir: &TempDir) -> Arc<Self> {
        Arc::new(Self {
            local: LocalWorkdirSession::materialized(
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
                manifest::SharedScope::new(manifest::Scope::writable(dir.path()).unwrap()),
                WorkdirSessionCapabilities::READ_WRITE,
            ),
            observe_hook: Mutex::new(None),
            stall_path: Mutex::new(None),
            stalled_observations: AtomicUsize::new(0),
        })
    }

    fn on_next_observe(&self, hook: impl FnOnce(&WorkdirPath) + Send + 'static) {
        let mut slot = self.observe_hook.lock().unwrap();
        assert!(slot.is_none(), "an observation hook is already armed");
        *slot = Some(Box::new(hook));
    }

    fn stall_observation(&self, path: Option<WorkdirPath>) {
        *self.stall_path.lock().unwrap() = path;
    }
}

#[async_trait]
impl WorkdirSession for HookedSession {
    fn workdir(&self) -> &Workdir {
        self.local.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.local.capabilities()
    }

    async fn authorize_scope_path(
        &self,
        request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        self.local.authorize_scope_path(request).await
    }

    async fn scope_rules_overlap(
        &self,
        request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        self.local.scope_rules_overlap(request).await
    }

    async fn checkout_observe(
        &self,
        path: WorkdirPath,
    ) -> Result<CheckoutObservation, WorkdirError> {
        let hook = { self.observe_hook.lock().unwrap().take() };
        if let Some(hook) = hook {
            hook(&path);
        }
        let should_stall = {
            self.stall_path
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|stall| stall.as_str() == path.as_str())
        };
        if should_stall {
            self.stalled_observations.fetch_add(1, Ordering::SeqCst);
            return pending().await;
        }
        self.local.checkout_observe(path).await
    }

    async fn checkout_execute(
        &self,
        request: CheckoutRequest,
    ) -> Result<CheckoutResult, WorkdirError> {
        self.local.checkout_execute(request).await
    }

    async fn checkout_search(
        &self,
        request: workdir::CheckoutSearchRequest,
    ) -> Result<workdir::CheckoutSearchResult, WorkdirError> {
        self.local.checkout_search(request).await
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.local.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.local.read(request).await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<ReadBytesResult, WorkdirError> {
        self.local.read_bytes(request).await
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.local.write(request).await
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        self.local.edit(request).await
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.local.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.local.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.local.grep(request).await
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.local.start_command(request).await
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.local.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.local.command_output(request).await
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.local.cancel_command(handle).await
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        self.local.close().await
    }
}

fn provider(router: Arc<WorkdirSessionRouter>) -> Arc<Provider> {
    Arc::new(Provider {
        router,
        tracker: tools::Tracker::new(),
        permissions: None,
        incarnation: "checkout-race-tests".into(),
        permits: Arc::new(tokio::sync::Semaphore::new(16)),
        deadline: DEADLINE,
    })
}

fn deadline_provider(router: Arc<WorkdirSessionRouter>) -> Arc<Provider> {
    let mut p = provider(router);
    let settings = Arc::get_mut(&mut p).unwrap();
    // Allow ample time for the tiny local search before stalling post-processing.
    // The outer test watchdog is intentionally much more generous than this.
    settings.deadline = Duration::from_millis(100);
    settings.permits = Arc::new(tokio::sync::Semaphore::new(1));
    p
}

fn context() -> WipCallContext {
    WipCallContext {
        execution: Default::default(),
        security_context: "checkout-race-tests".into(),
    }
}

fn assert_validator_mismatch<T>(result: Result<T, WipOperationError>) {
    match result {
        Err(WipOperationError::Protocol(e)) => {
            assert_eq!(e.code, ProtocolErrorCode::ValidatorMismatch);
        }
        Err(_) => panic!("expected ValidatorMismatch, received a non-protocol error"),
        Ok(_) => panic!("a changed attachment snapshot must not be published"),
    }
}

#[tokio::test]
async fn collection_list_rejects_alias_attached_during_observation_and_retries_coherently() {
    let main_dir = TempDir::new().unwrap();
    let extra_dir = TempDir::new().unwrap();
    let main = HookedSession::new(&main_dir);
    let extra: WorkdirSessionHandle = HookedSession::new(&extra_dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(WorkdirAttachmentAlias::new("main").unwrap(), main.clone())
        .unwrap();
    let p = provider(router.clone());
    let before = p.collection().object.validator;
    let hook_router = router.clone();
    main.on_next_observe(move |path| {
        assert!(path.is_root(), "collection enumeration observes the root");
        hook_router
            .attach(WorkdirAttachmentAlias::new("extra").unwrap(), extra)
            .unwrap();
    });

    let collection = Collection {
        provider: p.clone(),
    };
    assert_validator_mismatch(collection.call("list", &BTreeMap::new(), context()).await);
    assert_ne!(before, p.collection().object.validator);
    assert!(router.resolve(Some("extra")).is_ok(), "the hook must run");

    // The callback was consumed. A fresh snapshot includes both aliases, with
    // identity metadata from exactly the router state being enumerated.
    let output = match collection.call("list", &BTreeMap::new(), context()).await {
        Ok(output) => output,
        Err(_) => panic!("the stable retry must succeed"),
    };
    let value = wip_to_json(&output.value).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for alias in ["extra", "main"] {
        let item = items
            .iter()
            .find(|item| item["alias"] == json!(alias))
            .expect("each attached alias must appear in the stable snapshot");
        let selected = router.resolve(Some(alias)).unwrap();
        assert_eq!(item["alias"], json!(alias));
        assert_eq!(item["path"], json!(checkout_root(alias)));
        assert_eq!(item["slug"], json!(encode_identity(alias)));
        assert_eq!(item["generation"], json!(selected.generation));
        assert_eq!(
            item["working_directory_id"],
            json!(selected.session.workdir().id().as_str())
        );
    }
}

#[tokio::test]
async fn root_enumeration_rejects_alias_attached_during_observation() {
    let main_dir = TempDir::new().unwrap();
    let extra_dir = TempDir::new().unwrap();
    let main = HookedSession::new(&main_dir);
    let extra: WorkdirSessionHandle = HookedSession::new(&extra_dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(WorkdirAttachmentAlias::new("main").unwrap(), main.clone())
        .unwrap();
    let hook_router = router.clone();
    main.on_next_observe(move |_| {
        hook_router
            .attach(WorkdirAttachmentAlias::new("extra").unwrap(), extra)
            .unwrap();
    });
    let subtree = Arc::new(provider(router.clone()));

    match subtree.children(ROOT).await {
        Err(e) => assert_eq!(e.code, ProtocolErrorCode::ValidatorMismatch),
        Ok(_) => panic!("enumeration must reject a mixed attachment snapshot"),
    }
    assert!(router.resolve(Some("extra")).is_ok(), "the hook must run");
    let mut paths = subtree.children(ROOT).await.unwrap();
    paths.sort();
    assert_eq!(paths, vec![checkout_root("extra"), checkout_root("main")]);
}

#[tokio::test]
async fn file_handler_connection_fences_detach_and_same_session_alias_reuse() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.txt"), "unchanged\n").unwrap();
    let session: WorkdirSessionHandle = HookedSession::new(&dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    let alias = WorkdirAttachmentAlias::new("main").unwrap();
    router.attach(alias.clone(), session.clone()).unwrap();
    let selected = router.resolve(Some("main")).unwrap();
    let target = WorkdirPath::new("file.txt").unwrap();
    let observation = selected
        .session
        .checkout_observe(target.clone())
        .await
        .unwrap();
    let mut handler = FileHandler {
        provider: provider(router.clone()),
        alias: "main".into(),
        generation: selected.generation,
        workdir: selected.session.workdir().id().as_str().into(),
        target,
        observation,
    };
    drop(selected);
    assert!(handler.check_connection().is_ok());

    // No routed operation is active here. Reuse the exact same provider identity
    // to ensure generation, not changed workdir ID/content, causes rejection.
    // Detach may close the local session; no subsequent filesystem operation is
    // made on it. This test only exercises the router's connection identity.
    router.detach(&alias).await.unwrap();
    assert_validator_mismatch(handler.check_connection());
    router.attach(alias, session).unwrap();
    let replacement = router.resolve(Some("main")).unwrap();
    assert_ne!(replacement.generation, handler.generation);
    assert_eq!(replacement.session.workdir().id().as_str(), handler.workdir);
    assert_validator_mismatch(handler.check_connection());

    // Positive control: the new generation is accepted with the same identity.
    handler.generation = replacement.generation;
    assert!(handler.check_connection().is_ok());
}

fn assert_deadline<T>(result: Result<T, WipOperationError>, message: &str) {
    match result {
        Err(WipOperationError::Protocol(e)) => {
            assert_eq!(e.code, ProtocolErrorCode::ResourceLimitExceeded);
            assert_eq!(e.message, message);
        }
        Err(_) => panic!("expected a protocol deadline error"),
        Ok(_) => panic!("a stalled provider observation must time out"),
    }
}

fn assert_permit_released(p: &Provider) {
    assert_eq!(p.permits.available_permits(), 1);
    let permit = p
        .permits
        .try_acquire()
        .expect("the timed-out operation must release its permit");
    drop(permit);
}

#[tokio::test]
async fn collection_list_and_root_enumeration_bound_stalled_observation_and_release_permit() {
    let dir = TempDir::new().unwrap();
    let session = HookedSession::new(&dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session.clone(),
        )
        .unwrap();
    session.stall_observation(Some(WorkdirPath::new("").unwrap()));
    let p = deadline_provider(router);
    let collection = Collection {
        provider: p.clone(),
    };

    // The outer watchdog prevents a missing production timeout from hanging the
    // suite. The exact protocol message proves the inner enumeration timed out.
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        collection.call("list", &BTreeMap::new(), context()),
    )
    .await
    .expect("Collection.list must be bounded by the provider deadline");
    assert_deadline(result, "checkout enumeration deadline");
    assert_eq!(session.stalled_observations.load(Ordering::SeqCst), 1);
    assert_permit_released(&p);

    // Exercise the subtree entry point independently with the stall still armed.
    let subtree = Arc::new(p.clone());
    let result = tokio::time::timeout(Duration::from_secs(2), subtree.children(ROOT))
        .await
        .expect("root enumeration must be bounded by the provider deadline")
        .map_err(WipOperationError::Protocol);
    assert_deadline(result, "checkout enumeration deadline");
    assert_eq!(session.stalled_observations.load(Ordering::SeqCst), 2);
    assert_permit_released(&p);

    // Removing the stall makes the same provider usable immediately, without
    // resetting its semaphore or waiting for any background task to finish.
    session.stall_observation(None);
    let paths = tokio::time::timeout(Duration::from_secs(2), subtree.children(ROOT))
        .await
        .expect("enumeration must remain usable after cancellation")
        .unwrap();
    assert_eq!(paths, vec![checkout_root("main")]);
    assert_permit_released(&p);
}

#[tokio::test]
async fn grep_post_search_link_observation_is_deadline_bounded_and_releases_permit() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    let session = HookedSession::new(&dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session.clone(),
        )
        .unwrap();
    let p = deadline_provider(router);
    let root = checkout_root("main");
    let projection = p.node(&root).await.unwrap().unwrap();

    // Directory observations and the normal local grep/checkout execution can
    // complete. Only publishing the typed search result path gets stuck.
    session.stall_observation(Some(WorkdirPath::new("a.txt").unwrap()));
    let arguments = BTreeMap::from([
        ("pattern".into(), Value::String("needle".into())),
        (
            "output_mode".into(),
            Value::String("files_with_matches".into()),
        ),
    ]);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        projection.handler.call("grep", &arguments, context()),
    )
    .await
    .expect("post-search link observations must share the operation deadline");
    assert_deadline(result, "checkout result deadline");
    assert_eq!(session.stalled_observations.load(Ordering::SeqCst), 1);
    assert_permit_released(&p);

    // A successful retry both checks cancellation cleanup and proves that the
    // delegated search produces the typed path consumed by post-processing.
    session.stall_observation(None);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        projection.handler.call("grep", &arguments, context()),
    )
    .await
    .expect("grep must remain usable after cancelling result publication");
    let output = match result {
        Ok(output) => output,
        Err(_) => panic!("grep must succeed once the link observation is unstalled"),
    };
    let value = wip_to_json(&output.value).unwrap();
    assert_eq!(value["items"], json!([{"path": format!("{root}/a.txt")}]));
    assert_permit_released(&p);
}
