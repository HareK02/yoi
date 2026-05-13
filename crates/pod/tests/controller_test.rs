use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::types::Item;
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use session_store::{FsStore, LogEntry};

use pod::{Event, Method, Pod, PodController, PodHandle, PodManifest, PodStatus};

/// Reconstruct a worker-history-like `Vec<Item>` from the live session
/// log mirror held by the Pod's broadcast sink. Replaces the previous
/// `PodSharedState.history()` test helper now that the mirror lives in
/// the sink.
fn history_from_sink(handle: &PodHandle) -> Vec<Item> {
    let (entries, _rx) = handle.sink.subscribe_with_snapshot();
    let mut items = Vec::new();
    for entry in entries {
        match entry {
            LogEntry::SessionStart { history, .. } => {
                items.extend(history.into_iter().map(Item::from));
            }
            LogEntry::UserInput { segments, .. } => {
                let text = protocol::Segment::flatten_to_text(&segments);
                items.push(Item::user_message(text));
            }
            LogEntry::AssistantItems { items: i, .. }
            | LogEntry::ToolResults { items: i, .. }
            | LogEntry::HookInjectedItems { items: i, .. } => {
                items.extend(i.into_iter().map(Item::from));
            }
            _ => {}
        }
    }
    items
}

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
    make_pod_with_pwd(client).await.0
}

async fn make_pod_with_pwd(client: MockClient) -> (Pod<MockClient, FsStore>, std::path::PathBuf) {
    make_pod_with_pwd_and_manifest(client, MANIFEST_TOML).await
}

async fn make_pod_with_pwd_and_manifest(
    client: MockClient,
    manifest_toml: &str,
) -> (Pod<MockClient, FsStore>, std::path::PathBuf) {
    let manifest = PodManifest::from_toml(manifest_toml).unwrap();
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
    let pod = Pod::new(manifest, worker, store, pwd.clone(), scope)
        .await
        .unwrap();
    (pod, pwd)
}

async fn spawn_controller(pod: Pod<MockClient, FsStore>) -> PodHandle {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_base = tmp.path().to_owned();
    std::mem::forget(tmp);
    let (handle, _shutdown_rx) = PodController::spawn(pod, &runtime_base).await.unwrap();
    handle
}

async fn wait_for_status(handle: &PodHandle, status: PodStatus) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if handle.shared_state.get_status() == status {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for status {status:?}; current={:?}",
            handle.shared_state.get_status()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_end_returns_to_idle_without_busy_status() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("Hello")).await.unwrap();

    let mut saw_run_end = false;
    let mut saw_idle_status = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::RunEnd { result: protocol::RunResult::Finished }) => {
                        saw_run_end = true;
                    }
                    Ok(Event::Status { status: PodStatus::Idle }) if saw_run_end => {
                        saw_idle_status = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_run_end, "expected RunEnd::Finished");
    assert!(
        saw_idle_status,
        "expected idle status immediately after RunEnd"
    );
    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);
}

/// Mid-turn re-attach: a client connecting while the worker is still
/// running observes the in-flight `UserInput` entry in the connect-time
/// `Event::Snapshot`. This is the load-bearing property of the new
/// session-log-driven IPC: a late attacher reconstructs the running
/// view without needing the prior client's diff.
#[tokio::test]
async fn snapshot_includes_user_input_for_in_flight_turn() {
    let client = MockClient::sequential(vec![MockResponse::Hang(simple_text_events())]);
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    handle
        .send(Method::run_text("hello in-flight"))
        .await
        .unwrap();
    wait_for_status(&handle, PodStatus::Running).await;

    let stream = tokio::net::UnixStream::connect(handle.runtime_dir.socket_path())
        .await
        .unwrap();
    let (reader, _writer) = stream.into_split();
    let mut reader = protocol::stream::JsonLineReader::new(reader);

    loop {
        let event = reader.next::<Event>().await.unwrap().unwrap();
        match event {
            Event::Snapshot { entries, .. } => {
                // Walk the entries, find a `LogEntry::UserInput` and
                // confirm its segments flatten to our submitted text.
                let mut found = false;
                for value in entries {
                    let entry: session_store::LogEntry =
                        serde_json::from_value(value).expect("LogEntry deserialise");
                    if let session_store::LogEntry::UserInput { segments, .. } = entry {
                        let text = protocol::Segment::flatten_to_text(&segments);
                        if text == "hello in-flight" {
                            found = true;
                            break;
                        }
                    }
                }
                assert!(
                    found,
                    "snapshot must carry the in-flight UserInput entry"
                );
                return;
            }
            Event::Alert(_) => continue,
            other => panic!("expected Snapshot first, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn attach_snapshot_includes_current_status() {
    let client = MockClient::sequential(vec![MockResponse::Hang(simple_text_events())]);
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    handle.send(Method::run_text("Hello")).await.unwrap();
    wait_for_status(&handle, PodStatus::Running).await;

    let stream = tokio::net::UnixStream::connect(handle.runtime_dir.socket_path())
        .await
        .unwrap();
    let (reader, _writer) = stream.into_split();
    let mut reader = protocol::stream::JsonLineReader::new(reader);

    // First event after connect is the snapshot — it carries the current status.
    loop {
        let event = reader.next::<Event>().await.unwrap().unwrap();
        match event {
            Event::Snapshot { status, .. } => {
                assert_eq!(status, PodStatus::Running);
                return;
            }
            Event::Alert(_) => continue,
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

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

    handle.send(Method::run_text("Hello")).await.unwrap();

    // Wait for the run to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);
}

#[tokio::test]
async fn run_populates_history() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    handle.send(Method::run_text("Hello")).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let history = history_from_sink(&handle);
    assert!(
        history.len() >= 2,
        "history must include user + assistant items, got {history:?}"
    );
}

#[tokio::test]
async fn events_are_broadcast() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("Hello")).await.unwrap();

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
    handle.send(Method::run_text("first")).await.unwrap();

    // Immediately send second run (should get error)
    handle.send(Method::run_text("second")).await.unwrap();

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
async fn run_with_paste_segment_inlines_content_and_emits_typed_user_message() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    // Mixed input: plain text + a paste chip + trailing text. Pod must
    // flatten this into one user-message string (paste content inlined,
    // no `[Clipboard ...]` label leaking to the LLM); the
    // `Event::UserMessage` re-broadcast must carry the typed segments
    // unchanged so other clients can re-render the chip.
    let segments = vec![
        protocol::Segment::text("see "),
        protocol::Segment::Paste {
            id: 7,
            chars: 11,
            lines: 2,
            content: "line1\nline2".into(),
        },
        protocol::Segment::text(" thanks"),
    ];
    handle
        .send(Method::Run {
            input: segments.clone(),
        })
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut user_event_segments: Option<Vec<protocol::Segment>> = None;
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(Event::UserMessage { segments }) => user_event_segments = Some(segments),
                Ok(Event::TurnEnd { .. }) => break,
                Err(_) => break,
                _ => {}
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    let echoed = user_event_segments.expect("UserMessage event missing");
    assert_eq!(echoed.len(), 3, "all three segments must round-trip");
    assert!(matches!(echoed[1], protocol::Segment::Paste { id: 7, .. }));

    // The Worker received a single user message whose text is the
    // flattened body — paste content inlined, no chip label.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 1, "one LLM call expected");
    let user_text = requests[0]
        .items
        .iter()
        .find_map(|i| i.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        user_text.contains("see line1\nline2 thanks"),
        "got: {user_text:?}"
    );
    assert!(
        !user_text.contains("[Clipboard"),
        "label must not leak: {user_text:?}"
    );
}

#[tokio::test]
async fn run_with_resolvable_file_ref_attaches_system_message_after_user() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let (pod, pwd) = make_pod_with_pwd(client).await;
    std::fs::write(pwd.join("notes.md"), "alpha\nbeta\n").unwrap();
    let handle = spawn_controller(pod).await;

    let segments = vec![
        protocol::Segment::text("see "),
        protocol::Segment::FileRef {
            path: "notes.md".into(),
        },
    ];
    handle.send(Method::Run { input: segments }).await.unwrap();

    // Wait for the turn to complete.
    let mut rx = handle.subscribe();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(Event::TurnEnd { .. }) => break,
                Err(_) => break,
                _ => {}
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let requests = client_for_assert.captured_requests();
    let items = &requests[0].items;
    // The submit produces 2 history items: user message then file content.
    let user_idx = items
        .iter()
        .position(|i| i.is_user_message())
        .expect("user message present");
    let next = items
        .get(user_idx + 1)
        .expect("attachment item present after user");
    let next_text = next.as_text().unwrap_or_default();
    assert!(
        next_text.contains("[File: notes.md]"),
        "expected file header, got: {next_text:?}"
    );
    assert!(
        next_text.contains("alpha"),
        "expected file body, got: {next_text:?}"
    );
}

#[tokio::test]
async fn run_with_file_ref_uses_manifest_file_upload_limit() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let manifest_toml = format!("{MANIFEST_TOML}\n[worker.file_upload]\nmax_bytes = 5\n");
    let (pod, pwd) = make_pod_with_pwd_and_manifest(client, &manifest_toml).await;
    std::fs::write(pwd.join("long.txt"), "abcdefghij").unwrap();
    let handle = spawn_controller(pod).await;

    handle
        .send(Method::Run {
            input: vec![protocol::Segment::FileRef {
                path: "long.txt".into(),
            }],
        })
        .await
        .unwrap();

    let mut rx = handle.subscribe();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(Event::TurnEnd { .. }) => break,
                Err(_) => break,
                _ => {}
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let requests = client_for_assert.captured_requests();
    let attachment = requests[0]
        .items
        .iter()
        .find_map(|i| {
            let text = i.as_text()?;
            text.contains("[File: long.txt]").then_some(text)
        })
        .expect("file attachment present");
    assert!(attachment.contains("abcde"), "got: {attachment:?}");
    assert!(!attachment.contains("abcdef"), "got: {attachment:?}");
    assert!(
        attachment.contains("truncated, 10 bytes total"),
        "got: {attachment:?}"
    );
}

#[tokio::test]
async fn run_with_unresolved_segment_emits_alert_and_placeholder() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    let segments = vec![
        protocol::Segment::text("look at "),
        protocol::Segment::FileRef {
            path: "src/lib.rs".into(),
        },
    ];
    handle.send(Method::Run { input: segments }).await.unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut saw_alert_for_file_ref = false;
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(Event::Alert(a)) if a.message.contains("file ref @src/lib.rs") => {
                    saw_alert_for_file_ref = true;
                }
                Ok(Event::TurnEnd { .. }) => break,
                Err(_) => break,
                _ => {}
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(
        saw_alert_for_file_ref,
        "an Alert mentioning the unresolved file ref must be emitted"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let requests = client_for_assert.captured_requests();
    let user_text = requests[0]
        .items
        .iter()
        .find_map(|i| i.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    // The user message keeps the literal `@<path>` token (matching what
    // the user typed). Resolution failure surfaces via the Alert above;
    // the LLM still sees the intent as a sigil-prefixed reference.
    assert!(
        user_text.contains("@src/lib.rs"),
        "literal sigil missing, got: {user_text:?}"
    );
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
    let mut saw_notify_echo = false;
    let mut saw_turn_end = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Notify { ref message }) if message == "turn finished" => {
                        saw_notify_echo = true;
                    }
                    Ok(Event::TurnEnd { .. }) => { saw_turn_end = true; break; }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(
        saw_notify_echo,
        "Method::Notify on idle Pod should be echoed as Event::Notify"
    );
    assert!(saw_turn_end, "auto-triggered turn should complete");
    // Status flips back to Idle on the controller thread after RunEnd.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);

    // Exactly one request was made; it must contain the formatted
    // notification as one of the items (committed to history by
    // PodInterceptor::pending_history_appends and cloned into the
    // request context for that turn).
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 1, "one LLM call expected");
    let notify_in_request = requests[0].items.iter().any(|i| {
        i.as_text()
            .is_some_and(|t| t.contains("[Notification]") && t.contains("turn finished"))
    });
    assert!(
        notify_in_request,
        "injected system message missing from request, got items: {:?}",
        requests[0]
            .items
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );

    // The notification must also be persisted into the Worker history
    // (and therefore eventually into history.json), per
    // tickets/notify-history-persist.md.
    let history = history_from_sink(&handle);
    let notify_in_history = history.iter().any(|i| {
        i.as_text()
            .is_some_and(|t| t.contains("[Notification]") && t.contains("turn finished"))
    });
    assert!(
        notify_in_history,
        "notify must be committed to worker.history, got items: {:?}",
        history
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn pod_event_turn_ended_while_idle_auto_starts_turn_and_injects_system_message() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::PodEvent(protocol::PodEvent::TurnEnded {
            pod_name: "child".into(),
        }))
        .await
        .unwrap();

    let mut saw_pod_event_echo = false;
    let mut saw_turn_end = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::PodEvent(protocol::PodEvent::TurnEnded { ref pod_name }))
                        if pod_name == "child" =>
                    {
                        saw_pod_event_echo = true;
                    }
                    Ok(Event::TurnEnd { .. }) => { saw_turn_end = true; break; }
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(
        saw_pod_event_echo,
        "Method::PodEvent on idle Pod should be echoed as Event::PodEvent"
    );
    assert!(
        saw_turn_end,
        "PodEvent::TurnEnded on idle Pod should auto-start a turn"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.get_status(), PodStatus::Idle);

    let requests = client_for_assert.captured_requests();
    assert_eq!(
        requests.len(),
        1,
        "auto-kick should issue exactly one LLM request"
    );
    let event_in_request = requests[0].items.iter().any(|i| {
        i.as_text().is_some_and(|t| {
            t.contains("[Notification]") && t.contains("child") && t.contains("finished a turn")
        })
    });
    assert!(
        event_in_request,
        "rendered TurnEnded text missing from request, got items: {:?}",
        requests[0]
            .items
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );

    // Same item must be present in worker.history (persisted lane),
    // not just the per-request clone — see tickets/notify-history-persist.md.
    let history = history_from_sink(&handle);
    let event_in_history = history.iter().any(|i| {
        i.as_text().is_some_and(|t| {
            t.contains("[Notification]") && t.contains("child") && t.contains("finished a turn")
        })
    });
    assert!(
        event_in_history,
        "PodEvent must be committed to worker.history, got items: {:?}",
        history
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn notify_while_running_does_not_emit_already_running_error() {
    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("start")).await.unwrap();
    handle
        .send(Method::Notify {
            message: "ping".into(),
        })
        .await
        .unwrap();

    // Drain events until the run ends; AlreadyRunning must never appear.
    // The in-flight branch must still echo the Notify as a log element.
    let mut saw_notify_echo = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == pod::ErrorCode::AlreadyRunning => {
                        panic!("Notify while running must not produce AlreadyRunning");
                    }
                    Ok(Event::Notify { ref message }) if message == "ping" => {
                        saw_notify_echo = true;
                    }
                    Ok(Event::TurnEnd { .. }) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(
        saw_notify_echo,
        "in-flight Notify must still be echoed as Event::Notify"
    );
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
    writer.write(&Method::run_text("Hello")).await.unwrap();

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
async fn socket_pod_event_turn_ended_while_idle_auto_starts_turn() {
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    writer
        .write(&Method::PodEvent(protocol::PodEvent::TurnEnded {
            pod_name: "child".into(),
        }))
        .await
        .unwrap();

    let mut saw_pod_event_echo = false;
    let mut saw_turn_start = false;
    let mut saw_turn_end = false;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::PodEvent(protocol::PodEvent::TurnEnded { pod_name })))
                        if pod_name == "child" =>
                    {
                        saw_pod_event_echo = true;
                    }
                    Ok(Some(Event::TurnStart { .. })) => saw_turn_start = true,
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

    assert!(
        saw_pod_event_echo,
        "PodEvent::TurnEnded via socket should be echoed as Event::PodEvent"
    );
    assert!(
        saw_turn_start,
        "PodEvent::TurnEnded via socket should auto-start a turn"
    );
    assert!(
        saw_turn_end,
        "auto-triggered turn should reach turn_end via socket"
    );
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

    handle.send(Method::run_text("hello")).await.unwrap();

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
    let history = history_from_sink(&handle);
    let roles: Vec<&str> = history
        .iter()
        .filter_map(|i| match i {
            Item::Message { role, .. } => match role {
                llm_worker::Role::User => Some("user"),
                llm_worker::Role::Assistant => Some("assistant"),
                llm_worker::Role::System => Some("system"),
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant"],
        "history = user + assistant only; got {history:?}"
    );
    let assistant_text = history
        .iter()
        .find_map(|i| match i {
            Item::Message {
                role: llm_worker::Role::Assistant,
                content,
                ..
            } => Some(
                content
                    .iter()
                    .map(|p: &llm_worker::ContentPart| p.as_text().to_owned())
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(assistant_text, "resumed output");
    let has_tool_call = history.iter().any(|i| i.is_tool_call());
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

    handle.send(Method::run_text("first")).await.unwrap();

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
    handle.send(Method::run_text("new request")).await.unwrap();
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
            llm_worker::Item::Message { role, content, .. } if *role == llm_worker::Role::User => {
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
    let idx = |pred: &dyn Fn(&llm_worker::Item) -> bool| items.iter().position(pred).unwrap();
    let tool_result_idx = idx(
        &|i| matches!(i, llm_worker::Item::ToolResult { call_id, .. } if call_id == "call_orphan"),
    );
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
    assert!(
        tool_result_idx < sys_idx,
        "tool_result must precede system note"
    );
    assert!(
        sys_idx < user_idx,
        "system note must precede new user message"
    );
}
