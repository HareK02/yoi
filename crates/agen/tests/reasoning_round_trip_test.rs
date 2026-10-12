//! Integration tests for reasoning history round trips.
//!
//! Verifies that `Item::Reasoning` survives the lifecycle from Engine streaming
//! through history append to the next outgoing request.
//!
//! Covers Anthropic thinking and signatures, OpenAI Responses reasoning text,
//! summaries and encrypted content, and ordering reasoning before other assistant
//! output as required by Anthropic.

mod common;

use agen::Item;
use agen::llm_client::event::{
    BlockMetadata, BlockStart, BlockStop, BlockType, Event, ReasoningBlockData, ResponseStatus,
    StatusEvent,
};
use agen::{Engine, History};
use common::MockLlmClient;

fn reasoning_block(text: impl Into<String>, data: ReasoningBlockData) -> Vec<Event> {
    vec![
        Event::BlockStart(BlockStart {
            index: 100,
            block_type: BlockType::Thinking,
            metadata: BlockMetadata::Thinking,
        }),
        Event::BlockDelta(agen::llm_client::event::BlockDelta {
            index: 100,
            delta: agen::llm_client::event::DeltaContent::Thinking(text.into()),
        }),
        Event::BlockStop(BlockStop {
            index: 100,
            block_type: BlockType::Thinking,
            stop_reason: None,
            reasoning: Some(data),
        }),
    ]
}

/// Anthropic-style sequence: thinking block, text, completion.
/// History contains Reasoning with a signature followed by assistant_message.
#[tokio::test]
async fn anthropic_thinking_round_trips_signature_into_history() {
    let mut events = reasoning_block(
        "let me think...",
        ReasoningBlockData {
            id: None,
            text: None,
            summary: Vec::new(),
            encrypted_content: None,
            signature: Some("SIG-OPUS".into()),
        },
    );
    events.extend([
        Event::text_block_start(0),
        Event::text_delta(0, "Here's the answer"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockLlmClient::new(events);
    let engine = Engine::new(client);
    let mut history: History = History::new();
    let _out = engine.run(&mut history, "question?").await;

    let entries = history.entries();
    // user / reasoning / assistant_message
    assert_eq!(history.len(), 3, "history: {history:?}");

    assert!(matches!(entries[0].item, Item::Message { .. }));
    match &entries[1].item {
        Item::Reasoning {
            text, signature, ..
        } => {
            assert_eq!(text, "let me think...");
            assert_eq!(signature.as_deref(), Some("SIG-OPUS"));
        }
        other => panic!("expected Reasoning, got {other:?}"),
    }
    assert_eq!(entries[2].item.as_text(), Some("Here's the answer"));
}

/// OpenAI Responses reasoning with encrypted_content and summary is projected
/// into the corresponding `Item::Reasoning` fields.
#[tokio::test]
async fn openai_reasoning_round_trips_encrypted_and_summary() {
    let mut events = reasoning_block(
        "",
        ReasoningBlockData {
            id: Some("r1".into()),
            text: Some("inner reasoning".into()),
            summary: vec!["sum-A".into(), "sum-B".into()],
            encrypted_content: Some("ENC-OPAQUE".into()),
            signature: None,
        },
    );
    events.extend([
        Event::text_block_start(0),
        Event::text_delta(0, "answer"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockLlmClient::new(events);
    let engine = Engine::new(client);
    let mut history: History = History::new();
    let _out = engine.run(&mut history, "q").await;

    let entries = history.entries();
    match &entries[1].item {
        Item::Reasoning {
            text,
            summary,
            encrypted_content,
            signature,
            id,
            ..
        } => {
            assert_eq!(text, "inner reasoning");
            assert_eq!(summary, &vec!["sum-A".to_string(), "sum-B".to_string()]);
            assert_eq!(encrypted_content.as_deref(), Some("ENC-OPAQUE"));
            assert!(signature.is_none());
            assert_eq!(id.as_deref(), Some("r1"));
        }
        other => panic!("expected Reasoning, got {other:?}"),
    }
}

/// Reasoning precedes text/tool_call in the assistant turn because Anthropic
/// requires thinking at the start of the assistant message.
#[tokio::test]
async fn reasoning_precedes_text_in_assistant_burst() {
    let mut events = vec![
        // Even if the reasoning block finishes after text/tool_call output,
        // it is placed first among assistant items when appended to history.
        Event::text_block_start(0),
        Event::text_delta(0, "intermediate"),
        Event::text_block_stop(0, None),
    ];
    events.extend(reasoning_block(
        "after text",
        ReasoningBlockData {
            signature: Some("SIG".into()),
            ..Default::default()
        },
    ));
    events.push(Event::Status(StatusEvent {
        status: ResponseStatus::Completed,
    }));
    let client = MockLlmClient::new(events);
    let engine = Engine::new(client);
    let mut history: History = History::new();
    let _out = engine.run(&mut history, "q").await;

    let entries = history.entries();
    // user / reasoning (first assistant item) / assistant_message
    assert!(matches!(entries[1].item, Item::Reasoning { .. }));
    assert_eq!(entries[2].item.as_text(), Some("intermediate"));
}

/// Restored history: inject an Item::Reasoning with a signature as if restored
/// from history.json. Running the Engine preserves it in the next `Request::items`
/// sent to the LLM without modifying the reasoning item.
#[tokio::test]
async fn injected_reasoning_survives_into_outgoing_request() {
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use agen::llm_client::{ClientError, LlmClient, Request};

    /// Captures a Request and returns a stream containing only completion status.
    #[derive(Clone)]
    struct CapturingClient {
        captured: Arc<Mutex<Option<Request>>>,
    }

    #[async_trait]
    impl LlmClient for CapturingClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError>
        {
            *self.captured.lock().unwrap() = Some(request);
            let stream = futures::stream::iter(vec![Ok(Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }))]);
            Ok(Box::pin(stream))
        }
    }

    let captured = Arc::new(Mutex::new(None));
    let client = CapturingClient {
        captured: captured.clone(),
    };

    let mut engine = Engine::new(client);
    let mut history: History = History::new();
    // Restore existing history before continuing.
    engine.set_history(
        &mut history,
        vec![
            Item::user_message("prior question"),
            Item::reasoning("prior thinking").with_signature("SIG-PRIOR"),
            Item::assistant_message("prior answer"),
        ],
    );

    let _ = engine.run(&mut history, "follow up").await;

    let req = captured
        .lock()
        .unwrap()
        .take()
        .expect("client should have received a request");
    // The reasoning item must remain in the outgoing items.
    let mut found = false;
    for item in &req.items {
        if let Item::Reasoning {
            text, signature, ..
        } = item
        {
            assert_eq!(text, "prior thinking");
            assert_eq!(signature.as_deref(), Some("SIG-PRIOR"));
            found = true;
        }
    }
    assert!(
        found,
        "Reasoning item must survive into outgoing request items: {req:?}",
        req = req.items,
    );
}
