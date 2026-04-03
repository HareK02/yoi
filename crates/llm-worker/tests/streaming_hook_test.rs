//! Streaming hook tests

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::MockLlmClient;
use llm_worker::hook::{
    Hook, HookError, OnStreamChunk, OnStreamComplete, OnTextDelta, OnToolCallDelta,
    StreamChunkContext, StreamCompleteContext, StreamHookResult, TextDeltaContext,
    ToolCallDeltaContext,
};
use llm_worker::llm_client::event::{Event, ResponseStatus, StatusEvent};
use llm_worker::{Worker, WorkerError};

#[tokio::test]
async fn test_text_delta_hooks_run_in_registration_order() {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "A"),
        Event::text_delta(0, "B"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut worker = Worker::new(client);

    struct RecorderHook {
        label: &'static str,
        records: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Hook<OnTextDelta> for RecorderHook {
        async fn call(&self, input: &mut TextDeltaContext) -> Result<StreamHookResult, HookError> {
            self.records
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.label, input.delta));
            Ok(StreamHookResult::Continue)
        }
    }

    let records = Arc::new(Mutex::new(Vec::new()));
    worker.add_on_text_delta_hook(RecorderHook {
        label: "first",
        records: records.clone(),
    });
    worker.add_on_text_delta_hook(RecorderHook {
        label: "second",
        records: records.clone(),
    });

    let result = worker.run("hello").await;
    assert!(result.is_ok(), "run should succeed: {result:?}");

    let got = records.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            "first:A".to_string(),
            "second:A".to_string(),
            "first:B".to_string(),
            "second:B".to_string(),
        ]
    );
}

#[tokio::test]
async fn test_stream_chunk_and_stream_complete_hooks_are_called() {
    let events = vec![
        Event::ping(),
        Event::text_block_start(0),
        Event::text_delta(0, "hi"),
        Event::text_block_stop(0, None),
        Event::usage(10, 5),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut worker = Worker::new(client);

    struct ChunkCounter(Arc<Mutex<usize>>);

    #[async_trait]
    impl Hook<OnStreamChunk> for ChunkCounter {
        async fn call(
            &self,
            _input: &mut StreamChunkContext,
        ) -> Result<StreamHookResult, HookError> {
            let mut guard = self.0.lock().unwrap();
            *guard += 1;
            Ok(StreamHookResult::Continue)
        }
    }

    struct CompleteRecorder(Arc<Mutex<Vec<(usize, usize)>>>);

    #[async_trait]
    impl Hook<OnStreamComplete> for CompleteRecorder {
        async fn call(
            &self,
            input: &mut StreamCompleteContext,
        ) -> Result<StreamHookResult, HookError> {
            self.0.lock().unwrap().push((input.turn, input.event_count));
            Ok(StreamHookResult::Continue)
        }
    }

    let chunk_count = Arc::new(Mutex::new(0usize));
    let completes = Arc::new(Mutex::new(Vec::new()));

    worker.add_on_stream_chunk_hook(ChunkCounter(chunk_count.clone()));
    worker.add_on_stream_complete_hook(CompleteRecorder(completes.clone()));

    let result = worker.run("hello").await;
    assert!(result.is_ok(), "run should succeed: {result:?}");

    assert_eq!(*chunk_count.lock().unwrap(), 6);
    assert_eq!(completes.lock().unwrap().as_slice(), &[(0usize, 6usize)]);
}

#[tokio::test]
async fn test_tool_call_delta_hook_can_abort_run() {
    let events = vec![
        Event::tool_use_start(0, "call_1", "unknown_tool"),
        Event::tool_input_delta(0, r#"{"x":1}"#),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut worker = Worker::new(client);

    struct AbortToolDelta;

    #[async_trait]
    impl Hook<OnToolCallDelta> for AbortToolDelta {
        async fn call(
            &self,
            _input: &mut ToolCallDeltaContext,
        ) -> Result<StreamHookResult, HookError> {
            Ok(StreamHookResult::Abort("blocked by tool delta".to_string()))
        }
    }

    worker.add_on_tool_call_delta_hook(AbortToolDelta);

    let result = worker.run("hello").await;
    match result {
        Err(WorkerError::Aborted(reason)) => assert_eq!(reason, "blocked by tool delta"),
        other => panic!("expected aborted result, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_stream_hook_pause_is_mapped_to_aborted() {
    let events = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "pause me"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ];

    let client = MockLlmClient::new(events);
    let mut worker = Worker::new(client);

    struct PauseHook;

    #[async_trait]
    impl Hook<OnTextDelta> for PauseHook {
        async fn call(&self, _input: &mut TextDeltaContext) -> Result<StreamHookResult, HookError> {
            Ok(StreamHookResult::Pause)
        }
    }

    worker.add_on_text_delta_hook(PauseHook);

    let result = worker.run("hello").await;
    match result {
        Err(WorkerError::Aborted(reason)) => assert_eq!(reason, "Paused by stream hook"),
        other => panic!("expected aborted result, got: {other:?}"),
    }
}
