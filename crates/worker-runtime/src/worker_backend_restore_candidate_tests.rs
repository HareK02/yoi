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
        notify_from_source(handle, None).await;
    }

    async fn notify_from_source(
        handle: &WorkerHandle,
        source: Option<protocol::AuthenticatedInputSource>,
    ) {
        let mut events = handle.subscribe();
        let method = match source {
            Some(source) => Method::NotifyTracked {
                receipt_scope: protocol::NotificationReceiptScope::Source,
                notification_request_id: NOTICE.into(),
                message: NOTICE.into(),
                source,
            },
            None => Method::Notify {
                notification_request_id: NOTICE.into(),
                message: NOTICE.into(),
            },
        };
        handle.send(method).await.unwrap();
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
            Self::with_seed(false)
        }

        fn no_model() -> Self {
            Self::with_seed(true)
        }

        fn with_seed(no_model: bool) -> Self {
            Self::with_seed_source(no_model, None)
        }

        fn with_seed_source(
            no_model: bool,
            notice_source: Option<protocol::AuthenticatedInputSource>,
        ) -> Self {
            let root = restore_failure_tempdir();
            let model = if no_model {
                MockClient::new(Vec::new())
            } else {
                MockClient::sequential(vec![
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
                ])
            };
            let injected = model.clone();
            let factory = ProfileRuntimeWorkerFactory::new(root.path())
                .with_runtime_store_dir(root.path())
                .with_runtime_id("candidate-runtime")
                .with_spawn_model_client_factory(move || Box::new(injected.clone()));
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
                                    match saved_subjektiv_authorization_fixture(input) {
                                        Ok(response) => (
                                            axum::http::StatusCode::OK,
                                            axum::Json(serde_json::to_value(response).unwrap()),
                                        ),
                                        Err(message) => (
                                            axum::http::StatusCode::FORBIDDEN,
                                            axum::Json(serde_json::json!({ "message": message })),
                                        ),
                                    }
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
                    let seed_model = if no_model {
                        MockClient::new(Vec::new())
                    } else {
                        MockClient::sequential(vec![MockResponse::Hang(simple_text_events().into_iter().take(2).collect())])
                    };
                    let mut engine = Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(seed_model);
                    engine.set_system_prompt("Committed restore candidate fixture prompt");
                    let mut worker = Worker::new(
                        manifest.clone(),
                        engine,
                        CombinedStore::new(session.clone(), metadata.clone()),
                        RuntimeWorkspaceBackendRef::from_worker_request(&create, Some("candidate-runtime"))
                            .unwrap()
                            .worker_context(&worker_ref, Some(&crate::runtime::RuntimeWorkspaceScope::new(
                                "workspace-saved-subjektiv", "server-main",
                            )), None, None, None, None),
                        WorkerFilesystemAuthority::None,
                        Scope::empty(),
                    ).await.unwrap();
                    worker.enable_worker_metadata_write_through().unwrap();
                    worker.materialize_durable_session_head().await.unwrap();
                    let (session_id, segment_id) = (worker.session_id(), worker.segment_id());
                    let receipts = if no_model {
                        // Persist a real empty Session without starting a seed
                        // Controller or submitting any model input.
                        drop(worker);
                        Vec::new()
                    } else {
                        let workspace_client = worker.workspace_client_handle();
                        let started = PreparedWorker::new(
                            worker,
                            WorkerBootstrapLayout::RuntimeManagedRun { run_dir: fixture_root.join("seed") },
                        ).start().await.unwrap();
                        let mut seed = SeedController(Some(RuntimeWorkerController {
                            handle: started.handle,
                            shutdown: Arc::new(tokio::sync::Mutex::new(Some(started.shutdown))),
                            controller_task: started.controller_task,
                            workspace_client,
                            restore_streams: None,
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
                            if id == FIRST { notify_from_source(&handle, notice_source.clone()).await; }
                        }
                        assert_eq!(pending(&handle).await.submissions.len(), 2);
                        shutdown_profile_controller(seed.0.take().unwrap()).await;
                        receipts
                    };
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
            if !no_model {
                let checkpoint = queue_checkpoint(&fixture.entries());
                assert_eq!(checkpoint["pending"].as_array().unwrap().len(), 2);
                assert_eq!(
                    checkpoint["pending_notifications"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
            }
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
            let run_dir = self.backend.factory.worker_restore_run_dir(
                &self.request.worker_ref, self.request.operation_id,
            ).unwrap();
            // Private initialization owns only disposable operation-scoped run
            // artifacts. It has not published a listener, liveness files or a
            // delegated-child projection, and canonical files are checked by
            // each caller independently.
            let mut artifacts = std::fs::read_dir(&run_dir).unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect::<Vec<_>>();
            artifacts.sort();
            assert_eq!(artifacts, ["artifacts", "spawned", "worker.err.log", "worker.out.log"]);
            for log in ["worker.err.log", "worker.out.log"] {
                assert_eq!(std::fs::metadata(run_dir.join(log)).unwrap().len(), 0);
            }
            for directory in ["artifacts", "spawned"] {
                assert_eq!(std::fs::read_dir(run_dir.join(directory)).unwrap().count(), 0);
            }
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
    fn restore_run_reclamation_preserves_live_allocation_owner() {
        let fixture = Fixture::no_model();
        let factory = fixture.backend.factory.clone();
        let request = fixture.request.clone();
        let run = factory
            .worker_restore_run_dir(&request.worker_ref, request.operation_id)
            .unwrap();
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("retained-owner"), b"live allocation").unwrap();
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let allocation = worker::runtime::worker_allocation::install_top_level(
            name.clone(), std::process::id(), run.join("worker.sock"),
            Vec::new(), fixture.segment_id,
        ).unwrap();
        let failure = fixture.backend.run_on_adapter_runtime(async move {
            match factory.prepare_restore(request).await {
                Err(failure) => Ok(failure),
                Ok(_) => Err("live allocation unexpectedly reclaimed".into()),
            }
        }).unwrap();
        assert_eq!(failure.stage, RuntimeRestoreStage::Controller);
        assert_eq!(failure.cause, "restore run directory still has an allocation owner");
        let table = worker::runtime::worker_allocation::LockFileGuard::open(
            &worker::runtime::worker_allocation::default_allocation_path().unwrap(),
        ).unwrap();
        assert_eq!(table.data().find(&name).unwrap().pid, std::process::id());
        assert_eq!(fs::read(run.join("retained-owner")).unwrap(), b"live allocation");
        drop(table);
        drop(allocation);
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn session_availability_requires_attachable_current_controller() {
        let fixture = Fixture::no_model();
        let worker_ref = &fixture.request.worker_ref;
        let observe = || {
            fixture
                .backend
                .worker_session(WorkerSessionObservationRequest {
                    worker_ref: worker_ref.clone(),
                })
        };
        let assert_retained = |availability| {
            assert!(matches!(
                availability,
                runtime_api::WorkerSessionAvailability::RetainedSnapshot { identity, .. }
                    if identity.session_id == fixture.session_id.to_string()
                        && identity.segment_id == fixture.segment_id.to_string()
            ));
        };
        fixture.prepare();
        assert_retained(observe());
        let pending = fixture
            .backend
            .pending_restore(worker_ref)
            .unwrap()
            .unwrap();
        fixture
            .backend
            .activate_restored_worker(fixture.request.operation_id, worker_ref)
            .unwrap();
        let execution = fixture.backend.workers.lock().unwrap()[worker_ref].clone();
        assert!(!execution.handle.protocol_is_closed());
        assert!(!execution.shutdown_requested.load(Ordering::Acquire));
        assert!(matches!(
            observe(),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        #[cfg(feature = "ws-server")]
        let transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(worker_ref)
            .unwrap();
        #[cfg(feature = "ws-server")]
        transport.validate().unwrap();

        // Retain the same real operation owner alongside its registered actual
        // Controller to exercise the pending-resource visibility guard alone.
        let operation_lock = fixture.backend.worker_lock(worker_ref).unwrap();
        {
            let _guard = operation_lock.lock().unwrap();
            fixture
                .backend
                .pending_restores
                .lock()
                .unwrap()
                .insert(worker_ref.clone(), pending);
        }
        let pending_session = observe();
        #[cfg(feature = "ws-server")]
        let pending_protocol = (
            transport.validate(),
            fixture.backend.clone().attach_worker_protocol(worker_ref),
        );
        // Reset injected visibility before assertions so Fixture can also stop
        // and release its allocation when an assertion unwinds.
        {
            let _guard = operation_lock.lock().unwrap();
            fixture.backend.remove_pending_restore(worker_ref).unwrap();
        }
        assert_retained(pending_session);
        #[cfg(feature = "ws-server")]
        {
            assert!(pending_protocol.0.is_err());
            assert!(matches!(
                pending_protocol.1,
                Err(error) if error.outcome == crate::execution::WorkerExecutionOutcome::Busy
            ));
        }
        assert!(matches!(
            observe(),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        {
            let _guard = operation_lock.lock().unwrap();
            execution.shutdown_requested.store(true, Ordering::Release);
        }
        let stopping_session = observe();
        #[cfg(feature = "ws-server")]
        let stopping_protocol = (
            transport.validate(),
            fixture.backend.clone().attach_worker_protocol(worker_ref),
        );
        {
            let _guard = operation_lock.lock().unwrap();
            execution.shutdown_requested.store(false, Ordering::Release);
        }
        assert!(!execution.handle.protocol_is_closed());
        assert_retained(stopping_session);
        #[cfg(feature = "ws-server")]
        {
            assert!(stopping_protocol.0.is_err());
            assert!(stopping_protocol.1.is_err());
            transport.validate().unwrap();
        }
        assert!(matches!(
            observe(),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        let handle = execution.handle.clone();
        let command = next_internal_command(&execution.worker_state).unwrap();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                handle
                    .send(Method::Shutdown { command })
                    .await
                    .map_err(|error| error.to_string())?;
                tokio::time::timeout(WAIT, handle.protocol_closed())
                    .await
                    .map_err(|_| "Controller did not close".to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(execution.handle.protocol_is_closed());
        assert!(!execution.shutdown_requested.load(Ordering::Acquire));
        assert!(
            fixture
                .backend
                .workers
                .lock()
                .unwrap()
                .contains_key(worker_ref)
        );
        assert_retained(observe());
        #[cfg(feature = "ws-server")]
        {
            assert!(transport.validate().is_err());
            assert!(
                fixture
                    .backend
                    .clone()
                    .attach_worker_protocol(worker_ref)
                    .is_err()
            );
        }
        assert!(fixture.backend.stop_worker(worker_ref).is_accepted());
        #[cfg(feature = "ws-server")]
        drop(transport);
        drop(execution);
        fixture.assert_released();
        fixture.assert_quiet();
    }

    // Connects Profile restore activation to ordinary protocol input and the
    // durable Paused boundary; Runtime admission/Accepted ordering is outside it.
    #[test]
    #[cfg(feature = "ws-server")]
    #[serial_test::serial(worker_allocation)]
    fn activated_empty_candidate_accepts_body_input_and_queues_while_paused() {
        use crate::execution::WorkerProtocolTransport;

        const S1: &str = "candidate-body-s1";
        const S2: &str = "candidate-body-s2";
        const IDLE_ONLY: &str = "candidate-body-idle-only";
        const OUTPUT: &str = "committed candidate output before pause";

        // Keep one real connection (paired full snapshot + subscription) for
        // every method and observation. Never reconnect to hide missing events.
        fn observe(
            backend: &WorkerRuntimeExecutionBackend,
            mut transport: WorkerProtocolTransport,
            phase: &'static str,
            no_new_run: bool,
            mut matches: impl FnMut(&Event) -> bool + Send + 'static,
        ) -> (WorkerProtocolTransport, Event) {
            backend
                .run_on_adapter_runtime(async move {
                    let mut seen = Vec::new();
                    let matched = tokio::time::timeout(WAIT, async {
                        loop {
                            let event = transport
                                .events
                                .recv()
                                .await
                                .expect("attached protocol stream");
                            seen.push(format!("{event:?}"));
                            if no_new_run {
                                assert!(
                                    !matches!(
                                        event,
                                        Event::InvokeStart { .. }
                                            | Event::TurnStart { .. }
                                            | Event::UserMessage { .. }
                                            | Event::TextDelta { .. }
                                            | Event::TextDone { .. }
                                    ),
                                    "paused input started another run during {phase}: {event:?}"
                                );
                            }
                            if matches(&event) {
                                break event;
                            }
                            assert!(
                                !matches!(
                                    event,
                                    Event::Error { .. }
                                        | Event::SubmissionRejected { .. }
                                        | Event::NotificationRejected { .. }
                                        | Event::Shutdown
                                ),
                                "unexpected protocol failure during {phase}: {event:?}"
                            );
                        }
                    })
                    .await
                    .unwrap_or_else(|_| {
                        panic!("protocol deadline during {phase}; observed {seen:?}")
                    });
                    Ok((transport, matched))
                })
                .unwrap()
        }

        fn user_inputs(entries: &[LogEntry]) -> Vec<Vec<protocol::Segment>> {
            entries
                .iter()
                .filter_map(|entry| match entry {
                    LogEntry::AnnotatedUserInput { segments, .. } => Some(segments.clone()),
                    _ => None,
                })
                .collect()
        }

        // Declaration order keeps the bootstrap runtime (which owns the
        // existing loopback Host task) alive through Fixture::drop, even on panic.
        let bootstrap_backend;
        let mut fixture = Fixture::no_model();
        let script = MockClient::sequential(vec![MockResponse::Hang(vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, OUTPUT),
            LlmEvent::text_block_stop(0, None),
        ])]);
        // Hang explicitly chains stream::pending() after a complete text block.
        // Hang([]) would exercise pre-token rollback instead of durable Paused.
        let injected = script.clone();
        let factory = fixture
            .backend
            .factory
            .as_ref()
            .clone()
            .with_spawn_model_client_factory(move || Box::new(injected.clone()));
        bootstrap_backend = std::mem::replace(
            &mut fixture.backend,
            Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap()),
        );
        let worker_ref = &fixture.request.worker_ref;
        assert!(user_inputs(&fixture.entries()).is_empty());
        assert!(
            !fixture
                .entries()
                .iter()
                .any(|entry| matches!(entry, LogEntry::Invoke { .. }))
        );
        let before = fixture.canonical_files();
        fixture.prepare(); // actual Profile prepare_restore; not MockFactory::spawn
        fixture.assert_quiet(); // independent empty bootstrap client remains unused
        assert_eq!(script.call_count.load(Ordering::SeqCst), 0);
        assert!(script.captured.lock().unwrap().is_empty());
        assert_eq!(fixture.canonical_files(), before);
        assert!(
            fixture
                .backend
                .clone()
                .attach_worker_protocol(worker_ref)
                .is_err()
        );
        fixture
            .backend
            .activate_restored_worker(fixture.request.operation_id, worker_ref)
            .unwrap();
        let execution = fixture.backend.workers.lock().unwrap()[worker_ref].clone();
        let transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(worker_ref)
            .unwrap();
        transport.validate().unwrap();
        assert!(
            matches!(
                &transport.snapshot,
                Event::Snapshot { session, state, in_flight, .. }
                    if state.state == protocol::WorkerState::Idle
                        && session.pending_submissions == protocol::PendingSubmissionsSnapshot::default()
                        && session == &session_store::public_snapshot::project_current_session_snapshot(&fixture.entries())
                        && in_flight.is_empty()
            ),
            "expected full input-ready snapshot: {:?}",
            transport.snapshot
        );
        assert!(user_inputs(&fixture.entries()).is_empty());
        assert_eq!(script.call_count.load(Ordering::SeqCst), 0);

        assert!(
            transport
                .dispatch(Method::submit_text(S1, S1))
                .unwrap()
                .is_empty()
        );
        let (transport, _) = observe(
            &fixture.backend,
            transport,
            "S1 committed and streaming",
            false,
            {
                let (mut accepted, mut user_message, mut running, mut text_done) =
                    (false, false, false, false);
                move |event| {
                    match event {
                        Event::SubmissionAccepted {
                            submission_request_id,
                            disposition,
                            ..
                        } if submission_request_id == S1 => {
                            assert_eq!(*disposition, protocol::SubmissionDisposition::Started);
                            accepted = true;
                        }
                        Event::UserMessage { segments, .. } => {
                            assert_eq!(segments, &vec![protocol::Segment::text(S1)]);
                            user_message = true;
                        }
                        Event::WorkerState { snapshot } => {
                            running |= snapshot.state
                                == protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                                    protocol::WorkerRunState::Running,
                                ));
                        }
                        Event::TextDone { text } => {
                            assert_eq!(text, OUTPUT);
                            text_done = true;
                        }
                        _ => {}
                    }
                    accepted && user_message && running && text_done
                }
            },
        );
        // Started is only an admission receipt. Require the actual canonical
        // AnnotatedUserInput plus its model-visible history before sending Pause.
        assert_eq!(
            user_inputs(&fixture.entries()),
            vec![vec![protocol::Segment::text(S1)]]
        );
        let history = session_store::collect_state(&fixture.entries()).annotated_history;
        assert!(history.iter().any(|entry| matches!(
            &entry.item, LoggedItem::Message { content, .. }
                if content.iter().any(|part| matches!(part, LoggedContentPart::Text { text } if text == S1))
        )));
        assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
        let pause = next_internal_command(&execution.worker_state).unwrap();
        transport
            .dispatch(Method::Pause { command: pause })
            .unwrap();
        let (transport, _) = observe(&fixture.backend, transport, "Pause completion", false, {
            let (mut acknowledged, mut paused, mut ended) = (false, false, false);
            move |event| {
                match event {
                    Event::CommandAcknowledged { acknowledgement }
                        if acknowledgement.command_id == pause.command_id =>
                    {
                        assert_eq!(acknowledgement.command, protocol::WorkerCommandKind::Pause);
                        assert_eq!(
                            acknowledgement.disposition,
                            protocol::WorkerCommandDisposition::Accepted
                        );
                        acknowledged = true;
                    }
                    Event::WorkerState { snapshot } => {
                        paused |= snapshot.state
                            == protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                                protocol::WorkerRunState::Paused,
                            ));
                    }
                    Event::RunEnd { result } => {
                        assert_eq!(*result, protocol::RunResult::Paused);
                        ended = true;
                    }
                    _ => {}
                }
                acknowledged && paused && ended
            }
        });
        assert_eq!(
            execution.handle.shared_state.snapshot().state,
            protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Paused
            ))
        );
        let paused_entries = fixture.entries();
        assert_eq!(
            user_inputs(&paused_entries),
            vec![vec![protocol::Segment::text(S1)]]
        );
        let invokes = paused_entries
            .iter()
            .filter(|entry| matches!(entry, LogEntry::Invoke { .. }))
            .count();
        assert_eq!(invokes, 1);
        // The hanging model response has not appended a final assistant item.
        // Prove the actual durable Paused boundary, not a streaming TextDone's
        // assumed equivalence to a completed assistant history message.
        assert!(paused_entries.iter().any(|entry| matches!(
            entry,
            LogEntry::RunCompleted {
                interrupted: true,
                result: agen::EngineResult::Paused,
                ..
            }
        )));

        transport.dispatch(Method::submit_text(S2, S2)).unwrap();
        let (transport, queued) = observe(
            &fixture.backend,
            transport,
            "S2 Queued receipt",
            true,
            |event| matches!(event, Event::SubmissionAccepted { submission_request_id, .. } if submission_request_id == S2),
        );
        let submission_id = match &queued {
            Event::SubmissionAccepted {
                submission_id,
                disposition,
                ..
            } => {
                assert_eq!(*disposition, protocol::SubmissionDisposition::Queued);
                submission_id.clone()
            }
            _ => unreachable!(),
        };
        let checkpoint = queue_checkpoint(&fixture.entries());
        assert_eq!(checkpoint["pending"].as_array().unwrap().len(), 1);
        assert_eq!(checkpoint["pending"][0]["submission_request_id"], S2);
        assert_eq!(checkpoint["pending"][0]["submission_id"], submission_id);
        assert_eq!(
            checkpoint["pending"][0]["input"],
            serde_json::to_value(vec![protocol::Segment::text(S2)]).unwrap()
        );
        let receipts = checkpoint["receipts"].as_array().unwrap();
        let s2_receipts: Vec<_> = receipts
            .iter()
            .filter(|receipt| receipt["submission_request_id"] == S2)
            .collect();
        assert_eq!(s2_receipts.len(), 1);
        assert_eq!(s2_receipts[0]["submission_id"], submission_id);
        assert_eq!(s2_receipts[0]["disposition"], "queued");
        transport.dispatch(Method::ListPendingSubmissions).unwrap();
        let (transport, listed) = observe(
            &fixture.backend,
            transport,
            "S2 pending list",
            true,
            |event| matches!(event, Event::PendingSubmissionsChanged { .. }),
        );
        let pending = match listed {
            Event::PendingSubmissionsChanged { pending } => pending,
            _ => unreachable!(),
        };
        assert_eq!(pending.submissions.len(), 1);
        assert_eq!(pending.head_id.as_deref(), Some(submission_id.as_str()));
        assert_eq!(pending.submissions[0].submission_id, submission_id);

        let before_retry = fixture.canonical_files();
        transport.dispatch(Method::submit_text(S2, S2)).unwrap();
        let (transport, replay) = observe(
            &fixture.backend,
            transport,
            "S2 exact retry",
            true,
            |event| matches!(event, Event::SubmissionAccepted { submission_request_id, .. } if submission_request_id == S2),
        );
        assert_eq!(
            serde_json::to_value(replay).unwrap(),
            serde_json::to_value(queued).unwrap()
        );
        assert_eq!(fixture.canonical_files(), before_retry);
        transport.dispatch(Method::ListPendingSubmissions).unwrap();
        let (transport, listed) = observe(
            &fixture.backend,
            transport,
            "retry retains one pending",
            true,
            |event| matches!(event, Event::PendingSubmissionsChanged { .. }),
        );
        assert!(
            matches!(listed, Event::PendingSubmissionsChanged { pending: actual } if actual == pending)
        );

        // Idle-only admission is a different contract from ordinary Submit:
        // observe the real rejection on this connection, not a Queued receipt.
        let mut idle_only = WorkerInput::user(IDLE_ONLY);
        idle_only.kind = WorkerInputKind::UserIfIdle;
        idle_only.submission_request_id = Some(IDLE_ONLY.into());
        let rejected = fixture.backend.dispatch_input(worker_ref, idle_only);
        assert!(!rejected.is_accepted(), "{rejected:?}");
        assert!(
            rejected.message_or_default().contains("WorkerNotify"),
            "{rejected:?}"
        );
        let (transport, _) = observe(
            &fixture.backend,
            transport,
            "Paused idle-only rejection",
            true,
            |event| {
                matches!(event, Event::SubmissionRejected { submission_request_id, message }
                if submission_request_id == IDLE_ONLY && message.contains("WorkerNotify"))
            },
        );
        assert_eq!(fixture.canonical_files(), before_retry);
        assert_eq!(queue_checkpoint(&fixture.entries()), checkpoint);

        transport
            .dispatch(Method::CancelPendingSubmission { submission_id })
            .unwrap();
        let (transport, _) = observe(&fixture.backend, transport, "cancel S2", true, |event| {
            matches!(event, Event::PendingSubmissionsChanged { pending }
                if pending.submissions.is_empty() && pending.head_id.is_none())
        });
        transport.dispatch(Method::ListPendingSubmissions).unwrap();
        let (transport, _) = observe(
            &fixture.backend,
            transport,
            "empty pending list after cancel",
            true,
            |event| {
                matches!(event, Event::PendingSubmissionsChanged { pending }
                if pending == &protocol::PendingSubmissionsSnapshot::default())
            },
        );
        let entries = fixture.entries();
        assert!(
            queue_checkpoint(&entries)["pending"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            user_inputs(&entries),
            vec![vec![protocol::Segment::text(S1)]]
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, LogEntry::Invoke { .. }))
                .count(),
            invokes
        );
        assert_eq!(
            execution.handle.shared_state.snapshot().state,
            protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Paused
            ))
        );
        assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
        assert_eq!(script.captured.lock().unwrap().len(), 1);
        fixture.assert_quiet();
        transport.validate().unwrap();
        // Normal Stop owns shutdown and the Controller/transport abort+join
        // barrier; do not simulate closure or inject a terminal state.
        assert!(fixture.backend.stop_worker(worker_ref).is_accepted());
        assert!(execution.handle.protocol_is_closed());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        assert!(transport.validate().is_err());
        drop(transport);
        drop(execution);
        fixture.assert_released();
        assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
        drop(fixture);
        drop(bootstrap_backend);
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn prepare_saved_inputs_rejects_new_input_and_discard_preserves_authority() {
        let mut fixture = Fixture::new();
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
    fn notify_source_and_lost_ack_receipt_survive_stopped_delivery_and_explicit_restore() {
        let source = protocol::AuthenticatedInputSource::Account {
            account_id: "saved-chat-account".into(),
        };
        let fixture = Fixture::with_seed_source(false, Some(source.clone()));
        let mut retry = WorkerInput::notify(NOTICE);
        retry.source = Some(source.clone());
        retry.submission_request_id = Some(NOTICE.into());
        let before = fixture.canonical_files();
        // A direct notify is not a stopped-Worker mailbox or implicit Restore.
        assert!(
            !fixture
                .backend
                .dispatch_input(&fixture.request.worker_ref, retry.clone())
                .is_accepted()
        );
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert_eq!(fixture.canonical_files(), before);
        let checkpoint = queue_checkpoint(&fixture.entries());
        assert_eq!(
            checkpoint["notification_receipts"][0]["source_namespace"],
            source.namespace()
        );
        fixture.prepare();
        fixture
            .backend
            .activate_restored_worker(fixture.request.operation_id, &fixture.request.worker_ref)
            .unwrap();
        let handle = fixture.backend.workers.lock().unwrap()[&fixture.request.worker_ref]
            .handle
            .clone();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                finished(&handle, SECOND).await;
                Ok(())
            })
            .unwrap();
        // The seed Controller accepted durably before shutdown; its caller lost
        // that receipt. Replay through the concrete adapter must not append or run.
        let committed = fixture.canonical_files();
        let calls = fixture.model.call_count.load(Ordering::SeqCst);
        let accepted = fixture
            .backend
            .dispatch_input(&fixture.request.worker_ref, retry.clone());
        assert!(accepted.is_accepted(), "{accepted:?}");
        assert_eq!(
            accepted.notification.unwrap().notification_request_id,
            NOTICE
        );
        let mut conflict = retry;
        conflict.content = "different-body".into();
        let rejected = fixture
            .backend
            .dispatch_input(&fixture.request.worker_ref, conflict);
        assert_eq!(
            rejected.outcome,
            crate::execution::WorkerExecutionOutcome::Rejected
        );
        assert_eq!(fixture.canonical_files(), committed);
        assert_eq!(fixture.model.call_count.load(Ordering::SeqCst), calls);
        let history = session_store::collect_state(&fixture.entries()).annotated_history;
        let notices = history
            .iter()
            .filter(|entry| match &entry.item {
                LoggedItem::Message { content, .. } => content.iter().any(|part| {
                    matches!(part,
                LoggedContentPart::Text { text } if text.contains(NOTICE))
                }),
                _ => false,
            })
            .collect::<Vec<_>>();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].metadata.origin,
            LoggedSessionHistoryOrigin::HumanInput {
                account_id: "saved-chat-account".into()
            }
        );
        let requests = fixture.model.captured.lock().unwrap();
        assert_eq!(
            serde_json::to_string(&requests[0].items)
                .unwrap()
                .matches(NOTICE)
                .count(),
            1
        );
        assert_eq!(
            serde_json::to_string(&requests[2].items)
                .unwrap()
                .matches(NOTICE)
                .count(),
            1
        );
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
            fixture.host.calls.lock().unwrap().is_empty(),
            "local TaskCreate, FIFO drain and receipt replay must not call the Host"
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
                let memory_calls = Arc::new(AtomicUsize::new(0));
                let memories = memory_calls.clone();
                let authorization_operations = Arc::new(Mutex::new(Vec::new()));
                let operations = authorization_operations.clone();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                request.request.workspace_api.as_mut().unwrap().base_url = format!("http://{}", listener.local_addr().unwrap());
                let app = axum::Router::new()
                    .route("/api/w/workspace-saved-subjektiv/authorization/bound-check", axum::routing::post(
                        move |axum::Json(input): axum::Json<server_api::authorization::BoundOperationAuthorizationRequest>| {
                            let operations = operations.clone();
                            async move {
                                operations.lock().unwrap().push(input.operation.clone());
                                match saved_subjektiv_bound_authorization_fixture(input) {
                                    Ok(response) => (
                                        axum::http::StatusCode::OK,
                                        axum::Json(serde_json::to_value(response).unwrap()),
                                    ),
                                    Err(message) => (
                                        axum::http::StatusCode::FORBIDDEN,
                                        axum::Json(serde_json::json!({ "message": message })),
                                    ),
                                }
                            }
                        }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/memory", axum::routing::post(
                        move |axum::Json(input): axum::Json<server_api::SubjektivMemoryBackendRequest>| {
                            let memories = memories.clone();
                            async move {
                                assert!(matches!(input.operation,
                                    server_api::SubjektivMemoryBackendOperation::ResidentContext(_)));
                                memories.fetch_add(1, Ordering::SeqCst);
                                axum::Json(serde_json::json!({
                                    "result": "resident_context",
                                    "data": {
                                        "behavior_md": "Repaired Subject resident behavior",
                                        "memory_surface": { "availability": "ungenerated", "content": null },
                                    },
                                }))
                            }
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
                assert_eq!(memory_calls.load(Ordering::SeqCst), 0);
                assert!(authorization_operations.lock().unwrap().is_empty());
                factory.cleanup_failed_restore(&request).await.unwrap();
                assert!(factory.recover_saved_restore_source(&request).await.unwrap());
                let repaired = metadata.read_by_name(&name).unwrap().unwrap();
                assert_eq!(repaired.active.as_ref().unwrap().session_id, session_id);
                assert!(matches!(repaired.subjektiv_session_attribution,
                    Some(session_store::SubjektivSessionAttributionState::Confirmed { session_id: id, .. }) if id == session_id));
                let session = WorkerSessionStore::new(aggregate.join("session")).unwrap();
                let segments = session.list_segments(session_id).unwrap();
                assert_eq!(segments.len(), 1, "retry must retain exactly one seed");
                let entries = session.read_all(session_id,
                    repaired.active.as_ref().unwrap().segment_id.unwrap()).unwrap();
                assert_eq!(entries.len(), 2, "one head plus its protected-input checkpoint");
                let prompt = match &entries[0] {
                    LogEntry::AnnotatedSegmentStart {
                        session_id: id, system_prompt: Some(prompt), history,
                        forked_from: None, compacted_from: None, ..
                    } => {
                        assert_eq!(*id, session_id);
                        assert!(history.is_empty(), "repair must not admit model input");
                        prompt
                    }
                    other => panic!("repair must retain a complete initial head: {other:?}"),
                };
                let checkpoint = match &entries[1] {
                    LogEntry::Extension { domain, payload, .. }
                        if domain == "worker.protected_inputs.v1" => payload,
                    other => panic!("missing protected-input checkpoint: {other:?}"),
                };
                let expected_startups = usize::from(!materialized);
                assert_eq!(startup_calls.load(Ordering::SeqCst), expected_startups);
                assert_eq!(memory_calls.load(Ordering::SeqCst), expected_startups);
                assert_eq!(calls.load(Ordering::SeqCst), 2 + expected_startups,
                    "lost receipt retry plus publication verification only for a pending head");
                let mut operations = authorization_operations.lock().unwrap().clone();
                operations.sort();
                if materialized {
                    assert_eq!(session_files(), original);
                    assert_eq!(prompt, SAVED_SUBJEKTIV_SYSTEM_PROMPT);
                    assert_eq!(checkpoint, &serde_json::json!({ "version": 1, "kinds": [] }));
                    assert!(operations.is_empty(), "existing head repair must not refetch protected context");
                } else {
                    assert!(prompt.contains("Repaired Subject resident behavior"));
                    assert_eq!(checkpoint, &serde_json::json!({
                        "version": 1, "kinds": ["subject-resident"],
                    }));
                    assert_eq!(operations, [
                        "journal.startup.read",
                        // Both the resident source and its Host adapter authorize the read.
                        "memory.resident.read", "memory.resident.read",
                        "memory.resident.supply", "memory.resident.supply",
                    ], "empty Journal startup must not require a supply grant");
                }
                let after_repair = persisted_files(&aggregate);
                let calls_after_repair = calls.load(Ordering::SeqCst);
                let authorization_count = authorization_operations.lock().unwrap().len();
                assert!(!factory.recover_saved_restore_source(&request).await.unwrap());
                assert_eq!(persisted_files(&aggregate), after_repair);
                assert_eq!(calls.load(Ordering::SeqCst), calls_after_repair);
                assert_eq!(startup_calls.load(Ordering::SeqCst), expected_startups);
                assert_eq!(memory_calls.load(Ordering::SeqCst), expected_startups);
                assert_eq!(authorization_operations.lock().unwrap().len(), authorization_count);
                assert!(factory.failed_restore_resources.lock().unwrap().is_empty());
                drop(host);
                Ok(())
            }).unwrap();
            assert_eq!(model.call_count.load(Ordering::SeqCst), 0);
        }
    }

    #[cfg(feature = "fs-store")]
    #[test]
    #[serial_test::serial(worker_allocation)]
    fn attachment_preparation_replay_cannot_hide_logical_authority_revocation() {
        use crate::fs_store::{
            FsRuntimeStoreOptions, PersistedWorkerExecution, PersistedWorkerExecutionState,
            PersistedWorkerRecord,
        };
        let mut fixture = Fixture::new();
        Arc::get_mut(&mut fixture.backend)
            .unwrap()
            .spawn_restore_timeout = Duration::from_secs(2);
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.clone().unwrap();
        let options = FsRuntimeStoreOptions {
            root: fixture._root.path().join("catalog"),
            runtime_id: "candidate-runtime".into(),
            display_name: None,
        };
        let attachment = crate::catalog::LogicalWorkdirAttachment {
            alias: WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory_id: "workspace-only-workdir".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
        };
        // Only the initial Runtime catalog is seeded. The saved Profile, queue,
        // restore admission, revocation, replay and Stop use their real owners.
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            options.clone(),
            fixture.backend.clone(),
        )
        .unwrap();
        let create = fixture.request.request.clone();
        runtime
            .test_runtime_store()
            .write_worker_record(&PersistedWorkerRecord {
                restore_guard: crate::runtime::RestoreGuard::default(),
                worker_ref: worker_ref.clone(),
                worker_id: worker_ref.worker_id,
                profile: create.profile.clone(),
                display_name: None,
                profile_source: create.profile_source.reference(),
                config_bundle: create.config_bundle.clone(),
                created_at_ms: None,
                status: crate::catalog::WorkerStatus::Stopped,
                last_finished_submission_request_id: None,
                execution_state: PersistedWorkerExecutionState::Available(
                    PersistedWorkerExecution {
                        request: create,
                        binding: None,
                        restore_intent: crate::catalog::WorkerRestoreIntent::Explicit,
                    },
                ),
                workspace_id: Some(scope.workspace_id.clone()),
                workdir_attachments: Vec::new(),
                logical_workdir_attachments: vec![attachment.clone()],
            })
            .unwrap();
        drop(runtime);
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            options.clone(),
            fixture.backend.clone(),
        )
        .unwrap();
        assert!(
            runtime
                .worker_detail(&worker_ref)
                .unwrap()
                .workdir_attachments
                .is_empty()
        );
        let before = fixture.canonical_files();
        let authority = || -> serde_json::Value {
            serde_json::from_slice(
                &fs::read(
                    options
                        .root
                        .join("workers")
                        .join(worker_ref.worker_id.to_string())
                        .join("worker.json"),
                )
                .unwrap(),
            )
            .unwrap()
        };
        let release = Arc::new(tokio::sync::Notify::new());
        let (entered_tx, entered) = mpsc::channel();
        *fixture
            .backend
            .factory
            .restore_faults
            .factory_gate
            .lock()
            .unwrap() = Some(release.clone());
        *fixture
            .backend
            .factory
            .restore_faults
            .factory_entered
            .lock()
            .unwrap() = Some(entered_tx);
        let mut request = runtime.test_restore_request(&worker_ref);
        request.preparation.as_mut().unwrap().workdir_attachments =
            Some(serde_json::from_value(serde_json::to_value(vec![attachment]).unwrap()).unwrap());
        let first = runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
            .unwrap();
        entered.recv_timeout(WAIT).unwrap();
        assert_eq!(
            first.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "{first:?}"
        );
        let pending = fixture
            .backend
            .pending_restore(&worker_ref)
            .unwrap()
            .unwrap();
        assert!(!pending.request.logical_workdir_attachments.is_empty());
        assert!(pending.request.previous_workdir_attachments.is_empty());
        assert!(pending.request.deferred_workdir_sessions.is_empty());
        runtime
            .replace_worker_workdir_attachments_scoped(&scope, &worker_ref, Vec::new())
            .unwrap();
        assert!(authority().get("logical_workdir_attachments").is_none());
        release.notify_one();
        let replay = runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            replay.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "revocation hidden by saved preparation: {replay:?}"
        );
        assert!(
            replay
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("current Worker authority differs"),
            "{replay:?}"
        );
        let record = authority();
        assert!(record.get("logical_workdir_attachments").is_none());
        let owner = &record["restore_guard"]["owners"][&request.request_id];
        assert_eq!(
            owner["operation_id"],
            serde_json::to_value(pending.request.operation_id).unwrap()
        );
        assert!(owner["receipt"].is_null());
        assert_eq!(
            owner["workdir_attachment_preparation_base"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        let durable_guard =
            serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
        assert_eq!(durable_guard["owners"][&request.request_id], *owner);
        let (coordinated, preparation) = runtime
            .coordinate_worker_restore_operation(
                &worker_ref,
                runtime_api::WorkerRestoreCoordinationRequest {
                    expected_observation_token: request.expected_observation_token.clone(),
                    request_id: request.request_id.clone(),
                    preparation: None,
                },
                Some(&scope),
            )
            .unwrap();
        assert_eq!(
            coordinated.unwrap().state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        assert!(
            preparation.is_none(),
            "coordinator must not export the revoked preparation again"
        );
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(Arc::ptr_eq(
            &pending,
            &fixture
                .backend
                .pending_restore(&worker_ref)
                .unwrap()
                .unwrap()
        ));
        assert!(!pending.activation_started.load(Ordering::SeqCst));
        fixture.assert_quiet();
        assert_eq!(fixture.canonical_files(), before);
        runtime
            .stop_worker_scoped(&scope, &worker_ref, None)
            .unwrap();
        fixture.assert_released();
        fixture.assert_quiet();
        assert_eq!(fixture.canonical_files(), before);
        assert!(authority().get("logical_workdir_attachments").is_none());
        assert_ne!(
            runtime
                .restore_worker_operation_scoped(&scope, &worker_ref, request)
                .unwrap()
                .state,
            server_api::WorkerRestoreState::Accepted
        );
    }

    include!("worker_backend_restore_publication_tests.rs");
}
