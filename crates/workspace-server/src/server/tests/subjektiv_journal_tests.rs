//! Backend/Job/Subjektiv integration with real SQLite. RuntimeFixture also connects
//! the embedded Runtime, canonical source reader, pin/archive/Purge and real Session files.
//! Controller execution is deterministic; first-generation fixtures supply public bytes.
//! Tool-result persistence is a fixture boundary; no signed remote HTTP or model evaluation.
use super::*;
use crate::backend_job::{BackendJobGrants, SubjektivJournalGrant, journal_serialization_key};
use feature_storage::FeatureStorageError;
use sha2::{Digest, Sha256};
use subjektiv::journal::transport::*;
use subjektiv::journal::*;

#[path = "subjektiv_journal_connected_tests.rs"]
mod connected;

const JOURNAL_OPERATIONS: &[&str] = &[
    "session.record",
    "journal.startup.read",
    "journal.startup.supply",
    "journal.compaction.read",
    "journal.compaction.supply",
    "journal.recording.request",
    "journal.job.input.read",
    "journal.job.input.supply",
    "journal.entry.create",
    "journal.entry.rewrite",
    "journal.entry.fix",
    "journal.position.update",
    "journal.unchanged.confirm",
    "journal.checkpoint",
    "journal.state.read",
    "journal.operation.query",
    "journal.operation.coverage.read",
    "journal.operation.list",
    "journal.text.read",
    "journal.finish",
];

fn grant_journal_operations(store: &crate::subjektiv::SubjektivStore, subject: &str) {
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissionsUpdate,
    };
    let previous = store.subject_permissions(subject).unwrap();
    store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id: format!("journal-fixture-grant-{subject}"),
                expected_change_id: previous.map(|policy| policy.change_id),
                execution: JOURNAL_OPERATIONS
                    .iter()
                    .map(|operation| ScopedPermission {
                        operation: (*operation).into(),
                        resource: PermissionResource::Subject {
                            workspace_id: TEST_WORKSPACE_ID.into(),
                            subject_id: subject.into(),
                        },
                    })
                    .collect(),
                delegation: vec![],
            },
        )
        .unwrap();
}

fn spawn_journal_source(api: &WorkspaceApi, subject: &str, name: &str) -> RuntimeWorkerRef {
    seed_test_delivery_binding(api);
    api.spawn_workspace_worker(
        EMBEDDED_WORKER_RUNTIME_ID,
        WorkerSpawnRequest {
            requested_worker_name: Some(name.into()),
            intent: WorkerSpawnIntent::WorkspaceCompanion,
            singleton_key: Some(crate::subjektiv::subject_worker_singleton_key(subject).unwrap()),
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachment_requests: vec![],
            resolved_workdir_attachment_requests: vec![],
            resolved_workdir_attachments: vec![],
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: vec![],
            resolved_workspace_api: None,
            resolved_memory_settings: None,
            resolved_subjektiv_attached: false,
            resolved_control_operation: None,
            resolved_execution_grant: None,
        },
    )
    .unwrap()
    .worker
    .unwrap()
    .worker
}

fn bind_fixture_session(
    store: &crate::subjektiv::SubjektivStore,
    attr: crate::subjektiv::SubjectSessionAttribution,
) -> JournalHostContext {
    if store
        .subject_permissions(&attr.subject_id)
        .unwrap()
        .is_none()
    {
        grant_journal_operations(store, &attr.subject_id);
    }
    let binding = store
        .claim_journal_writer(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .unwrap();
    store.bind_journal_session(&binding, attr).unwrap()
}

#[tokio::test]
async fn journal_job_reserves_before_binding_and_retains_partial_receipts_on_cancel() {
    let Fixture {
        _dir,
        api,
        execution,
        store,
        host,
        request,
        range,
        reserved,
        source,
        id,
    } = Fixture::new().await;
    assert_eq!(reserved.attempt.state, BackendJobAttemptState::Reserved);
    assert!(
        store
            .active_journal_generation(&host, &id)
            .unwrap()
            .is_none()
    );
    execution.take_inputs();
    subjektiv_journal::prepare(&api, &reserved).unwrap();
    let original = store
        .active_journal_generation(&host, &id)
        .unwrap()
        .unwrap();
    subjektiv_journal::prepare(&api, &reserved).unwrap();
    assert_eq!(
        store
            .active_journal_generation(&host, &id)
            .unwrap()
            .unwrap(),
        original
    );
    assert!(execution.take_inputs().is_empty());
    let dispatched = api.dispatch_backend_job_reservation(reserved).unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    assert_eq!(execution.take_inputs().len(), 1);
    assert!(subjektiv_journal::execute(&api, &source, JournalJobRequest::ReadState).is_err());
    assert!(subjektiv_journal::finish(&api, worker).is_err());
    assert!(
        subjektiv_journal::execute(
            &api,
            worker,
            JournalJobRequest::ReadText {
                read: JournalTextRead {
                    target: JournalTextTarget::Source {
                        pointer: JournalSourcePointer {
                            segment_id: Uuid::new_v4(),
                            entry_ref: "E00002".into()
                        }
                    },
                    offset: 0,
                    limit: 1024,
                    content_token: None,
                }
            }
        )
        .is_err()
    );
    deliver_fixture_source(&store, &host, &id, &range);
    let operation = JournalOperation {
        request_id: "create-retained".into(),
        expected_state: original.initial_journal.state_token,
        action: JournalAction::Create {
            title: "ongoing work".into(),
            body: "Proposed; not executed".into(),
            ongoing: "Awaiting correction".into(),
            state: JournalEntryState::Open,
            source_end: None,
            correction_refs: vec![],
        },
        covered_ranges: vec![range.clone()],
    };
    let saved = subjektiv_journal::execute(
        &api,
        worker,
        JournalJobRequest::Execute {
            operation: operation.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        subjektiv_journal::execute(
            &api,
            worker,
            JournalJobRequest::Execute {
                operation: operation.clone()
            }
        )
        .unwrap(),
        saved
    );
    let mut conflict = operation;
    conflict.action = JournalAction::ConfirmUnchanged;
    assert!(
        subjektiv_journal::execute(
            &api,
            worker,
            JournalJobRequest::Execute {
                operation: conflict
            }
        )
        .is_err()
    );
    assert_eq!(store.read_journal(&host, &id).unwrap().process_order, 0);
    assert!(subjektiv_journal::finish(&api, worker).is_err());
    api.finish_backend_job_attempt(
        &request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobAttemptState::Failed,
        "cancelled",
        "test cancellation",
    )
    .unwrap();
    assert!(subjektiv_journal::execute(&api, worker, JournalJobRequest::ReadState).is_err());
    assert!(
        store
            .query_journal_operation(&host, &id, "create-retained")
            .unwrap()
            .is_some()
    );
    assert!(store.journal_retention_required(&host).unwrap());
    assert_eq!(
        store
            .list_journal_entries(&host, &id, None, 32)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn journal_route_rejects_missing_authenticated_source_before_admission() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let result = subjektiv_journal::recording_route(
        State(api),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        None,
        Json(JournalRecordingRequest {
            reason: JournalRecordingReason::RunCommitted,
        }),
    )
    .await;
    assert!(matches!(
        result.unwrap_err().error,
        Error::WorkspacePermissionDenied(_)
    ));
}

// Saturate the real finite Host lane without blocking the separate recording callback lane.
// Drop releases both workers even if a regression assertion panics.
struct JournalHostCapacityBarrier(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl JournalHostCapacityBarrier {
    async fn hold(api: &WorkspaceApi) -> Self {
        let barrier = Self(Arc::new((
            std::sync::Mutex::new(false),
            std::sync::Condvar::new(),
        )));
        let host = api.host_executors.host().unwrap();
        for _ in 0..2 {
            let held = barrier.0.clone();
            let (started, ready) = tokio::sync::oneshot::channel();
            host.dispatch(move || {
                let _ = started.send(());
                let (lock, changed) = &*held;
                let (_guard, timeout) = changed
                    .wait_timeout_while(
                        lock.lock().unwrap(),
                        std::time::Duration::from_secs(30),
                        |released| !*released,
                    )
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "Host capacity barrier was not released"
                );
            })
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), ready)
                .await
                .unwrap()
                .unwrap();
        }
        for _ in 0..64 {
            match host.dispatch(|| {}) {
                Ok(()) => {}
                Err(Error::DbCapacityExceeded) => return barrier,
                other => panic!("unexpected Host admission: {other:?}"),
            }
        }
        panic!("finite Host lane did not reject admission");
    }
}

impl Drop for JournalHostCapacityBarrier {
    fn drop(&mut self) {
        let (lock, changed) = &*self.0;
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
}

#[tokio::test]
async fn journal_host_capacity_rejection_keeps_same_reservation_autonomously_owned() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("capacity-recording").unwrap())
        .unwrap();
    let source = spawn_journal_source(&api, &subject.id, "capacity-source");
    let session = session_store::new_session_id();
    seed_session(
        &session_root(dir.path(), &source),
        session,
        message(SOURCE_SENTINEL),
        session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
    );
    let attr = store
        .record_session_attribution(
            crate::subjektiv::SubjectSessionAttribution::new(
                &subject.id,
                &source.runtime_id,
                &source.worker_id,
                session.to_string(),
            )
            .unwrap(),
        )
        .unwrap();
    bind_fixture_session(&store, attr);
    let callback = || {
        subjektiv_journal::recording_route(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            Some(axum::Extension(
                crate::worker_source::VerifiedRuntimeRequestSource {
                    runtime_id: source.runtime_id.clone(),
                    worker_id: Some(source.worker_id.clone()),
                    authentication_binding_id: "fixture-verified-binding".into(),
                    token_id_hash: None,
                },
            )),
            Json(JournalRecordingRequest {
                reason: JournalRecordingReason::RunCommitted,
            }),
        )
    };
    let held = JournalHostCapacityBarrier::hold(&api).await;
    assert_eq!(
        callback().await.unwrap().0,
        JournalRecordingResponse::Accepted
    );
    let jobs = api
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &session.to_string(), None, 100)
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    let attempt_id = crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt);
    let before = api
        .store
        .get_backend_job_attempt(TEST_WORKSPACE_ID, &job.request.job_id, &attempt_id)
        .unwrap()
        .unwrap();
    assert_eq!(before.state, BackendJobAttemptState::Reserved);
    assert!(before.worker.is_none());
    assert!(execution.take_inputs().is_empty());
    assert!(
        api.backend_job_recovery
            .recovery_owner_ids()
            .contains(&attempt_id),
        "accepted Journal reservation lost its owner when Host admission was rejected"
    );
    drop(held);
    // No request replay, manual sweep, restart or replacement Job drives this transition.
    let dispatched = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(25));
        loop {
            tick.tick().await;
            let current = api
                .store
                .get_backend_job_attempt(TEST_WORKSPACE_ID, &job.request.job_id, &attempt_id)
                .unwrap()
                .unwrap();
            if current.state == BackendJobAttemptState::Dispatched
                && !execution.inputs.lock().unwrap().is_empty()
            {
                break current;
            }
        }
    })
    .await
    .expect("same accepted Journal did not recover after Host capacity returned");
    assert_eq!(dispatched.attempt_id, before.attempt_id);
    assert_eq!(dispatched.deadline_at, before.deadline_at);
    assert_eq!(dispatched.input_digest, before.input_digest);
    assert_eq!(
        callback().await.unwrap().0,
        JournalRecordingResponse::Accepted
    );
    let inputs = execution.take_inputs();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].1, job.request.worker_input(&attempt_id).unwrap());
    assert_eq!(
        api.store
            .list_backend_job_attempts(TEST_WORKSPACE_ID, &job.request.job_id)
            .unwrap()
            .len(),
        1
    );
    api.stop_backend_job_recovery().await;
}

struct Fixture {
    _dir: tempfile::TempDir,
    api: WorkspaceApi,
    execution: Arc<DeterministicExecutionBackend>,
    store: crate::subjektiv::SubjektivStore,
    host: JournalHostContext,
    request: BackendJobRequest,
    range: JournalSourceRange,
    reserved: BackendJobReservation,
    source: RuntimeWorkerRef,
    id: String,
}
fn fixture_capture(range: &JournalSourceRange, text: &str) -> session_store::capture::CaptureDto {
    use session_store::capture::*;
    let descriptor = session_store::public_material::PublicMaterialDescriptor {
        identity: session_store::public_material::PublicMaterialIdentity {
            segment_id: range.source.pointer.segment_id,
            entry_ref: range.source.pointer.entry_ref.clone(),
        },
        canonical_order: range.source.order,
        origin: session_store::public_material::PublicMaterialOrigin::History {
            origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
        },
        sha256: range.source.content_sha256.clone(),
        total_bytes: range.source.total_bytes,
    };
    CaptureDto {
        version: 1,
        session_id: range.source.session_id.clone(),
        segment_id: range.source.pointer.segment_id.to_string(),
        view_entry_count: range.source.order + 1,
        entries: vec![CaptureEntryDto {
            entry_id: range
                .source
                .pointer
                .entry_ref
                .trim_start_matches('E')
                .into(),
            order: range.source.order,
            source: CaptureSource {
                segment_id: range.source.pointer.segment_id.to_string(),
                entry_index: range.source.order,
                provenance: protocol::SessionEntryProvenance::HumanInput,
                history_origin: Some(session_store::LoggedSessionHistoryOrigin::LocalHumanInput),
                derived_from: vec![],
            },
            policy: CaptureUsePolicy {
                recording_material: true,
                memory_evidence: false,
                observation_readable: false,
            },
            body: CaptureBody::CanonicalPublic {
                descriptor,
                byte_offset: range.bytes.start,
                text: text.into(),
                boundary: None,
            },
        }],
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("journal-test").unwrap())
            .unwrap();
        let source = spawn_journal_source(&api, &subject.id, "journal-source");
        let attr = store
            .record_session_attribution(
                crate::subjektiv::SubjectSessionAttribution::new(
                    &subject.id,
                    &source.runtime_id,
                    &source.worker_id,
                    Uuid::new_v4().to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        let host = bind_fixture_session(&store, attr.clone());
        let text = "original public input";
        let range = JournalSourceRange {
            source: JournalSource {
                session_id: attr.session_id.clone(),
                pointer: JournalSourcePointer {
                    segment_id: Uuid::new_v4(),
                    entry_ref: "E00001".into(),
                },
                order: 0,
                content_sha256: Sha256::digest(text.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                total_bytes: text.len() as u64,
            },
            bytes: JournalByteRange {
                start: 0,
                end: text.len() as u64,
            },
        };
        let id = store
            .accept_journal_sources(&host, "test", &[range.clone()])
            .unwrap();
        let request = BackendJobRequest {
            job_id: "journal-job-test".into(),
            purpose: "subjektiv_journal".into(),
            input_ref: format!("journal://{id}"),
            input: serde_json::to_value(
                subjektiv::journal::recording::JournalInput::build(
                    &store,
                    &host,
                    &id,
                    |_, _, _| Ok(fixture_capture(&range, text)),
                )
                .unwrap(),
            )
            .unwrap(),
            instruction: "Record this public input with sequential tools".into(),
            profile: "builtin:subjektiv-journal-recording".into(),
            grants: BackendJobGrants {
                subjektiv_journal: Some(SubjektivJournalGrant {
                    binding_id: host.writer_binding_id().unwrap().into(),
                    subject_id: subject.id,
                    session_id: attr.session_id,
                    journal_id: id.clone(),
                    selected_ranges: vec![range.clone()],
                }),
                ..Default::default()
            },
            serialization_key: Some(journal_serialization_key(&id)),
            source_worker: Some(source.clone()),
            notification_target: None,
            limits: Default::default(),
        };
        let reserved = api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
            .unwrap();

        Self {
            _dir: dir,
            api,
            execution,
            store,
            host,
            request,
            range,
            reserved,
            source,
            id,
        }
    }
}

#[tokio::test]
async fn journal_completion_replays_only_saved_checkpoint_and_fences_late_operations() {
    let f = Fixture::new().await;
    let dispatched = f.api.dispatch_backend_job_reservation(f.reserved).unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let initial = f
        .store
        .active_journal_generation(&f.host, &f.id)
        .unwrap()
        .unwrap();
    deliver_fixture_source(&f.store, &f.host, &f.id, &f.range);
    let response = subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "no-change".into(),
                expected_state: initial.initial_journal.state_token,
                action: JournalAction::ConfirmUnchanged,
                covered_ranges: vec![f.range.clone()],
            },
        },
    )
    .unwrap();
    let JournalJobResponse::Saved(receipt) = response else {
        panic!("receipt required")
    };
    assert!(subjektiv_journal::finish(&f.api, worker).is_err());
    let position = save_fixture_position(&f.api, worker, receipt.state_token);
    subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "checkpoint".into(),
                expected_state: position,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["no-change".into()],
                },
                covered_ranges: vec![f.range],
            },
        },
    )
    .unwrap();
    let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
    assert_eq!(completion.process_order, 1);
    assert_eq!(completion.process_byte, 0);
    assert_eq!(
        subjektiv_journal::finish(&f.api, worker).unwrap(),
        completion
    );
    let value = serde_json::to_value(&completion).unwrap();
    subjektiv_journal::validate_result(&f.api, &f.request, &dispatched.attempt, &value).unwrap();
    let mut extra = value.clone();
    extra["raw_source"] = serde_json::json!(SOURCE_SENTINEL);
    assert!(
        subjektiv_journal::validate_result(&f.api, &f.request, &dispatched.attempt, &extra)
            .is_err(),
        "result extras must not become a permanent source-text copy in the Jobs ledger"
    );
    let mut wrong = completion;
    wrong.state_token = "not-the-saved-state".into();
    assert!(
        subjektiv_journal::validate_result(
            &f.api,
            &f.request,
            &dispatched.attempt,
            &serde_json::to_value(wrong).unwrap()
        )
        .is_err()
    );
    assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
    assert!(!f.store.journal_retention_required(&f.host).unwrap());
    // Store authority boundary: Host Job-state gating is separate from revocation itself.
    let grant = f.request.grants.subjektiv_journal.as_ref().unwrap();
    let attr = f
        .store
        .session_attribution(&grant.session_id)
        .unwrap()
        .unwrap();
    let binding = JournalWriterBinding {
        subject_id: attr.subject_id.clone(),
        runtime_id: attr.runtime_id.clone(),
        worker_id: attr.worker_id.clone(),
        binding_id: grant.binding_id.clone(),
    };
    f.store.release_journal_writer(&binding).unwrap();
    let next = f
        .store
        .claim_journal_writer(&attr.subject_id, &attr.runtime_id, &attr.worker_id)
        .unwrap();
    assert_ne!(next.binding_id, binding.binding_id);
    assert!(f.store.bind_journal_session(&next, attr).is_err());
    let mut stale = dispatched.clone();
    stale
        .job
        .request
        .grants
        .subjektiv_journal
        .as_mut()
        .unwrap()
        .binding_id = next.binding_id;
    assert!(subjektiv_journal::prepare(&f.api, &stale).is_err());
    assert!(
        f.store
            .query_journal_operation(&f.host, &f.id, "no-change")
            .unwrap()
            .is_some()
    );
    // Exact saved completion is still recoverable; it is not a new write grant.
    subjektiv_journal::validate_result(&f.api, &f.request, &dispatched.attempt, &value).unwrap();
}

fn save_fixture_position(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    expected_state: String,
) -> String {
    let JournalJobResponse::Saved(receipt) = subjektiv_journal::execute(
        api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "position".into(),
                expected_state,
                action: JournalAction::UpdatePosition {
                    current_position: "No activity change; work remains pending.".into(),
                },
                covered_ranges: vec![],
            },
        },
    )
    .unwrap() else {
        panic!("saved position receipt required")
    };
    receipt.state_token
}

fn deliver_fixture_source(
    store: &crate::subjektiv::SubjektivStore,
    host: &JournalHostContext,
    id: &str,
    range: &JournalSourceRange,
) {
    let g = store.active_journal_generation(host, id).unwrap().unwrap();
    let context = JournalJobContext::validated(store, host, &g.id, g.job).unwrap();
    subjektiv::journal::service::execute_bounded(
        store,
        &context,
        JournalJobRequest::ReadText {
            read: JournalTextRead {
                target: JournalTextTarget::Source {
                    pointer: range.source.pointer.clone(),
                },
                offset: 0,
                limit: 16384,
                content_token: None,
            },
        },
        |source, start, _| {
            assert_eq!(source, &range.source);
            assert_eq!(start, 0);
            Ok(subjektiv::journal::recording::JournalMaterial {
                range: range.clone(),
                text: "original public input".into(),
            })
        },
    )
    .unwrap();
}

// Backend -> embedded Runtime -> canonical reader/retention -> real filesystem/SQLite.
// Controller execution is deterministic. Public history and the returned ReadJournalText
// tool result are persisted with the production Session store; this is not model evaluation.
struct RuntimeFixture {
    dir: tempfile::TempDir,
    api: WorkspaceApi,
    execution: Arc<DeterministicExecutionBackend>,
    job_store: Arc<SqliteWorkspaceStore>,
    store: crate::subjektiv::SubjektivStore,
    host: JournalHostContext,
    source: RuntimeWorkerRef,
    session: session_store::SessionId,
    request: BackendJobRequest,
    reserved: BackendJobReservation,
    range: JournalSourceRange,
    id: String,
}
const SOURCE_SENTINEL: &str = "journal-source-only-65db55a2-do-not-copy";
fn session_root(root: &std::path::Path, worker: &RuntimeWorkerRef) -> PathBuf {
    root.join(".test-embedded-runtime-store/workers")
        .join(&worker.worker_id)
        .join("session")
}
fn seed_session(
    root: &std::path::Path,
    session: session_store::SessionId,
    item: session_store::LoggedItem,
    origin: session_store::LoggedSessionHistoryOrigin,
) {
    seed_session_history(
        root,
        session,
        vec![session_store::LoggedHistoryEntry {
            item,
            metadata: session_store::LoggedSessionHistoryMetadata {
                input_metadata: None,
                entry_id: session_store::LoggedSessionHistoryEntryId("read-source-original".into()),
                origin,
                derivation: None,
            },
        }],
    );
}
fn seed_session_history(
    root: &std::path::Path,
    session: session_store::SessionId,
    history: Vec<session_store::LoggedHistoryEntry>,
) {
    use session_store::{LogEntry, Store as _, WorkerSessionStore};
    let segment = Uuid::new_v4();
    WorkerSessionStore::new(root)
        .unwrap()
        .create_segment(
            session,
            segment,
            &[LogEntry::AnnotatedSegmentStart {
                ts: 1,
                session_id: session,
                system_prompt: None,
                config: agen::llm_client::RequestConfig::default(),
                history,
                forked_from: None,
                compacted_from: None,
            }],
        )
        .unwrap();
    use session_store::WorkerMetadataStore as _;
    let aggregate = root.parent().unwrap();
    let name = format!(
        "worker-runtime-{}",
        aggregate.file_name().unwrap().to_str().unwrap()
    );
    session_store::WorkerAggregateStore::new(aggregate, &name)
        .unwrap()
        .write(
            &session_store::WorkerMetadata::new(
                &name,
                Some(session_store::WorkerActiveSegmentRef::active_segment(
                    session, segment,
                )),
            )
            .with_workspace_id(TEST_WORKSPACE_ID),
        )
        .unwrap();
}
fn message(text: &str) -> session_store::LoggedItem {
    session_store::LoggedItem::Message {
        role: session_store::LoggedRole::User,
        content: vec![session_store::LoggedContentPart::Text { text: text.into() }],
    }
}
pub(super) async fn model_supply_job_fixture(
    missing_origin: bool,
) -> (tempfile::TempDir, WorkspaceApi, BackendJobReservation) {
    let fixture = if missing_origin {
        RuntimeFixture::with_history_and_origin(
            vec![session_store::LoggedHistoryEntry {
                item: message(SOURCE_SENTINEL),
                metadata: session_store::LoggedSessionHistoryMetadata {
                    input_metadata: None,
                    entry_id: session_store::LoggedSessionHistoryEntryId(
                        "historical-source".into(),
                    ),
                    origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                    derivation: None,
                },
            }],
            false,
        )
        .await
    } else {
        RuntimeFixture::new().await
    };
    let reservation = if missing_origin {
        fixture.reserved
    } else {
        fixture.dispatch_input()
    };
    (fixture.dir, fixture.api, reservation)
}

impl RuntimeFixture {
    async fn new() -> Self {
        Self::with_history(vec![session_store::LoggedHistoryEntry {
            item: message(SOURCE_SENTINEL),
            metadata: session_store::LoggedSessionHistoryMetadata {
                input_metadata: None,
                entry_id: session_store::LoggedSessionHistoryEntryId("read-source-original".into()),
                origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
            },
        }])
        .await
    }
    async fn with_history(history: Vec<session_store::LoggedHistoryEntry>) -> Self {
        Self::with_history_and_origin(history, true).await
    }
    async fn with_history_and_origin(
        history: Vec<session_store::LoggedHistoryEntry>,
        frozen_origin: bool,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path());
        let job_store =
            Arc::new(SqliteWorkspaceStore::open_api_fixture(&config.database_path).unwrap());
        seed_test_registered_workspace(job_store.as_ref(), &config)
            .await
            .unwrap();
        let execution = Arc::new(DeterministicExecutionBackend::default());
        let api = WorkspaceApi::new_with_database_preparation(
            config,
            job_store.clone(),
            execution.clone(),
            BackendResourceBroker::default(),
            HostExecutors::default(),
            None,
            WorkspaceDatabasePreparation::PreparedFixture,
        )
        .await
        .unwrap();
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("source-reader-test").unwrap())
            .unwrap();
        let source = spawn_journal_source(&api, &subject.id, "original-source");
        let session = session_store::new_session_id();
        seed_session_history(&session_root(dir.path(), &source), session, history);
        let attr = store
            .record_session_attribution(
                crate::subjektiv::SubjectSessionAttribution::new(
                    &subject.id,
                    &source.runtime_id,
                    &source.worker_id,
                    session.to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        let host = bind_fixture_session(&store, attr.clone());
        let src = runtime_api::SessionPublicSource::Retained {
            worker_id: worker_runtime::identity::WorkerId::parse(&source.worker_id).unwrap(),
        };
        // Exercise the same authenticated Runtime adapter that the Backend uses at admission.
        api.runtime
            .session_material_pin(
                &source.runtime_id,
                runtime_api::SessionMaterialPinRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    source: src.clone(),
                    expected_session_id: session.to_string(),
                    pin_id: subjektiv::journal::source_retention_pin(
                        TEST_WORKSPACE_ID,
                        &attr.subject_id,
                        &attr.session_id,
                    )
                    .to_string(),
                },
            )
            .unwrap();
        let runtime_api::SessionMaterialCatalogAvailability::Page { page } = api
            .runtime
            .session_material_catalog(
                &source.runtime_id,
                runtime_api::SessionMaterialCatalogRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    source: src,
                    expected_session_id: session.to_string(),
                    expected_catalog_sha256: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .unwrap()
        else {
            panic!("canonical catalog required")
        };
        assert!(!page.descriptors.is_empty());
        let d = &page.descriptors[0];
        let range = JournalSourceRange {
            source: JournalSource {
                session_id: session.to_string(),
                pointer: JournalSourcePointer {
                    segment_id: Uuid::parse_str(&d.segment_id).unwrap(),
                    entry_ref: d.entry_ref.clone(),
                },
                order: d.canonical_order,
                content_sha256: d.sha256.clone(),
                total_bytes: d.total_bytes,
            },
            bytes: JournalByteRange {
                start: 0,
                end: d.total_bytes,
            },
        };
        let id = store
            .accept_journal_sources(&host, "source", &[range.clone()])
            .unwrap();
        let input = subjektiv::journal::recording::JournalInput::build(
            &store,
            &host,
            &id,
            |order, offset, budget| {
                subjektiv_journal::capture_input(&api, &attr, order, offset, budget)
                    .map_err(|e| JournalError::Conflict(e.to_string()))
            },
        )
        .unwrap();
        store
            .accept_journal_sources(&host, "bounded captured prefix", &input.selected_ranges)
            .unwrap();
        let mut request = subjektiv_journal::recording_request(&host, attr, &id, input).unwrap();
        if !frozen_origin {
            request.source_worker = None;
        }
        assert!(
            request.input["capture"]["entries"]
                .as_array()
                .is_some_and(|entries| !entries.is_empty())
        );
        let reserved = api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
            .unwrap();
        Self {
            dir,
            api,
            execution,
            job_store,
            store,
            host,
            source,
            session,
            request,
            reserved,
            range,
            id,
        }
    }
    fn stop_source(&self) {
        self.api
            .runtime
            .stop_worker(
                &self.source,
                WorkerLifecycleRequest {
                    stop_intent: None,
                    reason: Some("settle original source before permanent erase".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
    }
    fn dispatch_input(&self) -> BackendJobReservation {
        let dispatched = self
            .api
            .dispatch_backend_job_reservation(self.reserved.clone())
            .unwrap();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        let encoded = self
            .execution
            .inputs
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .clone();
        assert_eq!(
            encoded,
            self.request
                .worker_input(&dispatched.attempt.attempt_id)
                .unwrap()
        );
        assert!(encoded.contains(SOURCE_SENTINEL));
        assert!(
            subjektiv_journal::execute(
                &self.api,
                worker,
                JournalJobRequest::ReadText {
                    read: JournalTextRead {
                        target: JournalTextTarget::Source {
                            pointer: self.range.source.pointer.clone()
                        },
                        offset: 0,
                        limit: 16384,
                        content_token: None
                    }
                }
            )
            .is_err(),
            "body-bearing Jobs cannot read their original Session"
        );
        // Deterministic execution stands in for Controller persistence here; actual Runtime
        // input storage is exercised separately by its signed capture/delivery integration.
        seed_session(
            &session_root(self.dir.path(), worker),
            session_store::new_session_id(),
            message(&encoded),
            session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
        );
        dispatched
    }
    fn checkpoint(&self, worker: &RuntimeWorkerRef) {
        let g = self
            .store
            .active_journal_generation(&self.host, &self.id)
            .unwrap()
            .unwrap();
        let JournalJobResponse::Saved(receipt) = subjektiv_journal::execute(
            &self.api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: "record".into(),
                    expected_state: g.initial_journal.state_token,
                    action: JournalAction::ConfirmUnchanged,
                    covered_ranges: vec![self.range.clone()],
                },
            },
        )
        .unwrap() else {
            panic!("receipt")
        };
        let position = save_fixture_position(&self.api, worker, receipt.state_token);
        subjektiv_journal::execute(
            &self.api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: "checkpoint".into(),
                    expected_state: position,
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec!["record".into()],
                    },
                    covered_ranges: vec![self.range.clone()],
                },
            },
        )
        .unwrap();
    }
    /// Pending is NOT erasure success: observe durable Purge and retry the exact key.
    fn erase_pending_or_acknowledged(&self, key: &str) -> bool {
        match self
            .api
            .erase_subjektiv_journal_session(&self.session.to_string(), key)
        {
            Ok(()) => false,
            Err(Error::RepositoryConflict(message)) => {
                assert_eq!(
                    message,
                    "Journal delivered-copy Purge remains pending; retry the same erasure after cleanup"
                );
                true
            }
            Err(error) => panic!("unexpected erasure failure: {error}"),
        }
    }
    fn assert_erasure_acknowledged(&self, key: &str) {
        self.api
            .erase_subjektiv_journal_session(&self.session.to_string(), key)
            .unwrap();
        let saved = self
            .api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &self.request.job_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.input_disposition,
            crate::backend_job::JobInputDisposition::Erased
        );
        assert_eq!(saved.input_digest, self.request.input_digest().unwrap());
        assert!(
            !serde_json::to_string(&saved.request)
                .unwrap()
                .contains(SOURCE_SENTINEL)
        );
        assert!(saved.load_executable_request().is_err());
        self.assert_no_erased_resurrection();
    }
    fn assert_no_erased_resurrection(&self) {
        self.execution.take_inputs();
        // Internal dispatch must reread and reject retired input, including stale
        // terminal reservations. Exact public API replay is a separate contract.
        assert!(
            self.api
                .dispatch_backend_job_reservation(self.reserved.clone())
                .is_err()
        );
        assert!(
            self.execution.take_inputs().is_empty(),
            "stale saved body must not resurrect a delivered copy"
        );
    }
    fn assert_purged(&self, worker: &RuntimeWorkerRef) {
        assert!(!session_root(self.dir.path(), worker).exists());
        let removal = self
            .api
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            removal.plan.state,
            crate::retention::WorkerRemovalPlanState::Succeeded
        );
        assert_eq!(
            removal.plan.session_disposition,
            worker_runtime::retention::SessionDisposition::Purge
        );
        assert!(removal.plan.archive_id.is_none());
        assert!(
            self.api
                .config_store
                .worker_session_archive(TEST_WORKSPACE_ID, worker, &self.session.to_string())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            session_root(self.dir.path(), &self.source).exists(),
            !self.store.journal_session_erased(&self.host).unwrap(),
            "only explicit source erasure, never Job cleanup, removes the original conversation"
        );
    }
}

#[tokio::test]
async fn terminal_journal_attempts_purge_real_tool_result_sessions_without_changing_workspace_policy()
 {
    for outcome in ["completed", "cancelled", "erased"] {
        let f = RuntimeFixture::new().await;
        let before = f
            .api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let dispatched = f.dispatch_input();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        assert!(session_root(f.dir.path(), worker).exists());
        match outcome {
            "completed" => {
                f.checkpoint(worker);
                let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
                f.api
                    .accept_backend_job_result(
                        worker,
                        &BackendJobResultSubmission {
                            job_id: f.request.job_id.clone(),
                            attempt_id: dispatched.attempt.attempt_id.clone(),
                            input_digest: f.request.input_digest().unwrap(),
                            result: serde_json::to_value(completion).unwrap(),
                        },
                    )
                    .unwrap();
            }
            "cancelled" => {
                f.api
                    .finish_backend_job_attempt(
                        &f.request.job_id,
                        &dispatched.attempt.attempt_id,
                        BackendJobAttemptState::Failed,
                        "cancelled",
                        "explicit cancellation",
                    )
                    .unwrap();
            }
            "erased" => {
                f.stop_source();
                f.erase_pending_or_acknowledged("erase-original-session");
            }
            _ => unreachable!(),
        }
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        f.assert_purged(worker);
        assert_eq!(
            f.api
                .config_store
                .worker_retention_policy(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap(),
            before
        );
        assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
        if outcome == "erased" {
            f.assert_erasure_acknowledged("erase-original-session");
            assert!(f.store.journal_session_erased(&f.host).unwrap());
            f.assert_no_erased_resurrection();
        }
    }
}

/// A real SQLite writer lock blocks Job completion while the separate Journal store
/// can still save its checkpoint. Removing it does not drive recovery or resubmit input.
struct JournalCompletionSaveFault(rusqlite::Connection);

impl JournalCompletionSaveFault {
    fn install(f: &RuntimeFixture) -> Self {
        f.job_store
            .with_conn(|connection| {
                connection.busy_timeout(std::time::Duration::ZERO)?;
                Ok(())
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        Self(connection)
    }
}

impl Drop for JournalCompletionSaveFault {
    fn drop(&mut self) {
        self.0.execute_batch("ROLLBACK").unwrap();
    }
}

fn lock_journal_domain(f: &RuntimeFixture) -> JournalCompletionSaveFault {
    let database = f
        .api
        .feature_storage
        .open(&f.api.subjektiv_registration)
        .unwrap();
    let path: String = database
        .with_connection(|connection| {
            connection.busy_timeout(std::time::Duration::ZERO)?;
            Ok(connection.query_row("PRAGMA database_list", [], |row| row.get(2))?)
        })
        .unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    JournalCompletionSaveFault(connection)
}

#[tokio::test]
async fn domain_storage_lock_beyond_initial_settlement_recovers_original_journal_attempt() {
    for already_prepared in [false, true] {
        let f = RuntimeFixture::new().await;
        if already_prepared {
            subjektiv_journal::prepare(&f.api, &f.reserved).unwrap();
        }
        let binding = subjektiv::api::JobAttemptBinding {
            job_id: f.request.job_id.clone(),
            attempt_id: f.reserved.attempt.attempt_id.clone(),
        };
        let saved_generation = f
            .store
            .retained_journal_generation(&f.host, &f.id, &binding)
            .unwrap();
        assert_eq!(saved_generation.is_some(), already_prepared);
        let fault = lock_journal_domain(&f);
        // Real domain storage stays locked. The healthy Workspace DB must not be
        // used as proof that Journal preparation can proceed.
        let error = subjektiv_journal::prepare(&f.api, &f.reserved).unwrap_err();
        assert!(
            matches!(error, Error::SubjektivStorage(
            FeatureStorageError::Sqlite(rusqlite::Error::SqliteFailure(code, _)))
            if code.code == rusqlite::ErrorCode::DatabaseBusy),
            "{error}"
        );
        f.api.own_backend_job_recovery(&f.reserved.attempt);
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            f.api
                .backend_job_recovery
                .domain_storage_settlement
                .notified(),
        )
        .await
        .expect("domain operation did not retain ownership beyond six failed settlements");
        let pending = current_journal_attempt(&f, &binding.attempt_id);
        assert_eq!(pending.state, BackendJobAttemptState::Reserved);
        assert_eq!(
            pending.failure_category.as_deref(),
            Some("domain_storage_unavailable")
        );
        assert_eq!(pending.deadline_at, f.reserved.attempt.deadline_at);
        assert!(pending.worker.is_none());
        assert!(f.execution.inputs.lock().unwrap().is_empty());
        f.api
            .store
            .probe_backend_job_recovery(TEST_WORKSPACE_ID, &pending)
            .unwrap();
        assert_eq!(
            current_journal_attempt(&f, &binding.attempt_id).state,
            BackendJobAttemptState::Reserved
        );
        drop(fault);
        // Only release the independent DB lock. No second dispatch or recovery call.
        let dispatched = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let attempt = current_journal_attempt(&f, &binding.attempt_id);
                if attempt.state != BackendJobAttemptState::Reserved
                    && attempt.state != BackendJobAttemptState::Dispatching
                {
                    break attempt;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("same Journal attempt did not autonomously dispatch");
        // Observe the existing producer boundary before issuing fixture model operations.
        // The saved Dispatched row alone can precede its first input acceptance.
        {
            let gate = f
                .api
                .backend_job_recovery
                .dispatch_gate(&binding.attempt_id);
            let _dispatch_finished = gate.lock().unwrap();
        }
        assert_eq!(
            dispatched.state,
            BackendJobAttemptState::Dispatched,
            "{dispatched:?}"
        );
        assert_eq!(dispatched.attempt_id, binding.attempt_id);
        assert_eq!(dispatched.input_digest, f.reserved.attempt.input_digest);
        assert_eq!(dispatched.deadline_at, f.reserved.attempt.deadline_at);
        let worker = dispatched.worker.as_ref().unwrap();
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                worker,
                "pinned",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        f.checkpoint(worker);
        let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
        let completed = observe_journal_completion(&f).await;
        assert_eq!(completed.state, BackendJobState::Completed);
        assert_eq!(completed.current_attempt, f.reserved.attempt.attempt);
        assert_eq!(completed.input_digest, f.reserved.attempt.input_digest);
        assert_eq!(
            completed.result,
            Some(serde_json::to_value(completion).unwrap())
        );
        let terminal = current_journal_attempt(&f, &binding.attempt_id);
        assert_eq!(terminal.worker, dispatched.worker);
        assert_eq!(terminal.runtime_run_id, dispatched.runtime_run_id);
        let generation = f
            .store
            .retained_journal_generation(&f.host, &f.id, &binding)
            .unwrap()
            .unwrap();
        if let Some(saved) = saved_generation {
            assert_eq!(generation.id, saved.id);
        }
        assert_eq!(f.execution.inputs.lock().unwrap().len(), 1);
        assert_eq!(
            completed.load_executable_request().unwrap().input,
            f.request.input
        );
        assert!(
            f.api
                .store
                .get_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &binding.job_id,
                    &crate::backend_job::attempt_id(&binding.job_id, 2)
                )
                .unwrap()
                .is_none()
        );
        f.api.stop_backend_job_recovery().await;
    }
}

#[tokio::test]
async fn domain_storage_lock_stops_at_original_attempt_deadline_without_dispatch() {
    let mut f = RuntimeFixture::new().await;
    let deadline = (Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
    f.job_store.with_conn(|connection| {
        connection.execute("UPDATE backend_job_attempts SET deadline_at=?1 WHERE workspace_id=?2 AND attempt_id=?3",
            rusqlite::params![deadline, TEST_WORKSPACE_ID, f.reserved.attempt.attempt_id])?;
        Ok(())
    }).unwrap();
    f.reserved.attempt = current_journal_attempt(&f, &f.reserved.attempt.attempt_id);
    let fault = lock_journal_domain(&f);
    f.api.own_backend_job_recovery(&f.reserved.attempt);
    let failed = observe_journal_completion(&f).await;
    assert_eq!(failed.state, BackendJobState::Failed);
    assert_eq!(
        failed.failure_category.as_deref(),
        Some("recovery_cannot_continue")
    );
    let detail = failed.failure_detail.as_deref().unwrap();
    assert!(
        detail.contains("Original attempt deadline exceeded"),
        "{detail}"
    );
    assert!(detail.contains("Subjektiv storage"), "{detail}");
    let attempt = current_journal_attempt(&f, &f.reserved.attempt.attempt_id);
    assert_eq!(attempt.deadline_at, deadline);
    assert!(attempt.worker.is_none());
    assert!(f.execution.inputs.lock().unwrap().is_empty());
    drop(fault);
    assert!(
        f.store
            .active_journal_generation(&f.host, &f.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        failed.load_executable_request().unwrap().input,
        f.request.input
    );
    f.api.stop_backend_job_recovery().await;
}

fn current_journal_job(f: &RuntimeFixture) -> crate::backend_job::BackendJobRecord {
    f.api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
        .unwrap()
        .unwrap()
}

fn current_journal_attempt(f: &RuntimeFixture, attempt_id: &str) -> BackendJobAttemptRecord {
    f.api
        .store
        .get_backend_job_attempt(TEST_WORKSPACE_ID, &f.request.job_id, attempt_id)
        .unwrap()
        .unwrap()
}

async fn observe_journal_completion(f: &RuntimeFixture) -> crate::backend_job::BackendJobRecord {
    let observed = tokio::time::timeout(std::time::Duration::from_secs(40), async {
        loop {
            let job = current_journal_job(f);
            if job.state != BackendJobState::Pending {
                return job;
            }
            // Observe only; do not drive result submission or a recovery tick.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "autonomous recovery did not converge: job={:?}, attempt={:?}",
        current_journal_job(f),
        current_journal_attempt(f, &f.reserved.attempt.attempt_id)
    );
    observed.unwrap()
}

#[tokio::test]
async fn completion_save_fault_recovers_same_job_from_saved_checkpoint_without_user_retry() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    // Block cleanup independently so completion cannot hide teardown or retire
    // the immutable body before its recovery is observed.
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "pinned",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let fault = JournalCompletionSaveFault::install(&f);
    f.checkpoint(worker);
    let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
    let binding = subjektiv::api::JobAttemptBinding {
        job_id: f.request.job_id.clone(),
        attempt_id: dispatched.attempt.attempt_id.clone(),
    };
    let generation = f
        .store
        .retained_journal_generation(&f.host, &f.id, &binding)
        .unwrap()
        .unwrap();
    let checkpoint = f
        .store
        .query_journal_operation(&f.host, &f.id, "checkpoint")
        .unwrap()
        .unwrap();
    let accepted = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
    assert!(accepted.runtime_run_id.is_some());
    f.execution.take_inputs();

    let submission = BackendJobResultSubmission {
        job_id: f.request.job_id.clone(),
        attempt_id: accepted.attempt_id.clone(),
        input_digest: accepted.input_digest.clone(),
        result: serde_json::to_value(&completion).unwrap(),
    };
    let error = f
        .api
        .accept_backend_job_result(worker, &submission)
        .unwrap_err();
    assert!(
        matches!(error, Error::Sqlite(rusqlite::Error::SqliteFailure(code, _))
        if code.code == rusqlite::ErrorCode::DatabaseBusy),
        "{error}"
    );
    let pending = current_journal_job(&f);
    let pending_attempt = current_journal_attempt(&f, &accepted.attempt_id);
    assert_eq!(pending.state, BackendJobState::Pending);
    assert_eq!(pending_attempt.state, BackendJobAttemptState::Dispatched);
    assert!(pending.result.is_none());
    assert!(
        pending_attempt.result.is_none(),
        "attempt and Job commit must roll back together"
    );
    assert_eq!(
        pending.load_executable_request().unwrap().input,
        f.request.input
    );
    assert_eq!(pending.input_digest, accepted.input_digest);
    assert_eq!(pending_attempt.worker, accepted.worker);
    assert_eq!(pending_attempt.runtime_run_id, accepted.runtime_run_id);
    assert_eq!(
        f.store
            .retained_journal_generation(&f.host, &f.id, &generation.job)
            .unwrap(),
        Some(generation.clone())
    );
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap(),
        Some(checkpoint.clone())
    );
    assert!(
        f.api
            .require_backend_job_attempt_effects_ended(&pending_attempt)
            .is_err()
    );
    assert!(f.execution.take_inputs().is_empty());

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        f.api.backend_job_recovery.storage_probe_started.notified(),
    )
    .await
    .expect("Journal owner did not exhaust finite settlement before exact-attempt probe");
    drop(fault);
    // Only storage recovers. No second accept/retry/admission/recover call:
    // the production owner must resume this exact Job autonomously.
    let completed = observe_journal_completion(&f).await;
    assert_eq!(completed.state, BackendJobState::Completed);
    assert_eq!(completed.request.job_id, f.request.job_id);
    assert_eq!(completed.current_attempt, accepted.attempt);
    assert_eq!(completed.input_digest, accepted.input_digest);
    assert_eq!(
        completed.load_executable_request().unwrap().input,
        f.request.input
    );
    assert_eq!(completed.result, Some(submission.result));
    let ended = current_journal_attempt(&f, &accepted.attempt_id);
    assert_eq!(ended.state, BackendJobAttemptState::Completed);
    assert_eq!(ended.worker, accepted.worker);
    assert_eq!(ended.runtime_run_id, accepted.runtime_run_id);
    assert_eq!(ended.input_digest, accepted.input_digest);
    assert_eq!(ended.result, completed.result);
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap(),
        Some(checkpoint)
    );
    assert!(
        f.execution.take_inputs().is_empty(),
        "checkpoint recovery must not redispatch"
    );
    assert!(
        f.api
            .store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &f.request.job_id,
                &crate::backend_job::attempt_id(&f.request.job_id, accepted.attempt + 1)
            )
            .unwrap()
            .is_none()
    );
    let blocked = wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &accepted.attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    assert_eq!(blocked.state, BackendJobAttemptState::Completed);
    // Stop has independently joined the fixture-owned execution/background and
    // drained Host writers. Pinned retention rejects Purge, not that termination.
    f.api
        .require_backend_job_attempt_effects_ended(&blocked)
        .unwrap();
    assert_eq!(
        blocked.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Failed)
    );
    assert!(blocked.worker_cleanup_completed_at.is_none());
    assert_eq!(
        current_journal_job(&f).input_disposition,
        crate::backend_job::JobInputDisposition::Available
    );
    assert!(
        f.api
            .finish_backend_job_cleanup_retirement(&f.request.job_id, &accepted.attempt_id)
            .is_err()
    );
    assert!(
        session_root(f.dir.path(), worker).exists(),
        "completion does not prove Purge"
    );
    assert_eq!(
        f.store
            .retained_journal_generation(&f.host, &f.id, &generation.job)
            .unwrap()
            .unwrap()
            .job,
        generation.job
    );
}

#[tokio::test]
async fn partial_record_without_execution_receipts_preserves_input_and_rejects_continuation() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let generation = f
        .store
        .active_journal_generation(&f.host, &f.id)
        .unwrap()
        .unwrap();
    let JournalJobResponse::Saved(recorded) = subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "partial-record".into(),
                expected_state: generation.initial_journal.state_token.clone(),
                action: JournalAction::Create {
                    title: "ongoing".into(),
                    body: "Not yet checkpointed".into(),
                    ongoing: "continue".into(),
                    state: JournalEntryState::Open,
                    source_end: None,
                    correction_refs: vec![],
                },
                covered_ranges: vec![f.range.clone()],
            },
        },
    )
    .unwrap() else {
        panic!("saved partial record")
    };
    assert!(subjektiv_journal::finish(&f.api, worker).is_err());
    f.execution.take_inputs();
    let job = current_journal_job(&f);
    let attempt = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
    assert!(
        subjektiv_journal::reconcile_attempt(&f.api, &job, &attempt).is_err(),
        "missing owner receipts cannot be replaced by timeout or live-map observation"
    );
    assert_eq!(current_journal_job(&f).state, BackendJobState::Pending);
    assert_eq!(
        current_journal_job(&f)
            .load_executable_request()
            .unwrap()
            .input,
        f.request.input
    );
    assert!(
        f.api
            .store
            .reserve_backend_job_retry(
                TEST_WORKSPACE_ID,
                &f.request.job_id,
                &Utc::now().to_rfc3339()
            )
            .is_err()
    );
    assert_eq!(
        f.store.active_journal_generation(&f.host, &f.id).unwrap(),
        Some(generation)
    );
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "partial-record")
            .unwrap(),
        Some(recorded)
    );
    assert!(
        f.store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap()
            .is_none()
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(f.execution.take_inputs().is_empty());

    // Failure is neither recording coverage nor owner shutdown. Do not schedule
    // cleanup here or manufacture an effects-ended receipt for this live owner.
    let failed = f
        .api
        .store
        .finish_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "execution_failed",
            "deterministic failure; effects still require owner evidence",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    assert_eq!(failed.state, BackendJobAttemptState::Failed);
    assert!(
        f.api
            .require_backend_job_attempt_effects_ended(&failed)
            .is_err()
    );
    assert!(
        subjektiv_journal::require_job_effects_ended(&f.api, &current_journal_job(&f)).is_err()
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(subjektiv_journal::finish(&f.api, worker).is_err());
}

#[tokio::test]
async fn journal_reserved_restart_dispatches_exact_saved_intent_once() {
    let f = RuntimeFixture::new().await;
    f.execution.take_inputs();
    f.api.recover_backend_jobs().unwrap();
    let first = current_journal_attempt(&f, &f.reserved.attempt.attempt_id);
    assert_eq!(first.state, BackendJobAttemptState::Dispatched);
    assert!(first.runtime_run_id.is_some());
    assert_eq!(f.execution.take_inputs().len(), 1);
    f.api.recover_backend_jobs().unwrap();
    assert!(f.execution.take_inputs().is_empty());
    assert_eq!(current_journal_job(&f).request, f.request);
    let again = current_journal_attempt(&f, &first.attempt_id);
    assert_eq!(again.worker, first.worker);
    assert_eq!(again.runtime_run_id, first.runtime_run_id);
}

// Seed the post-migration boundary, not a new live legacy state. Actual ordered
// migration/rollback coverage belongs to backend_job_upgrade_tests; these fixtures
// test consumers of its immutable marker and preserve independent cleanup evidence.
fn fixture_journal_migration_stopped(api: &WorkspaceApi, job_id: &str) {
    let connection = rusqlite::Connection::open(&api.config.database_path).unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO backend_job_upgrade_history
        (workspace_id,job_id,state,failure_category,failure_detail,updated_at,completed_at)
        SELECT workspace_id,job_id,state,failure_category,failure_detail,updated_at,completed_at
        FROM backend_jobs WHERE workspace_id=?1 AND job_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, job_id],
    )
    .unwrap();
    tx.execute("INSERT INTO backend_job_attempt_upgrade_history
        (workspace_id,job_id,attempt_id,state,failure_category,failure_detail,updated_at,completed_at)
        SELECT workspace_id,job_id,attempt_id,state,failure_category,failure_detail,updated_at,completed_at
        FROM backend_job_attempts WHERE workspace_id=?1 AND job_id=?2", rusqlite::params![TEST_WORKSPACE_ID,job_id]).unwrap();
    for table in ["backend_job_attempts", "backend_jobs"] {
        tx.execute(&format!("UPDATE {table} SET state='failed',failure_category='version_upgrade_execution_aborted',
            failure_detail='version upgrade stopped this execution; no termination or recording proof',
            completed_at=?3,updated_at=?3 WHERE workspace_id=?1 AND job_id=?2 AND state NOT IN ('completed','failed')"),
            rusqlite::params![TEST_WORKSPACE_ID,job_id,Utc::now().to_rfc3339()]).unwrap();
    }
    tx.commit().unwrap();
}

#[tokio::test]
async fn migration_stopped_checkpoint_is_excluded_from_recovery_result_acceptance_and_retry() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    f.checkpoint(worker);
    let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
    let checkpoint = f
        .store
        .query_journal_operation(&f.host, &f.id, "checkpoint")
        .unwrap();
    let generation = f.store.active_journal_generation(&f.host, &f.id).unwrap();
    fixture_journal_migration_stopped(&f.api, &f.request.job_id);
    let stopped = current_journal_job(&f);
    let stopped_attempt = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
    assert_eq!(stopped.state, BackendJobState::Failed);
    assert_eq!(
        stopped.failure_category.as_deref(),
        Some("version_upgrade_execution_aborted")
    );
    f.execution.take_inputs();
    // Even a callback retaining the pre-cutover Pending snapshot must consult the
    // current migration marker before looking for an execution or saved completion.
    subjektiv_journal::reconcile_attempt(&f.api, &dispatched.job, &dispatched.attempt).unwrap();
    subjektiv_journal::recover_attempts(&f.api).unwrap();
    f.api.recover_backend_jobs().unwrap();
    assert!(
        f.api
            .accept_backend_job_result(
                worker,
                &BackendJobResultSubmission {
                    job_id: f.request.job_id.clone(),
                    attempt_id: stopped_attempt.attempt_id.clone(),
                    input_digest: stopped_attempt.input_digest.clone(),
                    result: serde_json::to_value(completion).unwrap(),
                }
            )
            .is_err()
    );
    assert!(
        f.api
            .store
            .reserve_backend_job_retry(
                TEST_WORKSPACE_ID,
                &f.request.job_id,
                &Utc::now().to_rfc3339()
            )
            .is_err()
    );
    let after = current_journal_job(&f);
    assert_eq!(after.state, BackendJobState::Failed);
    assert_eq!(after.current_attempt, stopped.current_attempt);
    assert_eq!(after.failure_category, stopped.failure_category);
    assert_eq!(
        after.load_executable_request().unwrap().input,
        f.request.input
    );
    assert_eq!(after.input_digest, stopped.input_digest);
    assert!(
        after.result.is_none(),
        "a migration Failed cannot be promoted from checkpoint"
    );
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap(),
        checkpoint
    );
    assert_eq!(
        f.store.active_journal_generation(&f.host, &f.id).unwrap(),
        generation
    );
    assert!(f.execution.take_inputs().is_empty());
    assert!(subjektiv_journal::require_job_effects_ended(&f.api, &after).is_err());
}

#[tokio::test]
async fn saved_checkpoint_never_promotes_erased_released_retired_or_missing_source_to_success() {
    for unavailable in ["erased", "body_released", "retired", "deleted_worker"] {
        let f = RuntimeFixture::new().await;
        // Join only this fixture's background recovery before constructing the cut.
        // This orders checkpoint/unavailability; it is not execution-ended evidence.
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            f.api.stop_backend_job_recovery(),
        )
        .await
        .expect("fixture recovery must quiesce before the unavailable-source cut");
        let dispatched = f.dispatch_input();
        let gate = f
            .api
            .backend_job_recovery
            .dispatch_gate(&dispatched.attempt.attempt_id);
        let dispatch_finished = gate.lock().unwrap();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        let generation = f
            .store
            .active_journal_generation(&f.host, &f.id)
            .unwrap()
            .unwrap();
        f.checkpoint(worker);
        let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
        let checkpoint = f
            .store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap();
        // These are pre-existing domain/Store facts at the recovery boundary, not
        // a claim that physical removal or all effects ended in this fixture.
        match unavailable {
            "erased" => {
                f.store
                    .erase_journal_session(&f.host, "erase-before-recovery")
                    .unwrap();
            }
            "body_released" => {
                f.store.release_removed_journal_body(&f.host).unwrap();
            }
            "retired" => {
                f.api
                    .store
                    .reclaim_backend_job_input(
                        TEST_WORKSPACE_ID,
                        &f.request.job_id,
                        &f.request.input_digest().unwrap(),
                        crate::backend_job::JobInputRetirementReason::LegacyInvalidation,
                    )
                    .unwrap();
            }
            "deleted_worker" => {
                f.api
                    .store
                    .delete_worker_registry(TEST_WORKSPACE_ID, &f.source)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let job = current_journal_job(&f);
        let attempt = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
        let registry = f
            .api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.source)
            .unwrap();
        // Read-only evidence remains observable after writer release; the public
        // active-generation API correctly refuses that released writer's authority.
        let generation_evidence = || {
            f.api
                .feature_storage
                .open(&f.api.subjektiv_registration)
                .unwrap()
                .with_connection(|connection| {
                    use rusqlite::OptionalExtension as _;
                    Ok(connection.query_row(
                        "SELECT json_object('snapshot',json(snapshot_json),'active',active,'completion',json(completion_json)) FROM journal_generations WHERE id=?1",
                        [&generation.id], |row| row.get::<_, String>(0),
                    ).optional()?)
                })
                .unwrap()
        };
        let retained = generation_evidence();
        if unavailable == "erased" {
            assert!(
                retained.is_none(),
                "explicit erasure removes generation evidence"
            );
        } else {
            let evidence: Value = serde_json::from_str(retained.as_ref().unwrap()).unwrap();
            assert_eq!(
                evidence["snapshot"],
                serde_json::to_value(&generation).unwrap()
            );
            assert_eq!(
                evidence["active"], 0,
                "saved domain completion closes only the generation"
            );
            assert_eq!(
                evidence["completion"],
                serde_json::to_value(&completion).unwrap()
            );
        }
        f.execution.take_inputs();
        subjektiv_journal::reconcile_attempt(&f.api, &job, &attempt).unwrap();
        drop(dispatch_finished);
        let failed = current_journal_job(&f);
        let failed_attempt = current_journal_attempt(&f, &attempt.attempt_id);
        assert_eq!(failed.state, BackendJobState::Failed, "{unavailable}");
        assert_eq!(
            failed.failure_category.as_deref(),
            Some(match unavailable {
                "erased" => "journal_session_erased",
                "body_released" => "journal_body_released",
                "retired" => "journal_input_unavailable",
                "deleted_worker" => "journal_source_worker_missing",
                _ => unreachable!(),
            })
        );
        assert!(failed.result.is_none());
        assert_eq!(failed.current_attempt, attempt.attempt);
        assert_eq!(failed.input_digest, attempt.input_digest);
        assert_eq!(failed.input_digest, f.request.input_digest().unwrap());
        assert_eq!(failed.intent_fingerprint, f.request.fingerprint().unwrap());
        if unavailable != "retired" {
            assert_eq!(failed.load_executable_request().unwrap(), &f.request);
        }
        assert!(
            f.api
                .accept_backend_job_result(
                    worker,
                    &BackendJobResultSubmission {
                        job_id: f.request.job_id.clone(),
                        attempt_id: attempt.attempt_id.clone(),
                        input_digest: attempt.input_digest.clone(),
                        result: serde_json::to_value(completion).unwrap(),
                    }
                )
                .is_err()
        );
        // The stale caller cannot execute. Available terminal input returns only
        // its saved Failed observation; explicitly retired input remains rejected.
        let replay = f.api.dispatch_backend_job_reservation(dispatched.clone());
        if unavailable == "retired" {
            assert!(matches!(replay, Err(Error::WorkspacePermissionDenied(_))));
        } else {
            let replay = replay.unwrap();
            assert!(replay.replayed, "{unavailable}");
            assert_eq!(replay.job, failed, "{unavailable}");
            assert_eq!(replay.attempt, failed_attempt, "{unavailable}");
        }
        assert_eq!(
            current_journal_job(&f),
            failed,
            "failure, input and retirement must survive replay: {unavailable}"
        );
        assert_eq!(
            current_journal_attempt(&f, &attempt.attempt_id),
            failed_attempt
        );
        assert_eq!(failed.request, job.request);
        assert_eq!(failed.intent_fingerprint, job.intent_fingerprint);
        assert_eq!(failed.input_disposition, job.input_disposition);
        assert_eq!(failed_attempt.state, BackendJobAttemptState::Failed);
        assert_eq!(failed_attempt.worker, attempt.worker);
        assert_eq!(failed_attempt.runtime_run_id, attempt.runtime_run_id);
        assert_eq!(failed_attempt.failure_category, failed.failure_category);
        assert!(failed_attempt.result.is_none());
        assert_eq!(generation_evidence(), retained);
        assert_eq!(
            f.store
                .journal_worker_was_released(
                    &f.request
                        .grants
                        .subjektiv_journal
                        .as_ref()
                        .unwrap()
                        .subject_id,
                    &f.source.runtime_id,
                    &f.source.worker_id,
                )
                .unwrap(),
            unavailable == "body_released"
        );
        assert_eq!(
            f.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &f.source)
                .unwrap(),
            registry
        );
        assert!(
            f.execution.take_inputs().is_empty(),
            "{unavailable} must not redispatch or delegate saved input"
        );
        if unavailable != "erased" {
            assert_eq!(
                f.store
                    .query_journal_operation(&f.host, &f.id, "checkpoint")
                    .unwrap(),
                checkpoint
            );
        } else {
            assert!(f.store.journal_session_erased(&f.host).unwrap());
        }
        if unavailable == "retired" {
            assert!(failed.load_executable_request().is_err());
            assert!(failed.request.input.is_null());
            assert_eq!(
                failed.input_disposition,
                crate::backend_job::JobInputDisposition::Invalidated
            );
        }
        if unavailable == "deleted_worker" {
            assert!(
                f.api
                    .store
                    .get_worker_registry(TEST_WORKSPACE_ID, &f.source)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[tokio::test]
async fn checkpoint_recovery_rejects_stale_snapshot_and_mismatched_durable_run() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    f.checkpoint(worker);
    let job = current_journal_job(&f);
    let attempt = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
    let mut stale = attempt.clone();
    stale.runtime_run_id = Some("old-owner-run".into());
    assert!(subjektiv_journal::reconcile_attempt(&f.api, &job, &stale).is_err());
    // Corrupt the external durable binding, then pass the newly read snapshot.
    // Comparing only the caller snapshot with the DB would incorrectly settle it.
    let connection = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    connection
        .execute(
            "UPDATE backend_job_attempts SET runtime_run_id='mismatched-run'
        WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",
            rusqlite::params![TEST_WORKSPACE_ID, f.request.job_id, attempt.attempt_id],
        )
        .unwrap();
    let mismatched = current_journal_attempt(&f, &attempt.attempt_id);
    assert!(
        subjektiv_journal::reconcile_attempt(&f.api, &current_journal_job(&f), &mismatched)
            .is_err()
    );
    assert_eq!(current_journal_job(&f).state, BackendJobState::Pending);
    assert!(current_journal_job(&f).result.is_none());
    assert!(
        f.store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        current_journal_job(&f)
            .load_executable_request()
            .unwrap()
            .input,
        f.request.input
    );
}

#[tokio::test]
async fn stopped_source_pin_releases_after_saved_completion_and_removal() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let root = f.dir.path().join(".test-embedded-runtime-store");
    let retention = worker_runtime::retention::runtime_session_retention(&root);
    assert_eq!(retention.pins(f.session).unwrap().len(), 1);
    f.api
        .runtime
        .stop_worker(
            &f.source,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("archive original".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    assert!(session_root(f.dir.path(), &f.source).exists());
    assert!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.session.to_string())
            .unwrap()
            .is_none()
    );
    // Stopping the body is not release. Keep its source while the active
    // writer/Job still depends on current ownership and saved completion.
    // The Job cannot fall back to a retained or archived Session read.
    let saved = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.load_executable_request().unwrap().input,
        f.request.input
    );
    assert!(
        serde_json::to_string(&saved.request.input)
            .unwrap()
            .contains(SOURCE_SENTINEL)
    );
    let reread = subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::ReadText {
            read: JournalTextRead {
                target: JournalTextTarget::Source {
                    pointer: f.range.source.pointer.clone(),
                },
                offset: 0,
                limit: 16384,
                content_token: None,
            },
        },
    )
    .unwrap_err();
    assert!(matches!(reread, Error::WorkspacePermissionDenied(message)
        if message.contains("raw source bodies are in immutable Job input")));
    assert_eq!(
        retention.pins(f.session).unwrap().len(),
        1,
        "delivery alone never releases original pin"
    );
    f.checkpoint(worker);
    let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
    f.api
        .accept_backend_job_result(
            worker,
            &BackendJobResultSubmission {
                job_id: f.request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id.clone(),
                input_digest: f.request.input_digest().unwrap(),
                result: serde_json::to_value(completion).unwrap(),
            },
        )
        .unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(
        !retention.pins(f.session).unwrap().is_empty(),
        "saved reflection alone does not release the still-owned source"
    );
    let response = WorkerRemovalService::new(&f.api)
        .execute_target_removal_on_host(f.api.runtime.as_ref(), &f.source, "archive original", None)
        .await
        .unwrap();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(
        retention.pins(f.session).unwrap().is_empty(),
        "completed reflection and confirmed removal allow archive and source pin release"
    );
    assert!(!session_root(f.dir.path(), worker).exists());
    assert!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.session.to_string())
            .unwrap()
            .is_some(),
        "the removed source is archived after saved reflection"
    );
}

#[tokio::test]
async fn journal_profile_name_without_host_job_binding_keeps_normal_archive_policy() {
    let f = RuntimeFixture::new().await;
    let mut registry = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    registry.profile = Some("builtin:subjektiv-journal-recording".into());
    f.api.store.upsert_worker_registry(&registry).unwrap();
    let inventory = f.api.runtime.worker_retention_inventory(&f.source).unwrap();
    let plan = f
        .api
        .store
        .plan_worker_removal(
            &crate::retention::WorkerRemovalPlanRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: f.source,
                reason: "profile is not purge authority".into(),
                journal_erasure: None,
            },
            &inventory,
        )
        .unwrap();
    assert_eq!(
        plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Archive
    );
}

#[tokio::test]
async fn saved_checkpoint_recovery_completes_without_proving_failed_cleanup_ended() {
    for recovery in ["journal", "backend"] {
        let f = RuntimeFixture::new().await;

        let dispatched = f.dispatch_input();
        let worker = dispatched.attempt.worker.as_ref().unwrap();

        // Inject a recoverable cleanup commit failure before checkpoint publication.
        // Pin/unpin changes Worker authority and deliberately makes the original child stale;
        // that negative contract is covered by the canonical-cleanup staleness tests.
        f.api
            .config_store
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER fail_checkpoint_cleanup_commit
                 BEFORE INSERT ON worker_registry_projection_removals
                 BEGIN SELECT RAISE(ABORT, 'injected checkpoint cleanup failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        f.checkpoint(worker); // Deliberately do not call finish: checkpoint receipt is durable.
        f.execution.take_inputs();

        if recovery == "journal" {
            subjektiv_journal::recover_attempts(&f.api).unwrap();
        } else {
            // Generic startup recovery must reach the same domain receipt owner.
            f.api.recover_backend_jobs().unwrap();
        }

        let blocked = wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Failed,
        )
        .await;

        assert_eq!(blocked.state, BackendJobAttemptState::Completed);
        let saved = f
            .api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
            .unwrap()
            .unwrap();
        assert_eq!(saved.state, BackendJobState::Completed);
        assert_eq!(saved.current_attempt, dispatched.attempt.attempt);
        assert_eq!(saved.result.as_ref().unwrap()["process_order"], 1);
        assert!(
            !serde_json::to_string(&saved.result)
                .unwrap()
                .contains(SOURCE_SENTINEL)
        );
        assert_eq!(
            saved.input_disposition,
            crate::backend_job::JobInputDisposition::Available,
            "Job completion cannot retire input before canonical cleanup succeeds"
        );
        let original_removal = f
            .api
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            original_removal.prior_failure_category.as_deref(),
            Some("metadata_commit_failed")
        );
        assert!(
            f.execution.take_inputs().is_empty(),
            "saved checkpoint must not cause model rerun"
        );
        f.api
            .config_store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_checkpoint_cleanup_commit;")?;
                Ok(())
            })
            .unwrap();

        f.api.recover_backend_jobs().unwrap();

        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        f.assert_purged(worker);
        let completed_removal = f
            .api
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .unwrap();
        assert_eq!(
            completed_removal.plan.operation_id,
            original_removal.plan.operation_id
        );
        assert_eq!(
            completed_removal.plan.input_fingerprint,
            original_removal.plan.input_fingerprint
        );
        assert_eq!(
            f.api
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
                .unwrap()
                .unwrap()
                .result,
            saved.result
        );
    }
}

#[tokio::test]
async fn stopped_partial_journal_recovery_fences_old_attempt_and_resumes_same_immutable_input() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let g = f
        .store
        .active_journal_generation(&f.host, &f.id)
        .unwrap()
        .unwrap();
    let JournalJobResponse::Saved(receipt) = subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "saved-before-stop".into(),
                expected_state: g.initial_journal.state_token,
                action: JournalAction::Create {
                    title: "unfinished".into(),
                    body: "Saved before stopped execution".into(),
                    ongoing: "resume remaining range".into(),
                    state: JournalEntryState::Open,
                    source_end: None,
                    correction_refs: vec![],
                },
                covered_ranges: vec![f.range.clone()],
            },
        },
    )
    .unwrap() else {
        panic!("saved receipt")
    };
    f.api
        .runtime
        .stop_worker(
            worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: Some("definitive stop before checkpoint".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
    f.execution.take_inputs();
    f.api.recover_backend_jobs().unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if !f.execution.inputs.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let old = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(old.state, BackendJobState::Pending);
    assert_eq!(old.request.job_id, f.request.job_id);
    assert_eq!(old.input_digest, f.request.input_digest().unwrap());
    assert_eq!(
        old.load_executable_request().unwrap().input,
        f.request.input
    );
    assert_eq!(old.current_attempt, dispatched.attempt.attempt + 1);
    f.assert_purged(worker);
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "saved-before-stop")
            .unwrap(),
        Some(receipt)
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert_eq!(
        f.store
            .list_journal_entries(&f.host, &f.id, None, 16)
            .unwrap()
            .items
            .len(),
        1
    );
    let next = f
        .store
        .active_journal_generation(&f.host, &f.id)
        .unwrap()
        .unwrap();
    assert_eq!(next.job.job_id, g.job.job_id);
    assert_ne!(next.job.attempt_id, g.job.attempt_id);
    assert_eq!(
        next.initial_open, g.initial_open,
        "original initial snapshot stays bound to same input"
    );
    assert_eq!(next.selected_ranges, g.selected_ranges);
    let new_inputs = f.execution.take_inputs();
    assert_eq!(new_inputs.len(), 1);
    assert!(new_inputs[0].1.contains(SOURCE_SENTINEL));
    assert_eq!(
        new_inputs[0].1,
        f.request.worker_input(&next.job.attempt_id).unwrap()
    );
    assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
    f.api.recover_backend_jobs().unwrap();
    assert!(
        f.execution.take_inputs().is_empty(),
        "recovery replay must not launch another recording attempt"
    );
}

#[tokio::test]
async fn failed_cancelled_and_erased_job_sessions_retry_purge_after_cleanup_failure() {
    for outcome in ["failed", "cancelled", "erased"] {
        let f = RuntimeFixture::new().await;
        let dispatched = f.dispatch_input();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                worker,
                "pinned",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        if outcome == "erased" {
            f.stop_source();
            assert!(
                f.erase_pending_or_acknowledged("erase-with-blocked-cleanup"),
                "pinned delivered copy must prevent erasure acknowledgement"
            );
        } else {
            f.api
                .finish_backend_job_attempt(
                    &f.request.job_id,
                    &dispatched.attempt.attempt_id,
                    BackendJobAttemptState::Failed,
                    outcome,
                    "terminal outcome before cleanup",
                )
                .unwrap();
        }
        let blocked = wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Failed,
        )
        .await;
        assert_eq!(blocked.state, BackendJobAttemptState::Failed);
        assert!(session_root(f.dir.path(), worker).exists());
        assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
        if outcome != "erased" {
            assert!(f.store.journal_retention_required(&f.host).unwrap());
            assert_eq!(
                worker_runtime::retention::runtime_session_retention(
                    &f.dir.path().join(".test-embedded-runtime-store")
                )
                .pins(f.session)
                .unwrap()
                .len(),
                1
            );
        }
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                worker,
                "normal",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        f.api.recover_backend_jobs().unwrap();
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        f.assert_purged(worker);
        if outcome == "erased" {
            f.assert_erasure_acknowledged("erase-with-blocked-cleanup");
            assert!(f.store.journal_session_erased(&f.host).unwrap());
            f.assert_no_erased_resurrection();
        }
    }
}

#[tokio::test]
async fn explicit_erasure_interrupts_completed_journal_cleanup_waiting_for_run_teardown() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    f.checkpoint(worker);
    let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
    f.execution.publish_worker_state(
        worker,
        protocol::WorkerStateSnapshot {
            last_command_id: 0,
            last_finished_submission_request_id: None,
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
        },
    );
    let accepted = f
        .api
        .accept_backend_job_result(
            worker,
            &BackendJobResultSubmission {
                job_id: f.request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id.clone(),
                input_digest: f.request.input_digest().unwrap(),
                result: serde_json::to_value(completion).unwrap(),
            },
        )
        .unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Executing,
    )
    .await;
    assert!(
        session_root(f.dir.path(), worker).exists(),
        "normal cleanup still waits for tracked response teardown"
    );
    f.stop_source();
    f.erase_pending_or_acknowledged("erase-while-completed-run-busy");
    // No Idle/finish marker is published by this fixture. The erasure fence must
    // cause a real Runtime stop/Purge instead of leaving plaintext indefinitely.
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    f.assert_purged(worker);
    f.assert_erasure_acknowledged("erase-while-completed-run-busy");
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
            .unwrap()
            .unwrap()
            .result,
        accepted.job.result
    );
    assert!(f.store.journal_session_erased(&f.host).unwrap());
    assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
}

/// Removal uses the production embedded WorkerRemove forwarder, target-bound proof,
/// common reception, catalog/retention planner, Runtime and SQLite. Source creation
/// calls the create handler directly (no HTTP auth); canonical Session contents,
/// attribution and profile disablement are fixtures, not model execution or proofs
/// of the normal publication/attribution connection.
struct SubjectRemovalFixture {
    dir: tempfile::TempDir,
    api: WorkspaceApi,
    execution: Arc<DeterministicExecutionBackend>,
    dispatcher: Arc<crate::worker_source::EmbeddedServerWorkerMutationDispatcher>,
    controller: RuntimeWorkerRef,
    target: RuntimeWorkerRef,
    store: crate::subjektiv::SubjektivStore,
    attr: crate::subjektiv::SubjectSessionAttribution,
    host: JournalHostContext,
}
impl SubjectRemovalFixture {
    async fn new() -> Self {
        Self::with_materialized_source(true).await
    }
    async fn with_materialized_source(materialized: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path());
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let execution = Arc::new(DeterministicExecutionBackend::default());
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            execution.clone(),
            BackendResourceBroker::default(),
            HostExecutors::default(),
            Some(dispatcher.clone()),
        )
        .await
        .unwrap();
        let Json(orchestrator) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
        )
        .await
        .unwrap();
        let controller = orchestrator.worker.unwrap();
        let controller = RuntimeWorkerRef::new(controller.runtime_id, controller.worker_id);
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("removal-source").unwrap())
            .unwrap();
        let Json(created) = scoped_create_workspace_worker(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                display_name: "removal-source".into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: subject.id.clone(),
                    }),
                },
                control_operation_id: None,
                execution_grant: None,
            }),
        )
        .await
        .unwrap();
        let target = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
        let session = store
            .subject_session_attributions(&subject.id)
            .unwrap()
            .first()
            .map(|attr| attr.session_id.parse().unwrap())
            .unwrap_or_else(session_store::new_session_id);
        if materialized {
            seed_session_history(
                &session_root(dir.path(), &target),
                session,
                ["admitted-prefix", "disabled-new-history"]
                    .into_iter()
                    .map(|text| session_store::LoggedHistoryEntry {
                        item: message(text),
                        metadata: session_store::LoggedSessionHistoryMetadata {
                            input_metadata: None,
                            entry_id: session_store::LoggedSessionHistoryEntryId(text.into()),
                            origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                            derivation: None,
                        },
                    })
                    .collect(),
            );
        } else {
            use session_store::WorkerMetadataStore as _;
            let root = session_root(dir.path(), &target);
            let aggregate = root.parent().unwrap();
            let name = format!("worker-runtime-{}", target.worker_id);
            session_store::WorkerAggregateStore::new(aggregate, &name)
                .unwrap()
                .write(
                    &session_store::WorkerMetadata::new(
                        &name,
                        Some(session_store::WorkerActiveSegmentRef::pending_segment(
                            session,
                        )),
                    )
                    .with_workspace_id(TEST_WORKSPACE_ID),
                )
                .unwrap();
        }
        let attr = store
            .record_session_attribution(
                crate::subjektiv::SubjectSessionAttribution::new(
                    &subject.id,
                    &target.runtime_id,
                    &target.worker_id,
                    session.to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        let host = bind_fixture_session(&store, attr.clone());
        seed_worker_control_grant(&api, &controller, &target, "subject-removal-fixture");
        api.runtime
            .stop_worker(
                &target,
                WorkerLifecycleRequest {
                    stop_intent: None,
                    reason: Some("test stopped Subject removal".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let summary = api.runtime.worker(&target).unwrap();
        assert_eq!(summary.state, "stopped");
        sync_worker_observation(&api, &summary).unwrap();
        Self {
            dir,
            api,
            execution,
            dispatcher,
            controller,
            target,
            store,
            attr,
            host,
        }
    }
    fn disable(&self) {
        let mut registry = self
            .api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &self.target)
            .unwrap()
            .unwrap();
        registry.profile = Some("builtin:default".into());
        self.api.store.upsert_worker_registry(&registry).unwrap();
    }
    /// Seed the historical release boundary for retention-recovery component tests.
    /// This deterministic stopped-Worker fixture has no Controller and does NOT
    /// prove disconnect. The connected test below exercises actual release proof.
    fn seed_completed_release_for_archive_recovery(&self) {
        let lease = self
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &self.target)
            .unwrap()
            .unwrap();
        let binding = JournalWriterBinding {
            subject_id: self.attr.subject_id.clone(),
            runtime_id: self.target.runtime_id.clone(),
            worker_id: self.target.worker_id.clone(),
            binding_id: self.host.writer_binding_id().unwrap().into(),
        };
        let operation = "historical-release-retention-fixture";
        self.api
            .config_store
            .reserve_subject_release(TEST_WORKSPACE_ID, &lease, &binding.binding_id, operation)
            .unwrap();
        self.store.release_journal_writer(&binding).unwrap();
        self.api
            .config_store
            .confirm_subject_release_proof(
                TEST_WORKSPACE_ID,
                &lease,
                &binding.binding_id,
                operation,
                false,
            )
            .unwrap();
        self.api
            .config_store
            .release_subject_singleton(TEST_WORKSPACE_ID, &lease, &binding.binding_id, operation)
            .unwrap();
    }
    fn pending_journal(&self) -> BackendJobReservation {
        self.pending_journal_input(false)
    }

    fn pending_journal_input(&self, legacy: bool) -> BackendJobReservation {
        let capture =
            subjektiv_journal::capture_input(&self.api, &self.attr, 0, 0, 48 * 1024).unwrap();
        let ranges = subjektiv::journal::recording::capture_ranges(&capture).unwrap();
        let id = self
            .store
            .accept_journal_sources(&self.host, "unfinished history", &ranges)
            .unwrap();
        let input = subjektiv::journal::recording::JournalInput::build(
            &self.store,
            &self.host,
            &id,
            |order, offset, budget| {
                subjektiv_journal::capture_input(&self.api, &self.attr, order, offset, budget)
                    .map_err(|e| JournalError::Conflict(e.to_string()))
            },
        )
        .unwrap();
        let mut request =
            subjektiv_journal::recording_request(&self.host, self.attr.clone(), &id, input)
                .unwrap();
        if legacy {
            let grant = request.grants.subjektiv_journal.as_mut().unwrap();
            grant.binding_id.clear();
            request.input =
                serde_json::json!({"journal_id":id, "selected_ranges":grant.selected_ranges});
        }
        let reserved = self
            .api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
            .unwrap();
        if legacy {
            self.store
                .prepare_journal_generation(
                    &self.host,
                    &id,
                    subjektiv::api::JobAttemptBinding {
                        job_id: request.job_id.clone(),
                        attempt_id: reserved.attempt.attempt_id.clone(),
                    },
                    request
                        .grants
                        .subjektiv_journal
                        .as_ref()
                        .unwrap()
                        .selected_ranges
                        .clone(),
                )
                .unwrap();
        } else {
            subjektiv_journal::prepare(&self.api, &reserved).unwrap();
        }
        reserved
    }

    fn assert_removed_with_history(&self) -> String {
        assert!(
            self.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &self.target)
                .unwrap()
                .is_none()
        );
        let key = crate::subjektiv::subject_worker_singleton_key(&self.attr.subject_id).unwrap();
        assert!(
            self.api
                .store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                .unwrap()
                .is_none()
        );
        assert!(
            self.store
                .journal_worker_was_released(
                    &self.attr.subject_id,
                    &self.target.runtime_id,
                    &self.target.worker_id
                )
                .unwrap()
        );
        assert!(!self.store.journal_session_erased(&self.host).unwrap());
        assert!(!session_root(self.dir.path(), &self.target).exists());
        self.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &self.target, &self.attr.session_id)
            .unwrap()
            .expect("ordinary deletion retains original history")
            .archive_id
    }

    fn assert_occupied(&self) {
        let key = crate::subjektiv::subject_worker_singleton_key(&self.attr.subject_id).unwrap();
        assert_eq!(
            self.api
                .store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                .unwrap()
                .unwrap()
                .worker,
            self.target
        );
    }

    fn remove(&self) -> worker::WorkspaceResponse {
        let key = self
            .api
            .store
            .worker_resource_key(TEST_WORKSPACE_ID, &self.target)
            .unwrap()
            .unwrap();
        worker_runtime::worker_source::RuntimeWorkerMutationForwarder::embedded(
            &self.controller.runtime_id,
            worker_runtime::RuntimeWorkspaceScope::new(TEST_WORKSPACE_ID, "embedded-backend"),
            &self.controller.worker_id,
            self.dispatcher.clone(),
        )
        .execute_worker_remove(&self.target.runtime_id, &key, "remove stopped Subject")
        .unwrap()
    }
}

#[tokio::test]
async fn subject_release_never_substitutes_catalog_idle_or_stopped_for_controller_reservation() {
    let f = SubjectRemovalFixture::new().await;
    let input = serde_json::from_value(serde_json::json!({
        "worker":f.target,"binding_id":f.host.writer_binding_id().unwrap(),"operation_id":"actual-controller-required",
    })).unwrap();
    let error = subjektiv_journal::release_subject(&f.api, input).unwrap_err();
    assert!(
        error.to_string().contains("actual") || error.to_string().contains("Controller"),
        "{error}"
    );
    let key = crate::subjektiv::subject_worker_singleton_key(&f.attr.subject_id).unwrap();
    assert_eq!(
        f.api
            .store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .unwrap()
            .worker,
        f.target
    );
    let binding = JournalWriterBinding {
        subject_id: f.attr.subject_id.clone(),
        runtime_id: f.target.runtime_id.clone(),
        worker_id: f.target.worker_id.clone(),
        binding_id: f.host.writer_binding_id().unwrap().into(),
    };
    assert!(f.store.journal_writer_active(&binding).unwrap());
    assert!(f.store.journal_for_session(&f.host).unwrap().is_none());
}

#[tokio::test]
async fn stopped_subject_removal_releases_occupancy_and_preserves_unfinished_history() {
    let f = SubjectRemovalFixture::new().await;
    let reserved = f.pending_journal();
    let grant = reserved
        .job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .unwrap();
    let before = f.store.read_journal(&f.host, &grant.journal_id).unwrap();
    // Even a Workspace Purge policy is not implicit authority to erase Subject history.
    let policy = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let purge = f
        .api
        .config_store
        .update_worker_retention_policy(
            TEST_WORKSPACE_ID,
            &policy.policy_digest,
            &crate::retention::WorkerRetentionPolicyUpdate {
                policy_id: "test-purge".into(),
                session_disposition: worker_runtime::retention::SessionDisposition::Purge,
                metadata_disposition: policy.metadata_disposition,
                archive_retention: policy.archive_retention,
                diagnostics_disposition: policy.diagnostics_disposition,
                diagnostics_retention_seconds: policy.diagnostics_retention_seconds,
            },
        )
        .unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    let archive = f.assert_removed_with_history();
    let runtime_api::SessionMaterialCatalogAvailability::Page { page } = f
        .api
        .runtime
        .session_material_catalog(
            &f.target.runtime_id,
            runtime_api::SessionMaterialCatalogRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                source: runtime_api::SessionPublicSource::Archived {
                    archive_id: archive,
                },
                expected_session_id: f.attr.session_id.clone(),
                expected_catalog_sha256: None,
                offset: 0,
                limit: 100,
            },
        )
        .unwrap()
    else {
        panic!("original history must remain readable")
    };
    assert_eq!(page.descriptors.len(), 2);
    let after = f.store.read_journal(&f.host, &grant.journal_id).unwrap();
    assert_eq!(before.process_order, after.process_order);
    assert_eq!(before.process_byte, after.process_byte);
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(
        f.store
            .active_journal_generation(&f.host, &grant.journal_id)
            .is_err()
    );
    assert!(subjektiv_journal::prepare(&f.api, &reserved).is_err());
    let job = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, BackendJobState::Failed);
    assert_eq!(
        job.load_executable_request().unwrap().input,
        reserved.job.request.input
    );
    assert_eq!(
        f.api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap(),
        purge
    );

    let Json(next) = scoped_create_workspace_worker(
        State(f.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: f.target.runtime_id.clone(),
            display_name: "replacement".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: f.attr.subject_id.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    assert_ne!(next.worker_id, f.target.worker_id);
    let key = crate::subjektiv::subject_worker_singleton_key(&f.attr.subject_id).unwrap();
    let successor = f
        .api
        .store
        .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
        .unwrap()
        .unwrap();
    assert_eq!(successor.worker.worker_id, next.worker_id);
    let replay = WorkerRemovalService::new(&f.api)
        .execute_target_removal_on_host(
            f.api.runtime.as_ref(),
            &f.target,
            "repeat old deletion",
            None,
        )
        .await
        .unwrap();
    assert_eq!(replay.status, StatusCode::OK.as_u16(), "{}", replay.body);
    assert_eq!(
        f.api
            .store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .unwrap(),
        successor
    );
    assert!(subjektiv_journal::bind_host_session(&f.api, f.attr.clone()).is_err());
    subjektiv_journal::drain_after_completion(&f.api, &reserved.job.request);
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
            .unwrap()
            .unwrap()
            .current_attempt,
        1
    );
}

#[tokio::test]
async fn subject_removal_archive_failure_does_not_release_writer_or_occupancy() {
    let f = SubjectRemovalFixture::new().await;
    let archive_parent = f
        .dir
        .path()
        .join(".test-embedded-runtime-store/archives/workers");
    std::fs::create_dir_all(archive_parent.parent().unwrap()).unwrap();
    std::fs::write(&archive_parent, "injected archive failure").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    f.assert_occupied();
    assert!(
        !f.store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.target.runtime_id,
                &f.target.worker_id
            )
            .unwrap()
    );
    assert!(session_root(f.dir.path(), &f.target).exists());
    let old = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    std::fs::remove_file(archive_parent).unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_removed_with_history();
    assert_eq!(
        f.api
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .unwrap()
            .plan
            .operation_id,
        old.plan.operation_id
    );
}

#[tokio::test]
async fn subject_removal_reopens_after_writer_revocation_before_catalog_commit() {
    let f = SubjectRemovalFixture::new().await;
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_subject_delete BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END;").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    assert!(!session_root(f.dir.path(), &f.target).exists());
    assert!(
        f.store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.target.runtime_id,
                &f.target.worker_id
            )
            .unwrap()
    );
    f.assert_occupied(); // Runtime deletion alone is not a successful Backend deletion.
    conn.execute_batch("DROP TRIGGER interrupt_subject_delete;")
        .unwrap();
    drop(conn);
    let f = f.reopen().await;
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_removed_with_history();
}

#[tokio::test]
async fn subject_removal_rejects_mismatched_runtime_receipts_before_revocation() {
    let f = SubjectRemovalFixture::new().await;
    let inventory = f.api.runtime.worker_retention_inventory(&f.target).unwrap();
    let plan = f
        .api
        .config_store
        .plan_worker_removal(
            &crate::retention::WorkerRemovalPlanRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: f.target.clone(),
                reason: "receipt recovery".into(),
                journal_erasure: None,
            },
            &inventory,
        )
        .unwrap();
    let prepared = f
        .api
        .config_store
        .prepare_worker_removal_execution(TEST_WORKSPACE_ID, &plan.plan_id, &plan.input_fingerprint)
        .unwrap();
    let service = WorkerRemovalService::new(&f.api);
    subjektiv_journal::validate_removal_retention(&service, &f.target, &prepared).unwrap();
    let result = f
        .api
        .runtime
        .execute_worker_retention(&f.target, prepared.runtime_request.clone())
        .unwrap();
    for variant in 0..5 {
        let mut wrong = result.clone();
        match variant {
            0 => wrong.source_removed = false,
            1 => wrong.operation_id.push_str("-wrong"),
            2 => wrong.input_fingerprint.push_str("-wrong"),
            3 => wrong
                .archive
                .as_mut()
                .unwrap()
                .source_session_id
                .push_str("-wrong"),
            _ => wrong.archive = None,
        }
        assert!(
            subjektiv_journal::finish_worker_removal(&service, &f.target, &prepared, &wrong, || ())
                .is_err()
        );
        f.assert_occupied();
        assert!(
            !f.store
                .journal_worker_was_released(
                    &f.attr.subject_id,
                    &f.target.runtime_id,
                    &f.target.worker_id
                )
                .unwrap()
        );
    }
    // Simulate a lost Runtime reply: ordinary retry must recover its exact receipt.
    let f = f.reopen().await;
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_removed_with_history();
}

#[tokio::test]
async fn subject_removal_failed_job_keeps_unrecorded_input_after_cleanup_without_restarting_writer()
{
    let f = SubjectRemovalFixture::new().await;
    let reserved = f.pending_journal();
    let dispatched = f
        .api
        .dispatch_backend_job_reservation(reserved.clone())
        .unwrap();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    seed_session(
        &session_root(f.dir.path(), worker),
        session_store::new_session_id(),
        message("delivered immutable Journal input"),
        session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
    );
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "pinned",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    f.api
        .finish_backend_job_attempt(
            &reserved.job.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "execution_timeout",
            "fixture execution deadline exceeded; shutdown is checked separately",
        )
        .unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &reserved.job.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    let response = f.remove();
    assert_ne!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_occupied();
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Failed
    );
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "normal",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &reserved.job.request.job_id,
            &dispatched.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    f.api.schedule_backend_job_worker_cleanup(&attempt);
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &reserved.job.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    // Cleanup's own reconciliation owns the CAS; do not race it with a stale snapshot.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job = f
                .api
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
                .unwrap()
                .unwrap();
            if job.state == BackendJobState::Failed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Exercise restart recovery against the persisted state while the asynchronous
    // cleanup task is still allowed to hold this fixture's Runtime owner lock.
    subjektiv_journal::recover(&f.api).unwrap();
    let job = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, BackendJobState::Failed);
    assert_eq!(job.current_attempt, 1);
    assert_eq!(
        job.load_executable_request().unwrap().input,
        reserved.job.request.input
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    f.assert_occupied();
}

#[tokio::test]
async fn subject_removal_erasure_between_runtime_receipt_and_commit_requires_reconciliation() {
    let f = SubjectRemovalFixture::new().await;
    let inventory = f.api.runtime.worker_retention_inventory(&f.target).unwrap();
    let plan = f
        .api
        .config_store
        .plan_worker_removal(
            &crate::retention::WorkerRemovalPlanRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: f.target.clone(),
                reason: "erase race".into(),
                journal_erasure: None,
            },
            &inventory,
        )
        .unwrap();
    let prepared = f
        .api
        .config_store
        .prepare_worker_removal_execution(TEST_WORKSPACE_ID, &plan.plan_id, &plan.input_fingerprint)
        .unwrap();
    let service = WorkerRemovalService::new(&f.api);
    subjektiv_journal::validate_removal_retention(&service, &f.target, &prepared).unwrap();
    let result = f
        .api
        .runtime
        .execute_worker_retention(&f.target, prepared.runtime_request.clone())
        .unwrap();
    // The domain fence commits, but the original moved to the not-yet-published archive.
    assert!(
        f.api
            .erase_subjektiv_journal_session(&f.attr.session_id, "erase-during-removal")
            .is_err()
    );
    assert!(f.store.journal_session_erased(&f.host).unwrap());
    let mut committed = false;
    assert!(
        subjektiv_journal::finish_worker_removal(&service, &f.target, &prepared, &result, || {
            committed = true;
        })
        .is_err()
    );
    assert!(!committed);
    f.assert_occupied();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_erased_removal();
    assert!(
        f.store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.target.runtime_id,
                &f.target.worker_id
            )
            .unwrap()
    );
}

#[tokio::test]
async fn subject_removal_prior_archive_policy_receipt_remains_recoverable() {
    let f = SubjectRemovalFixture::new().await;
    f.seed_completed_release_for_archive_recovery();
    let configured = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_legacy_removal BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END;").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    let prepared = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(prepared.plan.policy_id, configured.policy_id);
    assert_eq!(prepared.plan.policy_digest, configured.policy_digest);
    assert_eq!(
        prepared.plan.state,
        crate::retention::WorkerRemovalPlanState::Failed
    );
    assert!(!session_root(f.dir.path(), &f.target).exists());
    conn.execute_batch("DROP TRIGGER interrupt_legacy_removal;")
        .unwrap();
    drop(conn);
    let f = f.reopen().await;
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.assert_removed_with_history();
    let recovered = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(prepared.plan.operation_id, recovered.plan.operation_id);
    assert_eq!(
        prepared.plan.input_fingerprint,
        recovered.plan.input_fingerprint
    );
    assert_eq!(prepared.plan.policy_digest, recovered.plan.policy_digest);
}

async fn assert_unbound_pending_snapshot_keeps_source(legacy: bool, completed: bool) {
    let f = SubjectRemovalFixture::new().await;
    let reserved = f.pending_journal_input(legacy);
    let grant = reserved
        .job
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .unwrap();
    if completed {
        // Fixture a committed domain checkpoint whose final Job response was lost.
        // No Controller/model delivery is claimed by these direct domain writes.
        let g = f
            .store
            .active_journal_generation(&f.host, &grant.journal_id)
            .unwrap()
            .unwrap();
        let ctx = JournalJobContext::validated(&f.store, &f.host, &g.id, g.job).unwrap();
        let record = f
            .store
            .execute_journal(
                &ctx,
                JournalOperation {
                    request_id: "saved-record".into(),
                    expected_state: g.initial_journal.state_token,
                    action: JournalAction::ConfirmUnchanged,
                    covered_ranges: grant.selected_ranges.clone(),
                },
            )
            .unwrap();
        let position = f
            .store
            .execute_journal(
                &ctx,
                JournalOperation {
                    request_id: "saved-position".into(),
                    expected_state: record.state_token,
                    action: JournalAction::UpdatePosition {
                        current_position: "Saved before lost response".into(),
                    },
                    covered_ranges: vec![],
                },
            )
            .unwrap();
        f.store
            .execute_journal(
                &ctx,
                JournalOperation {
                    request_id: "saved-checkpoint".into(),
                    expected_state: position.state_token,
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec!["saved-record".into()],
                    },
                    covered_ranges: grant.selected_ranges.clone(),
                },
            )
            .unwrap();
        subjektiv::journal::service::finish(&f.store, &ctx).unwrap();
    }
    // Keep the original reservation and diagnosis; map absence proves no cleanup.
    f.api
        .store
        .record_backend_job_reconciliation(
            TEST_WORKSPACE_ID,
            &reserved.attempt,
            "response_lost",
            "fixture retained outcome",
            "Reconcile saved Journal operations and the original execution owner",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    f.assert_occupied();
    let job = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, BackendJobState::Pending);
    assert_eq!(job.current_attempt, 1);
}

#[tokio::test]
async fn subject_removal_keeps_unbound_checkpoint_without_execution_evidence() {
    assert_unbound_pending_snapshot_keeps_source(false, true).await;
}

#[tokio::test]
async fn subject_removal_keeps_legacy_checkpoint_without_execution_evidence() {
    assert_unbound_pending_snapshot_keeps_source(true, true).await;
}

#[tokio::test]
async fn subject_removal_blocks_legacy_unbound_pending_without_execution_evidence() {
    assert_unbound_pending_snapshot_keeps_source(true, false).await;
}

#[tokio::test]
async fn subject_removal_prior_purge_receipt_recovers_without_inventing_erasure() {
    assert_prior_purge_receipt(false).await;
}

#[tokio::test]
async fn subject_removal_prior_pending_purge_receipt_recovers_with_stale_runtime_catalog() {
    assert_prior_purge_receipt(true).await;
}

async fn assert_prior_purge_receipt(stale_catalog: bool) {
    let f = SubjectRemovalFixture::new().await;
    f.seed_completed_release_for_archive_recovery();
    let configured = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    f.api
        .config_store
        .update_worker_retention_policy(
            TEST_WORKSPACE_ID,
            &configured.policy_digest,
            &crate::retention::WorkerRetentionPolicyUpdate {
                policy_id: "old-ordinary-purge".into(),
                session_disposition: worker_runtime::retention::SessionDisposition::Purge,
                metadata_disposition: configured.metadata_disposition,
                archive_retention: configured.archive_retention,
                diagnostics_disposition: worker_runtime::retention::DiagnosticsDisposition::Purge,
                diagnostics_retention_seconds: None,
            },
        )
        .unwrap();
    let lease = f
        .api
        .config_store
        .retained_worker_singleton_lease(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    // Model the pre-upgrade planner's policy choice only; restore the exact Subject
    // reservation before execution. All subsequent retention/receipt paths are real.
    conn.execute("UPDATE worker_create_reservations SET singleton_key=NULL,singleton_generation=NULL WHERE workspace_id=?1 AND worker_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, f.target.worker_id]).unwrap();
    let inventory = f.api.runtime.worker_retention_inventory(&f.target).unwrap();
    let plan = f
        .api
        .config_store
        .plan_worker_removal(
            &crate::retention::WorkerRemovalPlanRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: f.target.clone(),
                reason: "pre-upgrade ordinary Purge".into(),
                journal_erasure: None,
            },
            &inventory,
        )
        .unwrap();
    conn.execute("UPDATE worker_create_reservations SET singleton_key=?3,singleton_generation=?4 WHERE workspace_id=?1 AND worker_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, f.target.worker_id, lease.key, lease.generation]).unwrap();
    drop(conn);
    assert_eq!(
        plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Purge
    );
    let prepared = f
        .api
        .config_store
        .prepare_worker_removal_execution(TEST_WORKSPACE_ID, &plan.plan_id, &plan.input_fingerprint)
        .unwrap();
    if stale_catalog {
        // Fixture the Runtime delete-to-final-receipt crash window. Its live
        // catalog deliberately remains stopped while the aggregate is gone.
        let request = &prepared.runtime_request;
        let pending = worker_runtime::retention::WorkerRetentionExecutionResult {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            worker_id: request.worker_id,
            session_disposition: request.session_disposition,
            diagnostics_disposition: request.diagnostics_disposition,
            archive: None,
            source_removed: false,
            diagnostics_retained: false,
        };
        let receipt = f
            .dir
            .path()
            .join(".test-embedded-runtime-store/retention/operations")
            .join(format!("{}.json", plan.operation_id));
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(
            receipt,
            serde_json::to_vec(&serde_json::json!({
                "schema_version":1, "request":request, "result":pending,
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::remove_dir_all(session_root(f.dir.path(), &f.target).parent().unwrap()).unwrap();
        assert_eq!(f.api.runtime.worker(&f.target).unwrap().state, "stopped");
    } else {
        let result = f
            .api
            .runtime
            .execute_worker_retention(&f.target, prepared.runtime_request.clone())
            .unwrap();
        assert!(result.source_removed);
    }
    f.api
        .config_store
        .fail_worker_removal(
            TEST_WORKSPACE_ID,
            &plan.operation_id,
            &plan.input_fingerprint,
            "metadata_commit_failed",
        )
        .unwrap();
    let f = if stale_catalog { f } else { f.reopen().await };
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(!f.store.journal_session_erased(&f.host).unwrap());
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_none()
    );
    let recovered = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.plan.operation_id, plan.operation_id);
    assert_eq!(recovered.plan.input_fingerprint, plan.input_fingerprint);
    assert_eq!(
        recovered.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
}

#[tokio::test]
async fn disabled_subject_removal_does_not_admit_new_public_history_or_reserve_a_job() {
    let f = SubjectRemovalFixture::new().await;
    f.disable();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_none()
    );
    assert!(f.store.journal_for_session(&f.host).unwrap().is_none());
    assert!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    f.assert_removed_with_history();
}

#[tokio::test]
async fn disabled_subject_removal_defers_unreflected_prefix_without_admitting_new_history() {
    let f = SubjectRemovalFixture::new().await;
    let runtime_api::SessionMaterialCatalogAvailability::Page { page } = f
        .api
        .runtime
        .session_material_catalog(
            &f.target.runtime_id,
            runtime_api::SessionMaterialCatalogRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                source: runtime_api::SessionPublicSource::Retained {
                    worker_id: worker_runtime::identity::WorkerId::parse(&f.target.worker_id)
                        .unwrap(),
                },
                expected_session_id: f.attr.session_id.clone(),
                expected_catalog_sha256: None,
                offset: 0,
                limit: 100,
            },
        )
        .unwrap()
    else {
        panic!("canonical source catalog required")
    };
    let ranges: Vec<_> = page
        .descriptors
        .into_iter()
        .map(|d| JournalSourceRange {
            source: JournalSource {
                session_id: f.attr.session_id.clone(),
                pointer: JournalSourcePointer {
                    segment_id: d.segment_id.parse().unwrap(),
                    entry_ref: d.entry_ref,
                },
                order: d.canonical_order,
                content_sha256: d.sha256,
                total_bytes: d.total_bytes,
            },
            bytes: JournalByteRange {
                start: 0,
                end: d.total_bytes,
            },
        })
        .collect();
    assert_eq!(ranges.len(), 2);
    let id = f
        .store
        .accept_journal_sources(&f.host, "accepted before disablement", &ranges[..1])
        .unwrap();
    let admitted = f
        .store
        .list_journal_sources(&f.host, &id, None, 32)
        .unwrap();
    assert_eq!(admitted.items.len(), 1);
    let before = f.store.read_journal(&f.host, &id).unwrap();
    let source_before = subjektiv_journal::capture_input(&f.api, &f.attr, 0, 0, 48 * 1024).unwrap();
    f.disable();
    let registry = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["code"], "journal_handoff_incomplete");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("unreflected input")
    );
    assert_eq!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap(),
        Some(registry)
    );
    assert!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.store.journal_for_session(&f.host).unwrap().as_deref(),
        Some(id.as_str())
    );
    assert_eq!(f.store.read_journal(&f.host, &id).unwrap(), before);
    assert_eq!(
        f.store
            .list_journal_sources(&f.host, &id, None, 32)
            .unwrap(),
        admitted
    );
    assert!(
        f.store
            .read_journal_source(&f.host, &id, ranges[1].source.order)
            .is_err()
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(!f.store.journal_session_erased(&f.host).unwrap());
    assert!(session_root(f.dir.path(), &f.target).exists());
    let runtime_api::SessionRecordingCaptureAvailability::Captured { slice } = f
        .api
        .runtime
        .session_recording_capture(
            &f.target.runtime_id,
            runtime_api::SessionRecordingCaptureRequest {
                workspace_id: TEST_WORKSPACE_ID.into(),
                source: runtime_api::SessionPublicSource::Retained {
                    worker_id: worker_runtime::identity::WorkerId::parse(&f.target.worker_id)
                        .unwrap(),
                },
                expected_session_id: f.attr.session_id.clone(),
                start: session_store::capture::RecordingPosition {
                    canonical_order: 0,
                    byte_offset: 0,
                },
                max_entries: MAX_JOURNAL_RANGES,
                max_serialized_bytes: 48 * 1024,
            },
        )
        .unwrap()
    else {
        panic!("ordinary removal must retain the readable original history")
    };
    assert_eq!(
        slice.capture, source_before,
        "ordinary removal must retain both the admitted and unadmitted original history"
    );
    assert!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.target, &f.attr.session_id)
            .unwrap()
            .is_none()
    );
    assert!(
        !f.store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.target.runtime_id,
                &f.target.worker_id,
            )
            .unwrap()
    );
    f.assert_occupied();
}

#[tokio::test]
async fn attributed_unmaterialized_subject_removal_releases_without_restoring_controller() {
    let f = SubjectRemovalFixture::with_materialized_source(false).await;
    let retention = source_retention(f.dir.path());
    let session = f.attr.session_id.parse().unwrap();
    // Fixture the pre-publication pin without materializing the Session.
    retention
        .pin(
            session,
            session_store::retention::SessionRetentionPin(
                subjektiv::journal::source_retention_pin(
                    TEST_WORKSPACE_ID,
                    &f.attr.subject_id,
                    &f.attr.session_id,
                )
                .to_string()
                .parse()
                .unwrap(),
            ),
            || Ok(true),
        )
        .unwrap();
    assert_eq!(retention.pins(session).unwrap().len(), 1);
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(retention.pins(session).unwrap().is_empty());
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_none()
    );
    assert!(f.store.journal_for_session(&f.host).unwrap().is_none());
    f.assert_removed_with_history();
}

#[tokio::test]
async fn missing_materialized_subject_source_blocks_removal_without_marking_it_empty() {
    let f = SubjectRemovalFixture::new().await;
    std::fs::remove_dir_all(session_root(f.dir.path(), &f.target)).unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "{}",
        response.body
    );
    assert!(response.body.contains("runtime_retention_failed"));
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_some()
    );
    assert!(f.store.journal_for_session(&f.host).unwrap().is_none());
    assert!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    f.assert_occupied();
}

fn source_retention(root: &std::path::Path) -> session_store::retention::SessionRetentionStore {
    session_store::retention::SessionRetentionStore::new(
        root.join(".test-embedded-runtime-store/retention/session-sources"),
    )
}

#[tokio::test]
async fn explicitly_erased_subject_removal_purges_without_generating_or_archiving() {
    let f = SubjectRemovalFixture::new().await;
    let policy = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap();
    f.api
        .erase_subjektiv_journal_session(&f.attr.session_id, "erase-before-normal-remove")
        .unwrap();
    assert!(f.store.journal_session_erased(&f.host).unwrap());
    assert!(!session_root(f.dir.path(), &f.target).exists());
    // Exact erase replay must resume the same permanent intent even after source removal.
    f.api
        .erase_subjektiv_journal_session(&f.attr.session_id, "erase-before-normal-remove")
        .unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_none()
    );
    let saved = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Purge
    );
    assert_eq!(
        saved.plan.diagnostics_disposition,
        worker_runtime::retention::DiagnosticsDisposition::Purge
    );
    assert!(saved.plan.archive_id.is_none());
    assert!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.target, &f.attr.session_id)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap(),
        policy
    );
    assert!(matches!(
        source_retention(f.dir.path()).ensure_live(f.attr.session_id.parse().unwrap()),
        Err(session_store::StoreError::SessionErased(_))
    ));
}

#[tokio::test]
async fn erased_subject_removal_retries_interrupted_physical_erasure_under_the_permanent_fence() {
    let f = SubjectRemovalFixture::new().await;
    // Fixture boundary: interrupt the real retention deletion after its durable fence,
    // before physical removal. WorkerRemove below uses the real Backend/Runtime erase path.
    f.store
        .erase_journal_session(&f.host, "erase-interrupted")
        .unwrap();
    let retention = source_retention(f.dir.path());
    let session = f.attr.session_id.parse().unwrap();
    assert!(
        retention
            .delete(
                session,
                session_store::retention::SessionDeletionIntent::ErasePermanently,
                || {
                    Err::<(), _>(session_store::StoreError::Io(std::io::Error::other(
                        "injected physical cleanup failure",
                    )))
                }
            )
            .is_err()
    );
    assert!(session_root(f.dir.path(), &f.target).exists());
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert!(
        !session_root(f.dir.path(), &f.target)
            .parent()
            .unwrap()
            .exists()
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.target, &f.attr.session_id)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        retention.ensure_live(session),
        Err(session_store::StoreError::SessionErased(_))
    ));
}

#[tokio::test]
async fn physical_erase_failure_keeps_domain_tombstone_and_job_cancellation_for_exact_retry() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_input();
    // Original source is not stopped: Runtime must reject physical erasure, after
    // the Backend has durably fenced all Journal adoption and cancelled its Job.
    assert!(
        f.api
            .erase_subjektiv_journal_session(&f.session.to_string(), "erase-unsettled-source")
            .is_err()
    );
    assert!(f.store.journal_session_erased(&f.host).unwrap());
    assert!(session_root(f.dir.path(), &f.source).exists());
    let job = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, BackendJobState::Failed);
    assert!(
        subjektiv_journal::execute(
            &f.api,
            dispatched.attempt.worker.as_ref().unwrap(),
            JournalJobRequest::ReadState
        )
        .is_err()
    );
    f.stop_source();
    f.erase_pending_or_acknowledged("erase-unsettled-source");
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    f.assert_erasure_acknowledged("erase-unsettled-source");
    f.assert_purged(dispatched.attempt.worker.as_ref().unwrap());
    assert!(!session_root(f.dir.path(), &f.source).exists());
    assert_eq!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.session.to_string(), None, 100)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn serialized_removal_input_cannot_supply_journal_erasure_authority() {
    let f = SubjectRemovalFixture::new().await;
    let request: crate::retention::WorkerRemovalPlanRequest = serde_json::from_value(json!({
        "workspace_id": TEST_WORKSPACE_ID, "worker": f.target, "reason": "untrusted erase claim",
        "journal_erasure": {"workspace_id": TEST_WORKSPACE_ID, "worker": f.target, "session_id": f.attr.session_id},
    })).unwrap();
    let inventory = f.api.runtime.worker_retention_inventory(&f.target).unwrap();
    let plan = f
        .api
        .store
        .plan_worker_removal(&request, &inventory)
        .unwrap();
    assert_eq!(
        plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Archive
    );
    assert!(!f.store.journal_session_erased(&f.host).unwrap());
    assert!(session_root(f.dir.path(), &f.target).exists());
}

impl SubjectRemovalFixture {
    async fn reopen(self) -> Self {
        let Self {
            dir,
            api,
            execution,
            dispatcher,
            controller,
            target,
            attr,
            host,
            store,
        } = self;
        let config = api.config.clone();
        drop(api);
        drop(execution);
        drop(dispatcher);
        drop(store);
        let control = Arc::new(test_control_store(&config));
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let execution = Arc::new(DeterministicExecutionBackend::default());
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            execution.clone(),
            BackendResourceBroker::default(),
            HostExecutors::default(),
            Some(dispatcher.clone()),
        )
        .await
        .unwrap();
        let store = open_subjektiv_store(&api).unwrap();
        Self {
            dir,
            api,
            execution,
            dispatcher,
            controller,
            target,
            attr,
            host,
            store,
        }
    }

    fn assert_erased_removal(&self) {
        assert!(
            self.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &self.target)
                .unwrap()
                .is_none()
        );
        assert!(
            self.api
                .config_store
                .worker_session_archive(TEST_WORKSPACE_ID, &self.target, &self.attr.session_id)
                .unwrap()
                .is_none()
        );
        assert!(
            self.api
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &self.attr.session_id, None, 100)
                .unwrap()
                .is_empty()
        );
        assert!(!session_root(self.dir.path(), &self.target).exists());
        assert!(matches!(
            source_retention(self.dir.path()).ensure_live(self.attr.session_id.parse().unwrap()),
            Err(session_store::StoreError::SessionErased(_))
        ));
    }
}

// Each case owns a complete isolated fixture; do not serialize independent
// recovery paths inside one test. Transitions within each helper remain sequential.
macro_rules! journal_recovery_cases {
    ($helper:ident; $($name:ident => ($($argument:expr),*)),+ $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                $helper($($argument),*).await;
            }
        )+
    };
}

async fn assert_erased_archive_plan_replans_without_republishing_sources(
    stage: &str,
    restart: bool,
) {
    let mut f = SubjectRemovalFixture::new().await;
    f.disable();
    f.seed_completed_release_for_archive_recovery();
    let policy = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    let archive_parent = f
        .dir
        .path()
        .join(".test-embedded-runtime-store/archives/workers");
    if stage == "planned" {
        conn.execute_batch("CREATE TRIGGER interrupt_removal BEFORE UPDATE OF state ON worker_removal_operations WHEN NEW.state='executing' BEGIN SELECT RAISE(ABORT,'injected execution interruption'); END;").unwrap();
    } else {
        // Fail Archive publication before any receipt. Suppress recording
        // failure too to model the executing-at-Host-stop durable boundary.
        std::fs::create_dir_all(archive_parent.parent().unwrap()).unwrap();
        std::fs::write(&archive_parent, "publication blocked").unwrap();
        if stage == "executing" {
            conn.execute_batch("CREATE TRIGGER interrupt_removal BEFORE UPDATE OF state ON worker_removal_operations WHEN NEW.state='failed' BEGIN SELECT RAISE(ABORT,'injected failure-save interruption'); END;").unwrap();
        }
    }
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    conn.execute_batch("DROP TRIGGER IF EXISTS interrupt_removal")
        .unwrap();
    if stage != "planned" {
        std::fs::remove_file(&archive_parent).unwrap();
    }
    let old = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(old.plan.state).unwrap(), stage);
    assert_eq!(
        old.plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Archive
    );
    f.api
        .erase_subjektiv_journal_session(&f.attr.session_id, "erase-interrupted-archive-plan")
        .unwrap();
    if restart {
        f = f.reopen().await;
    }
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{stage}/{restart}: {}",
        response.body
    );
    let saved = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Purge
    );
    assert_eq!(saved.plan.reason, old.plan.reason);
    assert_ne!(saved.plan.operation_id, old.plan.operation_id);
    let (state, fingerprint, detail): (String, String, String) = conn.query_row(
        "SELECT o.state,o.input_fingerprint,a.detail FROM worker_removal_operations o JOIN worker_retention_audit_events a USING(operation_id) WHERE o.operation_id=?1 AND a.event_kind='erased_archive_superseded'",
        [&old.plan.operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(state, "stale");
    assert_eq!(fingerprint, old.plan.input_fingerprint);
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["old_acceptance"], "unknown_archive_fenced");
    assert!(detail["source_removed"].is_null());
    f.assert_erased_removal();
    assert_eq!(
        f.api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap(),
        policy
    );
}

journal_recovery_cases! {
    assert_erased_archive_plan_replans_without_republishing_sources;
    erased_archive_planned_resend_replans_without_republishing_sources => ("planned", false),
    erased_archive_planned_reopen_replans_without_republishing_sources => ("planned", true),
    erased_archive_executing_resend_replans_without_republishing_sources => ("executing", false),
    erased_archive_executing_reopen_replans_without_republishing_sources => ("executing", true),
    erased_archive_failed_resend_replans_without_republishing_sources => ("failed", false),
    erased_archive_failed_reopen_replans_without_republishing_sources => ("failed", true),
}

async fn assert_erased_completed_archive_receipt_is_audited_without_republishing(restart: bool) {
    let mut f = SubjectRemovalFixture::new().await;
    f.disable();
    f.seed_completed_release_for_archive_recovery();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    // Runtime has committed source_removed, but Backend commit/response is lost.
    conn.execute_batch("CREATE TRIGGER interrupt_removal BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'injected metadata commit interruption'); END;").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    conn.execute_batch("DROP TRIGGER interrupt_removal")
        .unwrap();
    let old = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(
        old.plan.state,
        crate::retention::WorkerRemovalPlanState::Failed
    );
    let receipt = f
        .api
        .runtime
        .execute_worker_retention(&f.target, old.runtime_request.clone())
        .unwrap();
    assert!(receipt.source_removed);
    let archive_id = old.plan.archive_id.as_ref().unwrap();
    let archive_source = f
        .dir
        .path()
        .join(".test-embedded-runtime-store/archives/workers")
        .join(archive_id)
        .join("session");
    assert!(archive_source.exists());
    // The domain tombstone commits even though retained-source resolution fails.
    // Recovery must locate only this plan's exact reserved archive, not publish it.
    assert!(
        f.api
            .erase_subjektiv_journal_session(&f.attr.session_id, "erase-transferred-original")
            .is_err()
    );
    assert!(f.store.journal_session_erased(&f.host).unwrap());
    if restart {
        f = f.reopen().await;
    }
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{restart}: {}",
        response.body
    );
    let saved = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(saved.plan.operation_id, old.plan.operation_id);
    assert_eq!(saved.plan.input_fingerprint, old.plan.input_fingerprint);
    assert_eq!(saved.plan.session_disposition, old.plan.session_disposition);
    let detail: String = conn.query_row("SELECT detail FROM worker_retention_audit_events WHERE operation_id=?1 AND event_kind='erased_removal_receipt'", [&old.plan.operation_id], |r| r.get(0)).unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["source_removed"], true);
    assert_eq!(detail["old_acceptance"], "receipt_confirmed");
    assert!(!archive_source.exists());
    f.assert_erased_removal();
}

journal_recovery_cases! {
    assert_erased_completed_archive_receipt_is_audited_without_republishing;
    erased_completed_archive_receipt_resend_is_audited_without_republishing => (false),
    erased_completed_archive_receipt_reopen_is_audited_without_republishing => (true),
}

#[tokio::test]
async fn erased_archive_without_receipt_or_inventory_remains_unknown_without_replanning() {
    let f = SubjectRemovalFixture::new().await;
    f.disable();
    f.seed_completed_release_for_archive_recovery();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER interrupt_removal BEFORE UPDATE OF state ON worker_removal_operations WHEN NEW.state='executing' BEGIN SELECT RAISE(ABORT,'injected interruption'); END;").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    conn.execute_batch("DROP TRIGGER interrupt_removal")
        .unwrap();
    let old = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    f.api
        .erase_subjektiv_journal_session(&f.attr.session_id, "erase-with-unknown-outcome")
        .unwrap();
    // Corrupt/missing aggregate after an unacknowledged operation is NOT evidence
    // that source_removed completed. Runtime has no matching receipt here.
    std::fs::remove_file(
        session_root(f.dir.path(), &f.target)
            .parent()
            .unwrap()
            .join("worker.json"),
    )
    .unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "{}",
        response.body
    );
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["code"], "erased_removal_outcome_unknown");
    let saved = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(saved.plan.operation_id, old.plan.operation_id);
    assert_eq!(
        saved.plan.state,
        crate::retention::WorkerRemovalPlanState::Executing
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM worker_removal_operations WHERE worker_id=?1",
            [&f.target.worker_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

async fn assert_erased_archive_retained_diagnostics_receipt_finalizes_only_after_physical_ack(
    restart: bool,
    interrupt_erase: bool,
) {
    let mut f = SubjectRemovalFixture::new().await;
    f.disable();
    f.seed_completed_release_for_archive_recovery();
    let policy = f
        .api
        .config_store
        .worker_retention_policy(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let policy = f
        .api
        .config_store
        .update_worker_retention_policy(
            TEST_WORKSPACE_ID,
            &policy.policy_digest,
            &crate::retention::WorkerRetentionPolicyUpdate {
                policy_id: "fixture-retained-diagnostics".into(),
                session_disposition: policy.session_disposition,
                metadata_disposition: policy.metadata_disposition,
                archive_retention: policy.archive_retention,
                diagnostics_disposition: worker_runtime::retention::DiagnosticsDisposition::Retain,
                diagnostics_retention_seconds: Some(3600),
            },
        )
        .unwrap();
    let worker_root = session_root(f.dir.path(), &f.target)
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::create_dir_all(worker_root.join("runs/fixture-run")).unwrap();
    std::fs::write(
        worker_root.join("runs/fixture-run/worker.out.log"),
        "diagnostic original",
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
    // Real Runtime receipt exists; Backend commit/response is interrupted.
    conn.execute_batch("CREATE TRIGGER interrupt_removal BEFORE DELETE ON worker_registry BEGIN SELECT RAISE(ABORT,'injected metadata commit interruption'); END;").unwrap();
    assert_ne!(f.remove().status, StatusCode::OK.as_u16());
    conn.execute_batch("DROP TRIGGER interrupt_removal")
        .unwrap();
    let old = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    let runtime_root = f.dir.path().join(".test-embedded-runtime-store");
    let receipt_path = runtime_root
        .join("retention/operations")
        .join(format!("{}.json", old.plan.operation_id));
    let receipt_bytes = std::fs::read(&receipt_path).unwrap();
    let receipt = f
        .api
        .runtime
        .execute_worker_retention(&f.target, old.runtime_request.clone())
        .unwrap();
    assert!(receipt.source_removed);
    assert!(receipt.diagnostics_retained);
    let diagnostics_root = runtime_root
        .join("archives/diagnostics")
        .join(&old.plan.operation_id);
    let diagnostics_manifest = diagnostics_root.join("manifest.json");
    let diagnostics_bytes = std::fs::read(&diagnostics_manifest).unwrap();
    let archive_source = runtime_root
        .join("archives/workers")
        .join(old.plan.archive_id.as_ref().unwrap())
        .join("session");
    assert!(diagnostics_root.exists());
    assert!(archive_source.exists());
    // Domain erasure succeeds, but no physical acknowledgement can be
    // claimed from the old retained locator after aggregate removal.
    assert!(
        f.api
            .erase_subjektiv_journal_session(&f.attr.session_id, "erase-retained-diagnostics")
            .is_err()
    );
    if interrupt_erase {
        // Runtime must reject mismatched diagnostics identity rather than
        // acknowledge removal of an unrelated operation. No fake Host ack.
        let mut damaged: Value = serde_json::from_slice(&diagnostics_bytes).unwrap();
        damaged["input_fingerprint"] = json!("unrelated-operation");
        std::fs::write(&diagnostics_manifest, serde_json::to_vec(&damaged).unwrap()).unwrap();
        let response = f.remove();
        assert_eq!(
            response.status,
            StatusCode::CONFLICT.as_u16(),
            "{}",
            response.body
        );
        let body: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body["code"], "erased_removal_reconciliation_unconfirmed");
        assert!(
            f.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
                .unwrap()
                .is_some()
        );
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM worker_retention_audit_events WHERE operation_id=?1 AND event_kind='erased_removal_receipt'", [&old.plan.operation_id], |r| r.get(0)).unwrap();
        assert_eq!(
            count, 0,
            "failed acknowledgement cannot authorize finalization/audit"
        );
        assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt_bytes);
        let waiting = f
            .api
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .unwrap();
        assert_eq!(waiting.plan.operation_id, old.plan.operation_id);
        assert_eq!(waiting.plan.input_fingerprint, old.plan.input_fingerprint);
        assert_eq!(
            waiting.plan.state,
            crate::retention::WorkerRemovalPlanState::Executing
        );
        let plans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM worker_removal_operations WHERE worker_id=?1",
                [&f.target.worker_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            plans, 1,
            "missing physical ack cannot authorize a replacement plan"
        );
        std::fs::write(&diagnostics_manifest, &diagnostics_bytes).unwrap();
    }
    if restart {
        f = f.reopen().await;
    }
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "restart={restart}, interrupted={interrupt_erase}: {}",
        response.body
    );
    assert!(!diagnostics_root.exists());
    assert!(!archive_source.exists());
    assert_eq!(
        std::fs::read(&receipt_path).unwrap(),
        receipt_bytes,
        "historical Runtime receipt must not be rewritten"
    );
    let saved = f
        .api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.target)
        .unwrap()
        .unwrap();
    assert_eq!(saved.plan.operation_id, old.plan.operation_id);
    assert_eq!(saved.plan.input_fingerprint, old.plan.input_fingerprint);
    assert_eq!(
        saved.plan.diagnostics_disposition,
        old.plan.diagnostics_disposition
    );
    let detail: String = conn.query_row("SELECT detail FROM worker_retention_audit_events WHERE operation_id=?1 AND event_kind='erased_removal_receipt'", [&old.plan.operation_id], |r| r.get(0)).unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["source_removed"], true);
    assert_eq!(detail["old_diagnostics_retained"], true);
    assert_eq!(
        detail["runtime_erasure_ack"]["session_id"],
        f.attr.session_id
    );
    let diagnostics_catalog: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM worker_diagnostics_archives WHERE operation_id=?1",
            [&old.plan.operation_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(diagnostics_catalog, 0);
    let tombstone_archive: Option<String> = conn
        .query_row(
            "SELECT archive_id FROM worker_tombstones WHERE operation_id=?1",
            [&old.plan.operation_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(tombstone_archive.is_none());
    f.assert_erased_removal();
    assert_eq!(
        f.api
            .config_store
            .worker_retention_policy(TEST_WORKSPACE_ID)
            .unwrap(),
        Some(policy)
    );
}

journal_recovery_cases! {
    assert_erased_archive_retained_diagnostics_receipt_finalizes_only_after_physical_ack;
    erased_archive_retained_diagnostics_resend_finalizes_only_after_physical_ack => (false, false),
    erased_archive_retained_diagnostics_interrupted_resend_finalizes_only_after_physical_ack => (false, true),
    erased_archive_retained_diagnostics_reopen_finalizes_only_after_physical_ack => (true, false),
    erased_archive_retained_diagnostics_interrupted_reopen_finalizes_only_after_physical_ack => (true, true),
}

#[tokio::test]
async fn legacy_journal_grant_preserves_wire_digest_but_authorizes_neither_writer() {
    let f = SubjectRemovalFixture::new().await;
    let old = json!({"subject_id":f.attr.subject_id,"session_id":f.attr.session_id,"journal_id":"legacy-journal","selected_ranges":[]});
    let wire = serde_json::to_string(&old).unwrap();
    let grant: SubjektivJournalGrant = serde_json::from_str(&wire).unwrap();
    assert!(grant.binding_id.is_empty());
    assert_eq!(
        serde_json::to_value(&grant).unwrap(),
        old,
        "old canonical request/digest must not acquire a new empty binding_id field"
    );
    let roundtrip = serde_json::to_string(&serde_json::to_value(&grant).unwrap()).unwrap();
    assert_eq!(roundtrip, wire);
    assert_eq!(
        job::sha256(roundtrip.as_bytes()),
        job::sha256(wire.as_bytes())
    );
    let mut bound = grant.clone();
    bound.binding_id = f.host.writer_binding_id().unwrap().into();
    assert!(subjektiv_journal::host(&f.api, &f.store, &bound).is_ok());
    assert!(
        matches!(
            subjektiv_journal::host(&f.api, &f.store, &grant),
            Err(Error::WorkspacePermissionDenied(_))
        ),
        "wire compatibility is not authority even for the original writer"
    );
    let writer = JournalWriterBinding {
        subject_id: f.attr.subject_id.clone(),
        runtime_id: f.attr.runtime_id.clone(),
        worker_id: f.attr.worker_id.clone(),
        binding_id: f.host.writer_binding_id().unwrap().into(),
    };
    f.store.release_journal_writer(&writer).unwrap();
    let successor = f
        .store
        .claim_journal_writer(&f.attr.subject_id, &f.attr.runtime_id, "different-body")
        .unwrap();
    assert!(f.store.journal_writer_active(&successor).unwrap());
    assert!(!f.store.journal_writer_active(&writer).unwrap());
    assert_ne!(successor.binding_id, writer.binding_id);
    assert!(
        matches!(
            subjektiv_journal::host(&f.api, &f.store, &grant),
            Err(Error::WorkspacePermissionDenied(_))
        ),
        "legacy grant must never map to a later writer"
    );
    assert_eq!(serde_json::to_value(&grant).unwrap(), old);
}

/// Real embedded Runtime capture + SQLite + production dispatch; synthetic ordinary
/// Session entries only, no fixed Worker data, fixture generator or model calls.
mod immutable {
    use super::*;
    use session_store::{LogEntry, Store as _, WorkerSessionStore};
    use std::collections::BTreeMap;
    use subjektiv::journal::recording::JournalInput;

    fn history(id: &str, item: session_store::LoggedItem) -> session_store::LoggedHistoryEntry {
        session_store::LoggedHistoryEntry {
            item,
            metadata: session_store::LoggedSessionHistoryMetadata {
                input_metadata: None,
                entry_id: session_store::LoggedSessionHistoryEntryId(id.into()),
                origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
            },
        }
    }
    fn append(f: &RuntimeFixture, segment: Uuid, id: &str) {
        WorkerSessionStore::new(session_root(f.dir.path(), &f.source))
            .unwrap()
            .append(
                f.session,
                segment,
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: history(id, message(id)),
                },
            )
            .unwrap();
    }
    fn checkpoint_ranges(f: &RuntimeFixture, worker: &RuntimeWorkerRef, input: &JournalInput) {
        let state = f.store.read_journal(&f.host, &f.id).unwrap();
        let record_id = format!("immutable-record:{}", input.initial_state_token);
        let operation = JournalOperation {
            request_id: record_id.clone(),
            expected_state: state.state_token,
            action: JournalAction::ConfirmUnchanged,
            covered_ranges: input.selected_ranges.clone(),
        };
        let response = subjektiv_journal::execute(
            &f.api,
            worker,
            JournalJobRequest::Execute {
                operation: operation.clone(),
            },
        )
        .unwrap();
        // Exact operation replay after a lost response cannot duplicate or advance the cursor.
        assert_eq!(
            subjektiv_journal::execute(&f.api, worker, JournalJobRequest::Execute { operation })
                .unwrap(),
            response
        );
        let JournalJobResponse::Saved(record) = response else {
            panic!("record receipt")
        };
        assert_eq!(
            f.store
                .query_journal_operation(&f.host, &f.id, &record_id)
                .unwrap(),
            Some(record.clone())
        );
        assert_eq!(
            f.store.read_journal(&f.host, &f.id).unwrap().process_order,
            state.process_order
        );
        let JournalJobResponse::Saved(position) = subjektiv_journal::execute(
            &f.api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: format!("immutable-position:{}", input.initial_state_token),
                    expected_state: record.state_token,
                    action: JournalAction::UpdatePosition {
                        current_position: "Original prefix saved; remaining work unconfirmed"
                            .into(),
                    },
                    covered_ranges: vec![],
                },
            },
        )
        .unwrap() else {
            panic!("position receipt")
        };
        subjektiv_journal::execute(
            &f.api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: format!("immutable-checkpoint:{}", input.initial_state_token),
                    expected_state: position.state_token,
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec![record_id],
                    },
                    covered_ranges: input.selected_ranges.clone(),
                },
            },
        )
        .unwrap();
    }

    fn frozen_lifetime_evidence(
        f: &RuntimeFixture,
        generation: &subjektiv::journal::JournalGeneration,
        phase: &str,
    ) -> Value {
        let job = current_journal_job(f);
        let attempt = current_journal_attempt(f, &generation.job.attempt_id);
        let fence = f.job_store.with_conn(|connection| {
            use rusqlite::OptionalExtension as _;
            Ok(connection.query_row(
                "SELECT json_object('owner', json(owner_json), 'history', json(history_json),
                    'controller', json(controller_json), 'host', json(host_json), 'recorded_at', recorded_at)
                 FROM backend_job_execution_fences WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",
                rusqlite::params![TEST_WORKSPACE_ID, generation.job.job_id, generation.job.attempt_id],
                |row| row.get::<_, String>(0),
            ).optional()?)
        }).unwrap();
        let removal = attempt.worker.as_ref().and_then(|worker| {
            f.api
                .config_store
                .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
                .unwrap()
                .map(|saved| {
                    json!({
                        "operation_id": saved.plan.operation_id,
                        "input_fingerprint": saved.plan.input_fingerprint,
                        "state": saved.plan.state,
                        "worker": saved.plan.worker,
                        "session_disposition": saved.runtime_request.session_disposition,
                    })
                })
        });
        let domain = f
            .api
            .feature_storage
            .open(&f.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                let domain: String = connection.query_row(
                    "SELECT json_object('generation_id', g.id, 'job_id', g.job_id, 'attempt_id', g.attempt_id, 'active', g.active,
                        'completion', json(g.completion_json),
                        'process_order', j.process_order, 'process_byte', j.process_byte,
                        'checkpoints', (SELECT COUNT(*) FROM journal_operations o
                            WHERE o.generation_id=g.id
                            AND json_extract(o.request_json,'$.action.action')='checkpoint'))
                    FROM journal_generations g JOIN journals j ON j.id=g.journal_id
                    WHERE g.id=?1 AND g.job_id=?2 AND g.attempt_id=?3",
                    rusqlite::params![
                        generation.id,
                        generation.job.job_id,
                        generation.job.attempt_id
                    ],
                    |row| row.get(0),
                )?;
                Ok(domain)
            })
            .unwrap();
        let evidence = json!({
            "phase": phase,
            "job": {
                "job_id": job.request.job_id, "state": job.state,
                "current_attempt": job.current_attempt, "input_digest": job.input_digest,
                "input_disposition": job.input_disposition, "result": job.result,
                "failure_category": job.failure_category, "failure_detail": job.failure_detail,
            },
            "attempt": attempt,
            "retained": f.store.retained_journal_generation(&f.host, &f.id, &generation.job).unwrap(),
            "domain": serde_json::from_str::<Value>(&domain).unwrap(),
            "execution_fence": f.api.store.has_backend_job_execution_fence(TEST_WORKSPACE_ID, &attempt).unwrap(),
            "fence_receipt": fence.map(|raw| serde_json::from_str::<Value>(&raw).unwrap()),
            "canonical_removal": removal,
            "source_writer_binding_id": f.host.writer_binding_id(),
            "source_erased": f.store.journal_session_erased(&f.host).unwrap(),
            "source_registered": f.api.store.get_worker_registry(TEST_WORKSPACE_ID, &f.source).unwrap().is_some(),
        });
        eprintln!("frozen-generation-lifetime: {evidence}");
        evidence
    }

    #[tokio::test]
    async fn backend_immutable_input_survives_missing_source_and_predispatch_host_reopen() {
        let RuntimeFixture {
            dir,
            api,
            execution,
            job_store,
            store,
            host,
            source,
            session,
            request,
            reserved,
            range,
            id,
        } = RuntimeFixture::new().await;
        let frozen = serde_json::to_vec(&request.input).unwrap();
        let digest = request.input_digest().unwrap();
        subjektiv_journal::prepare(&api, &reserved).unwrap();
        let generation = store
            .active_journal_generation(&host, &id)
            .unwrap()
            .unwrap();
        let root = session_root(dir.path(), &source);
        let unavailable = dir.path().join("source-unavailable-during-job");
        std::fs::rename(&root, &unavailable).unwrap();
        let config = api.config.clone();
        drop(api);
        drop(store);
        drop(job_store);
        // Reopen after input save but before dispatch, with no readable original source.
        let job_store = Arc::new(test_control_store(&config));
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config.clone(),
            job_store.clone(),
            execution.clone(),
            BackendResourceBroker::default(),
            HostExecutors::default(),
            None,
        )
        .await
        .unwrap();
        let store = open_subjektiv_store(&api).unwrap();
        let f = RuntimeFixture {
            dir,
            api,
            execution,
            job_store,
            store,
            host,
            source,
            session,
            request,
            reserved,
            range,
            id,
        };
        // The production constructor itself restores the accepted owner and may
        // dispatch. Join that test owner's work, then use its real dispatch gate
        // to order the two receipt-reconciliation cuts below. No live Worker is stopped.
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            f.api.stop_backend_job_recovery(),
        )
        .await
        .expect("startup recovery did not quiesce for the frozen-generation cut");
        let gate = f
            .api
            .backend_job_recovery
            .dispatch_gate(&generation.job.attempt_id);
        let saved = current_journal_job(&f);
        let reopened = frozen_lifetime_evidence(&f, &generation, "reopen_before_checkpoint");
        assert_eq!(
            serde_json::to_vec(&saved.load_executable_request().unwrap().input).unwrap(),
            frozen
        );
        assert_eq!(saved.input_digest, digest);
        assert_eq!(saved.state, BackendJobState::Pending, "{reopened}");
        let dispatched = f
            .api
            .dispatch_backend_job_reservation(f.reserved.clone())
            .unwrap();
        assert_eq!(
            dispatched.attempt.state,
            BackendJobAttemptState::Dispatched,
            "{reopened}"
        );
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        // Background recovery was joined; initial dispatch has returned. Hold the
        // existing producer gate through the unreflected recovery and checkpoint.
        let dispatch_finished = gate.lock().unwrap();
        let before = frozen_lifetime_evidence(&f, &generation, "dispatch_finished");
        assert_eq!(before["domain"]["active"], 1, "{before}");
        assert!(before["domain"]["completion"].is_null(), "{before}");
        assert_eq!(before["domain"]["checkpoints"], 0, "{before}");
        assert_eq!(before["execution_fence"], false, "{before}");
        assert_eq!(
            f.store
                .retained_journal_generation(&f.host, &f.id, &generation.job)
                .unwrap(),
            Some(generation.clone()),
            "{before}"
        );
        assert!(!root.exists());
        let inputs = f.execution.take_inputs();
        assert_eq!(
            inputs.len(),
            1,
            "one exact submission, including startup dispatch"
        );
        assert_eq!(
            inputs[0].1,
            f.request
                .worker_input(&dispatched.attempt.attempt_id)
                .unwrap()
        );

        // Same callee as recover_backend_job_step, at an exact current owner cut.
        // There is neither a checkpoint nor an execution/Host fence. Incomplete
        // finish MUST NOT close; missing fences MUST NOT authorize continuation.
        let error = subjektiv_journal::reconcile_attempt(
            &f.api,
            &current_journal_job(&f),
            &current_journal_attempt(&f, &generation.job.attempt_id),
        )
        .expect_err("unreflected generation without execution fences cannot complete or retry");
        let unreflected = frozen_lifetime_evidence(&f, &generation, "owner_before_checkpoint");
        eprintln!("frozen-generation-lifetime: incomplete_owner_error={error}");
        assert!(
            matches!(error, Error::RepositoryConflict(_)),
            "{error}; {unreflected}"
        );
        assert_eq!(unreflected["domain"], before["domain"], "{unreflected}");
        assert_eq!(current_journal_job(&f).state, BackendJobState::Pending);
        assert_eq!(
            current_journal_attempt(&f, &generation.job.attempt_id).state,
            BackendJobAttemptState::Dispatched
        );

        // Keep canonical cleanup before claim: a terminal Job cannot hide the
        // generation's close cause behind purge, retirement or a later owner.
        let cleanup_gate = Arc::new(tokio::sync::Semaphore::new(0));
        *f.api
            .backend_job_recovery
            .cleanup_before_claim
            .lock()
            .unwrap() = Some(cleanup_gate.clone());
        f.checkpoint(worker);
        let checkpoint = f
            .store
            .query_journal_operation(&f.host, &f.id, "checkpoint")
            .unwrap()
            .unwrap();
        let checkpointed = frozen_lifetime_evidence(&f, &generation, "checkpoint_before_owner");
        assert_eq!(checkpointed["domain"]["active"], 1, "{checkpointed}");
        assert!(
            checkpointed["domain"]["completion"].is_null(),
            "{checkpointed}"
        );
        assert_eq!(checkpointed["domain"]["checkpoints"], 1, "{checkpointed}");
        assert_eq!(checkpointed["domain"]["process_order"], 1, "{checkpointed}");
        f.api.backend_job_recovery.resume_after_test_pause();
        f.api.own_backend_job_recovery(&dispatched.attempt);
        drop(dispatch_finished);

        // Release only the producer gate. No explicit finish/result/second input:
        // the actual root owner must recover the saved checkpoint on the same Job.
        let completed = observe_journal_completion(&f).await;
        let settled = frozen_lifetime_evidence(&f, &generation, "owner_completed_cleanup_blocked");
        assert_eq!(completed.state, BackendJobState::Completed, "{settled}");
        assert_eq!(completed.current_attempt, dispatched.attempt.attempt);
        assert_eq!(completed.input_digest, digest);
        let completion: JournalJobCompletion =
            serde_json::from_value(completed.result.clone().unwrap()).unwrap();
        assert_eq!(completion.generation_id, generation.id, "{settled}");
        assert_eq!(completion.process_order, 1);
        assert_eq!(completion.process_byte, 0);
        assert_eq!(settled["domain"]["active"], 0, "{settled}");
        assert_eq!(
            settled["domain"]["completion"],
            json!(completion),
            "{settled}"
        );
        // The retained completion is written atomically with the normal close.
        // Cleanup is still gated, so neither a cleanup close nor an execution
        // fence can be the cause of this active -> completed transition.
        assert_eq!(settled["domain"]["job_id"], json!(generation.job.job_id));
        assert_eq!(
            settled["domain"]["attempt_id"],
            json!(generation.job.attempt_id)
        );
        assert_eq!(settled["domain"]["generation_id"], json!(generation.id));
        assert_eq!(settled["domain"]["checkpoints"], 1, "{settled}");
        assert_eq!(settled["execution_fence"], false, "{settled}");
        let terminal = current_journal_attempt(&f, &generation.job.attempt_id);
        assert_eq!(terminal.worker, dispatched.attempt.worker);
        assert_eq!(terminal.runtime_run_id, dispatched.attempt.runtime_run_id);
        assert_eq!(
            terminal.worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Pending)
        );
        assert_eq!(
            f.store
                .retained_journal_generation(&f.host, &f.id, &generation.job)
                .unwrap(),
            Some(generation.clone()),
            "{settled}"
        );
        assert_eq!(
            f.store
                .query_journal_operation(&f.host, &f.id, "checkpoint")
                .unwrap(),
            Some(checkpoint)
        );
        assert_eq!(
            completed.load_executable_request().unwrap().input,
            f.request.input
        );
        assert!(
            f.execution.take_inputs().is_empty(),
            "owner must not resubmit input"
        );
        assert!(
            !root.exists(),
            "completion must not reacquire the original source"
        );

        std::fs::rename(&unavailable, &root).unwrap();
        cleanup_gate.add_permits(1);
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        let cleaned = frozen_lifetime_evidence(&f, &generation, "cleanup_completed");
        assert_eq!(cleaned["execution_fence"], true, "{cleaned}");
        assert_eq!(
            cleaned["domain"], settled["domain"],
            "cleanup cannot reclose a different owner"
        );
        assert_eq!(
            f.api
                .store
                .list_backend_job_attempts(TEST_WORKSPACE_ID, &f.request.job_id)
                .unwrap()
                .len(),
            1,
            "checkpoint recovery must not create a continuation attempt"
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            f.api.stop_backend_job_recovery(),
        )
        .await
        .expect("frozen-generation recovery did not finish");
    }

    #[tokio::test]
    async fn backend_over_100_entries_long_utf8_cap_continuation_and_postcapture_compaction() {
        let long = "原文🙂é".repeat(36_000);
        let mut entries: Vec<_> = (0..130)
            .map(|n| {
                history(
                    &format!("ordinary-{n}"),
                    message(&format!("ordinary original {n:03}")),
                )
            })
            .collect();
        entries.push(history(
            "long-tool",
            session_store::LoggedItem::ToolResult {
                call_id: "ordinary-tool".into(),
                summary: "NOT THE ORIGINAL BODY".into(),
                content: Some(long.clone()),
                attachments: vec![],
                disposition: Default::default(),
                is_error: false,
            },
        ));
        let f = RuntimeFixture::with_history(entries).await;
        let attr = f
            .store
            .session_attribution(&f.session.to_string())
            .unwrap()
            .unwrap();
        let original_segment = f.range.source.pointer.segment_id;
        let frozen = serde_json::to_vec(&f.request.input).unwrap();
        let rebuilt = JournalInput::build(&f.store, &f.host, &f.id, |order, offset, budget| {
            let captured = subjektiv_journal::capture_input(&f.api, &attr, order, offset, budget)
                .map_err(|e| JournalError::Conflict(e.to_string()))?;
            // Append after Host capture but before the enclosing Journal input is finalized.
            append(&f, original_segment, "append-before-dispatch");
            Ok(captured)
        })
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&serde_json::to_value(&rebuilt).unwrap()).unwrap(),
            frozen
        );
        assert!(
            !serde_json::to_string(&rebuilt)
                .unwrap()
                .contains("append-before-dispatch")
        );
        let first = f
            .api
            .dispatch_backend_job_reservation(f.reserved.clone())
            .unwrap();
        // A real compaction header carries exactly one excluded opening summary
        // and its complete original lineage; an empty header is intentionally invalid.
        let mut summary = history(
            "compaction-summary",
            session_store::LoggedItem::Message {
                role: session_store::LoggedRole::System,
                content: vec![session_store::LoggedContentPart::Text {
                    text: "PRIVATE_COMPACTION_SUMMARY_NEVER_CAPTURE".into(),
                }],
            },
        );
        summary.metadata.input_metadata = Some(
            session_store::HistoryInputMetadata::new(
                session_store::HistoryInputUsePolicy {
                    recording_material: session_store::InputEvidenceUse::Exclude,
                    memory_evidence: session_store::InputEvidenceUse::Exclude,
                    observation: session_store::InputObservationUse::Hidden,
                    compaction: session_store::InputCompactionUse::OpeningSummary,
                },
                None,
                None,
            )
            .unwrap(),
        );
        summary.metadata.origin = session_store::LoggedSessionHistoryOrigin::Provided {
            producer: "ordinary-compaction".into(),
        };
        summary.metadata.derivation = Some(session_store::LoggedHistoryDerivation {
            sources: (0..130)
                .map(|n| session_store::LoggedSessionHistoryEntryId(format!("ordinary-{n}")))
                .chain([
                    session_store::LoggedSessionHistoryEntryId("long-tool".into()),
                    session_store::LoggedSessionHistoryEntryId("append-before-dispatch".into()),
                ])
                .collect(),
        });
        // Actual compacted Segment/pointer movement while the first Job is executing.
        let compact = Uuid::new_v4();
        let root = session_root(f.dir.path(), &f.source);
        WorkerSessionStore::new(&root)
            .unwrap()
            .create_segment(
                f.session,
                compact,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 3,
                    session_id: f.session,
                    system_prompt: Some("PRIVATE_SYSTEM_NEVER_CAPTURE".into()),
                    config: Default::default(),
                    history: vec![summary],
                    forked_from: None,
                    compacted_from: Some(session_store::SegmentOrigin {
                        segment_id: original_segment,
                        at_turn_index: 0,
                    }),
                }],
            )
            .unwrap();
        use session_store::WorkerMetadataStore as _;
        let aggregate = root.parent().unwrap();
        let name = format!("worker-runtime-{}", f.source.worker_id);
        session_store::WorkerAggregateStore::new(aggregate, &name)
            .unwrap()
            .write(
                &session_store::WorkerMetadata::new(
                    &name,
                    Some(session_store::WorkerActiveSegmentRef::active_segment(
                        f.session, compact,
                    )),
                )
                .with_workspace_id(TEST_WORKSPACE_ID),
            )
            .unwrap();
        append(&f, compact, "append-during-job");
        assert_eq!(
            serde_json::to_vec(
                &f.api
                    .store
                    .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
                    .unwrap()
                    .unwrap()
                    .request
                    .input
            )
            .unwrap(),
            frozen
        );

        let mut reconstructed: BTreeMap<u64, (JournalSource, String)> = BTreeMap::new();
        let mut current = first;
        let mut generations = 0;
        let mut split_tool = 0;
        loop {
            generations += 1;
            assert!(generations < 20, "bounded continuation made no progress");
            let input = JournalInput::decode(&current.job.request.input).unwrap();
            assert!(
                serde_json::to_vec(&current.job.request.input)
                    .unwrap()
                    .len()
                    <= job::MAX_JOB_INPUT_BYTES
            );
            assert_eq!(
                input.selected_ranges,
                current
                    .job
                    .request
                    .grants
                    .subjektiv_journal
                    .as_ref()
                    .unwrap()
                    .selected_ranges
            );
            let before = f.store.read_journal(&f.host, &f.id).unwrap();
            assert_eq!(input.selected_ranges[0].source.order, before.process_order);
            assert_eq!(input.selected_ranges[0].bytes.start, before.process_byte);
            for (entry, range) in input.capture.entries.iter().zip(&input.selected_ranges) {
                let session_store::capture::CaptureBody::CanonicalPublic {
                    text, byte_offset, ..
                } = &entry.body
                else {
                    panic!("original canonical bytes")
                };
                let slot = reconstructed
                    .entry(range.source.order)
                    .or_insert_with(|| (range.source.clone(), String::new()));
                assert_eq!(slot.0, range.source);
                assert_eq!(
                    *byte_offset,
                    slot.1.len() as u64,
                    "no overlap or skipped bytes across Jobs"
                );
                assert_eq!(range.bytes.end - range.bytes.start, text.len() as u64);
                slot.1.push_str(text);
                if entry.entry_id == "long-tool" {
                    split_tool += 1;
                }
                assert!(!text.contains("PRIVATE_SYSTEM_NEVER_CAPTURE"));
                assert!(!text.contains("PRIVATE_COMPACTION_SUMMARY_NEVER_CAPTURE"));
            }
            let worker = current.attempt.worker.as_ref().unwrap();
            let encoded = current
                .job
                .request
                .worker_input(&current.attempt.attempt_id)
                .unwrap();
            assert!(
                f.execution
                    .take_inputs()
                    .iter()
                    .any(|(_, text)| text == &encoded)
            );
            // Host delivery is not a processing checkpoint.
            assert_eq!(
                f.store.read_journal(&f.host, &f.id).unwrap().process_order,
                before.process_order
            );
            checkpoint_ranges(&f, worker, &input);
            let completion = subjektiv_journal::finish(&f.api, worker).unwrap();
            let done = completion.process_order == 133 && completion.process_byte == 0;
            f.api
                .accept_backend_job_result(
                    worker,
                    &BackendJobResultSubmission {
                        job_id: current.job.request.job_id.clone(),
                        attempt_id: current.attempt.attempt_id.clone(),
                        input_digest: current.job.input_digest.clone(),
                        result: serde_json::to_value(completion).unwrap(),
                    },
                )
                .unwrap();
            wait_for_backend_job_worker_cleanup(
                &f.api,
                &current.job.request.job_id,
                &current.attempt.attempt_id,
                BackendJobWorkerCleanupState::Completed,
            )
            .await;
            if done {
                break;
            }
            current = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let jobs = f
                        .api
                        .store
                        .list_backend_journal_jobs(
                            TEST_WORKSPACE_ID,
                            &f.session.to_string(),
                            None,
                            100,
                        )
                        .unwrap();
                    for job in jobs
                        .into_iter()
                        .filter(|job| job.state == BackendJobState::Pending)
                    {
                        let attempt = f
                            .api
                            .store
                            .get_backend_job_attempt(
                                TEST_WORKSPACE_ID,
                                &job.request.job_id,
                                &crate::backend_job::attempt_id(
                                    &job.request.job_id,
                                    job.current_attempt,
                                ),
                            )
                            .unwrap()
                            .unwrap();
                        if attempt.state == BackendJobAttemptState::Dispatched {
                            return BackendJobReservation {
                                job,
                                attempt,
                                replayed: true,
                                resource_reused: false,
                            };
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                let state = f.store.read_journal(&f.host, &f.id).unwrap();
                let probe = JournalInput::build(&f.store, &f.host, &f.id, |order, offset, budget| {
                    subjektiv_journal::capture_input(&f.api, &attr, order, offset, budget)
                        .map_err(|e| JournalError::Conflict(e.to_string()))
                }).map(|input| input.selected_ranges.iter().map(|range| (range.source.order, range.bytes.clone())).collect::<Vec<_>>());
                panic!("production drain stalled after {generations} generations at {}/{}, next capture={probe:?}", state.process_order, state.process_byte);
            });
        }
        assert!(generations > 3);
        assert!(
            split_tool > 1,
            "long UTF-8 source must continue across whole-envelope caps"
        );
        assert_eq!(reconstructed.len(), 133);
        for (order, (source, bytes)) in &reconstructed {
            assert_eq!(
                bytes.len() as u64,
                source.total_bytes,
                "complete canonical source {order}"
            );
            let digest: String = Sha256::digest(bytes.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(
                digest, source.content_sha256,
                "exact original body, not summary or preview"
            );
        }
        assert!(reconstructed[&130].1.contains(&long));
        assert!(reconstructed[&131].1.contains("append-before-dispatch"));
        assert!(reconstructed[&132].1.contains("append-during-job"));
    }
}

#[tokio::test]
async fn explicit_erasure_reclaims_over_100_historical_body_inputs_and_waits_for_delivered_copy_ack()
 {
    let mut f = RuntimeFixture::new().await;
    f.api
        .store
        .finish_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &f.reserved.attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "ordinary_predispatch_failure",
            "saved input remains resumable",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    // Synthetic ordinary historical Job intents: no execution/model fixture. Failed
    // attempts deliberately keep resume bodies, so erasure must page beyond 100.
    for n in 0..105 {
        let mut request = f.request.clone();
        request.job_id = format!("historical-body-{n:03}");
        let reserved = f
            .api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
            .unwrap();
        assert_eq!(reserved.job.request.input, f.request.input);
        f.api
            .store
            .finish_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &request.job_id,
                &reserved.attempt.attempt_id,
                BackendJobAttemptState::Failed,
                "ordinary_historical_failure",
                "saved input still required for resume",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
    }
    f.reserved = f
        .api
        .store
        .reserve_backend_job_retry(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let delivered = f.dispatch_input();
    let worker = delivered.attempt.worker.as_ref().unwrap();
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "pinned",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    f.stop_source();
    let key = "erase-all-historical-job-bodies";
    assert!(
        f.erase_pending_or_acknowledged(key),
        "blocked copy must never acknowledge erasure"
    );
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &delivered.attempt.attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    assert!(
        session_root(f.dir.path(), worker).exists(),
        "failure injection remains effective"
    );
    let first = f
        .api
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.session.to_string(), None, 100)
        .unwrap();
    assert_eq!(first.len(), 100);
    let rest = f
        .api
        .store
        .list_backend_journal_jobs(
            TEST_WORKSPACE_ID,
            &f.session.to_string(),
            Some(&first.last().unwrap().request.job_id),
            100,
        )
        .unwrap();
    assert_eq!(rest.len(), 6);
    for saved in first.iter().chain(&rest) {
        assert_eq!(
            saved.input_disposition,
            crate::backend_job::JobInputDisposition::Erased
        );
        assert_eq!(saved.input_digest, f.request.input_digest().unwrap());
        assert!(saved.load_executable_request().is_err());
        for attempt in f
            .api
            .store
            .list_backend_job_attempts(TEST_WORKSPACE_ID, &saved.request.job_id)
            .unwrap()
        {
            if attempt.worker.is_none() {
                assert!(
                    attempt.allocation_precluded_at.is_some(),
                    "no inference from Failed or missing Worker"
                );
                f.api
                    .require_backend_job_attempt_effects_ended(&attempt)
                    .unwrap();
            } else {
                assert!(
                    attempt.allocation_precluded_at.is_none(),
                    "an admitted execution needs its separate termination/Host receipts"
                );
            }
        }
        assert!(
            !serde_json::to_string(&saved.request)
                .unwrap()
                .contains(SOURCE_SENTINEL)
        );
    }
    // Domain and input reclamation alone are not a delivered-copy cleanup ack.
    assert!(f.erase_pending_or_acknowledged(key));
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "normal",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    f.api.recover_backend_jobs().unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &delivered.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    f.assert_erasure_acknowledged(key);
    f.assert_purged(worker);
    assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
}

mod lifecycle_barrier {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    const DEADLINE: Duration = Duration::from_secs(20);
    // Bounded observation of an already signalled callback while the actual mutex
    // is demonstrably held. This is a channel timeout, never a scheduling sleep.
    const HELD_WINDOW: Duration = Duration::from_millis(100);

    pub(super) fn spawn<T: Send + 'static>(
        work: impl FnOnce() -> T + Send + 'static,
    ) -> (std::thread::JoinHandle<()>, Receiver<T>) {
        let runtime = tokio::runtime::Handle::current();
        let (tx, rx) = mpsc::channel();
        let join = std::thread::spawn(move || {
            let _runtime = runtime.enter();
            let result = work();
            tx.send(result).unwrap();
        });
        (join, rx)
    }
    pub(super) fn joined<T>(task: (std::thread::JoinHandle<()>, Receiver<T>)) -> T {
        let result = task
            .1
            .recv_timeout(DEADLINE)
            .expect("bounded callback progress; no gate/Runtime acknowledgement cycle");
        task.0.join().expect("callback thread joined");
        result
    }
    fn acceptance_barrier(f: &RuntimeFixture) -> (Receiver<RuntimeWorkerRef>, Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let root = f.dir.path().to_path_buf();
        let encoded = f
            .request
            .worker_input(&f.reserved.attempt.attempt_id)
            .unwrap();
        f.execution.before_input_returns(move |worker| {
            let worker =
                RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, worker.worker_id.to_string());
            seed_session(
                &session_root(&root, &worker),
                session_store::new_session_id(),
                message(&encoded),
                session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
            );
            entered_tx.send(worker).unwrap();
            // Model the Runtime's SubmissionAccepted boundary, NOT Tool completion.
            // A real Controller can send the first Tool concurrently. Waiting for
            // that Tool here would manufacture the very circular wait under test.
            release_rx
                .recv_timeout(DEADLINE)
                .expect("test released Runtime acceptance");
        });
        (entered_rx, release_tx)
    }
    fn dispatch(
        f: &RuntimeFixture,
    ) -> (
        std::thread::JoinHandle<()>,
        Receiver<Result<BackendJobReservation>>,
    ) {
        let api = f.api.clone();
        let reserved = f.reserved.clone();
        spawn(move || api.dispatch_backend_job_reservation(reserved))
    }
    fn operation(f: &RuntimeFixture) -> JournalOperation {
        let generation = f
            .store
            .active_journal_generation(&f.host, &f.id)
            .unwrap()
            .unwrap();
        JournalOperation {
            request_id: "first-tool-after-acceptance".into(),
            expected_state: generation.initial_journal.state_token,
            action: JournalAction::ConfirmUnchanged,
            covered_ranges: vec![f.range.clone()],
        }
    }
    fn assert_no_preaccept_attestation(f: &RuntimeFixture) {
        let generation = f
            .store
            .active_journal_generation(&f.host, &f.id)
            .unwrap()
            .unwrap();
        let context =
            JournalJobContext::validated(&f.store, &f.host, &generation.id, generation.job)
                .unwrap();
        let error = subjektiv::journal::service::execute_snapshot(
            &f.store,
            &context,
            &f.request.input_digest().unwrap(),
            JournalJobRequest::ReadState,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("trusted Host delivery"),
            "{error}"
        );
        assert!(
            f.store
                .query_journal_operation(&f.host, &f.id, "first-tool-after-acceptance")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.store.read_journal(&f.host, &f.id).unwrap().process_order,
            0
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dispatch_first_tool_waits_for_runtime_acceptance_then_progresses_without_ack_cycle() {
        let f = RuntimeFixture::new().await;
        let (entered, release) = acceptance_barrier(&f);
        let dispatch = dispatch(&f);
        let worker = entered.recv_timeout(DEADLINE).unwrap();
        let gate = subjektiv_journal::lifecycle_lock(
            &f.api,
            &f.request
                .grants
                .subjektiv_journal
                .as_ref()
                .unwrap()
                .subject_id,
        )
        .unwrap();
        assert!(matches!(
            gate.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let api = f.api.clone();
        let callback_worker = worker.clone();
        let op = operation(&f);
        let tool = spawn(move || {
            attempted_tx.send(()).unwrap();
            subjektiv_journal::execute(
                &api,
                &callback_worker,
                JournalJobRequest::Execute { operation: op },
            )
        });
        attempted_rx.recv_timeout(DEADLINE).unwrap();
        let (finish_tx, finish_rx) = mpsc::channel();
        let api = f.api.clone();
        let callback_worker = worker.clone();
        let early_finish = spawn(move || {
            finish_tx.send(()).unwrap();
            subjektiv_journal::finish(&api, &callback_worker)
        });
        finish_rx.recv_timeout(DEADLINE).unwrap();
        assert!(
            matches!(
                early_finish.1.recv_timeout(HELD_WINDOW),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "finish must wait for the same acceptance gate, not mutate before attestation"
        );
        assert!(
            matches!(
                tool.1.recv_timeout(HELD_WINDOW),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "first Tool must not bypass dispatch's held Subject gate"
        );
        assert_no_preaccept_attestation(&f);
        release.send(()).unwrap();
        // Dispatch must independently finish after acceptance, even with the Tool waiting.

        let dispatched = joined(dispatch).unwrap();

        let unfinished = joined(early_finish).unwrap_err().to_string();
        assert!(
            !unfinished.contains("trusted Host delivery"),
            "acceptance attestation now exists"
        );

        let JournalJobResponse::Saved(record) = joined(tool).unwrap() else {
            panic!("record receipt")
        };
        assert_eq!(dispatched.attempt.worker.as_ref(), Some(&worker));
        assert_eq!(
            f.store
                .query_journal_operation(&f.host, &f.id, "first-tool-after-acceptance")
                .unwrap(),
            Some(record.clone())
        );
        assert_eq!(
            f.store.read_journal(&f.host, &f.id).unwrap().process_order,
            0,
            "acceptance and mutation alone are not a checkpoint"
        );

        let position = save_fixture_position(&f.api, &worker, record.state_token);
        subjektiv_journal::execute(
            &f.api,
            &worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: "checkpoint-after-barrier".into(),
                    expected_state: position,
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec!["first-tool-after-acceptance".into()],
                    },
                    covered_ranges: vec![f.range.clone()],
                },
            },
        )
        .unwrap();
        // Recovery may settle the checkpoint before the Worker's finish callback.
        // Do not require a new live write grant after that exact outcome is saved.
        let saved = observe_journal_completion(&f).await;
        assert_eq!(saved.state, BackendJobState::Completed);
        assert_eq!(saved.current_attempt, dispatched.attempt.attempt);
        let attempt = current_journal_attempt(&f, &dispatched.attempt.attempt_id);
        assert_eq!(attempt.state, BackendJobAttemptState::Completed);
        assert_eq!(attempt.worker.as_ref(), Some(&worker));
        assert_eq!(attempt.runtime_run_id, dispatched.attempt.runtime_run_id);
        assert_eq!(attempt.result, saved.result);
        let completion: JournalJobCompletion =
            serde_json::from_value(saved.result.clone().unwrap()).unwrap();
        assert_eq!(completion.process_order, 1);
        assert_eq!(completion.process_byte, 0);
        let context = JournalJobContext::validated_recovery(
            &f.store,
            &f.host,
            &completion.generation_id,
            subjektiv::api::JobAttemptBinding {
                job_id: saved.request.job_id.clone(),
                attempt_id: attempt.attempt_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            journal::service::finish(&f.store, &context).unwrap(),
            completion
        );
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        f.assert_purged(&worker);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn erase_and_tool_race_dispatch_acceptance_without_inversion_and_require_cleanup_ack() {
        let f = RuntimeFixture::new().await;
        f.stop_source();
        let (entered, release) = acceptance_barrier(&f);
        let dispatch = dispatch(&f);
        let worker = entered.recv_timeout(DEADLINE).unwrap();
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                &worker,
                "pinned",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        let gate = subjektiv_journal::lifecycle_lock(
            &f.api,
            &f.request
                .grants
                .subjektiv_journal
                .as_ref()
                .unwrap()
                .subject_id,
        )
        .unwrap();
        assert!(matches!(
            gate.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let api = f.api.clone();
        let session = f.session.to_string();
        let key = "erase-racing-dispatch-acceptance";
        let erase_attempted = attempted_tx.clone();
        let erasure = spawn(move || {
            erase_attempted.send(()).unwrap();
            api.erase_subjektiv_journal_session(&session, key)
        });
        let api = f.api.clone();
        let callback_worker = worker.clone();
        let op = operation(&f);
        let tool = spawn(move || {
            attempted_tx.send(()).unwrap();
            subjektiv_journal::execute(
                &api,
                &callback_worker,
                JournalJobRequest::Execute { operation: op },
            )
        });
        attempted_rx.recv_timeout(DEADLINE).unwrap();
        attempted_rx.recv_timeout(DEADLINE).unwrap();
        assert!(matches!(
            erasure.1.recv_timeout(HELD_WINDOW),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            tool.1.recv_timeout(HELD_WINDOW),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(
            !f.store.journal_session_erased(&f.host).unwrap(),
            "erase must wait for the same dispatch gate"
        );
        assert_no_preaccept_attestation(&f);
        release.send(()).unwrap();
        let dispatched = joined(dispatch).unwrap();
        let first = joined(erasure).unwrap_err();
        assert!(matches!(first, Error::RepositoryConflict(message)
            if message == "Journal delivered-copy Purge remains pending; retry the same erasure after cleanup"));
        // Mutex waiter order is deliberately unspecified. A Tool either saves before
        // the tombstone or revalidates and is rejected; it cannot cross the fence.
        if let Ok(response) = joined(tool) {
            assert!(matches!(response, JournalJobResponse::Saved(_)));
        }
        assert!(f.store.journal_session_erased(&f.host).unwrap());
        let api = f.api.clone();
        let callback_worker = worker.clone();
        let op = JournalOperation {
            request_id: "post-tombstone-must-not-save".into(),
            expected_state: "irrelevant-after-erasure".into(),
            action: JournalAction::ConfirmUnchanged,
            covered_ranges: vec![f.range.clone()],
        };
        assert!(joined(spawn(move || {
            subjektiv_journal::execute(&api, &callback_worker, JournalJobRequest::ReadState)
                .is_err()
                && subjektiv_journal::execute(
                    &api,
                    &callback_worker,
                    JournalJobRequest::Execute { operation: op },
                )
                .is_err()
                && subjektiv_journal::finish(&api, &callback_worker).is_err()
        })));
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Failed,
        )
        .await;
        assert!(session_root(f.dir.path(), &worker).exists());
        assert!(f.erase_pending_or_acknowledged(key));
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                &worker,
                "normal",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        f.api.recover_backend_jobs().unwrap();
        wait_for_backend_job_worker_cleanup(
            &f.api,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobWorkerCleanupState::Completed,
        )
        .await;
        f.assert_erasure_acknowledged(key);
        f.assert_purged(&worker);
    }
}

mod journal_migration {
    use super::lifecycle_barrier::{joined, spawn};
    use super::*;
    use subjektiv::journal::recording::JournalInput;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reserved_legacy_descriptor_fails_without_reselection_and_preserves_partial_checkpoint()
    {
        let f = SubjectRemovalFixture::new().await;
        let capture = subjektiv_journal::capture_input(&f.api, &f.attr, 0, 0, 48 * 1024).unwrap();
        let ranges = subjektiv::journal::recording::capture_ranges(&capture).unwrap();
        assert_eq!(ranges.len(), 2);
        let id = f
            .store
            .accept_journal_sources(&f.host, "legacy admitted originals", &ranges)
            .unwrap();
        let input = JournalInput::build(&f.store, &f.host, &id, |order, offset, budget| {
            subjektiv_journal::capture_input(&f.api, &f.attr, order, offset, budget)
                .map_err(|e| JournalError::Conflict(e.to_string()))
        })
        .unwrap();
        let mut legacy =
            subjektiv_journal::recording_request(&f.host, f.attr.clone(), &id, input).unwrap();
        legacy.job_id = "reserved-descriptor-only-legacy".into();
        legacy.input = json!({"journal_id":id, "selected_ranges":ranges});
        legacy
            .grants
            .subjektiv_journal
            .as_mut()
            .unwrap()
            .binding_id
            .clear();
        let reserved = f
            .api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &legacy, &Utc::now().to_rfc3339())
            .unwrap();
        assert_eq!(reserved.attempt.state, BackendJobAttemptState::Reserved);
        assert_eq!(reserved.job.request, legacy);
        let digest = legacy.input_digest().unwrap();
        let fingerprint = legacy.fingerprint().unwrap();
        // A previous generation saved the first prefix before its acknowledgement
        // was lost. Seed that real domain checkpoint, not a model success claim.
        let binding = subjektiv::api::JobAttemptBinding {
            job_id: "previous-checkpointed-generation".into(),
            attempt_id: "previous-attempt".into(),
        };
        let generation = f
            .store
            .prepare_journal_generation(&f.host, &id, binding.clone(), vec![ranges[0].clone()])
            .unwrap();
        let ctx = JournalJobContext::validated(&f.store, &f.host, &generation.id, binding.clone())
            .unwrap();
        let record = f
            .store
            .execute_journal(
                &ctx,
                JournalOperation {
                    request_id: "legacy-saved-record".into(),
                    expected_state: generation.initial_journal.state_token.clone(),
                    action: JournalAction::Create {
                        title: "Saved before migration".into(),
                        body: "First original reflected; second still pending".into(),
                        ongoing: "Continue second original only".into(),
                        state: JournalEntryState::Open,
                        source_end: None,
                        correction_refs: vec![],
                    },
                    covered_ranges: vec![ranges[0].clone()],
                },
            )
            .unwrap();
        let position = f
            .store
            .execute_journal(
                &ctx,
                JournalOperation {
                    request_id: "legacy-saved-position".into(),
                    expected_state: record.state_token.clone(),
                    action: JournalAction::UpdatePosition {
                        current_position: "First original saved; remaining work unconfirmed".into(),
                    },
                    covered_ranges: vec![],
                },
            )
            .unwrap();
        let checkpoint = f
            .store
            .checkpoint_journal(
                &ctx,
                JournalOperation {
                    request_id: "legacy-saved-checkpoint".into(),
                    expected_state: position.state_token.clone(),
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec!["legacy-saved-record".into()],
                    },
                    covered_ranges: vec![ranges[0].clone()],
                },
            )
            .unwrap();
        f.store
            .close_journal_generation(&f.host, &generation.id, &binding)
            .unwrap();
        let before = f.store.read_journal(&f.host, &id).unwrap();
        assert_eq!(before.process_order, 1);
        assert_eq!(before.process_byte, 0);
        let sources = f
            .store
            .list_journal_sources(&f.host, &id, None, 32)
            .unwrap();
        let entries = f
            .store
            .list_journal_entries(&f.host, &id, None, 16)
            .unwrap();
        assert_eq!(entries.items.len(), 1);
        assert!(f.store.journal_retention_required(&f.host).unwrap());
        let registry = f
            .api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap();
        // This is the descriptor-only admission rejection boundary, NOT schema
        // migration. Make the live source unavailable so dispatch/recovery cannot
        // silently materialize a new body-bearing Job from the original Session.
        let root = session_root(f.dir.path(), &f.target);
        let unavailable = f.dir.path().join("legacy-source-unavailable");
        std::fs::rename(&root, &unavailable).unwrap();
        assert!(subjektiv_journal::capture_input(&f.api, &f.attr, 0, 0, 48 * 1024).is_err());
        f.execution.take_inputs();
        let original = reserved.clone();
        let api = f.api.clone();
        let rejected = joined(spawn(move || {
            api.dispatch_backend_job_reservation(reserved)
        }))
        .unwrap();
        let old = f
            .api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &legacy.job_id)
            .unwrap()
            .unwrap();
        let old_attempt = f
            .api
            .store
            .get_backend_job_attempt(
                TEST_WORKSPACE_ID,
                &legacy.job_id,
                &original.attempt.attempt_id,
            )
            .unwrap()
            .unwrap();
        assert_eq!(rejected.job, old);
        assert_eq!(rejected.attempt, old_attempt);
        assert_eq!(old.state, BackendJobState::Failed);
        assert_eq!(
            old.failure_category.as_deref(),
            Some("journal_descriptor_invalidated")
        );
        assert_eq!(
            old.input_disposition,
            crate::backend_job::JobInputDisposition::Available
        );
        assert_eq!(old.request, legacy);
        assert_eq!(old.load_executable_request().unwrap(), &legacy);
        assert_eq!(old.input_digest, digest);
        assert_eq!(old.intent_fingerprint, fingerprint);
        assert_eq!(old.current_attempt, 1);
        assert!(old.result.is_none());
        assert!(
            !f.api
                .store
                .backend_job_was_migrated(TEST_WORKSPACE_ID, &legacy.job_id)
                .unwrap(),
            "descriptor rejection must not impersonate schema migration"
        );
        assert_eq!(old_attempt.state, BackendJobAttemptState::Failed);
        assert_eq!(old_attempt.attempt_id, original.attempt.attempt_id);
        assert_eq!(old_attempt.attempt, 1);
        assert_eq!(old_attempt.input_digest, digest);
        assert_eq!(old_attempt.failure_category, old.failure_category);
        assert_eq!(old_attempt.failure_detail, old.failure_detail);
        assert!(old_attempt.worker.is_none());
        assert!(old_attempt.runtime_run_id.is_none());
        assert!(old_attempt.result.is_none());

        subjektiv_journal::reconcile_attempt(&f.api, &old, &old_attempt).unwrap();
        subjektiv_journal::recover_attempts(&f.api).unwrap();
        f.api.recover_backend_jobs().unwrap();
        let api = f.api.clone();
        let replay = joined(spawn(move || {
            api.dispatch_backend_job_reservation(original)
        }))
        .unwrap();
        assert!(
            replay.replayed,
            "terminal rejection is observation, not execution"
        );
        assert_eq!(replay.job, old);
        assert_eq!(replay.attempt, old_attempt);
        assert_eq!(
            f.api
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &legacy.job_id)
                .unwrap(),
            Some(old.clone())
        );
        assert_eq!(
            f.api
                .store
                .get_backend_job_attempt(TEST_WORKSPACE_ID, &legacy.job_id, &old_attempt.attempt_id)
                .unwrap(),
            Some(old_attempt)
        );
        assert_eq!(
            f.api
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
                .unwrap(),
            vec![old],
            "descriptor failure must not automatically select a new Job"
        );
        assert!(
            f.api
                .store
                .get_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &legacy.job_id,
                    &crate::backend_job::attempt_id(&legacy.job_id, 2),
                )
                .unwrap()
                .is_none(),
            "no automatic continuation attempt"
        );
        assert!(
            f.execution.take_inputs().is_empty(),
            "neither the old nor a new Job may submit"
        );
        assert!(
            !root.exists(),
            "dispatch must not recreate the unavailable source"
        );
        assert_eq!(f.store.read_journal(&f.host, &id).unwrap(), before);
        assert_eq!(
            f.store
                .list_journal_sources(&f.host, &id, None, 32)
                .unwrap(),
            sources
        );
        assert_eq!(
            f.store
                .list_journal_entries(&f.host, &id, None, 16)
                .unwrap(),
            entries
        );
        for (request_id, receipt) in [
            ("legacy-saved-record", record),
            ("legacy-saved-position", position),
            ("legacy-saved-checkpoint", checkpoint),
        ] {
            assert_eq!(
                f.store
                    .query_journal_operation(&f.host, &id, request_id)
                    .unwrap(),
                Some(receipt)
            );
        }
        assert_eq!(
            f.store
                .retained_journal_generation(&f.host, &id, &binding)
                .unwrap(),
            Some(generation)
        );
        assert!(
            f.store
                .active_journal_generation(&f.host, &id)
                .unwrap()
                .is_none()
        );
        assert!(
            f.store.journal_retention_required(&f.host).unwrap(),
            "Failed is not reflection of the second original"
        );
        assert_eq!(
            f.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
                .unwrap(),
            registry
        );
        assert!(!f.store.journal_session_erased(&f.host).unwrap());
        assert!(
            !f.store
                .journal_worker_was_released(
                    &f.attr.subject_id,
                    &f.target.runtime_id,
                    &f.target.worker_id,
                )
                .unwrap()
        );
        std::fs::rename(&unavailable, &root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tampered_or_missing_body_snapshot_is_not_legacy_and_never_falls_back_to_available_source()
     {
        for missing in [false, true] {
            let f = RuntimeFixture::new().await;
            f.api
                .store
                .finish_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &f.request.job_id,
                    &f.reserved.attempt.attempt_id,
                    BackendJobAttemptState::Failed,
                    "predispatch_fixture",
                    "release resource before independently saved malformed request",
                    &Utc::now().to_rfc3339(),
                )
                .unwrap();
            let mut malformed = f.request.clone();
            malformed.job_id = format!("malformed-body-snapshot-{missing}");
            if missing {
                malformed.input.as_object_mut().unwrap().remove("capture");
            } else {
                malformed.input["capture"]["entries"][0]["body"]["text"] =
                    json!("tampered body, same canonical descriptor");
            }
            assert!(JournalInput::decode(&malformed.input).is_err());
            let bad = f
                .api
                .store
                .reserve_backend_job(TEST_WORKSPACE_ID, &malformed, &Utc::now().to_rfc3339())
                .unwrap();
            assert_eq!(bad.job.request.input, malformed.input);
            assert!(
                bad.job.load_executable_request().is_ok(),
                "generic immutable digest is valid; semantic body integrity is not"
            );
            f.execution.take_inputs();
            let api = f.api.clone();
            let candidate = bad.clone();
            assert!(
                !joined(spawn(move || subjektiv_journal::migrate_reserved_legacy(
                    &api, &candidate
                )))
                .unwrap()
            );
            let api = f.api.clone();
            assert!(joined(spawn(move || api.dispatch_backend_job_reservation(bad))).is_err());
            assert!(f.execution.take_inputs().is_empty());
            assert!(
                session_root(f.dir.path(), &f.source).exists(),
                "original source is available but must not repair saved input"
            );
            let jobs = f
                .api
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.session.to_string(), None, 100)
                .unwrap();
            assert_eq!(
                jobs.len(),
                2,
                "malformed body must not create a replacement/migration Job"
            );
            let saved = jobs
                .iter()
                .find(|job| job.request.job_id == malformed.job_id)
                .unwrap();
            assert_eq!(
                saved.input_disposition,
                crate::backend_job::JobInputDisposition::Available
            );
            assert_eq!(saved.input_digest, malformed.input_digest().unwrap());
            assert_eq!(saved.request.input, malformed.input);
            assert!(
                f.store
                    .active_journal_generation(&f.host, &f.id)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                f.store.read_journal(&f.host, &f.id).unwrap().process_order,
                0
            );
        }
    }
}
