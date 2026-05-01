//! Phase 2 (memory.consolidation) post-run trigger.
//!
//! Covers the gating, lock and cleanup behaviour without exercising the
//! full sub-worker tool loop:
//!
//! - no `[memory]` section → no-op
//! - `[memory]` present but no thresholds → no-op
//! - staging empty → skip
//! - staging below thresholds → skip + lock not acquired
//! - staging above threshold → sub-worker runs, consumed entries removed
//! - existing live lock → skip without error
//!
//! The sub-worker is fed a no-op LLM response (plain text) so it returns
//! immediately. The post-run path then exercises lock acquisition,
//! cleanup, and the empty-payload fast path.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::Stream;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use memory::WorkspaceLayout;
use memory::extract::{ExtractedPayload, write_staging};
use memory::schema::SourceRef;
use session_store::FsStore;

use pod::Pod;

#[derive(Clone)]
struct MockClient {
    responses: Arc<Vec<Vec<LlmEvent>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for MockClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Config("mock client exhausted".into()));
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

fn done(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, text),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

const NO_MEMORY_TOML: &str = r#"
[pod]
name = "test-pod"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[[scope.allow]]
target = "./"
permission = "write"
"#;

const MEMORY_NO_THRESHOLDS_TOML: &str = r#"
[pod]
name = "test-pod"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[memory]

[[scope.allow]]
target = "./"
permission = "write"
"#;

const FILES_THRESHOLD_TOML: &str = r#"
[pod]
name = "test-pod"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]
max_tokens = 100

[memory]
consolidation_threshold_files = 2

[[scope.allow]]
target = "./"
permission = "write"
"#;

async fn make_pod_with(
    manifest_toml: &str,
    pwd: std::path::PathBuf,
    client: MockClient,
) -> Pod<MockClient, FsStore> {
    let manifest = pod::PodManifest::from_toml(manifest_toml).unwrap();

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    std::mem::forget(store_tmp);

    let scope = pod::Scope::writable(&pwd).unwrap();
    let worker = Worker::new(client);
    Pod::new(manifest, worker, store, pwd, scope).await.unwrap()
}

fn write_n_staging(layout: &WorkspaceLayout, n: usize) -> Vec<uuid::Uuid> {
    let mut ids = Vec::new();
    for i in 0..n {
        let (id, _) = write_staging(
            layout,
            SourceRef {
                session_id: format!("s-{i}"),
                range: [i as u64, i as u64],
            },
            ExtractedPayload::default(),
        )
        .unwrap();
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn no_memory_section_is_a_noop() {
    let pwd = tempfile::tempdir().unwrap();
    let client = MockClient::new(vec![]);
    let mut pod = make_pod_with(NO_MEMORY_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate()
        .await
        .expect("missing memory section must skip cleanly");
}

#[tokio::test]
async fn no_thresholds_is_a_noop() {
    let pwd = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(pwd.path().to_path_buf());
    write_n_staging(&layout, 5);

    let client = MockClient::new(vec![]);
    let mut pod = make_pod_with(MEMORY_NO_THRESHOLDS_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate()
        .await
        .expect("phase 2 disabled when both thresholds are None");

    // No staging entries removed.
    assert_eq!(
        memory::consolidate::list_staging_entries(&layout).len(),
        5
    );
}

#[tokio::test]
async fn empty_staging_skips() {
    let pwd = tempfile::tempdir().unwrap();
    let client = MockClient::new(vec![]);
    let mut pod = make_pod_with(FILES_THRESHOLD_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate().await.unwrap();
    // No mock calls expected.
}

#[tokio::test]
async fn below_threshold_skips_and_does_not_take_lock() {
    let pwd = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(pwd.path().to_path_buf());
    write_n_staging(&layout, 1); // threshold is 2

    let client = MockClient::new(vec![]);
    let mut pod = make_pod_with(FILES_THRESHOLD_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate().await.unwrap();

    // Staging untouched.
    assert_eq!(
        memory::consolidate::list_staging_entries(&layout).len(),
        1
    );
    // Lock file must not exist.
    let lock_path = layout.staging_dir().join(".consolidation.lock");
    assert!(!lock_path.exists(), "lock file should not be created");
}

#[tokio::test]
async fn fires_on_threshold_and_cleans_up_consumed_entries() {
    let pwd = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(pwd.path().to_path_buf());
    write_n_staging(&layout, 2); // threshold is 2 — fires.

    // Sub-worker is given a single text-only response. The Phase 2 prompt
    // tells it to call memory tools; the mock skips those, but `Worker::run`
    // returns Ok regardless once the LLM closes with a final text.
    let client = MockClient::new(vec![done("ok")]);
    let mut pod = make_pod_with(FILES_THRESHOLD_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate().await.unwrap();

    // Consumed entries removed.
    assert!(
        memory::consolidate::list_staging_entries(&layout).is_empty(),
        "consumed staging entries must be cleaned up"
    );
    // Lock removed too.
    let lock_path = layout.staging_dir().join(".consolidation.lock");
    assert!(
        !lock_path.exists(),
        "lock file must be removed on success"
    );
}

#[tokio::test]
async fn live_lock_held_by_other_pod_skips() {
    let pwd = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(pwd.path().to_path_buf());
    write_n_staging(&layout, 3);

    // Pre-acquire lock with this test's PID — definitely alive — and
    // *don't* release it. The Phase 2 path must skip without error.
    let _live_lock = memory::consolidate::StagingLock::acquire(
        &layout,
        std::process::id(),
        "other-pod",
        Vec::new(),
    )
    .unwrap();

    let client = MockClient::new(vec![]);
    let mut pod = make_pod_with(FILES_THRESHOLD_TOML, pwd.path().to_path_buf(), client).await;
    pod.try_post_run_consolidate()
        .await
        .expect("InUse lock must surface as graceful skip");

    // Staging untouched: lock holder owns the snapshot, not us.
    assert_eq!(
        memory::consolidate::list_staging_entries(&layout).len(),
        3
    );
}
