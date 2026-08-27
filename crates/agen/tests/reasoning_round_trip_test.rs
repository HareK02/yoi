//! Reasoning history round-trip 統合テスト
//!
//! Engine のストリーム → history append → 次リクエスト送出までの
//! ライフサイクルで `Item::Reasoning` が脱落せず保持されることを確認する。
//!
//! 検証点:
//! - Anthropic 由来の thinking + signature が `Item::Reasoning::signature` として
//!   history に残る
//! - OpenAI Responses 由来の reasoning text + summary + encrypted_content が
//!   `Item::Reasoning` の各フィールドに展開される
//! - 直前の reasoning は次の outgoing request の `request.items` の先頭付近に
//!   含まれる（assistant メッセージの先頭、Anthropic 仕様）

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

/// Anthropic 風: thinking ブロック → text → 終了 のシーケンス。
/// Engine history に Reasoning(signature 付き) → assistant_message が並ぶ。
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

/// OpenAI Responses 風: encrypted_content + summary を持った reasoning が
/// `Item::Reasoning` のフィールドに展開されること。
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

/// Reasoning は assistant ターン内で text/tool_call より先に並ぶこと（Anthropic
/// が thinking を assistant メッセージの先頭に要求するため）。
#[tokio::test]
async fn reasoning_precedes_text_in_assistant_burst() {
    let mut events = vec![
        // text/tool_call とは独立に、reasoning block が中盤で完了しても、
        // history append 時には assistant items の先頭に置かれる。
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
    // user / reasoning(先頭) / assistant_message
    assert!(matches!(entries[1].item, Item::Reasoning { .. }));
    assert_eq!(entries[2].item.as_text(), Some("intermediate"));
}

/// resume シナリオ: history.json 由来の Item::Reasoning(signature) を Engine に
/// 注入して run しても、次の outgoing request の `Request::items` にそのまま
/// 載って LLM へ渡る（engine は items を改変しない契約）。
#[tokio::test]
async fn injected_reasoning_survives_into_outgoing_request() {
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use agen::llm_client::{ClientError, LlmClient, Request};

    /// Request を 1 度だけキャプチャして空ストリームを返す client。
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
    // resume: 既存 history を流し込む
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
    // Reasoning item が outgoing items に保持されていること
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
