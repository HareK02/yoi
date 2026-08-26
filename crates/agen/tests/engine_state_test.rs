//! Engine state management tests
//!
//! Tests for state transitions using the Type-state pattern (Mutable/Locked)
//! and state preservation between turns.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agen::Item;
use agen::interceptor::{
    Interceptor, PreRequestAction, PreToolAction, ToolCallInfo, TurnEndAction,
};
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use agen::{Engine, EngineError, EngineResult};
use async_trait::async_trait;
use common::MockLlmClient;

// =============================================================================
// Mutable State Tests
// =============================================================================

/// Verify that system prompt can be set in Mutable state
#[test]
fn test_mutable_set_system_prompt() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);

    assert!(engine.get_system_prompt().is_none());

    engine.set_system_prompt("You are a helpful assistant.");
    assert_eq!(
        engine.get_system_prompt(),
        Some("You are a helpful assistant.")
    );
}

/// Verify that history can be freely edited in Mutable state
#[test]
fn test_mutable_history_manipulation() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);

    // Initial state is empty
    assert!(engine.history().is_empty());

    // Add to history
    engine
        .append_history(vec![Item::user_message("Hello")])
        .unwrap();
    engine
        .append_history(vec![Item::assistant_message("Hi there!")])
        .unwrap();
    assert_eq!(engine.history().len(), 2);

    // Append to history via the callback-aware API.
    engine
        .append_history(vec![Item::user_message("How are you?")])
        .unwrap();
    assert_eq!(engine.history().len(), 3);

    // Clear history
    engine.clear_history();
    assert!(engine.history().is_empty());

    // Set history
    let items = vec![
        Item::user_message("Test"),
        Item::assistant_message("Response"),
    ];
    engine.set_history(items);
    assert_eq!(engine.history().len(), 2);
}

/// Verify that Engine can be constructed using builder pattern
#[test]
fn test_mutable_builder_pattern() {
    let client = MockLlmClient::new(vec![]);
    let engine = Engine::new(client).system_prompt("System prompt");

    assert_eq!(engine.get_system_prompt(), Some("System prompt"));
    assert!(engine.history().is_empty());
}

/// Verify that multiple items can be added with append_history and callbacks fire.
#[test]
fn test_mutable_append_history() {
    let client = MockLlmClient::new(vec![]);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_callback = Arc::clone(&observed);
    let mut engine = Engine::new(client);
    engine.on_history_append(move |item| {
        if let Some(text) = item.as_text() {
            observed_for_callback.lock().unwrap().push(text.to_string());
        }
        Ok(())
    });

    engine
        .append_history(vec![Item::user_message("First")])
        .unwrap();

    engine
        .append_history(vec![
            Item::assistant_message("Response 1"),
            Item::user_message("Second"),
            Item::assistant_message("Response 2"),
        ])
        .unwrap();

    assert_eq!(engine.history().len(), 4);
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        ["First", "Response 1", "Second", "Response 2"]
    );
}

#[derive(Clone)]
struct CountingTool {
    name: String,
    calls: Arc<AtomicUsize>,
}

impl CountingTool {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            (
                ToolMeta::new(&tool.name)
                    .description("Counting tool")
                    .input_schema(serde_json::json!({"type":"object","properties":{}})),
                Arc::new(tool.clone()) as Arc<dyn Tool>,
            )
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for CountingTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(format!("{}-ok", self.name).into())
    }
}

/// Verify that tools can be registered in Mutable state.
#[test]
fn test_mutable_can_register_tool() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);
    let tool = CountingTool::new("count_tool");

    // register_tool is infallible (factory deferred to run-time flush)
    engine.register_tool(tool.definition());
}

/// A durable-history failure on a tool call must stop the turn before the
/// tool can produce an external side effect.
#[tokio::test]
async fn history_append_failure_stops_before_tool_execution() {
    let client = MockLlmClient::new(vec![
        Event::tool_use_start(0, "call_1", "count_tool"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let tool = CountingTool::new("count_tool");
    let mut engine = Engine::new(client);
    engine.register_tool(tool.definition());
    engine.on_history_append(|item| {
        if item.is_tool_call() {
            Err("simulated ENOSPC".to_string())
        } else {
            Ok(())
        }
    });

    let mut engine = engine.lock();
    let error = engine.run("use the tool").await.unwrap_err();

    assert!(
        matches!(error, EngineError::HistoryAppend(ref message) if message == "simulated ENOSPC")
    );
    assert_eq!(tool.call_count(), 0);
    assert_eq!(engine.history().len(), 1);
    assert_eq!(engine.history()[0].as_text(), Some("use the tool"));
}

// =============================================================================
// State Transition Tests
// =============================================================================

/// Verify that lock() transitions from Mutable -> Locked state
#[test]
fn test_lock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);

    engine.set_system_prompt("System");
    engine
        .append_history(vec![Item::user_message("Hello")])
        .unwrap();
    engine
        .append_history(vec![Item::assistant_message("Hi")])
        .unwrap();

    // Lock
    let locked_engine = engine.lock();

    // History and system prompt are still accessible in Locked state
    assert_eq!(locked_engine.get_system_prompt(), Some("System"));
    assert_eq!(locked_engine.history().len(), 2);
    assert_eq!(locked_engine.locked_prefix_len(), 2);
}

/// Verify that unlock() transitions from Locked -> Mutable state
#[test]
fn test_unlock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);

    engine
        .append_history(vec![Item::user_message("Hello")])
        .unwrap();
    let locked_engine = engine.lock();

    // Unlock
    let mut engine = locked_engine.unlock();

    // History operations are available again in Mutable state
    engine
        .append_history(vec![Item::assistant_message("Hi")])
        .unwrap();
    engine.clear_history();
    assert!(engine.history().is_empty());
}

// =============================================================================
// Turn Execution and State Preservation Tests
// =============================================================================

/// Verify that history is correctly updated after running a turn in Mutable state
#[tokio::test]
async fn test_mutable_run_updates_history() -> Result<(), EngineError> {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello, I'm an assistant!"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let engine = Engine::new(client);

    // Execute (Mutable::run consumes self, returns EngineRunOutput)
    let out = engine.run("Hi there").await?;
    let engine = out.engine;

    // History is updated
    let history = engine.history();
    assert_eq!(history.len(), 2); // user + assistant

    // User message
    assert_eq!(history[0].as_text(), Some("Hi there"));

    // Assistant message
    assert_eq!(history[1].as_text(), Some("Hello, I'm an assistant!"));

    Ok(())
}

/// Verify that history accumulates correctly over multiple turns in Locked state
#[tokio::test]
async fn test_locked_multi_turn_history_accumulation() {
    // Prepare responses for 2 requests
    let client = MockLlmClient::with_responses(vec![
        // First response
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Nice to meet you!"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        // Second response
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "I can help with that."),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);

    let engine = Engine::new(client).system_prompt("You are helpful.");

    // Lock (after setting system prompt)
    let mut locked_engine = engine.lock();
    assert_eq!(locked_engine.locked_prefix_len(), 0); // No items yet

    // Turn 1
    let result1 = locked_engine.run("Hello!").await;
    assert!(result1.is_ok());
    assert_eq!(locked_engine.history().len(), 2); // user + assistant

    // Turn 2
    let result2 = locked_engine.run("Can you help me?").await;
    assert!(result2.is_ok());
    assert_eq!(locked_engine.history().len(), 4); // 2 * (user + assistant)

    // Verify history contents
    let history = locked_engine.history();

    // Turn 1 user message
    assert_eq!(history[0].as_text(), Some("Hello!"));

    // Turn 1 assistant message
    assert_eq!(history[1].as_text(), Some("Nice to meet you!"));

    // Turn 2 user message
    assert_eq!(history[2].as_text(), Some("Can you help me?"));

    // Turn 2 assistant message
    assert_eq!(history[3].as_text(), Some("I can help with that."));
}

/// Verify that locked_prefix_len correctly records history length at lock time
#[tokio::test]
async fn test_locked_prefix_len_tracking() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Response 1"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Response 2"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);

    let mut engine = Engine::new(client);

    // Add items beforehand
    engine
        .append_history(vec![Item::user_message("Pre-existing message 1")])
        .unwrap();
    engine
        .append_history(vec![Item::assistant_message("Pre-existing response 1")])
        .unwrap();

    assert_eq!(engine.history().len(), 2);

    // Lock
    let mut locked_engine = engine.lock();
    assert_eq!(locked_engine.locked_prefix_len(), 2); // 2 items at lock time

    // Execute turn
    locked_engine.run("New message").await.unwrap();

    // History grows but locked_prefix_len remains unchanged
    assert_eq!(locked_engine.history().len(), 4); // 2 + 2
    assert_eq!(locked_engine.locked_prefix_len(), 2); // Unchanged
}

/// Verify that turn count is correctly incremented
#[tokio::test]
async fn test_turn_count_increment() -> Result<(), EngineError> {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Turn 1"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Turn 2"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);

    let engine = Engine::new(client);

    assert_eq!(engine.turn_count(), 0);
    assert_eq!(engine.llm_call_count(), 0);

    // First run consumes Mutable, returns EngineRunOutput
    let mut engine = engine.run("First").await?.engine;
    assert_eq!(engine.turn_count(), 1);
    // Retry not yet implemented → AgentTurn:LlmCall is 1:1.
    assert_eq!(engine.llm_call_count(), 1);

    // Subsequent runs on Locked take &mut self
    engine.run("Second").await?;
    assert_eq!(engine.turn_count(), 2);
    assert_eq!(engine.llm_call_count(), 2);

    Ok(())
}

/// Verify that history can be edited after unlock and re-locked
#[tokio::test]
async fn test_unlock_edit_relock() {
    let client = MockLlmClient::with_responses(vec![vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Response"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);

    let mut engine = Engine::new(client);
    engine
        .append_history(vec![
            Item::user_message("Hello"),
            Item::assistant_message("Hi"),
        ])
        .unwrap();

    // Lock -> Unlock
    let locked = engine.lock();
    assert_eq!(locked.locked_prefix_len(), 2);

    let mut unlocked = locked.unlock();

    // Edit history
    unlocked.clear_history();
    unlocked
        .append_history(vec![Item::user_message("Fresh start")])
        .unwrap();

    // Re-lock
    let relocked = unlocked.lock();
    assert_eq!(relocked.history().len(), 1);
    assert_eq!(relocked.locked_prefix_len(), 1);
}

/// Verify that tools registered before lock and after unlock remain effective.
#[tokio::test]
async fn test_lock_unlock_relock_tools_remain_effective() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_1", "tool_a"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "done-a"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::tool_use_start(0, "call_2", "tool_b"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "done-b"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);

    let mut engine = Engine::new(client);
    let tool_a = CountingTool::new("tool_a");
    engine.register_tool(tool_a.definition());

    let mut locked = engine.lock();
    locked.run("first").await.expect("first run");
    assert_eq!(tool_a.call_count(), 1, "tool_a should be called once");

    let mut unlocked = locked.unlock();
    let tool_b = CountingTool::new("tool_b");
    unlocked.register_tool(tool_b.definition());

    let mut relocked = unlocked.lock();
    relocked.run("second").await.expect("second run");

    assert_eq!(tool_a.call_count(), 1, "tool_a should not be called again");
    assert_eq!(tool_b.call_count(), 1, "tool_b should be called once");
}

// =============================================================================
// System Prompt Preservation Tests
// =============================================================================

/// Verify that system prompt is preserved in Locked state
#[test]
fn test_system_prompt_preserved_in_locked_state() {
    let client = MockLlmClient::new(vec![]);
    let engine = Engine::new(client).system_prompt("Important system prompt");

    let locked = engine.lock();
    assert_eq!(locked.get_system_prompt(), Some("Important system prompt"));

    let unlocked = locked.unlock();
    assert_eq!(
        unlocked.get_system_prompt(),
        Some("Important system prompt")
    );
}

/// Verify that system prompt can be changed after unlock -> re-lock
#[test]
fn test_system_prompt_change_after_unlock() {
    let client = MockLlmClient::new(vec![]);
    let engine = Engine::new(client).system_prompt("Original prompt");

    let locked = engine.lock();
    let mut unlocked = locked.unlock();

    unlocked.set_system_prompt("New prompt");
    assert_eq!(unlocked.get_system_prompt(), Some("New prompt"));

    let relocked = unlocked.lock();
    assert_eq!(relocked.get_system_prompt(), Some("New prompt"));
}

fn completed_text_events() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "done"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

struct YieldOnce {
    calls: AtomicUsize,
}

#[async_trait]
impl Interceptor for YieldOnce {
    async fn pre_llm_request(&self, _context: &mut Vec<Item>) -> PreRequestAction {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            PreRequestAction::Yield
        } else {
            PreRequestAction::Continue
        }
    }
}

struct PauseToolOnce {
    calls: AtomicUsize,
}

#[async_trait]
impl Interceptor for PauseToolOnce {
    async fn pre_tool_call(&self, _info: &mut ToolCallInfo) -> PreToolAction {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            PreToolAction::Pause
        } else {
            PreToolAction::Continue
        }
    }
}

struct ContinueTurnOnce {
    calls: AtomicUsize,
}

#[async_trait]
impl Interceptor for ContinueTurnOnce {
    async fn on_turn_end(&self, _history: &[Item]) -> TurnEndAction {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            TurnEndAction::ContinueWithMessages(vec![Item::system_message("continue")])
        } else {
            TurnEndAction::Finish
        }
    }
}

#[tokio::test]
async fn max_turns_is_scoped_to_each_fresh_run() {
    let responses = vec![completed_text_events(), completed_text_events()];
    let mut engine = Engine::new(MockLlmClient::with_responses(responses));
    engine.set_max_turns(Some(1));
    let mut engine = engine.lock();

    assert_eq!(engine.run("first").await.unwrap(), EngineResult::Finished);
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);

    assert_eq!(engine.run("second").await.unwrap(), EngineResult::Finished);
    assert_eq!(engine.turn_count(), 2);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn yielded_resume_keeps_the_same_unspent_turn_budget() {
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_interceptor(YieldOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock();

    assert_eq!(engine.run("start").await.unwrap(), EngineResult::Yielded);
    assert_eq!(engine.turn_count(), 0);
    assert_eq!(engine.active_run_turn_count(), Some(0));

    assert_eq!(engine.resume().await.unwrap(), EngineResult::Finished);
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn paused_tool_resume_does_not_reset_the_consumed_turn_budget() {
    let events = vec![
        Event::tool_use_start(0, "call_1", "count_tool"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];
    let tool = CountingTool::new("count_tool");
    let mut engine = Engine::new(MockLlmClient::new(events));
    engine.set_max_turns(Some(1));
    engine.register_tool(tool.definition());
    engine.set_interceptor(PauseToolOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock();

    assert_eq!(engine.run("call it").await.unwrap(), EngineResult::Paused);
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), Some(1));
    assert_eq!(tool.call_count(), 0);

    assert_eq!(engine.resume().await.unwrap(), EngineResult::LimitReached);
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
    assert_eq!(tool.call_count(), 1, "the consumed turn's tool still runs");
}

#[tokio::test]
async fn fresh_input_abandons_a_paused_run_and_starts_a_new_budget() {
    let tool_events = vec![
        Event::tool_use_start(0, "call_1", "count_tool"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];
    let client = MockLlmClient::with_responses(vec![tool_events, completed_text_events()]);
    let tool = CountingTool::new("count_tool");
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.register_tool(tool.definition());
    engine.set_interceptor(PauseToolOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock();

    assert_eq!(engine.run("pause").await.unwrap(), EngineResult::Paused);
    assert_eq!(engine.active_run_turn_count(), Some(1));

    assert_eq!(engine.run("replace").await.unwrap(), EngineResult::Finished);
    assert_eq!(engine.turn_count(), 2);
    assert_eq!(engine.active_run_turn_count(), None);
    assert_eq!(tool.call_count(), 1, "pending-tool semantics are unchanged");
}

#[tokio::test]
async fn interceptor_continuation_consumes_the_logical_run_budget() {
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_interceptor(ContinueTurnOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock();

    assert_eq!(
        engine.run("start").await.unwrap(),
        EngineResult::LimitReached
    );
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.llm_call_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn restored_active_run_budget_is_enforced_before_another_llm_call() {
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_turn_count(7);
    engine.set_last_run_interrupted(true);
    engine.set_active_run_turn_count(Some(1));
    let mut engine = engine.lock();

    assert_eq!(engine.resume().await.unwrap(), EngineResult::LimitReached);
    assert_eq!(engine.turn_count(), 7);
    assert_eq!(engine.llm_call_count(), 0);
    assert_eq!(engine.active_run_turn_count(), None);
}
