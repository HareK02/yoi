use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::llm_client::{ClientError, LlmClient, Request, ResponseStream};
use agen::tool::{Attachment, ImageAttachment};
use agen::{Engine, EngineRunExit, History, Item, ToolResultDisposition};
use async_trait::async_trait;
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
#[derive(Clone)]
struct Client {
    requests: Arc<Mutex<Vec<Request>>>,
    failures: usize,
    other_error: bool,
}
impl Client {
    fn new(failures: usize) -> Self {
        Self {
            requests: Arc::default(),
            failures,
            other_error: false,
        }
    }
}
#[async_trait]
impl LlmClient for Client {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request);
        if requests.len() <= self.failures {
            return Err(rejection());
        }
        if self.other_error {
            return Err(ClientError::Api {
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
}
#[async_trait]
impl agen::tool::Tool for CountingTool {
    async fn execute(
        &self,
        _: &str,
        _: agen::tool::ToolExecutionContext,
    ) -> Result<agen::tool::ToolOutput, agen::tool::ToolError> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
