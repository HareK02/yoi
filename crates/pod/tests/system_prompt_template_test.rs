use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::Stream;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use pod_store::{CombinedStore, FsPodStore};
use session_store::{FsStore, LogEntry, Store};

use pod::{Pod, PodError, PromptLoader, SystemPromptTemplate};

type TestStore = CombinedStore<FsStore, FsPodStore>;

// ---------------------------------------------------------------------------
// Mock LLM Client
// ---------------------------------------------------------------------------

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
        let idx = count.min(self.responses.len() - 1);
        let events = self.responses[idx].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

fn single_text_events(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, text),
        LlmEvent::text_block_stop(0, None),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

/// Emit a single `write_summary(text=...)` tool call as one LLM response.
fn write_summary_tool_use_events(call_id: &str, text: &str) -> Vec<LlmEvent> {
    let input = serde_json::json!({ "text": text }).to_string();
    vec![
        LlmEvent::tool_use_start(0, call_id, "write_summary"),
        LlmEvent::tool_input_delta(0, input),
        LlmEvent::tool_use_stop(0),
        LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

const MINIMAL_MANIFEST_TOML: &str = r#"
[pod]
name = "test-pod"
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

/// Build a Pod with a synthetic instruction template.
///
/// Writes `body` to a temp user-prompts dir under `$user/test`, builds a
/// PromptLoader pointing at it, parses the template, and installs it on
/// a Pod constructed directly via `Pod::new`.
async fn make_pod_with_body(
    body: &str,
    client: MockClient,
) -> Result<(Pod<MockClient, TestStore>, PathBuf), PodError> {
    let manifest = pod::PodManifest::from_toml(MINIMAL_MANIFEST_TOML).unwrap();

    let store_tmp = tempfile::tempdir().unwrap();
    let store = CombinedStore::new(
        FsStore::new(store_tmp.path()).unwrap(),
        FsPodStore::new(store_tmp.path().join("pods")).unwrap(),
    );
    std::mem::forget(store_tmp);

    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let user_prompts_tmp = tempfile::tempdir().unwrap();
    std::fs::write(user_prompts_tmp.path().join("test.md"), body).unwrap();
    let loader = PromptLoader::new(Some(user_prompts_tmp.path().to_path_buf()), None);
    std::mem::forget(user_prompts_tmp);

    let worker = Worker::new(client);
    let mut pod = Pod::new(manifest, worker, store, pwd.clone(), scope).await?;

    let template = SystemPromptTemplate::parse("$user/test", loader)
        .map_err(|source| PodError::InvalidSystemPromptTemplate { source })?;
    pod.set_system_prompt_template(template);

    Ok((pod, pwd))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_parse_rejects_invalid_syntax() {
    let user_prompts_tmp = tempfile::tempdir().unwrap();
    std::fs::write(user_prompts_tmp.path().join("broken.md"), "{{ unclosed").unwrap();
    let loader = PromptLoader::new(Some(user_prompts_tmp.path().to_path_buf()), None);
    let err = SystemPromptTemplate::parse("$user/broken", loader).unwrap_err();
    let pod_err: PodError = PodError::InvalidSystemPromptTemplate { source: err };
    assert!(matches!(
        pod_err,
        PodError::InvalidSystemPromptTemplate { .. }
    ));
}

#[tokio::test]
async fn template_is_not_materialised_before_first_run() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (pod, _pwd) = make_pod_with_body("hello", client).await.unwrap();
    // Before first run, worker still has no system prompt.
    assert!(pod.worker().get_system_prompt().is_none());
}

#[tokio::test]
async fn materialise_on_first_turn_populates_worker() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (mut pod, pwd) = make_pod_with_body(
        "date={{ date }} cwd={{ cwd }} tools={{ tools | join(',') }}",
        client,
    )
    .await
    .unwrap();
    pod.run_text("hi").await.unwrap();
    let rendered = pod
        .worker()
        .get_system_prompt()
        .expect("system prompt materialised")
        .to_string();
    assert!(rendered.contains("date="));
    assert!(rendered.contains("cwd="));
    assert!(rendered.contains(&pwd.display().to_string()));
    assert!(rendered.starts_with("date="));
    // Trailing fixed section must be appended.
    assert!(rendered.contains("## Working boundaries"));
}

#[tokio::test]
async fn session_start_state_captures_rendered_prompt() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (mut pod, pwd) = make_pod_with_body("hello cwd={{ cwd }}", client)
        .await
        .unwrap();
    pod.run_text("hi").await.unwrap();

    let entries = pod
        .store()
        .read_all(pod.session_id(), pod.segment_id())
        .unwrap();
    let first = entries.first().expect("at least one entry");
    match first {
        LogEntry::SegmentStart { system_prompt, .. } => {
            let sp = system_prompt.as_deref().expect("system prompt set");
            assert!(sp.starts_with("hello cwd="));
            assert!(sp.contains(&pwd.display().to_string()));
            assert!(sp.contains("## Working boundaries"));
        }
        other => panic!("expected SegmentStart as first entry, got {other:?}"),
    }
}

#[tokio::test]
async fn render_failure_propagates_as_pod_error() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (mut pod, _pwd) = make_pod_with_body("{{ ghost }}", client).await.unwrap();
    let err = pod.run_text("hi").await.unwrap_err();
    assert!(matches!(err, PodError::SystemPromptRender { .. }));
}

#[tokio::test]
async fn materialise_runs_only_once_across_turns() {
    let client = MockClient::new(vec![
        single_text_events("first"),
        single_text_events("second"),
    ]);
    let (mut pod, _pwd) = make_pod_with_body("fixed prompt {{ cwd }}", client)
        .await
        .unwrap();
    pod.run_text("one").await.unwrap();
    let first = pod.worker().get_system_prompt().unwrap().to_string();
    pod.run_text("two").await.unwrap();
    let second = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(first, second);
}

#[tokio::test]
async fn agents_md_is_injected_as_trailing_section_when_present() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (mut pod, pwd) = make_pod_with_body("BODY", client).await.unwrap();
    std::fs::write(pwd.join("AGENTS.md"), "# project rules\nbe kind").unwrap();

    pod.run_text("hi").await.unwrap();
    let rendered = pod.worker().get_system_prompt().unwrap().to_string();
    assert!(rendered.starts_with("BODY"));
    assert!(rendered.contains("## Project instructions (AGENTS.md)"));
    assert!(rendered.contains("# project rules"));
    assert!(rendered.contains("be kind"));
}

#[tokio::test]
async fn agents_md_absent_omits_trailing_section() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let (mut pod, _pwd) = make_pod_with_body("BODY", client).await.unwrap();
    pod.run_text("hi").await.unwrap();
    let rendered = pod.worker().get_system_prompt().unwrap().to_string();
    assert!(!rendered.contains("## Project instructions"));
    assert!(!rendered.contains("AGENTS.md"));
}

#[tokio::test]
async fn agents_md_not_reread_after_compact() {
    let client = MockClient::new(vec![
        single_text_events("a"), // pod.run_text("first")
        single_text_events("b"), // pod.run_text("second")
        write_summary_tool_use_events("call-1", "compacted summary"), // compact worker: tool_use
        single_text_events("done"), // compact worker: close
        single_text_events("c"), // pod.run_text("third")
    ]);
    let (mut pod, pwd) = make_pod_with_body("BODY", client).await.unwrap();
    let agents_path = pwd.join("AGENTS.md");
    std::fs::write(&agents_path, "original").unwrap();

    pod.run_text("first").await.unwrap();
    let before = pod.worker().get_system_prompt().unwrap().to_string();
    assert!(before.contains("original"));
    pod.run_text("second").await.unwrap();

    // Mutate the file after the first turn — must not affect the cached
    // system prompt either on a subsequent turn or across compaction.
    std::fs::write(&agents_path, "mutated").unwrap();
    pod.compact(0).await.unwrap();
    let after_compact = pod.worker().get_system_prompt().unwrap().to_string();
    assert!(after_compact.contains("original"));
    assert!(!after_compact.contains("mutated"));

    pod.run_text("third").await.unwrap();
    let after_third = pod.worker().get_system_prompt().unwrap().to_string();
    assert!(after_third.contains("original"));
    assert!(!after_third.contains("mutated"));
}

#[tokio::test]
async fn compact_aligns_user_segments_with_retained_history() {
    // retained_tokens=0 folds the entire conversation into the summary,
    // so retained_items has zero user_messages and self.user_segments
    // must be drained to match. A subsequent run() then appends fresh
    // segments cleanly without ghost entries from the pre-compaction era.
    let client = MockClient::new(vec![
        single_text_events("a"),
        single_text_events("b"),
        write_summary_tool_use_events("call-1", "compacted summary"),
        single_text_events("done"),
        single_text_events("c"),
    ]);
    let (mut pod, _pwd) = make_pod_with_body("BODY", client).await.unwrap();

    pod.run_text("first").await.unwrap();
    pod.run_text("second").await.unwrap();
    assert_eq!(pod.user_segments().len(), 2);

    pod.compact(0).await.unwrap();
    assert_eq!(
        pod.user_segments().len(),
        0,
        "compact(0) folds every user_message into the summary, so segments \
         must be drained to match retained_items"
    );

    pod.run_text("third").await.unwrap();
    assert_eq!(pod.user_segments().len(), 1);
}

#[tokio::test]
async fn compact_preserves_system_prompt() {
    let client = MockClient::new(vec![
        single_text_events("a"), // pod.run_text("first")
        single_text_events("b"), // pod.run_text("second")
        write_summary_tool_use_events("call-1", "compacted summary"), // compact worker: tool_use
        single_text_events("done"), // compact worker: close
        single_text_events("c"), // pod.run_text("third")
    ]);
    let (mut pod, _pwd) = make_pod_with_body("SP cwd={{ cwd }}", client)
        .await
        .unwrap();

    pod.run_text("first").await.unwrap();
    let before = pod.worker().get_system_prompt().unwrap().to_string();
    pod.run_text("second").await.unwrap();

    pod.compact(0).await.unwrap();

    let after = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(before, after);

    pod.run_text("third").await.unwrap();
    assert_eq!(pod.worker().get_system_prompt().unwrap(), after.as_str());
}
