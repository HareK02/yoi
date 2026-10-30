// Include directly in server::tests (this file owns its module):
//     include!("server_plugin_remote_controller_tests.rs");
// Reuses subject_authorization_tests' signed RuntimeHTTP/Controller harness.
// There is no second listener/scheduler/bootstrap implementation here. Only the
// external model and TLS peer DNS/socket/trust are substituted, not the guest,
// Remote Plugin providers, authorization, artifact delivery or effect receipts.
// Run: cargo test -p yoi-workspace-server --lib plugin_remote_controller_tests
// Prerequisites: openssl and the existing connected Runtime test environment.
mod plugin_remote_controller_tests {
    use super::plugin_host_tests::plugin_runtime_tests::{
        Model, assert_https_history, persisted_call_consumers,
    };
    use super::plugin_tests::request;
    use super::subject_authorization_tests::{CONNECTED_RUNTIME, ConnectedWorkerHarness};
    use super::*;
    use crate::plugin_https::test_support::{HOST, Peer, response};
    use crate::plugin_plan::PlanPinState;
    use agen::llm_client::{ClientError, LlmClient, Request as ModelRequest};
    use session_store::Store;
    use std::sync::Mutex;
    use std::time::Duration;

    const PROFILE: &str = "project:remote-plugin-controller";
    const TOOL: &str = "example_status";
    const WATCHDOG: Duration = Duration::from_secs(60);

    // Installation allocates the public namespace after the harness is started.
    // Defer only that name, reusing the existing scripted model implementation.
    #[derive(Clone)]
    struct InstalledToolModel {
        tool: Arc<Mutex<String>>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for InstalledToolModel {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: ModelRequest,
        ) -> std::result::Result<agen::llm_client::client::ResponseStream, ClientError> {
            let model = Model {
                tool_name: self.tool.lock().unwrap().clone(),
                requests: self.requests.clone(),
            };
            model.stream(request).await
        }
    }

    // Administrative setup only; all mutations pass the existing authenticated
    // upload/install/config handlers, with no manually saved plan or Worker row.
    async fn install_and_configure(
        f: &ConnectedWorkerHarness,
        peer: &Peer,
    ) -> (server_api::PluginOperationResponse, Vec<u8>) {
        use manifest::plugin::*;
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("plugin.toml"),
            include_str!("../../../resources/plugin/templates/rust-component-https/plugin.toml"),
        )
        .unwrap();
        fs::write(
            directory.path().join("plugin.component.wasm"),
            include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm"),
        )
        .unwrap();
        let limits = PluginPackageLimits::default();
        let package = pack_plugin_package(
            &read_plugin_directory(directory.path(), PluginSourceKind::Explicit, &limits).unwrap(),
            &limits,
        )
        .unwrap();
        let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
        let upload = request(
            build_router(f.api.clone()),
            Some(&f.owner),
            "POST",
            &format!("{prefix}/plugin-uploads"),
            package.bytes.clone(),
            true,
            StatusCode::OK,
        )
        .await;
        let (status, installed) = f
            .human(
                reqwest::Method::POST,
                &format!("{prefix}/plugins"),
                json!({"upload_id":upload["upload_id"], "alias":"remote_controller",
                "operation_key":"remote-controller-install"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{installed}");
        let installed: server_api::PluginOperationResponse =
            serde_json::from_value(installed).unwrap();
        let artifact = &installed.installation.artifacts[0];
        assert_eq!(artifact.tools[0].name, TOOL);
        let mut binding = peer.binding();
        binding.methods = vec![server_api::PluginHttpMethod::Get];
        binding.path_prefixes = vec!["/status".into()];
        let plugins = json!({"enabled":{"remote_controller":{
            "installation_id":installed.installation.installation_id,
            "expected_last_operation":installed.operation_id,
            "package_digest":artifact.package_digest, "artifact_digest":artifact.artifact_digest,
            "tools":[TOOL], "logging":false, "config":{}, "connections":{"api":"remote"}
        }}, "bindings":{"remote":binding}});
        let selection = json!({"selections":[{
            "installation_id":installed.installation.installation_id,
            "tools":[TOOL], "logging":false, "https_slots":["api"]
        }]});
        let content = format!(
            r#"{{
            plugins = {};
            profile = {{ default_profile = "{PROFILE}"; entries = [{{
                selector = "{PROFILE}"; label = "Remote controller"; description = "";
                profile = {{ slug = "remote-plugin-controller"; scope = "workspace_read";
                    delegation_scope = "workspace_read";
                    model = {{ scheme = "anthropic"; model_id = "scripted"; auth = {{ kind = "none"; }}; }};
                    plugins = {};
                }};
            }}]; }};
        }}"#,
            crate::plugin_settings::render_value(&plugins).unwrap(),
            crate::plugin_settings::render_value(&selection).unwrap()
        );
        let state = f
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
        let (status, result) = f
            .human(
                reqwest::Method::POST,
                &format!("{prefix}/config/source-tree/commit"),
                serde_json::to_value(commit).unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{result}");
        (installed, package.bytes)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 6)]
    async fn remote_runtime_http_create_runs_wasmtime_https_and_persists_ordinary_tool_history() {
        let mut peer = Peer::new(HOST, vec![response(503, &[], b"peer data")]).await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = Arc::new(Mutex::new(String::new()));
        let f = ConnectedWorkerHarness::start_with_plugins(
            InstalledToolModel {
                tool: tool.clone(),
                requests: requests.clone(),
            },
            Arc::new(peer.transport()),
        )
        .await;
        assert!(
            !f.api
                .store
                .is_hosting_admin("user-connected-authorization-owner")
                .unwrap(),
            "Plugin setup must use a non-operator Workspace owner"
        );
        let (installed, package) = install_and_configure(&f, &peer).await;
        let public_name = format!("{}__{TOOL}", installed.installation.installation_id);
        *tool.lock().unwrap() = public_name.clone();
        let (status, created) = tokio::time::timeout(
            WATCHDOG,
            f.human(
                reqwest::Method::POST,
                &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
                json!({"runtime_id":CONNECTED_RUNTIME, "display_name":"Remote Plugin controller",
                "profile":PROFILE, "initial_submit":[], "workdir_attachments":[],
                "feature_connections":{}}),
            ),
        )
        .await
        .expect("signed RuntimeHTTP create timed out");
        assert_eq!(status, StatusCode::OK, "{created}");
        let worker = RuntimeWorkerRef::new(
            created["runtime_id"].as_str().unwrap(),
            created["worker_id"].as_str().unwrap(),
        );
        assert_eq!(worker.runtime_id, CONNECTED_RUNTIME);
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
        let aggregate = f.runtime_root.join("workers").join(&worker.worker_id);
        let persisted: Value =
            serde_json::from_slice(&fs::read(aggregate.join("worker.json")).unwrap()).unwrap();
        assert_eq!(persisted["execution_state"]["state"], "available");
        assert_eq!(
            persisted["execution_state"]["execution"]["request"]["plugin_execution"],
            serde_json::to_value(runtime_api::PluginExecutionSnapshot {
                plan_id: saved.plan_id.clone(),
                fingerprint: saved.fingerprint.clone(),
                plan: saved.plan.clone(),
            })
            .unwrap()
        );
        let cache_files = fs::read_dir(f.runtime_root.with_extension("plugin-cache"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("artifact"))
            .collect::<Vec<_>>();
        assert_eq!(cache_files.len(), 1);
        assert_eq!(
            fs::read(&cache_files[0]).unwrap(),
            package,
            "Remote delivery must resolve the exact saved pin"
        );
        assert_eq!(
            peer.connection_count(),
            0,
            "controller preparation must not invoke the guest Tool"
        );
        assert!(requests.lock().unwrap().is_empty());

        let mut connection = connect_workspace_worker_protocol(&f.api, &worker, None)
            .await
            .unwrap();
        let greeting = tokio::time::timeout(WATCHDOG, async {
            loop {
                if let protocol::Event::Snapshot {
                    greeting, state, ..
                } = connection.events.recv().await.unwrap()
                {
                    assert_eq!(state.state, protocol::WorkerState::Idle);
                    break greeting;
                }
            }
        })
        .await
        .expect("Remote Controller did not publish its snapshot");
        assert!(greeting.tools.contains(&public_name));
        assert!(!greeting.tools.iter().any(|name| name == TOOL));
        connection
            .methods
            .send(protocol::Method::Submit {
                submission_request_id: "remote-controller-https-submit".into(),
                input: vec![protocol::Segment::Text {
                    content: "Check the service status.".into(),
                }],
            })
            .await
            .unwrap();
        tokio::time::timeout(WATCHDOG, async {
            let mut accepted = false;
            let mut finished = false;
            loop {
                match connection.events.recv().await.unwrap() {
                    protocol::Event::SubmissionAccepted {
                        submission_request_id,
                        ..
                    } => {
                        assert_eq!(submission_request_id, "remote-controller-https-submit");
                        accepted = true;
                    }
                    protocol::Event::SubmissionRejected { message, .. } => {
                        panic!("Submit rejected: {message}")
                    }
                    protocol::Event::RunEnd { result } => {
                        assert!(accepted);
                        assert_eq!(result, protocol::RunResult::Finished);
                        finished = true;
                    }
                    protocol::Event::WorkerState { snapshot }
                        if finished
                            && snapshot.state == protocol::WorkerState::Idle
                            && snapshot.last_finished_submission_request_id.as_deref()
                                == Some("remote-controller-https-submit") =>
                    {
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("Remote Controller HTTPS submission did not finish");
        {
            let observed = requests.lock().unwrap();
            assert_eq!(observed.len(), 2);
            assert!(
                observed[0]
                    .tools
                    .iter()
                    .any(|definition| definition.name == public_name)
            );
            assert_https_history(&observed[1].items, &public_name);
        }
        assert!(
            String::from_utf8(
                tokio::time::timeout(WATCHDOG, peer.observed())
                    .await
                    .unwrap()
            )
            .unwrap()
            .starts_with("GET /status HTTP/1.1\r\n")
        );
        assert_eq!(peer.connection_count(), 1);
        drop(connection);
        let api = f.api.clone();
        let stopped = worker.clone();
        tokio::time::timeout(
            WATCHDOG,
            tokio::task::spawn_blocking(move || {
                api.runtime.stop_worker(
                    &stopped,
                    WorkerLifecycleRequest {
                        reason: None,
                        ticket_assignment: None,
                    },
                )
            }),
        )
        .await
        .expect("Remote Controller stop timed out")
        .unwrap()
        .unwrap();

        // Reopen the Runtime's normal filesystem Session store, not a model-only
        // observation or an injected Session entry, after Controller shutdown.
        let reopened = session_store::WorkerSessionStore::new(aggregate.join("session")).unwrap();
        let session = reopened.session_id().unwrap().unwrap();
        let segments = reopened.list_segments(session).unwrap();
        assert_eq!(segments.len(), 1);
        let history = session_store::restore(&reopened, session, segments[0]).unwrap();
        assert_https_history(&history.history, &public_name);
        let (invocations, receipts): (i64, i64) = f
            .api
            .config_store
            .with_conn(|conn| {
                Ok((
                    conn.query_row(
                        "SELECT count(*) FROM plugin_invocations WHERE workspace_id=?1",
                        [TEST_WORKSPACE_ID],
                        |row| row.get(0),
                    )?,
                    conn.query_row(
                        "SELECT count(*) FROM plugin_effect_receipts WHERE workspace_id=?1",
                        [TEST_WORKSPACE_ID],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!((invocations, receipts), (1, 1));
        let (consumer, state, status, body): (String, String, i64, Vec<u8>) = f.api.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT i.consumer_json,r.state,r.response_status,r.response_body FROM plugin_invocations i \
                JOIN plugin_effect_receipts r USING(workspace_id,invocation_id) \
                WHERE i.workspace_id=?1 AND i.plan_id=?2 AND i.worker_id=?3 AND r.effect_key='status-v1'",
                rusqlite::params![TEST_WORKSPACE_ID, saved.plan_id, worker.worker_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?)
        }).unwrap();
        let consumers = persisted_call_consumers(session, &history, &public_name);
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
        shutdown_workspace_feature_storage(&f.api).await.unwrap();
    }
}
