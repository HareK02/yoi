#[cfg(feature = "fs-store")]
#[test]
fn stop_termination_real_join_survives_fs_reopen_rejects_replacement_and_legacy_missing_receipt() {
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
    assert!(matches!(
        backend.restore_worker(direct_restore_request(worker.worker_ref.clone())),
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
    assert!(reopened.delete_worker(&worker.worker_ref).is_err());
    assert!(worker_path.exists());
    drop(reopened);
    drop(backend);

    // Released schema 15 never held an original termination. Its migration must
    // preserve status/observation but must not reconstruct shutdown from them.
    let mut legacy = document;
    legacy["schema_version"] = serde_json::json!(15);
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
    let backend = new_backend();
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend).unwrap();
    assert_eq!(
        reopened.worker_detail(&worker.worker_ref).unwrap().status,
        crate::catalog::WorkerStatus::Stopped
    );
    assert!(
        reopened
            .execution_termination(&worker.worker_ref)
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
}

#[cfg(feature = "fs-store")]
#[test]
fn delete_after_reopen_consumes_original_stop_without_starting_or_restopping_controller() {
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
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime
        .create_worker(create_request("delete original stop"))
        .unwrap();
    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    // Inject mismatched observations, never positive termination evidence.
    // Deletion must keep the canonical original and refuse every substitution.
    for mismatch in ["worker", "execution", "operation"] {
        let mut observed = original.clone();
        match mismatch {
            "worker" => observed.worker_ref = WorkerRef::new(WorkerId::now_v7()),
            "execution" => observed.execution_id = uuid::Uuid::now_v7(),
            "operation" => {
                observed.stop_operation_id = crate::execution::WorkerLifecycleOperationId::new()
            }
            _ => unreachable!(),
        }
        backend
            .completed_stops
            .lock()
            .unwrap()
            .insert(worker.worker_ref.clone(), observed);
        assert!(
            runtime.delete_worker(&worker.worker_ref).is_err(),
            "{mismatch}"
        );
        assert!(
            options
                .root
                .join("workers")
                .join(worker.worker_id.to_string())
                .exists()
        );
    }
    backend
        .completed_stops
        .lock()
        .unwrap()
        .insert(worker.worker_ref.clone(), original.clone());
    drop(runtime);
    drop(backend);
    let no_launch = root.join("must-not-launch");
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(ProfileRuntimeWorkerFactory::new(&no_launch)).unwrap(),
    );
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    assert!(backend.completed_stops.lock().unwrap().is_empty());
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original)
    );
    reopened.delete_worker(&worker.worker_ref).unwrap();
    assert!(
        !options
            .root
            .join("workers")
            .join(worker.worker_id.to_string())
            .exists()
    );
    assert!(
        backend.completed_stops.lock().unwrap().is_empty(),
        "no new Stop was issued"
    );
    assert!(!no_launch.exists());
}
