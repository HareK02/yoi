//! Normal Server library boundary tests (`subject_authorization_tests` filter).
//! Router authentication, Host creation/reservation, Subject policy storage, Worker
//! Feature installation and Ticket SQLite writes are real. The original boundary
//! cases substitute WorkdirlessFixtureRuntime. The connected regression below also
//! runs a real ProfileRuntimeWorkerFactory/Controller and materialized Workdir;
//! only its external model stream is scripted. Neither fixture claims OS isolation.
use super::*;
use agen::tool::{Tool, ToolExecutionContext, ToolOutput};
use server_api::authorization::{
    PermissionResource, ScopedPermission, SubjectPermissions, SubjectPermissionsUpdate,
};
use std::sync::Mutex;
use ticket::NewTicket;
use worker::feature::builtin::ticket::{TicketFeature, TicketFeatureAccess};
use worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};

fn activation(name: &str) -> ScopedPermission {
    ScopedPermission {
        operation: name.into(),
        resource: PermissionResource::Feature,
    }
}
fn create_permission() -> ScopedPermission {
    ScopedPermission {
        operation: "ticket.create".into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}
fn authorization_path() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/authorization/check")
}
async fn response(api: &WorkspaceApi, request: Request<Body>) -> (StatusCode, Value) {
    let response = build_router(api.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}));
    (status, body)
}
fn human_request(token: &str, method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .unwrap()
}

struct Fixture {
    api: WorkspaceApi,
    identity: Arc<RuntimeIdentityMaterial>,
    runtime: WorkdirlessFixtureRuntime,
    subject: String,
    source: RuntimeWorkerRef,
    owner: String,
    _dir: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        let runtime = WorkdirlessFixtureRuntime::default();
        api.runtime.register_or_replace(runtime.clone());
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("authorization-boundary").unwrap())
            .unwrap()
            .id;
        // A normal body attachment, not a forged registry/singleton row. No
        // Workdir or Ticket exists. The profile deliberately requests only reads;
        // its enabled tool set must not become a ceiling on explicit delegation.
        install_read_only_subject_profile(&api);
        let Json(created) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                display_name: "Read-only Subject body".into(),
                singleton_key: None,
                profile: Some("project:authority-source".into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: subject.clone(),
                    }),
                },
                control_operation_id: None,
                execution_grant: None,
            }),
        )
        .await
        .unwrap();
        let source = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
        let identity = Arc::new(RuntimeIdentityMaterial::generate(&source.runtime_id).unwrap());
        configure_runtime_request_auth(&mut api, &identity, &source.runtime_id);
        // The execution fixture does not bootstrap a Session. Exercise the real
        // signed startup callback explicitly so the policy store sees the active
        // Journal writer, just as it does after normal Runtime startup.
        let (status, body) = response(&api, runtime_source_request(&identity, Some(&source.worker_id), "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions"),
            serde_json::to_vec(&json!({"session_id":session_store::new_session_id().to_string(),"create_if_missing":true})).unwrap())).await;
        assert_eq!(status, StatusCode::OK, "source startup callback: {body}");
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "subject-authority-owner").await;
        assert!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &source)
                .unwrap()
                .is_empty()
        );
        assert!(
            api.store
                .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &source)
                .unwrap()
                .is_some()
        );
        Self {
            api,
            identity,
            runtime,
            subject,
            source,
            owner,
            _dir: dir,
        }
    }
    fn policy_path(&self) -> String {
        format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions",
            self.subject
        )
    }
    async fn assign(
        &self,
        execution: Vec<ScopedPermission>,
        delegation: Vec<ScopedPermission>,
    ) -> SubjectPermissions {
        let previous = open_subjektiv_store(&self.api)
            .unwrap()
            .subject_permissions(&self.subject)
            .unwrap();
        let update = SubjectPermissionsUpdate {
            operation_id: Uuid::now_v7().to_string(),
            expected_change_id: previous.map(|policy| policy.change_id),
            execution,
            delegation,
        };
        let (status, body) = response(
            &self.api,
            human_request(
                &self.owner,
                "POST",
                &self.policy_path(),
                Some(serde_json::to_value(update).unwrap()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let mutation: server_api::SubjectPermissionsMutationResponse =
            serde_json::from_value(body).unwrap();
        assert!(
            mutation
                .interruptions
                .iter()
                .all(|evidence| evidence.backend_command_sessions_closed),
            "Backend cleanup evidence is independent of Runtime cancellation acceptance: {:?}",
            mutation.interruptions
        );
        mutation.permissions
    }
    async fn check(
        &self,
        worker: &RuntimeWorkerRef,
        permission: &ScopedPermission,
    ) -> (StatusCode, Value) {
        response(
            &self.api,
            runtime_source_request(
                &self.identity,
                Some(&worker.worker_id),
                "POST",
                &authorization_path(),
                serde_json::to_vec(permission).unwrap(),
            ),
        )
        .await
    }
    fn client(&self, worker: &RuntimeWorkerRef) -> Arc<RouterClient> {
        Arc::new(RouterClient {
            api: self.api.clone(),
            identity: self.identity.clone(),
            worker: worker.clone(),
            handle: tokio::runtime::Handle::current(),
            requests: Mutex::new(vec![]),
        })
    }
    fn child_request(&self, operation: &str, permissions: Vec<ScopedPermission>) -> Value {
        json!({"runtime_id":self.source.runtime_id,"display_name":"Ordinary delegated editor",
            "profile":"builtin:coder","initial_submit":[],"workdir_attachments":[],"feature_connections":{},
            "control_operation_id":operation,"execution_grant":{"permissions":permissions,"redelegation":[]}})
    }
    async fn spawn(&self, source: &RuntimeWorkerRef, body: Value) -> (StatusCode, Value) {
        response(
            &self.api,
            runtime_source_request(
                &self.identity,
                Some(&source.worker_id),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
                serde_json::to_vec(&body).unwrap(),
            ),
        )
        .await
    }
    async fn child(&self, operation: &str, permissions: Vec<ScopedPermission>) -> RuntimeWorkerRef {
        let (status, body) = self
            .spawn(&self.source, self.child_request(operation, permissions))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        RuntimeWorkerRef::new(
            body["runtime_id"].as_str().unwrap(),
            body["worker_id"].as_str().unwrap(),
        )
    }
    // Historical disconnection is fixture input, not evidence that this suite
    // stopped a running Subject controller. Production lease release is real.
    fn release_source_lease(&self) {
        let lease = self
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &self.source)
            .unwrap()
            .unwrap();
        let binding = "fixture-disconnected-writer";
        let operation = "fixture-disconnected-source";
        self.api
            .config_store
            .reserve_subject_release(TEST_WORKSPACE_ID, &lease, binding, operation)
            .unwrap();
        self.api
            .config_store
            .confirm_subject_release_proof(TEST_WORKSPACE_ID, &lease, binding, operation, false)
            .unwrap();
        self.api
            .config_store
            .release_subject_singleton(TEST_WORKSPACE_ID, &lease, binding, operation)
            .unwrap();
    }
}

fn install_read_only_subject_profile(api: &WorkspaceApi) {
    install_subject_profiles(api, "workspace_read");
}
fn install_subject_profiles(api: &WorkspaceApi, editor_scope: &str) {
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let content = r#"{ profile = { default_profile = "project:authority-source"; entries = [{ selector = "project:authority-source"; label = "Read-only Subject"; description = "Boundary fixture";
        profile = (import "$builtin/profiles/companion.dcdl") // { slug = "authority-source"; scope = "workspace_read"; delegation_scope = "workspace_read"; };
    }, { selector = "project:authority-editor"; label = "Editor"; description = "Mutable Profile boundary fixture";
        profile = (import "$builtin/profiles/coder.dcdl") // { slug = "authority-editor"; scope = "EDITOR_SCOPE"; };
    }]; }; }"#.replace("EDITOR_SCOPE", editor_scope);
    commit_workspace_config_tree(
        api,
        TEST_WORKSPACE_ID,
        &ConfigCommitRequest {
            base_digest: state.snapshot.digest.clone(),
            entrypoints: state.contract.entrypoints.clone(),
            changes: vec![config_source::ConfigTreeChange::Update {
                path: main.path.clone(),
                expected_digest: main.content_digest.clone(),
                content: content.into(),
            }],
        },
    )
    .unwrap();
}

/// Transport only: a fresh real signed proof passes the production router on
/// every call. The request log observes whether a denied Tool/WIP operation
/// crossed the Ticket backend HTTP boundary, rather than inferring from no row.
struct RouterClient {
    api: WorkspaceApi,
    identity: Arc<RuntimeIdentityMaterial>,
    worker: RuntimeWorkerRef,
    handle: tokio::runtime::Handle,
    requests: Mutex<Vec<(String, String)>>,
}
impl std::fmt::Debug for RouterClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedAuthorizationRouterClient")
            .field("worker", &self.worker)
            .finish_non_exhaustive()
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
    ) -> std::result::Result<WorkspaceResponse, WorkspaceClientError> {
        let method = match request.method {
            WorkspaceRequestMethod::Get => "GET",
            WorkspaceRequestMethod::Post => "POST",
            WorkspaceRequestMethod::Put => "PUT",
            WorkspaceRequestMethod::Patch => "PATCH",
            WorkspaceRequestMethod::Delete => "DELETE",
        };
        self.requests
            .lock()
            .unwrap()
            .push((method.into(), request.path.clone()));
        let request = runtime_source_request(
            &self.identity,
            Some(&self.worker.worker_id),
            method,
            &request.path,
            request.body.unwrap_or_default().into_bytes(),
        );
        self.handle.block_on(async {
            let response = build_router(self.api.clone())
                .oneshot(request)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            let status = response.status().as_u16();
            let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            Ok(WorkspaceResponse {
                status,
                body: String::from_utf8(bytes.to_vec())
                    .map_err(|e| WorkspaceClientError::Request(e.to_string()))?,
            })
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_current_body_uses_explicit_policy_and_old_unset_body_denies() {
    let f = Fixture::new().await;
    assert!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap()
            .is_none()
    );
    for permission in [activation("ticket"), create_permission()] {
        let (status, body) = f.check(&f.source, &permission).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "unset policy: {body}");
    }
    f.assign(vec![activation("ticket"), create_permission()], vec![])
        .await;
    for permission in [activation("ticket"), create_permission()] {
        let (status, body) = f.check(&f.source, &permission).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"scope":permission.resource}),
            "authorization exposes only effective resource scope, not identities or policy/binding material"
        );
    }
    for permission in [
        activation("web"),
        ScopedPermission {
            operation: "ticket.create".into(),
            resource: PermissionResource::Workspace {
                workspace_id: "another-workspace".into(),
            },
        },
    ] {
        assert_eq!(
            f.check(&f.source, &permission).await.0,
            StatusCode::FORBIDDEN
        );
    }
    // A non-Subject Worker keeps the preexisting contract, but cannot nominate a Subject.
    let Json(legacy) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Unattached legacy Worker".into(),
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
    let legacy = RuntimeWorkerRef::new(legacy.runtime_id, legacy.worker_id);
    let (status, body) = f.check(&legacy, &activation("ticket")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"scope":null}),
        "legacy response carries no authority identities"
    );
    let (status, _) = f
        .spawn(
            &legacy,
            f.child_request("legacy-cannot-issue", vec![activation("ticket")]),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorization_rejects_missing_source_unknown_identity_fields_and_tampered_signed_body() {
    let f = Fixture::new().await;
    f.assign(vec![activation("ticket")], vec![]).await;
    let body = serde_json::to_value(activation("ticket")).unwrap();
    let (status, error) = response(
        &f.api,
        human_request(&f.owner, "POST", &authorization_path(), Some(body.clone())),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "human actor is not a Worker source: {error}"
    );
    let (status, error) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            None,
            "POST",
            &authorization_path(),
            serde_json::to_vec(&body).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Runtime without Worker: {error}"
    );
    let unsigned = Request::builder()
        .method("POST")
        .uri(authorization_path())
        .header(CONTENT_TYPE, "application/json")
        .header("x-yoi-runtime-id", &f.source.runtime_id)
        .header("x-yoi-worker-id", &f.source.worker_id)
        .body(Body::from(body.to_string()))
        .unwrap();
    assert_eq!(response(&f.api, unsigned).await.0, StatusCode::UNAUTHORIZED);
    let mut hinted = runtime_source_request(
        &f.identity,
        Some(&f.source.worker_id),
        "POST",
        &authorization_path(),
        serde_json::to_vec(&activation("web")).unwrap(),
    );
    hinted
        .headers_mut()
        .insert("x-yoi-runtime-id", "unattached-runtime".parse().unwrap());
    hinted
        .headers_mut()
        .insert("x-yoi-worker-id", "unattached-worker".parse().unwrap());
    assert_eq!(
        response(&f.api, hinted).await.0,
        StatusCode::FORBIDDEN,
        "hints cannot change the verified source into an unrestricted legacy Worker"
    );
    for field in ["subject_id", "modelSubject", "runtime_id", "worker_id"] {
        let mut claimed = body.clone();
        claimed[field] = json!("self-claimed-identity");
        let (status, error) = response(
            &f.api,
            runtime_source_request(
                &f.identity,
                Some(&f.source.worker_id),
                "POST",
                &authorization_path(),
                serde_json::to_vec(&claimed).unwrap(),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{field}: {status} {error}");
    }
    let mut tampered = runtime_source_request(
        &f.identity,
        Some(&f.source.worker_id),
        "POST",
        &authorization_path(),
        serde_json::to_vec(&body).unwrap(),
    );
    *tampered.body_mut() = Body::from(serde_json::to_vec(&activation("web")).unwrap());
    assert_eq!(response(&f.api, tampered).await.0, StatusCode::UNAUTHORIZED);
    let forged_identity = RuntimeIdentityMaterial::generate(&f.source.runtime_id).unwrap();
    assert_eq!(
        response(
            &f.api,
            runtime_source_request(
                &forged_identity,
                Some(&f.source.worker_id),
                "POST",
                &authorization_path(),
                serde_json::to_vec(&body).unwrap()
            )
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_owner_account_can_read_assign_shrink_and_revoke_subject_permissions() {
    let f = Fixture::new().await;
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "GET", &f.policy_path(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["permissions"].is_null());
    let original = f
        .assign(
            vec![activation("ticket"), create_permission()],
            vec![activation("ticket")],
        )
        .await;
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "GET", &f.policy_path(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["permissions"],
        serde_json::to_value(&original).unwrap()
    );
    let foreign = seed_test_api_token(f.api.store.as_ref(), "foreign-policy-account");
    let update = json!({"operation_id":"untrusted-admin","expected_change_id":original.change_id,
        "execution":[],"delegation":[]});
    for method in ["GET", "POST"] {
        let bytes = if method == "POST" {
            serde_json::to_vec(&update).unwrap()
        } else {
            vec![]
        };
        for source in [Some(f.source.worker_id.as_str()), None] {
            let (status, body) = response(
                &f.api,
                runtime_source_request(
                    &f.identity,
                    source,
                    method,
                    &f.policy_path(),
                    bytes.clone(),
                ),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} Runtime/Workspace source {source:?}: {body}"
            );
        }
        let (status, body) = response(
            &f.api,
            human_request(
                &foreign,
                method,
                &f.policy_path(),
                (method == "POST").then(|| update.clone()),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "foreign account {method}: {body}"
        );
    }
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap(),
        Some(original.clone())
    );
    let shrunk = f.assign(vec![activation("ticket")], vec![]).await;
    assert_ne!(shrunk.change_id, original.change_id);
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &f.policy_path(), Some(update)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale policy replacement: {body}"
    );
    let revoked = f.assign(vec![], vec![]).await;
    assert!(revoked.execution.is_empty() && revoked.delegation.is_empty());
    assert_eq!(
        f.check(&f.source, &activation("ticket")).await.0,
        StatusCode::FORBIDDEN
    );
}

#[derive(Clone)]
struct UnusedModel;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for UnusedModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        panic!("authorization tests must not execute a model")
    }
}
async fn ticket_worker_host_with_service(
    root: &Path,
    client: Arc<RouterClient>,
) -> (
    worker::Worker<UnusedModel, session_store::FsStore>,
    bool,
    Option<Arc<dyn worker::feature::builtin::ticket::TicketService>>,
) {
    let manifest = worker::WorkerManifest::from_toml(
        r#"
        [worker]
        name = "authorization-tools"
        [model]
        scheme = "anthropic"
        model_id = "unused"
        [engine]
        [scope]
        allow = []
        [feature.workdir_catalog]
        enabled = false
        [feature.ticket]
        enabled = true
        authoring = true
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
    let report = host.install_features(worker::feature::FeatureRegistryBuilder::new().with_module(
        TicketFeature::new(client, TicketFeatureAccess::workspace_authoring()),
    ));
    let installed = !report.has_errors();
    let service = if installed {
        Some(
            report
                .services
                .service::<dyn worker::feature::builtin::ticket::TicketService>(
                    &worker::feature::ServiceId::builtin(
                        worker::feature::builtin::ticket::TICKET_SERVICE_ID,
                    ),
                )
                .unwrap(),
        )
    } else {
        None
    };
    (host, installed, service)
}
async fn ticket_worker_host(
    root: &Path,
    client: Arc<RouterClient>,
) -> (worker::Worker<UnusedModel, session_store::FsStore>, bool) {
    let (host, installed, _) = ticket_worker_host_with_service(root, client).await;
    (host, installed)
}
async fn ticket_tool_host(root: &Path, client: Arc<RouterClient>) -> (bool, Option<Arc<dyn Tool>>) {
    let (host, installed) = ticket_worker_host(root, client).await;
    let server = host.engine().tool_server_handle();
    server.flush_pending();
    let tool = server.get_tool("TicketCreate").map(|(_, tool)| tool);
    if !installed {
        assert!(
            tool.is_none(),
            "denied activation must not partially install TicketCreate"
        );
    }
    (installed, tool)
}
fn ticket_posts(client: &RouterClient) -> usize {
    client
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, path)| {
            method == "POST" && path == &format!("/api/w/{TEST_WORKSPACE_ID}/tickets")
        })
        .count()
}
fn value(output: ToolOutput) -> Value {
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_feature_activation_is_distinct_from_ticket_create_and_revocation_precedes_backend_call()
 {
    let f = Fixture::new().await;
    let client = f.client(&f.source);
    for (name, permissions, installed) in [
        ("unset", vec![], false),
        ("operation-only", vec![create_permission()], false),
        ("activation-only", vec![activation("ticket")], true),
    ] {
        f.assign(permissions, vec![]).await;
        let (actual, tool) = ticket_tool_host(&f._dir.path().join(name), client.clone()).await;
        assert_eq!(actual, installed, "{name}");
        if let Some(tool) = tool {
            let result = tool
                .execute(
                    r#"{"title":"must not reach TicketBackend"}"#,
                    ToolExecutionContext::direct(),
                )
                .await;
            assert!(result.is_err(), "activation alone cannot create a Ticket");
            assert_eq!(
                ticket_posts(&client),
                0,
                "denial must precede TicketBackend HTTP call"
            );
        }
    }
    f.assign(vec![activation("ticket"), create_permission()], vec![])
        .await;
    let (installed, tool) = ticket_tool_host(&f._dir.path().join("allowed"), client.clone()).await;
    assert!(installed);
    let tool = tool.unwrap();
    let created = value(
        tool.execute(
            r#"{"title":"Real authorized Ticket"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(ticket_posts(&client), 1);
    let backend = browser_ticket_backend(&f.api).unwrap();
    let records = backend.list(Default::default()).unwrap();
    assert_eq!(
        records.len(),
        1,
        "genuine SQLite Ticket backend result: {created}"
    );
    f.assign(vec![activation("ticket")], vec![]).await;
    let result = tool
        .execute(
            r#"{"title":"revoked after install"}"#,
            ToolExecutionContext::direct(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        ticket_posts(&client),
        1,
        "old installed Tool must recheck before backend call"
    );
    assert_eq!(backend.list(Default::default()).unwrap().len(), 1);
}

fn creation_record_counts(f: &Fixture) -> [i64; 3] {
    let connection = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    [
        "worker_create_reservations",
        "worker_registry",
        "worker_control_grants",
    ]
    .map(|table| {
        connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE workspace_id=?1"),
                [TEST_WORKSPACE_ID],
                |row| row.get(0),
            )
            .unwrap()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_create_routes_refuse_body_and_child_delegation_before_any_creation_effect() {
    let f = Fixture::new().await;
    let permissions = vec![activation("ticket"), create_permission()];
    f.assign(vec![], permissions.clone()).await;
    let mut child_request = f.child_request("redelegating-editor", permissions.clone());
    child_request["execution_grant"]["redelegation"] = json!(permissions);
    let (status, child) = f.spawn(&f.source, child_request).await;
    assert_eq!(status, StatusCode::OK, "{child}");
    let child = RuntimeWorkerRef::new(
        child["runtime_id"].as_str().unwrap(),
        child["worker_id"].as_str().unwrap(),
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let grants = store
        .list_subject_worker_execution_grants(&f.subject)
        .unwrap();
    let records = creation_record_counts(&f);
    let runtime_calls = f.runtime.spawn_requests().len();
    for source in [&f.source, &child] {
        for path in [
            format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            "/api/workers".into(),
        ] {
            let exact = f.child_request("generic-must-not-allocate", permissions.clone());
            let mut missing_id = exact.clone();
            missing_id
                .as_object_mut()
                .unwrap()
                .remove("control_operation_id");
            let mut blank_id = exact.clone();
            blank_id["control_operation_id"] = json!(" ");
            let mut conflicting = exact.clone();
            conflicting["display_name"] = json!("Different payload, same operation ID");
            for request in [exact.clone(), exact, missing_id, blank_id, conflicting] {
                let (status, body) = response(
                    &f.api,
                    runtime_source_request(
                        &f.identity,
                        Some(&source.worker_id),
                        "POST",
                        &path,
                        serde_json::to_vec(&request).unwrap(),
                    ),
                )
                .await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
                assert!(
                    body["message"].as_str().unwrap().contains("worker-control"),
                    "{body}"
                );
                assert_eq!(
                    creation_record_counts(&f),
                    records,
                    "{path}: no reservation, registry or control grant"
                );
                assert_eq!(
                    store
                        .list_subject_worker_execution_grants(&f.subject)
                        .unwrap(),
                    grants
                );
                assert_eq!(
                    f.runtime.spawn_requests().len(),
                    runtime_calls,
                    "{path}: no Runtime creation"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_create_routes_preserve_human_and_non_subject_worker_creation() {
    let f = Fixture::new().await;
    let mut request = f.child_request("unused-on-generic-route", vec![]);
    request.as_object_mut().unwrap().remove("execution_grant");
    request
        .as_object_mut()
        .unwrap()
        .remove("control_operation_id");
    for path in [
        format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
        "/api/workers".into(),
    ] {
        let (status, human_child) = response(
            &f.api,
            human_request(&f.owner, "POST", &path, Some(request.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "human {path}: {human_child}");
        let (status, worker_child) = response(
            &f.api,
            runtime_source_request(
                &f.identity,
                Some(human_child["worker_id"].as_str().unwrap()),
                "POST",
                &path,
                serde_json::to_vec(&request).unwrap(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "non-Subject {path}: {worker_child}");
        assert!(
            open_subjektiv_store(&f.api)
                .unwrap()
                .worker_execution_grant(
                    worker_child["runtime_id"].as_str().unwrap(),
                    worker_child["worker_id"].as_str().unwrap(),
                )
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subject_control_creation_recovers_control_grant_failure_and_lost_response_without_duplicates()
 {
    let f = Fixture::new().await;
    let permissions = vec![activation("ticket"), create_permission()];
    f.assign(vec![], permissions.clone()).await;
    let mut missing_id = f.child_request("control-grant-recovery", permissions.clone());
    missing_id
        .as_object_mut()
        .unwrap()
        .remove("control_operation_id");
    let records = creation_record_counts(&f);
    let (status, body) = f.spawn(&f.source, missing_id).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(creation_record_counts(&f), records);
    let connection = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_control_grant BEFORE INSERT ON worker_control_grants BEGIN SELECT RAISE(ABORT, 'injected control grant failure'); END;").unwrap();
    let request = f.child_request("control-grant-recovery", permissions);
    let (status, body) = f.spawn(&f.source, request.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let store = open_subjektiv_store(&f.api).unwrap();
    let grants = store
        .list_subject_worker_execution_grants(&f.subject)
        .unwrap();
    assert_eq!(
        grants.len(),
        1,
        "Worker exists before control grant persistence fails"
    );
    let child = RuntimeWorkerRef::new(&grants[0].runtime_id, &grants[0].worker_id);
    assert_eq!(
        f.runtime.workers.lock().unwrap().len(),
        2,
        "one source and one child"
    );
    assert!(
        f.api
            .store
            .list_active_worker_control_grants(TEST_WORKSPACE_ID, &f.source, 10)
            .unwrap()
            .is_empty()
    );
    connection
        .execute_batch("DROP TRIGGER fail_control_grant;")
        .unwrap();
    let runtime_calls = f.runtime.spawn_requests().len();
    for _ in 0..2 {
        // The first success is deliberately not used as a retry credential.
        let (status, body) = f.spawn(&f.source, request.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["worker_id"], child.worker_id);
        assert_eq!(
            store
                .list_subject_worker_execution_grants(&f.subject)
                .unwrap(),
            grants
        );
        let control = f
            .api
            .store
            .list_active_worker_control_grants(TEST_WORKSPACE_ID, &f.source, 10)
            .unwrap();
        assert_eq!(control.len(), 1);
        assert_eq!(control[0].subject, child);
        assert_eq!(
            f.runtime.spawn_requests().len(),
            runtime_calls,
            "completed creation must not resubmit to Runtime"
        );
    }
    let records = creation_record_counts(&f);
    let mut conflict = request;
    conflict["display_name"] = json!("Changed control retry");
    assert!(!f.spawn(&f.source, conflict).await.0.is_success());
    assert_eq!(creation_record_counts(&f), records);
    assert_eq!(f.runtime.spawn_requests().len(), runtime_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_delegation_creates_workdirless_ticketless_editor_and_replays_one_worker_and_grant()
 {
    let f = Fixture::new().await;
    let grant_permissions = vec![activation("ticket"), create_permission()];
    let policy = f.assign(vec![], grant_permissions.clone()).await;
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN,
        "delegation ceiling does not activate operations on the body"
    );
    let child = f
        .child("ordinary-editor-once", grant_permissions.clone())
        .await;
    let requests = f.runtime.spawn_requests();
    let source_manifest = crate::profile_settings::resolve_profile_manifest_from_config_bundle(
        requests[0].resolved_config_bundle.as_ref().unwrap(),
        "project:authority-source",
    )
    .unwrap();
    assert!(!source_manifest.scope.allow.is_empty());
    assert!(
        source_manifest
            .scope
            .allow
            .iter()
            .all(|rule| rule.permission == manifest::Permission::Read)
    );
    let editor_manifest = crate::profile_settings::resolve_profile_manifest_from_config_bundle(
        requests[1].resolved_config_bundle.as_ref().unwrap(),
        "builtin:coder",
    )
    .unwrap();
    assert!(
        editor_manifest
            .scope
            .allow
            .iter()
            .any(|rule| rule.permission == manifest::Permission::Write),
        "editor Profile resolution is independent of the parent's read-only Profile"
    );
    assert!(
        f.api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &child)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child)
            .unwrap()
            .is_none()
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let grant = store
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(grant.subject_id, f.subject);
    assert_eq!(grant.origin_runtime_id, f.source.runtime_id);
    assert_eq!(grant.origin_worker_id, f.source.worker_id);
    assert_eq!(grant.source_change_id, policy.change_id);
    assert_eq!(grant.permissions, grant_permissions);
    assert!(grant.redelegation.is_empty());
    assert_eq!(grant.profile, "builtin:coder");
    assert!(!grant.profile_digest.is_empty());
    let (status, replay) = f
        .spawn(
            &f.source,
            f.child_request("ordinary-editor-once", grant_permissions.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["worker_id"], child.worker_id);
    let worker_identities = f
        .runtime
        .workers
        .lock()
        .unwrap()
        .iter()
        .map(|summary| {
            (
                summary.worker.runtime_id.clone(),
                summary.worker.worker_id.clone(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        worker_identities.len(),
        2,
        "Runtime may receive replayed creation, but only one source and one reserved editor identity exist"
    );
    let replay_calls = f.runtime.spawn_requests().len();
    assert_eq!(
        store
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap(),
        Some(grant)
    );
    let mut changed = f.child_request("ordinary-editor-once", grant_permissions);
    changed["display_name"] = json!("changed replay payload");
    let (status, body) = f.spawn(&f.source, changed).await;
    assert!(
        !status.is_success(),
        "changed payload must not be adopted: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("workspace_worker_allocation_conflict"),
        "known allocation refusal, not unknown Runtime outcome: {status} {body}"
    );
    assert_eq!(f.runtime.spawn_requests().len(), replay_calls);
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::OK
    );
    let (status, body) = f
        .spawn(
            &child,
            f.child_request("undelegatable-grandchild", vec![activation("ticket")]),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "grant has no redelegation: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegated_creation_rejects_excess_permissions_and_body_identity_before_runtime_spawn() {
    let f = Fixture::new().await;
    f.assign(vec![], vec![activation("ticket")]).await;
    let mut singleton = f.child_request("singleton-escalation", vec![activation("ticket")]);
    singleton["singleton_key"] = json!("model-chosen-singleton");
    let mut body_attachment = f.child_request("subject-escalation", vec![activation("ticket")]);
    body_attachment["feature_connections"] = json!({"subjektiv":{"subject_id":f.subject}});
    let mut spoof = f.child_request("origin-escalation", vec![activation("ticket")]);
    spoof["execution_grant"]["subject_id"] = json!(f.subject);
    for request in [
        singleton,
        body_attachment,
        f.child_request("ceiling-escalation", vec![create_permission()]),
        spoof,
    ] {
        let (status, body) = f.spawn(&f.source, request).await;
        assert!(status.is_client_error(), "{status}: {body}");
    }
    assert_eq!(
        f.runtime.spawn_requests().len(),
        1,
        "only the genuine body reached Runtime execution"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_child_cannot_refresh_revoked_policy_and_released_origin_never_falls_back_to_legacy()
 {
    let f = Fixture::new().await;
    f.assign(vec![], vec![activation("ticket")]).await;
    let child = f.child("revocable-child", vec![activation("ticket")]).await;
    assert_eq!(
        f.check(&child, &activation("ticket")).await.0,
        StatusCode::OK
    );
    let before = open_subjektiv_store(&f.api)
        .unwrap()
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    f.assign(vec![], vec![]).await;
    assert_eq!(
        f.check(&child, &activation("ticket")).await.0,
        StatusCode::FORBIDDEN
    );
    let observation = f
        .api
        .runtime
        .worker(&child)
        .unwrap()
        .restore_observation_token
        .unwrap();
    let restore_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/restore",
        child.runtime_id, child.worker_id
    );
    let (status, body) = response(&f.api, human_request(&f.owner, "POST", &restore_path,
        Some(json!({"expected_observation_token":observation,"request_id":"restore-revoked-child"})))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.check(&child, &activation("ticket")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap(),
        Some(before)
    );
    // Regranting identical permissions changes the assignment identity. Old
    // grants remain revoked rather than refreshing during restore/reconnect.
    f.assign(vec![], vec![activation("ticket")]).await;
    assert_eq!(
        f.check(&child, &activation("ticket")).await.0,
        StatusCode::FORBIDDEN
    );
    let current = f
        .child("current-before-origin-release", vec![activation("ticket")])
        .await;
    assert_eq!(
        f.check(&current, &activation("ticket")).await.0,
        StatusCode::OK
    );
    f.release_source_lease();
    assert_eq!(
        f.check(&current, &activation("ticket")).await.0,
        StatusCode::CONFLICT,
        "the captured source lease is no longer current"
    );
    assert_eq!(
        f.check(&f.source, &activation("ticket")).await.0,
        StatusCode::UNAUTHORIZED,
        "released body fails signed-source authentication instead of falling through to legacy compatibility"
    );
    let (status, _) = f
        .spawn(
            &f.source,
            f.child_request("released-origin-spawn", vec![activation("ticket")]),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_wip_ticket_create_uses_same_signed_authorization_before_real_backend() {
    let f = Fixture::new().await;
    f.assign(vec![activation("ticket")], vec![]).await;
    let client = f.client(&f.source);
    let (mut host, installed) =
        ticket_worker_host(&f._dir.path().join("wip-host"), client.clone()).await;
    assert!(installed);
    let mut mounts = worker::wip::WipMountRegistry::new();
    let route = mounts.allocate_namespace("ticket", "tickets").unwrap();
    worker::feature::builtin::ticket::mount_workspace_http_ticket_wip(
        &mut mounts,
        client.clone(),
        TicketFeatureAccess::workspace_authoring(),
        None,
        &route,
    )
    .unwrap();
    worker::wip::install_wip_mode_with_mounts(
        host.engine_mut(),
        None,
        "subject-authorization-wip".into(),
        mounts,
    )
    .unwrap();
    let server = host.engine().tool_server_handle();
    server.flush_pending();
    assert!(
        server.get_tool("TicketCreate").is_none(),
        "native surface replaces compatibility Tool"
    );
    let inspect = server.get_tool("Inspect").unwrap().1;
    let invoke = server.get_tool("Invoke").unwrap().1;
    let inspected = value(
        inspect
            .execute(
                r#"{"path":"/tickets","refresh":true}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap(),
    );
    let reference = inspected["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|interface| interface["reference"]["name"] == "yoi.ticket/collection/v1")
        .expect("native Ticket collection interface")["reference"]
        .clone();
    let args = json!({"path":"/tickets","interface":reference,"operation":"create","arguments":{"title":"Native authorized Ticket"}}).to_string();
    assert!(
        invoke
            .execute(&args, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert_eq!(
        ticket_posts(&client),
        0,
        "WIP denial must not call TicketBackend HTTP"
    );
    f.assign(vec![activation("ticket"), create_permission()], vec![])
        .await;
    invoke
        .execute(&args, ToolExecutionContext::direct())
        .await
        .unwrap();
    assert_eq!(ticket_posts(&client), 1);
    let backend = browser_ticket_backend(&f.api).unwrap();
    let tickets = backend.list(Default::default()).unwrap();
    assert_eq!(tickets.len(), 1);
    f.assign(vec![activation("ticket")], vec![]).await;
    assert!(
        invoke
            .execute(&args, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert_eq!(
        ticket_posts(&client),
        1,
        "WIP does not cache an old operation allow"
    );
    assert_eq!(backend.list(Default::default()).unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_replacement_reports_interruption_evidence_but_exact_replay_does_not_interrupt_again()
 {
    let f = Fixture::new().await;
    let update = json!({"operation_id":"policy-interruption-once","expected_change_id":null,
        "execution":[activation("ticket")],"delegation":[]});
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &f.policy_path(), Some(update.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let committed: server_api::SubjectPermissionsMutationResponse =
        serde_json::from_value(body).unwrap();
    assert_eq!(committed.interruptions.len(), 1);
    let evidence = &committed.interruptions[0];
    assert_eq!(evidence.runtime_id, f.source.runtime_id);
    assert_eq!(evidence.worker_id, f.source.worker_id);
    assert!(
        evidence.cancellation_accepted,
        "execution fixture accepts cancel, not proof a turn stopped"
    );
    assert!(
        evidence.backend_command_sessions_closed,
        "cleanup evidence is separate from cancel acceptance"
    );
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap(),
        Some(committed.permissions.clone())
    );
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &f.policy_path(), Some(update)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let replay: server_api::SubjectPermissionsMutationResponse =
        serde_json::from_value(body).unwrap();
    assert_eq!(replay.permissions, committed.permissions);
    assert!(
        replay.interruptions.is_empty(),
        "receipt replay must not request a new interruption"
    );
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "GET", &f.policy_path(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let read: server_api::SubjectPermissionsResponse = serde_json::from_value(body).unwrap();
    assert_eq!(read.permissions, Some(replay.permissions));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grandchild_grant_uses_immediate_parent_subset_not_root_subject_ceiling() {
    let f = Fixture::new().await;
    f.assign(vec![], vec![activation("ticket"), create_permission()])
        .await;
    let mut request = f.child_request(
        "redelegating-parent",
        vec![activation("ticket"), create_permission()],
    );
    request["execution_grant"]["redelegation"] = json!([activation("ticket")]);
    let (status, body) = f.spawn(&f.source, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let parent = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let (status, body) = f
        .spawn(
            &parent,
            f.child_request("allowed-grandchild", vec![activation("ticket")]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let grandchild = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let grant = open_subjektiv_store(&f.api)
        .unwrap()
        .worker_execution_grant(&grandchild.runtime_id, &grandchild.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        grant.origin_worker_id, parent.worker_id,
        "keep immediate lineage instead of flattening to the body"
    );
    assert_eq!(
        f.check(&grandchild, &activation("ticket")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.check(&grandchild, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let calls = f.runtime.spawn_requests().len();
    let (status, body) = f
        .spawn(
            &parent,
            f.child_request("root-ceiling-escalation", vec![create_permission()]),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "root permits create but immediate parent cannot pass it: {body}"
    );
    assert_eq!(f.runtime.spawn_requests().len(), calls);
    f.assign(vec![], vec![]).await;
    assert_eq!(
        f.check(&grandchild, &activation("ticket")).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_profile_name_with_wider_resolved_scope_cannot_rebind_existing_operation() {
    let f = Fixture::new().await;
    f.assign(vec![], vec![activation("ticket")]).await;
    let mut request = f.child_request("profile-bound-once", vec![activation("ticket")]);
    request["profile"] = json!("project:authority-editor");
    let (status, body) = f.spawn(&f.source, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let original = store
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    let calls = f.runtime.spawn_requests().len();
    install_subject_profiles(&f.api, "workspace_write");
    let (status, body) = f.spawn(&f.source, request).await;
    assert!(
        !status.is_success(),
        "same-name Profile must not replace the operation's resolved scope: {body}"
    );
    assert_eq!(
        store
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap(),
        Some(original)
    );
    assert_eq!(
        f.runtime.spawn_requests().len(),
        calls,
        "changed bundle must fail before Runtime creation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_ticket_service_and_raw_backend_recheck_current_signed_source() {
    let f = Fixture::new().await;
    f.assign(vec![activation("ticket")], vec![]).await;
    let client = f.client(&f.source);
    let (_host, installed, service) =
        ticket_worker_host_with_service(&f._dir.path().join("service-host"), client.clone()).await;
    assert!(installed);
    let service = service.unwrap();
    let denied = service.clone();
    assert!(
        tokio::task::spawn_blocking(move || denied.create(NewTicket::new("Service denied")))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        ticket_posts(&client),
        0,
        "service denial happens before Ticket backend HTTP"
    );
    // A real unattached Worker has the legacy contract; hints cannot substitute
    // that principal for the signed Subject body.
    let Json(legacy) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Raw hint target".into(),
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
    // Bypassing the cooperative Feature check cannot bypass the Server's recheck.
    let mut raw = runtime_source_request(
        &f.identity,
        Some(&f.source.worker_id),
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets"),
        serde_json::to_vec(&NewTicket::new("Raw backend denied")).unwrap(),
    );
    raw.headers_mut()
        .insert("x-yoi-runtime-id", legacy.runtime_id.parse().unwrap());
    raw.headers_mut()
        .insert("x-yoi-worker-id", legacy.worker_id.parse().unwrap());
    let (status, body) = response(&f.api, raw).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "raw source hints cannot become another principal: {body}"
    );
    assert!(
        browser_ticket_backend(&f.api)
            .unwrap()
            .list(Default::default())
            .unwrap()
            .is_empty()
    );
    f.assign(vec![activation("ticket"), create_permission()], vec![])
        .await;
    let allowed = service.clone();
    let created =
        tokio::task::spawn_blocking(move || allowed.create(NewTicket::new("Service authorized")))
            .await
            .unwrap()
            .unwrap();
    let backend = browser_ticket_backend(&f.api).unwrap();
    assert_eq!(
        backend.show(created.id.into()).unwrap().meta.title,
        "Service authorized"
    );
    assert_eq!(ticket_posts(&client), 1);
    f.assign(vec![activation("ticket")], vec![]).await;
    assert!(
        tokio::task::spawn_blocking(move || service.create(NewTicket::new("Revoked service")))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(ticket_posts(&client), 1);
    let raw = runtime_source_request(
        &f.identity,
        Some(&f.source.worker_id),
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets"),
        serde_json::to_vec(&NewTicket::new("Raw revoked backend")).unwrap(),
    );
    assert_eq!(response(&f.api, raw).await.0, StatusCode::FORBIDDEN);
    assert_eq!(backend.list(Default::default()).unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_host_scope_guards_real_local_read_write_and_revocation_before_effects() {
    use workdir::{ReadRequest, WorkdirPath, WriteRequest};
    let f = Fixture::new().await;
    let resource = PermissionResource::Workdir {
        working_directory_id: "fixture-local-attachment".into(),
        path: "src".into(),
        recursive: true,
    };
    let permissions = vec![
        ScopedPermission {
            operation: "filesystem.read".into(),
            resource: resource.clone(),
        },
        ScopedPermission {
            operation: "filesystem.write".into(),
            resource: resource.clone(),
        },
    ];
    f.assign(vec![], permissions.clone()).await;
    let child = f.child("local-resource-child", permissions).await;
    // This independently exercises real local operations; the creation Runtime
    // fixture never executes the editor. The attachment ID is explicit fixture
    // input, not a claim that a real Runtime materialized this local directory.
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/source.txt"), b"source").unwrap();
    fs::write(root.path().join("private.txt"), b"secret").unwrap();
    let client = f.client(&child);
    let session = worker::authorized_workdir::authorized_workdir_session(
        Arc::new(workdir::LocalWorkdirSession::new(
            manifest::Scope::writable(root.path()).unwrap(),
            root.path().to_path_buf(),
        )),
        client,
        "fixture-local-attachment".into(),
    );
    let read = |path: &str| ReadRequest {
        path: WorkdirPath::new(path).unwrap(),
        offset: 0,
        limit: 100,
        max_bytes: 4096,
    };
    assert_eq!(
        session.read(read("src/source.txt")).await.unwrap().bytes,
        b"source"
    );
    assert!(session.read(read("private.txt")).await.is_err());
    session
        .write(WriteRequest {
            path: WorkdirPath::new("src/created.txt").unwrap(),
            content: b"created".to_vec(),
            expected_hash: None,
        })
        .await
        .unwrap();
    assert_eq!(
        fs::read(root.path().join("src/created.txt")).unwrap(),
        b"created"
    );
    assert!(
        session
            .write(WriteRequest {
                path: WorkdirPath::new("denied.txt").unwrap(),
                content: b"bad".to_vec(),
                expected_hash: None
            })
            .await
            .is_err()
    );
    assert!(!root.path().join("denied.txt").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            root.path().join("private.txt"),
            root.path().join("src/link"),
        )
        .unwrap();
        assert!(
            session.read(read("src/link")).await.is_err(),
            "Host's granted src prefix must not follow a symlink outside src"
        );
        assert!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("src/link").unwrap(),
                    content: b"escaped".to_vec(),
                    expected_hash: None
                })
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(root.path().join("private.txt")).unwrap(),
            b"secret"
        );
    }
    let (_, scope) = f
        .check(
            &child,
            &ScopedPermission {
                operation: "filesystem.read".into(),
                resource: PermissionResource::Workdir {
                    working_directory_id: "fixture-local-attachment".into(),
                    path: "src/source.txt".into(),
                    recursive: false,
                },
            },
        )
        .await;
    assert_eq!(
        scope,
        json!({"scope":resource}),
        "response preserves granted prefix rather than turning requested symlink path into a new root"
    );
    f.assign(vec![], vec![]).await;
    assert!(session.read(read("src/source.txt")).await.is_err());
    assert!(
        session
            .write(WriteRequest {
                path: WorkdirPath::new("src/created.txt").unwrap(),
                content: b"revoked".to_vec(),
                expected_hash: None
            })
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(root.path().join("src/created.txt")).unwrap(),
        b"created"
    );
    session.close().await.unwrap();
}

/// Script only the external model boundary. Tool results come back through the
/// production Controller; this model never reads or writes the fixture itself.
#[derive(Clone)]
struct ConnectedAuthorizationModel {
    completed: tokio::sync::mpsc::Sender<Vec<(String, bool, String)>>,
}

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for ConnectedAuthorizationModel {
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = std::result::Result<
                            agen::llm_client::event::Event,
                            agen::llm_client::error::ClientError,
                        >,
                    > + Send,
            >,
        >,
        agen::llm_client::error::ClientError,
    > {
        use agen::llm_client::{
            event::{Event, StopReason},
            types::Item,
        };
        let results: Vec<_> = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    is_error,
                    content,
                    summary,
                    ..
                } if matches!(call_id.as_str(), "connected-read" | "connected-write") => Some((
                    call_id.clone(),
                    *is_error,
                    format!("{summary}; {content:?}"),
                )),
                _ => None,
            })
            .collect();
        let events = if results.iter().any(|(id, _, _)| id == "connected-write") {
            // A bounded channel is our completion barrier: no timers, sleeps or
            // filesystem polling decide whether the real tools have executed.
            let _ = self.completed.try_send(results);
            vec![
                Event::text_block_start(0),
                Event::text_delta(0, "connected tools finished"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ]
        } else {
            let (id, name, input) = if results.iter().any(|(id, _, _)| id == "connected-read") {
                (
                    "connected-write",
                    "Write",
                    json!({"target_workdir":"workdir", "file_path":"README.md", "content":"MUST_NOT_REPLACE_README\n"}),
                )
            } else {
                (
                    "connected-read",
                    "Read",
                    json!({"target_workdir":"workdir", "file_path":"README.md"}),
                )
            };
            assert!(
                request.tools.iter().any(|tool| tool.name == name),
                "editing Profile must expose {name}; denial must not be schema absence"
            );
            vec![
                Event::tool_use_start(0, id, name),
                Event::tool_input_delta(0, input.to_string()),
                Event::tool_use_stop(0),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
}

/// Use the same signed source protocol as RuntimeOwnedWorkspaceClient, with a
/// per-test audience and listener rather than the older server-test/8787 fixture.
async fn connected_authorization_post(
    base_url: &str,
    identity: &RuntimeIdentityMaterial,
    source: &RuntimeWorkerRef,
    path: &str,
    body: &Value,
) -> (StatusCode, Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(identity)
        .issue(
            base_url,
            TEST_WORKSPACE_ID,
            Some(&source.worker_id),
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            "POST",
            path,
            &bytes,
            i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
            30,
        )
        .unwrap();
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap()
        .post(format!("{base_url}{path}"))
        .header(CONTENT_TYPE, "application/json")
        .header(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            proof,
        )
        .body(bytes)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let bytes = response.bytes().await.unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}));
    (status, body)
}

// The Runtime HTTP boundary owns synchronous lifecycle handlers. Give it an
// independent scheduler, matching separate product processes: a blocked Runtime
// request must not pin the Backend callback's Tokio local scheduling queue.
struct ConnectedRuntimeServer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl ConnectedRuntimeServer {
    fn start(listener: tokio::net::TcpListener, router: Router) -> Self {
        let listener = listener.into_std().unwrap();
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
            runtime.shutdown_timeout(std::time::Duration::from_secs(5));
        });
        Self {
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }
}
impl Drop for ConnectedRuntimeServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn connected_read_only_subject_delegates_editing_profile_to_real_runtime_read_but_not_write()
{
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;

    const RUNTIME: &str = "connected-subject-authorization";
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_url = format!("http://{}", backend_listener.local_addr().unwrap());
    let runtime_url = format!("http://{}", runtime_listener.local_addr().unwrap());
    let identity = RuntimeIdentityMaterial::generate(RUNTIME).unwrap();
    let runtime_root = dir.path().join("connected-runtime");
    let materializer = worker_runtime::working_directory::RuntimeGitMaterializer::new(
        dir.path().join("connected-workdirs"),
    );
    let (completed, mut completion) = tokio::sync::mpsc::channel(4);
    let model = ConnectedAuthorizationModel { completed };
    let factory = worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
        dir.path().join("connected-profiles"),
    )
    .with_remote_worker_mutation_identity(identity.clone())
    .with_workspace_request_client(
        worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
            TEST_WORKSPACE_ID,
            &backend_url,
            RUNTIME,
        )
        .with_runtime_request_source(&identity, &backend_url),
    )
    .with_runtime_store_dir(&runtime_root)
    .with_spawn_model_client_factory(move || Box::new(model.clone()));
    let execution = worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(factory)
        .unwrap()
        .with_working_directory_materializer(materializer.clone());
    let runtime = worker_runtime::Runtime::with_fs_store_and_execution_backend(
        worker_runtime::fs_store::FsRuntimeStoreOptions {
            root: runtime_root,
            runtime_id: RUNTIME.into(),
            display_name: None,
        },
        Arc::new(execution),
    )
    .unwrap();

    let mut config = test_server_config(dir.path());
    config.backend_base_url = Some(backend_url.clone());
    // The embedded Runtime is unused. Both source and child below execute on the
    // connected real Runtime above, not on this existing Host fixture backend.
    let control = Arc::new(test_control_store(&config));
    seed_test_registered_workspace(control.as_ref(), &config)
        .await
        .unwrap();
    let api = WorkspaceApi::new_with_execution_backend(
        config,
        control,
        Arc::new(DeterministicExecutionBackend::default()),
    )
    .await
    .unwrap();
    let now = now_registry_timestamp();
    api.store
        .upsert_workspace_runtime_binding_record(
            WorkspaceRuntimeBinding {
                workspace_id: TEST_WORKSPACE_ID.into(),
                runtime_id: RUNTIME.into(),
                display_name: RUNTIME.into(),
                base_url: runtime_url.clone(),
                public_key: identity.public_key.clone(),
                public_key_fingerprint: String::new(),
                binding_id: "connected-authorization-binding".into(),
                authentication_mode: StoredRuntimeAuthenticationMode::WorkspaceIdentity,
                created_at: now.clone(),
                updated_at: now,
                revoked_at: None,
            },
            false,
        )
        .await
        .unwrap();
    let binding = api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, RUNTIME)
        .await
        .unwrap()
        .unwrap();
    // Repository materialization and Worker startup must authenticate as this
    // Workspace, not the Runtime HTTP local-token fallback's `local` Workspace.
    let workspace_identity = api
        .signing_identities
        .provision_existing(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
        .unwrap();
    let verifier = worker_runtime::workspace_issuer::WorkspaceCapabilityVerifier::new(
        vec![
            worker_runtime::workspace_issuer::WorkspaceIssuerTrustRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                backend_url: backend_url.clone(),
                key_id: workspace_identity.key_id.clone(),
                algorithm: workspace_identity.algorithm.clone(),
                public_key: workspace_identity.public_key.clone().unwrap(),
                public_key_fingerprint: workspace_identity.public_key_fingerprint.clone().unwrap(),
                trust_id: "connected-authorization-workspace-trust".into(),
                state: worker_runtime::workspace_issuer::WorkspaceIssuerTrustState::Active,
                registered_at_unix: 1,
                updated_at_unix: 1,
            },
        ],
        Arc::new(
            worker_runtime::workspace_issuer::InMemoryWorkspaceClaimReplayProtection::default(),
        ),
    )
    .unwrap();
    let runtime_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_errors = runtime_errors.clone();
    let _runtime_server = ConnectedRuntimeServer::start(
        runtime_listener,
        worker_runtime::http_server::runtime_http_router_with_workspace_auth(
            runtime.clone(),
            None,
            worker_runtime::http_server::WorkspaceRuntimeHttpAuth {
                verifier,
                runtime_id: RUNTIME.into(),
            },
        )
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let errors = captured_errors.clone();
                async move {
                    let response = next.run(request).await;
                    if response.status().is_success() {
                        return response;
                    }
                    let (parts, body) = response.into_parts();
                    let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    errors.lock().unwrap().push(
                        body["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown fixture error")
                            .to_owned(),
                    );
                    Response::from_parts(parts, Body::from(bytes))
                }
            },
        )),
    );
    api.runtime.register_or_replace(
        RemoteWorkerRuntime::new(
            RemoteRuntimeConfig {
                runtime_id: RUNTIME.into(),
                workspace_id: Some(TEST_WORKSPACE_ID.into()),
                display_name: RUNTIME.into(),
                base_url: runtime_url,
                bearer_token: None,
                workspace_authorization: Some(crate::hosts::WorkspaceRuntimeAuthorization::new(
                    api.store.clone(),
                    api.signing_identities.clone(),
                    backend_url.clone(),
                    binding,
                )),
                strict_public_egress: false,
                cached_worker_creation_available: true,
                cached_os: "linux".into(),
                cached_arch: "x86_64".into(),
                cached_status: "active".into(),
                timeout: std::time::Duration::from_secs(30),
            },
            TEST_WORKSPACE_ID.to_string(),
            backend_url.clone(),
        )
        .unwrap(),
    );
    install_subject_profiles(&api, "workspace_write");
    seed_test_delivery_binding(&api);

    let authorizations = Arc::new(Mutex::new(Vec::<(Value, StatusCode)>::new()));
    let observed = authorizations.clone();
    let app = build_router(api.clone()).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            async move {
                if request.uri().path() != authorization_path() {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                let permission: Value = serde_json::from_slice(&bytes).unwrap();
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                observed
                    .lock()
                    .unwrap()
                    .push((permission, response.status()));
                response
            }
        },
    ));
    let _backend_server = AuthorizationCallbackServer::start(backend_listener, app);
    let owner =
        seed_test_workspace_owner_token(api.store.as_ref(), "connected-authorization-owner").await;
    let (status, created_workdir) = response(&api, human_request(&owner, "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
        Some(json!({"runtime_id":RUNTIME, "repository_key":"test-repository", "selector":"HEAD", "operation_id":"connected-authorization-workdir"})))).await;
    assert_eq!(status, StatusCode::CREATED, "{created_workdir}");
    let workdir_id = created_workdir["item"]["working_directory_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let binding = materializer
        .bind_working_directory(&workdir_id, None)
        .unwrap();
    let readme = binding.root().join("README.md");
    assert_eq!(fs::read_to_string(&readme).unwrap(), "clean\n");

    let subject_store = open_subjektiv_store(&api).unwrap();
    let subject = subject_store
        .create_subject(crate::subjektiv::SubjectRole::new("connected-authorization").unwrap())
        .unwrap()
        .id;
    let read_permission = ScopedPermission {
        operation: "filesystem.read".into(),
        resource: PermissionResource::Workdir {
            working_directory_id: workdir_id.clone(),
            path: ".".into(),
            recursive: true,
        },
    };
    let mut child_permissions: Vec<_> = [
        "task",
        "web",
        "image",
        "sub_worker",
        "flow",
        "worker",
        "workspace_worker_discovery",
        "objective",
        "manage_workdir",
        "workdir_catalog",
        "drive",
        "ticket",
        "merge_request",
        "subjektiv",
    ]
    .into_iter()
    .map(activation)
    .collect();
    child_permissions.push(read_permission.clone());
    // Set policy before startup: the actual source Controller sees this policy,
    // not an unset body or a registry/singleton row manufactured by the test.
    let (status, body) = response(&api, human_request(&owner, "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),
        Some(json!({"operation_id":"connected-authorization-policy", "expected_change_id":null,
            "execution":(["task", "subjektiv", "web", "image", "sub_worker", "worker", "workspace_worker_discovery", "objective", "manage_workdir", "workdir_catalog", "drive", "ticket"].into_iter().map(activation).collect::<Vec<_>>()),
            "delegation":child_permissions.clone()})))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let Json(source) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: RUNTIME.into(),
            display_name: "Connected read-only Subject body".into(),
            singleton_key: None,
            profile: Some("project:authority-source".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "source creation: {error:?}; Runtime: {:?}",
            runtime_errors.lock().unwrap()
        )
    });
    let source = RuntimeWorkerRef::new(source.runtime_id, source.worker_id);
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &source)
            .unwrap()
            .is_empty()
    );
    assert!(
        api.store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &source)
            .unwrap()
            .is_some()
    );
    let (status, body) = connected_authorization_post(
        &backend_url,
        &identity,
        &source,
        &authorization_path(),
        &serde_json::to_value(&read_permission).unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "body cannot execute its delegation ceiling: {body}"
    );

    // The normal noBash companion has no direct WorkerSpawn tool. Exercise its
    // signed source CreateWorkspaceWorkerRequest transport explicitly, as opposed
    // to installing a test-only privileged Feature or forging a child grant row.
    let request = CreateWorkspaceWorkerRequest {
        runtime_id: RUNTIME.into(),
        display_name: "Connected ordinary editing Profile child".into(),
        singleton_key: None,
        profile: Some("project:authority-editor".into()),
        ticket_assignment: None,
        initial_submit: vec![protocol::Segment::Text {
            content: "Read README.md, then attempt the denied Write.".into(),
        }],
        workdir_attachments: vec![server_api::BrowserWorkerWorkingDirectorySelection {
            alias: "workdir".into(),
            working_directory_id: workdir_id.clone(),
            relative_cwd: None,
        }],
        feature_connections: Default::default(),
        control_operation_id: Some("connected-authorization-child-once".into()),
        execution_grant: Some(server_api::authorization::WorkerDelegationRequest {
            permissions: child_permissions.clone(),
            redelegation: vec![],
        }),
    };
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers");
    authorizations.lock().unwrap().clear();
    let wire = serde_json::to_value(request).unwrap();
    let (status, child) =
        connected_authorization_post(&backend_url, &identity, &source, &path, &wire).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "normal signed source creation: {child}; Runtime: {:?}",
        runtime_errors.lock().unwrap()
    );
    let child_ref = RuntimeWorkerRef::new(
        child["runtime_id"].as_str().unwrap(),
        child["worker_id"].as_str().unwrap(),
    );
    let grant = subject_store
        .worker_execution_grant(&child_ref.runtime_id, &child_ref.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(grant.origin_worker_id, source.worker_id);
    assert_eq!(grant.permissions, child_permissions);
    assert!(grant.redelegation.is_empty());
    assert!(
        api.store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child_ref)
            .unwrap()
            .is_none(),
        "ordinary Profile child must not become a Subject body"
    );
    let (status, replay) =
        connected_authorization_post(&backend_url, &identity, &source, &path, &wire).await;
    assert_eq!(status, StatusCode::OK, "stable operation replay: {replay}");
    assert_eq!(replay["worker_id"], child["worker_id"]);

    let results = tokio::time::timeout(std::time::Duration::from_secs(30), completion.recv())
        .await
        .expect("real child Controller must return both tool results")
        .expect("scripted model completion channel");
    let read = results
        .iter()
        .find(|(id, _, _)| id == "connected-read")
        .unwrap();
    assert!(
        !read.1,
        "actual Read must succeed under the read-only grant: {read:?}"
    );
    assert!(
        read.2.contains("clean"),
        "Read must return materialized repository content: {read:?}"
    );
    let write = results
        .iter()
        .find(|(id, _, _)| id == "connected-write")
        .unwrap();
    assert!(
        write.1,
        "editing Profile must not enlarge the read-only grant: {write:?}"
    );
    assert!(
        write.2.contains("Workdir session does not support Write"),
        "the Host-attenuated attachment must deny before provider dispatch: {write:?}"
    );
    // The actual Write is refused even before provider dispatch by the narrowed
    // attachment capabilities. Independently query the same child's live Host
    // policy, rather than claiming that a refused Tool reached the write adapter.
    let mut denied_write = read_permission.clone();
    denied_write.operation = "filesystem.write".into();
    let (status, body) = connected_authorization_post(
        &backend_url,
        &identity,
        &child_ref,
        &authorization_path(),
        &serde_json::to_value(denied_write).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        fs::read_to_string(&readme).unwrap(),
        "clean\n",
        "denied Write has no effect on the actual Runtime materialization"
    );
    let observations = authorizations.lock().unwrap();
    for (operation, expected) in [
        ("filesystem.read", StatusCode::OK),
        ("filesystem.write", StatusCode::FORBIDDEN),
    ] {
        assert!(
            observations
                .iter()
                .any(|(permission, status)| permission["operation"] == operation
                    && permission["resource"]["working_directory_id"] == workdir_id
                    && *status == expected),
            "signed Host authorization must observe {operation} with {expected}: {observations:?}"
        );
    }
    assert!(
        browser_ticket_backend(&api)
            .unwrap()
            .list(Default::default())
            .unwrap()
            .is_empty(),
        "generic Profile delegation requires no Ticket"
    );
}
