// Server library integration tests (`plugin_delivery_tests` filter).
// Real generated HTTP routes, Runtime signatures/trust/JTI verification, Subject
// policy storage, catalog files, config commits and persisted plans are connected.
// Worker execution uses WorkdirlessFixtureRuntime; the interrupted-pull case
// injects a body-stream failure only after the real owner succeeds. No model or
// HTTPS destination is contacted. Each fixture owns its loopback listener and DB.
use super::*;
use crate::plugin_plan::{HostExecutionIdentity, PlanPinState, PluginPlanCreateReceipt};
use server_api::{
    PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest, PluginInstallRequest,
    PluginOperationResponse, PluginRevokeRequest,
    authorization::{PermissionResource, ScopedPermission, SubjectPermissionsUpdate},
};
use std::sync::Mutex;
use tokio::io::AsyncReadExt;
use worker_runtime::{
    workspace_issuer::{WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState},
    workspace_request::{
        RuntimeWorkspaceRequestClient, plugin_delivery::RemotePluginArtifactSource,
    },
};
use yoi_plugin_runtime::delivery::{
    ArtifactCache, ArtifactExpectation, ArtifactSource, CacheLimits, DeliveryError,
};

struct LoopbackServer(Option<tokio::task::JoinHandle<()>>);
impl LoopbackServer {
    async fn stop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
    }
}
impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

struct DeliveryFixture {
    api: WorkspaceApi,
    identity: RuntimeIdentityMaterial,
    worker: RuntimeWorkerRef,
    subject: Option<String>,
    installed: PluginOperationResponse,
    saved: PluginPlanCreateReceipt,
    request: PluginArtifactDeliveryRequest,
    bytes: Vec<u8>,
    registration: Arc<Mutex<WorkspaceIssuerTrustRecord>>,
    remote: RemotePluginArtifactSource,
    interrupt_pull: Arc<std::sync::atomic::AtomicBool>,
    server: LoopbackServer,
    url: String,
    dir: tempfile::TempDir,
}

fn artifact_package() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("plugin.toml"),
        include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml"),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.component.wasm"),
        include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    let limits = manifest::plugin::PluginPackageLimits::default();
    let package = manifest::plugin::read_plugin_directory(
        dir.path(),
        manifest::plugin::PluginSourceKind::Explicit,
        &limits,
    )
    .unwrap();
    manifest::plugin::pack_plugin_package(&package, &limits)
        .unwrap()
        .bytes
}

async fn delivery_worker(api: &WorkspaceApi, subject: Option<&str>) -> RuntimeWorkerRef {
    let Json(worker) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
            display_name: "Artifact consumer".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: subject.map(|subject| server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.into(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    RuntimeWorkerRef::new(worker.runtime_id, worker.worker_id)
}

fn configure_delivery_plan(
    api: &WorkspaceApi,
    installed: &PluginOperationResponse,
    execution: HostExecutionIdentity,
    key: &str,
) -> PluginPlanCreateReceipt {
    use config_source::{ConfigTreeChange, VirtualPath};
    let bundle = api.config_schema_registry.compose().unwrap();
    let state = api
        .config_store
        .ensure_workspace_config_materialized_with_schema(TEST_WORKSPACE_ID, "1", bundle.clone())
        .unwrap();
    let artifact = &installed.installation.artifacts[0];
    let value = json!({
        "plugins":{"enabled":{"delivery":{
            "installation_id": installed.installation.installation_id,
            "expected_last_operation": installed.operation_id,
            "package_digest": artifact.package_digest,
            "artifact_digest": artifact.artifact_digest,
            "tools":["example_echo"], "config":{}
        }}},
        "profile":{"entries":[{"selector":"project:delivery","profile":{
            "plugins":{"selections":[{"installation_id":installed.installation.installation_id,"tools":["example_echo"]}]}
        }}]}
    });
    let path = VirtualPath::parse("main.dcdl").unwrap();
    let candidate = api
        .config_store
        .evaluate_workspace_config_candidate_with_schema(
            TEST_WORKSPACE_ID,
            &ConfigCommitRequest {
                base_digest: state.snapshot.digest.clone(),
                entrypoints: vec![path.clone()],
                changes: vec![ConfigTreeChange::Update {
                    path: path.clone(),
                    expected_digest: state.snapshot.get(&path).unwrap().content_digest.clone(),
                    content: format!(
                        "{} as WorkspaceConfigSchema",
                        crate::plugin_settings::render_value(&value).unwrap()
                    ),
                }],
            },
            bundle,
        )
        .unwrap();
    let committed = api
        .config_store
        .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
        .unwrap();
    let plan = api
        .config_store
        .with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            crate::plugin_settings::resolve_worker_plugin_plan(
                &tx,
                TEST_WORKSPACE_ID,
                &committed.snapshot.digest,
                "project:delivery",
                execution,
            )
        })
        .unwrap();
    api.config_store
        .plugin_plan_create("delivery-host", key, &plan)
        .unwrap()
}

impl DeliveryFixture {
    async fn new(subject_bound: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        let subject = subject_bound.then(|| {
            open_subjektiv_store(&api)
                .unwrap()
                .create_subject(crate::subjektiv::SubjectRole::new("artifact-consumer").unwrap())
                .unwrap()
                .id
        });
        let worker = delivery_worker(&api, subject.as_deref()).await;
        let identity = RuntimeIdentityMaterial::generate(&worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut api, &identity, &worker.runtime_id);
        seed_worker_source_member(&api, &worker.runtime_id, &worker.worker_id);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        api.config.backend_base_url = Some(url.clone());
        let signing = api
            .signing_identities
            .provision_existing(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        let bundle = project_workspace_signing_identity(&api, signing)
            .unwrap()
            .public_bundle
            .unwrap();
        let (_, registered) = worker_runtime::workspace_issuer::add_workspace_issuer_trust(
            &mut Vec::new(),
            bundle,
            worker_runtime::auth::unix_now_seconds() as i64,
        )
        .unwrap();
        let registration = Arc::new(Mutex::new(registered));
        let bytes = artifact_package();
        let root = ServerConfig::server_data_root_for_data_dir(&api.config.data_root)
            .join("plugin-artifacts");
        let uploaded = api
            .config_store
            .plugin_upload(&root, TEST_WORKSPACE_ID, "delivery-host", &bytes)
            .unwrap();
        let installed = api
            .config_store
            .plugin_install(
                TEST_WORKSPACE_ID,
                "delivery-host",
                &PluginInstallRequest {
                    upload_id: uploaded.upload_id,
                    alias: "delivery".into(),
                    operation_key: "install-delivery".into(),
                },
            )
            .unwrap();
        let execution = api.plugin_execution_identity(&worker).await.unwrap();
        let saved = configure_delivery_plan(&api, &installed, execution, "delivery-launch");
        let pin = &saved.plan.plugins()[0];
        let request = PluginArtifactDeliveryRequest {
            plan_id: saved.plan_id.clone(),
            plan_fingerprint: saved.fingerprint.clone(),
            installation_id: pin.installation_id().into(),
            artifact_digest: pin.artifact_digest().into(),
        };
        let client =
            RuntimeWorkspaceRequestClient::new(TEST_WORKSPACE_ID, &url, &worker.runtime_id)
                .with_runtime_request_source(&identity, &url);
        let current = registration.clone();
        let remote = RemotePluginArtifactSource::new(
            client,
            worker.worker_id.clone(),
            Arc::new(move || Ok(current.lock().unwrap().clone())),
        )
        .unwrap();
        let interrupt_pull = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fault = interrupt_pull.clone();
        // Fault injection is outside the authenticated generated routes and owner:
        // successful pull must complete those checks before its HTTP stream fails.
        let app = build_router(api.clone()).layer(axum::middleware::from_fn(
            move |request: Request<Body>, next: axum::middleware::Next| {
                let fault = fault.clone();
                async move {
                    let pull = request.uri().path().ends_with("/plugin-artifacts/pull");
                    let response = next.run(request).await;
                    if !pull
                        || !response.status().is_success()
                        || !fault.load(std::sync::atomic::Ordering::SeqCst)
                    {
                        return response;
                    }
                    let (mut parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024).await.unwrap();
                    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
                    let prefix = bytes.slice(..bytes.len() / 2);
                    let stream =
                        futures::stream::once(async move { Ok::<_, std::io::Error>(prefix) })
                            .chain(futures::stream::once(async {
                                tokio::task::yield_now().await;
                                Err::<axum::body::Bytes, _>(std::io::Error::other(
                                    "injected delivery disconnect",
                                ))
                            }));
                    Response::from_parts(parts, Body::from_stream(stream))
                }
            },
        ));
        let server = LoopbackServer(Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        })));
        Self {
            api,
            identity,
            worker,
            subject,
            installed,
            saved,
            request,
            bytes,
            registration,
            remote,
            interrupt_pull,
            server,
            url,
            dir,
        }
    }
    fn expected(&self) -> ArtifactExpectation {
        ArtifactExpectation::new(self.installed.installation.artifacts[0].clone())
    }
    fn cache(&self, name: &str) -> ArtifactCache {
        ArtifactCache::new(self.dir.path().join(name), CacheLimits::default()).unwrap()
    }
    fn path(&self, suffix: &str) -> String {
        format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/plugin-artifacts/{suffix}")
    }
    fn proof(
        &self,
        workspace: &str,
        worker: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> String {
        worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&self.identity)
            .issue(
                &self.url,
                workspace,
                Some(worker),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                method,
                path,
                body,
                worker_runtime::auth::unix_now_seconds() as i64,
                30,
            )
            .unwrap()
    }
    async fn post(&self, path: &str, body: Vec<u8>, proof: Option<&str>) -> reqwest::Response {
        self.post_as(&self.worker.worker_id, path, body, proof)
            .await
    }
    async fn post_as(
        &self,
        worker_id: &str,
        path: &str,
        body: Vec<u8>,
        proof: Option<&str>,
    ) -> reqwest::Response {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let mut request = client
            .post(format!("{}{path}", self.url))
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header("x-yoi-runtime-id", &self.worker.runtime_id)
            .header("x-yoi-worker-id", worker_id)
            .body(body);
        if let Some(proof) = proof {
            request = request.header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            );
        }
        request.send().await.unwrap()
    }
    async fn signed_post(&self, suffix: &str, value: &impl serde::Serialize) -> reqwest::Response {
        let path = self.path(suffix);
        let body = serde_json::to_vec(value).unwrap();
        let proof = self.proof(
            TEST_WORKSPACE_ID,
            &self.worker.worker_id,
            "POST",
            &path,
            &body,
        );
        self.post(&path, body, Some(&proof)).await
    }
    fn artifact_permission(&self) -> ScopedPermission {
        ScopedPermission {
            operation: "plugin.artifact.read".into(),
            resource: PermissionResource::PluginInstallation {
                workspace_id: TEST_WORKSPACE_ID.into(),
                installation_id: self.request.installation_id.clone(),
            },
        }
    }
    fn policy(&self, execution: Vec<ScopedPermission>, delegation: Vec<ScopedPermission>) {
        let subject = self.subject.as_ref().unwrap();
        let store = open_subjektiv_store(&self.api).unwrap();
        let previous = store.subject_permissions(subject).unwrap();
        store
            .set_subject_permissions(
                subject,
                &SubjectPermissionsUpdate {
                    operation_id: uuid::Uuid::now_v7().to_string(),
                    expected_change_id: previous.map(|p| p.change_id),
                    execution,
                    delegation,
                },
            )
            .unwrap();
    }
    fn assert_pending_pin(&self) {
        let stored = self
            .api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &self.saved.plan_id)
            .unwrap();
        assert_eq!(stored.plan, self.saved.plan);
        assert_eq!(stored.pin_state, PlanPinState::Pending);
        let references: Vec<String> = self
            .api
            .config_store
            .with_conn(|conn| {
                Ok(conn
                    .prepare(
                        "SELECT kind FROM plugin_references WHERE workspace_id=?1 AND owner=?2",
                    )?
                    .query_map(
                        rusqlite::params![TEST_WORKSPACE_ID, self.saved.plan_id],
                        |row| row.get(0),
                    )?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(references, ["pending"]);
    }
}

async fn source_bytes(
    source: &dyn ArtifactSource,
    request: &PluginArtifactDeliveryRequest,
) -> Vec<u8> {
    let mut bytes = vec![];
    source
        .fetch(request)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    bytes
}

#[tokio::test]
async fn same_persisted_plan_remote_and_inprocess_return_exact_archive_and_digests() {
    use sha2::{Digest, Sha256};
    let f = DeliveryFixture::new(false).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    assert_eq!(
        f.remote.server_authority_identity(),
        local.server_authority_identity()
    );
    let remote_metadata = f.remote.authorize(&f.request).await.unwrap();
    assert_eq!(remote_metadata, local.authorize(&f.request).await.unwrap());
    assert_eq!(remote_metadata.workspace_id, TEST_WORKSPACE_ID);
    assert_eq!(remote_metadata.request, f.request);
    assert_eq!(
        remote_metadata.artifact,
        f.installed.installation.artifacts[0]
    );
    let remote = source_bytes(&f.remote, &f.request).await;
    assert_eq!(remote, source_bytes(&local, &f.request).await);
    assert_eq!(remote, f.bytes);
    assert_eq!(
        format!("sha256:{}", Sha256::digest(&remote).iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
        f.request.artifact_digest
    );
    let unpacked = manifest::plugin::read_plugin_package_bytes(
        &remote,
        manifest::plugin::PluginSourceKind::Explicit,
        &manifest::plugin::PluginPackageLimits::default(),
    )
    .unwrap();
    assert_eq!(
        unpacked.package.package_digest,
        remote_metadata.artifact.package_digest
    );
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert_eq!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await
                .unwrap()
                .bytes(),
            f.bytes
        );
    }
    f.assert_pending_pin();
}

#[tokio::test]
async fn signed_foreign_plan_pin_and_digest_only_requests_never_deliver_bytes() {
    let f = DeliveryFixture::new(false).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let root = ServerConfig::server_data_root_for_data_dir(&f.api.config.data_root)
        .join("plugin-artifacts");
    let uploaded = f
        .api
        .config_store
        .plugin_upload(&root, TEST_WORKSPACE_ID, "delivery-host", &f.bytes)
        .unwrap();
    let outside = f
        .api
        .config_store
        .plugin_install(
            TEST_WORKSPACE_ID,
            "delivery-host",
            &PluginInstallRequest {
                upload_id: uploaded.upload_id,
                alias: "outside-plan".into(),
                operation_key: "install-outside-plan".into(),
            },
        )
        .unwrap();
    let other_plan = configure_delivery_plan(
        &f.api,
        &outside,
        f.api.plugin_execution_identity(&f.worker).await.unwrap(),
        "outside-plan-launch",
    );
    let mut cases = vec![];
    let mut wrong = f.request.clone();
    wrong.plan_id = other_plan.plan_id;
    cases.push((wrong, 409));
    let mut wrong = f.request.clone();
    wrong.plan_id = "missing-plan".into();
    cases.push((wrong, 404));
    let mut wrong = f.request.clone();
    wrong.plan_fingerprint = format!("sha256:{}", "0".repeat(64));
    cases.push((wrong, 409));
    let mut wrong = f.request.clone();
    wrong.installation_id = outside.installation.installation_id;
    cases.push((wrong, 404));
    let mut wrong = f.request.clone();
    wrong.artifact_digest = format!("sha256:{}", "0".repeat(64));
    cases.push((wrong, 404));
    for (request, status) in cases {
        for suffix in ["authorize", "pull"] {
            let response = f.signed_post(suffix, &request).await;
            assert_eq!(response.status().as_u16(), status, "{suffix}: {request:?}");
            let error: server_api::RepositoryApiError = response.json().await.unwrap();
            assert!(error.error.starts_with("plugin_delivery_"));
        }
        assert!(matches!(
            f.remote.authorize(&request).await,
            Err(DeliveryError::Denied)
        ));
        assert!(local.fetch(&request).await.is_err());
    }
    for suffix in ["authorize", "pull"] {
        let response = f
            .signed_post(
                suffix,
                &json!({"artifact_digest":f.request.artifact_digest}),
            )
            .await;
        assert_eq!(response.status().as_u16(), 400);
    }
    // A valid, registered second Worker is authenticated but cannot borrow the plan.
    // Its builtin Profile selects no Plugins; it still cannot borrow this plan.
    let other = delivery_worker(&f.api, None).await;
    let path = f.path("pull");
    let body = serde_json::to_vec(&f.request).unwrap();
    let proof = f.proof(TEST_WORKSPACE_ID, &other.worker_id, "POST", &path, &body);
    let response = f
        .post_as(&other.worker_id, &path, body.clone(), Some(&proof))
        .await;
    assert_eq!(response.status().as_u16(), 409);
    let path = f
        .path("pull")
        .replace(TEST_WORKSPACE_ID, "foreign-workspace");
    let proof = f.proof(
        "foreign-workspace",
        &f.worker.worker_id,
        "POST",
        &path,
        &body,
    );
    assert_eq!(
        f.post(&path, body, Some(&proof)).await.status().as_u16(),
        401
    );
    f.assert_pending_pin();
}

#[tokio::test]
async fn delivery_requires_signature_bound_to_actual_body_method_path_and_single_use_jti() {
    let f = DeliveryFixture::new(false).await;
    let body = serde_json::to_vec(&f.request).unwrap();
    for suffix in ["authorize", "pull"] {
        let path = f.path(suffix);
        assert_eq!(
            f.post(&path, body.clone(), None).await.status().as_u16(),
            401
        );
        let proof = f.proof(TEST_WORKSPACE_ID, &f.worker.worker_id, "POST", &path, &body);
        let mut tampered = proof.clone().into_bytes();
        let index = tampered.len() - 8;
        tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            f.post(
                &path,
                body.clone(),
                Some(std::str::from_utf8(&tampered).unwrap())
            )
            .await
            .status()
            .as_u16(),
            401
        );
        let mut changed = body.clone();
        changed.push(b' ');
        assert_eq!(
            f.post(&path, changed, Some(&proof)).await.status().as_u16(),
            401
        );
        for (method, signed_path) in [("GET", path.clone()), ("POST", format!("{path}?changed=1"))]
        {
            let proof = f.proof(
                TEST_WORKSPACE_ID,
                &f.worker.worker_id,
                method,
                &signed_path,
                &body,
            );
            assert_eq!(
                f.post(&path, body.clone(), Some(&proof))
                    .await
                    .status()
                    .as_u16(),
                401
            );
        }
        let proof = f.proof(TEST_WORKSPACE_ID, &f.worker.worker_id, "POST", &path, &body);
        let response = f.post(&path, body.clone(), Some(&proof)).await;
        assert_eq!(response.status().as_u16(), 200);
        if suffix == "pull" {
            assert_eq!(response.bytes().await.unwrap().as_ref(), f.bytes);
        } else {
            assert_eq!(
                response
                    .json::<PluginArtifactDeliveryMetadata>()
                    .await
                    .unwrap()
                    .request,
                f.request
            );
        }
        let response = f.post(&path, body.clone(), Some(&proof)).await;
        assert_eq!(response.status().as_u16(), 401);
        let error: server_api::RepositoryApiError = response.json().await.unwrap();
        assert_eq!(error.error, "Unauthorized");
    }
    f.assert_pending_pin();
}

#[tokio::test]
async fn populated_cache_cannot_bypass_artifact_revocation_or_release_pins() {
    let f = DeliveryFixture::new(false).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    f.api
        .config_store
        .plugin_revoke(
            TEST_WORKSPACE_ID,
            "delivery-host",
            &f.request.installation_id,
            &PluginRevokeRequest {
                expected_last_operation: f.installed.operation_id.clone(),
                operation_key: "revoke-artifact".into(),
                artifact_digest: f.request.artifact_digest.clone(),
                reason: "withdraw executable".into(),
            },
        )
        .unwrap();
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert!(matches!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await,
            Err(DeliveryError::Denied)
        ));
        assert!(source.fetch(&f.request).await.is_err());
    }
    f.assert_pending_pin();
}

#[tokio::test]
async fn unavailable_owner_denies_previously_verified_remote_cache_hit() {
    let mut f = DeliveryFixture::new(false).await;
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    f.server.stop().await;
    assert!(matches!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await,
        Err(DeliveryError::Unavailable(_))
    ));
    // The same owner remains available through the non-HTTP adapter.
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    assert_eq!(
        cache
            .resolve(&local, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    f.assert_pending_pin();
}

#[tokio::test]
async fn released_persisted_plan_denies_remote_and_inprocess_warm_cache() {
    let f = DeliveryFixture::new(false).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    cache
        .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
        .await
        .unwrap();
    f.api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &f.saved.plan_id,
            PlanPinState::Pending,
            PlanPinState::Released,
        )
        .unwrap();
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert!(matches!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await,
            Err(DeliveryError::Denied)
        ));
    }
    assert_eq!(
        f.api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &f.saved.plan_id)
            .unwrap()
            .pin_state,
        PlanPinState::Released
    );
}

fn published_cache_files(root: &std::path::Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "artifact"))
        .collect()
}

#[tokio::test]
async fn real_remote_cache_converges_repairs_corruption_and_refetches_after_eviction() {
    let f = DeliveryFixture::new(false).await;
    let root = f.dir.path().join("runtime-cache");
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    let (first, second) = tokio::join!(
        cache.resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected),
        cache.resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected),
    );
    assert_eq!(first.unwrap().bytes(), f.bytes);
    assert_eq!(second.unwrap().bytes(), f.bytes);
    let files = published_cache_files(&root);
    assert_eq!(files.len(), 1);
    fs::write(&files[0], vec![0; f.bytes.len()]).unwrap();
    drop(cache);
    // Reopening the Runtime-owned cache must not trust a corrupt published file.
    let cache = f.cache("runtime-cache");
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    assert_eq!(fs::read(&files[0]).unwrap(), f.bytes);
    fs::remove_file(&files[0]).unwrap();
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    assert_eq!(published_cache_files(&root), files);
    f.assert_pending_pin();
}

#[tokio::test]
async fn interrupted_real_http_pull_does_not_publish_partial_cache_and_retry_recovers() {
    let f = DeliveryFixture::new(false).await;
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    f.interrupt_pull
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .is_err()
    );
    assert!(published_cache_files(&f.dir.path().join("runtime-cache")).is_empty());
    // Reopen after failure: neither abandoned staging bytes nor a previous owner
    // observation can be used as a verified executable on the next attempt.
    drop(cache);
    let cache = f.cache("runtime-cache");
    f.interrupt_pull
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    assert_eq!(
        published_cache_files(&f.dir.path().join("runtime-cache")).len(),
        1
    );
    f.assert_pending_pin();
}

#[tokio::test]
async fn missing_truncated_oversize_or_mismatched_owner_blob_never_publishes_remote_cache_entry() {
    let f = DeliveryFixture::new(false).await;
    let root = ServerConfig::server_data_root_for_data_dir(&f.api.config.data_root)
        .join("plugin-artifacts");
    let blob = root
        .join("blobs")
        .join(f.request.artifact_digest.strip_prefix("sha256:").unwrap());
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    fs::remove_file(&blob).unwrap();
    assert!(matches!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await,
        Err(DeliveryError::Unavailable(_))
    ));
    for bad in [vec![], vec![0; f.bytes.len() + 1], vec![0; f.bytes.len()]] {
        fs::write(&blob, bad).unwrap();
        assert!(matches!(
            cache
                .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
                .await,
            Err(DeliveryError::Unavailable(_))
        ));
        assert!(local.fetch(&f.request).await.is_err());
        assert!(published_cache_files(&f.dir.path().join("runtime-cache")).is_empty());
    }
    fs::write(&blob, &f.bytes).unwrap();
    assert_eq!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await
            .unwrap()
            .bytes(),
        f.bytes
    );
    f.assert_pending_pin();
}

#[tokio::test]
async fn foreign_server_registration_cannot_lend_its_metadata_to_existing_source() {
    let f = DeliveryFixture::new(false).await;
    let foreign = DeliveryFixture::new(false).await;
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    cache
        .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
        .await
        .unwrap();
    assert_ne!(
        f.remote.server_authority_identity(),
        foreign.remote.server_authority_identity()
    );
    // Even a valid registration for another live Server cannot retarget a source
    // or carry verified metadata across the original authority's cache key.
    *f.registration.lock().unwrap() = foreign.registration.lock().unwrap().clone();
    assert!(matches!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await,
        Err(DeliveryError::Denied)
    ));
    assert!(matches!(
        foreign.remote.authorize(&f.request).await,
        Err(DeliveryError::Denied)
    ));
    f.assert_pending_pin();
}

#[tokio::test]
async fn revoked_server_runtime_binding_denies_both_adapters_even_on_cache_hit() {
    let f = DeliveryFixture::new(false).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    cache
        .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
        .await
        .unwrap();
    let mut binding = f
        .api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &f.worker.runtime_id)
        .await
        .unwrap()
        .unwrap();
    binding.revoked_at = Some(now_registry_timestamp());
    f.api
        .config_store
        .upsert_workspace_runtime_binding(binding, true)
        .unwrap();
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert!(matches!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await,
            Err(DeliveryError::Denied)
        ));
    }
    f.assert_pending_pin();
}

#[tokio::test]
async fn subject_delegation_replacement_denies_child_warm_cache_and_preserves_saved_plan() {
    let f = DeliveryFixture::new(true).await;
    let permission = f.artifact_permission();
    f.policy(vec![], vec![permission.clone()]);
    // Use the signed, replay-safe Worker control route to issue the child grant.
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers");
    let body = serde_json::to_vec(&json!({
        "runtime_id": f.worker.runtime_id, "display_name":"Artifact delegated consumer",
        "profile":"builtin:coder", "initial_submit":[], "workdir_attachments":[], "feature_connections":{},
        "control_operation_id":"delivery-delegated-child",
        "execution_grant":{"permissions":[permission], "redelegation":[]}
    })).unwrap();
    let proof = f.proof(TEST_WORKSPACE_ID, &f.worker.worker_id, "POST", &path, &body);
    let response = f.post(&path, body, Some(&proof)).await;
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status.as_u16(), 200, "{body}");
    let child = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let execution = f.api.plugin_execution_identity(&child).await.unwrap();
    assert_eq!(
        execution.principal_id,
        format!("subject:{}", f.subject.as_ref().unwrap())
    );
    assert!(execution.delegation_id.is_some());
    let saved = configure_delivery_plan(&f.api, &f.installed, execution, "delegated-launch");
    let request = PluginArtifactDeliveryRequest {
        plan_id: saved.plan_id.clone(),
        plan_fingerprint: saved.fingerprint.clone(),
        ..f.request.clone()
    };
    let client = RuntimeWorkspaceRequestClient::new(TEST_WORKSPACE_ID, &f.url, &child.runtime_id)
        .with_runtime_request_source(&f.identity, &f.url);
    let current = f.registration.clone();
    let remote = RemotePluginArtifactSource::new(
        client,
        child.worker_id.clone(),
        Arc::new(move || Ok(current.lock().unwrap().clone())),
    )
    .unwrap();
    let local = f.api.plugin_artifact_source(child.clone()).await.unwrap();
    let cache = f.cache("child-runtime-cache");
    let expected = f.expected();
    for source in [&remote as &dyn ArtifactSource, &local] {
        assert_eq!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &request, &expected)
                .await
                .unwrap()
                .bytes(),
            f.bytes
        );
    }
    // Replacement fences the saved source_change_id, even with the same resource.
    f.policy(vec![], vec![]);
    assert!(f.api.plugin_execution_identity(&child).await.is_err());
    for source in [&remote as &dyn ArtifactSource, &local] {
        assert!(matches!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &request, &expected)
                .await,
            Err(DeliveryError::Denied)
        ));
        assert!(source.fetch(&request).await.is_err());
    }
    let retained = f
        .api
        .config_store
        .plugin_plan_load(TEST_WORKSPACE_ID, &saved.plan_id)
        .unwrap();
    assert_eq!(retained.plan, saved.plan);
    assert_eq!(retained.pin_state, PlanPinState::Pending);
    f.assert_pending_pin();
}

#[tokio::test]
async fn runtime_registration_revocation_cannot_reuse_remote_verified_cache() {
    let f = DeliveryFixture::new(false).await;
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    cache
        .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
        .await
        .unwrap();
    {
        let mut registration = f.registration.lock().unwrap();
        worker_runtime::workspace_issuer::revoke_workspace_issuer_trust(
            std::slice::from_mut(&mut *registration),
            TEST_WORKSPACE_ID,
            worker_runtime::auth::unix_now_seconds() as i64,
        )
        .unwrap();
        assert_eq!(registration.state, WorkspaceIssuerTrustState::Revoked);
    }
    assert!(matches!(
        cache
            .resolve(&f.remote, TEST_WORKSPACE_ID, &f.request, &expected)
            .await,
        Err(DeliveryError::Denied)
    ));
    f.assert_pending_pin();
}

#[tokio::test]
async fn subject_exact_artifact_permission_is_live_and_does_not_grant_https() {
    let f = DeliveryFixture::new(true).await;
    let local = f
        .api
        .plugin_artifact_source(f.worker.clone())
        .await
        .unwrap();
    let cache = f.cache("runtime-cache");
    let expected = f.expected();
    assert!(matches!(
        f.remote.authorize(&f.request).await,
        Err(DeliveryError::Denied)
    ));
    let permission = f.artifact_permission();
    let mut wrong_operation = permission.clone();
    wrong_operation.operation = "plugin".into();
    let mut wrong_resource = permission.clone();
    wrong_resource.resource = PermissionResource::Workspace {
        workspace_id: TEST_WORKSPACE_ID.into(),
    };
    let mut wrong_installation = permission.clone();
    wrong_installation.resource = PermissionResource::PluginInstallation {
        workspace_id: TEST_WORKSPACE_ID.into(),
        installation_id: "other-installation".into(),
    };
    for wrong in [wrong_operation, wrong_resource, wrong_installation] {
        f.policy(vec![wrong], vec![]);
        assert!(matches!(
            f.remote.authorize(&f.request).await,
            Err(DeliveryError::Denied)
        ));
        assert!(local.fetch(&f.request).await.is_err());
    }
    f.policy(vec![permission], vec![]);
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert_eq!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await
                .unwrap()
                .bytes(),
            f.bytes
        );
    }
    // Only a policy decision is requested; this test never contacts a destination.
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/authorization/check");
    let body = serde_json::to_vec(&json!({"operation":"plugin.https.request","resource":{
        "kind":"plugin_installation","workspace_id":TEST_WORKSPACE_ID,"installation_id":f.request.installation_id
    }})).unwrap();
    let proof = f.proof(TEST_WORKSPACE_ID, &f.worker.worker_id, "POST", &path, &body);
    let response = f.post(&path, body, Some(&proof)).await;
    assert_eq!(response.status().as_u16(), 403);
    f.policy(vec![], vec![]);
    for source in [&f.remote as &dyn ArtifactSource, &local] {
        assert!(matches!(
            cache
                .resolve(source, TEST_WORKSPACE_ID, &f.request, &expected)
                .await,
            Err(DeliveryError::Denied)
        ));
        assert!(source.fetch(&f.request).await.is_err());
    }
    f.assert_pending_pin();
}
