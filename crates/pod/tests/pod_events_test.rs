//! Integration tests for the `PodEvent` send / receive primitive.
//!
//! These tests drive `pod_events::fire_and_forget` and
//! `pod_events::apply_event_side_effects` directly — the full
//! Controller wiring is exercised by the existing controller /
//! spawn-pod tests, which rely on the same primitives.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pod::ipc::event::{apply_event_side_effects, fire_and_forget, render_event};
use pod::runtime::dir::{RuntimeDir, SpawnedPodRecord};
use pod::runtime::pod_registry::{self, LockFileGuard};
use pod::spawn::registry::SpawnedPodRegistry;
use protocol::stream::JsonLineReader;
use protocol::{Method, Permission, PodEvent, ScopeRule};
use tempfile::TempDir;
use tokio::net::UnixListener;

/// Serialises tests that mutate `INSOMNIA_RUNTIME_DIR`.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Take `ENV_LOCK` and clear any env vars that would outrank
/// `INSOMNIA_RUNTIME_DIR`; restore previous values on drop.
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

/// Point `INSOMNIA_RUNTIME_DIR` at `dir`. The pod-registry then lives at
/// `<dir>/pods.json` and Pod runtime sub-dirs at `<dir>/{pod_name}/`.
fn set_runtime_dir(dir: &std::path::Path) {
    unsafe {
        std::env::set_var("INSOMNIA_RUNTIME_DIR", dir);
    }
}

fn clear_runtime_dir() {
    unsafe {
        std::env::remove_var("INSOMNIA_RUNTIME_DIR");
    }
}

/// Accept a single connection, read one `Method`, and return it.
fn accept_one_method(listener: UnixListener) -> tokio::task::JoinHandle<Option<Method>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.ok()?;
        let (reader, _writer) = stream.into_split();
        let mut r = JsonLineReader::new(reader);
        r.next::<Method>().await.ok().flatten()
    })
}

#[test]
fn render_event_all_variants_mention_pod_name() {
    let t1 = render_event(&PodEvent::TurnEnded {
        pod_name: "alpha".into(),
    });
    assert!(t1.contains("alpha"), "{t1}");

    let t2 = render_event(&PodEvent::Errored {
        pod_name: "bravo".into(),
        message: "boom".into(),
    });
    assert!(t2.contains("bravo") && t2.contains("boom"), "{t2}");

    let t3 = render_event(&PodEvent::ShutDown {
        pod_name: "charlie".into(),
    });
    assert!(t3.contains("charlie"), "{t3}");

    let t4 = render_event(&PodEvent::ScopeSubDelegated {
        parent_pod: "delta".into(),
        sub_pod: "echo".into(),
        sub_socket: "/tmp/sock".into(),
        scope: vec![],
    });
    assert!(t4.contains("delta") && t4.contains("echo"), "{t4}");
}

#[tokio::test]
async fn fire_and_forget_delivers_pod_event_to_listener() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("parent.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let received = accept_one_method(listener);

    fire_and_forget(
        Some(socket_path.clone()),
        PodEvent::TurnEnded {
            pod_name: "child".into(),
        },
    );

    let method = tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .expect("send timed out")
        .unwrap()
        .expect("no method received");
    match method {
        Method::PodEvent(PodEvent::TurnEnded { pod_name }) => assert_eq!(pod_name, "child"),
        other => panic!("expected TurnEnded, got {other:?}"),
    }
}

#[tokio::test]
async fn fire_and_forget_with_none_socket_is_noop() {
    // Nothing binds and nothing listens; the call must not panic and
    // must not leak a task that never completes.
    fire_and_forget(
        None,
        PodEvent::ShutDown {
            pod_name: "x".into(),
        },
    );
    // Yield once so any accidentally-spawned task would surface.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Build a registry backed by a fresh runtime dir.
async fn fresh_registry(runtime_base: &std::path::Path, pod_name: &str) -> Arc<SpawnedPodRegistry> {
    let rd = RuntimeDir::create(runtime_base, pod_name).await.unwrap();
    SpawnedPodRegistry::new(Arc::new(rd))
}

#[tokio::test]
async fn apply_shutdown_removes_from_registry_and_tolerates_missing() {
    let _env = EnvGuard::acquire();
    let scope_dir = TempDir::new().unwrap();
    set_runtime_dir(scope_dir.path());

    let runtime_base = TempDir::new().unwrap();
    let registry = fresh_registry(runtime_base.path(), "parent").await;

    // Seed a child record; then ShutDown for it should remove it.
    registry
        .add(SpawnedPodRecord {
            pod_name: "child".into(),
            socket_path: "/tmp/child.sock".into(),
            scope_delegated: vec![],
            callback_address: "/tmp/parent.sock".into(),
        })
        .await
        .unwrap();

    let event = PodEvent::ShutDown {
        pod_name: "child".into(),
    };
    apply_event_side_effects(&event, &registry, "parent", &None).await;
    assert!(registry.get("child").await.is_none());

    // Second ShutDown for the same (now-missing) child must be a no-op,
    // not an error — this is the idempotency guarantee for out-of-order
    // delivery.
    apply_event_side_effects(&event, &registry, "parent", &None).await;
    assert!(registry.get("child").await.is_none());

    clear_runtime_dir();
}

#[tokio::test]
async fn apply_scope_sub_delegated_adds_grandchild_then_duplicate_is_noop() {
    let _env = EnvGuard::acquire();
    let scope_dir = TempDir::new().unwrap();
    set_runtime_dir(scope_dir.path());

    let runtime_base = TempDir::new().unwrap();
    let registry = fresh_registry(runtime_base.path(), "grandparent").await;

    // Seed the intermediate child so callback_address lookup succeeds.
    registry
        .add(SpawnedPodRecord {
            pod_name: "child".into(),
            socket_path: "/tmp/child.sock".into(),
            scope_delegated: vec![],
            callback_address: "/tmp/grandparent.sock".into(),
        })
        .await
        .unwrap();

    let event = PodEvent::ScopeSubDelegated {
        parent_pod: "child".into(),
        sub_pod: "grandchild".into(),
        sub_socket: "/tmp/grandchild.sock".into(),
        scope: vec![ScopeRule {
            target: scope_dir.path().to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
    };

    apply_event_side_effects(&event, &registry, "grandparent", &None).await;
    let gc = registry
        .get("grandchild")
        .await
        .expect("grandchild missing after ScopeSubDelegated");
    assert_eq!(gc.socket_path, PathBuf::from("/tmp/grandchild.sock"));
    assert_eq!(gc.callback_address, PathBuf::from("/tmp/child.sock"));

    // Duplicate delivery must not error and must not overwrite.
    apply_event_side_effects(&event, &registry, "grandparent", &None).await;
    let gc2 = registry.get("grandchild").await.unwrap();
    assert_eq!(gc2.socket_path, PathBuf::from("/tmp/grandchild.sock"));

    clear_runtime_dir();
}

#[tokio::test]
async fn apply_scope_sub_delegated_reemits_to_own_parent() {
    let _env = EnvGuard::acquire();
    let scope_dir = TempDir::new().unwrap();
    set_runtime_dir(scope_dir.path());

    let runtime_base = TempDir::new().unwrap();
    let registry = fresh_registry(runtime_base.path(), "B").await;

    // Bind a listener at "A's" socket so we can watch the re-emission
    // climb one level up the tree.
    let sock_dir = TempDir::new().unwrap();
    let a_socket = sock_dir.path().join("A.sock");
    let listener = UnixListener::bind(&a_socket).unwrap();
    let received = accept_one_method(listener);

    // Seed the child record that the event claims spawned the grandchild.
    registry
        .add(SpawnedPodRecord {
            pod_name: "C".into(),
            socket_path: "/tmp/C.sock".into(),
            scope_delegated: vec![],
            callback_address: "/tmp/B.sock".into(),
        })
        .await
        .unwrap();

    let event = PodEvent::ScopeSubDelegated {
        parent_pod: "C".into(),
        sub_pod: "D".into(),
        sub_socket: "/tmp/D.sock".into(),
        scope: vec![],
    };

    // Self is B, and B's parent socket is A's listener.
    apply_event_side_effects(&event, &registry, "B", &Some(a_socket.clone())).await;

    // A must see the re-emission with parent_pod set to "B" (the
    // sender from A's perspective), not "C" (the original sender's
    // local view).
    let method = tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .expect("re-emission timed out")
        .unwrap()
        .expect("no method received on A's socket");
    match method {
        Method::PodEvent(PodEvent::ScopeSubDelegated {
            parent_pod,
            sub_pod,
            ..
        }) => {
            assert_eq!(parent_pod, "B");
            assert_eq!(sub_pod, "D");
        }
        other => panic!("expected re-emitted ScopeSubDelegated, got {other:?}"),
    }

    clear_runtime_dir();
}

#[tokio::test]
async fn apply_turn_ended_and_errored_are_system_noops() {
    let _env = EnvGuard::acquire();
    let scope_dir = TempDir::new().unwrap();
    set_runtime_dir(scope_dir.path());

    let runtime_base = TempDir::new().unwrap();
    let registry = fresh_registry(runtime_base.path(), "parent").await;

    // Seed a child to verify it survives the no-op path.
    registry
        .add(SpawnedPodRecord {
            pod_name: "child".into(),
            socket_path: "/tmp/child.sock".into(),
            scope_delegated: vec![],
            callback_address: "/tmp/parent.sock".into(),
        })
        .await
        .unwrap();

    apply_event_side_effects(
        &PodEvent::TurnEnded {
            pod_name: "child".into(),
        },
        &registry,
        "parent",
        &None,
    )
    .await;
    apply_event_side_effects(
        &PodEvent::Errored {
            pod_name: "child".into(),
            message: "x".into(),
        },
        &registry,
        "parent",
        &None,
    )
    .await;

    assert!(registry.get("child").await.is_some());
    clear_runtime_dir();
}

#[tokio::test]
async fn shutdown_releases_scope_allocation_when_present() {
    let _env = EnvGuard::acquire();
    let scope_dir = TempDir::new().unwrap();
    let lock_path = scope_dir.path().join("pods.json");
    set_runtime_dir(scope_dir.path());

    // Install a top-level allocation for "kid" so ShutDown has
    // something to release.
    let guard = pod_registry::install_top_level(
        "kid".into(),
        std::process::id(),
        "/tmp/kid.sock".into(),
        vec![],
        session_store::new_segment_id(),
    )
    .unwrap();
    std::mem::forget(guard);

    let runtime_base = TempDir::new().unwrap();
    let registry = fresh_registry(runtime_base.path(), "parent").await;
    registry
        .add(SpawnedPodRecord {
            pod_name: "kid".into(),
            socket_path: "/tmp/kid.sock".into(),
            scope_delegated: vec![],
            callback_address: "/tmp/parent.sock".into(),
        })
        .await
        .unwrap();

    apply_event_side_effects(
        &PodEvent::ShutDown {
            pod_name: "kid".into(),
        },
        &registry,
        "parent",
        &None,
    )
    .await;

    // Allocation is gone from the pod-registry.
    let g = LockFileGuard::open(&lock_path).unwrap();
    assert!(
        g.data().find("kid").is_none(),
        "ShutDown should have released the scope allocation"
    );

    clear_runtime_dir();
}
