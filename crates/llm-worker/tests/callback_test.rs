//! Closure callback API tests
//!
//! Tests for the closure-based event subscription API on Worker.

mod common;

use std::sync::{Arc, Mutex};

use common::MockLlmClient;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent as ClientStatusEvent};

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
