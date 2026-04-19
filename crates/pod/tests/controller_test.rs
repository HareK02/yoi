use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use session_store::FsStore;

use pod::{Event, Method, Pod, PodController, PodManifest, PodStatus};

// ---------------------------------------------------------------------------
// Mock LLM Client
// ---------------------------------------------------------------------------

/// One scripted mock response.
#[derive(Clone)]
enum MockResponse {
    /// Emit the events and let the stream terminate naturally.
    Complete(Vec<LlmEvent>),
    /// Emit the events and then pend forever so the Worker blocks on
    /// `stream.next()` — used to exercise the Cancel/Pause path while a
    /// turn is actively in flight.
    Hang(Vec<LlmEvent>),
}

#[derive(Clone)]
struct MockClient {
    responses: Arc<Vec<MockResponse>>,
    call_count: Arc<AtomicUsize>,
    captured: Arc<Mutex<Vec<Request>>>,
}

impl MockClient {
    fn new(events: Vec<LlmEvent>) -> Self {
        Self::sequential(vec![MockResponse::Complete(events)])
    }

    /// Script multiple sequential responses. The Nth call to `stream()`
    /// returns the Nth entry.
    fn sequential(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn captured_requests(&self) -> Vec<Request> {
        self.captured.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for MockClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.captured.lock().unwrap().push(request);
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Api {
                status: Some(500),
                code: Some("mock".into()),
                message: "No more responses".into(),
            });
        }
        let response = self.responses[count].clone();
        let (events, hang) = match response {
            MockResponse::Complete(e) => (e, false),
            MockResponse::Hang(e) => (e, true),
        };
        let iter = futures::stream::iter(events.into_iter().map(Ok));
        if hang {
            let pending = futures::stream::pending::<Result<LlmEvent, ClientError>>();
            Ok(Box::pin(iter.chain(pending)))
        } else {
            Ok(Box::pin(iter))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_text_events() -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "Hello"),
        LlmEvent::text_delta(0, " World"),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

const MANIFEST_TOML: &str = r#"
[pod]
name = "test-pod"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[[scope.allow]]
target = "./"
permission = "write"
"#;

async fn make_pod(client: MockClient) -> Pod<MockClient, FsStore> {
    let manifest = PodManifest::from_toml(MANIFEST_TOML).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    std::mem::forget(store_tmp);

    // Separate tempdir to serve as the Pod's pwd/scope — these tests
    // exercise the controller via a mock client and never touch the
    // filesystem through tools, so a throwaway writable dir is enough.
    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = manifest::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let worker = Worker::new(client);
    Pod::new(manifest, worker, store, pwd, scope).await.unwrap()
}

use pod::PodHandle;

async fn spawn_controller(pod: Pod<MockClient, FsStore>) -> PodHandle {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_base = tmp.path().to_owned();
    std::mem::forget(tmp);
    let (handle, _shutdown_rx) = PodController::spawn(pod, &runtime_base).await.unwrap();
    handle
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shared_state_starts_idle() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);
}

#[tokio::test]
async fn run_updates_shared_state_to_idle_after_completion() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    handle
        .send(Method::Run {
            input: "Hello".into(),
        })
        .await
        .unwrap();

    // Wait for the run to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);
}

#[tokio::test]
async fn run_populates_history() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    handle
        .send(Method::Run {
            input: "Hello".into(),
        })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let history = handle.shared_state.history_json();
    assert_ne!(history, "[]");
    let parsed: serde_json::Value = serde_json::from_str(&history).unwrap();
    assert!(parsed.is_array());
    assert!(parsed.as_array().unwrap().len() >= 2); // user + assistant
}

#[tokio::test]
async fn events_are_broadcast() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Run {
            input: "Hello".into(),
        })
        .await
        .unwrap();

    let mut saw_turn_start = false;
    let mut saw_text_delta = false;
    let mut saw_text_done = false;
    let mut saw_turn_end = false;

    // Collect events with a timeout
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::TurnStart { .. }) => saw_turn_start = true,
                    Ok(Event::TextDelta { .. }) => saw_text_delta = true,
                    Ok(Event::TextDone { .. }) => saw_text_done = true,
                    Ok(Event::TurnEnd { .. }) => {
                        saw_turn_end = true;
                        break;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_turn_start, "should see turn_start");
    assert!(saw_text_delta, "should see text_delta");
    assert!(saw_text_done, "should see text_done");
    assert!(saw_turn_end, "should see turn_end");
}

#[tokio::test]
async fn double_run_returns_error() {
    // Create a client that streams slowly
    let events = vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "slow..."),
        // No stop/completed — the stream will end but without proper completion
    ];
    let client = MockClient::new(events);
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    // Send first run
    handle
        .send(Method::Run {
            input: "first".into(),
        })
        .await
        .unwrap();

    // Immediately send second run (should get error)
    handle
        .send(Method::Run {
            input: "second".into(),
        })
        .await
        .unwrap();

    // Look for the error event
    let mut saw_already_running = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) => {
                        if code == pod::ErrorCode::AlreadyRunning {
                            saw_already_running = true;
                            break;
                        }
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_already_running, "should see already_running error");
}

#[tokio::test]
async fn resume_without_pause_returns_error() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle.send(Method::Resume).await.unwrap();

    let mut saw_not_paused = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == pod::ErrorCode::NotPaused => {
                        saw_not_paused = true;
                        break;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_not_paused, "should see not_paused error");
}

#[tokio::test]
async fn cancel_without_run_returns_error() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle.send(Method::Cancel).await.unwrap();

    let mut saw_not_running = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == pod::ErrorCode::NotRunning => {
                        saw_not_running = true;
                        break;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_not_running, "should see not_running error");
}

#[tokio::test]
async fn notify_while_idle_auto_starts_turn_and_injects_system_message() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Notify {
            message: "turn finished".into(),
        })
        .await
        .unwrap();

    // Wait for the auto-started turn to complete.
    let mut saw_turn_end = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::TurnEnd { .. }) => { saw_turn_end = true; break; }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(saw_turn_end, "auto-triggered turn should complete");
    // Status flips back to Idle on the controller thread after RunEnd.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);

    // Exactly one request was made; it must contain the formatted
    // notification as the last item (injected into request_context by
    // PodInterceptor::pre_llm_request).
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 1, "one LLM call expected");
    let last_item_text = requests[0]
        .items
        .last()
        .and_then(|i| i.as_text())
        .unwrap_or_default()
        .to_string();
    assert!(
        last_item_text.contains("[Notification]"),
        "injected system message missing, got: {last_item_text:?}"
    );
    assert!(last_item_text.contains("turn finished"));
    assert!(last_item_text.contains("not a blocking request"));
}

#[tokio::test]
async fn notify_while_running_does_not_emit_already_running_error() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Run {
            input: "start".into(),
        })
        .await
        .unwrap();
    handle
        .send(Method::Notify {
            message: "ping".into(),
        })
        .await
        .unwrap();

    // Drain events until the run ends; AlreadyRunning must never appear.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == pod::ErrorCode::AlreadyRunning => {
                        panic!("Notify while running must not produce AlreadyRunning");
                    }
                    Ok(Event::TurnEnd { .. }) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
}

#[tokio::test]
async fn status_json_reflects_pod_name() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    let json = handle.shared_state.status_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["pod_name"], "test-pod");
}

// ---------------------------------------------------------------------------
// Socket transport tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn socket_run_receives_events() {
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    // Give the socket server a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    // Send run method via socket
    writer
        .write(&Method::Run {
            input: "Hello".into(),
        })
        .await
        .unwrap();

    // Collect events
    let mut saw_turn_start = false;
    let mut saw_text_delta = false;
    let mut saw_turn_end = false;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::TurnStart { .. })) => saw_turn_start = true,
                    Ok(Some(Event::TextDelta { .. })) => saw_text_delta = true,
                    Ok(Some(Event::TurnEnd { .. })) => {
                        saw_turn_end = true;
                        break;
                    }
                    Ok(None) | Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_turn_start, "should see turn_start via socket");
    assert!(saw_text_delta, "should see text_delta via socket");
    assert!(saw_turn_end, "should see turn_end via socket");
}

#[tokio::test]
async fn socket_invalid_method_returns_error() {
    use protocol::stream::JsonLineReader;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);

    // Send garbage
    writer.write_all(b"{\"bad\":\"json\"}\n").await.unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut saw_error = false;
    loop {
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::Error { .. })) => {
                        saw_error = true;
                        break;
                    }
                    Ok(None) | Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_error, "should see error for invalid method");
}

// ---------------------------------------------------------------------------
// Pause / Resume / Paused→Run
// ---------------------------------------------------------------------------

/// Tool that pends forever when called. Used to park a turn between
/// the ToolCall being committed to history and its ToolResult being
/// produced, so a `Method::Pause` leaves an orphan `tool_use` behind.
struct HangingTool;

#[async_trait]
impl Tool for HangingTool {
    async fn execute(&self, _input: &str) -> Result<ToolOutput, ToolError> {
        std::future::pending::<()>().await;
        unreachable!()
    }
}

fn hanging_tool_definition(name: &'static str) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description("test-only tool that pends forever")
                .input_schema(serde_json::json!({"type": "object"})),
            Arc::new(HangingTool) as Arc<dyn Tool>,
        )
    })
}

async fn drain_until<F: FnMut(&Event) -> bool>(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    timeout: std::time::Duration,
    mut done: F,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(e) => { if done(&e) { return true; } }
                    Err(_) => return false,
                }
            }
            _ = tokio::time::sleep_until(deadline) => return false,
        }
    }
}

/// Pause mid-stream, then Resume: status round-trips Running →
/// Paused → Running → Idle, and the final history contains exactly
/// one user turn plus the assistant reply produced by the resume call.
#[tokio::test]
async fn pause_then_resume_transitions_and_preserves_history_consistency() {
    // Response 1: hang after opening a text block (no stop / completed),
    // so the Worker is parked inside the stream read and `cancel_rx`
    // races it cleanly on Method::Pause.
    let hang = MockResponse::Hang(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "partial..."),
    ]);
    // Response 2: a clean assistant reply delivered on Resume.
    let ok = MockResponse::Complete(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "resumed output"),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockClient::sequential(vec![hang, ok]);
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Run {
            input: "hello".into(),
        })
        .await
        .unwrap();

    // Wait for the partial text_delta to confirm the first stream is
    // live before we pause.
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::TextDelta { .. }
        ))
        .await,
        "text_delta should arrive before pause"
    );

    handle.send(Method::Pause).await.unwrap();

    // The controller emits RunEnd { Paused } when the
    // WorkerError::Cancelled is translated under pause_requested.
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::Paused
            }
        ))
        .await,
        "expected RunEnd::Paused after Pause"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Paused);

    handle.send(Method::Resume).await.unwrap();

    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::Finished
            }
        ))
        .await,
        "expected RunEnd::Finished after Resume"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);

    // History consistency: exactly [user "hello", assistant
    // "resumed output"]. No artifacts from the aborted stream
    // (partial text is not committed), no orphan tool_use.
    let history_json = handle.shared_state.history_json();
    let items: Vec<serde_json::Value> = serde_json::from_str(&history_json).unwrap();
    let roles: Vec<&str> = items
        .iter()
        .filter_map(|i| i["role"].as_str())
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant"],
        "history = user + assistant only; got {items:?}"
    );
    let assistant_text = items[1]["content"]
        .as_array()
        .and_then(|parts| parts.iter().filter_map(|p| p["text"].as_str()).next())
        .unwrap_or("");
    assert_eq!(assistant_text, "resumed output");
    let has_tool_call = items
        .iter()
        .any(|i| i["type"].as_str() == Some("tool_call"));
    assert!(!has_tool_call, "no orphan tool_call in history");
}

/// Paused with an orphan `tool_use` in history + a fresh `Method::Run`
/// must produce a wire-valid next LLM request: the orphan is closed
/// with a synthetic `tool_result`, a system note is inserted, and the
/// new user input is appended.
#[tokio::test]
async fn paused_then_run_closes_orphan_tool_use_for_next_request() {
    // Response 1: emit a tool_use block (complete with stop) targeting
    // our hanging tool. The Worker commits the ToolCall to history,
    // then parks inside `execute_tools` waiting on the tool — which is
    // where Method::Pause catches it.
    let tool_name = "HangyTool";
    let first = MockResponse::Complete(vec![
        LlmEvent::tool_use_start(0, "call_orphan", tool_name),
        LlmEvent::tool_input_delta(0, "{}"),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    // Response 2: ordinary completion after the Paused→Run transition.
    let second = MockResponse::Complete(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "ok"),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockClient::sequential(vec![first, second]);
    let client_for_assert = client.clone();
    let mut pod = make_pod(client).await;
    pod.worker_mut()
        .register_tool(hanging_tool_definition(tool_name));
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Run {
            input: "first".into(),
        })
        .await
        .unwrap();

    // Wait for ToolCallDone — the ToolCall is committed to history
    // right before the Worker enters tool execution and pends.
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::ToolCallDone { .. }
        ))
        .await,
        "tool_call_done should arrive before pause"
    );

    handle.send(Method::Pause).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::Paused
            }
        ))
        .await,
        "expected RunEnd::Paused"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Paused);

    // New user input while Paused → controller routes to
    // `Pod::interrupt_and_run`, which closes the orphan + injects a
    // system note before the fresh user message.
    handle
        .send(Method::Run {
            input: "new request".into(),
        })
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::Finished
            }
        ))
        .await,
        "expected RunEnd::Finished after Paused→Run"
    );

    // The second LLM request carries the closure chain. Walk its items
    // and assert the invariants — order matters for wire correctness.
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 2, "two LLM calls expected");
    let items = &requests[1].items;

    // Find the ToolCall and ensure the immediately-subsequent
    // ToolResult (if any) carries the synthetic summary.
    let mut saw_synthetic_tool_result = false;
    let mut saw_interruption_note = false;
    let mut saw_new_user = false;
    for item in items {
        match item {
            llm_worker::Item::ToolResult {
                call_id, summary, ..
            } if call_id == "call_orphan" => {
                assert_eq!(summary, "[Interrupted by user]");
                saw_synthetic_tool_result = true;
            }
            llm_worker::Item::Message { role, content, .. }
                if *role == llm_worker::Role::System =>
            {
                let text: String = content.iter().map(|p| p.as_text()).collect();
                if text.contains("interrupted by the user") {
                    saw_interruption_note = true;
                }
            }
            llm_worker::Item::Message { role, content, .. }
                if *role == llm_worker::Role::User =>
            {
                let text: String = content.iter().map(|p| p.as_text()).collect();
                if text.contains("new request") {
                    saw_new_user = true;
                }
            }
            _ => {}
        }
    }
    assert!(
        saw_synthetic_tool_result,
        "synthetic tool_result for orphan missing in 2nd request items: {items:?}"
    );
    assert!(
        saw_interruption_note,
        "system interruption note missing in 2nd request items: {items:?}"
    );
    assert!(
        saw_new_user,
        "new user message missing in 2nd request items: {items:?}"
    );

    // Also confirm the closure chain is ordered: tool_result for the
    // orphan precedes the system note, which precedes the new user
    // message.
    let idx = |pred: &dyn Fn(&llm_worker::Item) -> bool| {
        items.iter().position(pred).unwrap()
    };
    let tool_result_idx = idx(&|i| matches!(i, llm_worker::Item::ToolResult { call_id, .. } if call_id == "call_orphan"));
    let sys_idx = idx(&|i| match i {
        llm_worker::Item::Message {
            role: llm_worker::Role::System,
            content,
            ..
        } => content
            .iter()
            .map(|p| p.as_text())
            .collect::<String>()
            .contains("interrupted by the user"),
        _ => false,
    });
    let user_idx = idx(&|i| match i {
        llm_worker::Item::Message {
            role: llm_worker::Role::User,
            content,
            ..
        } => content
            .iter()
            .map(|p| p.as_text())
            .collect::<String>()
            .contains("new request"),
        _ => false,
    });
    assert!(tool_result_idx < sys_idx, "tool_result must precede system note");
    assert!(sys_idx < user_idx, "system note must precede new user message");
}

