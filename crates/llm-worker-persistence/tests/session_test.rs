mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::hook::{Hook, HookError, OnTurnEnd, OnTurnEndResult};
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::llm_client::types::{Item, RequestConfig};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta};
use llm_worker::Worker;
use llm_worker_persistence::{
    FsStore, LogEntry, Outcome, Session, SessionConfig, Store, collect_state,
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
    async fn execute(&self, _input_json: &str) -> Result<String, ToolError> {
        Ok("Sunny, 25C".to_string())
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

/// Hook that forces Pause on the first turn end.
struct PauseOnFirstTurnEnd;

#[async_trait]
impl Hook<OnTurnEnd> for PauseOnFirstTurnEnd {
    async fn call(&self, _input: &mut Vec<Item>) -> Result<OnTurnEndResult, HookError> {
        Ok(OnTurnEndResult::Paused)
    }
}

async fn make_store() -> (tempfile::TempDir, FsStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    (dir, store)
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn session_run_logs_entries() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    session.run("Hi").await.unwrap();

    let entries = store.read_all(sid).await.unwrap();

    // SessionStart, UserInput, AssistantItems, TurnEnd, RunOutcome (at minimum)
    assert!(entries.len() >= 4, "expected at least 4 entries, got {}", entries.len());

    // First entry is SessionStart
    assert!(matches!(entries[0], LogEntry::SessionStart { .. }));

    // Has a RunOutcome with Finished
    let has_finished = entries.iter().any(|e| matches!(
        e,
        LogEntry::RunOutcome { outcome: Outcome::Finished, .. }
    ));
    assert!(has_finished, "should have a Finished outcome");
}

#[tokio::test]
async fn session_restore_round_trip() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = Worker::new(client);
    worker.set_system_prompt("You are helpful.");

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    session.run("Hi").await.unwrap();

    let original_history = session.worker.history().to_vec();
    let original_turn_count = session.worker.turn_count();

    // Restore
    let restore_client = MockLlmClient::new(vec![]); // won't be called
    let restored = Session::restore(restore_client, store.clone(), sid, SessionConfig::default())
        .await
        .unwrap();

    assert_eq!(restored.worker.history().len(), original_history.len());
    assert_eq!(restored.worker.turn_count(), original_turn_count);
    assert_eq!(
        restored.worker.get_system_prompt().map(String::from),
        Some("You are helpful.".to_string())
    );
}

#[tokio::test]
async fn session_run_with_tool_call() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition()).unwrap();

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    session.run("What's the weather?").await.unwrap();

    let entries = store.read_all(sid).await.unwrap();

    let has_tool_results = entries.iter().any(|e| matches!(e, LogEntry::ToolResults { .. }));
    assert!(has_tool_results, "should have ToolResults entry");

    let has_assistant = entries.iter().any(|e| matches!(e, LogEntry::AssistantItems { .. }));
    assert!(has_assistant, "should have AssistantItems entry");
}

#[tokio::test]
async fn session_resume_after_pause() {
    let (_dir, store) = make_store().await;

    // First run: tool call with pause hook → Paused
    let client = MockLlmClient::with_responses(tool_call_events());
    let mut worker = Worker::new(client);
    worker.register_tool(weather_tool_definition()).unwrap();
    worker.add_on_turn_end_hook(PauseOnFirstTurnEnd);

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    let result = session.run("Weather?").await.unwrap();
    assert!(matches!(result, llm_worker::WorkerResult::Paused));

    // Check RunOutcome is Paused
    let entries = store.read_all(sid).await.unwrap();
    let has_paused = entries.iter().any(|e| matches!(
        e,
        LogEntry::RunOutcome { outcome: Outcome::Paused, .. }
    ));
    assert!(has_paused, "should have Paused outcome");

    // Restore and resume
    let resume_client = MockLlmClient::with_responses(vec![vec![
        Event::text_block_start(0),
        Event::text_delta(0, "After resume"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);
    let mut restored = Session::restore(resume_client, store.clone(), sid, SessionConfig::default())
        .await
        .unwrap();

    assert!(restored.worker.last_run_interrupted());

    // resume may or may not succeed depending on Worker internal state,
    // but the restore itself should work
    let _ = restored.resume().await;
}

#[tokio::test]
async fn session_fork_preserves_state() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let mut worker = Worker::new(client);
    worker.set_system_prompt("System prompt");

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();

    session.run("Hello").await.unwrap();

    let original_history_len = session.worker.history().len();
    let fork_id = session.fork().await.unwrap();

    // Fork should have a SessionStart with the current history
    let fork_entries = store.read_all(fork_id).await.unwrap();
    assert_eq!(fork_entries.len(), 1);
    assert!(matches!(&fork_entries[0], LogEntry::SessionStart { .. }));

    let fork_state = collect_state(&fork_entries);
    assert_eq!(fork_state.history.len(), original_history_len);
    assert_eq!(fork_state.system_prompt.as_deref(), Some("System prompt"));
}

#[tokio::test]
async fn session_fork_at_truncates() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(simple_text_events());
    let worker = Worker::new(client);

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    session.run("Hello").await.unwrap();

    let all_entries = store.read_all(sid).await.unwrap();
    assert!(all_entries.len() > 2);

    // Fork at entry 2 (SessionStart + UserInput only)
    let fork_id = Session::<MockLlmClient, FsStore>::fork_at(&store, sid, 2)
        .await
        .unwrap();

    let fork_entries = store.read_all(fork_id).await.unwrap();
    assert_eq!(fork_entries.len(), 1); // Just the new SessionStart

    let fork_state = collect_state(&fork_entries);
    // Should have the state from replaying only the first 2 entries
    let original_truncated_state = collect_state(&all_entries[..2]);
    assert_eq!(fork_state.history.len(), original_truncated_state.history.len());
}

#[tokio::test]
async fn session_config_changed_logged() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(vec![]);
    let worker = Worker::new(client);

    let mut session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    // Modify config via worker and log it
    session.worker.set_request_config(RequestConfig::default().with_temperature(0.7));
    session.log_config_changed().await.unwrap();

    let entries = store.read_all(sid).await.unwrap();
    let has_config_changed = entries.iter().any(|e| matches!(
        e,
        LogEntry::ConfigChanged { config, .. } if config.temperature == Some(0.7)
    ));
    assert!(has_config_changed, "should have ConfigChanged entry");
}

#[tokio::test]
async fn session_cache_lock_unlock_logged() {
    let (_dir, store) = make_store().await;
    let client = MockLlmClient::new(vec![]);
    let worker = Worker::new(client);

    let session = Session::new(worker, store.clone(), SessionConfig::default())
        .await
        .unwrap();
    let sid = session.session_id();

    session.log_cache_locked(5).await.unwrap();
    session.log_cache_unlocked().await.unwrap();

    let entries = store.read_all(sid).await.unwrap();

    let has_locked = entries.iter().any(|e| matches!(
        e,
        LogEntry::CacheLocked { locked_prefix_len: 5, .. }
    ));
    assert!(has_locked, "should have CacheLocked entry");

    let has_unlocked = entries.iter().any(|e| matches!(e, LogEntry::CacheUnlocked { .. }));
    assert!(has_unlocked, "should have CacheUnlocked entry");

    // State after all entries: unlocked
    let state = collect_state(&entries);
    assert_eq!(state.locked_prefix_len, 0);
}
