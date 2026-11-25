#[cfg(feature = "http-server")]
mod spawn_tool_startup_tests {
    //! Real signed generated Spawn client -> Runtime-owned adapter -> installed
    //! Worker Tool -> Engine -> Session log/reopen -> next model request. Only
    //! Backend authorization decisions and captured fault responses are fixtures;
    //! Server's connected tests separately prove their real authorization/storage.
    use super::*;
    use crate::auth::{
        RUNTIME_REQUEST_SOURCE_PROOF_HEADER, RuntimeRequestSourceExpectation,
        WORKSPACE_REQUEST_PERMISSION, request_body_digest, verify_runtime_request_source,
    };
    use axum::{body::Body, http::StatusCode, response::Response, routing::any};

    #[derive(Clone)]
    struct DurableSpawnModel {
        inner: MockClient,
        root: PathBuf,
    }

    #[async_trait]
    impl LlmClient for DurableSpawnModel {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            if let Some(result) = request.items.iter().find(|item| {
                matches!(item, agen::Item::ToolResult { call_id, .. } if call_id == "spawn-call")
            }) {
                let store = WorkerSessionStore::new(&self.root).unwrap();
                let session = store.session_id().unwrap().unwrap();
                let segments = store.list_segments(session).unwrap();
                assert_eq!(segments.len(), 1);
                let entries = store.read_all(session, segments[0]).unwrap();
                let state = session_store::collect_state(&entries);
                let persisted = state.history.iter().filter(|item| {
                    matches!(item, agen::Item::ToolResult { call_id, .. } if call_id == "spawn-call")
                }).collect::<Vec<_>>();
                assert_eq!(persisted, vec![result], "model input must already be committed once");
            }
            self.inner.stream(request).await
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signed_spawn_startup_failure_reaches_tool_result_durable_log_and_next_model_request() {
        for fixture in [
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../runtime-api/tests/fixtures/startup-browser-session-record-denied.problem.json"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../runtime-api/tests/fixtures/startup-browser-registration-response-lost.problem.json"
            )),
        ] {
            let mut problem: serde_json::Value = serde_json::from_str(fixture).unwrap();
            let expected_details = problem["yoi_failure"]["details"].clone();
            let expected_facts = problem["yoi_startup"].clone();
            let status = problem["status"].as_u64().unwrap() as u16;
            problem["detail"] = "private-upstream-prose-canary".into();
            problem["yoi_failure"]["explanation"] = problem["detail"].clone();
            let response_body = problem.to_string();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let identity = RuntimeIdentityMaterial::generate("runtime-spawn-tool").unwrap();
            let public_key = identity.public_key.clone();
            let spawns = Arc::new(AtomicUsize::new(0));
            let permissions = Arc::new(Mutex::new(Vec::new()));
            let spawn_probe = spawns.clone();
            let permission_probe = permissions.clone();
            let router = axum::Router::new().fallback(any(move |request: axum::extract::Request| {
                let public_key = public_key.clone();
                let response_body = response_body.clone();
                let spawns = spawns.clone();
                let permissions = permissions.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body, 64 * 1024).await.unwrap();
                    let digest = request_body_digest(&bytes);
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                    verify_runtime_request_source(
                        parts.headers.get(RUNTIME_REQUEST_SOURCE_PROOF_HEADER).unwrap().to_str().unwrap(),
                        &public_key,
                        &RuntimeRequestSourceExpectation {
                            identity_id: "runtime-spawn-tool", audience: "server-spawn-tool",
                            workspace_id: "workspace-spawn-tool", worker_id: Some("caller-worker"),
                            permission: WORKSPACE_REQUEST_PERMISSION, method: parts.method.as_str(),
                            path: parts.uri.path(), body_digest: &digest, now_unix: now,
                        },
                    ).unwrap();
                    if parts.uri.path() == "/api/w/workspace-spawn-tool/authorization/check" {
                        let authorization: server_api::authorization::OperationAuthorizationRequest = serde_json::from_slice(&bytes).unwrap();
                        permissions.lock().unwrap().push(authorization.operation);
                        return Response::builder().status(200).header("content-type", "application/json").body(Body::from("{}".to_owned())).unwrap();
                    }
                    assert_eq!(parts.method, axum::http::Method::POST);
                    assert_eq!(parts.uri.path(), "/api/w/workspace-spawn-tool/worker-control/workers");
                    let request: server_api::CreateWorkspaceWorkerRequest = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(request.control_operation_id.as_deref(), Some("spawn-same-operation"));
                    assert_eq!(request.runtime_id, "target-runtime");
                    spawns.fetch_add(1, Ordering::SeqCst);
                    Response::builder().status(StatusCode::from_u16(status).unwrap()).header("content-type", "application/problem+json").body(Body::from(response_body)).unwrap()
                }
            }));
            struct ServerTask(tokio::task::JoinHandle<()>);
            impl Drop for ServerTask {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _server = ServerTask(tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            }));
            let workspace_client: Arc<dyn WorkspaceClient> = Arc::new(
                RuntimeOwnedWorkspaceClient::new(
                    "workspace-spawn-tool",
                    url,
                    "runtime-spawn-tool",
                    "caller-worker",
                )
                .with_runtime_request_source(&identity, "server-spawn-tool"),
            );
            let args = serde_json::json!({
                "runtime_id": "target-runtime", "profile": "builtin:companion",
                "operation_id": "spawn-same-operation", "initial_submit": [],
            })
            .to_string();
            let model = MockClient::sequential(vec![
                MockResponse::Complete(vec![
                    LlmEvent::tool_use_start(0, "spawn-call", "WorkerSpawn"),
                    LlmEvent::tool_input_delta(0, args),
                    LlmEvent::tool_use_stop(0),
                    LlmEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]),
                MockResponse::Complete(vec![
                    LlmEvent::text_delta(
                        0,
                        "Inspect the retained result, do not automatically retry.",
                    ),
                    LlmEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]),
            ]);
            let root = tempfile::tempdir().unwrap();
            let store = WorkerSessionStore::new(root.path().join("session")).unwrap();
            let manifest = WorkerManifest::from_toml(
                r#"
                [worker]
                name = "spawn-consumer-test"
                [model]
                scheme = "anthropic"
                model_id = "fixture"
                auth = { kind = "none" }
                [engine]
                max_tokens = 100
                [scope]
                allow = []
                [feature.subjektiv.profile]
                enabled = false
            "#,
            )
            .unwrap();
            let mut worker = Worker::new(
                manifest,
                Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                    DurableSpawnModel {
                        inner: model.clone(),
                        root: root.path().join("session"),
                    },
                ),
                store.clone(),
                WorkerWorkspaceContext::with_client(
                    Some(WorkspaceId::new("workspace-spawn-tool").unwrap()),
                    workspace_client.clone(),
                ),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            let writer = worker.log_writer_handle();
            worker.attach_log_writer(Arc::new(writer));
            worker.wire_history_persistence();
            worker
                .engine_mut()
                .set_system_prompt("Exercise the authorized Worker Spawn Tool once.");
            let registry = worker::feature::FeatureRegistryBuilder::new().with_module(
                worker::feature::builtin::manage_worker::manage_worker_feature(
                    workspace_client,
                    None,
                    true,
                ),
            );
            assert!(!worker.install_features(registry).has_errors());
            worker
                .run_text("Spawn the selected Worker once.")
                .await
                .unwrap();
            assert_eq!(
                spawn_probe.load(Ordering::SeqCst),
                1,
                "unknown facts never authorize another Spawn"
            );
            assert!(
                permission_probe
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|op| op == "worker.create")
            );
            let history = worker.history();
            let result = history.iter().find(|item| matches!(item, agen::Item::ToolResult { call_id, .. } if call_id == "spawn-call")).unwrap();
            let agen::Item::ToolResult {
                summary,
                content: Some(content),
                disposition,
                is_error,
                ..
            } = result
            else {
                panic!("expected full structured ToolResult")
            };
            assert!(*is_error);
            assert_eq!(*disposition, agen::tool::ToolResultDisposition::Error);
            assert!(summary.contains("Session registration"));
            let report: serde_json::Value = serde_json::from_str(content).unwrap();
            assert_eq!(report["failure"]["domain"], "session_registration");
            assert_eq!(report["failure"]["details"], expected_details);
            assert_eq!(report["result"]["startup"], expected_facts);
            assert_eq!(report["result"]["status"], status);
            assert!(!content.contains("private-upstream-prose-canary"));
            let requests = model.captured.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].items.iter().any(|item| item == result));
            drop(requests);
            let (session, segment) = (worker.session_id(), worker.segment_id());
            drop(worker);
            drop(store);
            let reopened = WorkerSessionStore::new(root.path().join("session")).unwrap();
            let entries = reopened.read_all(session, segment).unwrap();
            let persisted = entries
                .iter()
                .filter_map(|entry| match entry {
                    LogEntry::AnnotatedToolResult { entry, .. } => Some(&entry.item),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(persisted.len(), 1);
            let saved = serde_json::to_string(persisted[0]).unwrap();
            assert!(saved.contains("session_registration"));
            assert!(!saved.contains("private-upstream-prose-canary"));
            let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
            assert_eq!(value["summary"], summary.as_str());
            assert_eq!(value["content"], content.as_str());
        }
    }
}
