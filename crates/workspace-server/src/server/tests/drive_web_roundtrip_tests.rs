//! T-724's real API/SQLite/FeatureStorage/LocalFileSystem boundary, separate
//! from browser fixtures. The Deno child imports the production Web adapter.
use super::*;
use server_api::*;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

struct LoopbackServer {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl LoopbackServer {
    async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { origin, task }
    }
}
impl Drop for LoopbackServer {
    fn drop(&mut self) {
        // No detached listener survives success, panic, or child timeout.
        self.task.abort();
    }
}

struct RealDriveFixture {
    // TempDir outlives the API and listener; no user's DB/data directory is used.
    server: LoopbackServer,
    api: WorkspaceApi,
    owner: String,
    second: String,
    _temp: tempfile::TempDir,
}
impl RealDriveFixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config = test_server_config(temp.path());
        config.repositories.clear();
        let store = Arc::new(SqliteWorkspaceStore::open(&config.database_path).unwrap());
        let owner = seed_test_api_token(store.as_ref(), "drive-web-owner");
        let second = seed_test_api_token(store.as_ref(), "drive-web-second");
        let server_api = WorkspaceServerApi::new(config, store.clone());
        let server = LoopbackServer::start(workspace_server_router(server_api.clone())).await;
        // Exercise T-717's real repositoryless Workspace bootstrap, not a
        // pre-existing test Workspace with its repository merely hidden in UI.
        let response = reqwest::Client::new()
            .post(format!("{}/api/workspaces", server.origin))
            .bearer_auth(&owner)
            .json(&json!({"operation_key":"drive-web-bootstrap","display_name":"Drive only"}))
            .send()
            .await
            .unwrap();
        let created = http_json(response, StatusCode::CREATED).await;
        assert!(created["repository"].is_null(), "{created}");
        let workspace_id = created["workspace"]["workspace_id"].as_str().unwrap();
        assert!(store.list_repositories(workspace_id).unwrap().is_empty());
        let api = server_api
            .api_for_workspace(workspace_id)
            .await
            .unwrap()
            .unwrap();
        assert!(api.config.repositories.is_empty());
        // Cache the same production scoped router without starting unrelated
        // orchestrator subscription tasks: this fixture tests Drive HTTP, not
        // Runtime/Tools/WIP scheduling. The server's ingress/auth still runs.
        server_api
            .routers
            .lock()
            .await
            .insert(workspace_id.to_owned(), build_inner_router(api.clone()));
        Self {
            server,
            api,
            owner,
            second,
            _temp: temp,
        }
    }

    fn base(&self) -> String {
        format!("/api/w/{}/drive", self.api.workspace_id())
    }

    async fn user(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        expected: StatusCode,
    ) -> Value {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", self.server.origin))
            .bearer_auth(&self.owner);
        if let Some(body) = body {
            request = request.json(&body);
        }
        http_json(request.send().await.unwrap(), expected).await
    }

    async fn seed_workers(&self) -> (RuntimeIdentityMaterial, Vec<RuntimeWorkerRef>) {
        let runtime_id = WorkdirlessFixtureRuntime::RUNTIME_ID;
        let identity = RuntimeIdentityMaterial::generate(runtime_id).unwrap();
        let binding = WorkspaceRuntimeBinding {
            workspace_id: self.api.workspace_id().into(),
            runtime_id: runtime_id.into(),
            display_name: "Drive fixture runtime".into(),
            base_url: "https://runtime.example.invalid".into(),
            public_key: identity.public_key.clone(),
            public_key_fingerprint: String::new(),
            binding_revision: 1,
            state: StoredRuntimeBindingState::Verified,
            authentication_mode: StoredRuntimeAuthenticationMode::LegacyServerIssuer,
            workspace_key_id: None,
            workspace_key_generation: None,
            created_at: "1".into(),
            updated_at: "1".into(),
            revoked_at: None,
        };
        self.api
            .store
            .upsert_workspace_runtime_binding_record(binding, false)
            .await
            .unwrap();
        // Keep the production runtime trust gate: its expected snapshot must
        // match the just-seeded normalized durable binding, not bypass it.
        let current_binding = self
            .api
            .store
            .get_workspace_runtime_binding(self.api.workspace_id(), runtime_id)
            .await
            .unwrap()
            .unwrap();
        self.api
            .runtime_binding_expectations
            .write()
            .unwrap()
            .insert(
                (self.api.workspace_id().to_owned(), runtime_id.to_owned()),
                current_binding,
            );
        // Only the runtime process boundary is a deterministic fake. Workspace,
        // workers, grants, receipts, Drive nodes and content are real authorities.
        let runtime = WorkdirlessFixtureRuntime::default();
        self.api.runtime.register_or_replace(runtime.clone());
        let mut workers = vec![];
        for (name, access) in [
            ("Drive reader", "read_only"),
            ("Drive writer", "read_write"),
        ] {
            let Json(created) = create_workspace_worker(
                State(self.api.clone()),
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
                }),
            )
            .await
            .unwrap();
            let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
            // The reusable fixture defaults to TEST_WORKSPACE_ID. Its runtime
            // observation must identify this genuinely bootstrapped Workspace.
            {
                let mut observed = runtime.workers.lock().unwrap();
                let summary = observed
                    .iter_mut()
                    .find(|summary| summary.worker == worker)
                    .unwrap();
                summary.workspace.identity = self.api.workspace_id().to_owned();
                summary.workspace.workspace_id = Some(self.api.workspace_id().to_owned());
            }
            assert!(
                self.api
                    .store
                    .list_worker_workdir_links(self.api.workspace_id(), &worker)
                    .unwrap()
                    .is_empty()
            );
            self.user(reqwest::Method::POST, &format!("{}/grants", self.base()),
                Some(json!({"runtime_id":worker.runtime_id,"worker_id":worker.worker_id,"access":access})), StatusCode::OK).await;
            workers.push(worker);
        }
        (identity, workers)
    }

    async fn worker(
        &self,
        identity: &RuntimeIdentityMaterial,
        worker: &RuntimeWorkerRef,
        method: &str,
        path: &str,
        body: Option<Value>,
        expected: StatusCode,
    ) -> Value {
        let bytes = body
            .map(|v| serde_json::to_vec(&v).unwrap())
            .unwrap_or_default();
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(identity)
            .issue(
                self.api.config.backend_base_url.as_deref().unwrap(),
                self.api.workspace_id(),
                Some(&worker.worker_id),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                method,
                path,
                &bytes,
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
                30,
            )
            .unwrap();
        let response = reqwest::Client::new()
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{}{path}", self.server.origin),
            )
            .bearer_auth(&self.owner)
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .header(CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .unwrap();
        http_json(response, expected).await
    }

    async fn worker_artifact(&self) -> Value {
        let (identity, workers) = self.seed_workers().await;
        let root = self
            .user(
                reqwest::Method::GET,
                &format!("{}/root", self.base()),
                None,
                StatusCode::OK,
            )
            .await;
        let mutation = json!({"request_id":"worker-artifact","mutation":{"operation":"create_text","parent":root["entry"],"name":"worker.md","text":"# Worker artifact\n","content_type":"text/markdown"}});
        let denied = self
            .worker(
                &identity,
                &workers[0],
                "POST",
                &format!("{}/mutate", self.base()),
                Some(mutation.clone()),
                StatusCode::FORBIDDEN,
            )
            .await;
        assert_eq!(denied["code"], "denied");
        let saved = self
            .worker(
                &identity,
                &workers[1],
                "POST",
                &format!("{}/mutate", self.base()),
                Some(mutation),
                StatusCode::OK,
            )
            .await;
        let read_path = format!(
            "{}/read-text?entry_workspace_id={}&id={}",
            self.base(),
            self.api.workspace_id(),
            saved["entry"]["entry"]["node_id"].as_str().unwrap()
        );
        let read = self
            .worker(
                &identity,
                &workers[0],
                "GET",
                &read_path,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(read["text"], "# Worker artifact\n");
        // Revocation is observed by the next signed HTTP request, including one
        // carrying the owner's bearer token (runtime identity has precedence).
        let grants = self
            .user(
                reqwest::Method::GET,
                &format!("{}/grants", self.base()),
                None,
                StatusCode::OK,
            )
            .await;
        let grant = grants["grants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|grant| grant["worker_id"] == workers[0].worker_id)
            .unwrap();
        self.user(
            reqwest::Method::DELETE,
            &format!(
                "{}/grants/{}",
                self.base(),
                grant["grant_id"].as_str().unwrap()
            ),
            None,
            StatusCode::OK,
        )
        .await;
        let revoked = self
            .worker(
                &identity,
                &workers[0],
                "GET",
                &read_path,
                None,
                StatusCode::FORBIDDEN,
            )
            .await;
        assert_eq!(revoked["code"], "denied");
        let web = self
            .user(reqwest::Method::GET, &read_path, None, StatusCode::OK)
            .await;
        assert_eq!(web["text"], "# Worker artifact\n");
        saved["entry"].clone()
    }
}

async fn http_json(response: reqwest::Response, expected: StatusCode) -> Value {
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    let value: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    assert_eq!(status.as_u16(), expected.as_u16(), "{value}");
    value
}

#[tokio::test]
async fn repositoryless_drive_shares_worker_bytes_and_enforces_current_read_only_grants_over_http()
{
    let fixture = RealDriveFixture::new().await;
    let artifact = fixture.worker_artifact().await;
    let root = fixture
        .user(
            reqwest::Method::GET,
            &format!("{}/root", fixture.base()),
            None,
            StatusCode::OK,
        )
        .await;
    let page = fixture
        .user(
            reqwest::Method::GET,
            &format!(
                "{}/list?entry_workspace_id={}&id={}",
                fixture.base(),
                fixture.api.workspace_id(),
                root["entry"]["node_id"].as_str().unwrap()
            ),
            None,
            StatusCode::OK,
        )
        .await;
    assert_eq!(page["entries"], json!([artifact]));
    // Real LocalFileSystem persisted the Worker bytes; no mock object store.
    let blob_root = fixture.api.config.drive_blob_root().unwrap();
    let blobs: Vec<_> = fs::read_dir(blob_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(blobs.len(), 1);
    assert_eq!(fs::read(&blobs[0]).unwrap(), b"# Worker artifact\n");
}

#[tokio::test]
#[ignore = "explicit API/Web process roundtrip; requires Deno on PATH (see web/workspace/test/drive-real-api/README.md)"]
async fn repositoryless_drive_real_web_adapter_preserves_bytes_cas_and_latest_reference_lifecycle()
{
    let fixture = RealDriveFixture::new().await;
    let worker_entry = fixture.worker_artifact().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let web = root.join("web/workspace");
    let mut child = tokio::process::Command::new("deno")
        .current_dir(&web)
        .args([
            "run",
            "--cached-only",
            "--config",
            "test/drive-real-api/deno.json",
        ])
        .arg(format!(
            "--allow-net={}",
            fixture.server.origin.trim_start_matches("http://")
        ))
        .arg("test/drive-real-api/roundtrip.ts")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Deno must be installed for this explicit process test");
    let wire = serde_json::to_vec(&json!({
        "origin":fixture.server.origin,"workspace_id":fixture.api.workspace_id(),
        "owner_token":fixture.owner,"second_token":fixture.second,"worker_entry":worker_entry,
    }))
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&wire).await.unwrap();
    drop(stdin);
    // Timeout is only a hang bound; listener readiness is the successful bind
    // and child synchronization is completion, never sleeps or polling loops.
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("Deno real API roundtrip exceeded 60 seconds")
        .unwrap();
    // Defensive redaction: test failure diagnostics must never print credentials.
    let redact = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .replace(&fixture.owner, "[redacted]")
            .replace(&fixture.second, "[redacted]")
    };
    assert!(
        output.status.success(),
        "Deno real API roundtrip failed:\n{}\n{}",
        redact(&output.stdout),
        redact(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("real Web adapter roundtrip passed"));
    assert!(
        fixture
            .api
            .store
            .list_repositories(fixture.api.workspace_id())
            .unwrap()
            .is_empty()
    );
}
