mod subjektiv_initialization_tests {
    //! Real Runtime create, Profile factory, Controller, aggregate persistence,
    //! Stop and fresh same-ID restore. Only model I/O and loopback Workspace
    //! transport are replaced; real Backend policy/Job admission are not tested.
    //! Selected by the normal fs-store + ws-server + http-server lib suite and
    //! its existing isolated Worker allocation environment.
    use super::*;
    use protocol::session_failure::{
        SessionPrincipal, SessionRegistrationCause, session_registration_attachment,
    };

    const WORKSPACE: &str = "workspace-saved-subjektiv";
    const SUBJECT: &str = "saved-subject";

    #[derive(Default)]
    struct HostState {
        connections: AtomicUsize,
        records: Mutex<Vec<String>>,
        resident: AtomicUsize,
        startup: AtomicUsize,
        restored: AtomicBool,
        wrong_subject: AtomicBool,
        deny_record: bool,
    }
    struct Host {
        base_url: String,
        state: Arc<HostState>,
        task: tokio::task::JoinHandle<()>,
        _runtime: tokio::runtime::Runtime,
    }
    impl Drop for Host {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Host {
        fn new(deny_record: bool) -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let state = Arc::new(HostState {
                deny_record,
                ..Default::default()
            });
            let (base_url, task) = runtime.block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base_url = format!("http://{}", listener.local_addr().unwrap());
                let connection = state.clone();
                let record = state.clone();
                let resident = state.clone();
                let startup = state.clone();
                let app = axum::Router::new()
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/worker-connection", axum::routing::get(move || {
                        let state = connection.clone();
                        async move {
                            state.connections.fetch_add(1, Ordering::SeqCst);
                            let subject = if state.wrong_subject.load(Ordering::SeqCst) { "replacement-subject" } else { SUBJECT };
                            axum::Json(serde_json::json!({"workspace_id":WORKSPACE,"subject_id":subject,"kind":"subject_body"}))
                        }
                    }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/sessions", axum::routing::post(move |axum::Json(input): axum::Json<server_api::SubjektivRecordSessionRequest>| {
                        let state = record.clone();
                        async move {
                            assert!(!state.restored.load(Ordering::SeqCst), "restore must not replay Session registration");
                            assert!(input.create_if_missing);
                            state.records.lock().unwrap().push(input.session_id.clone());
                            if state.deny_record {
                                let error = server_api::RepositoryApiError::new(403,"PermissionDenied","private token /internal/denial",vec![])
                                    .with_failure(session_registration_attachment(SessionRegistrationCause::denied(SessionPrincipal::Subject)).unwrap());
                                (axum::http::StatusCode::FORBIDDEN, axum::Json(serde_json::to_value(error).unwrap()))
                            } else {
                                (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"session_id":input.session_id,"subject_id":SUBJECT})))
                            }
                        }
                    }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/memory", axum::routing::post(move |axum::Json(input): axum::Json<server_api::SubjektivMemoryBackendRequest>| {
                        let state = resident.clone();
                        async move {
                            assert!(!state.restored.load(Ordering::SeqCst), "restore must not reseed resident data");
                            assert!(matches!(input.operation, server_api::SubjektivMemoryBackendOperation::ResidentContext(_)));
                            assert_eq!(state.records.lock().unwrap().len(), 1);
                            state.resident.fetch_add(1, Ordering::SeqCst);
                            axum::Json(serde_json::json!({"result":"resident_context","data":{
                                "behavior_md":"Feature-owned initialization behavior",
                                "memory_surface":{"availability":"ungenerated","content":null}
                            }}))
                        }
                    }))
                    .route("/api/w/workspace-saved-subjektiv/subjektiv/journal/startup", axum::routing::get(move || {
                        let state = startup.clone();
                        async move {
                            assert!(!state.restored.load(Ordering::SeqCst), "restore must not reseed startup Journals");
                            assert_eq!(state.records.lock().unwrap().len(), 1);
                            state.startup.fetch_add(1, Ordering::SeqCst);
                            axum::Json(serde_json::json!({"subject_id":SUBJECT,"journals":[],"omitted":false}))
                        }
                    }))
                    .route("/api/w/workspace-saved-subjektiv/authorization/bound-check", axum::routing::post(|axum::Json(input): axum::Json<server_api::authorization::BoundOperationAuthorizationRequest>| async move {
                        authorization_response(saved_subjektiv_bound_authorization_fixture(input))
                    }))
                    .route("/api/w/workspace-saved-subjektiv/authorization/check", axum::routing::post(|axum::Json(input): axum::Json<server_api::authorization::OperationAuthorizationRequest>| async move {
                        authorization_response(saved_subjektiv_authorization_fixture(input))
                    }));
                (base_url, tokio::spawn(async move { axum::serve(listener,app).await.unwrap() }))
            });
            Self {
                base_url,
                state,
                task,
                _runtime: runtime,
            }
        }
    }

    fn authorization_response(
        result: Result<server_api::authorization::OperationAuthorizationResponse, &'static str>,
    ) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
        match result {
            Ok(response) => (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::to_value(response).unwrap()),
            ),
            Err(_) => (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(
                    serde_json::to_value(server_api::RepositoryApiError::new(
                        403,
                        "PermissionDenied",
                        "fixture operation is not granted",
                        vec![],
                    ))
                    .unwrap(),
                ),
            ),
        }
    }

    fn request(host: &Host) -> (CreateWorkerRequest, crate::config_bundle::ConfigBundle) {
        let archive = crate::profile_archive::ProfileSourceArchive::build_evaluated_profile(
            "profile-source-archive:subject-initialization".into(), "builtin:companion".into(),
            serde_json::json!({"slug":"subject-initialization","description":"Subject initialization test","scope":"workspace_read",
                "model":{"scheme":"anthropic","model_id":"unused","auth":{"kind":"none"}},
                "engine":{"max_tokens":100},"feature":{"memory":{"enabled":false},"subjektiv":{"enabled":true,"extraction":{"enabled":false}}}})
        ).unwrap();
        let mut bundle = test_bundle();
        bundle.metadata.workspace_id = WORKSPACE.into();
        bundle.profile_source_archive = Some(archive.clone());
        bundle.prompt_catalog = Some(
            worker::PromptCatalog::builtins_only()
                .unwrap()
                .projection()
                .clone(),
        );
        let bundle = bundle.with_computed_digest();
        let mut request = create_request("Subject initialization");
        request.profile_source = ProfileSourceArchiveSource::Embedded { archive };
        request.config_bundle = Some(ConfigBundleRef {
            id: bundle.metadata.id.clone(),
            digest: bundle.metadata.digest.clone(),
        });
        request.workspace_api = Some(WorkspaceApiRef {
            workspace_id: WORKSPACE.into(),
            base_url: host.base_url.clone(),
        });
        request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: WORKSPACE.into(),
            language: "English".into(),
        });
        request.feature_initialization =
            crate::catalog::test_subject_initialization(WORKSPACE, SUBJECT);
        (request, bundle)
    }

    #[derive(Clone)]
    struct Model {
        client: MockClient,
        host: Arc<HostState>,
        called: mpsc::Sender<()>,
    }
    #[async_trait]
    impl LlmClient for Model {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            assert_eq!(self.host.records.lock().unwrap().len(), 1);
            assert!(self.host.resident.load(Ordering::SeqCst) > 0);
            assert!(self.host.startup.load(Ordering::SeqCst) > 0);
            assert!(!self.host.deny_record);
            self.called.send(()).unwrap();
            self.client.stream(request).await
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn subject_create_initial_submit_stop_and_fresh_same_id_restore_preserve_saved_composition() {
        require_restore_test_environment();
        let root = tempfile::tempdir().unwrap();
        let runtime_root = root.path().join("runtime");
        let host = Host::new(false);
        let (called, observed) = mpsc::channel();
        let model = Model {
            client: MockClient::new(simple_text_events()),
            host: host.state.clone(),
            called,
        };
        let factory = || {
            let model = model.clone();
            ProfileRuntimeWorkerFactory::new(root.path().join("no-current-profile"))
                .with_runtime_store_dir(&runtime_root)
                .with_runtime_id("subject-initialization-runtime")
                .with_spawn_model_client_factory(move || Box::new(model.clone()))
        };
        let options = || {
            crate::FsRuntimeStoreOptions::new(&runtime_root)
                .with_runtime_id("subject-initialization-runtime")
        };
        let scope = crate::runtime::RuntimeWorkspaceScope::new(WORKSPACE, "server-main");
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory()).unwrap());
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options(), backend.clone())
                .unwrap();
        let (mut request, bundle) = request(&host);
        runtime.store_config_bundle(bundle).unwrap();
        request.initial_input = Some(WorkerInput::user("first Subject input"));
        let detail = runtime
            .create_worker_scoped(&scope, request.clone())
            .unwrap();
        observed
            .recv_timeout(Duration::from_secs(10))
            .expect("model reached only after identity, registration and protected startup data");
        let handle = backend
            .workers
            .lock()
            .unwrap()
            .get(&detail.worker_ref)
            .unwrap()
            .handle
            .clone();
        backend
            .run_on_adapter_runtime(async move {
                let mut events = handle.subscribe();
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let snapshot = handle.shared_state.snapshot();
                        if snapshot.state == protocol::WorkerState::Idle
                            && snapshot.last_finished_submission_request_id.is_some()
                        {
                            break;
                        }
                        let event = events.recv().await.expect("initial turn completion event");
                        assert!(
                            !matches!(
                                event,
                                Event::Error { .. } | Event::SubmissionRejected { .. }
                            ),
                            "initial turn failed: {event:?}"
                        );
                    }
                })
                .await
                .expect("first Subject turn must settle before Stop");
                Ok(())
            })
            .unwrap();
        runtime
            .stop_worker_scoped(&scope, &detail.worker_ref, None)
            .unwrap();
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&detail.worker_ref);
        let aggregate = runtime_root
            .join("workers")
            .join(detail.worker_ref.worker_id.to_string());
        let store = WorkerAggregateStore::open_read_only(&aggregate, &name).unwrap();
        let saved = store.read_by_name(&name).unwrap().unwrap();
        let active = saved.active.as_ref().unwrap();
        assert!(
            matches!(saved.subjektiv_session_attribution.as_ref(),Some(session_store::SubjektivSessionAttributionState::Confirmed {session_id,subject_id}) if *session_id==active.session_id && subject_id==SUBJECT)
        );
        let session = WorkerSessionStore::new(aggregate.join("session")).unwrap();
        let entries = session
            .read_all(active.session_id, active.segment_id.unwrap())
            .unwrap();
        assert!(entries.iter().any(|entry| matches!(entry,LogEntry::AnnotatedSegmentStart {system_prompt:Some(prompt),..} if prompt.contains("Feature-owned initialization behavior"))));
        let seed_reads = (
            host.state.resident.load(Ordering::SeqCst),
            host.state.startup.load(Ordering::SeqCst),
        );
        let checks = host.state.connections.load(Ordering::SeqCst);
        assert!(checks > 0);
        drop(runtime);
        drop(backend);
        host.state.restored.store(true, Ordering::SeqCst);
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory()).unwrap());
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options(), backend.clone())
                .unwrap();
        let restored = runtime
            .restore_worker_scoped(
                &scope,
                &detail.worker_ref,
                runtime.test_restore_request(&detail.worker_ref),
            )
            .unwrap();
        assert_eq!(restored.worker_ref, detail.worker_ref);
        runtime
            .stop_worker_scoped(&scope, &detail.worker_ref, None)
            .unwrap();
        let after = store.read_by_name(&name).unwrap().unwrap();
        assert_eq!(after.active, saved.active);
        assert_eq!(
            after.resolved_manifest_snapshot,
            saved.resolved_manifest_snapshot
        );
        assert_eq!(
            after.subjektiv_session_attribution,
            saved.subjektiv_session_attribution
        );
        assert_eq!(
            host.state.records.lock().unwrap().as_slice(),
            &[active.session_id.to_string()]
        );
        assert!(host.state.connections.load(Ordering::SeqCst) > checks);
        assert_eq!(
            (
                host.state.resident.load(Ordering::SeqCst),
                host.state.startup.load(Ordering::SeqCst)
            ),
            seed_reads
        );
        host.state.wrong_subject.store(true, Ordering::SeqCst);
        assert!(
            runtime
                .restore_worker_scoped(
                    &scope,
                    &detail.worker_ref,
                    runtime.test_restore_request(&detail.worker_ref)
                )
                .is_err(),
            "changed current Subject identity must not fall back to the saved attachment or register a new Session"
        );
        let refused = store.read_by_name(&name).unwrap().unwrap();
        assert_eq!(refused.active, after.active);
        assert_eq!(
            refused.resolved_manifest_snapshot,
            after.resolved_manifest_snapshot
        );
        assert_eq!(
            refused.subjektiv_session_attribution,
            after.subjektiv_session_attribution
        );
        assert_eq!(refused.startup_failure, after.startup_failure);
        assert_eq!(host.state.records.lock().unwrap().len(), 1);
        assert!(
            observed.try_recv().is_err(),
            "restore must not redispatch the saved initial Submit"
        );
        assert!(!root.path().join("no-current-profile").exists());
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn subject_create_typed_session_denial_keeps_returned_cause_without_initial_submit() {
        require_restore_test_environment();
        let root = tempfile::tempdir().unwrap();
        let runtime_root = root.path().join("runtime");
        let host = Host::new(true);
        let model = MockClient::new(Vec::new());
        let injected = model.clone();
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path().join("no-current-profile"))
                    .with_runtime_store_dir(&runtime_root)
                    .with_runtime_id("subject-denial-runtime")
                    .with_spawn_model_client_factory(move || Box::new(injected.clone())),
            )
            .unwrap(),
        );
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            crate::FsRuntimeStoreOptions::new(&runtime_root)
                .with_runtime_id("subject-denial-runtime"),
            backend.clone(),
        )
        .unwrap();
        let (mut request, bundle) = request(&host);
        runtime.store_config_bundle(bundle).unwrap();
        request.initial_input = Some(WorkerInput::user("must never reach model"));
        let worker_ref = WorkerRef::new(request.worker_id);
        let scope = crate::runtime::RuntimeWorkspaceScope::new(WORKSPACE, "server-main");
        let error = runtime.create_worker_scoped(&scope, request).unwrap_err();
        let result = error
            .execution_result()
            .expect("create refusal must retain typed result");
        let Some(error_contract::DecodeFailure::Known(cause)) =
            result.startup_failure::<SessionRegistrationCause>()
        else {
            panic!("typed Session denial must not become a generic Runtime failure")
        };
        assert_eq!(
            *cause.facts().cause(),
            SessionRegistrationCause::denied(SessionPrincipal::Subject)
        );
        // Completed Runtime create compensation removes the aggregate. The typed
        // denial remains in the returned result, not a fictitious retained Worker.
        let aggregate = runtime_root
            .join("workers")
            .join(worker_ref.worker_id.to_string());
        assert!(!aggregate.exists());
        assert_eq!(host.state.records.lock().unwrap().len(), 1);
        assert_eq!(host.state.resident.load(Ordering::SeqCst), 0);
        assert_eq!(host.state.startup.load(Ordering::SeqCst), 0);
        assert_eq!(model.call_count.load(Ordering::SeqCst), 0);
        assert!(!backend.workers.lock().unwrap().contains_key(&worker_ref));
        let saved = serde_json::to_string(&result.startup_failure).unwrap();
        assert!(!saved.contains("private token"));
        assert!(!saved.contains("/internal/denial"));
    }
}
