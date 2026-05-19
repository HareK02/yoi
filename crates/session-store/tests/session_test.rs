mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::interceptor::{Interceptor, TurnEndAction};
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::llm_client::types::{Item, RequestConfig};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use session_store::{FsStore, LogEntry, SegmentStartState, Store, collect_state};

// =============================================================================
// Helpers
// =============================================================================

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
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
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

/// Run a worker turn and persist via session-store functions.
/// Takes ownership of the worker (needed for lock/unlock) and returns it.
async fn run_and_persist(
    worker: Worker<MockLlmClient>,
    store: &FsStore,
    segment_id: session_store::SegmentId,
    input: &str,
) -> (Worker<MockLlmClient>, llm_worker::WorkerResult) {
    // Mirror Pod's run-entry contract: log the user input as segments
    // before the worker pushes its flattened user_message; save_delta
    // skips the resulting user_message item to avoid double-write.
    session_store::save_user_input(store, segment_id, vec![protocol::Segment::text(input)])
        .unwrap();

    let history_before = worker.history().len();

    let mut locked = worker.lock();
    let result = locked.run(input).await;
    let worker = locked.unlock();

    let new_items = &worker.history()[history_before..];
    session_store::save_delta(store, segment_id, new_items).unwrap();
    session_store::save_turn_end(store, segment_id, worker.turn_count()).unwrap();

    match &result {
        Ok(r) => {
            session_store::save_run_completed(
                store,
                segment_id,
                r.clone(),
                worker.last_run_interrupted(),
            )
            .unwrap();
        }
        Err(e) => {
            session_store::save_run_errored(
                store,
                segment_id,
                e.to_string(),
                worker.last_run_interrupted(),
            )
            .unwrap();
        }
    }

    let r = result.unwrap();
    (worker, r)
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn session_run_logs_entries() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, "Hi").await;
    let _ = &worker;

    let entries = store.read_all(sid).unwrap();

    // SegmentStart, UserInput, AssistantItems, TurnEnd, RunCompleted (at minimum)
    assert!(
        entries.len() >= 4,
        "expected at least 4 entries, got {}",
        entries.len()
    );

    // First entry is SegmentStart
    assert!(matches!(&entries[0], LogEntry::SegmentStart { .. }));

    // Has a RunCompleted with Finished
    let has_finished = entries.iter().any(|e| {
        matches!(
            e,
            LogEntry::RunCompleted {
                result: llm_worker::WorkerResult::Finished,
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
    let mut worker = Worker::new(client);
    worker.set_system_prompt("You are helpful.");

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, "Hi").await;

    let original_history_len = worker.history().len();
    let original_turn_count = worker.turn_count();

    // Restore
    let state = session_store::restore(&store, sid).unwrap();

    assert_eq!(state.history.len(), original_history_len);
    assert_eq!(state.turn_count, original_turn_count);
    assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
    assert_eq!(state.entries_count, store.read_entry_count(sid).unwrap());
}

#[tokio::test]
async fn session_run_with_tool_call() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition());

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (_worker, _) = run_and_persist(worker, &store, sid, "What's the weather?").await;

    let entries = store.read_all(sid).unwrap();

    let has_tool_results = entries
        .iter()
        .any(|e| matches!(e, LogEntry::ToolResult { .. }));
    assert!(has_tool_results, "should have ToolResult entry");

    let has_assistant = entries
        .iter()
        .any(|e| matches!(e, LogEntry::AssistantItem { .. }));
    assert!(has_assistant, "should have AssistantItem entry");
}

#[tokio::test]
async fn session_resume_after_pause() {
    let (_dir, store) = make_store();

    // First run: tool call with pause policy → Paused
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition());
    worker.set_interceptor(PausePolicy);

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (_worker, result) = run_and_persist(worker, &store, sid, "Weather?").await;
    assert!(matches!(result, llm_worker::WorkerResult::Paused));

    // Check RunCompleted is Paused
    let entries = store.read_all(sid).unwrap();
    let has_paused = entries.iter().any(|e| {
        matches!(
            e,
            LogEntry::RunCompleted {
                result: llm_worker::WorkerResult::Paused,
                ..
            }
        )
    });
    assert!(has_paused, "should have Paused outcome");

    // Restore state and verify
    let state = session_store::restore(&store, sid).unwrap();
    assert!(state.last_run_interrupted);
}

#[tokio::test]
async fn session_fork_preserves_state() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = Worker::new(client);
    worker.set_system_prompt("System prompt");

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, "Hello").await;

    let original_history_len = worker.history().len();
    let fork_id = session_store::fork(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    // Fork should have a SegmentStart with the current history
    let fork_entries = store.read_all(fork_id).unwrap();
    assert_eq!(fork_entries.len(), 1);
    assert!(matches!(&fork_entries[0], LogEntry::SegmentStart { .. }));

    let fork_state = collect_state(&fork_entries);
    assert_eq!(fork_state.history.len(), original_history_len);
    assert_eq!(fork_state.system_prompt.as_deref(), Some("System prompt"));
}

#[tokio::test]
async fn session_fork_at_truncates() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    let (worker, _) = run_and_persist(worker, &store, sid, "Hello").await;

    let all_entries = store.read_all(sid).unwrap();
    assert!(all_entries.len() > 2);

    // Fork at turn 1 (one completed turn).
    let fork_id = session_store::fork_at(&store, sid, worker.turn_count()).unwrap();

    let fork_entries = store.read_all(fork_id).unwrap();
    assert_eq!(fork_entries.len(), 1); // Just the new SegmentStart

    let fork_state = collect_state(&fork_entries);
    // History at fork point should match history right after the TurnEnd in
    // the source session.
    let turn_end_pos = all_entries
        .iter()
        .position(|e| matches!(e, LogEntry::TurnEnd { turn_count, .. } if *turn_count == worker.turn_count()))
        .expect("source session has the matching TurnEnd");
    let source_state_at_fork = collect_state(&all_entries[..=turn_end_pos]);
    assert_eq!(fork_state.history.len(), source_state_at_fork.history.len());
}

#[tokio::test]
async fn session_config_changed_logged() {
    let (_dir, store) = make_store();
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    let sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .unwrap();

    // Modify config and log it
    let new_config = RequestConfig::default().with_temperature(0.7);
    worker.set_request_config(new_config.clone());
    session_store::save_config_changed(&store, sid, &new_config).unwrap();

    let entries = store.read_all(sid).unwrap();
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

    // Create a session
    let client_a = MockLlmClient::new(simple_text_events());
    let worker_a = Worker::new(client_a);

    let original_sid = session_store::create_segment(
        &store,
        SegmentStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: worker_a.history(),
        },
    )
    .unwrap();
    let mut segment_id = original_sid;
    // Writer tracked: just the SegmentStart we wrote.
    let mut entries_written: usize = 1;

    // Simulate another Pod writing to the same session behind our back.
    let extra_entry = LogEntry::UserInput {
        ts: 9999,
        segments: vec![protocol::Segment::text("Interloper")],
    };
    store.append(original_sid, &extra_entry).unwrap();

    // Now the on-disk count exceeds our tally — ensure_head_or_fork should auto-fork.
    session_store::ensure_head_or_fork(
        &store,
        &mut segment_id,
        &mut entries_written,
        SegmentStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: worker_a.history(),
        },
    )
    .unwrap();

    // segment_id should now be different
    assert_ne!(segment_id, original_sid);

    // The fork session should exist and have entries
    let fork_entries = store.read_all(segment_id).unwrap();
    assert!(!fork_entries.is_empty());

    // Original session should still have the interloper entry
    let original_entries = store.read_all(original_sid).unwrap();
    let has_interloper = original_entries
        .iter()
        .any(|e| matches!(e, LogEntry::UserInput { .. }));
    assert!(has_interloper);
}
