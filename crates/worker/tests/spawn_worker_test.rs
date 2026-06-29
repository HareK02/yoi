//! Integration tests for the `SpawnWorker` tool.
//!
//! These tests exercise the tool's worker-allocation delegation, subprocess
//! launch, socket handoff, and `spawned_workers.json` write through an injected
//! typed runtime command. The mock command exits immediately while a
//! test-owned Unix listener pre-binds the predicted socket path, so the tool
//! sees the "child" as live.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use client::WorkerRuntimeCommand;
use llm_engine::tool::{ToolError, ToolOutput};
use manifest::{
    AuthRef, ModelManifest, Permission, SchemeKind, Scope, ScopeConfig, ScopeRule, SharedScope,
    WorkerManifest, WorkerManifestConfig, WorkerMetaConfig,
};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{Event, Method};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::UnixListener;
use worker::runtime::dir::{RuntimeDir, SpawnedWorkerRecord};
use worker::runtime::worker_allocation::{self, LockFileGuard};
use worker::spawn::registry::SpawnedWorkerRegistry;
use worker::spawn::tool::spawn_worker_tool_with_runtime_command;

/// Serialises tests that mutate `YOI_RUNTIME_DIR` across the
/// thread-pooled test harness.
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

/// Set up a tempdir, point `YOI_RUNTIME_DIR` at it (so
/// `workers.json` and per-Worker runtime subdirs both land in the
/// sandbox), and install a live top-level "spawner" allocation so the
/// tool has something to delegate from. Returns the tempdir (keeps it
/// alive for the test's lifetime), runtime base, spawner socket, and
/// the spawner's runtime dir.
async fn setup_spawner(
    spawner_name: &str,
    allow_root: &Path,
) -> (TempDir, PathBuf, PathBuf, Arc<RuntimeDir>) {
    let tmp = TempDir::new().unwrap();
    let runtime_base = tmp.path().to_path_buf();
    unsafe {
        // Outranking env vars must be cleared so `paths::runtime_dir`
        // resolves to our sandbox instead of the developer's real one.
        std::env::remove_var("YOI_HOME");
        std::env::remove_var("XDG_RUNTIME_DIR");
        std::env::set_var("YOI_RUNTIME_DIR", &runtime_base);
    }

    let spawner_rd = RuntimeDir::create(&runtime_base, spawner_name)
        .await
        .unwrap();
    let spawner_socket = spawner_rd.socket_path();

    let _guard = worker_allocation::install_top_level(
        spawner_name.into(),
        std::process::id(),
        spawner_socket.clone(),
        vec![ScopeRule {
            target: allow_root.to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
        session_store::new_segment_id(),
    )
    .unwrap();
    // Leak the guard — the spawner allocation needs to outlive the
    // tool call. Dropping it would auto-release the allocation, which
    // defeats the point of the test.
    std::mem::forget(_guard);

    (tmp, runtime_base, spawner_socket, Arc::new(spawner_rd))
}

/// Bind a Unix listener at the path the tool will predict for the
/// spawned worker. The tool only needs the socket to accept a connection
/// and receive one `Method::Run` line; the returned `UnixListener` is
/// read from by the caller in a joined task.
async fn bind_mock_worker_socket(
    runtime_base: &Path,
    worker_name: &str,
) -> (PathBuf, UnixListener) {
    let dir = runtime_base.join(worker_name);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let socket = dir.join("sock");
    let listener = UnixListener::bind(&socket).unwrap();
    (socket, listener)
}

/// Launch a tokio task that accepts connections until one carries a
/// `Method` line, then acknowledges it and returns it. `wait_for_socket`
/// inside the tool makes a probe connection that carries no data, so the
/// task must tolerate an empty connection and keep listening.
fn accept_one_method(listener: UnixListener) -> tokio::task::JoinHandle<Option<Method>> {
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.ok()?;
            let (reader, writer) = stream.into_split();
            let mut r = JsonLineReader::new(reader);
            let mut w = JsonLineWriter::new(writer);
            if w.write(&Event::Snapshot {
                entries: Vec::new(),
                greeting: protocol::Greeting {
                    worker_name: "child".into(),
                    cwd: "/tmp".into(),
                    provider: "test".into(),
                    model: "test".into(),
                    scope_summary: String::new(),
                    tools: Vec::new(),
                    context_window: 200_000,
                    context_tokens: 0,
                },
                status: protocol::WorkerStatus::Idle,
                in_flight: Default::default(),
            })
            .await
            .is_err()
            {
                continue;
            }
            if let Ok(Some(method)) = r.next::<Method>().await {
                w.write(&Event::UserMessage {
                    segments: vec![protocol::Segment::text("accepted")],
                })
                .await
                .ok()?;
                return Some(method);
            }
        }
    })
}

fn mock_runtime_command() -> WorkerRuntimeCommand {
    WorkerRuntimeCommand::new(which_true(), Vec::new())
}

fn cwd_recording_runtime_command(script_path: &Path, output_path: &Path) -> WorkerRuntimeCommand {
    let output = output_path.display();
    std::fs::write(
        script_path,
        format!(
            "tmp=\"{output}.tmp\"\npwd > \"$tmp\"\nprintf '%s\\n' \"$@\" >> \"$tmp\"\nmv \"$tmp\" \"{output}\"\n"
        ),
    )
    .unwrap();
    WorkerRuntimeCommand::new(which_sh(), vec![script_path.as_os_str().to_os_string()])
}

async fn read_recorded_runtime_invocation(output_path: &Path) -> Vec<String> {
    for _ in 0..50 {
        if let Ok(content) = std::fs::read_to_string(output_path) {
            return content.lines().map(str::to_owned).collect();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "runtime command did not record invocation at {}",
        output_path.display()
    );
}

/// `/bin/true` only exists on FHS-compliant systems. Resolve it via PATH
/// so the tests work regardless of distro.
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

fn which_sh() -> String {
    for dir in std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = dir.join("sh");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "/bin/sh".into()
}

/// Tests don't exercise the model — they intercept the spawned
/// child via a mock socket — but `spawn_worker_tool` needs a value to
/// embed in the overlay TOML. Any well-formed `ModelManifest` works.
fn dummy_model() -> ModelManifest {
    ModelManifest {
        scheme: Some(SchemeKind::Anthropic),
        base_url: None,
        model_id: Some("claude-test".into()),
        auth: Some(AuthRef::None),
        capability: None,
        ..Default::default()
    }
}

fn dummy_manifest(allow_root: &Path) -> WorkerManifest {
    dummy_manifest_with_delegation(allow_root, true)
}

fn dummy_manifest_with_delegation(allow_root: &Path, allow_delegation: bool) -> WorkerManifest {
    let direct_scope = ScopeConfig {
        allow: vec![ScopeRule {
            target: allow_root.to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
        deny: Vec::new(),
    };
    let delegation_scope = if allow_delegation {
        direct_scope.clone()
    } else {
        ScopeConfig::default()
    };
    dummy_manifest_with_scopes(direct_scope, delegation_scope)
}

fn dummy_manifest_with_scopes(
    direct_scope: ScopeConfig,
    delegation_scope: ScopeConfig,
) -> WorkerManifest {
    WorkerManifestConfig {
        worker: WorkerMetaConfig {
            name: Some("root".into()),
            prompt_pack: None,
        },
        model: dummy_model(),
        scope: direct_scope,
        delegation_scope,
        ..Default::default()
    }
    .try_into()
    .unwrap()
}

fn builtin_prompts() -> Arc<worker::PromptCatalog> {
    worker::PromptCatalog::builtins_only().unwrap()
}

/// Spawner-side `SharedScope` mirroring the `allow_root` granted by
/// `setup_spawner`. The tool revokes Write rules from this scope on
/// successful spawn — tests can `load()` it to assert the
/// revocation took effect.
fn shared_scope_for(allow_root: &Path) -> SharedScope {
    SharedScope::new(Scope::writable(allow_root).unwrap())
}

fn clear_env() {
    unsafe {
        std::env::remove_var("YOI_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn spawn_worker_launches_runtime_in_workspace_and_process_cwd() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let child_cwd = allow_root.path().join("child-cwd");
    std::fs::create_dir(&child_cwd).unwrap();
    let script = allow_root.path().join("record-pwd.sh");
    let output_path = allow_root.path().join("pwd.txt");
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let (_predicted_socket, listener) = bind_mock_worker_socket(&runtime_base, "child-cwd").await;
    let received = accept_one_method(listener);

    let registry = SpawnedWorkerRegistry::new(spawner_rd);
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_manifest(allow_root.path()),
        shared_scope_for(allow_root.path()),
        builtin_prompts(),
        cwd_recording_runtime_command(&script, &output_path),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child-cwd",
        "task": "hello",
        "profile": "inherit",
        "cwd": child_cwd.to_str().unwrap(),
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    tool.execute(&input, Default::default()).await.unwrap();
    assert!(matches!(received.await.unwrap(), Some(Method::Run { .. })));
    let invocation = read_recorded_runtime_invocation(&output_path).await;
    assert_eq!(invocation[0], child_cwd.to_str().unwrap());
    assert!(
        invocation
            .windows(2)
            .any(|pair| pair[0] == "--workspace" && pair[1] == allow_root.path().to_str().unwrap()),
        "invocation should carry inherited workspace root: {invocation:?}"
    );
    assert!(
        !invocation.iter().any(|arg| arg == "--tool-cwd"),
        "cwd should be process current directory, not a runtime argument: {invocation:?}"
    );

    clear_env();
}

#[tokio::test]
async fn spawn_worker_omitted_cwd_preserves_spawner_cwd() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let script = allow_root.path().join("record-pwd.sh");
    let output_path = allow_root.path().join("pwd.txt");
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let (_predicted_socket, listener) =
        bind_mock_worker_socket(&runtime_base, "child-default-cwd").await;
    let received = accept_one_method(listener);

    let registry = SpawnedWorkerRegistry::new(spawner_rd);
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_manifest(allow_root.path()),
        shared_scope_for(allow_root.path()),
        builtin_prompts(),
        cwd_recording_runtime_command(&script, &output_path),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child-default-cwd",
        "task": "hello",
        "profile": "inherit",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    tool.execute(&input, Default::default()).await.unwrap();
    assert!(matches!(received.await.unwrap(), Some(Method::Run { .. })));
    let invocation = read_recorded_runtime_invocation(&output_path).await;
    assert_eq!(invocation[0], allow_root.path().to_str().unwrap());
    assert!(
        !invocation.iter().any(|arg| arg == "--tool-cwd"),
        "omitted cwd should preserve spawner cwd as process cwd: {invocation:?}"
    );

    clear_env();
}

#[tokio::test]
async fn spawn_worker_delegates_scope_and_sends_run() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let (_predicted_socket, listener) = bind_mock_worker_socket(&runtime_base, "child").await;
    let received = accept_one_method(listener);

    let registry = SpawnedWorkerRegistry::new(spawner_rd.clone());
    let spawner_scope = shared_scope_for(allow_root.path());
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket.clone(),
        runtime_base.clone(),
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_manifest(allow_root.path()),
        spawner_scope.clone(),
        builtin_prompts(),
        mock_runtime_command(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child",
        "task": "hello",
        "profile": "inherit",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    // Pre-spawn: the spawner can write to the delegated path.
    assert!(
        spawner_scope
            .load()
            .is_writable(&allow_root.path().join("a.txt"))
    );

    let output: ToolOutput = tool.execute(&input, Default::default()).await.unwrap();
    assert!(
        output.summary.contains("child"),
        "summary: {}",
        output.summary
    );

    // Verify the tool delivered Method::Run to the socket.
    let method = received.await.unwrap().expect("expected one Method line");
    match method {
        Method::Run { input } => match input.as_slice() {
            [protocol::Segment::Text { content }] => assert_eq!(content, "hello"),
            other => panic!("expected single Text segment, got {other:?}"),
        },
        other => panic!("expected Run, got {other:?}"),
    }

    // Verify worker_allocation has the child allocation under `root`.
    let lock_path = worker_allocation::default_allocation_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    let child = guard
        .data()
        .find("child")
        .expect("child allocation missing after spawn");
    assert_eq!(child.delegated_from.as_deref(), Some("root"));
    drop(guard);

    // Verify spawned_workers.json was written.
    let spawned_file = spawner_rd.path().join("spawned_workers.json");
    let contents = std::fs::read_to_string(&spawned_file).unwrap();
    let records: Vec<SpawnedWorkerRecord> = serde_json::from_str(&contents).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].worker_name, "child");
    assert_eq!(records[0].callback_address, spawner_socket);

    // Post-spawn: the spawner's runtime scope has been demoted on the
    // delegated path. Write is gone, Read remains.
    let post = spawner_scope.load();
    assert_eq!(
        post.permission_at(&allow_root.path().join("a.txt")),
        Some(Permission::Read),
        "spawner should still be able to read delegated path"
    );

    clear_env();
}

#[tokio::test]
async fn spawn_worker_requires_explicit_delegation_even_with_direct_scope() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let manifest = dummy_manifest_with_delegation(allow_root.path(), false);
    let direct = Scope::from_config(&manifest.scope).unwrap();
    assert!(direct.is_writable(&allow_root.path().join("direct.txt")));

    let registry = SpawnedWorkerRegistry::new(spawner_rd.clone());
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        manifest,
        shared_scope_for(allow_root.path()),
        builtin_prompts(),
        mock_runtime_command(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child-no-delegation",
        "task": "hello",
        "profile": "inherit",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let err = tool.execute(&input, Default::default()).await.unwrap_err();
    match err {
        ToolError::InvalidArgument(message) => {
            assert!(message.contains("no delegation scope grant"), "{message}");
            assert!(message.contains("direct filesystem scope"), "{message}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }

    clear_env();
}

#[tokio::test]
async fn spawn_worker_rejects_child_non_recursive_scope_under_parent_non_recursive_delegation() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let child = allow_root.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let direct_scope = ScopeConfig {
        allow: vec![ScopeRule {
            target: allow_root.path().to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
        deny: Vec::new(),
    };
    let delegation_scope = ScopeConfig {
        allow: vec![ScopeRule {
            target: allow_root.path().to_path_buf(),
            permission: Permission::Write,
            recursive: false,
        }],
        deny: Vec::new(),
    };
    let manifest = dummy_manifest_with_scopes(direct_scope, delegation_scope);

    let registry = SpawnedWorkerRegistry::new(spawner_rd.clone());
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        manifest,
        shared_scope_for(allow_root.path()),
        builtin_prompts(),
        mock_runtime_command(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "child-nonrecursive-overgrant",
        "task": "hello",
        "profile": "inherit",
        "scope": [{
            "target": child.to_str().unwrap(),
            "permission": "write",
            "recursive": false
        }]
    })
    .to_string();

    let err = tool.execute(&input, Default::default()).await.unwrap_err();
    match err {
        ToolError::InvalidArgument(message) => {
            assert!(
                message.contains("outside this Worker's delegation scope grant"),
                "{message}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }

    clear_env();
}

#[tokio::test]
async fn spawn_worker_rejects_scope_outside_spawner() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

    let registry = SpawnedWorkerRegistry::new(spawner_rd);
    let spawner_scope = shared_scope_for(allow_root.path());
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_manifest(allow_root.path()),
        spawner_scope.clone(),
        builtin_prompts(),
        mock_runtime_command(),
    );
    let (_meta, tool) = def();

    // Request write access to a path the spawner doesn't own.
    let input = json!({
        "name": "child",
        "task": "nope",
        "profile": "inherit",
        "scope": [{
            "target": outside.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let err = tool.execute(&input, Default::default()).await.unwrap_err();
    match err {
        ToolError::InvalidArgument(msg) => {
            assert!(
                msg.contains("outside this Worker's delegation scope grant"),
                "expected delegation-scope wording: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }

    // The spawner's allocation is unchanged; no "child" appeared.
    let lock_path = worker_allocation::default_allocation_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    assert!(guard.data().find("child").is_none());

    // Failed spawn must not have demoted the spawner's scope either.
    assert!(
        spawner_scope
            .load()
            .is_writable(&allow_root.path().join("a.txt"))
    );

    clear_env();
}

#[tokio::test]
async fn spawn_worker_rolls_back_reservation_when_socket_never_appears() {
    let _env = EnvGuard::acquire();

    let allow_root = TempDir::new().unwrap();
    let (_tmp, runtime_base, spawner_socket, spawner_rd) =
        setup_spawner("root", allow_root.path()).await;

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

    let registry = SpawnedWorkerRegistry::new(spawner_rd);
    let spawner_scope = shared_scope_for(allow_root.path());
    let def = spawn_worker_tool_with_runtime_command(
        "root".into(),
        spawner_socket,
        runtime_base,
        allow_root.path().to_path_buf(),
        allow_root.path().to_path_buf(),
        registry,
        None,
        dummy_manifest(allow_root.path()),
        spawner_scope.clone(),
        builtin_prompts(),
        mock_runtime_command(),
    );
    let (_meta, tool) = def();

    let input = json!({
        "name": "ghost",
        "task": "will never be delivered",
        "profile": "inherit",
        "scope": [{
            "target": allow_root.path().to_str().unwrap(),
            "permission": "write"
        }]
    })
    .to_string();

    let err = tool.execute(&input, Default::default()).await.unwrap_err();
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
    let lock_path = worker_allocation::default_allocation_path().unwrap();
    let guard = LockFileGuard::open(&lock_path).unwrap();
    assert!(
        guard.data().find("ghost").is_none(),
        "allocation was not rolled back after socket wait timed out"
    );

    // Spawner's runtime scope must also be untouched — revoke is
    // performed only after exec_child succeeds.
    assert!(
        spawner_scope
            .load()
            .is_writable(&allow_root.path().join("a.txt"))
    );

    clear_env();
}
