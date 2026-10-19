//! Public standalone protocol through real mid-turn compaction and Journal Jobs.
//! Only external model streams are scripted. No history, attribution, hook or Job
//! result is injected; cancellation and rollback use the controller's public commands.
//! Reported model usage and a low configured request threshold force two rewrites.
//! The test attests durable public originals, not unfinished display deltas or meaning.

use super::*;
use agen::llm_client::{
    ClientError, LlmClient, Request,
    event::{Event, ResponseStatus, StatusEvent, StopReason, UsageEvent},
    types::Item,
};
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;

type ResponseStream = Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>;
const SUMMARY: &str = "DERIVED_COMPACTION_SENTINEL_NOT_AN_ORIGINAL";
const ROLLED_BACK: &str = "ROLLED_BACK_INPUT_MUST_NOT_REACH_JOURNAL";
const PARTIAL: &str = "public partial answer before cancellation";

#[derive(Clone, Default)]
struct CompactingDialog {
    shared: DialogAndJournal,
    dialog_calls: Arc<AtomicUsize>,
    compactions: Arc<AtomicUsize>,
    rollback_entered: Arc<tokio::sync::Notify>,
    partial_stream_paused: Arc<tokio::sync::Notify>,
}

fn events(events: Vec<Event>) -> ResponseStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}
fn done() -> ResponseStream {
    events(vec![
        Event::text_block_start(0),
        Event::text_delta(0, "done"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
    ])
}

#[async_trait::async_trait]
impl LlmClient for CompactingDialog {
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        if request
            .tools
            .iter()
            .any(|tool| tool.name == "write_summary")
        {
            if request.items.iter().any(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "compact-summary")) {
                return Ok(done());
            }
            let count = self.compactions.fetch_add(1, Ordering::SeqCst);
            assert!(count < 2, "unexpected additional compaction");
            return Ok(events(vec![
                Event::tool_use_start(0, "compact-summary", "write_summary"),
                Event::tool_input_delta(0, serde_json::json!({"text": SUMMARY}).to_string()),
                Event::tool_use_stop(0),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]));
        }
        if request.tools.iter().any(|tool| {
            matches!(
                tool.name.as_str(),
                "ReadJournalState" | "SubmitMemorySurface" | "SubmitJobResult"
            )
        }) {
            return self.shared.stream(request).await;
        }
        match self.dialog_calls.fetch_add(1, Ordering::SeqCst) {
            step @ (0 | 1) => {
                assert!(request.tools.iter().any(|tool| tool.name == "TaskCreate"));
                Ok(events(vec![
                    Event::tool_use_start(0, format!("public-task-{step}"), "TaskCreate"),
                    Event::tool_input_delta(
                        0,
                        serde_json::json!({
                            "subject": format!("original activity {step}"),
                            "description": format!("original task result before compaction {step}")
                        })
                        .to_string(),
                    ),
                    Event::tool_use_stop(0),
                    Event::Usage(UsageEvent {
                        input_tokens: Some(100_000),
                        output_tokens: Some(10),
                        total_tokens: Some(100_010),
                        ..Default::default()
                    }),
                    Event::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]))
            }
            2 => {
                let paused = self.partial_stream_paused.clone();
                Ok(Box::pin(
                    events(vec![
                        Event::text_block_start(0),
                        Event::text_delta(0, PARTIAL),
                        Event::text_block_stop(0, None),
                    ])
                    .chain(futures::stream::once(async move {
                        paused.notify_one();
                        std::future::pending::<Result<Event, ClientError>>().await
                    })),
                ))
            }
            3 => {
                // No AI event: cancelling this request must roll back the submitted input.
                self.rollback_entered.notify_one();
                std::future::pending::<()>().await;
                unreachable!()
            }
            other => panic!("unexpected dialog request {other}"),
        }
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

async fn cancel_and_wait(
    client: &mut client::Client<client::transport::in_process::Socket>,
    sequence: u64,
) {
    client
        .send(&protocol::Method::Cancel {
            command: protocol::WorkerCommandEnvelope::new(sequence),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            match client.next_event().await.unwrap().unwrap() {
                protocol::Event::RunEnd { result } => {
                    assert_ne!(result, protocol::RunResult::Finished);
                    return;
                }
                protocol::Event::Error { code, message } => panic!("{code:?}: {message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn public_mid_turn_compactions_preserve_originals_and_drain_after_cancel_and_rollback() {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let catalog = StandaloneSubjects::open(&state).unwrap();
        let subject = catalog.create("compaction-journal", None).unwrap();
        let model = CompactingDialog {
            shared: DialogAndJournal {
                journal: JournalScript {
                    gate_first: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let mut launch = crate::StandaloneLaunchConfig::new(
            temp.path(),
            &state,
            manifest::ProfileSelector::source_named(
                manifest::ProfileRegistrySource::Builtin,
                "standalone-subjektiv",
            ),
            "public-compaction-journal",
        )
        .with_subject(subject.id.clone())
        .resolve()
        .unwrap();
        launch.profile.manifest.compaction = Some(manifest::CompactionConfig {
            threshold: None,
            request_threshold: Some(50_000),
            retained_tokens: 0,
            ..Default::default()
        });
        let mut host = crate::StandaloneHost::start_with_model_client(launch, model.clone())
            .await
            .unwrap();
        let session = host.record().active_session_id;
        let workers = StandaloneWorkerStore::open_existing(&state).unwrap();
        let source_store = session_store::FsStore::new_with_retention(
            workers.sessions_dir(host.record().worker_id),
            crate::store::session_retention(&state),
        )
        .unwrap();
        let mut client = host.connect();
        client
            .send(&protocol::Method::submit_text(
                protocol::new_submission_request_id(),
                "original long turn survives compaction",
            ))
            .await
            .unwrap();
        let mut completed_compactions = 0;
        loop {
            match client.next_event().await.unwrap().unwrap() {
                protocol::Event::CompactionProgress {
                    compaction: Some(progress),
                } if progress.phase == protocol::CompactionPhase::Committing => {
                    assert_eq!(
                        progress.trigger,
                        protocol::CompactionTrigger::RequestThreshold
                    );
                    completed_compactions += 1;
                }
                protocol::Event::TextDelta { text } if text.contains(PARTIAL) => break,
                protocol::Event::RunEnd { .. } => panic!("dialog ended before cancellation"),
                protocol::Event::SessionEntryCommitted { entry } if matches!(entry.data, protocol::SessionSnapshotEntryData::RunError { .. }) => panic!("dialog failed before cancellation: {entry:?}"),
                protocol::Event::Error { code, message } => panic!("{code:?}: {message}"),
                _ => {}
            }
        }
        assert_eq!(model.compactions.load(Ordering::SeqCst), 2);
        assert_eq!(completed_compactions, 2);
        model.shared.journal.entered.notified().await;
        let context = JournalHostContext::validated_session(
            &catalog.store,
            catalog
                .store
                .session_attribution(&session.to_string())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let reader = JournalHostContext::validated_reader(&catalog.store, &subject.id).unwrap();
        let journal = catalog
            .store
            .list_journals(&reader, None, 1)
            .unwrap()
            .items
            .pop()
            .unwrap();
        let first = catalog
            .store
            .active_journal_generation(&context, &journal.id)
            .unwrap()
            .unwrap();
        assert!(
            catalog
                .store
                .list_journal_entries(&reader, &journal.id, None, 10)
                .unwrap()
                .items
                .is_empty(),
            "compaction must finish before generation is released"
        );
        model.partial_stream_paused.notified().await;
        cancel_and_wait(&mut client, 1).await;
        client
            .send(&protocol::Method::submit_text(
                protocol::new_submission_request_id(),
                ROLLED_BACK,
            ))
            .await
            .unwrap();
        model.rollback_entered.notified().await;
        let before_rollback: Vec<_> = source_store
            .list_segments(session)
            .unwrap()
            .iter()
            .flat_map(|segment| source_store.read_all(session, *segment).unwrap())
            .collect();
        assert!(
            serde_json::to_string(&before_rollback)
                .unwrap()
                .contains(ROLLED_BACK),
            "the submitted input must reach the actual durable rollback boundary"
        );
        cancel_and_wait(&mut client, 2).await;
        assert_eq!(
            catalog
                .store
                .active_journal_generation(&context, &journal.id)
                .unwrap()
                .unwrap(),
            first,
            "compaction/cancellation must not alter the running generation's frozen input"
        );

        assert!(matches!(source_store.delete_session(session),
            Err(session_store::StoreError::SessionPinned(id)) if id == session));
        let segments = source_store.list_segments(session).unwrap();
        assert_eq!(segments.len(), 3);
        let raw: Vec<_> = segments
            .iter()
            .flat_map(|segment| source_store.read_all(session, *segment).unwrap())
            .collect();
        for history in raw.iter().filter_map(|record| match record {
            session_store::LogEntry::AnnotatedSegmentStart { history, compacted_from: Some(_), .. } => Some(history),
            _ => None,
        }) {
            let mut calls = std::collections::HashSet::new();
            let mut results = 0;
            for entry in history {
                let is_tool = matches!(entry.item, session_store::LoggedItem::ToolCall { .. } | session_store::LoggedItem::ToolResult { .. });
                let Some(metadata) = entry.metadata.input_metadata.as_ref().filter(|_| is_tool) else { continue; };
                assert_eq!(metadata.usage().recording_material, session_store::InputEvidenceUse::Exclude);
                assert_eq!(metadata.usage().memory_evidence, session_store::InputEvidenceUse::Exclude);
                assert_eq!(metadata.usage().observation, session_store::InputObservationUse::Hidden);
                assert!(metadata.public_item().is_none());
                assert!(entry.metadata.derivation.is_some());
                match &entry.item {
                    session_store::LoggedItem::ToolCall { call_id, .. } => { calls.insert(call_id.clone()); }
                    session_store::LoggedItem::ToolResult { call_id, .. } => {
                        assert!(calls.contains(call_id), "derived Tool result must follow its real logical call");
                        results += 1;
                    }
                    _ => unreachable!(),
                }
            }
            assert!(!calls.is_empty());
            assert!(results >= calls.len());
        }
        assert_eq!(
            raw.iter()
                .filter(|entry| matches!(
                    entry,
                    session_store::LogEntry::AnnotatedSegmentStart {
                        compacted_from: Some(_),
                        ..
                    }
                ))
                .count(),
            2
        );
        assert!(
            !serde_json::to_string(&raw).unwrap().contains(ROLLED_BACK),
            "pre-AI cancellation must roll back the speculative input"
        );
        assert!(
            serde_json::to_string(&raw).unwrap().contains(SUMMARY),
            "real rewrite did not persist its summary"
        );
        let metadata =
            session_store::FsWorkerStore::new(workers.worker_metadata_dir(host.record().worker_id))
                .unwrap()
                .read_by_name(&host.record().storage_key)
                .unwrap()
                .unwrap();
        let originals =
            session_store::public_material::SessionPublicMaterialReader::open_fs_with_retention(
                &workers.sessions_dir(host.record().worker_id),
                session,
                metadata.active.unwrap().segment_id.unwrap(),
                session_store::public_material::PublicMaterialLimits::default(),
                crate::store::session_retention(&state),
            )
            .unwrap();
        let final_original = originals.descriptors().last().unwrap();
        let expected_cursor = (final_original.canonical_order + 1, 0);
        let canonical: Vec<_> = originals
            .descriptors()
            .map(|descriptor| {
                let page = originals
                    .read_content(descriptor, 0, MAX_JOURNAL_READ_BYTES)
                    .unwrap();
                assert_eq!(page.next_offset, descriptor.total_bytes);
                page.content
            })
            .collect();
        assert!(!canonical.join("\n").contains(ROLLED_BACK));
        assert!(!canonical.join("\n").contains(SUMMARY));

        // Resume the existing runner, then its one durable successor, with no more input.
        let jobs = host.jobs();
        model.shared.journal.release.notify_one();
        jobs.wait(&first.job.job_id).await.unwrap();
        model.shared.journal.entered.notified().await;
        let second = catalog
            .store
            .active_journal_generation(&context, &journal.id)
            .unwrap()
            .unwrap();
        assert_ne!(first.id, second.id);
        model.shared.journal.release.notify_one();
        jobs.wait(&second.job.job_id).await.unwrap();
        assert!(!catalog.store.journal_retention_required(&context).unwrap());
        let (saved, open) = catalog
            .store
            .journal_resume_state(&context, &journal.id)
            .unwrap();
        let open = open.unwrap();
        assert!(!open.ongoing.is_empty());
        let entries = catalog
            .store
            .list_journal_entries(&reader, &journal.id, None, 10)
            .unwrap();
        assert_eq!(
            entries.items.len(),
            1,
            "compaction/turn boundaries must not fix or duplicate the open entry"
        );
        assert_eq!(entries.items[0].id, open.id);
        let supplied = model
            .shared
            .journal
            .source_reads
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        for original in [
            "original long turn survives compaction",
            "original activity 0",
            "original activity 1",
        ] {
            assert!(
                supplied.contains(original),
                "Journal did not receive {original}"
            );
        }
        for original in canonical {
            assert!(
                supplied.contains(&original),
                "a durable public original was skipped"
            );
        }
        assert!(
            !supplied.contains(SUMMARY),
            "derived summary substituted for original input"
        );
        assert!(
            !supplied.contains(ROLLED_BACK),
            "cancelled input was admitted after rollback"
        );
        assert_eq!((saved.process_order, saved.process_byte), expected_cursor);
        let position = catalog
            .store
            .read_journal_text(
                &reader,
                &journal.id,
                None,
                JournalTextField::CurrentPosition,
                None,
                0,
                MAX_JOURNAL_READ_BYTES,
            )
            .unwrap();
        assert!(!position.text.is_empty());
        loop {
            match host.disconnect_subject().await {
                Ok(()) => break,
                Err(crate::StandaloneSubjectDisconnectError::Incomplete(message)) if message.contains("dispatcher") || message.contains("cleanup") => tokio::task::yield_now().await,
                Err(error) => panic!("recorded compaction/cancellation lineage must remain releasable while work is unfinished: {error}"),
            }
        }
        host.shutdown().await.unwrap();
    })
    .await
    .expect("bounded public compaction/Journal integration");
}
