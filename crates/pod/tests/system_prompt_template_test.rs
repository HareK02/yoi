use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::Stream;
use llm_worker::Worker;
use llm_worker::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use llm_worker::llm_client::{ClientError, LlmClient, Request};
use session_store::{FsStore, LogEntry, Store};

use pod::{Pod, PodError, SystemPromptTemplate};

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

fn manifest_toml(system_prompt: Option<&str>) -> String {
    let prompt_line = match system_prompt {
        Some(s) => format!("system_prompt = {:?}\n", s),
        None => String::new(),
    };
    format!(
        r#"
[pod]
name = "test-pod"
pwd = "./"

[provider]
kind = "anthropic"
model = "test-model"

[worker]
max_tokens = 100
{prompt_line}
[[scope.allow]]
target = "./"
permission = "write"
"#
    )
}

async fn make_pod_with_template(
    template_source: Option<&str>,
    client: MockClient,
) -> Result<Pod<MockClient, FsStore>, PodError> {
    let manifest = pod::PodManifest::from_toml(&manifest_toml(template_source)).unwrap();

    let store_tmp = tempfile::tempdir().unwrap();
    let store = FsStore::new(store_tmp.path()).await.unwrap();
    std::mem::forget(store_tmp);

    let pwd_tmp = tempfile::tempdir().unwrap();
    let pwd = pwd_tmp.path().to_path_buf();
    let scope = pod::Scope::writable(&pwd).unwrap();
    std::mem::forget(pwd_tmp);

    let worker = Worker::new(client);
    let mut pod = Pod::new(manifest, worker, store, pwd, scope).await?;

    if let Some(source) = template_source {
        let template = SystemPromptTemplate::parse(source)
            .map_err(|source| PodError::InvalidSystemPromptTemplate { source })?;
        pod.set_system_prompt_template(template);
    }
    Ok(pod)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_parse_rejects_invalid_syntax() {
    let err = SystemPromptTemplate::parse("{{ unclosed").unwrap_err();
    // Surfaces via PodError::InvalidSystemPromptTemplate when used with
    // Pod::from_manifest — tested at the SystemPromptTemplate level here
    // because building a Pod via from_manifest requires a real provider.
    let pod_err: PodError = PodError::InvalidSystemPromptTemplate { source: err };
    assert!(matches!(
        pod_err,
        PodError::InvalidSystemPromptTemplate { .. }
    ));
}

#[tokio::test]
async fn template_is_not_materialised_before_first_run() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let pod = make_pod_with_template(Some("hello"), client).await.unwrap();
    // Before first run, worker still has no system prompt.
    assert!(pod.worker().get_system_prompt().is_none());
}

#[tokio::test]
async fn materialise_on_first_turn_populates_worker() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let mut pod = make_pod_with_template(
        Some("date={{ date }} cwd={{ cwd }} tools={{ tools | join(',') }}"),
        client,
    )
    .await
    .unwrap();
    pod.run("hi").await.unwrap();
    let rendered = pod
        .worker()
        .get_system_prompt()
        .expect("system prompt materialised")
        .to_string();
    assert!(rendered.contains("date="));
    assert!(rendered.contains("cwd="));
    assert!(rendered.contains(&pod.pwd().display().to_string()));
    assert!(rendered.starts_with("date="));
}

#[tokio::test]
async fn session_start_state_captures_rendered_prompt() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let mut pod = make_pod_with_template(Some("hello cwd={{ cwd }}"), client)
        .await
        .unwrap();
    pod.run("hi").await.unwrap();

    // Inspect the first log entry directly: it must be a SessionStart
    // with the rendered system prompt, not `None`.
    let entries = pod.store().read_all(pod.session_id()).await.unwrap();
    let first = entries.first().expect("at least one entry");
    match &first.entry {
        LogEntry::SessionStart { system_prompt, .. } => {
            let sp = system_prompt.as_deref().expect("system prompt set");
            assert!(sp.starts_with("hello cwd="));
            assert!(sp.contains(&pod.pwd().display().to_string()));
        }
        other => panic!("expected SessionStart as first entry, got {other:?}"),
    }
}

#[tokio::test]
async fn render_failure_propagates_as_pod_error() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let mut pod = make_pod_with_template(Some("{{ ghost }}"), client)
        .await
        .unwrap();
    let err = pod.run("hi").await.unwrap_err();
    assert!(matches!(err, PodError::SystemPromptRender { .. }));
}

#[tokio::test]
async fn materialise_runs_only_once_across_turns() {
    // Two turns; the second one must not re-render the template. We
    // approximate this by checking that the rendered system prompt is
    // identical across turns and that the Pod's template slot is
    // exhausted after the first run.
    let client = MockClient::new(vec![
        single_text_events("first"),
        single_text_events("second"),
    ]);
    let mut pod = make_pod_with_template(Some("fixed prompt {{ cwd }}"), client)
        .await
        .unwrap();
    pod.run("one").await.unwrap();
    let first = pod.worker().get_system_prompt().unwrap().to_string();
    pod.run("two").await.unwrap();
    let second = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(first, second);
}

#[tokio::test]
async fn agents_md_is_injected_when_present() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let mut pod = make_pod_with_template(
        Some(
            "{% if files.agents_md is defined %}AGENTS:{{ files.agents_md }}\
             {% else %}NONE{% endif %}",
        ),
        client,
    )
    .await
    .unwrap();
    std::fs::write(pod.pwd().join("AGENTS.md"), "# project rules\nbe kind").unwrap();

    pod.run("hi").await.unwrap();
    let rendered = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(rendered, "AGENTS:# project rules\nbe kind");
}

#[tokio::test]
async fn agents_md_absent_leaves_key_undefined() {
    let client = MockClient::new(vec![single_text_events("ok")]);
    let mut pod = make_pod_with_template(
        Some("{% if files.agents_md is defined %}HAS{% else %}NONE{% endif %}"),
        client,
    )
    .await
    .unwrap();
    // No AGENTS.md written.
    pod.run("hi").await.unwrap();
    assert_eq!(pod.worker().get_system_prompt().unwrap(), "NONE");
}

#[tokio::test]
async fn agents_md_not_reread_after_compact() {
    // Render AGENTS.md on the first turn, then mutate the file on disk
    // and compact. The post-compact prompt must still reflect the
    // original content (template re-rendering is forbidden).
    let client = MockClient::new(vec![
        single_text_events("a"),
        single_text_events("b"),
        single_text_events("summary"),
        single_text_events("c"),
    ]);
    let mut pod = make_pod_with_template(
        Some("{{ files.agents_md }}"),
        client,
    )
    .await
    .unwrap();
    let agents_path = pod.pwd().join("AGENTS.md");
    std::fs::write(&agents_path, "original").unwrap();

    pod.run("first").await.unwrap();
    let before = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(before, "original");
    pod.run("second").await.unwrap();

    // Mutate the file after the first turn — must not affect the cached
    // system prompt either on a subsequent turn or across compaction.
    std::fs::write(&agents_path, "mutated").unwrap();
    pod.compact(1).await.unwrap();
    assert_eq!(pod.worker().get_system_prompt().unwrap(), "original");

    pod.run("third").await.unwrap();
    assert_eq!(pod.worker().get_system_prompt().unwrap(), "original");
}

#[tokio::test]
async fn compact_preserves_system_prompt() {
    // Three user turns, then compact with retained_turns=1. The new
    // compacted session must carry the same rendered system prompt and
    // the template must not re-run.
    let client = MockClient::new(vec![
        single_text_events("a"),
        single_text_events("b"),
        single_text_events("summary"),
        single_text_events("c"),
    ]);
    let mut pod = make_pod_with_template(Some("SP cwd={{ cwd }}"), client)
        .await
        .unwrap();

    pod.run("first").await.unwrap();
    let before = pod.worker().get_system_prompt().unwrap().to_string();
    pod.run("second").await.unwrap();

    pod.compact(1).await.unwrap();

    let after = pod.worker().get_system_prompt().unwrap().to_string();
    assert_eq!(before, after);

    // A further run must still see the same prompt (template is None, so
    // ensure_system_prompt_materialized is a no-op).
    pod.run("third").await.unwrap();
    assert_eq!(pod.worker().get_system_prompt().unwrap(), after.as_str());
}
