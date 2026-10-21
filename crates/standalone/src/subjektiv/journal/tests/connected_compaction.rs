//! Real LocalSubjectHost/Journal storage -> PreparedWorker -> compactor Request -> durable seed.
//! Only external LLM responses and the fixture's Job execution fence are substituted. The
//! in-flight generation is retained while its real adapter updates storage during generation.
//! Temporary storage and the parent fixture own cleanup; ordinary standalone lib tests select
//! this suite. Predetermined summaries attest transport and persistence, not model understanding.

use super::*;
use agen::llm_client::{
    ClientError, LlmClient, Request,
    event::{Event, ResponseStatus, StatusEvent},
    types::Item,
};
use futures::Stream;
use session_store::{CombinedStore, FsWorkerStore, LogEntry};
use std::pin::Pin;

const BEFORE: &str = "FROZEN_POSITION: B partially implemented; tests not run; result unconfirmed";
const AFTER: &str = "NEXT_POSITION: background Journal changed during compactor generation";
const COMPLEMENT: &str =
    "Unrecorded request: keep the user's constraint and finish the remaining checks.";

#[derive(Clone)]
struct MockClient {
    adapter: Arc<LocalJournalHost>,
    requests: Arc<Mutex<Vec<Request>>>,
    updated: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl LlmClient for MockClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == "write_summary")
        );
        let first = self.requests.lock().unwrap().is_empty();
        self.requests.lock().unwrap().push(request);
        let events = if first {
            assert!(!self.updated.swap(true, Ordering::SeqCst));
            let state = self.adapter.state().unwrap();
            save(
                &self.adapter,
                "position-during-compaction",
                &state.current_journal.state_token,
                JournalAction::UpdatePosition {
                    current_position: AFTER.into(),
                },
                vec![],
            );
            vec![
                Event::tool_use_start(0, "summary", "write_summary"),
                Event::tool_input_delta(0, serde_json::json!({ "text": COMPLEMENT }).to_string()),
                Event::tool_use_stop(0),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]
        } else {
            vec![
                Event::text_block_start(0),
                Event::text_delta(0, "summary saved"),
                Event::text_block_stop(0, None),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test]
async fn local_host_compaction_freezes_pending_journal_through_request_and_durable_seed_during_update()
 {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let fixture = Fixture::new("Keep the user's constraint; work remains unfinished and unconfirmed.");
        let (adapter, _, job) = fixture.adapter();
        let created = save(
            &adapter,
            "record-partial-work",
            &adapter.generation.initial_journal.state_token,
            JournalAction::Create {
                title: "A corrected to B".into(),
                body: "B partially implemented; tests not run; result unconfirmed".into(),
                ongoing: "Finish remaining checks".into(),
                state: JournalEntryState::Open,
                source_end: None,
                correction_refs: vec![],
            },
            adapter.generation.selected_ranges.clone(),
        );
        save(&adapter, "position-before-compaction", &created.state_token,
            JournalAction::UpdatePosition { current_position: BEFORE.into() }, vec![]);
        let before = adapter.state().unwrap();
        let expected = fixture.host().journal_compaction_context(&fixture.context).unwrap();
        let journal = &expected.journals[0];
        assert_eq!(journal.source_session_id, fixture.context.session_id);
        assert_eq!(journal.current_position, BEFORE);
        assert_eq!(journal.entries[0].state, JournalEntryState::Open);
        assert!(!before.selected_ranges.is_empty());
        assert_eq!(journal.process_order, before.current_journal.process_order);
        assert_eq!(journal.process_byte, before.current_journal.process_byte);
        assert_eq!(journal.process_order, 0);
        assert_eq!(journal.process_byte, 0, "saved open records do not advance processing checkpoint");
        let expected_json = serde_json::to_string(&expected).unwrap();
        let adapter = Arc::new(adapter);
        let model = MockClient { adapter: adapter.clone(), requests: Arc::default(), updated: Arc::new(AtomicBool::new(false)) };
        let workers = StandaloneWorkerStore::open_existing(&fixture.host().catalog.state_root).unwrap();
        let worker_id = fixture._allocation.as_ref().unwrap().worker_id();
        let store = CombinedStore::new(fixture.sessions.clone(), FsWorkerStore::new(workers.worker_metadata_dir(worker_id)).unwrap());
        let mut manifest = manifest::WorkerManifest::from_toml(&format!("[worker]\nname='{}'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]", fixture.context.worker_id)).unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let mut prepared = worker::WorkerBootstrap::new(
            manifest,
            store,
            worker::PromptCatalogSource::builtins_only(),
            worker::WorkerWorkspaceContext::no_workspace(),
            worker::WorkerFilesystemAuthority::None,
            worker::WorkerBootstrapLayout::Direct { runtime_base: fixture._temp.path().join("runtime") },
        ).with_model_client(model.clone()).with_subjektiv_host(fixture.host().clone()).prepare_restored("journal-test").await.unwrap();
        assert_eq!(prepared.worker().session_id().to_string(), fixture.context.session_id);
        let source_segment = prepared.worker().segment_id();
        let original = fixture.sessions.read_all(prepared.worker().session_id(), source_segment).unwrap();
        let generation = fixture.host().catalog.store.active_journal_generation(&adapter.context, &adapter.grant.journal_id).unwrap().unwrap();
        assert!(!fixture.host().session_fully_reflected(&fixture.context.session_id).unwrap());
        prepared.worker_mut().compact(0).await.unwrap();
        assert!(model.updated.load(Ordering::SeqCst));
        let worker = prepared.worker();
        assert_ne!(worker.segment_id(), source_segment);
        let seed = worker.store().read_all(worker.session_id(), worker.segment_id()).unwrap().into_iter().find_map(|entry| match entry { LogEntry::AnnotatedSegmentStart { history, .. } => Some(history), _ => None }).expect("durable compacted seed");
        let journal_items: Vec<_> = seed.iter().filter(|entry| entry.metadata.input_metadata.as_ref().is_some_and(|input| input.usage().compaction == session_store::InputCompactionUse::ReferenceInput)).collect();
        assert_eq!(journal_items.len(), 1);
        let journal_item = Item::from(journal_items[0].item.clone());
        let body = journal_item.as_text().unwrap();
        assert!(body.contains(&expected_json), "durable seed contains the full selected JSON and processing checkpoint");
        assert!(body.contains(BEFORE));
        assert!(!body.contains(AFTER));
        assert!(seed.iter().any(|entry| entry.metadata.input_metadata.as_ref().is_some_and(|input| input.usage().compaction == session_store::InputCompactionUse::OpeningSummary) && Item::from(entry.item.clone()).as_text().is_some_and(|text| text.contains(COMPLEMENT))));
        let requests = model.requests.lock().unwrap();
        let input = requests[0].items.iter().find_map(|item| match item {
            Item::Message { role: agen::llm_client::types::Role::User, .. } => item.as_text().filter(|text| text.contains("## Conversation overview/index")),
            _ => None,
        }).expect("actual compactor user request");
        assert!(input.ends_with(body), "exact same frozen reference goes to actual compactor and seed");
        assert_eq!(input.matches(&expected_json).count(), 1);
        assert!(!input.contains(AFTER));
        drop(requests);
        let latest = fixture.host().journal_compaction_context(&fixture.context).unwrap();
        assert_eq!(latest.journals[0].current_position, AFTER);
        assert_ne!(latest.journals[0].state_token, expected.journals[0].state_token);
        assert_eq!(latest.journals[0].process_order, expected.journals[0].process_order);
        assert_eq!(latest.journals[0].process_byte, expected.journals[0].process_byte);
        assert_eq!(latest.journals[0].entries, expected.journals[0].entries);
        assert_eq!(fixture.host().catalog.store.active_journal_generation(&adapter.context, &adapter.grant.journal_id).unwrap(), Some(generation));
        assert_eq!(fixture.host().jobs().unwrap().get(&job.request.job_id).unwrap().state, job.state);
        let unchanged: Vec<_> = fixture.sessions.read_all(worker.session_id(), source_segment).unwrap().into_iter().filter(|entry| !matches!(entry, LogEntry::Extension { domain, .. } if domain == "metrics")).collect();
        assert_eq!(serde_json::to_value(unchanged).unwrap(), serde_json::to_value(original).unwrap());
        assert!(!fixture.host().session_fully_reflected(&fixture.context.session_id).unwrap());
    }).await.expect("compaction must finish without recording Job completion");
}
