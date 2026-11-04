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
fn subject_delete_consumes_prior_canonical_stop_without_retagging_original_scope() {
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
}
