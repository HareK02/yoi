use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use agen::llm_client::{
    ClientError, LlmClient, Request, ResponseStream, ToolCallCompletionSupport,
    scheme::{Scheme, gemini::GeminiScheme, openai_chat::OpenAIScheme},
};
use agen::{Engine, EngineRunExit, History, ToolCallDispatchMode};
use async_trait::async_trait;

#[derive(Clone)]
struct SseFrame {
    event_type: &'static str,
    data: &'static str,
}

#[derive(Clone)]
struct SchemeReplayClient<S> {
    scheme: S,
    responses: Arc<Vec<Vec<SseFrame>>>,
    stream_calls: Arc<AtomicUsize>,
}

impl<S> SchemeReplayClient<S> {
    fn new(scheme: S, responses: Vec<Vec<SseFrame>>) -> Self {
        Self {
            scheme,
            responses: Arc::new(responses),
            stream_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn stream_count(&self) -> usize {
        self.stream_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl<S: Scheme> LlmClient for SchemeReplayClient<S> {
    async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
        let response_index = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let frames = self
            .responses
            .get(response_index)
            .ok_or_else(|| ClientError::Config("scheme replay responses exhausted".to_string()))?;
        let mut state = S::State::default();
        let mut output = Vec::new();
        for frame in frames {
            match self
                .scheme
                .parse_sse(frame.event_type, frame.data, &mut state)
            {
                Ok(events) => output.extend(events.into_iter().map(Ok)),
                Err(error) => {
                    output.push(Err(error));
                    break;
                }
            }
        }
        Ok(Box::pin(futures::stream::iter(output)))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        self.scheme.tool_call_completion_support()
    }
}

async fn assert_response_complete_scheme_accepts_clean_eof<S: Scheme>(
    scheme: S,
    frames: Vec<SseFrame>,
) {
    assert_eq!(
        scheme.tool_call_completion_support(),
        ToolCallCompletionSupport::ResponseComplete
    );

    for mode in [
        ToolCallDispatchMode::AfterResponse,
        ToolCallDispatchMode::OnToolCallComplete,
    ] {
        let client = SchemeReplayClient::new(scheme.clone(), vec![frames.clone()]);
        let probe = client.clone();
        let mut engine = Engine::new(client);
        engine.set_tool_call_dispatch_mode(mode);
        let mut history = History::new();

        let output = engine.run(&mut history, "hello").await;

        assert!(
            matches!(output.result, EngineRunExit::Finished),
            "{mode:?} should accept the response-complete scheme's clean EOF, got {:?}",
            output.result
        );
        assert_eq!(
            probe.stream_count(),
            1,
            "{mode:?} must not open a continuation request after the adapter's normal EOF"
        );
    }
}

#[tokio::test]
async fn openai_chat_clean_eof_completes_in_default_and_opt_in_fallback_modes() {
    assert_response_complete_scheme_accepts_clean_eof(
        OpenAIScheme::new(),
        vec![
            SseFrame {
                event_type: "message",
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":"stop"}]}"#,
            },
            SseFrame {
                event_type: "message",
                data: "[DONE]",
            },
        ],
    )
    .await;
}

#[tokio::test]
async fn gemini_clean_eof_completes_in_default_and_opt_in_fallback_modes() {
    assert_response_complete_scheme_accepts_clean_eof(
        GeminiScheme::new(),
        vec![
            SseFrame {
                event_type: "message",
                data: r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"},"finishReason":"STOP","index":0}]}"#,
            },
            SseFrame {
                event_type: "message",
                data: "[DONE]",
            },
        ],
    )
    .await;
}
