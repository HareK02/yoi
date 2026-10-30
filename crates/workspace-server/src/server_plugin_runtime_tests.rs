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
