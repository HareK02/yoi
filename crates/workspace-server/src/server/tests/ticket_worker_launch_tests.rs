// T-719 launch regressions. Parent wiring: mod ticket_worker_launch_tests;
// under server::tests. Runtime execution is in-process; no Worker binary is spawned.
use super::*;
use crate::hosts::{InternalRuntimeSummary, WorkspaceWorkerRuntime};
use worker_runtime::execution::{
    WorkerExecutionBackend, WorkerExecutionResult, WorkerExecutionSpawnRequest,
    WorkerExecutionSpawnResult, WorkspaceConfigFetchRequest, WorkspaceConfigFetchResult,
};

// The existing deterministic executor deliberately does not fetch Workspace
// Config. Supply only that narrow boundary here, using the Backend's real
// published Profile projection; leave Worker creation/control to the real Runtime.
#[derive(Default)]
struct LaunchExecutionBackend {
    snapshot_script: Mutex<std::collections::VecDeque<Option<protocol::WorkerStateSnapshot>>>,
    snapshot_script_worker: Mutex<Option<String>>,
    lose_initial_ack: std::sync::atomic::AtomicBool,
    fail_stop: std::sync::atomic::AtomicBool,
    execution: DeterministicExecutionBackend,
    provider_close_gate: Mutex<Option<Arc<super::workdir_session_close_tests::ProviderCloseGate>>>,
    bundles: Mutex<std::collections::HashMap<String, worker_runtime::config_bundle::ConfigBundle>>,
    creates: Mutex<Vec<worker_runtime::catalog::CreateWorkerRequest>>,
    submits: Mutex<
        Vec<(
            worker_runtime::identity::WorkerRef,
            worker_runtime::interaction::WorkerInput,
        )>,
    >,
}

impl LaunchExecutionBackend {
    fn creates_for(&self, worker_id: &str) -> Vec<worker_runtime::catalog::CreateWorkerRequest> {
        self.creates
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.worker_id.to_string() == worker_id)
            .cloned()
            .collect()
    }

    fn submits_for(&self, worker_id: &str) -> Vec<worker_runtime::interaction::WorkerInput> {
        self.submits
            .lock()
            .unwrap()
            .iter()
            .filter(|(worker, _)| worker.worker_id.to_string() == worker_id)
            .map(|(_, input)| input.clone())
            .collect()
    }

    fn publish_profile(&self, api: &WorkspaceApi, profile: &str) {
        let state = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let profiles = crate::profile_settings::project_profiles_from_workspace_config(
            TEST_WORKSPACE_ID,
            &state,
        )
        .unwrap();
        let prompts = api
            .prompt_projection_cache
            .resolve(TEST_WORKSPACE_ID, &state)
            .unwrap();
        let bundle =
            crate::profile_settings::build_virtual_profile_config_bundle_with_prompt_projection(
                &profiles,
                &state,
                TEST_WORKSPACE_ID,
                &api.config.workspace_created_at,
                profile,
                prompts.as_ref(),
            )
            .unwrap()
            .unwrap();
        self.bundles
            .lock()
            .unwrap()
            .insert(profile.to_string(), bundle);
    }
}

impl WorkerExecutionBackend for LaunchExecutionBackend {
    fn backend_id(&self) -> &str {
        "ticket-worker-launch-test"
    }

    fn fetch_workspace_config(
        &self,
        request: WorkspaceConfigFetchRequest,
    ) -> std::result::Result<WorkspaceConfigFetchResult, String> {
        assert_eq!(request.workspace_api.workspace_id, TEST_WORKSPACE_ID);
        let key = match request.profile {
            ProfileSelector::Builtin(key) | ProfileSelector::Named(key) => key,
        };
        let bundle = self
            .bundles
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("unpublished test Profile {key}"))?;
        assert_eq!(bundle.metadata.id, request.expected.id);
        assert_eq!(bundle.metadata.digest, request.expected.digest);
        Ok(WorkspaceConfigFetchResult::Modified(bundle))
    }

    fn observe_repository_ref(
        &self,
        request: &RepositoryRefObservationRequest,
    ) -> std::result::Result<
        RepositoryRefObservation,
        worker_runtime::working_directory::WorkingDirectoryDiagnostic,
    > {
        self.execution.materializer.observe_repository_ref(request)
    }

    fn working_directory(
        &self,
        id: &str,
    ) -> std::result::Result<
        worker_runtime::catalog::WorkingDirectoryStatus,
        worker_runtime::working_directory::WorkingDirectoryDiagnostic,
    > {
        self.execution.working_directory(id)
    }

    fn open_workdir_session(
        &self,
        id: &str,
    ) -> std::result::Result<
        WorkdirSessionHandle,
        worker_runtime::working_directory::WorkingDirectoryDiagnostic,
    > {
        let binding = self
            .execution
            .materializer
            .bind_working_directory(id, None)?;
        let scope = manifest::Scope::writable(binding.root()).unwrap();
        let session: WorkdirSessionHandle = Arc::new(
            workdir::LocalWorkdirSession::materialized_bound_with_environment(
                workdir::Workdir::new(id),
                binding.root().to_owned(),
                binding.root().to_owned(),
                manifest::SharedScope::new(scope),
                workdir::WorkdirSessionCapabilities::ALL,
                binding.command_environment(),
                binding.session_resources()?,
            ),
        );
        Ok(match self.provider_close_gate.lock().unwrap().as_ref() {
            Some(gate) => gate.wrap(session),
            None => session,
        })
    }

    fn create_working_directory(
        &self,
        request: &worker_runtime::catalog::WorkingDirectoryRequest,
    ) -> std::result::Result<
        worker_runtime::catalog::WorkingDirectoryStatus,
        worker_runtime::working_directory::WorkingDirectoryDiagnostic,
    > {
        self.execution.create_working_directory(request)
    }

    fn cleanup_working_directory(
        &self,
        id: &str,
    ) -> std::result::Result<
        worker_runtime::catalog::WorkingDirectoryStatus,
        worker_runtime::working_directory::WorkingDirectoryDiagnostic,
    > {
        self.execution.cleanup_working_directory(id)
    }

    fn spawn_worker(&self, mut request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        self.creates.lock().unwrap().push(request.request.clone());
        for claim in &request.request.workdir_attachments {
            let binding = self
                .execution
                .materializer
                .bind_working_directory(&claim.working_directory_id, claim.relative_cwd.as_deref())
                .unwrap();
            request
                .workdir_attachments
                .insert(claim.alias.clone(), binding);
        }
        self.execution.spawn_worker(request)
    }

    fn restore_worker(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        self.execution.restore_worker(request)
    }

    fn worker_snapshot(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> Option<protocol::Event> {
        if self
            .snapshot_script_worker
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|id| id == &worker.worker_id.to_string())
            && let Some(snapshot) = self.snapshot_script.lock().unwrap().pop_front()
        {
            let snapshot = snapshot?;
            let execution = self
                .execution
                .contexts
                .lock()
                .unwrap()
                .get(worker)
                .cloned()?;
            *execution.state.lock().unwrap() = snapshot;
        }
        self.execution.worker_snapshot(worker)
    }

    fn dispatch_input(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        input: worker_runtime::interaction::WorkerInput,
    ) -> WorkerExecutionResult {
        self.submits
            .lock()
            .unwrap()
            .push((worker.clone(), input.clone()));
        let mut result = self.execution.dispatch_input(worker, input);
        if self
            .lose_initial_ack
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            // Lose only the response acknowledgement, after the real fixture
            // execution committed its durable receipt. Never redispatch it.
            result.submission = None;
        }
        result
    }

    fn termination_receipt(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<Option<worker_runtime::execution::WorkerExecutionTermination>, String>
    {
        self.execution.termination_receipt(worker)
    }

    fn stop_worker_operation(
        &self,
        request: worker_runtime::execution::WorkerExecutionStopRequest,
    ) -> WorkerExecutionResult {
        if self.fail_stop.load(std::sync::atomic::Ordering::SeqCst) {
            return self.stop_worker(&request.worker_ref);
        }
        // Preserve the fixture's actual joined execution and operation-bound
        // termination receipt; an accepted legacy stop alone is not that evidence.
        self.execution.stop_worker_operation(request)
    }

    fn stop_worker(&self, worker: &worker_runtime::identity::WorkerRef) -> WorkerExecutionResult {
        if self.fail_stop.load(std::sync::atomic::Ordering::SeqCst) {
            return WorkerExecutionResult::errored(
                worker_runtime::execution::WorkerExecutionOperation::Stop,
                "TEST_ONLY_PRIVATE_STOP_CREDENTIAL /private/path",
            );
        }
        self.execution.stop_worker(worker)
    }
}

pub(super) struct LaunchFixture {
    _temp: tempfile::TempDir,
    pub(super) api: WorkspaceApi,
    owner_token: String,
    backend: Arc<LaunchExecutionBackend>,
    runtime_id: String,
    runtime_server: Option<tokio::task::JoinHandle<()>>,
    runtime_router: Option<Router>,
    runtime_identity: Option<RuntimeIdentityMaterial>,
}

impl Drop for LaunchFixture {
    fn drop(&mut self) {
        if let Some(gate) = self.backend.provider_close_gate.lock().unwrap().as_ref() {
            gate.release();
        }
        if let Some(server) = &self.runtime_server {
            server.abort();
        }
    }
}

impl LaunchFixture {
    pub(super) fn hold_provider_close(
        &self,
        gate: Arc<super::workdir_session_close_tests::ProviderCloseGate>,
    ) {
        *self.backend.provider_close_gate.lock().unwrap() = Some(gate);
    }

    pub(super) fn runtime_router(&self) -> Router {
        self.runtime_router.clone().unwrap()
    }

    pub(super) fn publish_profile(&self, profile: &str) {
        self.backend.publish_profile(&self.api, profile);
    }

    pub(super) fn seed_remote_session_for_cleanup(&self, worker: &RuntimeWorkerRef) {
        seed_worker_session_at_root(
            &self
                ._temp
                .path()
                .join(".test-embedded-runtime-store/source-runtime/workers")
                .join(&worker.worker_id),
        );
    }

    // Reuse the authenticated Runtime HTTP harness for Drive/source lifetime
    // assertions without duplicating its trust/bootstrap/listener lifecycle.
    pub(super) async fn with_source_workdir() -> (
        Self,
        worker_runtime::working_directory::WorkingDirectoryBinding,
    ) {
        let fixture = Self::new_with_runtime_store(true, true).await;
        init_clean_git_workspace(fixture._temp.path());
        let (_, Json(created)) = create_workspace_working_directory(
            &fixture.api,
            TEST_WORKSPACE_ID,
            Some(&fixture.runtime_id),
            BrowserWorkingDirectoryCreateRequest {
                runtime_id: Some(fixture.runtime_id.clone()),
                display_name: None,
                repository_key: "test-repository".into(),
                selector: Some("HEAD".into()),
                operation_id: Some("drive-source-workdir".into()),
            },
        )
        .await
        .unwrap();
        let binding = fixture
            .backend
            .execution
            .materializer
            .bind_working_directory(&created.item.working_directory_id, None)
            .unwrap();
        (fixture, binding)
    }

    async fn new(remote: bool) -> Self {
        Self::new_with_runtime_store(remote, false).await
    }

    async fn new_with_runtime_store(remote: bool, persistent: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = test_server_config(temp.path());
        let store = SqliteWorkspaceStore::open_for_test(config.database_path.clone()).unwrap();
        let backend = Arc::new(LaunchExecutionBackend {
            execution: DeterministicExecutionBackend {
                materializer: worker_runtime::working_directory::RuntimeGitMaterializer::new(
                    temp.path()
                        .join(".test-embedded-runtime-store/source-workdirs"),
                ),
                ..Default::default()
            },
            ..Default::default()
        });
        let api =
            WorkspaceApi::new_with_execution_backend(config, Arc::new(store), backend.clone())
                .await
                .unwrap();
        let owner_token =
            seed_test_workspace_owner_token(api.store.as_ref(), "ticket-launch-owner").await;
        let mut fixture = Self {
            _temp: temp,
            api,
            owner_token,
            backend,
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            runtime_server: None,
            runtime_router: None,
            runtime_identity: None,
        };
        if remote {
            fixture.runtime_id = "ticket-worker-real-http".to_string();
            use worker_runtime::workspace_issuer::{
                InMemoryWorkspaceClaimReplayProtection, WorkspaceCapabilityVerifier,
                WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState,
            };
            let runtime_identity = RuntimeIdentityMaterial::generate(&fixture.runtime_id).unwrap();
            fixture.runtime_identity = Some(runtime_identity.clone());
            let workspace_identity = fixture
                .api
                .signing_identities
                .provision_existing(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
                .unwrap();
            let backend_url = fixture.api.config.backend_base_url.clone().unwrap();
            let verifier = WorkspaceCapabilityVerifier::new(
                vec![WorkspaceIssuerTrustRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    backend_url: backend_url.clone(),
                    key_id: workspace_identity.key_id.clone(),
                    algorithm: workspace_identity.algorithm.clone(),
                    public_key: workspace_identity.public_key.clone().unwrap(),
                    public_key_fingerprint: workspace_identity
                        .public_key_fingerprint
                        .clone()
                        .unwrap(),
                    trust_id: "trust-launch-test".to_string(),
                    state: WorkspaceIssuerTrustState::Active,
                    registered_at_unix: 1,
                    updated_at_unix: 1,
                }],
                Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
            )
            .unwrap();
            let runtime = if persistent {
                worker_runtime::Runtime::with_fs_store_and_execution_backend(
                    worker_runtime::fs_store::FsRuntimeStoreOptions {
                        root: fixture
                            ._temp
                            .path()
                            .join(".test-embedded-runtime-store/source-runtime"),
                        runtime_id: fixture.runtime_id.clone(),
                        display_name: None,
                    },
                    fixture.backend.clone(),
                )
            } else {
                worker_runtime::Runtime::with_execution_backend(
                    worker_runtime::RuntimeOptions::default(),
                    fixture.backend.clone(),
                )
            }
            .unwrap();
            runtime.bind_runtime_identity(&fixture.runtime_id).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let binding = WorkspaceRuntimeBinding {
                workspace_id: TEST_WORKSPACE_ID.into(),
                runtime_id: fixture.runtime_id.clone(),
                display_name: "Ticket Worker real HTTP Runtime".into(),
                base_url: endpoint.clone(),
                public_key: runtime_identity.public_key.clone(),
                public_key_fingerprint: String::new(),
                binding_id: "binding-launch-test".to_string(),

                authentication_mode: StoredRuntimeAuthenticationMode::WorkspaceIdentity,

                created_at: TEST_CREATED_AT.into(),
                updated_at: TEST_CREATED_AT.into(),
                revoked_at: None,
            };
            fixture
                .api
                .store
                .upsert_workspace_runtime_binding_record(binding.clone(), false)
                .await
                .unwrap();
            let binding = fixture
                .api
                .store
                .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &fixture.runtime_id)
                .await
                .unwrap()
                .unwrap();
            let runtime_id = fixture.runtime_id.clone();
            let router = worker_runtime::http_server::runtime_http_router_with_workspace_auth(
                runtime,
                None,
                worker_runtime::http_server::WorkspaceRuntimeHttpAuth {
                    verifier,
                    runtime_id,
                },
            );
            fixture.runtime_router = Some(router.clone());
            fixture.runtime_server = Some(tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            }));
            let mut remote_config = RemoteRuntimeConfig {
                runtime_id: fixture.runtime_id.clone(),
                workspace_id: Some(TEST_WORKSPACE_ID.into()),
                display_name: binding.display_name.clone(),
                base_url: endpoint,
                bearer_token: None,
                workspace_authorization: None,
                strict_public_egress: false,
                cached_worker_creation_available: true,
                cached_os: "linux".into(),
                cached_arch: "x86_64".into(),
                cached_status: "active".into(),
                timeout: std::time::Duration::from_secs(5),
            };
            remote_config.workspace_authorization =
                Some(crate::hosts::WorkspaceRuntimeAuthorization::new(
                    fixture.api.store.clone(),
                    fixture.api.signing_identities.clone(),
                    backend_url.clone(),
                    binding,
                ));
            fixture.api.runtime.register_or_replace(
                RemoteWorkerRuntime::new(remote_config, TEST_WORKSPACE_ID.to_string(), backend_url)
                    .unwrap(),
            );
        }
        fixture
    }

    fn ticket(&self) -> String {
        browser_ticket_backend(&self.api)
            .unwrap()
            .create(ticket::NewTicket::new(
                "Explicit Worker launch on an open case",
            ))
            .unwrap()
            .id
    }

    fn case_action(&self, ticket_id: &str, operation_key: &str, action: ticket::TicketCaseAction) {
        let backend = browser_ticket_backend(&self.api).unwrap();
        let before = backend.show(ticket_id.into()).unwrap();
        backend
            .update_case(
                ticket_id,
                ticket::TicketCaseUpdate {
                    expected_content_digest: ticket::ticket_content_digest(&before),
                    expected_state: before.meta.workflow_state,
                    expected_closure_operation_key: before.closure_operation_key,
                    operation_key: operation_key.into(),
                    action,
                    explanation: "Explicit fixture case judgment".into(),
                    references: Vec::new(),
                    author: Some("fixture".into()),
                },
            )
            .unwrap();
    }

    fn payload(&self, _ticket_id: &str, operation_id: &str, profile: &str, flow: bool) -> Value {
        self.backend.publish_profile(&self.api, profile);
        let mut segments = vec![
            json!({"kind": "text", "content": "Research this Ticket without changing repositories."}),
        ];
        if flow {
            segments.insert(
                0,
                json!({"kind": "flow", "selector": "builtin:coder-review"}),
            );
        }
        json!({
            "runtime_id": self.runtime_id, "display_name": "Explicit Ticket Worker", "profile": profile,
            "initial_submit": segments, "workdir_attachments": [], "feature_connections": [],
            "authorization": { "kind": "assignment", "assignment": null },
            "control_operation_id": format!("launch-{operation_id}"),
        })
    }

    async fn post(&self, workspace_id: &str, payload: Value) -> (StatusCode, Value) {
        let response = build_router(self.api.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{workspace_id}/workers"))
                    .header("authorization", format!("Bearer {}", self.owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{status}: {error}: {}", String::from_utf8_lossy(&bytes))
        });
        (status, body)
    }

    fn assert_assigned(
        &self,
        ticket_id: &str,
        operation_id: &str,
        body: &Value,
    ) -> TicketWorkerAssignmentRecord {
        assert_eq!(body["workspace_id"], TEST_WORKSPACE_ID, "{body}");
        assert_eq!(body["runtime_id"], self.runtime_id, "{body}");
        assert!(
            body["console_href"]
                .as_str()
                .unwrap()
                .starts_with(&format!("/w/{TEST_WORKSPACE_ID}/workers/")),
            "{body}"
        );
        let current = self
            .api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, ticket_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            current.worker,
            RuntimeWorkerRef::new(&self.runtime_id, body["worker_id"].as_str().unwrap())
        );
        assert!(
            self.api
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &current.worker)
                .unwrap()
                .is_empty(),
            "workdirless Ticket Worker must have no authoritative attachments: {body}"
        );
        let operation = self
            .api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(operation.worker, Some(current.worker.clone()));
        assert_eq!(
            operation.assignment_id.as_deref(),
            Some(current.assignment_id.as_str())
        );
        assert_eq!(
            self.api.authority.ticket(ticket_id).unwrap().state.as_str(),
            "open"
        );
        current
    }
}

// Ordinary Backend creation is deliberately independent of Ticket claims. These
// tests use the real Embedded/HTTP Runtime and SQLite; only execution is scripted.
#[tokio::test(flavor = "multi_thread")]
async fn ordinary_spawn_preserves_explicit_flow_and_profile_without_ticket_or_workdir() {
    for remote in [false, true] {
        let f = LaunchFixture::new(remote).await;
        for (profile, flow) in [("builtin:intake", false), ("builtin:coder", true)] {
            let key = format!("ordinary-{flow}");
            let payload = f.payload("", &key, profile, flow);
            assert!(payload.get("ticket_assignment").is_none());
            let expected = payload["initial_submit"].clone();
            let (status, body) = f.post(TEST_WORKSPACE_ID, payload.clone()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let id = body["worker_id"].as_str().unwrap();
            assert_eq!(body["worker"]["profile"], profile);
            assert!(
                f.api
                    .store
                    .get_active_ticket_role_assignment_for_worker(
                        TEST_WORKSPACE_ID,
                        &RuntimeWorkerRef::new(&f.runtime_id, id)
                    )
                    .unwrap()
                    .is_none()
            );
            assert_eq!(f.backend.creates_for(id).len(), 1);
            assert_eq!(
                serde_json::to_value(&f.backend.submits_for(id)[0].segments).unwrap(),
                expected
            );
            let (status, replay) = f.post(TEST_WORKSPACE_ID, payload).await;
            assert_eq!(status, StatusCode::OK, "{replay}");
            assert_eq!(replay["worker_id"], body["worker_id"]);
            assert_eq!(f.backend.creates_for(id).len(), 1);
            assert_eq!(f.backend.submits_for(id).len(), 1);
        }
    }
}
impl LaunchFixture {
    async fn assign(&self, ticket: &str, worker: &Value, key: &str) -> (StatusCode, Value) {
        let response = build_router(self.api.clone()).oneshot(Request::builder().method("PUT")
            .uri(format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/worker"))
            .header("authorization",format!("Bearer {}",self.owner_token)).header("content-type","application/json")
            .body(Body::from(json!({"operation_id":key,"expected_assignment_id":null,"principal":{"kind":"worker","runtime_id":self.runtime_id,"worker_id":worker["worker_id"]},"workdir_bindings":[]}).to_string())).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"unparsed":String::from_utf8_lossy(&bytes)})),
        )
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn existing_idle_worker_assignment_is_separate_and_replays_without_restart_or_submit() {
    for remote in [false, true] {
        let mut f = LaunchFixture::new(remote).await;
        let ticket = f.ticket();
        let before = f.api.authority.ticket(&ticket).unwrap().content_digest;
        let mut create = f.payload("", "existing-idle", "builtin:companion", false);
        create["initial_submit"] = json!([]);
        let (status, worker) = f.post(TEST_WORKSPACE_ID, create).await;
        assert_eq!(status, StatusCode::OK, "{worker}");
        let id = worker["worker_id"].as_str().unwrap();
        assert!(f.backend.submits_for(id).is_empty());
        let (status, assigned) = f.assign(&ticket, &worker, "assign-existing").await;
        assert_eq!(status, StatusCode::OK, "{assigned}");
        if remote {
            // Reconstruct Backend state over reopened SQLite while the external
            // Runtime remains alive; no process-memory receipt authorizes replay.
            let runtime = f.api.runtime.clone();
            let store = Arc::new(
                SqliteWorkspaceStore::open_for_test(f.api.config.database_path.clone()).unwrap(),
            );
            let mut reopened = WorkspaceApi::new_with_execution_backend(
                f.api.config.clone().with_embedded_runtime_store_root(
                    f._temp.path().join("reopened-backend-unused-embedded"),
                ),
                store,
                f.backend.clone(),
            )
            .await
            .unwrap();
            reopened.runtime = runtime;
            f.api = reopened;
        }
        let (status, replay) = f.assign(&ticket, &worker, "assign-existing").await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(assigned, replay);
        f.assert_assigned(&ticket, "assign-existing", &worker);
        assert_eq!(
            f.backend.creates_for(id).len(),
            1,
            "assignment must not restart"
        );
        assert!(
            f.backend.submits_for(id).is_empty(),
            "assignment is not input acceptance"
        );
        assert_eq!(
            f.api.authority.ticket(&ticket).unwrap().content_digest,
            before
        );
        let segments = json!([
            {"kind":"flow","selector":"builtin:coder-review"},
            {"kind":"text","content":"Implement only the now-assigned Ticket"}
        ]);
        let response = build_router(f.api.clone()).oneshot(Request::builder().method("POST")
            .uri(format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{id}/input", f.runtime_id))
            .header("authorization", format!("Bearer {}", f.owner_token))
            .header("content-type", "application/json")
            .body(Body::from(json!({"kind":"user", "content":"Implement only the now-assigned Ticket", "segments":segments}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(f.backend.submits_for(id).len(), 1);
        assert_eq!(
            serde_json::to_value(&f.backend.submits_for(id)[0].segments).unwrap(),
            segments
        );
        let other = f.ticket();
        let (status, _) = f.assign(&other, &worker, "double-assign").await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn creation_success_assignment_failure_preserves_worker_and_no_initial_ticket_effect() {
    let f = LaunchFixture::new(false).await;
    let ticket = f.ticket();
    f.case_action(
        &ticket,
        "close-before-claim",
        ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::NotPlanned,
        },
    );
    let mut create = f.payload("", "created-unassigned", "builtin:companion", false);
    create["initial_submit"] = json!([]);
    let (status, worker) = f.post(TEST_WORKSPACE_ID, create).await;
    assert_eq!(status, StatusCode::OK, "{worker}");
    let (status, error) = f.assign(&ticket, &worker, "failed-claim").await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    let id = worker["worker_id"].as_str().unwrap();
    assert_eq!(f.backend.creates_for(id).len(), 1);
    assert!(f.backend.submits_for(id).is_empty());
    assert_eq!(
        f.api
            .runtime
            .worker(&RuntimeWorkerRef::new(&f.runtime_id, id))
            .unwrap()
            .state,
        "idle"
    );
    assert!(
        f.api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_none()
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn assignment_failure_keeps_same_worker_and_exact_receipt_instead_of_spawn_compensation() {
    let f = LaunchFixture::new(false).await;
    let ticket = f.ticket();
    let mut create = f.payload("", "claim-fault-worker", "builtin:companion", false);
    create["initial_submit"] = json!([]);
    let (_, worker) = f.post(TEST_WORKSPACE_ID, create).await;
    rusqlite::Connection::open(&f.api.config.database_path).unwrap().execute_batch("CREATE TRIGGER fail_claim BEFORE INSERT ON ticket_current_worker_assignments WHEN NEW.role='worker' BEGIN SELECT RAISE(ABORT,'injected claim failure'); END;").unwrap();
    let (status, _) = f.assign(&ticket, &worker, "failed-persist").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let op = f
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "failed-persist")
        .unwrap()
        .unwrap();
    assert_eq!(op.claim_state, "failed");
    let id = worker["worker_id"].as_str().unwrap();
    assert_eq!(f.backend.creates_for(id).len(), 1);
    assert!(f.backend.submits_for(id).is_empty());
    assert!(
        f.api
            .runtime
            .worker(&RuntimeWorkerRef::new(&f.runtime_id, id))
            .is_ok()
    );
    let (status, _) = f.assign(&ticket, &worker, "failed-persist").await;
    assert_eq!(status, StatusCode::CONFLICT);
}
#[tokio::test(flavor = "multi_thread")]
async fn legacy_claim_receipt_is_retained_and_never_reinterpreted_as_a_fresh_assignment() {
    let f = LaunchFixture::new(false).await;
    let ticket = f.ticket();
    let mut create = f.payload("", "legacy-reconcile-worker", "builtin:companion", false);
    create["initial_submit"] = json!([]);
    let (status, worker) = f.post(TEST_WORKSPACE_ID, create).await;
    assert_eq!(status, StatusCode::OK);
    let id = worker["worker_id"].as_str().unwrap();
    f.api
        .store
        .reserve_ticket_assignment_operation(
            TEST_WORKSPACE_ID,
            "legacy-operation",
            &ticket,
            &f.runtime_id,
            Some(id),
            "saved-combined-spawn-fingerprint",
            TEST_CREATED_AT,
        )
        .unwrap();
    let (status, _) = f.assign(&ticket, &worker, "legacy-operation").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let reopened = SqliteWorkspaceStore::open_for_test(f.api.config.database_path.clone()).unwrap();
    let saved = reopened
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "legacy-operation")
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.request_fingerprint.as_deref(),
        Some("saved-combined-spawn-fingerprint")
    );
    assert_eq!(saved.worker, Some(RuntimeWorkerRef::new(&f.runtime_id, id)));
    assert_eq!(saved.claim_state, "pending");
    assert!(saved.assignment_id.is_none());
    assert_eq!(f.backend.creates_for(id).len(), 1);
    assert!(f.backend.submits_for(id).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn new_worker_contract_rejects_old_combined_claim_before_creation() {
    let f = LaunchFixture::new(false).await;
    let mut p = f.payload("", "old-input", "builtin:companion", false);
    p["ticket_assignment"] = json!({"ticket_id":"T-1","operation_id":"old"});
    let response = build_router(f.api.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/w/{TEST_WORKSPACE_ID}/workers"))
                .header("authorization", format!("Bearer {}", f.owner_token))
                .header("content-type", "application/json")
                .body(Body::from(p.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(f.backend.creates.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn initial_input_ack_loss_preserves_creation_facts_across_embedded_and_remote() {
    use runtime_api::startup::*;
    for failed_cleanup in [false, true] {
        let mut observed = Vec::new();
        for remote in [false, true] {
            let fixture = LaunchFixture::new(remote).await;
            let ticket = fixture.ticket();
            let payload = fixture.payload(&ticket, "lost-initial-ack", "builtin:companion", false);
            fixture
                .backend
                .lose_initial_ack
                .store(true, std::sync::atomic::Ordering::SeqCst);
            fixture
                .backend
                .fail_stop
                .store(failed_cleanup, std::sync::atomic::Ordering::SeqCst);
            let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
            let DecodeStartupProblem::Known(failure) = decode_startup_problem(
                &serde_json::to_vec(&body).unwrap(),
                status.as_u16(),
                PROBLEM_CONTENT_TYPE,
                StartupBoundary::Browser,
            ) else {
                panic!("must retain typed creation facts: {body}");
            };
            assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
            assert_eq!(failure.operation().execution, ExecutionOutcome::Unknown);
            let finalize = failure.operation().finalize.unwrap();
            assert_eq!(finalize.stage, FinalizeStage::InitialInput);
            assert_eq!(
                finalize.compensation,
                if failed_cleanup {
                    CompensationOutcome::Unknown
                } else {
                    CompensationOutcome::Completed
                }
            );
            assert_eq!(body["yoi_failure"]["reason"], "invalid_response");
            assert!(
                !body
                    .to_string()
                    .contains("TEST_ONLY_PRIVATE_STOP_CREDENTIAL")
            );
            let creates = fixture.backend.creates.lock().unwrap().clone();
            assert_eq!(creates.len(), 1);
            let worker_id = creates[0].worker_id.to_string();
            let worker = RuntimeWorkerRef::new(&fixture.runtime_id, &worker_id);
            let Some(DecodeStoredWorkerCreateFailure::Known(saved)) =
                fixture.api.worker_create_failure(&worker).unwrap()
            else {
                panic!("retain queryable create result");
            };
            assert_eq!(saved.public_facts(), failure.public_facts());
            assert_eq!(
                fixture.backend.submits_for(&worker_id).len(),
                1,
                "observation failure never redispatches"
            );
            assert_eq!(
                fixture
                    .backend
                    .execution
                    .contexts
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|worker| worker.worker_id.to_string() == worker_id),
                failed_cleanup
            );
            let mut facts = failure.public_facts();
            facts.operation.correlation = None;
            observed.push((status, body["yoi_failure"]["reason"].clone(), facts));
        }
        assert_eq!(
            observed[0], observed[1],
            "same owner facts through Embedded and Remote"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mr_open_retains_source_before_first_review_and_observed_result() {
    let fixture = LaunchFixture::new(true).await;
    let dir = &fixture._temp;
    init_clean_git_workspace(dir.path());
    let git = |root: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(dir.path(), &["branch", "work/open-observation"]);
    let source_a = git(dir.path(), &["rev-parse", "work/open-observation"]);
    let api = &fixture.api;
    fixture.backend.publish_profile(api, "builtin:coder");
    let identity = fixture.runtime_identity.as_ref().unwrap();
    let mut input = ticket::NewTicket::new("Opening observation and pre-review push");
    set_test_ticket_target(&mut input, "test-repository", "develop");
    let ticket = browser_ticket_backend(&api).unwrap().create(input).unwrap();
    assign_test_orchestrator(&api, &ticket.id);
    let (_, Json(workdir)) = create_workspace_working_directory(
        api,
        TEST_WORKSPACE_ID,
        Some(&fixture.runtime_id),
        BrowserWorkingDirectoryCreateRequest {
            runtime_id: Some(fixture.runtime_id.clone()),
            display_name: None,
            repository_key: "test-repository".into(),
            selector: Some("work/open-observation".into()),
            operation_id: Some("opening-workdir".into()),
        },
    )
    .await
    .unwrap();
    let checkout = fixture
        .backend
        .execution
        .materializer
        .bind_working_directory(&workdir.item.working_directory_id, None)
        .unwrap();
    git(
        checkout.root(),
        &["checkout", "-B", "work/open-observation"],
    );
    let Json(created) = create_workspace_worker_as_owner(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: fixture.runtime_id.clone(),
            display_name: "source-observation-coder".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            initial_submit: Vec::new(),
            workdir_attachments: vec![server_api::BrowserWorkerWorkingDirectorySelection {
                alias: "checkout".into(),
                working_directory_id: workdir.item.working_directory_id,
                relative_cwd: None,
            }],
            feature_connections: Default::default(),
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: test_worker_operation_authorization(&[
                ("ticket", &["ticket.implementation_result.write"]),
                (
                    "merge_request",
                    &["merge_request.open", "merge_request.read"],
                ),
            ]),
        }),
    )
    .await
    .unwrap();
    let coder = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    seed_worker_source_member(&api, &coder.runtime_id, &coder.worker_id);
    api.store
        .set_current_ticket_worker_assignment(
            &TicketWorkerAssignmentRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                ticket_id: ticket.id.clone(),
                assignment_id: "opening-coder".into(),
                worker: coder.clone(),
                assigned_by: "test".into(),
                assigned_at: Utc::now().to_rfc3339(),
            },
            None,
            "opening-assigned",
            "opening-operation",
            false,
        )
        .unwrap();
    let app = build_router(api.clone());
    let call = |method: &'static str, path: String, input: Value, expected: StatusCode| {
        let body = if method == "GET" {
            Vec::new()
        } else {
            serde_json::to_vec(&input).unwrap()
        };
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity)
            .issue(
                api.config.backend_base_url.as_deref().unwrap(),
                TEST_WORKSPACE_ID,
                Some(&coder.worker_id),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                method,
                &path,
                &body,
                worker_runtime::auth::unix_now_seconds() as i64,
                30,
            )
            .unwrap();
        let request = Request::builder()
            .method(method)
            .uri(&path)
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .header("x-yoi-runtime-id", &coder.runtime_id)
            .header("x-yoi-worker-id", &coder.worker_id)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };
    let open_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/merge-request",
        ticket.id
    );
    let open_input = json!({"repository_key":"test-repository","selector_from":"work/open-observation","selector_to":"develop","summary":""});
    // A failed opening observation cannot leave behind a successfully opened MR.
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_opening_observation BEFORE INSERT ON merge_request_source_observations BEGIN SELECT RAISE(ABORT,'injected opening capture failure'); END;")?;
        Ok(())
    }).unwrap();
    call(
        "POST",
        open_path.clone(),
        open_input.clone(),
        StatusCode::CONFLICT,
    )
    .await;
    api.config_store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row("SELECT count(*) FROM merge_requests", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            conn.execute_batch("DROP TRIGGER reject_opening_observation;")?;
            Ok(())
        })
        .unwrap();
    let opened = call("POST", open_path, open_input, StatusCode::OK).await;
    let mr = opened["merge_request_id"].as_str().unwrap();
    api.config_store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT subject_ref FROM merge_request_source_observations WHERE merge_request_id=?1",[mr],|r|r.get::<_,String>(0))?,source_a);
        assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE kind='mr_source_changed'",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).unwrap();
    let publisher = tempfile::tempdir().unwrap();
    git(
        publisher.path(),
        &["clone", dir.path().to_str().unwrap(), "."],
    );
    git(publisher.path(), &["checkout", "work/open-observation"]);
    git(
        publisher.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "source-b",
        ],
    );
    git(
        publisher.path(),
        &["push", "origin", "work/open-observation"],
    );
    let source_b = git(publisher.path(), &["rev-parse", "HEAD"]);
    assert_ne!(source_a, source_b);
    for _ in 0..2 {
        call(
            "GET",
            format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr}"),
            Value::Null,
            StatusCode::OK,
        )
        .await;
    }
    api.config_store.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT count(*) FROM merge_request_thread_events WHERE merge_request_id=?1 AND kind IN ('review_requested','review')",[mr],|r|r.get::<_,i64>(0))?,0,"no review event mentions B");
        let (previous,current):(String,String)=conn.query_row("SELECT previous_subject_ref,subject_ref FROM merge_request_source_observations WHERE merge_request_id=?1 ORDER BY sequence DESC LIMIT 1",[mr],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert_eq!((previous,current),(source_a.clone(),source_b.clone()));
        assert_eq!(conn.query_row("SELECT count(*) FROM notification_sources WHERE kind='mr_source_changed' AND json_extract(facts_json,'$.merge_request_id')=?1",[mr],|r|r.get::<_,i64>(0))?,2,"one source per logical recipient, not per read");
        Ok(())
    }).unwrap();
    let result_path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/implementation-results",
        ticket.id
    );
    let mut result = json!({"operation_key":"observed-result","kind":"handoff","body":"Published new source; review not requested yet.","merge_request":{"merge_request_id":mr,"subject_ref":"forged-source"}});
    call(
        "POST",
        result_path.clone(),
        result.clone(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    result["merge_request"]["subject_ref"] = json!(source_b);
    let receipt = call("POST", result_path.clone(), result.clone(), StatusCode::OK).await;
    assert_eq!(
        call("POST", result_path, result, StatusCode::OK).await,
        receipt
    );
    api.runtime
        .stop_worker(
            &coder,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
}

// Real Runtime creation and SQLite finalization fail independently; only Stop is
// injected. Reconciliation must retain the create reservation while absence is unknown.

// The execution boundary is deterministic; both Backend adapters, Runtime detail
// refresh/publication, HTTP conversion (remote), and assignment persistence are real.
#[tokio::test(flavor = "multi_thread")]
async fn assignment_rejects_live_running_and_unavailable_despite_catalog_idle() {
    for remote in [false, true] {
        for unavailable in [false, true] {
            let f = LaunchFixture::new(remote).await;
            let ticket = f.ticket();
            let mut create = f.payload("", "live-assignment", "builtin:companion", false);
            create["initial_submit"] = json!([]);
            let (status, worker) = f.post(TEST_WORKSPACE_ID, create).await;
            assert_eq!(status, StatusCode::OK, "{worker}");
            let id = worker["worker_id"].as_str().unwrap();
            let mut running = protocol::WorkerStateSnapshot::initial();
            running.state_sequence = 1;
            running.state = protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            ));
            f.backend
                .snapshot_script
                .lock()
                .unwrap()
                .push_back(if unavailable { None } else { Some(running) });
            let (status, error) = f.assign(&ticket, &worker, "reject-live").await;
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "remote={remote} unavailable={unavailable}: {error}"
            );
            assert!(
                f.api
                    .store
                    .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
                    .unwrap()
                    .is_none()
            );
            assert!(f.backend.submits_for(id).is_empty());
            assert_eq!(f.backend.creates_for(id).len(), 1);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn assignment_rechecks_live_state_after_initial_idle_before_unchanged_commit() {
    for remote in [false, true] {
        let f = LaunchFixture::new(remote).await;
        let ticket = f.ticket();
        let mut create = f.payload("", "assignment-race", "builtin:companion", false);
        create["initial_submit"] = json!([]);
        let (status, worker) = f.post(TEST_WORKSPACE_ID, create).await;
        assert_eq!(status, StatusCode::OK, "{worker}");
        let mut running = protocol::WorkerStateSnapshot::initial();
        running.state_sequence = 1;
        running.state = protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
            protocol::WorkerRunState::Running,
        ));
        *f.backend.snapshot_script.lock().unwrap() = [
            Some(protocol::WorkerStateSnapshot::initial()),
            Some(running),
        ]
        .into();
        let (status, error) = f.assign(&ticket, &worker, "changed-before-commit").await;
        assert_eq!(status, StatusCode::CONFLICT, "remote={remote}: {error}");
        assert!(f.backend.snapshot_script.lock().unwrap().is_empty());
        assert!(
            f.api
                .store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
                .unwrap()
                .is_none()
        );
        let claim = f
            .api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "changed-before-commit")
            .unwrap()
            .unwrap();
        assert_eq!(claim.claim_state, "failed");
        assert!(claim.assignment_id.is_none());
        assert!(
            f.backend
                .submits_for(worker["worker_id"].as_str().unwrap())
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_worker_list_and_details_follow_embedded_and_remote_execution() {
    for remote in [false, true] {
        let f = LaunchFixture::new(remote).await;
        f.backend.execution.accept_restores();
        let mut create = f.payload("", "list-controller", "builtin:companion", false);
        create["initial_submit"] = json!([]);
        create["authorization"] = serde_json::to_value(test_worker_operation_authorization(&[(
            "worker",
            &["worker.list", "worker.restore"],
        )]))
        .unwrap();
        let (status, controller) = f.post(TEST_WORKSPACE_ID, create).await;
        assert_eq!(status, StatusCode::OK, "{controller}");
        let controller =
            RuntimeWorkerRef::new(&f.runtime_id, controller["worker_id"].as_str().unwrap());
        let mut create = f.payload("", "list-subject", "builtin:companion", false);
        create["initial_submit"] = json!([]);
        let (status, subject) = f.post(TEST_WORKSPACE_ID, create).await;
        assert_eq!(status, StatusCode::OK, "{subject}");
        let subject = RuntimeWorkerRef::new(&f.runtime_id, subject["worker_id"].as_str().unwrap());
        f.api
            .store
            .create_worker_control_grant(&WorkerControlGrantRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                grant_id: "list-execution".into(),
                controller: controller.clone(),
                subject: subject.clone(),
                relation: "spawned".into(),
                origin: "fixture".into(),
                permissions: vec!["send_input".into(), "restore".into()],
                operation_id: "list-execution".into(),
                created_at: now_registry_timestamp(),
                revoked_at: None,
            })
            .unwrap();
        for (sequence, status, expected) in [
            (
                1,
                protocol::WorkerStatus::Idle,
                server_api::WorkerListState::Idle,
            ),
            (
                2,
                protocol::WorkerStatus::Running,
                server_api::WorkerListState::Running,
            ),
            (
                3,
                protocol::WorkerStatus::Paused,
                server_api::WorkerListState::Paused,
            ),
            (
                4,
                protocol::WorkerStatus::Idle,
                server_api::WorkerListState::Idle,
            ),
        ] {
            let mut snapshot: protocol::WorkerStateSnapshot = status.into();
            snapshot.state_sequence = sequence;
            f.backend.execution.publish_worker_state(&subject, snapshot);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let listed = list_known_workers(
                        State(f.api.clone()),
                        AxumPath(ScopedWorkspacePath {
                            workspace_id: TEST_WORKSPACE_ID.into(),
                        }),
                        ticket_check_headers(&controller),
                    )
                    .await
                    .unwrap()
                    .0;
                    assert_eq!(listed.items.len(), 1);
                    if listed.items[0].state == expected {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("list must reflect current execution over both host adapters");
        }
        *f.backend.snapshot_script_worker.lock().unwrap() = Some(subject.worker_id.clone());
        f.backend.snapshot_script.lock().unwrap().push_back(None);
        let unavailable = list_known_workers(
            State(f.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            ticket_check_headers(&controller),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            unavailable.items[0].state,
            server_api::WorkerListState::Unavailable
        );
        // Reconnection replays the execution-owned latest snapshot, without a
        // new Run or a fabricated catalog state/sequence.
        f.backend.execution.publish_worker_state(
            &subject,
            protocol::WorkerStateSnapshot {
                state_sequence: 4,
                ..protocol::WorkerStateSnapshot::initial()
            },
        );
        let recovered = list_known_workers(
            State(f.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            ticket_check_headers(&controller),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            recovered.items[0].state,
            server_api::WorkerListState::Idle,
            "reconnected snapshot must restore observation without fabricating Idle from the catalog"
        );
        let get_details = || {
            known_worker_details(
                State(f.api.clone()),
                AxumPath(ScopedRuntimeWorkerPath {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    worker: subject.clone(),
                }),
                ticket_check_headers(&controller),
            )
        };
        let empty = get_details().await.unwrap().0;
        assert_eq!(empty.workdir_connections, Some(vec![]));
        f.api
            .runtime
            .stop_worker(
                &subject,
                WorkerLifecycleRequest {
                    stop_intent: None,
                    reason: Some("list restore token check".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let stopped = get_details().await.unwrap().0;
        assert_eq!(stopped.state, server_api::WorkerListState::Stopped);
        let token = stopped.restore_observation_token.unwrap();
        let restored = restore_known_worker(
            State(f.api.clone()),
            AxumPath(ScopedRuntimeWorkerPath {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: subject.clone(),
            }),
            ticket_check_headers(&controller),
            server_api::WorkerRestoreRequest {
                expected_observation_token: token.clone(),
                request_id: "list-restore".into(),
            },
        )
        .await
        .unwrap()
        .0
        .result;
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        f.api
            .runtime
            .stop_worker(
                &subject,
                WorkerLifecycleRequest {
                    stop_intent: None,
                    reason: None,
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let stale = restore_known_worker(
            State(f.api.clone()),
            AxumPath(ScopedRuntimeWorkerPath {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: subject.clone(),
            }),
            ticket_check_headers(&controller),
            server_api::WorkerRestoreRequest {
                expected_observation_token: token,
                request_id: "stale-list-restore".into(),
            },
        )
        .await;
        assert!(
            stale.is_err(),
            "remote={remote}: old detail token cannot survive ABA: {stale:?}"
        );
        assert_eq!(
            stale.unwrap_err().into_response().status(),
            StatusCode::CONFLICT
        );
    }
}
