#[derive(Clone)]
struct ConfigNoopLlmClient {
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
    calls: Arc<AtomicU64>,
}
#[async_trait]
impl agen::llm_client::LlmClient for ConfigNoopLlmClient {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = std::result::Result<
                            agen::llm_client::event::Event,
                            agen::llm_client::ClientError,
                        >,
                    > + Send,
            >,
        >,
        agen::llm_client::ClientError,
    > {
        let grant = self
            .api
            .store
            .current_workspace_config_grant(TEST_WORKSPACE_ID, &self.worker)
            .unwrap()
            .unwrap();
        assert!(
            self.api
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &self.worker)
                .unwrap()
                .iter()
                .any(|link| link.unlinked_at.is_none()
                    && link.alias == "workspace-config"
                    && link.workdir_id == grant.working_directory_id),
            "real Backend attachment must precede the dependent LLM request"
        );
        assert!(
            format!("{:?}", request.items).contains("Workspace configuration is available"),
            "selected handler's preparation context must reach the actual LLM request"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::empty()))
    }
}
// Real production Feature/WIP adapters over a signed in-process Backend HTTP router.
// No fake config provider, copied tree, or fabricated attachment ledger is used.
#[derive(Clone)]
struct ConfigRouterClient {
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
    identity: Arc<RuntimeIdentityMaterial>,
    runtime: tokio::runtime::Handle,
    paths: Arc<Mutex<Vec<String>>>,
    drop_next_commit_response: Arc<std::sync::atomic::AtomicBool>,
}
impl std::fmt::Debug for ConfigRouterClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Authenticated ConfigRouterClient")
    }
}
impl worker::WorkspaceClient for ConfigRouterClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(TEST_WORKSPACE_ID)
    }
    fn kind(&self) -> &str {
        "authenticated-backend-router"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: worker::WorkspaceRequest,
    ) -> std::result::Result<worker::WorkspaceResponse, worker::WorkspaceClientError> {
        use worker::{
            WorkspaceClientError as Failure, WorkspaceRequestMethod as Method, WorkspaceResponse,
        };
        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        };
        self.paths
            .lock()
            .unwrap()
            .push(format!("{method} {}", request.path));
        let is_commit = request.path.ends_with("/commit");
        let api = self.api.clone();
        let identity = self.identity.clone();
        let worker = self.worker.clone();
        let runtime = self.runtime.clone();
        // Feature validation is synchronous, including on a Tokio thread. A separate
        // thread enters the existing runtime instead of nesting a Tokio runtime.
        let response = std::thread::spawn(move || {
            runtime.block_on(async move {
                let body = request.body.unwrap_or_default().into_bytes();
                let signed = runtime_source_request(
                    &identity,
                    Some(&worker.worker_id),
                    method,
                    &request.path,
                    body,
                );
                let response = build_router(api)
                    .oneshot(signed)
                    .await
                    .map_err(|_| Failure::Request("router dispatch failed".into()))?;
                let status = response.status().as_u16();
                let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .map_err(|_| Failure::Request("response unavailable after dispatch".into()))?;
                let body = String::from_utf8(bytes.to_vec())
                    .map_err(|_| Failure::Request("invalid response".into()))?;
                Ok(WorkspaceResponse { status, body })
            })
        })
        .join()
        .map_err(|_| Failure::Request("dispatch completion unavailable".into()))??;
        if is_commit
            && response.is_success()
            && self
                .drop_next_commit_response
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Failure::Request(
                "simulated lost response AFTER committed dispatch".into(),
            ));
        }
        Ok(response)
    }
}
fn config_interface(path: &str) -> Value {
    // Exact contextual reference published by the current config Object.
    json!({"scope": path, "name": "yoi.workspace-config/node/v1"})
}
async fn config_discover(runtime: &worker::wip::WipRuntime, path: &str) {
    inspect_published_interface(runtime, path, &config_interface(path)).await;
}
async fn inspect_published_interface(
    runtime: &worker::wip::WipRuntime,
    path: &str,
    expected: &Value,
) {
    let tree = runtime.tree(path.into(), 0, true).await.unwrap();
    let tree: Value = serde_json::from_str(tree.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        tree["path"], path,
        "Object must come from live Tree: {tree}"
    );
    let inspected = runtime.inspect(path.into(), true).await.unwrap();
    let inspected: Value = serde_json::from_str(inspected.content.as_deref().unwrap()).unwrap();
    assert!(
        inspected["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|interface| &interface["reference"] == expected),
        "interface must come from live path-centered Inspect: {inspected}"
    );
}
async fn config_call(
    runtime: &worker::wip::WipRuntime,
    path: &str,
    operation: &str,
    args: Value,
) -> std::result::Result<agen::tool::ToolOutput, agen::tool::ToolError> {
    runtime
        .invoke(
            path.into(),
            wip_protocol::InterfaceReference {
                scope: path.into(),
                name: "yoi.workspace-config/node/v1".into(),
            },
            operation.into(),
            args,
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
}

async fn catalog_call(
    runtime: &worker::wip::WipRuntime,
    path: &str,
    base: &str,
    operation: &str,
) -> Value {
    // Live catalog publication contextualizes the Interface to this Object.
    let reference = json!({"scope": path, "name": base});
    inspect_published_interface(runtime, path, &reference).await;
    let output = runtime
        .invoke(
            path.into(),
            wip_protocol::InterfaceReference {
                scope: path.into(),
                name: base.into(),
            },
            operation.into(),
            json!({}),
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}

async fn assert_config_catalog(runtime: &worker::wip::WipRuntime, attached: &Attachment) {
    let list = catalog_call(
        runtime,
        "/workdir-attachments",
        "yoi.workdir-attachment/collection/v1",
        "list",
    )
    .await;
    let listed = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["connection_id"] == attached.connection_id)
        .unwrap();
    let read = catalog_call(
        runtime,
        &format!("/workdir-attachments/{}", attached.connection_id),
        "yoi.workdir-attachment/item/v1",
        "read",
    )
    .await;
    for item in [listed, &read] {
        assert_eq!(item["connection_id"], attached.connection_id);
        assert_eq!(item["working_directory_id"], attached.working_directory_id);
        assert_eq!(item["alias"], attached.alias);
        assert_eq!(item["content_path"], "/workspace-config");
        assert_eq!(
            item["access"],
            serde_json::to_value(attached.access).unwrap()
        );
        assert_eq!(item["name"], attached.name);
        assert_eq!(item["purpose"], attached.purpose);
        assert!(item.get("checkout_path").is_none() && item.get("checkout_slug").is_none());
    }
    let workdir = catalog_call(
        runtime,
        &format!("/workdirs/{}", attached.working_directory_id),
        "yoi.workdir/item/v1",
        "read",
    )
    .await;
    assert_eq!(workdir["source"]["kind"], "workspace_config");
    assert_eq!(workdir["source"]["content_path"], "/workspace-config");
    assert!(workdir["source"].get("grant_id").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_real_selected_invoke_backend_wip_edit_create_delete_import_and_unknown() {
    use worker::feature::builtin::workspace_config::wip::mount_workspace_config_wip;
    use worker::feature::builtin::workspace_config::{CONTENT_ROOT, WorkspaceConfigFeature};
    let mut fixture = manual_worker_assignment_fixture().await;
    let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let granted = grant(&fixture.api, &fixture.worker, Access::ReadWrite).await;
    let client = Arc::new(ConfigRouterClient {
        api: fixture.api.clone(),
        worker: fixture.worker.clone(),
        identity: Arc::new(identity),
        runtime: tokio::runtime::Handle::current(),
        paths: Default::default(),
        drop_next_commit_response: Default::default(),
    });
    let feature = WorkspaceConfigFeature::for_workspace(client.clone(), true);
    let mut mounts = worker::wip::WipMountRegistry::new();
    mount_workspace_config_wip(&mut mounts, &feature).unwrap();
    worker::feature::builtin::manage_workdir::wip::mount_workspace_workdir_wip(
        &mut mounts,
        &worker::feature::builtin::manage_workdir::ManageWorkdirFeature::new(client.clone()),
        true,
        false,
        None,
    )
    .unwrap();
    let wip = worker::wip::WipRuntime::from_mounts(
        mounts,
        format!(
            "{}:{}@{TEST_WORKSPACE_ID}",
            fixture.worker.runtime_id, fixture.worker.worker_id
        ),
    )
    .unwrap();
    let manifest=worker::WorkerManifest::from_toml("[worker]\nname=\"config-e2e\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"unused\"\n[engine]\n[scope]\nallow=[]\n").unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let engine =
        agen::Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
            ConfigNoopLlmClient {
                api: fixture.api.clone(),
                worker: fixture.worker.clone(),
                calls: calls.clone(),
            },
        );
    let store =
        session_store::FsStore::new(fixture._workspace.path().join("config-e2e-sessions")).unwrap();
    let mut host = worker::Worker::new(
        manifest,
        engine,
        store,
        worker::WorkerWorkspaceContext::with_client(
            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
            client.clone(),
        ),
        worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    let report =
        host.install_features(worker::feature::FeatureRegistryBuilder::new().with_module(feature));
    assert!(
        report.installed_tool_names().is_empty() && !report.has_errors(),
        "WIP-only installation cannot expose a compatibility config Tool"
    );
    let selected = report
        .chat_invocations
        .feature_completions("workspace-c")
        .remove(0)
        .invocation
        .unwrap();
    let invocation = protocol::FeatureInvocation {
        invocation_id: "selected-config-invoke".into(),
        identity: selected.identity,
        name: selected.name,
        arguments: vec![],
    };
    assert!(
        workspace_config::get(&fixture.api, &fixture.worker)
            .await
            .unwrap()
            .is_none()
    );
    host.run(vec![
        protocol::Segment::FeatureInvoke {
            invocation: invocation.clone(),
        },
        protocol::Segment::text("Edit this Workspace configuration"),
    ])
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "production Worker run must issue its dependent LLM request only after preparation"
    );
    let attached = workspace_config::get(&fixture.api, &fixture.worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attached.working_directory_id, granted.working_directory_id);
    assert_config_catalog(&wip, &attached).await;
    assert!(
        report
            .chat_invocations
            .invoke(&protocol::FeatureInvocation {
                invocation_id: "repeat-config-invoke".into(),
                ..invocation.clone()
            })
            .await
            .unwrap()
            .message
            .starts_with("Already attached")
    );
    assert_eq!(
        workspace_config::get(&fixture.api, &fixture.worker)
            .await
            .unwrap()
            .unwrap()
            .connection_id,
        attached.connection_id
    );
    config_discover(&wip, CONTENT_ROOT).await;
    config_call(
        &wip,
        CONTENT_ROOT,
        "create",
        json!({"path":"notes/wip.md","content_type":"text","content":"original note"}),
    )
    .await
    .unwrap();
    let path = "/workspace-config/notes/wip.md";
    config_discover(&wip, path).await;
    assert!(
        config_call(&wip, path, "read", json!({}))
            .await
            .unwrap()
            .content
            .unwrap()
            .contains("original note")
    );
    config_call(
        &wip,
        path,
        "edit",
        json!({"old_string":"original","new_string":"edited"}),
    )
    .await
    .unwrap();
    let state = fixture
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/wip.md").unwrap())
            .unwrap()
            .content,
        "edited note"
    );
    config_call(&wip, path, "write", json!({"content":"replacement note"}))
        .await
        .unwrap();
    config_discover(&wip, CONTENT_ROOT).await;
    config_call(&wip,CONTENT_ROOT,"apply_changes",json!({"changes":[
        {"kind":"create","path":"fragments/prompts.dcdl","content_type":"decodal","content":"{ common = { language = \"CONFIG_WIP_E2E\"; }; }"},
        {"kind":"update","path":"main.dcdl","content":"{ prompts = import \"./fragments/prompts.dcdl\"; } as WorkspaceConfigSchema"}
    ]})).await.unwrap();
    // Confirm through ORIGINAL config reader/projection, not a copied WIP store.
    let state = fixture
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let prompts = crate::prompt_settings::project_prompts_from_workspace_config(&state).unwrap();
    assert_eq!(prompts.templates["common.language"], "CONFIG_WIP_E2E");
    config_discover(&wip, path).await;
    config_call(&wip, path, "delete", json!({})).await.unwrap();
    assert!(
        fixture
            .api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap()
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/wip.md").unwrap())
            .is_none()
    );
    config_discover(&wip, CONTENT_ROOT).await;
    let before = fixture
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let dispatched = client
        .paths
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.ends_with("/commit"))
        .count();
    client
        .drop_next_commit_response
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let unknown=config_call(&wip,CONTENT_ROOT,"create",json!({"path":"notes/lost-response.md","content_type":"text","content":"committed exactly once"})).await.unwrap_err();
    assert!(unknown.to_string().contains("unknown"), "{unknown}");
    let after = fixture
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(after.snapshot.revision, before.snapshot.revision + 1);
    assert!(
        after
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/lost-response.md").unwrap())
            .is_some()
    );
    assert_eq!(
        client
            .paths
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.ends_with("/commit"))
            .count(),
        dispatched + 1,
        "a lost response cannot trigger automatic retry"
    );
    config_discover(&wip, CONTENT_ROOT).await;
    let readonly = workspace_config::attach(
        &fixture.api,
        &fixture.worker,
        Attach {
            alias: None,
            access: Some(Access::ReadOnly),
        },
    )
    .await
    .unwrap();
    assert_eq!(readonly.connection_id, attached.connection_id);
    assert_config_catalog(&wip, &readonly).await;
    workspace_config::revoke_grant(
        &fixture.api,
        &test_owner_actor(),
        TEST_WORKSPACE_ID,
        &granted.grant_id,
    )
    .await
    .unwrap();
    assert!(
        config_call(
            &wip,
            CONTENT_ROOT,
            "create",
            json!({"path":"notes/revoked.md","content":"denied"})
        )
        .await
        .is_err(),
        "cached interface reference never grants access after revoke"
    );
    let after_revoke = catalog_call(
        &wip,
        "/workdir-attachments",
        "yoi.workdir-attachment/collection/v1",
        "list",
    )
    .await;
    assert!(
        after_revoke["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["connection_id"] != attached.connection_id)
    );
    assert!(
        client
            .paths
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.contains("workdir-session") && !path.contains("bash"))
    );
}

// Capture the client from an actual production embedded controller factory,
// rather than fabricating an "embedded" WorkspaceClient in the test.
struct ConfigCaptureFactory {
    inner: worker_runtime::worker_backend::ProfileRuntimeWorkerFactory,
    client: Arc<Mutex<Option<Arc<dyn worker::WorkspaceClient>>>>,
    controller_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}
#[async_trait]
impl worker_runtime::worker_backend::RuntimeWorkerFactory for ConfigCaptureFactory {
    async fn spawn_controller(
        &self,
        mut request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        // No LLM request is issued; explicit unauthenticated test model avoids
        // depending on developer credentials during production bootstrap.
        let archive = worker_runtime::profile_archive::ProfileSourceArchive::build(worker_runtime::profile_archive::ProfileSourceArchiveInput {
            id: "profile-source-archive:config-adapter-e2e".into(),
            entrypoints: BTreeMap::from([("builtin:companion".into(), "profiles/default.dcdl".into())]),
            imports: BTreeMap::new(),
            sources: BTreeMap::from([("profiles/default.dcdl".into(), r#"{
                slug = "default"; description = "Config adapter test"; scope = "workspace_read";
                model = { scheme = "anthropic"; model_id = "test-model"; auth = { kind = "none"; }; };
                engine = { max_tokens = 100; };
            }"#.into())]),
        }).map_err(|error| error.to_string())?;
        request.request.profile_source =
            worker_runtime::catalog::ProfileSourceArchiveSource::Embedded { archive };
        let controller = self.inner.spawn_controller(request).await?;
        *self.client.lock().unwrap() = Some(controller.workspace_client.clone());
        *self.controller_abort.lock().unwrap() = Some(controller.controller_task.abort_handle());
        Ok(controller)
    }
    async fn restore_controller(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        self.inner.restore_controller(request).await
    }
}

async fn assert_config_production_http_adapter(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    client: Arc<dyn worker::WorkspaceClient>,
) {
    use worker::feature::builtin::workspace_config::backend::WorkspaceConfigBackend;
    use worker::feature::builtin::workspace_config::{
        WorkspaceConfigAttachmentBackend, WorkspaceConfigFeature,
    };
    assert_eq!(client.kind(), "runtime-owned-workspace-client");
    let backend = WorkspaceConfigBackend::new(client.clone());
    assert!(
        backend.current().is_err(),
        "valid source proof cannot manufacture a config grant"
    );
    grant(api, worker, Access::ReadWrite).await;
    backend
        .attach("production-adapter-test", None)
        .await
        .unwrap();
    let attached = workspace_config::get(api, worker).await.unwrap().unwrap();
    let feature = WorkspaceConfigFeature::for_workspace(client.clone(), true);
    let mut mounts = worker::wip::WipMountRegistry::new();
    worker::feature::builtin::workspace_config::wip::mount_workspace_config_wip(
        &mut mounts,
        &feature,
    )
    .unwrap();
    worker::feature::builtin::manage_workdir::wip::mount_workspace_workdir_wip(
        &mut mounts,
        &worker::feature::builtin::manage_workdir::ManageWorkdirFeature::new(client),
        true,
        false,
        None,
    )
    .unwrap();
    let wip = worker::wip::WipRuntime::from_mounts(
        mounts,
        format!(
            "{}:{}@{TEST_WORKSPACE_ID}",
            worker.runtime_id, worker.worker_id
        ),
    )
    .unwrap();
    assert_config_catalog(&wip, &attached).await;
    config_discover(&wip, "/workspace-config/main.dcdl").await;
    let main_interface = wip
        .inspect("/workspace-config/main.dcdl".into(), true)
        .await
        .unwrap()
        .content
        .unwrap();
    assert!(
        !main_interface.contains("delete("),
        "required entrypoint cannot advertise delete: {main_interface}"
    );
    assert!(
        config_call(&wip, "/workspace-config/main.dcdl", "read", json!({}))
            .await
            .unwrap()
            .content
            .unwrap()
            .contains("WorkspaceConfigSchema")
    );
    config_discover(&wip, "/workspace-config").await;
    config_call(
        &wip,
        "/workspace-config",
        "create",
        json!({"path":"notes/production-adapter.md","content_type":"text","content":"production source proof"}),
    )
    .await
    .unwrap();
    assert!(
        api.config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap()
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/production-adapter.md").unwrap())
            .is_some()
    );
    workspace_config::revoke_grant(
        api,
        &test_owner_actor(),
        TEST_WORKSPACE_ID,
        &attached_grant_id(api, worker),
    )
    .await
    .unwrap();
    assert!(backend.current().is_err());
    assert!(
        config_call(
            &wip,
            "/workspace-config",
            "create",
            json!({"path":"notes/revoked-adapter.md","content":"denied"})
        )
        .await
        .is_err()
    );
}
fn attached_grant_id(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> String {
    api.store
        .current_workspace_config_grant(TEST_WORKSPACE_ID, worker)
        .unwrap()
        .unwrap()
        .grant_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_actual_remote_and_embedded_clients_use_authenticated_workspace_request_scope()
 {
    use worker_runtime::auth::{
        RUNTIME_REQUEST_SOURCE_PROOF_HEADER, WORKSPACE_REQUEST_PERMISSION,
        decode_runtime_request_source_claims,
    };
    // Remote Runtime-owned HTTP adapter, including its real request signer.
    let mut fixture = manual_worker_assignment_fixture().await;
    let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
    configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
    let claims = Arc::new(Mutex::new(Vec::new()));
    let capture = claims.clone();
    let app = build_router(fixture.api.clone()).layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let capture = capture.clone();
            async move {
                let proof = request
                    .headers()
                    .get(RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                    .unwrap()
                    .to_str()
                    .unwrap();
                capture
                    .lock()
                    .unwrap()
                    .push(decode_runtime_request_source_claims(proof).unwrap());
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = worker_runtime::worker_source::RuntimeOwnedWorkspaceClient::new(
        TEST_WORKSPACE_ID,
        format!("http://{addr}"),
        &fixture.worker.runtime_id,
        &fixture.worker.worker_id,
    )
    .with_runtime_request_source(&identity, "server-test");
    assert_config_production_http_adapter(&fixture.api, &fixture.worker, Arc::new(client)).await;
    let observed = claims.lock().unwrap();
    assert!(observed.iter().any(|claim| claim.path.ends_with("/commit")));
    assert!(
        observed
            .iter()
            .all(|claim| claim.permission == WORKSPACE_REQUEST_PERMISSION
                && claim.iss == fixture.worker.runtime_id
                && claim.worker_id.as_deref() == Some(fixture.worker.worker_id.as_str())
                && claim.workspace_id == TEST_WORKSPACE_ID)
    );
    drop(observed);
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workspace-config");
    let wrong_scope = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity)
        .issue(
            "server-test",
            TEST_WORKSPACE_ID,
            Some(&fixture.worker.worker_id),
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
            "GET",
            &path,
            &[],
            i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
            30,
        )
        .unwrap();
    let request = Request::builder()
        .method("GET")
        .uri(&path)
        .header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, wrong_scope)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        build_router(fixture.api.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED,
        "valid signature under a different permission is not WorkspaceRequest authority"
    );
    server.abort();

    // Actual embedded Runtime factory -> real Worker controller -> captured
    // production WorkspaceClient -> signed HTTP -> production Backend middleware.
    let root = tempfile::tempdir().unwrap();
    init_clean_git_workspace(root.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let mut config = test_server_config(root.path());
    config.backend_base_url = Some("server-test".into());
    let store = Arc::new(SqliteWorkspaceStore::open(&config.database_path).unwrap());
    let captured = Arc::new(Mutex::new(None));
    let abort = Arc::new(Mutex::new(None));
    let factory = ConfigCaptureFactory {
        inner: worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(root.path())
            .with_embedded_worker_mutation_dispatcher(
                EMBEDDED_RUNTIME_ID,
                Arc::new(
                    crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                        config.clone(),
                        store.clone(),
                    ),
                ),
            )
            .with_workspace_request_client(
                worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                    TEST_WORKSPACE_ID,
                    &base,
                    EMBEDDED_RUNTIME_ID,
                )
                .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, "server-test"),
            )
            .with_runtime_store_dir(root.path().join("production-controller-store")),
        client: captured.clone(),
        controller_abort: abort.clone(),
    };
    let execution = Arc::new(
        worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(factory).unwrap(),
    );
    let mut api = WorkspaceApi::new_with_execution_backend(config, store, execution)
        .await
        .unwrap();
    configure_runtime_request_auth(
        &mut api,
        &EMBEDDED_RUNTIME_REQUEST_IDENTITY,
        EMBEDDED_RUNTIME_ID,
    );
    let server_api = api.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_router(server_api))
            .await
            .unwrap()
    });
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_RUNTIME_ID.into(),
            display_name: "Production config adapter".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            feature_connections: Default::default(),
            control_operation_id: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    let client = captured
        .lock()
        .unwrap()
        .take()
        .expect("production embedded factory must inject a WorkspaceClient");
    assert_config_production_http_adapter(&api, &worker, client).await;
    abort.lock().unwrap().take().unwrap().abort();
    server.abort();
}
