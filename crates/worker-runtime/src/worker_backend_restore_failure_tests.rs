// Integration coverage through EmbeddedRuntime -> real Profile factory -> real
// Worker/bootstrap, local Git Workdir provider, and allocation table. External
// model I/O is replaced for saved-input coverage; this is not product E2E.

struct RestoreOpenCountingMaterializer {
    inner: RuntimeGitMaterializer,
    opens: Arc<AtomicUsize>,
}

impl WorkingDirectoryMaterializer for RestoreOpenCountingMaterializer {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        self.inner.materialize(worker_ref, request)
    }

    fn create(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        self.inner.create(request)
    }

    fn authorize_repository_access(
        &self,
        request: &WorkingDirectoryRepositoryAccessRequest,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        self.inner.authorize_repository_access(request)
    }

    fn observe_repository_ref(
        &self,
        request: &RepositoryRefObservationRequest,
    ) -> Result<RepositoryRefObservation, WorkingDirectoryDiagnostic> {
        self.inner.observe_repository_ref(request)
    }

    fn bind_working_directory(
        &self,
        id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.inner.bind_working_directory(id, relative_cwd)
    }

    fn preflight_bind_working_directory(
        &self,
        id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        self.inner
            .preflight_bind_working_directory(id, relative_cwd)
    }

    fn list_working_directories(
        &self,
    ) -> Result<Vec<WorkingDirectoryStatus>, WorkingDirectoryDiagnostic> {
        self.inner.list_working_directories()
    }

    fn working_directory_status(
        &self,
        id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.inner.working_directory_status(id)
    }

    fn cleanup_working_directory(
        &self,
        id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        self.inner.cleanup_working_directory(id)
    }

    fn cleanup(&self, binding: &WorkingDirectoryBinding) -> Result<(), WorkingDirectoryDiagnostic> {
        self.inner.cleanup(binding)
    }
}

// Substitute only initial creation: a new aggregate/Session is left pending
// before its first head, without ever materializing or rewinding a saved head.
// All Restore preflight, repair, retry, activation and Stop use the real backend.
struct PendingProfileSeedBackend {
    inner: Arc<WorkerRuntimeExecutionBackend>,
}

impl WorkerExecutionBackend for PendingProfileSeedBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        let aggregate = self
            .inner
            .factory
            .worker_aggregate_dir(&request.worker_ref)
            .unwrap();
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        self.inner
            .run_on_adapter_runtime(async move {
                assert!(
                    !aggregate.join("session").exists(),
                    "pending fixture requires a new Session aggregate"
                );
                let manifest = manifest::WorkerManifest::from_toml(&format!(
                    r#"
[worker]
name = "{name}"
pwd = "./"
[scope]
allow = []
[model]
scheme = "anthropic"
model_id = "test-model"
auth = {{ kind = "none" }}
[engine]
max_tokens = 100
"#
                ))
                .unwrap();
                let sessions = WorkerSessionStore::new(aggregate.join("session")).unwrap();
                let metadata = WorkerAggregateStore::new(&aggregate, &name).unwrap();
                assert!(metadata.read_by_name(&name).unwrap().is_none());
                let engine =
                    Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                        MockClient::new(Vec::new()),
                    );
                let mut head = Worker::new(
                    manifest.clone(),
                    engine,
                    CombinedStore::new(sessions.clone(), metadata.clone()),
                    WorkerWorkspaceContext::unavailable(
                        None,
                        "initial creation interrupted before first head",
                    ),
                    WorkerFilesystemAuthority::None,
                    Scope::empty(),
                )
                .await
                .unwrap();
                head.enable_worker_metadata_write_through().unwrap();
                let mut saved = metadata.read_by_name(&name).unwrap().unwrap();
                let active = saved.active.as_ref().unwrap();
                assert!(active.segment_id.is_none());
                assert!(sessions.list_segments(active.session_id).unwrap().is_empty());
                saved.resolved_manifest_snapshot =
                    Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap());
                metadata.write(&saved).unwrap();
                drop(head);
                Ok(())
            })
            .unwrap();
        WorkerExecutionSpawnResult::Connected {
            worker_state: protocol::WorkerStateSnapshot::initial(),
            workdir_attachments: Vec::new(),
        }
    }

    fn fetch_workspace_config(
        &self,
        request: WorkspaceConfigFetchRequest,
    ) -> Result<WorkspaceConfigFetchResult, String> {
        self.inner.fetch_workspace_config(request)
    }

    fn observe_workspace_prompt_projection(
        &self,
        projection: worker::WorkspacePromptProjection,
    ) -> Result<(), String> {
        self.inner.observe_workspace_prompt_projection(projection)
    }

    fn preflight_restore(
        &self,
        request: &WorkerExecutionRestoreRequest,
    ) -> Result<(), WorkerExecutionResult> {
        self.inner.preflight_restore(request)
    }

    fn completed_restore_failure(
        &self,
        operation_id: crate::execution::WorkerLifecycleOperationId,
        worker_ref: &WorkerRef,
    ) -> Option<WorkerExecutionSpawnResult> {
        self.inner
            .completed_restore_failure(operation_id, worker_ref)
    }

    fn restore_worker(&self, request: WorkerExecutionRestoreRequest) -> WorkerExecutionSpawnResult {
        self.inner.restore_worker(request)
    }

    fn reconcile_restore(
        &self,
        request: WorkerExecutionRestoreRequest,
    ) -> WorkerExecutionSpawnResult {
        self.inner.reconcile_restore(request)
    }

    fn activate_restored_worker(
        &self,
        operation_id: crate::execution::WorkerLifecycleOperationId,
        worker_ref: &WorkerRef,
    ) -> Result<(), String> {
        self.inner
            .activate_restored_worker(operation_id, worker_ref)
    }

    fn dispatch_input(&self, worker_ref: &WorkerRef, input: WorkerInput) -> WorkerExecutionResult {
        self.inner.dispatch_input(worker_ref, input)
    }

    fn dispatch_method(&self, worker_ref: &WorkerRef, method: Method) -> WorkerExecutionResult {
        self.inner.dispatch_method(worker_ref, method)
    }

    fn stop_worker_operation(&self, request: WorkerExecutionStopRequest) -> WorkerExecutionResult {
        self.inner.stop_worker_operation(request)
    }
}

struct ProfileFailureFixture {
    runtime: EmbeddedRuntime,
    backend: Arc<WorkerRuntimeExecutionBackend>,
    worker: crate::catalog::WorkerDetail,
    provider_opens: Arc<AtomicUsize>,
    seed_handle: Option<WorkerHandle>,
    root: tempfile::TempDir,
}

impl ProfileFailureFixture {
    fn new(timeout: Duration) -> Self {
        Self::with_saved_inputs(timeout, None)
    }

    fn with_saved_inputs(timeout: Duration, model: Option<MockClient>) -> Self {
        Self::with_seed(timeout, model, false)
    }

    fn with_pending_head(timeout: Duration) -> Self {
        Self::with_seed(timeout, None, true)
    }

    fn with_seed(timeout: Duration, model: Option<MockClient>, pending_head: bool) -> Self {
        let root = restore_failure_tempdir();
        let repo = root.path().join("g");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(&repo, &["config", "user.name", "Yoi Test"]);
        fs::write(repo.join("README.md"), "clean\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        let store = root.path().join("s");
        let provider_opens = Arc::new(AtomicUsize::new(0));
        let mut factory =
            ProfileRuntimeWorkerFactory::new(root.path()).with_runtime_store_dir(&store);
        let queue_inputs = model.is_some();
        if let Some(model) = model {
            factory = factory.with_spawn_model_client_factory(move || Box::new(model.clone()));
        }
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(factory)
                .unwrap()
                .with_spawn_restore_timeout(timeout)
                .with_working_directory_materializer(RestoreOpenCountingMaterializer {
                    inner: RuntimeGitMaterializer::new(root.path().join("w")),
                    opens: provider_opens.clone(),
                }),
        );
        let execution: Arc<dyn WorkerExecutionBackend> = if pending_head {
            Arc::new(PendingProfileSeedBackend {
                inner: backend.clone(),
            })
        } else {
            backend.clone()
        };
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: store,
                runtime_id: "r".into(),
                display_name: None,
            },
            execution,
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("restore failure recovery");
        request.profile = ProfileSelector::Builtin("default".into());
        if !pending_head {
            request.workdir_attachment_requests = vec![WorkingDirectoryAttachmentRequest {
                alias: WorkdirAttachmentAlias::new("checkout").unwrap(),
                working_directory: working_directory_request(&repo),
            }];
        }
        let worker = runtime.create_worker(request).unwrap();
        let seed_handle = queue_inputs.then(|| {
            backend.workers.lock().unwrap()[&worker.worker_ref]
                .handle
                .clone()
        });
        if let Some(handle) = seed_handle.clone() {
            backend.run_on_adapter_runtime(async move {
                async fn event(events: &mut tokio::sync::broadcast::Receiver<Event>, predicate: impl Fn(&Event) -> bool) {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            let next = events.recv().await.unwrap();
                            assert!(!matches!(next, Event::Error { .. } | Event::SubmissionRejected { .. } | Event::NotificationRejected { .. }), "{next:?}");
                            if predicate(&next) { break; }
                        }
                    }).await.expect("saved-input fixture acknowledgement");
                }
                let mut events = handle.subscribe();
                handle.send(Method::submit_text("revocation-seed", "hold for saved input setup")).await.unwrap();
                event(&mut events, |next| matches!(next, Event::TextDelta { .. })).await;
                handle.send(Method::Pause { command: protocol::WorkerCommandEnvelope::new(handle.shared_state.snapshot().last_command_id + 1) }).await.unwrap();
                event(&mut events, |next| matches!(next, Event::WorkerState { snapshot } if snapshot.catalog_status() == protocol::WorkerStatus::Paused)).await;
                handle.send(Method::submit_text("revocation-saved-submit", "must stay queued")).await.unwrap();
                event(&mut events, |next| matches!(next, Event::SubmissionAccepted { submission_request_id, disposition: protocol::SubmissionDisposition::Queued, .. } if submission_request_id == "revocation-saved-submit")).await;
                handle.send(Method::Notify { notification_request_id: "revocation-saved-notice".into(), message: "must also stay queued".into() }).await.unwrap();
                event(&mut events, |next| matches!(next, Event::NotificationAccepted { notification_request_id } if notification_request_id == "revocation-saved-notice")).await;
                Ok(())
            }).unwrap();
        }
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        // Count restore provider access, not the initial create/stop setup.
        provider_opens.store(0, Ordering::SeqCst);
        Self {
            root,
            provider_opens,
            seed_handle,
            runtime,
            backend,
            worker,
        }
    }

    fn fail(&self, stage: RuntimeRestoreStage) {
        *self.backend.factory.restore_faults.fail_at.lock().unwrap() = Some(stage);
    }

    fn pin_unclaimed_restore_upload(&self) -> (PathBuf, BTreeMap<PathBuf, Vec<u8>>) {
        let aggregate = self
            .backend
            .factory
            .worker_aggregate_dir(&self.worker.worker_ref)
            .unwrap();
        let name =
            ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&self.worker.worker_ref);
        let metadata = WorkerAggregateStore::open_read_only(&aggregate, name.clone()).unwrap();
        let session_id = metadata
            .read_by_name(&name)
            .unwrap()
            .unwrap()
            .active
            .unwrap()
            .session_id;
        let session = WorkerSessionStore::new(aggregate.join("session")).unwrap();
        let orphan = session
            .write_uploaded_file(
                session_id,
                "orphan.txt",
                "text/plain",
                b"restore effect evidence",
                session_store::UploadedFileLimits::default(),
            )
            .unwrap();
        session
            .pin_uploaded_file(session_id, &orphan, "uncommitted-restore-input")
            .unwrap();
        let artifacts = aggregate.join("session/artifacts/paste");
        let pinned = persisted_files(&artifacts);
        assert!(!pinned.is_empty());
        (artifacts, pinned)
    }

    fn assert_retained_restore_effects(
        &self,
        request: &runtime_api::WorkerRestoreRequest,
        pending: &Arc<PendingRuntimeRestore>,
        stage: RuntimeRestoreStage,
    ) {
        let retained = self
            .backend
            .pending_restore(&self.worker.worker_ref)
            .unwrap()
            .expect("resource cleanup must not remove the operation's effect owner");
        assert!(Arc::ptr_eq(pending, &retained));
        {
            let result = retained.result.lock().unwrap();
            let failure = result.as_ref().unwrap().as_ref().err().unwrap();
            assert_eq!(failure.stage, stage);
            assert_eq!(failure.cleanup, RuntimeRestoreCleanup::Confirmed);
            assert!(
                failure.durable_effects_possible,
                "resource cleanup cannot erase prior canonical effects: {failure:?}"
            );
        }
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                self.root
                    .path()
                    .join("s/workers")
                    .join(self.worker.worker_ref.worker_id.to_string())
                    .join("worker.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            record["restore_guard"]["active_request_id"],
            request.request_id
        );
        let operation = serde_json::to_value(pending.request.operation_id).unwrap();
        let owner = &record["restore_guard"]["owners"][&request.request_id];
        assert_eq!(owner["operation_id"], operation);
        assert!(
            owner["receipt"].is_null(),
            "false terminal receipt: {owner:?}"
        );
        assert_eq!(
            record["execution_state"]["state"],
            "reconciliation_required"
        );
        assert_eq!(
            record["execution_state"]["execution"]["operation"],
            "restore"
        );
        assert_eq!(
            record["execution_state"]["execution"]["intent"]["operation_id"],
            operation
        );
        assert!(
            self.backend
                .completed_restore_failures
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(self.backend.workers.lock().unwrap().is_empty());
        assert!(
            self.backend
                .factory
                .failed_restore_resources
                .lock()
                .unwrap()
                .is_empty(),
            "cleanup confirmed resources, not rollback of canonical effects"
        );
        assert!(
            self.backend
                .factory
                .observation_hub
                .get(&self.worker.worker_ref)
                .is_none()
        );
        if !pending.request.deferred_workdir_sessions.is_empty() {
            self.assert_closed_deferred_sessions(pending);
        }
        assert_eq!(self.provider_opens.load(Ordering::SeqCst), 0);
    }

    fn assert_released(&self) {
        assert_eq!(self.provider_opens.load(Ordering::SeqCst), 0);
        assert!(self.backend.pending_restores.lock().unwrap().is_empty());
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
                .workers
                .lock()
                .unwrap()
                .is_empty()
        );
        let table = worker::runtime::worker_allocation::LockFileGuard::open(
            &worker::runtime::worker_allocation::default_allocation_path().unwrap(),
        )
        .unwrap();
        assert!(
            table
                .data()
                .find(&ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
                    &self.worker.worker_ref
                ))
                .is_none()
        );
        let aggregate = self
            .backend
            .factory
            .worker_aggregate_dir(&self.worker.worker_ref)
            .unwrap();
        assert!(fs::read_dir(aggregate.join("runs")).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("restore-")
        }));
        assert_eq!(
            self.runtime
                .worker_detail(&self.worker.worker_ref)
                .unwrap()
                .status,
            crate::catalog::WorkerStatus::Stopped
        );
    }

    fn assert_closed_deferred_sessions(&self, pending: &PendingRuntimeRestore) {
        let sessions = pending.request.deferred_workdir_sessions.clone();
        assert!(!sessions.is_empty());
        self.backend
            .run_on_adapter_runtime(async move {
                for (alias, session) in sessions {
                    let read = session
                        .read(workdir::ReadRequest {
                            path: workdir::WorkdirPath::new("README.md").unwrap(),
                            offset: 0,
                            limit: 10,
                            max_bytes: 1024,
                        })
                        .await;
                    assert!(
                        matches!(read, Err(workdir::WorkdirError::SessionClosed)),
                        "raw deferred attachment {alias} remained usable: {read:?}"
                    );
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(self.provider_opens.load(Ordering::SeqCst), 0);
    }

    fn resume_same_operation(
        &self,
        request: runtime_api::WorkerRestoreRequest,
        operation_id: crate::execution::WorkerLifecycleOperationId,
    ) {
        let result = self
            .runtime
            .restore_worker_operation(&self.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::Accepted,
            "{result:?}"
        );
        assert_eq!(
            self.backend.workers.lock().unwrap()[&self.worker.worker_ref].restore_operation_id,
            Some(operation_id)
        );
        let calls = self
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst);
        let replay = self
            .runtime
            .restore_worker_operation(&self.worker.worker_ref, request)
            .unwrap();
        assert_eq!(
            serde_json::to_value(replay).unwrap(),
            serde_json::to_value(result).unwrap()
        );
        assert_eq!(
            self.backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            calls
        );
        self.runtime
            .stop_worker(&self.worker.worker_ref, None)
            .unwrap();
        assert!(
            self.backend
                .factory
                .observation_hub
                .get(&self.worker.worker_ref)
                .is_none()
        );
        assert!(self.backend.pending_restores.lock().unwrap().is_empty());
        assert!(self.backend.workers.lock().unwrap().is_empty());
        assert!(
            self.backend
                .factory
                .failed_restore_resources
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    fn fresh_restore_and_old_replay(
        &self,
        request: runtime_api::WorkerRestoreRequest,
        failed: crate::runtime::RuntimeWorkerRestoreResult,
    ) {
        let calls = self
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst);
        let replay = self
            .runtime
            .restore_worker_operation(&self.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&failed).unwrap()
        );
        assert_eq!(
            self.backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            calls
        );
        let fresh = self.runtime.test_restore_request(&self.worker.worker_ref);
        assert_ne!(fresh.request_id, request.request_id);
        assert_ne!(
            fresh.expected_observation_token,
            request.expected_observation_token
        );
        let restored = self
            .runtime
            .restore_worker_operation(&self.worker.worker_ref, fresh)
            .unwrap();
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        assert_eq!(
            self.backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            calls + 1
        );
        assert_eq!(self.backend.workers.lock().unwrap().len(), 1);
        let replay = self
            .runtime
            .restore_worker_operation(&self.worker.worker_ref, request)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&failed).unwrap()
        );
        assert_eq!(
            self.backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            calls + 1
        );
        // Restore/cleanup and old-request replay must not provider-open the
        // retained attachment, including after a new execution was accepted.
        assert_eq!(self.provider_opens.load(Ordering::SeqCst), 0);
        // Only tear down after the fresh explicit Restore has already succeeded.
        self.runtime
            .stop_worker(&self.worker.worker_ref, None)
            .unwrap();
        assert_eq!(self.provider_opens.load(Ordering::SeqCst), 0);
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_prior_effects_survive_read_only_reprepare_failure() {
    for read_stage in [RuntimeRestoreStage::Metadata, RuntimeRestoreStage::Reconstruction] {
        let activation_stage = RuntimeRestoreStage::ActivationEffects;
        let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    let _pause = fixture.runtime.test_pause_restore_continuation();
        let (artifacts, pinned) = fixture.pin_unclaimed_restore_upload();
        fixture.fail(activation_stage);
        let request = fixture
            .runtime
            .test_restore_request(&fixture.worker.worker_ref);
        let first = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            first.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "{activation_stage:?}: {first:?}"
        );
        let pending = fixture
            .backend
            .pending_restore(&fixture.worker.worker_ref)
            .unwrap()
            .unwrap();
        assert!(pending.activation_started.load(Ordering::SeqCst));
        fixture.assert_retained_restore_effects(&request, &pending, activation_stage);
        let effected = persisted_files(&artifacts);
        assert_ne!(
            effected, pinned,
            "activation must actually reconcile the orphan pin"
        );

        // The next attempt needs no repair, and fails before any new mutation.
        // Resource cleanup is confirmed, but the first attempt's pin change
        // was not rolled back. Repeated sends must retain the same owner.
        for _ in 0..2 {
            fixture.fail(read_stage);
            let replay = fixture
                .runtime
                .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
                .unwrap();
            assert_eq!(
                replay.state,
                server_api::WorkerRestoreState::ReconciliationRequired,
                "{activation_stage:?} then {read_stage:?}: {replay:?}"
            );
            fixture.assert_retained_restore_effects(&request, &pending, read_stage);
            assert!(pending.activation_started.load(Ordering::SeqCst));
            assert_eq!(persisted_files(&artifacts), effected);
        }
        fixture.resume_same_operation(request, pending.request.operation_id);
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_prior_effects_from_pending_head_repair_survive_read_only_reprepare_failure() {
    let fixture = ProfileFailureFixture::with_pending_head(Duration::from_secs(5));
    let aggregate = fixture
        .backend
        .factory
        .worker_aggregate_dir(&fixture.worker.worker_ref)
        .unwrap();
    let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&fixture.worker.worker_ref);
    let metadata = WorkerAggregateStore::new(&aggregate, &name).unwrap();
    let initial = metadata.read_by_name(&name).unwrap().unwrap();
    let pending_session = initial.active.as_ref().unwrap().session_id;
    assert!(initial.active.as_ref().unwrap().segment_id.is_none());
    {
        let before = WorkerSessionStore::new(aggregate.join("session")).unwrap();
        assert!(before.list_sessions().unwrap().is_empty());
        assert!(before.list_segments(pending_session).unwrap().is_empty());
    }
    // The independent new Worker has never published a head. Its first segment
    // and active pointer must be created by production repair, not fixture
    // rewinding/deletion of a materialized Session.
    fixture.fail(RuntimeRestoreStage::Metadata);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let first = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{first:?}"
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    assert!(!pending.activation_started.load(Ordering::SeqCst));
    assert!(pending.request.deferred_workdir_sessions.is_empty());
    fixture.assert_retained_restore_effects(&request, &pending, RuntimeRestoreStage::Metadata);
    let failure = pending
        .result
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .err()
        .unwrap()
        .clone();
    let repaired = WorkerAggregateStore::open_read_only(&aggregate, &name)
        .unwrap()
        .read_by_name(&name)
        .unwrap()
        .unwrap();
    eprintln!(
        "pending-head repair first={first:?} failure={failure:?} active={:?}",
        repaired.active
    );
    let active = repaired
        .active
        .as_ref()
        .expect("real repair must retain the pending Session's active pointer");
    assert_eq!(active.session_id, pending_session);
    // Store handles opened before the first Manifest retain session_id=None;
    // they cannot observe that another owner has since published this Session.
    // Reopen read-only to require the canonical Manifest as well as its logs.
    let session = WorkerSessionStore::open_read_only(aggregate.join("session"))
        .unwrap_or_else(|error| {
            panic!(
                "repair Manifest reread failed: {error}; first={first:?}; failure={failure:?}; active={active:?}"
            )
        });
    assert_eq!(session.list_sessions().unwrap(), vec![pending_session]);
    let segments = session.list_segments(pending_session).unwrap();
    assert_eq!(
        segments.len(),
        1,
        "real repair must commit exactly one seed; failure={failure:?}; active={active:?}"
    );
    assert_eq!(active.segment_id, Some(segments[0]));
    let entries = session
        .read_all_read_only(pending_session, segments[0])
        .unwrap();
    assert!(matches!(
        entries.first(),
        Some(LogEntry::AnnotatedSegmentStart { .. })
    ));
    assert!(
        session_store::collect_state(&entries)
            .system_prompt
            .is_some()
    );
    assert_ne!(
        repaired, initial,
        "repair must publish the first active head"
    );
    let repaired_session = persisted_files(&aggregate.join("session"));

    for _ in 0..2 {
        fixture.fail(RuntimeRestoreStage::Reconstruction);
        let replay = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            replay.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "repair effect cannot become a rollback on the next read failure: {replay:?}"
        );
        fixture.assert_retained_restore_effects(
            &request,
            &pending,
            RuntimeRestoreStage::Reconstruction,
        );
        let current_metadata = WorkerAggregateStore::open_read_only(&aggregate, &name).unwrap();
        assert_eq!(
            current_metadata.read_by_name(&name).unwrap().unwrap(),
            repaired
        );
        let current_session =
            WorkerSessionStore::open_read_only(aggregate.join("session")).unwrap();
        assert_eq!(
            current_session.list_sessions().unwrap(),
            vec![pending_session]
        );
        assert_eq!(
            current_session.list_segments(pending_session).unwrap(),
            segments
        );
        assert_eq!(
            persisted_files(&aggregate.join("session")),
            repaired_session
        );
    }
    fixture.resume_same_operation(request, pending.request.operation_id);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_prior_effects_survive_rebuilt_candidate_pre_effect_activation_failure() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    let _pause = fixture.runtime.test_pause_restore_continuation();
    let (artifacts, pinned) = fixture.pin_unclaimed_restore_upload();
    fixture.fail(RuntimeRestoreStage::ActivationEffects);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let first = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{first:?}"
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    fixture.assert_retained_restore_effects(&request, &pending, RuntimeRestoreStage::ActivationEffects);
    let effected = persisted_files(&artifacts);
    assert_ne!(effected, pinned);

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
    let runtime = fixture.runtime.clone();
    let worker_ref = fixture.worker.worker_ref.clone();
    let resend = request.clone();
    let retry = std::thread::spawn(move || runtime.restore_worker_operation(&worker_ref, resend));
    let reached = entered.recv_timeout(Duration::from_secs(5));
    // This gate is after real read-only reconstruction. Change only saved
    // metadata, so the rebuilt candidate's first activation revalidation fails
    // before allocation/pin/metadata/delegation mutation. No factory or repair
    // implementation is replaced by a fake.
    let aggregate = fixture
        .backend
        .factory
        .worker_aggregate_dir(&fixture.worker.worker_ref)
        .unwrap();
    let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&fixture.worker.worker_ref);
    let metadata = WorkerAggregateStore::new(&aggregate, &name).unwrap();
    let mut changed = metadata.read_by_name(&name).unwrap().unwrap();
    changed.workspace_root = Some(fixture.root.path().join("changed-after-reconstruction"));
    metadata.write(&changed).unwrap();
    release.notify_one();
    let rebuilt = retry.join().unwrap().unwrap();
    reached.expect("same-operation retry must reach the real reconstructed candidate");
    assert_eq!(
        rebuilt.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{rebuilt:?}"
    );
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        2
    );
    assert!(pending.activation_started.load(Ordering::SeqCst));
    fixture.assert_retained_restore_effects(
        &request,
        &pending,
        RuntimeRestoreStage::Reconstruction,
    );
    assert_eq!(metadata.read_by_name(&name).unwrap().unwrap(), changed);
    assert_eq!(persisted_files(&artifacts), effected);

    fixture.fail(RuntimeRestoreStage::Metadata);
    let replay = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        replay.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "successful reconstruction must not erase the older effect: {replay:?}"
    );
    fixture.assert_retained_restore_effects(&request, &pending, RuntimeRestoreStage::Metadata);
    assert_eq!(metadata.read_by_name(&name).unwrap().unwrap(), changed);
    assert_eq!(persisted_files(&artifacts), effected);
    fixture.resume_same_operation(request, pending.request.operation_id);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_timeout_attachment_revocation_cannot_reactivate_old_candidate() {
    let model = MockClient::sequential(vec![MockResponse::Hang(
        simple_text_events().into_iter().take(2).collect(),
    )]);
    let fixture =
        ProfileFailureFixture::with_saved_inputs(Duration::from_secs(2), Some(model.clone()));
    let body_calls = model.call_count.load(Ordering::SeqCst);
    assert_eq!(body_calls, 1);
    let aggregate = fixture
        .backend
        .factory
        .worker_aggregate_dir(&fixture.worker.worker_ref)
        .unwrap();
    let session_before = persisted_files(&aggregate.join("session"));
    let metadata = WorkerAggregateStore::open_read_only(
        &aggregate,
        ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&fixture.worker.worker_ref),
    )
    .unwrap();
    let saved = metadata
        .read_by_name(&ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
            &fixture.worker.worker_ref,
        ))
        .unwrap()
        .unwrap();
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
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let first = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    assert!(!pending.request.logical_workdir_attachments.is_empty());
    let updated = fixture
        .runtime
        .replace_worker_workdir_attachments(&fixture.worker.worker_ref, Vec::new())
        .unwrap();
    assert!(updated.workdir_attachments.is_empty());
    let authority = || -> serde_json::Value {
        serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path()
                    .join("s/workers")
                    .join(fixture.worker.worker_ref.worker_id.to_string())
                    .join("worker.json"),
            )
            .unwrap(),
        )
        .unwrap()
    };
    assert!(authority().get("logical_workdir_attachments").is_none());
    release.notify_one();
    let replay = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        replay.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "current attachment revocation must fence the old candidate: {replay:?}"
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
    let owner = &record["restore_guard"]["owners"][&request.request_id];
    assert_eq!(
        owner["operation_id"],
        serde_json::to_value(pending.request.operation_id).unwrap()
    );
    assert!(owner["receipt"].is_null());
    assert!(fixture.backend.workers.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(
        &pending,
        &fixture
            .backend
            .pending_restore(&fixture.worker.worker_ref)
            .unwrap()
            .unwrap()
    ));
    assert_eq!(fixture.provider_opens.load(Ordering::SeqCst), 0);
    assert!(!pending.activation_started.load(Ordering::SeqCst));
    assert_eq!(model.call_count.load(Ordering::SeqCst), body_calls);
    assert_eq!(persisted_files(&aggregate.join("session")), session_before);
    assert_eq!(
        metadata.read_by_name(&saved.worker_name).unwrap().unwrap(),
        saved
    );
    let old = fixture.seed_handle.clone().unwrap();
    fixture
        .backend
        .run_on_adapter_runtime(async move {
            assert!(old.protocol_is_closed());
            let old_command = Method::Resume {
                command: protocol::WorkerCommandEnvelope::new(
                    old.shared_state.snapshot().last_command_id + 1,
                ),
            };
            assert!(
                old.send(old_command).await.is_err(),
                "old execution command must not enter the restore candidate"
            );
            Ok(())
        })
        .unwrap();
    assert!(authority().get("logical_workdir_attachments").is_none());
    let input = fixture.runtime.send_input(
        &fixture.worker.worker_ref,
        WorkerInput::user("must-not-enter-revoked-candidate"),
    );
    assert!(
        input.is_err(),
        "pending candidate must not admit fresh input: {input:?}"
    );
    // Stop owns cleanup of the original operation, never a newly rebound alias.
    fixture
        .runtime
        .stop_worker(&fixture.worker.worker_ref, None)
        .unwrap();
    fixture.assert_closed_deferred_sessions(&pending);
    assert_eq!(fixture.provider_opens.load(Ordering::SeqCst), 0);
    assert!(fixture.backend.pending_restores.lock().unwrap().is_empty());
    let old = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request)
        .unwrap();
    assert_ne!(old.state, server_api::WorkerRestoreState::Accepted);
    assert!(fixture.backend.workers.lock().unwrap().is_empty());
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_multistage_failure_cleans_then_fresh_restore_without_stop() {
    // Binding means runtime Worker/attachment-router binding, not eager provider
    // materialization. Provider access stays deferred through every boundary.
    for stage in [
        RuntimeRestoreStage::Reconstruction,
        RuntimeRestoreStage::Binding,
    ] {
        let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
        fixture.fail(stage);
        let request = fixture
            .runtime
            .test_restore_request(&fixture.worker.worker_ref);
        let result = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::RolledBack,
            "{stage:?}: {result:?}"
        );
        let diagnostic = serde_json::to_string(&result).unwrap();
        assert!(
            diagnostic.contains("injected restore boundary failure"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains(&format!("{stage:?}")), "{diagnostic}");
        assert!(!diagnostic.contains(&fixture.root.path().to_string_lossy().to_string()));
        assert!(diagnostic.len() < 1024);
        fixture.assert_released();
        eprintln!(
            "restore-fault stage={stage:?} outcome=RolledBack pending=0 controllers=0 allocations=0"
        );
        fixture.fresh_restore_and_old_replay(request, result);
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_cleanup_failure_retains_owner_and_cause_without_opening_deferred_provider() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    fixture.fail(RuntimeRestoreStage::ActivationEffects);
    fixture
        .backend
        .factory
        .restore_faults
        .cleanup_fail
        .store(true, Ordering::SeqCst);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let failed = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        failed.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    let message = pending
        .result
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .err()
        .unwrap()
        .to_string();
    assert!(
        message.contains("injected restore boundary failure"),
        "{message}"
    );
    assert!(message.contains("Workdir"), "{message}");
    {
        let retained = pending.result.lock().unwrap();
        let failure = retained.as_ref().unwrap().as_ref().err().unwrap();
        assert_eq!(failure.stage, RuntimeRestoreStage::ActivationEffects);
        assert_eq!(failure.resources, RuntimeRestoreResources::WorkerStartup);
        assert_eq!(
            failure.cleanup,
            RuntimeRestoreCleanup::Failed(RuntimeRestoreCleanupFailure::Workdir)
        );
        assert_eq!(
            failure.cleanup_failure,
            Some(RuntimeRestoreCleanupFailure::Workdir)
        );
    }
    let owned = fixture
        .backend
        .factory
        .failed_restore_resources
        .lock()
        .unwrap()
        .get(&fixture.worker.worker_ref)
        .cloned()
        .unwrap();
    assert_eq!(fixture.provider_opens.load(Ordering::SeqCst), 0);
    assert!(pending.request.workdir_attachments.is_empty());
    assert!(!pending.request.deferred_workdir_sessions.is_empty());
    assert_eq!(owned.operation_id, pending.request.operation_id);
    assert!(owned.allocation.is_some());
    // The private Controller has already closed its router before the later
    // factory cleanup fault. Keep the remaining cleanup owner, not live aliases.
    assert!(!owned.sessions.is_empty());
    assert!(
        owned
            .sessions
            .iter()
            .all(|sessions| sessions.aliases().is_empty())
    );
    // Raw deferred handles are closed by factory cleanup after this injected
    // fault is removed; do not open them while cleanup is still pending.
    let repeat = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        repeat.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    assert!(Arc::ptr_eq(
        &pending,
        &fixture
            .backend
            .pending_restore(&fixture.worker.worker_ref)
            .unwrap()
            .unwrap()
    ));
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.provider_opens.load(Ordering::SeqCst), 0);
    fixture
        .backend
        .factory
        .restore_faults
        .cleanup_fail
        .store(false, Ordering::SeqCst);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{result:?}"
    );
    assert!(
        serde_json::to_string(&result)
            .unwrap()
            .contains("injected restore boundary failure")
    );
    assert!(
        owned
            .sessions
            .iter()
            .all(|sessions| sessions.aliases().is_empty())
    );
    fixture.assert_closed_deferred_sessions(&pending);
    eprintln!(
        "restore-fault cleanup=Workdir owner-retained=true retry=same-operation outcome=ReconciliationRequired"
    );
    fixture.resume_same_operation(request, pending.request.operation_id);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_failure_before_router_binding_closes_unattached_raw_sessions() {
    for stage in [
        RuntimeRestoreStage::Metadata,
        RuntimeRestoreStage::Reconstruction,
        RuntimeRestoreStage::Binding,
    ] {
        let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
        fixture.fail(stage);
        fixture
            .backend
            .factory
            .restore_faults
            .cleanup_fail
            .store(true, Ordering::SeqCst);
        let request = fixture
            .runtime
            .test_restore_request(&fixture.worker.worker_ref);
        let failed = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            failed.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        let pending = fixture
            .backend
            .pending_restore(&fixture.worker.worker_ref)
            .unwrap()
            .unwrap();
        assert!(pending.request.workdir_attachments.is_empty());
        assert!(!pending.request.logical_workdir_attachments.is_empty());
        assert_eq!(fixture.provider_opens.load(Ordering::SeqCst), 0);
        fixture
            .backend
            .factory
            .restore_faults
            .cleanup_fail
            .store(false, Ordering::SeqCst);
        let rolled_back = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            rolled_back.state,
            server_api::WorkerRestoreState::RolledBack,
            "{stage:?}: {rolled_back:?}"
        );
        fixture.assert_released();
        fixture.assert_closed_deferred_sessions(&pending);
        fixture.fresh_restore_and_old_replay(request, rolled_back);
        // Retained old cleanup authority must not become the fresh execution's
        // same-alias connection, even after the new execution has stopped.
        fixture.assert_closed_deferred_sessions(&pending);
    }
}

#[tokio::test]
#[serial_test::serial(worker_allocation)]
async fn failed_restore_raw_cleanup_is_operation_bound_and_does_not_follow_reused_alias() {
    let root = restore_failure_tempdir();
    let factory = ProfileRuntimeWorkerFactory::new(root.path()).with_runtime_store_dir(root.path());
    let worker_ref = WorkerRef::new(WorkerId::now_v7());
    let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let raw = |opens: Arc<AtomicUsize>| {
        deferred_workdir::DeferredWorkdirSession::handle(
            "same-workdir",
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            move || {
                opens.fetch_add(1, Ordering::SeqCst);
                Err("offline fixture".into())
            },
        )
    };
    let mut old = direct_restore_request(worker_ref.clone());
    old.deferred_workdir_sessions
        .insert(alias.clone(), raw(opens.clone()));
    let mut current = direct_restore_request(worker_ref.clone());
    current
        .deferred_workdir_sessions
        .insert(alias.clone(), raw(opens.clone()));
    factory.failed_restore_resources.lock().unwrap().insert(
        worker_ref.clone(),
        FailedRestoreResources {
            operation_id: current.operation_id,
            sessions: Vec::new(),
            candidate_workdir_cleanup: None,
            allocation: None,
            feature_tasks: None,
        },
    );
    assert_eq!(
        factory.cleanup_failed_restore(&old).await,
        Err(RuntimeRestoreCleanupFailure::Ownership)
    );
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(
        factory.failed_restore_resources.lock().unwrap()[&worker_ref].operation_id,
        current.operation_id
    );
    factory.cleanup_failed_restore(&current).await.unwrap();
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    // Old raw ownership was not routed into, or closed with, the newer operation.
    let request = workdir::ReadRequest {
        path: workdir::WorkdirPath::new("README.md").unwrap(),
        offset: 0,
        limit: 10,
        max_bytes: 1024,
    };
    assert!(matches!(
        current.deferred_workdir_sessions[&alias]
            .read(request.clone())
            .await,
        Err(workdir::WorkdirError::SessionClosed)
    ));
    assert!(matches!(
        old.deferred_workdir_sessions[&alias]
            .read(request.clone())
            .await,
        Err(workdir::WorkdirError::Unavailable(_))
    ));
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    factory.cleanup_failed_restore(&old).await.unwrap();
    assert!(matches!(
        old.deferred_workdir_sessions[&alias].read(request).await,
        Err(workdir::WorkdirError::SessionClosed)
    ));
    assert_eq!(opens.load(Ordering::SeqCst), 1);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_cleanup_timeout_keeps_running_join_and_late_failure_reconciles_without_execution()
 {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(2));
    fixture.fail(RuntimeRestoreStage::ActivationEffects);
    let release = Arc::new(tokio::sync::Notify::new());
    let (entered_tx, entered) = mpsc::channel();
    *fixture
        .backend
        .factory
        .restore_faults
        .cleanup_gate
        .lock()
        .unwrap() = Some(release.clone());
    *fixture
        .backend
        .factory
        .restore_faults
        .cleanup_entered
        .lock()
        .unwrap() = Some(entered_tx);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    assert!(
        pending
            .result
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_ref()
            .err()
            .unwrap()
            .to_string()
            .contains("injected restore boundary failure")
    );
    assert!(pending.task.try_lock().unwrap().is_some());
    for _ in 0..2 {
        let result = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        assert!(Arc::ptr_eq(
            &pending,
            &fixture
                .backend
                .pending_restore(&fixture.worker.worker_ref)
                .unwrap()
                .unwrap()
        ));
    }
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        1
    );
    release.notify_one();
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::Accepted,
        "{result:?}"
    );
    assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
    eprintln!(
        "restore-fault cleanup=timeout retained-task=true late-completion=joined executions=1"
    );
    fixture.resume_same_operation(request, pending.request.operation_id);
}

#[cfg(unix)]
#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_real_socket_failure_retains_committed_candidate_until_cleanup() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    fixture
        .backend
        .factory
        .restore_faults
        .socket_failure
        .store(true, Ordering::SeqCst);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{result:?}"
    );
    let diagnostic = serde_json::to_string(&result).unwrap();
    assert!(
        diagnostic.contains("publication failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("InvalidInput"), "{diagnostic}");
    // This case deliberately exercises the real Unix socket path-limit error;
    // positive restore cases retain the factory's normal InProcess transport.
    // Short fixture roots must not accidentally turn it into a successful bind.
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    let operation_id = pending.request.operation_id;
    let socket = fixture
        .backend
        .factory
        .worker_restore_run_dir(&fixture.worker.worker_ref, operation_id)
        .unwrap()
        .join("worker.sock");
    assert!(socket.as_os_str().as_encoded_bytes().len() >= 108);
    assert!(!diagnostic.contains(&fixture.root.path().to_string_lossy().to_string()));
    assert!(!socket.exists());
    assert!(fixture.backend.workers.lock().unwrap().is_empty());
    assert!(pending.owns_prepared_candidate());
    assert!(pending.result.lock().unwrap().as_ref().unwrap().as_ref().unwrap().can_retry_publication());
    assert!(fixture.backend.factory.failed_restore_resources.lock().unwrap()[&fixture.worker.worker_ref].allocation.is_some());
    let durable: serde_json::Value = serde_json::from_slice(&fs::read(
        fixture.root.path().join("s/workers").join(fixture.worker.worker_ref.worker_id.to_string()).join("worker.json")
    ).unwrap()).unwrap();
    assert_eq!(durable["execution_state"]["execution"]["intent"]["commit"], "committed");
    assert!(durable["restore_guard"]["owners"][&request.request_id]["receipt"].is_null());
    assert!(
        fixture
            .backend
            .completed_restore_failures
            .lock()
            .unwrap()
            .is_empty()
    );
    fixture
        .backend
        .factory
        .restore_faults
        .socket_failure
        .store(false, Ordering::SeqCst);
    // A real permanent path-limit error cannot be repaired by silently
    // constructing another endpoint/transport. Explicit cleanup settles the
    // retained owner. This case does not assert autonomous terminal handling.
    drop(pending);
    fixture.runtime.stop_worker(&fixture.worker.worker_ref, None).unwrap();
    fixture.assert_released();
    assert!(!socket.parent().unwrap().exists());
    assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_read_only_refusal_replays_and_fresh_observation_can_restore() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    let aggregate = fixture
        .backend
        .factory
        .worker_aggregate_dir(&fixture.worker.worker_ref)
        .unwrap();
    fs::rename(aggregate.join("session"), aggregate.join("saved-session")).unwrap();
    let before = persisted_files(&aggregate);
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::Rejected,
        "{result:?}"
    );
    let mut after = persisted_files(&aggregate);
    let mut before = before;
    // The Runtime rejection receipt is expected to commit; source authority is not.
    before.remove(Path::new("worker.json"));
    after.remove(Path::new("worker.json"));
    assert_eq!(after, before);
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        0
    );
    fs::rename(aggregate.join("saved-session"), aggregate.join("session")).unwrap();
    // Creation's stopped observation entry is weak; querying it evicts it.
    assert!(
        fixture
            .backend
            .factory
            .observation_hub
            .get(&fixture.worker.worker_ref)
            .is_none()
    );
    fixture.assert_released();
    fixture.fresh_restore_and_old_replay(request, result);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_real_artifact_cleanup_failure_retains_same_owner_and_cause() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(2));
    fixture.fail(RuntimeRestoreStage::Binding);
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
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    let run = fixture
        .backend
        .factory
        .worker_restore_run_dir(&fixture.worker.worker_ref, pending.request.operation_id)
        .unwrap();
    fs::write(&run, "invalid disposable run artifact").unwrap();
    release.notify_one();
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired,
        "{result:?}"
    );
    let cause = pending
        .result
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .err()
        .unwrap()
        .clone();
    assert_eq!(cause.stage, RuntimeRestoreStage::Binding);
    assert_eq!(
        cause.cleanup_failure,
        Some(RuntimeRestoreCleanupFailure::Artifacts)
    );
    assert!(
        fixture
            .backend
            .factory
            .failed_restore_resources
            .lock()
            .unwrap()
            .contains_key(&fixture.worker.worker_ref)
    );
    assert!(fixture.backend.workers.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        1
    );
    fs::remove_file(&run).unwrap();
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::RolledBack,
        "{result:?}"
    );
    assert!(result.message.as_ref().unwrap().contains("Artifacts"));
    fixture.assert_released();
    eprintln!(
        "restore-fault cleanup=real-artifact-failure late-factory=true owner=retained original-cause=preserved"
    );
    fixture.fresh_restore_and_old_replay(request, result);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_stop_race_joins_late_factory_failure_without_second_controller() {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(2));
    fixture.fail(RuntimeRestoreStage::Binding);
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
    let request = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, request)
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let pending = fixture
        .backend
        .pending_restore(&fixture.worker.worker_ref)
        .unwrap()
        .unwrap();
    assert!(pending.result.lock().unwrap().is_none());
    let runtime = fixture.runtime.clone();
    let worker_ref = fixture.worker.worker_ref.clone();
    let (started_tx, started) = mpsc::channel();
    let stopping = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        runtime.stop_worker(&worker_ref, None)
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    // The factory still owns the allocation and cannot attest stopped yet.
    assert!(
        !pending
            .task
            .try_lock()
            .map(|task| task.as_ref().is_some_and(|task| task.is_finished()))
            .unwrap_or(false)
    );
    release.notify_one();
    stopping.join().unwrap().unwrap();
    fixture.assert_released();
    assert_eq!(
        fixture
            .backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        1
    );
    let fresh = fixture
        .runtime
        .test_restore_request(&fixture.worker.worker_ref);
    let result = fixture
        .runtime
        .restore_worker_operation(&fixture.worker.worker_ref, fresh)
        .unwrap();
    assert_eq!(
        result.state,
        server_api::WorkerRestoreState::Accepted,
        "{result:?}"
    );
    assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
    fixture
        .runtime
        .stop_worker(&fixture.worker.worker_ref, None)
        .unwrap();
    eprintln!("restore-fault race=Stop late-factory=joined controllers-at-most=1");
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_failure_settlement_write_retry_preserves_completed_cause_without_execution() {
    for fault in [
        crate::fs_store::AtomicWriteFault::BeforeRename,
        crate::fs_store::AtomicWriteFault::AfterRename,
    ] {
        let fixture = ProfileFailureFixture::new(Duration::from_secs(2));
        fixture.fail(RuntimeRestoreStage::Binding);
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
        let request = fixture
            .runtime
            .test_restore_request(&fixture.worker.worker_ref);
        let result = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        let operation_id = fixture
            .backend
            .pending_restore(&fixture.worker.worker_ref)
            .unwrap()
            .unwrap()
            .request
            .operation_id;
        fixture
            .runtime
            .test_runtime_store()
            .fail_next_worker_write(fault);
        release.notify_one();
        let result = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "{result:?}"
        );
        assert_eq!(
            result.reason_code.as_deref(),
            Some("worker_restore_rollback_commit_failed")
        );
        assert!(
            fixture
                .backend
                .pending_restore(&fixture.worker.worker_ref)
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .backend
                .factory
                .failed_restore_resources
                .lock()
                .unwrap()
                .is_empty()
        );
        let saved = fixture
            .backend
            .completed_restore_failure(operation_id, &fixture.worker.worker_ref)
            .unwrap();
        assert!(
            matches!(saved, WorkerExecutionSpawnResult::RolledBack(ref failure)
            if failure.message.as_ref().unwrap().contains("injected restore boundary failure"))
        );
        let result = fixture
            .runtime
            .restore_worker_operation(&fixture.worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::RolledBack,
            "{result:?}"
        );
        assert!(
            result
                .message
                .as_ref()
                .unwrap()
                .contains("injected restore boundary failure")
        );
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        fixture.assert_released();
        eprintln!(
            "restore-fault settlement={fault:?} completed-cause=retained replay-executions=1"
        );
        fixture.fresh_restore_and_old_replay(request, result);
    }
}

// Preflight accepts the real retained Profile source, but reconstruction
// discovers that its committed segment has no system prompt. No fault checkpoint
// supplies this Err.
#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_missing_committed_system_prompt_rolls_back_and_replays_classified_cause() {
    let root = restore_failure_tempdir();
    let runtime_store = root.path().join("s");
    let worker_ref = WorkerRef::new(WorkerId::now_v7());
    let (session_id, segment_id, _) = write_retained_profile_worker(&runtime_store, &worker_ref);
    let worker_name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
    let aggregate_dir = runtime_store
        .join("workers")
        .join(worker_ref.worker_id.to_string());
    let metadata_store = WorkerAggregateStore::new(&aggregate_dir, &worker_name).unwrap();
    let mut metadata = metadata_store.read_by_name(&worker_name).unwrap().unwrap();
    let manifest = WorkerManifest::from_toml(
        r#"
        [worker]
        name = "r"
        [scope]
        allow = []
        [model]
        scheme = "anthropic"
        model_id = "test"
        auth = { kind = "none" }
        [engine]
        instruction = "default"
        max_tokens = 100
        [feature.memory.profile]
        enabled = false
        "#,
    )
    .unwrap();
    let mut manifest = manifest;
    manifest.worker.name = worker_name.clone();
    metadata.resolved_manifest_snapshot =
        Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap());
    metadata_store.write(&metadata).unwrap();
    let session_store = WorkerSessionStore::new(aggregate_dir.join("session")).unwrap();
    let entries_before = session_store.read_all(session_id, segment_id).unwrap();
    assert!(
        session_store::collect_state(&entries_before)
            .system_prompt
            .is_none()
    );

    let backend = WorkerRuntimeExecutionBackend::new(
        ProfileRuntimeWorkerFactory::new(root.path().join("p"))
            .with_runtime_store_dir(&runtime_store),
    )
    .unwrap();
    let request = direct_restore_request(worker_ref.clone());
    backend.preflight_restore(&request).unwrap();
    let first = match backend.restore_worker(request.clone()) {
        WorkerExecutionSpawnResult::RolledBack(result) => result,
        other => panic!("missing committed prompt must finish rollback: {other:?}"),
    };
    let diagnostic = first.message.as_deref().unwrap();
    assert!(
        diagnostic.contains("saved segment system prompt missing"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("Reconstruction"), "{diagnostic}");
    assert!(diagnostic.contains("Confirmed"), "{diagnostic}");
    assert!(diagnostic.len() < 512, "{diagnostic}");
    for private in [
        root.path().to_string_lossy().into_owned(),
        session_id.to_string(),
        segment_id.to_string(),
        "retained response".into(),
    ] {
        assert!(!diagnostic.contains(&private), "{diagnostic}");
    }
    // A completed Err is not evidence of zero resources. Require the owner to
    // attest cleanup, and independently check its allocation/artifact authority.
    {
        let completed = backend.completed_restore_failures.lock().unwrap();
        let (operation_id, failure) = completed.get(&worker_ref).unwrap();
        assert_eq!(*operation_id, request.operation_id);
        assert_eq!(failure.stage, RuntimeRestoreStage::Reconstruction);
        assert_eq!(failure.resources, RuntimeRestoreResources::WorkerStartup);
        assert_eq!(failure.cleanup, RuntimeRestoreCleanup::Confirmed);
        assert_eq!(failure.cleanup_failure, None);
    }
    assert!(backend.pending_restore(&worker_ref).unwrap().is_none());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(
        backend
            .factory
            .failed_restore_resources
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(
        backend
            .factory
            .observation_hub
            .workers
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(
        !backend
            .factory
            .worker_restore_run_dir(&worker_ref, request.operation_id)
            .unwrap()
            .exists()
    );
    {
        let table = worker::runtime::worker_allocation::LockFileGuard::open(
            &worker::runtime::worker_allocation::default_allocation_path().unwrap(),
        )
        .unwrap();
        assert!(table.data().find(&worker_name).is_none());
        assert!(table.data().find("r").is_none());
    }
    for replay in [
        backend.reconcile_restore(request.clone()),
        backend.restore_worker(request),
    ] {
        let WorkerExecutionSpawnResult::RolledBack(result) = replay else {
            panic!("same request must replay confirmed failure: {replay:?}");
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
    }
    assert_eq!(
        backend
            .factory
            .restore_faults
            .executions
            .load(Ordering::SeqCst),
        1
    );
    assert!(backend.pending_restore(&worker_ref).unwrap().is_none());
    assert_eq!(
        metadata_store.read_by_name(&worker_name).unwrap().unwrap(),
        metadata
    );
    assert_eq!(
        serde_json::to_value(session_store.read_all(session_id, segment_id).unwrap()).unwrap(),
        serde_json::to_value(entries_before).unwrap()
    );
}
