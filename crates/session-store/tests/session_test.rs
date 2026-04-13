mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::interceptor::{Interceptor, TurnEndAction};
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::llm_client::types::{Item, RequestConfig};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use session_store::{
    EntryHash, FsStore, LogEntry, Outcome, SessionStartState, Store, collect_state,
};

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

async fn make_store() -> (tempfile::TempDir, FsStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    (dir, store)
}

/// Run a worker turn and persist via session-store functions.
/// Takes ownership of the worker (needed for lock/unlock) and returns it.
async fn run_and_persist(
    worker: Worker<MockLlmClient>,
    store: &FsStore,
    session_id: session_store::SessionId,
    head_hash: &mut Option<EntryHash>,
    input: &str,
) -> (Worker<MockLlmClient>, llm_worker::WorkerResult) {
    let history_before = worker.history().len();

    let mut locked = worker.lock();
    let result = locked.run(input).await;
    let worker = locked.unlock();

    let new_items = &worker.history()[history_before..];
    session_store::save_delta(store, session_id, head_hash, new_items)
        .await
        .unwrap();
    session_store::save_turn_end(store, session_id, head_hash, worker.turn_count())
        .await
        .unwrap();

    let outcome = match &result {
        Ok(llm_worker::WorkerResult::Finished) => Outcome::Finished,
        Ok(llm_worker::WorkerResult::Paused) => Outcome::Paused,
        Ok(llm_worker::WorkerResult::LimitReached) => Outcome::LimitReached,
        Ok(llm_worker::WorkerResult::Yielded) => Outcome::Yielded,
        Err(e) => Outcome::Error {
            message: e.to_string(),
        },
    };
    session_store::save_outcome(
        store,
        session_id,
        head_hash,
        outcome,
        worker.last_run_interrupted(),
    )
    .await
    .unwrap();

    let r = result.unwrap();
    (worker, r)
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn session_run_logs_entries() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();

    let mut head_hash = Some(head_hash);
    let (worker, _) = run_and_persist(worker, &store, sid, &mut head_hash, "Hi").await;
    let _ = &worker;

    let entries = store.read_all(sid).await.unwrap();

    // SessionStart, UserInput, AssistantItems, TurnEnd, RunOutcome (at minimum)
    assert!(
        entries.len() >= 4,
        "expected at least 4 entries, got {}",
        entries.len()
    );

    // First entry is SessionStart
    assert!(matches!(&entries[0].entry, LogEntry::SessionStart { .. }));

    // Has a RunOutcome with Finished
    let has_finished = entries.iter().any(|e| {
        matches!(
            &e.entry,
            LogEntry::RunOutcome {
                outcome: Outcome::Finished,
                ..
            }
        )
    });
    assert!(has_finished, "should have a Finished outcome");

    // Verify hash chain integrity
    assert!(entries[0].prev_hash.is_none());
    for i in 1..entries.len() {
        assert_eq!(
            entries[i].prev_hash.as_ref(),
            Some(&entries[i - 1].hash),
            "hash chain broken at entry {}",
            i
        );
    }
}

#[tokio::test]
async fn session_restore_round_trip() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = Worker::new(client);
    worker.set_system_prompt("You are helpful.");

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    let (worker, _) = run_and_persist(worker, &store, sid, &mut head_hash, "Hi").await;

    let original_history_len = worker.history().len();
    let original_turn_count = worker.turn_count();

    // Restore
    let state = session_store::restore(&store, sid).await.unwrap();

    assert_eq!(state.history.len(), original_history_len);
    assert_eq!(state.turn_count, original_turn_count);
    assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
    assert_eq!(state.head_hash, head_hash);
}

#[tokio::test]
async fn session_run_with_tool_call() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition());

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    let (_worker, _) =
        run_and_persist(worker, &store, sid, &mut head_hash, "What's the weather?").await;

    let entries = store.read_all(sid).await.unwrap();

    let has_tool_results = entries
        .iter()
        .any(|e| matches!(&e.entry, LogEntry::ToolResults { .. }));
    assert!(has_tool_results, "should have ToolResults entry");

    let has_assistant = entries
        .iter()
        .any(|e| matches!(&e.entry, LogEntry::AssistantItems { .. }));
    assert!(has_assistant, "should have AssistantItems entry");
}

#[tokio::test]
async fn session_resume_after_pause() {
    let (_dir, store) = make_store().await;

    // First run: tool call with pause policy → Paused
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition());
    worker.set_interceptor(PausePolicy);

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    let (_worker, result) = run_and_persist(worker, &store, sid, &mut head_hash, "Weather?").await;
    assert!(matches!(result, llm_worker::WorkerResult::Paused));

    // Check RunOutcome is Paused
    let entries = store.read_all(sid).await.unwrap();
    let has_paused = entries.iter().any(|e| {
        matches!(
            &e.entry,
            LogEntry::RunOutcome {
                outcome: Outcome::Paused,
                ..
            }
        )
    });
    assert!(has_paused, "should have Paused outcome");

    // Restore state and verify
    let state = session_store::restore(&store, sid).await.unwrap();
    assert!(state.last_run_interrupted);
}

#[tokio::test]
async fn session_fork_preserves_state() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = Worker::new(client);
    worker.set_system_prompt("System prompt");

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    let (worker, _) = run_and_persist(worker, &store, sid, &mut head_hash, "Hello").await;

    let original_history_len = worker.history().len();
    let fork_id = session_store::fork(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();

    // Fork should have a SessionStart with the current history
    let fork_entries = store.read_all(fork_id).await.unwrap();
    assert_eq!(fork_entries.len(), 1);
    assert!(matches!(
        &fork_entries[0].entry,
        LogEntry::SessionStart { .. }
    ));

    let fork_state = collect_state(&fork_entries);
    assert_eq!(fork_state.history.len(), original_history_len);
    assert_eq!(fork_state.system_prompt.as_deref(), Some("System prompt"));
}

#[tokio::test]
async fn session_fork_at_truncates() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    let (_worker, _) = run_and_persist(worker, &store, sid, &mut head_hash, "Hello").await;

    let all_entries = store.read_all(sid).await.unwrap();
    assert!(all_entries.len() > 2);

    // Fork at the hash of the 2nd entry (SessionStart + UserInput)
    let at_hash = &all_entries[1].hash;
    let fork_id = session_store::fork_at(&store, sid, at_hash).await.unwrap();

    let fork_entries = store.read_all(fork_id).await.unwrap();
    assert_eq!(fork_entries.len(), 1); // Just the new SessionStart

    let fork_state = collect_state(&fork_entries);
    // Should have the state from replaying only the first 2 entries
    let original_truncated_state = collect_state(&all_entries[..2]);
    assert_eq!(
        fork_state.history.len(),
        original_truncated_state.history.len()
    );
}

#[tokio::test]
async fn session_config_changed_logged() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    // Modify config and log it
    let new_config = RequestConfig::default().with_temperature(0.7);
    worker.set_request_config(new_config.clone());
    session_store::save_config_changed(&store, sid, &mut head_hash, &new_config)
        .await
        .unwrap();

    let entries = store.read_all(sid).await.unwrap();
    let has_config_changed = entries.iter().any(|e| {
        matches!(
            &e.entry,
            LogEntry::ConfigChanged { config, .. } if config.temperature == Some(0.7)
        )
    });
    assert!(has_config_changed, "should have ConfigChanged entry");
}

#[tokio::test]
async fn session_cache_lock_unlock_logged() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(vec![]);
    let worker = Worker::new(client);

    let (sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker.get_system_prompt(),
            config: worker.request_config(),
            history: worker.history(),
        },
    )
    .await
    .unwrap();
    let mut head_hash = Some(head_hash);

    session_store::save_cache_locked(&store, sid, &mut head_hash, 5)
        .await
        .unwrap();
    session_store::save_cache_unlocked(&store, sid, &mut head_hash)
        .await
        .unwrap();

    let entries = store.read_all(sid).await.unwrap();

    let has_locked = entries.iter().any(|e| {
        matches!(
            &e.entry,
            LogEntry::Locked {
                locked_prefix_len: 5,
                ..
            }
        )
    });
    assert!(has_locked, "should have Locked entry");

    let has_unlocked = entries
        .iter()
        .any(|e| matches!(&e.entry, LogEntry::CacheUnlocked { .. }));
    assert!(has_unlocked, "should have CacheUnlocked entry");

    // State after all entries: unlocked
    let state = collect_state(&entries);
    assert_eq!(state.locked_prefix_len, 0);
}

#[tokio::test]
async fn session_auto_forks_on_conflict() {
    let (_dir, store) = make_store().await;

    // Create a session
    let client_a = MockLlmClient::new(simple_text_events());
    let worker_a = Worker::new(client_a);

    let (original_sid, head_hash) = session_store::create_session(
        &store,
        SessionStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: worker_a.history(),
        },
    )
    .await
    .unwrap();
    let mut session_id = original_sid;
    let mut head_hash = Some(head_hash);

    // Simulate another Pod writing to the same session behind our back
    let extra_entry = LogEntry::UserInput {
        ts: 9999,
        item: Item::user_message("Interloper"),
    };
    let current_head = store.read_head_hash(original_sid).await.unwrap();
    let hash = session_store::compute_hash(current_head.as_ref(), &extra_entry);
    let hashed = session_store::HashedEntry {
        hash,
        prev_hash: current_head,
        entry: extra_entry,
    };
    store.append(original_sid, &hashed).await.unwrap();

    // Now head_hash is stale — ensure_head_or_fork should auto-fork
    session_store::ensure_head_or_fork(
        &store,
        &mut session_id,
        &mut head_hash,
        SessionStartState {
            system_prompt: worker_a.get_system_prompt(),
            config: worker_a.request_config(),
            history: worker_a.history(),
        },
    )
    .await
    .unwrap();

    // session_id should now be different
    assert_ne!(session_id, original_sid);

    // The fork session should exist and have entries
    let fork_entries = store.read_all(session_id).await.unwrap();
    assert!(!fork_entries.is_empty());

    // Original session should still have the interloper entry
    let original_entries = store.read_all(original_sid).await.unwrap();
    let has_interloper = original_entries.iter().any(|e| {
        if let LogEntry::UserInput { item, .. } = &e.entry {
            item.is_user_message()
        } else {
            false
        }
    });
    assert!(has_interloper);
}
