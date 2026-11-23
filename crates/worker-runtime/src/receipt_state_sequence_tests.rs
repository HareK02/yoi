// Exercise the real owner-locked store migration and canonical reader. Historical
// snapshots remain receipt evidence, never the current execution's observation.

fn receipt_sequence_document(worker: &PersistedWorkerRecord, schema: u32) -> serde_json::Value {
    let mut document = serde_json::to_value(WorkerAggregateRecord::from_persisted(worker)).unwrap();
    document["schema_version"] = serde_json::json!(schema);
    let snapshot = serde_json::json!({
        "last_command_id": 37,
        "last_finished_submission_request_id": "retained-finished-input",
        "state": {"kind": "idle"}
    });
    document["restore_guard"]["stop_owners"]["retained-stop"] = serde_json::json!({
        "operation_id": WorkerLifecycleOperationId::new(),
        "request": {"reason": "original reason", "stop_intent": {
            "request_id": "retained-stop",
            "expected_observation_token": document["restore_guard"]["observation_token"]
        }},
        "receipt": {
            "worker_ref": worker.worker_ref, "status": "stopped", "lifecycle": "stopped",
            "replayed": false, "worker_state": snapshot,
            "command": {"kind": "acknowledged", "acknowledgement": {
                "command_id": 37, "command": "shutdown", "disposition": "accepted",
                "state": snapshot
            }}
        }
    });
    document
}

fn install_receipt_sequence_document(
    store: &FsRuntimeStore,
    path: &Path,
    document: &serde_json::Value,
) {
    fs::write(path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    write_empty_schema_v8_store(&store.root);
    let runtime_path = store.root.join(RUNTIME_FILE);
    let mut runtime: serde_json::Value = read_json(&runtime_path, "read fixture").unwrap();
    runtime["schema_version"] = document["schema_version"].clone();
    fs::write(runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
}

#[test]
fn schema18_receipt_sequences_migrate_without_reapplying_lifecycle_conversion() {
    for sequence in [None, Some(91)] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let root = store.root.clone();
        let mut document = receipt_sequence_document(&worker, LIFECYCLE_SCHEMA_VERSION);
        let receipt = &mut document["restore_guard"]["stop_owners"]["retained-stop"]["receipt"];
        if let Some(sequence) = sequence {
            receipt["worker_state"]["state_sequence"] = serde_json::json!(sequence);
            receipt["command"]["acknowledgement"]["state"]["state_sequence"] =
                serde_json::json!(sequence);
        }
        let mut expected = document.clone();
        expected["schema_version"] = serde_json::json!(SCHEMA_VERSION);
        let receipt = &mut expected["restore_guard"]["stop_owners"]["retained-stop"]["receipt"];
        receipt["worker_state"]["state_sequence"] = serde_json::json!(sequence.unwrap_or(0));
        receipt["command"]["acknowledgement"]["state"]["state_sequence"] =
            serde_json::json!(sequence.unwrap_or(0));
        install_receipt_sequence_document(&store, &path, &document);
        let before = fs::read(&path).unwrap();
        let runtime_before = fs::read(root.join(RUNTIME_FILE)).unwrap();
        drop(store);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("test-runtime");
        assert!(
            FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime_before);
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
        let migrated: serde_json::Value = read_json(&path, "read migrated receipts").unwrap();
        assert_eq!(migrated, expected);
        opened.store.write_worker_record(&saved).unwrap();
        let committed = fs::read(&path).unwrap();
        drop(opened);
        let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        reopened
            .store
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), committed);
    }
}

#[test]
fn legacy_receipt_sequence_migration_rejects_invalid_evidence_without_writing() {
    for pointer in ["/worker_state", "/command/acknowledgement/state"] {
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(-1),
            serde_json::json!("0"),
        ] {
            let (_temp, store, worker, path) = canonical_worker_store();
            let root = store.root.clone();
            let mut document = receipt_sequence_document(&worker, LIFECYCLE_SCHEMA_VERSION);
            let receipt = &mut document["restore_guard"]["stop_owners"]["retained-stop"]["receipt"];
            receipt.pointer_mut(pointer).unwrap()["state_sequence"] = invalid;
            install_receipt_sequence_document(&store, &path, &document);
            let before = fs::read(&path).unwrap();
            let runtime_before = fs::read(root.join(RUNTIME_FILE)).unwrap();
            drop(store);
            assert!(FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime_before);
        }
    }
}

#[test]
fn current_receipt_decoder_requires_sequence_without_legacy_defaults() {
    let (_temp, store, worker, path) = canonical_worker_store();
    let document = receipt_sequence_document(&worker, SCHEMA_VERSION);
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let error = store.read_worker_record(&worker.worker_ref).unwrap_err();
    assert!(error.to_string().contains("state_sequence"));
    assert_eq!(fs::read(&path).unwrap(), before);
}
