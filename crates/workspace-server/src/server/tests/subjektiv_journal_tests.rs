//! Backend/Job/Subjektiv integration with real SQLite. RuntimeFixture also connects
//! the embedded Runtime, canonical source reader, pin/archive/Purge and real Session files.
//! Controller execution is deterministic; first-generation fixtures supply public bytes.
//! Tool-result persistence is a fixture boundary; no signed remote HTTP or model evaluation.
use super::*;
use crate::backend_job::{BackendJobGrants, SubjektivJournalGrant, journal_serialization_key};
use sha2::{Digest, Sha256};
use subjektiv::journal::transport::*;
use subjektiv::journal::*;

#[path = "subjektiv_journal_connected_tests.rs"]
mod connected;

fn bind_fixture_session(
    store: &crate::subjektiv::SubjektivStore,
    attr: crate::subjektiv::SubjectSessionAttribution,
) -> JournalHostContext {
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
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("journal-test").unwrap())
            .unwrap();
        let source = spawn_ticket_check_source(&api, "journal-source");
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
            input: serde_json::json!({"selected_ranges":[range]}),
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
            source_worker: None,
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
impl RuntimeFixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let source = spawn_ticket_check_source(&api, "original-source");
        let session = session_store::new_session_id();
        seed_session(
            &session_root(dir.path(), &source),
            session,
            message(SOURCE_SENTINEL),
            session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
        );
        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("source-reader-test").unwrap())
            .unwrap();
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
        assert_eq!(page.descriptors.len(), 1);
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
        let request = subjektiv_journal::recording_request(
            &host,
            attr,
            &id,
            vec![subjektiv::journal::recording::JournalMaterial {
                range: range.clone(),
                text: SOURCE_SENTINEL.into(),
            }],
        )
        .unwrap();
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains(SOURCE_SENTINEL)
        );
        let reserved = api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
            .unwrap();
        Self {
            dir,
            api,
            execution,
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
                    reason: Some("settle original source before permanent erase".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
    }
    fn dispatch_and_read(&self) -> BackendJobReservation {
        let dispatched = self
            .api
            .dispatch_backend_job_reservation(self.reserved.clone())
            .unwrap();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        let response = subjektiv_journal::execute(
            &self.api,
            worker,
            JournalJobRequest::ReadText {
                read: JournalTextRead {
                    target: JournalTextTarget::Source {
                        pointer: self.range.source.pointer.clone(),
                    },
                    offset: 0,
                    limit: 16384,
                    content_token: None,
                },
            },
        )
        .unwrap();
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains(SOURCE_SENTINEL));
        seed_session(
            &session_root(self.dir.path(), worker),
            session_store::new_session_id(),
            session_store::LoggedItem::ToolResult {
                call_id: "read-original".into(),
                summary: "ReadJournalText".into(),
                content: Some(encoded),
                attachments: vec![],
                disposition: agen::tool::ToolResultDisposition::Success,
                is_error: false,
            },
            session_store::LoggedSessionHistoryOrigin::ToolOutput {
                worker: session_store::LoggedWorkerSubject {
                    workspace_id: Some(TEST_WORKSPACE_ID.into()),
                    runtime_id: Some(worker.runtime_id.clone()),
                    worker_id: worker.worker_id.clone(),
                },
            },
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
        let dispatched = f.dispatch_and_read();
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
                f.api
                    .erase_subjektiv_journal_session(
                        &f.session.to_string(),
                        "erase-original-session",
                    )
                    .unwrap();
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
            assert!(f.store.journal_session_erased(&f.host).unwrap());
            assert!(
                f.api
                    .dispatch_backend_job_reservation(f.reserved.clone())
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn unknown_saved_completion_reconciles_same_binding_without_redispatch_after_lost_result() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_and_read();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    f.checkpoint(worker);
    let saved = subjektiv_journal::finish(&f.api, worker).unwrap(); // response/result delivery is lost
    f.execution.take_inputs();
    let unknown = f
        .api
        .store
        .finish_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobAttemptState::Unknown,
            "result_lost",
            "no result acknowledgement",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    subjektiv_journal::reconcile_unknown(&f.api, &dispatched.job, &unknown).unwrap();
    let cleaned = wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert_eq!(cleaned.state, BackendJobAttemptState::Completed);
    assert_eq!(cleaned.result, Some(serde_json::to_value(saved).unwrap()));
    assert_eq!(cleaned.worker, dispatched.attempt.worker);
    assert!(f.execution.take_inputs().is_empty());
    subjektiv_journal::reconcile_unknown(&f.api, &dispatched.job, &cleaned).unwrap();
    f.assert_purged(worker);
}

#[tokio::test]
async fn unknown_partial_waits_for_durable_purge_fence_and_retains_sources_when_cleanup_fails() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_and_read();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let g = f
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
                expected_state: g.initial_journal.state_token,
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
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "pinned",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let unknown = f
        .api
        .finish_backend_job_attempt(
            &f.request.job_id,
            &dispatched.attempt.attempt_id,
            BackendJobAttemptState::Unknown,
            "response_lost",
            "dispatch outcome unknown",
        )
        .unwrap();
    let failed = wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Failed,
    )
    .await;
    assert!(
        f.api
            .store
            .reconcile_backend_journal_attempt(
                TEST_WORKSPACE_ID,
                &failed,
                None,
                &Utc::now().to_rfc3339()
            )
            .is_err()
    );
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(session_root(f.dir.path(), worker).exists());
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Unknown
    );
    assert!(subjektiv_journal::execute(&f.api, worker, JournalJobRequest::ReadState).is_err());
    assert!(
        f.api
            .store
            .reconcile_backend_journal_attempt(
                TEST_WORKSPACE_ID,
                &unknown,
                None,
                &Utc::now().to_rfc3339()
            )
            .is_err(),
        "a stale cleanup snapshot must not overwrite newer evidence"
    );
    subjektiv_journal::reconcile_unknown(&f.api, &dispatched.job, &unknown).unwrap();
    f.api
        .store
        .update_worker_retention(
            TEST_WORKSPACE_ID,
            worker,
            "normal",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    f.api.recover_backend_job_worker_cleanups().unwrap();
    wait_for_backend_job_worker_cleanup(
        &f.api,
        &f.request.job_id,
        &dispatched.attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    // Cleanup commits before domain reconciliation; wait for its authoritative outcome.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job = f
                .api
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
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
    f.assert_purged(worker);
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert_eq!(
        f.store
            .query_journal_operation(&f.host, &f.id, "partial-record")
            .unwrap()
            .unwrap(),
        recorded
    );
    assert_eq!(
        worker_runtime::retention::runtime_session_retention(
            &f.dir.path().join(".test-embedded-runtime-store")
        )
        .pins(f.session)
        .unwrap()
        .len(),
        1,
        "source pin survives failed/partial Job cleanup"
    );
}

#[tokio::test]
async fn journal_reserved_restart_dispatches_exact_intent_once_and_unbound_unknown_is_not_replaced()
{
    let f = RuntimeFixture::new().await;
    f.execution.take_inputs();
    f.api.recover_backend_jobs().unwrap();
    let first = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &f.reserved.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(first.state, BackendJobAttemptState::Dispatched);
    assert_eq!(f.execution.take_inputs().len(), 1);
    f.api.recover_backend_jobs().unwrap();
    assert!(f.execution.take_inputs().is_empty());
    assert_eq!(
        f.api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &f.request.job_id)
            .unwrap()
            .unwrap()
            .request,
        f.request
    );

    let unknown_fixture = RuntimeFixture::new().await;
    let unknown = unknown_fixture
        .api
        .store
        .finish_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &unknown_fixture.request.job_id,
            &unknown_fixture.reserved.attempt.attempt_id,
            BackendJobAttemptState::Unknown,
            "allocation_unknown",
            "allocation outcome has no bound Worker",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    assert_eq!(
        unknown.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Completed)
    );
    assert!(
        subjektiv_journal::reconcile_unknown(
            &unknown_fixture.api,
            &unknown_fixture.reserved.job,
            &unknown
        )
        .is_err()
    );
    assert_eq!(
        unknown_fixture
            .api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &unknown_fixture.request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Unknown
    );
    assert!(
        unknown_fixture
            .store
            .active_journal_generation(&unknown_fixture.host, &unknown_fixture.id)
            .unwrap()
            .is_some(),
        "the generation is not closed before an actual lifecycle fence"
    );
    assert!(unknown_fixture.execution.take_inputs().is_empty());
}

#[tokio::test]
async fn archived_original_session_remains_readable_then_releases_its_pin_after_saved_completion() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_and_read();
    let worker = dispatched.attempt.worker.as_ref().unwrap();
    let root = f.dir.path().join(".test-embedded-runtime-store");
    let retention = worker_runtime::retention::runtime_session_retention(&root);
    assert_eq!(retention.pins(f.session).unwrap().len(), 1);
    f.api
        .runtime
        .stop_worker(
            &f.source,
            WorkerLifecycleRequest {
                reason: Some("archive original".into()),
                ticket_assignment: None,
            },
        )
        .unwrap();
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
    let archive = f
        .api
        .config_store
        .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.session.to_string())
        .unwrap()
        .expect("durable archive binding");
    assert!(!session_root(f.dir.path(), &f.source).exists());
    // Same frozen descriptor; material_source must now resolve through the durable
    // archive mapping, not attempt a stale retained Worker path.
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
    .unwrap();
    assert!(
        serde_json::to_string(&reread)
            .unwrap()
            .contains(SOURCE_SENTINEL)
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
        retention.pins(f.session).unwrap().is_empty(),
        "archive pin is released only after saved reflection"
    );
    assert!(!session_root(f.dir.path(), worker).exists());
    assert_eq!(
        f.api
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.session.to_string())
            .unwrap()
            .unwrap()
            .archive_id,
        archive.archive_id
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
async fn saved_checkpoint_unknown_auto_resolves_without_waiting_for_failed_session_cleanup() {
    for recovery in ["transition", "restart"] {
        let f = RuntimeFixture::new().await;
        let dispatched = f.dispatch_and_read();
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        f.checkpoint(worker); // Deliberately do not call finish: checkpoint receipt is durable.
        f.execution.take_inputs();
        f.api
            .store
            .update_worker_retention(
                TEST_WORKSPACE_ID,
                worker,
                "pinned",
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
        if recovery == "transition" {
            let result = f
                .api
                .finish_backend_job_attempt(
                    &f.request.job_id,
                    &dispatched.attempt.attempt_id,
                    BackendJobAttemptState::Unknown,
                    "result_lost",
                    "result acknowledgement unavailable",
                )
                .unwrap();
            assert_eq!(result.state, BackendJobAttemptState::Completed);
        } else {
            f.api
                .store
                .finish_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &f.request.job_id,
                    &dispatched.attempt.attempt_id,
                    BackendJobAttemptState::Unknown,
                    "result_lost",
                    "Backend stopped before outcome reconciliation",
                    &Utc::now().to_rfc3339(),
                )
                .unwrap();
            // The generic Job recovery entry must not require a later source notification.
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
        assert!(
            session_root(f.dir.path(), worker).exists(),
            "cleanup failure is not hidden by outcome resolution"
        );
        assert!(
            f.execution.take_inputs().is_empty(),
            "saved checkpoint must not cause model rerun"
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
        f.api.recover_backend_jobs().unwrap();
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
async fn stopped_partial_journal_recovery_fences_old_attempt_before_dispatching_only_remaining_work()
 {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_and_read();
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
    assert_eq!(old.state, BackendJobState::Failed);
    assert_eq!(
        old.failure_category.as_deref(),
        Some("journal_attempt_fenced")
    );
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
    assert_ne!(next.job, g.job);
    assert_eq!(next.initial_open.unwrap().title, "unfinished");
    let new_inputs = f.execution.take_inputs();
    assert_eq!(new_inputs.len(), 1);
    assert!(!new_inputs[0].1.contains(SOURCE_SENTINEL));
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
        let dispatched = f.dispatch_and_read();
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
            f.api
                .erase_subjektiv_journal_session(
                    &f.session.to_string(),
                    "erase-with-blocked-cleanup",
                )
                .unwrap();
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
            assert!(f.store.journal_session_erased(&f.host).unwrap());
            assert!(
                f.api
                    .dispatch_backend_job_reservation(f.reserved.clone())
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn explicit_erasure_interrupts_completed_journal_cleanup_waiting_for_run_teardown() {
    let f = RuntimeFixture::new().await;
    let dispatched = f.dispatch_and_read();
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
    f.api
        .erase_subjektiv_journal_session(&f.session.to_string(), "erase-while-completed-run-busy")
        .unwrap();
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
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            Arc::new(DeterministicExecutionBackend::default()),
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
async fn disabled_subject_removal_does_not_admit_new_public_history_or_reserve_a_job() {
    let f = SubjectRemovalFixture::new().await;
    f.disable();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
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
}

#[tokio::test]
async fn disabled_subject_removal_retains_only_the_already_admitted_prefix() {
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
    f.disable();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_some()
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
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(session_root(f.dir.path(), &f.target).exists());
}

#[tokio::test]
async fn attributed_unmaterialized_subject_still_requires_actual_idle_release() {
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
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    assert_eq!(retention.pins(session).unwrap().len(), 1);
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.target)
            .unwrap()
            .is_some()
    );
    assert!(f.store.journal_for_session(&f.host).unwrap().is_none());
}

#[tokio::test]
async fn missing_materialized_subject_source_blocks_removal_without_marking_it_empty() {
    let f = SubjectRemovalFixture::new().await;
    std::fs::remove_dir_all(session_root(f.dir.path(), &f.target)).unwrap();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    assert!(response.body.contains("journal_handoff_incomplete"));
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
    let dispatched = f.dispatch_and_read();
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
    f.api
        .erase_subjektiv_journal_session(&f.session.to_string(), "erase-unsettled-source")
        .unwrap();
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
            dispatcher,
            controller,
            target,
            attr,
            host,
            store,
        } = self;
        let config = api.config.clone();
        drop(api);
        drop(dispatcher);
        drop(store);
        let control = Arc::new(test_control_store(&config));
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            Arc::new(DeterministicExecutionBackend::default()),
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
async fn legacy_journal_grant_preserves_old_json_and_only_recovers_original_current_body() {
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
    assert_eq!(
        serde_json::to_string(&serde_json::to_value(&grant).unwrap()).unwrap(),
        wire
    );
    assert!(subjektiv_journal::host(&f.api, &f.store, &grant).is_ok());
    let writer = JournalWriterBinding {
        subject_id: f.attr.subject_id.clone(),
        runtime_id: f.attr.runtime_id.clone(),
        worker_id: f.attr.worker_id.clone(),
        binding_id: f.host.writer_binding_id().unwrap().into(),
    };
    f.store.release_journal_writer(&writer).unwrap();
    f.store
        .claim_journal_writer(&f.attr.subject_id, &f.attr.runtime_id, "different-body")
        .unwrap();
    assert!(
        subjektiv_journal::host(&f.api, &f.store, &grant).is_err(),
        "legacy grant must never map to a later writer"
    );
}
