use super::*;
use crate::auth::RuntimeIdentityMaterial;
use crate::catalog::{ProfileSelector, WorkspaceApiRef};
use crate::profile_archive::ProfileSourceGraphSummary;
use session_store::{LogEntry, Store};
use sha2::{Digest, Sha256};

const RUNTIME: &str = "fixture-runtime";
const WORKSPACE: &str = "fixture-workspace";
const NEW_URL: &str = "http://127.0.0.1:34567";

#[derive(Serialize)]
#[serde(untagged)]
enum ReceiptFixture<T> {
    Valid(T),
    Malformed(u8),
}

#[derive(Default, Serialize)]
struct CheckpointFixture {
    next_activation_sequence: u64,
    activating: Option<u8>,
    activating_notification: Vec<u8>,
    pending: Vec<u8>,
    pending_notifications: Vec<u8>,
    receipts: Vec<ReceiptFixture<SubmissionReceipt>>,
    notification_receipts: Vec<ReceiptFixture<NotificationReceipt>>,
    invocation_receipts: Vec<u8>,
    invocation_recovery_blocked: bool,
}

fn settled_checkpoint() -> CheckpointFixture {
    CheckpointFixture {
        next_activation_sequence: 2,
        receipts: vec![ReceiptFixture::Valid(SubmissionReceipt {
            submission_request_id: "completed-submission-request".into(),
            source_namespace: "fixture-source".into(),
            submission_id: "completed-submission".into(),
            payload_digest: "fixture-payload-digest".into(),
            disposition: protocol::SubmissionDisposition::Started,
        })],
        notification_receipts: vec![ReceiptFixture::Valid(NotificationReceipt {
            notification_request_id: "completed-notification-request".into(),
            source_namespace: "fixture-source".into(),
            payload_digest: "fixture-notification-digest".into(),
        })],
        ..Default::default()
    }
}

fn checkpoint_extension(checkpoint: &CheckpointFixture) -> session_store::SessionExtension {
    session_store::SessionExtension::new(
        "worker.pending_activations.v1",
        serde_json::to_value(checkpoint).unwrap(),
    )
}

fn worker_id() -> WorkerId {
    "01a1657d-b762-7791-87ae-a49f28cadbce".parse().unwrap()
}

fn resolved() -> manifest::WorkerManifest {
    let mut config = manifest::WorkerManifestConfig::resolution_defaults();
    config.worker.name = Some(runtime_worker_name(worker_id()));
    manifest::WorkerManifest::try_from(config).unwrap()
}

fn request() -> CreateWorkerRequest {
    CreateWorkerRequest {
        plugin_execution: None,
        worker_id: worker_id(),
        create_fingerprint: "new-create-fingerprint".into(),
        profile: ProfileSelector::Builtin("default".into()),
        display_name: Some("fixture".into()),
        profile_source: ProfileSourceArchiveSource::WorkspaceConfig {
            archive: ProfileSourceArchiveRef {
                id: "new-profile".into(),
                digest: "new-profile-digest".into(),
                size_bytes: 1,
                source_graph: ProfileSourceGraphSummary {
                    source_count: 1,
                    total_source_bytes: 1,
                    entrypoints: BTreeMap::from([("builtin:default".into(), "default.dcl".into())]),
                    import_count: 0,
                },
            },
        },
        config_bundle: Some(ConfigBundleRef {
            id: "new-config".into(),
            digest: "new-config-digest".into(),
        }),
        workspace_api: Some(WorkspaceApiRef {
            workspace_id: WORKSPACE.into(),
            base_url: NEW_URL.into(),
        }),
        initial_input: None,
        workdir_attachment_requests: Vec::new(),
        workdir_attachments: Vec::new(),
        worker_observation_enabled: false,
        worker_observation_grants: Vec::new(),
        memory_settings: Some(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: WORKSPACE.into(),
            language: "en".into(),
        }),
        subjektiv_attached: false,
        backend_job: None,
    }
}

fn initialize(root: &Path, runtime_id: &str, workspace_id: &str, url: &str) {
    #[derive(Serialize)]
    struct Auth {
        identity: RuntimeIdentityMaterial,
        workspace_issuers: Vec<WorkspaceIssuerTrustRecord>,
    }
    let issuer = RuntimeIdentityMaterial::generate("fixture-issuer").unwrap();
    let digest = Sha256::digest(crate::auth::decode_public_key(&issuer.public_key).unwrap());
    let fingerprint = format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let trust = WorkspaceIssuerTrustRecord::from_bundle(
        server_api::WorkspacePublicIdentityBundle {
            workspace_id: workspace_id.into(),
            backend_url: url.into(),
            key_id: "fixture-issuer".into(),
            algorithm: "ed25519".into(),
            public_key: issuer.public_key,
            public_key_fingerprint: fingerprint,
        },
        "fixture-trust".into(),
        100,
    )
    .unwrap();
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("auth.toml"),
        toml::to_string(&Auth {
            identity: RuntimeIdentityMaterial::generate(runtime_id).unwrap(),
            workspace_issuers: vec![trust],
        })
        .unwrap(),
    )
    .unwrap();
}

fn record(request: CreateWorkerRequest) -> PersistedWorkerRecord {
    PersistedWorkerRecord {
        restore_guard: crate::runtime::RestoreGuard::default(),
        worker_ref: WorkerRef::new(request.worker_id),
        worker_id: request.worker_id,
        profile: request.profile.clone(),
        display_name: request.display_name.clone(),
        profile_source: request.profile_source.reference(),
        config_bundle: request.config_bundle.clone(),
        created_at_ms: Some(123456),
        status: WorkerStatus::Stopped,
        last_finished_submission_request_id: Some("old-submission-operation".into()),
        workspace_id: Some(WORKSPACE.into()),
        workdir_attachments: vec![],
        logical_workdir_attachments: vec![],
        execution_state: PersistedWorkerExecutionState::Available(PersistedWorkerExecution {
            request,
            binding: Some(PersistedWorkerExecutionBinding {}),
            restore_intent: WorkerRestoreIntent::Explicit,
        }),
    }
}

fn source() -> (tempfile::TempDir, PathBuf, StoppedWorkerTemplate) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    initialize(&root, RUNTIME, WORKSPACE, "http://127.0.0.1:23456");
    drop(
        crate::Runtime::with_fs_store(FsRuntimeStoreOptions::new(&root).with_runtime_id(RUNTIME))
            .unwrap(),
    );
    let opened = FsRuntimeStore::open_or_create(root.clone(), RUNTIME).unwrap();
    let mut old = request();
    old.workspace_api.as_mut().unwrap().base_url = "http://127.0.0.1:23456".into();
    old.config_bundle.as_mut().unwrap().id = "old-config".into();
    opened.store.write_worker_record(&record(old)).unwrap();
    let worker = opened.store.worker_dir(&worker_id());
    let session_id = "01a1657d-b800-7777-8888-111111111111".parse().unwrap();
    let segment_id = "01a1657d-b818-77c1-bd73-176172a4bd82".parse().unwrap();
    let mut metadata = WorkerMetadata::new(
        runtime_worker_name(worker_id()),
        Some(WorkerActiveSegmentRef {
            session_id,
            segment_id: Some(segment_id),
        }),
    )
    .with_workspace_id(WORKSPACE)
    .with_workspace_root(PathBuf::from("/old/host/path"));
    metadata.resolved_manifest_snapshot =
        Some(manifest::write_persisted_worker_manifest_snapshot(&resolved()).unwrap());
    atomic_write_json(
        &worker.join(WORKER_METADATA_FILE),
        &metadata,
        "test metadata",
    )
    .unwrap();
    let sessions = WorkerSessionStore::new(worker.join("session")).unwrap();
    let history = session_store::LoggedHistoryEntry {
        item: session_store::LoggedItem::Message {
            role: session_store::LoggedRole::Assistant,
            content: vec![session_store::LoggedContentPart::Text {
                text: "artificial fixture reply".into(),
            }],
        },
        metadata: session_store::LoggedSessionHistoryMetadata {
            entry_id: session_store::LoggedSessionHistoryEntryId("fixed-entry-id".into()),
            origin: session_store::LoggedSessionHistoryOrigin::ModelOutput {
                worker: session_store::LoggedWorkerSubject {
                    workspace_id: Some(WORKSPACE.into()),
                    runtime_id: Some(RUNTIME.into()),
                    worker_id: worker_id().to_string(),
                },
            },
            derivation: None,
            input_metadata: None,
        },
    };
    sessions
        .create_segment(
            session_id,
            segment_id,
            &[
                LogEntry::AnnotatedSegmentStart {
                    ts: 123,
                    session_id,
                    system_prompt: Some("Harmless artificial fixture instructions.".into()),
                    config: Default::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                },
                LogEntry::Invoke {
                    ts: 124,
                    trigger: protocol::InvokeKind::UserSend,
                },
                LogEntry::AnnotatedUserInput {
                    ts: 124,
                    segments: vec![protocol::Segment::text(
                        "Harmless artificial fixture input.",
                    )],
                    extensions: vec![checkpoint_extension(&settled_checkpoint())],
                    history: vec![session_store::LoggedHistoryEntry {
                        item: session_store::LoggedItem::Message {
                            role: session_store::LoggedRole::User,
                            content: vec![session_store::LoggedContentPart::Text {
                                text: "Harmless artificial fixture input.".into(),
                            }],
                        },
                        metadata: session_store::LoggedSessionHistoryMetadata {
                            entry_id: session_store::LoggedSessionHistoryEntryId(
                                "fixed-user-id".into(),
                            ),
                            origin: session_store::LoggedSessionHistoryOrigin::BackendInstruction {
                                operation_id: None,
                            },
                            derivation: None,
                            input_metadata: None,
                        },
                    }],
                },
                LogEntry::AnnotatedAssistantItem {
                    ts: 125,
                    entry: history,
                },
                LogEntry::LlmUsage {
                    ts: 125,
                    history_len: 2,
                    input_total_tokens: 1,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    output_tokens: 1,
                },
                LogEntry::TurnEnd {
                    ts: 126,
                    turn_count: 1,
                },
                LogEntry::RunCompleted {
                    entry_id: Some(session_store::LoggedSessionHistoryEntryId(
                        "fixed-terminal-id".into(),
                    )),
                    ts: 126,
                    interrupted: false,
                    result: agen::EngineResult::Finished,
                    active_run_turn_count: None,
                },
            ],
        )
        .unwrap();
    fs::create_dir_all(worker.join("runs/old-run")).unwrap();
    fs::write(worker.join("runs/old-run/private.log"), "not exported").unwrap();
    drop(opened);
    let template = StoppedWorkerTemplate::extract(&worker).unwrap();
    (temp, worker, template)
}

fn destination(template: &StoppedWorkerTemplate) -> (tempfile::TempDir, FsRuntimeStoreOptions) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    initialize(
        &root,
        template.runtime_id(),
        template.workspace_id(),
        NEW_URL,
    );
    (
        temp,
        FsRuntimeStoreOptions::new(root).with_runtime_id(template.runtime_id()),
    )
}

fn assert_fresh(options: &FsRuntimeStoreOptions) {
    assert_eq!(fs::read_dir(&options.root).unwrap().count(), 1);
    assert!(options.root.join("auth.toml").is_file());
}

#[test]
fn extraction_install_roundtrip_preserves_bytes_and_is_inactive() {
    let (_source, worker, template) = source();
    let original_worker = fs::read(worker.join(WORKER_FILE)).unwrap();
    let original_metadata = fs::read(worker.join(WORKER_METADATA_FILE)).unwrap();
    let exported = serde_json::to_vec(&template).unwrap();
    let exported_text = String::from_utf8(exported.clone()).unwrap();
    for forbidden in [
        "old-config",
        "old-submission-operation",
        "restore_guard",
        "private_key",
        "http://",
        "/old/host",
        "private.log",
        "resolved_manifest_snapshot",
    ] {
        assert!(!exported_text.contains(forbidden));
    }
    let template: StoppedWorkerTemplate = serde_json::from_slice(&exported).unwrap();
    let (_target, options) = destination(&template);
    let auth_before = fs::read(options.root.join("auth.toml")).unwrap();
    template.install(&options, request(), &resolved()).unwrap();
    assert_eq!(
        fs::read(options.root.join("auth.toml")).unwrap(),
        auth_before
    );
    assert_eq!(fs::read(worker.join(WORKER_FILE)).unwrap(), original_worker);
    assert_eq!(
        fs::read(worker.join(WORKER_METADATA_FILE)).unwrap(),
        original_metadata
    );
    let installed = options
        .root
        .join(WORKERS_DIR)
        .join(template.worker_id().to_string());
    assert_eq!(
        fs::read(installed.join("session/session.json")).unwrap(),
        template.session_manifest
    );
    assert_eq!(
        fs::read(installed.join(format!("session/segments/{}.jsonl", template.segment_id())))
            .unwrap(),
        template.segment_jsonl
    );
    assert!(!installed.join("runs").exists());
    let metadata: WorkerMetadata =
        decode(&fs::read(installed.join(WORKER_METADATA_FILE)).unwrap()).unwrap();
    assert_eq!(metadata.active.unwrap().session_id, template.session_id());
    assert!(metadata.workspace_root.is_none() && metadata.resolved_manifest_snapshot.is_some());

    let opened = FsRuntimeStore::open_or_create(options.root.clone(), RUNTIME).unwrap();
    let state = opened.state.unwrap();
    assert!(state.diagnostics.is_empty());
    let saved = state.workers.get(&worker_id()).unwrap();
    assert_eq!(saved.status, WorkerStatus::Stopped);
    assert!(saved.last_finished_submission_request_id.is_none());
    let PersistedWorkerExecutionState::Available(execution) = &saved.execution_state else {
        panic!("not available")
    };
    assert_eq!(execution.restore_intent, WorkerRestoreIntent::Explicit);
    assert!(execution.binding.is_none());
    assert_eq!(execution.request, request());
    // Fresh restore guard, no operation owner/receipt history.
    assert_ne!(
        serde_json::to_vec(&saved.restore_guard).unwrap(),
        serde_json::to_vec(
            &decode::<WorkerAggregateRecord>(&original_worker)
                .unwrap()
                .restore_guard
        )
        .unwrap()
    );
    let guard: GuardState = decode(&serde_json::to_vec(&saved.restore_guard).unwrap()).unwrap();
    assert!(guard.active_request_id.is_none() && guard.owners.is_empty());
    drop(opened.store);

    struct NoExecution;
    impl crate::execution::WorkerExecutionBackend for NoExecution {
        fn backend_id(&self) -> &str {
            "fixture-no-execution"
        }
        fn spawn_worker(
            &self,
            _: crate::execution::WorkerExecutionSpawnRequest,
        ) -> crate::execution::WorkerExecutionSpawnResult {
            panic!("fixture executed")
        }
        fn dispatch_input(
            &self,
            _: &WorkerRef,
            _: crate::interaction::WorkerInput,
        ) -> crate::execution::WorkerExecutionResult {
            panic!("fixture submitted")
        }
        fn restore_worker(
            &self,
            _: crate::execution::WorkerExecutionRestoreRequest,
        ) -> crate::execution::WorkerExecutionSpawnResult {
            panic!("fixture auto-restored")
        }
    }
    let runtime =
        crate::Runtime::with_fs_store_and_execution_backend(options.clone(), Arc::new(NoExecution))
            .unwrap();
    assert_eq!(runtime.list_workers().unwrap().len(), 1);
    assert_eq!(runtime.summary().unwrap().active_worker_count, 0);
    assert_eq!(
        runtime
            .worker_detail(&WorkerRef::new(worker_id()))
            .unwrap()
            .status,
        WorkerStatus::Stopped
    );
    drop(runtime);
    let runtime = crate::Runtime::with_fs_store(options).unwrap();
    assert_eq!(
        runtime.list_workers().unwrap()[0].status,
        WorkerStatus::Stopped
    );
}

#[test]
fn install_rejects_schema_identity_input_and_overwrite() {
    let (_source, _, template) = source();
    for change in [
        "template",
        "runtime-schema",
        "session-schema",
        "session-id",
        "segment-id",
        "runtime-id",
        "workspace-id",
        "worker-id",
        "origin-id",
        "duplicate-id",
        "truncated",
    ] {
        let (_target, options) = destination(&template);
        let mut invalid = template.clone();
        match change {
            "template" => invalid.template_version += 1,
            "runtime-schema" => invalid.runtime_schema_version += 1,
            "session-schema" => {
                invalid.session_manifest = format!(
                    "{{\"schema_version\":999,\"session_id\":\"{}\"}}",
                    invalid.session_id
                )
                .into_bytes()
            }
            "session-id" => invalid.session_id = uuid::Uuid::now_v7(),
            "segment-id" => invalid.segment_id = uuid::Uuid::nil(), // no external attestation: valid new Segment ID is otherwise portable
            "runtime-id" => invalid.runtime_id = "other-runtime".into(),
            "workspace-id" => invalid.workspace_id = "other-workspace".into(),
            "worker-id" => invalid.worker_id = WorkerId::now_v7(),
            "origin-id" => {
                invalid.segment_jsonl = String::from_utf8(invalid.segment_jsonl)
                    .unwrap()
                    .replace(RUNTIME, "other-runtime")
                    .into_bytes()
            }
            "duplicate-id" => {
                let last = invalid
                    .segment_jsonl
                    .split_inclusive(|b| *b == b'\n')
                    .last()
                    .unwrap()
                    .to_vec();
                invalid.segment_jsonl.extend(last);
            }
            "truncated" => {
                invalid.segment_jsonl.pop();
            }
            _ => unreachable!(),
        }
        assert!(
            invalid.install(&options, request(), &resolved()).is_err(),
            "accepted {change}"
        );
        assert_fresh(&options);
    }
    let (_target, options) = destination(&template);
    let mut launch = request();
    launch.initial_input = Some(crate::interaction::WorkerInput::user("do not execute"));
    assert!(template.install(&options, launch, &resolved()).is_err());
    assert_fresh(&options);
    template.install(&options, request(), &resolved()).unwrap();
    let worker_path = options
        .root
        .join(WORKERS_DIR)
        .join(worker_id().to_string())
        .join(WORKER_FILE);
    let before = fs::read(&worker_path).unwrap();
    assert!(template.install(&options, request(), &resolved()).is_err());
    assert_eq!(fs::read(worker_path).unwrap(), before);
}

#[test]
fn rejects_uninitialized_untrusted_live_and_partial_destination() {
    let (_source, _, template) = source();
    let (_target, options) = destination(&template);
    let held = acquire_runtime_store_owner_lock(&options.root).unwrap();
    assert!(template.install(&options, request(), &resolved()).is_err());
    assert_fresh(&options);
    drop(held);
    initialize(&options.root, "wrong-runtime", WORKSPACE, NEW_URL);
    assert!(template.install(&options, request(), &resolved()).is_err());
    initialize(&options.root, RUNTIME, WORKSPACE, "http://127.0.0.1:23456");
    assert!(template.install(&options, request(), &resolved()).is_err());
    fs::remove_file(options.root.join("auth.toml")).unwrap();
    assert!(template.install(&options, request(), &resolved()).is_err());
    initialize(&options.root, RUNTIME, WORKSPACE, NEW_URL);
    fs::create_dir(options.root.join(WORKERS_DIR)).unwrap();
    assert!(template.install(&options, request(), &resolved()).is_err());
}

#[test]
fn extraction_rejects_nonstopped_pending_schema_and_metadata_mismatch() {
    for change in [
        "running",
        "pending",
        "guard-pending",
        "attachment",
        "schema",
        "worker-id",
        "metadata-workspace",
        "metadata-session",
        "artifact",
        "secret",
        "url",
        "path",
    ] {
        let (_source, worker, _) = source();
        let mut aggregate: WorkerAggregateRecord =
            decode(&fs::read(worker.join(WORKER_FILE)).unwrap()).unwrap();
        match change {
            "running" => aggregate.status = WorkerStatus::Running,
            "pending" => {
                aggregate.execution_state = WorkerExecutionStateRecord::ReconciliationRequired(
                    PersistedWorkerLifecycleOperation::Restore(PersistedWorkerRestoreOperation {
                        operation_id: WorkerLifecycleOperationId::new(),
                        mode: PersistedWorkerRestoreMode::Explicit,
                        commit: crate::runtime::RestoreCommitFact::NotCommitted,
                        source_repair_possible: false,
                        request: request(),
                        binding: None,
                        last_settled_status: WorkerStatus::Stopped,
                    }),
                )
            }
            "guard-pending" => aggregate.restore_guard = serde_json::from_str(r#"{"observation_token":"old-token","active_request_id":"pending-restore","owners":{}}"#).unwrap(),
            "attachment" => aggregate.logical_workdir_attachments.push(LogicalWorkdirAttachment {
                alias: workdir::WorkdirAttachmentAlias::new("source").unwrap(),
                working_directory_id: "old-workdir".into(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            }),
            "schema" => aggregate.schema_version += 1,
            "worker-id" => aggregate.worker_id = WorkerId::now_v7(),
            "metadata-workspace" | "metadata-session" => {
                let mut metadata: WorkerMetadata =
                    decode(&fs::read(worker.join(WORKER_METADATA_FILE)).unwrap()).unwrap();
                if change == "metadata-workspace" {
                    metadata.workspace_id = Some("wrong-workspace".into());
                } else {
                    metadata.active.as_mut().unwrap().session_id = uuid::Uuid::now_v7();
                }
                atomic_write_json(
                    &worker.join(WORKER_METADATA_FILE),
                    &metadata,
                    "test mismatch",
                )
                .unwrap();
            }
            "artifact" => {
                fs::create_dir_all(worker.join("session/artifacts/paste")).unwrap();
                fs::write(
                    worker.join("session/artifacts/paste/payload"),
                    "unsupported",
                )
                .unwrap();
            }
            "secret" | "url" | "path" => {
                let segment = fs::read_dir(worker.join("session/segments"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let replacement = match change {
                    "secret" => "Bearer secret",
                    "url" => "https://old-host.invalid",
                    _ => "/old/host/path",
                };
                let text = fs::read_to_string(&segment)
                    .unwrap()
                    .replace("artificial fixture reply", replacement);
                fs::write(segment, text).unwrap();
            }
            _ => unreachable!(),
        }
        atomic_write_json(&worker.join(WORKER_FILE), &aggregate, "test aggregate").unwrap();
        assert!(
            StoppedWorkerTemplate::extract(&worker).is_err(),
            "accepted {change}"
        );
    }
    let (_source, worker, _) = source();
    let held =
        acquire_runtime_store_owner_lock(worker.parent().unwrap().parent().unwrap()).unwrap();
    assert!(StoppedWorkerTemplate::extract(&worker).is_err());
    drop(held);
}

#[test]
fn schema_v12_settled_template_installs_current_aggregate_without_rewriting_session() {
    let (_source, _, mut template) = source();
    template.runtime_schema_version = RESTORE_ADMISSION_SCHEMA_VERSION;
    let bytes = serde_json::to_vec(&template).unwrap();
    let (_target, options) = destination(&template);
    template.install(&options, request(), &resolved()).unwrap();
    assert_eq!(serde_json::to_vec(&template).unwrap(), bytes);
    let worker = options.root.join(WORKERS_DIR).join(worker_id().to_string());
    assert_eq!(
        fs::read(worker.join("session/session.json")).unwrap(),
        template.session_manifest
    );
    assert_eq!(
        fs::read(worker.join(format!("session/segments/{}.jsonl", template.segment_id))).unwrap(),
        template.segment_jsonl
    );
    let aggregate: WorkerAggregateRecord =
        read_bounded_json(&worker.join(WORKER_FILE), "read installed fixture").unwrap();
    let saved = aggregate.validate(&worker.join(WORKER_FILE)).unwrap();
    assert_eq!(saved.status, WorkerStatus::Stopped);
    assert!(matches!(
        saved.execution_state,
        PersistedWorkerExecutionState::Available(_)
    ));
    // Unsupported template schemas do not enter the versioned reader.
    template.runtime_schema_version = RESTORE_ADMISSION_SCHEMA_VERSION - 1;
    assert!(template.validate_shape().is_err());
    template.runtime_schema_version = SCHEMA_VERSION + 1;
    assert!(template.validate_shape().is_err());
}

#[test]
fn fresh_manifest_and_memory_settings_are_required() {
    let (_source, _, template) = source();
    let (_target, options) = destination(&template);
    let mut manifest = resolved();
    manifest.worker.name = "wrong-worker".into();
    assert!(template.install(&options, request(), &manifest).is_err());
    let mut binding = request();
    binding.memory_settings = None;
    assert!(template.install(&options, binding, &resolved()).is_err());
    let mut binding = request();
    binding.memory_settings.as_mut().unwrap().workspace_id = "other-workspace".into();
    assert!(template.install(&options, binding, &resolved()).is_err());
    assert_fresh(&options);
}

#[test]
fn settled_checkpoints_preserve_receipts_but_refuse_pending_work() {
    let mut checkpoint = settled_checkpoint();
    assert!(validate_settled_activation(&checkpoint_extension(&checkpoint)).is_ok());
    checkpoint.pending.push(1);
    assert!(validate_settled_activation(&checkpoint_extension(&checkpoint)).is_err());
    checkpoint.pending.clear();
    checkpoint.activating = Some(1);
    assert!(validate_settled_activation(&checkpoint_extension(&checkpoint)).is_err());
    checkpoint.activating = None;
    checkpoint.invocation_receipts.push(1);
    assert!(validate_settled_activation(&checkpoint_extension(&checkpoint)).is_err());
    checkpoint.invocation_receipts.clear();
    checkpoint.invocation_recovery_blocked = true;
    assert!(validate_settled_activation(&checkpoint_extension(&checkpoint)).is_err());
}

// Deliberately corrupt only test-owned synthetic source files, never the lab.
fn assert_rejected_session_bytes(worker: &Path, template: &StoppedWorkerTemplate, bytes: Vec<u8>) {
    let path = worker.join(format!("session/segments/{}.jsonl", template.segment_id()));
    fs::write(path, &bytes).unwrap();
    let extraction_error = StoppedWorkerTemplate::extract(worker)
        .err()
        .expect("invalid source accepted");
    assert!(
        !extraction_error
            .to_string()
            .contains("synthetic-discarded-material")
    );
    let (_target, options) = destination(template);
    let mut invalid = template.clone();
    invalid.segment_jsonl = bytes;
    let installation_error = invalid
        .install(&options, request(), &resolved())
        .unwrap_err();
    assert!(
        !installation_error
            .to_string()
            .contains("synthetic-discarded-material")
    );
    assert_fresh(&options);
}

fn synthetic_logs(template: &StoppedWorkerTemplate) -> Vec<LogEntry> {
    template
        .segment_jsonl
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

fn encode_synthetic_logs(logs: &[LogEntry]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for log in logs {
        serde_json::to_writer(&mut bytes, log).unwrap();
        bytes.push(b'\n');
    }
    bytes
}

#[tokio::test]
async fn missing_system_prompt_matches_worker_restore_rejection() {
    let (_source, worker_dir, template) = source();
    let mut logs = synthetic_logs(&template);
    assert!(session_store::collect_state(&logs).system_prompt.is_some());
    let LogEntry::AnnotatedSegmentStart { system_prompt, .. } = &mut logs[0] else {
        panic!("missing seed")
    };
    *system_prompt = None;
    assert_rejected_session_bytes(&worker_dir, &template, encode_synthetic_logs(&logs));
    // The owning Worker constructor rejects this before model-client creation,
    // allocation, or execution. Do not start a controller or submit any input.
    let store = session_store::CombinedStore::new(
        WorkerSessionStore::open_read_only(worker_dir.join("session")).unwrap(),
        session_store::WorkerAggregateStore::open_read_only(&worker_dir, template.worker_name())
            .unwrap(),
    );
    let restored = worker::Worker::restore_from_manifest(
        template.session_id(),
        template.segment_id(),
        resolved(),
        store,
        worker::PromptCatalogSource::builtins_only(),
    )
    .await;
    assert!(matches!(
        restored,
        Err(worker::WorkerError::SegmentSystemPromptMissing { .. })
    ));
}

#[test]
fn malformed_submission_and_notification_receipts_fail_extraction_and_install() {
    let (_source, worker, template) = source();
    for notification in [false, true] {
        let mut checkpoint = settled_checkpoint();
        if notification {
            checkpoint.notification_receipts =
                vec![ReceiptFixture::Malformed(1), ReceiptFixture::Malformed(2)];
        } else {
            checkpoint.receipts = vec![ReceiptFixture::Malformed(1), ReceiptFixture::Malformed(2)];
        }
        let mut logs = synthetic_logs(&template);
        for log in &mut logs {
            if let LogEntry::AnnotatedUserInput { extensions, .. } = log {
                *extensions = vec![checkpoint_extension(&checkpoint)];
            }
        }
        assert_rejected_session_bytes(&worker, &template, encode_synthetic_logs(&logs));
    }
}

#[test]
fn unknown_fields_at_every_retained_record_depth_are_rejected_without_secret_diagnostics() {
    let (_source, worker, template) = source();
    let original = String::from_utf8(template.segment_jsonl.clone()).unwrap();
    // These values are harmless synthetic markers, not lab/private material.
    // The compatibility decoder normally discards many of these fields.
    let locations = [
        r#""kind":"annotated_segment_start""#,
        r#""kind":"invoke""#,
        r#""kind":"annotated_user_input""#,
        r#""kind":"annotated_assistant_item""#,
        r#""kind":"llm_usage""#,
        r#""kind":"turn_end""#,
        r#""kind":"run_completed""#,
        r#""max_tokens":null"#,
        r#""kind":"message","role":"assistant""#,
        r#""kind":"text","text":"artificial fixture reply""#,
        r#""kind":"text","content":"Harmless artificial fixture input.""#,
        r#""entry_id":"fixed-entry-id""#,
        r#""kind":"model_output""#,
        r#""worker_id":"01a1657d-b762-7791-87ae-a49f28cadbce""#,
        r#""domain":"worker.pending_activations.v1""#,
        r#""next_activation_sequence":2"#,
        r#""submission_request_id":"completed-submission-request""#,
        r#""notification_request_id":"completed-notification-request""#,
    ];
    for location in locations {
        assert!(original.contains(location));
        for extra in [
            r#", "reasoning":{"text":"synthetic-discarded-material"}"#,
            r#", "encrypted_content":"synthetic-discarded-material""#,
        ] {
            let altered = original.replacen(location, &format!("{location}{extra}"), 1);
            assert_rejected_session_bytes(&worker, &template, altered.into_bytes());
        }
    }
    let duplicate = original.replacen(
        r#""role":"assistant""#,
        r#""role":"assistant","role":"assistant""#,
        1,
    );
    assert_rejected_session_bytes(&worker, &template, duplicate.into_bytes());
}

#[test]
fn opaque_feature_payloads_cannot_bypass_strict_session_validation() {
    let (_source, worker, template) = source();
    let mut logs = synthetic_logs(&template);
    for log in &mut logs {
        if let LogEntry::AnnotatedUserInput { history, .. } = log {
            if let session_store::LoggedItem::Message { role, .. } = &mut history[0].item {
                *role = session_store::LoggedRole::System;
            }
            history[0].metadata.input_metadata = Some(
                session_store::HistoryInputMetadata::new(
                    session_store::HistoryInputUsePolicy {
                        recording_material: session_store::InputEvidenceUse::Exclude,
                        memory_evidence: session_store::InputEvidenceUse::Exclude,
                        observation: session_store::InputObservationUse::Hidden,
                        compaction: session_store::InputCompactionUse::Retain,
                    },
                    Some(
                        session_store::FeatureInputPayload::encode(
                            "fixture.test",
                            1,
                            &BTreeMap::from([("reasoning", "harmless synthetic payload")]),
                        )
                        .unwrap(),
                    ),
                    None,
                )
                .unwrap(),
            );
        }
    }
    assert_rejected_session_bytes(&worker, &template, encode_synthetic_logs(&logs));
}

#[cfg(unix)]
#[test]
fn refuses_source_and_destination_symlinks() {
    let (_source, worker, template) = source();
    let (_target, options) = destination(&template);
    std::os::unix::fs::symlink(worker.join(WORKER_FILE), options.root.join("linked")).unwrap();
    assert!(template.install(&options, request(), &resolved()).is_err());
    fs::remove_file(options.root.join("linked")).unwrap();
    std::os::unix::fs::symlink(worker.join(WORKER_FILE), worker.join("runs/link")).unwrap();
    assert!(StoppedWorkerTemplate::extract(&worker).is_err());
}

/// Opt-in only: source path supplied explicitly; assertions never display content.
#[test]
#[ignore = "requires the approved artificial lab aggregate via YOI_STOPPED_FIXTURE_SOURCE"]
fn approved_lab_extract_install() {
    let path = PathBuf::from(
        std::env::var_os("YOI_STOPPED_FIXTURE_SOURCE").expect("explicit source required"),
    );
    let template = StoppedWorkerTemplate::extract(&path).unwrap();
    let (_target, options) = destination(&template);
    let mut bind = request();
    bind.worker_id = template.worker_id();
    bind.workspace_api.as_mut().unwrap().workspace_id = template.workspace_id().into();
    bind.memory_settings.as_mut().unwrap().workspace_id = template.workspace_id().into();
    let mut manifest = resolved();
    manifest.worker.name = template.worker_name();
    template.install(&options, bind, &manifest).unwrap();
    let installed = options
        .root
        .join(WORKERS_DIR)
        .join(template.worker_id().to_string());
    for relative in [
        "session/session.json".to_string(),
        format!("session/segments/{}.jsonl", template.segment_id()),
    ] {
        assert!(
            fs::read(path.join(&relative)).unwrap() == fs::read(installed.join(relative)).unwrap()
        );
    }
    let runtime = crate::Runtime::with_fs_store(options).unwrap();
    assert_eq!(runtime.list_workers().unwrap().len(), 1);
    assert_eq!(runtime.summary().unwrap().active_worker_count, 0);
}
