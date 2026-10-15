//! Backend/Job/Subjektiv integration with real SQLite. RuntimeFixture also connects
//! the embedded Runtime, canonical source reader, pin/archive/Purge and real Session files.
//! Controller execution is deterministic; first-generation fixtures supply public bytes.
//! Tool-result persistence is a fixture boundary; no signed remote HTTP or model evaluation.
use super::*;
use crate::backend_job::{BackendJobGrants, SubjektivJournalGrant, journal_serialization_key};
use sha2::{Digest, Sha256};
use subjektiv::journal::transport::*;
use subjektiv::journal::*;

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
        let host = JournalHostContext::validated_session(&store, attr.clone()).unwrap();
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
    subjektiv_journal::execute(
        &f.api,
        worker,
        JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "checkpoint".into(),
                expected_state: receipt.state_token,
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
                history: vec![session_store::LoggedHistoryEntry {
                    item,
                    metadata: session_store::LoggedSessionHistoryMetadata {
                        entry_id: session_store::LoggedSessionHistoryEntryId(
                            "read-source-original".into(),
                        ),
                        origin,
                        derivation: None,
                    },
                }],
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
        let host = JournalHostContext::validated_session(&store, attr.clone()).unwrap();
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
                    pin_id: subjektiv_journal::source_pin_id(TEST_WORKSPACE_ID, &attr),
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
            attr,
            &id,
            vec![subjektiv::journal::recording::JournalMaterial {
                range: range.clone(),
                text: SOURCE_SENTINEL.into(),
            }],
        );
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
        subjektiv_journal::execute(
            &self.api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: "checkpoint".into(),
                    expected_state: receipt.state_token,
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
        assert!(
            session_root(self.dir.path(), &self.source).exists(),
            "the original conversation was not purged"
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
