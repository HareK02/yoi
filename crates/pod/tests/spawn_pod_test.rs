//! Integration tests for the `SpawnPod` tool.
//!
//! These tests exercise the tool's scope-lock delegation, subprocess
//! launch, socket handoff, and `spawned_pods.json` write without relying
//! on the real `pod` binary. `INSOMNIA_POD_COMMAND` is pointed at
//! `/bin/true` (which exits immediately) while a test-owned Unix
//! listener pre-binds the predicted socket path, so the tool sees the
//! "child" as live.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use llm_worker::tool::{ToolError, ToolOutput};
use manifest::{AuthRef, ModelConfig, Permission, SchemeKind, ScopeRule};
use pod::runtime_dir::{RuntimeDir, SpawnedPodRecord};
use pod::scope_lock::{self, LockFileGuard};
use pod::spawn_pod::spawn_pod_tool;
use pod::spawned_pod_registry::SpawnedPodRegistry;
use protocol::Method;
use protocol::stream::JsonLineReader;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::UnixListener;

/// Serialises tests that mutate `INSOMNIA_SCOPE_LOCK` /
/// `INSOMNIA_POD_COMMAND` across the thread-pooled test harness.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn acquire() -> Self {
        Self {
            _lock: ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// Set up a tempdir, point `INSOMNIA_SCOPE_LOCK` + runtime-dir base at
/// it, and install a live top-level "spawner" allocation so the tool
/// has something to delegate from. Returns the tempdir (keeps it alive
/// for the test's lifetime), runtime base, spawner socket, and the
/// spawner's runtime dir.
async fn setup_spawner(
    spawner_name: &str,
    allow_root: &Path,
) -> (TempDir, PathBuf, PathBuf, Arc<RuntimeDir>) {
    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("scope.lock");
    unsafe {
        std::env::set_var("INSOMNIA_SCOPE_LOCK", &lock_path);
    }

    let runtime_base = tmp.path().join("runtime");
    let spawner_rd = RuntimeDir::create(&runtime_base, spawner_name)
        .await
        .unwrap();
    let spawner_socket = spawner_rd.socket_path();

    let _guard = scope_lock::install_top_level(
        spawner_name.into(),
        std::process::id(),
        spawner_socket.clone(),
        vec![ScopeRule {
            target: allow_root.to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
    )
    .unwrap();
    // Leak the guard — the spawner allocation needs to outlive the
    // tool call. Dropping it would auto-release the allocation, which
    // defeats the point of the test.
    std::mem::forget(_guard);

    (tmp, runtime_base, spawner_socket, Arc::new(spawner_rd))
}

/// Bind a Unix listener at the path the tool will predict for the
/// spawned pod. The tool only needs the socket to accept a connection
/// and receive one `Method::Run` line; the returned `UnixListener` is
/// read from by the caller in a joined task.
async fn bind_mock_pod_socket(runtime_base: &Path, pod_name: &str) -> (PathBuf, UnixListener) {
    let dir = runtime_base.join(pod_name);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let socket = dir.join("sock");
    let listener = UnixListener::bind(&socket).unwrap();
    (socket, listener)
}

/// Launch a tokio task that accepts connections until one carries a
/// `Method` line, then returns it. `wait_for_socket` inside the tool
/// makes a probe connection that carries no data, so the task must
/// tolerate an empty connection and keep listening.
fn accept_one_method(
    listener: UnixListener,
) -> tokio::task::JoinHandle<Option<Method>> {
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.ok()?;
            let (reader, _writer) = stream.into_split();
            let mut r = JsonLineReader::new(reader);
            if let Ok(Some(method)) = r.next::<Method>().await {
                return Some(method);
            }
        }
    })
}

fn point_pod_command_at_true() {
    let path = which_true();
    unsafe {
        std::env::set_var("INSOMNIA_POD_COMMAND", &path);
    }
}

/// `/bin/true` only exists on FHS-compliant systems. On Nix, resolve it
/// via PATH so the tests work regardless of distro.
fn which_true() -> String {
    for dir in std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = dir.join("true");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "/bin/true".into()
}

/// Tests don't exercise the model — they intercept the spawned
/// child via a mock socket — but `spawn_pod_tool` needs a value to
/// embed in the overlay TOML. Any well-formed `ModelConfig` works.
fn dummy_model() -> ModelConfig {
    ModelConfig {
        scheme: SchemeKind::Anthropic,
        base_url: None,
        model_id: "claude-test".into(),
        auth: AuthRef::None,
        capability: None,
    }
}

fn clear_env() {
    unsafe {
        std::env::remove_var("INSOMNIA_SCOPE_LOCK");
        std::env::remove_var("INSOMNIA_POD_COMMAND");
    }
}

#[tokio::test]
async fn spawn_pod_delegates_scope_and_sends_run() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;
    point_pod_command_at_true();

    let (_predicted_socket, listener) = bind_mock_pod_socket(&runtime_base, "child").await;
    let received = accept_one_method(listener);

    let registry = SpawnedPodRegistry::new(spawner_rd.clone());
    let def = spawn_pod_tool(
        "root".into(),
        spawner_socket.clone(),
        runtime_base.clone(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_model(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child",
        "task": "hello",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let output: ToolOutput = tool.execute(&input).await.unwrap();
    assert!(output.summary.contains("child"), "summary: {}", output.summary);

    // Verify the tool delivered Method::Run to the socket.
    let method = received.await.unwrap().expect("expected one Method line");
    match method {
        Method::Run { input } => assert_eq!(input, "hello"),
        other => panic!("expected Run, got {other:?}"),
    }

    // Verify scope_lock has the child allocation under `root`.
    let lock_path = scope_lock::default_lock_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    let child = guard
        .data()
        .find("child")
        .expect("child allocation missing after spawn");
    assert_eq!(child.delegated_from.as_deref(), Some("root"));
    drop(guard);

    // Verify spawned_pods.json was written.
    let spawned_file = spawner_rd.path().join("spawned_pods.json");
    let contents = std::fs::read_to_string(&spawned_file).unwrap();
    let records: Vec<SpawnedPodRecord> = serde_json::from_str(&contents).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].pod_name, "child");
    assert_eq!(records[0].callback_address, spawner_socket);

    clear_env();
}

#[tokio::test]
async fn spawn_pod_rejects_scope_outside_spawner() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;
    point_pod_command_at_true();

    let registry = SpawnedPodRegistry::new(spawner_rd);
    let def = spawn_pod_tool(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_model(),
    );
    let (_meta, tool) = def();

    // Request write access to a path the spawner doesn't own.
    let input = json!({
        "name": "child",
        "task": "nope",
        "scope": [{
            "target": outside.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let err = tool.execute(&input).await.unwrap_err();
    match err {
        ToolError::InvalidArgument(msg) => {
            assert!(msg.contains("not within"), "expected NotSubset wording: {msg}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }

    // The spawner's allocation is unchanged; no "child" appeared.
    let lock_path = scope_lock::default_lock_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    assert!(guard.data().find("child").is_none());

    clear_env();
}

#[tokio::test]
async fn spawn_pod_rolls_back_reservation_when_socket_never_appears() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;
    point_pod_command_at_true();

    // Deliberately do NOT bind a socket at the predicted path. The
    // tool's wait_for_socket should time out, triggering rollback.
    // `SOCKET_WAIT_TIMEOUT` is 10s in production; we override via a
    // tighter env-based lock path and just accept the wait in test.
    // To keep the test fast, use a shorter wait by constructing a
    // short-lived separate instance.
    //
    // As the tool's timeout is internal, we accept the 10s wait here —
    // marked with `// slow_test`. Keep the rest of the test suite fast
    // by running this test alone when iterating.

    let registry = SpawnedPodRegistry::new(spawner_rd);
    let def = spawn_pod_tool(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_model(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "ghost",
        "task": "will never be delivered",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let err = tool.execute(&input).await.unwrap_err();
    match err {
        ToolError::ExecutionFailed(msg) => {
            assert!(
                msg.contains("socket did not appear"),
                "expected socket timeout wording: {msg}"
            );
        }
        other => panic!("expected ExecutionFailed, got {other:?}"),
    }

    // Rollback assertion: the reserved "ghost" allocation is gone.
    let lock_path = scope_lock::default_lock_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    assert!(
        guard.data().find("ghost").is_none(),
        "allocation was not rolled back after socket wait timed out"
    );

    clear_env();
}
