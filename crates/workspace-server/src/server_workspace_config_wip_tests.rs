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
fn contextual_reference(base: &str, path: &str) -> wip_protocol::InterfaceReference {
    wip_protocol::InterfaceReference {
        scope: path.into(),
        name: base.into(),
    }
}
fn config_interface(path: &str) -> wip_protocol::InterfaceReference {
    // Use the published contextual interface identity, not a model-managed validator.
    contextual_reference("yoi.workspace-config/node/v1", path)
}
async fn inspect_interface(runtime: &worker::wip::WipRuntime, path: &str, base: &str) -> Value {
    let tree = runtime.tree(path.into(), 0, true).await.unwrap();
    let tree: Value = serde_json::from_str(tree.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        tree["path"], path,
        "Object must come from live Tree: {tree}"
    );
    let output = runtime.inspect(path.into(), true).await.unwrap();
    let observed: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(observed["path"], path);
    for interface in observed["interfaces"].as_array().unwrap() {
        runtime
            .inspect(interface["path"].as_str().unwrap().into(), true)
            .await
            .unwrap();
    }
    assert!(
        observed["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|interface| { interface["reference"] == json!({"scope": path, "name": base}) }),
        "interface must come from live inspection: {observed}"
    );
    observed
}
async fn config_inspect(runtime: &worker::wip::WipRuntime, path: &str) -> Value {
    inspect_interface(runtime, path, "yoi.workspace-config/node/v1").await
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
            if matches!(operation, "edit" | "write") {
                wip_protocol::InterfaceReference {
                    scope: "/".into(),
                    name: format!("yoi.file/{operation}-text/v1"),
                }
            } else {
                config_interface(path)
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
    inspect_interface(runtime, path, base).await;
    let output = runtime
        .invoke(
            path.into(),
            contextual_reference(base, path),
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

// Ordinary workspace-server lib integration tests: model-facing Inspect/Invoke -> production
// WIP adapter -> signed Backend router -> temporary canonical store and its existing evaluator.
// Only HTTP transport is in-process; response loss is injected after real commit dispatch.
struct ConfigBatchWipFixture {
    backend: ManualCoderAssignmentFixture,
    client: Arc<ConfigRouterClient>,
    wip: worker::wip::WipRuntime,
}
fn config_batch_runtime(client: Arc<ConfigRouterClient>) -> worker::wip::WipRuntime {
    let feature = worker::feature::builtin::workspace_config::WorkspaceConfigFeature::for_workspace(
        client.clone(),
        true,
    );
    let mut mounts = worker::wip::WipMountRegistry::new();
    worker::feature::builtin::workspace_config::wip::mount_workspace_config_wip(
        &mut mounts,
        &feature,
    )
    .unwrap();
    worker::wip::WipRuntime::from_mounts(
        mounts,
        format!(
            "{}:{}@{TEST_WORKSPACE_ID}",
            client.worker.runtime_id, client.worker.worker_id
        ),
    )
    .unwrap()
}
impl ConfigBatchWipFixture {
    async fn new() -> Self {
        let mut backend = manual_worker_assignment_fixture().await;
        let identity = RuntimeIdentityMaterial::generate(&backend.worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut backend.api, &identity, &backend.worker.runtime_id);
        grant(&backend.api, &backend.worker, Access::ReadWrite).await;
        attach(&backend.api, &backend.worker).await;
        let client = Arc::new(ConfigRouterClient {
            api: backend.api.clone(),
            worker: backend.worker.clone(),
            identity: Arc::new(identity),
            runtime: tokio::runtime::Handle::current(),
            paths: Default::default(),
            drop_next_commit_response: Default::default(),
        });
        let wip = config_batch_runtime(client.clone());
        Self {
            backend,
            client,
            wip,
        }
    }
    fn state(&self) -> crate::config_source::WorkspaceConfigState {
        self.backend
            .api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap()
    }
    fn commits(&self) -> usize {
        self.client
            .paths
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.ends_with("/commit"))
            .count()
    }
    async fn inspect(&self) -> Value {
        let object = config_inspect(&self.wip, "/workspace-config").await;
        let interface = object["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|interface| {
                interface["reference"]
                    == json!({
                        "scope":"/workspace-config", "name":"yoi.workspace-config/node/v1"
                    })
            })
            .unwrap();
        let output = self
            .wip
            .inspect(interface["path"].as_str().unwrap().into(), true)
            .await
            .unwrap();
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
    }
    async fn apply(&self, changes: Value) -> Value {
        self.inspect().await;
        self.apply_observed(changes).await
    }
    async fn apply_observed(&self, changes: Value) -> Value {
        let before = self.commits();
        let output = config_call(
            &self.wip,
            "/workspace-config",
            "apply_changes",
            json!({"changes":changes}),
        )
        .await
        .unwrap();
        let result: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(result.as_object().unwrap().len(), 2, "{result}");
        assert_eq!(result["digest"], self.state().snapshot.digest);
        assert!(
            result["activation"]
                .as_str()
                .unwrap()
                .contains("not rewritten"),
            "{result}"
        );
        assert_eq!(
            self.commits(),
            before + 1,
            "one atomic batch, one commit dispatch"
        );
        result
    }
    async fn reject(&self, label: &str, arguments: Value) -> agen::tool::ToolError {
        self.inspect().await;
        let before = self.state();
        let commits = self.commits();
        let error = config_call(&self.wip, "/workspace-config", "apply_changes", arguments)
            .await
            .unwrap_err();
        assert!(
            !error.to_string().contains("outcome unknown"),
            "{label}: {error}"
        );
        assert_eq!(
            self.state(),
            before,
            "{label}: no partial canonical write or projection change"
        );
        assert!(
            self.commits() <= commits + 1,
            "{label}: never automatically retry rejection"
        );
        error
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_inspect_publishes_typed_cases_constraints_and_results() {
    let fixture = ConfigBatchWipFixture::new().await;
    let definition = fixture.inspect().await;
    let signature = definition["interface_signature"].as_str().unwrap();
    // Assert the actual Text View, not a descriptor constructed by the test. This is display
    // verification only: none of this text is parsed into production request arguments.
    let declarations = signature
        .lines()
        .filter(|line| !line.trim_start().starts_with("///"))
        .collect::<Vec<_>>()
        .join(" ");
    let declarations = declarations
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for expected in [
        "operation apply_changes(",
        "changes: [union {",
        "create({ path: string, content: string, content_type?: enum { decodal, text, }, }),",
        "update({ path: string, content: string, }),",
        "delete({ path: string, }),",
        "rename({ from: string, to: string, }),",
        "-> { digest: string, activation: string, }",
    ] {
        assert!(
            declarations.contains(expected),
            "missing {expected}: {signature}"
        );
    }
    assert!(!declarations.contains("changes: json"), "{signature}");
    assert!(!declarations.contains("changes?:"), "{signature}");
    assert!(
        !declarations.contains("expected_digest:"),
        "model cannot manage CAS: {signature}"
    );
    // Summaries (not invisible documentation.details) must explain non-type constraints.
    for expected in [
        "1..=256",
        "262144",
        "512",
        "UTF-8",
        "canonical",
        "relative",
        "both rename endpoints",
        "once per batch",
        "defaults to decodal",
        "existing entrypoints",
        "schema",
        "Whole-tree CAS",
        "stale",
        "not hot application",
        "OutcomeUnknown",
        "Raw kind-tagged input is not accepted",
        "composite",
        "all changes commit together or none",
    ] {
        assert!(
            signature.contains(expected),
            "missing public constraint {expected}: {signature}"
        );
    }
    for case in ["create", "update", "delete", "rename"] {
        // The renderer quotes documentation strings, including the JSON examples.
        assert!(
            signature.contains(&format!(r#"\"$case\":\"{case}\""#)),
            "missing public JSON example for {case}: {signature}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_model_union_cases_and_composite_persist_evaluated_imports() {
    let fixture = ConfigBatchWipFixture::new().await;
    // Public Invoke union encoding and fields shown by Inspect; no internal Change fixtures.
    fixture.apply(json!([
        {"$case":"create","value":{"path":"notes/edit.txt","content":"old","content_type":"text"}},
        {"$case":"create","value":{"path":"notes/source.txt","content":"move me","content_type":"text"}},
        {"$case":"create","value":{"path":"notes/obsolete.txt","content":"remove me","content_type":"text"}}
    ])).await;
    // Subsequent calls use the Client's returned root validator, without reinspection or
    // model-supplied preconditions. All four variants share the same public contract.
    fixture
        .apply_observed(json!([{"$case":"update","value":{"path":"notes/edit.txt","content":"updated"}}]))
        .await;
    fixture
        .apply_observed(
            json!([{"$case":"rename","value":{"from":"notes/source.txt","to":"notes/moved.txt"}}]),
        )
        .await;
    fixture
        .apply_observed(json!([{"$case":"delete","value":{"path":"notes/obsolete.txt"}}]))
        .await;
    let state = fixture.state();
    let entry = |path: &str| {
        state
            .snapshot
            .get(&config_source::VirtualPath::parse(path).unwrap())
    };
    assert_eq!(entry("notes/edit.txt").unwrap().content, "updated");
    assert_eq!(
        entry("notes/edit.txt").unwrap().content_type,
        config_source::ConfigContentType::Text
    );
    assert_eq!(entry("notes/moved.txt").unwrap().content, "move me");
    assert_eq!(
        entry("notes/moved.txt").unwrap().content_type,
        config_source::ConfigContentType::Text
    );
    assert!(entry("notes/source.txt").is_none());
    assert!(entry("notes/obsolete.txt").is_none());

    fixture.apply(json!([
        {"$case":"create","value":{"path":"fragments/batch.dcdl","content":"{ common = { language = \"TYPED_BATCH\"; }; }"}},
        {"$case":"update","value":{"path":"main.dcdl","content":"{ prompts = import \"./fragments/batch.dcdl\"; } as WorkspaceConfigSchema"}},
        {"$case":"delete","value":{"path":"notes/edit.txt"}},
        {"$case":"rename","value":{"from":"notes/moved.txt","to":"notes/final.txt"}}
    ])).await;
    let state = fixture.state();
    let fragment = state
        .snapshot
        .get(&config_source::VirtualPath::parse("fragments/batch.dcdl").unwrap())
        .unwrap();
    assert_eq!(
        fragment.content_type,
        config_source::ConfigContentType::Decodal,
        "omitted content_type defaults to decodal"
    );
    assert_eq!(
        fragment.content,
        "{\n    common = {\n        language = \"TYPED_BATCH\";\n    };\n}\n"    );
    assert_eq!(
        state
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/final.txt").unwrap())
            .unwrap()
            .content,
        "move me"
    );
    for absent in ["notes/edit.txt", "notes/moved.txt"] {
        assert!(
            state
                .snapshot
                .get(&config_source::VirtualPath::parse(absent).unwrap())
                .is_none()
        );
    }
    let prompts = crate::prompt_settings::project_prompts_from_workspace_config(&state).unwrap();
    assert_eq!(prompts.templates["common.language"], "TYPED_BATCH");
    assert_eq!(
        state.contract.entrypoints,
        vec![config_source::VirtualPath::parse("main.dcdl").unwrap()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_model_shape_errors_reject_without_commit() {
    let fixture = ConfigBatchWipFixture::new().await;
    for (label, arguments) in [
        ("missing changes", json!({})),
        ("changes not list", json!({"changes":{}})),
        (
            "unknown case",
            json!({"changes":[{"$case":"replace","value":{"path":"main.dcdl","content":"{}"}}]}),
        ),
        (
            "raw Backend kind is not WIP union",
            json!({"changes":[{"kind":"create","path":"notes/raw.txt","content":"raw","content_type":"text"}]}),
        ),
        (
            "create missing content",
            json!({"changes":[{"$case":"create","value":{"path":"notes/missing.txt"}}]}),
        ),
        (
            "update missing path",
            json!({"changes":[{"$case":"update","value":{"content":"{}"}}]}),
        ),
        (
            "delete missing path",
            json!({"changes":[{"$case":"delete","value":{}}]}),
        ),
        (
            "rename missing to",
            json!({"changes":[{"$case":"rename","value":{"from":"main.dcdl"}}]}),
        ),
        (
            "null is not omitted content_type",
            json!({"changes":[{"$case":"create","value":{"path":"notes/null.dcdl","content":"{}","content_type":null}}]}),
        ),
        (
            "invalid content_type",
            json!({"changes":[{"$case":"create","value":{"path":"notes/type.txt","content":"x","content_type":"json"}}]}),
        ),
        (
            "wrong field type",
            json!({"changes":[{"$case":"delete","value":{"path":123}}]}),
        ),
        (
            "extra payload field",
            json!({"changes":[{"$case":"update","value":{"path":"main.dcdl","content":"{}","expected_digest":"model supplied"}}]}),
        ),
        (
            "extra union field",
            json!({"changes":[{"$case":"delete","value":{"path":"main.dcdl"},"kind":"delete"}]}),
        ),
        (
            "extra argument",
            json!({"changes":[],"entrypoints":["notes/new.dcdl"]}),
        ),
    ] {
        let commits = fixture.commits();
        fixture.reject(label, arguments).await;
        assert_eq!(
            fixture.commits(),
            commits,
            "{label}: invalid model shape cannot reach commit"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_invalid_candidates_and_collisions_leave_canonical_tree_unchanged() {
    let fixture = ConfigBatchWipFixture::new().await;
    fixture.apply(json!([
        {"$case":"create","value":{"path":"notes/source.txt","content":"source","content_type":"text"}},
        {"$case":"create","value":{"path":"notes/other.txt","content":"other","content_type":"text"}}
    ])).await;
    for (label, changes) in [
        ("empty batch", json!([])),
        (
            "traversal",
            json!([{"$case":"create","value":{"path":"../escape","content":"{}"}}]),
        ),
        (
            "absolute path",
            json!([{"$case":"create","value":{"path":"/workspace-config/escape","content":"{}"}}]),
        ),
        (
            "noncanonical path",
            json!([{"$case":"create","value":{"path":"notes//escape","content":"{}"}}]),
        ),
        (
            "overlong UTF-8 path",
            json!([{"$case":"create","value":{"path":"é".repeat(257),"content":"{}"}}]),
        ),
        (
            "parse failure",
            json!([{"$case":"update","value":{"path":"main.dcdl","content":"{"}}]),
        ),
        (
            "schema failure",
            json!([{"$case":"update","value":{"path":"main.dcdl","content":"{ prompts = 42; } as WorkspaceConfigSchema"}}]),
        ),
        (
            "missing import",
            json!([{"$case":"update","value":{"path":"main.dcdl","content":"import \"./missing.dcdl\""}}]),
        ),
        (
            "duplicate target",
            json!([
                {"$case":"update","value":{"path":"notes/source.txt","content":"changed"}},
                {"$case":"delete","value":{"path":"notes/source.txt"}}
            ]),
        ),
        (
            "rename source touched twice",
            json!([
                {"$case":"rename","value":{"from":"notes/source.txt","to":"notes/moved.txt"}},
                {"$case":"delete","value":{"path":"notes/source.txt"}}
            ]),
        ),
        (
            "rename destination touched twice",
            json!([
                {"$case":"rename","value":{"from":"notes/source.txt","to":"notes/moved.txt"}},
                {"$case":"create","value":{"path":"notes/moved.txt","content":"collision","content_type":"text"}}
            ]),
        ),
        (
            "two rename destinations collide",
            json!([
                {"$case":"rename","value":{"from":"notes/source.txt","to":"notes/moved.txt"}},
                {"$case":"rename","value":{"from":"notes/other.txt","to":"notes/moved.txt"}}
            ]),
        ),
        (
            "occupied rename destination",
            json!([{"$case":"rename","value":{"from":"notes/source.txt","to":"notes/other.txt"}}]),
        ),
        (
            "required entrypoint deletion",
            json!([{"$case":"delete","value":{"path":"main.dcdl"}}]),
        ),
    ] {
        // A valid first change must not leak from a later rejected change/candidate.
        let mut batch = vec![
            json!({"$case":"create","value":{"path":"notes/atomic-marker.txt","content":"must not persist","content_type":"text"}}),
        ];
        if label == "empty batch" {
            batch.clear();
        } else {
            batch.extend(changes.as_array().unwrap().iter().cloned());
        }
        fixture.reject(label, json!({"changes":batch})).await;
    }
    let oversized = (0..257)
        .map(|index| {
            json!({"$case":"create","value":{
                "path":format!("notes/large-{index}.txt"),"content":"","content_type":"text"
            }})
        })
        .collect::<Vec<_>>();
    fixture
        .reject("batch limit", json!({"changes":oversized}))
        .await;
    fixture.reject("aggregate text limit", json!({"changes":[
        {"$case":"create","value":{"path":"notes/large-a.txt","content":"x".repeat(131072),"content_type":"text"}},
        {"$case":"create","value":{"path":"notes/large-b.txt","content":"x".repeat(131073),"content_type":"text"}}
    ]})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_stale_root_observation_rejects_unrelated_tree_change() {
    let fixture = ConfigBatchWipFixture::new().await;
    fixture.inspect().await;
    let before = fixture.state();
    // Independent model-facing cache over the same signed client and canonical store.
    let other = config_batch_runtime(fixture.client.clone());
    config_inspect(&other, "/workspace-config").await;
    config_call(&other, "/workspace-config", "apply_changes", json!({"changes":[
        {"$case":"create","value":{"path":"notes/other-editor.txt","content":"concurrent edit","content_type":"text"}}
    ]})).await.unwrap();
    let concurrent = fixture.state();
    assert_ne!(before.snapshot.digest, concurrent.snapshot.digest);
    assert_eq!(
        before
            .snapshot
            .get(&config_source::VirtualPath::parse("main.dcdl").unwrap()),
        concurrent
            .snapshot
            .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
    );
    let commits = fixture.commits();
    let error = config_call(&fixture.wip, "/workspace-config", "apply_changes", json!({"changes":[
        {"$case":"create","value":{"path":"notes/stale-editor.txt","content":"must not save","content_type":"text"}}
    ]})).await.unwrap_err();
    assert!(error.to_string().contains("ValidatorMismatch"), "{error}");
    assert_eq!(fixture.state(), concurrent);
    assert_eq!(
        fixture.commits(),
        commits,
        "stale root cannot dispatch a fresh commit"
    );
    fixture.apply(json!([{"$case":"create","value":{"path":"notes/fresh-editor.txt","content":"new decision","content_type":"text"}}])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_cached_contract_cannot_bypass_readonly_or_revocation() {
    for revoke in [false, true] {
        let fixture = ConfigBatchWipFixture::new().await;
        let definition = fixture.inspect().await;
        assert!(
            definition["interface_signature"]
                .as_str()
                .unwrap()
                .contains("apply_changes(")
        );
        let before = fixture.state();
        if revoke {
            workspace_config::revoke_grant(
                &fixture.backend.api,
                &test_owner_actor(),
                TEST_WORKSPACE_ID,
                &attached_grant_id(&fixture.backend.api, &fixture.backend.worker),
            )
            .await
            .unwrap();
        } else {
            workspace_config::attach(
                &fixture.backend.api,
                &fixture.backend.worker,
                Attach {
                    alias: None,
                    access: Some(Access::ReadOnly),
                },
            )
            .await
            .unwrap();
        }
        let commits = fixture.commits();
        let error = config_call(&fixture.wip, "/workspace-config", "apply_changes", json!({"changes":[
            {"$case":"create","value":{"path":"notes/denied.txt","content":"denied","content_type":"text"}}
        ]})).await.unwrap_err();
        assert!(
            !error.to_string().contains("outcome unknown"),
            "{error}"
        );
        assert_eq!(fixture.state(), before);
        assert_eq!(fixture.commits(), commits);
        if !revoke {
            let readonly = fixture.inspect().await;
            assert!(
                !readonly["interface_signature"]
                    .as_str()
                    .unwrap()
                    .contains("apply_changes("),
                "{readonly}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_config_apply_known_rejection_differs_from_lost_response_without_retry() {
    let fixture = ConfigBatchWipFixture::new().await;
    let commits = fixture.commits();
    fixture.reject("known Backend evaluation rejection", json!({"changes":[
        {"$case":"create","value":{"path":"notes/no-partial.txt","content":"not committed","content_type":"text"}},
        {"$case":"update","value":{"path":"main.dcdl","content":"{ prompts = 42; } as WorkspaceConfigSchema"}}
    ]})).await;
    assert_eq!(
        fixture.commits(),
        commits + 1,
        "known rejection came from real Backend evaluation"
    );
    fixture.inspect().await;
    let commits = fixture.commits();
    fixture
        .client
        .drop_next_commit_response
        .store(true, Ordering::SeqCst);
    let error = config_call(&fixture.wip, "/workspace-config", "apply_changes", json!({"changes":[
        {"$case":"create","value":{"path":"notes/unknown.txt","content":"committed once","content_type":"text"}}
    ]})).await.unwrap_err();
    assert!(
        error.to_string().contains("outcome unknown"),
        "{error}"
    );
    let state = fixture.state();
    assert_eq!(
        state
            .snapshot
            .get(&config_source::VirtualPath::parse("notes/unknown.txt").unwrap())
            .unwrap()
            .content,
        "committed once"
    );
    assert_eq!(
        fixture.commits(),
        commits + 1,
        "lost response never triggers automatic retry"
    );
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
    config_inspect(&wip, CONTENT_ROOT).await;
    config_call(
        &wip,
        CONTENT_ROOT,
        "create",
        json!({"path":"notes/wip.md","content_type":"text","content":"original note"}),
    )
    .await
    .unwrap();
    let path = "/workspace-config/notes/wip.md";
    config_inspect(&wip, path).await;
    let read = config_call(&wip, path, "read", json!({})).await.unwrap();
    let read: Value = serde_json::from_str(read.content.as_deref().unwrap()).unwrap();
    assert_eq!(read["content"], "original note");
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
    config_inspect(&wip, CONTENT_ROOT).await;
    config_call(&wip,CONTENT_ROOT,"apply_changes",json!({"changes":[
        {"$case":"create","value":{"path":"fragments/prompts.dcdl","content_type":"decodal","content":"{ common = { language = \"CONFIG_WIP_E2E\"; }; }"}},
        {"$case":"update","value":{"path":"main.dcdl","content":"{ prompts = import \"./fragments/prompts.dcdl\"; } as WorkspaceConfigSchema"}}
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
    config_inspect(&wip, path).await;
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
    config_inspect(&wip, CONTENT_ROOT).await;
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
    assert_ne!(after.snapshot.digest, before.snapshot.digest);
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
    config_inspect(&wip, CONTENT_ROOT).await;
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
    async fn preflight_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), String> {
        self.inner.preflight_restore(request).await
    }

    async fn recover_restore_source(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<bool, worker_runtime::worker_backend::RuntimeRestoreFailure> {
        self.inner.recover_restore_source(request).await
    }

    async fn prepare_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<
        Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let inner = self.inner.prepare_restore(request).await?;
        Ok(Box::new(ConfigCaptureCandidate {
            inner,
            client: self.client.clone(),
            controller_abort: self.controller_abort.clone(),
        }))
    }

    async fn cleanup_failed_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.cleanup_failed_restore(request).await
    }

    async fn reconcile_stopped_worker(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.reconcile_stopped_worker(worker_ref).await
    }

    fn activate_restored_controller(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        workspace_id: Option<&str>,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner
            .activate_restored_controller(worker_ref, workspace_id, handle)
    }
}

struct ConfigCaptureCandidate {
    inner: Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
    client: Arc<Mutex<Option<Arc<dyn worker::WorkspaceClient>>>>,
    controller_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

#[async_trait]
impl worker_runtime::worker_backend::RuntimeRestoreCandidate for ConfigCaptureCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        self.inner.initial_state()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<
        worker_runtime::worker_backend::RuntimeWorkerController,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let controller = self.inner.activate().await?;
        *self.client.lock().unwrap() = Some(controller.workspace_client.clone());
        *self.controller_abort.lock().unwrap() = Some(controller.controller_task.abort_handle());
        Ok(controller)
    }

    async fn discard(
        &mut self,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.discard().await
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
    let main_interface = config_inspect(&wip, "/workspace-config/main.dcdl").await;
    for interface in main_interface["interfaces"].as_array().unwrap() {
        assert!(interface.get("signature").is_none());
        let definition = wip
            .inspect(interface["path"].as_str().unwrap().into(), false)
            .await
            .unwrap();
        let definition: Value =
            serde_json::from_str(definition.content.as_deref().unwrap()).unwrap();
        assert!(
            !definition["interface_signature"]
                .as_str()
                .unwrap()
                .contains("delete("),
            "required entrypoint cannot advertise delete: {definition}"
        );
    }
    let main = config_call(&wip, "/workspace-config/main.dcdl", "read", json!({}))
        .await
        .unwrap();
    let main: Value = serde_json::from_str(main.content.as_deref().unwrap()).unwrap();
    assert!(
        main["content"]
            .as_str()
            .unwrap()
            .contains("WorkspaceConfigSchema")
    );
    config_inspect(&wip, "/workspace-config").await;
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
    let root = restored_parent_control_tests::connected_restore_tempdir();
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
    init_clean_git_workspace(root.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let mut config = test_server_config(root.path());
    config.backend_base_url = Some("server-test".into());
    let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
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
            execution_grant: None,
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
