// Runtime component tests use real filesystem persistence and replace only the
// external execution boundary. Actual Controller shutdown is tested by the adapter.
#[cfg(feature = "fs-store")]
#[test]
fn subject_delete_reserved_id_survives_unknown_reopen_and_rejects_identity_changes() {
    let root = tempfile::tempdir().unwrap();
    let options =
        crate::fs_store::FsRuntimeStoreOptions::new(root.path()).with_runtime_id("test-runtime");
    let scoped = scope("workspace-a", "server-a");
    let create = scoped_task_request("delayed create", "workspace-a");
    let request = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scoped.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "subject-delete-1".into(),
    };
    let backend = Arc::new(TestExecutionBackend::default());
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone()).unwrap();
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .is_err()
    );
    assert!(
        runtime.lock().unwrap().subject_delete_worker_fences[&create.worker_id]
            .response
            .is_none()
    );
    assert!(
        runtime
            .create_worker_scoped(&scoped, create.clone())
            .is_err()
    );
    assert_eq!(backend.deletion_checks.load(Ordering::SeqCst), 1);
    drop(runtime);
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone()).unwrap();
    assert!(
        runtime
            .create_worker_scoped(&scoped, create.clone())
            .is_err()
    );
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .is_err()
    );
    backend.deletion_confirmed.store(true, Ordering::SeqCst);
    // A generic adapter confirmation cannot replace the missing original
    // Stop child/receipt, even for a never-started catalog ID.
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .is_err()
    );
    assert!(
        runtime.lock().unwrap().subject_delete_worker_fences[&create.worker_id]
            .response
            .is_none()
    );
    for index in 0..4 {
        let mut changed = request.clone();
        match index {
            0 => changed.worker_id = WorkerId::now_v7(),
            1 => changed.operation_id.push_str("-changed"),
            2 => changed.create_fingerprint.push_str("-changed"),
            _ => changed.workspace_id.push_str("-changed"),
        }
        assert!(
            runtime
                .subject_delete_worker_scoped(&scoped, changed)
                .is_err()
        );
    }
    assert!(
        runtime
            .subject_delete_worker_scoped(&scope("workspace-a", "server-b"), request.clone())
            .is_err()
    );
    assert!(
        runtime
            .subject_delete_worker_scoped(&scope("workspace-b", "server-a"), request.clone())
            .is_err()
    );
    assert!(
        !runtime
            .lock()
            .unwrap()
            .forget_workspace_owner_if_unused("workspace-a")
    );
    drop(runtime);
    // A saved exclusion must not manufacture fresh shutdown proof on reopen.
    let backend = Arc::new(TestExecutionBackend::default());
    let runtime = Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
    assert_eq!(*backend.restore_count.lock().unwrap(), 0);
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .is_err()
    );
    backend.deletion_confirmed.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request)
            .is_err()
    );
    assert!(runtime.create_worker_scoped(&scoped, create).is_err());
}

#[cfg(feature = "fs-store")]
#[test]
fn subject_delete_existing_stopped_or_active_worker_preserves_record_and_allows_canonical_removal()
{
    for stopped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let backend = Arc::new(TestExecutionBackend::default());
        backend.deletion_confirmed.store(true, Ordering::SeqCst);
        let options = crate::fs_store::FsRuntimeStoreOptions::new(root.path())
            .with_runtime_id("test-runtime");
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone()).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let scoped = scope("workspace-a", "server-a");
        let create = scoped_task_request("subject body", "workspace-a");
        let worker = runtime
            .create_worker_scoped(&scoped, create.clone())
            .unwrap();
        if stopped {
            runtime.stop_worker(&worker.worker_ref, None).unwrap();
        }
        let request = runtime_api::SubjectDeleteWorkerRequest {
            workspace_id: scoped.workspace_id.clone(),
            worker_id: create.worker_id,
            create_fingerprint: create.create_fingerprint.clone(),
            operation_id: "delete-body".into(),
        };
        let mut mismatch = request.clone();
        mismatch.create_fingerprint.push_str("-wrong");
        assert!(
            runtime
                .subject_delete_worker_scoped(&scoped, mismatch)
                .is_err()
        );
        let mut mismatch = request.clone();
        mismatch.workspace_id = "workspace-b".into();
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope("workspace-b", "server-b"), mismatch)
                .is_err()
        );
        assert!(
            !runtime
                .lock()
                .unwrap()
                .subject_delete_worker_fences
                .contains_key(&create.worker_id)
        );
        // A lost Worker settlement cannot be reported as completed deletion.
        if !stopped {
            runtime
                .test_runtime_store()
                .fail_next_worker_write(crate::fs_store::AtomicWriteFault::AfterRename);
            assert!(
                runtime
                    .subject_delete_worker_scoped(&scoped, request.clone())
                    .is_err()
            );
        }
        let original = runtime.execution_termination(&worker.worker_ref).unwrap();
        let receipt = runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .unwrap();
        assert!(
            !receipt.originals_absent,
            "a stopped catalog record still has original material"
        );
        if let Some(original) = original {
            assert_eq!(
                runtime.execution_termination(&worker.worker_ref).unwrap(),
                Some(original)
            );
        }
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().status,
            WorkerStatus::Stopped
        );
        assert!(
            runtime
                .restore_worker(
                    &worker.worker_ref,
                    runtime.test_restore_request(&worker.worker_ref)
                )
                .is_err()
        );
        assert!(
            runtime
                .create_worker_scoped(&scoped, create.clone())
                .is_err()
        );
        let record = runtime
            .test_runtime_store()
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(record.worker_id, worker.worker_ref.worker_id);
        drop(runtime);
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
        assert!(
            runtime
                .restore_worker(
                    &worker.worker_ref,
                    runtime.test_restore_request(&worker.worker_ref)
                )
                .is_err()
        );
        assert_eq!(
            runtime
                .subject_delete_worker_scoped(&scoped, request.clone())
                .unwrap(),
            receipt
        );
        runtime
            .delete_worker_scoped(&scoped, &worker.worker_ref)
            .unwrap();
        // Canonical removal deletes the aggregate, not the permanent Subject
        // fence. Its exact retry must finish from the confirmed owner history,
        // without manufacturing another Stop from a missing catalog entry.
        let removed = runtime
            .subject_delete_worker_scoped(&scoped, request)
            .unwrap();
        assert!(removed.originals_absent);
        assert_eq!(removed.workspace_id, receipt.workspace_id);
        assert_eq!(removed.worker_id, receipt.worker_id);
        assert_eq!(removed.create_fingerprint, receipt.create_fingerprint);
        assert_eq!(removed.operation_id, receipt.operation_id);
        assert!(runtime.create_worker_scoped(&scoped, create).is_err());
        assert!(
            !runtime
                .lock()
                .unwrap()
                .forget_workspace_owner_if_unused("workspace-a")
        );
    }
}

#[cfg(feature = "fs-store")]
#[test]
fn subject_delete_unavailable_backend_never_issues_receipt_but_preserves_exclusion() {
    let root = tempfile::tempdir().unwrap();
    let options =
        crate::fs_store::FsRuntimeStoreOptions::new(root.path()).with_runtime_id("test-runtime");
    let scoped = scope("workspace-a", "server-a");
    let create = scoped_task_request("reserved without backend", "workspace-a");
    let request = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scoped.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "delete-offline".into(),
    };
    let runtime = Runtime::with_fs_store(options.clone()).unwrap();
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request.clone())
            .is_err()
    );
    assert!(
        runtime.lock().unwrap().subject_delete_worker_fences[&create.worker_id]
            .response
            .is_none()
    );
    drop(runtime);
    let backend = Arc::new(TestExecutionBackend::default());
    backend.deletion_confirmed.store(true, Ordering::SeqCst);
    let runtime = Runtime::with_fs_store_and_execution_backend(options, backend).unwrap();
    assert!(runtime.create_worker_scoped(&scoped, create).is_err());
    assert!(
        runtime
            .subject_delete_worker_scoped(&scoped, request)
            .is_err()
    );
}

#[cfg(feature = "fs-store")]
#[test]
fn subject_delete_missing_catalog_does_not_claim_residual_originals_absent() {
    let root = tempfile::tempdir().unwrap();
    let options =
        crate::fs_store::FsRuntimeStoreOptions::new(root.path()).with_runtime_id("test-runtime");
    let backend = Arc::new(TestExecutionBackend::default());
    backend.deletion_confirmed.store(true, Ordering::SeqCst);
    let runtime = Runtime::with_fs_store_and_execution_backend(options, backend).unwrap();
    let scoped = scope("workspace-a", "server-a");
    let create = scoped_task_request("unpublished original", "workspace-a");
    let original = root
        .path()
        .join("workers")
        .join(create.worker_id.to_string())
        .join("session/original.jsonl");
    std::fs::create_dir_all(original.parent().unwrap()).unwrap();
    std::fs::write(&original, "preserve original text").unwrap();
    let result = runtime.subject_delete_worker_scoped(
        &scoped,
        runtime_api::SubjectDeleteWorkerRequest {
            workspace_id: scoped.workspace_id.clone(),
            worker_id: create.worker_id,
            create_fingerprint: create.create_fingerprint.clone(),
            operation_id: "delete-unpublished".into(),
        },
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(original).unwrap(),
        "preserve original text"
    );
    assert!(runtime.create_worker_scoped(&scoped, create).is_err());
}
