//! Runtime integration, not a second implementation of Session migration.
//! The Feature records are frozen v4 wire copied from the Session Store fixture;
//! only the empty segment envelope is built with current APIs.
use super::tests::{request, write_json};
use super::*;
use crate::fs_store::{FsRuntimeStore, FsRuntimeStoreOptions};
use serde_json::{Value, json};
use session_store::{LogEntry, SegmentId, SessionId, Store, WorkerSessionStore};
use std::collections::BTreeMap;

const V4_SYSTEM_INPUTS: &str = include_str!("fixtures/system-input-v4.jsonl");

fn write_v4_session(aggregate: &Path) -> (SessionId, SegmentId) {
    let root = aggregate.join("session");
    let session = session_store::new_session_id();
    let segment = session_store::new_segment_id();
    let store = WorkerSessionStore::new(&root).unwrap();
    store
        .create_segment(
            session,
            segment,
            &[LogEntry::AnnotatedSegmentStart {
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
    let path = root.join("segments").join(format!("{segment}.jsonl"));
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(V4_SYSTEM_INPUTS.as_bytes());
    fs::write(path, bytes).unwrap();
    write_json(
        &root.join("session.json"),
        &json!({
            "schema_version": 4, "session_id": session,
        }),
    );
    (session, segment)
}

fn stopped_worker(root: &Path, schema: u32) -> (WorkerId, SessionId, SegmentId) {
    let worker = WorkerId::now_v7();
    let aggregate = root.join("workers").join(worker.to_string());
    // Serialize request defaults just as a production aggregate writer does.
    let request: crate::catalog::CreateWorkerRequest = serde_json::from_value(json!({
        "worker_id": worker,
        "workspace_api": {"workspace_id": "workspace-a", "base_url": "https://workspace.invalid"},
        "create_fingerprint": "runtime-v5-fixture",
        "profile": {"kind": "builtin", "value": "builtin:coder"},
        "profile_source": {"kind": "workspace_config", "archive": {
            "id": "archive-1", "digest": "sha256:archive", "size_bytes": 0,
            "source_graph": {"source_count": 0, "total_source_bytes": 0,
                "entrypoints": {}, "import_count": 0}
        }}
    }))
    .unwrap();
    write_json(
        &aggregate.join("worker.json"),
        &json!({
            "schema_version": schema,
            "worker_ref": {"worker_id": worker}, "worker_id": worker,
            "profile": request.profile,
            "profile_source": request.profile_source.reference(),
            "display_name": null, "config_bundle": null,
            "created_at_ms": 1234, "status": "stopped", "workspace_id": "workspace-a",
            "last_finished_submission_request_id": "retained-submission",
            "restore_guard": {"observation_token": "retained-token",
                "active_request_id": null, "owners": {}},
            "execution_state": {"state": "available", "execution": {
                "request": request, "binding": {}, "restore_intent": "explicit"
            }}
        }),
    );
    let (session, segment) = write_v4_session(&aggregate);
    let mut metadata = session_store::WorkerMetadata::new(
        format!("worker-runtime-{worker}"),
        Some(session_store::WorkerActiveSegmentRef::active_segment(
            session, segment,
        )),
    );
    metadata.workspace_id = Some("workspace-a".into());
    write_json(&aggregate.join("metadata.json"), &metadata);
    (worker, session, segment)
}

fn runtime_fixture(root: &Path, schema: u32) -> (WorkerId, SessionId, SegmentId) {
    fs::create_dir_all(root.join("workers")).unwrap();
    let fenced_worker = WorkerId::now_v7();
    write_json(
        &root.join("runtime.json"),
        &json!({
            "schema_version": schema, "display_name": "retained Runtime",
            "backend": "fs_store", "status": "running", "next_diagnostic_id": 1,
            "config_bundles": {}, "workspace_owners": {"workspace-a": "owner-a"},
            "diagnostics": [], "subject_releases": [],
            "subject_create_recovery_fences": {fenced_worker.to_string(): {
                "request": {"workspace_id": "workspace-a", "worker_id": fenced_worker,
                    "create_fingerprint": "retained-create", "operation_id": "retained-recovery"},
                "response": null
            }}
        }),
    );
    stopped_worker(root, schema)
}

// Include every file, not just schema markers: failures must preserve original
// history, metadata, archive receipts, checksums and retention evidence together.
fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn assert_generic(root: &Path, session: SessionId, segment: SegmentId) {
    use session_store::{InputCompactionUse, InputEvidenceUse, InputObservationUse};
    let store = WorkerSessionStore::open_read_only(root).unwrap();
    let entries = store.read_all_read_only(session, segment).unwrap();
    assert_eq!(entries.len(), 3);
    let originals: Vec<Value> = V4_SYSTEM_INPUTS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (log, old) in entries[1..].iter().zip(originals) {
        let LogEntry::AnnotatedSystemItem { entry, .. } = log else {
            panic!("lost System record: {log:?}");
        };
        let current = serde_json::to_value(log).unwrap();
        assert_eq!(current["ts"], old["ts"]);
        for field in ["entry_id", "origin", "derivation"] {
            assert_eq!(
                current["entry"]["metadata"][field],
                old["entry"]["metadata"][field]
            );
        }
        assert_eq!(current["entry"]["item"]["kind"], "input");
        assert_eq!(
            entry.item.history_text(),
            old["entry"]["item"]["body"].as_str().unwrap()
        );
        let input = entry.metadata.system_input.as_ref().unwrap();
        let usage = input.usage();
        assert_eq!(usage.recording_material, InputEvidenceUse::Exclude);
        assert_eq!(usage.memory_evidence, InputEvidenceUse::Exclude);
        assert_eq!(usage.observation, InputObservationUse::Hidden);
        let kind = old["entry"]["item"]["kind"].as_str().unwrap();
        let schema = if kind == "journal_startup" {
            assert_eq!(usage.compaction, InputCompactionUse::ReferenceInput);
            assert_eq!(current["extensions"], old["extensions"]);
            "journal:startup"
        } else {
            assert_eq!(usage.compaction, InputCompactionUse::Retain);
            "subject:behavior"
        };
        let payload = input.feature_payload().unwrap();
        assert_eq!(payload.schema(), schema);
        assert_eq!(payload.version(), 1);
        let mut expected_payload = old["entry"]["item"].clone();
        expected_payload.as_object_mut().unwrap().remove("kind");
        expected_payload.as_object_mut().unwrap().remove("body");
        assert_eq!(
            payload.decode::<Value>(schema, 1).unwrap(),
            expected_payload
        );
        let public = serde_json::to_value(input.public_item().unwrap()).unwrap();
        assert_eq!(public["kind"], kind);
        assert_eq!(public["body"], old["entry"]["item"]["body"]);
        assert_eq!(public["presentation"], json!({"kind": "context"}));
    }
}

fn completed_v4_archive(
    root: &Path,
    schema: u32,
) -> (
    SessionId,
    SegmentId,
    WorkerRetentionExecutionRequest,
    WorkerRetentionExecutionResult,
) {
    let (worker, session, segment) = stopped_worker(root, schema);
    let provider = FsWorkerRetentionProvider::new(root);
    let request = request(worker, SessionDisposition::Archive);
    // Frozen old-runtime receipt: ingestion and removal completed before the
    // current strict decoder existed. Do not ask the v5 reader to mint it.
    let manifest =
        commit_session_archive(&provider, &request, &provider.worker_dir(worker)).unwrap();
    let completed = WorkerRetentionExecutionResult {
        operation_id: request.operation_id.clone(),
        input_fingerprint: request.input_fingerprint.clone(),
        worker_id: worker,
        session_disposition: SessionDisposition::Archive,
        diagnostics_disposition: DiagnosticsDisposition::Purge,
        archive: Some(manifest),
        source_removed: true,
        diagnostics_retained: false,
    };
    write_json(
        &provider.operation_path(&request.operation_id).unwrap(),
        &RetentionOperationReceipt {
            schema_version: OPERATION_SCHEMA_VERSION,
            request: request.clone(),
            result: completed.clone(),
        },
    );
    fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
    (session, segment, request, completed)
}

#[test]
fn runtime_v11_and_current_route_v4_stopped_orphan_archive_through_staging() {
    for runtime_schema in [11, 12] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let (worker, session, segment) = runtime_fixture(&root, runtime_schema);
        let orphan = root
            .join("orphaned-workers")
            .join(WorkerId::now_v7().to_string());
        let (orphan_session, orphan_segment) = write_v4_session(&orphan);
        // Include an unregistered tree still under workers, not just an
        // already-archived orphan. Both Runtime source schemas must fence it
        // out of the live catalog without discarding its Session.
        let loose_id = WorkerId::now_v7();
        let loose = root.join("workers").join(loose_id.to_string());
        let (loose_session, loose_segment) = write_v4_session(&loose);
        let retained_loose = root.join("orphaned-workers").join(loose_id.to_string());
        let (archived_session, archived_segment, request, completed) =
            completed_v4_archive(&root, runtime_schema);
        let provider = FsWorkerRetentionProvider::new(&root);
        let archive = provider.archive_dir("archive-a").unwrap();
        // Also cover an archive which already has a local storage attestation
        // from an earlier schema migration. Keep ingestion bytes immutable;
        // harmless marker whitespace makes current storage differ from them.
        if runtime_schema == 12 {
            let marker = archive.join("session/session.json");
            let mut bytes = fs::read(&marker).unwrap();
            bytes.push(b'\n');
            fs::write(marker, bytes).unwrap();
            let ingestion = completed.archive.clone().unwrap();
            let (checksum_sha256, bytes, file_count) = checksum_files(
                &collect_files(&archive.join("session"), "test").unwrap(),
                "test",
            )
            .unwrap();
            write_json(
                &archive.join(ARCHIVE_STORAGE_FILE),
                &MigratedArchiveStorage {
                    schema_version: 1,
                    ingestion,
                    checksum_sha256,
                    bytes,
                    file_count,
                },
            );
        }
        let storage_before = fs::read(archive.join(ARCHIVE_STORAGE_FILE)).ok();
        let manifest_before = fs::read(archive.join("manifest.json")).unwrap();
        let receipt_path = provider.operation_path(&request.operation_id).unwrap();
        let receipt_before = fs::read(&receipt_path).unwrap();
        let stopped = root.join("workers").join(worker.to_string());
        assert!(WorkerSessionStore::open_read_only(stopped.join("session")).is_err());
        // Current decode itself must not be made permissive to fix routing.
        for line in V4_SYSTEM_INPUTS.lines() {
            assert!(serde_json::from_str::<LogEntry>(line).is_err());
        }
        let before = tree_bytes(&root);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        let plan = FsRuntimeStore::migration_plan(&options).unwrap();
        assert!(plan.migration_required);
        assert_eq!(plan.archived_orphan_worker_count, 1);
        assert_eq!(plan.current_schema_version, runtime_schema);
        assert_eq!(tree_bytes(&root), before, "preflight mutated source");
        // Exercise normal startup, not just the private migration helper.
        let opened = FsRuntimeStore::open_or_create(root.clone(), "runtime-a").unwrap();
        assert!(
            matches!(
                FsRuntimeStore::migrate(&options),
                Err(RuntimeError::RuntimeStoreAlreadyOpen { .. })
            ),
            "owner lock not fenced"
        );
        drop(opened);
        assert_generic(&stopped.join("session"), session, segment);
        assert_generic(&orphan.join("session"), orphan_session, orphan_segment);
        assert!(!loose.exists());
        assert_generic(
            &retained_loose.join("session"),
            loose_session,
            loose_segment,
        );
        assert_generic(&archive.join("session"), archived_session, archived_segment);
        assert_eq!(
            fs::read(archive.join("manifest.json")).unwrap(),
            manifest_before
        );
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_before);
        let (manifest, archive_session) = provider.session_archive("archive-a").unwrap();
        assert_eq!(completed.archive.as_ref(), Some(&manifest));
        assert_eq!(provider.execute(&request).unwrap(), completed);
        if let Some(storage_before) = storage_before {
            assert_ne!(
                fs::read(archive.join(ARCHIVE_STORAGE_FILE)).unwrap(),
                storage_before
            );
        }
        let storage: MigratedArchiveStorage =
            read_json(&archive.join(ARCHIVE_STORAGE_FILE), "test").unwrap();
        assert_eq!(storage.ingestion, manifest);
        let (checksum, bytes, count) =
            checksum_files(&collect_files(&archive_session, "test").unwrap(), "test").unwrap();
        assert_eq!(
            (storage.checksum_sha256, storage.bytes, storage.file_count),
            (checksum, bytes, count)
        );
        if runtime_schema == 12 {
            for name in [
                PathBuf::from("runtime.json"),
                PathBuf::from(format!("workers/{worker}/worker.json")),
                PathBuf::from(format!("workers/{worker}/metadata.json")),
            ] {
                assert_eq!(fs::read(root.join(&name)).unwrap(), before[&name]);
            }
        }
        let once = tree_bytes(&root);
        assert!(
            !FsRuntimeStore::migrate(&options)
                .unwrap()
                .migration_required
        );
        drop(FsRuntimeStore::open_or_create(root.clone(), "runtime-a").unwrap());
        assert_eq!(tree_bytes(&root), once, "retry rewrote migrated evidence");
        assert!(!temp.path().join(".runtime.schema-v12-staging").exists());
        assert!(!temp.path().join(".runtime.pre-schema-v12-backup").exists());
        // session-storage, not the immutable receipt's old checksum, attests
        // current bytes. Tampering with those bytes must remain detectable.
        let path = archive_session
            .join("segments")
            .join(format!("{archived_segment}.jsonl"));
        fs::write(path, b"tampered\n").unwrap();
        assert!(provider.session_archive("archive-a").is_err());
    }
}

#[test]
fn runtime_v5_archive_failure_discards_staging_without_losing_originals_and_retry_converges() {
    for runtime_schema in [11, 12] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let (worker, session, segment) = runtime_fixture(&root, runtime_schema);
        let (archived_session, archived_segment, request, completed) =
            completed_v4_archive(&root, runtime_schema);
        let provider = FsWorkerRetentionProvider::new(&root);
        let archive = provider.archive_dir("archive-a").unwrap();
        let path = archive
            .join("session/segments")
            .join(format!("{archived_segment}.jsonl"));
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"corrupt archive\n").unwrap();
        let before = tree_bytes(&root);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        assert!(
            FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        // workers are visited before archives: earlier successful staged
        // conversion must not leak into the original tree on this later error.
        assert!(FsRuntimeStore::open_or_create(root.clone(), "runtime-a").is_err());
        assert_eq!(tree_bytes(&root), before);
        assert!(!archive.join(ARCHIVE_STORAGE_FILE).exists());
        fs::write(&path, original).unwrap();
        FsRuntimeStore::migrate(&options).unwrap();
        assert_generic(
            &root
                .join("workers")
                .join(worker.to_string())
                .join("session"),
            session,
            segment,
        );
        assert_generic(&archive.join("session"), archived_session, archived_segment);
        assert_eq!(provider.execute(&request).unwrap(), completed);
    }
}

#[test]
fn runtime_v5_typed_legacy_payload_failure_keeps_valid_archive_evidence_on_retries() {
    for (from, to) in [
        (
            "\"snapshot_sha256\":\"snapshot-digest\"",
            "\"snapshot_sha256\":42",
        ),
        ("\"behavior_md\":\"exact behavior\"", "\"behavior_md\":[]"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        runtime_fixture(&root, 12);
        let (worker, _, segment) = stopped_worker(&root, 12);
        let provider = FsWorkerRetentionProvider::new(&root);
        let path = provider
            .worker_dir(worker)
            .join("session/segments")
            .join(format!("{segment}.jsonl"));
        let original = fs::read_to_string(&path).unwrap();
        assert!(original.contains(from));
        fs::write(&path, original.replace(from, to)).unwrap();
        let request = request(worker, SessionDisposition::Archive);
        let manifest =
            commit_session_archive(&provider, &request, &provider.worker_dir(worker)).unwrap();
        fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
        let archive = provider.archive_dir("archive-a").unwrap();
        // This archive checksum is valid; failure must come from the legacy
        // typed decoder, not from deliberately broken ingestion evidence.
        validate_existing_archive(&archive, &manifest, false).unwrap();
        let before = tree_bytes(&root);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        for _ in 0..2 {
            let error = FsRuntimeStore::migrate(&options).unwrap_err();
            let RuntimeError::StoreCorrupt {
                operation, path, ..
            } = error
            else {
                panic!("unexpected failure: {error}");
            };
            assert_eq!(operation, "migrate Session schema");
            assert!(path.starts_with(temp.path().join(".runtime.schema-v12-staging")));
            assert_eq!(tree_bytes(&root), before);
            validate_existing_archive(&archive, &manifest, false).unwrap();
        }
    }
}

#[test]
fn runtime_v5_probe_and_staging_skip_erased_originals() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    let (worker, live_session, live_segment) = runtime_fixture(&root, 12);
    let erased = root
        .join("orphaned-workers")
        .join(WorkerId::now_v7().to_string());
    let (session, segment) = write_v4_session(&erased);
    runtime_session_retention(&root)
        .delete(
            session,
            session_store::retention::SessionDeletionIntent::ErasePermanently,
            || Ok(()),
        )
        .unwrap();
    fs::write(
        erased
            .join("session/segments")
            .join(format!("{segment}.jsonl")),
        b"fenced malformed JSON\n",
    )
    .unwrap();
    let before = tree_bytes(&erased);
    let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
    assert!(
        FsRuntimeStore::migration_plan(&options)
            .unwrap()
            .migration_required
    );
    FsRuntimeStore::migrate(&options).unwrap();
    assert_generic(
        &root
            .join("workers")
            .join(worker.to_string())
            .join("session"),
        live_session,
        live_segment,
    );
    assert_eq!(tree_bytes(&erased), before);
    assert!(
        !FsRuntimeStore::migration_plan(&options)
            .unwrap()
            .migration_required
    );
    assert!(matches!(
        runtime_session_retention(&root).ensure_live(session),
        Err(session_store::StoreError::SessionErased(_))
    ));
}

#[test]
fn runtime_v5_rejects_broken_or_future_source_without_rewriting_evidence() {
    for corruption in ["broken-json", "future-schema"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let (worker, session, segment) = runtime_fixture(&root, 12);
        let session_root = root
            .join("workers")
            .join(worker.to_string())
            .join("session");
        if corruption == "broken-json" {
            fs::write(
                session_root
                    .join("segments")
                    .join(format!("{segment}.jsonl")),
                b"{not valid JSON\n",
            )
            .unwrap();
        } else {
            write_json(
                &session_root.join("session.json"),
                &json!({
                    "schema_version": 999, "session_id": session,
                }),
            );
        }
        let before = tree_bytes(&root);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        assert!(
            FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        assert_eq!(tree_bytes(&root), before);
        assert!(FsRuntimeStore::migrate(&options).is_err());
        assert_eq!(tree_bytes(&root), before);
        assert!(!temp.path().join(".runtime.schema-v12-staging").exists());
        assert!(!temp.path().join(".runtime.pre-schema-v12-backup").exists());
    }
}

#[cfg(unix)]
#[test]
fn current_runtime_session_migration_still_rejects_live_worker_sockets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    let (worker, _, _) = runtime_fixture(&root, 12);
    let before = tree_bytes(&root);
    let runs = root.join("workers").join(worker.to_string()).join("runs");
    fs::create_dir_all(runs.join("1")).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(runs.join("1/worker.sock")).unwrap();
    let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
    let error = FsRuntimeStore::migrate(&options).unwrap_err();
    assert!(error.to_string().contains("active Worker socket"));
    drop(listener);
    fs::remove_dir_all(runs).unwrap();
    assert_eq!(tree_bytes(&root), before);
    assert!(!temp.path().join(".runtime.schema-v12-staging").exists());
}
