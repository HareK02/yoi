use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use llm_worker::Worker;
use llm_worker_persistence::FsStore;

use pod::{
    Event, Method, Pod, PodController, PodManifest, PodStatus,
};

// ---------------------------------------------------------------------------
// Mock LLM Client
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockClient {
    responses: Arc<Vec<Vec<LlmEvent>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockClient {
    fn new(events: Vec<LlmEvent>) -> Self {
        Self {
            responses: Arc::new(vec![events]),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for MockClient {
    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Api {
                status: Some(500),
                code: Some("mock".into()),
                message: "No more responses".into(),
            });
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
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

[provider]
kind = "anthropic"
model = "test-model"

[worker]
max_tokens = 100
"#;

async fn make_pod(client: MockClient) -> Pod<MockClient, FsStore> {
    let manifest = PodManifest::from_toml(MANIFEST_TOML).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(tmp.path()).await.unwrap();
    // Leak tempdir to keep it alive
    std::mem::forget(tmp);
    let worker = Worker::new(client);
    Pod::new(manifest, worker, store, None).await.unwrap()
}

use pod::PodHandle;

async fn spawn_controller(pod: Pod<MockClient, FsStore>) -> PodHandle {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_base = tmp.path().to_owned();
    // Leak tempdir so it survives the test
    std::mem::forget(tmp);
    PodController::spawn(pod, &runtime_base).await.unwrap()
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
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    // Give the socket server a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send run method via socket
    writer
        .write_all(b"{\"method\":\"run\",\"params\":{\"input\":\"Hello\"}}\n")
        .await
        .unwrap();

    // Collect events
    let mut saw_turn_start = false;
    let mut saw_text_delta = false;
    let mut saw_turn_end = false;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
                        match parsed["event"].as_str() {
                            Some("turn_start") => saw_turn_start = true,
                            Some("text_delta") => saw_text_delta = true,
                            Some("turn_end") => {
                                saw_turn_end = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ => break,
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
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let client = MockClient::new(simple_text_events());
    let pod = make_pod(client).await;
    let handle = spawn_controller(pod).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let sock_path = handle.runtime_dir.socket_path();
    let stream = UnixStream::connect(&sock_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send garbage
    writer.write_all(b"{\"bad\":\"json\"}\n").await.unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut saw_error = false;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
                        if parsed["event"] == "error" {
                            saw_error = true;
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(saw_error, "should see error for invalid method");
}
