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
fn subject_job_removal_purge_unconfirmed_fence_never_invents_removed_execution_proof() {
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
    // Model proofless retained history, not a current crash window. Removing
    // only the Subject response is insufficient: the original canonical Stop
    // now survives removal and legitimately permits historical reconciliation.
    // Remove that proof too, leaving the completed removal request/result intact.
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
    let removal = serde_json::from_value(serde_json::to_value(&request.removal).unwrap()).unwrap();
    assert_eq!(
        runtime.execute_worker_retention(&removal).unwrap(),
        original_result
    );
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt);
    let error = runtime
        .subject_delete_worker_scoped(&scope, request.deletion.clone())
        .unwrap_err();
    let diagnostic = error.to_string().to_ascii_lowercase();
    assert!(
        diagnostic.contains("original") && diagnostic.contains("shutdown"),
        "expected rejection for absent original shutdown proof, got: {error}"
    );
    assert!(
        runtime
            .purge_subject_job_removal_scoped(&scope, request.clone())
            .is_err()
    );
    assert_eq!(tree(&temp.path().join("archives")), copies);
    assert_eq!(std::fs::read(receipt_path).unwrap(), receipt);
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(snapshot_path).unwrap()).unwrap();
    assert_eq!(
        snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()],
        unconfirmed_fence,
        "proofless history must not acquire a Subject response or a fabricated Stop child"
    );
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
    runtime_session_retention(temp.path())
        .ensure_live(session)
        .unwrap();
}
