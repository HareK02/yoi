// Integration coverage through EmbeddedRuntime -> real Profile factory -> real
// Worker/bootstrap, local Git Workdir provider, and allocation table. Model I/O
// is not invoked; this is a deterministic ordinary suite, not product E2E.

struct ProfileFailureFixture {
    runtime: EmbeddedRuntime,
    backend: Arc<WorkerRuntimeExecutionBackend>,
    worker: crate::catalog::WorkerDetail,
    _repo: tempfile::TempDir,
    root: tempfile::TempDir,
}

impl ProfileFailureFixture {
    fn new(timeout: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let store = root.path().join("private-restore-test/runtime");
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(
                ProfileRuntimeWorkerFactory::new(root.path()).with_runtime_store_dir(&store),
            )
            .unwrap()
            .with_spawn_restore_timeout(timeout)
            .with_working_directory_materializer(RuntimeGitMaterializer::new(
                root.path().join("workdirs"),
            )),
        );
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions {
                root: store,
                runtime_id: "restore-failure-test".into(),
                display_name: None,
            },
            backend.clone(),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = create_request("restore failure recovery");
        request.profile = ProfileSelector::Builtin("default".into());
        request.workdir_attachment_requests = vec![WorkingDirectoryAttachmentRequest {
            alias: WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory: working_directory_request(repo.path()),
        }];
        let worker = runtime.create_worker(request).unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        Self {
            root,
            _repo: repo,
            runtime,
            backend,
            worker,
        }
    }

    fn fail(&self, stage: RuntimeRestoreStage) {
        *self.backend.factory.restore_faults.fail_at.lock().unwrap() = Some(stage);
    }

    fn assert_released(&self) {
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
        // Only tear down after the fresh explicit Restore has already succeeded.
        self.runtime
            .stop_worker(&self.worker.worker_ref, None)
            .unwrap();
    }
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_multistage_failure_cleans_then_fresh_restore_without_stop() {
    for stage in [
        RuntimeRestoreStage::Reconstruction,
        RuntimeRestoreStage::Binding,
        RuntimeRestoreStage::Feature,
        RuntimeRestoreStage::Controller,
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
fn profile_restore_cleanup_failure_retains_owner_and_original_cause_until_same_operation_recovers()
{
    let fixture = ProfileFailureFixture::new(Duration::from_secs(5));
    fixture.fail(RuntimeRestoreStage::Feature);
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
    let owned = fixture
        .backend
        .factory
        .failed_restore_resources
        .lock()
        .unwrap()
        .get(&fixture.worker.worker_ref)
        .cloned()
        .unwrap();
    assert_eq!(owned.operation_id, pending.request.operation_id);
    assert!(owned.allocation.is_some());
    assert!(
        owned
            .sessions
            .iter()
            .any(|sessions| !sessions.aliases().is_empty())
    );
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
        server_api::WorkerRestoreState::RolledBack,
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
    fixture.assert_released();
    eprintln!(
        "restore-fault cleanup=Workdir owner-retained=true retry=same-operation outcome=RolledBack"
    );
    fixture.fresh_restore_and_old_replay(request, result);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_cleanup_timeout_keeps_running_join_and_late_failure_reconciles_without_execution()
 {
    let fixture = ProfileFailureFixture::new(Duration::from_secs(2));
    fixture.fail(RuntimeRestoreStage::Feature);
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
        server_api::WorkerRestoreState::RolledBack,
        "{result:?}"
    );
    fixture.assert_released();
    eprintln!(
        "restore-fault cleanup=timeout retained-task=true late-completion=joined executions=1"
    );
    fixture.fresh_restore_and_old_replay(request, result);
}

#[cfg(unix)]
#[test]
#[serial_test::serial(worker_allocation)]
fn profile_restore_real_socket_start_failure_removes_partial_run_and_allocation() {
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
        server_api::WorkerRestoreState::RolledBack,
        "{result:?}"
    );
    let diagnostic = serde_json::to_string(&result).unwrap();
    assert!(
        diagnostic.contains("controller startup failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("InvalidInput"), "{diagnostic}");
    assert!(!diagnostic.contains("private-restore-test"));
    fixture.assert_released();
    fixture
        .backend
        .factory
        .restore_faults
        .socket_failure
        .store(false, Ordering::SeqCst);
    eprintln!(
        "restore-fault stage=socket-start real-bootstrap=true partial-run=removed allocation=released"
    );
    fixture.fresh_restore_and_old_replay(request, result);
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

struct RacedBindingFactory {
    changed_checkout: PathBuf,
    preflights: AtomicUsize,
}

#[async_trait]
impl RuntimeWorkerFactory for RacedBindingFactory {
    async fn spawn_controller(
        &self,
        _: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, String> {
        panic!("binding-only failure must not create a controller")
    }
    async fn preflight_restore(&self, _: &WorkerExecutionRestoreRequest) -> Result<(), String> {
        // Inject another owner's checkout change AFTER the real materializer's
        // read-only checks, but before its mutable bind. Recovery stays real.
        if self.preflights.fetch_add(1, Ordering::SeqCst) == 0 {
            fs::remove_dir_all(&self.changed_checkout).unwrap();
        }
        Ok(())
    }
    async fn restore_controller(
        &self,
        _: WorkerExecutionRestoreRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        panic!("factory execution must not run after partial binding failed")
    }
}

#[test]
fn partial_restore_binding_failure_keeps_exact_operation_without_factory_cleanup_claim() {
    let root = tempfile::tempdir().unwrap();
    let repo = create_clean_repo();
    let materializer = RuntimeGitMaterializer::new(root.path());
    let first = materializer
        .create(&working_directory_request(repo.path()))
        .unwrap();
    let second = materializer
        .create(&working_directory_request(repo.path()))
        .unwrap();
    let mut request = direct_restore_request(WorkerRef::new(WorkerId::now_v7()));
    request.previous_workdir_attachments = vec![
        WorkingDirectoryAttachmentStatus {
            alias: WorkdirAttachmentAlias::new("first").unwrap(),
            working_directory: first.status(),
        },
        WorkingDirectoryAttachmentStatus {
            alias: WorkdirAttachmentAlias::new("second").unwrap(),
            working_directory: second.status(),
        },
    ];
    let backend = WorkerRuntimeExecutionBackend::new(RacedBindingFactory {
        changed_checkout: second.root().to_path_buf(),
        preflights: AtomicUsize::new(0),
    })
    .unwrap()
    .with_working_directory_materializer(materializer);
    assert!(matches!(
        backend.restore_worker(request.clone()),
        WorkerExecutionSpawnResult::ReconciliationRequired { .. }
    ));
    let pending = backend
        .pending_restore(&request.worker_ref)
        .unwrap()
        .unwrap();
    assert_eq!(pending.request.operation_id, request.operation_id);
    assert_eq!(pending.request.workdir_attachments.len(), 1);
    assert_eq!(
        pending
            .request
            .workdir_attachments
            .values()
            .next()
            .unwrap()
            .root(),
        first.root()
    );
    for _ in 0..2 {
        assert!(matches!(
            backend.reconcile_restore(request.clone()),
            WorkerExecutionSpawnResult::ReconciliationRequired { .. }
        ));
    }
    assert_eq!(backend.factory.preflights.load(Ordering::SeqCst), 1);
    assert!(!backend.stop_worker(&request.worker_ref).is_accepted());
    assert!(Arc::ptr_eq(
        &pending,
        &backend
            .pending_restore(&request.worker_ref)
            .unwrap()
            .unwrap()
    ));
    assert!(backend.workers.lock().unwrap().is_empty());
    eprintln!(
        "restore-fault stage=mutable-binding outcome=ReconciliationRequired partial-bindings=retained factory-executions=0"
    );
    // Fixture teardown drops the retained passive local binding. This is not
    // evidence that credential/policy materialization has been rolled back.
}
