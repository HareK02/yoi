//! Worker state management tests
//!
//! Tests for state transitions using the Type-state pattern (Mutable/Locked)
//! and state preservation between turns.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::Item;
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use llm_worker::{Worker, WorkerError};

// =============================================================================
// Mutable State Tests
// =============================================================================

/// Verify that system prompt can be set in Mutable state
#[test]
fn test_mutable_set_system_prompt() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    assert!(worker.get_system_prompt().is_none());

    worker.set_system_prompt("You are a helpful assistant.");
    assert_eq!(
        worker.get_system_prompt(),
        Some("You are a helpful assistant.")
    );
}

/// Verify that history can be freely edited in Mutable state
#[test]
fn test_mutable_history_manipulation() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    // Initial state is empty
    assert!(worker.history().is_empty());

    // Add to history
    worker.push_item(Item::user_message("Hello"));
    worker.push_item(Item::assistant_message("Hi there!"));
    assert_eq!(worker.history().len(), 2);

    // Mutable access to history
    worker
        .history_mut()
        .push(Item::user_message("How are you?"));
    assert_eq!(worker.history().len(), 3);

    // Clear history
    worker.clear_history();
    assert!(worker.history().is_empty());

    // Set history
    let items = vec![
        Item::user_message("Test"),
        Item::assistant_message("Response"),
    ];
    worker.set_history(items);
    assert_eq!(worker.history().len(), 2);
}

/// Verify that Worker can be constructed using builder pattern
#[test]
fn test_mutable_builder_pattern() {
    let client = MockLlmClient::new(vec![]);
    let worker = Worker::new(client)
        .system_prompt("System prompt")
        .with_item(Item::user_message("Hello"))
        .with_item(Item::assistant_message("Hi!"))
        .with_items(vec![
            Item::user_message("How are you?"),
            Item::assistant_message("I'm fine!"),
        ]);

    assert_eq!(worker.get_system_prompt(), Some("System prompt"));
    assert_eq!(worker.history().len(), 4);
}

/// Verify that multiple items can be added with extend_history
#[test]
fn test_mutable_extend_history() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    worker.push_item(Item::user_message("First"));

    worker.extend_history(vec![
        Item::assistant_message("Response 1"),
        Item::user_message("Second"),
        Item::assistant_message("Response 2"),
    ]);

    assert_eq!(worker.history().len(), 4);
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
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(format!("{}-ok", self.name).into())
    }
}

/// Verify that tools can be registered in Mutable state.
#[test]
fn test_mutable_can_register_tool() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);
    let tool = CountingTool::new("count_tool");

    // register_tool is infallible (factory deferred to run-time flush)
    worker.register_tool(tool.definition());
}

// =============================================================================
// State Transition Tests
// =============================================================================

/// Verify that lock() transitions from Mutable -> Locked state
#[test]
fn test_lock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    worker.set_system_prompt("System");
    worker.push_item(Item::user_message("Hello"));
    worker.push_item(Item::assistant_message("Hi"));

    // Lock
    let locked_worker = worker.lock();

    // History and system prompt are still accessible in Locked state
    assert_eq!(locked_worker.get_system_prompt(), Some("System"));
    assert_eq!(locked_worker.history().len(), 2);
    assert_eq!(locked_worker.locked_prefix_len(), 2);
}

/// Verify that unlock() transitions from Locked -> Mutable state
#[test]
fn test_unlock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut worker = Worker::new(client);

    worker.push_item(Item::user_message("Hello"));
    let locked_worker = worker.lock();

    // Unlock
    let mut worker = locked_worker.unlock();

    // History operations are available again in Mutable state
    worker.push_item(Item::assistant_message("Hi"));
    worker.clear_history();
    assert!(worker.history().is_empty());
}

// =============================================================================
// Turn Execution and State Preservation Tests
// =============================================================================

/// Verify that history is correctly updated after running a turn in Mutable state
#[tokio::test]
async fn test_mutable_run_updates_history() -> Result<(), WorkerError> {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello, I'm an assistant!"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let worker = Worker::new(client);

    // Execute (Mutable::run consumes self, returns RunOutput)
    let out = worker.run("Hi there").await?;
    let worker = out.worker;

    // History is updated
    let history = worker.history();
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

    let worker = Worker::new(client).system_prompt("You are helpful.");

    // Lock (after setting system prompt)
    let mut locked_worker = worker.lock();
    assert_eq!(locked_worker.locked_prefix_len(), 0); // No items yet

    // Turn 1
    let result1 = locked_worker.run("Hello!").await;
    assert!(result1.is_ok());
    assert_eq!(locked_worker.history().len(), 2); // user + assistant

    // Turn 2
    let result2 = locked_worker.run("Can you help me?").await;
    assert!(result2.is_ok());
    assert_eq!(locked_worker.history().len(), 4); // 2 * (user + assistant)

    // Verify history contents
    let history = locked_worker.history();

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

    let mut worker = Worker::new(client);

    // Add items beforehand
    worker.push_item(Item::user_message("Pre-existing message 1"));
    worker.push_item(Item::assistant_message("Pre-existing response 1"));

    assert_eq!(worker.history().len(), 2);

    // Lock
    let mut locked_worker = worker.lock();
    assert_eq!(locked_worker.locked_prefix_len(), 2); // 2 items at lock time

    // Execute turn
    locked_worker.run("New message").await.unwrap();

    // History grows but locked_prefix_len remains unchanged
    assert_eq!(locked_worker.history().len(), 4); // 2 + 2
    assert_eq!(locked_worker.locked_prefix_len(), 2); // Unchanged
}

/// Verify that turn count is correctly incremented
#[tokio::test]
async fn test_turn_count_increment() -> Result<(), WorkerError> {
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

    let worker = Worker::new(client);

    assert_eq!(worker.turn_count(), 0);

    // First run consumes Mutable, returns RunOutput
    let mut worker = worker.run("First").await?.worker;
    assert_eq!(worker.turn_count(), 1);

    // Subsequent runs on Locked take &mut self
    worker.run("Second").await?;
    assert_eq!(worker.turn_count(), 2);

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

    let worker = Worker::new(client)
        .with_item(Item::user_message("Hello"))
        .with_item(Item::assistant_message("Hi"));

    // Lock -> Unlock
    let locked = worker.lock();
    assert_eq!(locked.locked_prefix_len(), 2);

    let mut unlocked = locked.unlock();

    // Edit history
    unlocked.clear_history();
    unlocked.push_item(Item::user_message("Fresh start"));

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

    let mut worker = Worker::new(client);
    let tool_a = CountingTool::new("tool_a");
    worker.register_tool(tool_a.definition());

    let mut locked = worker.lock();
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
    let worker = Worker::new(client).system_prompt("Important system prompt");

    let locked = worker.lock();
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
    let worker = Worker::new(client).system_prompt("Original prompt");

    let locked = worker.lock();
    let mut unlocked = locked.unlock();

    unlocked.set_system_prompt("New prompt");
    assert_eq!(unlocked.get_system_prompt(), Some("New prompt"));

    let relocked = unlocked.lock();
    assert_eq!(relocked.get_system_prompt(), Some("New prompt"));
}
