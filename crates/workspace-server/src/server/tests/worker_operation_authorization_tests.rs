//! Signed-router component tests for operation use and independent directional control.
//! Subject policy APIs, SQLite grants, Feature installation and public Tools are real.
//! WorkdirlessFixtureRuntime substitutes execution; Accepted is not Controller processing.
//! Run with `cargo test -p yoi-workspace-server --lib worker_operation_authorization`.
use super::*;
use worker::feature::builtin::manage_worker::manage_worker_feature;

struct OperationFixture {
    inner: Fixture,
    stops: Arc<std::sync::atomic::AtomicUsize>,
}

impl std::ops::Deref for OperationFixture {
    type Target = Fixture;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl OperationFixture {
    async fn new() -> Self {
        let mut inner = Fixture::new().await;
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = stops.clone();
        inner.runtime.stop_callback = Some(Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        inner.api.runtime.register_or_replace(inner.runtime.clone());
        Self { inner, stops }
    }
}

const CONTROL_OPERATIONS: &[(&str, &str)] = &[
    ("worker.messaging", "notify"),
    ("worker.send_input", "send_input"),
    ("worker.cancel", "cancel"),
    ("worker.stop", "stop"),
    ("worker.restore", "restore"),
    ("worker.remove", "remove"),
    ("worker.observe", "observe"),
];

fn workspace_permission(operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}

fn operation_permission(operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::WorkerOperation {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}

fn control_permissions() -> Vec<ScopedPermission> {
    CONTROL_OPERATIONS
        .iter()
        .map(|(operation, _)| operation_permission(operation))
        .collect()
}

fn target_permission(operation: &str, target: &RuntimeWorkerRef) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Worker {
            runtime_id: target.runtime_id.clone(),
            worker_id: target.worker_id.clone(),
        },
    }
}

fn subject_value(worker: &RuntimeWorkerRef) -> Value {
    json!({"kind":"runtime_worker", "runtime_id":worker.runtime_id, "worker_id":worker.worker_id})
}

fn peer_grant(
    f: &OperationFixture,
    controller: &RuntimeWorkerRef,
    target: &RuntimeWorkerRef,
    permissions: &[&str],
) -> String {
    let id = Uuid::now_v7().to_string();
    f.api
        .store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: id.clone(),
            controller: controller.clone(),
            subject: target.clone(),
            relation: "peer".into(),
            origin: "explicit_test_delegation".into(),
            permissions: permissions
                .iter()
                .map(|permission| (*permission).into())
                .collect(),
            operation_id: id.clone(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
    id
}

async fn backend_operation(
    f: &OperationFixture,
    controller: &RuntimeWorkerRef,
    target: &RuntimeWorkerRef,
    permission: &str,
) -> (StatusCode, Value) {
    let base = format!(
        "/api/w/{TEST_WORKSPACE_ID}/worker-control/workers/{}/{}",
        target.runtime_id, target.worker_id
    );
    let (path, body) = match permission {
        "notify" | "send_input" => (
            format!("{base}/input"),
            json!({"kind":if permission == "notify" {"notify"} else {"user"}, "content":"authority matrix input"}),
        ),
        "restore" => {
            let observation = f
                .api
                .runtime
                .worker(target)
                .unwrap()
                .restore_observation_token
                .unwrap();
            (
                format!("{base}/restore"),
                json!({"expected_observation_token":observation, "request_id":Uuid::now_v7().to_string()}),
            )
        }
        "observe" => (
            format!("/api/w/{TEST_WORKSPACE_ID}/worker-observation/session"),
            subject_value(target),
        ),
        "remove" => (
            format!("/api/w/{TEST_WORKSPACE_ID}/workers/remove"),
            json!({"target_runtime_id":target.runtime_id,"target_worker_id":target.worker_id,"reason":"authorization matrix"}),
        ),
        "cancel" | "stop" => (
            format!("{base}/{permission}"),
            json!({"reason":"authorization matrix"}),
        ),
        _ => panic!("unknown matrix operation {permission}"),
    };
    let mut request = runtime_source_request(
        &f.identity,
        Some(&controller.worker_id),
        "POST",
        &path,
        serde_json::to_vec(&body).unwrap(),
    );
    if permission == "remove" {
        let proof =
            worker_runtime::auth::RuntimeWorkerMutationSourceSigner::from_identity(&f.identity)
                .issue_worker_remove(
                    "server-test",
                    TEST_WORKSPACE_ID,
                    &controller.worker_id,
                    &target.runtime_id,
                    &target.worker_id,
                    30,
                )
                .unwrap();
        request.headers_mut().insert(
            worker_runtime::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER,
            proof.parse().unwrap(),
        );
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        response(&f.api, request),
    )
    .await
    .unwrap_or_else(|_| panic!("Backend {permission} did not complete"))
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeEffects {
    inputs: usize,
    stops: usize,
    spawns: usize,
    restores: usize,
    workers: usize,
}

fn effects(f: &OperationFixture) -> RuntimeEffects {
    RuntimeEffects {
        inputs: f.runtime.input_requests.lock().unwrap().len(),
        stops: f.stops.load(std::sync::atomic::Ordering::SeqCst),
        spawns: f.runtime.spawn_requests().len(),
        restores: *f.runtime.restore_attempts.lock().unwrap(),
        workers: f.runtime.workers.lock().unwrap().len(),
    }
}

async fn assert_backend_denied(
    f: &OperationFixture,
    controller: &RuntimeWorkerRef,
    target: &RuntimeWorkerRef,
    expected: Option<StatusCode>,
) {
    let before = effects(f);
    let record = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, target)
        .unwrap();
    for (_, permission) in CONTROL_OPERATIONS {
        let (status, body) = backend_operation(f, controller, target, permission).await;
        if let Some(expected) = expected {
            assert_eq!(status, expected, "{permission}: {body}");
        } else {
            assert!(status.is_client_error(), "{permission}: {status} {body}");
        }
        assert_eq!(
            effects(f),
            before,
            "denied {permission} reached Runtime effects"
        );
        assert_eq!(
            f.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, target)
                .unwrap(),
            record,
            "denied {permission} mutated target registry"
        );
    }
}

async fn worker_tool_host(
    root: &Path,
    client: Arc<dyn WorkspaceClient>,
) -> worker::Worker<UnusedModel, session_store::FsStore> {
    worker_tool_host_with_observation(root, client, None).await
}

async fn worker_tool_host_with_observation(
    root: &Path,
    client: Arc<dyn WorkspaceClient>,
    observation: Option<Arc<dyn worker::feature::contracts::WorkerObservationProvider>>,
) -> worker::Worker<UnusedModel, session_store::FsStore> {
    let manifest = worker::WorkerManifest::from_toml(
        r#"
        [worker]
        name = "worker-operation-tools"
        [model]
        scheme = "anthropic"
        model_id = "unused"
        [engine]
        [scope]
        allow = []
        [feature.workdir_catalog]
        enabled = false
        [feature.worker]
        enabled = true
    "#,
    )
    .unwrap();
    let engine =
        agen::Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
            UnusedModel,
        );
    let mut host = worker::Worker::new(
        manifest,
        engine,
        session_store::FsStore::new(root).unwrap(),
        worker::WorkerWorkspaceContext::with_client(
            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
            client.clone(),
        ),
        worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    let mut features = worker::feature::FeatureRegistryBuilder::new()
        .with_module(manage_worker_feature(client, None, true));
    if let Some(provider) = observation {
        features = features.with_module(
            worker::feature::builtin::worker_observation::WorkerObservationFeature::new(provider),
        );
    }
    let report = host.install_features(features);
    assert!(
        !report.has_errors(),
        "Worker Feature must install without granting operation use"
    );
    host.engine().tool_server_handle().flush_pending();
    host
}

fn installed_tool(
    host: &worker::Worker<UnusedModel, session_store::FsStore>,
    name: &str,
) -> Arc<dyn Tool> {
    host.engine()
        .tool_server_handle()
        .get_tool(name)
        .unwrap_or_else(|| panic!("missing installed public {name}"))
        .1
}

fn execute_tool_result(
    host: &worker::Worker<UnusedModel, session_store::FsStore>,
    name: &str,
    input: Value,
) -> std::result::Result<ToolOutput, agen::tool::ToolError> {
    let tool = installed_tool(host, name);
    // The signed fixture transport is synchronous. Leave Tokio's async driver
    // before executing the public Tool, just as the real blocking adapter does.
    tokio::task::block_in_place(|| {
        futures::executor::block_on(
            tool.execute(&input.to_string(), ToolExecutionContext::direct()),
        )
    })
}

async fn execute_tool(
    host: &worker::Worker<UnusedModel, session_store::FsStore>,
    name: &str,
    input: Value,
) -> Value {
    value(execute_tool_result(host, name, input).unwrap_or_else(|error| panic!("{name}: {error}")))
}

fn tool_arguments(name: &str, target: &RuntimeWorkerRef) -> Value {
    let mut args = json!({"subject":subject_value(target)});
    match name {
        "WorkerSendInput" | "WorkerNotify" => args["content"] = json!("public tool input"),
        "WorkerRemove" => args["reason"] = json!("must not remove"),
        "WorkerRestore" => {
            args["expected_observation_token"] = json!("not-used-before-authorization")
        }
        _ => {}
    }
    args
}

// Only the protected Session read is substituted here. Installed observation /
// control Features and signed Host / Backend target checks remain production code.
struct CountedObservation(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl worker::feature::contracts::WorkerObservationProvider for CountedObservation {
    async fn list_worker_sessions(
        &self,
    ) -> std::result::Result<
        Vec<worker::feature::contracts::WorkerObservationSubject>,
        worker::feature::contracts::WorkerObservationError,
    > {
        panic!("known-target capture must not discover the collection")
    }
    async fn capture_worker_session(
        &self,
        _: &worker::feature::contracts::WorkerObservationSubjectRef,
    ) -> std::result::Result<
        worker::feature::contracts::WorkerSessionCapture,
        worker::feature::contracts::WorkerObservationError,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(worker::feature::contracts::WorkerSessionCapture {
            segment_id: "target-session".into(),
            session: protocol::SessionSnapshot {
                pending_submissions: Default::default(),
                entries: vec![protocol::SessionSnapshotEntry {
                    use_policy: None,
                    entry_id: "observed-entry".into(),
                    timestamp: 1,
                    provenance: protocol::SessionEntryProvenance::LegacyUnknown,
                    derived_from: vec![],
                    data: protocol::SessionSnapshotEntryData::Message {
                        role: protocol::SessionMessageRole::User,
                        content: vec![protocol::SessionContentPart::Text {
                            text: "observed input".into(),
                        }],
                    },
                }],
            },
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_observation_needs_only_observe_and_target_grant_not_collection_discovery() {
    let f = OperationFixture::new().await;
    f.assign(vec![workspace_permission("worker.create")], vec![])
        .await;
    let target = f.child("observed-target", vec![]).await;
    f.assign(vec![operation_permission("worker.observe")], vec![])
        .await;
    let provider = Arc::new(CountedObservation(std::sync::atomic::AtomicUsize::new(0)));
    let client = f.client(&f.source);
    let host = worker_tool_host_with_observation(
        &f._dir.path().join("observation-tools"),
        client.clone(),
        Some(provider.clone()),
    )
    .await;
    assert!(
        execute_tool_result(&host, "WorkerList", json!({})).is_err(),
        "observe never grants collection discovery"
    );
    let overview = execute_tool(
        &host,
        "ViewSessionOverview",
        json!({"subject":subject_value(&target)}),
    )
    .await;
    let entry_ref = overview["entries"][0]["entry_ref"].as_str().unwrap();
    let tools = [
        (
            "ViewSessionOverview",
            json!({"subject":subject_value(&target)}),
        ),
        (
            "SearchSessionEntries",
            json!({"subject":subject_value(&target), "query":"observed"}),
        ),
        (
            "ReadSessionEntry",
            json!({"subject":subject_value(&target), "entry_ref":entry_ref}),
        ),
    ];
    client.requests.lock().unwrap().clear();
    for (name, input) in &tools {
        execute_tool(&host, name, input.clone()).await;
    }
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, path)| !path.ends_with("/worker-control/workers")),
        "known-target public observation must not invoke WorkerList"
    );
    let grant = f
        .api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &f.source, &target)
        .unwrap()
        .unwrap();
    for missing in ["operation", "target", "revoked"] {
        match missing {
            "operation" => {
                f.assign(vec![], vec![]).await;
            }
            "target" => {
                f.assign(vec![operation_permission("worker.observe")], vec![])
                    .await;
                f.api
                    .store
                    .revoke_worker_control_grant(
                        TEST_WORKSPACE_ID,
                        &grant.grant_id,
                        &now_registry_timestamp(),
                    )
                    .unwrap();
            }
            "revoked" => {}
            _ => unreachable!(),
        }
        for (name, input) in &tools {
            let mut input = input.clone();
            if missing == "target" {
                input["subject"]["worker_id"] = json!("unknown-target");
            }
            let error = execute_tool_result(&host, name, input)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("not found or is not accessible"),
                "{missing} {name}: {error}"
            );
        }
        assert_eq!(
            provider.0.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "denied {missing} must not read or supply protected Session data"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_control_grant_does_not_replace_exact_operation_use_permission() {
    let f = OperationFixture::new().await;
    f.assign(vec![workspace_permission("worker.create")], vec![])
        .await;
    let target = f.child("permission-matrix-target", vec![]).await;
    assert!(
        f.api
            .store
            .get_active_worker_control_grant(TEST_WORKSPACE_ID, &f.source, &target)
            .unwrap()
            .is_some()
    );
    assert_backend_denied(&f, &f.source, &target, Some(StatusCode::FORBIDDEN)).await;
    // Old collection/Feature permissions and another Workspace never authorize
    // individual current Worker operations, even with a real target grant.
    let mut wrong_workspace = control_permissions();
    for permission in &mut wrong_workspace {
        permission.resource = PermissionResource::WorkerOperation {
            workspace_id: "another-workspace".into(),
        };
    }
    for mut permissions in [
        CONTROL_OPERATIONS
            .iter()
            .map(|(operation, _)| workspace_permission(operation))
            .collect(),
        vec![activation("worker")],
        wrong_workspace,
    ] {
        permissions.push(workspace_permission("worker.create"));
        f.assign(permissions, vec![]).await;
        assert_backend_denied(&f, &f.source, &target, Some(StatusCode::FORBIDDEN)).await;
    }
    f.assign(vec![operation_permission("worker.messaging")], vec![])
        .await;
    assert_eq!(
        backend_operation(&f, &f.source, &target, "notify").await.0,
        StatusCode::OK
    );
    for (_, permission) in CONTROL_OPERATIONS
        .iter()
        .filter(|(_, permission)| *permission != "notify")
    {
        let before = effects(&f);
        let (status, body) = backend_operation(&f, &f.source, &target, permission).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "messaging is not {permission}: {body}"
        );
        assert_eq!(effects(&f), before);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_operation_use_requires_explicit_directional_target_grant_and_revocation_is_live() {
    let f = OperationFixture::new().await;
    let permissions = control_permissions();
    f.assign(
        vec![workspace_permission("worker.create")],
        permissions.clone(),
    )
    .await;
    let sender = f.child("peer-sender", permissions.clone()).await;
    let recipient = f.child("peer-recipient", permissions).await;
    assert_eq!(
        f.check(&sender, &operation_permission("worker.messaging"))
            .await
            .0,
        StatusCode::OK
    );
    assert_backend_denied(&f, &sender, &recipient, None).await;
    let grant = peer_grant(&f, &sender, &recipient, &["notify"]);
    let before = effects(&f);
    let (status, body) = backend_operation(&f, &sender, &recipient, "notify").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "accepted");
    assert_eq!(effects(&f).inputs, before.inputs + 1);
    assert_backend_denied(&f, &recipient, &sender, None).await;
    for (_, permission) in CONTROL_OPERATIONS
        .iter()
        .filter(|(_, permission)| *permission != "notify")
    {
        let before = effects(&f);
        let (status, body) = backend_operation(&f, &sender, &recipient, permission).await;
        assert!(
            status.is_client_error(),
            "notify-only peer {permission}: {body}"
        );
        assert_eq!(effects(&f), before);
    }
    f.api
        .store
        .revoke_worker_control_grant(TEST_WORKSPACE_ID, &grant, &now_registry_timestamp())
        .unwrap();
    assert_eq!(
        f.check(&sender, &operation_permission("worker.messaging"))
            .await
            .0,
        StatusCode::OK,
        "revoking target control does not revoke operation use"
    );
    assert_backend_denied(&f, &sender, &recipient, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_targeted_message_permissions_preserve_target_direction_and_reject_alias_requests() {
    for legacy in ["worker.notify", "worker.result"] {
        let f = OperationFixture::new().await;
        f.assign(vec![workspace_permission("worker.create")], vec![])
            .await;
        let target = f.child("saved-target", vec![]).await;
        let other = f.child("saved-other-target", vec![]).await;
        let saved = target_permission(legacy, &target);
        let policy = f.assign(vec![saved.clone()], vec![]).await;
        assert_eq!(
            policy.execution,
            vec![saved.clone()],
            "saved target must not be rewritten into an operation-wide capability"
        );
        assert_eq!(
            f.check(&f.source, &operation_permission("worker.messaging"))
                .await
                .0,
            StatusCode::OK
        );
        let (status, body) = backend_operation(&f, &f.source, &target, "notify").await;
        assert_eq!(status, StatusCode::OK, "{legacy}: {body}");
        let before = effects(&f);
        let (status, body) = backend_operation(&f, &f.source, &other, "notify").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{legacy} must retain exact target despite another real control grant: {body}"
        );
        assert_eq!(effects(&f), before);
        for request in [
            saved,
            ScopedPermission {
                operation: legacy.into(),
                resource: operation_permission("worker.messaging").resource,
            },
            target_permission("worker.messaging", &target),
        ] {
            let (status, body) = f.check(&f.source, &request).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "storage-only alias/target must not become a public authorization request: {body}"
            );
        }
        assert_backend_denied(&f, &target, &other, None).await;
        assert_eq!(
            open_subjektiv_store(&f.api)
                .unwrap()
                .subject_permissions(&f.subject)
                .unwrap()
                .unwrap(),
            policy
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_worker_tools_register_without_authority_and_enforce_operation_matrix_before_effects()
 {
    let f = OperationFixture::new().await;
    let client = f.client(&f.source);
    let host = worker_tool_host(&f._dir.path().join("unset-worker-tools"), client.clone()).await;
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "registration is local"
    );
    let spawn = json!({"runtime_id":f.source.runtime_id,"operation_id":"public-spawn-denied","profile":"builtin:coder","execution_grant":{"permissions":[],"redelegation":[]}});
    for (name, args) in [("WorkerList", json!({})), ("WorkerSpawn", spawn)] {
        let before = effects(&f);
        assert!(
            execute_tool_result(&host, name, args).is_err(),
            "unset {name}"
        );
        assert_eq!(effects(&f), before);
    }
    f.assign(vec![workspace_permission("worker.create")], vec![])
        .await;
    let target = f.child("public-denied-target", vec![]).await;
    for name in [
        "WorkerNotify",
        "WorkerSendInput",
        "WorkerCancel",
        "WorkerStop",
        "WorkerRestore",
        "WorkerRemove",
    ] {
        let before = effects(&f);
        client.requests.lock().unwrap().clear();
        assert!(
            execute_tool_result(&host, name, tool_arguments(name, &target)).is_err(),
            "missing operation {name}"
        );
        assert_eq!(effects(&f), before);
        assert!(
            client
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|(_, path)| path == &authorization_path()),
            "{name} must refuse before the control backend"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_public_spawn_notify_stop_and_child_reply_use_real_backend_grants() {
    let f = OperationFixture::new().await;
    let reply = operation_permission("worker.messaging");
    let mut permissions = control_permissions();
    permissions.extend([
        workspace_permission("worker.create"),
        workspace_permission("worker.list"),
    ]);
    f.assign(permissions, vec![reply.clone()]).await;
    let client = f.client(&f.source);
    let host = worker_tool_host(&f._dir.path().join("parent-tools"), client).await;
    let created = execute_tool(&host, "WorkerSpawn", json!({
        "runtime_id":f.source.runtime_id,"operation_id":"public-worker-child","profile":"builtin:coder",
        "execution_grant":{"permissions":[reply],"redelegation":[]}
    })).await;
    let child = RuntimeWorkerRef::new(
        created["runtime_id"].as_str().unwrap(),
        created["worker_id"].as_str().unwrap(),
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &child)
            .unwrap()
            .is_some()
    );
    let listed = execute_tool(&host, "WorkerList", json!({})).await;
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["subject"] == subject_value(&child))
    );
    let notified = execute_tool(
        &host,
        "WorkerNotify",
        tool_arguments("WorkerNotify", &child),
    )
    .await;
    assert_eq!(notified["state"], "accepted");
    let child_host = worker_tool_host(&f._dir.path().join("reply-tools"), f.client(&child)).await;
    let before = effects(&f);
    let reply = execute_tool(
        &child_host,
        "WorkerNotify",
        json!({"subject":subject_value(&f.source),"content":"completed child result"}),
    )
    .await;
    assert_eq!(reply["state"], "accepted");
    assert_eq!(effects(&f).inputs, before.inputs + 1);
    for name in [
        "WorkerSendInput",
        "WorkerCancel",
        "WorkerStop",
        "WorkerRestore",
        "WorkerRemove",
    ] {
        let before = effects(&f);
        assert!(
            execute_tool_result(&child_host, name, tool_arguments(name, &f.source)).is_err(),
            "reply cannot authorize {name}"
        );
        assert_eq!(effects(&f), before);
    }
    for (_, permission) in CONTROL_OPERATIONS
        .iter()
        .filter(|(_, permission)| *permission != "notify")
    {
        let before = effects(&f);
        let (status, body) = backend_operation(&f, &child, &f.source, permission).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "direct reply escalation {permission}: {body}"
        );
        assert_eq!(effects(&f), before);
    }
    let reverse = f
        .api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &f.source)
        .unwrap()
        .unwrap();
    f.api
        .store
        .revoke_worker_control_grant(
            TEST_WORKSPACE_ID,
            &reverse.grant_id,
            &now_registry_timestamp(),
        )
        .unwrap();
    let replayed = execute_tool(&host, "WorkerSpawn", json!({
        "runtime_id":f.source.runtime_id,"operation_id":"public-worker-child","profile":"builtin:coder",
        "execution_grant":{"permissions":[operation_permission("worker.messaging")],"redelegation":[]}
    })).await;
    assert_eq!(replayed["worker_id"], created["worker_id"]);
    assert!(
        f.api
            .store
            .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &f.source)
            .unwrap()
            .is_none(),
        "exact Spawn replay must not revive revoked reply control"
    );
    let before = effects(&f);
    assert!(
        execute_tool_result(
            &child_host,
            "WorkerNotify",
            json!({"subject":subject_value(&f.source),"content":"revoked reply"})
        )
        .is_err()
    );
    assert_eq!(
        effects(&f),
        before,
        "revoked public Notify had no Runtime effect"
    );
    let stopped = execute_tool(&host, "WorkerStop", tool_arguments("WorkerStop", &child)).await;
    assert_eq!(stopped["state"], "accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replaced_policy_and_body_never_revive_delegated_worker_control() {
    let f = OperationFixture::new().await;
    let permissions = control_permissions();
    f.assign(
        vec![workspace_permission("worker.create")],
        permissions.clone(),
    )
    .await;
    let child = f.child("old-policy-controller", permissions.clone()).await;
    let target = f.child("policy-control-target", vec![]).await;
    peer_grant(
        &f,
        &child,
        &target,
        &[
            "notify",
            "send_input",
            "cancel",
            "stop",
            "restore",
            "remove",
            "observe",
        ],
    );
    let grant = open_subjektiv_store(&f.api)
        .unwrap()
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        backend_operation(&f, &child, &target, "notify").await.0,
        StatusCode::OK
    );
    f.assign(
        vec![workspace_permission("worker.create")],
        permissions.clone(),
    )
    .await;
    assert_backend_denied(&f, &child, &target, Some(StatusCode::FORBIDDEN)).await;
    // Identical replacement permissions do not refresh the captured policy ID.
    f.assign(
        vec![workspace_permission("worker.create")],
        permissions.clone(),
    )
    .await;
    assert_backend_denied(&f, &child, &target, Some(StatusCode::FORBIDDEN)).await;
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap(),
        Some(grant)
    );
    let current = f.child("old-body-controller", permissions).await;
    peer_grant(
        &f,
        &current,
        &target,
        &[
            "notify",
            "send_input",
            "cancel",
            "stop",
            "restore",
            "remove",
            "observe",
        ],
    );
    assert_eq!(
        backend_operation(&f, &current, &target, "notify").await.0,
        StatusCode::OK
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let attribution = store
        .subject_session_attributions(&f.subject)
        .unwrap()
        .into_iter()
        .find(|attr| attr.runtime_id == f.source.runtime_id && attr.worker_id == f.source.worker_id)
        .unwrap();
    let writer =
        subjektiv::journal::JournalHostContext::validated_session(&store, attribution).unwrap();
    store
        .release_journal_writer(&subjektiv::journal::JournalWriterBinding {
            subject_id: f.subject.clone(),
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone(),
            binding_id: writer.writer_binding_id().unwrap().into(),
        })
        .unwrap();
    f.release_source_lease();
    let Json(replacement) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Replacement control origin".into(),
            singleton_key: None,
            profile: Some("project:authority-source".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: f.subject.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    assert_ne!(replacement.worker_id, f.source.worker_id);
    assert_backend_denied(&f, &current, &target, None).await;
    assert_backend_denied(&f, &f.source, &target, Some(StatusCode::UNAUTHORIZED)).await;
}
