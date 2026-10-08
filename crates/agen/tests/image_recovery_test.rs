use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::llm_client::scheme::{Scheme, openai_responses::OpenAIResponsesScheme};
use agen::llm_client::{ClientError, LlmClient, Request, ResponseStream};
use agen::tool::{Attachment, ImageAttachment};
use agen::{Engine, EngineRunExit, History, Item, ToolResultDisposition};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn rejection() -> ClientError {
    ClientError::Api { status: Some(400), code: Some("invalid_value".into()), message: "The image you provided requires 52150 patches after processing, exceeding the limit of 30000. Please resize the image and try again.".into(), retry_after: None }
}

fn image_result(id: &str, width: u32, height: u32, padding: usize) -> Item {
    // PNG header: dimension inspection never decodes/allocates the pixel plane.
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.resize(40 + padding, 0);
    Item::ToolResult {
        id: None,
        call_id: id.into(),
        summary: "Attached image".into(),
        content: None,
        disposition: ToolResultDisposition::Success,
        is_error: false,
        attachments: vec![Attachment::Image(ImageAttachment::new("image/png", bytes))],
    }
}
fn history() -> History {
    History::from_items(vec![
        Item::user_message("inspect"),
        Item::tool_call("small", "ViewImage", "{}"),
        Item::tool_call("large", "ViewImage", "{}"),
        image_result("small", 10, 10, 2000),
        image_result("large", 1600, 33349, 0),
    ])
}
fn images(request: &Request) -> Vec<String> {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            Item::ToolResult {
                call_id,
                attachments,
                ..
            } if !attachments.is_empty() => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}
#[derive(Clone, Copy)]
enum RejectionPath {
    Http,
    SseError,
    SseNestedError,
    ResponseFailed,
}
const STREAM_PATHS: [RejectionPath; 3] = [
    RejectionPath::SseError,
    RejectionPath::SseNestedError,
    RejectionPath::ResponseFailed,
];

#[derive(Clone)]
struct Client {
    requests: Arc<Mutex<Vec<Request>>>,
    failures: usize,
    other_error: bool,
    path: RejectionPath,
    prefix: Vec<(&'static str, Value)>,
    wait_before_error: Option<Arc<tokio::sync::Notify>>,
}
impl Client {
    fn new(failures: usize) -> Self {
        Self {
            requests: Arc::default(),
            failures,
            other_error: false,
            path: RejectionPath::Http,
            prefix: Vec::new(),
            wait_before_error: None,
        }
    }
    fn streamed(failures: usize, path: RejectionPath) -> Self {
        Self {
            path,
            ..Self::new(failures)
        }
    }
    fn reject(&self, error: ClientError) -> Result<ResponseStream, ClientError> {
        if matches!(self.path, RejectionPath::Http) {
            return Err(error);
        }
        let ClientError::Api { code, message, .. } = error else {
            unreachable!()
        };
        let error = json!({"type":"invalid_request_error", "code":code, "message":message, "param":"input"});
        let (kind, data) = match self.path {
            RejectionPath::SseError => (
                "error",
                json!({"type":"error", "code":code, "message":message, "param":"input", "error_type":"invalid_request_error"}),
            ),
            RejectionPath::SseNestedError => ("error", json!({"type":"error", "error":error})),
            RejectionPath::ResponseFailed => (
                "response.failed",
                json!({"type":"response.failed", "response":{"id":"rejected", "error":error}}),
            ),
            RejectionPath::Http => unreachable!(),
        };
        let scheme = OpenAIResponsesScheme::new();
        let mut state = Default::default();
        let mut events = scheme.parse_sse("response.created", r#"{"response":{}}"#, &mut state)?;
        for (kind, data) in &self.prefix {
            events.extend(scheme.parse_sse(kind, &data.to_string(), &mut state)?);
        }
        events.extend(scheme.parse_sse(kind, &data.to_string(), &mut state)?);
        let wait = self.wait_before_error.clone();
        Ok(Box::pin(futures::stream::iter(events).then(move |event| {
            let wait = wait.clone();
            async move {
                if matches!(event, Event::Error(_))
                    && let Some(wait) = wait
                {
                    wait.notified().await;
                }
                Ok(event)
            }
        })))
    }
}
#[async_trait]
impl LlmClient for Client {
    fn tool_call_completion_support(&self) -> agen::llm_client::ToolCallCompletionSupport {
        agen::llm_client::ToolCallCompletionSupport::PerBlock
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request);
        if requests.len() <= self.failures {
            return self.reject(rejection());
        }
        if self.other_error {
            return self.reject(ClientError::Api {
                status: Some(400),
                code: Some("invalid_value".into()),
                message: "invalid tool arguments".into(),
                retry_after: None,
            });
        }
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(Event::text_block_start(0)),
            Ok(Event::text_delta(0, "resize and retry")),
            Ok(Event::text_block_stop(0, None)),
            Ok(Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            })),
        ])))
    }
}

#[tokio::test]
async fn removes_largest_pixels_not_largest_file_and_keeps_pairing() {
    let client = Client::new(1);
    let observer = client.clone();
    let mut history = history();
    let mut engine = Engine::new(client);
    let committed = Arc::new(Mutex::new(Vec::new()));
    let commits = committed.clone();
    engine.set_image_rejection_handler(move |original, replacement| {
        commits
            .lock()
            .unwrap()
            .push((original.item.clone(), replacement.clone()));
        Ok(())
    });
    let output = engine.run(&mut history, "continue").await;
    assert!(
        matches!(output.result, EngineRunExit::Finished),
        "{:?}",
        output.result
    );
    let requests = observer.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(images(&requests[0]), ["small", "large"]);
    assert_eq!(images(&requests[1]), ["small"]);
    assert!(requests[1].items.iter().any(|item| matches!(item, Item::ToolResult { call_id, is_error: true, disposition: ToolResultDisposition::Error, summary, .. } if call_id == "large" && summary.contains("retry ViewImage"))));
    assert_eq!(committed.lock().unwrap().len(), 1);
    assert_eq!(
        history
            .items()
            .filter(|item| matches!(item, Item::ToolResult {call_id, ..} if call_id == "large"))
            .count(),
        1
    );
}

#[tokio::test]
async fn multiple_rejections_remove_one_image_at_a_time_and_stop_at_exhaustion() {
    let client = Client::new(99);
    let observer = client.clone();
    let mut history = history();
    let out = Engine::new(client).run(&mut history, "continue").await;
    assert!(matches!(out.result, EngineRunExit::Interrupted(_)));
    let requests = observer.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(images(&requests[1]), ["small"]);
    assert!(images(&requests[2]).is_empty());
}

#[tokio::test]
async fn unrelated_error_stops_recovery_without_removing_another_image() {
    let mut client = Client::new(1);
    client.other_error = true;
    let observer = client.clone();
    let mut history = history();
    let out = Engine::new(client).run(&mut history, "continue").await;
    assert!(matches!(out.result, EngineRunExit::Interrupted(_)));
    assert_eq!(observer.requests.lock().unwrap().len(), 2);
    assert!(history.items().any(|item| matches!(item, Item::ToolResult {call_id, attachments, ..} if call_id == "small" && !attachments.is_empty())));
}

#[tokio::test]
async fn persistence_failure_does_not_change_history_or_retry() {
    let client = Client::new(99);
    let observer = client.clone();
    let mut history = history();
    let original = history.items_cloned();
    let mut engine = Engine::new(client);
    engine.set_image_rejection_handler(|_, _| Err("disk full".into()));
    let out = engine.run(&mut history, "continue").await;
    assert!(matches!(out.result, EngineRunExit::Interrupted(_)));
    assert_eq!(
        &history.items_cloned()[..original.len()],
        original.as_slice()
    );
    assert_eq!(observer.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn previously_accepted_images_are_not_recovery_candidates() {
    let client = Client::new(99);
    let observer = client.clone();
    let mut history = history();
    history.push(Item::assistant_message("I saw those images"));
    history.push(Item::tool_call("new", "ViewImage", "{}"));
    history.push(image_result("new", 1, 1, 0));
    let out = Engine::new(client).run(&mut history, "continue").await;
    assert!(matches!(out.result, EngineRunExit::Interrupted(_)));
    let requests = observer.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    // Already-consumed images are projected out, but their durable records
    // must not be rewritten as failures.
    assert!(images(&requests[1]).is_empty());
    assert_eq!(history.items().filter(|item| matches!(item, Item::ToolResult {call_id, is_error: false, attachments, ..} if (call_id == "small" || call_id == "large") && !attachments.is_empty())).count(), 2);
}

#[tokio::test]
async fn legacy_empty_failed_request_boundaries_do_not_hide_candidates() {
    let client = Client::new(1);
    let observer = client.clone();
    let mut history = history();
    history.push(Item::assistant_response_boundary("failed-1"));
    history.push(Item::assistant_response_boundary("failed-2"));
    let out = Engine::new(client).run(&mut history, "continue").await;
    assert!(matches!(out.result, EngineRunExit::Finished));
    assert_eq!(images(&observer.requests.lock().unwrap()[1]), ["small"]);
}

#[tokio::test]
async fn early_results_remain_candidates_despite_later_text_in_same_response() {
    let client = Client::new(1);
    let observer = client.clone();
    let mut history = History::from_items(vec![
        Item::user_message("inspect"),
        Item::assistant_response_boundary("response-1"),
        Item::tool_call("image", "ViewImage", "{}"),
        image_result("image", 99, 99, 0),
        Item::assistant_message("waiting for image"),
    ]);
    let out = Engine::new(client).run(&mut history, "continue").await;
    assert!(
        matches!(out.result, EngineRunExit::Finished),
        "{:?}; requests: {:?}",
        out.result,
        observer.requests.lock().unwrap()
    );
    assert!(images(&observer.requests.lock().unwrap()[1]).is_empty());
}

#[derive(Clone)]
struct CountingTool {
    image: bool,
    count: Arc<std::sync::atomic::AtomicUsize>,
    executed: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait]
impl agen::tool::Tool for CountingTool {
    async fn execute(
        &self,
        _: &str,
        _: agen::tool::ToolExecutionContext,
    ) -> Result<agen::tool::ToolOutput, agen::tool::ToolError> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(executed) = &self.executed {
            executed.notify_one();
        }
        let attachments = if self.image {
            let Item::ToolResult { attachments, .. } = image_result("image", 1600, 33349, 0) else {
                unreachable!()
            };
            attachments
        } else {
            Vec::new()
        };
        Ok(agen::tool::ToolOutput {
            summary: "executed once".into(),
            content: None,
            attachments,
        })
    }
}
#[derive(Clone)]
struct ToolClient(Client);
#[async_trait]
impl LlmClient for ToolClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    fn tool_call_completion_support(&self) -> agen::llm_client::ToolCallCompletionSupport {
        agen::llm_client::ToolCallCompletionSupport::PerBlock
    }
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        if self.0.requests.lock().unwrap().is_empty() {
            self.0.requests.lock().unwrap().push(request);
            return Ok(Box::pin(futures::stream::iter(
                vec![
                    Event::tool_use_start(0, "image", "ViewImage"),
                    Event::tool_input_delta(0, "{}"),
                    Event::tool_use_stop(0),
                    Event::tool_use_start(1, "side-effect", "Bash"),
                    Event::tool_input_delta(1, "{}"),
                    Event::tool_use_stop(1),
                    Event::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]
                .into_iter()
                .map(Ok),
            )));
        }
        self.0.stream(request).await
    }
}

#[tokio::test]
async fn fresh_image_recovery_does_not_repeat_tools_in_either_dispatch_mode() {
    for mode in [
        agen::ToolCallDispatchMode::AfterResponse,
        agen::ToolCallDispatchMode::OnToolCallComplete,
    ] {
        let client = Client::new(2);
        let mut engine = Engine::new(ToolClient(client.clone()));
        engine.set_tool_call_dispatch_mode(mode);
        let mut counts = Vec::new();
        for (name, image) in [("ViewImage", true), ("Bash", false)] {
            let tool = CountingTool {
                image,
                count: Arc::default(),
                executed: None,
            };
            counts.push(tool.count.clone());
            engine.register_tool(Arc::new(move || {
                (
                    agen::tool::ToolMeta::new(name)
                        .description("test")
                        .input_schema(serde_json::json!({"type":"object"})),
                    Arc::new(tool.clone()),
                )
            }));
        }
        let mut history = History::new();
        let out = engine.run(&mut history, "inspect").await;
        assert!(
            matches!(out.result, EngineRunExit::Finished),
            "{:?}",
            out.result
        );
        assert!(
            counts
                .iter()
                .all(|count| count.load(std::sync::atomic::Ordering::SeqCst) == 1)
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(images(&requests[1]), ["image"]);
        assert!(images(&requests[2]).is_empty());
    }
}

#[test]
fn image_error_classifier_is_not_a_generic_400_retry() {
    assert!(rejection().is_image_size_rejection());
    for (status, message) in [
        (400, "invalid image format"),
        (400, "image URL not found"),
        (400, "context window exceeds token limit"),
        (401, "image is too large"),
        (500, "image is too large"),
    ] {
        assert!(
            !ClientError::Api {
                status: Some(status),
                code: Some("invalid_value".into()),
                message: message.into(),
                retry_after: None
            }
            .is_image_size_rejection()
        );
    }
}

#[tokio::test]
async fn streamed_image_rejections_correct_largest_candidate_before_next_request() {
    for path in STREAM_PATHS {
        let client = Client::streamed(1, path);
        let mut history = history();
        let mut engine = Engine::new(client.clone());
        let committed = Arc::new(Mutex::new(Vec::new()));
        let commits = committed.clone();
        engine.set_image_rejection_handler(move |original, replacement| {
            commits
                .lock()
                .unwrap()
                .push((original.item.clone(), replacement.clone()));
            Ok(())
        });
        let out = engine.run(&mut history, "continue").await;
        assert!(
            matches!(out.result, EngineRunExit::Finished),
            "{:?}",
            out.result
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(images(&requests[0]), ["small", "large"]);
        assert_eq!(images(&requests[1]), ["small"]);
        assert!(requests[1].items.iter().any(|item| matches!(item, Item::ToolResult { call_id, is_error: true, disposition: ToolResultDisposition::Error, summary, .. } if call_id == "large" && summary.contains("Resize or crop") && summary.contains("retry ViewImage"))));
        assert!(requests[1].items.iter().any(|item| matches!(item, Item::ToolCall { call_id, name, .. } if call_id == "large" && name == "ViewImage")));
        assert_eq!(committed.lock().unwrap().len(), 1);
        assert_eq!(
            history
                .items()
                .filter(
                    |item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "large")
                )
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn streamed_rejections_stop_when_images_are_absent_or_candidates_exhausted() {
    for path in STREAM_PATHS {
        for (mut history, expected_requests) in [(History::new(), 1), (history(), 3)] {
            let client = Client::streamed(99, path);
            let out = Engine::new(client.clone())
                .run(&mut history, "continue")
                .await;
            assert!(
                matches!(out.result, EngineRunExit::Interrupted(_)),
                "{:?}",
                out.result
            );
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), expected_requests);
            assert!(images(requests.last().unwrap()).is_empty());
        }
    }
}

#[tokio::test]
async fn unrelated_streamed_invalid_value_preserves_images_and_diagnostic() {
    for path in STREAM_PATHS {
        let mut client = Client::streamed(0, path);
        client.other_error = true;
        let mut history = history();
        let original = history.items_cloned();
        let out = Engine::new(client.clone())
            .run(&mut history, "continue")
            .await;
        let EngineRunExit::Interrupted(reason) = out.result else {
            panic!("{:?}", out.result)
        };
        let reason = format!("{reason:?}");
        assert!(
            reason.contains("invalid_value")
                && reason.contains("invalid tool arguments")
                && reason.contains("diagnostic=")
                && reason.contains("input"),
            "{reason}"
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(
            &history.items_cloned()[..original.len()],
            original.as_slice()
        );
    }
}

#[tokio::test]
async fn streamed_correction_commit_failure_does_not_mutate_or_resend() {
    for path in STREAM_PATHS {
        let client = Client::streamed(99, path);
        let mut history = history();
        let original = history.items_cloned();
        let mut engine = Engine::new(client.clone());
        engine.set_image_rejection_handler(|_, _| Err("disk full".into()));
        let out = engine.run(&mut history, "continue").await;
        assert!(matches!(out.result, EngineRunExit::Interrupted(_)));
        assert_eq!(
            &history.items_cloned()[..original.len()],
            original.as_slice()
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn streamed_rejection_after_output_never_replays_or_corrects_input() {
    for path in STREAM_PATHS {
        // Start alone closes the replay window, as do partial/completed text,
        // reasoning and unrecognized events. None may change accepted input.
        for prefix in [
            vec![(
                "response.content_part.added",
                json!({"output_index":0,"content_index":0,"part":{"type":"output_text"}}),
            )],
            vec![(
                "response.output_text.delta",
                json!({"output_index":0,"content_index":0,"delta":"partial"}),
            )],
            vec![
                (
                    "response.output_text.delta",
                    json!({"output_index":0,"content_index":0,"delta":"completed"}),
                ),
                (
                    "response.content_part.done",
                    json!({"output_index":0,"content_index":0,"part":{"type":"output_text"}}),
                ),
            ],
            vec![(
                "response.reasoning_text.delta",
                json!({"output_index":0,"content_index":0,"delta":"thinking"}),
            )],
            vec![("future.unknown", json!({"value":"unknown semantics"}))],
        ] {
            let mut client = Client::streamed(1, path);
            client.prefix = prefix;
            let mut history = history();
            let original = history.items_cloned();
            let out = Engine::new(client.clone())
                .run(&mut history, "continue")
                .await;
            assert!(
                matches!(out.result, EngineRunExit::Interrupted(_)),
                "{:?}",
                out.result
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            assert_eq!(
                &history.items_cloned()[..original.len()],
                original.as_slice()
            );
        }
    }
}

#[test]
fn statusless_api_errors_are_not_image_retry_authority() {
    let ClientError::Api { code, message, .. } = rejection() else {
        unreachable!()
    };
    assert!(
        !ClientError::Api {
            status: None,
            code,
            message,
            retry_after: None
        }
        .is_image_size_rejection()
    );
}

#[tokio::test]
async fn streamed_rejection_after_tool_completion_never_reexecutes_side_effects() {
    for path in STREAM_PATHS {
        for mode in [
            agen::ToolCallDispatchMode::AfterResponse,
            agen::ToolCallDispatchMode::OnToolCallComplete,
        ] {
            let executed = Arc::new(tokio::sync::Notify::new());
            let mut client = Client::streamed(1, path);
            client.prefix = vec![
                (
                    "response.output_item.added",
                    json!({"output_index":0,"item":{"type":"function_call","call_id":"side-effect","name":"Bash","arguments":"{}"}}),
                ),
                (
                    "response.output_item.done",
                    json!({"output_index":0,"item":{"type":"function_call","call_id":"side-effect","name":"Bash","arguments":"{}"}}),
                ),
            ];
            if mode == agen::ToolCallDispatchMode::OnToolCallComplete {
                client.wait_before_error = Some(executed.clone());
            }
            let tool = CountingTool {
                image: false,
                count: Arc::default(),
                executed: Some(executed),
            };
            let count = tool.count.clone();
            let mut engine = Engine::new(client.clone());
            engine.set_tool_call_dispatch_mode(mode);
            engine.register_tool(Arc::new(move || {
                (
                    agen::tool::ToolMeta::new("Bash")
                        .description("side effect")
                        .input_schema(json!({"type":"object"})),
                    Arc::new(tool.clone()),
                )
            }));
            let mut history = history();
            let original = history.items_cloned();
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                engine.run(&mut history, "continue"),
            )
            .await
            .expect("tool admission/error cleanup must terminate");
            assert!(
                matches!(out.result, EngineRunExit::Interrupted(_)),
                "{:?}",
                out.result
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            assert_eq!(
                &history.items_cloned()[..original.len()],
                original.as_slice()
            );
            if mode == agen::ToolCallDispatchMode::OnToolCallComplete {
                assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert_eq!(history.items().filter(|item| matches!(item, Item::ToolCall { call_id, .. } if call_id == "side-effect")).count(), 1);
                assert_eq!(history.items().filter(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "side-effect")).count(), 1);
            } else {
                assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
                assert!(!history.items().any(|item| matches!(item, Item::ToolCall { call_id, .. } | Item::ToolResult { call_id, .. } if call_id == "side-effect")));
            }
        }
    }
}

#[test]
fn streamed_error_extra_fields_do_not_turn_unrelated_message_into_image_rejection() {
    let scheme = OpenAIResponsesScheme::new();
    let mut state = Default::default();
    let events = scheme.parse_sse("error", r#"{"type":"error","code":"invalid_value","message":"invalid tool arguments","param":"input","input":"image too large"}"#, &mut state).unwrap();
    let Event::Error(error) = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(error.code.as_deref(), Some("invalid_value"));
    assert!(
        error
            .message
            .starts_with("invalid tool arguments | diagnostic=")
    );
    // HTTP uses the same diagnostic policy, but still requires status authority.
    assert!(
        !ClientError::Api {
            status: Some(400),
            code: error.code.clone(),
            message: error.message.clone(),
            retry_after: None
        }
        .is_image_size_rejection()
    );
}
