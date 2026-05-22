//! Integration tests for the pod-comm tools (`SendToPod`,
//! `ReadPodOutput`, `StopPod`, `ListPods`).
//!
//! The real child Pod binary is not started. Instead each test stands
//! up a mock `UnixListener` that speaks the socket protocol directly:
//! it emits the connect-time `Event::Snapshot`, accepts methods such as
//! `Method::Run` / `Method::Shutdown`, and responds with the relevant
//! events when needed. This keeps the tests fast and independent of the
//! LLM layer — the tools are exercised for their wire behaviour alone.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use llm_worker::llm_client::types::{ContentPart, Item, Role};
use llm_worker::tool::ToolOutput;
use manifest::{Permission, Scope, ScopeRule, SharedScope};
use pod::runtime::dir::{RuntimeDir, SpawnedPodRecord};
use pod::runtime::pod_registry::{self, LockFileGuard};
use pod::spawn::comm_tools::{
    list_pods_tool, read_pod_output_tool, send_to_pod_tool, stop_pod_tool,
};
use pod::spawn::registry::SpawnedPodRegistry;
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, Greeting, Method};
use serde_json::json;
use session_store::{FsStore, PodMetadataStore};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Serialises env-mutating tests. The test harness runs tasks across
/// threads, and `INSOMNIA_RUNTIME_DIR` is a process-wide resource.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Take `ENV_LOCK` and clear any env vars that would outrank
/// `INSOMNIA_RUNTIME_DIR` in `paths::runtime_dir` resolution; restore
/// previous values on drop.
struct EnvGuard {
    prev_home: Option<String>,
    prev_xdg: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn acquire() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_home = std::env::var("INSOMNIA_HOME").ok();
        let prev_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe {
            std::env::remove_var("INSOMNIA_HOME");
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
        Self {
            prev_home,
            prev_xdg,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_home {
                Some(v) => std::env::set_var("INSOMNIA_HOME", v),
                None => std::env::remove_var("INSOMNIA_HOME"),
            }
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
            std::env::remove_var("INSOMNIA_RUNTIME_DIR");
        }
    }
}

/// Create a spawner-owned `RuntimeDir` + `SpawnedPodRegistry` scoped to
/// a fresh tempdir. The returned `TempDir` must be kept alive by the
/// caller for the duration of the test.
async fn setup_registry() -> (TempDir, Arc<SpawnedPodRegistry>, Arc<RuntimeDir>) {
    let tmp = TempDir::new().unwrap();
    let rd = RuntimeDir::create(tmp.path(), "spawner").await.unwrap();
    let rd = Arc::new(rd);
    let registry = SpawnedPodRegistry::new(rd.clone());
    (tmp, registry, rd)
}

/// Register a fake spawned-child record pointing at a given socket
/// path, with a trivial write-scope for `scope_path`. Does not touch
/// pods.json.
async fn register_child(
    registry: &SpawnedPodRegistry,
    name: &str,
    socket: &Path,
    scope_path: &Path,
) {
    let record = SpawnedPodRecord {
        pod_name: name.into(),
        socket_path: socket.to_path_buf(),
        scope_delegated: vec![ScopeRule {
            target: scope_path.to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
        callback_address: "/dev/null".into(),
    };
    registry.add(record).await.unwrap();
}

/// Bind a Unix listener at a socket path inside the given directory.
async fn bind_mock_socket(dir: &Path, name: &str) -> (PathBuf, UnixListener) {
    let socket = dir.join(format!("{name}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    (socket, listener)
}

/// Minimal connect-time snapshot used by mock socket servers.
fn empty_snapshot() -> Event {
    Event::Snapshot {
        entries: Vec::new(),
        greeting: Greeting {
            pod_name: "child".into(),
            cwd: "/tmp".into(),
            provider: "anthropic".into(),
            model: "x".into(),
            scope_summary: String::new(),
            tools: Vec::new(),
            context_window: 200_000,
            context_tokens: 0,
        },
        status: protocol::PodStatus::Idle,
    }
}

/// Accept one connection, send the protocol's connect-time snapshot,
/// and read exactly one `Method` line from it.
/// The reader half is kept open; caller awaits the returned handle.
fn accept_one_method(listener: UnixListener) -> JoinHandle<Option<Method>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.ok()?;
        let (r, w) = stream.into_split();
        let mut reader = JsonLineReader::new(r);
        let mut writer = JsonLineWriter::new(w);
        writer.write(&empty_snapshot()).await.ok()?;
        reader.next::<Method>().await.ok().flatten()
    })
}

/// Accept one connection, send the protocol's connect-time snapshot,
/// read one `Method`, then write `response` back. Used by `SendToPod`
/// tests to mock the real controller's `TurnStart` acknowledgement (or
/// its `AlreadyRunning` rejection).
fn accept_method_and_respond(
    listener: UnixListener,
    response: Event,
) -> JoinHandle<Option<Method>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.ok()?;
        let (r, w) = stream.into_split();
        let mut reader = JsonLineReader::new(r);
        let mut writer = JsonLineWriter::new(w);
        writer.write(&empty_snapshot()).await.ok()?;
        let method = reader.next::<Method>().await.ok().flatten();
        if method.is_some() {
            let _ = writer.write(&response).await;
        }
        method
    })
}

/// Pretend to be a spawned Pod whose connect-time snapshot carries a
/// fixed set of assistant items. Sends `Event::Snapshot` immediately on
/// every accept — the real Pod does the same, so `ReadPodOutput`'s
/// `fetch_history` just consumes the first non-Alert event.
fn serve_history(listener: UnixListener, items: Vec<Item>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (_r, w) = stream.into_split();
            let mut writer = JsonLineWriter::new(w);
            let entries: Vec<serde_json::Value> = items
                .iter()
                .map(|item| {
                    let entry = session_store::LogEntry::AssistantItem {
                        ts: 0,
                        item: session_store::LoggedItem::from(item),
                    };
                    serde_json::to_value(&entry).unwrap()
                })
                .collect();
            let event = Event::Snapshot {
                entries,
                greeting: Greeting {
                    pod_name: "child".into(),
                    cwd: "/tmp".into(),
                    provider: "anthropic".into(),
                    model: "x".into(),
                    scope_summary: String::new(),
                    tools: Vec::new(),
                    context_window: 200_000,
                    context_tokens: 0,
                },
                status: protocol::PodStatus::Idle,
            };
            let _ = writer.write(&event).await;
        }
    })
}

fn serve_pod_methods(listener: UnixListener) -> mpsc::Receiver<Method> {
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (r, w) = stream.into_split();
            let mut reader = JsonLineReader::new(r);
            let mut writer = JsonLineWriter::new(w);
            if writer.write(&empty_snapshot()).await.is_err() {
                continue;
            }
            let Some(method) = reader.next::<Method>().await.ok().flatten() else {
                continue;
            };
            let is_shutdown = matches!(method, Method::Shutdown);
            if matches!(method, Method::Run { .. }) {
                let _ = writer.write(&Event::TurnStart { turn: 1 }).await;
            }
            if tx.send(method).await.is_err() || is_shutdown {
                return;
            }
        }
    });
    rx
}

fn assistant(text: &str) -> Item {
    Item::Message {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::Text { text: text.into() }],
        status: None,
    }
}

// ---------------------------------------------------------------------------
// SendToPod
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_to_pod_delivers_run_method() {
    let (tmp, registry, _rd) = setup_registry().await;
    let (socket, listener) = bind_mock_socket(tmp.path(), "child").await;
    // Mock the controller's accept path: after reading the method,
    // ack with `TurnStart` so `SendToPod`'s confirmation loop succeeds.
    let received = accept_method_and_respond(listener, Event::TurnStart { turn: 1 });
    register_child(&registry, "child", &socket, tmp.path()).await;

    let def = send_to_pod_tool(registry);
    let (_meta, tool) = def();
    let input = json!({ "name": "child", "message": "hello there" }).to_string();
    let output: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(
        output.summary.contains("child"),
        "summary: {}",
        output.summary
    );

    let method = received.await.unwrap().expect("expected a method");
    match method {
        Method::Run { input } => match input.as_slice() {
            [protocol::Segment::Text { content }] => assert_eq!(content, "hello there"),
            other => panic!("expected single Text segment, got {other:?}"),
        },
        other => panic!("expected Run, got {other:?}"),
    }
}

#[tokio::test]
async fn send_to_pod_errors_on_unknown_pod() {
    let (_tmp, registry, _rd) = setup_registry().await;
    let def = send_to_pod_tool(registry);
    let (_meta, tool) = def();
    let input = json!({ "name": "nope", "message": "hi" }).to_string();
    let err = tool.execute(&input).await.unwrap_err();
    assert!(err.to_string().contains("no spawned pod"), "{err}");
}

#[tokio::test]
async fn send_to_pod_errors_when_pod_already_running() {
    let (tmp, registry, _rd) = setup_registry().await;
    let (socket, listener) = bind_mock_socket(tmp.path(), "child").await;
    // Respond with the same `Error { AlreadyRunning }` that the real
    // controller emits when `Method::Run` arrives during RUNNING.
    let received = accept_method_and_respond(
        listener,
        Event::Error {
            code: ErrorCode::AlreadyRunning,
            message: "Pod is already executing a turn".into(),
        },
    );
    register_child(&registry, "child", &socket, tmp.path()).await;

    let def = send_to_pod_tool(registry);
    let (_meta, tool) = def();
    let input = json!({ "name": "child", "message": "hi" }).to_string();
    let err = tool.execute(&input).await.unwrap_err();
    assert!(
        err.to_string().contains("already running"),
        "expected AlreadyRunning wording: {err}"
    );

    // Ensure the listener was in fact hit with a Method::Run before the
    // rejection path fired — otherwise we'd be asserting on an error
    // that came from a connect failure.
    let method = received.await.unwrap().expect("expected a method");
    assert!(matches!(method, Method::Run { .. }));
}

// ---------------------------------------------------------------------------
// ReadPodOutput
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_pod_output_returns_new_assistant_text_then_empty_on_second_call() {
    let (tmp, registry, _rd) = setup_registry().await;
    let (socket, listener) = bind_mock_socket(tmp.path(), "child").await;
    register_child(&registry, "child", &socket, tmp.path()).await;

    let items = vec![
        Item::user_message("hello"),
        assistant("hi back"),
        assistant("still working"),
    ];
    let _server = serve_history(listener, items);

    let def = read_pod_output_tool(registry);
    let (_meta, tool) = def();
    let input = json!({ "name": "child" }).to_string();

    let first: ToolOutput = tool.execute(&input).await.unwrap();
    let body = first.content.expect("first read should have content");
    assert!(body.contains("hi back"), "body: {body}");
    assert!(body.contains("still working"), "body: {body}");

    // Cursor now points past all items — second call returns no new text.
    let second: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(
        second.content.is_none(),
        "unexpected content: {:?}",
        second.content
    );
    assert!(
        second.summary.contains("no new assistant text"),
        "summary: {}",
        second.summary
    );
}

#[tokio::test]
async fn read_pod_output_reports_stopped_on_dead_socket() {
    let (tmp, registry, _rd) = setup_registry().await;
    // Register a record pointing at a socket that nobody is listening
    // on. Connect must fail → tool reports "stopped".
    let dead_socket = tmp.path().join("dead.sock");
    register_child(&registry, "child", &dead_socket, tmp.path()).await;

    let def = read_pod_output_tool(registry);
    let (_meta, tool) = def();
    let input = json!({ "name": "child" }).to_string();
    let output: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(output.summary.contains("stopped"), "{}", output.summary);
}

// ---------------------------------------------------------------------------
// StopPod
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stop_pod_sends_shutdown_and_releases_scope() {
    let _env = EnvGuard::acquire();
    let tmp = TempDir::new().unwrap();
    let store_tmp = TempDir::new().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    let rd = Arc::new(RuntimeDir::create(tmp.path(), "spawner").await.unwrap());
    let parent_scope = SharedScope::new(
        Scope::writable(tmp.path())
            .unwrap()
            .with_added_deny_rules([ScopeRule {
                target: tmp.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }])
            .unwrap(),
    );
    unsafe {
        std::env::set_var("INSOMNIA_RUNTIME_DIR", tmp.path());
    }
    let lock_path = tmp.path().join("pods.json");

    // Seed pods.json with a restored top-level `spawner` allocation whose
    // scope_deny contains the delegated child path plus the live child
    // allocation — mimics a parent resumed after SpawnPod.
    {
        let mut g = LockFileGuard::open(&lock_path).unwrap();
        let rule = ScopeRule {
            target: tmp.path().to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        };
        pod_registry::register_pod_with_deny(
            &mut g,
            "spawner".into(),
            std::process::id(),
            "/tmp/spawner.sock".into(),
            vec![rule.clone()],
            vec![rule.clone()],
            session_store::new_segment_id(),
        )
        .unwrap();
        pod_registry::register_pod(
            &mut g,
            "child".into(),
            std::process::id(),
            "/tmp/child.sock".into(),
            vec![rule],
            session_store::new_segment_id(),
        )
        .unwrap();
    }

    let loaded = SpawnedPodRegistry::load_from_pod_state_with_reclaim(
        rd.clone(),
        store.clone(),
        "spawner".into(),
        Some(parent_scope.clone()),
        None,
    )
    .await
    .unwrap();
    let registry = loaded.registry;

    let (socket, listener) = bind_mock_socket(tmp.path(), "child").await;
    let received = accept_one_method(listener);
    register_child(&registry, "child", &socket, tmp.path()).await;

    let def = stop_pod_tool(registry.clone());
    let (_meta, tool) = def();
    let input = json!({ "name": "child" }).to_string();
    let output: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(output.summary.contains("stopped"), "{}", output.summary);

    // The child got a Shutdown.
    let method = received.await.unwrap().expect("expected shutdown");
    assert!(matches!(method, Method::Shutdown));

    // Allocation for `child` is gone; `spawner` remains and its restored
    // dynamic deny layer has been reclaimed.
    {
        let g = LockFileGuard::open(&lock_path).unwrap();
        assert!(g.data().find("child").is_none(), "child still allocated");
        let spawner = g.data().find("spawner").expect("spawner missing");
        assert!(spawner.scope_deny.is_empty(), "deny not reclaimed");
    }
    assert_eq!(
        parent_scope
            .snapshot()
            .permission_at(&tmp.path().join("file.txt")),
        Some(Permission::Write)
    );

    // spawned_pods.json now lists zero children.
    let spawned = rd.path().join("spawned_pods.json");
    let contents = std::fs::read_to_string(&spawned).unwrap();
    let records: Vec<SpawnedPodRecord> = serde_json::from_str(&contents).unwrap();
    assert!(records.is_empty());
}

#[tokio::test]
async fn stop_pod_succeeds_even_when_child_unreachable() {
    let _env = EnvGuard::acquire();
    let (tmp, registry, _rd) = setup_registry().await;
    unsafe {
        std::env::set_var("INSOMNIA_RUNTIME_DIR", tmp.path());
    }

    // No live listener — socket never bound. Registered record points
    // at a dead path. StopPod should still clean up local bookkeeping.
    let dead_socket = tmp.path().join("dead.sock");
    register_child(&registry, "child", &dead_socket, tmp.path()).await;

    let def = stop_pod_tool(registry.clone());
    let (_meta, tool) = def();
    let input = json!({ "name": "child" }).to_string();
    let output: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(output.summary.contains("stopped"), "{}", output.summary);

    // Registry no longer knows about the child.
    assert!(registry.get("child").await.is_none());
}

// ---------------------------------------------------------------------------
// Persistence / restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restored_registry_uses_pod_state_without_runtime_file() {
    let _env = EnvGuard::acquire();
    let runtime_tmp = TempDir::new().unwrap();
    let store_tmp = TempDir::new().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    unsafe {
        std::env::set_var("INSOMNIA_RUNTIME_DIR", runtime_tmp.path());
    }

    let rd = Arc::new(
        RuntimeDir::create(runtime_tmp.path(), "spawner")
            .await
            .unwrap(),
    );
    let registry =
        SpawnedPodRegistry::load_from_pod_state(rd.clone(), store.clone(), "spawner".to_string())
            .await
            .unwrap();

    let (socket, listener) = bind_mock_socket(runtime_tmp.path(), "child").await;
    let mut received = serve_pod_methods(listener);
    register_child(&registry, "child", &socket, runtime_tmp.path()).await;

    std::fs::remove_file(rd.path().join("spawned_pods.json")).unwrap();

    let restored =
        SpawnedPodRegistry::load_from_pod_state(rd.clone(), store.clone(), "spawner".to_string())
            .await
            .unwrap();

    let def = list_pods_tool(restored.clone());
    let (_meta, tool) = def();
    let output: ToolOutput = tool.execute("{}").await.unwrap();
    assert!(output.summary.contains("1 pod"), "{}", output.summary);
    let body = output.content.expect("restored ListPods should list child");
    assert!(body.contains("child [alive]"), "body: {body}");

    let def = send_to_pod_tool(restored.clone());
    let (_meta, tool) = def();
    let input = json!({ "name": "child", "message": "after restart" }).to_string();
    tool.execute(&input).await.unwrap();
    match received.recv().await.expect("expected Run") {
        Method::Run { input } => match input.as_slice() {
            [protocol::Segment::Text { content }] => assert_eq!(content, "after restart"),
            other => panic!("expected single Text segment, got {other:?}"),
        },
        other => panic!("expected Run, got {other:?}"),
    }

    let def = stop_pod_tool(restored.clone());
    let (_meta, tool) = def();
    tool.execute(&json!({ "name": "child" }).to_string())
        .await
        .unwrap();
    assert!(matches!(
        received.recv().await.expect("expected Shutdown"),
        Method::Shutdown
    ));
    assert!(restored.get("child").await.is_none());

    let metadata = store
        .read_by_name("spawner")
        .unwrap()
        .expect("spawner metadata should remain");
    assert!(metadata.spawned_children.is_empty());
    let runtime_contents = std::fs::read_to_string(rd.path().join("spawned_pods.json")).unwrap();
    let runtime_records: Vec<SpawnedPodRecord> = serde_json::from_str(&runtime_contents).unwrap();
    assert!(runtime_records.is_empty());
}

#[tokio::test]
async fn load_from_pod_state_prunes_children_with_missing_sockets() {
    let runtime_tmp = TempDir::new().unwrap();
    let store_tmp = TempDir::new().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    let rd = Arc::new(
        RuntimeDir::create(runtime_tmp.path(), "spawner")
            .await
            .unwrap(),
    );
    let registry =
        SpawnedPodRegistry::load_from_pod_state(rd.clone(), store.clone(), "spawner".to_string())
            .await
            .unwrap();

    let (live_socket, listener) = bind_mock_socket(runtime_tmp.path(), "alive").await;
    let _server = serve_pod_methods(listener);
    register_child(&registry, "alive", &live_socket, runtime_tmp.path()).await;
    register_child(
        &registry,
        "missing",
        &runtime_tmp.path().join("missing.sock"),
        runtime_tmp.path(),
    )
    .await;

    let restored =
        SpawnedPodRegistry::load_from_pod_state(rd.clone(), store.clone(), "spawner".to_string())
            .await
            .unwrap();

    assert!(restored.get("alive").await.is_some());
    assert!(restored.get("missing").await.is_none());
    let metadata = store
        .read_by_name("spawner")
        .unwrap()
        .expect("spawner metadata should be written");
    assert_eq!(metadata.spawned_children.len(), 1);
    assert_eq!(metadata.spawned_children[0].pod_name, "alive");
}

#[tokio::test]
async fn load_from_pod_state_reclaims_pruned_child_scope_and_registry_deny() {
    let _env = EnvGuard::acquire();
    let runtime_tmp = TempDir::new().unwrap();
    let store_tmp = TempDir::new().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    unsafe {
        std::env::set_var("INSOMNIA_RUNTIME_DIR", runtime_tmp.path());
    }
    let rd = Arc::new(
        RuntimeDir::create(runtime_tmp.path(), "spawner")
            .await
            .unwrap(),
    );
    let missing_rule = ScopeRule {
        target: runtime_tmp.path().to_path_buf(),
        permission: Permission::Write,
        recursive: true,
    };

    {
        let mut g = LockFileGuard::open(&runtime_tmp.path().join("pods.json")).unwrap();
        pod_registry::register_pod_with_deny(
            &mut g,
            "spawner".into(),
            std::process::id(),
            "/tmp/spawner.sock".into(),
            vec![missing_rule.clone()],
            vec![missing_rule.clone()],
            session_store::new_segment_id(),
        )
        .unwrap();
        pod_registry::register_pod(
            &mut g,
            "missing".into(),
            std::process::id(),
            "/tmp/missing.sock".into(),
            vec![missing_rule.clone()],
            session_store::new_segment_id(),
        )
        .unwrap();
    }

    let parent_scope = SharedScope::new(
        Scope::writable(runtime_tmp.path())
            .unwrap()
            .with_added_deny_rules([missing_rule.clone()])
            .unwrap(),
    );
    let seed = SpawnedPodRegistry::load_from_pod_state(rd.clone(), store.clone(), "spawner".into())
        .await
        .unwrap();
    seed.add(SpawnedPodRecord {
        pod_name: "missing".into(),
        socket_path: runtime_tmp.path().join("missing.sock"),
        scope_delegated: vec![missing_rule.clone()],
        callback_address: "/dev/null".into(),
    })
    .await
    .unwrap();

    let loaded = SpawnedPodRegistry::load_from_pod_state_with_reclaim(
        rd.clone(),
        store.clone(),
        "spawner".into(),
        Some(parent_scope.clone()),
        None,
    )
    .await
    .unwrap();
    assert!(loaded.reclaimed_unreachable);
    assert!(loaded.registry.get("missing").await.is_none());
    assert_eq!(
        parent_scope
            .snapshot()
            .permission_at(&runtime_tmp.path().join("file.txt")),
        Some(Permission::Write)
    );

    let g = LockFileGuard::open(&runtime_tmp.path().join("pods.json")).unwrap();
    assert!(g.data().find("missing").is_none());
    assert!(g.data().find("spawner").unwrap().scope_deny.is_empty());
    let metadata = store
        .read_by_name("spawner")
        .unwrap()
        .expect("spawner metadata should remain");
    assert!(metadata.spawned_children.is_empty());
    let runtime_contents = std::fs::read_to_string(rd.path().join("spawned_pods.json")).unwrap();
    let runtime_records: Vec<SpawnedPodRecord> = serde_json::from_str(&runtime_contents).unwrap();
    assert!(runtime_records.is_empty());
}

// ---------------------------------------------------------------------------
// ListPods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_pods_reports_alive_and_stopped() {
    let (tmp, registry, _rd) = setup_registry().await;

    // One child is reachable…
    let (live_socket, listener) = bind_mock_socket(tmp.path(), "alive").await;
    // Keep the listener alive by moving it into a task that never exits.
    let _accept = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            drop(stream);
        }
    });
    register_child(&registry, "alive", &live_socket, tmp.path()).await;

    // …the other is not.
    let dead_socket = tmp.path().join("dead.sock");
    register_child(&registry, "dead", &dead_socket, tmp.path()).await;

    let def = list_pods_tool(registry);
    let (_meta, tool) = def();
    let output: ToolOutput = tool.execute("{}").await.unwrap();
    assert!(output.summary.contains("2 pod"), "{}", output.summary);
    let body = output.content.expect("list_pods should populate content");
    assert!(body.contains("alive [alive]"), "body: {body}");
    assert!(body.contains("dead [stopped]"), "body: {body}");
}

#[tokio::test]
async fn list_pods_empty_when_nothing_registered() {
    let (_tmp, registry, _rd) = setup_registry().await;
    let def = list_pods_tool(registry);
    let (_meta, tool) = def();
    let output: ToolOutput = tool.execute("{}").await.unwrap();
    assert!(
        output.summary.contains("no spawned pods"),
        "{}",
        output.summary
    );
    assert!(output.content.is_none());
}
