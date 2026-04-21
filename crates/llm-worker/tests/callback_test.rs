//! Closure callback API tests
//!
//! Tests for the closure-based event subscription API on Worker.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent as ClientStatusEvent};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};

// =============================================================================
// Tests
// =============================================================================

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
    let mut worker = Worker::new(client);

    let text_deltas = Arc::new(Mutex::new(Vec::new()));
    let text_completes = Arc::new(Mutex::new(Vec::new()));

    let deltas = text_deltas.clone();
    let completes = text_completes.clone();
    worker.on_text_block(move |block| {
        let d = deltas.clone();
        block.on_delta(move |text| {
            d.lock().unwrap().push(text.to_owned());
        });
        let c = completes.clone();
        block.on_stop(move |text| {
            c.lock().unwrap().push(text.to_owned());
        });
    });

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let result = worker.run("Greet me").await;
    assert!(result.is_ok(), "Worker should complete");

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
    let mut worker = Worker::new(client);

    let tool_starts = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let tool_completes = Arc::new(Mutex::new(Vec::new()));

    let starts = tool_starts.clone();
    let completes = tool_completes.clone();
    worker.on_tool_use_block(move |start, block| {
        starts
            .lock()
            .unwrap()
            .push((start.id.clone(), start.name.clone()));
        let c = completes.clone();
        block.on_stop(move |call| {
            c.lock().unwrap().push(call.clone());
        });
    });

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let _ = worker.run("Weather please").await;

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
    let mut worker = Worker::new(client);

    let turn_starts = Arc::new(Mutex::new(Vec::new()));
    let turn_ends = Arc::new(Mutex::new(Vec::new()));

    let starts = turn_starts.clone();
    worker.on_turn_start(move |turn| {
        starts.lock().unwrap().push(turn);
    });

    let ends = turn_ends.clone();
    worker.on_turn_end(move |turn| {
        ends.lock().unwrap().push(turn);
    });

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let result = worker.run("Do something").await;
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
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
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
    let mut worker = Worker::new(client);

    worker.register_tool(fixed_tool(
        "fixed",
        ToolOutput {
            summary: "did the thing".into(),
            content: Some("full detail body".into()),
        },
    ));

    let captured: Arc<Mutex<Vec<(String, String, Option<String>, bool)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    worker.on_tool_result(move |result| {
        sink.lock().unwrap().push((
            result.tool_use_id.clone(),
            result.summary.clone(),
            result.content.clone(),
            result.is_error,
        ));
    });

    let _ = worker.run("call it").await;

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
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
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
    let mut worker = Worker::new(client);

    worker.register_tool(erroring_tool("erroring", "boom"));

    let captured: Arc<Mutex<Vec<(String, String, Option<String>, bool)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    worker.on_tool_result(move |result| {
        sink.lock().unwrap().push((
            result.tool_use_id.clone(),
            result.summary.clone(),
            result.content.clone(),
            result.is_error,
        ));
    });

    let _ = worker.run("fail it").await;

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
    let mut worker = Worker::new(client);

    let usage_events = Arc::new(Mutex::new(Vec::new()));

    let usages = usage_events.clone();
    worker.on_usage(move |event| {
        usages.lock().unwrap().push(event.clone());
    });

    // Mutable::run consumes self, returns (Locked, WorkerResult)
    let _ = worker.run("Hello").await;

    let usages = usage_events.lock().unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].input_tokens, Some(100));
    assert_eq!(usages[0].output_tokens, Some(50));
}
