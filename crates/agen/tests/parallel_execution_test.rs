//! Parallel tool execution tests
//!
//! Verify that Engine executes multiple tools in parallel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agen::interceptor::{Interceptor, PostToolAction, PreToolAction, ToolCallInfo, ToolResultInfo};
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput, ToolResult,
    ToolResultDisposition,
};
use agen::{Engine, History, Item};
use async_trait::async_trait;

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
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(format!("Completed after {}ms", self.delay_ms).into())
    }
}

#[derive(Clone)]
struct FirstAttemptHangsTool {
    calls: Arc<AtomicUsize>,
}

impl FirstAttemptHangsTool {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new("hang_once")
                .description("Hangs on the first execution attempt")
                .input_schema(serde_json::json!({"type": "object"}));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for FirstAttemptHangsTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            std::future::pending::<()>().await;
        }
        Ok("completed on retry".to_string().into())
    }
}

#[derive(Clone)]
struct CooperativeCancelTool {
    calls: Arc<AtomicUsize>,
    cancelled: Arc<tokio::sync::Notify>,
}

impl CooperativeCancelTool {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new("cooperative")
                .description("Returns bounded progress after cancellation")
                .input_schema(serde_json::json!({"type": "object"}));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }
}

#[async_trait]
impl Tool for CooperativeCancelTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cancelled.notified().await;
        Err(ToolError::Cancelled(ToolOutput {
            summary: "cooperative command cancelled".to_string(),
            content: Some("stdout before cancellation\nstderr before cancellation".to_string()),
            attachments: Vec::new(),
        }))
    }

    async fn cancel(&self, _call_id: &str) -> Result<(), ToolError> {
        self.cancelled.notify_one();
        Ok(())
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
    let mut history: History = History::new();
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
    let _result = engine.run(&mut history, "Run all tools").await;
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
async fn completed_results_commit_before_publish_without_waiting_for_siblings() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_slow", "slow_first"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::tool_use_start(1, "call_fast", "fast_second"),
            Event::tool_input_delta(1, r#"{}"#),
            Event::tool_use_stop(1),
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
    let client_probe = client.clone();
    let mut engine = Engine::new(client);
    engine.register_tool(SlowTool::new("slow_first", 100).definition());
    engine.register_tool(SlowTool::new("fast_second", 5).definition());

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let published = observed.clone();
    engine.on_tool_result(move |result| {
        published
            .lock()
            .unwrap()
            .push(format!("publish:{}", result.tool_use_id));
    });

    let committed = observed.clone();
    let mut annotate = move |item: &Item| {
        if let Item::ToolResult { call_id, .. } = item {
            committed.lock().unwrap().push(format!("commit:{call_id}"));
        }
        Ok(())
    };
    let mut history = History::new();
    let _ = engine
        .run_with_annotation(&mut history, "run both", &mut annotate)
        .await;
    observed.lock().unwrap().push("run-returned".to_string());

    assert_eq!(
        observed.lock().unwrap().as_slice(),
        [
            "commit:call_fast",
            "publish:call_fast",
            "commit:call_slow",
            "publish:call_slow",
            "run-returned",
        ]
    );

    let committed_order: Vec<_> = history
        .iter()
        .filter_map(|entry| match &entry.item {
            Item::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(committed_order, ["call_fast", "call_slow"]);

    let requests = client_probe.requests();
    let projected_order: Vec<_> = requests[1]
        .items
        .iter()
        .filter_map(|item| match item {
            Item::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(projected_order, ["call_slow", "call_fast"]);
}

#[tokio::test]
async fn cancellation_preserves_completed_results_and_resume_skips_them() {
    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_hang", "hang_once"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::tool_use_start(1, "call_fast_a", "fast_a"),
            Event::tool_input_delta(1, r#"{}"#),
            Event::tool_use_stop(1),
            Event::tool_use_start(2, "call_fast_b", "fast_b"),
            Event::tool_input_delta(2, r#"{}"#),
            Event::tool_use_stop(2),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_block_start(0),
            Event::text_delta(0, "Recovered"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ]);
    let mut engine = Engine::new(client);
    let hanging = FirstAttemptHangsTool::new();
    let fast_a = SlowTool::new("fast_a", 1);
    let fast_b = SlowTool::new("fast_b", 2);
    engine.register_tool(hanging.definition());
    engine.register_tool(fast_a.definition());
    engine.register_tool(fast_b.definition());

    let cancel = engine.cancel_sender();
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.send(()).await.unwrap();
    });
    let mut history = History::new();
    let output = engine.run(&mut history, "start").await;
    let mut engine = output.engine;
    cancel_task.await.unwrap();

    let completed_before_resume = history
        .iter()
        .filter(|entry| {
            matches!(
                &entry.item,
                Item::ToolResult { call_id, .. }
                    if call_id == "call_fast_a" || call_id == "call_fast_b"
            )
        })
        .count();
    let unknown_before_resume = history
        .iter()
        .filter(|entry| {
            matches!(
                &entry.item,
                Item::ToolResult {
                    call_id,
                    disposition: ToolResultDisposition::OutcomeUnknown,
                    ..
                } if call_id == "call_hang"
            )
        })
        .count();
    assert_eq!(completed_before_resume, 2);
    assert_eq!(unknown_before_resume, 1);
    assert_eq!(fast_a.call_count(), 1);
    assert_eq!(fast_b.call_count(), 1);
    assert_eq!(hanging.call_count(), 1);

    let _ = engine.resume(&mut history).await;

    assert_eq!(
        fast_a.call_count(),
        1,
        "completed call must not be re-executed"
    );
    assert_eq!(
        fast_b.call_count(),
        1,
        "completed call must not be re-executed"
    );
    assert_eq!(
        hanging.call_count(),
        1,
        "OutcomeUnknown is terminal and must not be re-executed"
    );
    let completed_after_resume = history
        .iter()
        .filter(|entry| {
            matches!(
                &entry.item,
                Item::ToolResult { call_id, .. }
                    if call_id == "call_fast_a" || call_id == "call_fast_b"
            )
        })
        .count();
    assert_eq!(completed_after_resume, 2);
    assert_eq!(
        history
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.item,
                    Item::ToolResult {
                        call_id,
                        disposition: ToolResultDisposition::OutcomeUnknown,
                        ..
                    } if call_id == "call_hang"
                )
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn cooperative_cancellation_commits_bounded_terminal_output() {
    let client = MockLlmClient::with_responses(vec![vec![
        Event::tool_use_start(0, "call_cooperative", "cooperative"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);
    let mut engine = Engine::new(client);
    let tool = CooperativeCancelTool::new();
    engine.register_tool(tool.definition());
    let observed = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let published = observed.clone();
    engine.on_tool_result(move |_| published.lock().unwrap().push("published"));
    let committed = observed.clone();
    let mut annotate = move |item: &Item| {
        if matches!(item, Item::ToolResult { .. }) {
            committed.lock().unwrap().push("committed");
        }
        Ok(())
    };

    let cancel = engine.cancel_sender();
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.send(()).await.unwrap();
    });
    let mut history = History::new();
    let output = engine
        .run_with_annotation(&mut history, "start", &mut annotate)
        .await;
    observed.lock().unwrap().push("run-returned");
    cancel_task.await.unwrap();

    assert_eq!(
        observed.lock().unwrap().as_slice(),
        ["committed", "published", "run-returned"]
    );
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let terminal: Vec<_> = history
        .iter()
        .filter_map(|entry| match &entry.item {
            Item::ToolResult {
                call_id,
                disposition,
                content,
                ..
            } if call_id == "call_cooperative" => Some((*disposition, content.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].0, ToolResultDisposition::Cancelled);
    assert_eq!(
        terminal[0].1,
        Some("stdout before cancellation\nstderr before cancellation")
    );
    assert!(matches!(
        output.result,
        agen::EngineRunExit::Interrupted(agen::StopReason::Cancelled)
    ));
}

#[tokio::test]
async fn cancellation_completion_race_commits_one_terminal_output() {
    for iteration in 0..24u64 {
        let client = MockLlmClient::with_responses(vec![
            vec![
                Event::tool_use_start(0, "call_racy", "racy"),
                Event::tool_input_delta(0, r#"{}"#),
                Event::tool_use_stop(0),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ],
            vec![Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            })],
        ]);
        let mut engine = Engine::new(client);
        let delay = 2 + iteration % 3;
        let tool = SlowTool::new("racy", delay);
        engine.register_tool(tool.definition());
        let cancel = engine.cancel_sender();
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let _ = cancel.send(()).await;
        });

        let mut history = History::new();
        let _ = engine.run(&mut history, "race").await;
        cancel_task.await.unwrap();
        let terminal_count = history
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.item,
                    Item::ToolResult { call_id, .. } if call_id == "call_racy"
                )
            })
            .count();
        assert_eq!(terminal_count, 1, "iteration {iteration}");
        assert_eq!(tool.call_count(), 1, "iteration {iteration}");
    }
}

#[tokio::test]
async fn tool_result_commit_failure_prevents_publication() {
    let client = MockLlmClient::with_responses(vec![vec![
        Event::tool_use_start(0, "call_fast", "fast"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);
    let mut engine = Engine::new(client);
    engine.register_tool(SlowTool::new("fast", 1).definition());

    let published = Arc::new(AtomicUsize::new(0));
    let published_probe = published.clone();
    engine.on_tool_result(move |_| {
        published_probe.fetch_add(1, Ordering::SeqCst);
    });

    let mut history = History::new();
    let mut reject_tool_result = |item: &Item| {
        if matches!(item, Item::ToolResult { .. }) {
            Err("session log unavailable".to_string())
        } else {
            Ok(())
        }
    };
    let _ = engine
        .run_with_annotation(&mut history, "start", &mut reject_tool_result)
        .await;

    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert!(
        history
            .iter()
            .all(|entry| !matches!(entry.item, Item::ToolResult { .. }))
    );
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
    let mut history: History = History::new();
    let contexts = Arc::new(Mutex::new(Vec::new()));

    engine.register_tool(ContextRecordingTool::new("record_a", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("record_b", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("record_c", contexts.clone()).definition());

    let _ = engine.run(&mut history, "record contexts").await;

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
    let mut history: History = History::new();
    let contexts = Arc::new(Mutex::new(Vec::new()));

    engine.register_tool(ContextRecordingTool::new("record", contexts.clone()).definition());

    let _ = engine.run(&mut history, "record batches").await;

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
    let mut history: History = History::new();
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

    let _ = engine
        .run(&mut history, "record skipped and synthetic contexts")
        .await;

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
    let mut history: History = History::new();

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
    let _result = engine.run(&mut history, "Test hook").await;

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
    let mut history: History = History::new();

    #[derive(Clone)]
    struct SimpleTool;

    #[async_trait]
    impl Tool for SimpleTool {
        async fn execute(
            &self,
            _: &str,
            _ctx: agen::tool::ToolExecutionContext,
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
    let result = engine.run(&mut history, "Test modification").await;

    assert!(
        matches!(result.result, agen::EngineRunExit::Finished),
        "Engine should complete"
    );

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
    let mut history: History = History::new();
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

    let _result = engine.run(&mut history, "Test synthetic result").await;

    assert_eq!(blocked_clone.call_count(), 0, "Blocked tool should not run");
    assert!(history.items().any(|item| matches!(
        item,
        agen::Item::ToolResult {
            call_id,
            summary,
            is_error: true,
            ..
        } if call_id == "call_1" && summary == "permission denied"
    )));
}
