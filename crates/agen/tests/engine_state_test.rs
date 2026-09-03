//! Engine state management tests
//!
//! Tests for state transitions using the Type-state pattern (Mutable/Locked)
//! and state preservation between turns.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agen::Item;
use agen::interceptor::{
    AssistantTurnEndContext, Interceptor, InterceptorError, InterceptorPoint, InterceptorResult,
    PostToolAction, PreLlmRequestContext, PreRequestAction, PreToolAction, PromptAction,
    PromptSubmitContext, RunExitContext, ToolCallInfo, ToolResultInfo, TurnEndAction,
};
use agen::llm_client::{
    ClientError, LlmClient, Request, ResponseStream,
    event::{Event, ResponseStatus, StatusEvent},
};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use agen::{Engine, EngineError, EngineRunExit, History, RunInterruptionReason};
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
    let mut history: History = History::new();

    // Initial state is empty
    assert!(history.is_empty());

    // Add to history
    engine
        .append_history(&mut history, vec![Item::user_message("Hello")])
        .unwrap();
    engine
        .append_history(&mut history, vec![Item::assistant_message("Hi there!")])
        .unwrap();
    assert_eq!(history.len(), 2);

    // Append to history via the callback-aware API.
    engine
        .append_history(&mut history, vec![Item::user_message("How are you?")])
        .unwrap();
    assert_eq!(history.len(), 3);

    // Clear history
    engine.clear_history(&mut history);
    assert!(history.is_empty());

    // Set history
    let items = vec![
        Item::user_message("Test"),
        Item::assistant_message("Response"),
    ];
    engine.set_history(&mut history, items);
    assert_eq!(history.len(), 2);
}

/// Verify that Engine can be constructed using builder pattern
#[test]
fn test_mutable_builder_pattern() {
    let client = MockLlmClient::new(vec![]);
    let engine = Engine::new(client).system_prompt("System prompt");
    let history: History = History::new();

    assert_eq!(engine.get_system_prompt(), Some("System prompt"));
    assert!(history.is_empty());
}

/// Verify that multiple items can be added with append_history and callbacks fire.
#[test]
fn test_mutable_append_history() {
    let client = MockLlmClient::new(vec![]);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_callback = Arc::clone(&observed);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();
    engine.on_history_append(move |item| {
        if let Some(text) = item.as_text() {
            observed_for_callback.lock().unwrap().push(text.to_string());
        }
        Ok(())
    });

    engine
        .append_history(&mut history, vec![Item::user_message("First")])
        .unwrap();

    engine
        .append_history(
            &mut history,
            vec![
                Item::assistant_message("Response 1"),
                Item::user_message("Second"),
                Item::assistant_message("Response 2"),
            ],
        )
        .unwrap();

    assert_eq!(history.len(), 4);
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
    let mut history: History = History::new();
    engine.register_tool(tool.definition());
    engine.on_history_append(|item| {
        if item.is_tool_call() {
            Err("simulated ENOSPC".to_string())
        } else {
            Ok(())
        }
    });

    let mut engine = engine.lock(&history);
    let exit = engine.run(&mut history, "use the tool").await;

    assert!(
        matches!(exit, EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(EngineError::HistoryAppend(ref message))) if message == "simulated ENOSPC")
    );
    assert_eq!(tool.call_count(), 0);
    assert_eq!(history.len(), 1);
    assert_eq!(history.entries()[0].item.as_text(), Some("use the tool"));
}

// =============================================================================
// State Transition Tests
// =============================================================================

/// Verify that lock() transitions from Mutable -> Locked state
#[test]
fn test_lock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    engine.set_system_prompt("System");
    engine
        .append_history(&mut history, vec![Item::user_message("Hello")])
        .unwrap();
    engine
        .append_history(&mut history, vec![Item::assistant_message("Hi")])
        .unwrap();

    // Lock
    let locked_engine = engine.lock(&history);

    // History and system prompt are still accessible in Locked state
    assert_eq!(locked_engine.get_system_prompt(), Some("System"));
    assert_eq!(history.len(), 2);
    assert_eq!(locked_engine.locked_prefix_len(), 2);
}

/// Verify that unlock() transitions from Locked -> Mutable state
#[test]
fn test_unlock_transition() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    engine
        .append_history(&mut history, vec![Item::user_message("Hello")])
        .unwrap();
    let locked_engine = engine.lock(&history);

    // Unlock
    let mut engine = locked_engine.unlock();

    // History operations are available again in Mutable state
    engine
        .append_history(&mut history, vec![Item::assistant_message("Hi")])
        .unwrap();
    engine.clear_history(&mut history);
    assert!(history.is_empty());
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
    let mut history: History = History::new();

    // Execute (Mutable::run consumes self, returns EngineRunOutput)
    let _out = engine.run(&mut history, "Hi there").await;

    // History is updated
    let entries = history.entries();
    assert_eq!(history.len(), 2); // user + assistant

    // User message
    assert_eq!(entries[0].item.as_text(), Some("Hi there"));

    // Assistant message
    assert_eq!(entries[1].item.as_text(), Some("Hello, I'm an assistant!"));

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
    let mut history: History = History::new();

    // Lock (after setting system prompt)
    let mut locked_engine = engine.lock(&history);
    assert_eq!(locked_engine.locked_prefix_len(), 0); // No items yet

    // Turn 1
    let result1 = locked_engine.run(&mut history, "Hello!").await;
    assert!(matches!(result1, EngineRunExit::Finished));
    assert_eq!(history.len(), 2); // user + assistant

    // Turn 2
    let result2 = locked_engine.run(&mut history, "Can you help me?").await;
    assert!(matches!(result2, EngineRunExit::Finished));
    assert_eq!(history.len(), 4); // 2 * (user + assistant)

    // Verify history contents
    let entries = history.entries();

    // Turn 1 user message
    assert_eq!(entries[0].item.as_text(), Some("Hello!"));

    // Turn 1 assistant message
    assert_eq!(entries[1].item.as_text(), Some("Nice to meet you!"));

    // Turn 2 user message
    assert_eq!(entries[2].item.as_text(), Some("Can you help me?"));

    // Turn 2 assistant message
    assert_eq!(entries[3].item.as_text(), Some("I can help with that."));
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
    let mut history: History = History::new();

    // Add items beforehand
    engine
        .append_history(
            &mut history,
            vec![Item::user_message("Pre-existing message 1")],
        )
        .unwrap();
    engine
        .append_history(
            &mut history,
            vec![Item::assistant_message("Pre-existing response 1")],
        )
        .unwrap();

    assert_eq!(history.len(), 2);

    // Lock
    let mut locked_engine = engine.lock(&history);
    assert_eq!(locked_engine.locked_prefix_len(), 2); // 2 items at lock time

    // Execute turn
    locked_engine.run(&mut history, "New message").await;

    // History grows but locked_prefix_len remains unchanged
    assert_eq!(history.len(), 4); // 2 + 2
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
    let mut history: History = History::new();

    assert_eq!(engine.turn_count(), 0);
    assert_eq!(engine.llm_call_count(), 0);

    // First run consumes Mutable, returns EngineRunOutput
    let mut engine = engine.run(&mut history, "First").await.engine;
    assert_eq!(engine.turn_count(), 1);
    // Retry not yet implemented → AgentTurn:LlmCall is 1:1.
    assert_eq!(engine.llm_call_count(), 1);

    // Subsequent runs on Locked take &mut self
    assert!(matches!(
        engine.run(&mut history, "Second").await,
        EngineRunExit::Finished
    ));
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
    let mut history: History = History::new();
    engine
        .append_history(
            &mut history,
            vec![Item::user_message("Hello"), Item::assistant_message("Hi")],
        )
        .unwrap();

    // Lock -> Unlock
    let locked = engine.lock(&history);
    assert_eq!(locked.locked_prefix_len(), 2);

    let mut unlocked = locked.unlock();

    // Edit history
    unlocked.clear_history(&mut history);
    unlocked
        .append_history(&mut history, vec![Item::user_message("Fresh start")])
        .unwrap();

    // Re-lock
    let relocked = unlocked.lock(&history);
    assert_eq!(history.len(), 1);
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
    let mut history: History = History::new();
    let tool_a = CountingTool::new("tool_a");
    engine.register_tool(tool_a.definition());

    let mut locked = engine.lock(&history);
    assert!(matches!(
        locked.run(&mut history, "first").await,
        EngineRunExit::Finished
    ));
    assert_eq!(tool_a.call_count(), 1, "tool_a should be called once");

    let mut unlocked = locked.unlock();
    let tool_b = CountingTool::new("tool_b");
    unlocked.register_tool(tool_b.definition());

    let mut relocked = unlocked.lock(&history);
    assert!(matches!(
        relocked.run(&mut history, "second").await,
        EngineRunExit::Finished
    ));

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
    let history: History = History::new();

    let locked = engine.lock(&history);
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
    let history: History = History::new();

    let locked = engine.lock(&history);
    let mut unlocked = locked.unlock();

    unlocked.set_system_prompt("New prompt");
    assert_eq!(unlocked.get_system_prompt(), Some("New prompt"));

    let relocked = unlocked.lock(&history);
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
    async fn pre_llm_request(
        &self,
        _context: PreLlmRequestContext<'_>,
    ) -> InterceptorResult<PreRequestAction> {
        Ok(if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            PreRequestAction::Yield
        } else {
            PreRequestAction::Continue
        })
    }
}

struct PauseToolOnce {
    calls: AtomicUsize,
}

#[async_trait]
impl Interceptor for PauseToolOnce {
    async fn pre_tool_call(&self, _info: &mut ToolCallInfo) -> InterceptorResult<PreToolAction> {
        Ok(if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            PreToolAction::Pause
        } else {
            PreToolAction::Continue
        })
    }
}

struct ContinueTurnOnce {
    calls: AtomicUsize,
}

#[async_trait]
impl Interceptor for ContinueTurnOnce {
    async fn on_assistant_turn_end(
        &self,
        _context: AssistantTurnEndContext<'_>,
    ) -> InterceptorResult<TurnEndAction> {
        Ok(if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            TurnEndAction::ContinueWithMessages(vec![Item::system_message("continue")])
        } else {
            TurnEndAction::Finish
        })
    }
}

#[derive(Debug, Clone)]
struct FailingLifecycleInterceptor {
    failure: InterceptorPoint,
    calls: Arc<Mutex<Vec<InterceptorPoint>>>,
}

impl FailingLifecycleInterceptor {
    fn new(failure: InterceptorPoint) -> Self {
        Self {
            failure,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn record<T>(&self, point: InterceptorPoint, action: T) -> InterceptorResult<T> {
        self.calls.lock().unwrap().push(point);
        if self.failure == point {
            Err(InterceptorError::new(format!("{point} rejected")))
        } else {
            Ok(action)
        }
    }

    fn calls(&self) -> Vec<InterceptorPoint> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl Interceptor for FailingLifecycleInterceptor {
    async fn on_prompt_submit(
        &self,
        _context: PromptSubmitContext<'_>,
    ) -> InterceptorResult<PromptAction> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::PromptSubmit, PromptAction::Continue)
    }

    async fn pending_history_appends(&self) -> InterceptorResult<Vec<Item>> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::PendingHistoryAppends, Vec::new())
    }

    async fn pre_llm_request(
        &self,
        _context: PreLlmRequestContext<'_>,
    ) -> InterceptorResult<PreRequestAction> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::PreLlmRequest, PreRequestAction::Continue)
    }

    async fn pre_tool_call(&self, _info: &mut ToolCallInfo) -> InterceptorResult<PreToolAction> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::PreToolCall, PreToolAction::Continue)
    }

    async fn post_tool_call(&self, _info: &ToolResultInfo) -> InterceptorResult<PostToolAction> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::PostToolCall, PostToolAction::Continue)
    }

    async fn on_assistant_turn_end(
        &self,
        context: AssistantTurnEndContext<'_>,
    ) -> InterceptorResult<TurnEndAction> {
        tokio::task::yield_now().await;
        assert!(context.history.ends_with(context.assistant_items));
        if !context.tool_calls.is_empty() {
            assert_eq!(
                context
                    .assistant_items
                    .iter()
                    .filter(|item| matches!(item, Item::ToolCall { .. }))
                    .count(),
                context.tool_calls.len()
            );
        }
        self.record(InterceptorPoint::AssistantTurnEnd, TurnEndAction::Finish)
    }

    async fn on_run_exit(&self, _context: RunExitContext<'_>) -> InterceptorResult<()> {
        tokio::task::yield_now().await;
        self.record(InterceptorPoint::RunExit, ())
    }
}

fn expected_interceptor_calls(failure: InterceptorPoint) -> Vec<InterceptorPoint> {
    use InterceptorPoint as Point;

    let mut calls = match failure {
        Point::PromptSubmit => vec![Point::PromptSubmit],
        Point::PendingHistoryAppends => {
            vec![Point::PromptSubmit, Point::PendingHistoryAppends]
        }
        Point::PreLlmRequest => vec![
            Point::PromptSubmit,
            Point::PendingHistoryAppends,
            Point::PreLlmRequest,
        ],
        Point::PreToolCall => vec![
            Point::PromptSubmit,
            Point::PendingHistoryAppends,
            Point::PreLlmRequest,
            Point::AssistantTurnEnd,
            Point::PreToolCall,
        ],
        Point::PostToolCall => vec![
            Point::PromptSubmit,
            Point::PendingHistoryAppends,
            Point::PreLlmRequest,
            Point::AssistantTurnEnd,
            Point::PreToolCall,
            Point::PostToolCall,
        ],
        Point::AssistantTurnEnd => vec![
            Point::PromptSubmit,
            Point::PendingHistoryAppends,
            Point::PreLlmRequest,
            Point::AssistantTurnEnd,
        ],
        Point::RunExit => vec![
            Point::PromptSubmit,
            Point::PendingHistoryAppends,
            Point::PreLlmRequest,
            Point::AssistantTurnEnd,
        ],
    };
    calls.push(Point::RunExit);
    calls
}

#[tokio::test]
async fn interceptor_failures_are_typed_unexpected_and_each_lifecycle_point_runs_once() {
    use InterceptorPoint as Point;

    for failure_point in [
        Point::PromptSubmit,
        Point::PendingHistoryAppends,
        Point::PreLlmRequest,
        Point::PreToolCall,
        Point::PostToolCall,
        Point::AssistantTurnEnd,
        Point::RunExit,
    ] {
        let interceptor = FailingLifecycleInterceptor::new(failure_point);
        let needs_tool = matches!(failure_point, Point::PreToolCall | Point::PostToolCall);
        let events = if needs_tool {
            vec![
                Event::tool_use_start(0, "call-1", "count_tool"),
                Event::tool_input_delta(0, "{}"),
                Event::tool_use_stop(0),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]
        } else {
            completed_text_events()
        };
        let mut engine = Engine::new(MockLlmClient::new(events));
        engine.register_tool(CountingTool::new("count_tool").definition());
        engine.set_interceptor(interceptor.clone());
        let mut history = History::new();
        let mut engine = engine.lock(&history);

        let exit = engine.run(&mut history, "test").await;
        let EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
            EngineError::Interceptor(failure),
        )) = exit
        else {
            panic!("expected typed interceptor interruption at {failure_point}, got {exit:?}");
        };
        assert_eq!(failure.point(), failure_point);
        assert_eq!(
            failure.error().message(),
            format!("{failure_point} rejected")
        );
        assert_eq!(
            interceptor.calls(),
            expected_interceptor_calls(failure_point)
        );
        if failure_point == Point::PostToolCall {
            assert!(
                history
                    .items()
                    .any(|item| matches!(item, Item::ToolResult { .. })),
                "post-tool failure must not precede terminal output commit"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalMode {
    Finish,
    PauseOnce,
    Yield,
}

#[derive(Debug, Clone)]
struct RecordingTerminalInterceptor {
    mode: TerminalMode,
    assistant_turns: Arc<AtomicUsize>,
    exits: Arc<Mutex<Vec<&'static str>>>,
}

impl RecordingTerminalInterceptor {
    fn new(mode: TerminalMode) -> Self {
        Self {
            mode,
            assistant_turns: Arc::new(AtomicUsize::new(0)),
            exits: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn exits(&self) -> Vec<&'static str> {
        self.exits.lock().unwrap().clone()
    }
}

#[async_trait]
impl Interceptor for RecordingTerminalInterceptor {
    async fn pre_llm_request(
        &self,
        _context: PreLlmRequestContext<'_>,
    ) -> InterceptorResult<PreRequestAction> {
        Ok(if self.mode == TerminalMode::Yield {
            PreRequestAction::Yield
        } else {
            PreRequestAction::Continue
        })
    }

    async fn on_assistant_turn_end(
        &self,
        context: AssistantTurnEndContext<'_>,
    ) -> InterceptorResult<TurnEndAction> {
        assert!(!context.assistant_items.is_empty());
        assert!(
            context.history.ends_with(context.assistant_items),
            "assistant-turn callback must observe committed terminal items"
        );
        let turn = self.assistant_turns.fetch_add(1, Ordering::SeqCst);
        Ok(if self.mode == TerminalMode::PauseOnce && turn == 0 {
            TurnEndAction::Pause
        } else {
            TurnEndAction::Finish
        })
    }

    async fn on_run_exit(&self, context: RunExitContext<'_>) -> InterceptorResult<()> {
        let kind = match context.exit {
            EngineRunExit::Finished => "finished",
            EngineRunExit::Paused => "paused",
            EngineRunExit::Yielded => "yielded",
            EngineRunExit::Interrupted(RunInterruptionReason::LimitReached) => "limit",
            EngineRunExit::Interrupted(RunInterruptionReason::ContextWindowExceeded) => "context",
            EngineRunExit::Interrupted(RunInterruptionReason::Cancelled) => "cancelled",
            EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(_)) => "unexpected",
        };
        self.exits.lock().unwrap().push(kind);
        Ok(())
    }
}

#[derive(Clone)]
struct ContextWindowClient;

#[async_trait]
impl LlmClient for ContextWindowClient {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        Err(ClientError::ContextWindowExceeded)
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

#[tokio::test]
async fn terminal_observer_runs_once_for_every_exit_and_interruption_kind() {
    let finished = RecordingTerminalInterceptor::new(TerminalMode::Finish);
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_interceptor(finished.clone());
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "finish").await,
        EngineRunExit::Finished
    ));
    assert_eq!(finished.exits(), ["finished"]);

    let yielded = RecordingTerminalInterceptor::new(TerminalMode::Yield);
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_interceptor(yielded.clone());
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "yield").await,
        EngineRunExit::Yielded
    ));
    assert_eq!(yielded.exits(), ["yielded"]);

    let limited = RecordingTerminalInterceptor::new(TerminalMode::Finish);
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(0));
    engine.set_interceptor(limited.clone());
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "limit").await,
        EngineRunExit::Interrupted(RunInterruptionReason::LimitReached)
    ));
    assert_eq!(limited.exits(), ["limit"]);

    let cancelled = RecordingTerminalInterceptor::new(TerminalMode::Finish);
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_interceptor(cancelled.clone());
    engine.cancel();
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "cancel").await,
        EngineRunExit::Interrupted(RunInterruptionReason::Cancelled)
    ));
    assert_eq!(cancelled.exits(), ["cancelled"]);

    let context = RecordingTerminalInterceptor::new(TerminalMode::Finish);
    let mut engine = Engine::new(ContextWindowClient);
    engine.set_interceptor(context.clone());
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "context").await,
        EngineRunExit::Interrupted(RunInterruptionReason::ContextWindowExceeded)
    ));
    assert_eq!(context.exits(), ["context"]);

    let unexpected = FailingLifecycleInterceptor::new(InterceptorPoint::PromptSubmit);
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_interceptor(unexpected.clone());
    let mut history = History::new();
    assert!(matches!(
        engine.lock(&history).run(&mut history, "fail").await,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(EngineError::Interceptor(
            _
        )))
    ));
    assert_eq!(
        unexpected
            .calls()
            .iter()
            .filter(|point| **point == InterceptorPoint::RunExit)
            .count(),
        1
    );
}

#[tokio::test]
async fn terminal_observer_does_not_duplicate_on_resume() {
    let interceptor = RecordingTerminalInterceptor::new(TerminalMode::PauseOnce);
    let first_response = vec![
        Event::tool_use_start(0, "call-1", "count_tool"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];
    let client = MockLlmClient::with_responses(vec![first_response, completed_text_events()]);
    let tool = CountingTool::new("count_tool");
    let mut engine = Engine::new(client);
    engine.register_tool(tool.definition());
    engine.set_interceptor(interceptor.clone());
    let mut history = History::new();
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "pause").await,
        EngineRunExit::Paused
    ));
    assert_eq!(interceptor.exits(), ["paused"]);
    assert_eq!(
        tool.call_count(),
        0,
        "pause must retain the pending tool phase"
    );

    assert!(matches!(
        engine.resume(&mut history).await,
        EngineRunExit::Finished
    ));
    assert_eq!(interceptor.exits(), ["paused", "finished"]);
    assert_eq!(
        tool.call_count(),
        1,
        "resume must execute the retained tool once"
    );
}

#[tokio::test]
async fn max_turns_is_scoped_to_each_fresh_run() {
    let mut history: History = History::new();
    let responses = vec![completed_text_events(), completed_text_events()];
    let mut engine = Engine::new(MockLlmClient::with_responses(responses));
    engine.set_max_turns(Some(1));
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "first").await,
        EngineRunExit::Finished
    ));
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);

    assert!(matches!(
        engine.run(&mut history, "second").await,
        EngineRunExit::Finished
    ));
    assert_eq!(engine.turn_count(), 2);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn yielded_resume_keeps_the_same_unspent_turn_budget() {
    let mut history: History = History::new();
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_interceptor(YieldOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "start").await,
        EngineRunExit::Yielded
    ));
    assert_eq!(engine.turn_count(), 0);
    assert_eq!(engine.active_run_turn_count(), Some(0));

    assert!(matches!(
        engine.resume(&mut history).await,
        EngineRunExit::Finished
    ));
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn paused_tool_resume_does_not_reset_the_consumed_turn_budget() {
    let mut history: History = History::new();
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
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "call it").await,
        EngineRunExit::Paused
    ));
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), Some(1));
    assert_eq!(tool.call_count(), 0);

    assert!(matches!(
        engine.resume(&mut history).await,
        EngineRunExit::Interrupted(RunInterruptionReason::LimitReached)
    ));
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
    assert_eq!(tool.call_count(), 1, "the consumed turn's tool still runs");
}

#[tokio::test]
async fn fresh_input_abandons_a_paused_run_and_starts_a_new_budget() {
    let mut history: History = History::new();
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
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "pause").await,
        EngineRunExit::Paused
    ));
    assert_eq!(engine.active_run_turn_count(), Some(1));

    assert!(matches!(
        engine.run(&mut history, "replace").await,
        EngineRunExit::Finished
    ));
    assert_eq!(engine.turn_count(), 2);
    assert_eq!(engine.active_run_turn_count(), None);
    assert_eq!(tool.call_count(), 1, "pending-tool semantics are unchanged");
}

#[tokio::test]
async fn interceptor_continuation_consumes_the_logical_run_budget() {
    let mut history: History = History::new();
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_interceptor(ContinueTurnOnce {
        calls: AtomicUsize::new(0),
    });
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.run(&mut history, "start").await,
        EngineRunExit::Interrupted(RunInterruptionReason::LimitReached)
    ));
    assert_eq!(engine.turn_count(), 1);
    assert_eq!(engine.llm_call_count(), 1);
    assert_eq!(engine.active_run_turn_count(), None);
}

#[tokio::test]
async fn restored_active_run_budget_is_enforced_before_another_llm_call() {
    let mut history: History = History::new();
    let mut engine = Engine::new(MockLlmClient::new(completed_text_events()));
    engine.set_max_turns(Some(1));
    engine.set_turn_count(7);
    engine.set_active_run_turn_count(Some(1));
    let mut engine = engine.lock(&history);

    assert!(matches!(
        engine.resume(&mut history).await,
        EngineRunExit::Interrupted(RunInterruptionReason::LimitReached)
    ));
    assert_eq!(engine.turn_count(), 7);
    assert_eq!(engine.llm_call_count(), 0);
    assert_eq!(engine.active_run_turn_count(), None);
}
