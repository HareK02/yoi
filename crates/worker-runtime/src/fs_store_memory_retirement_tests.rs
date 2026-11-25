//! Real owner-locked Runtime tree migration, not a tolerant Manifest decoder.
//! Model/Backend effects are outside this storage suite; restore-policy admission
//! is covered by worker_backend::memory_retirement_tests.
use super::*;
use serde_json::{Value, json};

fn fixture(memory: Value) -> (tempfile::TempDir, PathBuf, WorkerRef, Value, Value) {
    let (temp, store, worker, path) = super::tests::canonical_worker_store();
    let root = store.root.clone();
    let mut record: Value = read_json(&path, "read fixture").unwrap();
    record["schema_version"] = json!(STOP_HISTORY_SCHEMA_VERSION);
    record["workspace_id"] = json!("workspace-a");
    let request = &mut record["execution_state"]["execution"]["request"];
    request["workspace_api"] =
        json!({"workspace_id":"workspace-a", "base_url":"https://workspace.invalid"});
    request["memory_settings"] = json!({"workspace_id":"workspace-a", "language":"日本語"});
    request["create_fingerprint"] = json!("original-frozen-create-fingerprint");
    let connection = subjektiv_api::worker_initialization::ResolvedWorkerConnection {
        workspace_id: "workspace-a".into(),
        subject_id: Some("subject-a".into()),
        kind: subjektiv_api::worker_initialization::WorkerConnectionKind::SubjectBody,
    };
    request["feature_initialization"] = serde_json::to_value(
        protocol::feature_initialization::FeatureInitializations::new(vec![
            protocol::feature_initialization::FeatureInitialization::encode(
                subjektiv_api::worker_initialization::FEATURE_ID,
                subjektiv_api::worker_initialization::VERSION,
                &connection,
            )
            .unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    atomic_write_json(&path, &record, "write frozen Worker fixture").unwrap();

    let mut config = manifest::WorkerManifestConfig::resolution_defaults();
    config.worker.name = Some(runtime_worker_name(worker.worker_id));
    let mut manifest = manifest::WorkerManifest::try_from(config).unwrap();
    manifest.feature.filesystem.enabled = false;
    manifest.feature.bash.enabled = false;
    manifest.feature.subjektiv.profile.enabled = true;
    manifest.feature.subjektiv.profile.extraction.enabled = true;
    manifest
        .feature
        .subjektiv
        .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
            workspace_id: "workspace-a".into(),
            language: "日本語".into(),
        })
        .unwrap();
    let mut snapshot = manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap();
    snapshot["manifest"]["feature"]["memory"] = memory;
    assert!(manifest::read_persisted_worker_manifest_snapshot(snapshot.clone()).is_err());
    let mut metadata =
        session_store::WorkerMetadata::new(runtime_worker_name(worker.worker_id), None);
    metadata.workspace_id = Some("workspace-a".into());
    metadata.resolved_manifest_snapshot = Some(snapshot);
    let metadata = serde_json::to_value(metadata).unwrap();
    atomic_write_json(
        &path.with_file_name(WORKER_METADATA_FILE),
        &metadata,
        "write frozen metadata",
    )
    .unwrap();
    // Opaque original source/history are not execution-policy conversion targets.
    fs::create_dir_all(root.join("profile-evidence")).unwrap();
    fs::write(
        root.join("profile-evidence/original.dcdl"),
        b"feature = { memory = { enabled = false; }; };\n",
    )
    .unwrap();
    fs::create_dir_all(path.parent().unwrap().join("session")).unwrap();
    fs::write(
        path.parent().unwrap().join("session/retained-history.txt"),
        b"Subject evidence is immutable\n",
    )
    .unwrap();

    super::tests::write_empty_schema_v8_store(&root);
    let mut runtime: Value = read_json(&root.join(RUNTIME_FILE), "read Runtime fixture").unwrap();
    runtime["schema_version"] = json!(STOP_HISTORY_SCHEMA_VERSION);
    runtime["workspace_owners"] = json!({"workspace-a":"owner-a"});
    let identity = json!({"workspace_id":"workspace-a", "worker_id":WorkerId::now_v7(),
        "create_fingerprint":"preserved-receipt-fingerprint", "operation_id":"recovery-a"});
    runtime["subject_create_recovery_fences"] = json!({identity["worker_id"].as_str().unwrap(): {
        "request":identity, "response":identity,
    }});
    runtime["subject_delete_worker_fences"] = json!({});
    // A nonempty typed bundle catches the schema-20 preservation whitelist regression.
    let bundle: ConfigBundle = serde_json::from_value(json!({"metadata":{
        "id":"frozen-bundle", "digest":"original-digest", "workspace_id":"workspace-a",
        "created_at":"2026-11-24T00:00:00Z", "provenance":{"source":"original"}},
        "profiles":[{"selector":{"kind":"builtin","value":"builtin:coder"}}]
    }))
    .unwrap();
    runtime["config_bundles"] = json!({"frozen-bundle":bundle});
    atomic_write_json(
        &root.join(RUNTIME_FILE),
        &runtime,
        "write frozen Runtime fixture",
    )
    .unwrap();
    drop(store);
    (temp, root, worker.worker_ref, record, runtime)
}

#[test]
fn schema_v20_default_restore_guard_is_not_reinterpreted_as_pre_v20_stop_history() {
    let (_temp, root, worker_ref, mut record, _) = fixture(json!({"profile":{"enabled":false}}));
    let path = root
        .join(WORKERS_DIR)
        .join(worker_ref.worker_id.to_string())
        .join(WORKER_FILE);
    let request = record["execution_state"]["execution"]["request"].clone();
    record.as_object_mut().unwrap().remove("restore_guard");
    atomic_write_json(&path, &record, "write schema-20 default guard fixture").unwrap();
    let opened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
    let restored = opened.store.read_worker_record(&worker_ref).unwrap();
    let after: Value = read_json(&path, "read migrated default guard fixture").unwrap();
    let guard = serde_json::to_value(&restored.restore_guard).unwrap();
    assert!(uuid::Uuid::parse_str(guard["observation_token"].as_str().unwrap()).is_ok());
    let mut expected = serde_json::to_value(crate::runtime::RestoreGuard::default()).unwrap();
    expected["observation_token"] = after["restore_guard"]["observation_token"].clone();
    assert_eq!(guard, expected);
    assert_eq!(guard, after["restore_guard"]);
    assert_eq!(after["execution_state"]["execution"]["request"], request);
}

#[test]
fn schema_v20_disabled_memory_migrates_subject_snapshot_without_rewriting_create_evidence() {
    let (_temp, root, worker_ref, mut record, mut runtime) = fixture(json!({"profile":{
        "enabled":false, "staging_tools":false,
        "resident":{"inject_summary":true}, "extraction":{"enabled":true},
        "consolidation":{"request_enabled":true}
    }}));
    let worker = root
        .join(WORKERS_DIR)
        .join(worker_ref.worker_id.to_string());
    let metadata_before: Value =
        read_json(&worker.join(WORKER_METADATA_FILE), "read metadata").unwrap();
    let runtime_before = fs::read(root.join(RUNTIME_FILE)).unwrap();
    let worker_before = fs::read(worker.join(WORKER_FILE)).unwrap();
    let metadata_bytes = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
    let options = FsRuntimeStoreOptions::new(root.clone()).with_runtime_id("test-runtime");
    let plan = FsRuntimeStore::migration_plan(&options).unwrap();
    assert!(plan.migration_required);
    assert_eq!(plan.migrated_worker_aggregate_count, 1);
    assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime_before);
    assert_eq!(fs::read(worker.join(WORKER_FILE)).unwrap(), worker_before);
    assert_eq!(
        fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(),
        metadata_bytes
    );

    let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
    let restored = opened.store.read_worker_record(&worker_ref).unwrap();
    assert_eq!(restored.worker_ref, worker_ref);
    record["schema_version"] = json!(SCHEMA_VERSION);
    assert_eq!(
        read_json::<Value>(&worker.join(WORKER_FILE), "read migrated Worker").unwrap(),
        record
    );
    runtime["schema_version"] = json!(SCHEMA_VERSION);
    assert_eq!(
        read_json::<Value>(&root.join(RUNTIME_FILE), "read migrated Runtime").unwrap(),
        runtime
    );
    let metadata_after: Value =
        read_json(&worker.join(WORKER_METADATA_FILE), "read migrated metadata").unwrap();
    let mut expected = metadata_before;
    expected["resolved_manifest_snapshot"]["manifest"]["feature"]
        .as_object_mut()
        .unwrap()
        .remove("memory");
    assert_eq!(metadata_after, expected);
    let manifest = manifest::read_persisted_worker_manifest_snapshot(
        metadata_after["resolved_manifest_snapshot"].clone(),
    )
    .unwrap();
    assert!(!manifest.feature.filesystem.enabled);
    assert!(!manifest.feature.bash.enabled);
    assert!(manifest.feature.subjektiv.execution_enabled());
    assert!(manifest.feature.subjektiv.profile.extraction.enabled);
    assert_eq!(
        manifest
            .feature
            .subjektiv
            .workspace_settings()
            .unwrap()
            .language,
        "日本語"
    );
    assert_eq!(
        fs::read(root.join("profile-evidence/original.dcdl")).unwrap(),
        b"feature = { memory = { enabled = false; }; };\n"
    );
    assert_eq!(
        fs::read(worker.join("session/retained-history.txt")).unwrap(),
        b"Subject evidence is immutable\n"
    );
    drop(opened);
    let after = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
    let reopened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
    reopened.store.read_worker_record(&worker_ref).unwrap();
    assert_eq!(fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(), after);
}

#[test]
fn schema_v20_enabled_or_ambiguous_memory_is_diagnosed_not_converted_to_subject_execution() {
    for memory in [
        json!({"profile":{"enabled":true}}),
        json!({"profile":{}}),
        json!({"enabled":false,"unknown":true}),
        json!({"profile":{"enabled":false},"workspace_settings":{"workspace_id":"workspace-a","language":"English"}}),
        Value::Null,
    ] {
        let (_temp, root, worker_ref, mut record, _) = fixture(memory);
        let worker = root
            .join(WORKERS_DIR)
            .join(worker_ref.worker_id.to_string());
        let before = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        let state = opened.store.load_runtime_state().unwrap();
        assert!(
            state
                .diagnostics
                .iter()
                .any(|d| d.code == "workspace_memory_worker_retired"
                    && d.worker_ref.as_ref() == Some(&worker_ref))
        );
        assert_eq!(fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(), before);
        let metadata: Value = read_json(
            &worker.join(WORKER_METADATA_FILE),
            "read unavailable metadata",
        )
        .unwrap();
        assert!(
            manifest::read_persisted_worker_manifest_snapshot(
                metadata["resolved_manifest_snapshot"].clone()
            )
            .is_err()
        );
        record["schema_version"] = json!(SCHEMA_VERSION);
        assert_eq!(
            read_json::<Value>(&worker.join(WORKER_FILE), "read unavailable Worker").unwrap(),
            record
        );
    }
}

#[test]
fn memory_retirement_owner_conflict_and_bad_snapshot_leave_original_tree_unchanged() {
    let (_temp, root, worker_ref, _, _) = fixture(json!({"profile":{"enabled":false}}));
    let worker = root
        .join(WORKERS_DIR)
        .join(worker_ref.worker_id.to_string());
    let owner = acquire_runtime_store_owner_lock(&root).unwrap();
    let before = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
    assert!(FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err());
    assert_eq!(fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(), before);
    drop(owner);
    let mut metadata: Value = serde_json::from_slice(&before).unwrap();
    metadata["resolved_manifest_snapshot"]["manifest"]["worker"]["name"] = json!("wrong-worker");
    atomic_write_json(
        &worker.join(WORKER_METADATA_FILE),
        &metadata,
        "write identity conflict",
    )
    .unwrap();
    let bad = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
    let runtime = fs::read(root.join(RUNTIME_FILE)).unwrap();
    assert!(FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err());
    assert_eq!(fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(), bad);
    assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime);
    assert!(
        !migration_sibling(&root, "schema-v13-staging")
            .unwrap()
            .exists()
    );
}

#[test]
fn retirement_recognizes_disabled_flat_and_nested_snapshots_without_subjektiv_conversion() {
    use memory_retirement::{Retirement, retire_disabled_memory};
    for memory in [
        json!({"enabled":false,"staging":false}),
        json!({"profile":{"enabled":false}}),
    ] {
        for wrapped in [false, true] {
            let manifest = json!({"feature":{"memory":memory,"subjektiv":{"profile":{"enabled":true}}},"memory":{"language":"English"}});
            let mut snapshot = if wrapped {
                json!({"schema_version":1,"manifest":manifest})
            } else {
                manifest
            };
            let original = snapshot.clone();
            assert_eq!(retire_disabled_memory(&mut snapshot), Retirement::Disabled);
            let manifest = snapshot.get("manifest").unwrap_or(&snapshot);
            assert!(manifest.get("memory").is_none());
            assert!(manifest["feature"].get("memory").is_none());
            assert_eq!(
                manifest["feature"]["subjektiv"],
                original.get("manifest").unwrap_or(&original)["feature"]["subjektiv"]
            );
            assert_eq!(retire_disabled_memory(&mut snapshot), Retirement::Unchanged);
        }
    }
    let mut legacy_profile = json!({"profile":{"source":{"kind":"registry","source":"builtin","name":"memory-consolidation"}},"feature":{"memory":{"profile":{"enabled":false}}}});
    let before = legacy_profile.clone();
    assert!(matches!(
        retire_disabled_memory(&mut legacy_profile),
        Retirement::Unavailable("removed Workspace Memory profile")
    ));
    assert_eq!(legacy_profile, before);
}
