//! WorkerSubscriber tests
//!
//! Tests for subscribing to events using WorkerSubscriber

mod common;

use std::sync::{Arc, Mutex};

use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::hook::ToolCall;
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent as ClientStatusEvent};
use llm_worker::subscriber::WorkerSubscriber;
use llm_worker::timeline::event::{ErrorEvent, StatusEvent, UsageEvent};
use llm_worker::timeline::{TextBlockEvent, ToolUseBlockEvent};

// =============================================================================
// Test Subscriber
// =============================================================================

/// Simple Subscriber implementation for testing
struct TestSubscriber {
    // Recording buffers
    text_deltas: Arc<Mutex<Vec<String>>>,
    text_completes: Arc<Mutex<Vec<String>>>,
    tool_call_completes: Arc<Mutex<Vec<ToolCall>>>,
    usage_events: Arc<Mutex<Vec<UsageEvent>>>,
    status_events: Arc<Mutex<Vec<StatusEvent>>>,
    turn_starts: Arc<Mutex<Vec<usize>>>,
    turn_ends: Arc<Mutex<Vec<usize>>>,
}

impl TestSubscriber {
    fn new() -> Self {
        Self {
            text_deltas: Arc::new(Mutex::new(Vec::new())),
            text_completes: Arc::new(Mutex::new(Vec::new())),
            tool_call_completes: Arc::new(Mutex::new(Vec::new())),
            usage_events: Arc::new(Mutex::new(Vec::new())),
            status_events: Arc::new(Mutex::new(Vec::new())),
            turn_starts: Arc::new(Mutex::new(Vec::new())),
            turn_ends: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl WorkerSubscriber for TestSubscriber {
    type TextBlockScope = String;
    type ToolUseBlockScope = ();

    fn on_text_block(&mut self, buffer: &mut String, event: &TextBlockEvent) {
        if let TextBlockEvent::Delta(text) = event {
            buffer.push_str(text);
            self.text_deltas.lock().unwrap().push(text.clone());
        }
    }

    fn on_text_complete(&mut self, text: &str) {
        self.text_completes.lock().unwrap().push(text.to_string());
    }

    fn on_tool_use_block(&mut self, _scope: &mut (), _event: &ToolUseBlockEvent) {
        // Process as needed
    }

    fn on_tool_call_complete(&mut self, call: &ToolCall) {
        self.tool_call_completes.lock().unwrap().push(call.clone());
    }

    fn on_usage(&mut self, event: &UsageEvent) {
        self.usage_events.lock().unwrap().push(event.clone());
    }

    fn on_status(&mut self, event: &StatusEvent) {
        self.status_events.lock().unwrap().push(event.clone());
    }

    fn on_error(&mut self, _event: &ErrorEvent) {
        // Process as needed
    }

    fn on_turn_start(&mut self, turn: usize) {
        self.turn_starts.lock().unwrap().push(turn);
    }

    fn on_turn_end(&mut self, turn: usize) {
        self.turn_ends.lock().unwrap().push(turn);
    }
}

// =============================================================================
// Tests
// =============================================================================

/// Verify that WorkerSubscriber correctly receives text block events
#[tokio::test]
async fn test_subscriber_text_block_events() {
    // Event sequence containing text response
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

    // Register Subscriber
    let subscriber = TestSubscriber::new();
    let text_deltas = subscriber.text_deltas.clone();
    let text_completes = subscriber.text_completes.clone();
    worker.subscribe(subscriber);

    // Execute
    let result = worker.run("Greet me").await;

    assert!(result.is_ok(), "Worker should complete: {:?}", result);

    // Verify deltas were collected
    let deltas = text_deltas.lock().unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0], "Hello, ");
    assert_eq!(deltas[1], "World!");

    // Verify complete text was collected
    let completes = text_completes.lock().unwrap();
    assert_eq!(completes.len(), 1);
    assert_eq!(completes[0], "Hello, World!");
}

/// Verify that WorkerSubscriber correctly receives tool call complete events
#[tokio::test]
async fn test_subscriber_tool_call_complete() {
    // Event sequence containing tool call
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

    // Register Subscriber
    let subscriber = TestSubscriber::new();
    let tool_call_completes = subscriber.tool_call_completes.clone();
    worker.subscribe(subscriber);

    // Execute
    let _ = worker.run("Weather please").await;

    // Verify tool call complete was collected
    let completes = tool_call_completes.lock().unwrap();
    assert_eq!(completes.len(), 1);
    assert_eq!(completes[0].name, "get_weather");
    assert_eq!(completes[0].id, "call_123");
    assert_eq!(completes[0].input["city"], "Tokyo");
}

/// Verify that WorkerSubscriber correctly receives turn events
#[tokio::test]
async fn test_subscriber_turn_events() {
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

    // Register Subscriber
    let subscriber = TestSubscriber::new();
    let turn_starts = subscriber.turn_starts.clone();
    let turn_ends = subscriber.turn_ends.clone();
    worker.subscribe(subscriber);

    // Execute
    let result = worker.run("Do something").await;

    assert!(result.is_ok());

    // Verify turn events were collected
    let starts = turn_starts.lock().unwrap();
    let ends = turn_ends.lock().unwrap();

    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0], 0); // First turn

    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0], 0);
}

/// Verify that WorkerSubscriber correctly receives Usage events
#[tokio::test]
async fn test_subscriber_usage_events() {
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

    // Register Subscriber
    let subscriber = TestSubscriber::new();
    let usage_events = subscriber.usage_events.clone();
    worker.subscribe(subscriber);

    // Execute
    let _ = worker.run("Hello").await;

    // Verify Usage events were collected
    let usages = usage_events.lock().unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].input_tokens, Some(100));
    assert_eq!(usages[0].output_tokens, Some(50));
}
