use super::*;

#[test]
fn ambiguous_duplicate_keys_are_not_rewritten_or_treated_as_disabled() {
    assert!(parse_document(r#"{"feature":{"memory":{"enabled":true,"enabled":false}}}"#).is_err());
    assert!(parse_document(r#"{"request":{"fingerprint":"one","fingerprint":"two"}}"#).is_err());
    let value = json!({"array":[null, true, 1, -2, 3.5, "text"], "object":{"nested":"value"}});
    assert_eq!(parse_document(&value.to_string()).unwrap(), value);
}

#[test]
fn missing_optional_metadata_snapshot_stays_missing() {
    let f = Fixture::new();
    let mut metadata: Value =
        serde_json::from_str(&regular_file(&f.metadata_path).unwrap()).unwrap();
    metadata
        .as_object_mut()
        .unwrap()
        .remove("resolved_manifest_snapshot");
    let before = encode(&metadata).unwrap();
    fs::write(&f.metadata_path, &before).unwrap();
    f.migrate().unwrap();
    assert_eq!(regular_file(&f.metadata_path).unwrap(), before);
    assert_eq!(f.store.load(f.id()).unwrap().schema_version, 3);
}

struct Fixture {
    _temp: tempfile::TempDir,
    store: StandaloneWorkerStore,
    allocation: StandaloneWorkerAllocation,
    record_path: PathBuf,
    metadata_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = StandaloneWorkerStore::open(temp.path().join("workers")).unwrap();
        let allocation = store
            .allocate(temp.path(), StaleLeasePolicy::Reject)
            .unwrap();
        let manifest = WorkerManifest::from_toml(
            r#"
[worker]
name = "migration-test"
[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"
[engine]
[scope]
allow = []
"#,
        )
        .unwrap();
        let record = store
            .commit_created_connected(
                &allocation,
                manifest.clone(),
                "migration-test".into(),
                session_store::new_session_id(),
                None,
                Some(crate::subjektiv::StandaloneSubjectBinding {
                    scope_id: "local-scope".into(),
                    subject_id: "local-subject".into(),
                }),
            )
            .unwrap();
        let record_path = store.worker_dir(record.worker_id).join(RECORD_FILE);
        let mut raw: Value = serde_json::from_str(&regular_file(&record_path).unwrap()).unwrap();
        raw["schema_version"] = json!(2);
        raw["manifest"]["manifest"]["feature"]["memory"] = json!({"profile": {"enabled": false}});
        raw["immutable_request"] =
            json!({"fingerprint":"request-hash", "bundle": ["unchanged", 19]});
        fs::write(&record_path, encode(&raw).unwrap()).unwrap();
        let metadata_dir = store
            .worker_metadata_dir(record.worker_id)
            .join("migration-test");
        fs::create_dir_all(&metadata_dir).unwrap();
        let metadata_path = metadata_dir.join("metadata.json");
        let metadata = session_store::WorkerMetadata::new(
            "migration-test",
            Some(session_store::WorkerActiveSegmentRef {
                session_id: record.active_session_id,
                segment_id: record.active_segment_id,
            }),
        );
        let mut metadata = serde_json::to_value(metadata).unwrap();
        metadata["resolved_manifest_snapshot"] =
            manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap();
        metadata["resolved_manifest_snapshot"]["manifest"]["feature"]["memory"] =
            json!({"enabled":false, "staging":false});
        metadata["immutable_request"] = raw["immutable_request"].clone();
        fs::write(&metadata_path, encode(&metadata).unwrap()).unwrap();
        Self {
            _temp: temp,
            store,
            allocation,
            record_path,
            metadata_path,
        }
    }

    fn id(&self) -> WorkerId {
        self.allocation.worker_id()
    }
    fn migrate(&self) -> Result<(), StandaloneStoreError> {
        self.store
            .migrate_disabled_workspace_memory(self.id(), self.allocation.lease())
    }
    fn pending(&self) -> RetirementCommit {
        RetirementCommit::prepare(
            self.id(),
            regular_file(&self.record_path).unwrap(),
            regular_file(&self.metadata_path).unwrap(),
        )
        .unwrap()
    }
    fn save_pending(&self, plan: &RetirementCommit) {
        fs::write(
            self.record_path.parent().unwrap().join(COMMIT_MARKER),
            serde_json::to_vec(plan).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn owner_migrates_both_snapshots_once_without_changing_other_data() {
    let f = Fixture::new();
    let record_before = regular_file(&f.record_path).unwrap();
    let metadata_before = regular_file(&f.metadata_path).unwrap();
    assert!(f.store.load(f.id()).is_err());
    let listed = f
        .store
        .list(f._temp.path(), StandaloneListScope::CurrentCwd, 10)
        .expect("disabled legacy policy must not prevent selecting its migration target");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].restore_blocker.is_none());
    let selected = listed[0].worker_id;
    assert_eq!(selected, f.id());
    assert_eq!(listed[0].cwd, f.allocation.cwd);
    assert_eq!(regular_file(&f.record_path).unwrap(), record_before);
    assert_eq!(regular_file(&f.metadata_path).unwrap(), metadata_before);
    let archive = f
        .record_path
        .parent()
        .unwrap()
        .join("immutable-profile.archive");
    fs::write(
        &archive,
        b"fingerprint/request/bundle/history must not change",
    )
    .unwrap();
    let session = f.store.sessions_dir(f.id()).join("history.jsonl");
    fs::write(&session, b"immutable historical request\n").unwrap();
    f.store
        .migrate_disabled_workspace_memory(selected, f.allocation.lease())
        .unwrap();
    let mut expected_record: Value = serde_json::from_str(&record_before).unwrap();
    expected_record["schema_version"] = json!(3);
    expected_record["manifest"]["manifest"]["feature"]
        .as_object_mut()
        .unwrap()
        .remove("memory");
    let mut expected_metadata: Value = serde_json::from_str(&metadata_before).unwrap();
    expected_metadata["resolved_manifest_snapshot"]["manifest"]["feature"]
        .as_object_mut()
        .unwrap()
        .remove("memory");
    let record_after = regular_file(&f.record_path).unwrap();
    let metadata_after = regular_file(&f.metadata_path).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&record_after).unwrap(),
        expected_record
    );
    assert_eq!(
        serde_json::from_str::<Value>(&metadata_after).unwrap(),
        expected_metadata
    );
    assert_eq!(f.store.load(f.id()).unwrap().schema_version, 3);
    f.migrate().unwrap();
    assert_eq!(regular_file(&f.record_path).unwrap(), record_after);
    assert_eq!(regular_file(&f.metadata_path).unwrap(), metadata_after);
    assert_eq!(
        fs::read(archive).unwrap(),
        b"fingerprint/request/bundle/history must not change"
    );
    assert_eq!(
        fs::read(session).unwrap(),
        b"immutable historical request\n"
    );
}

#[test]
fn disabled_shapes_are_storage_only_and_snapshot_versions_are_bounded() {
    let f = Fixture::new();
    let raw: Value = serde_json::from_str(&regular_file(&f.record_path).unwrap()).unwrap();
    for version in [None, Some(2), Some(3), Some(4), Some(5)] {
        for memory in [
            json!({"enabled":false}),
            json!({"profile":{"enabled":false}}),
            json!({
                "profile":{"enabled":false, "staging_tools":true,
                    "resident":{"inject_summary":true},
                    "extraction":{"enabled":true},
                    "consolidation":{"request_enabled":true}},
                "workspace_settings":null
            }),
        ] {
            let mut manifest = raw["manifest"]["manifest"].clone();
            manifest["feature"]["memory"] = memory;
            let mut snapshot = match version {
                None => manifest,
                Some(version) => json!({"schema_version":version, "manifest":manifest}),
            };
            assert!(manifest::read_persisted_worker_manifest_snapshot(snapshot.clone()).is_err());
            assert!(retire_snapshot(f.id(), &mut snapshot).unwrap());
            assert!(manifest::read_persisted_worker_manifest_snapshot(snapshot).is_ok());
        }
    }
    for version in [0, 1, 6, 999] {
        let mut snapshot = raw["manifest"].clone();
        snapshot["schema_version"] = json!(version);
        assert!(retire_snapshot(f.id(), &mut snapshot).is_err());
    }
}

#[test]
fn enabled_ambiguous_or_invalid_metadata_blocks_both_files_before_staging() {
    for memory in [
        Value::Null,
        json!({}),
        json!({"enabled":true}),
        json!({"enabled":"false"}),
        json!({"profile":{}}),
        json!({"profile":{"enabled":true}}),
        json!({"enabled":false, "profile":{"enabled":false}}),
        json!({"enabled":false, "unknown":false}),
        json!({"profile":{"enabled":false, "unknown":false}}),
        json!({"profile":{"enabled":false}, "workspace_settings":{"language":"English"}}),
    ] {
        let f = Fixture::new();
        let mut metadata: Value =
            serde_json::from_str(&regular_file(&f.metadata_path).unwrap()).unwrap();
        metadata["resolved_manifest_snapshot"]["manifest"]["feature"]["memory"] = memory;
        fs::write(&f.metadata_path, encode(&metadata).unwrap()).unwrap();
        let record_before = regular_file(&f.record_path).unwrap();
        let metadata_before = regular_file(&f.metadata_path).unwrap();
        let error = f.migrate().unwrap_err();
        assert!(
            matches!(
                error,
                StandaloneStoreError::WorkspaceMemoryRetirement { .. }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("cannot execute"));
        assert_eq!(regular_file(&f.record_path).unwrap(), record_before);
        assert_eq!(regular_file(&f.metadata_path).unwrap(), metadata_before);
        assert!(!f.record_path.parent().unwrap().join(COMMIT_MARKER).exists());
    }
}

#[test]
fn retained_ownership_projection_does_not_execute_or_modify_legacy_policy() {
    let f = Fixture::new();
    let mut record: Value = serde_json::from_str(&regular_file(&f.record_path).unwrap()).unwrap();
    record["manifest"]["manifest"]["feature"]["memory"] = json!({"profile":{"enabled":true}});
    record["status"] = json!("stopped");
    let before = encode(&record).unwrap();
    fs::write(&f.record_path, &before).unwrap();
    let root = f.store.root().join("subjektiv/retained-workers");
    fs::create_dir_all(&root).unwrap();
    let retained_dir = root.join(f.id().to_string());
    fs::rename(f.record_path.parent().unwrap(), &retained_dir).unwrap();
    let retained = StandaloneWorkerStore::open_existing(root).unwrap();
    let ownership = retained.load_session_ownership(f.id()).unwrap();
    assert_eq!(ownership.worker_id, f.id());
    assert_eq!(ownership.subject.unwrap().subject_id, "local-subject");
    assert!(retained.load(f.id()).is_err());
    assert_eq!(
        regular_file(&retained_dir.join(RECORD_FILE)).unwrap(),
        before
    );
}

#[test]
fn migration_requires_the_exact_current_worker_lease() {
    let f = Fixture::new();
    let other = f
        .store
        .allocate(f._temp.path(), StaleLeasePolicy::Reject)
        .unwrap();
    assert!(matches!(
        f.store
            .migrate_disabled_workspace_memory(f.id(), other.lease()),
        Err(StandaloneStoreError::LeaseOwnershipLost)
    ));
    let before = regular_file(&f.record_path).unwrap();
    let lease_path = &f.allocation.lease().path;
    let original = fs::read(lease_path).unwrap();
    let mut successor: LeaseRecord = serde_json::from_slice(&original).unwrap();
    successor.lease_id = Uuid::now_v7();
    fs::write(lease_path, serde_json::to_vec(&successor).unwrap()).unwrap();
    assert!(matches!(
        f.migrate(),
        Err(StandaloneStoreError::LeaseOwnershipLost)
    ));
    assert_eq!(regular_file(&f.record_path).unwrap(), before);
    fs::write(lease_path, original).unwrap();
}

#[test]
fn interrupted_retirement_rolls_forward_only_its_exact_files() {
    for completed in 0..=2 {
        let f = Fixture::new();
        let plan = f.pending();
        f.save_pending(&plan);
        if completed >= 1 {
            fs::write(&f.metadata_path, &plan.metadata_after).unwrap();
        }
        if completed >= 2 {
            fs::write(&f.record_path, &plan.record_after).unwrap();
        }
        assert!(matches!(
            f.store.load(f.id()),
            Err(StandaloneStoreError::IncompleteCommit(_))
        ));
        f.migrate().unwrap();
        assert_eq!(regular_file(&f.metadata_path).unwrap(), plan.metadata_after);
        assert_eq!(regular_file(&f.record_path).unwrap(), plan.record_after);
        assert!(f.store.load(f.id()).is_ok());
    }
    let f = Fixture::new();
    let plan = f.pending();
    f.save_pending(&plan);
    fs::write(&f.metadata_path, "{\"worker_name\":\"successor\"}").unwrap();
    assert!(matches!(
        f.migrate(),
        Err(StandaloneStoreError::MetadataChanged(_))
    ));
    assert_eq!(regular_file(&f.record_path).unwrap(), plan.record_before);
    assert_eq!(
        regular_file(&f.metadata_path).unwrap(),
        "{\"worker_name\":\"successor\"}"
    );
}

#[test]
fn unknown_or_tampered_commit_markers_remain_blocked() {
    let f = Fixture::new();
    let before = regular_file(&f.record_path).unwrap();
    let marker = f.record_path.parent().unwrap().join(COMMIT_MARKER);
    for payload in [f.id().to_string(), "{\"migration\":".into()] {
        fs::write(&marker, &payload).unwrap();
        assert!(matches!(
            f.migrate(),
            Err(StandaloneStoreError::IncompleteCommit(_))
        ));
        assert_eq!(regular_file(&marker).unwrap(), payload);
        assert_eq!(regular_file(&f.record_path).unwrap(), before);
    }
    let mut plan = f.pending();
    plan.record_after = plan.record_before.clone();
    f.save_pending(&plan);
    assert!(matches!(
        f.migrate(),
        Err(StandaloneStoreError::IncompleteCommit(_))
    ));
    assert_eq!(regular_file(&f.record_path).unwrap(), before);
}

#[test]
fn current_schema_cannot_be_used_to_revive_removed_memory() {
    let f = Fixture::new();
    let mut record: Value = serde_json::from_str(&regular_file(&f.record_path).unwrap()).unwrap();
    record["schema_version"] = json!(3);
    fs::write(&f.record_path, encode(&record).unwrap()).unwrap();
    assert!(matches!(
        f.migrate(),
        Err(StandaloneStoreError::WorkspaceMemoryRetirement { .. })
    ));
}

#[test]
fn listing_exposes_retired_policy_blockers_without_authority_or_writes() {
    for memory in [
        json!({"enabled":true}),
        json!({"profile":{}}),
        json!({"profile":{"enabled":false,"unknown":false}}),
    ] {
        let f = Fixture::new();
        let mut raw = parse_document(&regular_file(&f.record_path).unwrap()).unwrap();
        raw["manifest"]["manifest"]["feature"]["memory"] = memory;
        let before = encode(&raw).unwrap();
        fs::write(&f.record_path, &before).unwrap();
        let listed = f
            .store
            .list(f._temp.path(), StandaloneListScope::All, 10)
            .unwrap();
        assert_eq!(listed[0].worker_id, f.id());
        assert!(listed[0].restore_blocker.is_some());
        assert!(
            f.migrate().is_err(),
            "discovery must not admit the blocked policy"
        );
        assert_eq!(regular_file(&f.record_path).unwrap(), before);
        assert!(!f.record_path.parent().unwrap().join(COMMIT_MARKER).exists());
    }
}

#[test]
fn listing_rejects_malformed_unsupported_and_mismatched_storage_metadata() {
    let f = Fixture::new();
    let original = parse_document(&regular_file(&f.record_path).unwrap()).unwrap();
    for (pointer, value) in [
        ("/worker_id", json!(WorkerId::now_v7())),
        ("/storage_key", json!("../escape")),
        ("/active_session_id", json!("not-a-session-id")),
        ("/worker_name", json!("different-name")),
        ("/cwd/canonical_path", json!("relative/path")),
        ("/schema_version", json!(0)),
        ("/schema_version", json!(999)),
    ] {
        let mut raw = original.clone();
        *raw.pointer_mut(pointer).unwrap() = value;
        let before = encode(&raw).unwrap();
        fs::write(&f.record_path, &before).unwrap();
        assert!(
            f.store
                .list(f._temp.path(), StandaloneListScope::All, 10)
                .is_err(),
            "{pointer}"
        );
        assert_eq!(regular_file(&f.record_path).unwrap(), before);
    }
    for text in ["{", r#"{"worker_id":1,"worker_id":2}"#] {
        fs::write(&f.record_path, text).unwrap();
        assert!(matches!(
            f.store.list(f._temp.path(), StandaloneListScope::All, 10),
            Err(StandaloneStoreError::CorruptRecord { .. })
        ));
        assert_eq!(regular_file(&f.record_path).unwrap(), text);
    }
}

#[test]
fn discovery_never_promotes_unvalidated_execution_policy() {
    let f = Fixture::new();
    let mut raw = parse_document(&regular_file(&f.record_path).unwrap()).unwrap();
    raw["schema_version"] = json!(1);
    raw["manifest"]["manifest"]["unsupported_execution_policy"] = json!(true);
    let before = encode(&raw).unwrap();
    fs::write(&f.record_path, &before).unwrap();
    let metadata_before = regular_file(&f.metadata_path).unwrap();
    assert_eq!(
        f.store
            .list(f._temp.path(), StandaloneListScope::All, 10)
            .unwrap()[0]
            .worker_id,
        f.id()
    );
    assert!(f.store.load(f.id()).is_err());
    assert!(f.migrate().is_err());
    assert_eq!(regular_file(&f.record_path).unwrap(), before);
    assert_eq!(regular_file(&f.metadata_path).unwrap(), metadata_before);
    assert!(!f.record_path.parent().unwrap().join(COMMIT_MARKER).exists());
}

#[test]
fn listing_preserves_cwd_identity_sorting_and_limit_for_legacy_records() {
    let f = Fixture::new();
    let original = regular_file(&f.record_path).unwrap();
    let mut raw = parse_document(&original).unwrap();
    let other_id = WorkerId::now_v7();
    let other_dir = f.store.worker_dir(other_id);
    fs::create_dir(&other_dir).unwrap();
    raw["worker_id"] = json!(other_id);
    // Equal timestamps retain the stable descending WorkerId tie-break.
    fs::write(other_dir.join(RECORD_FILE), encode(&raw).unwrap()).unwrap();
    let mut ids = vec![f.id(), other_id];
    ids.sort_by_key(|id| std::cmp::Reverse(id.to_string()));
    let listed = f
        .store
        .list(f._temp.path(), StandaloneListScope::CurrentCwd, 1)
        .unwrap();
    assert_eq!(listed[0].worker_id, ids[0]);
    let other_cwd = f._temp.path().join("other-cwd");
    fs::create_dir(&other_cwd).unwrap();
    assert!(
        f.store
            .list(&other_cwd, StandaloneListScope::CurrentCwd, 10)
            .unwrap()
            .is_empty()
    );
    raw["cwd"]["inode"] = json!(0);
    fs::write(other_dir.join(RECORD_FILE), encode(&raw).unwrap()).unwrap();
    assert_eq!(
        f.store
            .list(f._temp.path(), StandaloneListScope::CurrentCwd, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.store
            .list(&other_cwd, StandaloneListScope::All, 10)
            .unwrap()
            .len(),
        2
    );
    assert!(
        f.store
            .list(f._temp.path(), StandaloneListScope::All, 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(regular_file(&f.record_path).unwrap(), original);
}
