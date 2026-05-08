//! Parallel tool execution tests
//!
//! Verify that Worker executes multiple tools in parallel.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use llm_worker::Worker;
use llm_worker::interceptor::{
    Interceptor, PostToolAction, PreToolAction, ToolCallInfo, ToolResultInfo,
};
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput, ToolResult};

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
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(format!("Completed after {}ms", self.delay_ms).into())
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

    let client = MockLlmClient::new(events);
    let mut worker = Worker::new(client);

    // Each tool waits 100ms
    let tool1 = SlowTool::new("slow_tool_1", 100);
    let tool2 = SlowTool::new("slow_tool_2", 100);
    let tool3 = SlowTool::new("slow_tool_3", 100);

    let tool1_clone = tool1.clone();
    let tool2_clone = tool2.clone();
    let tool3_clone = tool3.clone();

    worker.register_tool(tool1.definition());
    worker.register_tool(tool2.definition());
    worker.register_tool(tool3.definition());

    let start = Instant::now();
    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let _result = worker.run("Run all tools").await;
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

/// Hook: pre_tool_call - verify that skipped tools are not executed
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
    let mut worker = Worker::new(client);

    let allowed_tool = SlowTool::new("allowed_tool", 10);
    let blocked_tool = SlowTool::new("blocked_tool", 10);

    let allowed_clone = allowed_tool.clone();
    let blocked_clone = blocked_tool.clone();

    worker.register_tool(allowed_tool.definition());
    worker.register_tool(blocked_tool.definition());

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

    worker.set_interceptor(BlockingPolicy);

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let _result = worker.run("Test hook").await;

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

    let mut worker = Worker::new(client);

    #[derive(Clone)]
    struct SimpleTool;

    #[async_trait]
    impl Tool for SimpleTool {
        async fn execute(&self, _: &str) -> Result<ToolOutput, ToolError> {
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

    worker.register_tool(simple_tool_definition());

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
    worker.set_interceptor(ModifyingPolicy {
        modified_content: modified_content.clone(),
    });

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let result = worker.run("Test modification").await;

    assert!(result.is_ok(), "Worker should complete");

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
    let mut worker = Worker::new(client);
    let blocked_tool = SlowTool::new("blocked_tool", 10);
    let blocked_clone = blocked_tool.clone();
    worker.register_tool(blocked_tool.definition());

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

    worker.set_interceptor(SyntheticPolicy);

    let result = worker.run("Test synthetic result").await.unwrap();

    assert_eq!(blocked_clone.call_count(), 0, "Blocked tool should not run");
    assert!(result.worker.history().iter().any(|item| matches!(
        item,
        llm_worker::Item::ToolResult {
            call_id,
            summary,
            is_error: true,
            ..
        } if call_id == "call_1" && summary == "permission denied"
    )));
}
