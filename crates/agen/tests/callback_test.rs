//! Closure callback API tests
//!
//! Tests for the closure-based event subscription API on Engine.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::Engine;
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent as ClientStatusEvent};
use agen::llm_client::retry::RetryPolicy;
use agen::llm_client::{ClientError, LlmClient, Request, ResponseStream};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use common::MockLlmClient;

/// A per-run tracing sink: no global subscriber or output-body logging.
#[derive(Clone)]
struct DiagnosticLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for DiagnosticLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn output_limits_keep_partial_results_without_warning_callbacks() {
    use agen::tool::{Attachment, ImageAttachment, ToolOutputLimits, ToolResult};
    use agen::{History, Item, ToolResultDisposition};
    use tracing::instrument::WithSubscriber;

    let limit = 256;
    let cases = [
        ("below", "small result".to_string(), limit),
        ("exact", "x".repeat(limit), limit),
        ("Grep", "matched line\n".repeat(200), limit),
        ("Read", "あいうえお".repeat(200), 120),
    ];
    let mut events = Vec::new();
    for (index, name) in cases.iter().map(|c| c.0).chain(["erroring"]).enumerate() {
        events.extend([
            Event::tool_use_start(index, name, name),
            Event::tool_input_delta(index, "{}"),
            Event::tool_use_stop(index),
        ]);
    }
    events.push(Event::Status(ClientStatusEvent {
        status: ResponseStatus::Completed,
    }));
    let client = MockLlmClient::with_responses(vec![
        events,
        vec![Event::Status(ClientStatusEvent {
            status: ResponseStatus::Completed,
        })],
    ]);
    let probe = client.clone();
    let mut engine = Engine::new(client);
    engine.set_tool_call_dispatch_mode(agen::ToolCallDispatchMode::AfterResponse);
    engine.set_tool_output_limits(Some(ToolOutputLimits {
        default_max_bytes: limit,
        per_tool: [("Read".to_string(), 120)].into(),
    }));
    let attachments = vec![Attachment::Image(ImageAttachment::new(
        "image/png",
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec(),
    ))];
    for (name, content, _) in &cases {
        engine.register_tool(fixed_tool(
            name,
            ToolOutput {
                summary: format!("result from {name}"),
                content: Some(content.clone()),
                attachments: attachments.clone(),
            },
        ));
    }
    engine.register_tool(erroring_tool("erroring", "execution failed"));

    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = warnings.clone();
    engine.on_warning(move |warning| sink.lock().unwrap().push(warning.to_owned()));
    let published = Arc::new(Mutex::new(Vec::<ToolResult>::new()));
    let sink = published.clone();
    engine.on_tool_result(move |result| sink.lock().unwrap().push(result.clone()));
    let committed = Arc::new(Mutex::new(Vec::new()));
    let sink = committed.clone();
    engine.on_history_append(move |item| {
        if matches!(item, Item::ToolResult { .. }) {
            sink.lock().unwrap().push(item.clone());
        }
        Ok(())
    });

    let log = DiagnosticLog(Arc::new(Mutex::new(Vec::new())));
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let mut history = History::new();
    let run = engine
        .run(&mut history, "run tools")
        .with_subscriber(subscriber)
        .await;
    assert!(matches!(run.result, agen::EngineRunExit::Finished));
    assert!(warnings.lock().unwrap().is_empty());

    let requests = probe.requests();
    assert_eq!(requests.len(), 2);
    let saved: Vec<_> = history
        .iter()
        .filter_map(|entry| matches!(entry.item, Item::ToolResult { .. }).then_some(&entry.item))
        .collect();
    assert_eq!(saved.len(), cases.len() + 1);
    let committed = committed.lock().unwrap();
    let published = published.lock().unwrap();
    let logs = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    for item in saved {
        let Item::ToolResult {
            call_id,
            summary,
            content,
            disposition,
            is_error,
            attachments: saved_attachments,
            ..
        } = item
        else {
            unreachable!()
        };
        assert!(committed.iter().any(|candidate| candidate == item));
        assert!(requests[1].items.iter().any(|candidate| candidate == item));
        let result = published
            .iter()
            .find(|result| result.tool_use_id == *call_id)
            .unwrap();
        assert_eq!(&result.summary, summary);
        assert_eq!(&result.content, content);
        assert_eq!(&result.disposition, disposition);
        assert_eq!(&result.is_error, is_error);
        assert_eq!(&result.attachments, saved_attachments);
        if call_id == "erroring" {
            assert_eq!(*disposition, ToolResultDisposition::Error);
            assert!(*is_error);
            assert!(content.is_none());
            continue;
        }
        assert_eq!(*disposition, ToolResultDisposition::Success);
        assert!(!*is_error);
        assert_eq!(*summary, format!("result from {call_id}"));
        assert_eq!(saved_attachments, &attachments);
        let (_, original, cap) = cases.iter().find(|c| c.0 == call_id).unwrap();
        let content = content.as_ref().unwrap();
        assert!(content.len() <= *cap);
        if original.len() <= *cap {
            assert_eq!(content, original);
        } else {
            let (body, marker) = content.split_once("\n\n[truncated: ").unwrap();
            assert!(original.starts_with(body));
            assert_eq!(
                marker,
                format!(
                    "{} bytes dropped, refine your query]",
                    original.len() - body.len()
                )
            );
            let diagnostic = logs
                .lines()
                .find(|line| line.contains(&format!("tool={call_id}")))
                .unwrap();
            assert!(diagnostic.contains(&format!("before_bytes={}", original.len())));
            assert!(diagnostic.contains(&format!("after_bytes={}", content.len())));
            assert!(diagnostic.contains(&format!("limit_bytes={cap}")));
            assert!(!diagnostic.contains(body));
        }
    }
    assert_eq!(
        logs.lines()
            .filter(|line| line.contains("Tool output exceeded byte limit"))
            .count(),
        2
    );
}

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
    let mut history = agen::History::new();

    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    engine.on_llm_retry(move |llm_call, notice| {
        sink.lock().unwrap().push((llm_call, notice.clone()));
    });

    let result = engine.run(&mut history, "retry once").await;
    assert!(
        matches!(result.result, agen::EngineRunExit::Finished),
        "engine should succeed after one retry"
    );

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
    let mut history = agen::History::new();

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

    // Mutable::run consumes self, returns (Locked, EngineRunExit)
    let result = engine.run(&mut history, "Greet me").await;
    assert!(
        matches!(result.result, agen::EngineRunExit::Finished),
        "Engine should complete"
    );

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
    let mut history = agen::History::new();

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

    // Mutable::run consumes self, returns (Locked, EngineRunExit)
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
    let mut history = agen::History::new();

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

    // Mutable::run consumes self, returns (Locked, EngineRunExit)
    let result = engine.run(&mut history, "Do something").await;
    assert!(matches!(result.result, agen::EngineRunExit::Finished));

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
    let mut history = agen::History::new();

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
    let mut history = agen::History::new();

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
    let mut history = agen::History::new();

    let usage_events = Arc::new(Mutex::new(Vec::new()));

    let usages = usage_events.clone();
    engine.on_usage(move |event| {
        usages.lock().unwrap().push(event.clone());
    });

    // Mutable::run consumes self, returns (Locked, EngineRunExit)
    let _ = engine.run(&mut history, "Hello").await;

    let usages = usage_events.lock().unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].input_tokens, Some(100));
    assert_eq!(usages[0].output_tokens, Some(50));
}
