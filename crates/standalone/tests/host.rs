use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::client::LlmClient;
use agen::llm_client::error::ClientError;
use agen::llm_client::event::{Event as LlmEvent, StopReason};
use agen::llm_client::types::Request;
use async_trait::async_trait;
use futures::{Stream, stream};
use protocol::{Event, Method};
use standalone::{StandaloneHost, StandaloneLaunchConfig};
use uuid::Uuid;

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<VecDeque<Vec<LlmEvent>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl ScriptedClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.requests.lock().expect("requests lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .expect("scripted response");
        Ok(Box::pin(stream::iter(response.into_iter().map(Ok))))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

#[tokio::test]
async fn in_process_host_runs_text_and_read_tool_then_shuts_down() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("probe.txt"), "standalone tool evidence\n")
        .expect("write probe");
    let worker_name = format!("standalone-{}", Uuid::now_v7());
    let launch = StandaloneLaunchConfig::new(
        temp.path(),
        temp.path().join("state"),
        manifest::ProfileSelector::Default,
        &worker_name,
    )
    .resolve()
    .expect("resolve standalone profile");

    let client = ScriptedClient::new(vec![
        vec![
            LlmEvent::tool_use_start(0, "read-1", "Read"),
            LlmEvent::tool_input_delta(0, r#"{"file_path":"probe.txt"}"#),
            LlmEvent::tool_use_stop(0),
        ],
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "standalone response"),
            LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
        ],
    ]);
    let inspection = client.clone();
    let host = StandaloneHost::start_with_model_client(launch, client)
        .await
        .expect("start in-process host");
    let mut events = host.subscribe();

    host.send(Method::run_text("read the probe"))
        .await
        .expect("submit input");

    tokio::time::timeout(Duration::from_secs(30), async {
        let mut saw_text = false;
        let mut saw_tool_result = false;
        loop {
            match events.recv().await.expect("worker event") {
                Event::TextDelta { text } if text.contains("standalone response") => {
                    saw_text = true;
                }
                Event::ToolResult { .. } => {
                    saw_tool_result = true;
                }
                Event::RunEnd { .. } => {
                    assert!(saw_text, "stream must expose the model text delta");
                    assert!(saw_tool_result, "stream must expose the tool result");
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("run completed");

    let requests = inspection.requests();
    assert_eq!(requests.len(), 2);
    let tool_names = requests[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"Read"));
    assert!(tool_names.contains(&"TaskCreate"));
    assert!(tool_names.contains(&"SubWorkerSpawn"));
    assert!(format!("{:?}", requests[1].items).contains("standalone tool evidence"));
    assert!(
        !temp
            .path()
            .join("state/runtime")
            .join(&worker_name)
            .join("worker.sock")
            .exists()
    );

    host.shutdown().await.expect("graceful shutdown");
}

#[tokio::test]
async fn state_store_failure_is_redacted_and_starts_no_controller() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_path = temp.path().join("state-file-with-secret-name");
    std::fs::write(&state_path, "not a directory").expect("write blocking file");
    let launch = StandaloneLaunchConfig::new(
        temp.path(),
        &state_path,
        manifest::ProfileSelector::Default,
        format!("standalone-failure-{}", Uuid::now_v7()),
    )
    .resolve()
    .expect("resolve launch");
    let client = ScriptedClient::new(Vec::new());

    let error = StandaloneHost::start_with_model_client(launch, client)
        .await
        .err()
        .expect("state store startup rejected");
    assert_eq!(error, standalone::StandaloneStartupError::StateStore);
    assert_eq!(
        error.to_string(),
        "the standalone state store could not be opened"
    );
    assert!(!error.to_string().contains("secret-name"));
    assert!(
        !temp
            .path()
            .join("state-file-with-secret-name/runtime")
            .exists()
    );
}

#[test]
fn standalone_crate_has_no_tui_runtime_or_workspace_server_dependency() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("dependencies section")
        .split("[dev-dependencies]")
        .next()
        .expect("dependency body");
    for forbidden in ["tui", "worker-runtime", "yoi-workspace-server"] {
        assert!(
            !dependencies.lines().any(|line| {
                line.split_once('=')
                    .is_some_and(|(name, _)| name.trim() == forbidden)
            }),
            "standalone must not depend on {forbidden}"
        );
    }
}

#[test]
fn launch_rejects_path_profile_before_worker_startup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let error = StandaloneLaunchConfig::new(
        temp.path(),
        temp.path().join("state"),
        manifest::ProfileSelector::Path {
            path: temp.path().join("profile.dcdl"),
        },
        "standalone-path-profile",
    )
    .resolve()
    .err()
    .expect("path profile rejected");
    assert_eq!(
        error,
        standalone::StandaloneLaunchError::PathProfileUnsupported
    );
}
