//! Integration tests for `Pod::restore_from_manifest`'s pre-build
//! validation paths.
//!
//! These cases all return before `prepare_pod_common` runs, so they
//! do not need a real LLM client or pod-registry environment — only the
//! session store needs to be present.

use std::sync::{LazyLock, Mutex};

use pod::{Pod, PodError};
use session_store::{FsStore, StoreError};

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
async fn restore_from_manifest_rejects_unknown_segment() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    // A freshly-minted id with no jsonl file at all → store returns
    // NotFound, which `Pod::restore_from_manifest` surfaces verbatim
    // as `PodError::Store`.
    let unknown_sid = session_store::new_session_id();
    let unknown_seg = session_store::new_segment_id();
    let result = Pod::restore_from_manifest(
        unknown_sid,
        unknown_seg,
        manifest,
        store,
        pod::PromptLoader::builtins_only(),
    )
    .await;

    match result {
        Err(PodError::Store(StoreError::NotFound(id))) => assert_eq!(id, unknown_seg),
        Err(other) => panic!("expected Store(NotFound), got {other:?}"),
        Ok(_) => panic!("expected unknown segment to fail"),
    }
}

#[tokio::test]
async fn restore_from_manifest_rejects_empty_segment_log() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    // Pre-create an empty `<sid>/<segid>.jsonl` so `read_all` succeeds
    // with no entries. `collect_state` returns `entries_count = 0`,
    // which `restore_from_manifest` rejects with `SegmentEmpty` *before*
    // it gets as far as building the LLM client.
    let sid = session_store::new_session_id();
    let segid = session_store::new_segment_id();
    let dir = store_tmp.path().join(sid.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{segid}.jsonl")), b"").unwrap();

    let result = Pod::restore_from_manifest(
        sid,
        segid,
        manifest,
        store,
        pod::PromptLoader::builtins_only(),
    )
    .await;

    match result {
        Err(PodError::SegmentEmpty { segment_id }) => assert_eq!(segment_id, segid),
        Err(other) => panic!("expected SegmentEmpty, got {other:?}"),
        Ok(_) => panic!("expected empty segment log to fail"),
    }
}

#[tokio::test]
async fn restore_from_manifest_rejects_segment_without_scope_snapshot() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).unwrap();
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    let sid = session_store::new_session_id();
    let segid = session_store::new_segment_id();
    let state = session_store::SegmentStartState {
        system_prompt: None,
        config: &Default::default(),
        history: &[],
    };
    session_store::create_segment_with_ids(&store, sid, segid, state).unwrap();

    let result = Pod::restore_from_manifest(
        sid,
        segid,
        manifest,
        store,
        pod::PromptLoader::builtins_only(),
    )
    .await;

    match result {
        Err(PodError::SegmentScopeMissing { segment_id }) => assert_eq!(segment_id, segid),
        Err(other) => panic!("expected SegmentScopeMissing, got {other:?}"),
        Ok(_) => panic!("expected missing scope snapshot to fail"),
    }
}
