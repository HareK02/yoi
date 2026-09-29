//! Parallel tool execution tests
//!
//! Verify that Engine executes multiple tools in parallel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agen::interceptor::{
    AssistantTurnEndContext, Interceptor, InterceptorError, InterceptorErrorCategory,
    InterceptorPhase, InterceptorResult, PostToolAction, PreToolAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::llm_client::{
    ClientError, LlmClient, Request, ResponseStream, ToolCallCompletionSupport,
};
use agen::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput, ToolResult,
    ToolResultDisposition,
};
use agen::{
    Engine, EngineError, EngineRunExit, History, Item, RunInterruptionReason, ToolCallDispatchMode,
    ToolExecutionPolicy,
};
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
struct SafePauseTool {
    calls: Arc<AtomicUsize>,
    cancellations: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl SafePauseTool {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            cancellations: Arc::new(AtomicUsize::new(0)),
            release: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new("safe_pause")
                .description("Waits for a safe-boundary release")
                .input_schema(serde_json::json!({"type": "object"}));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }
}

#[async_trait]
impl Tool for SafePauseTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Ok(ToolOutput {
            summary: "safe-boundary complete".to_string(),
            content: Some("safe-boundary complete".to_string()),
            attachments: Vec::new(),
        })
    }

    async fn cancel(&self, _call_id: &str) -> Result<(), ToolError> {
        self.cancellations.fetch_add(1, Ordering::SeqCst);
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

#[derive(Clone)]
struct ControlledStreamClient {
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Result<Event, ClientError>>>>>,
    completion_support: ToolCallCompletionSupport,
    stream_calls: Arc<AtomicUsize>,
}

impl ControlledStreamClient {
    fn new(
        completion_support: ToolCallCompletionSupport,
    ) -> (
        Self,
        tokio::sync::mpsc::UnboundedSender<Result<Event, ClientError>>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                receiver: Arc::new(Mutex::new(Some(rx))),
                completion_support,
                stream_calls: Arc::new(AtomicUsize::new(0)),
            },
            tx,
        )
    }

    fn stream_count(&self) -> usize {
        self.stream_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmClient for ControlledStreamClient {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let mut rx = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| ClientError::Api {
                status: Some(500),
                code: Some("controlled_stream_exhausted".to_string()),
                message: "controlled stream already consumed".to_string(),
                retry_after: None,
            })?;
        Ok(Box::pin(futures::stream::poll_fn(move |cx| {
            rx.poll_recv(cx)
        })))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        self.completion_support
    }
}

#[derive(Clone)]
struct MultiControlledStreamClient {
    receivers: Arc<
        Mutex<
            std::collections::VecDeque<
                tokio::sync::mpsc::UnboundedReceiver<Result<Event, ClientError>>,
            >,
        >,
    >,
}

impl MultiControlledStreamClient {
    fn new(
        response_count: usize,
    ) -> (
        Self,
        Vec<tokio::sync::mpsc::UnboundedSender<Result<Event, ClientError>>>,
    ) {
        let mut senders = Vec::with_capacity(response_count);
        let mut receivers = std::collections::VecDeque::with_capacity(response_count);
        for _ in 0..response_count {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            senders.push(tx);
            receivers.push_back(rx);
        }
        (
            Self {
                receivers: Arc::new(Mutex::new(receivers)),
            },
            senders,
        )
    }
}

#[async_trait]
impl LlmClient for MultiControlledStreamClient {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        let mut rx =
            self.receivers
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ClientError::Api {
                    status: Some(500),
                    code: Some("multi_controlled_stream_exhausted".to_string()),
                    message: "controlled responses exhausted".to_string(),
                    retry_after: None,
                })?;
        Ok(Box::pin(futures::stream::poll_fn(move |cx| {
            rx.poll_recv(cx)
        })))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::PerBlock
    }
}

#[derive(Clone)]
struct BoundedStreamClient {
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<Result<Event, ClientError>>>>>,
}

impl BoundedStreamClient {
    fn new(capacity: usize) -> (Self, tokio::sync::mpsc::Sender<Result<Event, ClientError>>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);
        (
            Self {
                receiver: Arc::new(Mutex::new(Some(rx))),
            },
            tx,
        )
    }
}

#[async_trait]
impl LlmClient for BoundedStreamClient {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        let mut rx = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("bounded stream is consumed once");
        Ok(Box::pin(futures::stream::poll_fn(move |cx| {
            rx.poll_recv(cx)
        })))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::PerBlock
    }
}

#[derive(Clone)]
struct BarrierTool {
    name: &'static str,
    starts: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl BarrierTool {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            starts: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            (
                ToolMeta::new(tool.name).input_schema(serde_json::json!({"type":"object"})),
                Arc::new(tool.clone()) as Arc<dyn Tool>,
            )
        })
    }
}

#[async_trait]
impl Tool for BarrierTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        Ok(format!("{} complete", self.name).into())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn default_dispatch_waits_for_response_completion() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = BarrierTool::new("barrier_default");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.register_tool(tool.definition());

    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.on_stream_event(move |_, _, event| {
        if matches!(event, Event::Ping(_)) {
            let _ = observed_tx.send(());
        }
    });
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_default",
        "barrier_default",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tx.send(Ok(Event::ping())).unwrap();
    observed_rx.recv().await.unwrap();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);

    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("tool starts after response completion");
    probe.release.notify_one();
    let _ = run.await.unwrap();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn turn_end_policy_incompatibility_visibly_falls_back_before_side_effects() {
    struct TurnEndPause;

    #[async_trait]
    impl Interceptor for TurnEndPause {
        async fn on_assistant_turn_end(
            &self,
            _context: AssistantTurnEndContext<'_>,
        ) -> InterceptorResult<TurnEndAction> {
            Ok(TurnEndAction::Pause)
        }
    }

    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = BarrierTool::new("turn_end_fallback");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(TurnEndPause);
    engine.register_tool(tool.definition());
    let (warning_tx, mut warning_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.on_warning(move |warning| {
        let _ = warning_tx.send(warning.to_string());
    });
    let (ping_tx, mut ping_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.on_stream_event(move |_, _, event| {
        if matches!(event, Event::Ping(_)) {
            let _ = ping_tx.send(());
        }
    });

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_turn_end_fallback",
        "turn_end_fallback",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tx.send(Ok(Event::ping())).unwrap();
    ping_rx.recv().await.unwrap();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    assert!(
        warning_rx
            .recv()
            .await
            .is_some_and(|warning| warning.contains("whole-response turn-end gate"))
    );

    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    let (output, _history) = run.await.unwrap();
    assert!(matches!(output.result, EngineRunExit::Paused));
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn blocking_pre_tool_policy_keeps_receiving_stream_and_services_pause() {
    struct BlockingEarlyPolicy {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Interceptor for BlockingEarlyPolicy {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn pre_tool_call(
            &self,
            _info: &mut ToolCallInfo<'_>,
        ) -> InterceptorResult<PreToolAction> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(PreToolAction::Continue)
        }
    }

    let (client, tx) = BoundedStreamClient::new(1);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let tool = SlowTool::new("blocked_policy_tool", 0);
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(BlockingEarlyPolicy {
        entered: entered.clone(),
        release,
    });
    engine.register_tool(tool.definition());
    let pause = engine.pause_sender();
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_blocked_policy",
        "blocked_policy_tool",
    )))
    .await
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#)))
        .await
        .unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("pre-tool policy entered");

    tx.send(Ok(Event::ping())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), tx.send(Ok(Event::ping())))
        .await
        .expect("background pump continues draining the bounded provider stream")
        .unwrap();
    pause.send(()).await.unwrap();
    drop(tx);

    let (output, mut history) = run.await.unwrap();
    assert!(matches!(output.result, EngineRunExit::Paused));
    assert_eq!(probe.call_count(), 0);
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolCall {
            call_id,
            execution_id: None,
            status: Some(agen::llm_client::ItemStatus::Completed),
            ..
        } if call_id == "call_blocked_policy"
    )));

    let mut restored = Engine::new(MockLlmClient::new(vec![Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })]));
    restored.register_tool(probe.definition());
    let _ = restored.resume(&mut history).await;
    assert_eq!(probe.call_count(), 1);
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolResult { call_id, .. } if call_id == "call_blocked_policy"
    )));
}

#[tokio::test]
async fn blocking_pre_tool_policy_keeps_receiving_stream_and_services_cancel() {
    struct BlockingEarlyPolicy {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Interceptor for BlockingEarlyPolicy {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn pre_tool_call(
            &self,
            _info: &mut ToolCallInfo<'_>,
        ) -> InterceptorResult<PreToolAction> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(PreToolAction::Continue)
        }
    }

    let (client, tx) = BoundedStreamClient::new(1);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let tool = SlowTool::new("cancelled_policy_tool", 0);
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(BlockingEarlyPolicy {
        entered: entered.clone(),
        release,
    });
    engine.register_tool(tool.definition());
    let cancel = engine.cancel_sender();
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_cancelled_policy",
        "cancelled_policy_tool",
    )))
    .await
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#)))
        .await
        .unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("pre-tool policy entered");

    tx.send(Ok(Event::ping())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), tx.send(Ok(Event::ping())))
        .await
        .expect("background pump continues draining the bounded provider stream")
        .unwrap();
    cancel.send(()).await.unwrap();
    drop(tx);

    let (output, mut history) = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("cancel does not deadlock the bounded stream pump")
        .unwrap();
    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Cancelled)
    ));
    assert_eq!(probe.call_count(), 0);
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolCall {
            call_id,
            execution_id: None,
            status: Some(agen::llm_client::ItemStatus::Completed),
            ..
        } if call_id == "call_cancelled_policy"
    )));

    let mut restored = Engine::new(MockLlmClient::new(vec![Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })]));
    restored.register_tool(probe.definition());
    let _ = restored.resume(&mut history).await;
    assert_eq!(probe.call_count(), 1);
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolResult { call_id, .. } if call_id == "call_cancelled_policy"
    )));
}

#[tokio::test]
async fn per_block_dispatch_starts_siblings_without_waiting_for_stream_or_prior_result() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let first = BarrierTool::new("barrier_a");
    let second = BarrierTool::new("barrier_b");
    let first_probe = first.clone();
    let second_probe = second.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(first.definition());
    engine.register_tool(second.definition());

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run both").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(0, "call_a", "barrier_a")))
        .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_probe.started.notified())
        .await
        .expect("first tool starts while stream is open");

    tx.send(Ok(Event::tool_use_start(1, "call_b", "barrier_b")))
        .unwrap();
    tx.send(Ok(Event::tool_input_delta(1, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(1))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), second_probe.started.notified())
        .await
        .expect("second tool starts before first completes");
    assert_eq!(first_probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(second_probe.starts.load(Ordering::SeqCst), 1);

    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    first_probe.release.notify_one();
    second_probe.release.notify_one();
    let (_output, history) = run.await.unwrap();

    let calls = history
        .iter()
        .filter(|entry| matches!(entry.item, Item::ToolCall { .. }))
        .count();
    let results = history
        .iter()
        .filter(|entry| matches!(entry.item, Item::ToolResult { .. }))
        .count();
    assert_eq!((calls, results), (2, 2));
}

#[tokio::test]
async fn early_terminal_result_commits_while_response_stream_is_open() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = BarrierTool::new("commit_while_streaming");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());
    let (committed_tx, mut committed_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.on_history_append(move |item| {
        if matches!(item, Item::ToolResult { call_id, .. } if call_id == "call_commit_open") {
            let _ = committed_tx.send(());
        }
        Ok(())
    });

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_commit_open",
        "commit_while_streaming",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("tool starts while response remains open");
    probe.release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), committed_rx.recv())
        .await
        .expect("terminal result is durably committed before response release")
        .expect("commit notification channel remains open");

    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    let _ = run.await.unwrap();
}

#[derive(Clone, Copy)]
enum BlockingPostControl {
    Pause,
    Cancel,
}

async fn assert_blocking_post_hook_services_control(control: BlockingPostControl) {
    struct BlockingPostPolicy {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Interceptor for BlockingPostPolicy {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn post_tool_call(
            &self,
            _info: &ToolResultInfo<'_>,
        ) -> InterceptorResult<PostToolAction> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(PostToolAction::Continue)
        }
    }

    let (client, tx) = BoundedStreamClient::new(1);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let tool = SlowTool::new("blocking_post_tool", 0);
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(BlockingPostPolicy {
        entered: entered.clone(),
        release,
    });
    engine.register_tool(tool.definition());
    let pause = engine.pause_sender();
    let cancel = engine.cancel_sender();
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_blocking_post",
        "blocking_post_tool",
    )))
    .await
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#)))
        .await
        .unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("post-tool policy entered after the terminal result was committed");

    tx.send(Ok(Event::ping())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), tx.send(Ok(Event::ping())))
        .await
        .expect("background pump continues draining while post-tool policy blocks")
        .unwrap();
    match control {
        BlockingPostControl::Pause => pause.send(()).await.unwrap(),
        BlockingPostControl::Cancel => cancel.send(()).await.unwrap(),
    }
    drop(tx);

    let (output, history) = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("control request interrupts the blocked post-tool policy")
        .unwrap();
    match control {
        BlockingPostControl::Pause => assert!(matches!(output.result, EngineRunExit::Paused)),
        BlockingPostControl::Cancel => assert!(matches!(
            output.result,
            EngineRunExit::Interrupted(RunInterruptionReason::Cancelled)
        )),
    }
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolResult { call_id, .. } if call_id == "call_blocking_post"
    )));
}

#[tokio::test]
async fn blocking_post_tool_policy_services_pause_after_result_commit() {
    assert_blocking_post_hook_services_control(BlockingPostControl::Pause).await;
}

#[tokio::test]
async fn blocking_post_tool_policy_services_cancel_after_result_commit() {
    assert_blocking_post_hook_services_control(BlockingPostControl::Cancel).await;
}

#[tokio::test]
async fn reverse_stop_order_preserves_model_call_indexes() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(ContextRecordingTool::new("ordered_a", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("ordered_b", contexts.clone()).definition());

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(0, "call_order_a", "ordered_a")))
        .unwrap();
    tx.send(Ok(Event::tool_use_start(1, "call_order_b", "ordered_b")))
        .unwrap();
    tx.send(Ok(Event::tool_input_delta(1, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(1))).unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    let _ = run.await.unwrap();

    let by_id = contexts
        .lock()
        .unwrap()
        .iter()
        .map(|context| (context.call_id.clone(), context.call_index))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(by_id.get("call_order_a"), Some(&0));
    assert_eq!(by_id.get("call_order_b"), Some(&1));
}

#[tokio::test]
async fn early_pre_and_post_hooks_share_the_exact_response_turn_identity() {
    struct TurnRecordingPolicy {
        events: tokio::sync::mpsc::UnboundedSender<(InterceptorPhase, String, u64)>,
    }

    #[async_trait]
    impl Interceptor for TurnRecordingPolicy {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn pre_tool_call(
            &self,
            info: &mut ToolCallInfo<'_>,
        ) -> InterceptorResult<PreToolAction> {
            self.events
                .send((
                    InterceptorPhase::PreToolCall,
                    info.call.id.clone(),
                    info.invocation.turn_id.expect("pre hook turn").0,
                ))
                .unwrap();
            Ok(PreToolAction::Continue)
        }

        async fn post_tool_call(
            &self,
            info: &ToolResultInfo<'_>,
        ) -> InterceptorResult<PostToolAction> {
            self.events
                .send((
                    InterceptorPhase::PostToolCall,
                    info.call.id.clone(),
                    info.invocation.turn_id.expect("post hook turn").0,
                ))
                .unwrap();
            Ok(PostToolAction::Continue)
        }
    }

    let (client, mut responses) = MultiControlledStreamClient::new(3);
    let first = responses.remove(0);
    let second = responses.remove(0);
    let third = responses.remove(0);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(TurnRecordingPolicy { events: event_tx });
    engine.register_tool(SlowTool::new("turn_identity", 0).definition());
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    first
        .send(Ok(Event::tool_use_start(0, "call_turn_0", "turn_identity")))
        .unwrap();
    first.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    first.send(Ok(Event::tool_use_stop(0))).unwrap();
    let pre_0 = event_rx.recv().await.unwrap();
    let post_0 = event_rx.recv().await.unwrap();
    assert_eq!(
        pre_0,
        (InterceptorPhase::PreToolCall, "call_turn_0".into(), 0)
    );
    assert_eq!(
        post_0,
        (InterceptorPhase::PostToolCall, "call_turn_0".into(), 0)
    );
    first
        .send(Ok(Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        })))
        .unwrap();
    drop(first);

    second
        .send(Ok(Event::tool_use_start(0, "call_turn_1", "turn_identity")))
        .unwrap();
    second
        .send(Ok(Event::tool_input_delta(0, r#"{}"#)))
        .unwrap();
    second.send(Ok(Event::tool_use_stop(0))).unwrap();
    let pre_1 = event_rx.recv().await.unwrap();
    let post_1 = event_rx.recv().await.unwrap();
    assert_eq!(
        pre_1,
        (InterceptorPhase::PreToolCall, "call_turn_1".into(), 1)
    );
    assert_eq!(
        post_1,
        (InterceptorPhase::PostToolCall, "call_turn_1".into(), 1)
    );
    second
        .send(Ok(Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        })))
        .unwrap();
    drop(second);

    third.send(Ok(Event::text_delta(0, "done"))).unwrap();
    third
        .send(Ok(Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        })))
        .unwrap();
    drop(third);
    let (output, _) = run.await.unwrap();
    assert!(matches!(output.result, EngineRunExit::Finished));
}

#[tokio::test]
async fn reverse_stop_order_projects_next_provider_request_in_model_order() {
    let first_response = vec![
        Event::tool_use_start(0, "call_project_a", "project_a"),
        Event::tool_use_start(1, "call_project_b", "project_b"),
        Event::tool_input_delta(1, r#"{}"#),
        Event::tool_use_stop(1),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];
    let second_response = vec![
        Event::text_delta(0, "done"),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];
    let client = MockLlmClient::with_responses(vec![first_response, second_response])
        .with_completion_support(ToolCallCompletionSupport::PerBlock);
    let probe = client.clone();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(ContextRecordingTool::new("project_a", contexts.clone()).definition());
    engine.register_tool(ContextRecordingTool::new("project_b", contexts).definition());
    let mut history = History::new();

    let _ = engine.run(&mut history, "run").await;

    let requests = probe.requests();
    assert_eq!(requests.len(), 2);
    let tool_order = requests[1]
        .items
        .iter()
        .filter_map(|item| match item {
            Item::ToolCall { call_id, .. } => Some(format!("call:{call_id}")),
            Item::ToolResult { call_id, .. } => Some(format!("result:{call_id}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_order,
        [
            "call:call_project_a",
            "call:call_project_b",
            "result:call_project_a",
            "result:call_project_b",
        ]
    );
}

#[tokio::test]
async fn later_history_failure_terminalizes_already_started_early_sibling() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let first = BarrierTool::new("history_first");
    let second = BarrierTool::new("history_second");
    let first_probe = first.clone();
    let second_probe = second.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_tool_execution_policy(ToolExecutionPolicy {
        pause_safe_boundary_timeout: Duration::from_millis(1),
        cancellation_request_timeout: Duration::from_millis(1),
        terminal_confirmation_timeout: Duration::from_millis(1),
    });
    engine.register_tool(first.definition());
    engine.register_tool(second.definition());
    engine.on_history_append(|item| match item {
        Item::ToolCall { call_id, .. } if call_id == "call_history_second" => {
            Err("durable append rejected".to_string())
        }
        _ => Ok(()),
    });

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_history_first",
        "history_first",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_probe.started.notified())
        .await
        .expect("first side effect starts");
    tx.send(Ok(Event::tool_use_start(
        1,
        "call_history_second",
        "history_second",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(1, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(1))).unwrap();
    drop(tx);

    let (output, history) = run.await.unwrap();
    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
            EngineError::HistoryAppend(ref message)
        )) if message == "durable append rejected"
    ));
    assert_eq!(first_probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(second_probe.starts.load(Ordering::SeqCst), 0);
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::OutcomeUnknown,
            ..
        } if call_id == "call_history_first"
    )));
}

#[tokio::test]
async fn persisted_started_call_is_closed_unknown_and_never_resumed() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = SlowTool::new("must_not_resume_started", 0);
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.register_tool(tool.definition());
    let mut history = History::from_items(vec![
        Item::user_message("run"),
        Item::tool_call_json(
            "call_started_before_restore",
            "must_not_resume_started",
            serde_json::json!({}),
        )
        .with_tool_execution_metadata(
            0,
            Some("restored-batch:call_started_before_restore".to_string()),
        )
        .with_status(agen::llm_client::ItemStatus::InProgress),
    ]);

    let run = tokio::spawn(async move {
        let output = engine.resume(&mut history).await;
        (output, history)
    });
    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);
    let (_output, history) = run.await.unwrap();

    assert_eq!(probe.call_count(), 0);
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::OutcomeUnknown,
            ..
        } if call_id == "call_started_before_restore"
    )));
}

#[tokio::test]
async fn rejected_synthetic_result_commit_restores_as_non_runnable() {
    struct SyntheticDenial;

    #[async_trait]
    impl Interceptor for SyntheticDenial {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn pre_tool_call(
            &self,
            info: &mut ToolCallInfo<'_>,
        ) -> InterceptorResult<PreToolAction> {
            Ok(PreToolAction::SyntheticResult(ToolResult::error(
                &info.call.id,
                "permission denied",
            )))
        }
    }

    let client = MockLlmClient::with_responses(vec![
        vec![
            Event::tool_use_start(0, "call_denied_restore", "denied_restore"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
        vec![
            Event::text_delta(0, "restored safely"),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ],
    ])
    .with_completion_support(ToolCallCompletionSupport::PerBlock);
    let tool = SlowTool::new("denied_restore", 0);
    let probe = tool.clone();
    let reject_once = Arc::new(AtomicUsize::new(1));
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(SyntheticDenial);
    engine.register_tool(tool.definition());
    let reject_result = reject_once.clone();
    engine.on_history_append(move |item| {
        if matches!(item, Item::ToolResult { call_id, .. } if call_id == "call_denied_restore")
            && reject_result.swap(0, Ordering::SeqCst) == 1
        {
            return Err("injected synthetic result commit failure".to_string());
        }
        Ok(())
    });
    let mut history = History::new();

    let output = engine.run(&mut history, "run denied tool").await;
    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
            EngineError::HistoryAppend(ref message)
        )) if message == "injected synthetic result commit failure"
    ));
    assert_eq!(probe.call_count(), 0);
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolCall {
            call_id,
            execution_id: Some(_),
            status: Some(agen::llm_client::ItemStatus::InProgress),
            ..
        } if call_id == "call_denied_restore"
    )));
    assert!(!history.items().any(|item| matches!(
        item,
        Item::ToolResult { call_id, .. } if call_id == "call_denied_restore"
    )));

    let mut engine = output.engine;
    let resumed = engine.resume(&mut history).await;
    assert!(matches!(resumed, EngineRunExit::Finished));
    assert_eq!(
        probe.call_count(),
        0,
        "denied side effect must never run on restore"
    );
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::OutcomeUnknown,
            ..
        } if call_id == "call_denied_restore"
    )));
}

#[tokio::test]
async fn abort_during_early_admission_commits_synthetic_terminal() {
    struct AbortAdmission;

    #[async_trait]
    impl Interceptor for AbortAdmission {
        fn supports_early_tool_dispatch(&self) -> bool {
            true
        }

        async fn pre_tool_call(
            &self,
            _info: &mut ToolCallInfo<'_>,
        ) -> InterceptorResult<PreToolAction> {
            Ok(PreToolAction::Abort("denied after completion".to_string()))
        }
    }

    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = SlowTool::new("must_not_run_aborted", 0);
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.set_interceptor(AbortAdmission);
    engine.register_tool(tool.definition());
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_aborted_admission",
        "must_not_run_aborted",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    drop(tx);
    let (output, history) = run.await.unwrap();

    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(EngineError::Aborted(ref reason)))
            if reason == "denied after completion"
    ));
    assert_eq!(probe.call_count(), 0);
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            summary,
            ..
        } if call_id == "call_aborted_admission" && summary.contains("denied after completion")
    )));
}

#[tokio::test]
async fn malformed_completed_arguments_commit_error_without_executing_tool() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = SlowTool::new("must_not_run", 0);
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());
    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });

    tx.send(Ok(Event::tool_use_start(
        0,
        "call_malformed",
        "must_not_run",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{"broken":"#)))
        .unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tx.send(Ok(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    })))
    .unwrap();
    drop(tx);

    let (_output, history) = run.await.unwrap();
    assert_eq!(probe.call_count(), 0);
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            summary,
            ..
        } if call_id == "call_malformed" && summary.contains("complete JSON object")
    )));
}

#[tokio::test]
async fn pause_during_early_execution_drains_result_and_resume_does_not_rerun() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = BarrierTool::new("barrier_pause");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());
    let pause = engine.pause_sender();

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(0, "call_pause", "barrier_pause")))
        .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("tool starts before pause");
    pause.send(()).await.unwrap();
    probe.release.notify_one();
    drop(tx);

    let (output, mut history) = run.await.unwrap();
    assert!(matches!(output.result, EngineRunExit::Paused));
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult { call_id, .. } if call_id == "call_pause"
    )));
    let mut engine = output.engine;
    let _ = engine.resume(&mut history).await;
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn interrupted_stream_after_early_start_terminalizes_without_continuation_or_reexecution() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let tool = BarrierTool::new("barrier_interrupted");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_interrupted",
        "barrier_interrupted",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("tool starts before stream interruption");
    tx.send(Err(ClientError::Api {
        status: None,
        code: Some("connection_lost".to_string()),
        message: "connection lost".to_string(),
        retry_after: None,
    }))
    .unwrap();
    probe.release.notify_one();
    drop(tx);

    let (output, mut history) = run.await.unwrap();
    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(EngineError::Client(
            ClientError::Api {
                code: Some(ref code),
                ..
            }
        ))) if code == "early_tool_stream_interrupted"
    ));
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(&entry.item, Item::ToolCall { call_id, .. } if call_id == "call_interrupted"))
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(&entry.item, Item::ToolResult { call_id, .. } if call_id == "call_interrupted"))
            .count(),
        1
    );

    let mut engine = output.engine;
    let _ = engine.resume(&mut history).await;
    assert_eq!(
        probe.starts.load(Ordering::SeqCst),
        1,
        "terminalized early calls must not be resumed"
    );
}

#[tokio::test]
async fn clean_eof_without_provider_completion_after_early_start_stops_without_continuation() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::PerBlock);
    let client_probe = client.clone();
    let tool = BarrierTool::new("barrier_clean_eof");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_clean_eof",
        "barrier_clean_eof",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("tool starts before clean transport EOF");
    probe.release.notify_one();
    drop(tx);

    let (output, history) = run.await.unwrap();
    assert!(matches!(
        output.result,
        EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(EngineError::Client(
            ClientError::Api {
                code: Some(ref code),
                ..
            }
        ))) if code == "early_tool_stream_interrupted"
    ));
    assert_eq!(
        client_probe.stream_count(),
        1,
        "must not open a continuation request"
    );
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        history
            .items()
            .filter(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "call_clean_eof"))
            .count(),
        1
    );
}

#[tokio::test]
async fn unsupported_provider_visibly_defers_opt_in_dispatch() {
    let (client, tx) = ControlledStreamClient::new(ToolCallCompletionSupport::ResponseComplete);
    let tool = BarrierTool::new("barrier_fallback");
    let probe = tool.clone();
    let mut engine = Engine::new(client);
    engine.set_max_turns(Some(1));
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(tool.definition());
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let warning_probe = warnings.clone();
    engine.on_warning(move |warning| warning_probe.lock().unwrap().push(warning.to_string()));
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.on_stream_event(move |_, _, event| {
        if matches!(event, Event::Ping(_)) {
            let _ = observed_tx.send(());
        }
    });

    let run = tokio::spawn(async move {
        let mut history = History::new();
        let output = engine.run(&mut history, "run").await;
        (output, history)
    });
    tx.send(Ok(Event::tool_use_start(
        0,
        "call_fallback",
        "barrier_fallback",
    )))
    .unwrap();
    tx.send(Ok(Event::tool_input_delta(0, r#"{}"#))).unwrap();
    tx.send(Ok(Event::tool_use_stop(0))).unwrap();
    tx.send(Ok(Event::ping())).unwrap();
    observed_rx.recv().await.unwrap();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    assert!(warnings.lock().unwrap().iter().any(|warning| {
        warning.contains("provider does not expose a trustworthy per-call completion boundary")
    }));

    drop(tx);
    tokio::time::timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("fallback starts tool only after response completion");
    probe.release.notify_one();
    let _ = run.await.unwrap();
}

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
        agen::EngineRunExit::Interrupted(agen::RunInterruptionReason::Cancelled)
    ));
}

#[tokio::test]
async fn pause_waits_for_started_tool_terminal_without_cancelling_provider() {
    let client = MockLlmClient::with_responses(vec![vec![
        Event::tool_use_start(0, "call_safe_pause", "safe_pause"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);
    let mut engine = Engine::new(client);
    let tool = SafePauseTool::new();
    engine.register_tool(tool.definition());

    let pause = engine.pause_sender();
    let calls = Arc::clone(&tool.calls);
    let release = Arc::clone(&tool.release);
    let control = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tool execution starts");
        pause.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        release.notify_one();
    });

    let started_at = std::time::Instant::now();
    let mut history = History::new();
    let output = engine.run(&mut history, "pause safely").await;
    control.await.unwrap();

    assert!(started_at.elapsed() >= Duration::from_millis(50));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool.cancellations.load(Ordering::SeqCst), 0);
    assert!(matches!(output.result, agen::EngineRunExit::Paused));
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::Success,
            ..
        } if call_id == "call_safe_pause"
    )));
}

#[tokio::test]
async fn pause_escalates_to_explicit_cancel_and_confirm_after_safe_boundary_deadline() {
    let client = MockLlmClient::with_responses(vec![vec![
        Event::tool_use_start(0, "call_pause_cancel", "cooperative"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]]);
    let mut engine = Engine::new(client);
    engine.set_tool_execution_policy(ToolExecutionPolicy {
        pause_safe_boundary_timeout: Duration::from_millis(20),
        cancellation_request_timeout: Duration::from_millis(50),
        terminal_confirmation_timeout: Duration::from_millis(100),
    });
    let tool = CooperativeCancelTool::new();
    engine.register_tool(tool.definition());

    let pause = engine.pause_sender();
    let calls = Arc::clone(&tool.calls);
    let control = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tool execution starts");
        pause.send(()).await.unwrap();
    });

    let mut history = History::new();
    let output = engine.run(&mut history, "pause with escalation").await;
    control.await.unwrap();

    assert!(matches!(output.result, agen::EngineRunExit::Paused));
    assert!(history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::Cancelled,
            ..
        } if call_id == "call_pause_cancel"
    )));
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
        async fn pre_tool_call(
            &self,
            info: &mut ToolCallInfo<'_, ()>,
        ) -> InterceptorResult<PreToolAction> {
            self.pre_contexts.lock().unwrap().push(info.context.clone());
            Ok(match info.call.name.as_str() {
                "skip_tool" => PreToolAction::Skip,
                "synthetic_tool" => PreToolAction::SyntheticResult(ToolResult::from_output(
                    &info.call.id,
                    ToolOutput::from("synthetic result".to_string()),
                )),
                _ => PreToolAction::Continue,
            })
        }

        async fn post_tool_call(
            &self,
            info: &ToolResultInfo<'_, ()>,
        ) -> InterceptorResult<PostToolAction> {
            self.post_contexts
                .lock()
                .unwrap()
                .push(info.context.clone());
            Ok(PostToolAction::Continue)
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
        async fn pre_tool_call(
            &self,
            info: &mut ToolCallInfo<'_, ()>,
        ) -> InterceptorResult<PreToolAction> {
            Ok(if info.call.name == "blocked_tool" {
                PreToolAction::Skip
            } else {
                PreToolAction::Continue
            })
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

/// Hook: post_tool_call - verify that the committed terminal result is observed.
#[tokio::test]
async fn test_post_tool_call_observes_committed_result() {
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

    // Policy to observe the committed terminal result.
    struct ObservingPolicy {
        observed_content: Arc<std::sync::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl Interceptor for ObservingPolicy {
        async fn post_tool_call(
            &self,
            info: &ToolResultInfo<'_, ()>,
        ) -> InterceptorResult<PostToolAction> {
            assert_eq!(info.invocation.phase, InterceptorPhase::PostToolCall);
            assert_eq!(
                info.invocation.call_id,
                Some(agen::InterceptorCallId::Tool(info.call.id.clone()))
            );
            assert!(matches!(
                info.history.last().map(|entry| &entry.item),
                Some(Item::ToolResult { call_id, .. }) if call_id == &info.call.id
            ));
            *self.observed_content.lock().unwrap() = Some(info.result.summary.clone());
            Ok(PostToolAction::Continue)
        }
    }

    let observed_content = Arc::new(std::sync::Mutex::new(None));
    engine.set_interceptor(ObservingPolicy {
        observed_content: observed_content.clone(),
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let result = engine.run(&mut history, "Test observation").await;

    assert!(
        matches!(result.result, agen::EngineRunExit::Finished),
        "Engine should complete"
    );

    // Verify the interceptor observed the exact committed result.
    let observed = observed_content.lock().unwrap().clone();
    assert_eq!(observed.as_deref(), Some("Original Result"));
    assert!(history.items().any(|item| matches!(
        item,
        Item::ToolResult { summary, .. } if summary == "Original Result"
    )));
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
        async fn pre_tool_call(
            &self,
            info: &mut ToolCallInfo<'_, ()>,
        ) -> InterceptorResult<PreToolAction> {
            Ok(PreToolAction::SyntheticResult(ToolResult::error(
                info.call.id.clone(),
                "permission denied",
            )))
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

#[derive(Clone, Copy)]
enum InvalidIdentityMode {
    ContinuedCall,
    SyntheticResult,
}

struct InvalidIdentityPolicy(InvalidIdentityMode);

#[async_trait]
impl Interceptor for InvalidIdentityPolicy {
    async fn pre_tool_call(
        &self,
        info: &mut ToolCallInfo<'_, ()>,
    ) -> InterceptorResult<PreToolAction> {
        assert_eq!(info.invocation.phase, InterceptorPhase::PreToolCall);
        assert_eq!(
            info.invocation.call_id,
            Some(agen::InterceptorCallId::Tool("call_1".to_string()))
        );
        assert!(matches!(
            info.history.last().map(|entry| &entry.item),
            Some(Item::ToolCall { call_id, .. }) if call_id == "call_1"
        ));
        Ok(match self.0 {
            InvalidIdentityMode::ContinuedCall => {
                info.call.id = "different-call".to_string();
                PreToolAction::Continue
            }
            InvalidIdentityMode::SyntheticResult => PreToolAction::SyntheticResult(
                ToolResult::error("different-call", "invalid synthetic result"),
            ),
        })
    }
}

#[tokio::test]
async fn interceptor_cannot_change_tool_call_identity() {
    for mode in [
        InvalidIdentityMode::ContinuedCall,
        InvalidIdentityMode::SyntheticResult,
    ] {
        let client = MockLlmClient::new(vec![
            Event::tool_use_start(0, "call_1", "echo"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]);
        let mut engine = Engine::new(client);
        engine.register_tool(SlowTool::new("echo", 1).definition());
        engine.set_interceptor(InvalidIdentityPolicy(mode));
        let mut history = History::new();

        let result = engine.run(&mut history, "identity").await;
        let EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
            EngineError::Interceptor(failure),
        )) = result.result
        else {
            panic!("invalid tool identity must interrupt with a typed failure");
        };
        assert_eq!(failure.phase(), InterceptorPhase::PreToolCall);
        assert_eq!(
            failure.error().category(),
            InterceptorErrorCategory::ContractViolation
        );
        assert!(
            !history
                .items()
                .any(|item| matches!(item, Item::ToolResult { .. }))
        );
    }
}

#[tokio::test]
async fn post_tool_abort_commits_confirmed_result_before_stopping_run() {
    let client = MockLlmClient::new(vec![
        Event::tool_use_start(0, "call_confirmed", "confirmed"),
        Event::tool_input_delta(0, r#"{}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let mut engine = Engine::new(client);
    let tool = SlowTool::new("confirmed", 1);
    engine.register_tool(tool.definition());

    let observed = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    struct AbortAfterResult {
        lifecycle: Arc<Mutex<Vec<&'static str>>>,
    }
    #[async_trait]
    impl Interceptor for AbortAfterResult {
        async fn post_tool_call(
            &self,
            _info: &ToolResultInfo<'_, ()>,
        ) -> InterceptorResult<PostToolAction> {
            self.lifecycle.lock().unwrap().push("post_tool_call");
            Ok(PostToolAction::Abort("policy stopped the run".to_string()))
        }
    }
    engine.set_interceptor(AbortAfterResult {
        lifecycle: observed.clone(),
    });

    let published = observed.clone();
    engine.on_tool_result(move |_| published.lock().unwrap().push("published"));
    let committed = observed.clone();
    let mut annotate = move |item: &Item| {
        if matches!(item, Item::ToolResult { .. }) {
            committed.lock().unwrap().push("committed");
        }
        Ok(())
    };

    let mut history = History::new();
    let output = engine
        .run_with_annotation(&mut history, "run confirmed tool", &mut annotate)
        .await;
    observed.lock().unwrap().push("run-returned");

    assert_eq!(tool.call_count(), 1);
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        ["committed", "published", "post_tool_call", "run-returned"]
    );
    assert!(matches!(
        output.result,
        agen::EngineRunExit::Interrupted(agen::RunInterruptionReason::Unexpected(
            agen::EngineError::Aborted(ref reason)
        )) if reason == "policy stopped the run"
    ));
    let terminal: Vec<_> = history
        .iter()
        .filter_map(|entry| match &entry.item {
            Item::ToolResult {
                call_id,
                disposition,
                ..
            } if call_id == "call_confirmed" => Some(*disposition),
            _ => None,
        })
        .collect();
    assert_eq!(terminal, [ToolResultDisposition::Success]);
    assert!(!history.iter().any(|entry| matches!(
        &entry.item,
        Item::ToolResult {
            call_id,
            disposition: ToolResultDisposition::OutcomeUnknown,
            ..
        } if call_id == "call_confirmed"
    )));
}

#[derive(Clone, Copy)]
enum PostToolStopMode {
    Abort,
    Failure,
}

struct StopFirstParallelResult(PostToolStopMode);

#[async_trait]
impl Interceptor for StopFirstParallelResult {
    async fn post_tool_call(
        &self,
        info: &ToolResultInfo<'_, ()>,
    ) -> InterceptorResult<PostToolAction> {
        if info.call.id != "call_fast" {
            return Ok(PostToolAction::Continue);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        match self.0 {
            PostToolStopMode::Abort => Ok(PostToolAction::Abort("stop parallel batch".to_string())),
            PostToolStopMode::Failure => Err(InterceptorError::new(
                InterceptorErrorCategory::Policy,
                "reject parallel batch",
            )),
        }
    }
}

#[tokio::test]
async fn post_tool_stop_terminalizes_started_parallel_siblings_before_returning() {
    for mode in [PostToolStopMode::Abort, PostToolStopMode::Failure] {
        let first_response = vec![
            Event::tool_use_start(0, "call_fast", "fast"),
            Event::tool_input_delta(0, r#"{}"#),
            Event::tool_use_stop(0),
            Event::tool_use_start(1, "call_ready", "ready"),
            Event::tool_input_delta(1, r#"{}"#),
            Event::tool_use_stop(1),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ];
        let second_response = vec![
            Event::text_block_start(0),
            Event::text_delta(0, "next run completed"),
            Event::text_block_stop(0, None),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ];
        let client = MockLlmClient::with_responses(vec![first_response, second_response]);
        let mut engine = Engine::new(client);
        engine.register_tool(SlowTool::new("fast", 0).definition());
        engine.register_tool(SlowTool::new("ready", 1).definition());
        engine.set_interceptor(StopFirstParallelResult(mode));
        let mut history = History::new();

        let output = engine.run(&mut history, "parallel stop").await;
        match mode {
            PostToolStopMode::Abort => assert!(matches!(
                output.result,
                EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
                    EngineError::Aborted(ref reason)
                )) if reason == "stop parallel batch"
            )),
            PostToolStopMode::Failure => assert!(matches!(
                output.result,
                EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(
                    EngineError::Interceptor(ref failure)
                )) if failure.phase() == InterceptorPhase::PostToolCall
            )),
        }

        let terminal_ids: Vec<_> = history
            .iter()
            .filter_map(|entry| match &entry.item {
                Item::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(terminal_ids.len(), 2);
        assert!(terminal_ids.contains(&"call_fast"));
        assert!(terminal_ids.contains(&"call_ready"));

        let mut engine = output.engine;
        let next = engine.run(&mut history, "next run").await;
        assert!(matches!(next, EngineRunExit::Finished));
    }
}
