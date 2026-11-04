// Integration of Runtime's durable fence with the real adapter and Controller;
// MockFactory replaces only model/provider/profile construction for a hanging run.
#[test]
#[cfg(feature = "fs-store")]
#[serial_test::serial(worker_allocation)]
fn subject_delete_running_controller_confirms_shutdown_and_preserves_history() {
    let root = tempfile::tempdir().unwrap();
    let client = MockClient::sequential(vec![
        MockResponse::Complete(simple_text_events()),
        MockResponse::Hang(vec![]),
    ]);
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(MockFactory {
            client: client.clone(),
            runtime_base: root.path().join("control"),
            cwd: root.path().to_path_buf(),
            store_dir: root.path().join("sessions"),
            worker_metadata_dir: root.path().join("metadata"),
            observed_cwds: Arc::default(),
            observed_workspace_clients: Arc::default(),
        })
        .unwrap(),
    );
    let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
        crate::fs_store::FsRuntimeStoreOptions::new(root.path().join("catalog"))
            .with_runtime_id("delete-test"),
        backend.clone(),
    )
    .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("running Subject body");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    let worker = runtime
        .create_worker_scoped(&scope, create.clone())
        .unwrap();
    runtime
        .send_input(&worker.worker_ref, WorkerInput::user("settled history"))
        .unwrap();
    let execution = backend.workers.lock().unwrap()[&worker.worker_ref].clone();
    backend
        .run_on_adapter_runtime({
            let handle = execution.handle.clone();
            let client = client.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while client.call_count.load(Ordering::SeqCst) < 1
                        || handle.shared_state.catalog_status() != WorkerStatus::Idle
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .map_err(|error| error.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let before = persisted_files(&root.path().join("sessions"));
    assert!(!before.is_empty());
    runtime
        .send_input(&worker.worker_ref, WorkerInput::user("run until stopped"))
        .unwrap();
    backend
        .run_on_adapter_runtime({
            let handle = execution.handle.clone();
            let client = client.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while client.call_count.load(Ordering::SeqCst) < 2 {
                        tokio::task::yield_now().await;
                    }
                    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Running);
                })
                .await
                .map_err(|error| error.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let request = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "delete-running".into(),
    };
    runtime
        .subject_delete_worker_scoped(&scope, request.clone())
        .unwrap();
    assert!(execution.handle.protocol_is_closed());
    assert!(execution.tasks.tasks.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(backend.pending_restores.lock().unwrap().is_empty());
    assert_eq!(
        runtime.worker_detail(&worker.worker_ref).unwrap().status,
        crate::catalog::WorkerStatus::Stopped
    );
    // Preserve the settled prefix. Ordinary Controller empty-turn rollback remains unchanged.
    let after = persisted_files(&root.path().join("sessions"));
    for (path, bytes) in before {
        let entries = |bytes: &[u8]| {
            bytes
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>()
        };
        assert!(
            entries(&after[&path]).starts_with(&entries(&bytes)),
            "{path:?} BEFORE={:?} AFTER={:?}",
            entries(&bytes),
            entries(&after[&path])
        );
    }
    assert!(runtime.create_worker_scoped(&scope, create).is_err());
    assert!(
        runtime
            .restore_worker(
                &worker.worker_ref,
                runtime.test_restore_request(&worker.worker_ref)
            )
            .is_err()
    );
    runtime
        .subject_delete_worker_scoped(&scope, request)
        .unwrap();
    assert_eq!(client.call_count.load(Ordering::SeqCst), 2);
}

#[test]
#[serial_test::serial(worker_allocation)]
#[cfg(feature = "http-server")]
fn subject_delete_unregistered_profile_controller_retries_unknown_shutdown_without_erasing_session()
{
    let root = tempfile::tempdir().unwrap();
    let (backend, request) = profile_stop_fixture(root.path());
    let execution = backend.workers.lock().unwrap()[&request.worker_ref].clone();
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: request
            .workspace_scope
            .as_ref()
            .unwrap()
            .workspace_id
            .clone(),
        worker_id: request.worker_ref.worker_id,
        create_fingerprint: request.request.create_fingerprint.clone(),
        operation_id: "delete-unregistered".into(),
    };
    let mut bad = deletion.clone();
    bad.create_fingerprint.push_str("-wrong");
    assert!(backend.preflight_subject_delete_worker(&bad).is_err());
    assert!(backend.subject_delete_worker(&bad).is_err());
    let mut bad = deletion.clone();
    bad.workspace_id.push_str("-wrong");
    assert!(backend.subject_delete_worker(&bad).is_err());
    assert!(
        backend.admissions.lock().unwrap()[&request.worker_ref]
            .deletion
            .is_none()
    );
    let preparing = execution.clone();
    backend
        .run_on_adapter_runtime(async move {
            prepare_rejected_shutdown(&preparing, false, None).await;
            Ok(())
        })
        .unwrap();
    let session_root = root
        .path()
        .join("runtime/workers")
        .join(request.worker_ref.worker_id.to_string())
        .join("session");
    let before = persisted_files(&session_root);
    assert!(!before.is_empty());
    assert!(
        backend
            .subject_delete_worker(&deletion)
            .unwrap_err()
            .contains("Conflict")
    );
    assert!(
        backend
            .workers
            .lock()
            .unwrap()
            .contains_key(&request.worker_ref)
    );
    assert!(matches!(
        backend.restore_worker(request.clone()),
        WorkerExecutionSpawnResult::Rejected(_)
    ));
    let stop_operation_id = backend.admissions.lock().unwrap()[&request.worker_ref]
        .deletion_stop_operation_id
        .expect("the original rejected Stop keeps its identity for retry");
    backend.subject_delete_worker(&deletion).unwrap();
    let original = backend
        .termination_receipt(&request.worker_ref)
        .unwrap()
        .unwrap();
    assert_eq!(original.stop_operation_id, stop_operation_id);
    assert_eq!(original.execution_id, execution.tasks.execution_id);
    backend.subject_delete_worker(&deletion).unwrap();
    assert_eq!(
        backend.termination_receipt(&request.worker_ref).unwrap(),
        Some(original)
    );
    assert!(execution.handle.protocol_is_closed());
    assert!(execution.tasks.tasks.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert_eq!(persisted_files(&session_root), before);
    assert!(backend.preflight_restore(&request).is_err());
    assert!(
        backend
            .activate_restored_worker(request.operation_id, &request.worker_ref)
            .is_err()
    );
}

#[test]
#[serial_test::serial(worker_allocation)]
fn subject_delete_pending_restore_fences_late_controller_and_never_restarts_it() {
    let root = tempfile::tempdir().unwrap();
    let restores = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let handles = Arc::new(Mutex::new(Vec::new()));
    let backend = WorkerRuntimeExecutionBackend::new(LateControllerFactory {
        inner: MockFactory {
            client: MockClient::new(vec![]),
            runtime_base: root.path().join("control"),
            cwd: root.path().to_path_buf(),
            store_dir: root.path().join("sessions"),
            worker_metadata_dir: root.path().join("metadata"),
            observed_cwds: Arc::default(),
            observed_workspace_clients: Arc::default(),
        },
        restores: restores.clone(),
        release: release.clone(),
        actual_handles: handles.clone(),
    })
    .unwrap()
    .with_spawn_restore_timeout(Duration::from_millis(1));
    let mut request = direct_restore_request(WorkerRef::new(WorkerId::now_v7()));
    request.workspace_scope = Some(crate::runtime::RuntimeWorkspaceScope::new(
        "ws-test",
        "server-test",
    ));
    request.request.workspace_api = Some(WorkspaceApiRef {
        workspace_id: "ws-test".into(),
        base_url: "https://workspace.invalid".into(),
    });
    assert!(matches!(
        backend.restore_worker(request.clone()),
        WorkerExecutionSpawnResult::ReconciliationRequired { .. }
    ));
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: "ws-test".into(),
        worker_id: request.worker_ref.worker_id,
        create_fingerprint: request.request.create_fingerprint.clone(),
        operation_id: "delete-pending".into(),
    };
    // Unfinished factory ownership is unknown, never shutdown success.
    assert!(
        backend
            .subject_delete_worker(&deletion)
            .unwrap_err()
            .contains("pending")
    );
    assert!(
        backend
            .pending_restores
            .lock()
            .unwrap()
            .contains_key(&request.worker_ref)
    );
    assert!(
        backend
            .activate_restored_worker(request.operation_id, &request.worker_ref)
            .is_err()
    );
    release.notify_one();
    backend.subject_delete_worker(&deletion).unwrap();
    assert!(backend.pending_restores.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert_eq!(restores.load(Ordering::SeqCst), 1);
    assert_eq!(handles.lock().unwrap().len(), 1);
    assert!(handles.lock().unwrap()[0].protocol_is_closed());
    assert!(matches!(
        backend.restore_worker(request),
        WorkerExecutionSpawnResult::Rejected(_)
    ));
}

#[test]
fn subject_delete_without_original_owner_retains_child_identity_but_never_issues_receipt() {
    let root = tempfile::tempdir().unwrap();
    let backend = WorkerRuntimeExecutionBackend::new(MockFactory {
        client: MockClient::new(vec![]),
        runtime_base: root.path().join("must-not-launch"),
        cwd: root.path().to_path_buf(),
        store_dir: root.path().join("sessions"),
        worker_metadata_dir: root.path().join("metadata"),
        observed_cwds: Arc::default(),
        observed_workspace_clients: Arc::default(),
    })
    .unwrap();
    let worker_ref = WorkerRef::new(WorkerId::now_v7());
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: "ws-test".into(),
        worker_id: worker_ref.worker_id,
        create_fingerprint: "not-admitted".into(),
        operation_id: "delete-without-owner".into(),
    };
    assert!(
        backend
            .subject_delete_worker(&deletion)
            .unwrap_err()
            .contains("map absence")
    );
    let child = backend.admissions.lock().unwrap()[&worker_ref]
        .deletion_stop_operation_id
        .unwrap();
    assert!(
        backend
            .subject_delete_worker(&deletion)
            .unwrap_err()
            .contains("map absence")
    );
    assert_eq!(
        backend.admissions.lock().unwrap()[&worker_ref].deletion_stop_operation_id,
        Some(child)
    );
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert!(backend.termination_receipt(&worker_ref).unwrap().is_none());
    assert!(!root.path().join("must-not-launch").exists());
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_cancelled_release_history_consumes_prior_canonical_stop() {
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(MockFactory {
            client: MockClient::new(vec![]),
            runtime_base: PathBuf::from(temp.path().file_name().unwrap()).join("c"),
            cwd: root.clone(),
            store_dir: root.join("sessions"),
            worker_metadata_dir: root.join("metadata"),
            observed_cwds: Arc::default(),
            observed_workspace_clients: Arc::default(),
        })
        .unwrap(),
    );
    let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
        crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
            .with_runtime_id("delete-prior-stop"),
        backend.clone(),
    )
    .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("prior Stop Subject deletion");
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    let worker = runtime
        .create_worker_scoped(&scope, create.clone())
        .unwrap();
    let mut release = runtime_api::SubjectReleaseRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: worker.worker_ref.worker_id,
        binding_id: "cancelled-binding".into(),
        operation_id: "release-before-stop".into(),
        action: runtime_api::SubjectReleaseAction::Reserve,
    };
    assert_eq!(
        runtime
            .subject_release_scoped(&scope, release.clone())
            .unwrap()
            .state,
        runtime_api::SubjectReleaseState::Reserved
    );
    assert!(
        runtime
            .stop_worker_scoped(&scope, &worker.worker_ref, None)
            .is_err()
    );
    release.action = runtime_api::SubjectReleaseAction::Cancel;
    assert_eq!(
        runtime
            .subject_release_scoped(&scope, release.clone())
            .unwrap()
            .state,
        runtime_api::SubjectReleaseState::Cancelled
    );
    {
        let reservations = backend.subject_reservations.lock().unwrap();
        let saved = &reservations[&(worker.worker_ref.clone(), release.operation_id.clone())];
        assert_eq!(saved.state, runtime_api::SubjectReleaseState::Cancelled);
        assert!(saved.guard.is_none());
    }
    runtime
        .stop_worker_scoped(&scope, &worker.worker_ref, None)
        .unwrap();
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint,
        operation_id: "delete-prior-stop".into(),
    };
    // Retained non-Cancelled state still blocks canonical consumption even if
    // its local guard is unavailable; only the terminal no-guard row is harmless.
    let key = (worker.worker_ref.clone(), release.operation_id);
    backend
        .subject_reservations
        .lock()
        .unwrap()
        .get_mut(&key)
        .unwrap()
        .state = runtime_api::SubjectReleaseState::Reserved;
    assert_eq!(
        backend
            .reconcile_termination_receipt(&worker.worker_ref, &original)
            .unwrap(),
        None
    );
    assert!(
        backend
            .subject_delete_worker_stop(
                &deletion,
                WorkerExecutionStopRequest {
                    worker_ref: worker.worker_ref.clone(),
                    operation_id: original.stop_operation_id,
                },
                Some(&original),
            )
            .is_err()
    );
    backend
        .subject_reservations
        .lock()
        .unwrap()
        .get_mut(&key)
        .unwrap()
        .state = runtime_api::SubjectReleaseState::Cancelled;
    runtime
        .subject_delete_worker_scoped(&scope, deletion.clone())
        .unwrap();
    backend.subject_delete_worker(&deletion).unwrap();
    assert_eq!(
        backend.termination_receipt(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        backend.admissions.lock().unwrap()[&worker.worker_ref].deletion_stop_operation_id,
        Some(original.stop_operation_id)
    );
    assert_eq!(
        runtime.execution_termination(&worker.worker_ref).unwrap(),
        Some(original)
    );
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(backend.pending_spawns.lock().unwrap().is_empty());
    assert!(backend.pending_restores.lock().unwrap().is_empty());
}

// Only the extra factory-owned cleanup is faulted. Controller Stop, scope join,
// Runtime persistence, cold loading, and reconciliation remain production code.
#[cfg(feature = "fs-store")]
struct SubjectCleanupFaultFactory {
    inner: MockFactory,
    spawns: Arc<AtomicUsize>,
    cleanup_allowed: Arc<AtomicBool>,
    cleanup_calls: Arc<AtomicUsize>,
}

#[cfg(feature = "fs-store")]
#[async_trait]
impl RuntimeWorkerFactory for SubjectCleanupFaultFactory {
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        self.inner.spawn_controller(request).await
    }

    async fn prepare_restore(
        &self,
        _: WorkerExecutionRestoreRequest,
    ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        panic!("saved Stop recovery must not prepare a Controller")
    }

    async fn reconcile_stopped_worker(&self, _: &WorkerRef) -> Result<(), String> {
        self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        if self.cleanup_allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("injected extra factory cleanup failure after actual termination".into())
        }
    }
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_pending_actual_termination_survives_cold_cleanup_retry() {
    use crate::fs_store::{
        PersistedWorkerExecutionState as Execution, PersistedWorkerLifecycleOperation as Operation,
    };
    // Both direct Subject retry of pending B and ordinary startup reconciliation
    // must consume the same saved whole receipt, without a warm successful Stop.
    for startup_recovers in [false, true] {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let cleanup_allowed = Arc::new(AtomicBool::new(false));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let observed_cwds = Arc::new(Mutex::new(Vec::new()));
        let new_backend = || {
            Arc::new(
                WorkerRuntimeExecutionBackend::new(SubjectCleanupFaultFactory {
                    inner: MockFactory {
                        client: MockClient::new(vec![]),
                        runtime_base: PathBuf::from(temp.path().file_name().unwrap()).join("c"),
                        cwd: root.clone(),
                        store_dir: root.join("sessions"),
                        worker_metadata_dir: root.join("metadata"),
                        observed_cwds: observed_cwds.clone(),
                        observed_workspace_clients: Arc::default(),
                    },
                    spawns: spawns.clone(),
                    cleanup_allowed: cleanup_allowed.clone(),
                    cleanup_calls: cleanup_calls.clone(),
                })
                .unwrap(),
            )
        };
        let backend = new_backend();
        let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
            .with_runtime_id("subject-pending-cleanup");
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
        let mut create = create_request("Subject Stop pending extra cleanup");
        create.workspace_api = Some(WorkspaceApiRef {
            workspace_id: scope.workspace_id.clone(),
            base_url: "https://workspace.invalid".into(),
        });
        create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: scope.workspace_id.clone(),
            language: "English".into(),
        });
        let worker = runtime
            .create_worker_scoped(&scope, create.clone())
            .unwrap();
        let execution = backend.workers.lock().unwrap()[&worker.worker_ref].clone();
        let deletion = runtime_api::SubjectDeleteWorkerRequest {
            workspace_id: scope.workspace_id.clone(),
            worker_id: create.worker_id,
            create_fingerprint: create.create_fingerprint.clone(),
            operation_id: "subject-pending-cleanup".into(),
        };
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, deletion.clone())
                .is_err()
        );
        let original = runtime
            .execution_termination(&worker.worker_ref)
            .unwrap()
            .unwrap();
        assert_eq!(original.execution_id, execution.tasks.execution_id);
        assert!(original.confirms_scope_shutdown(&worker.worker_ref, original.stop_operation_id));
        assert!(execution.handle.protocol_is_closed());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        assert!(backend.workers.lock().unwrap().is_empty());
        let before = runtime
            .test_runtime_store()
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert!(
            matches!(before.execution_state, Execution::ReconciliationRequired(Operation::Stop(ref pending))
            if pending.operation_id == original.stop_operation_id)
        );
        // Read the saved aggregate, not the adapter's still-warm receipt cache.
        assert_eq!(
            serde_json::to_value(&before.restore_guard).unwrap()["execution_termination"],
            serde_json::to_value(&original).unwrap()
        );
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        drop(execution);
        drop(runtime);
        drop(backend);

        cleanup_allowed.store(startup_recovers, Ordering::SeqCst);
        let cold = new_backend();
        assert!(cold.completed_stops.lock().unwrap().is_empty());
        let reopened =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options, cold.clone()).unwrap();
        let loaded = reopened
            .test_runtime_store()
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        if !startup_recovers {
            assert!(
                matches!(loaded.execution_state, Execution::ReconciliationRequired(Operation::Stop(ref pending))
                if pending.operation_id == original.stop_operation_id)
            );
            assert_eq!(
                serde_json::to_value(&loaded.restore_guard).unwrap()["execution_termination"],
                serde_json::to_value(&original).unwrap()
            );
            cleanup_allowed.store(true, Ordering::SeqCst);
        } else {
            assert!(!matches!(
                loaded.execution_state,
                Execution::ReconciliationRequired(_)
            ));
            assert_eq!(loaded.status, crate::catalog::WorkerStatus::Stopped);
        }
        let cleanup_before = cleanup_calls.load(Ordering::SeqCst);
        reopened
            .subject_delete_worker_scoped(&scope, deletion)
            .unwrap();
        assert!(cleanup_calls.load(Ordering::SeqCst) > cleanup_before);
        let settled = reopened
            .test_runtime_store()
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert!(!matches!(
            settled.execution_state,
            Execution::ReconciliationRequired(_)
        ));
        assert_eq!(settled.status, crate::catalog::WorkerStatus::Stopped);
        assert_eq!(
            serde_json::to_value(&settled.restore_guard).unwrap()["execution_termination"],
            serde_json::to_value(&original).unwrap()
        );
        assert_eq!(
            reopened.execution_termination(&worker.worker_ref).unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            cold.admissions.lock().unwrap()[&worker.worker_ref].deletion_stop_operation_id,
            Some(original.stop_operation_id)
        );
        assert!(cold.completed_stops.lock().unwrap().is_empty());
        assert!(cold.workers.lock().unwrap().is_empty());
        assert!(cold.pending_spawns.lock().unwrap().is_empty());
        assert!(cold.pending_restores.lock().unwrap().is_empty());
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            1,
            "cold retry must not spawn a Controller"
        );
        assert!(reopened.create_worker_scoped(&scope, create).is_err());
    }
}

// This wrapper injects only the canonical write acknowledgement boundary after
// the real owner has stopped. It never supplies a termination receipt itself.
#[cfg(feature = "fs-store")]
struct SubjectStopSaveFaultBackend {
    inner: Arc<WorkerRuntimeExecutionBackend<MockFactory>>,
    store: Mutex<Option<crate::fs_store::FsRuntimeStore>>,
    fault: Mutex<Option<crate::fs_store::AtomicWriteFault>>,
}
#[cfg(feature = "fs-store")]
impl WorkerExecutionBackend for SubjectStopSaveFaultBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }
    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        self.inner.spawn_worker(request)
    }
    fn dispatch_input(&self, worker: &WorkerRef, input: WorkerInput) -> WorkerExecutionResult {
        self.inner.dispatch_input(worker, input)
    }
    fn preflight_subject_delete_worker(
        &self,
        request: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<(), String> {
        self.inner.preflight_subject_delete_worker(request)
    }
    fn fence_subject_delete_worker(
        &self,
        request: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<(), String> {
        self.inner.fence_subject_delete_worker(request)
    }
    fn subject_delete_worker_stop(
        &self,
        request: &runtime_api::SubjectDeleteWorkerRequest,
        stop: WorkerExecutionStopRequest,
        canonical: Option<&crate::execution::WorkerExecutionTermination>,
    ) -> Result<WorkerExecutionResult, String> {
        let result = self
            .inner
            .subject_delete_worker_stop(request, stop, canonical)?;
        if result.is_accepted()
            && canonical.is_none()
            && let Some(fault) = self.fault.lock().unwrap().take()
        {
            self.store
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .fail_next_worker_write(fault);
        }
        Ok(result)
    }
    fn reconcile_termination_receipt(
        &self,
        worker: &WorkerRef,
        saved: &crate::execution::WorkerExecutionTermination,
    ) -> Result<Option<crate::execution::WorkerExecutionTermination>, String> {
        self.inner.reconcile_termination_receipt(worker, saved)
    }
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_first_stop_saves_original_child_and_reconciles_interrupted_commit() {
    for fault in [
        None,
        Some(crate::fs_store::AtomicWriteFault::BeforeRename),
        Some(crate::fs_store::AtomicWriteFault::AfterRename),
    ] {
        let temp = tempfile::tempdir_in(".").unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let new_backend = || {
            Arc::new(
                WorkerRuntimeExecutionBackend::new(MockFactory {
                    client: MockClient::new(vec![]),
                    runtime_base: PathBuf::from(temp.path().file_name().unwrap()).join("c"),
                    cwd: root.clone(),
                    store_dir: root.join("sessions"),
                    worker_metadata_dir: root.join("metadata"),
                    observed_cwds: Arc::default(),
                    observed_workspace_clients: Arc::default(),
                })
                .unwrap(),
            )
        };
        let inner = new_backend();
        let backend = Arc::new(SubjectStopSaveFaultBackend {
            inner: inner.clone(),
            store: Mutex::new(None),
            fault: Mutex::new(fault),
        });
        let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
            .with_runtime_id("subject-first-stop");
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        *backend.store.lock().unwrap() = Some(runtime.test_runtime_store());
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
        let mut create = create_request("Subject first Stop original proof");
        create.workspace_api = Some(WorkspaceApiRef {
            workspace_id: scope.workspace_id.clone(),
            base_url: "https://workspace.invalid".into(),
        });
        create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: scope.workspace_id.clone(),
            language: "English".into(),
        });
        let worker = runtime
            .create_worker_scoped(&scope, create.clone())
            .unwrap();
        let execution = inner.workers.lock().unwrap()[&worker.worker_ref].clone();
        let request = runtime_api::SubjectDeleteWorkerRequest {
            workspace_id: scope.workspace_id.clone(),
            worker_id: create.worker_id,
            create_fingerprint: create.create_fingerprint.clone(),
            operation_id: "subject-first-stop".into(),
        };
        let first = runtime.subject_delete_worker_scoped(&scope, request.clone());
        if fault.is_some() {
            assert!(first.is_err());
        } else {
            first.unwrap();
        }
        let original = inner
            .termination_receipt(&worker.worker_ref)
            .unwrap()
            .unwrap();
        assert_eq!(original.execution_id, execution.tasks.execution_id);
        assert!(original.confirms_scope_shutdown(&worker.worker_ref, original.stop_operation_id));
        assert!(execution.handle.protocol_is_closed());
        assert!(execution.tasks.tasks.lock().unwrap().is_empty());
        assert_eq!(
            inner.admissions.lock().unwrap()[&worker.worker_ref].deletion_stop_operation_id,
            Some(original.stop_operation_id)
        );
        if matches!(fault, Some(crate::fs_store::AtomicWriteFault::AfterRename)) {
            runtime
                .test_runtime_store()
                .fail_next_worker_reconcile_sync();
            assert!(
                runtime
                    .subject_delete_worker_scoped(&scope, request.clone())
                    .is_err()
            );
        }
        // BeforeRename retains the same pending operation; AfterRename must
        // reread/sync the committed original, never begin/clear/retag it.
        runtime
            .subject_delete_worker_scoped(&scope, request.clone())
            .unwrap();
        assert_eq!(
            runtime.execution_termination(&worker.worker_ref).unwrap(),
            Some(original.clone())
        );
        drop(runtime);
        drop(backend);
        drop(inner);
        let cold = new_backend();
        let reopened =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options, cold.clone()).unwrap();
        assert!(cold.completed_stops.lock().unwrap().is_empty());
        reopened
            .subject_delete_worker_scoped(&scope, request.clone())
            .unwrap();
        assert_eq!(
            reopened.execution_termination(&worker.worker_ref).unwrap(),
            Some(original.clone())
        );
        assert!(
            cold.completed_stops.lock().unwrap().is_empty(),
            "canonical read must not mint another Stop"
        );
        assert_eq!(
            cold.admissions.lock().unwrap()[&worker.worker_ref].deletion_stop_operation_id,
            Some(original.stop_operation_id)
        );
        assert!(cold.workers.lock().unwrap().is_empty());
        assert!(cold.factory.observed_cwds.lock().unwrap().is_empty());
        assert!(reopened.create_worker_scoped(&scope, create).is_err());
    }
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_cold_prior_stop_rejects_replacement_and_mismatched_originals() {
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let new_backend = || {
        Arc::new(
            WorkerRuntimeExecutionBackend::new(MockFactory {
                client: MockClient::new(vec![]),
                runtime_base: PathBuf::from(temp.path().file_name().unwrap()).join("c"),
                cwd: root.clone(),
                store_dir: root.join("sessions"),
                worker_metadata_dir: root.join("metadata"),
                observed_cwds: Arc::default(),
                observed_workspace_clients: Arc::default(),
            })
            .unwrap(),
        )
    };
    let backend = new_backend();
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
        .with_runtime_id("subject-cold-prior");
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("prior real Stop cold Subject");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    let worker = runtime
        .create_worker_scoped(&scope, create.clone())
        .unwrap();
    runtime
        .stop_worker_scoped(&scope, &worker.worker_ref, None)
        .unwrap();
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    drop(runtime);
    drop(backend);
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "delete-cold-prior".into(),
    };
    for invalid in [
        "worker",
        "operation",
        "execution",
        "feature",
        "whole-receipt",
        "candidate",
        "child",
        "valid",
    ] {
        let backend = new_backend();
        if invalid == "candidate" {
            let mut request = direct_restore_request(worker.worker_ref.clone());
            request.workspace_scope = Some(scope.clone());
            request.request = create.clone();
            assert!(matches!(
                backend.restore_worker(request),
                WorkerExecutionSpawnResult::Connected { .. }
            ));
        } else if invalid == "child" {
            backend.admissions.lock().unwrap().insert(
                worker.worker_ref.clone(),
                WorkerAdmission {
                    workspace_id: Some(scope.workspace_id.clone()),
                    create_fingerprint: create.create_fingerprint.clone(),
                    deletion: Some(deletion.clone()),
                    deletion_stop_operation_id: Some(
                        crate::execution::WorkerLifecycleOperationId::new(),
                    ),
                },
            );
        } else if invalid != "valid" {
            let mut wrong = original.clone();
            match invalid {
                "worker" => wrong.worker_ref = WorkerRef::new(WorkerId::now_v7()),
                "operation" => {
                    wrong.stop_operation_id = crate::execution::WorkerLifecycleOperationId::new()
                }
                "execution" => wrong.execution_id = uuid::Uuid::now_v7(),
                "feature" => {
                    wrong
                        .feature_failures
                        .push(crate::execution::WorkerFeatureShutdownFailure {
                            category: worker::hook::HookErrorCategory::Timeout,
                            diagnostic: "not joined".into(),
                        })
                }
                "whole-receipt" => wrong
                    .submissions
                    .push(crate::execution::WorkerSubmissionAck {
                        submission_request_id: "other".into(),
                        submission_id: "other".into(),
                        disposition: protocol::SubmissionDisposition::Started,
                    }),
                _ => unreachable!(),
            }
            backend
                .completed_stops
                .lock()
                .unwrap()
                .insert(worker.worker_ref.clone(), wrong);
        }
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        let result = runtime.subject_delete_worker_scoped(&scope, deletion.clone());
        if invalid == "valid" {
            result.unwrap();
            assert_eq!(
                runtime.execution_termination(&worker.worker_ref).unwrap(),
                Some(original.clone())
            );
            assert!(backend.completed_stops.lock().unwrap().is_empty());
        } else {
            assert!(result.is_err(), "{invalid}");
            let record = runtime
                .test_runtime_store()
                .read_worker_record(&worker.worker_ref)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&record.restore_guard).unwrap()["execution_termination"],
                serde_json::to_value(&original).unwrap()
            );
            if invalid == "candidate" {
                assert!(
                    backend
                        .pending_restore(&worker.worker_ref)
                        .unwrap()
                        .is_some()
                );
                let stopped = backend.stop_worker_operation(WorkerExecutionStopRequest {
                    worker_ref: worker.worker_ref.clone(),
                    operation_id: crate::execution::WorkerLifecycleOperationId::new(),
                });
                assert!(stopped.is_accepted(), "{stopped:?}");
            }
        }
    }
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_stopped_catalog_without_original_never_certifies_empty_backend() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(MockFactory {
            client: MockClient::new(vec![]),
            runtime_base: root.path().join("no-launch"),
            cwd: root.path().to_path_buf(),
            store_dir: root.path().join("sessions"),
            worker_metadata_dir: root.path().join("metadata"),
            observed_cwds: Arc::default(),
            observed_workspace_clients: Arc::default(),
        })
        .unwrap(),
    );
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.path().join("catalog"))
        .with_runtime_id("missing-original");
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("never started");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    let worker_ref = WorkerRef::new(create.worker_id);
    runtime
        .test_runtime_store()
        .write_worker_record(&crate::fs_store::PersistedWorkerRecord {
            restore_guard: crate::runtime::RestoreGuard::default(),
            worker_ref: worker_ref.clone(),
            worker_id: create.worker_id,
            profile: create.profile.clone(),
            display_name: None,
            profile_source: create.profile_source.reference(),
            config_bundle: create.config_bundle.clone(),
            created_at_ms: None,
            status: crate::catalog::WorkerStatus::Stopped,
            last_finished_submission_request_id: None,
            execution_state: crate::fs_store::PersistedWorkerExecutionState::Available(
                crate::fs_store::PersistedWorkerExecution {
                    request: create.clone(),
                    binding: None,
                    restore_intent: crate::catalog::WorkerRestoreIntent::Explicit,
                },
            ),
            workspace_id: Some(scope.workspace_id.clone()),
            workdir_attachments: Vec::new(),
            logical_workdir_attachments: Vec::new(),
        })
        .unwrap();
    drop(runtime);
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint,
        operation_id: "missing-original".into(),
    };
    assert!(
        runtime
            .subject_delete_worker_scoped(&scope, deletion.clone())
            .is_err()
    );
    assert!(
        runtime
            .subject_delete_worker_scoped(&scope, deletion)
            .is_err()
    );
    assert!(
        runtime
            .execution_termination(&worker_ref)
            .unwrap()
            .is_none()
    );
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert!(runtime.worker_detail(&worker_ref).is_ok());
    assert!(!root.path().join("no-launch").exists());
}

#[test]
#[cfg(feature = "fs-store")]
fn subject_delete_confirmed_removal_retries_warm_and_cold_without_new_stop() {
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let spawns = Arc::new(AtomicUsize::new(0));
    let cleanup_allowed = Arc::new(AtomicBool::new(true));
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let new_backend = || {
        Arc::new(
            WorkerRuntimeExecutionBackend::new(SubjectCleanupFaultFactory {
                inner: MockFactory {
                    client: MockClient::new(vec![]),
                    runtime_base: PathBuf::from(temp.path().file_name().unwrap()).join("c"),
                    cwd: root.clone(),
                    store_dir: root.join("sessions"),
                    worker_metadata_dir: root.join("metadata"),
                    observed_cwds: Arc::default(),
                    observed_workspace_clients: Arc::default(),
                },
                spawns: spawns.clone(),
                cleanup_allowed: cleanup_allowed.clone(),
                cleanup_calls: cleanup_calls.clone(),
            })
            .unwrap(),
        )
    };
    let backend = new_backend();
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
        .with_runtime_id("subject-after-removal");
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("Subject confirmed removal retry");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    let worker = runtime
        .create_worker_scoped(&scope, create.clone())
        .unwrap();
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "same-confirmed-delete".into(),
    };
    let confirmed = runtime
        .subject_delete_worker_scoped(&scope, deletion.clone())
        .unwrap();
    assert!(!confirmed.originals_absent);
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    assert!(original.confirms_scope_shutdown(&worker.worker_ref, original.stop_operation_id));
    runtime
        .delete_worker_scoped(&scope, &worker.worker_ref)
        .unwrap();
    assert!(runtime.worker_detail(&worker.worker_ref).is_err());
    let absent = runtime
        .subject_delete_worker_scoped(&scope, deletion.clone())
        .unwrap();
    let mut expected = confirmed;
    expected.originals_absent = true;
    assert_eq!(absent, expected);
    assert_eq!(
        backend.completed_stops.lock().unwrap()[&worker.worker_ref],
        original
    );
    drop(runtime);
    drop(backend);
    let cold = new_backend();
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), cold.clone())
            .unwrap();
    cleanup_allowed.store(false, Ordering::SeqCst);
    assert!(
        reopened
            .subject_delete_worker_scoped(&scope, deletion.clone())
            .is_err(),
        "saved response cannot bypass current factory cleanup"
    );
    cleanup_allowed.store(true, Ordering::SeqCst);
    let material = options
        .root
        .join("workers")
        .join(create.worker_id.to_string());
    fs::create_dir_all(&material).unwrap();
    fs::write(material.join("residual"), "unconfirmed material").unwrap();
    assert!(
        reopened
            .subject_delete_worker_scoped(&scope, deletion.clone())
            .is_err(),
        "catalog absence cannot hide residual original files"
    );
    fs::remove_file(material.join("residual")).unwrap();
    fs::remove_dir(&material).unwrap();
    assert_eq!(
        reopened
            .subject_delete_worker_scoped(&scope, deletion)
            .unwrap(),
        expected
    );
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    assert!(
        cold.completed_stops.lock().unwrap().is_empty(),
        "absence confirmation never mints another termination"
    );
    assert!(cold.workers.lock().unwrap().is_empty());
    assert!(cold.pending_spawns.lock().unwrap().is_empty());
    assert!(cold.pending_restores.lock().unwrap().is_empty());
    assert!(reopened.create_worker_scoped(&scope, create).is_err());
}
