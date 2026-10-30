// Include directly in server::tests (this file owns its module).
// Production Embedded Server assembly -> ProfileRuntimeWorkerFactory -> actual
// Controller/Engine -> pinned Wasmtime Tool/Service -> HTTPS, Ticket outbox,
// SQLite receipts/cursors and Session store.
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
    use super::plugin_host_tests::plugin_runtime_tests::{
        Model, assert_https_history, persisted_call_consumers,
    };
    use super::plugin_tests::request;
    use super::*;
    use crate::plugin_https::test_support::{HOST, Peer, response};
    use crate::plugin_plan::PlanPinState;
    use session_store::Store;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    const PROFILE: &str = "project:plugin-controller";
    const TOOL: &str = "example_status";
    const WATCHDOG: Duration = Duration::from_secs(60);

    enum FixtureSurface {
        Tool,
        HttpsTool,
        Service,
    }

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
            Self::with_request_count(peer, 1).await
        }

        async fn with_request_count(peer: &Peer, request_count: usize) -> Self {
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
            let transport = Arc::new(peer.transport_for_requests(request_count));
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
            self.install_surface(
                peer,
                if https {
                    FixtureSurface::HttpsTool
                } else {
                    FixtureSurface::Tool
                },
            )
            .await
        }

        async fn install_surface(
            &self,
            peer: &Peer,
            surface: FixtureSurface,
        ) -> (server_api::PluginOperationResponse, Vec<u8>) {
            use manifest::plugin::*;
            let directory = tempfile::tempdir().unwrap();
            let (manifest, component): (&str, &[u8]) = match surface {
                FixtureSurface::Service => (
                    include_str!("../../plugin-runtime/tests/service-guest/plugin.toml"),
                    include_bytes!("../tests/fixtures/ticket-service-v2.component.wasm"),
                ),
                FixtureSurface::HttpsTool => (
                    include_str!(
                        "../../../resources/plugin/templates/rust-component-https/plugin.toml"
                    ),
                    include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm"),
                ),
                FixtureSurface::Tool => (
                    include_str!(
                        "../../../resources/plugin/templates/rust-component-tool/plugin.toml"
                    ),
                    include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
                ),
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
            if let Some(tool) = installed.installation.artifacts[0].tools.first() {
                *self.model_tool.lock().unwrap() =
                    format!("{}__{}", installed.installation.installation_id, tool.name,);
            }
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
            let service = artifact.services.first().map(|service| &service.name);
            let tool = artifact.tools.first().map(|tool| &tool.name);
            let https = tool.is_some_and(|tool| tool == TOOL);
            let tools = tool.into_iter().collect::<Vec<_>>();
            let services = service.into_iter().collect::<Vec<_>>();
            let config = if service.is_some() {
                json!({"mode":"normal"})
            } else {
                json!({})
            };
            let plugins = json!({"enabled":{"controller":{
                "installation_id":installed.installation.installation_id,
                "expected_last_operation":installed.operation_id,
                "package_digest":artifact.package_digest,"artifact_digest":artifact.artifact_digest,
                "tools":tools,"services":services,"ticket_events":service.is_some(),"logging":service.is_some(),"config":config,
                "connections":if https { json!({"api":"remote"}) } else { json!({}) }
            }},"bindings":if https { json!({"remote":binding}) } else { json!({}) }});
            let selection = if selected {
                json!({"selections":[{"installation_id":installed.installation.installation_id,
                    "tools":tools,"services":services,"ticket_events":service.is_some(),"logging":service.is_some(),"https_slots":if https { vec!["api"] } else { vec![] }}]})
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
            let (session, saved) = self.saved_state(worker);
            (session, saved.history)
        }

        fn saved_state(
            &self,
            worker: &RuntimeWorkerRef,
        ) -> (session_store::SessionId, session_store::RestoredState) {
            let reopened =
                session_store::WorkerSessionStore::new(self.aggregate(worker).join("session"))
                    .unwrap();
            let session = reopened.session_id().unwrap().unwrap();
            let segments = reopened.list_segments(session).unwrap();
            assert_eq!(segments.len(), 1);
            let saved = session_store::restore(&reopened, session, segments[0]).unwrap();
            (session, saved)
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
        submit_id(connection, "controller-https-submit").await;
    }

    async fn submit_id(connection: &mut WorkspaceWorkerProtocolConnection, request_id: &str) {
        connection
            .methods
            .send(protocol::Method::Submit {
                submission_request_id: request_id.into(),
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
        finish_id(connection, expected, "controller-https-submit").await;
    }

    async fn finish_id(
        connection: &mut WorkspaceWorkerProtocolConnection,
        expected: protocol::RunResult,
        request_id: &str,
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
                        assert_eq!(submission_request_id, request_id);
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
                                == Some(request_id) =>
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
    async fn repeated_provider_call_id_has_distinct_durable_receipts_and_completed_restore_never_reruns()
     {
        // Gemini also reuses a response-local id for the same Tool. Deliberately
        // emit the identical provider id in two explicit user submissions.
        let peer = Peer::new(
            HOST,
            vec![response(503, &[], b"old"), response(200, &[], b"fresh")],
        )
        .await;
        let f = Fixture::with_request_count(&peer, 2).await;
        f.install(&peer).await;
        let worker = f.create(PROFILE).await;
        let public_name = f.model_tool.lock().unwrap().clone();
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        snapshot(&mut connection).await;
        for request in ["first-explicit-call", "second-explicit-call"] {
            submit_id(&mut connection, request).await;
            finish_id(&mut connection, protocol::RunResult::Finished, request).await;
        }
        assert_eq!(
            peer.connection_count(),
            2,
            "a new logical call must not replay the first receipt; model requests: {:?}",
            f.requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| &r.items)
                .collect::<Vec<_>>()
        );
        assert_eq!(f.count("plugin_invocations"), 2);
        assert_eq!(f.count("plugin_effect_receipts"), 2);
        let statuses = |history: &[agen::Item]| -> Vec<u16> {
            history
                .iter()
                .filter_map(|item| match item {
                    agen::Item::ToolResult {
                        call_id,
                        is_error: false,
                        content: Some(content),
                        ..
                    } if call_id == "https-call" => Some(
                        serde_json::from_str::<Value>(content).unwrap()["structured"]["status"]
                            .as_u64()
                            .unwrap() as u16,
                    ),
                    _ => None,
                })
                .collect()
        };
        {
            let requests = f.requests.lock().unwrap();
            assert_eq!(requests.len(), 4);
            assert_eq!(statuses(&requests[3].items), [503, 200]);
        }
        drop(connection);
        f.stop(&worker).await;
        let (session, saved) = f.saved_state(&worker);
        assert_eq!(statuses(&saved.history), [503, 200]);
        let calls: Vec<_> = saved
            .annotated_history
            .iter()
            .filter_map(|entry| match &entry.item {
                session_store::LoggedItem::ToolCall { call_id, name, .. }
                    if name == &public_name =>
                {
                    Some((call_id, &entry.metadata.entry_id))
                }
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].0, calls[1].0,
            "provider id must actually be reused in the fixture"
        );
        assert_ne!(calls[0].1, calls[1].1);
        let mut expected = persisted_call_consumers(session, &saved, &public_name);
        expected.sort();
        let consumers: Vec<String> = f.api.config_store.with_conn(|conn| {
            let mut statement = conn.prepare("SELECT json_extract(consumer_json,'$.tool_call_id') FROM plugin_invocations WHERE workspace_id=?1 AND worker_id=?2 ORDER BY 1")?;
            Ok(statement.query_map(rusqlite::params![TEST_WORKSPACE_ID, worker.worker_id], |row| row.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).unwrap();
        assert_eq!(
            consumers, expected,
            "receipts must bind committed Host annotation ids, not response-local ids"
        );
        let restored = f.restore(&worker, "completed-reused-id-restore").await;
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        assert_eq!(f.requests.lock().unwrap().len(), 4);
        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        assert!(snapshot(&mut connection).await.tools.contains(&public_name));
        assert_eq!(peer.connection_count(), 2);
        assert_eq!(f.count("plugin_invocations"), 2);
        assert_eq!(f.count("plugin_effect_receipts"), 2);
        drop(connection);
        f.stop(&worker).await;
        let (restored_session, saved) = f.saved_state(&worker);
        assert_eq!(restored_session, session);
        let mut restored = persisted_call_consumers(session, &saved, &public_name);
        restored.sort();
        assert_eq!(restored, expected);
        assert_eq!(statuses(&saved.history), [503, 200]);
        f.close().await;
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
    async fn embedded_service_audited_recovery_wakes_same_controller_and_restore_keeps_cursor() {
        use crate::plugin_service::Consumer;
        use rusqlite::OptionalExtension;
        use server_api::{PluginServiceDiagnostics, PluginServicePauseReason};

        async fn until(
            phase: &str,
            observe: impl Fn() -> Option<Consumer>,
            ready: impl Fn(&Consumer) -> bool,
        ) -> Consumer {
            tokio::time::timeout(WATCHDOG, async {
                loop {
                    if let Some(consumer) = observe().filter(&ready) {
                        return consumer;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                let state = observe().map(|c| {
                    (
                        c.consumer_id,
                        c.cursor,
                        c.pause,
                        c.current_event_id,
                        c.attempt,
                        c.next_attempt_ms,
                        c.lease_token.is_some(),
                    )
                });
                panic!("Service {phase} did not converge: {state:?}");
            })
        }

        let peer = Peer::new(HOST, vec![]).await;
        let f = Fixture::new(&peer).await;
        let (installed, _) = f.install_surface(&peer, FixtureSurface::Service).await;
        let worker = f.create(PROFILE).await;
        let state = || {
            f.api.config_store.with_conn(|conn| {
                let id: Option<String> = conn.query_row(
                    "SELECT consumer_id FROM plugin_service_consumers WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
                    rusqlite::params![TEST_WORKSPACE_ID, worker.runtime_id, worker.worker_id],
                    |row| row.get(0),
                ).optional()?;
                id.map(|id| crate::plugin_service::load(conn, TEST_WORKSPACE_ID, &id)).transpose()
            }).unwrap()
        };
        let before = until("initial factory lease", &state, |c| c.lease_token.is_some()).await;
        let pin = f.execution_request(&worker)["plugin_execution"].clone();
        assert_eq!(pin["plan_id"], before.plan_id);
        assert_eq!(f.model_clients.load(Ordering::SeqCst), 1);
        let router = build_router(f.api.clone());
        let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
        let path = format!(
            "{prefix}/plugins/worker-diagnostics/{}/{}",
            worker.runtime_id, worker.worker_id
        );
        let observed = request(
            router.clone(),
            Some(&f.owner),
            "GET",
            &path,
            vec![],
            false,
            StatusCode::OK,
        )
        .await;
        let observed: server_api::PluginWorkerDiagnostics =
            serde_json::from_value(observed).unwrap();
        let server_api::PluginWorkerDiagnosticSelection::Plan {
            plan_id, plugins, ..
        } = observed.selection
        else {
            panic!("sealed Service launch selection missing");
        };
        assert_eq!(plan_id, before.plan_id);
        assert_eq!(plugins[0].selected_services, ["ticket_events"]);
        assert_eq!(
            plugins[0].service_consumers[0].diagnostics.consumer_id,
            before.consumer_id
        );
        assert_eq!(
            plugins[0].admission,
            server_api::PluginDiagnosticAdmission::Admissible
        );
        let consumer_path = format!("{prefix}/plugins/service-consumers/{}", before.consumer_id);
        let recover_path = format!("{prefix}/plugins/service-consumers/recover");
        request(
            router.clone(),
            None,
            "GET",
            &consumer_path,
            vec![],
            false,
            StatusCode::UNAUTHORIZED,
        )
        .await;
        let observed = request(
            router.clone(),
            Some(&f.owner),
            "GET",
            &consumer_path,
            vec![],
            false,
            StatusCode::OK,
        )
        .await;
        assert_eq!(observed["consumer_id"], before.consumer_id);
        assert_eq!(observed["lease_active"], true);
        assert!(!observed.to_string().contains("lease_token"));
        request(router.clone(), Some(&f.owner), "POST", &recover_path, serde_json::to_vec(&json!({
            "consumer_id":before.consumer_id,"expected_pause":"rejected","action":{"kind":"resume"},"operation_key":"not-paused","reason":"must not override an active consumer"
        })).unwrap(), false, StatusCode::CONFLICT).await;
        let unchanged = state().unwrap();
        assert_eq!(unchanged.cursor, before.cursor);
        assert_eq!(unchanged.lease_token, before.lease_token);
        assert!(unchanged.pause.is_none());
        assert_eq!(f.count("plugin_invocations"), 0);
        assert_eq!(peer.connection_count(), 0);
        assert!(f.requests.lock().unwrap().is_empty());
        assert_eq!(
            installed.installation.artifacts[0].services[0].name,
            "ticket_events"
        );

        // The real guest requests extension_write, which this events/logging-only
        // selection denies. Its RetryAfter exhausts the durable delivery budget.
        let ticket = browser_ticket_backend(&f.api)
            .unwrap()
            .create(ticket::NewTicket::new("Same Controller Service recovery"))
            .unwrap();
        let exhausted = until("initial attempts exhausted and released", &state, |c| {
            c.pause == Some(PluginServicePauseReason::AttemptsExhausted) && c.lease_token.is_none()
        })
        .await;
        let observed = request(
            router.clone(),
            Some(&f.owner),
            "GET",
            &consumer_path,
            vec![],
            false,
            StatusCode::OK,
        )
        .await;
        let paused: PluginServiceDiagnostics = serde_json::from_value(observed).unwrap();
        assert_eq!(paused.consumer_id, before.consumer_id);
        assert_eq!(
            paused.pause_reason,
            Some(PluginServicePauseReason::AttemptsExhausted)
        );
        assert!(!paused.lease_active);
        assert_eq!(paused.attempt, crate::plugin_service::MAX_ATTEMPTS);
        assert_eq!(paused.current_event_id, exhausted.current_event_id);
        let event_id = paused.current_event_id.as_ref().unwrap();
        let sequence: i64 = f.api.config_store.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT sequence FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND event_id=?2 AND ticket_id=?3",
                rusqlite::params![TEST_WORKSPACE_ID, event_id, ticket.id.as_str()],
                |row| row.get(0),
            )?)
        }).unwrap();
        assert!(sequence > before.cursor);
        assert_eq!(exhausted.cursor, before.cursor);
        assert!(exhausted.next_attempt_ms > 0);
        assert!(exhausted.invocation_id.is_some());
        assert_eq!(f.count("plugin_invocations"), 1);

        let resume_reason = "Retry the observed denied event on its existing Controller";
        let resumed = request(router.clone(), Some(&f.owner), "POST", &recover_path, serde_json::to_vec(&json!({
            "consumer_id":paused.consumer_id,"expected_pause":paused.pause_reason.unwrap(),
            "action":{"kind":"resume"},"operation_key":"controller-resume","reason":resume_reason
        })).unwrap(), false, StatusCode::OK).await;
        assert_eq!(resumed["consumer_id"], before.consumer_id);
        assert!(resumed["pause_reason"].is_null());
        assert_eq!(resumed["current_event_id"], event_id.as_str());

        // No stop/restore, Submit, manual Acquire, or replacement owner occurs
        // here. A later durable retry deadline proves new guest delivery rather
        // than accidentally accepting the already exhausted pre-recovery state.
        let retried = until(
            "same Controller recovery and renewed exhaustion",
            &state,
            |c| {
                c.consumer_id == before.consumer_id
                    && c.current_event_id.as_ref() == Some(event_id)
                    && c.pause == paused.pause_reason
                    && c.lease_token.is_none()
                    && c.next_attempt_ms > exhausted.next_attempt_ms
            },
        )
        .await;
        assert_eq!(retried.plan_id, before.plan_id);
        assert_eq!(retried.cursor, before.cursor);
        assert_eq!(retried.attempt, crate::plugin_service::MAX_ATTEMPTS);
        assert_eq!(retried.invocation_id, exhausted.invocation_id);
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.model_clients.load(Ordering::SeqCst), 1);
        let observed = request(
            router.clone(),
            Some(&f.owner),
            "GET",
            &consumer_path,
            vec![],
            false,
            StatusCode::OK,
        )
        .await;
        let paused_again: PluginServiceDiagnostics = serde_json::from_value(observed).unwrap();
        assert_eq!(paused_again, paused);

        let skip_reason = "Explicitly abandon the observed denied queue head";
        let skipped = request(router.clone(), Some(&f.owner), "POST", &recover_path, serde_json::to_vec(&json!({
            "consumer_id":paused_again.consumer_id,"expected_pause":paused_again.pause_reason.unwrap(),
            "action":{"kind":"skip","event_id":event_id},"operation_key":"controller-skip","reason":skip_reason
        })).unwrap(), false, StatusCode::OK).await;
        assert_eq!(skipped["consumer_id"], before.consumer_id);
        assert!(skipped["pause_reason"].is_null());
        assert!(skipped["current_event_id"].is_null());
        let checkpoint = until(
            "same Controller reacquisition after audited Skip",
            &state,
            |c| {
                c.consumer_id == before.consumer_id
                    && c.cursor == sequence
                    && c.pause.is_none()
                    && c.current_event_id.is_none()
                    && c.lease_token.is_some()
            },
        )
        .await;
        assert!(checkpoint.cursor > 0);
        assert_eq!(checkpoint.plan_id, before.plan_id);
        assert_ne!(checkpoint.lease_token, before.lease_token);
        assert_eq!(f.model_clients.load(Ordering::SeqCst), 1);

        let owner = f
            .api
            .store
            .get_workspace(TEST_WORKSPACE_ID)
            .await
            .unwrap()
            .unwrap()
            .owner_account_id;
        let audit: Vec<(String, String, String, String, String)> = f.api.config_store.with_conn(|conn| {
            Ok(conn.prepare(
                "SELECT operation_key,consumer_id,actor,reason,action_json FROM plugin_service_recovery_audit WHERE workspace_id=?1 ORDER BY operation_key",
            )?.query_map([TEST_WORKSPACE_ID], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?
                .collect::<rusqlite::Result<_>>()?)
        }).unwrap();
        assert_eq!(
            audit.len(),
            2,
            "the rejected no-op must not create an audit"
        );
        for (row, (key, reason, action)) in audit.iter().zip([
            ("controller-resume", resume_reason, json!({"kind":"resume"})),
            (
                "controller-skip",
                skip_reason,
                json!({"kind":"skip","event_id":event_id}),
            ),
        ]) {
            assert_eq!(row.0, key);
            assert_eq!(row.1, before.consumer_id);
            assert_eq!(
                serde_json::from_str::<Value>(&row.2).unwrap(),
                json!([owner, "user-plugin-controller-owner"])
            );
            assert_eq!(row.3, reason);
            assert_eq!(serde_json::from_str::<Value>(&row.4).unwrap(), action);
        }

        f.stop(&worker).await;
        assert!(state().unwrap().lease_token.is_none());
        f.restore(&worker, "restore-service").await;
        let restored = until("restored factory lease", &state, |c| {
            c.lease_token.is_some()
        })
        .await;
        assert_eq!(restored.consumer_id, checkpoint.consumer_id);
        assert_eq!(restored.plan_id, checkpoint.plan_id);
        assert_eq!(restored.cursor, checkpoint.cursor);
        assert!(restored.pause.is_none());
        assert!(restored.current_event_id.is_none());
        assert_ne!(restored.lease_token, checkpoint.lease_token);
        assert_eq!(f.execution_request(&worker)["plugin_execution"], pin);
        let retained = f
            .api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &restored.plan_id)
            .unwrap();
        assert_eq!(retained.pin_state, PlanPinState::Active);
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        assert_eq!(peer.connection_count(), 0);
        assert!(f.requests.lock().unwrap().is_empty());
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
        let consumers = persisted_call_consumers(session, &f.saved_state(&worker).1, &public_name);
        assert_eq!(consumers.len(), 1);
        let consumer_id = consumers[0].clone();
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
