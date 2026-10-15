//! Integration of standalone attribution, original FsStore material, shared Journal storage
//! and the local Job adapter. Only the outer model/execution lease is replaced in these cases.
//! The generic Job runner is exercised separately by the standalone Jobs integration suite.

use super::*;
use crate::store::{StaleLeasePolicy, StandaloneWorkerAllocation, StandaloneWorkerStore};
use session_store::{Store, WorkerMetadataStore};
use std::sync::atomic::AtomicBool;

struct Fixture {
    _temp: tempfile::TempDir,
    _allocation: Option<StandaloneWorkerAllocation>,
    connection: SubjectConnection,
    sessions: session_store::FsStore,
    context: SubjektivHostContext,
    segment: session_store::SegmentId,
}
impl Fixture {
    fn new(text: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let catalog = StandaloneSubjects::open(&state).unwrap();
        let subject = catalog.create("journal-owner", None).unwrap();
        let workers = StandaloneWorkerStore::open_existing(&state).unwrap();
        let allocation = workers
            .allocate(temp.path(), StaleLeasePolicy::Reject)
            .unwrap();
        let worker = allocation.worker_id();
        let session = session_store::SessionId::now_v7();
        let segment = uuid::Uuid::now_v7();
        let manifest = manifest::WorkerManifest::from_toml("[worker]\nname=\"journal-test\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"test\"\n[engine]\n[scope]\nallow=[]\n").unwrap();
        let mut connection = catalog.connect(&subject.id, worker).unwrap();
        workers
            .commit_created_connected(
                &allocation,
                manifest,
                "journal-test".into(),
                session,
                Some(segment),
                Some(connection.binding.clone()),
            )
            .unwrap();
        let worker_store =
            session_store::FsWorkerStore::new(workers.worker_metadata_dir(worker)).unwrap();
        worker_store
            .write(&session_store::WorkerMetadata::new(
                "journal-test",
                Some(session_store::WorkerActiveSegmentRef::active_segment(
                    session, segment,
                )),
            ))
            .unwrap();
        let sessions = session_store::FsStore::new_with_retention(
            workers.sessions_dir(worker),
            crate::store::session_retention(&state),
        )
        .unwrap();
        sessions
            .create_segment(
                session,
                segment,
                &[session_store::LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id: session,
                    system_prompt: Some("PRIVATE PROMPT".into()),
                    config: Default::default(),
                    history: vec![message("first", text)],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        let context = SubjektivHostContext {
            worker_id: format!("standalone-{worker}"),
            session_id: session.to_string(),
        };
        connection.host.record_session(&context, true).unwrap();
        let jobs =
            crate::jobs::StandaloneJobs::open(&connection.jobs_path, temp.path().into()).unwrap();
        connection.host.bind_jobs(jobs).unwrap();
        // Tests retain explicit shutdown ownership without launching model tasks.
        connection.armed = false;
        Self {
            _temp: temp,
            _allocation: Some(allocation),
            connection,
            sessions,
            context,
            segment,
        }
    }
    fn host(&self) -> &Arc<LocalSubjectHost> {
        &self.connection.host
    }
    fn append(&self, id: &str, text: &str) {
        self.sessions
            .append(
                self.context.session_id.parse().unwrap(),
                self.segment,
                &session_store::LogEntry::AnnotatedUserInput {
                    ts: 2,
                    segments: vec![],
                    extensions: vec![],
                    history: vec![message(id, text)],
                },
            )
            .unwrap();
    }
    fn stop_worker(&mut self) -> (protocol::WorkerId, StandaloneWorkerStore) {
        let worker = self._allocation.as_ref().unwrap().worker_id();
        let workers =
            StandaloneWorkerStore::open_existing(&self.host().catalog.state_root).unwrap();
        let record = workers.load(worker).unwrap();
        workers
            .mark_stopped(
                &record,
                record.active_session_id,
                record.active_segment_id,
                crate::store::StandaloneShutdownReason::UserExit,
            )
            .unwrap();
        self.connection.close().unwrap();
        self._allocation
            .take()
            .unwrap()
            .into_lease()
            .release()
            .unwrap();
        (worker, workers)
    }
    fn assert_job_storage_excludes(&self, snapshot: &JobSnapshot, sentinel: &str) {
        assert!(
            !serde_json::to_string(&snapshot.request)
                .unwrap()
                .contains(sentinel)
        );
        assert!(
            !serde_json::to_string(&snapshot.domain_grant)
                .unwrap()
                .contains(sentinel)
        );
        // Include SQLite/WAL/journal files: immutable request/grant storage must never hold
        // original text, even in freed pages left behind by explicit source erasure.
        for entry in fs::read_dir(self.connection.jobs_path.parent().unwrap()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                let bytes = fs::read(entry.path()).unwrap();
                assert!(
                    !bytes
                        .windows(sentinel.len())
                        .any(|window| window == sentinel.as_bytes()),
                    "original text persisted in {}",
                    entry.path().display()
                );
            }
        }
    }
    fn adapter(&self) -> (LocalJournalHost, Arc<TestFence>, JobSnapshot) {
        self.host().admit_journal(&self.context).unwrap();
        let (jobs, id) = self.host().next_journal_job().unwrap().unwrap();
        let snapshot = jobs.get(&id).unwrap();
        let grant = self.host().journal_grant(&snapshot).unwrap();
        let context = authority(self.host(), &grant.session_id).unwrap();
        let generation = self
            .host()
            .catalog
            .store
            .prepare_journal_generation(
                &context,
                &grant.journal_id,
                JobAttemptBinding {
                    job_id: id,
                    attempt_id: snapshot.attempt.attempt_id.clone(),
                },
                grant.ranges.clone(),
            )
            .unwrap();
        let fence = Arc::new(TestFence(AtomicBool::new(true)));
        let job_context = JournalJobContext::validated(
            &self.host().catalog.store,
            &context,
            &generation.id,
            generation.job.clone(),
        )
        .unwrap();
        let adapter = LocalJournalHost {
            host: self.host().clone(),
            grant,
            context,
            job_context,
            generation,
            fence: fence.clone(),
        };
        adapter.read_all_sources();
        (adapter, fence, snapshot)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.connection.close().unwrap();
    }
}
fn message(id: &str, text: &str) -> session_store::LoggedHistoryEntry {
    session_store::LoggedHistoryEntry {
        item: session_store::LoggedItem::Message {
            role: session_store::LoggedRole::User,
            content: vec![session_store::LoggedContentPart::Text { text: text.into() }],
        },
        metadata: session_store::LoggedSessionHistoryMetadata {
            entry_id: session_store::LoggedSessionHistoryEntryId(id.into()),
            origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
            derivation: None,
        },
    }
}
impl LocalJournalHost {
    fn read_all_sources(&self) {
        for range in &self.generation.selected_ranges {
            let mut offset = range.bytes.start as usize;
            let mut content_token = None;
            loop {
                let response = self
                    .execute(JournalJobRequest::ReadText {
                        read: JournalTextRead {
                            target: JournalTextTarget::Source {
                                pointer: range.source.pointer.clone(),
                            },
                            offset,
                            limit: MAX_JOURNAL_READ_BYTES,
                            content_token: content_token.clone(),
                        },
                    })
                    .unwrap();
                let JournalJobResponse::Text(text) = response else {
                    panic!()
                };
                match text.next_byte {
                    Some(next) => {
                        assert!(next > offset);
                        offset = next;
                        content_token = Some(text.content_token);
                    }
                    None => break,
                }
            }
        }
    }
    fn state(&self) -> Result<JournalJobState, SubjektivHostError> {
        match self.execute(JournalJobRequest::ReadState)? {
            JournalJobResponse::State(state) => Ok(state),
            _ => panic!("expected state"),
        }
    }
}

struct TestFence(AtomicBool);
impl crate::jobs::JobAttemptFence for TestFence {
    fn ensure_live(&self) -> Result<(), String> {
        if self.0.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("cancelled".into())
        }
    }
}
fn save(
    adapter: &LocalJournalHost,
    id: &str,
    expected_state: &str,
    action: JournalAction,
    covered_ranges: Vec<JournalSourceRange>,
) -> JournalReceipt {
    let JournalJobResponse::Saved(receipt) = adapter
        .execute(JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: id.into(),
                expected_state: expected_state.into(),
                action,
                covered_ranges,
            },
        })
        .unwrap()
    else {
        panic!("expected receipt")
    };
    receipt
}

#[test]
fn duplicate_admission_reserves_one_job_and_excludes_private_prompt() {
    let fixture = Fixture::new("short request");
    fixture.host().admit_journal(&fixture.context).unwrap();
    let (jobs, id) = fixture.host().next_journal_job().unwrap().unwrap();
    fixture.host().admit_journal(&fixture.context).unwrap();
    let (_, repeated) = fixture.host().next_journal_job().unwrap().unwrap();
    assert_eq!(id, repeated);
    assert_eq!(jobs.pending().unwrap().len(), 1);
    let input = jobs.get(&id).unwrap().request.input.to_string();
    assert!(
        !input.contains("short request"),
        "Job storage retains descriptors, never a second copy of original text"
    );
    assert!(!input.contains("PRIVATE PROMPT"));
    let foreign = SubjektivHostContext {
        worker_id: protocol::WorkerId::now_v7().to_string(),
        ..fixture.context.clone()
    };
    assert!(fixture.host().admit_journal(&foreign).is_err());
}

#[test]
fn sequential_records_survive_lost_response_and_checkpoint_does_not_fix_open() {
    let fixture = Fixture::new("proposed A, corrected to B; only part was performed; interrupted");
    let (adapter, _, _) = fixture.adapter();
    let initial = adapter.generation.initial_journal.state_token.clone();
    let ranges = adapter.generation.selected_ranges.clone();
    let operation = JournalOperation {
        request_id: "create".into(),
        expected_state: initial,
        action: JournalAction::Create {
            title: "Changed plan".into(),
            body: "Proposal corrected; partial action, outcome unconfirmed.".into(),
            ongoing: "Remaining work".into(),
            state: JournalEntryState::Open,
            source_end: None,
            correction_refs: vec![],
        },
        covered_ranges: ranges.clone(),
    };
    let first = adapter
        .execute(JournalJobRequest::Execute {
            operation: operation.clone(),
        })
        .unwrap();
    assert_eq!(
        first,
        adapter
            .execute(JournalJobRequest::Execute {
                operation: operation.clone()
            })
            .unwrap()
    );
    let JournalJobResponse::Saved(created) = first else {
        panic!()
    };
    assert!(
        adapter.finish().is_err(),
        "a saved entry does not imply processed input"
    );
    let position = save(
        &adapter,
        "position",
        &created.state_token,
        JournalAction::UpdatePosition {
            current_position: "B selected, unfinished".into(),
        },
        vec![],
    );
    let checkpoint = save(
        &adapter,
        "checkpoint",
        &position.state_token,
        JournalAction::Checkpoint {
            record_operation_ids: vec!["create".into()],
        },
        ranges,
    );
    let completed = adapter.finish().unwrap();
    assert_eq!(completed.state_token, checkpoint.state_token);
    assert_eq!(
        adapter.finish().unwrap(),
        completed,
        "lost completion response reuses the exact saved receipt"
    );
    let (journal, open) = fixture
        .host()
        .catalog
        .store
        .journal_resume_state(&adapter.context, &adapter.grant.journal_id)
        .unwrap();
    assert_eq!(journal.current_position, "B selected, unfinished");
    assert_eq!(open.unwrap().state, JournalEntryState::Open);
    let mut altered = operation;
    altered.expected_state = checkpoint.state_token;
    assert!(
        adapter
            .execute(JournalJobRequest::Execute { operation: altered })
            .is_err()
    );
}

#[test]
fn generation_input_stays_frozen_and_next_prefix_includes_last_input() {
    let fixture = Fixture::new("first turn");
    let (adapter, _, _) = fixture.adapter();
    fixture.append("last", "last input while generation runs");
    fixture.host().admit_journal(&fixture.context).unwrap();
    let state = adapter.state().unwrap();
    assert_eq!(state.selected_ranges.len(), 1);
    assert_eq!(state.selected_ranges[0].source.pointer.entry_ref, "Efirst");
    let nochange = save(
        &adapter,
        "checked",
        &state.current_journal.state_token,
        JournalAction::ConfirmUnchanged,
        state.selected_ranges.clone(),
    );
    save(
        &adapter,
        "checkpoint",
        &nochange.state_token,
        JournalAction::Checkpoint {
            record_operation_ids: vec!["checked".into()],
        },
        state.selected_ranges,
    );
    fixture
        .host()
        .catalog
        .store
        .close_journal_generation(
            &adapter.context,
            &adapter.generation.id,
            &adapter.generation.job,
        )
        .unwrap();
    let materials = fixture
        .host()
        .select_journal_materials(&adapter.context, &adapter.grant.journal_id)
        .unwrap();
    assert_eq!(materials[0].range.source.pointer.entry_ref, "Elast");
    assert!(
        materials[0]
            .text
            .contains("last input while generation runs")
    );
}

#[test]
fn revoked_attempt_and_foreign_source_are_rejected_without_advancing_cursor() {
    let fixture = Fixture::new("request");
    let (adapter, fence, _) = fixture.adapter();
    let mut pointer = adapter.generation.selected_ranges[0].source.pointer.clone();
    pointer.entry_ref = "Eforeign".into();
    assert!(
        adapter
            .execute(JournalJobRequest::ReadText {
                read: JournalTextRead {
                    target: JournalTextTarget::Source { pointer },
                    offset: 0,
                    limit: 100,
                    content_token: None
                }
            })
            .is_err()
    );
    fence.0.store(false, Ordering::Release);
    assert!(adapter.execute(JournalJobRequest::ReadState).is_err());
    assert!(adapter.finish().is_err());
    let journal = fixture
        .host()
        .catalog
        .store
        .read_journal(&adapter.context, &adapter.grant.journal_id)
        .unwrap();
    assert_eq!((journal.process_order, journal.process_byte), (0, 0));
}

#[test]
fn large_original_is_chunked_without_claiming_unread_suffix() {
    let original = "漢字🙂\n\"".repeat(50_000);
    let fixture = Fixture::new(&original);
    let (adapter, _, _) = fixture.adapter();
    let range = &adapter.grant.ranges[0];
    assert!(range.bytes.end < range.source.total_bytes);
    assert!(range.bytes.end - range.bytes.start < INPUT_BUDGET as u64);
    let snapshot = fixture
        .host()
        .jobs()
        .unwrap()
        .pending()
        .unwrap()
        .pop()
        .unwrap();
    assert!(serde_json::to_vec(&snapshot.request.input).unwrap().len() < 128 * 1024);
    assert!(!snapshot.request.input.to_string().contains("漢字"));
    assert!(adapter.finish().is_err());
}

#[derive(Clone, Default)]
struct JournalScript {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    gate_first: bool,
    pause_after_record: bool,
    record_saved: Arc<tokio::sync::Notify>,
}
impl JournalScript {
    fn response(
        &self,
        request: &agen::llm_client::types::Request,
    ) -> Vec<agen::llm_client::event::Event> {
        use agen::llm_client::{
            event::{Event, StopReason},
            types::Item,
        };
        use serde_json::{Value, json};
        let result = |id: &str| -> Option<Value> {
            request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    content,
                    is_error,
                    summary,
                    ..
                } if call_id == id => {
                    assert!(!is_error, "{id}: {summary}; {content:?}");
                    Some(
                        content
                            .as_deref()
                            .map(|content| serde_json::from_str(content).unwrap())
                            .unwrap_or(Value::Null),
                    )
                }
                _ => None,
            })
        };
        if result("submit").is_some() {
            return vec![
                Event::text_block_start(0),
                Event::text_delta(0, "saved"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ];
        }
        let (name, id, input) = if result("checkpoint").is_some() {
            ("SubmitJobResult", "submit", json!({"result":{}}))
        } else if let Some(position) = result("position") {
            let state = result("state").unwrap();
            (
                "CheckpointJournal",
                "checkpoint",
                json!({"request_id":format!("checkpoint:{}",state["value"]["generation_id"].as_str().unwrap()),
                "expected_state":position["value"]["state_token"],"action":{"action":"checkpoint","record_operation_ids":[format!("record:{}",state["value"]["generation_id"].as_str().unwrap())]},
                "covered_ranges":state["value"]["selected_ranges"]}),
            )
        } else if let Some(record) = result("record") {
            let state = result("state").unwrap();
            (
                "UpdateJournalPosition",
                "position",
                json!({"request_id":format!("position:{}",state["value"]["generation_id"].as_str().unwrap()),
                "expected_state":record["value"]["state_token"],"action":{"action":"update_position","current_position":"Changed direction; partial work remains; outcome unconfirmed"},"covered_ranges":[]}),
            )
        } else if let Some(state) = result("state") {
            let state = &state["value"];
            for (index, range) in state["selected_ranges"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let mut offset = range["bytes"]["start"].as_u64().unwrap();
                let mut token = Value::Null;
                loop {
                    let id = format!("source-{index}-{offset}");
                    if let Some(page) = result(&id) {
                        let page = &page["value"];
                        if let Some(next) = page["next_byte"].as_u64() {
                            assert!(next > offset);
                            offset = next;
                            token = page["content_token"].clone();
                        } else {
                            break;
                        }
                    } else {
                        let input = json!({"target":{"target":"source","pointer":range["source"]["pointer"]},"offset":offset,"limit":MAX_JOURNAL_READ_BYTES,"content_token":token});
                        return vec![
                            Event::tool_use_start(0, &id, "ReadJournalText"),
                            Event::tool_input_delta(0, input.to_string()),
                            Event::tool_use_stop(0),
                        ];
                    }
                }
            }
            let (name, action) = if state["current_open"].is_null() {
                (
                    "CreateJournalEntry",
                    json!({"action":"create","title":"Changed direction","body":"Proposed A; user corrected it to B. Only part performed; interrupted.","ongoing":"Remaining work and unknown result","state":"open","source_end":null,"correction_refs":[]}),
                )
            } else {
                (
                    "RewriteJournalEntry",
                    json!({"action":"rewrite","entry_id":state["current_open"]["id"],"title":"Changed direction","body":"B selected after correction; partial work and last input retained.","ongoing":"Remaining work"}),
                )
            };
            (
                name,
                "record",
                json!({"request_id":format!("record:{}",state["generation_id"].as_str().unwrap()),"expected_state":state["current_journal"]["state_token"],"action":action,"covered_ranges":state["selected_ranges"]}),
            )
        } else {
            ("ReadJournalState", "state", json!({}))
        };
        assert!(request.tools.iter().any(|tool| tool.name == name));
        for forbidden in [
            "Bash",
            "Read",
            "MemoryApplyCandidate",
            "SubjektivMemoryRemember",
            "SubjektivSessionRead",
        ] {
            assert!(
                !request.tools.iter().any(|tool| tool.name == forbidden),
                "Journal obtained {forbidden}"
            );
        }
        vec![
            Event::tool_use_start(0, id, name),
            Event::tool_input_delta(0, input.to_string()),
            Event::tool_use_stop(0),
        ]
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for JournalScript {
    async fn stream(
        &self,
        request: agen::llm_client::types::Request,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            agen::llm_client::event::Event,
                            agen::llm_client::error::ClientError,
                        >,
                    > + Send,
            >,
        >,
        agen::llm_client::error::ClientError,
    > {
        let first = !request.items.iter().any(|item| matches!(item, agen::llm_client::types::Item::ToolResult { call_id, .. } if call_id == "state"));
        if first {
            self.entered.notify_one();
            if self.gate_first {
                self.release.notified().await;
            }
        }
        if self.pause_after_record && request.items.iter().any(|item| matches!(item, agen::llm_client::types::Item::ToolResult { call_id, .. } if call_id == "record"))
            && !request.items.iter().any(|item| matches!(item, agen::llm_client::types::Item::ToolResult { call_id, .. } if call_id == "position")) {
            self.record_saved.notify_one();
            self.release.notified().await;
        }
        let events = self.response(&request);
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
}

#[tokio::test]
async fn real_job_tools_save_and_drain_last_input_without_another_turn() {
    let fixture = Fixture::new("Proposed A, corrected to B; partly performed; interrupted");
    let model = JournalScript {
        gate_first: true,
        ..Default::default()
    };
    let jobs = fixture.host().jobs().unwrap();
    jobs.bind_model_client(Box::new(model.clone()));
    fixture
        .host()
        .request_journal_recording(&fixture.context, JournalRecordingReason::RunCommitted)
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), model.entered.notified())
        .await
        .unwrap();
    let context = authority(fixture.host(), &fixture.context.session_id).unwrap();
    let journal = fixture
        .host()
        .catalog
        .store
        .list_journals(&context, None, 1)
        .unwrap()
        .items
        .pop()
        .unwrap();
    let first = fixture
        .host()
        .catalog
        .store
        .active_journal_generation(&context, &journal.id)
        .unwrap()
        .unwrap();
    fixture.append("last-input", "No additional user turn will follow");
    fixture
        .host()
        .request_journal_recording(&fixture.context, JournalRecordingReason::RunCommitted)
        .unwrap();
    model.release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        jobs.wait(&first.job.job_id),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), model.entered.notified())
        .await
        .unwrap();
    let second = fixture
        .host()
        .catalog
        .store
        .active_journal_generation(&context, &journal.id)
        .unwrap()
        .unwrap();
    assert_ne!(first.id, second.id);
    model.release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        jobs.wait(&second.job.job_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        fixture
            .host()
            .session_fully_reflected(&fixture.context.session_id)
            .unwrap()
    );
    let (_, open) = fixture
        .host()
        .catalog
        .store
        .journal_resume_state(&context, &journal.id)
        .unwrap();
    assert!(open.unwrap().body.contains("last input"));
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_reopen_reconciles_partial_saved_operations_without_duplicate_create() {
    let mut fixture = Fixture::new("proposed, corrected, partially performed, then interrupted");
    let state = fixture.host().catalog.state_root.clone();
    let worker = fixture._allocation.as_ref().unwrap().worker_id();
    let binding = fixture.connection.binding.clone();
    fixture.host().admit_journal(&fixture.context).unwrap();
    let (jobs, id) = fixture.host().next_journal_job().unwrap().unwrap();
    let model = JournalScript {
        pause_after_record: true,
        ..Default::default()
    };
    jobs.start_with_model_client(&id, model.clone())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        model.record_saved.notified(),
    )
    .await
    .unwrap();
    let context = authority(fixture.host(), &fixture.context.session_id).unwrap();
    let journal_id = fixture
        .host()
        .catalog
        .store
        .journal_for_session(&context)
        .unwrap()
        .unwrap();
    let entries = fixture
        .host()
        .catalog
        .store
        .list_journal_entries(&context, &journal_id, None, 32)
        .unwrap()
        .items;
    assert_eq!(entries.len(), 1);
    let saved_entry = entries[0].id.clone();
    jobs.shutdown().await.unwrap();
    fixture.connection.close().unwrap();
    let mut reopened = StandaloneSubjects::open_existing(&state)
        .unwrap()
        .connect(&binding.subject_id, worker)
        .unwrap();
    let jobs = crate::jobs::StandaloneJobs::open(&reopened.jobs_path, fixture._temp.path().into())
        .unwrap();
    reopened.host.bind_jobs(jobs.clone()).unwrap();
    assert_eq!(jobs.get(&id).unwrap().state, JobState::Unknown);
    // Domain reconciliation inspects the now-fenced prior attempt and preserves its receipts.
    let (_, replacement) = reopened.host.next_journal_job().unwrap().unwrap();
    assert_ne!(
        replacement, id,
        "unknown external execution is not blindly replayed"
    );
    jobs.start_with_model_client(&replacement, JournalScript::default())
        .await
        .unwrap();
    let completed =
        tokio::time::timeout(std::time::Duration::from_secs(20), jobs.wait(&replacement))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(completed.state, JobState::Completed);
    reopened
        .host
        .complete_journal_job(&jobs, &completed)
        .unwrap();
    let context = authority(&reopened.host, &fixture.context.session_id).unwrap();
    let entries = reopened
        .host
        .catalog
        .store
        .list_journal_entries(&context, &journal_id, None, 32)
        .unwrap()
        .items;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, saved_entry);
    assert!(
        reopened
            .host
            .session_fully_reflected(&fixture.context.session_id)
            .unwrap()
    );
    jobs.shutdown().await.unwrap();
    reopened.close().unwrap();
}

#[tokio::test]
async fn stopped_worker_removal_retains_originals_and_the_existing_durable_request() {
    let mut fixture = Fixture::new("Unrecorded stopped Worker");
    let state = fixture.host().catalog.state_root.clone();
    let binding = fixture.connection.binding.clone();
    let (worker, workers) = fixture.stop_worker();
    workers.delete(worker).unwrap();
    assert!(matches!(
        workers.load(worker),
        Err(crate::store::StandaloneStoreError::WorkerNotFound(_))
    ));
    let retained = retained_workers_root(&state).join(worker.to_string());
    assert!(
        retained
            .join("sessions")
            .join(&fixture.context.session_id)
            .is_dir()
    );
    workers.delete(worker).unwrap();
    let mut reopened = StandaloneSubjects::open_existing(&state)
        .unwrap()
        .connect(&binding.subject_id, protocol::WorkerId::now_v7())
        .unwrap();
    let jobs = crate::jobs::StandaloneJobs::open(&reopened.jobs_path, fixture._temp.path().into())
        .unwrap();
    reopened.host.bind_jobs(jobs.clone()).unwrap();
    assert_eq!(
        jobs.pending().unwrap().len(),
        1,
        "repeated deletion must not multiply requests"
    );
    let (_, id) = reopened.host.next_journal_job().unwrap().unwrap();
    let snapshot = jobs.get(&id).unwrap();
    assert!(
        grant(
            reopened.host.clone(),
            &snapshot,
            Arc::new(TestFence(AtomicBool::new(true)))
        )
        .is_ok(),
        "retained originals remain readable through the real grant path"
    );
    assert!(
        retained.is_dir(),
        "unfinished generation cannot prune original material"
    );
    jobs.start_with_model_client(&id, JournalScript::default())
        .await
        .unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(20), jobs.wait(&id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, JobState::Completed);
    reopened
        .host
        .complete_journal_job(&jobs, &completed)
        .unwrap();
    assert!(
        !retained.exists(),
        "only reflected original material may be pruned"
    );
    let context = authority(&reopened.host, &fixture.context.session_id).unwrap();
    let journal = reopened
        .host
        .catalog
        .store
        .list_journals(&context, None, 1)
        .unwrap()
        .items
        .pop()
        .unwrap();
    let (_, open) = reopened
        .host
        .catalog
        .store
        .journal_resume_state(&context, &journal.id)
        .unwrap();
    assert!(
        open.unwrap().body.contains("Only part performed"),
        "Journal remains readable without the removed Worker/Session"
    );
    jobs.shutdown().await.unwrap();
    reopened.close().unwrap();
}

#[test]
fn missing_original_material_blocks_worker_removal_without_erasing_state() {
    let mut fixture = Fixture::new("Unrecorded stopped Worker");
    let (worker, workers) = fixture.stop_worker();
    fs::remove_file(
        workers
            .sessions_dir(worker)
            .join(&fixture.context.session_id)
            .join(format!("{}.jsonl", fixture.segment)),
    )
    .unwrap();
    assert!(matches!(
        workers.delete(worker),
        Err(crate::store::StandaloneStoreError::JournalRetention(_))
    ));
    assert!(
        workers.load(worker).is_ok(),
        "unconfirmed handoff must not remove the Worker"
    );
}

#[test]
fn frozen_original_remains_readable_after_worker_switches_its_active_session() {
    let fixture = Fixture::new("old Session original");
    let (adapter, _, _) = fixture.adapter();
    let workers = StandaloneWorkerStore::open_existing(&fixture.host().catalog.state_root).unwrap();
    let worker = fixture._allocation.as_ref().unwrap().worker_id();
    let record = workers.load(worker).unwrap();
    let metadata = session_store::FsWorkerStore::new(workers.worker_metadata_dir(worker)).unwrap();
    metadata
        .write(&session_store::WorkerMetadata::new(
            record.storage_key,
            Some(session_store::WorkerActiveSegmentRef::active_segment(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )),
        ))
        .unwrap();
    let response = adapter
        .execute(JournalJobRequest::ReadText {
            read: JournalTextRead {
                target: JournalTextTarget::Source {
                    pointer: adapter.generation.selected_ranges[0].source.pointer.clone(),
                },
                offset: 0,
                limit: MAX_JOURNAL_READ_BYTES,
                content_token: None,
            },
        })
        .unwrap();
    let JournalJobResponse::Text(page) = response else {
        panic!()
    };
    assert!(page.text.contains("old Session original"));
    assert!(
        fixture
            .host()
            .reconcile_journal_session(&fixture.context.session_id)
            .is_err(),
        "unknown final active lineage must not authorize new admission/deletion"
    );
}

#[test]
fn explicit_erasure_fences_late_tools_and_raw_publication_without_forcing_generation() {
    let mut fixture = Fixture::new("private activity to erase");
    let (adapter, _, snapshot) = fixture.adapter();
    let state = fixture.host().catalog.state_root.clone();
    let binding = fixture.connection.binding.clone();
    assert!(
        fixture
            .sessions
            .delete_session(fixture.context.session_id.parse().unwrap())
            .is_err(),
        "ordinary Session retention respects the durable Journal pin"
    );
    fixture.connection.close().unwrap();
    StandaloneSubjects::open_existing(&state)
        .unwrap()
        .erase_session_completely(
            &binding.subject_id,
            &fixture.context.session_id,
            "explicit-erasure",
        )
        .unwrap();
    StandaloneSubjects::open_existing(&state)
        .unwrap()
        .erase_session_completely(
            &binding.subject_id,
            &fixture.context.session_id,
            "explicit-erasure",
        )
        .unwrap();
    assert!(
        !fixture
            .sessions
            .root_dir()
            .join(&fixture.context.session_id)
            .exists()
    );
    assert!(
        fixture
            .sessions
            .append(
                fixture.context.session_id.parse().unwrap(),
                fixture.segment,
                &session_store::LogEntry::AnnotatedUserInput {
                    ts: 5,
                    segments: vec![],
                    extensions: vec![],
                    history: vec![message("late", "must not return")]
                }
            )
            .is_err()
    );
    let catalog = StandaloneSubjects::open_existing(&state).unwrap();
    let context =
        JournalHostContext::validated_reader(&catalog.store, &binding.subject_id).unwrap();
    assert!(
        catalog
            .store
            .list_journals(&context, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
    let jobs = crate::jobs::StandaloneJobs::open(
        &fixture.connection.jobs_path,
        fixture._temp.path().into(),
    )
    .unwrap();
    assert_eq!(
        jobs.get(&snapshot.request.job_id).unwrap().state,
        JobState::Cancelled
    );
    assert!(jobs.pending().unwrap().is_empty());
    let delayed = JournalJobRequest::Execute {
        operation: JournalOperation {
            request_id: "late".into(),
            expected_state: adapter.generation.initial_journal.state_token.clone(),
            action: JournalAction::ConfirmUnchanged,
            covered_ranges: adapter.generation.selected_ranges.clone(),
        },
    };
    assert!(
        subjektiv::journal::service::execute_bounded(
            &catalog.store,
            &adapter.job_context,
            delayed,
            |_, _, _| panic!("erased Job must not fetch original material")
        )
        .is_err()
    );
}

#[test]
fn text_continuation_rejects_stale_content_and_invalid_utf8() {
    let fixture = Fixture::new("request");
    let (adapter, _, _) = fixture.adapter();
    let saved = save(
        &adapter,
        "position",
        &adapter.generation.initial_journal.state_token,
        JournalAction::UpdatePosition {
            current_position: "漢字🙂".into(),
        },
        vec![],
    );
    let read = JournalTextRead {
        target: JournalTextTarget::Position {
            snapshot: JournalSnapshot::Current,
        },
        offset: 0,
        limit: 5,
        content_token: None,
    };
    let JournalJobResponse::Text(first) = adapter
        .execute(JournalJobRequest::ReadText { read: read.clone() })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(first.text, "漢");
    assert_eq!(first.next_byte, Some(3));
    assert!(
        adapter
            .execute(JournalJobRequest::ReadText {
                read: JournalTextRead {
                    offset: 1,
                    content_token: Some(first.content_token.clone()),
                    ..read.clone()
                }
            })
            .is_err()
    );
    save(
        &adapter,
        "changed",
        &saved.state_token,
        JournalAction::UpdatePosition {
            current_position: "漢字🙂 changed".into(),
        },
        vec![],
    );
    assert!(
        adapter
            .execute(JournalJobRequest::ReadText {
                read: JournalTextRead {
                    offset: 3,
                    content_token: Some(first.content_token),
                    ..read.clone()
                }
            })
            .is_err()
    );
    assert!(
        adapter
            .execute(JournalJobRequest::ReadText {
                read: JournalTextRead { offset: 3, ..read }
            })
            .is_err()
    );
}

#[test]
fn relocated_erasure_fences_old_and_retained_readers_publishers_and_late_pins() {
    use session_store::public_material::{
        PublicMaterialError, PublicMaterialLimits, SessionPublicMaterialReader,
    };
    const SENTINEL: &str = "original-plaintext-sentinel-must-never-enter-immutable-job-storage";
    let mut fixture = Fixture::new(SENTINEL);
    let (_, _, snapshot) = fixture.adapter();
    fixture.assert_job_storage_excludes(&snapshot, SENTINEL);
    let state = fixture.host().catalog.state_root.clone();
    let binding = fixture.connection.binding.clone();
    let session: session_store::SessionId = fixture.context.session_id.parse().unwrap();
    let old_reader = fixture
        .host()
        .material_reader(&fixture.context.session_id)
        .unwrap();
    let descriptor = old_reader.descriptors().next().unwrap().clone();
    let (worker, workers) = fixture.stop_worker();
    workers.delete(worker).unwrap();
    let retained = retained_workers_root(&state).join(worker.to_string());
    let retained_sessions = retained.join("sessions");
    let retained_store = session_store::FsStore::new_with_retention(
        &retained_sessions,
        crate::store::session_retention(&state),
    )
    .unwrap();
    let retained_reader = SessionPublicMaterialReader::open_fs_with_retention(
        &retained_sessions,
        session,
        fixture.segment,
        PublicMaterialLimits::default(),
        crate::store::session_retention(&state),
    )
    .unwrap();
    assert!(
        retained_reader
            .read_content(&descriptor, 0, 1024)
            .unwrap()
            .content
            .contains(SENTINEL)
    );
    assert!(!retained_sessions.join(".retention").exists());
    assert!(state.join("retention/session-sources").is_dir());
    for _ in 0..2 {
        StandaloneSubjects::open_existing(&state)
            .unwrap()
            .erase_session_completely(
                &binding.subject_id,
                &fixture.context.session_id,
                "relocated-erasure",
            )
            .unwrap();
    }
    for source_store in [&fixture.sessions, &retained_store] {
        assert!(
            matches!(source_store.append(session, fixture.segment, &session_store::LogEntry::TurnEnd { ts: 7, turn_count: 1 }), Err(session_store::StoreError::SessionErased(id)) if id == session)
        );
        assert!(
            matches!(source_store.pin_session(session, session_store::retention::SessionRetentionPin(session)), Err(session_store::StoreError::SessionErased(id)) if id == session)
        );
        assert!(!source_store.root_dir().join(session.to_string()).exists());
    }
    for reader in [&old_reader, &retained_reader] {
        assert!(matches!(
            reader.read_content(&descriptor, 0, 1024),
            Err(PublicMaterialError::Erased)
        ));
    }
    assert!(matches!(
        SessionPublicMaterialReader::open_fs_with_retention(
            &retained_sessions,
            session,
            fixture.segment,
            PublicMaterialLimits::default(),
            crate::store::session_retention(&state),
        ),
        Err(PublicMaterialError::Erased)
    ));
    let mut reopened = StandaloneSubjects::open_existing(&state)
        .unwrap()
        .connect(&binding.subject_id, protocol::WorkerId::now_v7())
        .unwrap();
    let jobs = crate::jobs::StandaloneJobs::open(&reopened.jobs_path, fixture._temp.path().into())
        .unwrap();
    reopened.host.bind_jobs(jobs.clone()).unwrap();
    reopened.host.recover_journals();
    assert!(reopened.host.next_journal_job().unwrap().is_none());
    reopened.host.prune_retained_workers().unwrap();
    assert!(!retained.exists());
    assert!(
        matches!(fixture.sessions.retention().lock_live(session), Err(session_store::StoreError::SessionErased(id)) if id == session)
    );
    assert_eq!(
        jobs.get(&snapshot.request.job_id).unwrap().state,
        JobState::Cancelled
    );
    fixture.assert_job_storage_excludes(&jobs.get(&snapshot.request.job_id).unwrap(), SENTINEL);
    reopened.close().unwrap();
}

#[test]
fn erased_session_reconciliation_and_removal_do_not_require_originals_or_generate() {
    use session_store::retention::SessionDeletionIntent;
    let mut fixture = Fixture::new("erasure pending physical cleanup");
    let session: session_store::SessionId = fixture.context.session_id.parse().unwrap();
    let context = authority(fixture.host(), &fixture.context.session_id).unwrap();
    fixture
        .host()
        .catalog
        .store
        .erase_journal_session(&context, "failed-physical-erasure")
        .unwrap();
    // A previous explicit erasure may leave bytes behind. Its durable intent, not an error
    // message or missing source, permits resuming physical cleanup during ordinary removal.
    assert!(
        fixture
            .sessions
            .retention()
            .delete(session, SessionDeletionIntent::ErasePermanently, || Err::<
                (),
                session_store::StoreError,
            >(
                std::io::Error::other("injected physical failure").into()
            ))
            .is_err()
    );
    assert!(
        !fixture
            .host()
            .reconcile_journal_session(&fixture.context.session_id)
            .unwrap()
    );
    assert!(
        fixture
            .host()
            .session_fully_reflected(&fixture.context.session_id)
            .unwrap()
    );
    assert_eq!(
        fixture
            .host()
            .request_journal_recording(&fixture.context, JournalRecordingReason::RunCommitted)
            .unwrap(),
        JournalRecordingResponse::NoNewSources
    );
    fixture
        .host()
        .record_session(&fixture.context, true)
        .unwrap();
    let jobs = fixture.host().jobs().unwrap();
    assert!(jobs.pending().unwrap().is_empty());
    assert!(fixture.host().next_journal_job().unwrap().is_none());
    let (worker, workers) = fixture.stop_worker();
    workers.delete(worker).unwrap();
    assert!(!workers.root().join(worker.to_string()).exists());
    assert!(
        !retained_workers_root(workers.root())
            .join(worker.to_string())
            .exists()
    );
    assert!(
        matches!(fixture.sessions.retention().lock_live(session), Err(session_store::StoreError::SessionErased(id)) if id == session)
    );
}

#[tokio::test]
async fn late_terminal_boundary_after_reflected_task_prefix_is_read_by_next_job() {
    use session_store::public_material::PublicMaterialOrigin;
    let fixture = Fixture::new("initial request");
    let session = fixture.context.session_id.parse().unwrap();
    fixture
        .sessions
        .append(
            session,
            fixture.segment,
            &session_store::LogEntry::Invoke {
                ts: 2,
                trigger: protocol::InvokeKind::UserSend,
            },
        )
        .unwrap();
    let mut output = message(
        "task-output",
        "partial work; Task completed but broader work remains",
    );
    let session_store::LoggedItem::Message { role, .. } = &mut output.item else {
        panic!()
    };
    *role = session_store::LoggedRole::Assistant;
    output.metadata.origin = session_store::LoggedSessionHistoryOrigin::LegacyUnknown;
    fixture
        .sessions
        .append(
            session,
            fixture.segment,
            &session_store::LogEntry::AnnotatedAssistantItem {
                ts: 3,
                entry: output,
            },
        )
        .unwrap();
    fixture.host().admit_journal(&fixture.context).unwrap();
    let before = fixture
        .host()
        .journal_sources(&fixture.context.session_id)
        .unwrap();
    let (jobs, first) = fixture.host().next_journal_job().unwrap().unwrap();
    jobs.start_with_model_client(&first, JournalScript::default())
        .await
        .unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(20), jobs.wait(&first))
        .await
        .unwrap()
        .unwrap();
    fixture
        .host()
        .complete_journal_job(&jobs, &completed)
        .unwrap();
    assert!(
        fixture
            .host()
            .session_fully_reflected(&fixture.context.session_id)
            .unwrap()
    );
    // No new public body follows this Task-safe prefix. The Host fact must still be new
    // material; it must neither change prior body digests nor disappear as "already read".
    fixture
        .sessions
        .append(
            session,
            fixture.segment,
            &session_store::LogEntry::RunErrored {
                ts: 4,
                entry_id: None,
                interrupted: true,
                message: "SECRET_PROVIDER_DETAIL_NOT_PUBLIC".into(),
                failure: None,
            },
        )
        .unwrap();
    assert!(fixture.host().admit_journal(&fixture.context).unwrap());
    let after = fixture
        .host()
        .journal_sources(&fixture.context.session_id)
        .unwrap();
    assert_eq!(&after[..before.len()], &before);
    assert_eq!(after.len(), before.len() + 1);
    let (_, next) = fixture.host().next_journal_job().unwrap().unwrap();
    let snapshot = jobs.get(&next).unwrap();
    let grant = fixture.host().journal_grant(&snapshot).unwrap();
    assert_eq!(grant.ranges.len(), 1);
    assert_eq!(
        grant.origins[0].origin,
        PublicMaterialOrigin::HostRunBoundary
    );
    let content = fixture
        .host()
        .journal_source_window(&grant.ranges[0].source, 0, 4096)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["kind"], "run_boundary");
    assert_eq!(value["boundary"]["kind"], "errored");
    assert!(!content.contains("SECRET_PROVIDER_DETAIL_NOT_PUBLIC"));
    jobs.start_with_model_client(&next, JournalScript::default())
        .await
        .unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(20), jobs.wait(&next))
        .await
        .unwrap()
        .unwrap();
    fixture
        .host()
        .complete_journal_job(&jobs, &completed)
        .unwrap();
    assert!(
        fixture
            .host()
            .session_fully_reflected(&fixture.context.session_id)
            .unwrap()
    );
    assert!(fixture.host().next_journal_job().unwrap().is_none());
    jobs.shutdown().await.unwrap();
}
