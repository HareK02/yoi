// Included inside worker_backend::tests, alongside the restore failure suite.
// These integration tests connect the real Profile factory, canonical stores,
// restore owner, and Controller. Only model I/O and the Workspace HTTP transport
// are substituted. The loopback Host is not an authentication/policy test.
// Run in the ordinary worker-runtime lib suite with the default product features
// and the filter `restore_candidate::`. No live service is used. Like the
// failure suite, this requires the existing Yoi path settings to resolve into
// target/restore-test-environment; the common fixture rejects unsafe fallbacks
// before any factory, Git, or allocation-table operation.

#[cfg(feature = "http-server")]
mod restore_candidate {
    use super::*;

    const FIRST: &str = "candidate-saved-first";
    const SECOND: &str = "candidate-saved-second";
    const NOTICE: &str = "candidate-saved-notice";
    const FRESH: &str = "candidate-fresh-input";
    const WAIT: Duration = Duration::from_secs(5);

    struct HostProbe {
        calls: Arc<Mutex<Vec<(String, String)>>>,
        task: Option<tokio::task::JoinHandle<()>>,
    }

    impl Drop for HostProbe {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }

    // Aborting on unwind prevents a failed setup assertion from leaving its
    // hanging model turn alive. Successful setup uses the normal shutdown/join.
    struct SeedController(Option<RuntimeWorkerController>);

    impl Drop for SeedController {
        fn drop(&mut self) {
            if let Some(controller) = self.0.as_ref() {
                controller.controller_task.abort();
            }
        }
    }

    async fn receive_matching(
        events: &mut tokio::sync::broadcast::Receiver<Event>,
        phase: &str,
        mut matches: impl FnMut(&Event) -> bool,
    ) -> Event {
        let mut seen = Vec::new();
        tokio::time::timeout(WAIT, async {
            loop {
                let event = events.recv().await.expect("Controller event stream");
                seen.push(format!("{event:?}"));
                if matches(&event) {
                    return event;
                }
                assert!(
                    !matches!(
                        event,
                        Event::Error { .. }
                            | Event::SubmissionRejected { .. }
                            | Event::NotificationRejected { .. }
                    ),
                    "unexpected Controller rejection: {event:?}"
                );
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("Controller acknowledgement deadline during {phase}; observed {seen:?}")
        })
    }

    async fn submit(handle: &WorkerHandle, id: &str) -> Event {
        let mut events = handle.subscribe();
        handle
            .send(Method::Submit {
                submission_request_id: id.into(),
                input: vec![protocol::Segment::Text { content: id.into() }],
            })
            .await
            .unwrap();
        receive_matching(&mut events, id, |event| {
            matches!(event, Event::SubmissionAccepted { submission_request_id, .. }
                if submission_request_id == id)
        })
        .await
    }

    async fn notify(handle: &WorkerHandle) {
        let mut events = handle.subscribe();
        handle
            .send(Method::Notify {
                notification_request_id: NOTICE.into(),
                message: NOTICE.into(),
            })
            .await
            .unwrap();
        receive_matching(&mut events, "notification acceptance", |event| {
            matches!(event, Event::NotificationAccepted { notification_request_id }
                if notification_request_id == NOTICE)
        })
        .await;
    }

    async fn pending(handle: &WorkerHandle) -> protocol::PendingSubmissionsSnapshot {
        let mut events = handle.subscribe();
        handle.send(Method::ListPendingSubmissions).await.unwrap();
        match receive_matching(&mut events, "pending queue snapshot", |event| {
            matches!(event, Event::PendingSubmissionsChanged { .. })
        })
        .await
        {
            Event::PendingSubmissionsChanged { pending } => pending,
            _ => unreachable!(),
        }
    }

    async fn finished(handle: &WorkerHandle, id: &str) {
        let mut events = handle.subscribe();
        let is_finished = |snapshot: &protocol::WorkerStateSnapshot| {
            snapshot.last_finished_submission_request_id.as_deref() == Some(id)
                && snapshot.state == protocol::WorkerState::Idle
        };
        if !is_finished(&handle.shared_state.snapshot()) {
            receive_matching(
                &mut events,
                id,
                |event| matches!(event, Event::WorkerState { snapshot } if is_finished(snapshot)),
            )
            .await;
        }
    }

    fn queue_checkpoint(entries: &[LogEntry]) -> serde_json::Value {
        entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == "worker.pending_activations.v1" => Some(payload.clone()),
                _ => None,
            })
            .expect("real Controller must have committed its pending activation checkpoint")
    }

    struct Fixture {
        backend: Arc<WorkerRuntimeExecutionBackend>,
        request: WorkerExecutionRestoreRequest,
        model: MockClient,
        session: WorkerSessionStore,
        session_id: session_store::SessionId,
        segment_id: session_store::SegmentId,
        receipts: Vec<serde_json::Value>,
        host: HostProbe,
        _root: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_transport(WorkerControllerTransport::InProcess)
        }

        fn with_transport(transport: WorkerControllerTransport) -> Self {
            let root = restore_failure_tempdir();
            let model = MockClient::sequential(vec![
                MockResponse::Complete(vec![
                    LlmEvent::tool_use_start(0, "candidate-task-probe", "TaskCreate"),
                    LlmEvent::tool_input_delta(
                        0,
                        r#"{"subject":"Activated candidate","description":"Execution probe"}"#,
                    ),
                    LlmEvent::tool_use_stop(0),
                    LlmEvent::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]),
                MockResponse::Complete(simple_text_events()),
                MockResponse::Complete(simple_text_events()),
                MockResponse::Complete(simple_text_events()),
            ]);
            let injected = model.clone();
            let mut factory = ProfileRuntimeWorkerFactory::new(root.path())
                .with_runtime_store_dir(root.path())
                .with_runtime_id("candidate-runtime")
                .with_spawn_model_client_factory(move || Box::new(injected.clone()));
            factory.controller_transport = transport;
            let catalog = worker::PromptCatalog::builtins_only()
                .unwrap()
                .projection()
                .clone();
            factory
                .observe_workspace_prompt_projection(
                    worker::WorkspacePromptProjection::new(
                        "workspace-saved-subjektiv",
                        "candidate-fixture",
                        catalog.catalog_digest.clone(),
                        catalog,
                    )
                    .unwrap(),
                )
                .unwrap();
            let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
            let fixture_root = root.path().to_path_buf();
            let (request, session, session_id, segment_id, receipts, host) = backend
                .run_on_adapter_runtime(async move {
                    let calls = Arc::new(Mutex::new(Vec::new()));
                    let authorization_calls = calls.clone();
                    let other_calls = calls.clone();
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let base_url = format!("http://{}", listener.local_addr().unwrap());
                    let authorization_path =
                        "/api/w/workspace-saved-subjektiv/authorization/check";
                    let app = axum::Router::new()
                        .route(
                            authorization_path,
                            axum::routing::post(move |axum::Json(input): axum::Json<
                                server_api::authorization::OperationAuthorizationRequest,
                            >| {
                                let calls = authorization_calls.clone();
                                async move {
                                    calls.lock().unwrap().push((
                                        "POST".into(), authorization_path.into(),
                                    ));
                                    let response = if input.operation == "task"
                                        && input.resource == server_api::authorization::PermissionResource::Feature
                                    {
                                        server_api::authorization::OperationAuthorizationResponse {
                                            scope: Some(server_api::authorization::PermissionResource::Feature),
                                        }
                                    } else {
                                        saved_subjektiv_authorization_fixture(input)
                                            .expect("only the saved fixture Feature grants are available")
                                    };
                                    axum::Json(response)
                                }
                            }),
                        )
                        .fallback(move |request: axum::extract::Request| {
                            let calls = other_calls.clone();
                            async move {
                                calls.lock().unwrap().push((
                                    request.method().to_string(), request.uri().path().to_owned(),
                                ));
                                axum::http::StatusCode::FORBIDDEN
                            }
                        });
                    let host = HostProbe {
                        calls,
                        task: Some(tokio::spawn(async move { axum::serve(listener, app).await.unwrap() })),
                    };
                    let worker_ref = WorkerRef::new(WorkerId::now_v7());
                    let (mut manifest, mut create) =
                        saved_subjektiv_policy(&fixture_root, &worker_ref);
                    // Normal restore deliberately excludes Subject repair. Keep
                    // a real Workspace client so accidental Host effects are visible.
                    manifest.feature.subjektiv = manifest::ResolvedSubjektivFeatureConfig::default();
                    manifest.feature.task.enabled = true;
                    create.subjektiv_attached = false;
                    create.workspace_api.as_mut().unwrap().base_url = base_url;
                    let aggregate = fixture_root.join("workers").join(worker_ref.worker_id.to_string());
                    let session = WorkerSessionStore::new(aggregate.join("session")).unwrap();
                    let metadata = WorkerAggregateStore::new(&aggregate, &manifest.worker.name).unwrap();
                    let mut engine = Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                        MockClient::sequential(vec![MockResponse::Hang(simple_text_events().into_iter().take(2).collect())]),
                    );
                    engine.set_system_prompt("Committed restore candidate fixture prompt");
                    let mut worker = Worker::new(
                        manifest.clone(),
                        engine,
                        CombinedStore::new(session.clone(), metadata.clone()),
                        RuntimeWorkspaceBackendRef::from_worker_request(&create, Some("candidate-runtime"))
                            .worker_context(&worker_ref, Some(&crate::runtime::RuntimeWorkspaceScope::new(
                                "workspace-saved-subjektiv", "server-main",
                            )), None, None, None, None),
                        WorkerFilesystemAuthority::None,
                        Scope::empty(),
                    ).await.unwrap();
                    worker.enable_worker_metadata_write_through().unwrap();
                    worker.materialize_durable_session_head().await.unwrap();
                    let (session_id, segment_id) = (worker.session_id(), worker.segment_id());
                    let workspace_client = worker.workspace_client_handle();
                    let started = PreparedWorker::new(
                        worker,
                        WorkerBootstrapLayout::RuntimeManagedRun { run_dir: fixture_root.join("seed") },
                        WorkerControllerTransport::InProcess,
                    ).start().await.unwrap();
                    let mut seed = SeedController(Some(RuntimeWorkerController {
                        handle: started.handle,
                        shutdown: Arc::new(tokio::sync::Mutex::new(Some(started.shutdown))),
                        controller_task: started.controller_task,
                        workspace_client,
                    }));
                    let handle = seed.0.as_ref().unwrap().handle.clone();
                    let mut started_events = handle.subscribe();
                    submit(&handle, "candidate-setup-turn").await;
                    // Running and streaming observations can arrive in either
                    // order. Do not discard TextDelta while awaiting WorkerState.
                    let mut running = handle.shared_state.catalog_status() == WorkerStatus::Running;
                    let mut streaming = false;
                    receive_matching(&mut started_events, "seed running and streaming", |event| {
                        if let Event::WorkerState { snapshot } = event {
                            running |= snapshot.catalog_status() == protocol::WorkerStatus::Running;
                        }
                        streaming |= matches!(event, Event::TextDelta { .. });
                        running && streaming
                    }).await;
                    let state = Arc::new(RwLock::new(handle.shared_state.snapshot()));
                    let mut events = handle.subscribe();
                    handle.send(Method::Pause { command: next_internal_command(&state).unwrap() })
                        .await.unwrap();
                    receive_matching(&mut events, "seed pause acknowledgement", |event| {
                        matches!(event, Event::WorkerState { snapshot }
                            if snapshot.catalog_status() == protocol::WorkerStatus::Paused)
                    }).await;
                    let mut receipts = Vec::new();
                    for id in [FIRST, SECOND] {
                        let receipt = submit(&handle, id).await;
                        assert!(matches!(receipt, Event::SubmissionAccepted {
                            disposition: protocol::SubmissionDisposition::Queued, ..
                        }));
                        receipts.push(serde_json::to_value(receipt).unwrap());
                        if id == FIRST { notify(&handle).await; }
                    }
                    assert_eq!(pending(&handle).await.submissions.len(), 2);
                    shutdown_profile_controller(seed.0.take().unwrap()).await;
                    // This orphan pin would be removed by the normal activation
                    // reconciliation. Preparation and discard must leave it alone.
                    let orphan = session.write_uploaded_file(
                        session_id, "orphan.txt", "text/plain", b"retained orphan",
                        session_store::UploadedFileLimits::default(),
                    ).unwrap();
                    session.pin_uploaded_file(session_id, &orphan, "uncommitted-acceptance").unwrap();
                    let mut saved = metadata.read_by_name(&ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref))
                        .unwrap().unwrap();
                    saved.resolved_manifest_snapshot = Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap());
                    saved.workspace_root = Some(fixture_root.join("stale-derived-root"));
                    metadata.write(&saved).unwrap();
                    let mut request = direct_restore_request(worker_ref);
                    request.request = create;
                    request.workspace_scope = Some(crate::runtime::RuntimeWorkspaceScope::new(
                        "workspace-saved-subjektiv", "server-main",
                    ));
                    Ok((request, session, session_id, segment_id, receipts, host))
                })
                .unwrap();
            let fixture = Self {
                backend,
                request,
                model,
                session,
                session_id,
                segment_id,
                receipts,
                host,
                _root: root,
            };
            let checkpoint = queue_checkpoint(&fixture.entries());
            assert_eq!(checkpoint["pending"].as_array().unwrap().len(), 2);
            assert_eq!(
                checkpoint["pending_notifications"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            fixture.assert_quiet();
            fixture
        }

        fn entries(&self) -> Vec<LogEntry> {
            self.session
                .read_all(self.session_id, self.segment_id)
                .unwrap()
        }

        fn canonical_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
            let aggregate = self
                .backend
                .factory
                .worker_aggregate_dir(&self.request.worker_ref)
                .unwrap();
            let mut files = persisted_files(&aggregate);
            files.retain(|path, _| !path.components().any(|part| part.as_os_str() == "runs"));
            files
        }

        fn assert_no_modifying_host_calls(&self) {
            assert!(
                self.host
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|(method, path)| {
                        method == "GET"
                            || method == "HEAD"
                            || (method == "POST"
                                && path == "/api/w/workspace-saved-subjektiv/authorization/check")
                    }),
                "unexpected modifying Host call"
            );
        }

        fn assert_quiet(&self) {
            assert_eq!(self.model.call_count.load(Ordering::SeqCst), 0);
            assert!(self.model.captured.lock().unwrap().is_empty());
            self.assert_no_modifying_host_calls();
            assert!(
                session_store::collect_state(&self.entries())
                    .annotated_history
                    .iter()
                    .all(|entry| {
                        !matches!(
                            entry.item,
                            LoggedItem::ToolCall { .. } | LoggedItem::ToolResult { .. }
                        )
                    }),
                "prepare committed tool execution"
            );
        }

        fn prepare(&self) {
            assert!(matches!(
                self.backend.restore_worker(self.request.clone()),
                WorkerExecutionSpawnResult::Connected { .. }
            ));
            // Connected here is the backend's prepared snapshot, not publication.
            assert!(
                self.backend
                    .pending_restore(&self.request.worker_ref)
                    .unwrap()
                    .is_some()
            );
            assert!(self.backend.workers.lock().unwrap().is_empty());
            assert!(
                self.backend
                    .factory
                    .observation_hub
                    .get(&self.request.worker_ref)
                    .is_none()
            );
            assert!(
                !self
                    .backend
                    .factory
                    .worker_restore_run_dir(&self.request.worker_ref, self.request.operation_id,)
                    .unwrap()
                    .exists()
            );
            let table = worker::runtime::worker_allocation::LockFileGuard::open(
                &worker::runtime::worker_allocation::default_allocation_path().unwrap(),
            )
            .unwrap();
            assert!(
                table
                    .data()
                    .find(&ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
                        &self.request.worker_ref,
                    ))
                    .is_none()
            );
        }

        fn assert_no_allocation(&self) {
            let table = worker::runtime::worker_allocation::LockFileGuard::open(
                &worker::runtime::worker_allocation::default_allocation_path().unwrap(),
            )
            .unwrap();
            assert!(
                table
                    .data()
                    .find(&ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
                        &self.request.worker_ref,
                    ))
                    .is_none()
            );
        }

        fn assert_released(&self) {
            self.assert_no_allocation();
            assert!(
                self.backend
                    .pending_restore(&self.request.worker_ref)
                    .unwrap()
                    .is_none()
            );
            assert!(self.backend.workers.lock().unwrap().is_empty());
            assert!(
                self.backend
                    .factory
                    .failed_restore_resources
                    .lock()
                    .unwrap()
                    .is_empty()
            );
            assert!(
                self.backend
                    .factory
                    .observation_hub
                    .get(&self.request.worker_ref)
                    .is_none()
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Keep the storage and loopback transport alive until owned tasks stop.
            let _ = self.backend.stop_worker(&self.request.worker_ref);
            if let Some(task) = self.host.task.take() {
                task.abort();
                let _ = self.backend.run_on_adapter_runtime(async move {
                    let _ = task.await;
                    Ok(())
                });
            }
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn prepare_saved_inputs_rejects_new_input_and_discard_preserves_authority() {
        // Exercise the configured socket transport without ever publishing or
        // binding it; activation coverage below uses the normal InProcess path.
        let mut fixture = Fixture::with_transport(WorkerControllerTransport::UnixSocket);
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = opens.clone();
        let raw = deferred_workdir::DeferredWorkdirSession::handle(
            "candidate-discard-resource",
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Err("not opened during prepare".into())
            },
        );
        fixture.request.deferred_workdir_sessions.insert(
            WorkdirAttachmentAlias::new("checkout").unwrap(),
            raw.clone(),
        );
        let before = fixture.canonical_files();
        fixture.prepare();
        for input in [
            WorkerInput::user("must-not-accept"),
            WorkerInput::notify("must-not-notify"),
        ] {
            assert!(
                !fixture
                    .backend
                    .dispatch_input(&fixture.request.worker_ref, input)
                    .is_accepted()
            );
        }
        #[cfg(feature = "ws-server")]
        assert!(
            fixture
                .backend
                .clone()
                .attach_worker_protocol(&fixture.request.worker_ref)
                .is_err()
        );
        assert!(!matches!(
            fixture
                .backend
                .worker_session(WorkerSessionObservationRequest {
                    worker_ref: fixture.request.worker_ref.clone(),
                }),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        fixture.assert_quiet();
        assert_eq!(fixture.canonical_files(), before);
        assert!(
            fixture
                .backend
                .factory
                .failed_restore_resources
                .lock()
                .unwrap()[&fixture.request.worker_ref]
                .candidate_workdir_cleanup
                .is_some(),
            "retain the real prepared Worker's close-only resource owner"
        );
        assert!(
            fixture
                .backend
                .stop_worker(&fixture.request.worker_ref)
                .is_accepted()
        );
        fixture.assert_released();

        let factory = fixture.backend.factory.clone();
        let request = fixture.request.clone();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                // The adapter resolves and owns its own raw handles. Separately
                // exercise the factory seam with the exact supplied cleanup handle.
                let mut candidate = factory
                    .prepare_restore(request)
                    .await
                    .map_err(|error| error.to_string())?;
                candidate
                    .discard()
                    .await
                    .map_err(|error| error.to_string())?;
                assert!(matches!(
                    raw.read(workdir::ReadRequest {
                        path: workdir::WorkdirPath::new("probe.txt").unwrap(),
                        offset: 0,
                        limit: 1,
                        max_bytes: 32,
                    })
                    .await,
                    Err(workdir::WorkdirError::SessionClosed)
                ));
                Ok(())
            })
            .unwrap();
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        fixture.assert_quiet();
        assert_eq!(fixture.canonical_files(), before);
        assert!(
            fixture
                .backend
                .stop_worker(&fixture.request.worker_ref)
                .is_accepted()
        );
        assert_eq!(fixture.canonical_files(), before);
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn prepare_failure_at_each_pre_activation_checkpoint_preserves_saved_authority() {
        // Feature and Controller are activation checkpoints, not preparation.
        for stage in [
            RuntimeRestoreStage::Metadata,
            RuntimeRestoreStage::Reconstruction,
            RuntimeRestoreStage::Binding,
        ] {
            let fixture = Fixture::new();
            let before = fixture.canonical_files();
            *fixture
                .backend
                .factory
                .restore_faults
                .fail_at
                .lock()
                .unwrap() = Some(stage);
            let result = fixture.backend.restore_worker(fixture.request.clone());
            assert!(
                matches!(result, WorkerExecutionSpawnResult::RolledBack(_)),
                "{stage:?}: {result:?}"
            );
            fixture.assert_released();
            fixture.assert_quiet();
            assert_eq!(fixture.canonical_files(), before, "{stage:?}");
            let completed = fixture.backend.completed_restore_failures.lock().unwrap();
            let (_, failure) = &completed[&fixture.request.worker_ref];
            assert_eq!(failure.stage, stage);
            assert_eq!(failure.cleanup, RuntimeRestoreCleanup::Confirmed);
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn activate_saved_inputs_drains_fifo_once_and_replays_durable_receipts() {
        let fixture = Fixture::new();
        let before = fixture.canonical_files();
        fixture.prepare();
        fixture.assert_quiet();
        assert_eq!(fixture.canonical_files(), before);
        fixture
            .backend
            .activate_restored_worker(fixture.request.operation_id, &fixture.request.worker_ref)
            .unwrap();
        let handle = fixture.backend.workers.lock().unwrap()[&fixture.request.worker_ref]
            .handle
            .clone();
        let model = fixture.model.clone();
        let receipts = fixture.receipts.clone();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                finished(&handle, SECOND).await;
                assert_eq!(model.call_count.load(Ordering::SeqCst), 3);
                {
                    let requests = model.captured.lock().unwrap();
                    let first = serde_json::to_string(&requests[0].items).unwrap();
                    let second = serde_json::to_string(&requests[2].items).unwrap();
                    assert!(
                        requests[0]
                            .tools
                            .iter()
                            .any(|tool| tool.name == "TaskCreate")
                    );
                    assert!(first.contains(FIRST));
                    assert!(!first.contains(SECOND), "second Submit overtook the first");
                    assert_eq!(first.matches(NOTICE).count(), 1);
                    assert!(second.find(FIRST).unwrap() < second.find(SECOND).unwrap());
                    assert_eq!(second.matches(NOTICE).count(), 1);
                }
                let committed = serde_json::to_value(handle.committed_entries()).unwrap();
                for (id, mut expected) in [FIRST, SECOND].into_iter().zip(receipts) {
                    // Activation advances the durable receipt from Queued to Started;
                    // replay must preserve its original submission identity.
                    expected["data"]["disposition"] = serde_json::json!("started");
                    assert_eq!(
                        serde_json::to_value(submit(&handle, id).await).unwrap(),
                        expected
                    );
                }
                notify(&handle).await;
                assert!(pending(&handle).await.submissions.is_empty());
                assert_eq!(model.call_count.load(Ordering::SeqCst), 3);
                assert_eq!(
                    serde_json::to_value(handle.committed_entries()).unwrap(),
                    committed
                );
                Ok(())
            })
            .unwrap();
        let history = session_store::collect_state(&fixture.entries()).annotated_history;
        assert!(
            history.iter().any(|entry| matches!(
                &entry.item,
                LoggedItem::ToolResult { call_id, is_error: false, .. }
                    if call_id == "candidate-task-probe"
            )),
            "activation must execute the real TaskCreate tool, not just decode a call"
        );
        for marker in [FIRST, SECOND, NOTICE] {
            let count = history
                .iter()
                .filter(|entry| {
                    match &entry.item {
                LoggedItem::Message { content, .. } => content.iter().any(|part| {
                    matches!(part, LoggedContentPart::Text { text } if text.contains(marker))
                }),
                _ => false,
            }
                })
                .count();
            assert_eq!(count, 1, "{marker} must commit exactly once");
        }
        let mut fresh = WorkerInput::user(FRESH);
        fresh.submission_request_id = Some(FRESH.into());
        assert!(
            fixture
                .backend
                .dispatch_input(&fixture.request.worker_ref, fresh)
                .is_accepted()
        );
        let handle = fixture.backend.workers.lock().unwrap()[&fixture.request.worker_ref]
            .handle
            .clone();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                finished(&handle, FRESH).await;
                assert!(pending(&handle).await.submissions.is_empty());
                Ok(())
            })
            .unwrap();
        assert_eq!(fixture.model.call_count.load(Ordering::SeqCst), 4);
        assert!(
            !fixture.host.calls.lock().unwrap().is_empty(),
            "activation must reach the configured Host transport"
        );
        fixture.assert_no_modifying_host_calls();
        assert!(
            fixture
                .backend
                .stop_worker(&fixture.request.worker_ref)
                .is_accepted()
        );
        fixture.assert_released();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn explicit_repair_reconciles_lost_attribution_response_without_reseeding() {
        for materialized in [true, false] {
            let root = restore_failure_tempdir();
            let model = MockClient::new(Vec::new());
            let injected = model.clone();
            let backend = WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path())
                    .with_runtime_store_dir(root.path())
                    .with_runtime_id("repair-runtime")
                    .with_spawn_model_client_factory(move || Box::new(injected.clone())),
            )
            .unwrap();
            let factory = backend.factory.clone();
            let fixture_root = root.path().to_path_buf();
            backend.run_on_adapter_runtime(async move {
                let mut request = saved_subjektiv_restore_fixture(&fixture_root, materialized).await;
                let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
                let aggregate = fixture_root.join("workers").join(request.worker_ref.worker_id.to_string());
                let metadata = WorkerAggregateStore::new(&aggregate, &name).unwrap();
                let mut saved = metadata.read_by_name(&name).unwrap().unwrap();
                let session_id = saved.active.as_ref().unwrap().session_id;
                saved.subjektiv_session_attribution = Some(session_store::SubjektivSessionAttributionState::OutcomeUnknown { session_id, diagnostic: "saved interrupted attribution".into() });
                metadata.write(&saved).unwrap();
                // Explicit repair may acquire artifact locks; compare canonical
                // contents separately from that lock-file side effect.
                let session_files = || {
                    let mut files = persisted_files(&aggregate.join("session"));
                    files.remove(Path::new("artifacts/paste/.aggregate.lock"));
                    files
                };
                let original = session_files();
                let mut bundle = test_bundle();
                bundle.metadata.workspace_id = "workspace-saved-subjektiv".into();
                bundle.prompt_catalog = Some(worker::PromptCatalog::builtins_only().unwrap().projection().clone());
                request.config_bundle = Some(bundle.with_computed_digest());

                // Substitute only the Host transport. A committed attribution
                // with an undecodable response models lost acknowledgement;
                // its durable Session-keyed receipt survives the retry. This
                // does not test the real Host's signature or Subject policy.
                let attribution_path = fixture_root.join("host-attribution.json");
                let stored = attribution_path.clone();
                let calls = Arc::new(AtomicUsize::new(0));
                let record_calls = calls.clone();
                let startup_calls = Arc::new(AtomicUsize::new(0));
                let startups = startup_calls.clone();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                request.request.workspace_api.as_mut().unwrap().base_url = format!("http://{}", listener.local_addr().unwrap());
                let app = axum::Router::new()
                    .route("/api/w/workspace-saved-subjektiv/authorization/check", axum::routing::post(
                        |axum::Json(input): axum::Json<server_api::authorization::OperationAuthorizationRequest>| async move {
                            axum::Json(saved_subjektiv_authorization_fixture(input).unwrap())
                        }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/sessions", axum::routing::post(
                        move |axum::Json(input): axum::Json<server_api::SubjektivRecordSessionRequest>| {
                            let stored = stored.clone();
                            let calls = record_calls.clone();
                            async move {
                                assert_eq!(input.session_id, session_id.to_string());
                                let ordinal = calls.fetch_add(1, Ordering::SeqCst);
                                if !stored.exists() {
                                    assert!(input.create_if_missing);
                                    fs::write(&stored, serde_json::to_vec(&server_api::SubjektivRecordSessionResponse {
                                        session_id: input.session_id, subject_id: "saved-subject".into(),
                                    }).unwrap()).unwrap();
                                }
                                (axum::http::StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/json")],
                                 if ordinal == 0 { "{}".to_string() } else { fs::read_to_string(stored).unwrap() })
                            }
                        }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/journal/startup", axum::routing::get(move || {
                        let startups = startups.clone();
                        async move {
                            startups.fetch_add(1, Ordering::SeqCst);
                            axum::Json(serde_json::json!({ "subject_id": "saved-subject", "journals": [], "omitted": false }))
                        }
                    }));
                let host = HostProbe { calls: Arc::new(Mutex::new(Vec::new())), task: Some(tokio::spawn(async move { axum::serve(listener, app).await.unwrap() })) };
                // Same real factory and same operation for both repairs. No
                // candidate/controller is constructed by the test itself.
                assert!(factory.recover_saved_restore_source(&request).await.is_err());
                assert!(attribution_path.exists(), "Host committed before response loss");
                assert_eq!(session_files(), original);
                let failed = metadata.read_by_name(&name).unwrap().unwrap();
                assert!(matches!(failed.subjektiv_session_attribution,
                    Some(session_store::SubjektivSessionAttributionState::OutcomeUnknown { session_id: id, .. }) if id == session_id));
                assert_eq!(startup_calls.load(Ordering::SeqCst), 0);
                factory.cleanup_failed_restore(&request).await.unwrap();
                assert!(factory.recover_saved_restore_source(&request).await.unwrap());
                let repaired = metadata.read_by_name(&name).unwrap().unwrap();
                assert_eq!(repaired.active.as_ref().unwrap().session_id, session_id);
                assert!(matches!(repaired.subjektiv_session_attribution,
                    Some(session_store::SubjektivSessionAttributionState::Confirmed { session_id: id, .. }) if id == session_id));
                let segments = WorkerSessionStore::new(aggregate.join("session")).unwrap()
                    .list_segments(session_id).unwrap();
                assert_eq!(segments.len(), 1, "retry must retain exactly one seed");
                if materialized {
                    assert_eq!(session_files(), original);
                    assert_eq!(startup_calls.load(Ordering::SeqCst), 0);
                } else {
                    assert_eq!(startup_calls.load(Ordering::SeqCst), 1);
                }
                let after_repair = persisted_files(&aggregate);
                let calls_after_repair = calls.load(Ordering::SeqCst);
                assert!(!factory.recover_saved_restore_source(&request).await.unwrap());
                assert_eq!(persisted_files(&aggregate), after_repair);
                assert_eq!(calls.load(Ordering::SeqCst), calls_after_repair);
                assert!(factory.failed_restore_resources.lock().unwrap().is_empty());
                drop(host);
                Ok(())
            }).unwrap();
            assert_eq!(model.call_count.load(Ordering::SeqCst), 0);
        }
    }
}
