//! Connect schema-17 aggregate migration, canonical retention receipts and the
//! production Subject consumer using temporary storage. Only the execution
//! boundary is replaced: any Stop, historical proof lookup or spawn is a failure.
//! The frozen receipt models pre-shutdown-field success, not copied live data.
use super::tests::write_json;
use super::*;
use crate::execution::*;
use crate::identity::WorkerRef;
use crate::interaction::WorkerInput;
use crate::{FsRuntimeStoreOptions, Runtime, RuntimeWorkspaceScope};
use serde_json::{Value, json};
use session_store::Store;
use std::sync::Arc;

const LEGACY_SUCCESS: &str = include_str!("fixtures/legacy-removal-success.json");

struct NoHistoricalStop;

impl WorkerExecutionBackend for NoHistoricalStop {
    fn backend_id(&self) -> &str {
        "legacy-removal-fixture"
    }
    fn preflight_subject_delete_worker(
        &self,
        _: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<(), String> {
        Ok(())
    }
    fn fence_subject_delete_worker(
        &self,
        _: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<(), String> {
        Ok(())
    }
    fn confirm_subject_delete_worker_absent(
        &self,
        _: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<(), String> {
        Ok(())
    }
    fn spawn_worker(&self, _: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        panic!("must not recreate a removed Worker")
    }
    fn dispatch_input(&self, _: &WorkerRef, _: WorkerInput) -> WorkerExecutionResult {
        panic!("must not dispatch to a removed Worker")
    }
    fn stop_worker_operation(&self, _: WorkerExecutionStopRequest) -> WorkerExecutionResult {
        panic!("settled state must not issue Stop")
    }
    fn subject_delete_worker_stop(
        &self,
        _: &runtime_api::SubjectDeleteWorkerRequest,
        _: WorkerExecutionStopRequest,
        _: Option<&WorkerExecutionTermination>,
    ) -> Result<WorkerExecutionResult, String> {
        panic!("settled state must not reconcile Stop")
    }
    fn reconcile_termination_receipt(
        &self,
        _: &WorkerRef,
        _: &WorkerExecutionTermination,
    ) -> Result<Option<WorkerExecutionTermination>, String> {
        panic!("completed removal must not re-attest historical Stop")
    }
}

fn old_receipt(
    root: &Path,
    index: u8,
    disposition: SessionDisposition,
) -> (
    WorkerRetentionExecutionRequest,
    runtime_api::SubjectDeleteWorkerRequest,
) {
    let mut wire: Value = serde_json::from_str(LEGACY_SUCCESS).unwrap();
    let worker: WorkerId = format!("00000000-0000-7000-8000-{index:012}")
        .parse()
        .unwrap();
    let operation = format!("legacy-removal-{index}");
    let fingerprint = format!("legacy-retention-fingerprint-{index}");
    let archive = format!("legacy-archive-{index}");
    for field in ["request", "result"] {
        wire[field]["worker_id"] = json!(worker);
        wire[field]["operation_id"] = json!(operation);
        wire[field]["input_fingerprint"] = json!(fingerprint);
        wire[field]["session_disposition"] = json!(disposition);
    }
    if disposition == SessionDisposition::Archive {
        wire["request"]["archive_id"] = json!(archive);
        let manifest = &mut wire["result"]["archive"];
        manifest["source_worker_id"] = json!(worker);
        manifest["operation_id"] = json!(operation);
        manifest["input_fingerprint"] = json!(fingerprint);
        manifest["archive_id"] = json!(archive);
        let archive_root = root.join("archives/workers").join(&archive);
        let session: session_store::SessionId = format!("10000000-0000-4000-8000-{index:012}")
            .parse()
            .unwrap();
        let store = session_store::WorkerSessionStore::new(archive_root.join("session")).unwrap();
        store
            .create_segment(
                session,
                session_store::new_segment_id(),
                &[session_store::LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id: session,
                    system_prompt: None,
                    config: Default::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        manifest["source_session_id"] = json!(session);
        let (checksum, bytes, count) = checksum_files(
            &collect_files(&archive_root.join("session"), "fixture archive").unwrap(),
            "fixture archive",
        )
        .unwrap();
        manifest["content_checksum_sha256"] = json!(checksum);
        manifest["content_bytes"] = json!(bytes);
        manifest["content_file_count"] = json!(count);
        write_json(&archive_root.join("manifest.json"), manifest);
    } else {
        wire["request"]["archive_id"] = Value::Null;
        wire["result"]["archive"] = Value::Null;
    }
    let request: WorkerRetentionExecutionRequest =
        serde_json::from_value(wire["request"].clone()).unwrap();
    write_json(
        &root
            .join("retention/operations")
            .join(format!("{operation}.json")),
        &wire,
    );
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: request.workspace_id.clone(),
        worker_id: worker,
        create_fingerprint: format!("original-allocation-{index}"),
        operation_id: format!("subject-delete-{index}"),
    };
    (request, deletion)
}

fn read_value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn four_legacy_successes_resume_pending_subject_after_aggregate_migration_but_missing_fifth_does_not()
 {
    let root = tempfile::tempdir().unwrap();
    let (stopped, _, _) = super::runtime_v5_tests::runtime_fixture(root.path(), 17);
    let mut saved = read_value(&root.path().join("runtime.json"));
    let mut children = Vec::new();
    let provider = FsWorkerRetentionProvider::new(root.path());
    for index in 1..=4 {
        let (request, deletion) = old_receipt(root.path(), index, SessionDisposition::Archive);
        let path = provider.operation_path(&request.operation_id).unwrap();
        let digest = read_operation_receipt(&path)
            .unwrap()
            .unwrap()
            .identity_sha256()
            .unwrap();
        let mut fence = json!({"request": deletion, "response": null});
        // Existing child binding survives normalization, including an interrupted
        // acknowledgement between receipt rewrite and parent snapshot commit.
        if index == 4 {
            fence["canonical_removal"] = json!({"request": request, "receipt_sha256": digest});
        }
        saved["subject_delete_worker_fences"][deletion.worker_id.to_string()] = fence;
        children.push((request, deletion, digest));
    }
    let missing = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: "workspace-a".into(),
        worker_id: "00000000-0000-7000-8000-000000000005".parse().unwrap(),
        create_fingerprint: "removed-create-reservation-only".into(),
        operation_id: "subject-delete-5".into(),
    };
    saved["subject_delete_worker_fences"][missing.worker_id.to_string()] =
        json!({"request": missing, "response": null});
    write_json(&root.path().join("runtime.json"), &saved);
    let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-a");
    let scope = RuntimeWorkspaceScope::new("workspace-a", "owner-a");
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options.clone(), Arc::new(NoHistoricalStop))
            .unwrap();
    runtime.bind_runtime_identity("runtime-a").unwrap();
    // The T-825 migration turns old Available/Stopped into settled Stopped;
    // the consumer must not request a receipt even after reopening that state.
    let stopped_deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: "workspace-a".into(),
        worker_id: stopped,
        create_fingerprint: "runtime-v5-fixture".into(),
        operation_id: "delete-old-stopped".into(),
    };
    assert!(
        !runtime
            .subject_delete_worker_scoped(&scope, stopped_deletion.clone())
            .unwrap()
            .originals_absent
    );
    let mut normalized = Vec::new();
    for (request, deletion, digest) in &children {
        let archive = provider
            .archive_dir(request.archive_id.as_ref().unwrap())
            .unwrap();
        let original_manifest = fs::read(archive.join("manifest.json")).unwrap();
        let original_payload = collect_files(&archive.join("session"), "fixture originals")
            .unwrap()
            .into_iter()
            .map(|(relative, path)| (relative, fs::read(path).unwrap()))
            .collect::<Vec<_>>();
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, deletion.clone())
                .unwrap()
                .originals_absent
        );
        let path = provider.operation_path(&request.operation_id).unwrap();
        let wire = read_value(&path);
        assert_eq!(wire["schema_version"], OPERATION_SCHEMA_VERSION);
        assert!(wire.get("shutdown").is_none());
        assert!(
            wire.get("create_fingerprint").is_none(),
            "do not manufacture unknown historical identity"
        );
        let receipt = read_operation_receipt(&path).unwrap().unwrap();
        assert_eq!(receipt.request, *request);
        assert_eq!(receipt.identity_sha256().unwrap(), *digest);
        assert_eq!(
            fs::read(archive.join("manifest.json")).unwrap(),
            original_manifest
        );
        for (relative, bytes) in original_payload {
            assert_eq!(
                fs::read(archive.join("session").join(relative)).unwrap(),
                bytes
            );
        }
        normalized.push(fs::read(&path).unwrap());
        assert_eq!(provider.execute(request).unwrap(), receipt.result);
    }
    let error = runtime
        .subject_delete_worker_scoped(&scope, missing.clone())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("completed canonical removal receipt"),
        "{error}"
    );
    assert!(
        !provider
            .operation_path("legacy-removal-5")
            .unwrap()
            .exists()
    );
    drop(runtime);
    let runtime =
        Runtime::with_fs_store_and_execution_backend(options, Arc::new(NoHistoricalStop)).unwrap();
    runtime.bind_runtime_identity("runtime-a").unwrap();
    for ((request, deletion, _), bytes) in children.iter().zip(normalized) {
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, deletion.clone())
                .unwrap()
                .originals_absent
        );
        assert_eq!(
            fs::read(provider.operation_path(&request.operation_id).unwrap()).unwrap(),
            bytes,
            "exact retry must not rewrite the normalized operation"
        );
        let mut changed = deletion.clone();
        changed.create_fingerprint.push_str("-changed");
        assert!(
            runtime
                .subject_delete_worker_scoped(&scope, changed)
                .is_err()
        );
        assert!(
            runtime
                .subject_delete_worker_scoped(
                    &RuntimeWorkspaceScope::new("workspace-b", "owner-a"),
                    deletion.clone()
                )
                .is_err()
        );
    }
    assert!(
        !runtime
            .subject_delete_worker_scoped(&scope, stopped_deletion)
            .unwrap()
            .originals_absent
    );
    assert!(
        runtime
            .subject_delete_worker_scoped(&scope, missing.clone())
            .is_err()
    );
    let saved = read_value(&root.path().join("runtime.json"));
    assert!(
        saved["subject_delete_worker_fences"][missing.worker_id.to_string()]["response"].is_null()
    );
}

#[test]
fn legacy_success_rejects_contradictions_without_rewriting_original_evidence() {
    for case in [
        "pending",
        "worker",
        "workspace",
        "runtime",
        "purge",
        "diagnostics",
        "archive",
        "reappeared",
        "successor",
        "fingerprint",
        "archive_missing",
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("workers")).unwrap();
        let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Archive);
        let provider = FsWorkerRetentionProvider::new(root.path());
        let path = provider.operation_path(&request.operation_id).unwrap();
        let mut wire = read_value(&path);
        match case {
            "pending" => wire["result"]["source_removed"] = json!(false),
            "worker" => wire["result"]["worker_id"] = json!(WorkerId::now_v7()),
            "workspace" => wire["request"]["workspace_id"] = json!("workspace-b"),
            "runtime" => wire["request"]["source_runtime_id"] = json!("runtime-b"),
            "purge" => wire["result"]["session_disposition"] = json!("purge"),
            "diagnostics" => wire["result"]["diagnostics_retained"] = json!(true),
            "archive" => wire["result"]["archive"]["operation_id"] = json!("another-operation"),
            "reappeared" => fs::create_dir_all(provider.worker_dir(request.worker_id)).unwrap(),
            "successor" => {
                let mut successor = wire.clone();
                successor["request"]["operation_id"] = json!("successor");
                successor["result"]["operation_id"] = json!("successor");
                successor["result"]["archive"]["operation_id"] = json!("successor");
                write_json(&provider.operation_path("successor").unwrap(), &successor);
            }
            "fingerprint" => wire["create_fingerprint"] = json!("another-generation"),
            "archive_missing" => fs::remove_file(
                provider
                    .archive_dir(request.archive_id.as_ref().unwrap())
                    .unwrap()
                    .join("manifest.json"),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        write_json(&path, &wire);
        let original = fs::read(&path).unwrap();
        assert!(
            provider
                .subject_completed_removal(&deletion, "runtime-a", None)
                .is_err(),
            "{case}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "{case}: rejected normalization must preserve evidence"
        );
    }
}

// Both the public recovery entry and the Subject consumer must reject lost
// originals before normalizing a receipt or acknowledging its bound child.
#[test]
fn archive_payload_faults_preserve_receipt_and_subject_child_across_reopen() {
    for (completed, schema) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        for fault in ["missing", "corrupt"] {
            let root = tempfile::tempdir().unwrap();
            super::runtime_v5_tests::runtime_fixture(root.path(), 17);
            let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Archive);
            let provider = FsWorkerRetentionProvider::new(root.path());
            let path = provider.operation_path(&request.operation_id).unwrap();
            let archive = provider
                .archive_dir(request.archive_id.as_ref().unwrap())
                .unwrap();
            let session_root = archive.join("session");
            let original_payload = collect_files(&session_root, "fixture originals")
                .unwrap()
                .into_iter()
                .map(|(relative, path)| (relative, fs::read(path).unwrap()))
                .collect::<Vec<_>>();
            let mut wire = read_value(&path);
            wire["schema_version"] = json!(schema);
            wire["result"]["source_removed"] = json!(completed);
            write_json(&path, &wire);
            let original_receipt = fs::read(&path).unwrap();
            let digest = read_operation_receipt(&path)
                .unwrap()
                .unwrap()
                .identity_sha256()
                .unwrap();
            let mut fence = json!({"request": deletion, "response": null});
            if completed {
                fence["canonical_removal"] = json!({"request": request, "receipt_sha256": digest});
            }
            let snapshot_path = root.path().join("runtime.json");
            let mut snapshot = read_value(&snapshot_path);
            snapshot["subject_delete_worker_fences"][deletion.worker_id.to_string()] =
                fence.clone();
            write_json(&snapshot_path, &snapshot);
            if fault == "missing" {
                fs::remove_dir_all(&session_root).unwrap();
            } else {
                let (_, segment) = collect_files(&session_root, "fixture segment")
                    .unwrap()
                    .into_iter()
                    .find(|(relative, _)| relative.extension().is_some_and(|ext| ext == "jsonl"))
                    .unwrap();
                OpenOptions::new()
                    .append(true)
                    .open(segment)
                    .unwrap()
                    .write_all(b"corrupt after publication\n")
                    .unwrap();
            }
            let damaged_archive = collect_files(&archive, "damaged fixture")
                .unwrap()
                .into_iter()
                .map(|(relative, path)| (relative, fs::read(path).unwrap()))
                .collect::<Vec<_>>();
            let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-a");
            let scope = RuntimeWorkspaceScope::new("workspace-a", "owner-a");
            for _ in 0..2 {
                let runtime = Runtime::with_fs_store_and_execution_backend(
                    options.clone(),
                    Arc::new(NoHistoricalStop),
                )
                .unwrap();
                runtime.bind_runtime_identity("runtime-a").unwrap();
                let error = runtime
                    .execute_worker_retention(&request)
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("archive checksum or content summary mismatch"),
                    "{completed}/{fault}: {error}"
                );
                assert_eq!(fs::read(&path).unwrap(), original_receipt);
                assert!(
                    runtime
                        .subject_delete_worker_scoped(&scope, deletion.clone())
                        .is_err()
                );
                assert_eq!(fs::read(&path).unwrap(), original_receipt);
                assert_eq!(
                    read_value(&snapshot_path)["subject_delete_worker_fences"]
                        [deletion.worker_id.to_string()],
                    fence
                );
                assert!(!provider.worker_dir(request.worker_id).exists());
                assert_eq!(
                    collect_files(&archive, "after rejection")
                        .unwrap()
                        .into_iter()
                        .map(|(relative, path)| (relative, fs::read(path).unwrap()))
                        .collect::<Vec<_>>(),
                    damaged_archive
                );
            }
            // Repair only this isolated fixture. No new operation or child is
            // minted; the original pending removal may now finish and replay.
            for (relative, bytes) in original_payload {
                let destination = session_root.join(relative);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::write(destination, bytes).unwrap();
            }
            let runtime =
                Runtime::with_fs_store_and_execution_backend(options, Arc::new(NoHistoricalStop))
                    .unwrap();
            runtime.bind_runtime_identity("runtime-a").unwrap();
            let result = runtime.execute_worker_retention(&request).unwrap();
            assert!(result.source_removed);
            assert!(
                runtime
                    .subject_delete_worker_scoped(&scope, deletion.clone())
                    .unwrap()
                    .originals_absent
            );
            let normalized = read_operation_receipt(&path).unwrap().unwrap();
            assert_eq!(normalized.request, request);
            if completed {
                assert_eq!(normalized.identity_sha256().unwrap(), digest);
            }
            let bytes = fs::read(&path).unwrap();
            assert_eq!(runtime.execute_worker_retention(&request).unwrap(), result);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
}

#[test]
fn matching_explicit_erasure_allows_original_operation_retry_but_unrelated_erasure_does_not() {
    for completed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        super::runtime_v5_tests::runtime_fixture(root.path(), 17);
        let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Archive);
        let provider = FsWorkerRetentionProvider::new(root.path());
        let path = provider.operation_path(&request.operation_id).unwrap();
        let archive = provider
            .archive_dir(request.archive_id.as_ref().unwrap())
            .unwrap();
        let mut wire = read_value(&path);
        wire["result"]["source_removed"] = json!(completed);
        write_json(&path, &wire);
        let original = fs::read(&path).unwrap();
        let session = wire["result"]["archive"]["source_session_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let retention = runtime_session_retention(root.path());
        let intent = session_store::retention::SessionDeletionIntent::ErasePermanently;
        let unrelated = session_store::new_session_id();
        retention.delete(unrelated, intent, || Ok(())).unwrap();
        fs::remove_dir_all(archive.join("session")).unwrap();
        assert!(provider.execute(&request).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        retention.delete(session, intent, || Ok(())).unwrap();
        let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-a");
        let scope = RuntimeWorkspaceScope::new("workspace-a", "owner-a");
        let mut normalized = None;
        for _ in 0..2 {
            let runtime = Runtime::with_fs_store_and_execution_backend(
                options.clone(),
                Arc::new(NoHistoricalStop),
            )
            .unwrap();
            runtime.bind_runtime_identity("runtime-a").unwrap();
            let result = runtime.execute_worker_retention(&request).unwrap();
            assert!(result.source_removed);
            assert_eq!(result.operation_id, request.operation_id);
            assert!(
                runtime
                    .subject_delete_worker_scoped(&scope, deletion.clone())
                    .unwrap()
                    .originals_absent
            );
            let bytes = fs::read(&path).unwrap();
            if let Some(saved) = &normalized {
                assert_eq!(&bytes, saved);
            }
            normalized = Some(bytes);
            assert!(!archive.join("session").exists());
            assert!(!provider.worker_dir(request.worker_id).exists());
        }
    }
}

#[test]
fn primary_erasure_never_substitutes_for_a_retained_siblings_erasure() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("workers")).unwrap();
    let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Archive);
    let provider = FsWorkerRetentionProvider::new(root.path());
    let path = provider.operation_path(&request.operation_id).unwrap();
    let archive = provider
        .archive_dir(request.archive_id.as_ref().unwrap())
        .unwrap();
    let session = archive.join("session");
    let retained = session_store::new_session_id();
    let segment = session_store::new_segment_id();
    session_store::WorkerSessionStore::new(
        session.join("retained-sessions").join(retained.to_string()),
    )
    .unwrap()
    .create_segment(
        retained,
        segment,
        &[session_store::LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id: retained,
            system_prompt: None,
            config: Default::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        }],
    )
    .unwrap();
    let mut receipt = read_value(&path);
    let manifest = &mut receipt["result"]["archive"];
    manifest["retained_sessions"] =
        json!([session_store::worker_sources::RetainedSessionBinding {
            owner: session_store::worker_sources::WorkerSessionOwner {
                workspace_id: request.workspace_id.clone(),
                worker_id: request.worker_id.to_string()
            },
            session_id: retained,
            active_segment_id: segment,
        }]);
    let (checksum, bytes, count) =
        checksum_files(&collect_files(&session, "fixture").unwrap(), "fixture").unwrap();
    manifest["content_checksum_sha256"] = json!(checksum);
    manifest["content_bytes"] = json!(bytes);
    manifest["content_file_count"] = json!(count);
    write_json(&archive.join("manifest.json"), manifest);
    write_json(&path, &receipt);
    let original = fs::read(&path).unwrap();
    let primary = receipt["result"]["archive"]["source_session_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let retention = runtime_session_retention(root.path());
    let intent = session_store::retention::SessionDeletionIntent::ErasePermanently;
    retention
        .delete(primary, intent, || {
            remove_archived_session_copies(root.path(), primary)
        })
        .unwrap();
    // Lost sibling originals are NOT authorized by erasing the primary.
    fs::remove_dir_all(session.join("retained-sessions").join(retained.to_string())).unwrap();
    assert!(provider.execute(&request).is_err());
    assert!(
        provider
            .subject_completed_removal(&deletion, "runtime-a", None)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    // The explicit whole-owner action still fences/erases all remaining copies,
    // without changing the historical receipt or requiring preservation proof.
    provider.purge_subject_job_removal(&request).unwrap();
    provider.purge_subject_job_removal(&request).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(
        matches!(retention.ensure_live(retained), Err(session_store::StoreError::SessionErased(id)) if id == retained)
    );
    assert!(provider.execute(&request).unwrap().source_removed);
    assert!(
        provider
            .subject_completed_removal(&deletion, "runtime-a", None)
            .is_ok()
    );
}

#[test]
fn legacy_purge_success_is_not_archive_success_and_exact_replay_keeps_operation() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("workers")).unwrap();
    let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Purge);
    let provider = FsWorkerRetentionProvider::new(root.path());
    let (child, digest) = provider
        .subject_completed_removal(&deletion, "runtime-a", None)
        .unwrap();
    assert_eq!(child, request);
    let result = provider.execute(&request).unwrap();
    assert!(result.source_removed);
    assert!(result.archive.is_none());
    assert_eq!(
        provider
            .subject_completed_removal(&deletion, "runtime-a", Some((&child, &digest)))
            .unwrap(),
        (child, digest)
    );
    let mut changed = request;
    changed.session_disposition = SessionDisposition::Archive;
    changed.archive_id = Some("not-purge".into());
    assert!(provider.completed_for(&changed).is_err());
}

#[test]
fn normalization_preserves_original_shutdown_submissions_and_bound_subject_digest() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("workers")).unwrap();
    let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Purge);
    let provider = FsWorkerRetentionProvider::new(root.path());
    let path = provider.operation_path(&request.operation_id).unwrap();
    let mut receipt = read_operation_receipt(&path).unwrap().unwrap();
    let operation_id = WorkerLifecycleOperationId::new();
    let termination = WorkerExecutionTermination {
        scope: WorkerExecutionTerminationScope::Controller,
        worker_ref: WorkerRef::new(request.worker_id),
        stop_operation_id: operation_id,
        execution_id: uuid::Uuid::now_v7(),
        shutdown_command_id: Some(31),
        submissions: vec![WorkerSubmissionAck {
            submission_request_id: "accepted-before-stop".into(),
            submission_id: "original-submission".into(),
            disposition: protocol::SubmissionDisposition::Queued,
        }],
        feature_failures: vec![],
    };
    receipt.shutdown = Some(RetainedWorkerShutdown {
        create_fingerprint: deletion.create_fingerprint.clone(),
        stop_operation_id: operation_id,
        termination: termination.clone(),
    });
    // This is the old writer's exact shape/order, independent of the new digest
    // method. A persisted parent binding must survive the schema rewrite.
    #[derive(Serialize)]
    struct OriginalReceipt<'a> {
        schema_version: u32,
        request: &'a WorkerRetentionExecutionRequest,
        result: &'a WorkerRetentionExecutionResult,
        shutdown: &'a Option<RetainedWorkerShutdown>,
    }
    let old = OriginalReceipt {
        schema_version: 1,
        request: &request,
        result: &receipt.result,
        shutdown: &receipt.shutdown,
    };
    let digest = Sha256::digest(serde_json::to_vec(&old).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    write_json(&path, &old);
    let (child, normalized_digest) = provider
        .subject_completed_removal(&deletion, "runtime-a", Some((&request, &digest)))
        .unwrap();
    assert_eq!(child, request);
    assert_eq!(normalized_digest, digest);
    let normalized = read_operation_receipt(&path).unwrap().unwrap();
    assert_eq!(normalized.shutdown.unwrap().termination, termination);
    assert!(normalized.create_fingerprint.is_none());
    let bytes = fs::read(&path).unwrap();
    assert_eq!(provider.execute(&request).unwrap(), receipt.result);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn not_started_subject_consumer_never_requests_shutdown_or_fabricates_stop() {
    let root = tempfile::tempdir().unwrap();
    let (worker, _, _) =
        super::runtime_v5_tests::runtime_fixture(root.path(), crate::fs_store::SCHEMA_VERSION);
    let path = root
        .path()
        .join("workers")
        .join(worker.to_string())
        .join("worker.json");
    let mut wire = read_value(&path);
    wire["execution_state"]["state"] = json!("not_started");
    wire["execution_state"]["execution"]["binding"] = Value::Null;
    write_json(&path, &wire);
    let options = FsRuntimeStoreOptions::new(root.path()).with_runtime_id("runtime-a");
    let scope = RuntimeWorkspaceScope::new("workspace-a", "owner-a");
    let deletion = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: "workspace-a".into(),
        worker_id: worker,
        create_fingerprint: "runtime-v5-fixture".into(),
        operation_id: "delete-not-started".into(),
    };
    for _ in 0..2 {
        let runtime = Runtime::with_fs_store_and_execution_backend(
            options.clone(),
            Arc::new(NoHistoricalStop),
        )
        .unwrap();
        assert!(
            !runtime
                .subject_delete_worker_scoped(&scope, deletion.clone())
                .unwrap()
                .originals_absent
        );
        assert!(
            runtime
                .execution_termination(&WorkerRef::new(worker))
                .unwrap()
                .is_none()
        );
        assert!(!root.path().join("retention/operations").exists());
    }
}

#[test]
fn interrupted_removal_without_published_archive_remains_pending_not_inferred_success() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("workers")).unwrap();
    let (request, deletion) = old_receipt(root.path(), 1, SessionDisposition::Archive);
    let provider = FsWorkerRetentionProvider::new(root.path());
    let path = provider.operation_path(&request.operation_id).unwrap();
    let mut wire = read_value(&path);
    wire["result"]["source_removed"] = json!(false);
    write_json(&path, &wire);
    fs::remove_file(
        provider
            .archive_dir(request.archive_id.as_ref().unwrap())
            .unwrap()
            .join("manifest.json"),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        provider
            .subject_completed_removal(&deletion, "runtime-a", None)
            .is_err()
    );
    assert!(provider.recover_after_source_removal(&request).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
