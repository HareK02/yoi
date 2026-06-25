use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_engine::Engine;
use llm_engine::llm_client::event::{ErrorEvent, Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_engine::llm_client::types::Item;
use llm_engine::llm_client::{ClientError, LlmClient, Request};
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use pod_store::{CombinedStore, FsWorkerStore};
use session_store::{FsStore, LogEntry};

use worker::{Event, Method, Worker, WorkerController, WorkerHandle, WorkerManifest, WorkerStatus};

type TestStore = CombinedStore<FsStore, FsWorkerStore>;

/// Reconstruct a worker-history-like `Vec<Item>` from the live session
/// log mirror held by the Worker's broadcast sink. Replaces the previous
/// `WorkerSharedState.history()` test helper now that the mirror lives in
/// the sink.
fn history_from_sink(handle: &WorkerHandle) -> Vec<Item> {
    let (entries, _rx) = handle.sink.subscribe_with_snapshot();
    let mut items = Vec::new();
    for entry in entries {
        match entry {
            LogEntry::SegmentStart { history, .. } => {
                items.extend(history.into_iter().map(Item::from));
            }
            LogEntry::UserInput { segments, .. } => {
                let text = protocol::Segment::flatten_to_text(&segments);
                items.push(Item::user_message(text));
            }
            LogEntry::AssistantItem { item, .. } | LogEntry::ToolResult { item, .. } => {
                items.push(Item::from(item));
            }
            LogEntry::SystemItem { item, .. } => {
                items.push(item.to_history_item());
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
    /// Emit the events and then pend forever so the Engine blocks on
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
                retry_after: None,
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
[worker]
name = "test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[[scope.allow]]
target = "./"
permission = "write"
"#;

async fn make_worker(client: MockClient) -> Worker<MockClient, TestStore> {
    make_worker_with_pwd(client).await.0
}

async fn make_worker_with_pwd(
    client: MockClient,
) -> (Worker<MockClient, TestStore>, std::path::PathBuf) {
    make_worker_with_pwd_and_manifest(client, MANIFEST_TOML).await
}

async fn make_worker_with_pwd_and_manifest(
    client: MockClient,
    manifest_toml: &str,
) -> (Worker<MockClient, TestStore>, std::path::PathBuf) {
    let manifest = WorkerManifest::from_toml(manifest_toml).unwrap();
    let store_tmp = tempfile::tempdir().unwrap();
    let store = CombinedStore::new(
        FsStore::new(store_tmp.path()).unwrap(),
        FsWorkerStore::new(store_tmp.path().join("pods")).unwrap(),
    );
    std::mem::forget(store_tmp);

    // Separate tempdir to serve as the Worker's pwd/scope — these tests
    // exercise the controller via a mock client and never touch the
    // filesystem through tools, so a throwaway writable dir is enough.
    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = manifest::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let worker = Engine::new(client);
    let worker = Worker::new(manifest, worker, store, pwd.clone(), scope)
        .await
        .unwrap();
    (worker, pwd)
}

async fn spawn_controller(worker: Worker<MockClient, TestStore>) -> WorkerHandle {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_base = tmp.path().to_owned();
    std::mem::forget(tmp);
    let (handle, _shutdown_rx) = WorkerController::spawn(worker, &runtime_base)
        .await
        .unwrap();
    handle
}

async fn wait_for_status(handle: &WorkerHandle, status: WorkerStatus) {
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

fn request_tool_names(request: &Request) -> Vec<String> {
    let mut names = request
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn wait_for_captured_request(client: &MockClient) -> Request {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let requests = client.captured_requests();
        if let Some(request) = requests.into_iter().next() {
            return request;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for captured LLM request"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn feature_flags_default_to_core_tool_surface_only() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle.send(Method::run_text("Hello")).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert_eq!(
        names,
        vec![
            "ActiveWorkflowCancel",
            "ActiveWorkflowComplete",
            "ActiveWorkflowList",
            "Bash",
            "Edit",
            "Glob",
            "Grep",
            "Read",
            "Write"
        ]
    );
    assert!(!names.iter().any(|name| name == "TaskCreate"));
    assert!(!names.iter().any(|name| name == "WebSearch"));
    assert!(!names.iter().any(|name| name == "SpawnWorker"));
}

#[tokio::test]
async fn enabled_task_and_web_features_register_their_tools() {
    let manifest = r#"
[worker]
name = "feature-test-worker"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.task]
enabled = true

[feature.web]
enabled = true

[web]
enabled = false

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker_with_pwd_and_manifest(client, manifest).await.0;
    let handle = spawn_controller(worker).await;

    handle.send(Method::run_text("Hello")).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert!(names.iter().any(|name| name == "TaskCreate"));
    assert!(names.iter().any(|name| name == "TaskUpdate"));
    assert!(names.iter().any(|name| name == "WebSearch"));
    assert!(names.iter().any(|name| name == "WebFetch"));
    assert!(!names.iter().any(|name| name == "SpawnWorker"));
    assert!(!names.iter().any(|name| name == "MemoryRead"));
}

#[tokio::test]
async fn project_role_tool_surfaces_keep_task_disabled_and_workers_role_scoped() {
    struct Case {
        role: &'static str,
        workers_enabled: bool,
    }

    let cases = [
        Case {
            role: "orchestrator",
            workers_enabled: true,
        },
        Case {
            role: "coder",
            workers_enabled: false,
        },
        Case {
            role: "intake",
            workers_enabled: false,
        },
        Case {
            role: "reviewer",
            workers_enabled: false,
        },
        Case {
            role: "companion",
            workers_enabled: false,
        },
    ];

    for case in cases {
        let delegation = if case.workers_enabled {
            r#"
[[delegation_scope.allow]]
target = "/tmp"
permission = "write"
"#
        } else {
            ""
        };
        let manifest = format!(
            r#"
[worker]
name = "role-surface-{role}"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.task]
enabled = false

[feature.workers]
enabled = {workers_enabled}

[[scope.allow]]
target = "./"
permission = "write"
{delegation}
"#,
            role = case.role,
            workers_enabled = case.workers_enabled,
            delegation = delegation,
        );
        let client = MockClient::new(simple_text_events());
        let client_for_assert = client.clone();
        let worker = make_worker_with_pwd_and_manifest(client, &manifest).await.0;
        let handle = spawn_controller(worker).await;

        handle.send(Method::run_text("Hello")).await.unwrap();
        wait_for_status(&handle, WorkerStatus::Idle).await;

        let request = wait_for_captured_request(&client_for_assert).await;
        let names = request_tool_names(&request);
        assert!(
            !names.iter().any(|name| name == "TaskCreate"),
            "{} role must not expose Task tools: {names:?}",
            case.role
        );
        assert_eq!(
            names.iter().any(|name| name == "SpawnWorker"),
            case.workers_enabled,
            "{} role Worker tool exposure mismatch: {names:?}",
            case.role
        );
    }
}

#[tokio::test]
async fn workers_feature_requires_delegation_scope() {
    let manifest = r#"
[worker]
name = "worker-management-feature-test"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.workers]
enabled = true

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(simple_text_events());
    let worker = make_worker_with_pwd_and_manifest(client, manifest).await.0;
    let tmp = tempfile::tempdir().unwrap();
    let result = WorkerController::spawn(worker, tmp.path()).await;
    assert!(result.is_err());
    let message = result.err().unwrap().to_string();
    assert!(
        message.contains("[feature.workers].enabled = true requires non-empty"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn run_end_returns_to_idle_without_busy_status() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
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
                    Ok(Event::Status { status: WorkerStatus::Idle }) if saw_run_end => {
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
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);
}

#[tokio::test]
async fn provider_stream_error_records_run_errored() {
    let client = MockClient::new(vec![LlmEvent::Error(ErrorEvent {
        code: Some("context_length_exceeded".into()),
        message: "request too large".into(),
    })]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("ping")).await.unwrap();

    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::Error {
                code: protocol::ErrorCode::ProviderError,
                message,
            } if message.contains("context_length_exceeded")
        ))
        .await,
        "provider stream error should be surfaced as a live provider error"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let (entries, _rx) = handle.sink.subscribe_with_snapshot();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            LogEntry::RunErrored { message, .. }
                if message.contains("context_length_exceeded")
        )),
        "provider stream error should be persisted as RunErrored"
    );
    assert!(
        !entries.iter().any(|entry| matches!(
            entry,
            LogEntry::RunCompleted {
                result: llm_engine::EngineResult::Finished,
                ..
            }
        )),
        "provider stream error must not be recorded as a finished run"
    );
}

/// Mid-turn re-attach: a client connecting while the worker is still
/// running observes the in-flight `UserInput` entry in the connect-time
/// `Event::Snapshot`. This is the load-bearing property of the new
/// session-log-driven IPC: a late attacher reconstructs the running
/// view without needing the prior client's diff.
#[tokio::test]
async fn snapshot_includes_user_input_for_in_flight_turn() {
    let client = MockClient::sequential(vec![MockResponse::Hang(simple_text_events())]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::run_text("hello in-flight"))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;

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
                assert!(found, "snapshot must carry the in-flight UserInput entry");
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle.send(Method::run_text("Hello")).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;

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
                assert_eq!(status, WorkerStatus::Running);
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);
}

#[tokio::test]
async fn run_updates_shared_state_to_idle_after_completion() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle.send(Method::run_text("Hello")).await.unwrap();

    // Wait for the run to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);
}

#[tokio::test]
async fn run_populates_history() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
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
    // Keep the first turn in-flight until the test drops the handle. A
    // finite stream can finish before the second Method reaches the
    // controller in the full test suite, making this assertion racy.
    let events = vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "slow..."),
    ];
    let client = MockClient::sequential(vec![MockResponse::Hang(events)]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    // Send first run and wait until the controller has entered Running.
    handle.send(Method::run_text("first")).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;

    // Now the second run must be rejected by drive_turn's live Method arm.
    handle.send(Method::run_text("second")).await.unwrap();

    // Look for the error event
    let mut saw_already_running = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) => {
                        if code == worker::ErrorCode::AlreadyRunning {
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::Resume).await.unwrap();

    let mut saw_not_paused = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == worker::ErrorCode::NotPaused => {
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::Cancel).await.unwrap();

    let mut saw_not_running = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == worker::ErrorCode::NotRunning => {
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let (_snapshot, mut entry_rx) = handle.sink.subscribe_with_snapshot();
    let mut event_rx = handle.subscribe();

    // Mixed input: plain text + a paste chip + trailing text. Worker must
    // flatten this into one user-message string (paste content inlined,
    // no `[Clipboard ...]` label leaking to the LLM); the committed
    // `LogEntry::UserInput` must carry the typed segments unchanged so
    // socket clients can derive `Event::UserMessage` and re-render the chip.
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
    let mut saw_turn_end = false;
    let mut user_input_segments: Option<Vec<protocol::Segment>> = None;
    loop {
        tokio::select! {
            event = event_rx.recv() => match event {
                Ok(Event::TurnEnd { .. }) => {
                    saw_turn_end = true;
                    if user_input_segments.is_some() {
                        break;
                    }
                }
                Err(_) => break,
                _ => {}
            },
            entry = entry_rx.recv() => match entry {
                Ok(session_store::LogEntry::UserInput { segments, .. }) => {
                    user_input_segments = Some(segments);
                    if saw_turn_end {
                        break;
                    }
                }
                Err(_) => break,
                _ => {}
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    assert!(saw_turn_end, "TurnEnd event missing");
    let echoed = user_input_segments.expect("committed UserInput entry missing");
    assert_eq!(echoed, segments, "typed segments must round-trip unchanged");

    // The Engine received a single user message whose text is the
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
    let (worker, pwd) = make_worker_with_pwd(client).await;
    std::fs::write(pwd.join("notes.md"), "alpha\nbeta\n").unwrap();
    let handle = spawn_controller(worker).await;

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
    let manifest_toml = format!("{MANIFEST_TOML}\n[engine.file_upload]\nmax_bytes = 5\n");
    let (worker, pwd) = make_worker_with_pwd_and_manifest(client, &manifest_toml).await;
    std::fs::write(pwd.join("long.txt"), "abcdefghij").unwrap();
    let handle = spawn_controller(worker).await;

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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Notify {
            message: "turn finished".into(),
            auto_run: true,
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
    // Wait for the post-run persist_turn (Flush + TurnEnd + RunCompleted
    // commits) to finish; the controller flips status to Idle right
    // after that.
    wait_for_status(&handle, WorkerStatus::Idle).await;
    // The live echo arrives via the sink's `Event::SystemItem` lane,
    // not on the `event_tx` broadcast that `handle.subscribe()` taps.
    // Verify the notification landed on the sink mirror instead.
    let (entries, _) = handle.sink.subscribe_with_snapshot();
    let saw_notify_in_mirror = entries.iter().any(|e| {
        matches!(
            e,
            session_store::LogEntry::SystemItem {
                item: session_store::SystemItem::Notification { message, .. },
                ..
            } if message == "turn finished"
        )
    });
    assert!(
        saw_notify_in_mirror,
        "Method::Notify should commit a SystemItem::Notification entry; mirror = {entries:?}"
    );

    // Exactly one request was made; it must contain the formatted
    // notification as one of the items (committed to history by
    // WorkerInterceptor::pending_history_appends and cloned into the
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

    // The notification must also be persisted into the Engine history
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
async fn notify_while_idle_with_auto_run_false_waits_for_explicit_run() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::Notify {
            message: "progress snapshot".into(),
            auto_run: false,
        })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);
    assert!(
        client_for_assert.captured_requests().is_empty(),
        "weak Notify must not stage RunForNotification while idle"
    );

    handle.send(Method::run_text("continue")).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if !client_for_assert.captured_requests().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "explicit run did not reach the mock LLM"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let requests = client_for_assert.captured_requests();
    assert_eq!(
        requests.len(),
        1,
        "explicit run should drain the queued notification"
    );
    let notify_in_request = requests[0].items.iter().any(|i| {
        i.as_text()
            .is_some_and(|t| t.contains("[Notification]") && t.contains("progress snapshot"))
    });
    assert!(
        notify_in_request,
        "queued weak notification must be history-backed on the next explicit run; got items: {:?}",
        requests[0]
            .items
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn worker_event_turn_ended_while_idle_auto_starts_turn_and_injects_system_message() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::WorkerEvent(protocol::WorkerEvent::TurnEnded {
            worker_name: "child".into(),
        }))
        .await
        .unwrap();

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
    assert!(
        saw_turn_end,
        "WorkerEvent::TurnEnded on idle Worker should auto-start a turn"
    );
    // Wait for the post-run persist_turn to complete before reading the
    // mirror — TurnEnd fires inside the worker loop, persist_turn (and
    // its Flush of the drain queue) runs afterwards.
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let (entries, _) = handle.sink.subscribe_with_snapshot();
    let saw_worker_event_in_mirror = entries.iter().any(|e| {
        matches!(
            e,
            session_store::LogEntry::SystemItem {
                item: session_store::SystemItem::WorkerEvent {
                    event: protocol::WorkerEvent::TurnEnded { worker_name },
                    ..
                },
                ..
            } if worker_name == "child"
        )
    });
    assert!(
        saw_worker_event_in_mirror,
        "Method::WorkerEvent should commit a SystemItem::WorkerEvent entry"
    );
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);

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
        "WorkerEvent must be committed to worker.history, got items: {:?}",
        history
            .iter()
            .filter_map(|i| i.as_text())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn worker_event_scope_sub_delegated_while_idle_stays_control_plane_only() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::WorkerEvent(
            protocol::WorkerEvent::ScopeSubDelegated {
                parent_worker: "child".into(),
                sub_worker: "grandchild".into(),
                sub_socket: "/tmp/grandchild.sock".into(),
                scope: vec![],
            },
        ))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        handle.shared_state.get_status(),
        WorkerStatus::Idle,
        "control-plane ScopeSubDelegated must not auto-start the parent LLM"
    );
    assert!(
        client_for_assert.captured_requests().is_empty(),
        "ScopeSubDelegated must not issue an LLM request"
    );

    let (entries, _) = handle.sink.subscribe_with_snapshot();
    let saw_scope_event_in_mirror = entries.iter().any(|entry| {
        matches!(
            entry,
            session_store::LogEntry::SystemItem {
                item: session_store::SystemItem::WorkerEvent {
                    event: protocol::WorkerEvent::ScopeSubDelegated { .. },
                    ..
                },
                ..
            }
        )
    });
    assert!(
        !saw_scope_event_in_mirror,
        "ScopeSubDelegated must not create an agent-visible SystemItem::WorkerEvent; mirror = {entries:?}"
    );
}

#[tokio::test]
async fn notify_while_running_does_not_emit_already_running_error() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("start")).await.unwrap();
    handle
        .send(Method::Notify {
            message: "ping".into(),
            auto_run: true,
        })
        .await
        .unwrap();

    // Drain events until the run ends; AlreadyRunning must never appear.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::Error { code, .. }) if code == worker::ErrorCode::AlreadyRunning => {
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
    // The core property of this test is "no AlreadyRunning error fires
    // when Notify arrives mid-run". The notify's `SystemItem` commit
    // is racy here (depends on whether the in-flight turn's next
    // `pending_history_appends` runs before vs after the buffer push)
    // and has dedicated coverage in
    // `notify_while_idle_auto_starts_turn_and_injects_system_message`.
    wait_for_status(&handle, WorkerStatus::Idle).await;
}

#[tokio::test]
async fn status_json_reflects_worker_name() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    let json = handle.shared_state.status_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["worker_name"], "test-worker");
}

// ---------------------------------------------------------------------------
// Socket transport tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn socket_run_receives_events() {
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

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
async fn socket_worker_event_turn_ended_while_idle_auto_starts_turn() {
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    writer
        .write(&Method::WorkerEvent(protocol::WorkerEvent::TurnEnded {
            worker_name: "child".into(),
        }))
        .await
        .unwrap();

    let mut saw_worker_event_echo = false;
    let mut saw_turn_start = false;
    let mut saw_turn_end = false;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    // The SystemItem and TurnEnd events arrive through independent
    // broadcast lanes (sink fan-out vs `event_tx`), so their relative
    // order on the wire is non-deterministic. Keep reading until both
    // are observed (or the deadline trips), rather than breaking on
    // the first TurnEnd.
    loop {
        if saw_worker_event_echo && saw_turn_end {
            break;
        }
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::SystemItem { ref item }))
                        if item.get("kind").and_then(|k| k.as_str()) == Some("worker_event")
                            && item
                                .pointer("/event/worker_name")
                                .and_then(|v| v.as_str()) == Some("child") =>
                    {
                        saw_worker_event_echo = true;
                    }
                    Ok(Some(Event::TurnStart { .. })) => saw_turn_start = true,
                    Ok(Some(Event::TurnEnd { .. })) => {
                        saw_turn_end = true;
                    }
                    Ok(None) | Err(_) => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(
        saw_worker_event_echo,
        "WorkerEvent::TurnEnded via socket should be echoed as Event::SystemItem(WorkerEvent)"
    );
    assert!(
        saw_turn_start,
        "WorkerEvent::TurnEnded via socket should auto-start a turn"
    );
    assert!(
        saw_turn_end,
        "auto-triggered turn should reach turn_end via socket"
    );
}

async fn socket_error_after_method_line(
    handle: &WorkerHandle,
    line: &[u8],
) -> (worker::ErrorCode, String) {
    use protocol::stream::JsonLineReader;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);

    writer.write_all(line).await.unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::Error { code, message })) => return (code, message),
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("socket closed before invalid-method error"),
                    Err(e) => panic!("socket read failed before invalid-method error: {e}"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                panic!("timed out waiting for invalid-method error")
            }
        }
    }
}

#[tokio::test]
async fn socket_schema_invalid_method_returns_error() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (code, message) = socket_error_after_method_line(&handle, b"{\"bad\":\"json\"}\n").await;

    assert_eq!(code, worker::ErrorCode::InvalidRequest);
    assert!(
        message.contains("invalid method"),
        "expected invalid-method diagnostic, got: {message}"
    );
}

#[tokio::test]
async fn socket_malformed_method_returns_error() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (code, message) = socket_error_after_method_line(&handle, b"{not-json}\n").await;

    assert_eq!(code, worker::ErrorCode::InvalidRequest);
    assert!(
        message.contains("invalid method"),
        "expected invalid-method diagnostic, got: {message}"
    );
}

#[tokio::test]
async fn socket_peer_close_without_method_does_not_broadcast_error() {
    use protocol::stream::JsonLineReader;
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut broadcast_rx = handle.subscribe();
    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = reader.next::<Event>() => {
                match event {
                    Ok(Some(Event::Snapshot { .. })) => break,
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("socket closed before connect-time snapshot"),
                    Err(e) => panic!("socket read failed before connect-time snapshot: {e}"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                panic!("timed out waiting for connect-time snapshot")
            }
        }
    }

    drop(writer);
    drop(reader);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        tokio::select! {
            event = broadcast_rx.recv() => {
                match event {
                    Ok(Event::Error { code, message }) => {
                        panic!("peer close without Method broadcast error {code:?}: {message}")
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        panic!("broadcast receiver lagged while checking peer close: {n}")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
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
    async fn execute(
        &self,
        _input: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
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
    // so the Engine is parked inside the stream read and `cancel_rx`
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
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
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
    // EngineError::Cancelled is translated under pause_requested.
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
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Paused);

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
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Idle);

    // History consistency: exactly [user "hello", assistant
    // "resumed output"]. No artifacts from the aborted stream
    // (partial text is not committed), no orphan tool_use.
    let history = history_from_sink(&handle);
    let roles: Vec<&str> = history
        .iter()
        .filter_map(|i| match i {
            Item::Message { role, .. } => match role {
                llm_engine::Role::User => Some("user"),
                llm_engine::Role::Assistant => Some("assistant"),
                llm_engine::Role::System => Some("system"),
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
                role: llm_engine::Role::Assistant,
                content,
                ..
            } => Some(
                content
                    .iter()
                    .map(|p: &llm_engine::ContentPart| p.as_text().to_owned())
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
    // our hanging tool. The Engine commits the ToolCall to history,
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
    let mut worker = make_worker(client).await;
    worker
        .engine_mut()
        .register_tool(hanging_tool_definition(tool_name));
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("first")).await.unwrap();

    // Wait for ToolCallDone — the ToolCall is committed to history
    // right before the Engine enters tool execution and pends.
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
    assert_eq!(handle.shared_state.get_status(), WorkerStatus::Paused);

    // New user input while Paused → `Worker::run` observes
    // `last_run_interrupted` and runs its interrupt-prep step, which
    // closes the orphan + injects a system note before the fresh user
    // message.
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
            llm_engine::Item::ToolResult {
                call_id, summary, ..
            } if call_id == "call_orphan" => {
                assert_eq!(summary, "[Interrupted by user]");
                saw_synthetic_tool_result = true;
            }
            llm_engine::Item::Message { role, content, .. }
                if *role == llm_engine::Role::System =>
            {
                let text: String = content.iter().map(|p| p.as_text()).collect();
                if text.contains("interrupted by the user") {
                    saw_interruption_note = true;
                }
            }
            llm_engine::Item::Message { role, content, .. } if *role == llm_engine::Role::User => {
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
    let idx = |pred: &dyn Fn(&llm_engine::Item) -> bool| items.iter().position(pred).unwrap();
    let tool_result_idx = idx(
        &|i| matches!(i, llm_engine::Item::ToolResult { call_id, .. } if call_id == "call_orphan"),
    );
    let sys_idx = idx(&|i| match i {
        llm_engine::Item::Message {
            role: llm_engine::Role::System,
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
        llm_engine::Item::Message {
            role: llm_engine::Role::User,
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

#[tokio::test]
async fn paused_cancel_abandons_resume_and_next_input_is_fresh_run() {
    let tool_name = "HangyTool";
    let first = MockResponse::Complete(vec![
        LlmEvent::tool_use_start(0, "call_cancelled", tool_name),
        LlmEvent::tool_input_delta(0, "{}"),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let second = MockResponse::Complete(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "fresh output"),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockClient::sequential(vec![first, second]);
    let client_for_assert = client.clone();
    let mut worker = make_worker(client).await;
    worker
        .engine_mut()
        .register_tool(hanging_tool_definition(tool_name));
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("first")).await.unwrap();
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
    wait_for_status(&handle, WorkerStatus::Paused).await;

    handle.send(Method::Cancel).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let (entries_after_cancel, _rx_after_cancel) = handle.sink.subscribe_with_snapshot();
    assert!(
        entries_after_cancel
            .iter()
            .any(|entry| matches!(entry, LogEntry::PausedTurnAbandoned { .. })),
        "paused cancel should have an explicit lifecycle log entry: {entries_after_cancel:?}"
    );
    assert!(
        !entries_after_cancel.iter().any(|entry| matches!(
            entry,
            LogEntry::RunCompleted {
                result: llm_engine::EngineResult::Finished,
                interrupted: false,
                ..
            }
        )),
        "paused cancel must not be logged as a normal finished run: {entries_after_cancel:?}"
    );
    assert_eq!(
        client_for_assert.captured_requests().len(),
        1,
        "paused cancel must not resume or start another LLM request"
    );

    handle.send(Method::Resume).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::Error {
                code: worker::ErrorCode::NotPaused,
                ..
            }
        ))
        .await,
        "resume after paused cancel should be rejected as not paused"
    );
    assert_eq!(
        client_for_assert.captured_requests().len(),
        1,
        "rejected resume must not call the LLM"
    );

    handle
        .send(Method::run_text("fresh request"))
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
        "expected RunEnd::Finished for fresh run"
    );

    let requests = client_for_assert.captured_requests();
    assert_eq!(
        requests.len(),
        2,
        "fresh input should start exactly one new LLM request"
    );
    let items = &requests[1].items;
    assert!(
        items.iter().any(|item| matches!(
            item,
            llm_engine::Item::ToolResult { call_id, summary, .. }
                if call_id == "call_cancelled" && summary == "[Interrupted by user]"
        )),
        "paused cancel should close orphan tool_use before future requests: {items:?}"
    );
    assert!(
        items.iter().any(|item| matches!(
            item,
            llm_engine::Item::Message {
                role: llm_engine::Role::System,
                ..
            } if item_text_contains(item, "interrupted by the user")
        )),
        "paused cancel should record an explicit interruption note: {items:?}"
    );
    assert!(
        items.iter().any(|item| matches!(
            item,
            llm_engine::Item::Message {
                role: llm_engine::Role::User,
                ..
            } if item_text_contains(item, "fresh request")
        )),
        "fresh user input should be part of the next normal run: {items:?}"
    );
}

fn item_text_contains(item: &Item, needle: &str) -> bool {
    item.as_text().unwrap_or_default().contains(needle)
}

async fn snapshot_contains_user_input(handle: &WorkerHandle, needle: &str) -> bool {
    let stream = tokio::net::UnixStream::connect(handle.runtime_dir.socket_path())
        .await
        .unwrap();
    let (reader, _writer) = stream.into_split();
    let mut reader = protocol::stream::JsonLineReader::new(reader);

    loop {
        let event = reader.next::<Event>().await.unwrap().unwrap();
        match event {
            Event::Snapshot { entries, .. } => {
                return entries.into_iter().any(|value| {
                    let entry: session_store::LogEntry =
                        serde_json::from_value(value).expect("LogEntry deserialise");
                    match entry {
                        session_store::LogEntry::UserInput { segments, .. } => {
                            protocol::Segment::flatten_to_text(&segments).contains(needle)
                        }
                        _ => false,
                    }
                });
            }
            Event::Alert(_) => continue,
            other => panic!("expected Snapshot first, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn empty_turn_cancel_rolls_back_submit_entries_and_emits_signal() {
    let client = MockClient::sequential(vec![MockResponse::Hang(vec![])]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("rollback me")).await.unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle.send(Method::Cancel).await.unwrap();

    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::RolledBack
            }
        ))
        .await,
        "expected RunEnd::RolledBack after empty cancel"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let history = history_from_sink(&handle);
    assert!(
        !history
            .iter()
            .any(|item| item_text_contains(item, "rollback me")),
        "rolled-back user input must not remain in history: {history:?}"
    );
}

#[tokio::test]
async fn empty_turn_pause_rolls_back_and_snapshot_does_not_restore_input() {
    let client = MockClient::sequential(vec![MockResponse::Hang(vec![])]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::run_text("pause rollback"))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle.send(Method::Pause).await.unwrap();

    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::RolledBack
            }
        ))
        .await,
        "expected RunEnd::RolledBack after empty pause"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    assert!(
        !snapshot_contains_user_input(&handle, "pause rollback").await,
        "attach snapshot must not resurrect rolled-back empty turn input"
    );
}

#[tokio::test]
async fn empty_turn_rollback_removes_only_the_most_recent_turn() {
    let client = MockClient::sequential(vec![
        MockResponse::Complete(simple_text_events()),
        MockResponse::Hang(vec![]),
    ]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle.send(Method::run_text("first kept")).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::Finished
            }
        ))
        .await,
        "expected first run to finish"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    handle
        .send(Method::run_text("second rolled back"))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle.send(Method::Cancel).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::RunEnd {
                result: protocol::RunResult::RolledBack
            }
        ))
        .await,
        "expected empty second run to roll back"
    );

    let history = history_from_sink(&handle);
    assert!(
        history
            .iter()
            .any(|item| item_text_contains(item, "first kept"))
    );
    assert!(
        history
            .iter()
            .any(|item| item_text_contains(item, "Hello World"))
    );
    assert!(
        !history
            .iter()
            .any(|item| item_text_contains(item, "second rolled back")),
        "rollback must affect only the most recent empty turn: {history:?}"
    );
}

#[tokio::test]
async fn pause_after_assistant_token_does_not_rollback() {
    let client = MockClient::sequential(vec![MockResponse::Hang(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "committed before pause"),
        LlmEvent::text_block_stop(0, None),
    ])]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::run_text("keep this turn"))
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::TextDone { .. }
        ))
        .await,
        "assistant token should be visible before pause"
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
        "pause after assistant output must keep the existing Paused path"
    );
    wait_for_status(&handle, WorkerStatus::Paused).await;

    let history = history_from_sink(&handle);
    assert!(
        history
            .iter()
            .any(|item| item_text_contains(item, "keep this turn")),
        "token-visible turn must keep its UserInput entry: {history:?}"
    );
}
