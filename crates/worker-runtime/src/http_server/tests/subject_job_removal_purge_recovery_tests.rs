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
fn subject_job_removal_purge_unconfirmed_durable_deletion_fence_preserves_copies_until_reconciled()
{
    let f = Fixture::new();
    let request = f.removal_purge_request(SessionDisposition::Archive);
    f.complete_removal(&request);
    let copies = f.copies();
    let receipt_path = f.receipt_path();
    let receipt = std::fs::read(&receipt_path).unwrap();
    let Fixture {
        temp,
        runtime,
        scope,
        store,
        ..
    } = f;
    drop(store);
    drop(runtime);
    // Frozen durable crash-window fixture: permanent admission exclusion was
    // saved, but no confirmed execution-shutdown response was committed.
    let snapshot_path = temp.path().join("runtime.json");
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    snapshot["subject_delete_worker_fences"][request.deletion.worker_id.to_string()]["response"] =
        serde_json::Value::Null;
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
    runtime
        .subject_delete_worker_scoped(&scope, request.deletion.clone())
        .unwrap();
    runtime
        .purge_subject_job_removal_scoped(&scope, request)
        .unwrap();
    assert!(
        !temp
            .path()
            .join("archives/workers/canonical-archive/session")
            .exists()
    );
    assert_eq!(std::fs::read(receipt_path).unwrap(), receipt);
}
