// Included by server_plugin_host_tests.rs. These tests connect the production
// pinned Runtime bridge, Feature planner/installer, ToolRegistry, Engine and
// filesystem Session history to the real catalog/config/plan, signed Host owner,
// artifact delivery, Wasmtime HTTPS Component, TLS peer and SQLite receipts.
// Only the external model and the peer's DNS/socket/trust are substituted.
// Fixture's Worker creation still uses WorkdirlessFixtureRuntime: this is NOT a
// complete create-controller/restore vertical or a Runtime lifecycle test.
// Run: cargo test -p yoi-workspace-server --lib plugin_runtime_tests
// Prerequisite: openssl, as required by the shared Peer fixture.
pub(super) mod plugin_runtime_tests {
    use super::*;
    use agen::llm_client::event::{Event as ModelEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, LlmClient, Request as ModelRequest};
    use agen::tool::ToolExecutionContext;
    use session_store::Store;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;
    use worker::feature::{FeatureId, FeatureInstallPlan, FeatureRegistryBuilder};
    use worker_runtime::identity::WorkerRef;
    use worker_runtime::plugin::{
        PinnedPluginToolRuntime, PluginRuntimeProviders, PluginRuntimeServices, PluginToolRuntime,
    };
    use yoi_plugin_runtime::delivery::ArtifactSource;

    // This factory binds actual adapters to the fixture's exact authenticated
    // Worker; it supplies neither guest behavior nor admission responses.
    struct RuntimeProviders {
        worker: WorkerRef,
        artifacts: Arc<dyn ArtifactSource>,
        owner: Arc<dyn PluginHostOwner>,
    }

    #[async_trait::async_trait]
    impl PluginRuntimeProviders for RuntimeProviders {
        async fn connect(
            &self,
            workspace: &str,
            worker: &WorkerRef,
        ) -> std::result::Result<PluginRuntimeServices, String> {
            assert_eq!(workspace, TEST_WORKSPACE_ID);
            assert_eq!(worker, &self.worker);
            Ok(PluginRuntimeServices {
                artifacts: self.artifacts.clone(),
                owner: self.owner.clone(),
            })
        }
    }

    // Artifact delivery needs its own registered HTTP audience. The Host suite's
    // SignedAuthorizer deliberately uses "server-test", so keep its existing
    // owner helper and use a separately URL-bound router for delivery. Both
    // adapters reach the same real Server stores and execution identity owner.
    async fn artifact_source(
        f: &Fixture,
        remote: bool,
    ) -> (Arc<dyn ArtifactSource>, Option<ServerTask>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mut api = f.api.clone();
        api.config.backend_base_url = Some(url.clone());
        let signing = api
            .signing_identities
            .provision_existing(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        let bundle = project_workspace_signing_identity(&api, signing)
            .unwrap()
            .public_bundle
            .unwrap();
        if !remote {
            return (
                Arc::new(api.plugin_artifact_source(f.source.clone()).await.unwrap()),
                None,
            );
        }
        let (_, registration) = worker_runtime::workspace_issuer::add_workspace_issuer_trust(
            &mut Vec::new(),
            bundle,
            worker_runtime::auth::unix_now_seconds() as i64,
        )
        .unwrap();
        let client = worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
            TEST_WORKSPACE_ID,
            &url,
            &f.source.runtime_id,
        )
        .with_runtime_request_source(&f.identity, &url);
        let source =
            worker_runtime::workspace_request::plugin_delivery::RemotePluginArtifactSource::new(
                client,
                f.source.worker_id.clone(),
                Arc::new(move || Ok(registration.clone())),
            )
            .unwrap();
        let router = build_router(api);
        let server = ServerTask(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        (Arc::new(source), Some(server))
    }

    fn allow_artifact_read(f: &Fixture) {
        // Preserve the shared Host policy and grant the exact additional
        // operation that real artifact delivery checks; no broad Feature grant.
        let store = open_subjektiv_store(&f.api).unwrap();
        let previous = store.subject_permissions(&f.subject).unwrap().unwrap();
        let mut execution = previous.execution;
        execution.push(ScopedPermission {
            operation: "plugin.artifact.read".into(),
            resource: PermissionResource::PluginInstallation {
                workspace_id: TEST_WORKSPACE_ID.into(),
                installation_id: f.installed.installation.installation_id.clone(),
            },
        });
        store
            .set_subject_permissions(
                &f.subject,
                &SubjectPermissionsUpdate {
                    operation_id: Uuid::now_v7().to_string(),
                    expected_change_id: Some(previous.change_id),
                    execution,
                    delegation: previous.delegation,
                },
            )
            .unwrap();
    }

    #[derive(Clone)]
    pub(in crate::server::tests) struct Model {
        pub(in crate::server::tests) tool_name: String,
        pub(in crate::server::tests) requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for Model {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: ModelRequest,
        ) -> std::result::Result<agen::llm_client::client::ResponseStream, ClientError> {
            let turn = {
                let mut requests = self.requests.lock().unwrap();
                let turn = requests.len();
                requests.push(request);
                turn
            };
            let events = match turn {
                0 | 2 => vec![
                    ModelEvent::tool_use_start(0, "https-call", &self.tool_name),
                    ModelEvent::tool_input_delta(
                        0,
                        if self.tool_name.ends_with("__example_echo") {
                            r#"{"text":"probe"}"#
                        } else {
                            "{}"
                        },
                    ),
                    ModelEvent::tool_use_stop(0),
                ],
                1 | 3 => vec![
                    ModelEvent::text_block_start(0),
                    ModelEvent::text_delta(0, "Observed the HTTPS tool result."),
                    ModelEvent::text_block_stop(0, None),
                ],
                _ => panic!("unexpected extra model turn"),
            };
            Ok(Box::pin(futures::stream::iter(
                events
                    .into_iter()
                    .chain([ModelEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    })])
                    .map(Ok),
            )))
        }
    }

    pub(in crate::server::tests) fn persisted_call_consumers(
        session: session_store::SessionId,
        saved: &session_store::RestoredState,
        public_name: &str,
    ) -> Vec<String> {
        saved
            .annotated_history
            .iter()
            .filter_map(|entry| match &entry.item {
                session_store::LoggedItem::ToolCall { name, .. } if name == public_name => {
                    let bytes =
                        serde_json::to_vec(&(session.to_string(), &entry.metadata.entry_id.0))
                            .unwrap();
                    Some(format!(
                        "tool-{}",
                        Sha256::digest(bytes)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    ))
                }
                _ => None,
            })
            .collect()
    }

    pub(in crate::server::tests) fn assert_https_history(items: &[agen::Item], public_name: &str) {
        let calls = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => Some((call_id, name, arguments)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_str(), "https-call");
        assert_eq!(calls[0].1, public_name);
        assert_eq!(
            serde_json::from_str::<Value>(calls[0].2).unwrap(),
            json!({})
        );
        let results = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolResult {
                    call_id,
                    summary,
                    content,
                    disposition,
                    is_error,
                    attachments,
                    ..
                } => Some((
                    call_id,
                    summary,
                    content,
                    disposition,
                    is_error,
                    attachments,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        let (call, summary, content, disposition, is_error, attachments) = results[0];
        assert_eq!(call, calls[0].0);
        assert_eq!(
            *disposition,
            agen::ToolResultDisposition::Success,
            "{summary}: {content:?}"
        );
        assert!(!*is_error);
        assert!(attachments.is_empty());
        assert_eq!(summary, "HTTP response observed");
        let output: Value = serde_json::from_str(content.as_deref().unwrap()).unwrap();
        assert_eq!(output["structured"], json!({"status":503,"body_bytes":9}));
    }

    struct PauseBeforeFirstCall;

    #[async_trait::async_trait]
    impl worker::hook::Hook<worker::hook::PreToolCall> for PauseBeforeFirstCall {
        async fn call(
            &self,
            _: &worker::hook::ToolCallSummary,
        ) -> std::result::Result<worker::hook::HookPreToolAction, worker::hook::HookError> {
            Ok(worker::hook::HookPreToolAction::Pause)
        }
    }

    async fn https_through_runtime_and_history(remote: bool, restore_pending: bool) {
        let mut peer = Peer::new(HOST, vec![response(503, &[], b"peer data")]).await;
        let mut f = Fixture::new(Some(peer_binding(&peer))).await;
        f.api.plugin_https_transport = Some(Arc::new(peer.transport()));
        allow_artifact_read(&f);
        let (artifacts, mut delivery_server) = artifact_source(&f, remote).await;
        let (owner, mut owner_server) = if remote {
            let (owner, server) =
                remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
            (owner, Some(server))
        } else {
            // Fixture::owner constructs EmbeddedPluginHostOwner::authenticated
            // with SignedAuthorizer; it never injects a trusted request context.
            (f.owner(Some(peer.transport())).await, None)
        };
        let worker_ref = WorkerRef::try_from(&f.source).unwrap();
        let providers = Arc::new(RuntimeProviders {
            worker: worker_ref.clone(),
            artifacts,
            owner,
        });
        let bridge =
            PinnedPluginToolRuntime::new(providers, &f._directory.path().join("runtime-cache"))
                .unwrap();
        let public_name = format!("{}__{}", f.installed.installation.installation_id, f.tool);
        let model = Model {
            tool_name: public_name.clone(),
            requests: Arc::default(),
        };
        let mut manifest = worker::WorkerManifest::from_toml(&format!(
            "[worker]\nname=\"{}\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"scripted\"\n[engine]\n[scope]\nallow=[]\n",
            f.source.worker_id
        ))
        .unwrap();
        if restore_pending {
            manifest
                .plugins
                .selections
                .push(manifest::PluginProfileSelection {
                    ticket_read: false,
                    ticket_data_read: vec![],
                    ticket_data_write: vec![],
                    installation_id: f.installed.installation.installation_id.clone(),
                    tools: vec![f.tool.into()],
                    logging: false,
                    https_slots: vec!["api".into()],
                    config: Default::default(),
                });
        }
        let store_root = f._directory.path().join("tool-history");
        let metadata_root = f._directory.path().join("worker-metadata");
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(&store_root).unwrap(),
            session_store::FsWorkerStore::new(&metadata_root).unwrap(),
        );
        let mut host = worker::Worker::new(
            manifest.clone(),
            agen::Engine::new_annotated(Box::new(model.clone()) as Box<dyn LlmClient>)
                .system_prompt("Use the selected HTTPS tool."),
            store,
            worker::WorkerWorkspaceContext::unavailable(
                Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                "unrelated Workspace operations are not bound by this fixture",
            ),
            worker::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        let session_id = host.session_id();
        let snapshot = runtime_api::PluginExecutionSnapshot {
            plan_id: f.plan.plan_id.clone(),
            fingerprint: f.plan.fingerprint.clone(),
            plan: f.plan.plan.clone(),
        };
        let feature = bridge
            .prepare(&worker_ref, &snapshot, &session_id.to_string())
            .await
            .unwrap();
        assert_eq!(
            f.count("plugin_invocations"),
            0,
            "preparation is not a Tool call"
        );
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        let registry = FeatureRegistryBuilder::new().with_module(feature);
        let install_plan: FeatureInstallPlan = registry.plan().unwrap();
        assert_eq!(
            install_plan.ordered_features(),
            &[FeatureId::new("plugin:tools").unwrap()]
        );
        let report = host.install_features(registry);
        assert!(!report.has_errors(), "{}", report.error_message());
        let tools = host.engine().tool_server_handle();
        tools.flush_pending();
        let definitions = tools.tool_definitions_sorted();
        assert_eq!(
            definitions
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [public_name.as_str()]
        );
        let (metadata, _) = tools.get_tool(&public_name).unwrap();
        assert_eq!(
            metadata.input_schema,
            f.installed.installation.artifacts[0].tools[0].input_schema
        );
        assert_eq!(
            metadata.origin.unwrap().plugin_ref,
            f.installed.installation.installation_id
        );
        assert!(
            tools.get_tool(f.tool).is_none(),
            "bare guest names are not public tools"
        );
        if restore_pending {
            host.enable_worker_metadata_write_through().unwrap();
            host.add_pre_tool_call_hook(PauseBeforeFirstCall);
        }
        host.wire_history_persistence();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            host.run(vec![protocol::Segment::Text {
                content: "Check the selected service status.".into(),
            }]),
        )
        .await
        .expect("HTTPS Tool/Engine must finish within the fixture deadline")
        .unwrap();
        if restore_pending {
            assert!(matches!(result, worker::WorkerRunResult::Paused));
            assert_eq!(peer.connection_count(), 0);
            assert_eq!(f.count("plugin_invocations"), 0);
            let reopened = session_store::FsStore::new(&store_root).unwrap();
            let segments = reopened.list_segments(session_id).unwrap();
            let pending = session_store::restore(&reopened, session_id, segments[0]).unwrap();
            let expected = persisted_call_consumers(session_id, &pending, &public_name);
            assert_eq!(
                expected.len(),
                1,
                "paused call must already have a durable Host identity"
            );
            assert!(
                !pending
                    .history
                    .iter()
                    .any(|item| matches!(item, agen::Item::ToolResult { .. }))
            );
            drop(host);
            let mut restored = worker::Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                &manifest.worker.name, manifest.clone(), session_store::CombinedStore::new(
                    reopened, session_store::FsWorkerStore::new(&metadata_root).unwrap(),
                ),
                worker::PromptCatalogSource::builtins_only(),
                worker::WorkerWorkspaceContext::unavailable(Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()), "unrelated Workspace operations unavailable"),
                worker::WorkerFilesystemAuthority::None, Some(Box::new(model.clone())),
            ).await.unwrap().activate().await.unwrap();
            assert_eq!(restored.session_id(), session_id);
            let reloaded = session_store::restore(
                &session_store::FsStore::new(&store_root).unwrap(),
                session_id,
                segments[0],
            )
            .unwrap();
            assert_eq!(
                persisted_call_consumers(session_id, &reloaded, &public_name),
                expected
            );
            let feature = bridge
                .prepare(&worker_ref, &snapshot, &session_id.to_string())
                .await
                .unwrap();
            let report =
                restored.install_features(FeatureRegistryBuilder::new().with_module(feature));
            assert!(!report.has_errors(), "{}", report.error_message());
            restored.wire_history_persistence();
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(30), restored.resume())
                    .await
                    .unwrap()
                    .unwrap();
            assert!(matches!(result, worker::WorkerRunResult::Finished));
            host = restored;
        } else {
            assert!(matches!(result, worker::WorkerRunResult::Finished));
        }
        assert_https_history(&host.history(), &public_name);
        {
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0]
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                [public_name.as_str()]
            );
            assert_https_history(&requests[1].items, &public_name);
        }
        assert!(
            String::from_utf8(peer.observed().await)
                .unwrap()
                .starts_with("GET /status HTTP/1.1\r\n")
        );
        assert_eq!(peer.connection_count(), 1);
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);

        // Reopen the actual filesystem Store and reconstruct canonical history,
        // rather than treating the model's captured request as persistence proof.
        let reopened = session_store::FsStore::new(&store_root).unwrap();
        let segments = reopened.list_segments(session_id).unwrap();
        assert_eq!(segments.len(), 1);
        let saved = session_store::restore(&reopened, session_id, segments[0]).unwrap();
        assert_https_history(&saved.history, &public_name);
        let consumers = persisted_call_consumers(session_id, &saved, &public_name);
        assert_eq!(consumers.len(), 1);
        let consumer_id = consumers[0].clone();
        let (consumer, state, status, body): (String, String, i64, Vec<u8>) = f
            .api
            .config_store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT i.consumer_json,r.state,r.response_status,r.response_body \
                     FROM plugin_invocations i JOIN plugin_effect_receipts r \
                     USING(workspace_id,invocation_id) \
                     WHERE i.workspace_id=?1 AND i.plan_id=?2 AND i.worker_id=?3 \
                     AND i.installation_id=?4 AND r.effect_key='status-v1'",
                    rusqlite::params![
                        TEST_WORKSPACE_ID,
                        f.plan.plan_id,
                        f.source.worker_id,
                        f.installed.installation.installation_id
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<PluginConsumer>(&consumer).unwrap(),
            PluginConsumer::ToolCall {
                tool_call_id: consumer_id,
                tool_name: f.tool.into()
            }
        );
        assert_eq!(state, "completed");
        assert_eq!(status, 503);
        assert_eq!(body, b"peer data");

        // Current authority still governs a registered Tool and cached bytes.
        // Denial must neither start a new invocation nor send another request.
        f.deny();
        assert!(
            tools
                .call_tool(
                    &public_name,
                    "{}",
                    ToolExecutionContext::new("denied-call", "denied-batch", 0)
                        .with_logical_call_id("denied-persisted-call"),
                )
                .await
                .is_err()
        );
        assert!(
            bridge
                .prepare(&worker_ref, &snapshot, &session_id.to_string())
                .await
                .is_err()
        );
        assert_eq!(peer.connection_count(), 1);
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        for server in [&mut owner_server, &mut delivery_server]
            .into_iter()
            .flatten()
        {
            server.0.abort();
            let _ = (&mut server.0).await;
        }
        f.shutdown().await;
    }

    // Same-name calls in one Gemini response must remain distinct through
    // adapter normalization, committed Host history and real Component dispatch.
    #[derive(Clone)]
    struct GeminiTwoCallModel {
        tool_name: String,
        https: bool,
        separate_frames: bool,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for GeminiTwoCallModel {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        fn tool_call_completion_support(&self) -> agen::llm_client::ToolCallCompletionSupport {
            use agen::llm_client::scheme::{Scheme, gemini::GeminiScheme};
            // Match HttpTransport: Gemini does not have authoritative per-call
            // done events. Do not silently test the Engine's PerBlock path.
            let support = GeminiScheme::new().tool_call_completion_support();
            assert_eq!(
                support,
                agen::llm_client::ToolCallCompletionSupport::ResponseComplete
            );
            support
        }

        async fn stream(
            &self,
            request: ModelRequest,
        ) -> std::result::Result<agen::llm_client::client::ResponseStream, ClientError> {
            use agen::llm_client::scheme::{Scheme, gemini::GeminiScheme};
            let turn = {
                let mut requests = self.requests.lock().unwrap();
                let turn = requests.len();
                requests.push(request);
                turn
            };
            let scheme = GeminiScheme::new();
            let mut state = <GeminiScheme as Scheme>::State::default();
            let frames = match turn {
                // Exercise both two parts in one frame and part-index restart
                // across frames. Neither path fabricates Engine call IDs.
                0 => {
                    let parts = ["1", "2"]
                        .into_iter()
                        .map(|text| {
                            json!({"functionCall":{"name":self.tool_name,"args":
                            if self.https { json!({}) } else { json!({"text":text}) }}})
                        })
                        .collect::<Vec<_>>();
                    let chunks = if self.separate_frames {
                        parts.into_iter().map(|part| vec![part]).collect::<Vec<_>>()
                    } else {
                        vec![parts]
                    };
                    chunks.into_iter().map(|parts| {
                        json!({"candidates":[{"index":0,"content":{"role":"model","parts":parts}}]})
                    }).chain([json!({"candidates":[{"index":0,"finishReason":"STOP"}]})]).collect::<Vec<_>>()
                }
                1 => vec![json!({"candidates":[{"index":0,"content":{"role":"model",
                    "parts":[{"text":"Observed both tool results."}]},"finishReason":"STOP"}]})],
                _ => panic!("restore must not start an extra model turn"),
            };
            let mut events = Vec::new();
            for frame in frames {
                events.extend(scheme.parse_sse("", &frame.to_string(), &mut state)?);
            }
            // Transport owns this terminal event; the scheme parses wire frames.
            events.push(ModelEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }));
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }
    }

    fn assert_gemini_two_call_history(items: &[agen::Item], public_name: &str, https: bool) {
        let calls = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => Some((call_id, name, arguments)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2, "same-name calls must not alias");
        assert_ne!(
            calls[0].0, calls[1].0,
            "provider correlation IDs must be distinct"
        );
        let results = items
            .iter()
            .filter_map(|item| match item {
                agen::Item::ToolResult {
                    call_id,
                    summary,
                    content,
                    disposition,
                    is_error,
                    ..
                } => Some((call_id, summary, content, disposition, is_error)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        for (index, (id, name, arguments)) in calls.iter().enumerate() {
            assert_eq!(name.as_str(), public_name);
            let text = (index + 1).to_string();
            assert_eq!(
                serde_json::from_str::<Value>(arguments).unwrap(),
                if https {
                    json!({})
                } else {
                    json!({"text":text})
                }
            );
            let matching = results
                .iter()
                .filter(|result| result.0 == *id)
                .collect::<Vec<_>>();
            assert_eq!(
                matching.len(),
                1,
                "each call has exactly one correlated result"
            );
            let (_, summary, content, disposition, is_error) = *matching[0];
            assert_eq!(
                *disposition,
                agen::ToolResultDisposition::Success,
                "{summary}: {content:?}"
            );
            assert!(!*is_error);
            let output: Value = serde_json::from_str(content.as_deref().unwrap()).unwrap();
            if https {
                assert_eq!(summary, "HTTP response observed");
                assert_eq!(output["structured"], json!({"status":503,"body_bytes":9}));
            } else {
                assert_eq!(
                    output["structured"],
                    json!({"tool":"example_echo","text":text})
                );
            }
        }
    }

    fn gemini_committed_calls(
        saved: &session_store::RestoredState,
        public_name: &str,
    ) -> Vec<session_store::LoggedHistoryEntry> {
        let calls = saved.annotated_history.iter().filter(|entry| matches!(
            &entry.item, session_store::LoggedItem::ToolCall { name, .. } if name == public_name
        )).cloned().collect::<Vec<_>>();
        assert_eq!(
            calls.len(),
            2,
            "both calls must have committed Host entries"
        );
        assert_ne!(calls[0].metadata.entry_id, calls[1].metadata.entry_id);
        for call in &calls {
            assert!(matches!(
                &call.metadata.origin,
                session_store::LoggedSessionHistoryOrigin::ModelOutput { .. }
            ));
        }
        calls
    }

    fn gemini_invocation_rows(f: &Fixture) -> Vec<(String, String)> {
        f.api
            .config_store
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT invocation_id,consumer_json FROM plugin_invocations \
                 WHERE workspace_id=?1 AND plan_id=?2 AND worker_id=?3 \
                 AND installation_id=?4 ORDER BY invocation_id",
                )?;
                let rows = statement.query_map(
                    rusqlite::params![
                        TEST_WORKSPACE_ID,
                        f.plan.plan_id,
                        f.source.worker_id,
                        f.installed.installation.installation_id,
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap()
    }

    fn gemini_receipt_rows(f: &Fixture) -> Vec<(String, String, String, i64, Vec<u8>)> {
        f.api
            .config_store
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT r.invocation_id,r.effect_key,r.state,r.response_status,r.response_body \
                 FROM plugin_invocations i JOIN plugin_effect_receipts r \
                 USING(workspace_id,invocation_id) WHERE i.workspace_id=?1 \
                 AND i.plan_id=?2 AND i.worker_id=?3 AND i.installation_id=?4 \
                 ORDER BY r.invocation_id,r.effect_key",
                )?;
                let rows = statement.query_map(
                    rusqlite::params![
                        TEST_WORKSPACE_ID,
                        f.plan.plan_id,
                        f.source.worker_id,
                        f.installed.installation.installation_id,
                    ],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap()
    }

    async fn gemini_two_calls_through_runtime_and_restore(https: bool, restore_pending: bool) {
        let mut peer = if https {
            Some(
                Peer::new(
                    HOST,
                    vec![
                        response(503, &[], b"peer data"),
                        response(503, &[], b"peer data"),
                    ],
                )
                .await,
            )
        } else {
            None
        };
        // Pure Echo accepts distinct text inputs; HTTPSComponent only accepts
        // {}, but must still perform TWO independent effects/receipts.
        let mut f = Fixture::new(peer.as_ref().map(peer_binding)).await;
        f.api.plugin_https_transport = peer
            .as_ref()
            .map(|peer| Arc::new(peer.transport_for_requests(2)));
        allow_artifact_read(&f);
        let (artifacts, delivery_server) = artifact_source(&f, false).await;
        assert!(delivery_server.is_none());
        let worker_ref = WorkerRef::try_from(&f.source).unwrap();
        let providers = Arc::new(RuntimeProviders {
            worker: worker_ref.clone(),
            artifacts,
            owner: f
                .owner(peer.as_ref().map(|peer| peer.transport_for_requests(2)))
                .await,
        });
        let bridge = PinnedPluginToolRuntime::new(
            providers,
            &f._directory.path().join("gemini-runtime-cache"),
        )
        .unwrap();
        let public_name = format!("{}__{}", f.installed.installation.installation_id, f.tool);
        let model = GeminiTwoCallModel {
            tool_name: public_name.clone(),
            https,
            separate_frames: restore_pending,
            requests: Arc::default(),
        };
        let mut manifest = worker::WorkerManifest::from_toml(&format!(
            "[worker]\nname=\"{}\"\n[model]\nscheme=\"gemini\"\nmodel_id=\"scripted\"\n[engine]\n[scope]\nallow=[]\n",
            f.source.worker_id,
        )).unwrap();
        // Needed by the real metadata restore path, not a new Profile grant.
        manifest
            .plugins
            .selections
            .push(manifest::PluginProfileSelection {
                    ticket_read: false,
                    ticket_data_read: vec![],
                    ticket_data_write: vec![],
                installation_id: f.installed.installation.installation_id.clone(),
                tools: vec![f.tool.into()],
                logging: false,
                https_slots: if https { vec!["api".into()] } else { vec![] },
                config: Default::default(),
            });
        let store_root = f._directory.path().join("gemini-tool-history");
        let metadata_root = f._directory.path().join("gemini-worker-metadata");
        let mut host = worker::Worker::new(
            manifest.clone(),
            agen::Engine::new_annotated(Box::new(model.clone()) as Box<dyn LlmClient>)
                .system_prompt("Use the selected tool twice."),
            session_store::CombinedStore::new(
                session_store::FsStore::new(&store_root).unwrap(),
                session_store::FsWorkerStore::new(&metadata_root).unwrap(),
            ),
            worker::WorkerWorkspaceContext::unavailable(
                Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                "unrelated Workspace operations unavailable",
            ),
            worker::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        let session_id = host.session_id();
        let snapshot = runtime_api::PluginExecutionSnapshot {
            plan_id: f.plan.plan_id.clone(),
            fingerprint: f.plan.fingerprint.clone(),
            plan: f.plan.plan.clone(),
        };
        let feature = bridge
            .prepare(&worker_ref, &snapshot, &session_id.to_string())
            .await
            .unwrap();
        let report = host.install_features(FeatureRegistryBuilder::new().with_module(feature));
        assert!(!report.has_errors(), "{}", report.error_message());
        host.enable_worker_metadata_write_through().unwrap();
        if restore_pending {
            host.add_pre_tool_call_hook(PauseBeforeFirstCall);
        }
        host.wire_history_persistence();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            host.run(vec![protocol::Segment::Text {
                content: "Call the selected tool twice in one response.".into(),
            }]),
        )
        .await
        .expect("two-call runtime deadline")
        .unwrap();
        let mut pending_calls = None;
        if restore_pending {
            assert!(matches!(result, worker::WorkerRunResult::Paused));
            assert_eq!(model.requests.lock().unwrap().len(), 1);
            assert_eq!(f.count("plugin_invocations"), 0);
            assert_eq!(f.count("plugin_effect_receipts"), 0);
            if let Some(peer) = &peer {
                assert_eq!(peer.connection_count(), 0);
            }
            let reopened = session_store::FsStore::new(&store_root).unwrap();
            let segments = reopened.list_segments(session_id).unwrap();
            assert_eq!(segments.len(), 1);
            let pending = session_store::restore(&reopened, session_id, segments[0]).unwrap();
            let calls = gemini_committed_calls(&pending, &public_name);
            let consumers = persisted_call_consumers(session_id, &pending, &public_name);
            assert_eq!(consumers.len(), 2);
            assert_ne!(consumers[0], consumers[1]);
            assert!(
                !pending
                    .history
                    .iter()
                    .any(|item| matches!(item, agen::Item::ToolResult { .. }))
            );
            // Inspect live Host annotations as well as the reopened Store.
            let live_ids = host
                .session_history()
                .iter()
                .filter_map(|entry| match &entry.item {
                    agen::Item::ToolCall { .. } => Some(entry.annotation.entry_id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                live_ids,
                calls
                    .iter()
                    .map(|entry| entry.metadata.entry_id.clone())
                    .collect::<Vec<_>>()
            );
            drop(host);
            let mut restored = worker::Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                &manifest.worker.name, manifest.clone(), session_store::CombinedStore::new(
                    reopened, session_store::FsWorkerStore::new(&metadata_root).unwrap(),
                ), worker::PromptCatalogSource::builtins_only(),
                worker::WorkerWorkspaceContext::unavailable(Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()), "unrelated Workspace operations unavailable"),
                worker::WorkerFilesystemAuthority::None, Some(Box::new(model.clone())),
            ).await.unwrap().activate().await.unwrap();
            assert_eq!(restored.session_id(), session_id);
            let reloaded = session_store::restore(
                &session_store::FsStore::new(&store_root).unwrap(),
                session_id,
                segments[0],
            )
            .unwrap();
            assert_eq!(gemini_committed_calls(&reloaded, &public_name), calls);
            assert_eq!(
                persisted_call_consumers(session_id, &reloaded, &public_name),
                consumers
            );
            let restored_ids = restored
                .session_history()
                .iter()
                .filter_map(|entry| match &entry.item {
                    agen::Item::ToolCall { .. } => Some(entry.annotation.entry_id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                restored_ids, live_ids,
                "metadata reopen retains both live Host entries"
            );
            let feature = bridge
                .prepare(&worker_ref, &snapshot, &session_id.to_string())
                .await
                .unwrap();
            let report =
                restored.install_features(FeatureRegistryBuilder::new().with_module(feature));
            assert!(!report.has_errors(), "{}", report.error_message());
            restored.wire_history_persistence();
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(30), restored.resume())
                    .await
                    .unwrap()
                    .unwrap();
            assert!(matches!(result, worker::WorkerRunResult::Finished));
            pending_calls = Some((calls, consumers));
            host = restored;
        } else {
            assert!(matches!(result, worker::WorkerRunResult::Finished));
        }
        assert_gemini_two_call_history(&host.history(), &public_name, https);
        {
            use agen::llm_client::scheme::{Scheme, gemini::GeminiScheme};
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0]
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                [public_name.as_str()]
            );
            assert_gemini_two_call_history(&requests[1].items, &public_name, https);
            let scheme = GeminiScheme::new();
            let body =
                scheme.build_request_body("scripted", &requests[1], &scheme.default_capability());
            let responses = body["contents"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|content| content["parts"].as_array().unwrap())
                .filter_map(|part| part.get("functionResponse"))
                .collect::<Vec<_>>();
            assert_eq!(responses.len(), 2);
            for response in responses {
                assert_eq!(
                    response["name"], public_name,
                    "functionResponse uses the real Tool name, not a generated call ID"
                );
                assert_eq!(response["response"]["name"], public_name);
            }
        }
        let reopened = session_store::FsStore::new(&store_root).unwrap();
        let segments = reopened.list_segments(session_id).unwrap();
        assert_eq!(segments.len(), 1);
        let saved = session_store::restore(&reopened, session_id, segments[0]).unwrap();
        assert_gemini_two_call_history(&saved.history, &public_name, https);
        let calls = gemini_committed_calls(&saved, &public_name);
        let mut consumers = persisted_call_consumers(session_id, &saved, &public_name);
        assert_eq!(consumers.len(), 2);
        assert_ne!(consumers[0], consumers[1]);
        if let Some((pending, expected_consumers)) = pending_calls {
            assert_eq!(
                calls, pending,
                "resume preserves both committed Host entries"
            );
            assert_eq!(consumers, expected_consumers);
        }
        let invocations = gemini_invocation_rows(&f);
        assert_eq!(invocations.len(), 2);
        assert_ne!(invocations[0].0, invocations[1].0);
        let mut actual_consumers = invocations
            .iter()
            .map(
                |(_, consumer)| match serde_json::from_str::<PluginConsumer>(consumer).unwrap() {
                    PluginConsumer::ToolCall {
                        tool_call_id,
                        tool_name,
                    } => {
                        assert_eq!(tool_name, f.tool);
                        tool_call_id
                    }
                    other => panic!("unexpected consumer: {other:?}"),
                },
            )
            .collect::<Vec<_>>();
        consumers.sort();
        actual_consumers.sort();
        assert_eq!(
            actual_consumers, consumers,
            "SQLite admission uses both durable Host consumers"
        );
        let receipts = gemini_receipt_rows(&f);
        assert_eq!(f.count("plugin_invocations"), 2);
        assert_eq!(receipts.len(), if https { 2 } else { 0 });
        assert_eq!(f.count("plugin_effect_receipts"), if https { 2 } else { 0 });
        if let Some(peer) = &mut peer {
            assert_eq!(
                peer.connection_count(),
                2,
                "identical HTTPS inputs are separate effects"
            );
            for _ in 0..2 {
                let observed =
                    tokio::time::timeout(std::time::Duration::from_secs(5), peer.observed())
                        .await
                        .unwrap();
                assert!(
                    String::from_utf8(observed)
                        .unwrap()
                        .starts_with("GET /status HTTP/1.1\r\n")
                );
            }
            for (index, (invocation, key, state, status, body)) in receipts.iter().enumerate() {
                assert_eq!(invocation, &invocations[index].0);
                assert_eq!(key, "status-v1");
                assert_eq!(state, "completed");
                assert_eq!(*status, 503);
                assert_eq!(body, b"peer data");
            }
        }
        // A completed metadata reopen/activation is not a new run. Do not call
        // resume() (which requires a paused logical run), or fabricate history.
        drop(host);
        let mut completed =
            worker::Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                &manifest.worker.name,
                manifest.clone(),
                session_store::CombinedStore::new(
                    reopened,
                    session_store::FsWorkerStore::new(&metadata_root).unwrap(),
                ),
                worker::PromptCatalogSource::builtins_only(),
                worker::WorkerWorkspaceContext::unavailable(
                    Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                    "unrelated Workspace operations unavailable",
                ),
                worker::WorkerFilesystemAuthority::None,
                Some(Box::new(model.clone())),
            )
            .await
            .unwrap()
            .activate()
            .await
            .unwrap();
        assert_eq!(completed.session_id(), session_id);
        let feature = bridge
            .prepare(&worker_ref, &snapshot, &session_id.to_string())
            .await
            .unwrap();
        let report = completed.install_features(FeatureRegistryBuilder::new().with_module(feature));
        assert!(!report.has_errors(), "{}", report.error_message());
        completed.wire_history_persistence();
        assert!(completed.engine().active_run_turn_count().is_none());
        assert_gemini_two_call_history(&completed.history(), &public_name, https);
        let completed_ids = completed
            .session_history()
            .iter()
            .filter_map(|entry| match &entry.item {
                agen::Item::ToolCall { .. } => Some(entry.annotation.entry_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completed_ids,
            calls
                .iter()
                .map(|entry| entry.metadata.entry_id.clone())
                .collect::<Vec<_>>(),
            "completed Worker activation retains both committed Host identities"
        );
        let after = session_store::restore(
            &session_store::FsStore::new(&store_root).unwrap(),
            session_id,
            segments[0],
        )
        .unwrap();
        assert_eq!(after.annotated_history, saved.annotated_history);
        assert_eq!(gemini_committed_calls(&after, &public_name), calls);
        assert_eq!(gemini_invocation_rows(&f), invocations);
        assert_eq!(gemini_receipt_rows(&f), receipts);
        assert_eq!(model.requests.lock().unwrap().len(), 2);
        assert_eq!(f.count("plugin_invocations"), 2);
        assert_eq!(f.count("plugin_effect_receipts"), if https { 2 } else { 0 });
        if let Some(peer) = &peer {
            assert_eq!(peer.connection_count(), 2);
        }
        drop(completed);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn gemini_same_response_pure_calls_keep_distinct_inputs_and_completed_restore_is_inert() {
        gemini_two_calls_through_runtime_and_restore(false, false).await;
    }

    #[tokio::test]
    async fn gemini_same_response_https_calls_commit_two_effects_and_completed_restore_is_inert() {
        gemini_two_calls_through_runtime_and_restore(true, false).await;
    }

    #[tokio::test]
    async fn gemini_same_response_pending_pure_calls_restore_both_host_consumers() {
        gemini_two_calls_through_runtime_and_restore(false, true).await;
    }

    #[tokio::test]
    async fn gemini_same_response_pending_https_calls_restore_both_host_consumers_and_effects() {
        gemini_two_calls_through_runtime_and_restore(true, true).await;
    }

    #[tokio::test]
    async fn remote_pinned_https_tool_commits_normal_history_and_receipt_then_fences_live_denial() {
        https_through_runtime_and_history(true, false).await;
    }

    #[tokio::test]
    async fn embedded_pinned_https_tool_commits_normal_history_and_receipt_then_fences_live_denial()
    {
        https_through_runtime_and_history(false, false).await;
    }

    #[tokio::test]
    async fn pending_tool_call_restores_its_committed_host_identity_before_first_effect() {
        https_through_runtime_and_history(false, true).await;
    }
}
