//! Closure callback API tests
//!
//! Tests for the closure-based event subscription API on Engine.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::event::{Event, ResponseStatus, StatusEvent as ClientStatusEvent};
use agen::llm_client::retry::RetryPolicy;
use agen::llm_client::{ClientError, LlmClient, Request, ResponseStream};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use agen::{Engine, History};
use async_trait::async_trait;
use common::MockLlmClient;

#[derive(Clone)]
struct FailOnceClient {
    calls: Arc<AtomicUsize>,
    events: Vec<Event>,
}

#[async_trait]
impl LlmClient for FailOnceClient {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(ClientError::Api {
                status: Some(504),
                code: None,
                message: "gateway timeout".into(),
                retry_after: None,
            });
        }
        Ok(Box::pin(futures::stream::iter(
            self.events.clone().into_iter().map(Ok),
        )))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

#[tokio::test]
async fn test_callback_llm_retry_event() {
    let events = vec![Event::Status(ClientStatusEvent {
        status: ResponseStatus::Completed,
    })];
    let client = FailOnceClient {
        calls: Arc::new(AtomicUsize::new(0)),
        events,
    };
    let mut engine = Engine::new(client).with_retry_policy(RetryPolicy {
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        max_attempts: 2,
        total_timeout: Duration::from_secs(1),
    });
    let mut history: History = History::new();

    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    engine.on_llm_retry(move |llm_call, notice| {
        sink.lock().unwrap().push((llm_call, notice.clone()));
    });

    let result = engine.run(&mut history, "retry once").await;
    assert!(result.is_ok(), "engine should succeed after one retry");

    let notices = notices.lock().unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0, 0);
    assert_eq!(notices[0].1.failed_attempt, 1);
    assert_eq!(notices[0].1.max_attempts, 2);
    assert_eq!(notices[0].1.status, Some(504));
}

/// Verify that on_text_block correctly receives delta and stop events
#[tokio::test]
async fn test_callback_text_block_events() {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello, "),
        Event::text_delta(0, "World!"),
        Event::text_block_stop(0, None),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    let text_deltas = Arc::new(Mutex::new(Vec::new()));
    let text_completes = Arc::new(Mutex::new(Vec::new()));

    let deltas = text_deltas.clone();
    let completes = text_completes.clone();
    engine.on_text_block(move |block| {
        let d = deltas.clone();
        block.on_delta(move |text| {
            d.lock().unwrap().push(text.to_owned());
        });
        let c = completes.clone();
        block.on_stop(move |text| {
            c.lock().unwrap().push(text.to_owned());
        });
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let result = engine.run(&mut history, "Greet me").await;
    assert!(result.is_ok(), "Engine should complete");

    let deltas = text_deltas.lock().unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0], "Hello, ");
    assert_eq!(deltas[1], "World!");

    let completes = text_completes.lock().unwrap();
    assert_eq!(completes.len(), 1);
    assert_eq!(completes[0], "Hello, World!");
}

/// Verify that on_tool_use_block correctly receives start info and stop with ToolCall
#[tokio::test]
async fn test_callback_tool_call_complete() {
    let events = vec![
        Event::tool_use_start(0, "call_123", "get_weather"),
        Event::tool_input_delta(0, r#"{"city":"#),
        Event::tool_input_delta(0, r#""Tokyo"}"#),
        Event::tool_use_stop(0),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    let tool_starts = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let tool_completes = Arc::new(Mutex::new(Vec::new()));

    let starts = tool_starts.clone();
    let completes = tool_completes.clone();
    engine.on_tool_use_block(move |start, block| {
        starts
            .lock()
            .unwrap()
            .push((start.id.clone(), start.name.clone()));
        let c = completes.clone();
        block.on_stop(move |call| {
            c.lock().unwrap().push(call.clone());
        });
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let _ = engine.run(&mut history, "Weather please").await;

    let starts = tool_starts.lock().unwrap();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].0, "call_123");
    assert_eq!(starts[0].1, "get_weather");

    let completes = tool_completes.lock().unwrap();
    assert_eq!(completes.len(), 1);
    assert_eq!(completes[0].name, "get_weather");
    assert_eq!(completes[0].id, "call_123");
    assert_eq!(completes[0].input["city"], "Tokyo");
}

/// Verify that on_turn_start and on_turn_end callbacks are called
#[tokio::test]
async fn test_callback_turn_events() {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Done!"),
        Event::text_block_stop(0, None),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    let turn_starts = Arc::new(Mutex::new(Vec::new()));
    let turn_ends = Arc::new(Mutex::new(Vec::new()));

    let starts = turn_starts.clone();
    engine.on_turn_start(move |turn| {
        starts.lock().unwrap().push(turn);
    });

    let ends = turn_ends.clone();
    engine.on_turn_end(move |turn| {
        ends.lock().unwrap().push(turn);
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let result = engine.run(&mut history, "Do something").await;
    assert!(result.is_ok());

    let starts = turn_starts.lock().unwrap();
    let ends = turn_ends.lock().unwrap();

    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0], 0);

    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0], 0);
}

/// Stub tool returning a fixed [`ToolOutput`] for result-callback tests.
struct FixedOutputTool {
    output: ToolOutput,
}

#[async_trait]
impl Tool for FixedOutputTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(self.output.clone())
    }
}

fn fixed_tool(name: &'static str, output: ToolOutput) -> ToolDefinition {
    Arc::new(move || {
        let meta = ToolMeta::new(name).input_schema(serde_json::json!({"type":"object"}));
        (
            meta,
            Arc::new(FixedOutputTool {
                output: output.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

/// Verify that on_tool_result fires once per executed tool with
/// summary/content/is_error matching what the tool returned.
#[tokio::test]
async fn test_callback_tool_result_events() {
    let events = vec![
        Event::tool_use_start(0, "call_1", "fixed"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    engine.register_tool(fixed_tool(
        "fixed",
        ToolOutput {
            summary: "did the thing".into(),
            content: Some("full detail body".into()),
            attachments: Vec::new(),
        },
    ));

    let captured: Arc<Mutex<Vec<(String, String, Option<String>, bool)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    engine.on_tool_result(move |result| {
        sink.lock().unwrap().push((
            result.tool_use_id.clone(),
            result.summary.clone(),
            result.content.clone(),
            result.is_error,
        ));
    });

    let _ = engine.run(&mut history, "call it").await;

    let observed = captured.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].0, "call_1");
    assert_eq!(observed[0].1, "did the thing");
    assert_eq!(observed[0].2.as_deref(), Some("full detail body"));
    assert!(!observed[0].3);
}

/// Stub tool that always fails, for exercising the error path through
/// `on_tool_result`.
struct ErroringTool {
    message: String,
}

#[async_trait]
impl Tool for ErroringTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError::ExecutionFailed(self.message.clone()))
    }
}

fn erroring_tool(name: &'static str, message: &'static str) -> ToolDefinition {
    Arc::new(move || {
        let meta = ToolMeta::new(name).input_schema(serde_json::json!({"type":"object"}));
        (
            meta,
            Arc::new(ErroringTool {
                message: message.to_string(),
            }) as Arc<dyn Tool>,
        )
    })
}

/// Verify on_tool_result also fires for failed executions with
/// is_error=true, and that the ToolOutput content channel stays empty.
#[tokio::test]
async fn test_callback_tool_result_error_path() {
    let events = vec![
        Event::tool_use_start(0, "call_err", "erroring"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    engine.register_tool(erroring_tool("erroring", "boom"));

    let captured: Arc<Mutex<Vec<(String, String, Option<String>, bool)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    engine.on_tool_result(move |result| {
        sink.lock().unwrap().push((
            result.tool_use_id.clone(),
            result.summary.clone(),
            result.content.clone(),
            result.is_error,
        ));
    });

    let _ = engine.run(&mut history, "fail it").await;

    let observed = captured.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].0, "call_err");
    assert!(
        observed[0].1.contains("boom"),
        "summary should carry the error message: {}",
        observed[0].1
    );
    assert!(observed[0].2.is_none());
    assert!(observed[0].3);
}

/// Verify that on_usage callback receives usage events
#[tokio::test]
async fn test_callback_usage_events() {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello"),
        Event::text_block_stop(0, None),
        Event::usage(100, 50),
        Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut engine = Engine::new(client);
    let mut history: History = History::new();

    let usage_events = Arc::new(Mutex::new(Vec::new()));

    let usages = usage_events.clone();
    engine.on_usage(move |event| {
        usages.lock().unwrap().push(event.clone());
    });

    // Mutable::run consumes self, returns (Locked, EngineResult)
    let _ = engine.run(&mut history, "Hello").await;

    let usages = usage_events.lock().unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].input_tokens, Some(100));
    assert_eq!(usages[0].output_tokens, Some(50));
}
