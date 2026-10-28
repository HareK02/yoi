#[test]
fn subject_delete_schema14_migration_adds_exclusion_store_without_changing_worker_or_receipts() {
    for with_fence in [false, true] {
        let (_temp, store, worker, path) = canonical_worker_store();
        let root = store.root.clone();
        let mut legacy =
            serde_json::to_value(WorkerAggregateRecord::from_persisted(&worker)).unwrap();
        legacy["schema_version"] = serde_json::json!(RESTORE_REPAIR_SCHEMA_VERSION);
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let id = WorkerId::now_v7();
        let request = serde_json::json!({"workspace_id": "workspace-a", "worker_id": id,
            "create_fingerprint": "fenced-create", "operation_id": "delete-1"});
        let mut response = request.clone();
        response["originals_absent"] = false.into();
        let fence =
            serde_json::json!({ id.to_string(): {"request": request, "response": response} });
        let mut runtime = serde_json::json!({"schema_version": RESTORE_REPAIR_SCHEMA_VERSION,
            "display_name": "retained", "backend": "fs_store", "status": "running", "next_diagnostic_id": 1,
            "workspace_owners": {"workspace-a": "server-a"}, "diagnostics": [],
            "config_bundles": {}, "subject_create_recovery_fences": {}, "subject_releases": []});
        if with_fence {
            runtime["subject_delete_worker_fences"] = fence.clone();
        }
        fs::write(
            root.join(RUNTIME_FILE),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        drop(store);
        let options = FsRuntimeStoreOptions::new(&root).with_runtime_id("test-runtime");
        let before = fs::read(&path).unwrap();
        assert!(
            FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        legacy["schema_version"] = serde_json::json!(SCHEMA_VERSION);
        assert_eq!(
            read_json::<serde_json::Value>(&path, "read migrated Worker").unwrap(),
            legacy
        );
        let expected = if with_fence {
            fence
        } else {
            serde_json::json!({})
        };
        assert_eq!(
            serde_json::to_value(&opened.state.as_ref().unwrap().subject_delete_worker_fences)
                .unwrap(),
            expected
        );
        opened
            .store
            .write_runtime_snapshot(opened.state.as_ref().unwrap())
            .unwrap();
        drop(opened);
        let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        assert_eq!(
            serde_json::to_value(&reopened.state.unwrap().subject_delete_worker_fences).unwrap(),
            expected
        );
    }
}

#[test]
fn subject_delete_store_rejects_missing_exclusion_field_malformed_receipt_and_workspace_owner() {
    for invalid in 0..4 {
        let (_temp, store, _worker, _path) = canonical_worker_store();
        let root = store.root.clone();
        let id = WorkerId::now_v7();
        let request = serde_json::json!({"workspace_id": "workspace-a", "worker_id": id,
            "create_fingerprint": "fenced-create", "operation_id": "delete-1"});
        let mut response = request.clone();
        response["originals_absent"] = false.into();
        let mut runtime = serde_json::json!({"schema_version": SCHEMA_VERSION,
            "display_name": "retained", "backend": "fs_store", "status": "running", "next_diagnostic_id": 1,
            "workspace_owners": {"workspace-a": "server-a"}, "diagnostics": [],
            "subject_delete_worker_fences": { id.to_string(): {"request": request, "response": response} }});
        match invalid {
            0 => {
                runtime
                    .as_object_mut()
                    .unwrap()
                    .remove("subject_delete_worker_fences");
            }
            1 => {
                runtime["subject_delete_worker_fences"][id.to_string()]["response"]["create_fingerprint"] =
                    "different".into()
            }
            2 => runtime["workspace_owners"] = serde_json::json!({}),
            _ => {
                runtime["subject_delete_worker_fences"][id.to_string()]["request"]["worker_id"] =
                    WorkerId::now_v7().to_string().into()
            }
        }
        let bytes = serde_json::to_vec(&runtime).unwrap();
        fs::write(root.join(RUNTIME_FILE), &bytes).unwrap();
        drop(store);
        assert!(FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err());
        assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), bytes);
    }
}
