//! Backend Job allocation -> production Embedded Runtime factory/Controller -> signed
//! loopback Host checks. Only the external model is replaced. The factory wrapper
//! records real Controllers and holds a completed allocation before Backend binding;
//! it never substitutes creation, authorization, input admission or restore results.
//! Register directly under server::tests. No Git/Workdir is needed by this fixture.
use super::restored_parent_control_tests::connected_restore_tempdir;
use super::*;
use agen::llm_client::{ClientError, LlmClient, Request as ModelRequest, ResponseStream};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration as TestDuration;
use worker_runtime::worker_backend::{
    RuntimeRestoreCandidate, RuntimeRestoreCleanupFailure, RuntimeRestoreFailure,
    RuntimeWorkerController, RuntimeWorkerFactory,
};

const WAIT: TestDuration = TestDuration::from_secs(30);

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("connected Feature initialization boundary timed out")
}

#[derive(Clone)]
struct PendingModel {
    calls: Arc<AtomicUsize>,
    entered: tokio::sync::mpsc::Sender<ModelRequest>,
}

#[async_trait::async_trait]
impl LlmClient for PendingModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> std::result::Result<ResponseStream, ClientError> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        self.entered
            .try_send(request)
            .expect("bounded model observation");
        // Keep this genuine Job attempt live until the test owns its Stop. A final
        // prose response would not be a Job result and could start unrelated cleanup.
        Ok(Box::pin(futures::stream::pending()))
    }
}

#[derive(Clone)]
struct Created {
    worker: RuntimeWorkerRef,
    is_job: bool,
    initialization: protocol::feature_initialization::FeatureInitializations,
    handle: worker::WorkerHandle,
}

#[derive(Default)]
struct Probe {
    created: Mutex<Vec<Created>>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    spawned: tokio::sync::Notify,
    release: tokio::sync::Notify,
    restored: AtomicUsize,
}

struct ObservedFactory {
    inner: ProfileRuntimeWorkerFactory,
    probe: Arc<Probe>,
}

#[async_trait::async_trait]
impl RuntimeWorkerFactory for ObservedFactory {
    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> std::result::Result<(), String> {
        self.inner.observe_workspace_prompt_projection(projection)
    }

    async fn fetch_workspace_config(
        &self,
        request: worker_runtime::execution::WorkspaceConfigFetchRequest,
    ) -> std::result::Result<worker_runtime::execution::WorkspaceConfigFetchResult, String> {
        self.inner.fetch_workspace_config(request).await
    }

    fn retained_session_snapshot(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<
        session_store::RetainedSessionSnapshot,
        session_store::RetainedSnapshotReadError,
    > {
        self.inner.retained_session_snapshot(worker)
    }

    fn retained_session_history_page(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> std::result::Result<protocol::SessionHistoryPage, session_store::RetainedHistoryReadError>
    {
        self.inner
            .retained_session_history_page(worker, cursor, limit)
    }

    fn retained_session_attachment(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        session: &str,
        attachment: &str,
    ) -> std::result::Result<
        session_store::RetainedSessionAttachment,
        session_store::RetainedAttachmentReadError,
    > {
        self.inner
            .retained_session_attachment(worker, session, attachment)
    }

    async fn spawn_controller(
        &self,
        request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        let worker = RuntimeWorkerRef::new(
            EMBEDDED_WORKER_RUNTIME_ID,
            request.worker_ref.worker_id.to_string(),
        );
        let is_job = request.request.backend_job.is_some();
        let initialization = request.request.feature_initialization.clone();
        let controller = self.inner.spawn_controller(request).await?;
        self.probe
            .tasks
            .lock()
            .unwrap()
            .push(controller.controller_task.abort_handle());
        self.probe.created.lock().unwrap().push(Created {
            worker,
            is_job,
            initialization,
            handle: controller.handle.clone(),
        });
        if is_job {
            self.probe.spawned.notify_one();
            // This is AFTER real Feature initialization, BEFORE the successful
            // factory return permits Backend to bind the accepted attempt identity.
            bounded(self.probe.release.notified()).await;
        }
        Ok(controller)
    }

    async fn cleanup_failed_spawn(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), RuntimeRestoreCleanupFailure> {
        self.inner.cleanup_failed_spawn(worker).await
    }

    async fn preflight_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), RuntimeRestoreFailure> {
        self.inner.preflight_restore(request).await
    }

    async fn recover_restore_source(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<bool, RuntimeRestoreFailure> {
        self.inner.recover_restore_source(request).await
    }

    async fn prepare_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        let inner = self.inner.prepare_restore(request).await?;
        Ok(Box::new(ObservedCandidate {
            inner,
            probe: self.probe.clone(),
        }))
    }

    async fn cleanup_failed_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), RuntimeRestoreCleanupFailure> {
        self.inner.cleanup_failed_restore(request).await
    }

    async fn reconcile_stopped_worker(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.reconcile_stopped_worker(worker).await
    }

    fn activate_restored_controller(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        workspace: Option<&str>,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner
            .activate_restored_controller(worker, workspace, handle)
    }

    fn deactivate_stopped_controller(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner.deactivate_stopped_controller(worker, handle)
    }
}

struct ObservedCandidate {
    inner: Box<dyn RuntimeRestoreCandidate>,
    probe: Arc<Probe>,
}

#[async_trait::async_trait]
impl RuntimeRestoreCandidate for ObservedCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        self.inner.initial_state()
    }
    fn has_durable_effects(&self) -> bool {
        self.inner.has_durable_effects()
    }
    fn can_retry_publication(&self) -> bool {
        self.inner.can_retry_publication()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        let controller = self.inner.activate().await?;
        self.probe
            .tasks
            .lock()
            .unwrap()
            .push(controller.controller_task.abort_handle());
        self.probe.restored.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(controller)
    }

    async fn discard(&mut self) -> std::result::Result<(), RuntimeRestoreCleanupFailure> {
        self.inner.discard().await
    }
}

struct ConnectedJob {
    api: WorkspaceApi,
    subject: String,
    source: Option<RuntimeWorkerRef>,
    probe: Arc<Probe>,
    requests: Arc<Mutex<Vec<(String, StatusCode)>>>,
    calls: Arc<AtomicUsize>,
    models: tokio::sync::mpsc::Receiver<ModelRequest>,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for ConnectedJob {
    fn drop(&mut self) {
        self.probe.release.notify_one();
        self.api
            .backend_job_recovery
            .stopping
            .store(true, AtomicOrdering::Release);
        self.api.backend_job_recovery.changed.notify_waiters();
        self.server.abort();
        for task in self.probe.tasks.lock().unwrap().iter() {
            task.abort();
        }
    }
}

impl ConnectedJob {
    async fn new() -> Self {
        let dir = connected_restore_tempdir();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = test_server_config(dir.path());
        config.repositories.clear();
        config.backend_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let broker = BackendResourceBroker::default();
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let probe = Arc::new(Probe::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered, models) = tokio::sync::mpsc::channel(4);
        let model = PendingModel {
            calls: calls.clone(),
            entered,
        };
        let audience = embedded_runtime_request_audience(&config).unwrap();
        let inner = ProfileRuntimeWorkerFactory::new(config.workspace_execution_root.clone())
            .with_embedded_worker_mutation_dispatcher(
                EMBEDDED_WORKER_RUNTIME_ID,
                dispatcher.clone(),
            )
            .with_workspace_request_client(
                worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                    TEST_WORKSPACE_ID,
                    audience.clone(),
                    EMBEDDED_WORKER_RUNTIME_ID,
                )
                .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience),
            )
            .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
            .with_resource_client(Arc::new(broker.clone()))
            .with_spawn_model_client_factory(move || Box::new(model.clone()));
        let execution = WorkerRuntimeExecutionBackend::new(ObservedFactory {
            inner,
            probe: probe.clone(),
        })
        .unwrap();
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            Arc::new(execution),
            broker,
            HostExecutors::default(),
            Some(dispatcher),
        )
        .await
        .unwrap();
        seed_test_delivery_binding(&api);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let app = build_router(api.clone()).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = observed.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    let response = next.run(request).await;
                    let mut requests = observed.lock().unwrap();
                    assert!(requests.len() < 128, "bounded connected HTTP observations");
                    requests.push((path, response.status()));
                    response
                }
            },
        ));
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("connected-feature-job").unwrap())
            .unwrap()
            .id;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Own cleanup before any fallible Controller/Host operation below.
        let mut fixture = Self {
            api,
            subject,
            source: None,
            probe,
            requests,
            calls,
            models,
            server,
            _dir: dir,
        };
        let api = &fixture.api;
        let subject = &fixture.subject;
        // Owner-checked real policy storage, not a replacement permission evaluator.
        set_test_subject_policy_as_owner(
            api,
            subject,
            test_operation_assignment(
                api,
                &[
                    "session.record",
                    "memory.resident.read",
                    "memory.resident.supply",
                    "journal.startup.read",
                    "journal.startup.supply",
                    "memory.consolidation.request",
                    "memory.consolidation.supply",
                ],
            ),
            Default::default(),
        );
        let Json(created) = bounded(create_workspace_worker_as_owner(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                display_name: "connected Feature Job origin".into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections:
                    crate::server::subjektiv_worker_initialization::subject_connection(
                        subject.clone(),
                    )
                    .unwrap(),
                control_operation_id: Some(Uuid::now_v7().to_string()),
                authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                    assignment: None,
                },
            }),
        ))
        .await
        .unwrap();
        fixture.source = Some(RuntimeWorkerRef::new(created.runtime_id, created.worker_id));
        assert_eq!(fixture.probe.created.lock().unwrap().len(), 1);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 0);
        fixture.requests.lock().unwrap().clear();
        fixture
    }

    fn request(&self) -> BackendJobRequest {
        BackendJobRequest {
            job_id: format!("connected-feature-init:{}", Uuid::now_v7()),
            purpose: "subjektiv_consolidation".into(),
            input_ref: format!("subjektiv://{}/consolidation", self.subject),
            input: serde_json::json!({"subject_id":self.subject,"candidate_ids":[]}),
            instruction: "Inspect the immutable candidate batch; final prose is not completion."
                .into(),
            profile: "builtin:subjektiv-memory-consolidation".into(),
            grants: crate::backend_job::BackendJobGrants {
                subjektiv_consolidation: Some(crate::backend_job::SubjektivConsolidationGrant {
                    subject_id: self.subject.clone(),
                    candidate_ids: vec![],
                }),
                ..Default::default()
            },
            serialization_key: None,
            source_worker: Some(self.source.clone().expect("real source Controller created")),
            notification_target: None,
            limits: Default::default(),
        }
    }

    fn dispatch(
        &self,
        request: BackendJobRequest,
    ) -> tokio::task::JoinHandle<crate::Result<BackendJobReservation>> {
        let api = self.api.clone();
        tokio::task::spawn_blocking(move || api.dispatch_backend_job(&request))
    }

    async fn allocated(&self) -> Created {
        bounded(self.probe.spawned.notified()).await;
        let created = self
            .probe
            .created
            .lock()
            .unwrap()
            .iter()
            .find(|created| created.is_job)
            .unwrap()
            .clone();
        assert_eq!(self.calls.load(AtomicOrdering::SeqCst), 0);
        assert!(
            subjektiv_job::active_grant(&self.api, TEST_WORKSPACE_ID, &created.worker)
                .unwrap()
                .is_none(),
            "allocation has not yet been bound as an accepted Backend attempt"
        );
        let connection: subjektiv_api::worker_initialization::ResolvedWorkerConnection = created
            .initialization
            .get(subjektiv_api::worker_initialization::FEATURE_ID)
            .unwrap()
            .decode(
                subjektiv_api::worker_initialization::FEATURE_ID,
                subjektiv_api::worker_initialization::VERSION,
            )
            .unwrap();
        assert_eq!(
            connection.kind,
            subjektiv_api::worker_initialization::WorkerConnectionKind::MemoryConsolidation
        );
        assert_eq!(
            connection.subject_id.as_deref(),
            Some(self.subject.as_str())
        );
        assert_eq!(connection.workspace_id, TEST_WORKSPACE_ID);
        use session_store::WorkerMetadataStore as _;
        let aggregate = self
            .api
            .config
            .embedded_runtime_store_root
            .join("workers")
            .join(&created.worker.worker_id);
        let name = format!("worker-runtime-{}", created.worker.worker_id);
        let metadata = session_store::WorkerAggregateStore::open_read_only(aggregate, &name)
            .unwrap()
            .read_by_name(&name)
            .unwrap()
            .unwrap();
        assert!(
            metadata.subjektiv_session_attribution.is_none(),
            "a Job must not acquire Subject body Session attribution"
        );
        assert!(
            self.connection_checks().is_empty(),
            "New allocation must not query active_grant"
        );
        assert!(
            !serde_json::to_string(&created.handle.sink.subscribe_with_snapshot().0)
                .unwrap()
                .contains("input_json:"),
            "allocation is still before initial protected Submit"
        );
        created
    }

    fn connection_checks(&self) -> Vec<StatusCode> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.ends_with("/subjektiv/worker-connection"))
            .map(|(_, status)| *status)
            .collect()
    }

    async fn stop(&self, worker: &RuntimeWorkerRef) {
        let runtime = self.api.runtime.clone();
        let worker = worker.clone();
        let result = bounded(tokio::task::spawn_blocking(move || {
            runtime.stop_worker(&worker, WorkerLifecycleRequest::default())
        }))
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            result.state,
            InternalWorkerOperationState::Accepted,
            "{result:?}"
        );
    }

    async fn restore(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> std::result::Result<InternalWorkerRestoreResult, RuntimeRegistryError> {
        let request = runtime_api::WorkerRestoreRequest {
            expected_observation_token: self
                .api
                .runtime
                .worker(worker)
                .unwrap()
                .restore_observation_token
                .expect("real Stop supplies a restore observation"),
            request_id: Uuid::now_v7().to_string(),
            preparation: Some(runtime_api::WorkerRestorePreparation::default()),
        };
        let runtime = self.api.runtime.clone();
        let worker = worker.clone();
        bounded(tokio::task::spawn_blocking(move || {
            runtime.restore_worker(&worker, request)
        }))
        .await
        .unwrap()
    }

    async fn running(&mut self) -> (Created, BackendJobReservation) {
        let request = self.request();
        let expected = request
            .worker_input(&crate::backend_job::attempt_id(&request.job_id, 1))
            .unwrap();
        let dispatch = self.dispatch(request);
        let created = self.allocated().await;
        self.probe.release.notify_one();
        let reservation = bounded(dispatch).await.unwrap().unwrap();
        assert_eq!(
            reservation.attempt.state,
            BackendJobAttemptState::Dispatched
        );
        assert_eq!(reservation.attempt.worker.as_ref(), Some(&created.worker));
        let model = bounded(self.models.recv())
            .await
            .expect("initial real Controller model request");
        assert!(
            model
                .items
                .iter()
                .filter_map(agen::llm_client::types::Item::as_text)
                .any(|text| text.contains(&expected)),
            "real frozen input must reach the model"
        );
        assert_eq!(self.calls.load(AtomicOrdering::SeqCst), 1);
        // Quiesce only future producer recovery/cleanup. Stop and Restore below
        // remain real; no fake terminal record or fake Runtime response is installed.
        bounded(self.api.stop_backend_job_recovery()).await;
        (created, reservation)
    }

    async fn close(&self) {
        bounded(self.api.stop_backend_job_recovery()).await;
        let workers: Vec<_> = self
            .probe
            .created
            .lock()
            .unwrap()
            .iter()
            .map(|created| created.worker.clone())
            .collect();
        for worker in workers {
            if self
                .api
                .runtime
                .worker(&worker)
                .is_ok_and(|worker| !worker.state.eq_ignore_ascii_case("stopped"))
            {
                self.stop(&worker).await;
            }
        }
        bounded(shutdown_workspace_feature_storage(&self.api))
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn new_job_initializes_without_active_grant_get_but_same_id_restore_checks_it() {
    let mut fixture = ConnectedJob::new().await;
    let (created, reservation) = fixture.running().await;
    assert!(fixture.connection_checks().is_empty());
    assert!(
        subjektiv_job::active_grant(&fixture.api, TEST_WORKSPACE_ID, &created.worker)
            .unwrap()
            .is_some()
    );
    fixture.stop(&created.worker).await;
    assert!(created.handle.protocol_is_closed());
    assert!(
        subjektiv_job::active_grant(&fixture.api, TEST_WORKSPACE_ID, &created.worker)
            .unwrap()
            .is_some()
    );
    let restored = fixture.restore(&created.worker).await.unwrap();
    assert_eq!(
        restored.state,
        server_api::WorkerRestoreState::Accepted,
        "{restored:?}"
    );
    assert!(
        !fixture.connection_checks().is_empty(),
        "Restore must contact the current signed Host"
    );
    assert!(
        fixture
            .connection_checks()
            .iter()
            .all(|status| *status == StatusCode::OK)
    );
    assert_eq!(fixture.probe.restored.load(AtomicOrdering::SeqCst), 1);
    // Activation may resume saved input according to the production restore
    // contract. This test supplies no second Submit and does not forbid that
    // behavior; the refused-restore cases below require no additional model call.
    let saved = fixture
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &reservation.attempt.job_id,
            &reservation.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(saved.worker, Some(created.worker));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stopped_job_restore_rejects_revoked_policy_or_terminal_attempt_before_publication() {
    for terminal_attempt in [false, true] {
        let mut fixture = ConnectedJob::new().await;
        let (created, reservation) = fixture.running().await;
        fixture.stop(&created.worker).await;
        if terminal_attempt {
            // Revoke the original live attempt using its production transition.
            // Recovery has quiesced, so retained stopped catalog data stays available
            // to exercise initialization refusal rather than merely missing Worker.
            fixture
                .api
                .finish_backend_job_attempt(
                    &reservation.attempt.job_id,
                    &reservation.attempt.attempt_id,
                    BackendJobAttemptState::Failed,
                    "connected_test_cancelled",
                    "Restore must not resurrect a terminal Job",
                )
                .unwrap();
            assert!(
                subjektiv_job::active_grant(&fixture.api, TEST_WORKSPACE_ID, &created.worker)
                    .is_err()
            );
        } else {
            set_test_subject_policy_as_owner(
                &fixture.api,
                &fixture.subject,
                test_operation_assignment(&fixture.api, &[]),
                Default::default(),
            );
            assert!(
                subjektiv_job::active_grant(&fixture.api, TEST_WORKSPACE_ID, &created.worker)
                    .unwrap()
                    .is_some(),
                "a frozen grant is not current operation permission"
            );
        }
        fixture.requests.lock().unwrap().clear();
        let restored = fixture.restore(&created.worker).await;
        assert!(
            !restored
                .as_ref()
                .is_ok_and(|result| result.state == server_api::WorkerRestoreState::Accepted),
            "revoked Job unexpectedly restored: {restored:?}"
        );
        assert!(
            fixture.connection_checks().contains(&StatusCode::FORBIDDEN),
            "the real current-connection endpoint must refuse restore, not a fixture preflight"
        );
        assert_eq!(fixture.probe.restored.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 1);
        assert!(
            fixture
                .api
                .runtime
                .worker(&created.worker)
                .unwrap()
                .state
                .eq_ignore_ascii_case("stopped")
        );
        fixture.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn policy_denied_during_real_job_creation_prevents_initial_protected_submit_and_llm() {
    let fixture = ConnectedJob::new().await;
    let request = fixture.request();
    let job_id = request.job_id.clone();
    let dispatch = fixture.dispatch(request);
    let created = fixture.allocated().await;
    // The real factory has initialized, but Backend still cannot bind/submit. This
    // recreates the slow-create policy race without forging a Job attempt or grant.
    set_test_subject_policy_as_owner(
        &fixture.api,
        &fixture.subject,
        test_operation_assignment(&fixture.api, &[]),
        Default::default(),
    );
    fixture.probe.release.notify_one();
    let outcome = bounded(dispatch).await.unwrap();
    assert!(
        outcome.is_err(),
        "post-create input authorization must refuse: {outcome:?}"
    );
    bounded(fixture.api.stop_backend_job_recovery()).await;
    let attempts = fixture
        .api
        .store
        .list_backend_job_attempts(TEST_WORKSPACE_ID, &job_id)
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].worker.as_ref(), Some(&created.worker));
    assert_eq!(attempts[0].state, BackendJobAttemptState::Failed);
    assert_eq!(
        attempts[0].failure_category.as_deref(),
        Some("input_authorization_denied")
    );
    assert_eq!(fixture.calls.load(AtomicOrdering::SeqCst), 0);
    let history = serde_json::to_string(&created.handle.sink.subscribe_with_snapshot().0).unwrap();
    assert!(
        !history.contains("input_json:"),
        "denied frozen input reached canonical Session: {history}"
    );
    assert!(
        fixture.connection_checks().is_empty(),
        "New must not use restore-only active_grant GET"
    );
    fixture.close().await;
}
