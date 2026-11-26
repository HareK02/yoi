//! Real owner-locked Runtime migration, publication and reopen. No execution
//! backend is substituted for storage; live restoration is covered separately
//! by worker_backend::subjektiv_initialization_tests.
use super::*;
use serde_json::{Value, json};
use subjektiv_api::worker_initialization::{
    FEATURE_ID, ResolvedWorkerConnection, VERSION, WorkerConnectionKind,
};

fn fixture(source: u32, attached: bool) -> (tempfile::TempDir, PathBuf, WorkerRef, Value) {
    let (temp, store, worker, path) = super::tests::canonical_worker_store();
    let root = store.root.clone();
    let mut record: Value = if source == OLDEST_SCHEMA_VERSION {
        super::tests::schema_v6_worker_document(worker.worker_id)
    } else {
        read_json(&path, "read Worker fixture").unwrap()
    };
    record["schema_version"] = json!(source);
    record["workspace_id"] = json!("workspace-a");
    let request = if source == OLDEST_SCHEMA_VERSION {
        &mut record["request"]
    } else {
        &mut record["execution_state"]["execution"]["request"]
    };
    request
        .as_object_mut()
        .unwrap()
        .remove("feature_initialization");
    request["subjektiv_attached"] = json!(attached);
    request["workspace_api"] =
        json!({"workspace_id":"workspace-a", "base_url":"https://workspace.invalid"});
    request["memory_settings"] = json!({"workspace_id":"workspace-a", "language":"日本語"});
    request["create_fingerprint"] = json!("original-frozen-create-fingerprint");
    atomic_write_json(&path, &record, "write legacy Worker fixture").unwrap();
    super::tests::write_empty_schema_v8_store(&root);
    let runtime_path = root.join(RUNTIME_FILE);
    let mut runtime: Value = read_json(&runtime_path, "read Runtime fixture").unwrap();
    runtime["schema_version"] = json!(source);
    runtime["workspace_owners"] = json!({"workspace-a":"owner-a"});
    runtime["subject_create_recovery_fences"] = json!({});
    runtime["subject_delete_worker_fences"] = json!({});
    atomic_write_json(&runtime_path, &runtime, "write legacy Runtime fixture").unwrap();
    let session = "01900000-0000-7000-8000-000000000001";
    let metadata = json!({
        "worker_name":runtime_worker_name(worker.worker_id), "workspace_id":"workspace-a",
        "active":{"session_id":session},
        "subjektiv_session_attribution":{"state":"confirmed", "session_id":session,"subject_id":"subject-a"}
    });
    atomic_write_json(
        &path.with_file_name(WORKER_METADATA_FILE),
        &metadata,
        "write saved attribution",
    )
    .unwrap();
    fs::create_dir_all(path.parent().unwrap().join("session")).unwrap();
    fs::write(
        path.parent().unwrap().join("session/retained-history.txt"),
        b"immutable Session history\n",
    )
    .unwrap();
    drop(store);
    (temp, root, worker.worker_ref, record)
}

fn worker_path(root: &Path, worker: &WorkerRef) -> PathBuf {
    root.join(WORKERS_DIR)
        .join(worker.worker_id.to_string())
        .join(WORKER_FILE)
}

fn connection(record: &Value) -> ResolvedWorkerConnection {
    let request: CreateWorkerRequest =
        serde_json::from_value(record["execution_state"]["execution"]["request"].clone()).unwrap();
    request
        .feature_initialization
        .get(FEATURE_ID)
        .unwrap()
        .decode(FEATURE_ID, VERSION)
        .unwrap()
}

#[test]
fn legacy_confirmed_attachment_migrates_without_changing_fingerprint_history_or_attribution() {
    for source in [
        OLDEST_SCHEMA_VERSION,
        STOP_HISTORY_SCHEMA_VERSION,
        MEMORY_RETIREMENT_SCHEMA_VERSION,
    ] {
        let (_temp, root, worker, original) = fixture(source, true);
        let path = worker_path(&root, &worker);
        let metadata = fs::read(path.with_file_name(WORKER_METADATA_FILE)).unwrap();
        let before = fs::read(&path).unwrap();
        let runtime_before = fs::read(root.join(RUNTIME_FILE)).unwrap();
        let options = FsRuntimeStoreOptions::new(root.clone()).with_runtime_id("test-runtime");
        let plan = FsRuntimeStore::migration_plan(&options).unwrap();
        assert!(plan.migration_required);
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "planning must be read-only"
        );
        assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime_before);
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        opened.store.read_worker_record(&worker).unwrap();
        let after: Value = read_json(&path, "read migrated Worker").unwrap();
        let resolved = connection(&after);
        assert_eq!(resolved.workspace_id, "workspace-a");
        assert_eq!(resolved.subject_id.as_deref(), Some("subject-a"));
        assert_eq!(resolved.kind, WorkerConnectionKind::SubjectBody);
        resolved.validate().unwrap();
        let request = &after["execution_state"]["execution"]["request"];
        let mut expected = if source == OLDEST_SCHEMA_VERSION {
            original["request"].clone()
        } else {
            original["execution_state"]["execution"]["request"].clone()
        };
        expected
            .as_object_mut()
            .unwrap()
            .remove("subjektiv_attached");
        expected["feature_initialization"] = request["feature_initialization"].clone();
        assert_eq!(
            *request, expected,
            "only the attachment representation changes"
        );
        assert_eq!(
            fs::read(path.with_file_name(WORKER_METADATA_FILE)).unwrap(),
            metadata
        );
        assert_eq!(
            fs::read(path.parent().unwrap().join("session/retained-history.txt")).unwrap(),
            b"immutable Session history\n"
        );
        assert_eq!(after["schema_version"], SCHEMA_VERSION);
        drop(opened);
        let published = fs::read(&path).unwrap();
        let runtime_published = fs::read(root.join(RUNTIME_FILE)).unwrap();
        let reopened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        reopened.store.read_worker_record(&worker).unwrap();
        assert_eq!(fs::read(&path).unwrap(), published);
        assert_eq!(
            fs::read(root.join(RUNTIME_FILE)).unwrap(),
            runtime_published
        );
        drop(reopened);
        assert!(
            !FsRuntimeStore::migration_plan(&options)
                .unwrap()
                .migration_required
        );
    }
}

#[test]
fn split_execution_record_migrates_attachment_before_the_current_request_decoder() {
    let (_temp, root, worker, original) = fixture(OLDEST_SCHEMA_VERSION, true);
    let path = worker_path(&root, &worker);
    let mut split = migrate_worker_document(original, OLDEST_SCHEMA_VERSION, None, &path).unwrap();
    let request = &mut split.execution["request"];
    request
        .as_object_mut()
        .unwrap()
        .remove("feature_initialization");
    request["subjektiv_attached"] = json!(true);
    atomic_write_json(&path, &split.identity, "write old split identity").unwrap();
    atomic_write_json(
        &path.with_file_name(WORKER_EXECUTION_FILE),
        &split.execution,
        "write old split execution",
    )
    .unwrap();
    let runtime_path = root.join(RUNTIME_FILE);
    let mut runtime: Value = read_json(&runtime_path, "read split Runtime").unwrap();
    runtime["schema_version"] = json!(PREVIOUS_SCHEMA_VERSION);
    atomic_write_json(&runtime_path, &runtime, "write split Runtime").unwrap();
    let opened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
    opened.store.read_worker_record(&worker).unwrap();
    let after: Value = read_json(&path, "read migrated split Worker").unwrap();
    assert_eq!(connection(&after).subject_id.as_deref(), Some("subject-a"));
    assert!(!path.with_file_name(WORKER_EXECUTION_FILE).exists());
}

#[test]
fn attachment_conversion_covers_retained_restore_and_interrupted_stop_specifications() {
    let (_temp, root, worker, original) = fixture(MEMORY_RETIREMENT_SCHEMA_VERSION, true);
    let path = worker_path(&root, &worker);
    let request = original["execution_state"]["execution"]["request"].clone();
    let mut retained = json!({"execution_state":{"execution":{
        "request":request, "execution":{"request":request}, "intent":{
            "request":request, "pending_restore":{"request":request},
            "restore_finalization":{"execution":{"request":request}}
        }
    }}});
    feature_initialization_migration::migrate(&mut retained, &path).unwrap();
    for pointer in [
        "/execution_state/execution/request",
        "/execution_state/execution/execution/request",
        "/execution_state/execution/intent/request",
        "/execution_state/execution/intent/pending_restore/request",
        "/execution_state/execution/intent/restore_finalization/execution/request",
    ] {
        let migrated = retained.pointer(pointer).unwrap();
        assert!(migrated.get("subjektiv_attached").is_none());
        let typed: CreateWorkerRequest = serde_json::from_value(migrated.clone()).unwrap();
        assert_eq!(
            typed.create_fingerprint,
            "original-frozen-create-fingerprint"
        );
        let resolved: ResolvedWorkerConnection = typed
            .feature_initialization
            .get(FEATURE_ID)
            .unwrap()
            .decode(FEATURE_ID, VERSION)
            .unwrap();
        assert_eq!(resolved.subject_id.as_deref(), Some("subject-a"));
    }
}

#[test]
fn legacy_unattached_request_keeps_settings_and_does_not_infer_connection_from_metadata() {
    for flag in [Some(false), None] {
        let (_temp, root, worker, mut original) = fixture(MEMORY_RETIREMENT_SCHEMA_VERSION, false);
        let path = worker_path(&root, &worker);
        if flag.is_none() {
            original["execution_state"]["execution"]["request"]
                .as_object_mut()
                .unwrap()
                .remove("subjektiv_attached");
            atomic_write_json(&path, &original, "write omitted false flag").unwrap();
        }
        let opened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        opened.store.read_worker_record(&worker).unwrap();
        let after: Value = read_json(&path, "read migrated ordinary Worker").unwrap();
        let request = &after["execution_state"]["execution"]["request"];
        assert_eq!(request["feature_initialization"], json!([]));
        assert_eq!(
            request["memory_settings"],
            original["execution_state"]["execution"]["request"]["memory_settings"]
        );
        assert_eq!(
            request["create_fingerprint"],
            "original-frozen-create-fingerprint"
        );
        assert!(
            opened
                .store
                .load_runtime_state()
                .unwrap()
                .diagnostics
                .is_empty()
        );
    }
}

#[test]
fn unknown_attachment_keeps_connection_unavailable_instead_of_blocking_store_startup() {
    for evidence in ["missing", "unconfirmed", "stale_session", "job"] {
        let (_temp, root, worker, mut original) = fixture(MEMORY_RETIREMENT_SCHEMA_VERSION, true);
        let path = worker_path(&root, &worker);
        let metadata_path = path.with_file_name(WORKER_METADATA_FILE);
        let mut metadata: Value = read_json(&metadata_path, "read attribution fixture").unwrap();
        match evidence {
            "missing" => {
                fs::remove_file(&metadata_path).unwrap();
            }
            "unconfirmed" => {
                metadata["subjektiv_session_attribution"] = Value::Null;
                atomic_write_json(&metadata_path, &metadata, "write unknown attribution").unwrap();
            }
            "stale_session" => {
                metadata["active"]["session_id"] = json!("01900000-0000-7000-8000-000000000002");
                atomic_write_json(&metadata_path, &metadata, "write stale attribution").unwrap();
            }
            "job" => {
                original["execution_state"]["execution"]["request"]["backend_job"] = json!({
                    "job_id":"job-a", "attempt_id":"attempt-a", "subjektiv_consolidation":true, "subjektiv_journal":false
                });
                atomic_write_json(&path, &original, "write old Job binding").unwrap();
            }
            _ => unreachable!(),
        }
        let opened = FsRuntimeStore::open_or_create(root.clone(), "test-runtime").unwrap();
        opened.store.read_worker_record(&worker).unwrap();
        let after: Value = read_json(&path, "read unresolved connection").unwrap();
        let resolved = connection(&after);
        assert_eq!(resolved.subject_id, None);
        assert_eq!(
            resolved.kind,
            if evidence == "job" {
                WorkerConnectionKind::MemoryConsolidation
            } else {
                WorkerConnectionKind::SubjectBody
            }
        );
        assert!(
            resolved.validate().is_err(),
            "unknown identity must not enable restoration"
        );
        let state = opened.store.load_runtime_state().unwrap();
        assert!(
            state
                .diagnostics
                .iter()
                .any(|d| d.code == "worker_subject_identity_unresolved"
                    && d.worker_ref.as_ref() == Some(&worker))
        );
        assert_eq!(
            after["execution_state"]["execution"]["request"]["backend_job"],
            original["execution_state"]["execution"]["request"]["backend_job"]
        );
        drop(opened);
        let reopened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
        assert_eq!(
            reopened
                .store
                .load_runtime_state()
                .unwrap()
                .diagnostics
                .len(),
            state.diagnostics.len()
        );
    }
}

#[test]
fn invalid_or_conflicting_attachment_rolls_back_without_rewriting_original_tree() {
    for invalid in [json!("true"), json!(null), json!(true)] {
        let (_temp, root, worker, mut original) = fixture(MEMORY_RETIREMENT_SCHEMA_VERSION, true);
        let path = worker_path(&root, &worker);
        original["execution_state"]["execution"]["request"]["subjektiv_attached"] = invalid.clone();
        if invalid == true {
            original["execution_state"]["execution"]["request"]["feature_initialization"] =
                serde_json::to_value(crate::catalog::test_subject_initialization(
                    "workspace-a",
                    "another-subject",
                ))
                .unwrap();
        }
        atomic_write_json(&path, &original, "write corrupt attachment fixture").unwrap();
        let worker_before = fs::read(&path).unwrap();
        let runtime_before = fs::read(root.join(RUNTIME_FILE)).unwrap();
        let metadata_before = fs::read(path.with_file_name(WORKER_METADATA_FILE)).unwrap();
        assert!(FsRuntimeStore::open_or_create(root.clone(), "test-runtime").is_err());
        assert_eq!(fs::read(&path).unwrap(), worker_before);
        assert_eq!(fs::read(root.join(RUNTIME_FILE)).unwrap(), runtime_before);
        assert_eq!(
            fs::read(path.with_file_name(WORKER_METADATA_FILE)).unwrap(),
            metadata_before
        );
        assert!(
            !migration_sibling(&root, "schema-v13-staging")
                .unwrap()
                .exists()
        );
    }
}

#[test]
fn attachment_migration_does_not_search_user_input_or_feature_payloads() {
    let (_temp, root, worker, mut original) = fixture(MEMORY_RETIREMENT_SCHEMA_VERSION, false);
    let path = worker_path(&root, &worker);
    // A separately supplied Feature payload is opaque to the Runtime migration.
    original["execution_state"]["execution"]["request"]
        .as_object_mut()
        .unwrap()
        .remove("subjektiv_attached");
    original["execution_state"]["execution"]["request"]["feature_initialization"] = json!([{
        "feature_id":"plugin:example", "version":1, "payload":{"request":{"subjektiv_attached":true}}
    }]);
    original["execution_state"]["execution"]["request"]["initial_input"] = serde_json::to_value(
        crate::interaction::WorkerInput::user("subjektiv_attached must remain literal history"),
    )
    .unwrap();
    atomic_write_json(&path, &original, "write opaque payload fixture").unwrap();
    let opened = FsRuntimeStore::open_or_create(root, "test-runtime").unwrap();
    opened.store.read_worker_record(&worker).unwrap();
    let after: Value = read_json(&path, "read opaque payload").unwrap();
    assert_eq!(after["execution_state"], original["execution_state"]);
}
