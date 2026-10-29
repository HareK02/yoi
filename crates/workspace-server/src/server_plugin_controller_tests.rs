// Include directly in server::tests (this file owns its module).
// Production Embedded Server assembly -> ProfileRuntimeWorkerFactory -> actual
// Controller/Engine -> pinned Wasmtime Tool -> HTTPS -> SQLite and Session store.
// Only the external model and the TLS peer's DNS/socket/trust are substituted.
// No FixtureRuntime, manually prepared Plugin Feature, or fabricated Host owner.
//
// Shared-helper visibility required in the existing suites (no Fixture exposed):
// - plugin_tests::request: pub(super)
// - plugin_host_tests::plugin_runtime_tests: pub(super)
// - that module's Model, Model.tool_name, Model.requests and assert_https_history:
//   pub(in crate::server::tests)
// TLS Peer is already crate-visible. WorkerSessionStore needs no new wrapper.
// Run: cargo test -p yoi-workspace-server --lib plugin_controller_tests
// Prerequisites: openssl and the normal connected-restore test environment.
mod plugin_controller_tests {
    use super::plugin_host_tests::plugin_runtime_tests::{Model, assert_https_history};
    use super::plugin_tests::request;
    use super::*;
    use crate::plugin_https::test_support::{HOST, Peer, response};
    use crate::plugin_plan::PlanPinState;
    use session_store::Store;
    use sha2::Digest;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    const PROFILE: &str = "project:plugin-controller";
    const TOOL: &str = "example_status";
    const WATCHDOG: Duration = Duration::from_secs(60);

    struct Fixture {
        api: WorkspaceApi,
        owner: String,
        model_tool: Arc<Mutex<String>>,
        requests: Arc<Mutex<Vec<agen::llm_client::Request>>>,
        model_clients: Arc<AtomicUsize>,
        _directory: tempfile::TempDir,
    }

    impl Fixture {
        async fn new(peer: &Peer) -> Self {
            // Use the established short-path/environment guard for real restore
            // (not a mock controller or a replacement execution backend).
            let directory = super::restored_parent_control_tests::connected_restore_tempdir();
            let mut config = test_server_config(directory.path());
            config.repositories.clear();
            let store =
                Arc::new(SqliteWorkspaceStore::open_api_fixture(&config.database_path).unwrap());
            seed_test_registered_workspace(store.as_ref(), &config)
                .await
                .unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let model_tool = Arc::new(Mutex::new(String::new()));
            let model_clients = Arc::new(AtomicUsize::new(0));
            let clients = model_clients.clone();
            let observed = requests.clone();
            let tool = model_tool.clone();
            let transport = Arc::new(peer.transport());
            let api = WorkspaceApi::new_with_embedded_configurators(
                config,
                store,
                HostExecutors::default(),
                move |factory| {
                    factory.with_spawn_model_client_factory(move || {
                        clients.fetch_add(1, Ordering::SeqCst);
                        Box::new(Model {
                            tool_name: tool.lock().unwrap().clone(),
                            requests: observed.clone(),
                        })
                    })
                },
                move |api| api.plugin_https_transport = Some(transport),
            )
            .await
            .unwrap();
            let owner =
                seed_test_workspace_owner_token(api.store.as_ref(), "plugin-controller-owner")
                    .await;
            // Delivery/cache authority is the active Workspace signing identity,
            // not the Embedded Runtime key or the human catalog credential.
            request(
                build_router(api.clone()),
                Some(&owner),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/settings/signing-identity/provision"),
                b"{}".to_vec(),
                false,
                StatusCode::OK,
            )
            .await;
            Self {
                api,
                owner,
                model_tool,
                requests,
                model_clients,
                _directory: directory,
            }
        }

        async fn install(&self, peer: &Peer) -> (server_api::PluginOperationResponse, Vec<u8>) {
            self.install_component(peer, true).await
        }

        async fn install_component(
            &self,
            peer: &Peer,
            https: bool,
        ) -> (server_api::PluginOperationResponse, Vec<u8>) {
            use manifest::plugin::*;
            let directory = tempfile::tempdir().unwrap();
            let (manifest, component): (&str, &[u8]) = if https {
                (
                    include_str!(
                        "../../../resources/plugin/templates/rust-component-https/plugin.toml"
                    ),
                    include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm"),
                )
            } else {
                (
                    include_str!(
                        "../../../resources/plugin/templates/rust-component-tool/plugin.toml"
                    ),
                    include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
                )
            };
            fs::write(directory.path().join("plugin.toml"), manifest).unwrap();
            fs::write(directory.path().join("plugin.component.wasm"), component).unwrap();
            let limits = PluginPackageLimits::default();
            let package = pack_plugin_package(
                &read_plugin_directory(directory.path(), PluginSourceKind::Explicit, &limits)
                    .unwrap(),
                &limits,
            )
            .unwrap();
            let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
            let router = build_router(self.api.clone());
            request(
                router.clone(),
                Some(&self.owner),
                "GET",
                &format!("{prefix}/plugins"),
                vec![],
                false,
                StatusCode::OK,
            )
            .await;
            let upload = request(
                router.clone(),
                Some(&self.owner),
                "POST",
                &format!("{prefix}/plugin-uploads"),
                package.bytes.clone(),
                true,
                StatusCode::OK,
            )
            .await;
            let installed = request(router, Some(&self.owner), "POST", &format!("{prefix}/plugins"),
                serde_json::to_vec(&json!({
                    "upload_id": upload["upload_id"], "alias": "controller", "operation_key": "install"
                })).unwrap(), false, StatusCode::OK).await;
            let installed: server_api::PluginOperationResponse =
                serde_json::from_value(installed).unwrap();
            *self.model_tool.lock().unwrap() = format!(
                "{}__{}",
                installed.installation.installation_id,
                installed.installation.artifacts[0].tools[0].name
            );
            self.configure(&installed, peer, true).await;
            (installed, package.bytes)
        }

        async fn configure(
            &self,
            installed: &server_api::PluginOperationResponse,
            peer: &Peer,
            selected: bool,
        ) {
            let artifact = &installed.installation.artifacts[0];
            let mut binding = peer.binding();
            binding.methods = vec![server_api::PluginHttpMethod::Get];
            binding.path_prefixes = vec!["/status".into()];
            let tool = &artifact.tools[0].name;
            let https = tool == TOOL;
            let plugins = json!({"enabled":{"controller":{
                "installation_id":installed.installation.installation_id,
                "expected_last_operation":installed.operation_id,
                "package_digest":artifact.package_digest,"artifact_digest":artifact.artifact_digest,
                "tools":[tool],"logging":false,"config":{},
                "connections":if https { json!({"api":"remote"}) } else { json!({}) }
            }},"bindings":if https { json!({"remote":binding}) } else { json!({}) }});
            let selection = if selected {
                json!({"selections":[{"installation_id":installed.installation.installation_id,
                    "tools":[tool],"logging":false,"https_slots":if https { vec!["api"] } else { vec![] }}]})
            } else {
                json!({"selections":[]})
            };
            // This deliberately minimal value Profile exercises project selection
            // without adding unrelated Subject startup/Jobs or credential access.
            let content = format!(
                r#"{{
                plugins = {};
                profile = {{ default_profile = "{PROFILE}"; entries = [{{
                    selector = "{PROFILE}"; label = "Controller"; description = "";
                    profile = {{ slug = "plugin-controller"; scope = "workspace_read";
                        delegation_scope = "workspace_read";
                        model = {{ scheme = "anthropic"; model_id = "scripted"; auth = {{ kind = "none"; }}; }};
                        plugins = {};
                    }};
                }}]; }};
            }}"#,
                crate::plugin_settings::render_value(&plugins).unwrap(),
                crate::plugin_settings::render_value(&selection).unwrap()
            );
            let state = self
                .api
                .config_store
                .load_workspace_config(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap();
            let path = config_source::VirtualPath::parse("main.dcdl").unwrap();
            let commit = ConfigCommitRequest {
                base_digest: state.snapshot.digest.clone(),
                entrypoints: state.contract.entrypoints.clone(),
                changes: vec![config_source::ConfigTreeChange::Update {
                    path: path.clone(),
                    expected_digest: state.snapshot.get(&path).unwrap().content_digest.clone(),
                    content,
                }],
            };
            request(
                build_router(self.api.clone()),
                Some(&self.owner),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/config/source-tree/commit"),
                serde_json::to_vec(&commit).unwrap(),
                false,
                StatusCode::CREATED,
            )
            .await;
        }

        async fn revoke(&self, installed: &server_api::PluginOperationResponse) {
            let revoked = request(
                build_router(self.api.clone()),
                Some(&self.owner),
                "POST",
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/plugins/{}/revoke",
                    installed.installation.installation_id
                ),
                serde_json::to_vec(&json!({
                    "expected_last_operation": installed.operation_id,
                    "operation_key": "controller-revoke",
                    "artifact_digest": installed.installation.artifacts[0].artifact_digest,
                    "reason": "Withdraw controller artifact"
                }))
                .unwrap(),
                false,
                StatusCode::OK,
            )
            .await;
            assert_eq!(revoked["installation"]["artifacts"][0]["revoked"], true);
        }

        fn cached_artifact(&self, package: &[u8]) -> PathBuf {
            let cache = self
                .api
                .config
                .embedded_runtime_store_root
                .with_extension("plugin-cache");
            let files = fs::read_dir(cache)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("artifact"))
                .collect::<Vec<_>>();
            assert_eq!(files.len(), 1);
            assert_eq!(fs::read(&files[0]).unwrap(), package);
            files[0].clone()
        }

        async fn restore(
            &self,
            worker: &RuntimeWorkerRef,
            request_id: &str,
        ) -> InternalWorkerRestoreResult {
            let api = self.api.clone();
            let worker = worker.clone();
            let request_id = request_id.to_string();
            tokio::time::timeout(
                WATCHDOG,
                tokio::task::spawn_blocking(move || {
                    let observation = api
                        .runtime
                        .worker(&worker)
                        .unwrap()
                        .restore_observation_token
                        .unwrap();
                    api.restore_workspace_worker(
                        &worker,
                        server_api::WorkerRestoreRequest {
                            expected_observation_token: observation,
                            request_id,
                        },
                    )
                }),
            )
            .await
            .expect("Controller restore exceeded the fixture deadline")
            .unwrap()
            .unwrap()
        }

        async fn create(&self, profile: &str) -> RuntimeWorkerRef {
            let Json(created) = create_workspace_worker(
                State(self.api.clone()),
                HeaderMap::new(),
                Json(create_request(profile)),
            )
            .await
            .unwrap();
            RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
        }

        fn aggregate(&self, worker: &RuntimeWorkerRef) -> PathBuf {
            self.api
                .config
                .embedded_runtime_store_root
                .join("workers")
                .join(&worker.worker_id)
        }

        fn execution_request(&self, worker: &RuntimeWorkerRef) -> Value {
            let saved: Value = serde_json::from_slice(
                &fs::read(self.aggregate(worker).join("worker.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(saved["execution_state"]["state"], "available");
            saved["execution_state"]["execution"]["request"].clone()
        }

        fn saved_history(
            &self,
            worker: &RuntimeWorkerRef,
        ) -> (session_store::SessionId, Vec<agen::Item>) {
            let reopened =
                session_store::WorkerSessionStore::new(self.aggregate(worker).join("session"))
                    .unwrap();
            let session = reopened.session_id().unwrap().unwrap();
            let segments = reopened.list_segments(session).unwrap();
            assert_eq!(segments.len(), 1);
            let saved = session_store::restore(&reopened, session, segments[0]).unwrap();
            (session, saved.history)
        }

        fn count(&self, table: &str) -> i64 {
            assert!(
                [
                    "plugin_invocations",
                    "plugin_effect_receipts",
                    "plugin_execution_plans"
                ]
                .contains(&table)
            );
            self.api
                .config_store
                .with_conn(|conn| {
                    Ok(conn.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE workspace_id=?1"),
                        [TEST_WORKSPACE_ID],
                        |row| row.get(0),
                    )?)
                })
                .unwrap()
        }

        fn effect_identity(&self, worker: &RuntimeWorkerRef) -> (String, String, String) {
            self.api
                .config_store
                .with_conn(|conn| {
                    Ok(conn.query_row(
                "SELECT r.invocation_id,r.effect_key,r.fingerprint FROM plugin_effect_receipts r \
                 JOIN plugin_invocations i USING(workspace_id,invocation_id) \
                 WHERE i.workspace_id=?1 AND i.worker_id=?2",
                rusqlite::params![TEST_WORKSPACE_ID, worker.worker_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?)
                })
                .unwrap()
        }

        async fn stop(&self, worker: &RuntimeWorkerRef) {
            let api = self.api.clone();
            let worker = worker.clone();
            tokio::task::spawn_blocking(move || {
                api.runtime.stop_worker(
                    &worker,
                    WorkerLifecycleRequest {
                        reason: None,
                        ticket_assignment: None,
                    },
                )
            })
            .await
            .unwrap()
            .unwrap();
        }

        async fn close(self) {
            shutdown_workspace_feature_storage(&self.api).await.unwrap();
        }
    }

    fn create_request(profile: &str) -> CreateWorkspaceWorkerRequest {
        CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "Plugin controller".into(),
            singleton_key: None,
            profile: Some(profile.into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }
    }

    async fn snapshot(connection: &mut WorkspaceWorkerProtocolConnection) -> protocol::Greeting {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                if let protocol::Event::Snapshot {
                    greeting, state, ..
                } = connection.events.recv().await.unwrap()
                {
                    assert_eq!(state.state, protocol::WorkerState::Idle);
                    return greeting;
                }
            }
        })
        .await
        .expect("real Controller did not publish a snapshot")
    }

    async fn submit(connection: &mut WorkspaceWorkerProtocolConnection) {
        connection
            .methods
            .send(protocol::Method::Submit {
                submission_request_id: "controller-https-submit".into(),
                input: vec![protocol::Segment::Text {
                    content: "Check the service status.".into(),
                }],
            })
            .await
            .unwrap();
    }

    async fn submit_and_finish(connection: &mut WorkspaceWorkerProtocolConnection) {
        submit(connection).await;
        finish(connection, protocol::RunResult::Finished).await;
    }

    async fn finish(
        connection: &mut WorkspaceWorkerProtocolConnection,
        expected: protocol::RunResult,
    ) {
        tokio::time::timeout(WATCHDOG, async {
            let mut accepted = false;
            let mut finished = false;
            loop {
                match connection.events.recv().await.unwrap() {
                    protocol::Event::SubmissionAccepted {
                        submission_request_id,
                        ..
                    } => {
                        assert_eq!(submission_request_id, "controller-https-submit");
                        accepted = true;
                    }
                    protocol::Event::SubmissionRejected { message, .. } => {
                        panic!("Submit rejected: {message}")
                    }
                    protocol::Event::RunEnd { result } => {
                        assert!(accepted);
                        assert_eq!(result, expected);
                        finished = true;
                    }
                    protocol::Event::WorkerState { snapshot }
                        if finished
                            && snapshot.state == protocol::WorkerState::Idle
                            && snapshot.last_finished_submission_request_id.as_deref()
                                == Some("controller-https-submit") =>
                    {
                        return;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("real Controller HTTPS submission did not finish");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_diagnostics_observes_saved_pin_and_live_refusal_without_execution_or_paths() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(&peer).await;
        let (installed, _) = f.install(&peer).await;
        let worker = f.create(PROFILE).await;
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/plugins/worker-diagnostics/{}/{}",
            worker.runtime_id, worker.worker_id
        );
        let router = build_router(f.api.clone());
        let reader = seed_test_api_token(f.api.store.as_ref(), "plugin-diagnostics-reader");
        request(
            router.clone(),
            None,
            "GET",
            &path,
            vec![],
            false,
            StatusCode::UNAUTHORIZED,
        )
        .await;
        request(
            router.clone(),
            Some(&reader),
            "GET",
            &path,
            vec![],
            false,
            StatusCode::FORBIDDEN,
        )
        .await;
        for revoked in [false, true] {
            if revoked {
                f.revoke(&installed).await;
            }
            let value = request(
                router.clone(),
                Some(&f.owner),
                "GET",
                &path,
                vec![],
                false,
                StatusCode::OK,
            )
            .await;
            let text = value.to_string();
            for private in [
                HOST,
                f.owner.as_str(),
                f.api.config.embedded_runtime_store_root.to_str().unwrap(),
                "base_url",
                "secret_ref",
                "cache_path",
            ] {
                assert!(
                    !text.contains(private),
                    "diagnostic exposed private transport or credential information"
                );
            }
            let observed: server_api::PluginWorkerDiagnostics =
                serde_json::from_value(value).unwrap();
            let server_api::PluginWorkerDiagnosticSelection::Plan {
                pin_state, plugins, ..
            } = observed.selection
            else {
                panic!("expected saved plan")
            };
            assert_eq!(pin_state, server_api::PluginDiagnosticPinState::Active);
            assert_eq!(plugins.len(), 1);
            assert_eq!(
                plugins[0].installation_id,
                installed.installation.installation_id
            );
            assert_eq!(
                plugins[0].surfaces,
                vec![server_api::PluginDiagnosticSurface::Tool]
            );
            assert_eq!(plugins[0].selected_tools, vec![TOOL]);
            assert_eq!(plugins[0].connections[0].slot, "api");
            assert!(matches!(
                plugins[0].connections[0].effective_limits,
                server_api::PluginDiagnosticConnectionLimits::Available { .. }
            ));
            if revoked {
                assert!(matches!(
                    plugins[0].admission,
                    server_api::PluginDiagnosticAdmission::Refused {
                        reason_code: server_api::PluginHostErrorCode::Denied
                    }
                ));
            } else {
                assert_eq!(
                    plugins[0].admission,
                    server_api::PluginDiagnosticAdmission::Admissible
                );
            }
            assert_eq!(f.count("plugin_invocations"), 0);
            assert_eq!(f.count("plugin_effect_receipts"), 0);
            assert_eq!(peer.connection_count(), 0);
            assert!(f.requests.lock().unwrap().is_empty());
        }
        f.stop(&worker).await;
        f.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_builtin_create_uses_profile_factory_and_seals_empty_plugin_selection() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(&peer).await;
        let worker = f.create("builtin:companion").await;
        assert_eq!(f.model_clients.load(Ordering::SeqCst), 1);
        assert!(f.requests.lock().unwrap().is_empty());
        let selected = f
            .api
            .config_store
            .with_conn(|conn| {
                crate::plugin_plan::worker_launch_selection(
                    conn,
                    TEST_WORKSPACE_ID,
                    &worker.runtime_id,
                    &worker.worker_id,
                )
            })
            .unwrap();
        assert!(matches!(selected, Some(None)));
        assert!(f.execution_request(&worker)["plugin_execution"].is_null());
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(
            !snapshot(&mut connection)
                .await
                .tools
                .iter()
                .any(|tool| tool.ends_with("__example_status"))
        );
        assert_eq!(f.count("plugin_execution_plans"), 0);
        assert_eq!(peer.connection_count(), 0);
        drop(connection);
        f.stop(&worker).await;
        f.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_project_controller_https_history_and_same_plan_restore_refetch_cold_cache() {
        let mut peer = Peer::new(HOST, vec![response(503, &[], b"peer data")]).await;
        let f = Fixture::new(&peer).await;
        let (installed, package) = f.install(&peer).await;
        let public_name = f.model_tool.lock().unwrap().clone();
        let worker = f.create(PROFILE).await;
        let saved = f
            .api
            .config_store
            .with_conn(|conn| {
                crate::plugin_plan::worker_launch_plan(
                    conn,
                    TEST_WORKSPACE_ID,
                    &worker.runtime_id,
                    &worker.worker_id,
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(saved.pin_state, PlanPinState::Active);
        assert_eq!(saved.plan.plugins().len(), 1);
        assert_eq!(
            saved.plan.plugins()[0].installation_id(),
            installed.installation.installation_id
        );
        let expected = runtime_api::PluginExecutionSnapshot {
            plan_id: saved.plan_id.clone(),
            fingerprint: saved.fingerprint.clone(),
            plan: saved.plan.clone(),
        };
        assert_eq!(
            f.execution_request(&worker)["plugin_execution"],
            serde_json::to_value(&expected).unwrap()
        );
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        let greeting = snapshot(&mut connection).await;
        assert!(greeting.tools.contains(&public_name));
        assert!(!greeting.tools.iter().any(|tool| tool == TOOL));
        assert_eq!(
            f.count("plugin_invocations"),
            0,
            "preparation must not invoke a Tool"
        );
        submit_and_finish(&mut connection).await;
        let effect_states: Vec<(String,Option<String>)> = f.api.config_store.with_conn(|conn| {
            let mut statement = conn.prepare("SELECT state,json_extract(error_json,'$.code') FROM plugin_effect_receipts WHERE workspace_id=?1")?;
            Ok(statement.query_map([TEST_WORKSPACE_ID], |row| Ok((row.get(0)?,row.get(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).unwrap();
        assert_eq!(
            effect_states,
            vec![("completed".into(), None)],
            "HTTPS receipt observation before history assertions"
        );
        {
            let requests = f.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(
                requests[0]
                    .tools
                    .iter()
                    .any(|tool| tool.name == public_name)
            );
            assert_https_history(&requests[1].items, &public_name);
        }
        assert!(
            String::from_utf8(
                tokio::time::timeout(WATCHDOG, peer.observed_within(WATCHDOG))
                    .await
                    .unwrap()
            )
            .unwrap()
            .starts_with("GET /status HTTP/1.1\r\n")
        );
        assert_eq!(peer.connection_count(), 1);
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        drop(connection);
        f.stop(&worker).await;
        let (session, history) = f.saved_history(&worker);
        assert_https_history(&history, &public_name);
        let (consumer, state, status, body): (String, String, i64, Vec<u8>) = f.api.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT i.consumer_json,r.state,r.response_status,r.response_body FROM plugin_invocations i \
                JOIN plugin_effect_receipts r USING(workspace_id,invocation_id) \
                WHERE i.workspace_id=?1 AND i.plan_id=?2 AND i.worker_id=?3 AND r.effect_key='status-v1'",
                rusqlite::params![TEST_WORKSPACE_ID, saved.plan_id, worker.worker_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?)
        }).unwrap();
        let identity = serde_json::to_vec(&(session.to_string(), "https-call")).unwrap();
        let consumer_id = format!(
            "tool-{}",
            sha2::Sha256::digest(identity)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert_eq!(
            serde_json::from_str::<server_api::PluginConsumer>(&consumer).unwrap(),
            server_api::PluginConsumer::ToolCall {
                tool_call_id: consumer_id,
                tool_name: TOOL.into()
            }
        );
        assert_eq!(state, "completed");
        assert_eq!(status, 503);
        assert_eq!(body, b"peer data");

        // Current Profile now selects nothing. Normal restore must not resolve
        // it again, nor treat previously cached bytes as a restored selection.
        f.configure(&installed, &peer, false).await;
        let current = f
            .api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let reselected = f
            .api
            .config_store
            .with_immediate_transaction(|tx| {
                crate::plugin_settings::resolve_worker_plugin_plan(
                    tx,
                    TEST_WORKSPACE_ID,
                    &current.snapshot.digest,
                    PROFILE,
                    saved.plan.execution().clone(),
                )
            })
            .unwrap();
        assert!(reselected.plugins().is_empty());
        let cache_file = f.cached_artifact(&package);
        fs::remove_file(&cache_file).unwrap();
        assert!(!cache_file.exists());
        let restored = f.restore(&worker, "same-plan-controller-restore").await;
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        assert_eq!(f.model_clients.load(Ordering::SeqCst), 2);
        assert_eq!(
            f.requests.lock().unwrap().len(),
            2,
            "restore must not submit another model turn"
        );
        assert_eq!(
            fs::read(&cache_file).unwrap(),
            package,
            "cold restore must refetch and verify the exact package"
        );
        assert_eq!(
            f.execution_request(&worker)["plugin_execution"],
            serde_json::to_value(&expected).unwrap()
        );
        let retained = f
            .api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &saved.plan_id)
            .unwrap();
        assert_eq!(retained.pin_state, PlanPinState::Active);
        assert_eq!(retained.fingerprint, saved.fingerprint);
        assert_eq!(retained.plan, saved.plan);
        assert_eq!(
            f.count("plugin_execution_plans"),
            1,
            "restore must not acquire a new plan"
        );
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(
            snapshot(&mut connection).await.tools.contains(&public_name),
            "restored Controller must publish the saved Tool even though current selection is empty"
        );
        assert_eq!(
            peer.connection_count(),
            1,
            "restore preparation must not repeat the HTTP effect"
        );
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        drop(connection);
        f.stop(&worker).await;
        let (restored_session, history) = f.saved_history(&worker);
        assert_eq!(restored_session, session);
        assert_https_history(&history, &public_name);
        f.close().await;
    }

    fn assert_failed_tool_history(items: &[agen::Item], public_name: &str) {
        let calls = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolCall { call_id, name, .. } => {
                    Some((call_id.as_str(), name.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls, vec![("https-call", public_name)]);
        let results = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolResult {
                    call_id,
                    disposition,
                    is_error,
                    content,
                    ..
                } => Some((call_id.as_str(), disposition, is_error, content)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            results.len(),
            1,
            "denial must be persisted as the ordinary Tool result"
        );
        assert_eq!(results[0].0, "https-call");
        assert_ne!(*results[0].1, agen::ToolResultDisposition::Success);
        assert!(*results[0].2);
        assert!(
            !results[0].3.as_deref().unwrap_or("").contains("peer data"),
            "a fenced response must not leak into ordinary Tool history"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_pure_and_https_registered_tool_rechecks_revoke_and_persists_denial() {
        for https in [false, true] {
            let peer = Peer::new(HOST, vec![]).await;
            let f = Fixture::new(&peer).await;
            let (installed, package) = f.install_component(&peer, https).await;
            let worker = f.create(PROFILE).await;
            let public_name = f.model_tool.lock().unwrap().clone();
            let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
                .await
                .unwrap();
            assert!(snapshot(&mut connection).await.tools.contains(&public_name));
            let cache_file = f.cached_artifact(&package);
            f.revoke(&installed).await;
            submit_and_finish(&mut connection).await;
            {
                let requests = f.requests.lock().unwrap();
                assert_eq!(requests.len(), 2);
                assert!(
                    requests[0]
                        .tools
                        .iter()
                        .any(|tool| tool.name == public_name)
                );
                assert_failed_tool_history(&requests[1].items, &public_name);
                assert!(
                    requests[1].items.iter().any(|item| matches!(item,
                agen::Item::ToolResult { summary, .. }
                    if summary.contains("Plugin live admission refused or unavailable"))),
                    "revoke must fail at live admission, not at schema validation or missing Tool lookup"
                );
            }
            assert_eq!(
                f.count("plugin_invocations"),
                0,
                "registered/cached must not authorize a fresh call"
            );
            assert_eq!(f.count("plugin_effect_receipts"), 0);
            assert_eq!(peer.connection_count(), 0);
            assert_eq!(
                fs::read(cache_file).unwrap(),
                package,
                "revoke denial must not rely on removing bytes"
            );
            drop(connection);
            f.stop(&worker).await;
            assert_failed_tool_history(&f.saved_history(&worker).1, &public_name);
            f.close().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_dispatched_https_revoke_withholds_late_response_from_model_and_saved_history()
    {
        use crate::plugin_https::test_support::Reply;
        let (release, response_ready) = tokio::sync::oneshot::channel();
        let mut peer = Peer::new(HOST, vec![Reply::Gated(response_ready)]).await;
        let f = Fixture::new(&peer).await;
        let (installed, _) = f.install(&peer).await;
        let worker = f.create(PROFILE).await;
        let public_name = f.model_tool.lock().unwrap().clone();
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(snapshot(&mut connection).await.tools.contains(&public_name));
        submit(&mut connection).await;
        let observed = tokio::time::timeout(WATCHDOG, peer.observed_within(WATCHDOG))
            .await
            .unwrap();
        assert!(
            String::from_utf8(observed)
                .unwrap()
                .starts_with("GET /status HTTP/1.1\r\n")
        );
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(
            f.count("plugin_effect_receipts"),
            1,
            "dispatch must already have its durable effect key"
        );
        f.revoke(&installed).await;
        // The request has already reached the external peer. Release a response
        // only after revocation commits: this asserts fencing, not rollback.
        release
            .send(match response(503, &[], b"peer data") {
                Reply::Bytes(bytes) => bytes,
                _ => unreachable!(),
            })
            .unwrap();
        finish(&mut connection, protocol::RunResult::Finished).await;
        {
            let requests = f.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_failed_tool_history(&requests[1].items, &public_name);
        }
        assert_eq!(
            peer.connection_count(),
            1,
            "no fresh effect key may resend a revoked call"
        );
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        drop(connection);
        f.stop(&worker).await;
        assert_failed_tool_history(&f.saved_history(&worker).1, &public_name);
        f.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_controller_cancel_closes_inflight_https_and_restore_does_not_resend() {
        use crate::plugin_https::test_support::Reply;
        let mut peer = Peer::new(HOST, vec![Reply::Stall]).await;
        let f = Fixture::new(&peer).await;
        f.install(&peer).await;
        let worker = f.create(PROFILE).await;
        let public_name = f.model_tool.lock().unwrap().clone();
        let persisted = f.execution_request(&worker)["plugin_execution"].clone();
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(snapshot(&mut connection).await.tools.contains(&public_name));
        submit(&mut connection).await;
        tokio::time::timeout(WATCHDOG, peer.observed_within(WATCHDOG))
            .await
            .unwrap();
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        let effect = f.effect_identity(&worker);
        assert_eq!(effect.1, "status-v1");
        connection
            .methods
            .send(protocol::Method::Cancel {
                command: protocol::WorkerCommandEnvelope::new(901),
            })
            .await
            .unwrap();
        finish(&mut connection, protocol::RunResult::Cancelled).await;
        peer.closed().await;
        // The Controller bounds cancellation acknowledgement. If the owner call
        // misses that deadline, cancellation is unknown, not confirmed. In both
        // cases the already-admitted effect must remain consumed and may never
        // expose a successful response or be re-keyed on restore.
        let (state, response, error): (String, Option<u16>, Option<String>) = f.api.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT state,response_status,error_json FROM plugin_effect_receipts WHERE workspace_id=?1 AND invocation_id=?2 AND effect_key='status-v1'",
                rusqlite::params![TEST_WORKSPACE_ID, effect.0], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?)
        }).unwrap();
        assert!(matches!(state.as_str(), "pending" | "completed"));
        assert!(
            response.is_none(),
            "cancelled submission must not publish an HTTPS success"
        );
        let error: server_api::PluginHostError = serde_json::from_str(&error.unwrap()).unwrap();
        assert_eq!(
            error.effect,
            server_api::PluginEffectObservation::Indeterminate
        );
        assert_eq!(
            f.requests.lock().unwrap().len(),
            1,
            "cancelled result must not start another model turn"
        );
        drop(connection);
        f.stop(&worker).await;
        let (session, history) = f.saved_history(&worker);
        assert_failed_tool_history(&history, &public_name);
        let restored = f.restore(&worker, "cancelled-controller-restore").await;
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        assert_eq!(f.execution_request(&worker)["plugin_execution"], persisted);
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(snapshot(&mut connection).await.tools.contains(&public_name));
        assert_eq!(
            f.requests.lock().unwrap().len(),
            1,
            "restore must not implicitly resume a cancelled submission"
        );
        assert_eq!(
            peer.connection_count(),
            1,
            "restore must not resend the indeterminate HTTP effect"
        );
        assert_eq!(
            f.effect_identity(&worker),
            effect,
            "restore must retain the original invocation/effect-key fingerprint"
        );
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        drop(connection);
        f.stop(&worker).await;
        let (restored_session, history) = f.saved_history(&worker);
        assert_eq!(restored_session, session);
        assert_failed_tool_history(&history, &public_name);
        f.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_pure_and_https_revoked_restore_refuses_warm_and_cold_saved_plan_without_controller()
     {
        for https in [false, true] {
            for cold in [false, true] {
                let peer = Peer::new(HOST, vec![]).await;
                let f = Fixture::new(&peer).await;
                let (installed, package) = f.install_component(&peer, https).await;
                let worker = f.create(PROFILE).await;
                let public_name = f.model_tool.lock().unwrap().clone();
                let persisted = f.execution_request(&worker)["plugin_execution"].clone();
                let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
                    .await
                    .unwrap();
                assert!(snapshot(&mut connection).await.tools.contains(&public_name));
                drop(connection);
                f.stop(&worker).await;
                let plan_id = persisted["plan_id"].as_str().unwrap();
                let before = f
                    .api
                    .config_store
                    .plugin_plan_load(TEST_WORKSPACE_ID, plan_id)
                    .unwrap();
                assert_ne!(
                    before.pin_state,
                    PlanPinState::Released,
                    "stop must retain the restorable pin"
                );
                let cache_file = f.cached_artifact(&package);
                if cold {
                    fs::remove_file(&cache_file).unwrap();
                }
                f.revoke(&installed).await;
                let restored = f.restore(&worker, "revoked-controller-restore").await;
                assert!(
                    matches!(
                        restored.state,
                        server_api::WorkerRestoreState::Rejected
                            | server_api::WorkerRestoreState::RolledBack
                    ),
                    "{https}/{cold}: {restored:?}"
                );
                assert!(
                    restored.diagnostics.iter().any(|diagnostic| diagnostic
                        .message
                        .contains("Plugin restore preparation refused or unavailable")),
                    "must fail in the production Plugin restore preparation, not an unrelated prerequisite: {restored:?}"
                );
                assert!(
                    connect_workspace_worker_protocol(&f.api, &worker, None)
                        .await
                        .is_err(),
                    "failed preparation must not publish a usable Controller"
                );
                assert_eq!(f.execution_request(&worker)["plugin_execution"], persisted);
                let after = f
                    .api
                    .config_store
                    .plugin_plan_load(TEST_WORKSPACE_ID, plan_id)
                    .unwrap();
                assert_eq!(
                    after.plan, before.plan,
                    "denial must not reselect current config"
                );
                assert_eq!(after.fingerprint, before.fingerprint);
                assert_eq!(
                    after.pin_state, before.pin_state,
                    "failed restore must not lose the pin"
                );
                assert_eq!(f.count("plugin_execution_plans"), 1);
                assert_eq!(f.count("plugin_invocations"), 0);
                assert_eq!(f.count("plugin_effect_receipts"), 0);
                assert!(
                    f.requests.lock().unwrap().is_empty(),
                    "prepare must not submit a model turn"
                );
                assert_eq!(peer.connection_count(), 0);
                if !cold {
                    assert_eq!(
                        fs::read(&cache_file).unwrap(),
                        package,
                        "warm cache must not bypass current admission"
                    );
                }
                f.close().await;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_project_create_refuses_missing_artifact_in_real_factory_and_releases_pin() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(&peer).await;
        let (installed, _) = f.install(&peer).await;
        // Keep catalog/config valid, but simulate unavailable authoritative bytes.
        // This is deliberately a delivery refusal, not a claim of policy denial.
        let digest = &installed.installation.artifacts[0].artifact_digest;
        let blob = ServerConfig::server_data_root_for_data_dir(&f.api.config.data_root)
            .join("plugin-artifacts/blobs")
            .join(digest.strip_prefix("sha256:").unwrap());
        assert!(blob.is_file());
        fs::remove_file(blob).unwrap();
        assert!(
            create_workspace_worker(
                State(f.api.clone()),
                HeaderMap::new(),
                Json(create_request(PROFILE))
            )
            .await
            .is_err()
        );
        assert_eq!(
            f.model_clients.load(Ordering::SeqCst),
            1,
            "failure must reach the production factory, not fail in Profile selection before dispatch"
        );
        assert!(f.requests.lock().unwrap().is_empty());
        let plans: Vec<(String, String)> = f.api.config_store.with_conn(|conn| {
            let mut statement = conn.prepare("SELECT plan_id,pin_state FROM plugin_execution_plans WHERE workspace_id=?1 AND actor=?2")?;
            Ok(statement.query_map(rusqlite::params![TEST_WORKSPACE_ID, crate::plugin_plan::WORKER_LAUNCH_ACTOR],
                |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?)
        }).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].1, "released");
        let saved = f
            .api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &plans[0].0)
            .unwrap();
        let worker = RuntimeWorkerRef::new(
            &saved.plan.execution().runtime_id,
            &saved.plan.execution().worker_id,
        );
        assert!(
            f.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .is_none()
        );
        assert!(
            f.api.runtime.worker(&worker).is_err(),
            "failed startup must not publish a Controller"
        );
        assert_eq!(f.count("plugin_invocations"), 0);
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        assert_eq!(peer.connection_count(), 0);
        f.close().await;
    }
}
