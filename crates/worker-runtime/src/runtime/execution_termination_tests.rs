// These catalog tests substitute the execution response. The real Controller
// join and fs-store reopen connection is covered in worker_backend tests.
#[test]
fn stop_termination_unknown_save_reconciles_original_operation_before_and_after_reopen() {
    for reconcile_in_process in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let options = crate::fs_store::FsRuntimeStoreOptions::new(dir.path())
            .with_runtime_id("termination-unknown-save");
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone()).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(task_request("original receipt"))
            .unwrap();
        let operation_id = WorkerLifecycleOperationId::new();
        runtime
            .begin_stop_operation(&worker.worker_ref, operation_id)
            .unwrap();
        let result = backend.stop_worker_operation(WorkerExecutionStopRequest {
            worker_ref: worker.worker_ref.clone(),
            operation_id,
        });
        let original = result.termination.clone().unwrap();
        runtime_store(&runtime)
            .fail_next_worker_write(crate::fs_store::AtomicWriteFault::AfterRename);
        assert!(matches!(
            runtime.commit_stopped_worker(&worker.worker_ref, operation_id, result),
            Err(RuntimeError::StoreCommitOutcomeUnknown { .. })
        ));
        if reconcile_in_process {
            runtime_store(&runtime).fail_next_worker_reconcile_sync();
            assert!(
                runtime.execution_termination(&worker.worker_ref).is_err(),
                "unknown durability must not fall back to the backend's in-memory receipt"
            );
            assert_eq!(
                runtime.execution_termination(&worker.worker_ref).unwrap(),
                Some(original.clone())
            );
            assert_eq!(
                runtime.worker_detail(&worker.worker_ref).unwrap().status,
                WorkerStatus::Stopped
            );
        }
        assert_eq!(
            backend.stop_operation_ids.lock().unwrap().as_slice(),
            &[operation_id]
        );
        drop(runtime);
        drop(backend);
        let backend = Arc::new(
            crate::worker_backend::WorkerRuntimeExecutionBackend::new(
                crate::worker_backend::ProfileRuntimeWorkerFactory::new(
                    dir.path().join("no-launch"),
                ),
            )
            .unwrap(),
        );
        let reopened = Runtime::with_fs_store_and_execution_backend(options, backend).unwrap();
        assert_eq!(
            reopened.execution_termination(&worker.worker_ref).unwrap(),
            Some(original)
        );
        assert!(!dir.path().join("no-launch").exists());
    }
}

#[test]
fn stop_termination_rejects_missing_wrong_operation_run_worker_and_feature_failure() {
    for invalid in ["missing", "operation", "execution", "worker", "feature"] {
        let (runtime, backend) = runtime_and_backend();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime.create_worker(task_request(invalid)).unwrap();
        let operation_id = WorkerLifecycleOperationId::new();
        runtime
            .begin_stop_operation(&worker.worker_ref, operation_id)
            .unwrap();
        let mut result = backend.stop_worker_operation(WorkerExecutionStopRequest {
            worker_ref: worker.worker_ref.clone(),
            operation_id,
        });
        match invalid {
            "missing" => result.termination = None,
            "operation" => {
                result.termination.as_mut().unwrap().stop_operation_id =
                    WorkerLifecycleOperationId::new()
            }
            "execution" => result.execution_id = Some(uuid::Uuid::now_v7()),
            "worker" => {
                result.termination.as_mut().unwrap().worker_ref = WorkerRef::new(WorkerId::now_v7())
            }
            "feature" => result.termination.as_mut().unwrap().feature_failures.push(
                crate::execution::WorkerFeatureShutdownFailure {
                    category: worker::hook::HookErrorCategory::Timeout,
                    diagnostic: "Feature did not terminate".into(),
                },
            ),
            _ => unreachable!(),
        }
        assert!(
            runtime
                .commit_stopped_worker(&worker.worker_ref, operation_id, result)
                .is_err(),
            "{invalid}"
        );
        assert!(
            runtime
                .execution_termination(&worker.worker_ref)
                .unwrap()
                .is_none(),
            "{invalid}"
        );
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .worker(&worker.worker_ref)
                .unwrap()
                .pending_stop
                .unwrap()
                .operation_id,
            operation_id
        );
    }
}

#[test]
fn stop_termination_cleanup_error_retains_join_without_settling_stop_or_overwriting_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let options = crate::fs_store::FsRuntimeStoreOptions::new(dir.path())
        .with_runtime_id("termination-cleanup");
    let backend = Arc::new(TestExecutionBackend::default());
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone()).unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime
        .create_worker(task_request("cleanup failed"))
        .unwrap();
    let operation_id = WorkerLifecycleOperationId::new();
    runtime
        .begin_stop_operation(&worker.worker_ref, operation_id)
        .unwrap();
    let mut result = backend.stop_worker_operation(WorkerExecutionStopRequest {
        worker_ref: worker.worker_ref.clone(),
        operation_id,
    });
    result.outcome = crate::execution::WorkerExecutionOutcome::Errored;
    result.message = Some("artifact cleanup failed after scope join".into());
    let original = result.termination.clone().unwrap();
    assert!(
        matches!(runtime.commit_stopped_worker(&worker.worker_ref, operation_id, result.clone()),
        Err(RuntimeError::WorkerExecutionRejected { result: rejected, .. }) if rejected == result)
    );
    assert_eq!(
        runtime.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    let mut changed = result.clone();
    changed
        .termination
        .as_mut()
        .unwrap()
        .submissions
        .push(crate::execution::WorkerSubmissionAck {
            submission_request_id: "not-original".into(),
            submission_id: "other".into(),
            disposition: protocol::SubmissionDisposition::Started,
        });
    assert!(matches!(
        runtime.commit_stopped_worker(&worker.worker_ref, operation_id, changed),
        Err(RuntimeError::InvalidRequest(_))
    ));
    assert_eq!(
        runtime.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    drop(runtime);
    let backend = Arc::new(
        crate::worker_backend::WorkerRuntimeExecutionBackend::new(
            crate::worker_backend::ProfileRuntimeWorkerFactory::new(dir.path().join("no-launch")),
        )
        .unwrap(),
    );
    let reopened = Runtime::with_fs_store_and_execution_backend(options, backend).unwrap();
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original)
    );
    assert_eq!(
        reopened
            .lock()
            .unwrap()
            .worker(&worker.worker_ref)
            .unwrap()
            .pending_stop
            .unwrap()
            .operation_id,
        operation_id
    );
}
