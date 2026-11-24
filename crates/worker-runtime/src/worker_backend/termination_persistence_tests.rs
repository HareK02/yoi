#[cfg(feature = "fs-store")]
#[test]
fn stop_termination_survives_reopen_and_removal_rejects_only_current_replacement_resources() {
    // Keep socket names relative and short. The focused invocation runs from an
    // isolated scratch cwd; data paths are still explicit absolute fixture paths.
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let socket_base = PathBuf::from(temp.path().file_name().unwrap()).join("c");
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("runtime"))
        .with_runtime_id("original-scope-receipt");
    let new_backend = || {
        Arc::new(
            WorkerRuntimeExecutionBackend::new(MockFactory {
                client: MockClient::sequential(vec![]),
                runtime_base: socket_base.clone(),
                cwd: root.clone(),
                store_dir: root.join("sessions"),
                worker_metadata_dir: root.join("metadata"),
                observed_cwds: Arc::new(Mutex::new(Vec::new())),
                observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
            })
            .unwrap(),
        )
    };
    let backend = new_backend();
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime
        .create_worker(create_request("durable original receipt"))
        .unwrap();
    assert!(
        runtime
            .execution_termination(&worker.worker_ref)
            .unwrap()
            .is_none()
    );
    let mut input = WorkerInput::user("original submission");
    input.submission_request_id = Some("original-request".into());
    let acceptance = runtime
        .send_input(&worker.worker_ref, input)
        .unwrap()
        .submission
        .unwrap();
    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    assert_eq!(
        original.scope,
        crate::execution::WorkerExecutionTerminationScope::Controller
    );
    assert!(original.shutdown_command_id.is_some());
    assert!(original.feature_failures.is_empty());
    assert!(original.submissions.contains(&acceptance));
    assert_eq!(
        backend.termination_receipt(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    assert!(backend.workers.lock().unwrap().is_empty());
    let worker_path = options
        .root
        .join("workers")
        .join(worker.worker_id.to_string())
        .join("worker.json");
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&worker_path).unwrap()).unwrap();
    assert_eq!(
        document["restore_guard"]["execution_termination"],
        serde_json::to_value(&original).unwrap()
    );
    drop(runtime);
    drop(backend);

    let backend = new_backend();
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert!(backend.workers.lock().unwrap().is_empty());
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );

    // Exercise the backend ownership check even if a replacement was installed
    // outside Runtime's lifecycle coordinator. Empty old maps cannot bless it.
    let restore = direct_restore_request(worker.worker_ref.clone());
    assert!(matches!(
        backend.restore_worker(restore.clone()),
        WorkerExecutionSpawnResult::Connected { .. }
    ));
    assert!(
        reopened
            .execution_termination(&worker.worker_ref)
            .unwrap()
            .is_none()
    );
    assert!(reopened.delete_worker(&worker.worker_ref).is_err());
    assert!(worker_path.exists());
    assert!(
        backend
            .pending_restore(&worker.worker_ref)
            .unwrap()
            .is_some(),
        "deletion must not stop/discard a replacement candidate with the old receipt"
    );
    backend
        .activate_restored_worker(restore.operation_id, &worker.worker_ref)
        .unwrap();
    assert!(
        backend
            .pending_restore(&worker.worker_ref)
            .unwrap()
            .is_none()
    );
    assert!(
        backend
            .workers
            .lock()
            .unwrap()
            .contains_key(&worker.worker_ref)
    );
    assert!(reopened.delete_worker(&worker.worker_ref).is_err());
    assert!(worker_path.exists());
    assert!(
        backend
            .workers
            .lock()
            .unwrap()
            .contains_key(&worker.worker_ref)
    );
    let replacement_stop = backend.stop_worker_operation(WorkerExecutionStopRequest {
        worker_ref: worker.worker_ref.clone(),
        operation_id: crate::execution::WorkerLifecycleOperationId::new(),
    });
    assert!(replacement_stop.is_accepted(), "{replacement_stop:?}");
    assert_ne!(
        replacement_stop.termination.as_ref().unwrap().execution_id,
        original.execution_id
    );
    assert!(
        reopened.execution_termination(&worker.worker_ref).is_err(),
        "a different run/stop-operation receipt must not replace canonical evidence"
    );
    assert!(backend.workers.lock().unwrap().is_empty());
    assert!(
        backend
            .pending_restore(&worker.worker_ref)
            .unwrap()
            .is_none()
    );
    reopened.delete_worker(&worker.worker_ref).unwrap();
    assert!(!worker_path.exists());
}

struct RemovalReadOnlyFactory {
    cleanup_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl RuntimeWorkerFactory for RemovalReadOnlyFactory {
    async fn spawn_controller(
        &self,
        _: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        panic!("removal must not start a Controller")
    }

    async fn prepare_restore(
        &self,
        _: WorkerExecutionRestoreRequest,
    ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        panic!("removal must not prepare a replacement")
    }

    async fn reconcile_stopped_worker(&self, _: &WorkerRef) -> Result<(), String> {
        self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        Err("settled removal must not repeat Stop cleanup".into())
    }
}

#[test]
fn worker_removal_preflight_rejects_pending_spawn_without_changing_ownership() {
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let backend = WorkerRuntimeExecutionBackend::new(RemovalReadOnlyFactory {
        cleanup_calls: cleanup_calls.clone(),
    })
    .unwrap();
    let worker_ref = WorkerRef::new(WorkerId::now_v7());
    let pending = Arc::new(PendingRuntimeSpawn {
        execution_id: uuid::Uuid::now_v7(),
        task: tokio::sync::Mutex::new(None),
        result: Mutex::new(None),
        workdir_attachments: BTreeMap::new(),
        rollback_workdirs: Vec::new(),
        failure: Mutex::new(Some("spawn completion unknown".into())),
    });
    backend
        .pending_spawns
        .lock()
        .unwrap()
        .insert(worker_ref.clone(), pending.clone());
    assert!(backend.preflight_worker_removal(&worker_ref).is_err());
    assert!(Arc::ptr_eq(
        &backend.pending_spawns.lock().unwrap()[&worker_ref],
        &pending
    ));
    backend
        .preflight_worker_removal(&WorkerRef::new(WorkerId::now_v7()))
        .unwrap();
    backend.pending_spawns.lock().unwrap().remove(&worker_ref);
    backend.preflight_worker_removal(&worker_ref).unwrap();
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn worker_removal_preflight_rejects_active_but_not_finished_subject_reservations() {
    use runtime_api::SubjectReleaseState as State;
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let backend = WorkerRuntimeExecutionBackend::new(RemovalReadOnlyFactory {
        cleanup_calls: cleanup_calls.clone(),
    })
    .unwrap();
    let worker_ref = WorkerRef::new(WorkerId::now_v7());
    let key = (worker_ref.clone(), "binding".to_string());
    for state in [State::Reserved, State::Cancelled, State::Disconnected] {
        backend.subject_reservations.lock().unwrap().insert(
            key.clone(),
            SubjectControllerReservation {
                request: runtime_api::SubjectReleaseRequest {
                    workspace_id: "ws-test".into(),
                    worker_id: worker_ref.worker_id,
                    binding_id: key.1.clone(),
                    operation_id: "release".into(),
                    action: runtime_api::SubjectReleaseAction::Reserve,
                },
                guard: None,
                state,
            },
        );
        assert_eq!(
            backend.preflight_worker_removal(&worker_ref).is_ok(),
            state != State::Reserved
        );
        assert_eq!(
            backend.subject_reservations.lock().unwrap()[&key].state,
            state
        );
        backend
            .preflight_worker_removal(&WorkerRef::new(WorkerId::now_v7()))
            .unwrap();
    }
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
}

fn poison_removal_test_mutex<T>(mutex: &Mutex<T>) {
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("injected removal registry poison");
        }))
        .is_err()
    );
}

#[test]
fn worker_removal_preflight_rejects_poisoned_current_resource_or_operation_locks() {
    for resource in [
        "locks",
        "operation",
        "workers",
        "spawns",
        "restores",
        "reservations",
    ] {
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let backend = WorkerRuntimeExecutionBackend::new(RemovalReadOnlyFactory {
            cleanup_calls: cleanup_calls.clone(),
        })
        .unwrap();
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        match resource {
            "locks" => poison_removal_test_mutex(&backend.worker_locks),
            "operation" => poison_removal_test_mutex(&backend.worker_lock(&worker_ref).unwrap()),
            "workers" => poison_removal_test_mutex(&backend.workers),
            "spawns" => poison_removal_test_mutex(&backend.pending_spawns),
            "restores" => poison_removal_test_mutex(&backend.pending_restores),
            "reservations" => poison_removal_test_mutex(&backend.subject_reservations),
            _ => unreachable!(),
        }
        assert!(
            backend.preflight_worker_removal(&worker_ref).is_err(),
            "{resource}"
        );
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0, "{resource}");
    }
}

#[cfg(feature = "fs-store")]
fn stopped_worker_removal_fixture() -> (
    tempfile::TempDir,
    crate::fs_store::FsRuntimeStoreOptions,
    WorkerRef,
    crate::execution::WorkerExecutionTermination,
) {
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("runtime"))
        .with_runtime_id("delete-original-stop");
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
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend).unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime
        .create_worker(create_request("delete original stop"))
        .unwrap();
    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    drop(runtime);
    (temp, options, worker.worker_ref, original)
}

#[cfg(feature = "fs-store")]
#[test]
fn legacy_settled_worker_without_termination_receipt_can_be_deleted() {
    let (_temp, options, worker_ref, _) = stopped_worker_removal_fixture();
    let worker_path = options
        .root
        .join("workers")
        .join(worker_ref.worker_id.to_string())
        .join("worker.json");
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&worker_path).unwrap()).unwrap();
    // Released schema 15 never held an original termination. Its migration must
    // preserve status/observation but must not reconstruct shutdown from them.
    let mut legacy = document;
    legacy["schema_version"] = serde_json::json!(15);
    legacy["execution_state"]["state"] = serde_json::json!("available");
    legacy["execution_state"]["execution"]["restore_intent"] = serde_json::json!("explicit");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("workdir_attachments_guard");
    legacy["restore_guard"]
        .as_object_mut()
        .unwrap()
        .remove("execution_termination");
    let observation = legacy["restore_guard"]["observation_token"].clone();
    fs::write(&worker_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let runtime_path = options.root.join("runtime.json");
    let mut runtime_document: serde_json::Value =
        serde_json::from_slice(&fs::read(&runtime_path).unwrap()).unwrap();
    runtime_document["schema_version"] = serde_json::json!(15);
    fs::write(
        &runtime_path,
        serde_json::to_vec_pretty(&runtime_document).unwrap(),
    )
    .unwrap();
    crate::fs_store::FsRuntimeStore::migrate(&options).unwrap();
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(RemovalReadOnlyFactory {
            cleanup_calls: cleanup_calls.clone(),
        })
        .unwrap(),
    );
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend).unwrap();
    assert_eq!(
        reopened.worker_detail(&worker_ref).unwrap().status,
        crate::catalog::WorkerStatus::Stopped
    );
    assert!(
        reopened
            .execution_termination(&worker_ref)
            .unwrap()
            .is_none()
    );
    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(&worker_path).unwrap()).unwrap();
    assert_eq!(migrated["restore_guard"]["observation_token"], observation);
    assert!(
        migrated["restore_guard"]
            .get("execution_termination")
            .is_none()
    );
    reopened.delete_worker(&worker_ref).unwrap();
    assert!(!worker_path.exists());
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
}

#[cfg(feature = "fs-store")]
#[test]
fn delete_after_reopen_does_not_reprove_termination_or_repeat_stop_cleanup() {
    // Each case gets its own canonical Worker: successful deletion is permanent.
    for mismatch in ["missing", "worker", "execution", "operation", "poisoned"] {
        let (_temp, options, worker_ref, original) = stopped_worker_removal_fixture();
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(RemovalReadOnlyFactory {
                cleanup_calls: cleanup_calls.clone(),
            })
            .unwrap(),
        );
        let reopened =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                .unwrap();
        assert!(backend.completed_stops.lock().unwrap().is_empty());
        assert_eq!(
            reopened.execution_termination(&worker_ref).unwrap(),
            Some(original.clone())
        );
        let mut observed = original;
        match mismatch {
            "missing" => {}
            "worker" => observed.worker_ref = WorkerRef::new(WorkerId::now_v7()),
            "execution" => observed.execution_id = uuid::Uuid::now_v7(),
            "operation" => {
                observed.stop_operation_id = crate::execution::WorkerLifecycleOperationId::new()
            }
            "poisoned" => poison_removal_test_mutex(&backend.completed_stops),
            _ => unreachable!(),
        }
        if !matches!(mismatch, "missing" | "poisoned") {
            backend
                .completed_stops
                .lock()
                .unwrap()
                .insert(worker_ref.clone(), observed);
        }
        reopened.delete_worker(&worker_ref).unwrap();
        assert!(
            !options
                .root
                .join("workers")
                .join(worker_ref.worker_id.to_string())
                .exists(),
            "{mismatch}"
        );
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0, "{mismatch}");
        assert!(backend.workers.lock().unwrap().is_empty());
        assert!(backend.pending_spawns.lock().unwrap().is_empty());
        assert!(backend.pending_restores.lock().unwrap().is_empty());
        let receipts = backend
            .completed_stops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            receipts.len(),
            usize::from(!matches!(mismatch, "missing" | "poisoned"))
        );
    }
}
