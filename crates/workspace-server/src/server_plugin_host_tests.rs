// Component -> Wasmtime -> OwnerHost -> Server admission/config/catalog/SQLite
// receipts -> isolated TLS peer. Only the external peer's DNS/socket/trust and
// unrelated Worker execution are substituted; Worker creation, signed source
// proof verification, current Subject permissions and all Host checks are real.
// Run with `cargo test -p yoi-workspace-server --lib plugin_host_tests`.
// The shared TLS fixture requires openssl; tests never mutate process environment.
use super::*;
use crate::plugin_binding::PluginConnectionBinding;
use crate::plugin_https::{
    HttpsTransport,
    test_support::{HOST, Peer, response},
};
use crate::plugin_plan::{PlanPinState, PluginPlanCreateReceipt};
use server_api::authorization::{PermissionResource, ScopedPermission, SubjectPermissionsUpdate};
use server_api::{
    PluginConsumer, PluginHostError, PluginHostErrorCode, PluginHostOperation, PluginHostRequest,
    PluginHostResponse, PluginHttpMethod, PluginHttpRequest,
};
use yoi_plugin_runtime::host::{CancellationToken, Grants, InvocationContext};
use yoi_plugin_runtime::owner::{OwnerHost, PluginHostOwner};
use yoi_plugin_runtime::{CompiledTool, ExecutionError, Executor, Limits};

const PROFILE: &str = "project:plugin-integration";
const METHODS: &[PluginHttpMethod] = &[
    PluginHttpMethod::Get,
    PluginHttpMethod::Head,
    PluginHttpMethod::Post,
    PluginHttpMethod::Put,
    PluginHttpMethod::Patch,
    PluginHttpMethod::Delete,
    PluginHttpMethod::Options,
];

struct Fixture {
    api: WorkspaceApi,
    runtime: WorkdirlessFixtureRuntime,
    identity: RuntimeIdentityMaterial,
    source: RuntimeWorkerRef,
    subject: String,
    installed: server_api::PluginOperationResponse,
    plan: PluginPlanCreateReceipt,
    executor: Executor,
    component: CompiledTool,
    tool: &'static str,
    binding: Option<PluginConnectionBinding>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(binding: Option<PluginConnectionBinding>) -> Self {
        use manifest::plugin::*;
        let directory = tempfile::tempdir().unwrap();
        let mut api = test_api(directory.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        let runtime = WorkdirlessFixtureRuntime::default();
        api.runtime.register_or_replace(runtime.clone());
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("plugin-host-integration").unwrap())
            .unwrap()
            .id;
        let https = binding.is_some();
        let (manifest, component, tool) = if https {
            (include_str!("../../../resources/plugin/templates/rust-component-https/plugin.toml")
                .replace("methods = [\"GET\"]", "methods = [\"GET\", \"HEAD\", \"POST\", \"PUT\", \"PATCH\", \"DELETE\", \"OPTIONS\"]"),
             include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm").as_slice(),
             "example_status")
        } else {
            (
                include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml")
                    .into(),
                include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm").as_slice(),
                "example_echo",
            )
        };
        let package_dir = tempfile::tempdir().unwrap();
        fs::write(package_dir.path().join("plugin.toml"), manifest).unwrap();
        fs::write(package_dir.path().join("plugin.component.wasm"), component).unwrap();
        let limits = PluginPackageLimits::default();
        let package = pack_plugin_package(
            &read_plugin_directory(package_dir.path(), PluginSourceKind::Explicit, &limits)
                .unwrap(),
            &limits,
        )
        .unwrap();
        let upload = api
            .config_store
            .plugin_upload(
                &ServerConfig::server_data_root_for_data_dir(&api.config.data_root)
                    .join("plugin-artifacts"),
                TEST_WORKSPACE_ID,
                "fixture-owner",
                &package.bytes,
            )
            .unwrap();
        let installed = api
            .config_store
            .plugin_install(
                TEST_WORKSPACE_ID,
                "fixture-owner",
                &server_api::PluginInstallRequest {
                    upload_id: upload.upload_id,
                    alias: "integration".into(),
                    operation_key: "install".into(),
                },
            )
            .unwrap();
        commit_config(&api, &installed, tool, binding.as_ref());
        let Json(created) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                display_name: "Plugin Subject body".into(),
                singleton_key: None,
                profile: Some(PROFILE.into()),
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
        let identity = RuntimeIdentityMaterial::generate(&source.runtime_id).unwrap();
        configure_runtime_request_auth(&mut api, &identity, &source.runtime_id);
        set_policy(
            &api,
            &subject,
            &installed.installation.installation_id,
            true,
        );
        // Do not guess a Subject/Worker principal spelling. The same live identity
        // owner that production admission uses constructs the retained launch plan.
        let execution = api.plugin_execution_identity(&source).await.unwrap();
        let state = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let plan = api
            .config_store
            .with_immediate_transaction(|tx| {
                let plan = crate::plugin_settings::resolve_worker_plugin_plan(
                    tx,
                    TEST_WORKSPACE_ID,
                    &state.snapshot.digest,
                    PROFILE,
                    execution,
                )?;
                crate::plugin_plan::plugin_plan_create(tx, "fixture-host", "launch", &plan)
            })
            .unwrap();
        api.config_store
            .plugin_plan_transition(
                TEST_WORKSPACE_ID,
                &plan.plan_id,
                PlanPinState::Pending,
                PlanPinState::Active,
            )
            .unwrap();
        let executor = Executor::new(Limits::default()).unwrap();
        let component = executor
            .compile(
                &package.bytes,
                &package.package_digest,
                &package.artifact_digest,
                package.artifact_size,
            )
            .unwrap();
        Self {
            api,
            runtime,
            identity,
            source,
            subject,
            installed,
            plan,
            executor,
            component,
            tool,
            binding,
            _directory: directory,
        }
    }

    fn context(&self) -> InvocationContext {
        InvocationContext {
            workspace: TEST_WORKSPACE_ID.into(),
            installation: self.installed.installation.installation_id.clone(),
            plan: self.plan.plan_id.clone(),
            subject: self.plan.plan.execution().principal_id.clone(),
            invocation_id: String::new(),
            artifact: self.component.identity().clone(),
            tool_name: self.tool.into(),
            grants: Grants {
                logging: false,
                connection_slots: if self.binding.is_some() {
                    ["api".into()].into_iter().collect()
                } else {
                    Default::default()
                },
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(20),
            cancellation: CancellationToken::new(),
        }
    }

    fn consumer(&self, id: &str) -> PluginConsumer {
        PluginConsumer::ToolCall {
            tool_call_id: id.into(),
            tool_name: self.tool.into(),
        }
    }

    async fn verified_context(&self) -> server_api::ServerRequestContext {
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/plugin-host");
        let request = runtime_source_request(
            &self.identity,
            Some(&self.source.worker_id),
            "POST",
            &path,
            b"{}".to_vec(),
        );
        let proof = request.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
            .to_str()
            .unwrap();
        let verified = crate::worker_source::verify_runtime_request_source_proof(
            &self.api,
            proof,
            TEST_WORKSPACE_ID,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            "POST",
            &path,
            &worker_runtime::auth::request_body_digest(b"{}"),
        )
        .await
        .unwrap();
        server_request_context(
            None,
            None,
            Some(server_api::ServerRuntimeSource {
                authentication_binding_id: verified.authentication_binding_id,
                runtime_id: verified.runtime_id,
                worker_id: verified.worker_id,
            }),
            None,
            request.headers(),
        )
    }

    async fn owner(&self, transport: Option<HttpsTransport>) -> Arc<dyn PluginHostOwner> {
        let owner = plugin_host::EmbeddedPluginHostOwner::authenticated(
            Arc::new(self.api.clone()),
            TEST_WORKSPACE_ID.into(),
            Arc::new(SignedAuthorizer {
                signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(
                    &self.identity,
                ),
                worker_id: self.source.worker_id.clone(),
            }),
        );
        Arc::new(match transport {
            None => owner,
            Some(transport) => owner.with_transport(transport),
        })
    }

    async fn host(&self, owner: Arc<dyn PluginHostOwner>, call: &str) -> Arc<OwnerHost> {
        Arc::new(
            OwnerHost::begin(owner, self.context(), self.consumer(call))
                .await
                .unwrap(),
        )
    }

    async fn invoke(
        &self,
        host: Arc<OwnerHost>,
    ) -> std::result::Result<yoi_plugin_runtime::InvocationResult, ExecutionError> {
        let input = if self.binding.is_some() {
            "{}"
        } else {
            r#"{"text":"ordinary result"}"#
        };
        self.executor
            .invoke(
                &self.component,
                host.context().clone(),
                host,
                self.tool,
                input,
                "{}",
            )
            .await
    }

    fn deny(&self) {
        set_policy(
            &self.api,
            &self.subject,
            &self.installed.installation.installation_id,
            false,
        );
    }

    fn count(&self, table: &str) -> i64 {
        assert!(["plugin_invocations", "plugin_effect_receipts"].contains(&table));
        self.api
            .config_store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE workspace_id=?1"),
                    [TEST_WORKSPACE_ID],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }

    fn revoke(&self) {
        self.api
            .config_store
            .plugin_revoke(
                TEST_WORKSPACE_ID,
                "fixture-owner",
                &self.installed.installation.installation_id,
                &server_api::PluginRevokeRequest {
                    expected_last_operation: self.installed.operation_id.clone(),
                    operation_key: "revoke".into(),
                    artifact_digest: self.installed.installation.artifacts[0]
                        .artifact_digest
                        .clone(),
                    reason: "withdrawn during invocation".into(),
                },
            )
            .unwrap();
    }

    async fn shutdown(&self) {
        shutdown_workspace_feature_storage(&self.api).await.unwrap();
    }
}

fn set_policy(api: &WorkspaceApi, subject: &str, installation: &str, granted: bool) {
    let store = open_subjektiv_store(api).unwrap();
    let previous = store.subject_permissions(subject).unwrap();
    let execution = if granted {
        [
            "plugin.admit",
            "plugin.publish",
            "plugin.connection.open",
            "plugin.http.send",
            "plugin.log",
        ]
        .into_iter()
        .map(|operation| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::PluginInstallation {
                workspace_id: TEST_WORKSPACE_ID.into(),
                installation_id: installation.into(),
            },
        })
        .collect()
    } else {
        vec![]
    };
    store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: previous.map(|p| p.change_id),
                execution,
                delegation: vec![],
            },
        )
        .unwrap();
}

fn commit_config(
    api: &WorkspaceApi,
    installed: &server_api::PluginOperationResponse,
    tool: &str,
    binding: Option<&PluginConnectionBinding>,
) {
    let artifact = &installed.installation.artifacts[0];
    let plugins = json!({"enabled":{"integration":{
        "installation_id":installed.installation.installation_id,
        "expected_last_operation":installed.operation_id,
        "package_digest":artifact.package_digest,"artifact_digest":artifact.artifact_digest,
        "tools":[tool],"logging":false,"config":{},
        "connections":if binding.is_some() { json!({"api":"remote"}) } else { json!({}) }
    }},"bindings":if let Some(binding) = binding { json!({"remote":binding}) } else { json!({}) }});
    let selection = json!({"selections":[{"installation_id":installed.installation.installation_id,
        "tools":[tool],"logging":false,"https_slots":if binding.is_some() { vec!["api"] } else { vec![] }}]});
    let content = format!(
        r#"{{
        plugins = {};
        profile = {{ default_profile = "{PROFILE}"; entries = [{{
            selector = "{PROFILE}"; label = "Plugin integration"; description = "";
            profile = (import "$builtin/profiles/companion.dcdl") // {{
                slug = "plugin-integration"; scope = "workspace_read";
                delegation_scope = "workspace_read"; plugins = {};
            }};
        }}]; }};
    }}"#,
        crate::plugin_settings::render_value(&plugins).unwrap(),
        crate::plugin_settings::render_value(&selection).unwrap()
    );
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let path = config_source::VirtualPath::parse("main.dcdl").unwrap();
    commit_workspace_config_tree(
        api,
        TEST_WORKSPACE_ID,
        &ConfigCommitRequest {
            base_digest: state.snapshot.digest.clone(),
            entrypoints: state.contract.entrypoints.clone(),
            changes: vec![config_source::ConfigTreeChange::Update {
                path: path.clone(),
                expected_digest: state.snapshot.get(&path).unwrap().content_digest.clone(),
                content,
            }],
        },
    )
    .unwrap();
}

fn peer_binding(peer: &Peer) -> PluginConnectionBinding {
    let mut binding = peer.binding();
    binding.methods = METHODS.to_vec();
    binding.path_prefixes = vec!["/status".into()];
    binding
}

fn send(invocation: &str, method: PluginHttpMethod, key: &str) -> PluginHostRequest {
    PluginHostRequest::Send {
        invocation_id: invocation.into(),
        slot: "api".into(),
        request: PluginHttpRequest {
            method,
            path: "/status".into(),
            query: vec![],
            headers: vec![],
            body: vec![],
            timeout_ms: 5_000,
            effect_key: key.into(),
        },
    }
}

fn assert_denied(result: std::result::Result<PluginHostResponse, PluginHostError>) {
    assert_eq!(result.unwrap_err().code, PluginHostErrorCode::Denied);
}

#[tokio::test]
async fn component_https_result_uses_real_owner_and_durable_get_receipt() {
    let mut peer = Peer::new(
        HOST,
        vec![response(
            503,
            &[("content-type", "text/plain")],
            b"peer data",
        )],
    )
    .await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let owner = f.owner(Some(peer.transport())).await;
    let host = f.host(owner.clone(), "status-call").await;
    let invocation = host.context().invocation_id.clone();
    let result = f.invoke(host).await.unwrap();
    assert_eq!(result.output.summary, "HTTP response observed");
    assert_eq!(
        serde_json::from_str::<Value>(result.output.structured_json.as_deref().unwrap()).unwrap(),
        json!({"status":503,"body_bytes":9})
    );
    assert!(
        String::from_utf8(peer.observed().await)
            .unwrap()
            .starts_with("GET /status HTTP/1.1\r\n")
    );
    assert_eq!(f.count("plugin_effect_receipts"), 1);
    let replay = owner
        .call(send(&invocation, PluginHttpMethod::Get, "status-v1"))
        .await
        .unwrap();
    let PluginHostResponse::Response { response } = replay else {
        panic!("expected response")
    };
    assert_eq!(response.status, 503);
    assert_eq!(response.body, b"peer data");
    assert_eq!(peer.connection_count(), 1, "GET replay must not resend");
    f.shutdown().await;
}

#[tokio::test]
async fn pure_component_still_requires_current_admission_and_result_publication() {
    let f = Fixture::new(None).await;
    let host = f.host(f.owner(None).await, "pure-success").await;
    let result = f.invoke(host).await.unwrap();
    assert_eq!(result.output.summary, "example_echo echoed text");
    assert_eq!(
        serde_json::from_str::<Value>(result.output.structured_json.as_deref().unwrap()).unwrap(),
        json!({"tool":"example_echo", "text":"ordinary result"})
    );
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    let host = f.host(f.owner(None).await, "pure-denied").await;
    f.deny();
    assert!(matches!(f.invoke(host).await,
        Err(ExecutionError::Host(error)) if error.code == yoi_plugin_runtime::host::errors::Code::Denied));
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

#[tokio::test]
async fn current_permission_removal_rejects_send_and_publication_without_connecting() {
    let peer = Peer::new(HOST, vec![]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let owner = f.owner(Some(peer.transport())).await;
    let host = f.host(owner.clone(), "permission-revoked").await;
    let invocation_id = host.context().invocation_id.clone();
    f.deny();
    assert_denied(
        owner
            .call(send(&invocation_id, PluginHttpMethod::Get, "never-sent"))
            .await,
    );
    assert_denied(
        owner
            .call(PluginHostRequest::Authorize {
                invocation_id,
                operation: PluginHostOperation::Publish,
            })
            .await,
    );
    assert_eq!(peer.connection_count(), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    assert!(f.invoke(host).await.is_err());
    f.shutdown().await;
}

#[tokio::test]
async fn revoked_artifact_rejects_send_and_publication_without_connecting() {
    let peer = Peer::new(HOST, vec![]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let owner = f.owner(Some(peer.transport())).await;
    let host = f.host(owner.clone(), "artifact-revoked").await;
    let invocation_id = host.context().invocation_id.clone();
    f.revoke();
    assert_denied(
        owner
            .call(send(&invocation_id, PluginHttpMethod::Get, "never-sent"))
            .await,
    );
    assert_denied(
        owner
            .call(PluginHostRequest::Authorize {
                invocation_id,
                operation: PluginHostOperation::Publish,
            })
            .await,
    );
    assert_eq!(peer.connection_count(), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

#[tokio::test]
async fn every_http_method_replays_saved_response_and_conflicting_key_never_resends() {
    let mut peer = Peer::new(
        HOST,
        METHODS.iter().map(|_| response(200, &[], b"")).collect(),
    )
    .await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let answers = vec![vec!["8.8.8.8:443".parse().unwrap()]; METHODS.len()];
    let owner = f.owner(Some(peer.transport_with_answers(answers))).await;
    let host = f.host(owner.clone(), "all-methods").await;
    let id = &host.context().invocation_id;
    for (index, method) in METHODS.iter().enumerate() {
        let key = format!("effect-{index}");
        let request = send(id, method.clone(), &key);
        let first = owner.call(request.clone()).await.unwrap();
        let observed = String::from_utf8(peer.observed().await).unwrap();
        assert!(observed.starts_with(&format!(
            "{} /status HTTP/1.1\r\n",
            serde_json::to_value(method).unwrap().as_str().unwrap()
        )));
        assert_eq!(owner.call(request).await.unwrap(), first);
        let mut conflict = send(id, method.clone(), &key);
        if let PluginHostRequest::Send { request, .. } = &mut conflict {
            request.query.push(("different".into(), "request".into()));
        }
        assert_eq!(
            owner.call(conflict).await.unwrap_err().code,
            PluginHostErrorCode::Conflict
        );
        assert_eq!(peer.connection_count(), index + 1);
    }
    assert_eq!(f.count("plugin_effect_receipts"), METHODS.len() as i64);
    f.shutdown().await;
}

#[tokio::test]
async fn signed_route_rechecks_current_worker_policy_and_rejects_foreign_workspace() {
    let peer = Peer::new(HOST, vec![]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/plugin-host");
    let begin = PluginHostRequest::Begin {
        plan_id: f.plan.plan_id.clone(),
        installation_id: f.installed.installation.installation_id.clone(),
        consumer: f.consumer("signed-call"),
    };
    let app = build_router(f.api.clone());
    let response = app
        .clone()
        .oneshot(runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&begin).unwrap(),
        ))
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
    let PluginHostResponse::Invocation { invocation_id, .. } =
        serde_json::from_slice(&bytes).unwrap()
    else {
        panic!("expected owner invocation")
    };
    f.deny();
    for request in [
        send(&invocation_id, PluginHttpMethod::Get, "denied"),
        PluginHostRequest::Authorize {
            invocation_id,
            operation: PluginHostOperation::Publish,
        },
    ] {
        let response = app
            .clone()
            .oneshot(runtime_source_request(
                &f.identity,
                Some(&f.source.worker_id),
                "POST",
                &path,
                serde_json::to_vec(&request).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<PluginHostError>(&body)
                .unwrap()
                .code,
            PluginHostErrorCode::Denied
        );
    }
    let foreign = app
        .oneshot(runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            "/api/w/foreign/plugin-host",
            serde_json::to_vec(&begin).unwrap(),
        ))
        .await
        .unwrap();
    assert!(!foreign.status().is_success());
    assert_eq!(peer.connection_count(), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

#[tokio::test]
async fn all_methods_lost_dispatch_acknowledgement_replays_indeterminate_without_resending() {
    use crate::plugin_https::test_support::Reply;
    let mut peer = Peer::new(HOST, METHODS.iter().map(|_| Reply::Stall).collect()).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let answers = vec![vec!["8.8.8.8:443".parse().unwrap()]; METHODS.len()];
    let owner = f.owner(Some(peer.transport_with_answers(answers))).await;
    let host = f.host(owner.clone(), "lost-acknowledgements").await;
    for (index, method) in METHODS.iter().enumerate() {
        let request = send(
            &host.context().invocation_id,
            method.clone(),
            &format!("lost-{index}"),
        );
        // The real peer accepted the request; drop the owner future before its
        // response. The committed pending receipt must fence all later resends.
        {
            let attempt = owner.call(request.clone());
            tokio::pin!(attempt);
            tokio::select! {
                observed = peer.observed() => assert!(!observed.is_empty()),
                result = &mut attempt => panic!("peer must hold the response: {result:?}"),
            }
        }
        let replay = owner.call(request).await.unwrap_err();
        assert_eq!(replay.code, PluginHostErrorCode::Indeterminate);
        assert_eq!(
            replay.effect,
            server_api::PluginEffectObservation::Indeterminate
        );
        assert_eq!(peer.connection_count(), index + 1);
    }
    assert_eq!(f.count("plugin_effect_receipts"), METHODS.len() as i64);
    let pending = f
        .api
        .config_store
        .with_conn(|conn| {
            Ok(conn.query_row(
        "SELECT count(*) FROM plugin_effect_receipts WHERE workspace_id=?1 AND state='pending'",
        [TEST_WORKSPACE_ID], |row| row.get::<_, i64>(0))?)
        })
        .unwrap();
    assert_eq!(pending, METHODS.len() as i64);
    // A new real receipt connection and owner have no in-memory replay state.
    // This models lost owner completion, not an arbitrary OS crash.
    let mut recovered_api = f.api.clone();
    recovered_api.config_store =
        Arc::new(SqliteWorkspaceStore::open_for_test(&f.api.config.database_path).unwrap());
    let recovered = Arc::new(
        plugin_host::EmbeddedPluginHostOwner::new(
            Arc::new(recovered_api.clone()),
            f.verified_context().await,
            TEST_WORKSPACE_ID.into(),
        )
        .with_transport(peer.transport()),
    );
    let restored = f.host(recovered.clone(), "lost-acknowledgements").await;
    assert_eq!(
        restored.context().invocation_id,
        host.context().invocation_id
    );
    for (index, method) in METHODS.iter().enumerate() {
        let error = recovered
            .call(send(
                &restored.context().invocation_id,
                method.clone(),
                &format!("lost-{index}"),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code, PluginHostErrorCode::Indeterminate);
    }
    let expiry = recovered_api
        .config_store
        .with_immediate_transaction(|tx| {
            let record = crate::plugin_receipt::lookup(
                tx,
                TEST_WORKSPACE_ID,
                &restored.context().invocation_id,
            )?;
            crate::plugin_receipt::prune(tx, TEST_WORKSPACE_ID, record.expires_at_ms)
        })
        .unwrap();
    assert_eq!(expiry, METHODS.len());
    for key in ["lost-0", "unused-after-retention"] {
        assert_eq!(
            recovered
                .call(send(
                    &restored.context().invocation_id,
                    PluginHttpMethod::Get,
                    key
                ))
                .await
                .unwrap_err()
                .code,
            PluginHostErrorCode::ReceiptExpired
        );
    }
    assert_eq!(peer.connection_count(), METHODS.len());
    assert_eq!(
        recovered
            .call(PluginHostRequest::Authorize {
                invocation_id: restored.context().invocation_id.clone(),
                operation: PluginHostOperation::Publish,
            })
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::ReceiptExpired
    );
    assert_eq!(
        recovered
            .call(PluginHostRequest::Begin {
                plan_id: f.plan.plan_id.clone(),
                installation_id: f.installed.installation.installation_id.clone(),
                consumer: f.consumer("lost-acknowledgements"),
            })
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::ReceiptExpired
    );
    f.shutdown().await;
}

#[tokio::test]
async fn secret_rotation_uses_current_value_but_binding_policy_change_denies_receipt_replay() {
    let mut peer = Peer::new(
        HOST,
        vec![response(200, &[], b"one"), response(200, &[], b"two")],
    )
    .await;
    let mut binding = peer_binding(&peer);
    binding.secret_ref = "peer-credential".into();
    let f = Fixture::new(Some(binding.clone())).await;
    let root = ServerConfig::workspace_backend_data_root_for_data_dir(
        &f.api.config.data_root,
        TEST_WORKSPACE_ID,
    );
    let secrets = secrets::SecretStore::new(root);
    secrets
        .set(
            "plugin/peer-credential",
            secrets::SecretValue::new("Bearer first-value"),
        )
        .unwrap();
    let owner = f
        .owner(Some(peer.transport_with_answers(vec![
            vec![
                "8.8.8.8:443".parse().unwrap()
            ];
            2
        ])))
        .await;
    let host = f.host(owner.clone(), "credential-rotation").await;
    let first = send(
        &host.context().invocation_id,
        PluginHttpMethod::Get,
        "first",
    );
    let saved = owner.call(first.clone()).await.unwrap();
    let bytes = String::from_utf8(peer.observed().await)
        .unwrap()
        .to_ascii_lowercase();
    assert!(bytes.contains("authorization: bearer first-value\r\n"));
    secrets
        .set(
            "plugin/peer-credential",
            secrets::SecretValue::new("Bearer rotated-value"),
        )
        .unwrap();
    assert_eq!(owner.call(first.clone()).await.unwrap(), saved);
    assert_eq!(
        peer.connection_count(),
        1,
        "credential rotation must not invalidate receipt identity"
    );
    owner
        .call(send(
            &host.context().invocation_id,
            PluginHttpMethod::Get,
            "second",
        ))
        .await
        .unwrap();
    let bytes = String::from_utf8(peer.observed().await)
        .unwrap()
        .to_ascii_lowercase();
    assert!(bytes.contains("authorization: bearer rotated-value\r\n"));
    assert!(!bytes.contains("first-value"));
    // Remove policy permissions without changing installation/artifact/secret
    // identity. Old immutable ceilings must not authorize the changed policy.
    binding.response_headers = vec!["content-type".into()];
    binding.request_headers.clear();
    commit_config(&f.api, &f.installed, f.tool, Some(&binding));
    assert_denied(owner.call(first).await);
    assert_eq!(peer.connection_count(), 2);
    let stored = f.api.config_store.with_conn(|conn| Ok(conn.query_row(
        "SELECT response_body FROM plugin_effect_receipts WHERE workspace_id=?1 AND effect_key='first'",
        [TEST_WORKSPACE_ID], |row| row.get::<_, Vec<u8>>(0))?)).unwrap();
    assert_eq!(
        stored, b"one",
        "denied replay does not destroy retained evidence"
    );
    f.shutdown().await;
}

#[tokio::test]
async fn unavailable_receipt_owner_rejects_send_and_result_publication_without_connecting() {
    let peer = Peer::new(HOST, vec![]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let owner = f.owner(Some(peer.transport())).await;
    let host = f.host(owner.clone(), "owner-unavailable").await;
    // Fault the real DB boundary, not the authorizer. No mocked permission check
    // or synthetic Unavailable response participates in this test.
    f.api
        .config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "ALTER TABLE plugin_invocations RENAME TO unavailable_plugin_invocations",
            )?;
            Ok(())
        })
        .unwrap();
    for request in [
        send(
            &host.context().invocation_id,
            PluginHttpMethod::Get,
            "never-sent",
        ),
        PluginHostRequest::Authorize {
            invocation_id: host.context().invocation_id.clone(),
            operation: PluginHostOperation::Publish,
        },
    ] {
        assert_eq!(
            owner.call(request).await.unwrap_err().code,
            PluginHostErrorCode::Unavailable
        );
    }
    assert!(f.invoke(host).await.is_err());
    assert_eq!(peer.connection_count(), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

struct RevokeAtPublication {
    inner: Arc<dyn PluginHostOwner>,
    api: WorkspaceApi,
    subject: String,
    installation: String,
}
#[async_trait]
impl PluginHostOwner for RevokeAtPublication {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> std::result::Result<PluginHostResponse, PluginHostError> {
        if matches!(
            &request,
            PluginHostRequest::Authorize {
                operation: PluginHostOperation::Publish,
                ..
            }
        ) {
            set_policy(&self.api, &self.subject, &self.installation, false);
        }
        self.inner.call(request).await
    }
}

#[tokio::test]
async fn pure_component_output_is_withheld_when_authority_changes_before_publication() {
    let f = Fixture::new(None).await;
    let owner = Arc::new(RevokeAtPublication {
        inner: f.owner(None).await,
        api: f.api.clone(),
        subject: f.subject.clone(),
        installation: f.installed.installation.installation_id.clone(),
    });
    let host = f.host(owner, "pure-publication-revoked").await;
    assert!(matches!(f.invoke(host).await,
        Err(ExecutionError::Host(error)) if error.code == yoi_plugin_runtime::host::errors::Code::Denied));
    assert_eq!(f.count("plugin_invocations"), 1);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

#[tokio::test]
async fn ungranted_subject_cannot_begin_invocation_or_create_a_network_receipt() {
    let peer = Peer::new(HOST, vec![]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    f.deny();
    let owner = f.owner(Some(peer.transport())).await;
    let denied = OwnerHost::begin(owner, f.context(), f.consumer("ungranted")).await;
    assert!(matches!(denied, Err(error) if error.code == PluginHostErrorCode::Denied));
    assert_eq!(f.count("plugin_invocations"), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    assert_eq!(peer.connection_count(), 0);
    f.shutdown().await;
}

struct SignedAuthorizer {
    signer: worker_runtime::auth::RuntimeRequestSourceSigner,
    worker_id: String,
}
impl server_api::client_support::RequestAuthorizer for SignedAuthorizer {
    fn authorize(
        &self,
        request: server_api::client_support::AuthorizerRequest<'_>,
    ) -> std::result::Result<HeaderMap, server_api::client_support::AuthorizationError> {
        let proof = self
            .signer
            .issue(
                "server-test",
                TEST_WORKSPACE_ID,
                Some(&self.worker_id),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                request.method.as_str(),
                request.path_and_query,
                request.body,
                i64::try_from(worker_runtime::auth::unix_now_seconds())
                    .map_err(|_| server_api::client_support::AuthorizationError::new())?,
                30,
            )
            .map_err(|_| server_api::client_support::AuthorizationError::new())?;
        let mut headers = HeaderMap::new();
        headers.insert(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            proof
                .parse()
                .map_err(|_| server_api::client_support::AuthorizationError::new())?,
        );
        Ok(headers)
    }
}

struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn remote_generated_client_signed_owner_returns_same_component_result_and_current_denial() {
    use yoi_plugin_runtime::owner::RemotePluginHostOwner;
    let f = Fixture::new(None).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(f.api.clone());
    let mut server = ServerTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = server_api::ServerApiClient::builder(format!("http://{address}"))
        .unwrap()
        .client(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
        )
        .authorizer(SignedAuthorizer {
            signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity),
            worker_id: f.source.worker_id.clone(),
        })
        .build()
        .unwrap();
    let owner = Arc::new(RemotePluginHostOwner::new(
        Arc::new(client),
        TEST_WORKSPACE_ID.into(),
    ));
    let host = f.host(owner.clone(), "remote-pure-success").await;
    let result = f.invoke(host).await.unwrap();
    assert_eq!(result.output.summary, "example_echo echoed text");
    assert_eq!(
        serde_json::from_str::<Value>(result.output.structured_json.as_deref().unwrap()).unwrap(),
        json!({"tool":"example_echo", "text":"ordinary result"})
    );
    let host = f.host(owner, "remote-pure-denied").await;
    f.deny();
    assert!(matches!(f.invoke(host).await,
        Err(ExecutionError::Host(error)) if error.code == yoi_plugin_runtime::host::errors::Code::Denied));
    assert_eq!(f.count("plugin_invocations"), 2);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    server.0.abort();
    let _ = (&mut server.0).await;
    f.shutdown().await;
}

#[tokio::test]
async fn revocation_or_cancellation_during_dns_prevents_dispatch_and_recovery() {
    for cancel in [false, true] {
        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(Some(peer_binding(&peer))).await;
        let (transport, started, release) = peer.transport_with_dns_gate();
        let owner = f.owner(Some(transport)).await;
        let host = f.host(owner.clone(), "dns-race").await;
        let id = host.context().invocation_id.clone();
        let call = owner.call(send(&id, PluginHttpMethod::Post, "key"));
        tokio::pin!(call);
        tokio::select! {
            result = &mut call => panic!("send finished before DNS gate: {result:?}"),
            result = started => result.unwrap(),
        }
        assert_eq!(
            f.count("plugin_effect_receipts"),
            1,
            "admission committed before DNS"
        );
        if cancel {
            host.cancel().await.unwrap();
            host.cancel().await.unwrap();
        } else {
            f.revoke();
        }
        release.send(vec![peer.public_address()]).unwrap();
        let error = call.await.unwrap_err();
        assert_eq!(
            error.code,
            if cancel {
                PluginHostErrorCode::Cancelled
            } else {
                PluginHostErrorCode::Denied
            }
        );
        assert!(
            owner
                .call(send(&id, PluginHttpMethod::Post, "key"))
                .await
                .is_err()
        );
        assert!(
            owner
                .call(send(&id, PluginHttpMethod::Get, "unused-key"))
                .await
                .is_err()
        );
        assert_eq!(peer.connection_count(), 0);
        f.shutdown().await;
    }
}

#[tokio::test]
async fn authority_removed_after_dispatch_withholds_response_and_restored_read_replays_without_send()
 {
    use crate::plugin_https::test_support::Reply;
    let (release, waiting) = tokio::sync::oneshot::channel();
    let mut peer = Peer::new(HOST, vec![Reply::Gated(waiting)]).await;
    let f = Fixture::new(Some(peer_binding(&peer))).await;
    let owner = f.owner(Some(peer.transport())).await;
    let host = f.host(owner.clone(), "publication-race").await;
    let id = host.context().invocation_id.clone();
    let call = owner.call(send(&id, PluginHttpMethod::Post, "key"));
    tokio::pin!(call);
    tokio::select! {
        result = &mut call => panic!("send finished before peer observed: {result:?}"),
        _ = peer.observed() => {},
    }
    f.deny();
    release
        .send(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata".to_vec())
        .unwrap();
    assert_denied(call.await);
    assert_denied(owner.call(send(&id, PluginHttpMethod::Post, "key")).await);
    set_policy(
        &f.api,
        &f.subject,
        &f.installed.installation.installation_id,
        true,
    );
    let replay = owner
        .call(send(&id, PluginHttpMethod::Post, "key"))
        .await
        .unwrap();
    assert!(
        matches!(replay, PluginHostResponse::Response { response } if response.body == b"data")
    );
    assert_eq!(peer.connection_count(), 1);
    f.shutdown().await;
}

#[tokio::test]
async fn generated_remote_owner_component_https_and_embedded_replay_share_receipt_and_live_denial()
{
    use yoi_plugin_runtime::owner::RemotePluginHostOwner;
    let mut peer = Peer::new(HOST, vec![response(503, &[], b"peer data")]).await;
    let mut f = Fixture::new(Some(peer_binding(&peer))).await;
    f.api.plugin_https_transport = Some(Arc::new(peer.transport()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(f.api.clone());
    let mut server = ServerTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = server_api::ServerApiClient::builder(format!("http://{address}"))
        .unwrap()
        .client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
        )
        .authorizer(SignedAuthorizer {
            signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity),
            worker_id: f.source.worker_id.clone(),
        })
        .build()
        .unwrap();
    let owner = Arc::new(RemotePluginHostOwner::new(
        Arc::new(client),
        TEST_WORKSPACE_ID.into(),
    ));
    let host = f.host(owner.clone(), "remote-https").await;
    let id = host.context().invocation_id.clone();
    let result = f.invoke(host).await.unwrap();
    assert_eq!(result.output.summary, "HTTP response observed");
    assert!(
        String::from_utf8(peer.observed().await)
            .unwrap()
            .starts_with("GET /status HTTP/1.1\r\n")
    );
    let remote = owner
        .call(send(&id, PluginHttpMethod::Get, "status-v1"))
        .await
        .unwrap();
    let embedded = f.owner(Some(peer.transport())).await;
    assert_eq!(
        embedded
            .call(send(&id, PluginHttpMethod::Get, "status-v1"))
            .await
            .unwrap(),
        remote
    );
    assert_eq!(f.count("plugin_effect_receipts"), 1);
    f.deny();
    assert_denied(
        owner
            .call(send(&id, PluginHttpMethod::Get, "status-v1"))
            .await,
    );
    assert_denied(
        embedded
            .call(send(&id, PluginHttpMethod::Get, "status-v1"))
            .await,
    );
    assert_eq!(peer.connection_count(), 1);
    server.0.abort();
    let _ = (&mut server.0).await;
    f.shutdown().await;
}

async fn remote_owner_for(
    f: &Fixture,
    worker_id: &str,
    workspace: &str,
) -> (Arc<dyn PluginHostOwner>, ServerTask) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(f.api.clone());
    let task = ServerTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = server_api::ServerApiClient::builder(format!("http://{address}"))
        .unwrap()
        .response_body_limit(yoi_plugin_runtime::owner::PLUGIN_HOST_RESPONSE_BODY_LIMIT)
        .client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
        )
        .authorizer(SignedAuthorizer {
            signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity),
            worker_id: worker_id.into(),
        })
        .build()
        .unwrap();
    (
        Arc::new(yoi_plugin_runtime::owner::RemotePluginHostOwner::new(
            Arc::new(client),
            workspace.into(),
        )),
        task,
    )
}

fn prepare(f: &Fixture) -> PluginHostRequest {
    PluginHostRequest::Prepare {
        plan_id: f.plan.plan_id.clone(),
        installation_id: f.installed.installation.installation_id.clone(),
        tools: vec![f.tool.into()],
    }
}

#[tokio::test]
async fn pure_and_https_restore_prepare_rechecks_live_owner_without_consuming_calls_or_receipts() {
    for https in [false, true] {
        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(https.then(|| peer_binding(&peer))).await;
        let embedded = f.owner(Some(peer.transport())).await;
        let (remote, mut server) =
            remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
        for owner in [&embedded, &remote] {
            assert_eq!(
                owner.call(prepare(&f)).await.unwrap(),
                PluginHostResponse::Authorized
            );
            owner
                .prepare(
                    f.plan.plan_id.clone(),
                    f.installed.installation.installation_id.clone(),
                    vec![f.tool.into()],
                )
                .await
                .unwrap();
            let mut expanded = prepare(&f);
            if let PluginHostRequest::Prepare { tools, .. } = &mut expanded {
                tools.push("not-selected".into());
            }
            assert_denied(owner.call(expanded).await);
        }
        f.api
            .config_store
            .plugin_plan_transition(
                TEST_WORKSPACE_ID,
                &f.plan.plan_id,
                PlanPinState::Active,
                PlanPinState::Restorable,
            )
            .unwrap();
        for owner in [&embedded, &remote] {
            assert_eq!(
                owner.call(prepare(&f)).await.unwrap(),
                PluginHostResponse::Authorized
            );
        }
        f.deny();
        for owner in [&embedded, &remote] {
            assert_denied(owner.call(prepare(&f)).await);
        }
        set_policy(
            &f.api,
            &f.subject,
            &f.installed.installation.installation_id,
            true,
        );
        f.revoke();
        for owner in [&embedded, &remote] {
            assert_denied(owner.call(prepare(&f)).await);
        }
        assert_eq!(f.count("plugin_invocations"), 0);
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        assert_eq!(peer.connection_count(), 0);
        server.0.abort();
        let _ = (&mut server.0).await;
        f.shutdown().await;
    }
}

#[tokio::test]
async fn prepare_unavailable_owner_foreign_workspace_and_unsigned_embedded_never_admit_or_consume()
{
    let f = Fixture::new(None).await;
    let unsigned = plugin_host::EmbeddedPluginHostOwner::authenticated(
        Arc::new(f.api.clone()),
        TEST_WORKSPACE_ID.into(),
        Arc::new(server_api::client_support::NoAuthorizer),
    );
    assert_denied(unsigned.call(prepare(&f)).await);
    let (foreign, mut foreign_server) =
        remote_owner_for(&f, &f.source.worker_id, "foreign-workspace").await;
    assert!(foreign.call(prepare(&f)).await.is_err());
    let embedded = f.owner(None).await;
    let (remote, mut server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    f.api
        .config_store
        .with_conn(|conn| {
            conn.execute_batch("ALTER TABLE plugin_execution_plans RENAME COLUMN plan_json TO unavailable_plan_json")?;
            Ok(())
        })
        .unwrap();
    for owner in [&embedded, &remote] {
        assert_eq!(
            owner.call(prepare(&f)).await.unwrap_err().code,
            PluginHostErrorCode::Unavailable
        );
    }
    assert_eq!(f.count("plugin_invocations"), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    server.0.abort();
    let _ = (&mut server.0).await;
    foreign_server.0.abort();
    let _ = (&mut foreign_server.0).await;
    f.shutdown().await;
}

#[tokio::test]
async fn canonical_reserved_child_identity_and_prepare_match_live_owner_and_fence_lost_delegation()
{
    use server_api::authorization::WorkerDelegationRequest;
    let f = Fixture::new(None).await;
    let store = open_subjektiv_store(&f.api).unwrap();
    let previous = store.subject_permissions(&f.subject).unwrap().unwrap();
    store
        .set_subject_permissions(
            &f.subject,
            &SubjectPermissionsUpdate {
                operation_id: "delegate-plugin".into(),
                expected_change_id: Some(previous.change_id),
                execution: previous.execution.clone(),
                delegation: previous.execution.clone(),
            },
        )
        .unwrap();
    let request = WorkerDelegationRequest {
        permissions: previous.execution,
        redelegation: vec![],
    };
    let grant = subject_authorization::prepare_delegation(
        &f.api,
        &f.source,
        Some(&request),
        &f.source.runtime_id,
        PROFILE,
        "profile-digest",
        None,
        false,
    )
    .unwrap()
    .unwrap();
    let memory = f
        .api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reserved = f
        .api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            &f.source.runtime_id,
            "prepare-child",
            "sha256:reserved-child",
            None,
            &memory,
        )
        .unwrap();
    assert!(
        f.api
            .plugin_reserved_execution_identity(&f.source.runtime_id, &reserved, Some(&grant))
            .is_err(),
        "unbound delegation must not become an ordinary Worker"
    );
    subject_authorization::bind_reserved_grant(&f.api, &grant, &reserved.worker_id.to_string())
        .unwrap();
    let child = RuntimeWorkerRef::new(&f.source.runtime_id, reserved.worker_id.to_string());
    assert!(f.api.runtime.worker(&child).is_err());
    let identity = f
        .api
        .plugin_reserved_execution_identity(&child.runtime_id, &reserved, Some(&grant))
        .unwrap();
    let mut forged = reserved.clone();
    forged.create_fingerprint.push_str("different");
    assert!(
        f.api
            .plugin_reserved_execution_identity(&child.runtime_id, &forged, Some(&grant))
            .is_err()
    );
    let state = f
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let plan = f
        .api
        .config_store
        .with_immediate_transaction(|tx| {
            let plan = crate::plugin_settings::resolve_worker_plugin_plan(
                tx,
                TEST_WORKSPACE_ID,
                &state.snapshot.digest,
                PROFILE,
                identity.clone(),
            )?;
            crate::plugin_plan::plugin_plan_create(tx, "fixture-host", "child-plan", &plan)
        })
        .unwrap();
    let embedded = Arc::new(plugin_host::EmbeddedPluginHostOwner::authenticated(
        Arc::new(f.api.clone()),
        TEST_WORKSPACE_ID.into(),
        Arc::new(SignedAuthorizer {
            signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity),
            worker_id: child.worker_id.clone(),
        }),
    )) as Arc<dyn PluginHostOwner>;
    let (remote, mut server) = remote_owner_for(&f, &child.worker_id, TEST_WORKSPACE_ID).await;
    let preflight = PluginHostRequest::Prepare {
        plan_id: plan.plan_id,
        installation_id: f.installed.installation.installation_id.clone(),
        tools: vec![f.tool.into()],
    };
    for owner in [&embedded, &remote] {
        assert_eq!(
            owner.call(preflight.clone()).await.unwrap(),
            PluginHostResponse::Authorized
        );
    }
    // Only unrelated Worker execution/summary is substituted by this fixture.
    // The identity, reservation, signed proof and delegation checks stay real.
    let mut summary = f.api.runtime.worker(&f.source).unwrap();
    summary.worker.worker_id = child.worker_id.clone();
    summary.singleton_key = None;
    f.runtime.workers.lock().unwrap().push(summary);
    assert_eq!(
        f.api.plugin_execution_identity(&child).await.unwrap(),
        identity
    );
    f.deny();
    assert!(
        f.api
            .plugin_reserved_execution_identity(&child.runtime_id, &reserved, Some(&grant))
            .is_err()
    );
    for owner in [&embedded, &remote] {
        assert_denied(owner.call(preflight.clone()).await);
    }
    assert_eq!(f.count("plugin_invocations"), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    server.0.abort();
    let _ = (&mut server.0).await;
    f.shutdown().await;
}

#[tokio::test]
async fn reserved_job_without_canonical_job_binding_cannot_fall_back_to_ordinary_identity() {
    let f = Fixture::new(None).await;
    let memory = f
        .api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reserved = f
        .api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            &f.source.runtime_id,
            &crate::backend_job::allocation_key("not-bound-attempt"),
            "sha256:unbound-job",
            None,
            &memory,
        )
        .unwrap();
    assert!(
        f.api
            .plugin_reserved_execution_identity(&f.source.runtime_id, &reserved, None)
            .is_err()
    );
    assert_eq!(f.count("plugin_invocations"), 0);
    assert_eq!(f.count("plugin_effect_receipts"), 0);
    f.shutdown().await;
}

#[tokio::test]
async fn active_remote_and_embedded_sends_close_on_cancel_revoke_permission_loss_and_owner_failure()
{
    use crate::plugin_https::test_support::Reply;
    use server_api::PluginEffectObservation;
    for remote in [false, true] {
        for action in ["cancel", "revoke", "permission", "unavailable"] {
            let mut peer = Peer::new(HOST, vec![Reply::Stall]).await;
            let mut f = Fixture::new(Some(peer_binding(&peer))).await;
            f.api.plugin_https_transport = Some(Arc::new(peer.transport_for_requests(4)));
            let (remote_owner, server) =
                remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
            let owner = if remote {
                remote_owner
            } else {
                f.owner(Some(peer.transport_for_requests(4))).await
            };
            let host = f.host(owner.clone(), "active-interruption").await;
            let id = host.context().invocation_id.clone();
            let request = send(&id, PluginHttpMethod::Post, "once");
            let mut recovered_api = f.api.clone();
            recovered_api.config_store =
                Arc::new(SqliteWorkspaceStore::open_for_test(&f.api.config.database_path).unwrap());
            let attempt = owner.call(request.clone());
            tokio::pin!(attempt);
            tokio::select! {
                result = &mut attempt => panic!("request must reach the stalled peer: {result:?}"),
                _ = peer.observed() => {},
            }
            match action {
                "cancel" => host.cancel().await.unwrap(),
                "revoke" => f.revoke(),
                "permission" => f.deny(),
                "unavailable" => f
                    .api
                    .config_store
                    .with_conn(|conn| {
                        conn.execute_batch(
                            "ALTER TABLE plugin_execution_plans RENAME TO unavailable_plugin_plans",
                        )?;
                        Ok(())
                    })
                    .unwrap(),
                _ => unreachable!(),
            }
            // No response is released. HTTP has a five-second timeout; authority
            // loss must close the owner transport substantially before that.
            let error = tokio::time::timeout(std::time::Duration::from_secs(2), &mut attempt)
                .await
                .expect("active transport did not stop")
                .unwrap_err();
            assert_eq!(
                error.effect,
                PluginEffectObservation::Indeterminate,
                "{remote}/{action}"
            );
            peer.closed().await;
            if action == "unavailable" {
                f.api
                    .config_store
                    .with_conn(|conn| {
                        conn.execute_batch(
                            "ALTER TABLE unavailable_plugin_plans RENAME TO plugin_execution_plans",
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
            if action == "permission" {
                set_policy(
                    &f.api,
                    &f.subject,
                    &f.installed.installation.installation_id,
                    true,
                );
            }
            // Another actual owner with an independent SQLite connection has no
            // local attempt state. Reopen after ordinary cancellation/revocation;
            // the unavailable-owner fault deliberately changes SQLite DDL quotes.
            if action != "unavailable" {
                recovered_api.config_store = Arc::new(
                    SqliteWorkspaceStore::open_for_test(&f.api.config.database_path).unwrap(),
                );
            }
            let recovered = plugin_host::EmbeddedPluginHostOwner::new(
                Arc::new(recovered_api),
                f.verified_context().await,
                TEST_WORKSPACE_ID.into(),
            )
            .with_transport(peer.transport_for_requests(4));
            assert!(recovered.call(request).await.is_err());
            assert_eq!(f.count("plugin_effect_receipts"), 1);
            assert_eq!(
                peer.connection_count(),
                1,
                "recovery resent {remote}/{action}"
            );
            drop(server);
            f.shutdown().await;
        }
    }
}

#[tokio::test]
async fn binary_ceiling_matches_generated_remote_router_and_embedded_owner_limits() {
    let ceiling = crate::plugin_binding::PluginConnectionPolicyCeiling::default().limits;
    let body = vec![255; ceiling.max_request_bytes as usize];
    let mut peer = Peer::new(
        HOST,
        vec![response(200, &[], &body), response(200, &[], &body)],
    )
    .await;
    let mut binding = peer_binding(&peer);
    binding.limits.max_request_bytes = ceiling.max_request_bytes;
    binding.limits.max_response_bytes = ceiling.max_response_bytes;
    let mut f = Fixture::new(Some(binding)).await;
    f.api.plugin_https_transport = Some(Arc::new(peer.transport_for_requests(4)));
    let embedded = f.owner(Some(peer.transport_for_requests(4))).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = f.host(remote.clone(), "binary-boundary").await;
    let id = &host.context().invocation_id;
    for (index, owner) in [&embedded, &remote].into_iter().enumerate() {
        let mut input = send(id, PluginHttpMethod::Post, &format!("binary-{index}"));
        if let PluginHostRequest::Send { request, .. } = &mut input {
            request.body = body.clone();
        }
        assert!(serde_json::to_vec(&input).unwrap().len() > 4 * 1024 * 1024);
        let PluginHostResponse::Response { response } = owner.call(input.clone()).await.unwrap()
        else {
            panic!("expected response")
        };
        assert_eq!(response.status, 200);
        assert_eq!(response.body.len(), body.len());
        assert!(response.body.iter().all(|byte| *byte == 255));
        let observed = peer.observed().await;
        assert!(observed.ends_with(&body));
        assert_eq!(peer.connection_count(), index + 1);
        // Replays cross adapters without a new connection.
        embedded.call(input).await.unwrap();
        let mut oversized = send(id, PluginHttpMethod::Post, &format!("oversized-{index}"));
        if let PluginHostRequest::Send { request, .. } = &mut oversized {
            request.body = vec![255; body.len() + 1];
        }
        let error = owner.call(oversized).await.unwrap_err();
        assert_eq!(error.code, PluginHostErrorCode::QuotaExceeded);
        assert_eq!(peer.connection_count(), index + 1);
    }
    assert_eq!(f.count("plugin_effect_receipts"), 2);
    let mut enormous = send(id, PluginHttpMethod::Post, "wire-overflow");
    if let PluginHostRequest::Send { request, .. } = &mut enormous {
        request.body = vec![255; 2 * body.len()];
    }
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/plugin-host");
    let response = build_router(f.api.clone())
        .oneshot(runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(&enormous).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(peer.connection_count(), 2);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn other_workspace_control_plane_hosts_ips_and_dns_aliases_never_connect() {
    for dynamic in [false, true] {
        for host in [HOST, "8.8.8.8", "runtime-alias.example.com"] {
            let peer = Peer::new(HOST, vec![]).await;
            let mut binding = peer_binding(&peer);
            binding.origin = format!("https://{host}:{}", peer.public_address().port());
            let mut f = Fixture::new(Some(binding)).await;
            let endpoint = format!("https://{HOST}:{}", peer.public_address().port());
            if dynamic {
                let mut workspace = test_workspace();
                workspace.workspace_id = "workspace-b".into();
                f.api.store.upsert_workspace(&workspace).await.unwrap();
                f.api
                    .store
                    .upsert_workspace_runtime_binding_record(
                        WorkspaceRuntimeBinding {
                            workspace_id: "workspace-b".into(),
                            runtime_id: "only-b".into(),
                            display_name: "Only B".into(),
                            base_url: endpoint,
                            public_key: f.identity.public_key.clone(),
                            public_key_fingerprint: String::new(),
                            binding_id: String::new(),
                            authentication_mode:
                                StoredRuntimeAuthenticationMode::LegacyServerIssuer,
                            created_at: "1".into(),
                            updated_at: "1".into(),
                            revoked_at: None,
                        },
                        true,
                    )
                    .await
                    .unwrap();
            } else {
                let mut operational = f.api.config.clone();
                operational.remote_runtime_sources.push(
                    RemoteRuntimeConfig::new("only-b", "Only B", endpoint, None)
                        .with_workspace_id("workspace-b"),
                );
                f.api.config = operational
                    .for_catalog_workspace(&test_workspace(), vec![])
                    .unwrap();
                assert!(
                    !f.api
                        .config
                        .remote_runtime_sources
                        .iter()
                        .any(|source| source.runtime_id == "only-b")
                );
            }
            // Only external DNS/socket/trust are substituted. The operational
            // inventory and concrete shared owner/adapters remain production code.
            let policy = plugin_host::transport_policy(&f.api, |name, port| async move {
                let ip = if name == HOST { "8.8.8.8" } else { "9.9.9.9" };
                Ok(vec![format!("{ip}:{port}").parse().unwrap()])
            })
            .await
            .unwrap();
            assert!(policy.denied_hosts.contains(HOST));
            assert!(
                policy
                    .denied_addresses
                    .contains(&"8.8.8.8".parse().unwrap())
            );
            f.api.plugin_https_transport =
                Some(Arc::new(peer.transport_with_policy(policy.clone())));
            let embedded = f.owner(Some(peer.transport_with_policy(policy))).await;
            let (remote, server) =
                remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
            let invocation = f.host(embedded.clone(), "forbidden-control-plane").await;
            for owner in [&embedded, &remote] {
                assert_denied(
                    owner
                        .call(send(
                            &invocation.context().invocation_id,
                            PluginHttpMethod::Get,
                            "blocked",
                        ))
                        .await,
                );
            }
            assert_eq!(peer.connection_count(), 0, "{dynamic}/{host}");
            drop(server);
            f.shutdown().await;
        }
    }
}

#[tokio::test]
async fn stalled_authority_monitor_closes_active_transport_and_consumes_effect() {
    use crate::plugin_https::test_support::Reply;
    for remote in [false, true] {
        let mut peer = Peer::new(HOST, vec![Reply::Stall]).await;
        let mut f = Fixture::new(Some(peer_binding(&peer))).await;
        f.api.plugin_https_transport = Some(Arc::new(peer.transport_for_requests(4)));
        let (remote_owner, server) =
            remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
        let owner = if remote {
            remote_owner
        } else {
            f.owner(Some(peer.transport_for_requests(4))).await
        };
        let host = f.host(owner.clone(), "monitor-unavailable").await;
        let request = send(
            &host.context().invocation_id,
            PluginHttpMethod::Post,
            "once",
        );
        let attempt = owner.call(request.clone());
        tokio::pin!(attempt);
        tokio::select! {
            result = &mut attempt => panic!("request must reach peer: {result:?}"),
            _ = peer.observed() => {},
        }
        // Hold the actual authority DB connection. The monitor cannot complete
        // its read, and may not treat a past permission as continued authority.
        let store = f.api.config_store.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        let held = tokio::task::spawn_blocking(move || {
            store.with_conn(|_| {
                let _ = started.send(());
                let _ = gate.blocking_recv();
                Ok(())
            })
        });
        waiting.await.unwrap();
        tokio::select! {
            result = &mut attempt => panic!("DB is held but owner finished: {result:?}"),
            _ = peer.closed() => {},
        }
        release.send(()).unwrap();
        held.await.unwrap().unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(3), &mut attempt)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.effect,
            server_api::PluginEffectObservation::Indeterminate
        );
        assert_eq!(error.code, PluginHostErrorCode::Unavailable);
        assert!(owner.call(request).await.is_err());
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        assert_eq!(peer.connection_count(), 1);
        drop(server);
        f.shutdown().await;
    }
}
