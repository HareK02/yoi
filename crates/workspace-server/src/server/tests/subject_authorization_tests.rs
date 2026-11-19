//! Normal Server library boundary tests (`subject_authorization_tests` filter).
//! Router authentication, Host creation/reservation, Subject policy storage, Worker
//! Feature installation and Ticket SQLite writes are real. Launch policies are
//! explicit owner-checked Store setup; their management HTTP router is outside
//! this suite. Subject policy edits and current signed Worker creation use real
//! routers. Migrated scoped-policy child fixtures seed retained grants separately;
//! they do not claim that the current API can issue legacy resource capabilities.
//! The original boundary
//! cases substitute WorkdirlessFixtureRuntime. The connected regression below also
//! runs a real ProfileRuntimeWorkerFactory/Controller and materialized Workdir;
//! only its external model stream is scripted. Neither fixture claims OS isolation.
use super::*;
#[path = "chat_delegation_connected_tests.rs"]
mod chat_delegation_connected;
#[path = "chat_tests.rs"]
mod chat_tests;
#[path = "journal_native_authority_tests.rs"]
mod journal_native_authority;
#[path = "memory_native_authority_tests.rs"]
mod memory_native_authority;
#[path = "native_lock_order_tests.rs"]
mod native_lock_order;
#[path = "session_registration_connected_tests.rs"]
mod session_registration_connected;
#[path = "staging_native_authority_tests.rs"]
mod staging_native_authority;
#[path = "subject_body_cleanup_connected_tests.rs"]
mod subject_body_cleanup_connected;
#[path = "subject_body_connected_tests.rs"]
mod subject_body_connected;
#[path = "subject_body_tests.rs"]
mod subject_body_tests;
#[path = "subject_handling_authority_tests.rs"]
mod subject_handling_authority;
#[path = "subject_job_model_supply_tests.rs"]
mod subject_job_model_supply;
#[path = "subject_mr_completion_admission_tests.rs"]
mod subject_mr_completion_admission;
#[path = "subject_permission_management_tests.rs"]
mod subject_permission_management;
#[path = "subject_ticket_migration_connected_tests.rs"]
mod subject_ticket_migration_connected;
#[path = "ticket_parent_connected_tests.rs"]
mod ticket_parent_connected;
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
fn query_permission() -> ScopedPermission {
    ScopedPermission {
        operation: "ticket.query".into(),
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

fn standalone_assignment(source: &str) -> feature_permissions::PermissionAssignment {
    use config_source::{
        ConfigContentType, ConfigEntry, ConfigSchemaContribution, ConfigTreeSnapshot,
        SnapshotEnvironment, ToolchainContract, VirtualPath, WorkspaceConfigSchemaBundle,
    };

    // Evaluate only a transient assignment, never a Workspace source or Subject ID.
    let path = VirtualPath::parse("assignment.dcdl").unwrap();
    let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        path.clone(),
        ConfigContentType::Decodal,
        format!("{{ assignment = {source}; }}"),
    )
    .unwrap()])
    .unwrap();
    let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
        "builtin:fixture-assignment",
        "assignment",
        "1",
        r#"{ assignment = {
            sets = [...String] default [];
            namespaces = [...{ namespace = String; keys = [...String]; }] default [];
            exclude_sets = [...String] default [];
            exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
            revoked = Bool default false;
        }; }"#,
    )
    .unwrap()])
    .unwrap();
    let evaluated = SnapshotEnvironment::new(snapshot)
        .evaluate_contract(&ToolchainContract::with_schema_bundle(
            1,
            vec![path],
            1,
            schema,
        ))
        .unwrap();
    serde_json::from_value::<crate::policy_config::AssignmentSource>(
        evaluated.projections[0].data_json["assignment"].clone(),
    )
    .unwrap()
    .assignment()
}

fn key_assignment(keys: &[&str]) -> feature_permissions::PermissionAssignment {
    server_api::authorization::CurrentSubjectPolicy::from_legacy(SubjectPermissions {
        change_id: "fixture-keys".into(),
        execution: keys.iter().map(|key| workspace_operation(key)).collect(),
        delegation: vec![],
    })
    .unwrap()
    .execution
}

async fn seed_owner_launch_policy(
    api: &WorkspaceApi,
    owner: &str,
    id: &str,
    profiles: &[&str],
    assignment: feature_permissions::PermissionAssignment,
) -> server_api::launch_policy::WorkerLaunchPolicy {
    // Explicit fixture policy through the owner-checked Store, as in the core
    // grant suite. Policy-management HTTP wiring is not this suite's boundary.
    let headers = human_request(owner, "GET", "/", None).headers().clone();
    let actor = resolve_actor(&ServerAuthApi::from(api), &headers)
        .await
        .unwrap()
        .unwrap();
    let policy = api
        .config_store
        .mutate_worker_launch_policy(
            TEST_WORKSPACE_ID,
            &actor.account_id,
            &server_api::launch_policy::WorkerLaunchPolicyMutation {
                operation_key: format!("create-{id}"),
                policy_id: id.into(),
                expected: None,
                action: server_api::launch_policy::WorkerLaunchPolicyAction::Create {
                    name: id.into(),
                    allowed_profiles: profiles.iter().map(|profile| (*profile).into()).collect(),
                    child_permissions: assignment,
                },
            },
        )
        .unwrap()
        .policy;
    // Project the newly declared usage key before the explicit Subject PUT.
    let root =
        ServerConfig::server_data_root_for_data_dir(&api.config.data_root).join("plugin-artifacts");
    api.config_store
        .sync_plugin_permission_registry(
            &root,
            TEST_WORKSPACE_ID,
            &mut api.permission_registry.write().unwrap(),
        )
        .unwrap();
    policy
}

async fn owner_subject_policy(
    api: &WorkspaceApi,
    owner: &str,
    subject: &str,
    execution: feature_permissions::PermissionAssignment,
) -> server_api::authorization::CurrentSubjectPolicy {
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permission-policy");
    let (status, before) = response(api, human_request(owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let (status, body) = response(api, human_request(owner, "PUT", &path,
        Some(json!({"operation_id":Uuid::now_v7().to_string(), "expected_change_id":before["change_id"],
            "action":{"kind":"replace", "execution":execution,
                "delegation":feature_permissions::PermissionAssignment::default()}})),
    )).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value::<server_api::subject_policy::SubjectPermissionPolicyMutationResponse>(
        body,
    )
    .unwrap()
    .policy
    .unwrap()
}

fn policy_create_request(
    runtime: &str,
    operation: &str,
    policy: &server_api::launch_policy::WorkerLaunchPolicy,
) -> Value {
    json!({"runtime_id":runtime, "display_name":"Policy-bound editor", "profile":policy.allowed_profiles[0],
        "initial_submit":[], "workdir_attachments":[], "feature_connections":{}, "control_operation_id":operation,
        "authorization":{"kind":"policy", "policy_id":policy.policy_id, "policy_authorization_key":policy.authorization_key}})
}

#[path = "core_grant_continuity_tests.rs"]
mod core_grant_continuity;

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
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "subject-authority-owner").await;
        let initial_configuration = api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        let policy_path =
            format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permission-policy");
        let (status, current) =
            response(&api, human_request(&owner, "GET", &policy_path, None)).await;
        assert_eq!(status, StatusCode::OK, "{current}");
        let (status, startup_policy) = response(
            &api,
            human_request(
                &owner,
                "PUT",
                &policy_path,
                Some(json!({
                    "operation_id": "fixture-session-setup",
                    "expected_change_id": current["change_id"],
                    "action": {
                        "kind": "replace",
                        "execution": standalone_assignment(r#"{ namespaces = [{ namespace = "session"; keys = ["session.record"]; }]; }"#),
                        "delegation": standalone_assignment("{}")
                    }
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{startup_policy}");
        let creation_context = server_request_context(
            Some(test_owner_actor()),
            None,
            None,
            None,
            &HeaderMap::new(),
        );
        let Json(created) = create_workspace_worker_authenticated(
            api.clone(),
            HeaderMap::new(),
            CreateWorkspaceWorkerRequest {
                runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                display_name: "Read-only Subject body".into(),
                singleton_key: None,
                profile: Some("project:authority-source".into()),
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: subject.clone(),
                    }),
                },
                control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
                authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                    assignment: None,
                },
            },
            None,
            Some(&creation_context),
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
        // Leave the fixture unset through authenticated management, retaining the
        // setup receipt without bypassing the canonical policy owner.
        let (status, removed) = response(
            &api,
            human_request(
                &owner,
                "PUT",
                &policy_path,
                Some(json!({
                    "operation_id": "fixture-session-policy-removal",
                    "expected_change_id": startup_policy["change_id"],
                    "action": { "kind": "remove" }
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{removed}");
        assert_eq!(removed["policy"], Value::Null);
        assert!(removed["change_id"].is_string());
        let after_setup = api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        assert_eq!(after_setup.state, initial_configuration.state);
        assert_eq!(after_setup.history_head, initial_configuration.history_head);
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
    async fn adopt_policy(&self, execution: &str, delegation: &str) {
        use server_api::subject_policy::{
            SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation,
            SubjectPermissionPolicyResponse,
        };

        let workspace = self
            .api
            .store
            .get_workspace(TEST_WORKSPACE_ID)
            .await
            .unwrap()
            .unwrap();
        let before = self
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &workspace.owner_account_id)
            .unwrap();
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permission-policy",
            self.subject
        );
        let (status, body) =
            response(&self.api, human_request(&self.owner, "GET", &path, None)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let current: SubjectPermissionPolicyResponse = serde_json::from_value(body).unwrap();
        assert!(current.can_manage);
        let request = SubjectPermissionPolicyMutation {
            operation_id: Uuid::now_v7().to_string(),
            expected_change_id: current.change_id,
            action: SubjectPermissionPolicyAction::Replace {
                execution: standalone_assignment(execution),
                delegation: standalone_assignment(delegation),
            },
        };
        let (status, body) = response(
            &self.api,
            human_request(
                &self.owner,
                "PUT",
                &path,
                Some(serde_json::to_value(request).unwrap()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["policy"].is_object(), "{body}");
        let after = self
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &workspace.owner_account_id)
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.history_head, before.history_head);
    }

    // Historical scoped restrictions are storage fixture input, not a management
    // payload. Model the adopted Server row; the native legacy writer is sealed.
    fn seed_saved_policy(
        &self,
        execution: Vec<ScopedPermission>,
        delegation: Vec<ScopedPermission>,
    ) -> SubjectPermissions {
        let legacy = SubjectPermissions {
            change_id: Uuid::now_v7().to_string(),
            execution,
            delegation,
        };
        let keyed = |lane: &[ScopedPermission]| {
            lane.iter()
                .filter(|permission| {
                    feature_permissions::PermissionKey::parse(
                        server_api::authorization::canonical_worker_operation(
                            &permission.operation,
                        )
                        .unwrap_or(&permission.operation),
                    )
                    .is_ok()
                })
                .cloned()
                .collect()
        };
        let mut current =
            server_api::authorization::CurrentSubjectPolicy::from_legacy(SubjectPermissions {
                change_id: legacy.change_id.clone(),
                execution: keyed(&legacy.execution),
                delegation: keyed(&legacy.delegation),
            })
            .unwrap();
        // Unrepresentable scalar names remain historical evidence, not live keys.
        current.legacy_constraints = Some(legacy.clone());
        let saved =
            crate::policy_config::AdoptedSubjectPolicy::from_current(&self.subject, &current);
        self.api.config_store.with_immediate_transaction(|connection| {
            connection.execute(
                "DELETE FROM subject_permission_tombstones WHERE workspace_id=?1 AND subject_id=?2",
                rusqlite::params![TEST_WORKSPACE_ID, self.subject],
            )?;
            connection.execute(
                "INSERT INTO workspace_configuration_policies(workspace_id,subject_id,policy_json) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,subject_id) DO UPDATE SET policy_json=excluded.policy_json",
                rusqlite::params![TEST_WORKSPACE_ID, self.subject, serde_json::to_string(&saved).unwrap()],
            )?;
            Ok(())
        }).unwrap();
        legacy
    }
    async fn assign(
        &self,
        execution: Vec<ScopedPermission>,
        delegation: Vec<ScopedPermission>,
    ) -> SubjectPermissions {
        // Domain-scope tests retain historical constraints. Current owner writes
        // select keys only; they cannot issue or alter those legacy restrictions.
        let previous = self
            .api
            .config_store
            .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
            .unwrap();
        self.seed_saved_policy(execution, delegation);
        let selected = self
            .api
            .config_store
            .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
            .unwrap()
            .unwrap();
        let mut retained = selected.clone();
        retained.execution = previous
            .as_ref()
            .map(|policy| policy.execution.clone())
            .unwrap_or_default();
        retained.delegation = previous
            .as_ref()
            .map(|policy| policy.delegation.clone())
            .unwrap_or_default();
        let saved =
            crate::policy_config::AdoptedSubjectPolicy::from_current(&self.subject, &retained);
        self.api.config_store.with_immediate_transaction(|connection| {
            connection.execute(
                "UPDATE workspace_configuration_policies SET policy_json=?3 WHERE workspace_id=?1 AND subject_id=?2",
                rusqlite::params![TEST_WORKSPACE_ID, self.subject, serde_json::to_string(&saved).unwrap()],
            )?;
            Ok(())
        }).unwrap();
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permission-policy",
            self.subject
        );
        let (status, body) = response(
            &self.api,
            human_request(
                &self.owner,
                "PUT",
                &path,
                Some(json!({
                    "operation_id": Uuid::now_v7().to_string(),
                    "expected_change_id": retained.change_id,
                    "action": {
                        "kind": "replace",
                        "execution": selected.execution,
                        "delegation": selected.delegation
                    }
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        open_subjektiv_store(&self.api)
            .unwrap()
            .subject_permissions(&self.subject)
            .unwrap()
            .unwrap()
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
        // Current issuance selects operation keys, not resource capabilities. Never
        // silently discard an exact legacy target; those fixtures use the retained
        // grant seed below rather than the public creation API.
        assert!(permissions.iter().all(|permission| matches!(
            &permission.resource,
            PermissionResource::Workspace { workspace_id }
                | PermissionResource::WorkerOperation { workspace_id }
                if workspace_id == TEST_WORKSPACE_ID
        )));
        let keys: Vec<_> = permissions
            .iter()
            .map(|permission| permission.operation.as_str())
            .collect();
        json!({"runtime_id":self.source.runtime_id,"display_name":"Ordinary delegated editor",
            "profile":"builtin:coder","initial_submit":[],"workdir_attachments":[],"feature_connections":{},
            "control_operation_id":operation,
            "authorization":{"kind":"assignment","assignment":key_assignment(&keys)}})
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
        let policy = self
            .api
            .config_store
            .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
            .unwrap();
        if policy.is_some_and(|policy| policy.legacy_constraints.is_some()) {
            // assign/seed_saved_policy explicitly model migrated scoped authority.
            // Its children are retained data, NOT successful current API issuance.
            return self.seed_retained_child(operation, permissions);
        }
        let (status, body) = self
            .spawn(&self.source, self.child_request(operation, permissions))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        RuntimeWorkerRef::new(
            body["runtime_id"].as_str().unwrap(),
            body["worker_id"].as_str().unwrap(),
        )
    }
    fn seed_retained_child(
        &self,
        operation: &str,
        permissions: Vec<ScopedPermission>,
    ) -> RuntimeWorkerRef {
        // This component Runtime does not execute controllers. Seed an already
        // retained identity and its historical scoped grant without calling spawn,
        // issuing a core grant, changing the Subject policy, or claiming a body.
        // The retained-grant binder still checks the exact delegation ceiling and
        // source change ID, so invalid fixture grants fail rather than widening it.
        let child = RuntimeWorkerRef::new(&self.source.runtime_id, Uuid::now_v7().to_string());
        let grant = subject_authorization::prepare_delegation(
            &self.api,
            &self.source,
            Some(&server_api::authorization::WorkerDelegationRequest {
                permissions,
                redelegation: vec![],
            }),
            &child.runtime_id,
            "builtin:coder",
            "fixture-retained-coder-digest",
            None,
            false,
        )
        .unwrap()
        .unwrap();
        subject_authorization::bind_reserved_grant(&self.api, &grant, &child.worker_id).unwrap();
        let now = now_registry_timestamp();
        self.api
            .store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: child.clone(),
                display_name: operation.into(),
                profile: Some("builtin:coder".into()),
                retention_state: "normal".into(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
        let mut summary = self.api.runtime.worker(&self.source).unwrap();
        summary.worker = child.clone();
        summary.display_name = operation.into();
        summary.label = operation.into();
        summary.profile = Some("builtin:coder".into());
        summary.singleton_key = None;
        self.runtime.workers.lock().unwrap().push(summary);
        core_worker_creation::persist_spawn_control(
            &self.api,
            &self.source,
            &child,
            &format!("fixture-retained:{operation}"),
            &child.worker_id,
            false,
        )
        .unwrap();
        assert!(
            self.api
                .config_store
                .core_worker_execution_grant(TEST_WORKSPACE_ID, &child.runtime_id, &child.worker_id)
                .unwrap()
                .is_none()
        );
        assert!(
            self.api
                .store
                .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child)
                .unwrap()
                .is_none()
        );
        child
    }
    async fn policy_child(
        &self,
        operation: &str,
        policy: &server_api::launch_policy::WorkerLaunchPolicy,
    ) -> RuntimeWorkerRef {
        let (status, body) = self
            .spawn(
                &self.source,
                policy_create_request(&self.source.runtime_id, operation, policy),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        RuntimeWorkerRef::new(
            body["runtime_id"].as_str().unwrap(),
            body["worker_id"].as_str().unwrap(),
        )
    }
    fn core_grant(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> server_api::authorization::CoreWorkerExecutionGrant {
        self.api
            .config_store
            .core_worker_execution_grant(TEST_WORKSPACE_ID, &worker.runtime_id, &worker.worker_id)
            .unwrap()
            .unwrap()
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

pub(super) fn install_read_only_subject_profile(api: &WorkspaceApi) {
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
    let request = ConfigCommitRequest {
        base_digest: state.snapshot.digest.clone(),
        entrypoints: state.contract.entrypoints.clone(),
        changes: vec![config_source::ConfigTreeChange::Update {
            path: main.path.clone(),
            expected_digest: main.content_digest.clone(),
            content,
        }],
    };
    let owner: String = api
        .config_store
        .with_conn(|connection| {
            Ok(connection.query_row(
                "SELECT owner_account_id FROM workspaces WHERE workspace_id=?1",
                [TEST_WORKSPACE_ID],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let registry = api.permission_registry.read().unwrap();
    api.config_store
        .adopt_configuration(
            TEST_WORKSPACE_ID,
            &owner,
            &crate::config_adoption::ConfigurationAdoptionRequest {
                operation_id: uuid::Uuid::new_v4().to_string(),
                expected_head: api
                    .config_store
                    .configuration_head(TEST_WORKSPACE_ID, &owner)
                    .unwrap(),
                expected_catalog_digest: crate::config_adoption::catalog_digest(registry.host())
                    .unwrap(),
                source: request,
            },
            api.config_schema_registry.compose().unwrap(),
            registry.host(),
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
            let content_type = response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            Ok(WorkspaceResponse {
                content_type,
                status,
                body: String::from_utf8(bytes.to_vec())
                    .map_err(|e| WorkspaceClientError::Request(e.to_string()))?,
            })
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixture_assignment_child_uses_current_issuance_and_rejects_excess_before_spawn() {
    let f = Fixture::new().await;
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "worker"; keys = ["worker.create"]; }]; }"#,
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.query"]; }]; }"#,
    )
    .await;
    let child = f
        .child("fixture-current-assignment", vec![query_permission()])
        .await;
    let grant = f.core_grant(&child);
    assert_eq!(grant.assignment, key_assignment(&["ticket.query"]));
    assert!(matches!(
        &grant.source,
        server_api::authorization::CoreWorkerGrantSource::Delegation {
            parent: server_api::authorization::CoreWorkerGrantParent::SubjectBody { subject_id, .. }
        } if subject_id == &f.subject
    ));
    assert!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(f.check(&child, &query_permission()).await.0, StatusCode::OK);
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let records = creation_record_counts(&f);
    let calls = f.runtime.spawn_requests().len();
    let (status, body) = f
        .spawn(
            &f.source,
            f.child_request("fixture-excess-assignment", vec![create_permission()]),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(creation_record_counts(&f), records);
    assert_eq!(f.runtime.spawn_requests().len(), calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixture_retained_child_preserves_exact_scope_without_current_api_issuance() {
    let f = Fixture::new().await;
    let allowed = seed_resource_ticket(&f, "Retained child target", "retained-child-target");
    let other = seed_resource_ticket(&f, "Other child target", "other-child-target");
    let permission = ticket_read_permission(&allowed[0]);
    let policy = f
        .assign(
            vec![workspace_operation("worker.create")],
            vec![permission.clone()],
        )
        .await;
    let calls = f.runtime.spawn_requests().len();
    let records = creation_record_counts(&f);
    let (status, body) = f
        .spawn(
            &f.source,
            f.child_request("legacy-cannot-issue-assignment", vec![]),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(creation_record_counts(&f), records);
    assert_eq!(f.runtime.spawn_requests().len(), calls);

    let child = f
        .child("retained-exact-child", vec![permission.clone()])
        .await;
    assert_eq!(
        f.runtime.spawn_requests().len(),
        calls,
        "seed is not a Runtime launch"
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let saved = store
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.permissions, vec![permission.clone()]);
    assert!(saved.redelegation.is_empty());
    assert_eq!(saved.source_change_id, policy.change_id);
    assert_eq!(store.subject_permissions(&f.subject).unwrap(), Some(policy));
    assert_eq!(f.check(&child, &permission).await.0, StatusCode::OK);
    assert_eq!(
        f.check(&child, &ticket_read_permission(&other[0])).await.0,
        StatusCode::FORBIDDEN
    );
    f.assign(vec![], vec![]).await;
    assert_eq!(f.check(&child, &permission).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        store
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap(),
        Some(saved)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_owner_adoption_authorizes_signed_ticket_effects_and_revocation_without_id_grants()
 {
    let f = Fixture::new().await;
    f.adopt_policy(r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create", "ticket.read", "ticket.query"]; }]; }"#, "{}").await;
    let policy = f
        .api
        .config_store
        .adopted_subject_policy(TEST_WORKSPACE_ID, &f.subject)
        .unwrap()
        .unwrap();
    assert!(
        policy.legacy_constraints.is_none(),
        "normal new key policy does not synthesize an all-target legacy grant"
    );
    let client = f.client(&f.source);
    let (installed, tool) =
        ticket_tool_host(&f._dir.path().join("canonical-ticket"), client.clone()).await;
    assert!(installed);
    tool.unwrap()
        .execute(
            r#"{"title":"Canonical assigned operation reaches durable Ticket storage"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(ticket_posts(&client), 1);
    let tickets = browser_ticket_backend(&f.api)
        .unwrap()
        .list(Default::default())
        .unwrap();
    assert_eq!(tickets.len(), 1);
    let reference = tickets[0].id.clone();
    let read = ticket_read_permission(&reference);
    assert_eq!(f.check(&f.source, &read).await.0, StatusCode::OK);
    let mut foreign = read.clone();
    foreign.resource = PermissionResource::Ticket {
        workspace_id: "foreign-workspace".into(),
        ticket_id: reference,
    };
    assert_eq!(f.check(&f.source, &foreign).await.0, StatusCode::FORBIDDEN);
    f.adopt_policy("{}", "{}").await;
    assert_eq!(f.check(&f.source, &read).await.0, StatusCode::FORBIDDEN);
    let (_, tool) =
        ticket_tool_host(&f._dir.path().join("canonical-revoked"), client.clone()).await;
    assert!(
        tool.unwrap()
            .execute(
                r#"{"title":"Revoked policy must not create"}"#,
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    assert_eq!(
        browser_ticket_backend(&f.api)
            .unwrap()
            .list(Default::default())
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_policy_keeps_migrated_ticket_id_limits_and_explicit_exclusions() {
    let f = Fixture::new().await;
    let a = seed_resource_ticket(&f, "Migration exact target A", "canonical-migrate-a");
    let b = seed_resource_ticket(&f, "Migration denied target B", "canonical-migrate-b");
    f.assign(vec![ticket_read_permission(&a[0])], vec![]).await;
    f.adopt_policy(r#"{ sets = ["ticket"]; }"#, "{}").await;
    let policy = f
        .api
        .config_store
        .adopted_subject_policy(TEST_WORKSPACE_ID, &f.subject)
        .unwrap()
        .unwrap();
    assert!(policy.legacy_constraints.is_some());
    for alias in &a {
        assert_eq!(
            f.check(&f.source, &ticket_read_permission(alias)).await.0,
            StatusCode::OK
        );
    }
    for alias in &b {
        assert_eq!(
            f.check(&f.source, &ticket_read_permission(alias)).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN,
        "adding a Set cannot discard migrated per-operation domain limits"
    );
    f.adopt_policy(r#"{ sets = ["ticket"]; exclude_namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; }"#, "{}").await;
    assert_eq!(
        f.check(&f.source, &ticket_read_permission(&a[0])).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_delegation_freezes_child_operations_and_rechecks_parent_current_ceiling() {
    use server_api::authorization::{CoreWorkerGrantParent, CoreWorkerGrantSource};
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "editor",
        &["builtin:coder"],
        key_assignment(&["ticket.create"]),
    )
    .await;
    let source = owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["ticket.create", "worker.create", "worker.launch.editor"]),
    )
    .await;
    let child = f.policy_child("canonical-explicit-child", &policy).await;
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.check(&child, &query_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let saved = f.core_grant(&child);
    assert_eq!(saved.assignment, policy.child_permissions);
    assert_eq!(
        saved.issuance_ceiling,
        Some(vec![
            feature_permissions::PermissionKey::parse("ticket.create").unwrap()
        ])
    );
    assert_eq!(
        saved.source,
        CoreWorkerGrantSource::LaunchPolicy {
            policy_id: policy.policy_id.clone(),
            policy_authorization_key: policy.authorization_key.clone(),
            parent: CoreWorkerGrantParent::SubjectBody {
                subject_id: f.subject.clone(),
                source_change_id: source.change_id,
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone()
            },
        }
    );
    subject_authorization::revalidate_core_grant(&f.api, &saved).unwrap();
    let client = f.client(&child);
    let (installed, tool) = ticket_tool_host(
        &f._dir.path().join("canonical-child-effect"),
        client.clone(),
    )
    .await;
    assert!(installed);
    tool.unwrap()
        .execute(
            r#"{"title":"Policy child creates only its granted operation"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(ticket_posts(&client), 1);
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["ticket.create"]),
    )
    .await;
    assert!(subject_authorization::revalidate_core_grant(&f.api, &saved).is_err());
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.core_grant(&child),
        saved,
        "revocation never rewrites the retained issuance binding"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrated_exact_keys_observe_feature_removal_without_expanding_children_or_reviving_scalars()
 {
    let f = Fixture::new().await;
    declared_ticket_children(&f, true);
    let root = workspace_operation("ticket.hoge");
    let child = workspace_operation("ticket.hoge.fuga");
    f.seed_saved_policy(
        vec![root.clone(), create_permission(), activation("mcp")],
        vec![],
    );
    assert_eq!(f.check(&f.source, &root).await.0, StatusCode::OK);
    assert_eq!(
        f.check(&f.source, &child).await.0,
        StatusCode::FORBIDDEN,
        "legacy exact keys are not silently reinterpreted as a children assignment"
    );
    f.api
        .permission_registry
        .write()
        .unwrap()
        .set_enabled(&worker::feature::FeatureId::builtin("ticket"), false)
        .unwrap();
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.check(&f.source, &activation("mcp")).await.0,
        StatusCode::FORBIDDEN,
        "scalar activation evidence cannot become a canonical Feature key"
    );
    {
        let mut registry = f.api.permission_registry.write().unwrap();
        let namespace = feature_permissions::NamespaceId::parse("ticket").unwrap();
        let handle = registry
            .namespace_handle(&worker::feature::FeatureId::builtin("ticket"), &namespace)
            .unwrap()
            .clone();
        let mut declaration = registry
            .host()
            .catalog()
            .namespaces
            .into_iter()
            .find(|entry| entry.namespace == namespace)
            .unwrap()
            .declaration;
        declaration
            .permissions
            .retain(|definition| definition.key.as_str() != "ticket.hoge");
        registry.update(&handle, declaration).unwrap();
        registry
            .set_enabled(&worker::feature::FeatureId::builtin("ticket"), true)
            .unwrap();
    }
    assert_eq!(
        f.check(&f.source, &root).await.0,
        StatusCode::FORBIDDEN,
        "unchanged legacy storage cannot keep a removed operation alive"
    );
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN,
        "an invalid historical namespace selection remains fail-closed"
    );
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
        "{}",
    )
    .await;
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_subject_scope_and_callee_service_retain_authenticated_original_subject() {
    let f = Fixture::new().await;
    let other = open_subjektiv_store(&f.api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("other-protected-subject").unwrap())
        .unwrap()
        .id;
    let reader = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "memory-reader",
        &["builtin:coder"],
        key_assignment(&["memory.resident.read"]),
    )
    .await;
    let memory = r#"{ namespaces = [{ namespace = "memory"; keys = ["memory.resident.read"]; }, { namespace = "worker"; keys = ["worker.create"]; }]; }"#;
    f.adopt_policy(memory, memory).await;
    assert_eq!(
        f.check(
            &f.source,
            &subject_permission(&f.subject, "memory.resident.read")
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.check(
            &f.source,
            &subject_permission(&other, "memory.resident.read")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = f
        .spawn(
            &f.source,
            policy_create_request(&f.source.runtime_id, "cannot-launch-without-usage", &reader),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let client = f.client(&f.source);
    let (_host, installed, service) =
        ticket_worker_host_with_service(&f._dir.path().join("callee-service"), client.clone())
            .await;
    assert!(installed);
    let service = service.unwrap();
    let denied = service.clone();
    assert!(
        tokio::task::spawn_blocking(
            move || denied.create(NewTicket::new("Caller memory rights are not Ticket rights"))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(ticket_posts(&client), 0);
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
        "{}",
    )
    .await;
    let created = tokio::task::spawn_blocking(move || {
        service.create(NewTicket::new(
            "Callee checks its own key for original source",
        ))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ticket_posts(&client), 1);
    assert_eq!(
        browser_ticket_backend(&f.api)
            .unwrap()
            .show(created.id.into())
            .unwrap()
            .meta
            .title,
        "Callee checks its own key for original source"
    );
}

fn declared_ticket_children(f: &Fixture, include_children: bool) {
    use feature_permissions::{NamespaceId, PermissionDefinition, PermissionKey};
    let mut registry = f.api.permission_registry.write().unwrap();
    let namespace = NamespaceId::parse("ticket").unwrap();
    let handle = registry
        .namespace_handle(&worker::feature::FeatureId::builtin("ticket"), &namespace)
        .unwrap()
        .clone();
    let mut declaration = registry
        .host()
        .catalog()
        .namespaces
        .into_iter()
        .find(|entry| entry.namespace == namespace)
        .unwrap()
        .declaration;
    declaration
        .permissions
        .retain(|definition| !definition.key.as_str().starts_with("ticket.hoge"));
    for (key, child) in [
        ("ticket.hoge", Some("ticket.hoge.fuga")),
        ("ticket.hoge.fuga", Some("ticket.hoge.fuga.leaf")),
        ("ticket.hoge.fuga.leaf", None),
        ("ticket.hoge.unrelated", None),
    ] {
        declaration.permissions.push(PermissionDefinition {
            key: PermissionKey::parse(key).unwrap(),
            description: "Explicit graph fixture".into(),
            children: child
                .filter(|_| include_children)
                .map(|key| PermissionKey::parse(key).unwrap())
                .into_iter()
                .collect(),
        });
    }
    registry.update(&handle, declaration).unwrap();
}

fn workspace_operation(operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_signed_multilevel_children_follow_current_edges_but_never_expand_frozen_children()
 {
    let f = Fixture::new().await;
    declared_ticket_children(&f, false);
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "graph",
        &["builtin:coder"],
        key_assignment(&["ticket.hoge"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["ticket.hoge", "worker.create", "worker.launch.graph"]),
    )
    .await;
    let root = workspace_operation("ticket.hoge");
    let descendant = workspace_operation("ticket.hoge.fuga.leaf");
    let same_prefix = workspace_operation("ticket.hoge.unrelated");
    let old_child = f.policy_child("before-graph-growth", &policy).await;
    let frozen = f.core_grant(&old_child);
    assert_eq!(f.check(&old_child, &root).await.0, StatusCode::OK);
    declared_ticket_children(&f, true);
    assert_eq!(f.check(&f.source, &descendant).await.0, StatusCode::OK);
    assert_eq!(
        f.check(&f.source, &same_prefix).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.check(&old_child, &descendant).await.0,
        StatusCode::FORBIDDEN,
        "current children do not enlarge the earlier issuance ceiling"
    );
    let current_child = f.policy_child("after-graph-growth", &policy).await;
    assert_eq!(f.check(&current_child, &descendant).await.0, StatusCode::OK);
    declared_ticket_children(&f, false);
    assert_eq!(
        f.check(&current_child, &descendant).await.0,
        StatusCode::FORBIDDEN,
        "a finite ceiling is still intersected with today's declared edges"
    );
    assert_eq!(f.check(&current_child, &root).await.0, StatusCode::OK);
    assert_eq!(f.core_grant(&old_child), frozen);
}

struct TrustedAuthorizationPlugin;
impl worker::feature::FeatureModule for TrustedAuthorizationPlugin {
    fn descriptor(&self) -> worker::feature::FeatureDescriptor {
        let mut descriptor = worker::feature::FeatureDescriptor::builtin(
            "authorization-plugin",
            "Authorization fixture",
        );
        descriptor.id = worker::feature::FeatureId::new("plugin:authorization-fixture").unwrap();
        descriptor.runtime = worker::feature::FeatureRuntimeKind::ExternalPlugin;
        descriptor
    }
    fn install(
        &self,
        _: &mut worker::feature::FeatureInstallContext<'_>,
    ) -> std::result::Result<(), worker::feature::FeatureInstallError> {
        Ok(())
    }
    fn declare_permissions(
        &self,
        registrar: &mut worker::feature::permissions::FeaturePermissionRegistrar<'_>,
    ) -> std::result::Result<(), worker::feature::FeatureInstallError> {
        use feature_permissions::*;
        let namespace = NamespaceId::parse("external_fixture").unwrap();
        let handle = registrar.namespace(namespace.clone())?;
        let key = PermissionKey::parse("external_fixture.read.deep").unwrap();
        registrar.declare(
            &handle,
            NamespaceDeclaration {
                permissions: vec![PermissionDefinition {
                    key: key.clone(),
                    description: "Trusted test Plugin".into(),
                    children: vec![],
                }],
                sets: vec![SetContribution {
                    set: PermissionSetId::parse("ticket").unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace,
                        keys: vec![key],
                    })],
                }],
            },
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_existing_set_holder_observes_trusted_plugin_add_disable_remove_without_child_expansion()
 {
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "before-plugin",
        &["builtin:coder"],
        key_assignment(&["ticket.create"]),
    )
    .await;
    f.adopt_policy(r#"{ sets = ["ticket"]; namespaces = [{ namespace = "worker"; keys = ["worker.create", "worker.launch.before-plugin"]; }]; }"#, "{}").await;
    let child = f.policy_child("before-plugin-child", &policy).await;
    let operation = workspace_operation("external_fixture.read.deep");
    assert_eq!(
        f.check(&f.source, &operation).await.0,
        StatusCode::FORBIDDEN
    );
    let plugin = worker::feature::FeatureId::new("plugin:authorization-fixture").unwrap();
    f.api
        .permission_registry
        .write()
        .unwrap()
        .add_module(&TrustedAuthorizationPlugin)
        .unwrap();
    assert_eq!(f.check(&f.source, &operation).await.0, StatusCode::OK);
    assert_eq!(f.check(&child, &operation).await.0, StatusCode::FORBIDDEN);
    for enabled in [false, true] {
        f.api
            .permission_registry
            .write()
            .unwrap()
            .set_enabled(&plugin, enabled)
            .unwrap();
        assert_eq!(
            f.check(&f.source, &operation).await.0,
            if enabled {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert_eq!(
            f.check(&f.source, &create_permission()).await.0,
            StatusCode::OK
        );
    }
    f.api
        .permission_registry
        .write()
        .unwrap()
        .remove(&plugin)
        .unwrap();
    assert_eq!(
        f.check(&f.source, &operation).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::OK
    );
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
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
        "{}",
    )
    .await;
    for permission in [create_permission()] {
        let (status, body) = f.check(&f.source, &permission).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"scope":permission.resource}),
            "authorization exposes only effective resource scope, not identities or policy/binding material"
        );
    }
    for permission in [
        activation("ticket"),
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
    // An ordinary Worker receives only its explicit owner-issued key binding.
    let Json(legacy) = create_workspace_worker_as_owner(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Explicitly assigned ordinary Worker".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: Some(standalone_assignment(
                    r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
                )),
            },
        }),
    )
    .await
    .unwrap();
    let legacy = RuntimeWorkerRef::new(legacy.runtime_id, legacy.worker_id);
    let (status, body) = f.check(&legacy, &create_permission()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"scope":create_permission().resource}),
        "ordinary execution binding exposes only the requested scope"
    );
    assert_eq!(
        f.check(&legacy, &query_permission()).await.0,
        StatusCode::FORBIDDEN,
        "owner assignment does not grant another operation"
    );
    let mut without_create = f.child_request("worker-without-create-operation", vec![]);
    without_create["authorization"] = json!({"kind":"assignment", "assignment":null});
    let (status, _) = f.spawn(&legacy, without_create).await;
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
    f.seed_saved_policy(workspace_grants.clone(), vec![]);
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
    f.seed_saved_policy(feature_names.into_iter().map(activation).collect(), vec![]);
    for permission in &requested {
        assert_eq!(
            f.check(&f.source, permission).await.0,
            StatusCode::FORBIDDEN,
            "{permission:?}"
        );
    }
    f.seed_saved_policy(requested.clone(), vec![]);
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
    f.seed_saved_policy(permissions.clone(), vec![]);
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
    f.assign(vec![create_permission()], vec![]).await;
    let body = serde_json::to_value(create_permission()).unwrap();
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
async fn only_owner_account_can_assign_shrink_and_revoke_subject_permissions() {
    let f = Fixture::new().await;
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permission-policy",
        f.subject
    );
    let (status, body) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["policy"].is_null());
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create", "ticket.query"]; }]; }"#,
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
    ).await;
    let (status, original) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::OK, "{original}");
    let foreign = seed_test_api_token(f.api.store.as_ref(), "foreign-policy-account");
    let update = json!({"operation_id":"untrusted-admin", "expected_change_id":original["change_id"],
        "action":{"kind":"replace", "execution":standalone_assignment("{}"), "delegation":standalone_assignment("{}")}});
    for method in ["GET", "PUT"] {
        let bytes = if method == "PUT" {
            serde_json::to_vec(&update).unwrap()
        } else {
            vec![]
        };
        for source in [Some(f.source.worker_id.as_str()), None] {
            let (status, body) = response(
                &f.api,
                runtime_source_request(&f.identity, source, method, &path, bytes.clone()),
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
                &path,
                (method == "PUT").then(|| update.clone()),
            ),
        )
        .await;
        assert_eq!(
            status,
            if method == "GET" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "non-owner account {method}: {body}"
        );
        if method == "GET" {
            assert_eq!(body["can_manage"], false);
            assert_eq!(body["policy"], original["policy"]);
        }
    }
    let (_, unchanged) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(unchanged["policy"], original["policy"]);
    f.adopt_policy(
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.query"]; }]; }"#,
        "{}",
    )
    .await;
    let (_, shrunk) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_ne!(shrunk["change_id"], original["change_id"]);
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body) =
        response(&f.api, human_request(&f.owner, "PUT", &path, Some(update))).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale policy replacement: {body}"
    );
    f.adopt_policy("{}", "{}").await;
    assert_eq!(
        f.check(&f.source, &query_permission()).await.0,
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
        f.seed_saved_policy(permissions.clone(), vec![]);
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
    f.assign(vec![], vec![]).await;
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
    assert!(error.contains("403"), "{error}");
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
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "generic",
        &["builtin:coder"],
        key_assignment(&["ticket.query", "ticket.create"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.generic"]),
    )
    .await;
    let child_request = policy_create_request(&f.source.runtime_id, "generic-child", &policy);
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
            let exact =
                policy_create_request(&f.source.runtime_id, "generic-must-not-allocate", &policy);
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
async fn generic_create_routes_accept_owner_and_require_worker_control_path() {
    let f = Fixture::new().await;
    let mut request = f.child_request("unused-on-generic-route", vec![]);
    request["authorization"] = json!({"kind":"assignment", "assignment":null});
    request["control_operation_id"] = json!("generic-owner-once");
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
        let records = creation_record_counts(&f);
        let runtime_calls = f.runtime.spawn_requests().len();
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
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "non-Subject {path}: {worker_child}"
        );
        assert_eq!(creation_record_counts(&f), records);
        assert_eq!(f.runtime.spawn_requests().len(), runtime_calls);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subject_control_creation_recovers_control_grant_failure_and_lost_response_without_duplicates()
 {
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "recovery",
        &["builtin:coder"],
        key_assignment(&["ticket.query", "ticket.create"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.recovery"]),
    )
    .await;
    let request = policy_create_request(&f.source.runtime_id, "control-grant-recovery", &policy);
    let mut missing_id = request.clone();
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
    let (status, body) = f.spawn(&f.source, request.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let child = f
        .runtime
        .workers
        .lock()
        .unwrap()
        .iter()
        .find(|worker| worker.worker != f.source)
        .unwrap()
        .worker
        .clone();
    let saved = f.core_grant(&child);
    assert_eq!(f.runtime.workers.lock().unwrap().len(), 2);
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
    let calls = f.runtime.spawn_requests().len();
    for _ in 0..2 {
        let (status, body) = f.spawn(&f.source, request.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["worker_id"], child.worker_id);
        assert_eq!(f.core_grant(&child), saved);
        let grants = f
            .api
            .store
            .list_active_worker_control_grants(TEST_WORKSPACE_ID, &f.source, 10)
            .unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].subject, child);
        assert_eq!(
            f.runtime.spawn_requests().len(),
            calls,
            "completed retry must not dispatch another creation"
        );
    }
    let records = creation_record_counts(&f);
    let mut conflict = request;
    conflict["display_name"] = json!("conflicting retry");
    assert!(!f.spawn(&f.source, conflict).await.0.is_success());
    assert_eq!(creation_record_counts(&f), records);
    assert_eq!(f.runtime.spawn_requests().len(), calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_delegation_creates_workdirless_ticketless_editor_and_replays_one_worker_and_grant()
 {
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "ordinary-editor",
        &["builtin:coder"],
        key_assignment(&["ticket.create"]),
    )
    .await;
    let query_policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "query-editor",
        &["builtin:coder"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    let source_policy = owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.ordinary-editor"]),
    )
    .await;
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::FORBIDDEN,
        "policy use permission does not grant its child operations to the body"
    );
    let child = f.policy_child("ordinary-editor-once", &policy).await;
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
    let grant = f.core_grant(&child);
    assert_eq!(grant.assignment, policy.child_permissions);
    assert_eq!(
        grant.source,
        server_api::authorization::CoreWorkerGrantSource::LaunchPolicy {
            policy_id: policy.policy_id.clone(),
            policy_authorization_key: policy.authorization_key.clone(),
            parent: server_api::authorization::CoreWorkerGrantParent::SubjectBody {
                subject_id: f.subject.clone(),
                source_change_id: source_policy.change_id,
                runtime_id: f.source.runtime_id.clone(),
                worker_id: f.source.worker_id.clone()
            },
        }
    );
    assert_eq!(grant.profile, "builtin:coder");
    assert!(!grant.profile_digest.is_empty());
    let (status, replay) = f
        .spawn(
            &f.source,
            policy_create_request(&f.source.runtime_id, "ordinary-editor-once", &policy),
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
    assert_eq!(f.core_grant(&child), grant);
    let mut changed = policy_create_request(&f.source.runtime_id, "ordinary-editor-once", &policy);
    changed["display_name"] = json!("changed replay payload");
    let (status, body) = f.spawn(&f.source, changed).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "changed payload must be a known refusal, not an unknown Runtime outcome: {body}"
    );
    assert_eq!(f.core_grant(&child), grant);
    assert_eq!(f.runtime.spawn_requests().len(), replay_calls);
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::OK
    );
    let (status, body) = f
        .spawn(
            &child,
            policy_create_request(&child.runtime_id, "undelegatable-grandchild", &query_policy),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "child has no worker.create or query policy use grant: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegated_creation_rejects_excess_permissions_and_body_identity_before_runtime_spawn() {
    let f = Fixture::new().await;
    let reader = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "restricted-reader",
        &["builtin:coder"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    let writer = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "ungranted-writer",
        &["builtin:coder"],
        key_assignment(&["ticket.create"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.restricted-reader"]),
    )
    .await;
    let mut singleton =
        policy_create_request(&f.source.runtime_id, "singleton-escalation", &reader);
    singleton["singleton_key"] = json!("model-chosen-singleton");
    let mut body_attachment =
        policy_create_request(&f.source.runtime_id, "subject-escalation", &reader);
    body_attachment["feature_connections"] = json!({"subjektiv":{"subject_id":f.subject}});
    let mut spoof = policy_create_request(&f.source.runtime_id, "origin-escalation", &reader);
    spoof["authorization"]["subject_id"] = json!(f.subject);
    let records = creation_record_counts(&f);
    for request in [
        singleton,
        body_attachment,
        policy_create_request(&f.source.runtime_id, "ungranted-policy-escalation", &writer),
        spoof,
    ] {
        let (status, body) = f.spawn(&f.source, request).await;
        assert!(status.is_client_error(), "{status}: {body}");
        assert_eq!(creation_record_counts(&f), records);
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
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "revocable",
        &["builtin:coder"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.revocable"]),
    )
    .await;
    let child = f.policy_child("revocable-child", &policy).await;
    assert_eq!(f.check(&child, &query_permission()).await.0, StatusCode::OK);
    let before = f.core_grant(&child);
    owner_subject_policy(&f.api, &f.owner, &f.subject, key_assignment(&[])).await;
    assert_eq!(
        f.check(&child, &query_permission()).await.0,
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
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "revoked authority cannot be restored: {body}"
    );
    assert_eq!(
        f.check(&child, &query_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.core_grant(&child), before);
    // Regranting identical permissions changes the assignment identity. Old
    // grants remain revoked rather than refreshing during restore/reconnect.
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.revocable"]),
    )
    .await;
    assert_eq!(
        f.check(&child, &query_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let current = f
        .policy_child("current-before-origin-release", &policy)
        .await;
    assert_eq!(
        f.check(&current, &query_permission()).await.0,
        StatusCode::OK
    );
    let current_grant = f.core_grant(&current);
    subject_authorization::revalidate_core_grant(&f.api, &current_grant).unwrap();
    f.release_source_lease();
    subject_authorization::revalidate_core_grant(&f.api, &current_grant).unwrap();
    assert_eq!(
        f.check(&current, &query_permission()).await.0,
        StatusCode::OK,
        "body release does not revoke the Subject's continuous delegation authority"
    );
    assert_eq!(f.core_grant(&current), current_grant);
    assert_eq!(
        f.check(&f.source, &query_permission()).await.0,
        StatusCode::UNAUTHORIZED,
        "released body fails signed-source authentication instead of falling through to legacy compatibility"
    );
    let (status, _) = f
        .spawn(
            &f.source,
            policy_create_request(&f.source.runtime_id, "released-origin-spawn", &policy),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_wip_ticket_create_uses_same_signed_authorization_before_real_backend() {
    let f = Fixture::new().await;
    f.assign(vec![], vec![]).await;
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
        &route,
    )
    .unwrap();
    worker::wip::install_wip_mode_with_mounts(
        host.engine_mut(),
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
    f.assign(vec![], vec![]).await;
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
async fn policy_replacement_closes_backend_session_but_exact_replay_preserves_new_session() {
    use workdir::WorkdirSession;
    let f = Fixture::new().await;
    let root = f._dir.path().join("interruption-session");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("proof.txt"), b"retained").unwrap();
    let session = Arc::new(workdir::LocalWorkdirSession::new(
        manifest::Scope::writable(&root).unwrap(),
        root.clone(),
    ));
    f.api
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(f.source.clone(), session.clone());
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permission-policy",
        f.subject
    );
    let (_, before) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    let update = json!({"operation_id":"policy-interruption-once","expected_change_id":before["change_id"],
        "action":{"kind":"replace","execution":key_assignment(&["ticket.create"]),"delegation":key_assignment(&[])}});
    let (status, committed) = response(
        &f.api,
        human_request(&f.owner, "PUT", &path, Some(update.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert!(
        !f.api
            .workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .contains_key(&f.source),
        "committed replacement closes the actual Backend session registry"
    );
    assert!(
        session
            .read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("proof.txt").unwrap(),
                offset: 0,
                limit: 100,
                max_bytes: 4096,
            })
            .await
            .is_err(),
        "replacement closes the actual provider session, not only its registry row"
    );
    let replacement = Arc::new(workdir::LocalWorkdirSession::new(
        manifest::Scope::writable(&root).unwrap(),
        root,
    ));
    f.api
        .workdir_sessions
        .lock()
        .unwrap()
        .insert_attachment(f.source.clone(), replacement.clone());
    let (status, replayed) =
        response(&f.api, human_request(&f.owner, "PUT", &path, Some(update))).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed, committed);
    assert!(
        f.api
            .workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .contains_key(&f.source),
        "exact replay must not interrupt a new Backend session"
    );
    let read = workdir::ReadRequest {
        path: workdir::WorkdirPath::new("proof.txt").unwrap(),
        offset: 0,
        limit: 100,
        max_bytes: 4096,
    };
    assert_eq!(replacement.read(read).await.unwrap().bytes, b"retained");
    let (_, current) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(current["policy"], committed["policy"]);
    close_worker_workdir_sessions(&f.api.workdir_sessions, &f.source)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grandchild_grant_requires_immediate_parent_policy_use_not_root_subject_authority() {
    use server_api::authorization::{CoreWorkerGrantParent, CoreWorkerGrantSource};
    let f = Fixture::new().await;
    let query = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "query-child",
        &["builtin:coder"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    let create = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "create-child",
        &["builtin:coder"],
        key_assignment(&["ticket.create"]),
    )
    .await;
    let parent_policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "parent",
        &["builtin:coder"],
        key_assignment(&["worker.create", "worker.launch.query-child"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&[
            "worker.create",
            "worker.launch.parent",
            "worker.launch.create-child",
        ]),
    )
    .await;
    let parent = f.policy_child("policy-parent", &parent_policy).await;
    let (status, body) = f
        .spawn(
            &parent,
            policy_create_request(&parent.runtime_id, "allowed-grandchild", &query),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let saved = f.core_grant(&child);
    match saved.source {
        CoreWorkerGrantSource::LaunchPolicy {
            parent:
                CoreWorkerGrantParent::WorkerGrant {
                    grant_id,
                    runtime_id,
                    worker_id,
                },
            ..
        } => {
            assert_eq!(grant_id, f.core_grant(&parent).grant_id);
            assert_eq!(runtime_id, parent.runtime_id);
            assert_eq!(worker_id, parent.worker_id);
        }
        other => panic!("expected immediate parent provenance, got {other:?}"),
    }
    assert_eq!(f.check(&child, &query_permission()).await.0, StatusCode::OK);
    assert_eq!(
        f.check(&child, &create_permission()).await.0,
        StatusCode::FORBIDDEN
    );
    let calls = f.runtime.spawn_requests().len();
    let records = creation_record_counts(&f);
    let (status, body) = f
        .spawn(
            &parent,
            policy_create_request(
                &parent.runtime_id,
                "root-authority-is-not-parent-authority",
                &create,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(f.runtime.spawn_requests().len(), calls);
    assert_eq!(creation_record_counts(&f), records);
    owner_subject_policy(&f.api, &f.owner, &f.subject, key_assignment(&[])).await;
    assert_eq!(
        f.check(&child, &query_permission()).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_profile_name_with_wider_resolved_scope_cannot_rebind_existing_operation() {
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "profile-editor",
        &["project:authority-editor"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.profile-editor"]),
    )
    .await;
    let mut request = policy_create_request(&f.source.runtime_id, "profile-bound-once", &policy);
    request["profile"] = json!("project:authority-editor");
    let (status, body) = f.spawn(&f.source, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    let original = f.core_grant(&child);
    let original_bundle = f
        .runtime
        .spawn_requests()
        .last()
        .unwrap()
        .resolved_config_bundle
        .clone();
    install_subject_profiles(&f.api, "workspace_write");
    let (status, body) = f.spawn(&f.source, request.clone()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "exact replay retains the saved Profile: {body}"
    );
    assert_eq!(body["worker_id"], child.worker_id);
    assert_eq!(f.core_grant(&child), original);
    assert_eq!(
        f.runtime
            .spawn_requests()
            .last()
            .unwrap()
            .resolved_config_bundle,
        original_bundle,
        "same-name Profile edits must not replace the operation's resolved bundle"
    );
    request["control_operation_id"] = json!("profile-bound-after-edit");
    let (status, body) = f.spawn(&f.source, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "new operation may resolve the edited Profile: {body}"
    );
    let next = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    assert_ne!(next, child);
    assert_ne!(f.core_grant(&next).profile_digest, original.profile_digest);
    assert_ne!(
        f.runtime
            .spawn_requests()
            .last()
            .unwrap()
            .resolved_config_bundle,
        original_bundle
    );
    assert_eq!(f.core_grant(&child), original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_ticket_service_and_raw_backend_recheck_current_signed_source() {
    let f = Fixture::new().await;
    f.assign(vec![], vec![]).await;
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
    // A separately authenticated ordinary Worker cannot substitute its principal
    // for the signed Subject body through raw source hints.
    let Json(legacy) = create_workspace_worker_as_owner(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Raw hint target".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: None,
            },
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
    f.assign(vec![], vec![]).await;
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
async fn signed_host_keys_and_provider_scope_guard_real_local_read_write_and_revocation_before_effects()
 {
    use workdir::{ReadRequest, WorkdirPath, WriteRequest};
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "filesystem",
        &["builtin:coder"],
        key_assignment(&["filesystem.read", "filesystem.write"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&["worker.create", "worker.launch.filesystem"]),
    )
    .await;
    let child = f.policy_child("local-resource-child", &policy).await;
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
            manifest::Scope::writable(&root.path().join("src")).unwrap(),
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
            "the provider src scope must not follow a symlink outside src"
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
        json!({"scope":PermissionResource::Workdir { working_directory_id:"fixture-local-attachment".into(), path:"src/source.txt".into(), recursive:false }}),
        "a Feature key returns concrete context, not a broader resource capability"
    );
    // Internal scope partitioning must work with the real Host's request-only
    // resource response. Keep the same live authorization adapter as the parent.
    let broker = workdir::WorkdirToolBroker::new(session.clone());
    let internal = broker
        .scope(workdir::WorkdirToolScope {
            rules: vec![workdir::WorkdirToolScopeRule {
                target: WorkdirPath::new("src").unwrap(),
                permission: workdir::WorkdirToolScopePermission::Write,
                recursive: true,
                symlink_policy: Default::default(),
            }],
            cwd: WorkdirPath::new("src").unwrap(),
            command: false,
        })
        .await
        .unwrap();
    assert_eq!(
        internal
            .tool_session()
            .read(read("source.txt"))
            .await
            .unwrap()
            .bytes,
        b"source"
    );
    internal
        .tool_session()
        .write(WriteRequest {
            path: WorkdirPath::new("internal.txt").unwrap(),
            content: b"internal".to_vec(),
            expected_hash: None,
        })
        .await
        .unwrap();
    owner_subject_policy(&f.api, &f.owner, &f.subject, key_assignment(&[])).await;
    assert!(
        internal
            .tool_session()
            .read(read("source.txt"))
            .await
            .is_err()
    );
    assert!(
        internal
            .tool_session()
            .write(WriteRequest {
                path: WorkdirPath::new("after-revoke.txt").unwrap(),
                content: b"denied".to_vec(),
                expected_hash: None,
            })
            .await
            .is_err()
    );
    assert!(!root.path().join("src/after-revoke.txt").exists());
    internal.close().await.unwrap();
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
async fn delegated_subject_child_cannot_assign_existing_worker_by_omitting_subject_assignment() {
    let f = Fixture::new().await;
    let grandchild_policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "ticket-grandchild",
        &["builtin:coder"],
        key_assignment(&[]),
    )
    .await;
    let child_policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "ticket-parent",
        &["builtin:coder"],
        key_assignment(&[
            "worker.create",
            "worker.launch.ticket-grandchild",
            "ticket.assignment.assign",
        ]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&[
            "worker.create",
            "worker.launch.ticket-parent",
            "ticket.assignment.assign",
        ]),
    )
    .await;
    let request = policy_create_request(
        &f.source.runtime_id,
        "ticket-initiation-source",
        &child_policy,
    );
    let (status, created) = f.spawn(&f.source, request).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let child = RuntimeWorkerRef::new(
        created["runtime_id"].as_str().unwrap(),
        created["worker_id"].as_str().unwrap(),
    );
    // The child's ordinary redelegation remains valid: body ownership is a
    // Ticket-initiation condition, not a blanket ban on general Worker creation.
    let (status, ordinary) = f
        .spawn(
            &child,
            policy_create_request(&child.runtime_id, "ordinary-grandchild", &grandchild_policy),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ordinary}");

    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(NewTicket::new("Another Subject's Ticket"))
        .unwrap();
    assign_test_orchestrator(&f.api, &ticket.id);
    let spawns = f.runtime.spawn_requests().len();
    let request = json!({
        "operation_id":"child-ticket-start-without-assignment", "expected_assignment_id":null,
        "principal":{"kind":"worker", "runtime_id":ordinary["runtime_id"], "worker_id":ordinary["worker_id"]},
        "workdir_bindings":[],
    });
    let (status, denied) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&child.worker_id),
            "PUT",
            &format!(
                "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                ticket.id
            ),
            serde_json::to_vec(&request).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(
        denied["yoi_failure"]["category"], "authorization",
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

pub(super) const CONNECTED_RUNTIME: &str = "connected-subject-authorization";

/// Shared signed Backend/Runtime connection. Only the supplied model is replaced;
/// temporary roots, request authentication and Controller/tool dispatch stay real.
pub(super) struct ConnectedWorkerHarness {
    pub(super) api: WorkspaceApi,
    // Expose only the persisted root needed to reopen ordinary Session history.
    pub(super) runtime_root: PathBuf,
    runtime: worker_runtime::Runtime,
    factory: worker_runtime::worker_backend::ProfileRuntimeWorkerFactory,
    identity: RuntimeIdentityMaterial,
    backend_url: String,
    pub(super) owner: String,
    materializer: worker_runtime::working_directory::RuntimeGitMaterializer,
    authorizations: Arc<Mutex<Vec<(Value, StatusCode)>>>,
    runtime_errors: Arc<Mutex<Vec<String>>>,
    runtime_startup_failures: Arc<Mutex<Vec<runtime_api::WorkerStartupFailure>>>,
    _backend_server: AuthorizationCallbackServer,
    _runtime_server: ConnectedRuntimeServer,
    _dir: tempfile::TempDir,
}
impl ConnectedWorkerHarness {
    /// Exercise the authenticated Browser TCP/HTTP boundary without discarding
    /// media type or rewriting its body. Bound both elapsed time and streamed bytes;
    /// redirects/retries must not turn a failed create response into another POST.
    pub(super) async fn human_raw(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
        let mut response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap()
            .request(method, format!("{}{path}", self.backend_url))
            .bearer_auth(&self.owner)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let headers = response.headers().clone();
        assert!(
            response
                .content_length()
                .is_none_or(|len| len <= RESPONSE_LIMIT as u64)
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.unwrap() {
            assert!(
                chunk.len() <= RESPONSE_LIMIT - bytes.len(),
                "Browser response exceeds test limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        (status, headers, bytes)
    }

    pub(super) async fn human(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let (status, _, bytes) = self.human_raw(method.clone(), path, body).await;
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

    /// Keep the existing signed RuntimeHTTP assembly, adding only the production
    /// Plugin bridge and a TLS-peer transport on the Server before either router
    /// can handle a request. No guest, delivery or owner implementation is replaced.
    pub(super) async fn start_with_plugins<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        transport: Arc<crate::plugin_https::HttpsTransport>,
    ) -> Self {
        Self::start_configured(model, Some(transport), |router| router).await
    }

    async fn start_with_layer<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        app_layer: impl FnOnce(Router) -> Router,
    ) -> Self {
        Self::start_configured(model, None, app_layer).await
    }

    async fn start_with_database<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        prepare: impl FnOnce(&ServerConfig),
    ) -> Self {
        Self::start_prepared(model, None, |router| router, prepare).await
    }

    async fn start_configured<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        plugin_https_transport: Option<Arc<crate::plugin_https::HttpsTransport>>,
        app_layer: impl FnOnce(Router) -> Router,
    ) -> Self {
        Self::start_prepared(model, plugin_https_transport, app_layer, |_| {}).await
    }

    async fn start_prepared<M: agen::llm_client::LlmClient + Clone + 'static>(
        model: M,
        plugin_https_transport: Option<Arc<crate::plugin_https::HttpsTransport>>,
        app_layer: impl FnOnce(Router) -> Router,
        prepare: impl FnOnce(&ServerConfig),
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

        let mut config = test_server_config(dir.path());
        config.backend_base_url = Some(backend_url.clone());
        prepare(&config);
        // The embedded Runtime is unused. Connected Workers execute on the real
        // Runtime assembled below, not on this Host fixture's embedded backend.
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let mut api = WorkspaceApi::new_with_execution_backend(
            config,
            control,
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let plugins_enabled = plugin_https_transport.is_some();
        api.plugin_https_transport = plugin_https_transport;
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
        let registration = worker_runtime::workspace_issuer::WorkspaceIssuerTrustRecord {
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
        };
        let verifier = worker_runtime::workspace_issuer::WorkspaceCapabilityVerifier::new(
            vec![registration.clone()],
            Arc::new(
                worker_runtime::workspace_issuer::InMemoryWorkspaceClaimReplayProtection::default(),
            ),
        )
        .unwrap();
        let workspace_client =
            worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                TEST_WORKSPACE_ID,
                &backend_url,
                RUNTIME,
            )
            .with_runtime_request_source(&identity, &backend_url);
        let mut factory = worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
            dir.path().join("connected-profiles"),
        )
        .with_remote_worker_mutation_identity(identity.clone())
        .with_workspace_request_client(workspace_client.clone())
        .with_runtime_store_dir(&runtime_root)
        .with_spawn_model_client_factory(move || Box::new(model.clone()));
        if plugins_enabled {
            // This is the Runtime's registered Server, also used by the HTTP
            // verifier above, not a fabricated Plugin admission response. All
            // admission/delivery/Host operations still traverse the signed client.
            let current: Arc<worker_runtime::plugin::remote::CurrentPluginServer> =
                Arc::new(move |workspace| {
                    if workspace == registration.workspace_id {
                        Ok(registration.clone())
                    } else {
                        Err(yoi_plugin_runtime::delivery::DeliveryError::Denied)
                    }
                });
            // Remote providers build the standard signed Host client, retaining
            // PLUGIN_HOST_RESPONSE_BODY_LIMIT (finite 4.5 MiB); do not substitute
            // an unbounded test client or an in-process owner fallback here.
            let providers = Arc::new(
                worker_runtime::plugin::remote::RemotePluginRuntimeProviders::new(
                    std::collections::HashMap::from([(
                        TEST_WORKSPACE_ID.to_string(),
                        workspace_client,
                    )]),
                    current,
                ),
            );
            let plugins = worker_runtime::plugin::PinnedPluginToolRuntime::new(
                providers,
                &runtime_root.with_extension("plugin-cache"),
            )
            .unwrap();
            factory = factory.with_plugin_runtime(Arc::new(plugins));
        }
        // Freeze the fully configured factory only after installing the bridge.
        let execution =
            worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(factory.clone())
                .unwrap()
                .with_working_directory_materializer(materializer.clone());
        let runtime = worker_runtime::Runtime::with_fs_store_and_execution_backend(
            worker_runtime::fs_store::FsRuntimeStoreOptions {
                root: runtime_root.clone(),
                runtime_id: RUNTIME.into(),
                display_name: None,
            },
            Arc::new(execution),
        )
        .unwrap();
        let runtime_errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_errors = runtime_errors.clone();
        let runtime_startup_failures = Arc::new(Mutex::new(Vec::new()));
        let captured_startup_failures = runtime_startup_failures.clone();
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
                    let startup_failures = captured_startup_failures.clone();
                    async move {
                        let response = next.run(request).await;
                        if !response.status().is_client_error()
                            && !response.status().is_server_error()
                        {
                            return response;
                        }
                        let (parts, body) = response.into_parts();
                        let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                        let content_type = parts
                            .headers
                            .get(axum::http::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("");
                        let body: Value = serde_json::from_slice(&bytes).unwrap();
                        let message = if content_type == runtime_api::startup::PROBLEM_CONTENT_TYPE
                        {
                            use runtime_api::startup::{
                                DecodeStartupProblem, StartupBoundary, decode_startup_problem,
                            };
                            if let DecodeStartupProblem::Known(failure) = decode_startup_problem(
                                &bytes,
                                parts.status.as_u16(),
                                content_type,
                                StartupBoundary::Runtime,
                            ) {
                                startup_failures
                                    .lock()
                                    .unwrap()
                                    .push(failure.startup().clone());
                            } else {
                                // Cleanup probes also use Control and Workdir management
                                // Problems; validate them without counting another startup failure.
                                assert!(
                                    matches!(
                                        api_macros::http::decode_problem_response::<
                                            runtime_api::control_failure::RuntimeControlCause,
                                        >(
                                            &bytes,
                                            parts.status.as_u16(),
                                            content_type,
                                            &runtime_api::control_failure::ControlBoundary,
                                            error_contract::Budget::new(16384, 16384),
                                        ),
                                        error_contract::DecodeFailure::Known(_)
                                    ) || matches!(
                                        api_macros::http::decode_problem_response::<
                                            workdir::management_failure::WorkdirManagementFailure,
                                        >(
                                            &bytes,
                                            parts.status.as_u16(),
                                            content_type,
                                            &runtime_api::management_failure::ManagementBoundary,
                                            error_contract::Budget::new(16384, 16384),
                                        ),
                                        error_contract::DecodeFailure::Known(_)
                                    ),
                                    "RuntimeHTTP must emit a valid Startup, Control or Workdir management Problem"
                                );
                            }
                            body["detail"].as_str().expect("Problem detail")
                        } else {
                            body["error"]["message"]
                                .as_str()
                                .unwrap_or("unknown fixture error")
                        };
                        errors.lock().unwrap().push(message.to_owned());
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
            runtime_root,
            runtime,
            factory,
            identity,
            backend_url,
            owner,
            materializer,
            authorizations,
            runtime_errors,
            runtime_startup_failures,
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
    let policy = seed_owner_launch_policy(
        api,
        owner,
        "authority-editor",
        &["project:authority-editor"],
        key_assignment(&["filesystem.read"]),
    )
    .await;
    // Explicit startup and launch-use keys; the body itself has no filesystem key.
    owner_subject_policy(
        api,
        owner,
        &subject,
        key_assignment(&[
            "session.record",
            "memory.resident.read",
            "memory.resident.supply",
            "journal.startup.read",
            "journal.startup.supply",
            "worker.create",
            "worker.launch.authority-editor",
        ]),
    )
    .await;
    let Json(source) = create_workspace_worker_as_owner(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: RUNTIME.into(),
            display_name: "Connected read-only Subject body".into(),
            singleton_key: None,
            profile: Some("project:authority-source".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.clone(),
                }),
            },
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: None,
            },
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
        "body cannot execute the child policy's filesystem key: {body}"
    );

    // The normal noBash companion has no direct WorkerSpawn tool. Exercise its
    // signed source CreateWorkspaceWorkerRequest transport explicitly, as opposed
    // to installing a test-only privileged Feature or forging a child grant row.
    let request = CreateWorkspaceWorkerRequest {
        runtime_id: RUNTIME.into(),
        display_name: "Connected ordinary editing Profile child".into(),
        singleton_key: None,
        profile: Some("project:authority-editor".into()),
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
        authorization: server_api::launch_policy::WorkerCreationAuthorization::Policy {
            policy_id: policy.policy_id.clone(),
            policy_authorization_key: policy.authorization_key.clone(),
        },
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
    let grant = api
        .config_store
        .core_worker_execution_grant(
            TEST_WORKSPACE_ID,
            &child_ref.runtime_id,
            &child_ref.worker_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(grant.assignment, policy.child_permissions);
    assert!(
        matches!(&grant.source,server_api::authorization::CoreWorkerGrantSource::LaunchPolicy {
        parent:server_api::authorization::CoreWorkerGrantParent::SubjectBody {subject_id,worker_id,..},..
    } if subject_id==&subject && worker_id==&source.worker_id)
    );
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
    // Check the same child's live Host policy independently of Tool failure
    // wording, then verify the real materialization was not modified.
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
    delegated: tokio::sync::mpsc::Sender<Value>,
    reported: Arc<Mutex<std::collections::BTreeSet<String>>>,
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
        let delegation = request.items.iter().find_map(|item| {
            let Item::Message { content, .. } = item else {
                return None;
            };
            content.iter().find_map(|part| {
                let ContentPart::Text { text } = part else {
                    return None;
                };
                text.strip_prefix("connected-ticket-delegation:")
                    .map(|json| serde_json::from_str::<Value>(json).unwrap())
            })
        });
        if let Some(input) = delegation {
            let result = |id: &str| {
                request.items.iter().find_map(|item| match item {
                    Item::ToolResult {
                        call_id,
                        is_error,
                        content,
                        summary,
                        ..
                    } if call_id == id => {
                        assert!(!is_error, "public {id} failed: {summary}; {content:?}");
                        Some(
                            serde_json::from_str::<Value>(content.as_ref().unwrap_or(summary))
                                .unwrap(),
                        )
                    }
                    _ => None,
                })
            };
            let ticket = input["assign"]["ticket"].as_str().unwrap();
            let tool = if let Some(sent) = result("delegate-send") {
                if self.reported.lock().unwrap().insert(ticket.into()) {
                    self.delegated
                        .try_send(
                            json!({"ticket":ticket, "assigned":result("delegate-assign").unwrap(),
                        "sent":sent}),
                        )
                        .unwrap();
                }
                return Ok(Box::pin(futures::stream::iter(end().into_iter().map(Ok))));
            } else if let Some(replay) = result("delegate-replay") {
                assert_eq!(
                    replay,
                    result("delegate-assign").unwrap(),
                    "exact claim replay must preserve its receipt"
                );
                ("delegate-send", "WorkerSendInput", input["send"].clone())
            } else if result("delegate-assign").is_some() {
                (
                    "delegate-replay",
                    "TicketWorkerAssign",
                    input["assign"].clone(),
                )
            } else {
                (
                    "delegate-assign",
                    "TicketWorkerAssign",
                    input["assign"].clone(),
                )
            };
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool_definition| tool_definition.name == tool.1),
                "Profile must expose {}",
                tool.1
            );
            return Ok(Box::pin(futures::stream::iter(
                vec![
                    Event::tool_use_start(0, tool.0, tool.1),
                    Event::tool_input_delta(0, tool.2.to_string()),
                    Event::tool_use_stop(0),
                ]
                .into_iter()
                .map(Ok),
            )));
        }
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
        assignment: feature_permissions::PermissionAssignment,
        change: String,
        request: Value,
    }
    let (completed, mut completion) = tokio::sync::mpsc::channel(2);
    let (delegated, mut delegations) = tokio::sync::mpsc::channel(2);
    let f = ConnectedWorkerHarness::start(ConcurrentTicketAuthorizationModel {
        overlap: Arc::new(tokio::sync::Barrier::new(2)),
        delegated,
        reported: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
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
        // Key permissions are not resource capabilities. Actual Ticket assignment
        // and the single Runtime attachment bind each child to its own work.
        let assignment = key_assignment(&[
            "filesystem.read",
            "filesystem.write",
            "ticket.read",
            "ticket.implementation_result.write",
            "workdir.detach",
        ]);
        let launch = seed_owner_launch_policy(
            api,
            &f.owner,
            &format!("parallel-{label}"),
            &["project:authority-editor"],
            assignment.clone(),
        )
        .await;
        let policy = owner_subject_policy(
            api,
            &f.owner,
            &subject,
            key_assignment(&[
                "session.record",
                "memory.resident.read",
                "memory.resident.supply",
                "journal.startup.read",
                "journal.startup.supply",
                "ticket.assignment.assign",
                "worker.create",
                "worker.send_input",
                &format!("worker.launch.parallel-{label}"),
            ]),
        )
        .await;
        let (status, source) = f.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            json!({"runtime_id":CONNECTED_RUNTIME, "display_name":format!("{label} read-only body"),
                "profile":"project:authority-source", "initial_submit":[], "workdir_attachments":[],
                "feature_connections":{"subjektiv":{"subject_id":subject}},
                "control_operation_id":format!("parallel-{label}-body"),
                "authorization":{"kind":"assignment", "assignment":null}})).await;
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
            "initial_submit":[],
            "workdir_attachments":[{"alias":"workdir", "working_directory_id":workdir}],
            "feature_connections":{}, "control_operation_id":format!("parallel-{label}-control"),
            "authorization":{"kind":"policy", "policy_id":launch.policy_id, "policy_authorization_key":launch.authorization_key}});
        cases.push(Case {
            subject,
            ticket: ticket.id,
            branch,
            workdir,
            root,
            source,
            delegation,
            content,
            assignment,
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
            "signed empty ordinary spawn: {child}; Runtime: {:?}",
            f.runtime_errors.lock().unwrap()
        );
        let child = RuntimeWorkerRef::new(
            child["runtime_id"].as_str().unwrap(),
            child["worker_id"].as_str().unwrap(),
        );
        let grant = api
            .config_store
            .core_worker_execution_grant(TEST_WORKSPACE_ID, &child.runtime_id, &child.worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(grant.assignment, case.assignment);
        assert!(
            matches!(&grant.source,server_api::authorization::CoreWorkerGrantSource::LaunchPolicy {
            parent:server_api::authorization::CoreWorkerGrantParent::SubjectBody {subject_id,worker_id,source_change_id,..},..
        } if subject_id==&case.subject && worker_id==&case.source.worker_id && source_change_id==&case.change)
        );
        assert!(
            api.store
                .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &case.ticket)
                .unwrap()
                .is_none(),
            "ordinary creation must not claim the Ticket"
        );
        assert_eq!(
            api.runtime.worker(&child).unwrap().state,
            "idle",
            "empty spawn must not start a turn"
        );
        assert_eq!(
            fs::read_to_string(case.root.join("README.md")).unwrap(),
            "clean\n"
        );
        let links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &child)
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].workdir_id, case.workdir);
        children.push(child);
    }
    assert_ne!(children[0], children[1]);
    // Workdir identities come from the authoritative live links, not a guessed
    // alias or a key permission. The Subject then invokes real public Tools:
    // independent assignment (and exact replay) followed by typed user input.
    for (case, child) in cases.iter().zip(&children) {
        let links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, child)
            .unwrap();
        let bindings: Vec<_> = links.iter().map(|link| json!({
            "alias":link.alias, "working_directory_id":link.workdir_id, "connection_id":link.connection_id,
        })).collect();
        let content = format!(
            "connected-ticket-case:{}",
            json!({"ticket":case.ticket,"content":case.content})
        );
        let input = json!({
            "assign":{"ticket":case.ticket, "runtime_id":child.runtime_id, "worker_id":child.worker_id,
                "orchestrator_assignment_id":case.delegation, "operation_id":format!("{}-existing-worker",case.subject),
                "workdir_bindings":bindings},
            "send":{"subject":{"kind":"runtime_worker", "runtime_id":child.runtime_id,"worker_id":child.worker_id},
                "content":content,"segments":[{"kind":"text","content":content}]},
        });
        let (status, accepted) = f
            .human(
                reqwest::Method::POST,
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input",
                    case.source.runtime_id, case.source.worker_id
                ),
                json!({"kind":"user", "content":format!("connected-ticket-delegation:{input}")}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{accepted}");
    }
    for _ in 0..2 {
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(45), delegations.recv())
            .await
            .expect("public assignment and input must finish")
            .expect("delegation receipt");
        let case = cases
            .iter()
            .find(|case| case.ticket == receipt["ticket"])
            .unwrap();
        let child = &children[cases
            .iter()
            .position(|other| other.ticket == case.ticket)
            .unwrap()];
        let assigned = api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &case.ticket)
            .unwrap()
            .unwrap();
        assert_eq!(&assigned.worker, child);
        assert_eq!(
            receipt["assigned"]["assignment"]["assignment_id"],
            assigned.assignment_id
        );
        let operation = api
            .store
            .get_ticket_assignment_operation(
                TEST_WORKSPACE_ID,
                &format!("{}-existing-worker", case.subject),
            )
            .unwrap()
            .unwrap();
        assert_eq!(operation.worker.as_ref(), Some(child));
        assert_eq!(
            operation.assignment_id.as_deref(),
            Some(assigned.assignment_id.as_str())
        );
    }
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
        // A key is not ownership: resolve the authenticated child's attachment.
        let own_link = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &children[i])
            .unwrap()
            .remove(0);
        let foreign_link = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &children[1 - i])
            .unwrap()
            .remove(0);
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/authorization/bound-check");
        for (expected_connection_id, expected) in [
            (&own_link.connection_id, StatusCode::OK),
            (&foreign_link.connection_id, StatusCode::FORBIDDEN),
        ] {
            let (status,body)=connected_authorization_post(&f.backend_url,&f.identity,&children[i],&path,
                &json!({"operation":"workdir.detach","target":{"kind":"workdir_attachment","alias":"workdir","expected_connection_id":expected_connection_id}})).await;
            assert_eq!(
                status, expected,
                "a child cannot borrow the other child's attachment: {body}"
            );
        }
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
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "attachment",
        &["builtin:coder"],
        key_assignment(&["workdir.detach"]),
    )
    .await;
    owner_subject_policy(
        &f.api,
        &f.owner,
        &f.subject,
        key_assignment(&[
            "workdir.detach",
            "worker.create",
            "worker.launch.attachment",
        ]),
    )
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
    let child = f.policy_child("bound-attachment-child", &policy).await;
    let (status, body) =
        attachment_bound_check(&f, &child, "checkout", Some(&link.connection_id)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a delegated operation cannot borrow its source's alias: {body}"
    );
    owner_subject_policy(&f.api, &f.owner, &f.subject, key_assignment(&[])).await;
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
async fn ordinary_bound_attachment_returns_concrete_resource_and_rejects_replaced_connection() {
    let f = Fixture::new().await;
    let Json(created) = create_workspace_worker_as_owner(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Explicitly assigned attachment owner".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: Some(standalone_assignment(
                    r#"{ namespaces = [{ namespace = "workdir"; keys = ["workdir.detach"]; }]; }"#,
                )),
            },
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

/// Job dispatch still uses its real reservation, registry and attempt writes.
/// Only the embedded Runtime execution is held idle, so an unrelated external
/// model failure cannot race this suite's policy/attempt assertions.
struct HeldAuthorizationJobRuntime(WorkdirlessFixtureRuntime, worker_runtime::Runtime);
impl crate::hosts::WorkspaceWorkerRuntime for HeldAuthorizationJobRuntime {
    fn sync_config_bundle(
        &self,
        bundle: worker_runtime::config_bundle::ConfigBundle,
    ) -> crate::hosts::ConfigBundleSyncResult {
        crate::hosts::ConfigBundleSyncResult {
            state: crate::hosts::InternalWorkerOperationState::Accepted,
            availability: Some(
                self.1
                    .store_config_bundle(bundle)
                    .expect("valid Host Job configuration"),
            ),
            diagnostics: vec![],
        }
    }
    fn replace_worker_workspace_api(
        &self,
        worker_id: &str,
        workspace_api: WorkspaceApiRef,
    ) -> crate::hosts::WorkerWorkspaceApiResult {
        self.0
            .replace_worker_workspace_api(worker_id, workspace_api)
    }
    fn runtime_id(&self) -> &str {
        EMBEDDED_WORKER_RUNTIME_ID
    }
    fn runtime_summary(&self, limit: usize) -> crate::hosts::InternalRuntimeSummary {
        let mut summary = self.0.runtime_summary(limit);
        summary.runtime_id = EMBEDDED_WORKER_RUNTIME_ID.into();
        summary
    }
    fn list_hosts(&self, limit: usize) -> crate::hosts::RuntimeList<InternalHostSummary> {
        self.0.list_hosts(limit)
    }
    fn list_workers(&self, limit: usize) -> crate::hosts::RuntimeList<InternalWorkerSummary> {
        self.0.list_workers(limit)
    }
    fn worker(&self, worker_id: &str) -> crate::hosts::WorkerLookupResult {
        self.0.worker(worker_id)
    }
    fn spawn_worker(
        &self,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        let mut result = self.0.spawn_worker(binding, request);
        if let Some(worker) = &mut result.worker {
            worker.worker.runtime_id = EMBEDDED_WORKER_RUNTIME_ID.into();
        }
        for worker in self.0.workers.lock().unwrap().iter_mut() {
            worker.worker.runtime_id = EMBEDDED_WORKER_RUNTIME_ID.into();
        }
        result
    }
    fn send_input(
        &self,
        worker_id: &str,
        request: WorkerInputRequest,
    ) -> crate::hosts::WorkerInputResult {
        let run_id = request.submission_request_id.clone();
        let mut result = self.0.send_input(worker_id, request);
        result.worker.runtime_id = EMBEDDED_WORKER_RUNTIME_ID.into();
        result.runtime_run_id = run_id;
        result
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_job_intersects_current_execution_with_attempt_subject_and_job_kind() {
    let f = Fixture::new().await;
    f.api
        .runtime
        .register_or_replace(HeldAuthorizationJobRuntime(
            WorkdirlessFixtureRuntime::default(),
            worker_runtime::Runtime::new_memory(),
        ));
    f.adopt_policy(r#"{ sets = ["memory", "ticket"]; }"#, "{}")
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
    let allowed = subject_permission(&f.subject, "memory.surface.prepare");
    // Dispatch selects the embedded Job Runtime, not the body fixture's signing
    // Runtime. Exercise the production bound-operation checker with the actual
    // persisted attempt Worker; do not forge a proof using the body's identity.
    let check = |permission: &ScopedPermission| {
        subject_authorization::authorize_scope(
            &f.api,
            worker,
            &server_api::authorization::OperationAuthorizationRequest {
                operation: permission.operation.clone(),
                resource: permission.resource.clone(),
            },
        )
    };
    assert!(check(&allowed).is_ok());
    assert!(
        check(&subject_permission(
            "another-subject",
            "memory.surface.prepare"
        ))
        .is_err()
    );
    assert!(check(&create_permission()).is_err());
    assert!(check(&subject_permission(&f.subject, "journal.state.read")).is_err());
    f.adopt_policy(r#"{ sets = ["memory", "ticket"]; exclude_namespaces = [{ namespace = "memory"; keys = ["memory.surface.prepare"]; }]; }"#, "{}").await;
    assert!(check(&allowed).is_err());
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
            .unwrap()
            .unwrap()
            .request,
        job.request,
        "current policy checks do not rewrite or reissue a saved attempt request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_origin_replacement_revokes_new_operations_without_changing_frozen_request() {
    let f = Fixture::new().await;
    f.api
        .runtime
        .register_or_replace(HeldAuthorizationJobRuntime(
            WorkdirlessFixtureRuntime::default(),
            worker_runtime::Runtime::new_memory(),
        ));
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
    assert!(
        subjektiv_job::active_grant(&f.api, TEST_WORKSPACE_ID, worker).is_ok(),
        "the held attempt is live before its origin is replaced"
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
    let Json(replacement) = create_workspace_worker_as_owner(
        State(f.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Replacement Job origin".into(),
            singleton_key: None,
            profile: Some("project:authority-source".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: f.subject.clone(),
                }),
            },
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: None,
            },
        }),
    )
    .await
    .unwrap();
    let replacement = RuntimeWorkerRef::new(replacement.runtime_id, replacement.worker_id);
    assert!(subject_authorization::authorize_scope(&f.api, &replacement, &needed).is_ok());
    assert!(
        subject_authorization::authorize_job_input(&f.api, &job.request).is_err(),
        "frozen input supply must reject the old origin independently of attempt cleanup"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_native_admission_reads_reserved_authority_while_host_writer_owns_shared_connection()
{
    // Real native Feature admission and Host writer reservation, canonical policy,
    // singleton/Job checks and persisted Memory effect. Only Runtime execution is
    // held. The observer does not authorize or replace any production check.
    for job_worker in [false, true] {
        let fixture = Fixture::new().await;
        fixture
            .adopt_policy(r#"{ sets = ["memory"]; }"#, "{}")
            .await;
        let worker = if job_worker {
            fixture
                .api
                .runtime
                .register_or_replace(HeldAuthorizationJobRuntime(
                    WorkdirlessFixtureRuntime::default(),
                    worker_runtime::Runtime::new_memory(),
                ));
            let output = subjektiv_job::dispatch(
                &fixture.api,
                &fixture.subject,
                Some(fixture.source.clone()),
                0,
                0,
            )
            .unwrap();
            assert_eq!(output.status, "started");
            let job = fixture
                .api
                .store
                .find_active_backend_job_for_resource(
                    TEST_WORKSPACE_ID,
                    &crate::backend_job::subjektiv_serialization_key(&fixture.subject),
                )
                .unwrap()
                .unwrap();
            fixture
                .api
                .store
                .get_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &job.request.job_id,
                    &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
                )
                .unwrap()
                .unwrap()
                .worker
                .unwrap()
        } else {
            fixture.source.clone()
        };
        let store = open_subjektiv_store(&fixture.api).unwrap();
        let context = subjektiv::api::HostOperationContext::validated_consolidation(
            &store,
            &fixture.subject,
            Some(vec![]),
            None,
        )
        .unwrap();
        let (reserve_tx, reserve_rx) = std::sync::mpsc::sync_channel(0);
        let (owned_tx, owned_rx) = std::sync::mpsc::sync_channel(0);
        let ordinary = fixture.api.config_store.clone();
        let writer = std::thread::spawn(move || {
            reserve_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            ordinary.with_conn(|connection| {
                // The original connection mutex is held before reauthorization
                // proceeds. Its real native writer waits behind Memory's fence.
                owned_tx.send(()).unwrap();
                connection.execute_batch("BEGIN IMMEDIATE; ROLLBACK")?;
                Ok(())
            })
        });
        let owned_rx = Mutex::new(owned_rx);
        let first_admission = std::sync::Once::new();
        let effect_store = subject_authorization::memory_effect_store_after_reservation(
            &fixture.api,
            &store,
            &worker,
            &fixture.subject,
            "memory.surface.prepare",
            Arc::new(move || {
                // Force the writer overlap for the mutation only. Its returned
                // resident surface now performs a separate protected admission.
                first_admission.call_once(|| {
                    reserve_tx.send(()).unwrap();
                    owned_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                });
            }),
        );
        let result = subjektiv::api::execute(
            &effect_store,
            &context,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                serde_json::from_value(json!({})).unwrap(),
            ),
        );
        // Join before assertions so the waiting writer is recovered even on an
        // effect error. An ordinary-connection reentry makes this writer BUSY.
        let writer_result = writer.join().unwrap();
        assert!(writer_result.is_ok(), "job={job_worker}: {writer_result:?}");
        assert!(result.is_ok(), "job={job_worker}: {result:?}");
        fixture.adopt_policy("{}", "{}").await;
        let revoked = subject_authorization::subject_effect_store(
            &fixture.api,
            &store,
            &worker,
            &fixture.subject,
            "memory.surface.prepare",
        );
        let result = subjektiv::api::execute(
            &revoked,
            &context,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                serde_json::from_value(json!({})).unwrap(),
            ),
        );
        assert!(
            matches!(
                result,
                Err(subjektiv::api::OperationError::PermissionDenied(_))
            ),
            "current revocation must deny another effect: job={job_worker}: {result:?}"
        );
        fixture
            .api
            .feature_storage
            .open(&fixture.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                let count: i64 = connection.query_row(
                    "SELECT count(*) FROM surface_generation_runs WHERE subject_id=?1",
                    [&fixture.subject],
                    |row| row.get(0),
                )?;
                assert_eq!(
                    count, 1,
                    "only the admitted effect persists: job={job_worker}"
                );
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn memory_native_effect_rechecks_prepared_authority_and_preserves_no_write_on_revocation() {
    // Native admission component: actual owner adoption, Worker singleton,
    // registered Feature resolver and SQLite effects. Context preparation is
    // explicit; HTTP role dispatch/Job attempts are covered by the Job suite.
    for change in ["unchanged", "policy", "body"] {
        let fixture = Fixture::new().await;
        fixture.adopt_policy(r#"{ namespaces = [{ namespace = "memory"; keys = ["memory.surface.prepare"]; }]; }"#, "{}").await;
        let store = open_subjektiv_store(&fixture.api).unwrap();
        let context = subjektiv::api::HostOperationContext::validated_consolidation(
            &store,
            &fixture.subject,
            Some(vec![]),
            None,
        )
        .unwrap();
        authorize_subjektiv_operation(
            &fixture.api,
            &fixture.source,
            TEST_WORKSPACE_ID,
            &fixture.subject,
            "memory.surface.prepare",
        )
        .unwrap();
        match change {
            "policy" => fixture.adopt_policy("{}", "{}").await,
            "body" => fixture.release_source_lease(),
            _ => {}
        }
        let effect_store = subject_authorization::subject_effect_store(
            &fixture.api,
            &store,
            &fixture.source,
            &fixture.subject,
            "memory.surface.prepare",
        );
        let result = subjektiv::api::execute(
            &effect_store,
            &context,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                serde_json::from_value(json!({})).unwrap(),
            ),
        );
        if change == "unchanged" {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(
                matches!(
                    result,
                    Err(subjektiv::api::OperationError::PermissionDenied(_))
                ),
                "{change}: {result:?}"
            );
        }
        fixture
            .api
            .feature_storage
            .open(&fixture.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                let count: i64 = connection.query_row(
                    "SELECT count(*) FROM surface_generation_runs WHERE subject_id=?1",
                    [&fixture.subject],
                    |row| row.get(0),
                )?;
                assert_eq!(count, if change == "unchanged" { 1 } else { 0 });
                Ok(())
            })
            .unwrap();
    }
}
