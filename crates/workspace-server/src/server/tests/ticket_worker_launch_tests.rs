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
    execution: DeterministicExecutionBackend,
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

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        self.creates.lock().unwrap().push(request.request.clone());
        self.execution.spawn_worker(request)
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
        self.execution.dispatch_input(worker, input)
    }

    fn stop_worker(&self, worker: &worker_runtime::identity::WorkerRef) -> WorkerExecutionResult {
        self.execution.stop_worker(worker)
    }
}

struct LaunchFixture {
    _temp: tempfile::TempDir,
    api: WorkspaceApi,
    backend: Arc<LaunchExecutionBackend>,
    runtime_id: String,
    runtime_server: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for LaunchFixture {
    fn drop(&mut self) {
        if let Some(server) = &self.runtime_server {
            server.abort();
        }
    }
}

impl LaunchFixture {
    async fn new(remote: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = test_server_config(temp.path());
        let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
        let backend = Arc::new(LaunchExecutionBackend::default());
        let api =
            WorkspaceApi::new_with_execution_backend(config, Arc::new(store), backend.clone())
                .await
                .unwrap();
        let mut fixture = Self {
            _temp: temp,
            api,
            backend,
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            runtime_server: None,
        };
        if remote {
            fixture.runtime_id = "ticket-worker-real-http".to_string();
            use worker_runtime::workspace_issuer::{
                InMemoryWorkspaceClaimReplayProtection,
                InMemoryWorkspaceRuntimeVerificationAuthority, RuntimeVerificationSigner,
                WorkspaceCapabilityVerifier, WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState,
            };
            let runtime_identity = RuntimeIdentityMaterial::generate(&fixture.runtime_id).unwrap();
            let signer = RuntimeVerificationSigner::from_identity(&runtime_identity).unwrap();
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
                    identity_revision: workspace_identity.revision,
                    trust_generation: workspace_identity.revision,
                    state: WorkspaceIssuerTrustState::Active,
                    registered_at_unix: 1,
                    updated_at_unix: 1,
                }],
                Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
            )
            .unwrap();
            let runtime = worker_runtime::Runtime::with_execution_backend(
                worker_runtime::RuntimeOptions::default(),
                fixture.backend.clone(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let binding = WorkspaceRuntimeBinding {
                workspace_id: TEST_WORKSPACE_ID.into(),
                runtime_id: fixture.runtime_id.clone(),
                display_name: "Ticket Worker real HTTP Runtime".into(),
                base_url: endpoint.clone(),
                public_key: runtime_identity.public_key.clone(),
                public_key_fingerprint: signer.public_key_fingerprint().to_string(),
                binding_revision: 1,
                state: StoredRuntimeBindingState::Configured,
                authentication_mode: StoredRuntimeAuthenticationMode::WorkspaceIdentity,
                workspace_key_id: Some(workspace_identity.key_id),
                workspace_key_generation: Some(workspace_identity.revision),
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
            fixture.runtime_server = Some(tokio::spawn(async move {
                worker_runtime::http_server::serve_runtime_http_with_workspace_auth(
                    runtime,
                    listener,
                    None,
                    worker_runtime::http_server::WorkspaceRuntimeHttpAuth {
                        verifier,
                        signer,
                        verifications: Arc::new(
                            InMemoryWorkspaceRuntimeVerificationAuthority::default(),
                        ),
                    },
                )
                .await
                .unwrap();
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
            fixture.api.runtime.register_or_replace(
                RemoteWorkerRuntime::new(
                    remote_config.clone(),
                    TEST_WORKSPACE_ID.to_string(),
                    backend_url.clone(),
                )
                .unwrap(),
            );
            let verified = perform_workspace_runtime_verification(
                &fixture.api,
                fixture.api.runtime.clone(),
                &binding,
            )
            .await
            .unwrap();
            remote_config.workspace_authorization =
                Some(crate::hosts::WorkspaceRuntimeAuthorization::new(
                    fixture.api.store.clone(),
                    fixture.api.signing_identities.clone(),
                    backend_url.clone(),
                    Some(verified),
                ));
            fixture.api.runtime.register_or_replace(
                RemoteWorkerRuntime::new(remote_config, TEST_WORKSPACE_ID.to_string(), backend_url)
                    .unwrap(),
            );
        }
        fixture
    }

    fn ticket(&self, state: TicketWorkflowState) -> String {
        let mut input =
            ticket::NewTicket::new(format!("Explicit Worker launch from {}", state.as_str()));
        input.workflow_state = Some(state);
        browser_ticket_backend(&self.api)
            .unwrap()
            .create(input)
            .unwrap()
            .id
    }

    fn payload(&self, ticket_id: &str, operation_id: &str, profile: &str, flow: bool) -> Value {
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
            "ticket_assignment": {"ticket_id": ticket_id, "operation_id": operation_id},
            "initial_submit": segments, "workdir_attachments": [], "feature_connections": {},
        })
    }

    async fn post(&self, workspace_id: &str, payload: Value) -> (StatusCode, Value) {
        let response = build_inner_router(self.api.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{workspace_id}/workers"))
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
            self.api.authority.ticket(ticket_id).unwrap().state,
            "inprogress"
        );
        current
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn embedded_explicit_worker_starts_each_active_state_without_targets_flow_or_orchestrator() {
    let fixture = LaunchFixture::new(false).await;
    for state in [
        TicketWorkflowState::Planning,
        TicketWorkflowState::Ready,
        TicketWorkflowState::Queued,
        TicketWorkflowState::InProgress,
    ] {
        let ticket_id = fixture.ticket(state);
        let operation_id = format!("explicit-{}", state.as_str());
        assert!(
            fixture
                .api
                .store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket_id,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap()
                .is_none()
        );
        let payload = fixture.payload(&ticket_id, &operation_id, "builtin:companion", false);
        let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", state.as_str());
        let assignment = fixture.assert_assigned(&ticket_id, &operation_id, &body);
        assert_eq!(body["worker"]["profile"], "builtin:companion", "{body}");
        let creates = fixture.backend.creates_for(&assignment.worker.worker_id);
        assert_eq!(creates.len(), 1);
        assert!(creates[0].workdir_attachments.is_empty());
        let submits = fixture.backend.submits_for(&assignment.worker.worker_id);
        assert_eq!(submits.len(), 1);
        assert_eq!(
            serde_json::to_value(&submits[0].segments).unwrap(),
            json!([{"kind": "text", "content": "Research this Ticket without changing repositories."}])
        );
        assert!(submits[0].submission_request_id.is_some());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn real_runtime_http_worker_launch_preserves_optional_flow_profile_submit_and_assignment() {
    let fixture = LaunchFixture::new(true).await;
    for (profile, flow) in [("builtin:intake", false), ("builtin:coder", true)] {
        let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
        let operation_id = format!("real-http-{flow}");
        let payload = fixture.payload(&ticket_id, &operation_id, profile, flow);
        let expected_submit = payload["initial_submit"].clone();
        let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
        assert_eq!(status, StatusCode::OK, "{profile}: {body}");
        fixture.assert_assigned(&ticket_id, &operation_id, &body);
        assert_eq!(body["worker"]["profile"], profile, "{body}");
        let creates = fixture
            .backend
            .creates_for(body["worker_id"].as_str().unwrap());
        assert_eq!(creates.len(), 1);
        let create = &creates[0];
        assert_eq!(
            serde_json::to_value(&create.initial_input.as_ref().unwrap().segments).unwrap(),
            expected_submit
        );
        assert!(create.workdir_attachments.is_empty());
        assert_eq!(
            create.workspace_api.as_ref().unwrap().workspace_id,
            TEST_WORKSPACE_ID
        );
        let submits = fixture
            .backend
            .submits_for(body["worker_id"].as_str().unwrap());
        assert_eq!(submits.len(), 1);
        assert_eq!(
            serde_json::to_value(&submits[0].segments).unwrap(),
            expected_submit
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_launch_uses_published_project_profile_and_workspace_default_on_real_runtime_http() {
    let fixture = LaunchFixture::new(true).await;
    let state = fixture
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = config_source::VirtualPath::parse("main.dcdl").unwrap();
    let request = crate::config_source::ConfigCommitRequest {
        base_revision: state.snapshot.revision,
        base_digest: state.snapshot.digest.clone(),
        entrypoints: state.contract.entrypoints.clone(),
        changes: vec![config_source::ConfigTreeChange::Update {
            path: main.clone(),
            expected_digest: state.snapshot.entries[&main].content_digest.clone(),
            content: r#"{
                profile = {
                    default_profile = "project:research";
                    entries = [{ selector = "project:research"; label = "Research";
                        description = "Ticket research recipe";
                        profile = import "$builtin/profiles/intake.dcdl";
                    }];
                };
            }"#
            .into(),
        }],
    };
    let candidate = fixture
        .api
        .config_store
        .evaluate_workspace_config_candidate_with_schema(
            TEST_WORKSPACE_ID,
            &request,
            fixture.api.config_schema_registry.compose().unwrap(),
        )
        .unwrap();
    fixture
        .api
        .config_store
        .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
        .unwrap();
    for explicit in [true, false] {
        let ticket_id = fixture.ticket(TicketWorkflowState::Ready);
        let operation_id = format!("project-profile-{explicit}");
        let mut payload = fixture.payload(&ticket_id, &operation_id, "project:research", false);
        if !explicit {
            payload.as_object_mut().unwrap().remove("profile");
        }
        let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        fixture.assert_assigned(&ticket_id, &operation_id, &body);
        assert_eq!(body["worker"]["profile"], "project:research", "{body}");
        let creates = fixture
            .backend
            .creates_for(body["worker_id"].as_str().unwrap());
        assert_eq!(creates.len(), 1);
        assert_eq!(
            creates[0].profile,
            ProfileSelector::Named("project:research".into())
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_or_closed_ticket_launch_rejects_before_runtime_or_assignment_reservation() {
    let fixture = LaunchFixture::new(false).await;
    for state in [TicketWorkflowState::Done, TicketWorkflowState::Closed] {
        let ticket_id = fixture.ticket(state);
        let operation_id = format!("terminal-{}", state.as_str());
        let payload = fixture.payload(&ticket_id, &operation_id, "builtin:companion", false);
        let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(
            body["message"].as_str().unwrap().contains(state.as_str()),
            "{body}"
        );
        assert_eq!(
            fixture.api.authority.ticket(&ticket_id).unwrap().state,
            state.as_str()
        );
        assert!(
            fixture
                .api
                .store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket_id)
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .api
                .store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &operation_id)
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.backend.creates.lock().unwrap().is_empty());
    assert!(fixture.backend.submits.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_exact_launch_replays_keep_one_worker_and_committed_assignment() {
    let fixture = Arc::new(LaunchFixture::new(false).await);
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    let payload = fixture.payload(&ticket_id, "concurrent-launch", "builtin:companion", false);
    let barrier = Arc::new(tokio::sync::Barrier::new(5));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let fixture = fixture.clone();
        let payload = payload.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            fixture.post(TEST_WORKSPACE_ID, payload).await
        }));
    }
    barrier.wait().await;
    let mut assigned = None;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        let current = fixture.assert_assigned(&ticket_id, "concurrent-launch", &body);
        if let Some(expected) = &assigned {
            assert_eq!(&current, expected);
        }
        assigned = Some(current);
    }
    let assigned = assigned.unwrap();
    assert_eq!(
        fixture
            .backend
            .creates_for(&assigned.worker.worker_id)
            .len(),
        1
    );
    assert_eq!(
        fixture
            .backend
            .submits_for(&assigned.worker.worker_id)
            .len(),
        1
    );
    assert!(fixture.backend.execution.stops.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn undispatched_launch_rejection_settles_claim_and_allows_corrected_operation() {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    let mut rejected = fixture.payload(&ticket_id, "invalid-resources", "builtin:companion", false);
    rejected["workdir_attachments"] =
        json!([{"alias":"checkout","working_directory_id":"missing-workdir"}]);
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, rejected.clone()).await;
    assert!(!status.is_success(), "{body}");
    let receipt = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "invalid-resources")
        .unwrap()
        .unwrap();
    assert_eq!(receipt.claim_state, "failed");
    assert!(fixture.backend.creates.lock().unwrap().is_empty());
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, rejected).await;
    assert!(
        !status.is_success(),
        "failed receipt cannot restart: {body}"
    );
    let corrected = fixture.payload(
        &ticket_id,
        "corrected-resources",
        "builtin:companion",
        false,
    );
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, corrected).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.assert_assigned(&ticket_id, "corrected-resources", &body);
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_launch_retry_reuses_assignment_and_does_not_repeat_initial_submit() {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Ready);
    let payload = fixture.payload(&ticket_id, "worker-retry", "builtin:companion", false);
    let (status, first) = fixture.post(TEST_WORKSPACE_ID, payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let assignment = fixture.assert_assigned(&ticket_id, "worker-retry", &first);
    let (status, replay) = fixture.post(TEST_WORKSPACE_ID, payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["worker_id"], first["worker_id"]);
    assert_eq!(
        fixture
            .assert_assigned(&ticket_id, "worker-retry", &replay)
            .assignment_id,
        assignment.assignment_id
    );
    let mut attachment_conflict = payload.clone();
    attachment_conflict["workdir_attachments"] = json!([{"alias":"changed", "working_directory_id":"different-workdir", "relative_cwd":"docs"}]);
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, attachment_conflict).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "changed attachment intent: {body}"
    );
    let mut conflict = payload;
    conflict["initial_submit"][0]["content"] =
        json!("A different operation payload must not reuse the reservation.");
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, conflict).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{body}"
    );
    assert_eq!(
        fixture
            .backend
            .creates_for(&assignment.worker.worker_id)
            .len(),
        1
    );
    assert_eq!(
        fixture
            .backend
            .submits_for(&assignment.worker.worker_id)
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn generic_ticket_worker_unknown_input_outcome_preserves_assignment_and_launch_retry_identity()
 {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    let payload = fixture.payload(
        &ticket_id,
        "worker-unknown-input",
        "builtin:companion",
        false,
    );
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let assignment = fixture.assert_assigned(&ticket_id, "worker-unknown-input", &body);
    fixture
        .backend
        .execution
        .reject_inputs("acceptance response unavailable");
    let worker = WorkspaceWorker::resolve(
        &fixture.api,
        &assignment.worker.runtime_id,
        &assignment.worker.worker_id,
    )
    .unwrap();
    let result = worker
        .input(
            &WorkerOperationContext::Backend,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: "Continue Ticket research".into(),
                submission_request_id: Some("unknown-worker-submit".into()),
                segments: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result.disposition,
        WorkerInputDisposition::Unknown,
        "{result:?}"
    );
    assert!(fixture.backend.execution.stops.lock().unwrap().is_empty());
    let (status, replay) = fixture.post(TEST_WORKSPACE_ID, payload).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    let current = fixture.assert_assigned(&ticket_id, "worker-unknown-input", &replay);
    assert_eq!(current.assignment_id, assignment.assignment_id);
    assert_eq!(current.worker, assignment.worker);
    assert_eq!(
        fixture
            .backend
            .creates_for(&assignment.worker.worker_id)
            .len(),
        1
    );
    assert_eq!(
        fixture
            .backend
            .submits_for(&assignment.worker.worker_id)
            .len(),
        2,
        "initial Submit and one outcome-unknown input; replay must not re-submit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_worker_launch_rejects_foreign_workspace_and_unpublished_profile_with_body() {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    let payload = fixture.payload(&ticket_id, "scope-rejected", "builtin:companion", false);
    let (status, body) = fixture
        .post("019d0000-0000-7000-8000-0000000000bb", payload.clone())
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{body}"
    );
    let mut unknown = payload;
    unknown["profile"] = json!("project:not-published");
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, unknown).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Backend-published"),
        "{body}"
    );
    assert!(fixture.backend.creates.lock().unwrap().is_empty());
    assert!(
        fixture
            .api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "scope-rejected")
            .unwrap()
            .is_none()
    );
}

// This is the capability policy boundary. The API/runtime tests above separately
// prove transport, creation, Submit acknowledgement, and durable assignment.
#[tokio::test]
async fn read_only_ticket_targets_allow_extra_external_grants_without_widening_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let mut input =
        ticket::NewTicket::new("Read-only Ticket with explicit external research grant");
    input.targets = vec![test_ticket_target(
        "test-repository",
        "develop",
        TicketTargetAccess::ReadOnly,
    )];
    let ticket_id = browser_ticket_backend(&api)
        .unwrap()
        .create(input)
        .unwrap()
        .id;
    let workdir = WorkdirRegistryRecord {
        workspace_id: TEST_WORKSPACE_ID.into(),
        workdir_id: "read-only-target".into(),
        display_name: None,
        source: WorkdirRegistrySource::Repository {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            repository_id: test_repository_id(&api),
        },
        creation_selector: Some("develop".into()),
        creation_ref: None,
        creation_tree: None,
        current_selector: Some("develop".into()),
        current_ref: None,
        current_tree: None,
        observed_at_epoch_seconds: None,
        materialization_status: "present".into(),
        cleanliness: "clean".into(),
        created_at: TEST_CREATED_AT.into(),
        updated_at: TEST_CREATED_AT.into(),
    };
    api.store.upsert_workdir_registry(&workdir).unwrap();
    let external = WorkdirRegistryRecord {
        workdir_id: "external-research".into(),
        source: WorkdirRegistrySource::ExternalGrant {
            grant_id: "research-grant".into(),
        },
        creation_selector: None,
        current_selector: None,
        ..workdir.clone()
    };
    api.store
        .create_external_workdir_grant(
            &ExternalWorkdirGrantRecord {
                grant_id: "research-grant".into(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                workdir_id: external.workdir_id.clone(),
                provider_instance_id: "research-provider".into(),
                display_name: "Research".into(),
                permissions: "read_only".into(),
                created_by: "fixture".into(),
                created_at: TEST_CREATED_AT.into(),
                expires_at: None,
                generation: 1,
                status: "pending".into(),
                updated_at: TEST_CREATED_AT.into(),
            },
            &external,
        )
        .unwrap();
    let mut request = WorkerSpawnRequest {
        requested_worker_name: Some("Read-only research Worker".into()),
        intent: WorkerSpawnIntent::TicketRole {
            ticket_id,
            role: TicketWorkerRole::Worker,
        },
        singleton_key: None,
        acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
            expected_segments: 1,
        },
        profile: ProfileSelector::Builtin("builtin:intake".into()),
        ticket_assignment: None,
        initial_submit: vec![Segment::text("Research without writes")],
        workdir_attachment_requests: Vec::new(),
        resolved_workdir_attachment_requests: Vec::new(),
        resolved_workdir_attachments: vec![
            WorkingDirectoryAttachmentClaim {
                alias: workdir::WorkdirAttachmentAlias::new("target").unwrap(),
                working_directory_id: workdir.workdir_id.clone(),
                relative_cwd: None,
                capabilities: workdir::WorkdirSessionCapabilities::ALL,
            },
            WorkingDirectoryAttachmentClaim {
                alias: workdir::WorkdirAttachmentAlias::new("research").unwrap(),
                working_directory_id: external.workdir_id.clone(),
                relative_cwd: None,
                capabilities: workdir::WorkdirSessionCapabilities::ALL,
            },
        ],
        resolved_config_bundle: None,
        resolved_worker_observation_enabled: false,
        resolved_worker_observation_grants: Vec::new(),
        resolved_workspace_api: None,
        resolved_memory_settings: None,
        resolved_subjektiv_attached: false,
        resolved_control_operation: None,
    };
    api.validate_worker_spawn_repository_scope(EMBEDDED_WORKER_RUNTIME_ID, &mut request)
        .unwrap();
    assert!(
        request
            .resolved_workdir_attachments
            .iter()
            .all(|claim| claim.capabilities == workdir::WorkdirSessionCapabilities::READ_ONLY)
    );
    // Repository targets bound allowable scope, not mandatory resources.
    request
        .resolved_workdir_attachments
        .retain(|claim| claim.alias.as_str() == "research");
    api.validate_worker_spawn_repository_scope(EMBEDDED_WORKER_RUNTIME_ID, &mut request)
        .unwrap();
    assert_eq!(
        request.resolved_workdir_attachments[0].capabilities,
        workdir::WorkdirSessionCapabilities::READ_ONLY
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_assignment_finalize_failure_compensates_real_embedded_runtime_and_preserves_planning()
 {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    rusqlite::Connection::open(&fixture.api.config.database_path)
        .unwrap()
        .execute_batch(
            r#"
        CREATE TRIGGER fail_generic_worker_assignment
        BEFORE INSERT ON ticket_current_worker_assignments
        WHEN NEW.role = 'worker'
        BEGIN SELECT RAISE(ABORT, 'injected Worker assignment failure'); END;
    "#,
        )
        .unwrap();
    let payload = fixture.payload(&ticket_id, "compensated-worker", "builtin:companion", false);
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("compensat"),
        "{body}"
    );
    let creates = fixture
        .backend
        .creates
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.display_name.as_deref() == Some("Explicit Ticket Worker"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 1);
    let worker_id = creates[0].worker_id.to_string();
    assert!(
        fixture
            .backend
            .execution
            .stops
            .lock()
            .unwrap()
            .iter()
            .any(|worker| worker.worker_id.to_string() == worker_id)
    );
    assert!(
        fixture
            .backend
            .execution
            .contexts
            .lock()
            .unwrap()
            .keys()
            .all(|worker| worker.worker_id.to_string() != worker_id)
    );
    assert!(
        fixture
            .api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture.api.authority.ticket(&ticket_id).unwrap().state,
        "planning"
    );
    let counts: (i64, i64) = fixture.api.config_store.with_conn(|conn| Ok((
        conn.query_row("SELECT COUNT(*) FROM worker_registry WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3", rusqlite::params![TEST_WORKSPACE_ID, fixture.runtime_id, worker_id], |row| row.get(0))?,
        conn.query_row("SELECT COUNT(*) FROM worker_create_reservations WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND state = 'reserved'", rusqlite::params![TEST_WORKSPACE_ID, fixture.runtime_id, worker_id], |row| row.get(0))?,
    ))).unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn choosing_reviewer_profile_does_not_register_a_reviewer_child_or_grant_mr_review() {
    let fixture = LaunchFixture::new(false).await;
    let ticket_id = fixture.ticket(TicketWorkflowState::Planning);
    let payload = fixture.payload(&ticket_id, "ordinary-reviewer", "builtin:reviewer", false);
    let (status, body) = fixture.post(TEST_WORKSPACE_ID, payload).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let current = fixture.assert_assigned(&ticket_id, "ordinary-reviewer", &body);
    assert_eq!(body["worker"]["profile"], "builtin:reviewer", "{body}");
    let store = merge_request_store(&fixture.api, TEST_WORKSPACE_ID).unwrap();
    let auth = merge_request::MergeRequestAuth {
        workspace_id: TEST_WORKSPACE_ID.into(),
        repository_id: test_repository_id(&fixture.api),
        runtime_id: current.worker.runtime_id.clone(),
        worker_id: current.worker.worker_id.clone(),
        assignment_id: current.assignment_id,
    };
    let mr_id = "ordinary-profile-is-not-review-authority";
    store
        .open_merge_request(merge_request::OpenMergeRequest {
            merge_request_id: mr_id.into(),
            ticket_id: ticket_id.clone(),
            repository_id: auth.repository_id.clone(),
            selector_from: "work/research".into(),
            selector_to: "develop".into(),
            summary: String::new(),
            auth: auth.clone(),
            now: Utc::now(),
        })
        .unwrap();
    let error = store
        .request_review(merge_request::RequestMergeRequestReview {
            merge_request_id: mr_id.into(),
            ticket_id: ticket_id.clone(),
            ticket_item_revision: ticket_item_checker::item_revision(
                &browser_ticket_backend(&fixture.api)
                    .unwrap()
                    .show(ticket_id.clone().into())
                    .unwrap(),
            ),
            ticket_merge_request_subjects: vec![merge_request::MergeRequestReviewSubject {
                merge_request_id: mr_id.into(),
                subject_ref: "immutable-source".into(),
            }],
            subject_ref: "immutable-source".into(),
            child_session_id: current.worker.worker_id,
            capability_token: "profile-alone-is-not-a-capability".into(),
            auth,
            now: Utc::now(),
        })
        .unwrap_err();
    assert!(
        matches!(error, merge_request::MergeRequestError::Unauthorized(ref message) if message == "reviewer child attestation missing"),
        "{error}"
    );
    assert!(
        store
            .authorize_review_submission(mr_id, "profile-alone-is-not-a-capability")
            .is_err()
    );
}

// Narrow provider fault seam: creation succeeds and the summary is then lost,
// while the delete default cannot confirm compensation. No product process runs.
struct LostSpawnResponseRuntime {
    inner: WorkdirlessFixtureRuntime,
}

impl crate::hosts::WorkspaceWorkerRuntime for LostSpawnResponseRuntime {
    fn runtime_id(&self) -> &str {
        self.inner.runtime_id()
    }
    fn runtime_summary(&self, limit: usize) -> InternalRuntimeSummary {
        self.inner.runtime_summary(limit)
    }
    fn list_hosts(
        &self,
        limit: usize,
    ) -> crate::hosts::RuntimeList<crate::hosts::InternalHostSummary> {
        self.inner.list_hosts(limit)
    }
    fn list_workers(&self, limit: usize) -> crate::hosts::RuntimeList<InternalWorkerSummary> {
        self.inner.list_workers(limit)
    }
    fn worker(&self, worker_id: &str) -> crate::hosts::WorkerLookupResult {
        self.inner.worker(worker_id)
    }
    fn working_directory(&self, id: &str) -> crate::hosts::RuntimeWorkingDirectoryResult {
        self.inner.working_directory(id)
    }
    fn spawn_worker(
        &self,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        let result = self.inner.spawn_worker(binding, request);
        assert_eq!(result.state, InternalWorkerOperationState::Accepted);
        assert!(result.worker.is_some());
        WorkerSpawnResult {
            state: InternalWorkerOperationState::Rejected,
            worker: None,
            acceptance_evidence: Vec::new(),
            diagnostics: vec![RuntimeDiagnostic::new(
                "spawn_response_lost",
                "error",
                "Created Worker but lost the response",
            )],
        }
    }
}

#[tokio::test]
async fn uncertain_launch_replay_after_target_edit_keeps_claim_and_blocks_competing_start() {
    let fixture = manual_worker_assignment_fixture().await;
    for id in [&fixture.main_workdir_id, &fixture.docs_workdir_id] {
        fixture
            .api
            .store
            .detach_worker_workdir(
                TEST_WORKSPACE_ID,
                &fixture.worker,
                Some(id),
                TEST_CREATED_AT,
            )
            .unwrap();
    }
    sync_runtime_worker_workdir_attachments(&fixture.api, &fixture.worker).unwrap();
    fixture
        .api
        .runtime
        .register_or_replace(LostSpawnResponseRuntime {
            inner: fixture.runtime.clone(),
        });
    let initial_spawns = fixture.runtime.spawn_requests().len();
    let payload = json!({
        "runtime_id": fixture.worker.runtime_id,
        "display_name": "Uncertain Ticket Worker", "profile": "builtin:companion",
        "ticket_assignment": {"ticket_id": fixture.ticket_id, "operation_id": "lost-spawn"},
        "initial_submit": [{"kind":"text", "content":"Research the Ticket"}],
        "workdir_attachments": [
            {"alias":"checkout", "working_directory_id":fixture.main_workdir_id},
            {"alias":"docs", "working_directory_id":fixture.docs_workdir_id}
        ], "feature_connections": {}
    });
    let launch = |body: Value| {
        create_workspace_worker(
            State(fixture.api.clone()),
            HeaderMap::new(),
            Json(serde_json::from_value(body).unwrap()),
        )
    };
    let lost = launch(payload.clone()).await.unwrap_err();
    assert!(
        lost.diagnostics
            .iter()
            .any(|d| d.code == "worker_spawn_compensation_create_reservation_retained"),
        "{lost:?}"
    );
    let receipt = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "lost-spawn")
        .unwrap()
        .unwrap();
    assert_eq!(receipt.claim_state, "pending");
    assert!(receipt.assignment_id.is_none());
    let original_worker = receipt.worker.clone().unwrap();
    assert!(
        fixture
            .runtime
            .worker(&original_worker.worker_id)
            .worker
            .is_some()
    );
    assert_eq!(fixture.runtime.spawn_requests().len(), initial_spawns + 1);

    browser_ticket_backend(&fixture.api)
        .unwrap()
        .edit_item(
            fixture.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Set {
                    targets: vec![
                        test_ticket_target(
                            "test-repository",
                            "changed-selector",
                            TicketTargetAccess::ReadWrite,
                        ),
                        test_ticket_target("docs", "develop", TicketTargetAccess::ReadOnly),
                    ],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let rejected = launch(payload.clone()).await.unwrap_err();
    assert!(
        rejected.error.to_string().contains("selector"),
        "{rejected:?}"
    );
    let retained = fixture
        .api
        .store
        .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "lost-spawn")
        .unwrap()
        .unwrap();
    assert_eq!(retained.claim_state, "pending");
    assert_eq!(retained.worker, Some(original_worker.clone()));
    assert_eq!(retained.request_fingerprint, receipt.request_fingerprint);
    assert!(retained.failure_reason.is_none());
    let mut competing = payload;
    competing["ticket_assignment"]["operation_id"] = json!("competing-spawn");
    let conflict = launch(competing).await.unwrap_err();
    assert!(
        conflict.error.to_string().contains("pending"),
        "{conflict:?}"
    );
    assert!(
        fixture
            .api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "competing-spawn")
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.runtime.spawn_requests().len(), initial_spawns + 1);
    assert!(
        fixture
            .runtime
            .worker(&original_worker.worker_id)
            .worker
            .is_some()
    );
    assert_eq!(
        fixture
            .api
            .authority
            .ticket(&fixture.ticket_id)
            .unwrap()
            .state,
        "ready"
    );
}
