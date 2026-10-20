//! Tools/native WIP -> signed WorkspaceClient -> isolated Server DB and LocalFileSystem.
//! Default fixtures have no Repository/Workdir and substitute Runtime execution.
//! Lifetime cases use real embedded Runtime stop/removal or the shared authenticated
//! Runtime HTTP harness for actual Git Workdir removal; controller execution remains
//! deterministic. Import sessions have explicit read scope (and real materializer
//! leases in the removal case). No external model, live data, or browser is used.
//! Run with the normal Server lib target and the `worker_drive_tests` filter.
use super::*;
use agen::tool::{Attachment, ToolError, ToolExecutionContext, ToolOutput};
use std::result::Result;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use workdir::WorkdirSessionRouter;
use worker::feature::builtin::drive::DriveFeature;
use worker::{
    WorkspaceBinaryRequest, WorkspaceBinaryResponse, WorkspaceClient, WorkspaceClientError,
    WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse,
};

/// Only transport is substituted. Each request gets a fresh real Runtime proof
/// and passes the production router, registry, current grant, DB and blob store.
struct RouterClient {
    api: WorkspaceApi,
    identity: Arc<RuntimeIdentityMaterial>,
    worker: RuntimeWorkerRef,
    handle: tokio::runtime::Handle,
    lose_next_mutation_response: AtomicBool,
    lose_next_upload_response: AtomicBool,
    requests: Mutex<Vec<(String, String)>>,
}
impl std::fmt::Debug for RouterClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterClient")
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}
impl RouterClient {
    fn request(
        &self,
        method: WorkspaceRequestMethod,
        path: String,
        bytes: Vec<u8>,
        limit: usize,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        let method = match method {
            WorkspaceRequestMethod::Get => "GET",
            WorkspaceRequestMethod::Post => "POST",
            WorkspaceRequestMethod::Put => "PUT",
            WorkspaceRequestMethod::Patch => "PATCH",
            WorkspaceRequestMethod::Delete => "DELETE",
        };
        self.requests
            .lock()
            .unwrap()
            .push((method.into(), path.clone()));
        let mut request = runtime_source_request(
            &self.identity,
            Some(&self.worker.worker_id),
            method,
            &path,
            bytes,
        );
        if method == "PUT" {
            request
                .headers_mut()
                .insert(CONTENT_TYPE, "application/octet-stream".parse().unwrap());
        }
        // DriveBackend invokes sync WorkspaceClient methods on spawn_blocking.
        self.handle.block_on(async {
            let response = build_router(self.api.clone())
                .oneshot(request)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            let status = response.status().as_u16();
            let body = to_bytes(response.into_body(), limit)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?
                .to_vec();
            if method == "POST"
                && path.ends_with("/mutate")
                && (200..300).contains(&status)
                && self
                    .lose_next_mutation_response
                    .swap(false, Ordering::SeqCst)
            {
                return Err(WorkspaceClientError::Request(
                    "response lost after commit".into(),
                ));
            }
            if method == "PUT"
                && path.contains("/upload?")
                && (200..300).contains(&status)
                && self.lose_next_upload_response.swap(false, Ordering::SeqCst)
            {
                return Err(WorkspaceClientError::Request(
                    "upload reply lost after commit".into(),
                ));
            }
            Ok(WorkspaceBinaryResponse { status, body })
        })
    }
}
impl WorkspaceClient for RouterClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(TEST_WORKSPACE_ID)
    }
    fn kind(&self) -> &str {
        "signed-in-process"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let response = self.request(
            request.method,
            request.path,
            request.body.unwrap_or_default().into_bytes(),
            2 * 1024 * 1024,
        )?;
        Ok(WorkspaceResponse {
            status: response.status,
            body: String::from_utf8(response.body)
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?,
        })
    }
    fn execute_binary(
        &self,
        request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        self.request(
            request.method,
            request.path,
            request.body.unwrap_or_default(),
            request.max_response_bytes,
        )
    }
}

struct Fixture {
    api: WorkspaceApi,
    owner: String,
    clients: Vec<Arc<RouterClient>>,
    features: Vec<DriveFeature>,
    grants: Vec<String>,
    _temp: tempfile::TempDir,
}
impl Fixture {
    async fn new(access: [&str; 2]) -> Self {
        Self::with_runtime(access, false).await
    }
    async fn with_runtime(access: [&str; 2], embedded: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config = test_server_config(temp.path());
        config.repositories.clear();
        let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
        let execution = Arc::new(DeterministicExecutionBackend {
            materializer: worker_runtime::working_directory::RuntimeGitMaterializer::new(
                temp.path().join("runtime-workdirs"),
            ),
            ..Default::default()
        });
        let api = WorkspaceApi::new_with_execution_backend(config, store, execution)
            .await
            .unwrap();
        Self::on_api(api, temp, access, embedded).await
    }
    async fn on_api(
        mut api: WorkspaceApi,
        temp: tempfile::TempDir,
        access: [&str; 2],
        embedded: bool,
    ) -> Self {
        let owner = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
        let runtime_id = if embedded {
            EMBEDDED_RUNTIME_ID
        } else {
            register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
            api.runtime
                .register_or_replace(WorkdirlessFixtureRuntime::default());
            WorkdirlessFixtureRuntime::RUNTIME_ID
        };
        let mut workers = Vec::new();
        for name in ["Drive first", "Drive second"] {
            let Json(created) = create_workspace_worker(
                State(api.clone()),
                HeaderMap::new(),
                Json(CreateWorkspaceWorkerRequest {
                    runtime_id: runtime_id.into(),
                    display_name: name.into(),
                    singleton_key: None,
                    profile: Some("builtin:coder".into()),
                    ticket_assignment: None,
                    initial_submit: vec![],
                    workdir_attachments: vec![],
                    feature_connections: Default::default(),
                    control_operation_id: None,
                    execution_grant: None,
                }),
            )
            .await
            .unwrap();
            workers.push(RuntimeWorkerRef::new(created.runtime_id, created.worker_id));
        }
        let identity = Arc::new(RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap());
        configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
        let mut grants = Vec::new();
        for (worker, access) in workers.iter().zip(access) {
            assert!(
                api.store
                    .list_worker_workdir_links(TEST_WORKSPACE_ID, worker)
                    .unwrap()
                    .is_empty()
            );
            let grant = owner_call(&api, &owner, "POST", &format!("{}/grants", base()),
                Some(json!({"runtime_id":worker.runtime_id,"worker_id":worker.worker_id,"access":access}))).await;
            grants.push(grant["grant_id"].as_str().unwrap().into());
        }
        assert!(!temp.path().join(".git").exists());
        let clients: Vec<_> = workers
            .into_iter()
            .map(|worker| {
                Arc::new(RouterClient {
                    api: api.clone(),
                    identity: identity.clone(),
                    worker,
                    handle: tokio::runtime::Handle::current(),
                    lose_next_mutation_response: AtomicBool::new(false),
                    lose_next_upload_response: AtomicBool::new(false),
                    requests: Mutex::new(vec![]),
                })
            })
            .collect();
        let features = clients
            .iter()
            .map(|client| {
                DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::default()))
            })
            .collect();
        Self {
            api,
            owner,
            clients,
            features,
            grants,
            _temp: temp,
        }
    }
    async fn root(&self, worker: usize) -> Value {
        value(
            tool(&self.features[worker], "DriveRoot", json!({}))
                .await
                .unwrap(),
        )["metadata"]["entry"]
            .clone()
    }
    async fn create(&self, name: &str, content: &str) -> Value {
        let root = self.root(0).await;
        value(
            tool(
                &self.features[0],
                "DriveCreateText",
                json!({"parent":root,"name":name,"content":content}),
            )
            .await
            .unwrap(),
        )
    }
    async fn upload_text(&self, name: &str, text: &str) -> Value {
        use sha2::{Digest, Sha256};
        let root = self.root(0).await;
        let digest: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("operation", "create")
            .append_pair("request_id", &format!("user-upload-{name}"))
            .append_pair("entry_workspace_id", TEST_WORKSPACE_ID)
            .append_pair("parent_id", root["node_id"].as_str().unwrap())
            .append_pair("name", name)
            .append_pair("content_type", "text/markdown")
            .append_pair("size", &text.len().to_string())
            .append_pair("sha256", &digest)
            .finish();
        let response = build_router(self.api.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("{}/upload?{query}", base()))
                    .header("authorization", format!("Bearer {}", self.owner))
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(text.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap()
    }
    fn importer(&self, worker: usize) -> DriveFeature {
        let root = self._temp.path().join("import-source");
        fs::create_dir_all(root.join("public")).unwrap();
        fs::write(root.join("secret.bin"), b"not readable").unwrap();
        let scope = manifest::Scope::from_config(&manifest::ScopeConfig {
            allow: vec![manifest::ScopeRule {
                target: root.join("public"),
                permission: manifest::Permission::Read,
                recursive: true,
                symlink_policy: manifest::SymlinkPolicy::Resolved,
            }],
            deny: vec![],
        })
        .unwrap();
        let session = workdir::LocalWorkdirSession::materialized(
            root.clone(),
            root,
            manifest::SharedScope::new(scope),
            workdir::WorkdirSessionCapabilities::READ_ONLY,
        );
        let router = Arc::new(WorkdirSessionRouter::new());
        router
            .attach(
                workdir::WorkdirAttachmentAlias::new("source").unwrap(),
                Arc::new(session),
            )
            .unwrap();
        DriveFeature::new(self.clients[worker].clone(), router)
    }
    async fn revoke(&self, worker: usize) {
        owner_call(
            &self.api,
            &self.owner,
            "DELETE",
            &format!("{}/grants/{}", base(), self.grants[worker]),
            None,
        )
        .await;
    }
}
fn base() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/drive")
}
async fn owner_call(
    api: &WorkspaceApi,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
async fn tool(feature: &DriveFeature, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
    let definition = feature
        .tools()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    let (_, tool) = definition();
    tool.execute(
        &args.to_string(),
        ToolExecutionContext::new(uuid::Uuid::now_v7().to_string(), "drive-test", 0),
    )
    .await
}
fn value(output: ToolOutput) -> Value {
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}
fn entry(created: &Value) -> Value {
    created["entry"]["metadata"]["entry"].clone()
}
fn denied(result: Result<ToolOutput, ToolError>) {
    let error = result.unwrap_err();
    assert!(error.to_string().contains("denied"), "{error:?}");
}
fn conflict(result: Result<ToolOutput, ToolError>) {
    assert!(
        matches!(result, Err(ToolError::StructuredConflict { ref code, .. }) if code == "drive_conflict"),
        "{result:?}"
    );
}

#[tokio::test]
async fn workdirless_workers_share_markdown_through_current_read_and_write_grants() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let created = f.create("日本語 %.md", "# Shared\n").await;
    let id = entry(&created);
    assert_eq!(
        created["entry"]["metadata"]["content_type"],
        "text/markdown"
    );
    assert!(
        created["entry"]["path"]
            .as_str()
            .unwrap()
            .contains(id["node_id"].as_str().unwrap())
    );
    assert_eq!(
        value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "# Shared\n"
    );
    denied(
        tool(
            &f.features[1],
            "DriveWrite",
            json!({"entry":id,"content":"forbidden"}),
        )
        .await,
    );
    tool(&f.features[0], "DriveRead", json!({"entry":id}))
        .await
        .unwrap();
    let updated = value(
        tool(
            &f.features[0],
            "DriveWrite",
            json!({"entry":id,"content":"# Updated\n"}),
        )
        .await
        .unwrap(),
    );
    assert_ne!(
        updated["entry"]["metadata"]["last_mutation_id"],
        created["entry"]["metadata"]["last_mutation_id"]
    );
    assert_eq!(
        value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "# Updated\n"
    );
    let metadata = value(
        tool(&f.features[1], "DriveMetadata", json!({"entry":id}))
            .await
            .unwrap(),
    );
    assert_eq!(
        metadata, updated["entry"],
        "re-read proves durable mutation"
    );
}

#[tokio::test]
async fn user_uploaded_markdown_roundtrips_through_workdirless_tools_and_native_wip_at_one_identity()
 {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let original = "# ユーザー資料\n未処理\n";
    let updated_text = "# ユーザー資料\n処理済み ✓\n";
    for native in [false, true] {
        let uploaded = f
            .upload_text(if native { "native.md" } else { "tools.md" }, original)
            .await;
        let id = uploaded["entry"]["entry"].clone();
        let updated = if native {
            let tools = native_tools(&f.features[0]);
            // Resolve the production path projection, not an invented node path.
            let metadata = value(
                tool(&f.features[0], "DriveMetadata", json!({"entry":id}))
                    .await
                    .unwrap(),
            );
            let path = metadata["path"].as_str().unwrap();
            let inspected = value(
                native_tool(&tools, "Inspect", json!({"path":path,"refresh":true}))
                    .await
                    .unwrap(),
            );
            let reference = inspected["interfaces"][0]["reference"].clone();
            let read = value(
                native_tool(
                    &tools,
                    "Invoke",
                    json!({"path":path,"interface":reference,"operation":"read","arguments":{}}),
                )
                .await
                .unwrap(),
            );
            assert_eq!(read["text"], original);
            assert_eq!(read["entry"]["metadata"]["entry"], id);
            assert_eq!(
                read["entry"]["metadata"]["last_mutation_id"],
                uploaded["entry"]["last_mutation_id"]
            );
            value(native_tool(&tools, "Invoke", json!({"path":path,"interface":file_write_reference(&inspected),"operation":"write","arguments":{"content":updated_text}})).await.unwrap())["metadata"].clone()
        } else {
            let read = value(
                tool(&f.features[0], "DriveRead", json!({"entry":id}))
                    .await
                    .unwrap(),
            );
            assert_eq!(read["text"], original);
            assert_eq!(read["entry"]["metadata"]["entry"], id);
            assert_eq!(
                read["entry"]["metadata"]["last_mutation_id"],
                uploaded["entry"]["last_mutation_id"]
            );
            value(
                tool(
                    &f.features[0],
                    "DriveWrite",
                    json!({"entry":id,"content":updated_text}),
                )
                .await
                .unwrap(),
            )
        };
        assert_eq!(entry(&updated), id);
        let mutation = &updated["entry"]["metadata"]["last_mutation_id"];
        assert_ne!(*mutation, uploaded["entry"]["last_mutation_id"]);
        assert_eq!(*mutation, updated["request_id"]);
        let independent = value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap(),
        );
        assert_eq!(independent["text"], updated_text);
        assert_eq!(independent["entry"], updated["entry"]);
        let metadata = owner_call(
            &f.api,
            &f.owner,
            "GET",
            &format!(
                "{}/metadata?entry_workspace_id={TEST_WORKSPACE_ID}&id={}",
                base(),
                id["node_id"].as_str().unwrap()
            ),
            None,
        )
        .await;
        assert_eq!(metadata, updated["entry"]["metadata"]);
        let receipt = owner_call(
            &f.api,
            &f.owner,
            "GET",
            &format!(
                "{}/requests/{}",
                base(),
                updated["request_id"].as_str().unwrap()
            ),
            None,
        )
        .await;
        assert_eq!(receipt["state"], "committed");
        assert_eq!(receipt["response"]["entry"], metadata);
        let response = build_router(f.api.clone()).oneshot(Request::builder()
            .uri(format!("{}/download?entry_workspace_id={TEST_WORKSPACE_ID}&id={}&expected_mutation_id={}", base(), id["node_id"].as_str().unwrap(), mutation.as_str().unwrap()))
            .header("authorization", format!("Bearer {}", f.owner))
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "native={native}");
        assert_eq!(
            to_bytes(response.into_body(), 65536)
                .await
                .unwrap()
                .as_ref(),
            updated_text.as_bytes()
        );
    }
}

#[tokio::test]
async fn source_workdir_removal_keeps_published_drive_bytes_and_receipt() {
    let (source, binding) =
        super::ticket_worker_launch_tests::LaunchFixture::with_source_workdir().await;
    let workdir_id = binding.working_directory.id.clone();
    let f = Fixture::on_api(
        source.api.clone(),
        tempfile::tempdir().unwrap(),
        ["read_write", "read_only"],
        false,
    )
    .await;
    let bytes = b"clean\n";
    let root_path = binding.root().to_owned();
    let materialization_root = binding.working_directory_root().to_owned();
    let scope = manifest::Scope::from_config(&manifest::ScopeConfig {
        allow: vec![manifest::ScopeRule {
            target: root_path.clone(),
            permission: manifest::Permission::Read,
            recursive: true,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        }],
        deny: vec![],
    })
    .unwrap();
    let sessions = Arc::new(WorkdirSessionRouter::default());
    sessions
        .attach(
            workdir::WorkdirAttachmentAlias::new("source").unwrap(),
            Arc::new(
                workdir::LocalWorkdirSession::materialized_bound_with_environment(
                    workdir::Workdir::new(workdir_id.clone()),
                    root_path.clone(),
                    root_path.clone(),
                    manifest::SharedScope::new(scope),
                    workdir::WorkdirSessionCapabilities::READ_ONLY,
                    binding.command_environment(),
                    binding.session_resources().unwrap(),
                ),
            ),
        )
        .unwrap();
    let importer = DriveFeature::new(f.clients[0].clone(), sessions.clone());
    let root = f.root(0).await;
    let saved = value(tool(&importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":"README.md","parent":root,"name":"retained.md","content_type":"text/markdown"})).await.unwrap());
    sessions.close_all().await.unwrap();
    drop(importer);
    drop(sessions);
    drop(binding);
    assert!(root_path.join("README.md").exists());
    let api = source.api.clone();
    let removal_id = workdir_id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        execute_workdir_removal(
            &api,
            &removal_id,
            "account:drive-test",
            "source no longer needed",
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        removed.disposition,
        server_api::WorkingDirectoryRemovalDisposition::Removed,
        "{removed:?}"
    );
    assert!(!removed.retryable);
    assert!(
        !materialization_root.exists(),
        "actual source materialization must be gone"
    );
    assert!(
        f.api
            .store
            .get_workdir_registry(TEST_WORKSPACE_ID, &workdir_id)
            .unwrap()
            .is_none()
    );
    let fresh = DriveFeature::new(
        f.clients[1].clone(),
        Arc::new(WorkdirSessionRouter::default()),
    );
    let read = value(
        tool(&fresh, "DriveRead", json!({"entry":entry(&saved)}))
            .await
            .unwrap(),
    );
    assert_eq!(read["entry"], saved["entry"]);
    assert_eq!(read["text"].as_str().unwrap().as_bytes(), bytes);
    let receipt = value(
        tool(
            &fresh,
            "DriveRequestStatus",
            json!({"request_id":saved["request_id"]}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(receipt["state"], "committed");
    assert_eq!(receipt["response"]["entry"], saved["entry"]["metadata"]);
}

#[tokio::test]
async fn worker_removal_preserves_drive_reference_for_explicitly_granted_replacement() {
    // Real embedded Runtime lifecycle/retention; only controller execution is
    // deterministic. No Workdir or external model is involved in this case.
    let f = Fixture::with_runtime(["read_write", "read_only"], true).await;
    let saved = f.create("retained.md", "# Beyond the Worker\n").await;
    let id = entry(&saved);
    let old = &f.clients[0].worker;
    let worker = WorkspaceWorker::resolve_async(&f.api, &old.runtime_id, &old.worker_id)
        .await
        .unwrap();
    let stopped = worker
        .stop(
            &WorkerOperationContext::Backend,
            WorkerLifecycleRequest {
                reason: Some("Drive lifetime test".into()),
                ticket_assignment: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    assert_eq!(f.api.runtime.worker(old).unwrap().state, "stopped");
    seed_worker_session_for_cleanup(f._temp.path(), old);
    let plan = build_runtime_cleanup_plan(&f.api, &old.runtime_id).unwrap();
    let candidate = plan
        .workers
        .iter()
        .find(|c| c.runtime_worker_id == old.worker_id)
        .unwrap();
    assert_eq!(candidate.blocking_reason, None);
    worker
        .delete_from_plan(&WorkerOperationContext::Backend, candidate)
        .await
        .unwrap();
    assert!(f.api.runtime.worker(old).is_err());
    let client = f.clients[0].clone();
    let capability = tokio::task::spawn_blocking(move || {
        client.request(
            WorkspaceRequestMethod::Get,
            format!("{}/capabilities", base()),
            vec![],
            65536,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(!(200..300).contains(&capability.status));
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, old)
            .unwrap()
            .is_none()
    );
    denied(tool(&f.features[0], "DriveRead", json!({"entry":id})).await);
    let Json(created) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_RUNTIME_ID.into(),
            display_name: "Replacement reader".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    assert_ne!(created.worker_id, old.worker_id);
    let client = Arc::new(RouterClient {
        api: f.api.clone(),
        identity: f.clients[0].identity.clone(),
        worker: RuntimeWorkerRef::new(created.runtime_id, created.worker_id),
        handle: tokio::runtime::Handle::current(),
        lose_next_mutation_response: AtomicBool::new(false),
        lose_next_upload_response: AtomicBool::new(false),
        requests: Mutex::new(vec![]),
    });
    let replacement = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::default()));
    denied(tool(&replacement, "DriveRead", json!({"entry":id})).await);
    owner_call(&f.api, &f.owner, "POST", &format!("{}/grants", base()), Some(json!({"runtime_id":client.worker.runtime_id,"worker_id":client.worker.worker_id,"access":"read_only"}))).await;
    let read = value(
        tool(&replacement, "DriveRead", json!({"entry":id}))
            .await
            .unwrap(),
    );
    assert_eq!(read["entry"], saved["entry"]);
    assert_eq!(read["text"], "# Beyond the Worker\n");
    let receipt = owner_call(
        &f.api,
        &f.owner,
        "GET",
        &format!(
            "{}/requests/{}",
            base(),
            saved["request_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(receipt["state"], "committed");
    assert_eq!(receipt["response"]["entry"], saved["entry"]["metadata"]);
    // Stop remaining deterministic controllers before dropping isolated resources.
    for target in [&f.clients[1].worker, &client.worker] {
        let stopped = f
            .api
            .runtime
            .stop_worker(
                target,
                WorkerLifecycleRequest {
                    reason: None,
                    ticket_assignment: None,
                },
            )
            .unwrap();
        assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    }
}

#[tokio::test]
async fn simultaneous_worker_edits_publish_exactly_one_last_mutation_id_and_preserve_the_loser_conflict()
 {
    let f = Fixture::new(["read_write", "read_write"]).await;
    let id = entry(&f.create("simultaneous.md", "original").await);
    for feature in &f.features {
        tool(feature, "DriveRead", json!({"entry":id}))
            .await
            .unwrap();
    }
    let (first, second) = tokio::join!(
        tool(
            &f.features[0],
            "DriveEdit",
            json!({"entry":id,"old_string":"original","new_string":"first"})
        ),
        tool(
            &f.features[1],
            "DriveEdit",
            json!({"entry":id,"old_string":"original","new_string":"second"})
        ),
    );
    let winner = match (first, second) {
        (Ok(_output), other) => {
            conflict(other);
            "first"
        }
        (other, Ok(_output)) => {
            conflict(other);
            "second"
        }
        (first, second) => panic!("expected one DB winner and one conflict: {first:?}, {second:?}"),
    };
    let observed = value(
        tool(&f.features[0], "DriveRead", json!({"entry":id}))
            .await
            .unwrap(),
    );
    assert_eq!(observed["text"], winner);
}

#[tokio::test]
async fn stale_worker_observations_cannot_write_edit_relocate_or_delete_newer_last_mutation_id() {
    let f = Fixture::new(["read_write", "read_write"]).await;
    let id = entry(&f.create("cas.md", "old").await);
    let root = f.root(0).await;
    for worker in &f.features {
        tool(worker, "DriveRead", json!({"entry":id}))
            .await
            .unwrap();
    }
    tool(
        &f.features[0],
        "DriveWrite",
        json!({"entry":id,"content":"new"}),
    )
    .await
    .unwrap();
    for (name, args) in [
        ("DriveWrite", json!({"entry":id,"content":"lost"})),
        (
            "DriveEdit",
            json!({"entry":id,"old_string":"new","new_string":"lost"}),
        ),
        (
            "DriveRelocate",
            json!({"entry":id,"parent":root,"name":"lost.md"}),
        ),
        ("DriveDelete", json!({"entry":id})),
    ] {
        conflict(tool(&f.features[1], name, args).await);
    }
    let current = value(
        tool(&f.features[1], "DriveRead", json!({"entry":id}))
            .await
            .unwrap(),
    );
    assert_eq!(current["text"], "new");
    assert_eq!(current["entry"]["metadata"]["name"], "cas.md");
    tool(
        &f.features[1],
        "DriveWrite",
        json!({"entry":id,"content":"observed"}),
    )
    .await
    .unwrap();
    assert_eq!(
        value(
            tool(&f.features[0], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "observed"
    );
}

#[tokio::test]
async fn edit_requires_unique_old_string_unless_replace_all_is_explicit() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let id = entry(&f.create("edit.md", "same same").await);
    tool(&f.features[0], "DriveRead", json!({"entry":id}))
        .await
        .unwrap();
    for old in ["same", "absent", ""] {
        let result = tool(
            &f.features[0],
            "DriveEdit",
            json!({"entry":id,"old_string":old,"new_string":"new"}),
        )
        .await;
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(_))),
            "{result:?}"
        );
    }
    tool(
        &f.features[0],
        "DriveEdit",
        json!({"entry":id,"old_string":"same","new_string":"new","replace_all":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "new new"
    );
}

#[tokio::test]
async fn revocation_invalidates_cached_metadata_reads_writes_and_receipt_queries() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let created = f.create("revoke.md", "keep").await;
    let id = entry(&created);
    tool(&f.features[0], "DriveRead", json!({"entry":id}))
        .await
        .unwrap();
    f.revoke(0).await;
    for (name, args) in [
        ("DriveRoot", json!({})),
        ("DriveMetadata", json!({"entry":id})),
        ("DriveRead", json!({"entry":id})),
        ("DriveWrite", json!({"entry":id,"content":"lost"})),
        ("DriveDelete", json!({"entry":id})),
        ("DriveList", json!({})),
        ("DriveSearch", json!({"query":"revoke"})),
        (
            "DriveRequestStatus",
            json!({"request_id":created["request_id"]}),
        ),
    ] {
        denied(tool(&f.features[0], name, args).await);
    }
    assert_eq!(
        value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "keep"
    );
}

#[tokio::test]
async fn foreign_and_malformed_tool_refs_never_resolve_same_number_in_current_workspace() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let id = entry(&f.create("safe.md", "safe").await);
    let foreign = json!({"workspace_id":"foreign","node_id":id["node_id"]});
    for name in [
        "DriveRead",
        "DriveMetadata",
        "DriveDelete",
        "DriveViewImage",
    ] {
        denied(tool(&f.features[0], name, json!({"entry":foreign})).await);
    }
    for node in [
        json!(1),
        json!("01"),
        json!("0"),
        json!("../1"),
        json!("9223372036854775808"),
    ] {
        let result = tool(
            &f.features[0],
            "DriveRead",
            json!({"entry":{"workspace_id":TEST_WORKSPACE_ID,"node_id":node}}),
        )
        .await;
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(_))),
            "{result:?}"
        );
    }
    assert_eq!(
        value(
            tool(&f.features[1], "DriveRead", json!({"entry":id}))
                .await
                .unwrap()
        )["text"],
        "safe"
    );
}

#[tokio::test]
async fn relocate_keeps_stable_identity_and_recreation_never_revives_deleted_reference() {
    let f = Fixture::new(["read_write", "read_write"]).await;
    let created = f.create("before.md", "content").await;
    let id = entry(&created);
    let root = f.root(0).await;
    tool(&f.features[0], "DriveMetadata", json!({"entry":id}))
        .await
        .unwrap();
    let renamed = value(
        tool(
            &f.features[0],
            "DriveRelocate",
            json!({"entry":id,"parent":root,"name":"after.md"}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(renamed["entry"]["path"], created["entry"]["path"]);
    assert_eq!(entry(&renamed), id);
    for feature in &f.features {
        tool(feature, "DriveMetadata", json!({"entry":id}))
            .await
            .unwrap();
    }
    tool(&f.features[0], "DriveDelete", json!({"entry":id}))
        .await
        .unwrap();
    let recreated = f.create("after.md", "replacement").await;
    assert_ne!(entry(&recreated), id);
    for (name, args) in [
        ("DriveRead", json!({"entry":id})),
        ("DriveWrite", json!({"entry":id,"content":"lost"})),
        ("DriveDelete", json!({"entry":id})),
    ] {
        let result = tool(&f.features[1], name, args).await;
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
    assert_eq!(
        value(
            tool(
                &f.features[1],
                "DriveRead",
                json!({"entry":entry(&recreated)})
            )
            .await
            .unwrap()
        )["text"],
        "replacement"
    );
}

#[tokio::test]
async fn unsafe_names_and_oversized_text_cannot_publish_and_reads_truncate_at_utf8_boundary() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let root = f.root(0).await;
    for name in ["", ".", "..", "../escape", "a/b", "a\\b", "bad\nname"] {
        let result = tool(
            &f.features[0],
            "DriveCreateText",
            json!({"parent":root,"name":name,"content":"bad"}),
        )
        .await;
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(_))),
            "name={name:?}: {result:?}"
        );
    }
    let result = tool(&f.features[0], "DriveCreateText", json!({"parent":root,"name":"large.md","content":"a".repeat(server_api::DRIVE_TEXT_MAX_BYTES + 1)})).await;
    assert!(
        matches!(result, Err(ToolError::InvalidArgument(_))),
        "{result:?}"
    );
    assert!(
        value(tool(&f.features[0], "DriveList", json!({})).await.unwrap())["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let id = entry(&f.create("bounded.md", "ééé").await);
    let read = value(
        tool(
            &f.features[1],
            "DriveRead",
            json!({"entry":id,"max_bytes":3}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(read["text"], "é");
    assert_eq!(read["truncated"], true);
    for limit in [0, 65537] {
        let result = tool(
            &f.features[1],
            "DriveRead",
            json!({"entry":id,"max_bytes":limit}),
        )
        .await;
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(_))),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn omitted_discovery_limits_use_backend_maximum_pages_in_tools_and_native_wip() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    // This case owns discovery defaults, not repeated mutation routing. Seed through
    // the real Drive owner (including blobs/receipts); reads below retain signed tools/WIP.
    let drive = f.api.drive().await.unwrap();
    let expected = tokio::task::spawn_blocking(move || {
        let root = drive.root().unwrap().id;
        let mut expected = std::collections::BTreeSet::new();
        for index in 0..201 {
            let created = drive
                .mutate(
                    &format!("default-page-seed-{index:03}"),
                    "pagination-fixture",
                    workspace_drive::Mutation::CreateFile {
                        parent: root,
                        name: format!("default-page-{index:03}.md"),
                        content_type: "text/markdown".into(),
                        bytes: b"bounded".to_vec(),
                    },
                )
                .unwrap();
            expected.insert(String::from(created.node.id));
        }
        expected
    })
    .await
    .unwrap();
    let native = native_tools(&f.features[1]);
    let root = value(
        native_tool(&native, "Inspect", json!({"path":"/drive","refresh":true}))
            .await
            .unwrap(),
    );
    let reference = root["interfaces"][0]["reference"].clone();
    for use_native in [false, true] {
        for (name, operation, mut args, maximum) in [
            (
                "DriveList",
                "list",
                json!({}),
                server_api::DRIVE_PAGE_MAX_LIMIT as usize,
            ),
            (
                "DriveSearch",
                "search",
                json!({"query":"default-page-"}),
                server_api::DRIVE_SEARCH_MAX_LIMIT as usize,
            ),
        ] {
            let mut actual = Vec::new();
            for page_index in 0..2 {
                let output = if use_native {
                    native_tool(&native, "Invoke", json!({"path":"/drive","interface":reference,"operation":operation,"arguments":args})).await
                } else {
                    tool(&f.features[1], name, args.clone()).await
                };
                let page = value(output.unwrap());
                let entries = page["entries"].as_array().unwrap();
                assert_eq!(
                    entries.len(),
                    if page_index == 0 {
                        maximum
                    } else {
                        201 - maximum
                    },
                    "native={use_native} operation={operation}"
                );
                actual.extend(entries.iter().map(|e| {
                    e["metadata"]["entry"]["node_id"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                }));
                if page_index == 0 {
                    assert!(page["next_after"].is_string());
                    args["after"] = page["next_after"].clone();
                } else {
                    assert!(page["next_after"].is_null());
                }
            }
            assert_eq!(actual.len(), expected.len());
            assert_eq!(
                actual
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>(),
                expected
            );
        }
    }
}

#[tokio::test]
async fn list_and_search_pages_are_bounded_disjoint_and_cursors_follow_backend_scope() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let mut expected = Vec::new();
    for name in ["page-a.md", "page-b.md", "page-c.md"] {
        expected.push(entry(&f.create(name, "needle").await));
    }
    for (name, mut args) in [
        ("DriveList", json!({"limit":2})),
        ("DriveSearch", json!({"query":"page-","limit":2})),
    ] {
        let first = value(tool(&f.features[1], name, args.clone()).await.unwrap());
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        assert!(first["next_after"].is_string());
        args["after"] = first["next_after"].clone();
        let second = value(tool(&f.features[1], name, args).await.unwrap());
        assert_eq!(second["entries"].as_array().unwrap().len(), 1);
        assert!(second["next_after"].is_null());
        let actual: Vec<_> = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["entries"].as_array().unwrap())
            .map(|e| e["metadata"]["entry"].clone())
            .collect();
        assert_eq!(actual.len(), expected.len());
        for id in &expected {
            assert_eq!(actual.iter().filter(|e| *e == id).count(), 1);
        }
        if name == "DriveSearch" {
            let result = tool(
                &f.features[1],
                name,
                json!({"query":"different","limit":2,"after":first["next_after"]}),
            )
            .await;
            // T-722 cursors bind Workspace/node position, not query text. A
            // new query at the same position is valid and cannot widen authority.
            assert!(
                value(result.unwrap())["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    assert!(
        value(
            tool(&f.features[1], "DriveSearch", json!({"query":"needle"}))
                .await
                .unwrap()
        )["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        value(
            tool(
                &f.features[1],
                "DriveSearch",
                json!({"query":"needle","include_text":true})
            )
            .await
            .unwrap()
        )["entries"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for (name, args) in [
        ("DriveList", json!({"limit":201})),
        ("DriveSearch", json!({"query":"page","limit":129})),
    ] {
        let result = tool(&f.features[1], name, args).await;
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(_))),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn lost_mutation_response_is_resolved_by_receipt_without_resending_effect() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    f.clients[0]
        .lose_next_mutation_response
        .store(true, Ordering::SeqCst);
    let created = f.create("receipt.md", "once").await;
    let status = value(
        tool(
            &f.features[0],
            "DriveRequestStatus",
            json!({"request_id":created["request_id"]}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(status["state"], "committed");
    assert_eq!(status["response"]["entry"]["entry"], entry(&created));
    assert_eq!(
        f.clients[0]
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p)| m == "POST" && p.ends_with("/mutate"))
            .count(),
        1
    );
    let listed = value(tool(&f.features[1], "DriveList", json!({})).await.unwrap());
    assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed["entries"][0]["metadata"]["last_mutation_id"],
        created["entry"]["metadata"]["last_mutation_id"]
    );
}

#[tokio::test]
async fn scoped_workdir_import_publishes_document_and_image_bytes_with_bounded_reads() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let importer = f.importer(0);
    let root = f.root(0).await;
    // A binary document larger than one chunk exercises the real bytes route,
    // not the text API or a UTF-8-only source fallback.
    let document: Vec<u8> = (0..70_003).map(|n| (n % 256) as u8).collect();
    let image = include_bytes!("../../../../tools/tests/fixtures/view-image/valid.png").to_vec();
    for (name, media, bytes) in [
        ("document.bin", "application/octet-stream", document),
        ("image.png", "image/png", image),
    ] {
        fs::write(
            f._temp.path().join("import-source/public").join(name),
            &bytes,
        )
        .unwrap();
        let saved = value(tool(&importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":format!("public/{name}"),"parent":root,"name":name,"content_type":media})).await.unwrap());
        let id = entry(&saved);
        let metadata = value(
            tool(&f.features[1], "DriveMetadata", json!({"entry":id}))
                .await
                .unwrap(),
        );
        assert_eq!(
            metadata, saved["entry"],
            "re-read through independent Worker must see persisted entry"
        );
        assert_eq!(metadata["metadata"]["size"], bytes.len());
        assert_eq!(metadata["metadata"]["content_type"], media);
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("entry_workspace_id", TEST_WORKSPACE_ID)
            .append_pair("id", id["node_id"].as_str().unwrap())
            .append_pair(
                "expected_mutation_id",
                metadata["metadata"]["last_mutation_id"].as_str().unwrap(),
            )
            .append_pair("offset", "0")
            .append_pair("length", "17")
            .finish();
        let client = f.clients[1].clone();
        let response = tokio::task::spawn_blocking(move || {
            client.execute_binary(WorkspaceBinaryRequest {
                method: WorkspaceRequestMethod::Get,
                path: format!("{}/read-chunk?{query}", base()),
                body: None,
                max_response_bytes: 17,
            })
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, bytes[..17.min(bytes.len())]);
        // Full download independently verifies the persisted blob and digest.
        let path = format!(
            "{}/download?entry_workspace_id={TEST_WORKSPACE_ID}&id={}",
            base(),
            id["node_id"].as_str().unwrap()
        );
        let response = build_router(f.api.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {}", f.owner))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(
                response.into_body(),
                server_api::DRIVE_FILE_MAX_BYTES as usize
            )
            .await
            .unwrap()
            .as_ref(),
            bytes
        );
        if media == "image/png" {
            let output = tool(&f.features[1], "DriveViewImage", json!({"entry":id}))
                .await
                .unwrap();
            let [Attachment::Image(image)] = output.attachments.as_slice() else {
                panic!("imported image attachment required");
            };
            assert_eq!(image.data(), bytes);
        }
    }
    assert_eq!(
        value(tool(&f.features[1], "DriveList", json!({})).await.unwrap())["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn workdir_import_rejects_unknown_alias_and_unreadable_or_nonlogical_source_before_upload() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let importer = f.importer(0);
    let root = f.root(0).await;
    let source = f._temp.path().join("import-source");
    fs::write(source.join("public/allowed.bin"), b"allowed").unwrap();
    fs::write(
        source.join("public/oversized.bin"),
        vec![0; server_api::DRIVE_FILE_MAX_BYTES as usize + 1],
    )
    .unwrap();
    for (alias, path) in [
        ("foreign", "public/allowed.bin".to_string()),
        ("unknown", "public/allowed.bin".to_string()),
        ("source", "secret.bin".to_string()),
        ("source", "public/missing.bin".to_string()),
        ("source", "../secret.bin".to_string()),
        (
            "source",
            source.join("public/allowed.bin").display().to_string(),
        ),
        ("source", "public/oversized.bin".to_string()),
    ] {
        let result = tool(&importer, "DriveSaveWorkdir", json!({"target_workdir":alias,"path":path,"parent":root,"name":"rejected.bin","content_type":"application/octet-stream"})).await;
        assert!(
            matches!(
                result,
                Err(ToolError::ExecutionFailed(_)) | Err(ToolError::InvalidArgument(_))
            ),
            "alias={alias}, path={path}: {result:?}"
        );
    }
    assert!(
        !f.clients[0]
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(method, _)| method == "PUT"),
        "denied source must never reach upload"
    );
    assert!(
        value(tool(&f.features[1], "DriveList", json!({})).await.unwrap())["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workdir_source_read_permission_does_not_grant_drive_destination_write() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let readonly_importer = f.importer(1);
    let writer_importer = f.importer(0);
    fs::write(
        f._temp.path().join("import-source/public/source.bin"),
        b"readable source",
    )
    .unwrap();
    let root = f.root(0).await;
    denied(tool(&readonly_importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":"public/source.bin","parent":root,"name":"denied.bin","content_type":"application/octet-stream"})).await);
    let foreign = json!({"workspace_id":"foreign","node_id":root["node_id"]});
    denied(tool(&writer_importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":"public/source.bin","parent":foreign,"name":"foreign.bin","content_type":"application/octet-stream"})).await);
    f.revoke(0).await;
    denied(tool(&writer_importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":"public/source.bin","parent":root,"name":"revoked.bin","content_type":"application/octet-stream"})).await);
    assert!(
        value(tool(&f.features[1], "DriveList", json!({})).await.unwrap())["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn lost_binary_upload_reply_reconciles_same_request_without_duplicate_publication() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let importer = f.importer(0);
    let bytes = b"\x00\xffbinary receipt";
    fs::write(
        f._temp.path().join("import-source/public/receipt.bin"),
        bytes,
    )
    .unwrap();
    let root = f.root(0).await;
    f.clients[0]
        .lose_next_upload_response
        .store(true, Ordering::SeqCst);
    let saved = value(tool(&importer, "DriveSaveWorkdir", json!({"target_workdir":"source","path":"public/receipt.bin","parent":root,"name":"receipt.bin","content_type":"application/octet-stream"})).await.unwrap());
    let request_id = saved["request_id"].as_str().unwrap();
    let status = value(
        tool(
            &importer,
            "DriveRequestStatus",
            json!({"request_id":request_id}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(status["state"], "committed");
    assert_eq!(status["response"]["entry"]["entry"], entry(&saved));
    let requests = f.clients[0].requests.lock().unwrap().clone();
    let uploads: Vec<_> = requests
        .iter()
        .filter(|(method, path)| method == "PUT" && path.contains("/upload?"))
        .collect();
    assert_eq!(
        uploads.len(),
        1,
        "upload must not be resent after unknown transport outcome"
    );
    assert!(uploads[0].1.contains(&format!("request_id={request_id}")));
    assert!(
        requests
            .iter()
            .any(|(method, path)| method == "GET"
                && path.ends_with(&format!("/requests/{request_id}"))),
        "receipt lookup must retain original upload request identity"
    );
    let listed = value(tool(&f.features[1], "DriveList", json!({})).await.unwrap());
    assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    assert_eq!(listed["entries"][0], saved["entry"]);
}

#[derive(Clone)]
struct NoLlm;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for NoLlm {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError> {
        panic!("native tool fixture must not call an LLM")
    }
}
fn file_write_reference(observed: &Value) -> Value {
    observed["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["reference"]["name"] == "yoi.file/write-text/v1")
        .expect("write capability")["reference"]
        .clone()
}
fn native_tools(
    feature: &DriveFeature,
) -> std::collections::BTreeMap<String, Arc<dyn agen::tool::Tool>> {
    native_tools_with_checkouts(feature, None)
}
fn native_tools_with_checkouts(
    feature: &DriveFeature,
    router: Option<Arc<WorkdirSessionRouter>>,
) -> std::collections::BTreeMap<String, Arc<dyn agen::tool::Tool>> {
    let mut mounts = worker::wip::WipMountRegistry::new();
    worker::feature::builtin::drive::wip::mount_drive_wip(&mut mounts, feature).unwrap();
    if let Some(router) = router {
        worker::checkout::mount_checkouts(&mut mounts, router, tools::Tracker::new(), None)
            .unwrap();
    }
    let mut engine = agen::Engine::<_, agen::state::Mutable, ()>::new_annotated(NoLlm);
    engine.register_tools(
        feature
            .tools()
            .into_iter()
            .map(|(_, definition)| definition),
    );
    worker::wip::install_wip_mode_with_mounts(&mut engine, None, "drive-reader".into(), mounts)
        .unwrap();
    let handle = engine.tool_server_handle();
    handle.flush_pending();
    assert!(
        handle.get_tool("DriveRead").is_none(),
        "native installation must replace compatibility Drive tools"
    );
    ["Tree", "Inspect", "Invoke"]
        .into_iter()
        .map(|name| (name.into(), handle.get_tool(name).unwrap().1))
        .collect()
}
async fn native_tool(
    tools: &std::collections::BTreeMap<String, Arc<dyn agen::tool::Tool>>,
    name: &str,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    tools[name]
        .execute(
            &args.to_string(),
            ToolExecutionContext::new(uuid::Uuid::now_v7().to_string(), "native-drive-test", 0),
        )
        .await
}

#[tokio::test]
async fn native_tree_does_not_index_entries_but_direct_inspect_and_invoke_use_current_grants() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let created = f.create("native.md", "native content").await;
    let path = created["entry"]["path"].as_str().unwrap().to_string();
    let tools = native_tools(&f.features[1]);
    let tree = value(
        native_tool(
            &tools,
            "Tree",
            json!({"path":"/drive","depth":8,"refresh":true}),
        )
        .await
        .unwrap(),
    );
    assert!(
        !tree.to_string().contains(&path),
        "Drive entries are non-indexable: {tree}"
    );
    let inspected = value(
        native_tool(&tools, "Inspect", json!({"path":path,"refresh":true}))
            .await
            .unwrap(),
    );
    assert!(
        inspected["object_signature"]
            .as_str()
            .unwrap()
            .contains("native.md")
    );
    let reference = inspected["interfaces"][0]["reference"].clone();
    let read = value(
        native_tool(
            &tools,
            "Invoke",
            json!({"path":path,"interface":reference,"operation":"read","arguments":{}}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(read["text"], "native content");
    let denied_write = native_tool(&tools, "Invoke", json!({"path":path,"interface":reference,"operation":"write","arguments":{"content":"lost"}})).await;
    assert!(denied_write.is_err());
    assert!(
        !inspected["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["reference"]["name"] == "yoi.file/write-text/v1")
    );
    f.revoke(1).await;
    assert!(
        native_tool(
            &tools,
            "Invoke",
            json!({"path":path,"interface":reference,"operation":"read","arguments":{}})
        )
        .await
        .is_err(),
        "cached native observation is not authority"
    );
    assert!(
        native_tool(&tools, "Inspect", json!({"path":path,"refresh":true}))
            .await
            .is_err(),
        "revoked entry must not publish"
    );
    assert_eq!(
        value(
            tool(
                &f.features[0],
                "DriveRead",
                json!({"entry":entry(&created)})
            )
            .await
            .unwrap()
        )["text"],
        "native content"
    );
}

#[tokio::test]
async fn native_create_and_write_use_client_managed_committed_requests_and_reject_stale_worker() {
    let f = Fixture::new(["read_write", "read_write"]).await;
    let first = native_tools(&f.features[0]);
    let second = native_tools(&f.features[1]);
    let root = value(
        native_tool(&first, "Inspect", json!({"path":"/drive","refresh":true}))
            .await
            .unwrap(),
    );
    let created = value(native_tool(&first, "Invoke", json!({"path":"/drive","interface":root["interfaces"][0]["reference"],"operation":"create_text","arguments":{"name":"native-cas.md","content":"original"}})).await.unwrap());
    let path = created["entry"]["path"].as_str().unwrap();
    let observed_first = value(
        native_tool(&first, "Inspect", json!({"path":path,"refresh":true}))
            .await
            .unwrap(),
    );
    let observed_second = value(
        native_tool(&second, "Inspect", json!({"path":path,"refresh":true}))
            .await
            .unwrap(),
    );
    let updated = value(native_tool(&first, "Invoke", json!({"path":path,"interface":file_write_reference(&observed_first),"operation":"write","arguments":{"content":"committed"}})).await.unwrap())["metadata"].clone();
    assert_ne!(
        updated["entry"]["metadata"]["last_mutation_id"],
        created["entry"]["metadata"]["last_mutation_id"]
    );
    let error = native_tool(&second, "Invoke", json!({"path":path,"interface":file_write_reference(&observed_second),"operation":"write","arguments":{"content":"lost"}})).await.unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("validator"),
        "stale native mutation must report validator conflict: {error:?}"
    );
    assert_eq!(
        value(
            tool(
                &f.features[1],
                "DriveRead",
                json!({"entry":entry(&created)})
            )
            .await
            .unwrap()
        )["text"],
        "committed"
    );
}

#[tokio::test]
async fn image_attachment_retains_original_bytes_after_drive_update_and_revocation() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new(["read_write", "read_only"]).await;
    let root = f.root(0).await;
    let original =
        include_bytes!("../../../../tools/tests/fixtures/view-image/valid.png").as_slice();
    let digest = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let upload = |operation: &str,
                  request: &str,
                  bytes: &[u8],
                  id: Option<&Value>,
                  last_mutation_id: Option<&str>| {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("operation", operation)
            .append_pair("request_id", request)
            .append_pair("entry_workspace_id", TEST_WORKSPACE_ID)
            .append_pair("content_type", "image/png")
            .append_pair("size", &bytes.len().to_string())
            .append_pair("sha256", &digest(bytes));
        if let Some(id) = id {
            query
                .append_pair("id", id["node_id"].as_str().unwrap())
                .append_pair("expected_mutation_id", last_mutation_id.unwrap());
        } else {
            query
                .append_pair("parent_id", root["node_id"].as_str().unwrap())
                .append_pair("name", "image.png");
        }
        format!("{}/upload?{}", base(), query.finish())
    };
    async fn put(f: &Fixture, path: String, bytes: &[u8]) -> Value {
        let response = build_router(f.api.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(path)
                    .header("authorization", format!("Bearer {}", f.owner))
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(bytes.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap()
    }
    let saved = put(
        &f,
        upload("create", "image-create", original, None, None),
        original,
    )
    .await;
    let id = saved["entry"]["entry"].clone();
    let output = tool(&f.features[1], "DriveViewImage", json!({"entry":id}))
        .await
        .unwrap();
    let [Attachment::Image(image)] = output.attachments.as_slice() else {
        panic!("image attachment required");
    };
    assert_eq!(image.mime_type(), "image/png");
    assert_eq!(image.data(), original);
    let replacement =
        include_bytes!("../../../../tools/tests/fixtures/view-image/valid.gif").as_slice();
    put(
        &f,
        upload(
            "update",
            "image-update",
            replacement,
            Some(&id),
            saved["entry"]["last_mutation_id"].as_str(),
        ),
        replacement,
    )
    .await;
    let new = tool(&f.features[1], "DriveViewImage", json!({"entry":id}))
        .await
        .unwrap();
    let [Attachment::Image(new_image)] = new.attachments.as_slice() else {
        panic!("image attachment required");
    };
    assert_eq!(new_image.data(), replacement);
    f.revoke(1).await;
    denied(tool(&f.features[1], "DriveViewImage", json!({"entry":id})).await);
    assert_eq!(
        image.data(),
        original,
        "history attachment must not fetch latest Drive content"
    );
}

#[tokio::test]
async fn common_file_inspect_invoke_crosses_drive_and_checkout_without_transferring_authority() {
    let f = Fixture::new(["read_write", "read_only"]).await;
    let created = f.create("common.md", "first\n雪 needle\nlast\n").await;
    let path = created["entry"]["path"].as_str().unwrap();
    let checkout = f._temp.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    fs::write(checkout.join("file"), "first\n雪 needle\nlast\n").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            workdir::WorkdirAttachmentAlias::new("main").unwrap(),
            Arc::new(workdir::LocalWorkdirSession::materialized(
                checkout.clone(),
                checkout.clone(),
                manifest::SharedScope::new(manifest::Scope::writable(&checkout).unwrap()),
                workdir::WorkdirSessionCapabilities::READ_WRITE,
            )),
        )
        .unwrap();
    let tools = native_tools_with_checkouts(&f.features[0], Some(router));
    let drive = value(
        native_tool(&tools, "Inspect", json!({"path":path}))
            .await
            .unwrap(),
    );
    let local = value(
        native_tool(&tools, "Inspect", json!({"path":"/checkouts/main/file"}))
            .await
            .unwrap(),
    );
    for name in [
        "yoi.file/read-lines/v1",
        "yoi.file/search-text/v1",
        "yoi.file/edit-text/v1",
        "yoi.file/write-text/v1",
    ] {
        let selected = drive["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["reference"]["name"] == name)
            .unwrap();
        let reference = &selected["reference"];
        assert_eq!(reference["scope"], "/");
        assert!(
            local["interfaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["reference"] == *reference)
        );
        let signature = value(
            native_tool(&tools, "Inspect", json!({"path":selected["path"]}))
                .await
                .unwrap(),
        );
        assert!(
            signature["interface_signature"]
                .as_str()
                .unwrap()
                .contains(name)
        );
        let (operation, arguments) = match name {
            "yoi.file/read-lines/v1" => ("read", json!({"offset":1,"limit":1})),
            "yoi.file/search-text/v1" => ("search", json!({"pattern":"needle","limit":2})),
            "yoi.file/edit-text/v1" => {
                ("edit", json!({"old_string":"needle","new_string":"found"}))
            }
            _ => ("write", json!({"content":"saved\n"})),
        };
        let mut results = vec![];
        for target in [path, "/checkouts/main/file"] {
            results.push(value(native_tool(&tools,"Invoke",json!({"path":target,"interface":reference,"operation":operation,"arguments":arguments})).await.unwrap()));
        }
        if matches!(operation, "read" | "search") {
            assert_eq!(results[0], results[1]);
        } else {
            assert!(results.iter().all(|v| v.get("metadata").is_some()));
        }
    }
    assert_eq!(
        fs::read_to_string(checkout.join("file")).unwrap(),
        "saved\n"
    );
    assert_eq!(
        value(
            tool(
                &f.features[1],
                "DriveRead",
                json!({"entry":entry(&created)})
            )
            .await
            .unwrap()
        )["text"],
        "saved\n"
    );
    // A capability query is not a grant: drop the writer to readonly after the common write was cached.
    f.revoke(0).await;
    owner_call(&f.api,&f.owner,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":f.clients[0].worker.runtime_id,"worker_id":f.clients[0].worker.worker_id,"access":"read_only"}))).await;
    assert!(native_tool(&tools,"Invoke",json!({"path":path,"interface":file_write_reference(&drive),"operation":"write","arguments":{"content":"lost"}})).await.is_err());
    let refreshed = value(
        native_tool(&tools, "Inspect", json!({"path":path,"refresh":true}))
            .await
            .unwrap(),
    );
    assert!(
        !refreshed["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["reference"]["name"] == "yoi.file/write-text/v1")
    );
    let common_read = json!({"scope":"/","name":"yoi.file/read-lines/v1"});
    assert_eq!(
        value(
            native_tool(
                &tools,
                "Invoke",
                json!({"path":path,"interface":common_read,"operation":"read","arguments":{}})
            )
            .await
            .unwrap()
        )["content"],
        "     1\tsaved\n"
    );
    assert_eq!(
        value(
            tool(
                &f.features[1],
                "DriveRead",
                json!({"entry":entry(&created)})
            )
            .await
            .unwrap()
        )["text"],
        "saved\n"
    );
}
