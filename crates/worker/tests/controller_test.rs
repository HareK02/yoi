use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agen::Engine;
use agen::llm_client::event::{
    ErrorEvent, Event as LlmEvent, ResponseStatus, StatusEvent, UsageEvent,
};
use agen::llm_client::types::Item;
use agen::llm_client::{ClientError, LlmClient, Request};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use manifest::{ProfileRegistrySource, ProfileResolveOptions, ProfileResolver, ProfileSelector};
use session_store::{CombinedStore, FsWorkerStore};
use session_store::{FsStore, LogEntry};
use workdir::{
    CommandOutputRequest, CommandRequest, LocalWorkdirSession, Workdir, WorkdirError,
    WorkdirSessionCapabilities, WorkdirSessionHandle,
};

use worker::{
    Event, Method, Worker, WorkerController, WorkerFilesystemAuthority, WorkerHandle,
    WorkerManifest, WorkerStatus, WorkerWorkspaceContext, WorkspaceClient, WorkspaceClientError,
    WorkspaceRequest, WorkspaceResponse,
};

type TestStore = CombinedStore<FsStore, FsWorkerStore>;

static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(1);

fn worker_command(_handle: &WorkerHandle) -> protocol::WorkerCommandEnvelope {
    protocol::WorkerCommandEnvelope::new(NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed))
}

/// Reconstruct a worker-history-like `Vec<Item>` from the live session
/// log mirror held by the Worker's broadcast sink. Replaces the previous
/// `WorkerSharedState.history()` test helper now that the mirror lives in
/// the sink.
fn history_from_sink(handle: &WorkerHandle) -> Vec<Item> {
    let (entries, _rx) = handle.sink.subscribe_with_snapshot();
    let mut items = Vec::new();
    for entry in entries {
        match entry {
            LogEntry::AnnotatedSegmentStart { history, .. } => {
                items.extend(history.into_iter().map(|entry| Item::from(entry.item)));
            }
            LogEntry::AnnotatedUserInput { history, .. } => {
                items.extend(history.into_iter().map(|entry| Item::from(entry.item)));
            }
            LogEntry::AnnotatedAssistantItem { entry, .. }
            | LogEntry::AnnotatedToolResult { entry, .. } => {
                items.push(Item::from(entry.item));
            }
            LogEntry::AnnotatedSystemItem { entry, .. } => {
                items.push(entry.item.to_history_item());
            }
            _ => {}
        }
    }
    items
}

fn system_item(entry: &LogEntry) -> Option<&session_store::SystemItem> {
    match entry {
        LogEntry::AnnotatedSystemItem { entry, .. } => Some(&entry.item),
        _ => None,
    }
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

fn bash_tool_events(call_id: &str, command: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::tool_use_start(0, call_id, "Bash"),
        LlmEvent::tool_input_delta(
            0,
            serde_json::json!({
                "command": command,
                "cwd": ".",
            })
            .to_string(),
        ),
        LlmEvent::tool_use_stop(0),
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

#[derive(Debug)]
struct AvailableWorkspaceClient;

impl WorkspaceClient for AvailableWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-1")
    }

    fn kind(&self) -> &str {
        "test"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable("test".to_string()))
    }
}

#[derive(Debug)]
struct NoopWorkspaceClient;

impl WorkspaceClient for NoopWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-test")
    }

    fn kind(&self) -> &str {
        "test-noop"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(
            "test client does not execute requests".to_string(),
        ))
    }
}

async fn make_worker_with_pwd_and_manifest(
    client: MockClient,
    manifest_toml: &str,
) -> (Worker<MockClient, TestStore>, std::path::PathBuf) {
    make_worker_with_pwd_manifest_and_workspace_context(
        client,
        manifest_toml,
        WorkerWorkspaceContext::local_filesystem(None),
    )
    .await
}

async fn make_worker_with_pwd_manifest_and_workspace_context(
    client: MockClient,
    manifest_toml: &str,
    workspace_context: WorkerWorkspaceContext,
) -> (Worker<MockClient, TestStore>, std::path::PathBuf) {
    let manifest = WorkerManifest::from_toml(manifest_toml).unwrap();
    make_worker_with_manifest_and_workspace_context(client, manifest, workspace_context).await
}

async fn make_worker_with_manifest_and_workspace_context(
    client: MockClient,
    manifest: WorkerManifest,
    workspace_context: WorkerWorkspaceContext,
) -> (Worker<MockClient, TestStore>, std::path::PathBuf) {
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

    let worker =
        Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(client);
    let authority = WorkerFilesystemAuthority::local(pwd.clone(), pwd.clone());
    let worker = Worker::new(manifest, worker, store, workspace_context, authority, scope)
        .await
        .unwrap();
    (worker, pwd)
}

async fn spawn_controller(worker: Worker<MockClient, TestStore>) -> WorkerHandle {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_base = tmp.path().to_owned();
    std::mem::forget(tmp);
    let bash_output_dir = runtime_base.join("bash-output");
    let (handle, _shutdown_rx) = WorkerController::spawn(worker, &runtime_base, &bash_output_dir)
        .await
        .unwrap();
    handle
}

#[tokio::test]
async fn canonical_worker_handle_uploads_and_submits_attachment() {
    let root = tempfile::tempdir().unwrap();
    let manifest = WorkerManifest::from_toml(MANIFEST_TOML).unwrap();
    let session = session_store::WorkerSessionStore::new(root.path().join("session")).unwrap();
    let store = CombinedStore::new(
        session.clone(),
        session_store::WorkerAggregateStore::new(root.path(), manifest.worker.name.clone())
            .unwrap(),
    );
    let client = MockClient::new(simple_text_events());
    let engine = Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
        client.clone(),
    );
    let worker = Worker::new(
        manifest,
        engine,
        store,
        WorkerWorkspaceContext::local_filesystem(None),
        WorkerFilesystemAuthority::local(root.path().to_owned(), root.path().to_owned()),
        manifest::Scope::writable(root.path()).unwrap(),
    )
    .await
    .unwrap();
    let (handle, shutdown_rx) = WorkerController::spawn(
        worker,
        &root.path().join("runtime"),
        &root.path().join("bash-output"),
    )
    .await
    .unwrap();
    let mut rx = handle.subscribe();
    let file = handle
        .upload_file_with_context(
            "notes.txt",
            "text/plain",
            b"attached context",
            &session_store::UploadedFileUploadContext {
                upload_id: "upload-1".into(),
                principal_id: "account-1".into(),
                workspace_id: "workspace-1".into(),
                runtime_id: "runtime-1".into(),
                worker_id: "worker-1".into(),
            },
        )
        .unwrap();
    handle
        .send(Method::Submit {
            submission_request_id: "attachment-submit".into(),
            input: vec![protocol::Segment::UploadedFile { file: file.clone() }],
        })
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
            matches!(
                event,
                Event::RunEnd {
                    result: protocol::RunResult::Finished
                }
            )
        })
        .await
    );
    let requests = client.captured_requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].items.iter().any(|item| {
        item.as_text()
            .is_some_and(|text| text.contains("notes.txt") && text.contains(&file.artifact_id))
    }));
    assert_eq!(handle.delete_uncommitted_uploaded_files().unwrap(), 0);
    use session_store::Store;
    let (bound, content) = session
        .read_uploaded_file_by_id(session.session_id().unwrap().unwrap(), &file.artifact_id)
        .unwrap();
    assert!(bound.source_entry_id.is_some());
    assert_eq!(content, b"attached context");
    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), shutdown_rx)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn controller_grants_read_scope_for_exact_bash_output_directory() {
    let worker = make_worker(MockClient::new(simple_text_events())).await;
    let shared_scope = worker.scope().clone();
    let runtime_base = tempfile::tempdir().unwrap();
    let worker_tmp = tempfile::tempdir().unwrap();
    let bash_output_dir = worker_tmp.path().join("worker-1").join("bash-output");

    let (handle, shutdown_rx) =
        WorkerController::spawn(worker, runtime_base.path(), &bash_output_dir)
            .await
            .unwrap();

    assert!(bash_output_dir.is_dir());
    assert!(shared_scope.snapshot().allow_rules().iter().any(|rule| {
        rule.target == bash_output_dir
            && rule.permission == manifest::Permission::Read
            && rule.recursive
    }));
    assert!(!handle.runtime_dir.path().join("bash-output").exists());

    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    shutdown_rx.await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_bound_workdir_session() {
    let (mut worker, pwd) = make_worker_with_pwd(MockClient::new(simple_text_events())).await;
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new("controller-test-workdir"),
        pwd.clone(),
        pwd,
        worker.scope().clone(),
        WorkdirSessionCapabilities::ALL,
    ));
    let command = session
        .start_command(CommandRequest {
            command: "sleep 30".to_owned(),
            timeout_secs: 60,
            output_limit: 1024,
            cwd: workdir::WorkdirPath::root(),
            spill_dir: None,
            tool_call_id: None,
        })
        .await
        .unwrap();
    worker.bind_single_workdir_session(Some(Arc::clone(&session)));

    let runtime_base = tempfile::tempdir().unwrap();
    let bash_output_dir = runtime_base.path().join("bash-output");
    let (handle, shutdown_rx) =
        WorkerController::spawn(worker, runtime_base.path(), &bash_output_dir)
            .await
            .unwrap();
    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown_rx)
        .await
        .expect("controller should shut down")
        .expect("controller shutdown signal should remain open");

    assert!(matches!(
        session.command_status(command).await,
        Err(WorkdirError::Unavailable(_))
    ));
}

#[tokio::test]
async fn controller_filters_workdir_command_not_owned_by_worker() {
    let (mut worker, pwd) = make_worker_with_pwd(MockClient::new(simple_text_events())).await;
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new("controller-foreign-command-workdir"),
        pwd.clone(),
        pwd,
        worker.scope().clone(),
        WorkdirSessionCapabilities::ALL,
    ));
    worker.bind_single_workdir_session(Some(Arc::clone(&session)));
    let handle = spawn_controller(worker).await;
    let mut events = handle.subscribe();

    let command = session
        .start_command(CommandRequest {
            command: "sleep 0.2".to_owned(),
            timeout_secs: 5,
            output_limit: 1024,
            cwd: workdir::WorkdirPath::root(),
            spill_dir: None,
            tool_call_id: Some("foreign-tool-call".into()),
        })
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv())
            .await
            .is_err(),
        "a command without a Worker-owned tool call must not enter its protocol stream"
    );
    let Event::Snapshot { in_flight, .. } = handle.snapshot_event() else {
        panic!("worker snapshot expected");
    };
    assert!(in_flight.commands.is_empty());

    let output = session
        .command_output(CommandOutputRequest {
            handle: command,
            cursor: 0,
            limit: 1024,
            wait: true,
        })
        .await
        .unwrap();
    assert_eq!(output.status, workdir::CommandStatus::Completed);
    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn controller_projects_workdir_command_events_and_snapshot_state() {
    let client = MockClient::sequential(vec![
        MockResponse::Complete(bash_tool_events(
            "tool-command-1",
            "printf ready; sleep 0.3; printf done",
        )),
        MockResponse::Complete(simple_text_events()),
    ]);
    let (mut worker, pwd) = make_worker_with_pwd(client).await;
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new("controller-command-observation-workdir"),
        pwd.clone(),
        pwd,
        worker.scope().clone(),
        WorkdirSessionCapabilities::ALL,
    ));
    worker.bind_single_workdir_session(Some(Arc::clone(&session)));
    let handle = spawn_controller(worker).await;
    let mut events = handle.subscribe();
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "run the command",
        ))
        .await
        .unwrap();

    let mut command_id = None;
    let mut saw_output = false;
    while !saw_output {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("command event should arrive")
            .unwrap();
        match event {
            Event::Command {
                event:
                    protocol::CommandEvent::Started {
                        command_id: started_id,
                        tool_call_id,
                        ..
                    },
            } => {
                assert_eq!(tool_call_id.as_deref(), Some("tool-command-1"));
                command_id = Some(started_id);
            }
            Event::Command {
                event:
                    protocol::CommandEvent::Output {
                        command_id: output_id,
                        stream: protocol::CommandStream::Stdout,
                        content,
                        ..
                    },
            } if command_id.as_deref() == Some(output_id.as_str()) && content.contains("ready") => {
                saw_output = true;
            }
            _ => {}
        }
    }
    let command_id = command_id.expect("owned command start should be projected");

    let Event::Snapshot { in_flight, .. } = handle.snapshot_event() else {
        panic!("worker snapshot expected");
    };
    assert_eq!(in_flight.commands.len(), 1);
    assert_eq!(in_flight.commands[0].command_id, command_id);
    assert_eq!(in_flight.commands[0].stdout.content, "ready");
    assert_eq!(
        in_flight.commands[0].status,
        protocol::CommandStatus::Running
    );

    let saw_terminal = drain_until(&mut events, std::time::Duration::from_secs(2), |event| {
        matches!(
            event,
            Event::Command {
                event: protocol::CommandEvent::Terminal {
                    command_id: terminal_id,
                    status: protocol::CommandStatus::Completed,
                    exit_code: Some(0),
                    ..
                }
            } if terminal_id == &command_id
        )
    })
    .await;
    assert!(saw_terminal, "completed command event should arrive");

    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn controller_refreshes_command_snapshot_after_high_output_provider_lag() {
    let command_text = "dd if=/dev/zero bs=8192 count=300 2>/dev/null | tr '\\0' x; sleep 5";
    let client = MockClient::sequential(vec![
        MockResponse::Complete(bash_tool_events("tool-high-output", command_text)),
        MockResponse::Complete(simple_text_events()),
    ]);
    let (mut worker, pwd) = make_worker_with_pwd(client).await;
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new("controller-command-lag-recovery-workdir"),
        pwd.clone(),
        pwd,
        worker.scope().clone(),
        WorkdirSessionCapabilities::ALL,
    ));
    worker.bind_single_workdir_session(Some(Arc::clone(&session)));
    let handle = spawn_controller(worker).await;
    let mut events = handle.subscribe();
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "run the high-output command",
        ))
        .await
        .unwrap();

    let command_id = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("owned command start should arrive")
            .unwrap();
        if let Event::Command {
            event:
                protocol::CommandEvent::Started {
                    command_id,
                    tool_call_id: Some(tool_call_id),
                    ..
                },
        } = event
            && tool_call_id == "tool-high-output"
        {
            break command_id;
        }
    };

    // Local command telemetry uses 8 KiB chunks and a 256-event channel. One
    // synchronous file-poll burst with 300 chunks deterministically makes the
    // worker-side receiver observe `Lagged` before this command terminates.
    let expected_end_offset = 300_u64 * 8192;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let recovered = loop {
        let Event::Snapshot { in_flight, .. } = handle.snapshot_event() else {
            panic!("worker snapshot expected");
        };
        if let Some(snapshot) = in_flight
            .commands
            .iter()
            .find(|snapshot| snapshot.command_id == command_id)
            && snapshot.stdout.end_offset >= expected_end_offset
        {
            break snapshot.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for lag recovery snapshot"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

    assert_eq!(recovered.tool_call_id.as_deref(), Some("tool-high-output"));
    assert_eq!(recovered.status, protocol::CommandStatus::Running);
    assert!(recovered.stdout.truncated);
    assert!(recovered.stdout.start_offset > 0);
    assert_eq!(recovered.stdout.end_offset, expected_end_offset);
    assert!(recovered.stdout.content.len() <= 32 * 1024);
    assert!(recovered.stdout.content.bytes().all(|byte| byte == b'x'));

    let command = workdir::CommandHandle(command_id);
    session.cancel_command(command).await.unwrap();
    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn controller_startup_failure_closes_bound_workdir_session() {
    let (mut worker, pwd) = make_worker_with_pwd(MockClient::new(simple_text_events())).await;
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
        Workdir::new("controller-startup-failure-workdir"),
        pwd.clone(),
        pwd,
        worker.scope().clone(),
        WorkdirSessionCapabilities::ALL,
    ));
    worker.bind_single_workdir_session(Some(Arc::clone(&session)));
    let runtime_base = tempfile::tempdir().unwrap();
    let invalid_runtime_base = runtime_base.path().join("not-a-directory");
    std::fs::write(&invalid_runtime_base, "file").unwrap();

    let bash_output_dir = runtime_base.path().join("bash-output");
    assert!(
        WorkerController::spawn(worker, &invalid_runtime_base, &bash_output_dir)
            .await
            .is_err()
    );
    assert!(matches!(
        session
            .start_command(CommandRequest {
                command: "printf unreachable".to_owned(),
                timeout_secs: 5,
                output_limit: 1024,
                cwd: workdir::WorkdirPath::root(),
                spill_dir: None,
                tool_call_id: None,
            })
            .await,
        Err(WorkdirError::Unavailable(_))
    ));
}

async fn wait_for_status(handle: &WorkerHandle, status: WorkerStatus) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if handle.shared_state.catalog_status() == status {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for status {status:?}; current={:?}",
            handle.shared_state.catalog_status()
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert_eq!(
        names,
        vec![
            "Bash",
            "Edit",
            "Glob",
            "Grep",
            "Read",
            "ReadInputArtifact",
            "SearchInputArtifact",
            "Write",
        ]
    );
    assert!(!names.iter().any(|name| name == "TaskCreate"));
    assert!(!names.iter().any(|name| name == "WebSearch"));
    assert!(!names.iter().any(|name| name == "SubWorkerSpawn"));
}

#[tokio::test]
async fn backend_job_capability_is_explicit_and_preserves_profile_features() {
    for bound in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let mut resolved = ProfileResolver::new()
            .with_workspace_base(workspace.path())
            .resolve(
                &ProfileSelector::source_named(ProfileRegistrySource::Builtin, "backend-job"),
                ProfileResolveOptions::with_worker_name("backend-job-worker"),
            )
            .unwrap();
        // A custom instruction and enabled feature must survive trusted Job binding.
        if bound {
            resolved.manifest.engine.instruction = "default".into();
            resolved.manifest.feature.task.enabled = true;
        }
        let instruction = resolved.manifest.engine.instruction.clone();
        let client = MockClient::new(simple_text_events());
        let client_for_assert = client.clone();
        let (mut worker, _pwd) = make_worker_with_manifest_and_workspace_context(
            client,
            resolved.manifest,
            WorkerWorkspaceContext::with_client(None, Arc::new(AvailableWorkspaceClient)),
        )
        .await;
        if bound {
            worker
                .bind_backend_job(worker::BackendJobExecutionBinding {
                    job_id: "job-1".into(),
                    attempt_id: "attempt-1".into(),
                    input_revision: Some("input-1".into()),
                    subjektiv_consolidation: false,
                })
                .unwrap();
        }
        assert_eq!(worker.manifest().engine.instruction, instruction);
        let handle = spawn_controller(worker).await;
        handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "Run the bounded Backend Job.",
            ))
            .await
            .unwrap();
        wait_for_status(&handle, WorkerStatus::Idle).await;
        let request = wait_for_captured_request(&client_for_assert).await;
        let names = request_tool_names(&request);
        assert_eq!(
            names.iter().any(|name| name == "SubmitBackendJobResult"),
            bound
        );
        assert_eq!(names.iter().any(|name| name == "TaskCreate"), bound);
    }
}

#[tokio::test]
async fn consolidation_job_preserves_custom_instruction_and_explicit_tool_policy() {
    let workspace = tempfile::tempdir().unwrap();
    let mut manifest = ProfileResolver::new()
        .with_workspace_base(workspace.path())
        .resolve(
            &ProfileSelector::source_named(
                ProfileRegistrySource::Builtin,
                "subjektiv-memory-consolidation",
            ),
            ProfileResolveOptions::with_worker_name("custom-consolidation-job"),
        )
        .unwrap()
        .manifest;
    manifest.engine.instruction = "default".into();
    manifest.feature.task.enabled = true;
    manifest
        .feature
        .subjektiv
        .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-1".into(),
            settings_revision: 1,
            language: "English".into(),
        })
        .unwrap();
    let client = MockClient::new(simple_text_events());
    let captured = client.clone();
    let (mut worker, _pwd) = make_worker_with_manifest_and_workspace_context(
        client,
        manifest,
        WorkerWorkspaceContext::with_client(None, Arc::new(AvailableWorkspaceClient)),
    )
    .await;
    worker
        .engine_mut()
        .set_system_prompt("chosen custom instruction");
    worker
        .bind_backend_job(worker::BackendJobExecutionBinding {
            job_id: "job-1".into(),
            attempt_id: "attempt-1".into(),
            input_revision: None,
            subjektiv_consolidation: true,
        })
        .unwrap();
    let handle = spawn_controller(worker).await;
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Inspect the bounded batch.",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let request = wait_for_captured_request(&captured).await;
    assert_eq!(
        request.system_prompt.as_deref(),
        Some("chosen custom instruction")
    );
    let tools = request_tool_names(&request);
    for tool in [
        "SubmitBackendJobResult",
        "MemoryApplyCandidate",
        "TaskCreate",
    ] {
        assert!(tools.iter().any(|name| name == tool));
    }
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert!(names.iter().any(|name| name == "TaskCreate"));
    assert!(names.iter().any(|name| name == "TaskUpdate"));
    assert!(names.iter().any(|name| name == "WebSearch"));
    assert!(names.iter().any(|name| name == "WebFetch"));
    assert!(!names.iter().any(|name| name == "SubWorkerSpawn"));
    assert!(!names.iter().any(|name| name == "MemoryRead"));
}

#[tokio::test]
async fn project_role_tool_surfaces_keep_task_disabled_and_workers_role_scoped() {
    struct Case {
        role: &'static str,
        sub_worker_enabled: bool,
    }

    let cases = [
        Case {
            role: "orchestrator",
            sub_worker_enabled: true,
        },
        Case {
            role: "coder",
            sub_worker_enabled: false,
        },
        Case {
            role: "intake",
            sub_worker_enabled: false,
        },
        Case {
            role: "reviewer",
            sub_worker_enabled: false,
        },
        Case {
            role: "companion",
            sub_worker_enabled: false,
        },
    ];

    for case in cases {
        let delegation = if case.sub_worker_enabled {
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

[feature.sub_worker]
enabled = {sub_worker_enabled}

[[scope.allow]]
target = "./"
permission = "write"
{delegation}
"#,
            role = case.role,
            sub_worker_enabled = case.sub_worker_enabled,
            delegation = delegation,
        );
        let client = MockClient::new(simple_text_events());
        let client_for_assert = client.clone();
        let worker = make_worker_with_pwd_and_manifest(client, &manifest).await.0;
        let handle = spawn_controller(worker).await;

        handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "Hello",
            ))
            .await
            .unwrap();
        wait_for_status(&handle, WorkerStatus::Idle).await;

        let request = wait_for_captured_request(&client_for_assert).await;
        let names = request_tool_names(&request);
        assert!(
            !names.iter().any(|name| name == "TaskCreate"),
            "{} role must not expose Task tools: {names:?}",
            case.role
        );
        assert_eq!(
            names.iter().any(|name| name == "SubWorkerSpawn"),
            case.sub_worker_enabled,
            "{} role SubWorker tool exposure mismatch: {names:?}",
            case.role
        );
        for control_tool in ["WorkerList", "WorkerSendInput", "WorkerStop"] {
            assert_eq!(
                names.iter().any(|name| name == control_tool),
                case.sub_worker_enabled,
                "{} role {control_tool} exposure mismatch: {names:?}",
                case.role
            );
        }
        for stale_alias in ["SubWorkerList", "SubWorkerSend", "SubWorkerStop"] {
            assert!(
                !names.iter().any(|name| name == stale_alias),
                "{} role exposed stale alias {stale_alias}: {names:?}",
                case.role
            );
        }
    }
}

#[tokio::test]
async fn builtin_orchestrator_exposes_worker_remove_and_workdir_delete() {
    let workspace = tempfile::tempdir().unwrap();
    let resolved = ProfileResolver::new()
        .with_workspace_base(workspace.path())
        .resolve(
            &ProfileSelector::source_named(ProfileRegistrySource::Builtin, "orchestrator"),
            ProfileResolveOptions::with_worker_name("orchestrator-worker"),
        )
        .unwrap();
    assert!(resolved.manifest.feature.subjektiv.enabled());
    assert!(!resolved.manifest.feature.subjektiv.execution_enabled());
    let workspace_context =
        WorkerWorkspaceContext::with_client(None, Arc::new(NoopWorkspaceClient));
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let (worker, _pwd) = make_worker_with_manifest_and_workspace_context(
        client,
        resolved.manifest,
        workspace_context,
    )
    .await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let request = wait_for_captured_request(&client_for_assert).await;
    let installed = request_tool_names(&request);

    assert!(installed.iter().any(|name| name == "WorkerRemove"));
    assert!(installed.iter().any(|name| name == "WorkdirDelete"));
    assert!(
        installed.iter().all(|name| !name.starts_with("Subjektiv")),
        "ordinary Orchestrator must not install subject-scoped tools: {installed:?}"
    );
}

#[tokio::test]
async fn builtin_coder_commits_without_unattached_subjektiv_lifecycle() {
    let workspace = tempfile::tempdir().unwrap();
    let resolved = ProfileResolver::new()
        .with_workspace_base(workspace.path())
        .resolve(
            &ProfileSelector::source_named(ProfileRegistrySource::Builtin, "coder"),
            ProfileResolveOptions::with_worker_name("coder-worker"),
        )
        .unwrap();
    assert!(resolved.manifest.feature.subjektiv.enabled());
    assert!(!resolved.manifest.feature.subjektiv.execution_enabled());
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let (worker, _pwd) = make_worker_with_manifest_and_workspace_context(
        client,
        resolved.manifest,
        WorkerWorkspaceContext::with_client(None, Arc::new(NoopWorkspaceClient)),
    )
    .await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let installed = request_tool_names(&wait_for_captured_request(&client_for_assert).await);
    assert!(
        installed.iter().all(|name| !name.starts_with("Subjektiv")),
        "ordinary Coder must not install subject-scoped tools: {installed:?}"
    );
}

#[tokio::test]
async fn worker_and_sub_worker_features_install_one_canonical_control_surface() {
    let manifest = r#"
[worker]
name = "combined-worker-control-feature-test"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.worker]
enabled = true
direct_spawn = false

[feature.sub_worker]
enabled = true

[[scope.allow]]
target = "./"
permission = "write"

[[delegation_scope.allow]]
target = "/tmp"
permission = "write"
"#;
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker_with_pwd_manifest_and_workspace_context(
        client,
        manifest,
        WorkerWorkspaceContext::with_client(None, Arc::new(NoopWorkspaceClient)),
    )
    .await
    .0;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert!(names.iter().any(|name| name == "SubWorkerSpawn"));
    assert!(!names.iter().any(|name| name == "WorkerSpawn"));
    for control_tool in ["WorkerList", "WorkerSendInput", "WorkerStop"] {
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == control_tool)
                .count(),
            1,
            "expected one {control_tool} contribution: {names:?}"
        );
    }
    for stale_alias in ["SubWorkerList", "SubWorkerSend", "SubWorkerStop"] {
        assert!(!names.iter().any(|name| name == stale_alias));
    }
}

#[tokio::test]
async fn workspace_worker_discovery_requires_workspace_authority_and_stays_separate_from_worker_list()
 {
    let manifest_toml = r#"
[worker]
name = "workspace-worker-discovery-test"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.workspace_worker_discovery]
enabled = true

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let (worker, _pwd) = make_worker_with_pwd_manifest_and_workspace_context(
        client,
        manifest_toml,
        WorkerWorkspaceContext::with_client(None, Arc::new(AvailableWorkspaceClient)),
    )
    .await;
    let handle = spawn_controller(worker).await;
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let request = wait_for_captured_request(&client_for_assert).await;
    let names = request_tool_names(&request);
    assert!(names.iter().any(|name| name == "ListWorkspaceWorkers"));
    assert!(!names.iter().any(|name| name == "WorkerList"));
}

#[tokio::test]
async fn sub_worker_feature_exposure_does_not_require_delegation_scope() {
    let manifest = r#"
[worker]
name = "worker-management-feature-test"
pwd = "./"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
max_tokens = 100

[feature.sub_worker]
enabled = true

[[scope.allow]]
target = "./"
permission = "write"
"#;
    let client = MockClient::new(simple_text_events());
    let worker = make_worker_with_pwd_and_manifest(client, manifest).await.0;
    let tmp = tempfile::tempdir().unwrap();
    let bash_output_dir = tmp.path().join("bash-output");
    let result = WorkerController::spawn(worker, tmp.path(), &bash_output_dir).await;
    assert!(
        result.is_ok(),
        "feature exposure must not imply delegation authority"
    );
}

#[tokio::test]
async fn started_submit_emits_one_durable_acceptance_receipt() {
    let worker = make_worker(MockClient::new(simple_text_events())).await;
    let handle = spawn_controller(worker).await;
    let mut events = handle.subscribe();
    let submission_request_id = protocol::new_submission_request_id();
    handle
        .send(Method::submit_text(submission_request_id.clone(), "start"))
        .await
        .unwrap();

    let mut receipts = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match events.recv().await.unwrap() {
                Event::SubmissionAccepted {
                    submission_request_id: received_request_id,
                    submission_id,
                    disposition,
                } if received_request_id == submission_request_id => {
                    receipts.push((submission_id, disposition));
                }
                Event::TurnEnd { .. } => break,
                _ => {}
            }
        }
    })
    .await
    .expect("submitted turn completes");

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].1, protocol::SubmissionDisposition::Started);
}

#[tokio::test]
async fn run_end_returns_to_idle_without_busy_status() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();
    let submission_request_id = protocol::new_submission_request_id();

    handle
        .send(Method::submit_text(submission_request_id.clone(), "Hello"))
        .await
        .unwrap();

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
                    Ok(Event::WorkerState { snapshot })
                        if saw_run_end
                            && snapshot.catalog_status() == WorkerStatus::Idle
                            && snapshot.last_finished_submission_request_id.as_deref()
                                == Some(submission_request_id.as_str()) =>
                    {
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
        "expected exact Submit teardown fence in idle status immediately after RunEnd"
    );
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "ping",
        ))
        .await
        .unwrap();

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
                result: agen::EngineResult::Finished,
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
    let mut events = handle.subscribe();

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "hello in-flight",
        ))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                events.recv().await,
                Ok(Event::WorkerState { snapshot }) if snapshot.catalog_status() == WorkerStatus::Running
            ) {
                break;
            }
        }
    })
    .await
    .expect("running status event");

    // The Running event is the in-flight visibility fence: the committed
    // annotated input must already be available to an immediately attaching
    // subscriber rather than racing behind this status transition.
    let stream = tokio::net::UnixStream::connect(handle.runtime_dir.socket_path())
        .await
        .unwrap();
    let (reader, _writer) = stream.into_split();
    let mut reader = protocol::stream::JsonLineReader::new(reader);

    loop {
        let event = reader.next::<Event>().await.unwrap().unwrap();
        match event {
            Event::Snapshot { session, .. } => {
                let found = session.entries.iter().any(|entry| match &entry.data {
                    protocol::SessionSnapshotEntryData::UserInput { segments } => {
                        protocol::Segment::flatten_to_text(segments) == "hello in-flight"
                    }
                    protocol::SessionSnapshotEntryData::Message {
                        role: protocol::SessionMessageRole::User,
                        content,
                    } => content.iter().any(|part| {
                        matches!(
                            part,
                            protocol::SessionContentPart::Text { text }
                                if text == "hello in-flight"
                        )
                    }),
                    _ => false,
                });
                assert!(
                    found,
                    "snapshot must carry the in-flight UserInput entry: {session:?}"
                );
                return;
            }
            Event::Alert(_) => continue,
            other => panic!("expected Snapshot first, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn reconnect_snapshot_restores_live_run_accounting_from_engine_events() {
    let client = MockClient::new(vec![
        LlmEvent::tool_use_start(0, "hold-call", "Hold"),
        LlmEvent::tool_input_delta(0, "{}"),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Usage(UsageEvent {
            input_tokens: Some(25_000),
            output_tokens: Some(300),
            cache_read_input_tokens: Some(20_000),
            ..Default::default()
        }),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let mut worker = make_worker(client).await;
    worker
        .engine_mut()
        .register_tool(hanging_tool_definition("Hold"));
    let handle = spawn_controller(worker).await;
    let mut events = handle.sink.subscribe_with_snapshot().1;
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "keep running",
        ))
        .await
        .unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let LogEntry::LlmUsage {
                ts,
                input_total_tokens,
                cache_read_tokens,
                output_tokens,
                ..
            } = events.recv().await.unwrap()
            {
                break (ts, input_total_tokens, cache_read_tokens, output_tokens);
            }
        }
    })
    .await
    .expect("live accounting");
    assert_eq!((observed.1, observed.2, observed.3), (25_000, 20_000, 300));
    for _ in 0..2 {
        let Event::Snapshot { session, .. } = handle.snapshot_event() else {
            panic!("expected snapshot")
        };
        let usage: Vec<_> = session
            .entries
            .iter()
            .filter(|e| matches!(e.data, protocol::SessionSnapshotEntryData::Usage { .. }))
            .collect();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].timestamp, observed.0);
        assert!(session.entries.iter().any(|e| matches!(
            e.data,
            protocol::SessionSnapshotEntryData::Invoke { .. }
        ) && e.timestamp > 0));
    }
}

#[tokio::test]
async fn attach_snapshot_includes_current_status() {
    let client = MockClient::sequential(vec![MockResponse::Hang(simple_text_events())]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();
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
            Event::Snapshot { state, .. } => {
                assert_eq!(state.catalog_status(), WorkerStatus::Running);
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

    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);
}

#[tokio::test]
async fn run_updates_shared_state_to_idle_after_completion() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();

    // Wait for the run to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);
}

#[tokio::test]
async fn run_populates_history() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
        .await
        .unwrap();

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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
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
async fn submit_while_running_or_paused_is_durably_queued() {
    async fn wait_for_rejection(
        rx: &mut tokio::sync::broadcast::Receiver<Event>,
        expected_request_id: &str,
    ) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Ok(Event::SubmissionRejected {
                    submission_request_id,
                    message,
                }) = rx.recv().await
                    && submission_request_id == expected_request_id
                {
                    break message;
                }
            }
        })
        .await
        .expect("Submit rejection")
    }

    async fn wait_for_acceptance(
        rx: &mut tokio::sync::broadcast::Receiver<Event>,
        expected_request_id: &str,
    ) -> protocol::SubmissionDisposition {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Ok(Event::SubmissionAccepted {
                    submission_request_id,
                    disposition,
                    ..
                }) = rx.recv().await
                    && submission_request_id == expected_request_id
                {
                    break disposition;
                }
            }
        })
        .await
        .expect("Submit acceptance")
    }

    let events = vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "slow..."),
    ];
    let client = MockClient::sequential(vec![
        MockResponse::Hang(events),
        MockResponse::Complete(simple_text_events()),
        MockResponse::Complete(simple_text_events()),
        MockResponse::Complete(simple_text_events()),
    ]);
    let captured = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::submit_text("request-first", "first"))
        .await
        .unwrap();
    let first = wait_for_acceptance(&mut rx, "request-first").await;
    assert_eq!(first, protocol::SubmissionDisposition::Started);
    wait_for_status(&handle, WorkerStatus::Running).await;

    handle
        .send(Method::submit_text("request-first", "first"))
        .await
        .unwrap();
    let replay = wait_for_acceptance(&mut rx, "request-first").await;
    assert_eq!(replay, protocol::SubmissionDisposition::Started);

    handle
        .send(Method::submit_text("request-running", "second"))
        .await
        .unwrap();

    assert_eq!(
        wait_for_acceptance(&mut rx, "request-running").await,
        protocol::SubmissionDisposition::Queued
    );
    handle
        .send(Method::submit_text("request-running", "second"))
        .await
        .unwrap();
    assert_eq!(
        wait_for_acceptance(&mut rx, "request-running").await,
        protocol::SubmissionDisposition::Queued
    );
    handle
        .send(Method::submit_text("request-running", "conflict"))
        .await
        .unwrap();
    assert!(
        wait_for_rejection(&mut rx, "request-running")
            .await
            .contains("different payload")
    );

    handle.send(Method::ListPendingSubmissions).await.unwrap();
    let pending = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Ok(Event::PendingSubmissionsChanged { pending }) = rx.recv().await {
                break pending;
            }
        }
    })
    .await
    .expect("pending snapshot");
    assert_eq!(pending.submissions.len(), 1);

    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Paused).await;
    handle
        .send(Method::submit_text("request-paused", "third"))
        .await
        .unwrap();

    assert_eq!(
        wait_for_acceptance(&mut rx, "request-paused").await,
        protocol::SubmissionDisposition::Queued
    );
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Paused);
    handle.send(Method::ListPendingSubmissions).await.unwrap();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Ok(Event::PendingSubmissionsChanged { pending }) = rx.recv().await {
                break pending;
            }
        }
    })
    .await
    .expect("paused pending snapshot");
    assert_eq!(snapshot.submissions.len(), 2);
    assert_eq!(snapshot.head_id, pending.head_id);
    handle
        .send(Method::ContinuePending {
            expected_revision: snapshot.revision,
            expected_head_id: snapshot.head_id.unwrap(),
        })
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(1), |event| {
            matches!(event, Event::Error { message, .. } if message.contains("Resume or Cancel"))
        })
        .await
    );
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Paused);

    // Resume completes the original run, then normal completion drains each
    // queued input exactly once and in acceptance order.
    handle
        .send(Method::Resume {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        assert!(
            drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
                matches!(
                    event,
                    Event::RunEnd {
                        result: protocol::RunResult::Finished
                    }
                )
            })
            .await
        );
    }
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let requests = captured.captured_requests();
    assert_eq!(requests.len(), 4);
    let last_inputs: Vec<_> = requests[1..]
        .iter()
        .map(|request| {
            request
                .items
                .iter()
                .rev()
                .find_map(|item| match item {
                    Item::Message {
                        role: agen::Role::User,
                        content,
                        ..
                    } => Some(
                        content
                            .iter()
                            .map(|part| part.as_text())
                            .collect::<String>(),
                    ),
                    _ => None,
                })
                .unwrap()
        })
        .collect();
    assert_eq!(last_inputs, vec!["first", "second", "third"]);
}

#[tokio::test]
async fn resume_without_pause_returns_invalid_state_acknowledgement() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Resume {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

    let mut saw_not_paused = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::CommandAcknowledged { acknowledgement })
                        if acknowledgement.command == protocol::WorkerCommandKind::Resume
                            && acknowledgement.disposition
                                == protocol::WorkerCommandDisposition::InvalidState => {
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

    assert!(saw_not_paused, "should see invalid-state acknowledgement");
}

#[tokio::test]
async fn cancel_without_run_returns_invalid_state_acknowledgement() {
    let client = MockClient::new(simple_text_events());
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

    let mut saw_not_running = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(Event::CommandAcknowledged { acknowledgement })
                        if acknowledgement.command == protocol::WorkerCommandKind::Cancel
                            && acknowledgement.disposition
                                == protocol::WorkerCommandDisposition::InvalidState => {
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

    assert!(saw_not_running, "should see invalid-state acknowledgement");
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
    // `LogEntry::AnnotatedUserInput` must carry the typed segments unchanged so
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
        .send(Method::Submit {
            submission_request_id: protocol::new_submission_request_id(),
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
                Ok(session_store::LogEntry::AnnotatedUserInput { segments, .. }) => {
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
    handle
        .send(Method::Submit {
            submission_request_id: protocol::new_submission_request_id(),
            input: segments,
        })
        .await
        .unwrap();

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
        .send(Method::Submit {
            submission_request_id: protocol::new_submission_request_id(),
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
    handle
        .send(Method::Submit {
            submission_request_id: protocol::new_submission_request_id(),
            input: segments,
        })
        .await
        .unwrap();

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
    let worker = make_worker(client)
        .await
        .with_notification_coalesce_delay(std::time::Duration::from_millis(20));
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::Notify {
            notification_request_id: protocol::new_submission_request_id(),
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
    // Wait for the post-run persist_turn (Flush + TurnEnd + RunCompleted
    // commits) to finish; the controller flips status to Idle right
    // after that.
    wait_for_status(&handle, WorkerStatus::Idle).await;
    // The live echo arrives via the sink's `Event::SystemItem` lane,
    // not on the `working_event_tx` broadcast that `handle.subscribe()` taps.
    // Verify the notification landed on the sink mirror instead.
    let (entries, _) = handle.sink.subscribe_with_snapshot();
    let saw_notify_in_mirror = entries.iter().any(|e| {
        matches!(
            system_item(e),
            Some(session_store::SystemItem::Notification { message, .. }) if message == "turn finished"
        )
    });
    assert!(
        saw_notify_in_mirror,
        "Method::Notify should commit a SystemItem::Notification entry; mirror = {entries:?}"
    );
    let queue_checkpoint_is_atomic = entries.iter().any(|entry| match entry {
        LogEntry::AnnotatedSystemItem { extensions, .. } => extensions.iter().any(|extension| {
            extension.domain == "worker.pending_activations.v1"
                && extension.payload["pending_notifications"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
        }),
        _ => false,
    });
    assert!(
        queue_checkpoint_is_atomic,
        "notification history and queue claim must share one log entry"
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
async fn repeated_notify_while_idle_coalesces_and_auto_starts_one_turn() {
    let client = MockClient::new(simple_text_events());
    let client_for_assert = client.clone();
    let worker = make_worker(client)
        .await
        .with_notification_coalesce_delay(std::time::Duration::from_millis(50));
    let handle = spawn_controller(worker).await;
    let notification_request_id = protocol::new_submission_request_id();

    for _ in 0..2 {
        handle
            .send(Method::Notify {
                notification_request_id: notification_request_id.clone(),
                message: "progress snapshot".into(),
            })
            .await
            .unwrap();
    }
    handle
        .send(Method::Notify {
            notification_request_id: "notify-coalesced-2".into(),
            message: "second coalesced notification".into(),
        })
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if !client_for_assert.captured_requests().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "coalescing deadline did not start a notification turn"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    wait_for_status(&handle, WorkerStatus::Idle).await;
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 1, "coalesced notifications need one turn");
    let notifications = requests[0]
        .items
        .iter()
        .filter_map(|item| item.as_text())
        .filter(|text| text.contains("[Notification]"))
        .collect::<Vec<_>>();
    assert_eq!(
        notifications.len(),
        2,
        "duplicate receipt must not duplicate content"
    );
    assert!(notifications[0].contains("progress snapshot"));
    assert!(notifications[1].contains("second coalesced notification"));
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
            system_item(e),
            Some(session_store::SystemItem::WorkerEvent {
                event: protocol::WorkerEvent::TurnEnded { worker_name },
                ..
            }) if worker_name == "child"
        )
    });
    assert!(
        saw_worker_event_in_mirror,
        "Method::WorkerEvent should commit a SystemItem::WorkerEvent entry"
    );
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);

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
        handle.shared_state.catalog_status(),
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
            system_item(entry),
            Some(session_store::SystemItem::WorkerEvent {
                event: protocol::WorkerEvent::ScopeSubDelegated { .. },
                ..
            })
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "start",
        ))
        .await
        .unwrap();
    handle
        .send(Method::Notify {
            notification_request_id: protocol::new_submission_request_id(),
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
async fn notify_while_running_is_deduped_and_survives_until_next_model_boundary() {
    let client = MockClient::sequential(vec![
        MockResponse::Hang(Vec::new()),
        MockResponse::Complete(simple_text_events()),
    ]);
    let client_for_assert = client.clone();
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "first",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;

    let notification_request_id = protocol::new_submission_request_id();
    for _ in 0..2 {
        handle
            .send(Method::Notify {
                notification_request_id: notification_request_id.clone(),
                message: "durable weak notice".into(),
            })
            .await
            .unwrap();
    }
    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let mut rx = handle.subscribe();
    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "second",
        ))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(rx.recv().await, Ok(Event::TurnEnd { .. })) {
                break;
            }
        }
    })
    .await
    .expect("second submit completes");

    let requests = client_for_assert.captured_requests();
    let notice_count = requests[1]
        .items
        .iter()
        .filter_map(|item| item.as_text())
        .filter(|text| text.contains("durable weak notice"))
        .count();
    assert_eq!(notice_count, 1);
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
async fn shutdown_closes_method_admission_before_terminal_confirmation() {
    let worker = make_worker(MockClient::new(simple_text_events())).await;
    let runtime_base = tempfile::tempdir().unwrap();
    let bash_output_dir = runtime_base.path().join("bash-output");
    let (handle, mut shutdown_rx) =
        WorkerController::spawn(worker, runtime_base.path(), &bash_output_dir)
            .await
            .unwrap();
    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                biased;
                result = handle.send(Method::ListRewindTargets) => {
                    if result.is_err() {
                        break;
                    }
                }
                result = &mut shutdown_rx => {
                    result.expect("controller shutdown signal should remain open");
                    panic!("method admission remained open until terminal confirmation");
                }
            }
        }
    })
    .await
    .expect("method admission did not close during shutdown");
    shutdown_rx.await.unwrap();
}

#[tokio::test]
async fn shutdown_joins_socket_server_with_active_connection() {
    use tokio::net::UnixStream;

    let worker = make_worker(MockClient::new(simple_text_events())).await;
    let runtime_base = tempfile::tempdir().unwrap();
    let bash_output_dir = runtime_base.path().join("bash-output");
    let (handle, shutdown_rx) =
        WorkerController::spawn(worker, runtime_base.path(), &bash_output_dir)
            .await
            .unwrap();
    let socket_path = handle.runtime_dir.socket_path();
    let _connection = UnixStream::connect(&socket_path).await.unwrap();

    handle
        .send(Method::Shutdown {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown_rx)
        .await
        .expect("controller should join its socket tasks")
        .expect("controller shutdown signal should remain open");
    assert!(!socket_path.exists());
}

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
    writer
        .write(&Method::submit_text(
            protocol::new_submission_request_id(),
            "Hello",
        ))
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
    // broadcast lanes (sink fan-out vs `working_event_tx`), so their relative
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
                    Ok(Some(Event::SystemItem { ref item, .. }))
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
        _ctx: agen::tool::ToolExecutionContext,
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

fn idle_only_input(request_id: &str) -> Method {
    Method::SubmitIfIdle {
        submission_request_id: request_id.to_string(),
        input: vec![protocol::Segment::text(request_id)],
        source: protocol::AuthenticatedInputSource::Backend {
            operation_id: request_id.to_string(),
        },
    }
}

#[tokio::test]
async fn idle_only_submit_rejects_admission_race_and_paused_without_persisting() {
    let client = MockClient::sequential(vec![MockResponse::Hang(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "partial"),
    ])]);
    let handle = spawn_controller(make_worker(client.clone()).await).await;
    let mut rx = handle.subscribe();
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);
    // Both callers can observe Idle. Admission of the first request must make
    // the second fail rather than silently scheduling a later turn.
    handle.send(idle_only_input("first-idle")).await.unwrap();
    handle.send(idle_only_input("racing-input")).await.unwrap();
    let mut accepted_first = false;
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
            match event {
                Event::SubmissionAccepted {
                    submission_request_id,
                    ..
                } => {
                    assert_eq!(submission_request_id, "first-idle");
                    accepted_first = true;
                    false
                }
                Event::SubmissionRejected {
                    submission_request_id,
                    message,
                } => {
                    assert_eq!(submission_request_id, "racing-input");
                    assert!(message.contains("WorkerNotify"));
                    true
                }
                _ => false,
            }
        })
        .await
    );
    assert!(accepted_first);
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Paused).await;
    handle.send(idle_only_input("paused-input")).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
            matches!(event, Event::SubmissionRejected { submission_request_id, message }
            if submission_request_id == "paused-input" && message.contains("WorkerNotify"))
        })
        .await
    );
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Paused);
    handle.send(Method::ListPendingSubmissions).await.unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
            if let Event::PendingSubmissionsChanged { pending } = event {
                assert!(pending.submissions.is_empty());
                true
            } else {
                false
            }
        })
        .await
    );
    assert_eq!(client.captured_requests().len(), 1);
    let entries = serde_json::to_string(&handle.committed_entries()).unwrap();
    assert!(!entries.contains("racing-input"));
    assert!(!entries.contains("paused-input"));
    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;
}

/// Paused → Running → Idle. A notification accepted while Paused must not
/// auto-resume the Worker, and explicit Resume must inject it while preserving
/// the interrupted turn's history consistency.
#[tokio::test]
async fn pause_then_resume_preserves_notifications_and_history_consistency() {
    // Response 1: report billable partial usage, then hang after opening a text
    // block (no stop / completed). Pause must flush that usage without treating
    // the provider request as normally completed.
    let hang = MockResponse::Hang(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "partial..."),
        LlmEvent::Usage(UsageEvent {
            input_tokens: Some(90),
            output_tokens: Some(2),
            total_tokens: Some(92),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }),
    ]);
    // Response 2: a clean assistant reply delivered on Resume.
    let ok = MockResponse::Complete(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "resumed output"),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Usage(UsageEvent {
            input_tokens: Some(100),
            output_tokens: Some(3),
            total_tokens: Some(103),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]);
    let client = MockClient::sequential(vec![hang, ok]);
    let client_for_assert = client.clone();
    let worker = make_worker(client)
        .await
        .with_notification_coalesce_delay(std::time::Duration::from_millis(20));
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "hello",
        ))
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

    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

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
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Paused);
    let (paused_entries, _) = handle.sink.subscribe_with_snapshot();
    let paused_usage = paused_entries
        .iter()
        .filter_map(|entry| match entry {
            LogEntry::LlmUsage {
                input_total_tokens,
                output_tokens,
                ..
            } => Some((*input_total_tokens, *output_tokens)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paused_usage,
        vec![(90, 2)],
        "Pause must durably account for usage already observed on the interrupted request"
    );

    handle
        .send(Method::Notify {
            notification_request_id: "notify-while-paused".into(),
            message: "resume context".into(),
        })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(
        handle.shared_state.catalog_status(),
        WorkerStatus::Paused,
        "notification deadline must not auto-resume a paused Worker"
    );
    assert_eq!(
        client_for_assert.captured_requests().len(),
        1,
        "paused notification must wait for an explicit resume"
    );

    handle
        .send(Method::Resume {
            command: worker_command(&handle),
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
        "expected RunEnd::Finished after Resume"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Idle);
    let requests = client_for_assert.captured_requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].items.iter().any(|item| {
        item.as_text()
            .is_some_and(|text| text.contains("[Notification]") && text.contains("resume context"))
    }));

    // History consistency: the interrupted partial response is absent, while
    // the queued notification is committed on the explicit resume.
    let history = history_from_sink(&handle);
    let roles: Vec<&str> = history
        .iter()
        .filter_map(|i| match i {
            Item::Message { role, .. } => match role {
                agen::Role::User => Some("user"),
                agen::Role::Assistant => Some("assistant"),
                agen::Role::System => Some("system"),
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "system", "assistant"],
        "history should retain user input, queued notification, and resumed output; got {history:?}"
    );
    let assistant_text = history
        .iter()
        .find_map(|i| match i {
            Item::Message {
                role: agen::Role::Assistant,
                content,
                ..
            } => Some(
                content
                    .iter()
                    .map(|p: &agen::ContentPart| p.as_text().to_owned())
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(assistant_text, "resumed output");
    let (final_entries, _) = handle.sink.subscribe_with_snapshot();
    let final_usage = final_entries
        .iter()
        .filter_map(|entry| match entry {
            LogEntry::LlmUsage {
                input_total_tokens,
                output_tokens,
                ..
            } => Some((*input_total_tokens, *output_tokens)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        final_usage,
        vec![(90, 2), (100, 3)],
        "the paused request must remain a distinct billing record rather than merging into resume"
    );
    let has_tool_call = history.iter().any(|i| i.is_tool_call());
    assert!(!has_tool_call, "no orphan tool_call in history");
}

/// Paused with an orphan `tool_use` in history must queue a fresh Submit.
/// After explicit Cancel and ContinuePending, the queued Submit must produce
/// a wire-valid next LLM request: the orphan is closed with a synthetic
/// `tool_result`, a system note is inserted, and the new user input is appended.
#[tokio::test]
async fn paused_submit_waits_for_cancel_and_continue_pending() {
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "first",
        ))
        .await
        .unwrap();

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

    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
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
    assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Paused);

    let paused_request_id = protocol::new_submission_request_id();
    handle
        .send(Method::submit_text(
            paused_request_id.clone(),
            "new request",
        ))
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |event| {
            matches!(
                event,
                Event::SubmissionAccepted {
                    submission_request_id,
                    disposition: protocol::SubmissionDisposition::Queued,
                    ..
                } if submission_request_id == &paused_request_id
            )
        })
        .await,
        "Paused Submit must be queued"
    );
    assert_eq!(
        client_for_assert.captured_requests().len(),
        1,
        "queued Paused Submit must not start another LLM request"
    );

    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Idle).await;

    // Cancel must not start the queued turn. Explicit ContinuePending runs
    // interrupt prep, closing the orphan before committing the queued input.
    assert_eq!(client_for_assert.captured_requests().len(), 1);
    handle.send(Method::ListPendingSubmissions).await.unwrap();
    let pending = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Ok(Event::PendingSubmissionsChanged { pending }) = rx.recv().await {
                break pending;
            }
        }
    })
    .await
    .expect("pending input retained after Cancel");
    assert_eq!(pending.submissions.len(), 1);
    handle
        .send(Method::ContinuePending {
            expected_revision: pending.revision,
            expected_head_id: pending.head_id.unwrap(),
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
        "expected RunEnd::Finished after Cancel→ContinuePending"
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
            agen::Item::ToolResult {
                call_id,
                summary,
                disposition,
                ..
            } if call_id == "call_orphan" => {
                assert_eq!(summary, "Tool execution outcome unknown");
                assert_eq!(*disposition, agen::ToolResultDisposition::OutcomeUnknown);
                saw_synthetic_tool_result = true;
            }
            agen::Item::Message { role, content, .. } if *role == agen::Role::System => {
                let text: String = content.iter().map(|p| p.as_text()).collect();
                if text.contains("interrupted by the user") {
                    saw_interruption_note = true;
                }
            }
            agen::Item::Message { role, content, .. } if *role == agen::Role::User => {
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
    let idx = |pred: &dyn Fn(&agen::Item) -> bool| items.iter().position(pred).unwrap();
    let tool_result_idx =
        idx(&|i| matches!(i, agen::Item::ToolResult { call_id, .. } if call_id == "call_orphan"));
    let sys_idx = idx(&|i| match i {
        agen::Item::Message {
            role: agen::Role::System,
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
        agen::Item::Message {
            role: agen::Role::User,
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "first",
        ))
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::ToolCallDone { .. }
        ))
        .await,
        "tool_call_done should arrive before pause"
    );

    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
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

    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
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
                result: agen::EngineResult::Finished,
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

    handle
        .send(Method::Resume {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
    assert!(
        drain_until(&mut rx, std::time::Duration::from_secs(2), |e| matches!(
            e,
            Event::CommandAcknowledged { acknowledgement }
                if acknowledgement.command == protocol::WorkerCommandKind::Resume
                    && acknowledgement.disposition
                        == protocol::WorkerCommandDisposition::InvalidState
        ))
        .await,
        "resume after paused cancel should receive invalid-state acknowledgement"
    );
    assert_eq!(
        client_for_assert.captured_requests().len(),
        1,
        "rejected resume must not call the LLM"
    );

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "fresh request",
        ))
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
            agen::Item::ToolResult {
                call_id,
                disposition: agen::ToolResultDisposition::OutcomeUnknown,
                ..
            } if call_id == "call_cancelled"
        )),
        "paused cancel should close orphan tool_use before future requests: {items:?}"
    );
    assert!(
        items.iter().any(|item| matches!(
            item,
            agen::Item::Message {
                role: agen::Role::System,
                ..
            } if item_text_contains(item, "interrupted by the user")
        )),
        "paused cancel should record an explicit interruption note: {items:?}"
    );
    assert!(
        items.iter().any(|item| matches!(
            item,
            agen::Item::Message {
                role: agen::Role::User,
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
            Event::Snapshot { session, .. } => {
                return session.entries.into_iter().any(|entry| match entry.data {
                    protocol::SessionSnapshotEntryData::UserInput { segments } => {
                        protocol::Segment::flatten_to_text(&segments).contains(needle)
                    }
                    _ => false,
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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "rollback me",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

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
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "pause rollback",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

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

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "first kept",
        ))
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
        "expected first run to finish"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "second rolled back",
        ))
        .await
        .unwrap();
    wait_for_status(&handle, WorkerStatus::Running).await;
    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();
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
async fn cancel_after_assistant_output_is_a_non_failure_terminal() {
    let client = MockClient::sequential(vec![MockResponse::Hang(vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "committed before cancel"),
        LlmEvent::text_block_stop(0, None),
    ])]);
    let worker = make_worker(client).await;
    let handle = spawn_controller(worker).await;
    let mut rx = handle.subscribe();

    handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "cancel this active turn",
        ))
        .await
        .unwrap();
    assert!(
        drain_until(
            &mut rx,
            std::time::Duration::from_secs(2),
            |event| matches!(event, Event::TextDone { .. })
        )
        .await,
        "assistant output should be durable before cancellation"
    );
    handle
        .send(Method::Cancel {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

    assert!(
        drain_until(
            &mut rx,
            std::time::Duration::from_secs(2),
            |event| matches!(
                event,
                Event::RunEnd {
                    result: protocol::RunResult::Cancelled
                }
            )
        )
        .await,
        "intentional cancellation must emit RunEnd::Cancelled, not Event::Error"
    );
    wait_for_status(&handle, WorkerStatus::Idle).await;

    let (entries, _) = handle.sink.subscribe_with_snapshot();
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, LogEntry::RunCancelled { .. })),
        "cancelled run must keep a durable non-failure terminal: {entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LogEntry::RunErrored { .. })),
        "intentional cancellation must not be restored as a run error: {entries:?}"
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
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "keep this turn",
        ))
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
    handle
        .send(Method::Pause {
            command: worker_command(&handle),
        })
        .await
        .unwrap();

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
