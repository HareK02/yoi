mod common;

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use agen::interceptor::{Interceptor, TurnEndAction};
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::llm_client::types::{Item, RequestConfig};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use agen::{Engine, History};
use async_trait::async_trait;
use common::MockLlmClient;
use session_store::{FsStore, LogEntry, SegmentStartState, Store, collect_state};

// =============================================================================
// Helpers
// =============================================================================

fn annotated(items: &[Item]) -> Vec<session_store::LoggedHistoryEntry> {
    items
        .iter()
        .cloned()
        .map(|item| session_store::LoggedHistoryEntry {
            item: session_store::LoggedItem::from(item),
            metadata: session_store::LoggedSessionHistoryMetadata {
                entry_id: session_store::LoggedSessionHistoryEntryId::new(),
                origin: session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        })
        .collect()
}

fn simple_text_events() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello!"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

fn tool_call_events() -> Vec<Vec<Event>> {
    vec![
        // 1st response: tool call
        vec![
            Event::tool_use_start(0, "call_1", "get_weather"),
            Event::tool_input_delta(0, r#"{"city":"Tokyo"}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        // 2nd response: final text
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "It's sunny in Tokyo!"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]
}

#[derive(Clone)]
struct MockWeatherTool;

#[async_trait]
impl Tool for MockWeatherTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok("Sunny, 25C".to_string().into())
    }
}

fn weather_tool_definition() -> ToolDefinition {
    Arc::new(|| {
        let meta = ToolMeta::new("get_weather")
            .description("Get weather")
            .input_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }));
        (meta, Arc::new(MockWeatherTool) as Arc<dyn Tool>)
    })
}

/// Policy that forces Pause on every turn end.
struct PausePolicy;

#[async_trait]
impl Interceptor for PausePolicy {
    async fn on_turn_end(&self, _history: &[Item]) -> TurnEndAction {
        TurnEndAction::Pause
    }
}

fn make_store() -> (tempfile::TempDir, FsStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    (dir, store)
}

struct TestWorker {
    engine: Engine<MockLlmClient>,
    history: History,
}

impl TestWorker {
    fn new(engine: Engine<MockLlmClient>) -> Self {
        Self {
            engine,
            history: History::new(),
        }
    }

    fn history(&self) -> Vec<Item> {
        self.history.items_cloned()
    }
}

impl Deref for TestWorker {
    type Target = Engine<MockLlmClient>;

    fn deref(&self) -> &Self::Target {
        &self.engine
    }
}

impl DerefMut for TestWorker {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.engine
    }
}

/// Run a worker turn and persist via session-store functions.
/// Takes ownership of the worker (needed for lock/unlock) and returns it.
async fn run_and_persist(
    mut worker: TestWorker,
    store: &FsStore,
    session_id: session_store::SessionId,
    segment_id: session_store::SegmentId,
    input: &str,
) -> (TestWorker, agen::EngineRunExit) {
    // Mirror Worker's run-entry contract: log the user input as segments
    // before the worker pushes its flattened user_message; save_delta
    // skips the resulting user_message item to avoid double-write.
    session_store::save_user_input(
        store,
        session_id,
        segment_id,
        vec![protocol::Segment::text(input)],
        annotated(&[Item::user_message(input)]),
    )
    .unwrap();

    let history_before = worker.history.len();

    let mut locked = worker.engine.lock(&worker.history);
    let result = locked.run(&mut worker.history, input).await;
    worker.engine = locked.unlock();

    let projected = worker.history();
    let new_items = annotated(&projected[history_before..]);
    session_store::save_delta(store, session_id, segment_id, &new_items).unwrap();
    session_store::save_turn_end(store, session_id, segment_id, worker.turn_count()).unwrap();

    match &result {
        agen::EngineRunExit::Finished
        | agen::EngineRunExit::Paused
        | agen::EngineRunExit::Yielded => {
            let (legacy_result, interrupted) = match &result {
                agen::EngineRunExit::Finished => (agen::EngineResult::Finished, false),
                agen::EngineRunExit::Paused => (agen::EngineResult::Paused, true),
                agen::EngineRunExit::Yielded => (agen::EngineResult::Yielded, true),
                agen::EngineRunExit::Interrupted(_) => unreachable!(),
            };
            session_store::save_run_completed(
                store,
                session_id,
                segment_id,
                legacy_result,
                interrupted,
                worker.active_run_turn_count(),
            )
            .unwrap();
        }
        agen::EngineRunExit::Interrupted(agen::StopReason::LimitReached) => {
            session_store::save_run_completed(
                store,
                session_id,
                segment_id,
                agen::EngineResult::LimitReached,
                false,
                worker.active_run_turn_count(),
            )
            .unwrap();
        }
        agen::EngineRunExit::Interrupted(reason) => {
            session_store::save_run_errored(
                store,
                session_id,
                segment_id,
                format!("{reason:?}"),
                true,
            )
            .unwrap();
        }
    }

    (worker, result)
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn session_run_logs_entries() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let worker = TestWorker::new(Engine::new(client));

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, segid, "Hi").await;
    let _ = &worker;

    let entries = store.read_all(sid, segid).unwrap();

    // SegmentStart, UserInput, AssistantItem, TurnEnd, RunCompleted (at minimum)
    assert!(
        entries.len() >= 4,
        "expected at least 4 entries, got {}",
        entries.len()
    );

    // First entry is SegmentStart
    assert!(matches!(
        &entries[0],
        LogEntry::AnnotatedSegmentStart { .. }
    ));

    // Has a RunCompleted with Finished
    let has_finished = entries.iter().any(|e| {
        matches!(
            e,
            LogEntry::RunCompleted {
                result: agen::EngineResult::Finished,
                ..
            }
        )
    });
    assert!(has_finished, "should have a Finished outcome");
}

#[tokio::test]
async fn session_restore_round_trip() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = TestWorker::new(Engine::new(client));
    worker.set_system_prompt("You are helpful.");

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, segid, "Hi").await;

    let original_history_len = worker.history().len();
    let original_turn_count = worker.turn_count();

    // Restore
    let state = session_store::restore(&store, sid, segid).unwrap();

    assert_eq!(state.session_id, Some(sid));
    assert_eq!(state.history.len(), original_history_len);
    assert_eq!(state.turn_count, original_turn_count);
    assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
    assert_eq!(
        state.entries_count,
        store.read_entry_count(sid, segid).unwrap()
    );

    // Shim by segment ID alone.
    let by_segment = session_store::restore_by_segment(&store, segid).unwrap();
    assert_eq!(by_segment.session_id, Some(sid));
}

#[tokio::test]
async fn session_run_with_tool_call() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = TestWorker::new(Engine::new(client));
    worker.register_tool(weather_tool_definition());

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (_worker, _) = run_and_persist(worker, &store, sid, segid, "What's the weather?").await;

    let entries = store.read_all(sid, segid).unwrap();

    let has_tool_results = entries
        .iter()
        .any(|e| matches!(e, LogEntry::AnnotatedToolResult { .. }));
    assert!(has_tool_results, "should have ToolResult entry");

    let has_assistant = entries
        .iter()
        .any(|e| matches!(e, LogEntry::AnnotatedAssistantItem { .. }));
    assert!(has_assistant, "should have AssistantItem entry");
}

#[tokio::test]
async fn session_resume_after_pause() {
    let (_dir, store) = make_store();

    // First run: tool call with pause policy → Paused
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = TestWorker::new(Engine::new(client));
    worker.register_tool(weather_tool_definition());
    worker.set_interceptor(PausePolicy);

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (_worker, result) = run_and_persist(worker, &store, sid, segid, "Weather?").await;
    assert!(matches!(result, agen::EngineRunExit::Paused));

    // Check RunCompleted is Paused
    let entries = store.read_all(sid, segid).unwrap();
    let has_paused = entries.iter().any(|e| {
        matches!(
            e,
            LogEntry::RunCompleted {
                result: agen::EngineResult::Paused,
                ..
            }
        )
    });
    assert!(has_paused, "should have Paused outcome");

    // Restore state and verify
    let state = session_store::restore(&store, sid, segid).unwrap();
    assert!(state.last_run_interrupted);
    assert_eq!(state.active_run_turn_count, Some(2));
}

#[tokio::test]
async fn session_fork_creates_new_session() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = TestWorker::new(Engine::new(client));
    worker.set_system_prompt("System prompt");

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, segid, "Hello").await;

    let original_history_len = worker.history().len();
    let (fork_sid, fork_segid) = session_store::fork(
        &store,
        sid,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();
    assert_ne!(fork_sid, sid, "`fork` mints a fresh Session");

    // Fork should have a SegmentStart with the current history
    let fork_entries = store.read_all(fork_sid, fork_segid).unwrap();
    assert_eq!(fork_entries.len(), 1);
    assert!(matches!(
        &fork_entries[0],
        LogEntry::AnnotatedSegmentStart { .. }
    ));

    let fork_state = collect_state(&fork_entries);
    assert_eq!(fork_state.session_id, Some(fork_sid));
    assert_eq!(fork_state.history.len(), original_history_len);
    assert_eq!(fork_state.system_prompt.as_deref(), Some("System prompt"));
}

#[tokio::test]
async fn session_fork_at_truncates_within_session() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let worker = TestWorker::new(Engine::new(client));

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, segid, "Hello").await;

    let all_entries = store.read_all(sid, segid).unwrap();
    assert!(all_entries.len() > 2);

    // Fork at turn 1 (one completed turn). Stays in same Session.
    let fork_segid = session_store::fork_at(&store, sid, segid, worker.turn_count()).unwrap();

    let fork_entries = store.read_all(sid, fork_segid).unwrap();
    assert_eq!(fork_entries.len(), 1); // Just the new SegmentStart

    let fork_state = collect_state(&fork_entries);
    assert_eq!(fork_state.session_id, Some(sid), "fork_at inherits Session");

    // History at fork point should match history right after the TurnEnd in
    // the source segment.
    let turn_end_pos = all_entries
        .iter()
        .position(|e| matches!(e, LogEntry::TurnEnd { turn_count, .. } if *turn_count == worker.turn_count()))
        .expect("source segment has the matching TurnEnd");
    let source_state_at_fork = collect_state(&all_entries[..=turn_end_pos]);
    assert_eq!(fork_state.history.len(), source_state_at_fork.history.len());
    assert_eq!(
        fork_state.annotated_history, source_state_at_fork.annotated_history,
        "fork_at must preserve every retained history entry identity and provenance",
    );
    assert!(fork_state.annotated_history.iter().all(|entry| {
        !entry.metadata.entry_id.0.is_empty()
            && matches!(
                entry.metadata.origin,
                session_store::LoggedSessionHistoryOrigin::LegacyUnknown
                    | session_store::LoggedSessionHistoryOrigin::HumanInput { .. }
                    | session_store::LoggedSessionHistoryOrigin::WorkerInput { .. }
                    | session_store::LoggedSessionHistoryOrigin::BackendInstruction { .. }
                    | session_store::LoggedSessionHistoryOrigin::ModelOutput { .. }
                    | session_store::LoggedSessionHistoryOrigin::ToolOutput { .. }
                    | session_store::LoggedSessionHistoryOrigin::DerivedSummary
            )
    }));

    // list_segments should show both source and fork in the same Session.
    let segs = store.list_segments(sid).unwrap();
    assert!(segs.contains(&segid));
    assert!(segs.contains(&fork_segid));
}

#[tokio::test]
async fn session_config_changed_logged() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(vec![]);
    let mut worker = TestWorker::new(Engine::new(client));

    let (sid, segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    // Modify config and log it
    let new_config = RequestConfig::default().with_temperature(0.7);
    worker.set_request_config(new_config.clone());
    session_store::save_config_changed(&store, sid, segid, &new_config).unwrap();

    let entries = store.read_all(sid, segid).unwrap();
    let has_config_changed = entries.iter().any(|e| {
        matches!(
            e,
            LogEntry::ConfigChanged { config, .. } if config.temperature == Some(0.7)
        )
    });
    assert!(has_config_changed, "should have ConfigChanged entry");
}

#[tokio::test]
async fn session_auto_forks_on_conflict() {
    let (_dir, store) = make_store();

    // Create a segment
    let client_a = MockLlmClient::new(simple_text_events());
    let worker_a = TestWorker::new(Engine::new(client_a));

    let (sid, original_segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: annotated(&worker_a.history()),
        },
    )
    .unwrap();
    let mut segment_id = original_segid;
    // Writer tracked: just the SegmentStart we wrote.
    let mut entries_written: usize = 1;

    // Simulate another Worker writing to the same segment behind our back.
    session_store::save_user_input(
        &store,
        sid,
        original_segid,
        vec![protocol::Segment::text("Interloper")],
        annotated(&[Item::user_message("Interloper")]),
    )
    .unwrap();

    // Now the on-disk count exceeds our tally — ensure_head_or_fork should auto-fork.
    session_store::ensure_head_or_fork(
        &store,
        sid,
        &mut segment_id,
        &mut entries_written,
        /* at_turn_index */ 0,
        SegmentStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: annotated(&worker_a.history()),
        },
    )
    .unwrap();

    // segment_id should now be different but live in the same Session.
    assert_ne!(segment_id, original_segid);

    // The fork segment should exist and have entries
    let fork_entries = store.read_all(sid, segment_id).unwrap();
    assert!(!fork_entries.is_empty());
    let fork_state = collect_state(&fork_entries);
    assert_eq!(
        fork_state.session_id,
        Some(sid),
        "auto-fork inherits Session"
    );

    // The new segment records its lineage forward via forked_from; the
    // source segment is left immutable (no terminal marker written back).
    match &fork_entries[0] {
        LogEntry::AnnotatedSegmentStart {
            forked_from: Some(origin),
            ..
        } => {
            assert_eq!(origin.segment_id, original_segid);
            assert_eq!(origin.at_turn_index, 0);
        }
        other => panic!("expected SegmentStart with forked_from, got {other:?}"),
    }

    // Original segment should still have the interloper entry and NO
    // terminal fork marker — it is byte-for-byte unchanged.
    let original_entries = store.read_all(sid, original_segid).unwrap();
    assert_eq!(
        original_entries.len(),
        2,
        "source segment holds only SegmentStart + interloper UserInput"
    );
    let has_interloper = original_entries
        .iter()
        .any(|e| matches!(e, LogEntry::AnnotatedUserInput { .. }));
    assert!(has_interloper);
}

/// Nested past-fork: forking a segment that is itself a fork must not
/// require touching any ancestor. Each `fork_at` only reads its direct
/// source and seeds a new segment, so a chain of forks composes cleanly.
#[tokio::test]
async fn nested_past_fork_leaves_ancestors_immutable() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let worker = TestWorker::new(Engine::new(client));

    let (sid, root_segid) = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: annotated(&worker.history()),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, root_segid, "Hello").await;
    let root_before = store.read_all(sid, root_segid).unwrap();

    // First past-fork at the completed turn.
    let fork1 = session_store::fork_at(&store, sid, root_segid, worker.turn_count()).unwrap();
    // Fork the fork (turn 0 = right after its SegmentStart seed).
    let fork2 = session_store::fork_at(&store, sid, fork1, 0).unwrap();

    // All three are distinct, all in the same Session.
    assert_ne!(fork1, root_segid);
    assert_ne!(fork2, fork1);
    for seg in [root_segid, fork1, fork2] {
        assert_eq!(
            collect_state(&store.read_all(sid, seg).unwrap()).session_id,
            Some(sid)
        );
    }

    // The root and fork1 are untouched by forking their descendants.
    assert_eq!(
        store.read_all(sid, root_segid).unwrap().len(),
        root_before.len()
    );
    let fork1_entries = store.read_all(sid, fork1).unwrap();
    assert_eq!(
        fork1_entries.len(),
        1,
        "fork1 is just its SegmentStart seed"
    );

    // fork2's lineage points at fork1, not the root.
    match &store.read_all(sid, fork2).unwrap()[0] {
        LogEntry::AnnotatedSegmentStart {
            forked_from: Some(origin),
            ..
        } => assert_eq!(origin.segment_id, fork1),
        other => panic!("expected SegmentStart with forked_from, got {other:?}"),
    }
}
