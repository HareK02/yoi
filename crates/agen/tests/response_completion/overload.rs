//! Engine recovery with production SSE decoders and a scripted model transport.
//! No live provider: requests, persisted history and tool effects are observed.
use agen::llm_client::{
    ClientError, LlmClient, Request, ResponseStream, ToolCallCompletionSupport,
    event::{Event, ResponseStatus, StatusEvent},
    retry::RetryPolicy,
    scheme::{Scheme, anthropic::AnthropicScheme, openai_responses::OpenAIResponsesScheme},
};
use agen::{Engine, EngineRunExit, History, Item, ToolCallDispatchMode};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(Clone)]
enum Reply {
    Http(u16),
    Events(Vec<Event>),
}

#[derive(Clone)]
struct Client {
    replies: Arc<Vec<Reply>>,
    requests: Arc<Mutex<Vec<Request>>>,
    before_error: Option<Arc<tokio::sync::Notify>>,
}
impl Client {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Arc::new(replies),
            requests: Arc::default(),
            before_error: None,
        }
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
#[async_trait]
impl LlmClient for Client {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::PerBlock
    }
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len() - 1
        };
        match self
            .replies
            .get(index)
            .expect("unexpected extra provider request")
        {
            Reply::Http(status) => Err(ClientError::Api {
                status: Some(*status),
                code: None,
                message: "HTTP unavailable".into(),
                retry_after: None,
            }),
            Reply::Events(events) => {
                let gate = self.before_error.clone();
                Ok(Box::pin(futures::stream::iter(events.clone()).then(
                    move |event| {
                        let gate = gate.clone();
                        async move {
                            if matches!(event, Event::Error(_))
                                && let Some(gate) = gate
                            {
                                gate.notified().await;
                            }
                            Ok(event)
                        }
                    },
                )))
            }
        }
    }
}

fn overload(openai: bool) -> Event {
    let mut events = if openai {
        OpenAIResponsesScheme::new()
            .parse_sse(
                "error",
                r#"{"type":"error","code":"server_is_overloaded","message":"provider at capacity"}"#,
                &mut Default::default(),
            )
            .unwrap()
    } else {
        AnthropicScheme::new().parse_sse("error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"provider at capacity"}}"#,
            &mut Default::default()).unwrap()
    };
    events.remove(0)
}
fn finished() -> Reply {
    Reply::Events(vec![
        Event::text_block_start(0),
        Event::text_delta(0, "recovered"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ])
}
fn policy() -> RetryPolicy {
    RetryPolicy {
        base: Duration::ZERO,
        cap: Duration::ZERO,
        max_attempts: 4,
        total_timeout: Duration::from_secs(40),
    }
}

#[tokio::test]
async fn streamed_overload_replays_before_output_with_retry_notice_and_original_request() {
    for openai in [false, true] {
        for prefix in [
            vec![],
            vec![
                Event::Status(StatusEvent {
                    status: ResponseStatus::Started,
                }),
                Event::Ping(Default::default()),
                Event::Usage(Default::default()),
            ],
        ] {
            let mut events = prefix;
            events.push(overload(openai));
            // Queued events from the rejected response must not survive the retry.
            events.extend([
                Event::text_block_start(0),
                Event::text_delta(0, "stale rejected output"),
            ]);
            let client = Client::new(vec![Reply::Events(events), finished()]);
            let notices = Arc::new(Mutex::new(Vec::new()));
            let sink = notices.clone();
            let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
            engine.on_llm_retry(move |_, notice| sink.lock().unwrap().push(notice.clone()));
            let mut history = History::new();
            let output = engine.run(&mut history, "perform task").await;
            assert!(
                matches!(output.result, EngineRunExit::Finished),
                "{:?}",
                output.result
            );
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                serde_json::to_value(&requests[0].items).unwrap(),
                serde_json::to_value(&requests[1].items).unwrap()
            );
            let notices = notices.lock().unwrap();
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].failed_attempt, 1);
            assert_eq!(notices[0].status, None, "SSE must not invent HTTP status");
            assert!(notices[0].error.contains("provider at capacity"));
            let history = serde_json::to_string(&history.items_cloned()).unwrap();
            assert!(history.contains("recovered"));
            assert!(!history.contains("stale rejected output"));
        }
    }
}

#[tokio::test]
async fn http_and_streamed_overload_share_attempt_budget_and_preserve_final_error() {
    let client = Client::new(vec![
        Reply::Http(503),
        Reply::Events(vec![overload(false)]),
        Reply::Http(529),
        Reply::Events(vec![overload(false)]),
    ]);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
    engine.on_llm_retry(move |_, n| sink.lock().unwrap().push(n.failed_attempt));
    let result = engine.run(&mut History::new(), "task").await.result;
    assert!(matches!(result, EngineRunExit::Interrupted(_)));
    let error = format!("{result:?}");
    assert!(
        error.contains("overloaded_error") && error.contains("provider at capacity"),
        "{error}"
    );
    assert_eq!(client.count(), 4);
    assert_eq!(*notices.lock().unwrap(), [1, 2, 3]);
}

#[tokio::test]
async fn disabled_or_expired_retry_budget_does_not_reopen_stream() {
    for limited in [
        RetryPolicy {
            max_attempts: 1,
            ..policy()
        },
        RetryPolicy {
            total_timeout: Duration::ZERO,
            ..policy()
        },
    ] {
        let client = Client::new(vec![Reply::Events(vec![overload(false)])]);
        let result = Engine::new(client.clone())
            .with_retry_policy(limited)
            .run(&mut History::new(), "task")
            .await
            .result;
        assert!(matches!(result, EngineRunExit::Interrupted(_)));
        assert_eq!(client.count(), 1);
    }
}

#[tokio::test]
async fn unrelated_error_or_overload_in_message_alone_does_not_retry() {
    for code in [
        None,
        Some("authentication_error"),
        Some("invalid_request_error"),
        Some("overloaded_error"),
        Some("server_is_overloaded"),
    ] {
        let error = Event::Error(agen::llm_client::event::ErrorEvent {
            code: code.map(str::to_owned),
            message: "overloaded_error server_is_overloaded".into(),
            retry: None,
        });
        let client = Client::new(vec![Reply::Events(vec![error])]);
        let result = Engine::new(client.clone())
            .with_retry_policy(policy())
            .run(&mut History::new(), "task")
            .await
            .result;
        assert!(matches!(result, EngineRunExit::Interrupted(_)));
        assert_eq!(client.count(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn provider_delay_is_honored_or_rejected_without_early_retry() {
    for (seconds, should_retry) in [(3, true), (323, false), (u64::MAX, false)] {
        let payload = serde_json::json!({"type":"error", "error": {
            "code":"server_is_overloaded", "type":"service_unavailable_error",
            "message":"provider at capacity", "headers":{"retry-after":seconds.to_string()}
        }});
        let events = OpenAIResponsesScheme::new()
            .parse_sse("error", &payload.to_string(), &mut Default::default())
            .unwrap();
        let client = Client::new(vec![Reply::Events(events), finished()]);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = notices.clone();
        let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
        engine.on_llm_retry(move |_, notice| sink.lock().unwrap().push(notice.wait));
        let started = tokio::time::Instant::now();
        let result = engine.run(&mut History::new(), "task").await.result;
        if should_retry {
            assert!(matches!(result, EngineRunExit::Finished), "{result:?}");
            assert_eq!(client.count(), 2);
            assert!(started.elapsed() >= Duration::from_secs(seconds));
            // Local cap is zero: provider advice must not be shortened by it.
            assert_eq!(*notices.lock().unwrap(), [Duration::from_secs(seconds)]);
        } else {
            assert!(
                matches!(result, EngineRunExit::Interrupted(_)),
                "{result:?}"
            );
            let diagnostic = format!("{result:?}");
            assert!(diagnostic.contains("server_is_overloaded"));
            assert!(diagnostic.contains("retry-after"));
            assert!(diagnostic.contains(&seconds.to_string()));
            assert_eq!(client.count(), 1);
            assert!(notices.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn engine_retries_provider_advice_without_interpreting_diagnostic_code() {
    let client = Client::new(vec![
        Reply::Events(vec![Event::Error(agen::llm_client::event::ErrorEvent {
            code: Some("adapter_classified_transient".into()),
            message: "temporary failure".into(),
            retry: Some(agen::llm_client::event::RetryAdvice { retry_after: None }),
        })]),
        finished(),
    ]);
    let result = Engine::new(client.clone())
        .with_retry_policy(policy())
        .run(&mut History::new(), "task")
        .await
        .result;
    assert!(matches!(result, EngineRunExit::Finished), "{result:?}");
    assert_eq!(client.count(), 2);
}

#[tokio::test]
async fn overload_mid_response_continues_with_completed_blocks_and_shared_budget() {
    let partial = vec![
        Event::text_block_start(0),
        Event::text_delta(0, "saved partial"),
        Event::text_block_stop(0, None),
        Event::text_block_start(1),
        Event::text_delta(1, "uncommitted tail"),
        overload(false),
    ];
    let client = Client::new(vec![
        Reply::Events(partial),
        Reply::Events(vec![overload(false)]),
        finished(),
    ]);
    let mut history = History::new();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
    engine.on_llm_retry(move |_, n| sink.lock().unwrap().push(n.failed_attempt));
    let result = engine.run(&mut history, "task").await.result;
    assert!(matches!(result, EngineRunExit::Finished), "{result:?}");
    assert_eq!(client.count(), 3);
    assert_eq!(*notices.lock().unwrap(), [1, 2]);
    let requests = client.requests.lock().unwrap();
    assert!(
        serde_json::to_string(&requests[1].items)
            .unwrap()
            .contains("saved partial")
    );
    assert_eq!(history.items().filter(|item| matches!(item, Item::Message { role: agen::Role::Assistant, content, .. } if content.iter().any(|p| p.as_text().contains("saved partial")))).count(), 1);
}

#[tokio::test]
async fn overload_backoff_can_be_cancelled_or_paused_without_resending() {
    for cancel in [true, false] {
        let client = Client::new(vec![Reply::Events(vec![overload(false)])]);
        let mut engine = Engine::new(client.clone()).with_retry_policy(RetryPolicy {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(1),
            ..policy()
        });
        let signal = if cancel {
            engine.cancel_sender()
        } else {
            engine.pause_sender()
        };
        engine.on_llm_retry(move |_, _| {
            signal.try_send(()).unwrap();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            engine.run(&mut History::new(), "task"),
        )
        .await
        .unwrap()
        .result;
        if cancel {
            assert!(
                matches!(
                    result,
                    EngineRunExit::Interrupted(agen::RunInterruptionReason::Cancelled)
                ),
                "{result:?}"
            );
        } else {
            assert!(matches!(result, EngineRunExit::Paused), "{result:?}");
        }
        assert_eq!(client.count(), 1);
    }
}

#[tokio::test]
async fn overload_before_tool_admission_drops_unexecuted_calls_and_retries() {
    let tool = SideEffect {
        count: Arc::default(),
        executed: Arc::default(),
    };
    let count = tool.count.clone();
    let client = Client::new(vec![
        Reply::Events(vec![
            Event::tool_use_start(0, "unexecuted", "Effect"),
            Event::tool_input_delta(0, "{}"),
            Event::tool_use_stop(0),
            overload(false),
        ]),
        finished(),
    ]);
    let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::AfterResponse);
    engine.register_tool(Arc::new(move || {
        (
            agen::tool::ToolMeta::new("Effect")
                .description("count one effect")
                .input_schema(serde_json::json!({"type":"object"})),
            Arc::new(tool.clone()),
        )
    }));
    let mut history = History::new();
    let result = engine.run(&mut history, "task").await.result;
    assert!(matches!(result, EngineRunExit::Finished), "{result:?}");
    assert_eq!(client.count(), 2);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(!history.items().any(|item| matches!(item, Item::ToolCall { call_id, .. } | Item::ToolResult { call_id, .. } if call_id == "unexecuted")));
}

#[tokio::test]
async fn successful_response_resets_retry_budget_without_reexecuting_prior_tools() {
    let tool = SideEffect {
        count: Arc::default(),
        executed: Arc::default(),
    };
    let count = tool.count.clone();
    let client = Client::new(vec![
        Reply::Events(vec![overload(false)]),
        Reply::Events(vec![
            Event::tool_use_start(0, "effect", "Effect"),
            Event::tool_input_delta(0, "{}"),
            Event::tool_use_stop(0),
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }),
        ]),
        Reply::Events(vec![overload(false)]),
        finished(),
    ]);
    let mut engine = Engine::new(client.clone()).with_retry_policy(RetryPolicy {
        max_attempts: 2,
        ..policy()
    });
    engine.register_tool(Arc::new(move || {
        (
            agen::tool::ToolMeta::new("Effect")
                .description("count one effect")
                .input_schema(serde_json::json!({"type":"object"})),
            Arc::new(tool.clone()),
        )
    }));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    engine.on_llm_retry(move |_, n| sink.lock().unwrap().push(n.failed_attempt));
    let mut history = History::new();
    let result = engine.run(&mut history, "task").await.result;
    assert!(matches!(result, EngineRunExit::Finished), "{result:?}");
    assert_eq!(client.count(), 4);
    assert_eq!(*notices.lock().unwrap(), [1, 1]);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        history
            .items()
            .filter(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "effect"))
            .count(),
        1
    );
}

#[derive(Clone)]
struct SideEffect {
    count: Arc<AtomicUsize>,
    executed: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl agen::tool::Tool for SideEffect {
    async fn execute(
        &self,
        _: &str,
        _: agen::tool::ToolExecutionContext,
    ) -> Result<agen::tool::ToolOutput, agen::tool::ToolError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.executed.notify_one();
        Ok(agen::tool::ToolOutput {
            summary: "effect committed".into(),
            content: None,
            attachments: Vec::new(),
        })
    }
}

#[tokio::test]
async fn overload_after_early_tool_execution_stops_without_replaying_side_effect() {
    let executed = Arc::new(tokio::sync::Notify::new());
    let tool = SideEffect {
        count: Arc::default(),
        executed: executed.clone(),
    };
    let count = tool.count.clone();
    let mut client = Client::new(vec![Reply::Events(vec![
        Event::tool_use_start(0, "effect", "Effect"),
        Event::tool_input_delta(0, "{}"),
        Event::tool_use_stop(0),
        overload(false),
    ])]);
    client.before_error = Some(executed);
    let mut engine = Engine::new(client.clone()).with_retry_policy(policy());
    engine.set_tool_call_dispatch_mode(ToolCallDispatchMode::OnToolCallComplete);
    engine.register_tool(Arc::new(move || {
        (
            agen::tool::ToolMeta::new("Effect")
                .description("count one effect")
                .input_schema(serde_json::json!({"type":"object"})),
            Arc::new(tool.clone()),
        )
    }));
    let mut history = History::new();
    let result = tokio::time::timeout(Duration::from_secs(5), engine.run(&mut history, "task"))
        .await
        .unwrap()
        .result;
    assert!(
        matches!(result, EngineRunExit::Interrupted(_)),
        "{result:?}"
    );
    assert_eq!(client.count(), 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        history
            .items()
            .filter(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "effect"))
            .count(),
        1
    );
}
