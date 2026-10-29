//! Normal Server library boundary tests (`subject_authorization_tests` filter).
//! Router authentication, Host creation/reservation, Subject policy storage, Worker
//! Feature installation and Ticket SQLite writes are real. The original boundary
//! cases substitute WorkdirlessFixtureRuntime. The connected regression below also
//! runs a real ProfileRuntimeWorkerFactory/Controller and materialized Workdir;
//! only its external model stream is scripted. Neither fixture claims OS isolation.
use super::*;
#[path = "chat_delegation_connected_tests.rs"]
mod chat_delegation_connected;
#[path = "chat_tests.rs"]
mod chat_tests;
#[path = "subject_job_model_supply_tests.rs"]
mod subject_job_model_supply;
#[path = "ticket_worker_messaging_connected_tests.rs"]
mod ticket_worker_messaging_connected;
#[path = "worker_messaging_connected_tests.rs"]
mod worker_messaging_connected;
#[path = "worker_operation_authorization_tests.rs"]
mod worker_operation_authorization;
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
fn subject_permission(subject: &str, operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Subject {
            workspace_id: TEST_WORKSPACE_ID.into(),
            subject_id: subject.into(),
        },
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
        open_subjektiv_store(&api)
            .unwrap()
            .set_subject_permissions(
                &subject,
                &SubjectPermissionsUpdate {
                    operation_id: "fixture-session-setup".into(),
                    expected_change_id: None,
                    execution: vec![subject_permission(&subject, "session.record")],
                    delegation: vec![],
                },
            )
            .unwrap();
        // The execution fixture does not bootstrap a Session. Exercise the real
        // signed startup callback explicitly so the policy store sees the active
        // Journal writer, just as it does after normal Runtime startup.
        let (status, body) = response(&api, runtime_source_request(&identity, Some(&source.worker_id), "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions"),
            serde_json::to_vec(&json!({"session_id":session_store::new_session_id().to_string(),"create_if_missing":true})).unwrap())).await;
        assert_eq!(status, StatusCode::OK, "source startup callback: {body}");
        // Reproduce a migrated/unset policy only after authorized setup. Retain
        // immutable setup history; no protected callback is bypassed by this input.
        api.feature_storage
            .open(&api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM subject_permissions WHERE subject_id = ?1",
                    [&subject],
                )?;
                Ok(())
            })
            .unwrap();
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
    let legacy_client = f.client(&legacy);
    let (installed, tool) =
        ticket_tool_host(&f._dir.path().join("legacy-ticket"), legacy_client.clone()).await;
    assert!(installed);
    assert!(legacy_client.requests.lock().unwrap().is_empty());
    tool.unwrap()
        .execute(
            r#"{"title":"Unattached Worker retains provider-authorized create"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(ticket_posts(&legacy_client), 1);
    assert_eq!(
        browser_ticket_backend(&f.api)
            .unwrap()
            .list(Default::default())
            .unwrap()
            .len(),
        1
    );
}

fn concrete_resource_permissions(f: &Fixture) -> Vec<ScopedPermission> {
    let workspace = TEST_WORKSPACE_ID.to_owned();
    [
        (
            "ticket.read",
            PermissionResource::Ticket {
                workspace_id: workspace.clone(),
                ticket_id: "resource-ticket".into(),
            },
        ),
        (
            "objective.read",
            PermissionResource::Objective {
                workspace_id: workspace.clone(),
                objective_id: "resource-objective".into(),
            },
        ),
        (
            "merge_request.read",
            PermissionResource::MergeRequest {
                workspace_id: workspace.clone(),
                merge_request_id: "resource-mr".into(),
            },
        ),
        (
            "memory.resident.read",
            PermissionResource::Subject {
                workspace_id: workspace.clone(),
                subject_id: f.subject.clone(),
            },
        ),
        (
            "drive.read_text",
            PermissionResource::DriveEntry {
                workspace_id: workspace.clone(),
                node_id: "resource-entry".into(),
            },
        ),
        (
            "drive.request_status",
            PermissionResource::DriveRequest {
                workspace_id: workspace.clone(),
                request_id: "resource-request".into(),
            },
        ),
        (
            "workdir.create",
            PermissionResource::Repository {
                workspace_id: workspace.clone(),
                repository_key: "test-repository".into(),
            },
        ),
        (
            "workdir.detach",
            PermissionResource::WorkdirAttachment {
                workspace_id: workspace.clone(),
                connection_id: "resource-attachment".into(),
            },
        ),
        (
            "workspace_config.read",
            PermissionResource::WorkspaceConfig {
                workspace_id: workspace,
                path: "main.dcdl".into(),
                recursive: false,
            },
        ),
        (
            "web.fetch",
            PermissionResource::Network {
                url: "https://example.invalid/resource".into(),
            },
        ),
        (
            "subworker.observe",
            PermissionResource::SubWorker {
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone(),
                name: "resource-child".into(),
            },
        ),
    ]
    .into_iter()
    .map(|(operation, resource)| ScopedPermission {
        operation: operation.into(),
        resource,
    })
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_host_never_expands_saved_workspace_or_feature_grants_to_new_resource_kinds() {
    let f = Fixture::new().await;
    let requested = concrete_resource_permissions(&f);
    let workspace_grants: Vec<_> = requested
        .iter()
        .map(|permission| ScopedPermission {
            operation: permission.operation.clone(),
            resource: PermissionResource::Workspace {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
        })
        .collect();
    f.assign(workspace_grants.clone(), vec![]).await;
    for permission in &requested {
        let (status, body) = f.check(&f.source, permission).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "Workspace is not a parent resource: {permission:?}: {body}"
        );
    }
    // Same-operation Workspace grants above isolate kind containment from the
    // separate rule that an old Feature activation name grants no dotted operation.
    let feature_names: std::collections::BTreeSet<_> = requested
        .iter()
        .map(|permission| permission.operation.split('.').next().unwrap())
        .collect();
    f.assign(feature_names.into_iter().map(activation).collect(), vec![])
        .await;
    for permission in &requested {
        assert_eq!(
            f.check(&f.source, permission).await.0,
            StatusCode::FORBIDDEN,
            "{permission:?}"
        );
    }
    f.assign(requested.clone(), vec![]).await;
    for permission in &requested {
        let (status, body) = f.check(&f.source, permission).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "exact resource must be usable: {permission:?}: {body}"
        );
        assert_eq!(body, json!({"scope": permission.resource}));
    }
    // These are current persisted policies, not mocked authorization answers.
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap()
            .unwrap()
            .execution,
        requested
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_host_rejects_cross_workspace_resource_claims_before_alias_resolution() {
    let f = Fixture::new().await;
    let permissions = concrete_resource_permissions(&f);
    f.assign(permissions.clone(), vec![]).await;
    for permission in permissions
        .iter()
        .filter(|permission| permission.resource.workspace_id().is_some())
    {
        let mut forged = serde_json::to_value(permission).unwrap();
        forged["resource"]["workspace_id"] = json!("forged-workspace");
        let forged: ScopedPermission = serde_json::from_value(forged).unwrap();
        let (status, body) = f.check(&f.source, &forged).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{forged:?}: {body}");
        assert!(
            !body.to_string().contains("scope"),
            "refusal must not return effective scope: {body}"
        );
    }
}

fn seed_resource_ticket(f: &Fixture, title: &str, slug: &str) -> [String; 3] {
    let backend = browser_ticket_backend(&f.api).unwrap();
    let mut input = NewTicket::new(title);
    input.slug = Some(slug.into());
    input.body = "PRIVATE_AUTHORIZATION_BODY_MUST_NOT_BE_RETURNED".into();
    let created = backend.create(input).unwrap();
    let stored = backend.show(created.id.clone().into()).unwrap();
    let resource_key = created.resource_key.unwrap();
    assert!(resource_key.starts_with("T-"));
    assert_eq!(stored.meta.slug, slug);
    assert_ne!(created.id, resource_key);
    assert_ne!(created.id, slug);
    [created.id, resource_key, stored.meta.slug]
}

fn ticket_read_permission(reference: &str) -> ScopedPermission {
    ScopedPermission {
        operation: "ticket.read".into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: reference.into(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_host_ticket_grant_accepts_only_same_ticket_internal_id_code_and_slug() {
    let f = Fixture::new().await;
    let a = seed_resource_ticket(&f, "Allowed resource A", "resource-a-slug");
    let b = seed_resource_ticket(&f, "Denied resource B", "resource-b-slug");
    for granted in &a {
        f.assign(vec![ticket_read_permission(granted)], vec![])
            .await;
        for requested in &a {
            let permission = ticket_read_permission(requested);
            let (status, body) = f.check(&f.source, &permission).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "grant {granted}, request {requested}: {body}"
            );
            assert_eq!(
                body,
                json!({"scope": permission.resource}),
                "same identity must return the caller's requested spelling, not a broader scope"
            );
        }
        let absent = f
            .check(
                &f.source,
                &ticket_read_permission("resource-does-not-exist"),
            )
            .await;
        assert_eq!(absent.0, StatusCode::FORBIDDEN);
        for requested in &b {
            assert_eq!(
                f.check(&f.source, &ticket_read_permission(requested)).await,
                absent,
                "another existing Ticket and a missing Ticket must have the same refusal"
            );
        }
        // Alias lookup cannot cross resource kinds even when identity text matches.
        for resource in [
            PermissionResource::Objective {
                workspace_id: TEST_WORKSPACE_ID.into(),
                objective_id: granted.clone(),
            },
            PermissionResource::MergeRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                merge_request_id: granted.clone(),
            },
        ] {
            let permission = ScopedPermission {
                operation: "ticket.read".into(),
                resource,
            };
            assert_eq!(
                f.check(&f.source, &permission).await.0,
                StatusCode::FORBIDDEN
            );
        }
    }
    f.assign(vec![], vec![]).await;
    let absent = f
        .check(
            &f.source,
            &ticket_read_permission("resource-does-not-exist"),
        )
        .await;
    for reference in a.iter().chain(&b) {
        assert_eq!(
            f.check(&f.source, &ticket_read_permission(reference)).await,
            absent,
            "revocation must deny all spellings of an existing target just like an absent target"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_host_ungranted_operation_does_not_disclose_ticket_or_subject_existence() {
    let f = Fixture::new().await;
    let ticket = seed_resource_ticket(&f, "Private existence fixture", "private-existence-slug");
    // An unrelated operation on the right Subject is not permission to read it.
    f.assign(
        vec![subject_permission(&f.subject, "session.record")],
        vec![],
    )
    .await;
    for (existing, missing) in [
        (
            ticket_read_permission(&ticket[0]),
            ticket_read_permission("no-such-ticket"),
        ),
        (
            subject_permission(&f.subject, "memory.resident.read"),
            subject_permission("no-such-subject", "memory.resident.read"),
        ),
    ] {
        let denial = f.check(&f.source, &existing).await;
        assert_eq!(denial.0, StatusCode::FORBIDDEN);
        assert_eq!(
            denial,
            f.check(&f.source, &missing).await,
            "an ungranted operation must not distinguish existing and absent protected targets"
        );
        assert!(!denial.1.to_string().contains("PRIVATE_AUTHORIZATION_BODY"));
    }
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
    client: Arc<dyn WorkspaceClient>,
) -> (
    worker::Worker<UnusedModel, session_store::FsStore>,
    bool,
    Option<Arc<dyn worker::feature::contracts::TicketService>>,
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
                .service::<dyn worker::feature::contracts::TicketService>(
                    &worker::feature::ServiceId::builtin(
                        worker::feature::contracts::TICKET_SERVICE_ID,
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
    client: Arc<dyn WorkspaceClient>,
) -> (worker::Worker<UnusedModel, session_store::FsStore>, bool) {
    let (host, installed, _) = ticket_worker_host_with_service(root, client).await;
    (host, installed)
}
async fn ticket_tool_host(
    root: &Path,
    client: Arc<dyn WorkspaceClient>,
) -> (bool, Option<Arc<dyn Tool>>) {
    let (host, installed) = ticket_worker_host(root, client).await;
    let server = host.engine().tool_server_handle();
    server.flush_pending();
    let tool = server.get_tool("TicketCreate").map(|(_, tool)| tool);
    if !installed {
        assert!(
            tool.is_none(),
            "failed installation must not partially install TicketCreate"
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
async fn ticket_registration_is_local_and_only_create_permission_allows_backend_effects() {
    let f = Fixture::new().await;
    let client = f.client(&f.source);
    let mut wrong_workspace = create_permission();
    wrong_workspace.resource = PermissionResource::Ticket {
        workspace_id: TEST_WORKSPACE_ID.into(),
        ticket_id: "another-ticket".into(),
    };
    let (installed, tool) = ticket_tool_host(&f._dir.path().join("unset"), client.clone()).await;
    assert!(installed);
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "registration performs no HTTP"
    );
    assert!(
        tool.unwrap()
            .execute(
                r#"{"title":"unset must not create"}"#,
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert_eq!(ticket_posts(&client), 0);
    for (name, permissions) in [
        ("empty", vec![]),
        ("old-feature-only", vec![activation("ticket")]),
        ("wrong-resource", vec![wrong_workspace]),
    ] {
        f.assign(permissions, vec![]).await;
        client.requests.lock().unwrap().clear();
        let (installed, tool) = ticket_tool_host(&f._dir.path().join(name), client.clone()).await;
        assert!(installed, "{name}");
        assert!(
            client.requests.lock().unwrap().is_empty(),
            "{name}: registration performs no HTTP"
        );
        let result = tool
            .unwrap()
            .execute(
                r#"{"title":"must not reach TicketBackend"}"#,
                ToolExecutionContext::direct(),
            )
            .await;
        assert!(result.is_err(), "{name}");
        assert_eq!(
            ticket_posts(&client),
            0,
            "{name}: deny before downstream create"
        );
    }
    f.assign(vec![create_permission()], vec![]).await;
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

fn local_task_manifest() -> worker::WorkerManifest {
    worker::WorkerManifest::from_toml(
        r#"
        [worker]
        name = "local-authorization-tasks"
        [model]
        scheme = "anthropic"
        model_id = "unused"
        [engine]
        [scope]
        allow = []
        [feature.workdir_catalog]
        enabled = false
        [feature.task]
        enabled = true
    "#,
    )
    .unwrap()
}

fn local_bootstrap(
    root: &Path,
    manifest: worker::WorkerManifest,
    client: Arc<dyn WorkspaceClient>,
) -> worker::WorkerBootstrap<
    session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>,
> {
    worker::WorkerBootstrap::new(
        manifest,
        session_store::CombinedStore::new(
            session_store::FsStore::new(root.join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(root.join("worker-metadata")).unwrap(),
        ),
        worker::PromptCatalogSource::builtins_only(),
        worker::WorkerWorkspaceContext::with_client(
            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
            client,
        ),
        worker::WorkerFilesystemAuthority::None,
        worker::WorkerBootstrapLayout::Direct {
            runtime_base: root.join("runtime"),
        },
    )
    .with_model_client(UnusedModel)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unset_subject_local_task_startup_create_update_and_history_replay_make_no_http_calls() {
    let f = Fixture::new().await;
    let client = f.client(&f.source);
    assert!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap()
            .is_none()
    );
    let prepared = local_bootstrap(
        &f._dir.path().join("task-start"),
        local_task_manifest(),
        client.clone(),
    )
    .prepare()
    .await
    .unwrap();
    let tools = prepared.worker().engine().tool_server_handle();
    let boot = prepared.start().await.unwrap();
    tools.flush_pending();
    let create_args = r#"{"subject":"Local task","description":"No external operation"}"#;
    let create = tools.get_tool("TaskCreate").unwrap().1;
    let created = value(
        create
            .execute(create_args, ToolExecutionContext::direct())
            .await
            .unwrap(),
    );
    assert_eq!(created["taskid"], 1);
    assert_eq!(created["status"], "pending");
    let update_args = r#"{"taskid":1,"status":"completed"}"#;
    let update = tools.get_tool("TaskUpdate").unwrap().1;
    let updated = value(
        update
            .execute(update_args, ToolExecutionContext::direct())
            .await
            .unwrap(),
    );
    assert_eq!(updated["status"], "completed");
    boot.handle
        .send(protocol::Method::Shutdown {
            command: protocol::WorkerCommandEnvelope::new(1),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), boot.controller_task)
        .await
        .unwrap()
        .unwrap();

    // Replay uses the Feature's production history boundary, not a test store.
    let history = [
        agen::Item::tool_call("local-create", "TaskCreate", create_args),
        agen::Item::tool_call("local-update", "TaskUpdate", update_args),
    ];
    let replay = worker::feature::builtin::TaskFeature::from_history(&history);
    let mut prepared = local_bootstrap(
        &f._dir.path().join("task-replay"),
        local_task_manifest(),
        client.clone(),
    )
    .prepare()
    .await
    .unwrap();
    let report = prepared
        .worker_mut()
        .install_features(worker::feature::FeatureRegistryBuilder::new().with_module(replay));
    assert!(!report.has_errors(), "{}", report.error_message());
    let tools = prepared.worker().engine().tool_server_handle();
    tools.flush_pending();
    let task = value(
        tools
            .get_tool("TaskGet")
            .unwrap()
            .1
            .execute(r#"{"taskid":1}"#, ToolExecutionContext::direct())
            .await
            .unwrap(),
    );
    assert_eq!(task, updated, "replay preserves local Task state");
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "startup, Tools and replay must make zero activation or operation HTTP calls"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unset_subject_mcp_activation_denies_before_discovery_process_spawn() {
    let f = Fixture::new().await;
    let client = f.client(&f.source);
    let marker = f._dir.path().join("mcp-spawned");
    let mut manifest = local_task_manifest();
    manifest
        .mcp
        .stdio_servers
        .push(manifest::McpStdioServerConfig {
            name: "must-not-discover".into(),
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf spawned > \"$1\"".into(),
                "mcp-fixture".into(),
                marker.display().to_string(),
            ],
            cwd: None,
            env: Default::default(),
        });
    let error = match local_bootstrap(&f._dir.path().join("mcp-denied"), manifest, client.clone())
        .start()
        .await
    {
        Ok(boot) => {
            boot.controller_task.abort();
            panic!("MCP startup must be denied")
        }
        Err(error) => format!("{error:?}"),
    };
    assert!(error.contains("mcp") && error.contains("403"), "{error}");
    assert!(
        !marker.exists(),
        "denial must precede discovery's process effect"
    );
    let requests = client.requests.lock().unwrap();
    assert_eq!(
        requests.as_slice(),
        &[("POST".into(), authorization_path())],
        "only MCP's retained activation check may run"
    );
}

#[derive(Debug, Default)]
struct FailedAuthorizationTransport {
    requests: Mutex<Vec<String>>,
}
impl WorkspaceClient for FailedAuthorizationTransport {
    fn workspace_id(&self) -> Option<&str> {
        Some(TEST_WORKSPACE_ID)
    }
    fn kind(&self) -> &str {
        "failed-transport"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> std::result::Result<WorkspaceResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request.path);
        Err(WorkspaceClientError::Request(
            "injected connection failure".into(),
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_registration_without_host_is_local_but_operations_never_fall_back_to_standalone() {
    let dir = tempfile::tempdir().unwrap();
    let failed = Arc::new(FailedAuthorizationTransport::default());
    let bound_id = worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap();
    for (name, client) in [
        (
            "unconnected",
            worker::WorkerWorkspaceContext::no_workspace().client_handle(),
        ),
        (
            "standalone",
            worker::WorkerWorkspaceContext::local_filesystem(None).client_handle(),
        ),
        (
            "unavailable-bound",
            worker::WorkerWorkspaceContext::unavailable(Some(bound_id), "connection unavailable")
                .client_handle(),
        ),
        (
            "transport-error",
            failed.clone() as Arc<dyn WorkspaceClient>,
        ),
    ] {
        let (installed, tool) = ticket_tool_host(&dir.path().join(name), client).await;
        assert!(installed, "{name}: local registration needs no Host");
        let error = tool
            .unwrap()
            .execute(
                r#"{"title":"must not create"}"#,
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(
            !error.to_string().contains("403"),
            "unavailable is not policy denial: {name}: {error}"
        );
    }
    assert_eq!(
        failed.requests.lock().unwrap().as_slice(),
        &[authorization_path()],
        "communication failure must not issue downstream create"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subject_operation_allow_does_not_enlarge_real_provider_write_scope() {
    let f = Fixture::new().await;
    let permission = ScopedPermission {
        operation: "filesystem.write".into(),
        resource: PermissionResource::Workdir {
            working_directory_id: "read-only-provider".into(),
            path: ".".into(),
            recursive: true,
        },
    };
    f.assign(vec![permission.clone()], vec![]).await;
    assert_eq!(f.check(&f.source, &permission).await.0, StatusCode::OK);
    let root = tempfile::tempdir().unwrap();
    let mut config = manifest::ScopeConfig {
        allow: manifest::Scope::writable(root.path())
            .unwrap()
            .allow_rules(),
        deny: vec![],
    };
    config.allow[0].permission = manifest::Permission::Read;
    let client = f.client(&f.source);
    let session = worker::authorized_workdir::authorized_workdir_session(
        Arc::new(workdir::LocalWorkdirSession::new(
            manifest::Scope::from_config(&config).unwrap(),
            root.path().to_path_buf(),
        )),
        client.clone(),
        "read-only-provider".into(),
    );
    let result = session
        .write(workdir::WriteRequest {
            path: workdir::WorkdirPath::new("denied.txt").unwrap(),
            content: b"must-not-write".to_vec(),
            expected_hash: None,
        })
        .await;
    assert!(
        result.is_err(),
        "Host grant cannot override the provider's read-only scope"
    );
    assert!(!root.path().join("denied.txt").exists());
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(_, path)| path == &authorization_path()),
        "real Host authorization must be connected to provider dispatch"
    );
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
    let grant_permissions = vec![create_permission()];
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
        .find(|interface| interface["reference"]["name"] == "yoi.ticket/collection/v2")
        .expect("native Ticket collection interface")["reference"]
        .clone();
    let args = json!({"path":"/tickets","interface":reference,"operation":"create","arguments":{"content":{"title":"Native authorized Ticket"}}}).to_string();
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
    f.assign(vec![create_permission()], vec![]).await;
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
    f.assign(vec![create_permission()], vec![]).await;
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

#[tokio::test]
async fn delegated_subject_child_cannot_downgrade_ticket_start_to_manual_by_omitting_assignment() {
    let f = Fixture::new().await;
    let create = ScopedPermission {
        operation: "worker.create".into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    };
    f.assign(vec![create.clone()], vec![create.clone()]).await;
    let mut request = f.child_request("ticket-initiation-source", vec![create.clone()]);
    request["execution_grant"]["redelegation"] = json!([create]);
    let (status, created) = f.spawn(&f.source, request).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let child = RuntimeWorkerRef::new(
        created["runtime_id"].as_str().unwrap(),
        created["worker_id"].as_str().unwrap(),
    );
    // The child's ordinary redelegation remains valid: body ownership is a
    // Ticket-initiation condition, not a blanket ban on general Worker creation.
    let (status, ordinary) = f
        .spawn(&child, f.child_request("ordinary-grandchild", vec![]))
        .await;
    assert_eq!(status, StatusCode::OK, "{ordinary}");

    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(NewTicket::new("Another Subject's Ticket"))
        .unwrap();
    assign_test_orchestrator(&f.api, &ticket.id);
    let spawns = f.runtime.spawn_requests().len();
    let mut request = f.child_request("child-ticket-control", vec![]);
    request["ticket_assignment"] = json!({
        "ticket_id": ticket.id, "operation_id":"child-ticket-start-without-assignment",
    });
    let (status, denied) = f.spawn(&child, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert!(
        denied["message"].as_str().unwrap().contains("Subject body"),
        "{denied}"
    );
    assert_eq!(f.runtime.spawn_requests().len(), spawns);
    assert!(
        f.api
            .store
            .get_ticket_assignment_operation(
                TEST_WORKSPACE_ID,
                "child-ticket-start-without-assignment",
            )
            .unwrap()
            .is_none()
    );
}

const CONNECTED_RUNTIME: &str = "connected-subject-authorization";

/// Shared signed Backend/Runtime connection. Only the supplied model is replaced;
/// temporary roots, request authentication and Controller/tool dispatch stay real.
struct ConnectedWorkerHarness {
    api: WorkspaceApi,
    runtime: worker_runtime::Runtime,
    factory: worker_runtime::worker_backend::ProfileRuntimeWorkerFactory,
    identity: RuntimeIdentityMaterial,
    backend_url: String,
    owner: String,
    materializer: worker_runtime::working_directory::RuntimeGitMaterializer,
    authorizations: Arc<Mutex<Vec<(Value, StatusCode)>>>,
    runtime_errors: Arc<Mutex<Vec<String>>>,
    _backend_server: AuthorizationCallbackServer,
    _runtime_server: ConnectedRuntimeServer,
    _dir: tempfile::TempDir,
}
impl ConnectedWorkerHarness {
    async fn human(&self, method: reqwest::Method, path: &str, body: Value) -> (StatusCode, Value) {
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap()
            .request(method.clone(), format!("{}{path}", self.backend_url))
            .bearer_auth(&self.owner)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let bytes = response.bytes().await.unwrap();
        let body = if status == StatusCode::NO_CONTENT {
            assert!(bytes.is_empty(), "{method} {path}: HTTP204 must be empty");
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: HTTP{status}, invalid JSON: {error}; body={:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, body)
    }

    async fn start<M: agen::llm_client::LlmClient + Clone + 'static>(model: M) -> Self {
        Self::start_with_layer(model, |router| router).await
    }

    async fn start_with_layer<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        app_layer: impl FnOnce(Router) -> Router,
    ) -> Self {
        const RUNTIME: &str = CONNECTED_RUNTIME;
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
        let execution =
            worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(factory.clone())
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
                    public_key_fingerprint: workspace_identity
                        .public_key_fingerprint
                        .clone()
                        .unwrap(),
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
        let runtime_server = ConnectedRuntimeServer::start(
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
                        if !response.status().is_client_error()
                            && !response.status().is_server_error()
                        {
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
                    workspace_authorization: Some(
                        crate::hosts::WorkspaceRuntimeAuthorization::new(
                            api.store.clone(),
                            api.signing_identities.clone(),
                            backend_url.clone(),
                            binding,
                        ),
                    ),
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
        let backend_server = AuthorizationCallbackServer::start(backend_listener, app_layer(app));
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "connected-authorization-owner")
                .await;

        Self {
            api,
            runtime,
            factory,
            identity,
            backend_url,
            owner,
            materializer,
            authorizations,
            runtime_errors,
            _backend_server: backend_server,
            _runtime_server: runtime_server,
            _dir: dir,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn connected_read_only_subject_delegates_editing_profile_to_real_runtime_read_but_not_write()
{
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;

    const RUNTIME: &str = CONNECTED_RUNTIME;
    let (completed, mut completion) = tokio::sync::mpsc::channel(4);
    let harness = ConnectedWorkerHarness::start(ConnectedAuthorizationModel { completed }).await;
    let api = &harness.api;
    let identity = &harness.identity;
    let backend_url = &harness.backend_url;
    let owner = &harness.owner;
    let materializer = &harness.materializer;
    let authorizations = &harness.authorizations;
    let runtime_errors = &harness.runtime_errors;
    install_subject_profiles(api, "workspace_write");
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
    let child_permissions = vec![read_permission.clone()];
    // Set policy before startup: the actual source Controller sees this policy,
    // not an unset body or a registry/singleton row manufactured by the test.
    let (status, body) = response(&api, human_request(&owner, "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),
        Some(json!({"operation_id":"connected-authorization-policy", "expected_change_id":null,
            "execution":(["session.record", "memory.resident.read", "memory.resident.supply", "journal.startup.read", "journal.startup.supply"].into_iter().map(|operation| subject_permission(&subject, operation)).collect::<Vec<_>>()),
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

/// Deterministic external-model substitute, not an LLM evaluation. Each real
/// Controller reads before joining the barrier, so serialized child execution
/// cannot pass. File IO and result persistence always use production tools.
#[derive(Clone)]
struct ConcurrentTicketAuthorizationModel {
    overlap: Arc<tokio::sync::Barrier>,
    completed: tokio::sync::mpsc::Sender<(String, Vec<(String, bool, String)>)>,
}

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for ConcurrentTicketAuthorizationModel {
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
            types::{ContentPart, Item},
        };
        let input = request.items.iter().find_map(|item| {
            let Item::Message { content, .. } = item else {
                return None;
            };
            content.iter().find_map(|part| {
                let ContentPart::Text { text } = part else {
                    return None;
                };
                text.strip_prefix("connected-ticket-case:")
                    .map(|json| serde_json::from_str::<Value>(json).unwrap())
            })
        });
        let end = || {
            vec![
                Event::text_block_start(0),
                Event::text_delta(0, "scripted turn finished"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ]
        };
        let Some(input) = input else {
            // Bodies may receive normal lifecycle/notification input. That must
            // never run the child script or count as a concurrent child turn.
            return Ok(Box::pin(futures::stream::iter(end().into_iter().map(Ok))));
        };
        let ticket = input["ticket"].as_str().unwrap();
        let expected = input["content"].as_str().unwrap();
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
                } if call_id.starts_with("parallel-") => Some((
                    call_id.clone(),
                    *is_error,
                    format!("{summary}; {content:?}"),
                )),
                _ => None,
            })
            .collect();
        assert!(
            results.iter().all(|(_, error, _)| !error),
            "real child tool failed: {results:?}"
        );
        let has = |id: &str| results.iter().any(|(call, _, _)| call == id);
        let events = if has("parallel-report") {
            self.completed.try_send((ticket.into(), results)).unwrap();
            end()
        } else {
            let (id, name, arguments) = if has("parallel-reread") {
                assert!(
                    results
                        .iter()
                        .find(|(id, _, _)| id == "parallel-reread")
                        .unwrap()
                        .2
                        .contains(expected),
                    "result must follow the real post-write Read: {results:?}"
                );
                (
                    "parallel-report",
                    "TicketImplementationResult",
                    json!({"ticket":ticket, "kind":"result",
                    "body":format!("Verified README.md contains {expected}"), "merge_request":null}),
                )
            } else if has("parallel-write") {
                (
                    "parallel-reread",
                    "Read",
                    json!({"target_workdir":"workdir", "file_path":"README.md"}),
                )
            } else if has("parallel-read") {
                assert!(
                    results[0].2.contains("clean"),
                    "actual initial read: {results:?}"
                );
                tokio::time::timeout(std::time::Duration::from_secs(30), self.overlap.wait())
                    .await
                    .expect("both real child turns must overlap after their initial Read");
                (
                    "parallel-write",
                    "Write",
                    json!({"target_workdir":"workdir", "file_path":"README.md", "content":expected}),
                )
            } else {
                (
                    "parallel-read",
                    "Read",
                    json!({"target_workdir":"workdir", "file_path":"README.md"}),
                )
            };
            assert!(
                request.tools.iter().any(|tool| tool.name == name),
                "Profile must expose {name}"
            );
            vec![
                Event::tool_use_start(0, id, name),
                Event::tool_input_delta(0, arguments.to_string()),
                Event::tool_use_stop(0),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn connected_two_subjects_delegate_distinct_tickets_concurrently_on_separate_branches() {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;

    struct Case {
        subject: String,
        ticket: String,
        branch: String,
        workdir: String,
        root: PathBuf,
        source: RuntimeWorkerRef,
        delegation: String,
        content: String,
        permissions: Vec<ScopedPermission>,
        change: String,
        request: Value,
    }
    let (completed, mut completion) = tokio::sync::mpsc::channel(2);
    let f = ConnectedWorkerHarness::start(ConcurrentTicketAuthorizationModel {
        overlap: Arc::new(tokio::sync::Barrier::new(2)),
        completed,
    })
    .await;
    let api = &f.api;
    install_subject_profiles(api, "workspace_write");
    let subjects = open_subjektiv_store(api).unwrap();
    let mut cases = Vec::new();
    for label in ["alpha", "beta"] {
        let branch = format!("subject-{label}-work");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(f._dir.path())
                .args(["branch", &branch])
                .status()
                .unwrap()
                .success()
        );
        let (status, created) = f
            .human(
                reqwest::Method::POST,
                &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
                json!({"runtime_id":CONNECTED_RUNTIME, "repository_key":"test-repository",
                "selector":branch, "operation_id":format!("parallel-{label}-workdir")}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let workdir = created["item"]["working_directory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = f
            .materializer
            .bind_working_directory(&workdir, None)
            .unwrap()
            .root()
            .to_path_buf();
        // Runtime materialization intentionally checks out detached. Establish
        // each working branch as fixture setup before either child starts; the
        // real Controller tools below must mutate these separate checkouts.
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["switch", "-c", &branch])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(root.join("README.md")).unwrap(),
            "clean\n"
        );
        let subject = subjects
            .create_subject(
                crate::subjektiv::SubjectRole::new(format!("parallel-{label}")).unwrap(),
            )
            .unwrap()
            .id;
        let mut input = NewTicket::new(format!("Connected {label} work"));
        set_test_ticket_target(&mut input, "test-repository", &branch);
        let ticket = browser_ticket_backend(api).unwrap().create(input).unwrap();
        let ticket_permission = |operation: &str| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Ticket {
                workspace_id: TEST_WORKSPACE_ID.into(),
                ticket_id: ticket.id.clone(),
            },
        };
        // No workspace-wide delegation grant: exactly this file in this Workdir
        // and read/result operations on this Ticket. The body stays Workdirless.
        let permissions: Vec<_> = ["filesystem.read", "filesystem.write"]
            .into_iter()
            .map(|operation| ScopedPermission {
                operation: operation.into(),
                resource: PermissionResource::Workdir {
                    working_directory_id: workdir.clone(),
                    path: "README.md".into(),
                    recursive: false,
                },
            })
            .chain([
                ticket_permission("ticket.read"),
                ticket_permission("ticket.implementation_result.write"),
            ])
            .collect();
        let mut execution: Vec<_> = [
            "session.record",
            "memory.resident.read",
            "memory.resident.supply",
            "journal.startup.read",
            "journal.startup.supply",
        ]
        .into_iter()
        .map(|operation| subject_permission(&subject, operation))
        .collect();
        execution.push(ticket_permission("ticket.assignment.assign"));
        execution.push(ScopedPermission {
            operation: "worker.create".into(),
            resource: PermissionResource::Workspace {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
        });
        let (status, policy) = f
            .human(
                reqwest::Method::POST,
                &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),
                json!({"operation_id":format!("parallel-{label}-policy"), "expected_change_id":null,
                "execution":execution, "delegation":permissions}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{policy}");
        let policy = subjects.subject_permissions(&subject).unwrap().unwrap();
        assert_eq!(policy.delegation, permissions);
        let (status, source) = f.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            json!({"runtime_id":CONNECTED_RUNTIME, "display_name":format!("{label} read-only body"),
                "profile":"project:authority-source", "initial_submit":[], "workdir_attachments":[],
                "feature_connections":{"subjektiv":{"subject_id":subject}}})).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "connect: {source}; Runtime: {:?}",
            f.runtime_errors.lock().unwrap()
        );
        let source = RuntimeWorkerRef::new(
            source["runtime_id"].as_str().unwrap(),
            source["worker_id"].as_str().unwrap(),
        );
        assert!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &source)
                .unwrap()
                .is_empty()
        );
        let lease = api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &source)
            .unwrap()
            .unwrap();
        assert_eq!(
            lease.key,
            crate::subjektiv::subject_worker_singleton_key(&subject).unwrap()
        );
        let (status, assigned) = f.human(reqwest::Method::PUT,
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/orchestrator", ticket.id),
            json!({"operation_id":format!("parallel-{label}-assign"), "expected_assignment_id":null,
                "principal":{"kind":"subject", "subject_id":subject}})).await;
        assert_eq!(status, StatusCode::OK, "{assigned}");
        assert_eq!(assigned["assignment"]["principal"]["subject_id"], subject);
        let delegation = assigned["assignment"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let content = format!("connected {label} result");
        let request = json!({"runtime_id":CONNECTED_RUNTIME,
            "display_name":format!("{label} delegated Ticket editor"), "profile":"project:authority-editor",
            "ticket_assignment":{"ticket_id":ticket.id, "operation_id":format!("parallel-{label}-start"),
                "orchestrator_assignment_id":delegation},
            "initial_submit":[{"kind":"text", "content":format!("connected-ticket-case:{}", json!({"ticket":ticket.id,"content":content}))}],
            "workdir_attachments":[{"alias":"workdir", "working_directory_id":workdir}],
            "feature_connections":{}, "control_operation_id":format!("parallel-{label}-control"),
            "execution_grant":{"permissions":permissions, "redelegation":[]}});
        cases.push(Case {
            subject,
            ticket: ticket.id,
            branch,
            workdir,
            root,
            source,
            delegation,
            content,
            permissions,
            change: policy.change_id,
            request,
        });
    }
    assert_ne!(cases[0].subject, cases[1].subject);
    assert_ne!(cases[0].ticket, cases[1].ticket);
    assert_ne!(cases[0].branch, cases[1].branch);
    assert_ne!(cases[0].workdir, cases[1].workdir);
    assert_ne!(cases[0].root, cases[1].root);
    assert_ne!(cases[0].source, cases[1].source);
    assert!(
        api.store
            .current_worker_singleton_owner(
                TEST_WORKSPACE_ID,
                crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY
            )
            .unwrap()
            .is_none()
    );

    let path = format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers");
    let (alpha, beta) = tokio::join!(
        connected_authorization_post(
            &f.backend_url,
            &f.identity,
            &cases[0].source,
            &path,
            &cases[0].request
        ),
        connected_authorization_post(
            &f.backend_url,
            &f.identity,
            &cases[1].source,
            &path,
            &cases[1].request
        ),
    );
    let mut children = Vec::new();
    for (case, (status, child)) in cases.iter().zip([alpha, beta]) {
        assert_eq!(
            status,
            StatusCode::OK,
            "signed Ticket spawn: {child}; Runtime: {:?}",
            f.runtime_errors.lock().unwrap()
        );
        let child = RuntimeWorkerRef::new(
            child["runtime_id"].as_str().unwrap(),
            child["worker_id"].as_str().unwrap(),
        );
        let grant = subjects
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(grant.subject_id, case.subject);
        assert_eq!(grant.origin_worker_id, case.source.worker_id);
        assert_eq!(grant.source_change_id, case.change);
        assert_eq!(grant.permissions, case.permissions);
        assert!(grant.redelegation.is_empty());
        assert!(
            api.store
                .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child)
                .unwrap()
                .is_none()
        );
        let assigned = api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &case.ticket)
            .unwrap()
            .unwrap();
        assert_eq!(assigned.worker, child);
        let links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &child)
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].workdir_id, case.workdir);
        children.push(child);
    }
    assert_ne!(children[0], children[1]);
    let mut completions = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let (ticket, results) =
            tokio::time::timeout(std::time::Duration::from_secs(45), completion.recv())
                .await
                .expect("both barrier-overlapped Controllers must finish")
                .expect("model completion");
        assert_eq!(
            results.len(),
            4,
            "Read, Write, Read, persisted result: {results:?}"
        );
        assert!(
            completions.insert(ticket, results).is_none(),
            "each Ticket completes once"
        );
    }
    let saved = api.config_store.with_conn(|conn| {
        let mut statement = conn.prepare("SELECT ticket_id, worker_id, result_id FROM ticket_implementation_results WHERE workspace_id=?1 ORDER BY ticket_id")?;
        Ok(statement.query_map([TEST_WORKSPACE_ID], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?)
    }).unwrap();
    assert_eq!(saved.len(), 2);
    assert_ne!(saved[0].2, saved[1].2);
    for (i, case) in cases.iter().enumerate() {
        assert!(completions.contains_key(&case.ticket));
        assert_eq!(
            fs::read_to_string(case.root.join("README.md")).unwrap(),
            case.content
        );
        let branch = std::process::Command::new("git")
            .arg("-C")
            .arg(&case.root)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert!(branch.status.success());
        assert_eq!(
            String::from_utf8(branch.stdout).unwrap().trim(),
            case.branch
        );
        let row = saved.iter().find(|row| row.0 == case.ticket).unwrap();
        assert_eq!(row.1, children[i].worker_id);
        let receipt = api
            .config_store
            .implementation_result(TEST_WORKSPACE_ID, &case.ticket, &row.2)
            .unwrap();
        assert_eq!(
            receipt.request.body,
            format!("Verified README.md contains {}", case.content)
        );
        assert!(
            api.config_store
                .implementation_result(TEST_WORKSPACE_ID, &cases[1 - i].ticket, &row.2)
                .is_err(),
            "the other Ticket cannot read this result"
        );
        let current = api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &case.ticket)
            .unwrap()
            .remove(0);
        assert_eq!(current.assignment_id, case.delegation);
        assert_eq!(
            current.principal,
            ticket::TicketAssignmentPrincipalSnapshot::Subject {
                subject_id: case.subject.clone()
            }
        );
        assert_eq!(
            current.tracking.unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Waiting,
            "child result is not a Subject self-report"
        );
        assert_eq!(
            api.authority.ticket(&case.ticket).unwrap().state,
            server_api::BrowserTicketState::Open
        );
        // Live Host policy also rejects crossing the other Subject's Workdir.
        let mut other = case.permissions[0].clone();
        other.resource = PermissionResource::Workdir {
            working_directory_id: cases[1 - i].workdir.clone(),
            path: "README.md".into(),
            recursive: false,
        };
        let (status, body) = connected_authorization_post(
            &f.backend_url,
            &f.identity,
            &children[i],
            &authorization_path(),
            &serde_json::to_value(other).unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "cross-Subject Workdir: {body}"
        );
    }
    assert_eq!(
        fs::read_to_string(f._dir.path().join("README.md")).unwrap(),
        "clean\n",
        "distinct runtime Workdirs must not mutate the repository source"
    );
}

fn attach_authorization_metadata(
    f: &Fixture,
    worker: &RuntimeWorkerRef,
    id: &str,
) -> WorkerWorkdirLinkRecord {
    // Materialization/provider execution is outside this metadata-resolution test.
    // Connection identities still come from the production attachment ledger.
    seed_cleanup_workdir_for_runtime(&f.api, id, &worker.runtime_id, "present", "clean");
    f.api
        .store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: worker.clone(),
            workdir_id: id.into(),
            alias: "checkout".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: now_registry_timestamp(),
            unlinked_at: None,
        })
        .unwrap()
}

async fn attachment_bound_check(
    f: &Fixture,
    worker: &RuntimeWorkerRef,
    alias: &str,
    expected: Option<&str>,
) -> (StatusCode, Value) {
    response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&worker.worker_id),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/authorization/bound-check"),
            serde_json::to_vec(&json!({"operation":"workdir.detach", "target":{
                "kind":"workdir_attachment", "alias":alias, "expected_connection_id":expected,
            }}))
            .unwrap(),
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bound_attachment_uses_current_source_metadata_and_policy_not_alias_as_authority() {
    let f = Fixture::new().await;
    let link = attach_authorization_metadata(&f, &f.source, "bound-subject-workdir");
    let permission = ScopedPermission {
        operation: "workdir.detach".into(),
        resource: PermissionResource::WorkdirAttachment {
            workspace_id: TEST_WORKSPACE_ID.into(),
            connection_id: link.connection_id.clone(),
        },
    };
    let (status, denied) = attachment_bound_check(&f, &f.source, "checkout", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unset: {denied}");
    assert!(denied.get("scope").is_none());
    f.assign(vec![permission.clone()], vec![permission.clone()])
        .await;
    let (status, allowed) =
        attachment_bound_check(&f, &f.source, "checkout", Some(&link.connection_id)).await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    assert_eq!(allowed, json!({"scope":permission.resource}));
    for (alias, expected) in [
        ("missing", None),
        ("checkout", Some("different-connection")),
    ] {
        let (status, body) = attachment_bound_check(&f, &f.source, alias, expected).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.get("scope").is_none());
    }
    let child = f.child("bound-attachment-child", vec![permission]).await;
    let (status, body) =
        attachment_bound_check(&f, &child, "checkout", Some(&link.connection_id)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a delegated operation cannot borrow its source's alias: {body}"
    );
    f.assign(vec![], vec![]).await;
    assert_eq!(
        attachment_bound_check(&f, &f.source, "checkout", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let retained = f
        .api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &f.source)
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].connection_id, link.connection_id);
    assert!(
        retained[0].unlinked_at.is_none(),
        "authorization alone never detaches the route"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_bound_attachment_returns_concrete_resource_and_rejects_replaced_connection() {
    let f = Fixture::new().await;
    let Json(created) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Legacy attachment owner".into(),
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
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    let first = attach_authorization_metadata(&f, &worker, "legacy-bound-workdir");
    let (status, body) =
        attachment_bound_check(&f, &worker, "checkout", Some(&first.connection_id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"scope":PermissionResource::WorkdirAttachment {
            workspace_id: TEST_WORKSPACE_ID.into(), connection_id: first.connection_id.clone(),
        }})
    );
    f.api
        .store
        .detach_worker_workdir_connection(
            TEST_WORKSPACE_ID,
            &worker,
            "checkout",
            &first.connection_id,
            &now_registry_timestamp(),
        )
        .unwrap();
    let second = attach_authorization_metadata(&f, &worker, "legacy-bound-workdir");
    assert_ne!(second.connection_id, first.connection_id);
    let (status, body) =
        attachment_bound_check(&f, &worker, "checkout", Some(&first.connection_id)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "stale connection must not select replacement: {body}"
    );
    assert!(body.get("scope").is_none());
    let (status, body) = attachment_bound_check(&f, &worker, "checkout", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scope"]["connection_id"], second.connection_id);
    assert!(
        f.api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .iter()
            .any(|link| link.connection_id == second.connection_id && link.unlinked_at.is_none())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_origin_replacement_revokes_new_operations_without_changing_frozen_request() {
    let f = Fixture::new().await;
    let permission = subject_permission(&f.subject, "memory.surface.prepare");
    f.assign(
        vec![
            permission.clone(),
            create_permission(),
            subject_permission(&f.subject, "memory.consolidation.request"),
            subject_permission(&f.subject, "memory.consolidation.supply"),
        ],
        vec![],
    )
    .await;
    let output = subjektiv_job::dispatch(&f.api, &f.subject, Some(f.source.clone()), 0, 0).unwrap();
    assert_eq!(output.status, "started");
    let job = f
        .api
        .store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(&f.subject),
        )
        .unwrap()
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap();
    let worker = attempt.worker.as_ref().unwrap();
    let needed = server_api::authorization::OperationAuthorizationRequest {
        operation: permission.operation.clone(),
        resource: permission.resource.clone(),
    };
    assert_eq!(
        subject_authorization::authorize_scope(&f.api, worker, &needed).unwrap(),
        Some(permission.resource)
    );
    let forbidden = server_api::authorization::OperationAuthorizationRequest {
        operation: "ticket.create".into(),
        resource: create_permission().resource,
    };
    assert!(
        subject_authorization::authorize_scope(&f.api, worker, &forbidden).is_err(),
        "Job capabilities must not inherit its body's unrelated Workspace permissions"
    );
    // The signed startup callback created a real, empty writer binding. Release
    // it through the domain's pending-source/generation checks before applying
    // historical disconnection input; singleton release is not writer revocation.
    // This execution fixture has no live Controller and does not prove cancellation.
    let store = open_subjektiv_store(&f.api).unwrap();
    let attribution = store
        .subject_session_attributions(&f.subject)
        .unwrap()
        .into_iter()
        .find(|attr| attr.runtime_id == f.source.runtime_id && attr.worker_id == f.source.worker_id)
        .unwrap();
    let host =
        subjektiv::journal::JournalHostContext::validated_session(&store, attribution).unwrap();
    store
        .release_journal_writer(&subjektiv::journal::JournalWriterBinding {
            subject_id: f.subject.clone(),
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone(),
            binding_id: host.writer_binding_id().unwrap().into(),
        })
        .unwrap();
    f.release_source_lease();
    let Json(replacement) = create_workspace_worker(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Replacement Job origin".into(),
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
    let replacement = RuntimeWorkerRef::new(replacement.runtime_id, replacement.worker_id);
    assert!(subject_authorization::authorize_scope(&f.api, &replacement, &needed).is_ok());
    assert!(
        subjektiv_job::active_grant(&f.api, TEST_WORKSPACE_ID, worker).is_ok(),
        "the attempt itself is still live"
    );
    assert!(
        subject_authorization::authorize_scope(&f.api, worker, &needed).is_err(),
        "a new body cannot refresh an old frozen Job origin"
    );
    let saved = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.request, job.request);
    assert_eq!(saved.request.source_worker.as_ref(), Some(&f.source));
}
