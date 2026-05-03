//! Integration tests for `Pod::restore_from_manifest`'s pre-build
//! validation paths.
//!
//! These cases all return before `prepare_pod_common` runs, so they
//! do not need a real LLM client or pod-registry environment — only the
//! session store needs to be present.

use std::sync::{LazyLock, Mutex};

use pod::{Pod, PodError};
use session_store::{FsStore, SessionId, StoreError};

const MINIMAL_MANIFEST_TOML: &str = r#"
[pod]
name = "restore-test"
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

/// Serialises tests that mutate runtime-dir env vars, mirroring the
/// pattern used by other integration tests in this crate.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[tokio::test]
async fn restore_from_manifest_rejects_unknown_session() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    // A freshly-minted id with no jsonl file at all → store returns
    // NotFound, which `Pod::restore_from_manifest` surfaces verbatim
    // as `PodError::Store`.
    let unknown = session_store::new_session_id();
    let result =
        Pod::restore_from_manifest(unknown, manifest, store, pod::PromptLoader::builtins_only())
            .await;

    match result {
        Err(PodError::Store(StoreError::NotFound(id))) => assert_eq!(id, unknown),
        Err(other) => panic!("expected Store(NotFound), got {other:?}"),
        Ok(_) => panic!("expected unknown session to fail"),
    }
}

#[tokio::test]
async fn restore_from_manifest_rejects_empty_session_log() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    // Pre-create an empty `<id>.jsonl` so `read_all` succeeds with no
    // entries. `collect_state` returns `head_hash = None`, which
    // `restore_from_manifest` rejects with `SessionEmpty` *before* it
    // gets as far as building the LLM client — so the test does not
    // need credentials or a runtime sandbox.
    let id: SessionId = session_store::new_session_id();
    let path = store_tmp.path().join(format!("{id}.jsonl"));
    std::fs::write(&path, b"").unwrap();

    let result =
        Pod::restore_from_manifest(id, manifest, store, pod::PromptLoader::builtins_only()).await;

    match result {
        Err(PodError::SessionEmpty { session_id }) => assert_eq!(session_id, id),
        Err(other) => panic!("expected SessionEmpty, got {other:?}"),
        Ok(_) => panic!("expected empty session log to fail"),
    }
}

#[tokio::test]
async fn restore_from_manifest_rejects_session_without_scope_snapshot() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    let id = session_store::new_session_id();
    let state = session_store::SessionStartState {
        system_prompt: None,
        config: &Default::default(),
        history: &[],
    };
    session_store::create_session_with_id(&store, id, state)
        .await
        .unwrap();

    let result =
        Pod::restore_from_manifest(id, manifest, store, pod::PromptLoader::builtins_only()).await;

    match result {
        Err(PodError::SessionScopeMissing { session_id }) => assert_eq!(session_id, id),
        Err(other) => panic!("expected SessionScopeMissing, got {other:?}"),
        Ok(_) => panic!("expected missing scope snapshot to fail"),
    }
}
