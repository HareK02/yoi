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
        let input = entry.metadata.input_metadata.as_ref().unwrap();
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
fn corrupt_archive_does_not_block_startup_or_healthy_session_migration_and_repair_converges() {
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
        let (healthy_worker, healthy_session, healthy_segment) =
            stopped_worker(&root, runtime_schema);
        let mut healthy_request =
            super::tests::request(healthy_worker, SessionDisposition::Archive);
        healthy_request.operation_id = "healthy-operation".into();
        healthy_request.archive_id = Some("healthy-archive".into());
        let healthy_manifest = commit_session_archive(
            &provider,
            &healthy_request,
            &provider.worker_dir(healthy_worker),
        )
        .unwrap();
        fs::remove_dir_all(provider.worker_dir(healthy_worker)).unwrap();
        let before = tree_bytes(&archive);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        assert!(
            FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        drop(FsRuntimeStore::open_or_create(root.clone(), "runtime-a").unwrap());
        assert_eq!(tree_bytes(&archive), before);
        assert!(provider.session_archive("archive-a").is_err());
        let (manifest, healthy_root) = provider.session_archive("healthy-archive").unwrap();
        assert_eq!(manifest, healthy_manifest);
        assert_generic(&healthy_root, healthy_session, healthy_segment);
        assert!(!archive.join(ARCHIVE_STORAGE_FILE).exists());
        assert_generic(
            &root
                .join("workers")
                .join(worker.to_string())
                .join("session"),
            session,
            segment,
        );
        assert!(
            !FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        drop(FsRuntimeStore::open_or_create(root.clone(), "runtime-a").unwrap());
        assert_eq!(tree_bytes(&archive), before);
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
fn unmigratable_archive_preserves_evidence_across_startup_and_reopen() {
    for (field, value) in [
        ("snapshot_sha256", json!(42)),
        ("behavior_md", json!([])),
        ("snapshot", json!({"large": "x".repeat(128 * 1024 + 1)})),
        ("body", json!("x".repeat(256 * 1024 + 1))),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let (live_worker, live_session, live_segment) = runtime_fixture(&root, 12);
        let (worker, _, segment) = stopped_worker(&root, 12);
        let provider = FsWorkerRetentionProvider::new(&root);
        let path = provider
            .worker_dir(worker)
            .join("session/segments")
            .join(format!("{segment}.jsonl"));
        let source = fs::read_to_string(&path).unwrap();
        let mut replaced = false;
        let mut altered = String::new();
        for line in source.lines() {
            let mut entry: Value = serde_json::from_str(line).unwrap();
            if let Some(item) = entry.pointer_mut("/entry/item")
                && item.get(field).is_some()
            {
                item[field] = value.clone();
                replaced = true;
            }
            altered.push_str(&serde_json::to_string(&entry).unwrap());
            altered.push('\n');
        }
        assert!(replaced);
        fs::write(&path, altered).unwrap();
        let request = request(worker, SessionDisposition::Archive);
        let manifest =
            commit_session_archive(&provider, &request, &provider.worker_dir(worker)).unwrap();
        fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
        let archive = provider.archive_dir("archive-a").unwrap();
        validate_existing_archive(&archive, &manifest, false).unwrap();
        let before = tree_bytes(&archive);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        for _ in 0..2 {
            let runtime = crate::Runtime::with_fs_store(options.clone()).unwrap();
            assert_eq!(runtime.summary().unwrap().worker_count, 1);
            runtime.bind_runtime_identity("runtime-a").unwrap();
            let search = runtime_api::SessionPublicSearchRequest {
                workspace_id: "workspace-a".into(),
                source: runtime_api::SessionPublicSource::Archived {
                    archive_id: "archive-a".into(),
                },
                expected_session_id: manifest.source_session_id.clone(),
                expected_generation: None,
                query: None,
                kind: None,
                tool_name: None,
                tool_part: runtime_api::SessionPublicToolPart::Both,
                scan_cursor: None,
                limit: 10,
                max_scan_bytes: 1024 * 1024,
                max_segments: 10,
                max_entries: 100,
            };
            let availability = runtime
                .session_public_search_scoped(
                    &crate::runtime::RuntimeWorkspaceScope::new("workspace-a", "owner-a"),
                    &search,
                )
                .unwrap();
            assert!(matches!(
                availability,
                runtime_api::SessionPublicSearchAvailability::Unavailable {
                    reason: runtime_api::SessionPublicUnavailableReason::ArchiveIncomplete,
                    ..
                }
            ));
            drop(runtime);
            assert_eq!(tree_bytes(&archive), before);
            validate_existing_archive(&archive, &manifest, false).unwrap();
            assert!(matches!(
                provider.session_archive("archive-a"),
                Err(RuntimeError::StoreCorrupt { .. })
            ));
            assert!(
                !FsRuntimeStore::migration_plan(&options)
                    .unwrap()
                    .migration_required
            );
            assert_generic(
                &provider.worker_dir(live_worker).join("session"),
                live_session,
                live_segment,
            );
        }
    }
}

#[test]
fn unreadable_archive_metadata_does_not_block_archive_only_runtime_startup() {
    for (path, field, value) in [
        ("session/session.json", None, Value::Null),
        ("manifest.json", None, Value::Null),
        (ARCHIVE_STORAGE_FILE, None, Value::Null),
        ("session/session.json", Some("schema_version"), json!(999)),
        ("manifest.json", Some("schema_version"), json!(999)),
        (
            "manifest.json",
            Some("archive_id"),
            json!("another-archive"),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let (worker, _, _) = runtime_fixture(&root, 12);
        completed_v4_archive(&root, 12);
        let provider = FsWorkerRetentionProvider::new(&root);
        fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
        let archive = provider.archive_dir("archive-a").unwrap();
        if let Some(field) = field {
            let mut record: Value = read_json(&archive.join(path), "test").unwrap();
            record[field] = value;
            write_json(&archive.join(path), &record);
        } else {
            fs::write(archive.join(path), b"{broken\n").unwrap();
        }
        let before = tree_bytes(&archive);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
        for _ in 0..2 {
            assert!(
                !FsRuntimeStore::migration_plan(&options)
                    .unwrap()
                    .migration_required
            );
            let runtime = crate::Runtime::with_fs_store(options.clone()).unwrap();
            assert_eq!(runtime.summary().unwrap().worker_count, 0);
            drop(runtime);
            assert!(provider.session_archive("archive-a").is_err());
            assert_eq!(tree_bytes(&archive), before);
        }
    }
}

#[test]
fn archive_isolation_does_not_bypass_corrupt_erasure_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    runtime_fixture(&root, 12);
    let (session, _, _, _) = completed_v4_archive(&root, 12);
    let ledger = root.join("retention/session-sources");
    fs::create_dir_all(&ledger).unwrap();
    fs::write(ledger.join(format!("{session}.json")), b"{broken\n").unwrap();
    let before = tree_bytes(&root);
    let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
    assert!(FsRuntimeStore::migration_plan(&options).is_err());
    assert!(crate::Runtime::with_fs_store(options).is_err());
    assert_eq!(tree_bytes(&root), before);
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
    // The same TempDir owns the relative bind/connect reference. No cwd change
    // or alias is needed, and the real live-socket migration refusal stays tested.
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = Path::new(temp.path().file_name().unwrap()).join("runtime");
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

#[test]
fn current_runtime_upgrades_v5_role_metadata_through_staging_without_reserializing_owner_evidence()
{
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    let (worker, session, segment) = runtime_fixture(&root, 12);
    let aggregate = root.join("workers").join(worker.to_string());
    let path = aggregate
        .join("session/segments")
        .join(format!("{segment}.jsonl"));
    let old = fs::read_to_string(&path).unwrap();
    let header = old.lines().next().unwrap();
    let source = include_str!(
        "../../../session-store/src/legacy_session_log/tests/fixtures/v5-role-metadata.jsonl"
    );
    fs::write(&path, format!("{header}\n{source}")).unwrap();
    write_json(
        &aggregate.join("session/session.json"),
        &json!({"schema_version":5, "session_id": session}),
    );
    let owner_before = fs::read(root.join("runtime.json")).unwrap();
    let worker_before = fs::read(aggregate.join("worker.json")).unwrap();
    let metadata_before = fs::read(aggregate.join("metadata.json")).unwrap();
    assert!(WorkerSessionStore::open_read_only(aggregate.join("session")).is_err());
    let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("runtime-a");
    assert!(
        FsRuntimeStore::migration_plan(&options)
            .unwrap()
            .migration_required
    );
    FsRuntimeStore::migrate(&options).unwrap();
    let store = WorkerSessionStore::open_read_only(aggregate.join("session")).unwrap();
    let entries = store.read_all_read_only(session, segment).unwrap();
    assert_eq!(entries.len(), 6);
    for (current, old) in entries[1..].iter().zip(source.lines()) {
        let current = serde_json::to_value(current).unwrap();
        let old: Value = serde_json::from_str(old).unwrap();
        assert_eq!(current["ts"], old["ts"]);
        assert_eq!(current["entry"]["item"], old["entry"]["item"]);
        for field in ["entry_id", "origin", "derivation"] {
            assert_eq!(
                current["entry"]["metadata"][field],
                old["entry"]["metadata"][field]
            );
        }
        if current["entry"]["metadata"]["origin"]["kind"] == "derived_summary" {
            assert_eq!(
                current["entry"]["metadata"]["input_metadata"]["usage"]["recording_material"],
                "exclude"
            );
            assert_eq!(
                current["entry"]["metadata"]["input_metadata"]["usage"]["memory_evidence"],
                "exclude"
            );
        }
    }
    assert_eq!(fs::read(root.join("runtime.json")).unwrap(), owner_before);
    assert_eq!(
        fs::read(aggregate.join("worker.json")).unwrap(),
        worker_before
    );
    assert_eq!(
        fs::read(aggregate.join("metadata.json")).unwrap(),
        metadata_before
    );
    let once = tree_bytes(&aggregate.join("session"));
    assert!(
        !FsRuntimeStore::migration_plan(&options)
            .unwrap()
            .migration_required
    );
    FsRuntimeStore::migrate(&options).unwrap();
    assert_eq!(tree_bytes(&aggregate.join("session")), once);
}
