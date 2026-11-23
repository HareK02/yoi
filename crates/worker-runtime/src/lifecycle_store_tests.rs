// These tests cross the released JSON decoder, real staged migration, canonical
// writer and reopen. They do not substitute a mock for persistence or recovery.

fn install_schema17_document(store: &FsRuntimeStore, path: &Path, document: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    write_empty_schema_v8_store(&store.root);
    let runtime_path = store.root.join(RUNTIME_FILE);
    let mut runtime: serde_json::Value = read_json(&runtime_path, "read fixture").unwrap();
    runtime["schema_version"] = serde_json::json!(ATTACHMENT_AUTHORITY_SCHEMA_VERSION);
    fs::write(runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
}

fn schema17_document(worker: &PersistedWorkerRecord) -> serde_json::Value {
    let mut document = legacy_lifecycle_document(
        serde_json::to_value(WorkerAggregateRecord::from_persisted(worker)).unwrap(),
    );
    document["schema_version"] = serde_json::json!(ATTACHMENT_AUTHORITY_SCHEMA_VERSION);
    document
}

fn legacy_pending_restore(document: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "operation_id": WorkerLifecycleOperationId::new(), "mode": "explicit",
        "commit": "not_committed", "source_repair_possible": false,
        "request": document["execution_state"]["execution"]["request"],
        "binding": document["execution_state"]["execution"]["binding"],
        "last_settled_status": document["status"]
    })
}

#[test]
fn schema17_lifecycle_roundtrip_preserves_empty_authority_history_and_unresolved_create() {
    for status in ["idle", "running", "paused", "stopped"] {
        for unavailable in [false, true] {
            let (_temp, store, worker, path) = canonical_worker_store();
            let root = store.root.clone();
            let mut document = schema17_document(&worker);
            document["status"] = serde_json::json!(status);
            if unavailable {
                document["execution_state"] = serde_json::json!({"state": "unavailable"});
            } else {
                document["execution_state"]["execution"]["restore_intent"] =
                    serde_json::json!(if status == "stopped" {
                        "explicit"
                    } else {
                        "automatic"
                    });
            }
            // Deliberately retained local information must not resurrect an empty
            // schema17 authority set during migration or any subsequent reopen.
            document["workdir_attachments"] = serde_json::json!([{
                "alias": "checkout", "working_directory": {"summary": {
                    "working_directory_id": "retained-local", "repository_id": "repo",
                    "materializer_kind": "runtime_git_clone", "status": "active"
                }}
            }]);
            document["logical_workdir_attachments"] = serde_json::json!([]);
            document["last_finished_submission_request_id"] =
                serde_json::json!("accepted-input-fence");
            document["workdir_attachments_guard"]["owners"]["revoke-old"] = serde_json::json!({
                "workspace_id": null,
                "receipt": {
                    "worker_ref": worker.worker_ref,
                    "request": {"request_id": "revoke-old", "expected_generation": "previous-authority", "logical_workdir_attachments": []},
                    "resulting_generation": document["workdir_attachments_guard"]["generation"]
                }
            });
            let guard = document["workdir_attachments_guard"].clone();
            let restore_guard = document["restore_guard"].clone();
            install_schema17_document(&store, &path, &document);
            let before = fs::read(&path).unwrap();
            drop(store);
            let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("test-runtime");
            assert!(
                FsRuntimeStore::migration_plan(&options)
                    .unwrap()
                    .migration_required
            );
            assert_eq!(fs::read(&path).unwrap(), before);
            let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
            let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
            let disk: serde_json::Value = read_json(&path, "read migrated lifecycle").unwrap();
            assert_eq!(
                disk["execution_state"]["state"],
                if status == "stopped" {
                    "stopped"
                } else if unavailable {
                    "reconciliation_required"
                } else {
                    "runnable"
                }
            );
            if unavailable && status != "stopped" {
                assert_eq!(disk["execution_state"]["execution"]["operation"], "create");
                assert!(disk["execution_state"]["execution"]["intent"]["operation_id"].is_null());
            }
            assert!(saved.logical_workdir_attachments.is_empty());
            assert_eq!(disk["workdir_attachments_guard"], guard);
            assert_eq!(disk["restore_guard"], restore_guard);
            assert_eq!(
                saved.last_finished_submission_request_id.as_deref(),
                Some("accepted-input-fence")
            );
            assert!(
                !serde_json::to_string(&disk)
                    .unwrap()
                    .contains("restore_intent")
            );
            opened.store.write_worker_record(&saved).unwrap();
            let committed = fs::read(&path).unwrap();
            drop(opened);
            let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
            let saved = reopened
                .store
                .read_worker_record(&worker.worker_ref)
                .unwrap();
            assert!(saved.logical_workdir_attachments.is_empty());
            assert_eq!(fs::read(&path).unwrap(), committed);
        }
    }
}

#[test]
fn schema17_pending_restore_and_stop_preserve_phase_owner_and_nonempty_authority() {
    for under_stop in [false, true] {
        for commit in ["not_committed", "committed", "legacy_unknown"] {
            let (_temp, store, worker, path) = canonical_worker_store();
            let root = store.root.clone();
            let mut document = schema17_document(&worker);
            document["workspace_id"] = serde_json::json!("workspace-a");
            document["execution_state"]["execution"]["request"]["workspace_api"] = serde_json::json!({
                "workspace_id": "workspace-a", "base_url": "https://workspace.invalid"
            });
            document["logical_workdir_attachments"] = serde_json::json!([{
                "alias": "remote", "working_directory_id": "logical-only",
                "capabilities": workdir::WorkdirSessionCapabilities::READ_ONLY
            }]);
            let authority = document["logical_workdir_attachments"].clone();
            let mut restore = legacy_pending_restore(&document);
            restore["commit"] = serde_json::json!(commit);
            let operation = if under_stop {
                serde_json::json!({"operation": "stop", "intent": {
                    "operation_id": WorkerLifecycleOperationId::new(),
                    "pending_restore": restore, "request": restore["request"],
                    "binding": {}, "restore_intent": "automatic",
                    "last_settled_status": document["status"]
                }})
            } else {
                serde_json::json!({"operation": "restore", "intent": restore})
            };
            document["execution_state"] =
                serde_json::json!({"state": "reconciliation_required", "execution": operation});
            let operation_id =
                document["execution_state"]["execution"]["intent"]["operation_id"].clone();
            let before_guard = document["restore_guard"].clone();
            install_schema17_document(&store, &path, &document);
            // Workspace ownership is a separate Runtime fact, not inferred from a
            // retained logical attachment or a lifecycle operation.
            let runtime_path = root.join(RUNTIME_FILE);
            let mut runtime: serde_json::Value = read_json(&runtime_path, "read fixture").unwrap();
            runtime["workspace_owners"] = serde_json::json!({"workspace-a": "owner-a"});
            fs::write(runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
            drop(store);
            let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
            let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
            let disk: serde_json::Value = read_json(&path, "read lifecycle").unwrap();
            assert_eq!(disk["logical_workdir_attachments"], authority);
            assert_eq!(disk["restore_guard"], before_guard);
            let intent = &disk["execution_state"]["execution"]["intent"];
            assert_eq!(intent["operation_id"], operation_id);
            assert_eq!(
                if under_stop {
                    &intent["pending_restore"]["commit"]
                } else {
                    &intent["commit"]
                },
                commit
            );
            if under_stop {
                assert_eq!(intent["progress"]["phase"], "awaiting_termination");
            }
            assert!(
                !serde_json::to_string(&disk)
                    .unwrap()
                    .contains("restore_intent")
            );
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
}

#[test]
fn schema17_guard_only_restore_admission_is_not_migrated_to_stable_stopped_or_runnable() {
    for (status, unavailable) in [("stopped", false), ("running", false), ("stopped", true)] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let root = store.root.clone();
        let mut document = schema17_document(&worker);
        document["status"] = serde_json::json!(status);
        document["execution_state"]["execution"]["restore_intent"] =
            serde_json::json!(if status == "stopped" {
                "explicit"
            } else {
                "automatic"
            });
        if unavailable {
            document["execution_state"] = serde_json::json!({"state": "unavailable"});
        }
        let operation_id = WorkerLifecycleOperationId::new();
        document["restore_guard"]["active_request_id"] = serde_json::json!("admitted");
        document["restore_guard"]["owners"]["admitted"] = serde_json::json!({
            "fingerprint": "retained-admission", "operation_id": operation_id,
            "request": {"request_id": "admitted", "expected_observation_token": document["restore_guard"]["observation_token"]},
            "preparation_applied": false, "execution_started": false, "receipt": null
        });
        let guard = document["restore_guard"].clone();
        install_schema17_document(&store, &path, &document);
        drop(store);
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
        let disk: serde_json::Value = read_json(&path, "read lifecycle").unwrap();
        assert_eq!(disk["execution_state"]["state"], "restore_admitted");
        assert_eq!(
            disk["execution_state"]["execution"]["operation_id"],
            operation_id.to_string()
        );
        assert_eq!(disk["restore_guard"], guard);
        opened.store.write_worker_record(&saved).unwrap();
        let bytes = fs::read(&path).unwrap();
        drop(opened);
        let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        reopened
            .store
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn schema17_invalid_old_combinations_fail_without_replacing_original_tree() {
    for fault in [
        "intent",
        "binding",
        "request",
        "nested-request",
        "active-owner",
    ] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let root = store.root.clone();
        let mut document = schema17_document(&worker);
        match fault {
            "intent" => {
                document["execution_state"]["execution"]["restore_intent"] =
                    serde_json::json!("explicit")
            }
            "binding" => {
                document["execution_state"]["execution"]["binding"] = serde_json::Value::Null
            }
            "request" => {
                document["execution_state"]["execution"]["request"]["worker_id"] =
                    serde_json::json!(WorkerId::now_v7())
            }
            "nested-request" => {
                let mut restore = legacy_pending_restore(&document);
                restore["request"]["worker_id"] = serde_json::json!(WorkerId::now_v7());
                document["execution_state"] = serde_json::json!({"state": "reconciliation_required", "execution": {"operation": "stop", "intent": {
                    "operation_id": WorkerLifecycleOperationId::new(), "request": document["execution_state"]["execution"]["request"],
                    "binding": {}, "restore_intent": "automatic", "last_settled_status": document["status"], "pending_restore": restore
                }}});
            }
            "active-owner" => {
                document["restore_guard"]["active_request_id"] = serde_json::json!("missing")
            }
            _ => unreachable!(),
        }
        install_schema17_document(&store, &path, &document);
        let bytes = fs::read(&path).unwrap();
        let runtime_bytes = fs::read(root.join(RUNTIME_FILE)).unwrap();
        drop(store);
        assert!(
            FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err(),
            "{fault}"
        );
        assert_eq!(fs::read(path).unwrap(), bytes, "{fault}");
        assert_eq!(
            fs::read(root.join(RUNTIME_FILE)).unwrap(),
            runtime_bytes,
            "{fault}"
        );
    }
}

#[test]
fn schema18_decoder_rejects_old_intent_and_unbound_runnable_stop() {
    for fault in ["intent", "unbound-stop", "not-started-bound"] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let mut document =
            serde_json::to_value(WorkerAggregateRecord::from_persisted(&worker)).unwrap();
        match fault {
            "intent" => {
                document["execution_state"]["execution"]["restore_intent"] =
                    serde_json::json!("automatic")
            }
            "unbound-stop" => {
                document["execution_state"] = serde_json::json!({"state": "reconciliation_required", "execution": {"operation": "stop", "intent": {
                    "operation_id": WorkerLifecycleOperationId::new(), "pending_restore": null,
                    "request": document["execution_state"]["execution"]["request"], "binding": null,
                    "last_settled_status": document["status"], "progress": {"phase": "awaiting_termination"}
                }}});
            }
            "not-started-bound" => {
                document["status"] = serde_json::json!("stopped");
                document["execution_state"]["state"] = serde_json::json!("not_started");
            }
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(
            store.read_worker_record(&worker.worker_ref).is_err(),
            "{fault}"
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn schema17_joined_stop_keeps_original_operation_and_shutdown_receipt_on_reopen() {
    let (_temp, store, worker, path) = canonical_worker_store();
    let root = store.root.clone();
    let mut document = schema17_document(&worker);
    let stop_id = WorkerLifecycleOperationId::new();
    let termination = crate::execution::WorkerExecutionTermination::joined_startup_candidate(
        worker.worker_ref.clone(),
        stop_id,
        uuid::Uuid::now_v7(),
    );
    document["restore_guard"]["execution_termination"] =
        serde_json::to_value(&termination).unwrap();
    document["execution_state"] = serde_json::json!({"state": "reconciliation_required", "execution": {"operation": "stop", "intent": {
        "operation_id": stop_id, "request": document["execution_state"]["execution"]["request"],
        "binding": {}, "last_settled_status": document["status"], "restore_intent": "automatic"
    }}});
    install_schema17_document(&store, &path, &document);
    drop(store);
    let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
    let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
    let disk: serde_json::Value = read_json(&path, "read joined Stop").unwrap();
    assert_eq!(
        disk["execution_state"]["execution"]["intent"]["progress"],
        serde_json::json!({
            "phase": "scope_terminated", "termination": termination
        })
    );
    opened.store.write_worker_record(&saved).unwrap();
    let bytes = fs::read(&path).unwrap();
    drop(opened);
    let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
    reopened
        .store
        .read_worker_record(&worker.worker_ref)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // Malformed current JSON cannot swap the joined execution while keeping the
    // older guard's evidence, even though both receipts address this Worker.
    let mut invalid = disk;
    invalid["execution_state"]["execution"]["intent"]["progress"]["termination"]["execution_id"] =
        serde_json::json!(uuid::Uuid::now_v7());
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(
        reopened
            .store
            .read_worker_record(&worker.worker_ref)
            .is_err()
    );
}

#[test]
fn schema17_historical_receipts_survive_without_becoming_current_execution() {
    let (_temp, store, worker, path) = canonical_worker_store();
    let root = store.root.clone();
    let mut document = schema17_document(&worker);
    let token = document["restore_guard"]["observation_token"].clone();
    let mut old_worker = document.clone();
    for key in [
        "schema_version",
        "execution_state",
        "restore_guard",
        "workdir_attachments_guard",
        "last_finished_submission_request_id",
    ] {
        old_worker.as_object_mut().unwrap().remove(key);
    }
    old_worker["execution_metadata_available"] = serde_json::json!(true);
    old_worker["restore_observation_token"] = token.clone();
    let restore_id = WorkerLifecycleOperationId::new();
    let restore_receipt = serde_json::json!({"state": "accepted", "worker": old_worker, "reason_code": null, "message": null});
    document["restore_guard"]["owners"]["old-restore"] = serde_json::json!({
        "fingerprint": "old-restore-input", "operation_id": restore_id,
        "request": {"request_id": "old-restore", "expected_observation_token": token},
        "preparation_applied": true, "execution_started": true, "receipt": restore_receipt
    });
    let stop_id = WorkerLifecycleOperationId::new();
    let stop_receipt = serde_json::json!({"worker_ref": worker.worker_ref, "status": "stopped", "worker_state": null, "replayed": false});
    document["restore_guard"]["stop_owners"]["old-stop"] = serde_json::json!({
        "request": {"reason": "original-stop", "stop_intent": {"request_id": "old-stop", "expected_observation_token": token}},
        "operation_id": stop_id, "receipt": stop_receipt
    });
    // Current state is explicitly stopped, not the Active historical Restore.
    document["status"] = serde_json::json!("stopped");
    document["execution_state"]["execution"]["restore_intent"] = serde_json::json!("explicit");
    install_schema17_document(&store, &path, &document);
    drop(store);
    let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
    let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
    let disk: serde_json::Value = read_json(&path, "read historical receipts").unwrap();
    assert_eq!(disk["execution_state"]["state"], "stopped");
    assert_eq!(disk["restore_guard"]["observation_token"], token);
    let mut restored_receipt = disk["restore_guard"]["owners"]["old-restore"]["receipt"].clone();
    assert_eq!(restored_receipt["worker"]["lifecycle"], "active");
    restored_receipt["worker"]
        .as_object_mut()
        .unwrap()
        .remove("lifecycle");
    restored_receipt["worker"]
        .as_object_mut()
        .unwrap()
        .remove("execution_reconstructed");
    // Default-valued optional fields can be re-serialized; compare the actual
    // original request identity, historical status, profile and accepted result.
    assert_eq!(restored_receipt["state"], restore_receipt["state"]);
    assert_eq!(
        restored_receipt["worker"]["worker_ref"],
        restore_receipt["worker"]["worker_ref"]
    );
    assert_eq!(
        restored_receipt["worker"]["status"],
        restore_receipt["worker"]["status"]
    );
    assert_eq!(
        restored_receipt["worker"]["profile_source"],
        restore_receipt["worker"]["profile_source"]
    );
    let mut restored_stop = disk["restore_guard"]["stop_owners"]["old-stop"]["receipt"].clone();
    assert_eq!(restored_stop["lifecycle"], "stopped");
    restored_stop.as_object_mut().unwrap().remove("lifecycle");
    // Optional null snapshot is omitted by the canonical current writer.
    restored_stop["worker_state"] = serde_json::Value::Null;
    assert_eq!(restored_stop, stop_receipt);
    assert_eq!(
        disk["restore_guard"]["owners"]["old-restore"]["operation_id"],
        restore_id.to_string()
    );
    assert_eq!(
        disk["restore_guard"]["stop_owners"]["old-stop"]["operation_id"],
        stop_id.to_string()
    );
    opened.store.write_worker_record(&saved).unwrap();
    let bytes = fs::read(&path).unwrap();
    drop(opened);
    let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
    reopened
        .store
        .read_worker_record(&worker.worker_ref)
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn schema17_finalization_gaps_preserve_owner_without_reverting_to_uncommitted_admission() {
    for terminal in [false, true] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let root = store.root.clone();
        let mut document = schema17_document(&worker);
        let operation_id = WorkerLifecycleOperationId::new();
        let receipt = if terminal {
            serde_json::json!({"state": "rolled_back", "worker": null, "reason_code": "retained-failure", "message": "original cause"})
        } else {
            serde_json::Value::Null
        };
        document["restore_guard"]["active_request_id"] = serde_json::json!("original-request");
        document["restore_guard"]["owners"]["original-request"] = serde_json::json!({
            "fingerprint": "original-fingerprint", "operation_id": operation_id,
            "request": {"request_id": "original-request", "expected_observation_token": document["restore_guard"]["observation_token"]},
            "preparation_applied": true, "execution_started": true, "receipt": receipt
        });
        let owner = document["restore_guard"]["owners"]["original-request"].clone();
        install_schema17_document(&store, &path, &document);
        drop(store);
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let saved = opened.store.read_worker_record(&worker.worker_ref).unwrap();
        let disk: serde_json::Value = read_json(&path, "read finalization recovery").unwrap();
        assert_eq!(disk["execution_state"]["state"], "restore_finalization");
        assert_eq!(
            disk["execution_state"]["execution"]["operation_id"],
            operation_id.to_string()
        );
        assert_eq!(
            disk["execution_state"]["execution"]["evidence"],
            if terminal {
                "receipt_stored"
            } else {
                "legacy_aggregate_published"
            }
        );
        assert_eq!(disk["restore_guard"]["owners"]["original-request"], owner);
        opened.store.write_worker_record(&saved).unwrap();
        let bytes = fs::read(&path).unwrap();
        drop(opened);
        let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        reopened
            .store
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // Current writers must use the explicit recovery discriminator. Merely
        // combining Runnable with a retained active marker is not valid schema18.
        let mut invalid = disk;
        invalid["execution_state"] = serde_json::json!({"state": "runnable", "execution": invalid["execution_state"]["execution"]["execution"]});
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            reopened
                .store
                .read_worker_record(&worker.worker_ref)
                .is_err()
        );
    }
}
