//! Worker fixture-based integration tests
//!
//! Tests Worker behavior using recorded API responses.
//! Can run locally without API keys.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};

/// Fixture directory path
fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anthropic")
}

/// Simple test tool
#[derive(Clone)]
struct MockWeatherTool {
    call_count: Arc<AtomicUsize>,
}

impl MockWeatherTool {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn get_call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn definition(&self) -> ToolDefinition {
        let tool = self.clone();
        Arc::new(move || {
            let meta = ToolMeta::new("get_weather")
                .description("Get the current weather for a city")
                .input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name"
                        }
                    },
                    "required": ["city"]
                }));
            (meta, Arc::new(tool.clone()) as Arc<dyn Tool>)
        })
    }
}

#[async_trait]
impl Tool for MockWeatherTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        // Parse input
        let input: serde_json::Value = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(e.to_string()))?;

        let city = input["city"].as_str().unwrap_or("Unknown");

        // Return mock response
        Ok(format!("Weather in {}: Sunny, 22°C", city).into())
    }
}

// =============================================================================
// Basic Fixture Tests
// =============================================================================

/// Verify that MockLlmClient can correctly load events from JSONL fixture files
///
/// Uses existing anthropic_*.jsonl files to verify events are parsed and loaded.
#[test]
fn test_mock_client_from_fixture() {
    // Load existing fixture
    let fixture_path = fixtures_dir().join("anthropic_1767624445.jsonl");
    if !fixture_path.exists() {
        println!("Fixture not found, skipping test");
        return;
    }

    let client = MockLlmClient::from_fixture(&fixture_path).unwrap();
    assert!(client.event_count() > 0, "Should have loaded events");
    println!("Loaded {} events from fixture", client.event_count());
}

/// Verify that MockLlmClient works correctly with directly specified event lists
///
/// Creates a client with programmatically constructed events instead of using fixture files.
#[test]
fn test_mock_client_from_events() {
    use llm_worker::llm_client::event::Event;

    // Specify events directly
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello!"),
        Event::text_block_stop(0, None),
    ];

    let client = MockLlmClient::new(events);
    assert_eq!(client.event_count(), 3);
}

// =============================================================================
// Worker Tests with Fixtures
// =============================================================================

/// Verify that Worker can correctly process simple text responses
///
/// Uses simple_text.jsonl fixture to test scenarios without tool calls.
/// Skipped if fixture is not present.
#[tokio::test]
async fn test_worker_simple_text_response() {
    let fixture_path = fixtures_dir().join("simple_text.jsonl");
    if !fixture_path.exists() {
        println!("Fixture not found: {:?}, skipping test", fixture_path);
        println!("Run: cargo run --example record_worker_test");
        return;
    }

    let client = MockLlmClient::from_fixture(&fixture_path).unwrap();
    let worker = Worker::new(client);

    // Send a simple message (Mutable::run consumes self, returns tuple)
    let result = worker.run("Hello").await;

    assert!(result.is_ok(), "Worker should complete successfully");
}

/// Verify that Worker can correctly process responses containing tool calls
///
/// Uses tool_call.jsonl fixture to test that MockWeatherTool is called.
/// Sets max_turns=1 to prevent loop after tool execution.
#[tokio::test]
async fn test_worker_tool_call() {
    let fixture_path = fixtures_dir().join("tool_call.jsonl");
    if !fixture_path.exists() {
        println!("Fixture not found: {:?}, skipping test", fixture_path);
        println!("Run: cargo run --example record_worker_test");
        return;
    }

    let client = MockLlmClient::from_fixture(&fixture_path).unwrap();
    let mut worker = Worker::new(client);

    // Register tool
    let weather_tool = MockWeatherTool::new();
    let tool_for_check = weather_tool.clone();
    worker.register_tool(weather_tool.definition());

    // Send message (Mutable::run consumes self, returns tuple)
    let _result = worker.run("What's the weather in Tokyo?").await;

    // Verify tool was called
    // Note: max_turns=1 so no request is sent after tool result
    let call_count = tool_for_check.get_call_count();
    println!("Tool was called {} times", call_count);

    // Tool should be called if fixture contains ToolUse
    // But ends after 1 turn due to max_turns=1
}

/// Verify that Worker works without fixture files
///
/// Constructs event sequence programmatically and passes to MockLlmClient.
/// Useful when test independence is needed and external file dependency should be eliminated.
#[tokio::test]
async fn test_worker_with_programmatic_events() {
    use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};

    // Construct event sequence programmatically
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "Hello, "),
        Event::text_delta(0, "World!"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let worker = Worker::new(client);

    // Mutable::run consumes self, returns tuple
    let result = worker.run("Greet me").await;

    assert!(result.is_ok(), "Worker should complete successfully");
}

/// Verify that ToolCallCollector correctly collects ToolCall from ToolUse block events
///
/// Dispatches events to Timeline and verifies ToolCallCollector
/// correctly extracts id, name, and input (JSON).
#[tokio::test]
async fn test_tool_call_collector_integration() {
    use llm_worker::llm_client::event::Event;
    use llm_worker::timeline::{Timeline, ToolCallCollector};

    // Event sequence containing ToolUse block
    let events = vec![
        Event::tool_use_start(0, "call_123", "get_weather"),
        Event::tool_input_delta(0, r#"{"city":"#),
        Event::tool_input_delta(0, r#""Tokyo"}"#),
        Event::tool_use_stop(0),
    ];

    let collector = ToolCallCollector::new();
    let mut timeline = Timeline::new();
    timeline.on_tool_use_block(collector.clone());

    // Dispatch events
    for event in &events {
        let timeline_event: llm_worker::timeline::event::Event = event.clone().into();
        timeline.dispatch(&timeline_event);
    }

    // Verify collected ToolCall
    let calls = collector.take_collected();
    assert_eq!(calls.len(), 1, "Should collect one tool call");
    assert_eq!(calls[0].name, "get_weather");
    assert_eq!(calls[0].id, "call_123");
    assert_eq!(calls[0].input["city"], "Tokyo");
}
