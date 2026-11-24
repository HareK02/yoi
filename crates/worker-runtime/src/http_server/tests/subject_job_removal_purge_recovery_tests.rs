//! Real filesystem failures and reopened durable fences for the additional purge.
//! Uses the same production stores/router fixture as the copy-erasure suite.
use super::*;

#[test]
fn subject_job_removal_purge_partial_physical_failure_keeps_fence_and_retry_erases_after_reopen() {
    let f = Fixture::new();
    let mut request = f.removal_purge_request(SessionDisposition::Archive);
    request.removal.diagnostics_disposition = runtime_api::DiagnosticsDisposition::Retain;
    f.complete_removal(&request);
    let receipt_path = f.receipt_path();
    let receipt = std::fs::read(&receipt_path).unwrap();
    let session_root = f
        .temp
        .path()
        .join("archives/workers/canonical-archive/session");
    let parked = f.temp.path().join("interrupted-session-fixture");
    std::fs::rename(&session_root, &parked).unwrap();
    std::fs::write(&session_root, b"deterministic directory removal failure").unwrap();
    assert!(
        f.runtime
            .purge_subject_job_removal_scoped(&f.scope, request.clone())
            .is_err()
    );
    assert!(matches!(
        runtime_session_retention(f.temp.path()).ensure_live(f.session),
        Err(session_store::StoreError::SessionErased(_))
    ));
    assert!(
        !f.temp
            .path()
            .join("archives/diagnostics/remove-source")
            .exists()
    );
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt);
    std::fs::remove_file(&session_root).unwrap();
    std::fs::rename(parked, &session_root).unwrap();
    let Fixture {
        temp,
        runtime,
        scope,
        store,
        session,
        ..
    } = f;
    drop(store);
    drop(runtime);
    let runtime = Runtime::with_fs_store_and_execution_backend(
        crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test"),
        Arc::new(AcceptingBackend),
    )
    .unwrap();
    runtime.bind_runtime_identity("runtime-test").unwrap();
    runtime
        .purge_subject_job_removal_scoped(&scope, request)
        .unwrap();
    assert!(!session_root.exists());
    assert!(matches!(
        runtime_session_retention(temp.path()).lock_live(session),
        Err(session_store::StoreError::SessionErased(_))
    ));
    assert_eq!(std::fs::read(receipt_path).unwrap(), receipt);
}

#[test]
fn subject_delete_confirmed_fence_resumes_pending_child_after_source_removal_and_reopen() {
    let f = Fixture::new();
    let request = f.removal_purge_request(SessionDisposition::Archive);
    let completed = f.complete_removal(&request);
    let receipt_path = f.receipt_path();
    let mut pending: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    pending["result"]["source_removed"] = serde_json::json!(false);
    let pending_bytes = serde_json::to_vec(&pending).unwrap();
    std::fs::write(&receipt_path, &pending_bytes).unwrap();
    let Fixture {
        temp,
        runtime,
        scope,
        store,
        session,
        ..
    } = f;
    drop(store);
    drop(runtime);
    let options = crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test");
    for _ in 0..2 {
        let runtime = Runtime::with_fs_store_and_execution_backend(
            options.clone(),
            Arc::new(AcceptingBackend),
        )
        .unwrap();
        runtime.bind_runtime_identity("runtime-test").unwrap();
        let observed = runtime
            .subject_delete_worker_scoped(&scope, request.deletion.clone())
            .unwrap();
        assert!(!observed.originals_absent);
        assert!(observed.completed_removal.is_none());
        assert_eq!(observed.pending_removal.as_ref(), Some(&request.removal));
        assert!(
            runtime
                .purge_subject_job_removal_scoped(&scope, request.clone())
                .is_err()
        );
        runtime_session_retention(temp.path())
            .ensure_live(session)
            .unwrap();
        assert_eq!(std::fs::read(&receipt_path).unwrap(), pending_bytes);
        // Once a pending child was observed, neither disappearance nor another
        // generation can fall back to the confirmed direct-deletion case.
        std::fs::remove_file(&receipt_path).unwrap();
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, request.deletion.clone())
                .is_err()
        );
        let mut changed = pending.clone();
        changed["create_fingerprint"] = serde_json::json!("another-generation");
        std::fs::write(&receipt_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, request.deletion.clone())
                .is_err()
        );
        std::fs::write(&receipt_path, &pending_bytes).unwrap();
    }
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options, Arc::new(AcceptingBackend)).unwrap();
    runtime.bind_runtime_identity("runtime-test").unwrap();
    let removal = serde_json::from_value(serde_json::to_value(&request.removal).unwrap()).unwrap();
    assert_eq!(
        runtime.execute_worker_retention(&removal).unwrap(),
        completed
    );
    let observed = runtime
        .subject_delete_worker_scoped(&scope, request.deletion.clone())
        .unwrap();
    assert!(observed.originals_absent);
    assert_eq!(observed.completed_removal.as_ref(), Some(&request.removal));
    assert!(observed.pending_removal.is_none());
    runtime
        .purge_subject_job_removal_scoped(&scope, request)
        .unwrap();
    assert!(
        matches!(runtime_session_retention(temp.path()).ensure_live(session), Err(session_store::StoreError::SessionErased(id)) if id == session)
    );
}

#[test]
fn subject_job_removal_purge_legacy_success_reconciles_fence_without_inventing_shutdown() {
    let f = Fixture::new();
    let request = f.removal_purge_request(SessionDisposition::Archive);
    let original_result = f.complete_removal(&request);
    let copies = f.copies();
    let receipt_path = f.receipt_path();
    let mut legacy_receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    let Fixture {
        temp,
        runtime,
        scope,
        store,
        session,
        ..
    } = f;
    drop(store);
    drop(runtime);
    // Model a pre-shutdown-field success and a lost Subject response. The
    // completed canonical removal remains authoritative; normalizing it must
    // neither invent historical execution observations nor erase archive bytes.
    legacy_receipt["schema_version"] = serde_json::json!(1);
    legacy_receipt
        .as_object_mut()
        .unwrap()
        .remove("create_fingerprint");
    assert!(
        legacy_receipt
            .as_object_mut()
            .unwrap()
            .remove("shutdown")
            .is_some_and(|shutdown| shutdown.is_object())
    );
    let receipt = serde_json::to_vec(&legacy_receipt).unwrap();
    std::fs::write(&receipt_path, &receipt).unwrap();
    let snapshot_path = temp.path().join("runtime.json");
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    let fence =
        &mut snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()];
    assert!(fence["response"].is_object());
    assert!(fence.get("canonical_removal").is_none());
    fence["response"] = serde_json::Value::Null;
    let unconfirmed_fence = fence.clone();
    std::fs::write(&snapshot_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let runtime = Runtime::with_fs_store_and_execution_backend(
        crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test"),
        Arc::new(AcceptingBackend),
    )
    .unwrap();
    runtime.bind_runtime_identity("runtime-test").unwrap();
    assert!(
        runtime
            .purge_subject_job_removal_scoped(&scope, request.clone())
            .is_err()
    );
    assert_eq!(tree(&temp.path().join("archives")), copies);
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt);
    let response = runtime
        .subject_delete_worker_scoped(&scope, request.deletion.clone())
        .unwrap();
    assert!(response.originals_absent);
    assert_eq!(response.operation_id, request.deletion.operation_id);
    assert_eq!(
        response.create_fingerprint,
        request.deletion.create_fingerprint
    );
    assert_eq!(tree(&temp.path().join("archives")), copies);
    runtime_session_retention(temp.path())
        .ensure_live(session)
        .unwrap();
    let normalized_bytes = std::fs::read(&receipt_path).unwrap();
    let normalized: serde_json::Value = serde_json::from_slice(&normalized_bytes).unwrap();
    assert_eq!(normalized["request"], legacy_receipt["request"]);
    assert_eq!(normalized["result"], legacy_receipt["result"]);
    assert!(
        normalized
            .get("shutdown")
            .is_none_or(serde_json::Value::is_null)
    );
    assert!(
        normalized
            .get("create_fingerprint")
            .is_none_or(serde_json::Value::is_null)
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    let confirmed_fence =
        &snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()];
    assert_eq!(confirmed_fence["request"], unconfirmed_fence["request"]);
    assert_eq!(
        confirmed_fence["response"],
        serde_json::to_value(&response).unwrap()
    );
    assert_eq!(
        confirmed_fence["canonical_removal"]["request"],
        normalized["request"]
    );
    assert!(
        confirmed_fence["canonical_removal"]
            .get("shutdown")
            .is_none()
    );
    for _ in 0..2 {
        assert_eq!(
            runtime
                .subject_delete_worker_scoped(&scope, request.deletion.clone())
                .unwrap(),
            response
        );
        let removal =
            serde_json::from_value(serde_json::to_value(&request.removal).unwrap()).unwrap();
        assert_eq!(
            runtime.execute_worker_retention(&removal).unwrap(),
            original_result
        );
        runtime
            .purge_subject_job_removal_scoped(&scope, request.clone())
            .unwrap();
        assert_eq!(std::fs::read(&receipt_path).unwrap(), normalized_bytes);
    }
    assert!(
        !temp
            .path()
            .join("archives/workers/canonical-archive/session")
            .exists()
    );
    assert!(matches!(
        runtime_session_retention(temp.path()).ensure_live(session),
        Err(session_store::StoreError::SessionErased(_))
    ));
    assert!(
        runtime
            .worker_detail(&WorkerRef::new(request.deletion.worker_id))
            .is_err()
    );
    assert!(
        !temp
            .path()
            .join("workers")
            .join(request.deletion.worker_id.to_string())
            .exists()
    );
}

#[test]
fn subject_job_removal_purge_unresolved_removal_never_confirms_absent_worker() {
    for condition in ["missing", "pending", "foreign_workspace"] {
        let f = Fixture::new();
        let request = f.removal_purge_request(SessionDisposition::Archive);
        f.complete_removal(&request);
        let copies = f.copies();
        let receipt_path = f.receipt_path();
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        let Fixture {
            temp,
            runtime,
            scope,
            store,
            session,
            ..
        } = f;
        drop(store);
        drop(runtime);
        receipt["schema_version"] = serde_json::json!(1);
        receipt.as_object_mut().unwrap().remove("shutdown");
        receipt
            .as_object_mut()
            .unwrap()
            .remove("create_fingerprint");
        match condition {
            "missing" => std::fs::remove_file(&receipt_path).unwrap(),
            "pending" => {
                receipt["result"]["source_removed"] = serde_json::json!(false);
                std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
            "foreign_workspace" => {
                receipt["request"]["workspace_id"] = serde_json::json!("other-workspace");
                std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        let retained_receipt = std::fs::read(&receipt_path).ok();
        let snapshot_path = temp.path().join("runtime.json");
        let mut snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        let fence =
            &mut snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()];
        fence["response"] = serde_json::Value::Null;
        assert!(fence.get("canonical_removal").is_none());
        let unconfirmed = fence.clone();
        std::fs::write(&snapshot_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::FsRuntimeStoreOptions::new(temp.path()).with_runtime_id("runtime-test"),
            Arc::new(AcceptingBackend),
        )
        .unwrap();
        runtime.bind_runtime_identity("runtime-test").unwrap();
        for _ in 0..2 {
            assert!(
                runtime
                    .subject_delete_worker_scoped(&scope, request.deletion.clone())
                    .is_err(),
                "{condition}"
            );
            assert!(
                runtime
                    .purge_subject_job_removal_scoped(&scope, request.clone())
                    .is_err(),
                "{condition}"
            );
        }
        assert_eq!(tree(&temp.path().join("archives")), copies, "{condition}");
        assert_eq!(
            std::fs::read(&receipt_path).ok(),
            retained_receipt,
            "{condition}"
        );
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        assert_eq!(
            snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()],
            unconfirmed,
            "{condition}"
        );
        runtime_session_retention(temp.path())
            .ensure_live(session)
            .unwrap();
    }
}
