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
    GrepResult, ListRequest, ListResult, LocalWorkdirSession, ReadBytesRequest, ReadBytesResult,
    ReadRequest, ReadResult, StatRequest, StatResult, Workdir, WorkdirAttachmentAlias,
    WorkdirError, WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest, WorkdirSession,
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
    searches: AtomicUsize,
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
            searches: AtomicUsize::new(0),
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
        self.searches.fetch_add(1, Ordering::SeqCst);
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

#[tokio::test]
async fn checkout_wip_file_operations_share_core_and_keep_prior_read_boundary() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("text.txt"), "旧 旧\n").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            HookedSession::new(&dir),
        )
        .unwrap();
    let provider = provider(router);
    let route = format!("{}/text.txt", checkout_root("main"));
    let projection = provider.node(&route).await.unwrap().unwrap();
    let invalid = BTreeMap::from([
        ("old_string".into(), Value::String("旧".into())),
        ("new_string".into(), Value::String("旧".into())),
    ]);
    assert!(
        matches!(projection.handler.call("edit", &invalid, context()).await,
        Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::InvalidArguments)
    );
    let edit = BTreeMap::from([
        ("old_string".into(), Value::String("旧".into())),
        ("new_string".into(), Value::String("新".into())),
        ("replace_all".into(), Value::Boolean(true)),
    ]);
    // Valid shared arguments do not bypass the provider's Read requirement.
    assert!(
        projection
            .handler
            .call("edit", &edit, context())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("text.txt")).unwrap(),
        "旧 旧\n"
    );
    projection
        .handler
        .call("read", &BTreeMap::new(), context())
        .await
        .unwrap_or_else(|_| panic!("shared read failed"));
    projection
        .handler
        .call("edit", &edit, context())
        .await
        .unwrap_or_else(|_| panic!("shared edit failed after read"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("text.txt")).unwrap(),
        "新 新\n"
    );
    let projection = provider.node(&route).await.unwrap().unwrap();
    projection
        .handler
        .call(
            "write",
            &BTreeMap::from([("content".into(), Value::String("written\n".into()))]),
            context(),
        )
        .await
        .unwrap_or_else(|_| panic!("shared write failed after read/edit"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("text.txt")).unwrap(),
        "written\n"
    );
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
async fn grep_returns_typed_entry_without_observing_stalled_result() {
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

    // Directory/search can complete. The result path must never be observed
    // during result publication, even when it would block indefinitely.
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
    .expect("completed search must return without acquiring its entries");
    let output = result.unwrap_or_else(|_| panic!("successful search must not observe each entry"));
    assert_eq!(
        wip_to_json(&output.value).unwrap()["items"],
        json!([{"entry": format!("{root}/a.txt")}])
    );
    assert_eq!(session.stalled_observations.load(Ordering::SeqCst), 0);
    assert_permit_released(&p);

    // A second explicit search produces the same typed coordinate, without
    // acquiring it even when it could resolve successfully.
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
    assert_eq!(value["items"], json!([{"entry": format!("{root}/a.txt")}]));
    assert_permit_released(&p);
}

#[tokio::test]
async fn checkout_content_children_never_call_provider_list_even_for_deep_paths() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    for n in 0..1100 {
        std::fs::write(dir.path().join(format!("file-{n}")), "fixture").unwrap();
    }
    let session = HookedSession::new(&dir);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session.clone(),
        )
        .unwrap();
    let subtree = Arc::new(provider(router));
    assert_eq!(
        subtree.children(ROOT).await.unwrap(),
        vec![checkout_root("main")]
    );
    for path in ["/checkouts/main", "/checkouts/main/nested/deep"] {
        assert!(subtree.publication(path).await.unwrap().is_some());
        assert!(subtree.children(path).await.unwrap().is_empty());
    }
    assert_eq!(session.searches.load(Ordering::SeqCst), 0);
}

/// Deterministic adapter fixture for the production post-success publication
/// boundary. Execute through the real provider/Tools, then detach after the
/// routed operation guard has finished, before publishing the committed result.
/// No race timing or production-only test hook is needed.
struct DetachBeforePublication {
    file: FileHandler,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl WipOperationHandler for DetachBeforePublication {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let tool = match operation {
            "write" => "Write",
            "edit" => "Edit",
            "create_file" => "Create",
            _ => panic!("mutation fixture only"),
        };
        let result = tools::execute_checkout_tool(
            self.file.provider.router.clone(),
            self.file.provider.tracker.clone(),
            &self.file.alias,
            self.file.generation,
            self.file.target.clone(),
            self.file.observation.validator.clone(),
            tool,
            wip_to_json(&Value::Record(arguments.clone())).unwrap(),
            context.execution,
        )
        .await
        .map_err(map_tool_error)?;
        self.file
            .provider
            .router
            .detach(&WorkdirAttachmentAlias::new(&self.file.alias).unwrap())
            .await
            .unwrap();
        self.file.publish_result(tool, result)
    }
}

#[tokio::test]
async fn committed_checkout_mutation_losing_connection_is_unknown_not_rejected_or_replayed() {
    use crate::wip::{WipAuditOutcome, WipRuntime};
    for (operation, arguments, target, result_path, expected) in [
        (
            "write",
            json!({"content":"saved\n"}),
            "a.txt",
            "a.txt",
            "saved\n",
        ),
        (
            "edit",
            json!({"old_string":"old", "new_string":"changed"}),
            "a.txt",
            "a.txt",
            "changed\n",
        ),
        (
            "create_file",
            json!({"path":"new.txt", "content":"created\n"}),
            "dir",
            "dir/new.txt",
            "created\n",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("dir")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
        let router = Arc::new(WorkdirSessionRouter::new());
        router
            .attach(
                WorkdirAttachmentAlias::new("main").unwrap(),
                HookedSession::new(&dir),
            )
            .unwrap();
        let p = provider(router.clone());
        if operation != "create_file" {
            p.node("/checkouts/main/a.txt")
                .await
                .unwrap()
                .unwrap()
                .handler
                .call("read", &BTreeMap::new(), context())
                .await
                .unwrap_or_else(|_| panic!("prior Read failed"));
        }
        let selected = router.resolve(Some("main")).unwrap();
        let target = WorkdirPath::new(target).unwrap();
        let observation = selected
            .session
            .checkout_observe(target.clone())
            .await
            .unwrap();
        let file = FileHandler {
            provider: p.clone(),
            alias: "main".into(),
            generation: selected.generation,
            workdir: selected.session.workdir().id().as_str().into(),
            target,
            observation,
        };
        drop(selected);
        let calls = Arc::new(AtomicUsize::new(0));
        let route = object_path("main", &file.target);
        let mut projection = p.node(&route).await.unwrap().unwrap();
        let interface = projection.interface.clone();
        projection.handler = Arc::new(DetachBeforePublication {
            file,
            calls: calls.clone(),
        });
        let mut registry = WipMountRegistry::new();
        registry
            .allocate_namespace("checkout", "checkouts")
            .unwrap();
        // Static observation metadata isolates the response boundary; real
        // execution still uses checked provider authority and prior Read.
        registry
            .mount(p.node("/checkouts/main").await.unwrap().unwrap())
            .unwrap();
        registry.mount(projection).unwrap();
        let runtime = WipRuntime::from_mounts(registry, "post-commit-test".into()).unwrap();
        let error = runtime
            .invoke(
                route,
                interface,
                operation.into(),
                arguments,
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("outcome unknown"),
            "{operation}: {error}"
        );
        assert_eq!(
            runtime.audit().last().unwrap().outcome,
            WipAuditOutcome::OutcomeUnknown
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(result_path)).unwrap(),
            expected
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "completed mutation must never replay"
        );
        assert_eq!(runtime.metrics().operation_round_trips, 1);
    }
}

#[tokio::test]
async fn readonly_checkout_result_losing_connection_keeps_validator_mismatch() {
    let dir = TempDir::new().unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    let alias = WorkdirAttachmentAlias::new("main").unwrap();
    router
        .attach(alias.clone(), HookedSession::new(&dir))
        .unwrap();
    let selected = router.resolve(Some("main")).unwrap();
    let file = FileHandler {
        provider: provider(router.clone()),
        alias: "main".into(),
        generation: selected.generation,
        workdir: selected.session.workdir().id().as_str().into(),
        target: WorkdirPath::root(),
        observation: selected
            .session
            .checkout_observe(WorkdirPath::root())
            .await
            .unwrap(),
    };
    drop(selected);
    router.detach(&alias).await.unwrap();
    for tool in ["Read", "List", "Glob", "Grep"] {
        let result = tools::CheckoutToolOutput {
            output: agen::tool::ToolOutput {
                summary: "finished".into(),
                content: None,
                attachments: vec![],
            },
            paths: vec![],
            listing: None,
            validator: None,
        };
        assert_validator_mismatch(file.publish_result(tool, result));
    }
}
