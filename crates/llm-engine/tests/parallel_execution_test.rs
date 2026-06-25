//! Parallel tool execution tests
//!
//! Verify that Engine executes multiple tools in parallel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use llm_engine::Engine;
use llm_engine::interceptor::{
    Interceptor, PostToolAction, PreToolAction, ToolCallInfo, ToolResultInfo,
};
use llm_engine::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput, ToolResult,
};

mod common;
use common::MockLlmClient;

// =============================================================================
// Parallel Execution Test Tools
// =============================================================================

/// Tool that waits for a specified time before responding
#[derive(Clone)]
struct SlowTool {
    name: String,
    delay_ms: u64,
    call_count: Arc<AtomicUsize>,
}

impl SlowTool {
    fn new(name: impl Into<String>, delay_ms: u64) -> Self {
        Self {
            name: name.into(),
            delay_ms,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Create ToolDefinition
    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new(&tool.name)
                .description("A tool that waits before responding")
                .input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }
}

#[async_trait]
impl Tool for SlowTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(format!("Completed after {}ms", self.delay_ms).into())
    }
}

#[derive(Clone)]
struct ContextRecordingTool {
    name: String,
    contexts: Arc<Mutex<Vec<ToolExecutionContext>>>,
}

impl ContextRecordingTool {
    fn new(name: impl Into<String>, contexts: Arc<Mutex<Vec<ToolExecutionContext>>>) -> Self {
        Self {
            name: name.into(),
            contexts,
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new(&tool.name)
                .description("Records tool execution context")
                .input_schema(serde_json::json!({"type": "object"}));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }
}

#[async_trait]
impl Tool for ContextRecordingTool {
    async fn execute(
        &self,
        _input_json: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.contexts.lock().unwrap().push(ctx);
        Ok("recorded".to_string().into())
    }
}

// =============================================================================
// Tests
// =============================================================================

/// Verify that multiple tools are executed in parallel
///
/// If each tool takes 100ms, sequential execution would take 300ms+,
/// but parallel execution should complete in about 100ms.
#[tokio::test]
async fn test_parallel_tool_execution() {
    // Event sequence containing 3 tool calls
    let events = vec![
        Event::tool_use_start(0, "call_1", "slow_tool_1"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::tool_use_start(1, "call_2", "slow_tool_2"),
        Event::tool_input_delta(1, r#"{}"#),
        Event::tool_use_stop(1),
        Event::tool_use_start(2, "call_3", "slow_tool_3"),
        Event::tool_input_delta(2, r#"{}"#),
        Event::tool_use_stop(2),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::with_responses(vec![
        events,
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Done"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let tool1 = SlowTool::new("slow_tool_1", 100);
    let tool2 = SlowTool::new("slow_tool_2", 100);
    let tool3 = SlowTool::new("slow_tool_3", 100);

    let tool1_clone = tool1.clone();
    let tool2_clone = tool2.clone();
    let tool3_clone = tool3.clone();

    engine.register_tool(tool1.definition());
    engine.register_tool(tool2.definition());
    engine.register_tool(tool3.definition());

    let start = Instant::now();
    // Mutable::run consumes self, returns (Locked, EngineResult)
    let _result = engine.run("Run all tools").await;
    let elapsed = start.elapsed();

    // Verify all tools were called
    assert_eq!(tool1_clone.call_count(), 1, "Tool 1 should be called once");
    assert_eq!(tool2_clone.call_count(), 1, "Tool 2 should be called once");
    assert_eq!(tool3_clone.call_count(), 1, "Tool 3 should be called once");

    // Parallel execution should complete in under 200ms (sequential would be 300ms+)
    // Using 250ms as threshold with margin
    assert!(
        elapsed < Duration::from_millis(250),
        "Parallel execution should complete in ~100ms, but took {:?}",
        elapsed
    );

    println!("Parallel execution completed in {:?}", elapsed);
}

#[tokio::test]
async fn test_tool_execution_context_order_and_batch_id() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_a", "record_a"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::tool_use_start(1, "call_b", "record_b"),
            Event::tool_input_delta(1, r#"{}"#),
            Event::tool_use_stop(1),
            Event::tool_use_start(2, "call_c", "record_c"),
            Event::tool_input_delta(2, r#"{}"#),
            Event::tool_use_stop(2),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Done"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let contexts = Arc::new(Mutex::new(Vec::new()));

    engine.register_tool(ContextRecordingTool::new("record_a", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("record_b", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("record_c", contexts.clone()).definition());

    let _ = engine.run("record contexts").await;

    let mut contexts = contexts.lock().unwrap().clone();
    contexts.sort_by_key(|ctx| ctx.call_index);

    assert_eq!(contexts.len(), 3);
    assert_eq!(contexts[0].call_id, "call_a");
    assert_eq!(contexts[0].call_index, 0);
    assert_eq!(contexts[1].call_id, "call_b");
    assert_eq!(contexts[1].call_index, 1);
    assert_eq!(contexts[2].call_id, "call_c");
    assert_eq!(contexts[2].call_index, 2);
    assert_eq!(contexts[0].batch_id, contexts[1].batch_id);
    assert_eq!(contexts[1].batch_id, contexts[2].batch_id);
}

#[tokio::test]
async fn test_tool_execution_context_batch_id_changes_between_batches() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_first", "record"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::tool_use_start(0, "call_second", "record"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Done"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let contexts = Arc::new(Mutex::new(Vec::new()));

    engine.register_tool(ContextRecordingTool::new("record", contexts.clone()).definition());

    let _ = engine.run("record batches").await;

    let contexts = contexts.lock().unwrap().clone();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].call_id, "call_first");
    assert_eq!(contexts[0].call_index, 0);
    assert_eq!(contexts[1].call_id, "call_second");
    assert_eq!(contexts[1].call_index, 0);
    assert_ne!(contexts[0].batch_id, contexts[1].batch_id);
}

#[tokio::test]
async fn test_tool_execution_context_for_skipped_and_synthetic_paths() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_run", "record"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::tool_use_start(1, "call_skip", "skip_tool"),
            Event::tool_input_delta(1, r#"{}"#),
            Event::tool_use_stop(1),
            Event::tool_use_start(2, "call_synth", "synthetic_tool"),
            Event::tool_input_delta(2, r#"{}"#),
            Event::tool_use_stop(2),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Done"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let executed_contexts = Arc::new(Mutex::new(Vec::new()));
    let pre_contexts = Arc::new(Mutex::new(Vec::new()));
    let post_contexts = Arc::new(Mutex::new(Vec::new()));

    engine
        .register_tool(ContextRecordingTool::new("record", executed_contexts.clone()).definition());
    engine.register_tool(
        ContextRecordingTool::new("skip_tool", executed_contexts.clone()).definition(),
    );
    engine.register_tool(
        ContextRecordingTool::new("synthetic_tool", executed_contexts.clone()).definition(),
    );

    struct ContextPolicy {
        pre_contexts: Arc<Mutex<Vec<ToolExecutionContext>>>,
        post_contexts: Arc<Mutex<Vec<ToolExecutionContext>>>,
    }

    #[async_trait]
    impl Interceptor for ContextPolicy {
        async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
            self.pre_contexts.lock().unwrap().push(info.context.clone());
            match info.call.name.as_str() {
                "skip_tool" => PreToolAction::Skip,
                "synthetic_tool" => PreToolAction::SyntheticResult(ToolResult::from_output(
                    &info.call.id,
                    ToolOutput::from("synthetic result".to_string()),
                )),
                _ => PreToolAction::Continue,
            }
        }

        async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
            self.post_contexts
                .lock()
                .unwrap()
                .push(info.context.clone());
            PostToolAction::Continue
        }
    }

    engine.set_interceptor(ContextPolicy {
        pre_contexts: pre_contexts.clone(),
        post_contexts: post_contexts.clone(),
    });

    let _ = engine.run("record skipped and synthetic contexts").await;

    let mut pre_contexts = pre_contexts.lock().unwrap().clone();
    pre_contexts.sort_by_key(|ctx| ctx.call_index);
    assert_eq!(pre_contexts.len(), 3);
    assert_eq!(pre_contexts[0].call_id, "call_run");
    assert_eq!(pre_contexts[0].call_index, 0);
    assert_eq!(pre_contexts[1].call_id, "call_skip");
    assert_eq!(pre_contexts[1].call_index, 1);
    assert_eq!(pre_contexts[2].call_id, "call_synth");
    assert_eq!(pre_contexts[2].call_index, 2);
    assert_eq!(pre_contexts[0].batch_id, pre_contexts[1].batch_id);
    assert_eq!(pre_contexts[1].batch_id, pre_contexts[2].batch_id);

    let executed_contexts = executed_contexts.lock().unwrap().clone();
    assert_eq!(executed_contexts.len(), 1);
    assert_eq!(executed_contexts[0].call_id, "call_run");
    assert_eq!(executed_contexts[0].call_index, 0);

    let mut post_contexts = post_contexts.lock().unwrap().clone();
    post_contexts.sort_by_key(|ctx| ctx.call_index);
    assert_eq!(post_contexts.len(), 2);
    assert_eq!(post_contexts[0].call_id, "call_run");
    assert_eq!(post_contexts[0].call_index, 0);
    assert_eq!(post_contexts[1].call_id, "call_synth");
    assert_eq!(post_contexts[1].call_index, 2);
    assert_eq!(post_contexts[0].batch_id, post_contexts[1].batch_id);
}

#[tokio::test]
async fn test_before_tool_call_skip() {
    let events = vec![
        Event::tool_use_start(0, "call_1", "allowed_tool"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::tool_use_start(1, "call_2", "blocked_tool"),
        Event::tool_input_delta(1, r#"{}"#),
        Event::tool_use_stop(1),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);

    let allowed_tool = SlowTool::new("allowed_tool", 10);
    let blocked_tool = SlowTool::new("blocked_tool", 10);

    let allowed_clone = allowed_tool.clone();
    let blocked_clone = blocked_tool.clone();

    engine.register_tool(allowed_tool.definition());
    engine.register_tool(blocked_tool.definition());

    // Policy to skip "blocked_tool"
    struct BlockingPolicy;

    #[async_trait]
    impl Interceptor for BlockingPolicy {
        async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
            if info.call.name == "blocked_tool" {
                PreToolAction::Skip
            } else {
                PreToolAction::Continue
            }
        }
    }

    engine.set_interceptor(BlockingPolicy);

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let _result = engine.run("Test hook").await;

    // allowed_tool is called, but blocked_tool is not
    assert_eq!(
        allowed_clone.call_count(),
        1,
        "Allowed tool should be called"
    );
    assert_eq!(
        blocked_clone.call_count(),
        0,
        "Blocked tool should not be called"
    );
}

/// Hook: post_tool_call - verify that results can be modified
#[tokio::test]
async fn test_post_tool_call_modification() {
    // Prepare responses for multiple requests
    let client = MockLlmClient::with_responses(vec![
        // First request: tool call
        vec![
            Event::tool_use_start(0, "call_1", "test_tool"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        // Second request: text response after receiving tool result
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Done!"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);

    let mut engine = Engine::new(client);

    #[derive(Clone)]
    struct SimpleTool;

    #[async_trait]
    impl Tool for SimpleTool {
        async fn execute(
            &self,
            _: &str,
            _ctx: llm_engine::tool::ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok("Original Result".to_string().into())
        }
    }

    fn simple_tool_definition() -> ToolDefinition {
        Arc::new(|| {
            let meta = ToolMeta::new("test_tool")
                .description("Test")
                .input_schema(serde_json::json!({}));
            (meta, Arc::new(SimpleTool) as Arc<dyn Tool>)
        })
    }

    engine.register_tool(simple_tool_definition());

    // Policy to modify results
    struct ModifyingPolicy {
        modified_content: Arc<std::sync::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl Interceptor for ModifyingPolicy {
        async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
            info.result.summary = format!("[Modified] {}", info.result.summary);
            *self.modified_content.lock().unwrap() = Some(info.result.summary.clone());
            PostToolAction::Continue
        }
    }

    let modified_content = Arc::new(std::sync::Mutex::new(None));
    engine.set_interceptor(ModifyingPolicy {
        modified_content: modified_content.clone(),
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let result = engine.run("Test modification").await;

    assert!(result.is_ok(), "Engine should complete");

    // Verify hook was called and content was modified
    let content = modified_content.lock().unwrap().clone();
    assert!(content.is_some(), "Hook should have been called");
    assert!(
        content.unwrap().contains("[Modified]"),
        "Result should be modified"
    );
}

/// Hook: pre_tool_call synthetic result - skipped tool gets an error result in history.
#[tokio::test]
async fn test_before_tool_call_synthetic_result_committed() {
    let events = vec![
        Event::tool_use_start(0, "call_1", "blocked_tool"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::with_responses(vec![
        events,
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Denied."),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let blocked_tool = SlowTool::new("blocked_tool", 10);
    let blocked_clone = blocked_tool.clone();
    engine.register_tool(blocked_tool.definition());

    struct SyntheticPolicy;

    #[async_trait]
    impl Interceptor for SyntheticPolicy {
        async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
            PreToolAction::SyntheticResult(ToolResult::error(
                info.call.id.clone(),
                "permission denied",
            ))
        }
    }

    engine.set_interceptor(SyntheticPolicy);

    let result = engine.run("Test synthetic result").await.unwrap();

    assert_eq!(blocked_clone.call_count(), 0, "Blocked tool should not run");
    assert!(result.engine.history().iter().any(|item| matches!(
        item,
        llm_engine::Item::ToolResult {
            call_id,
            summary,
            is_error: true,
            ..
        } if call_id == "call_1" && summary == "permission denied"
    )));
}
