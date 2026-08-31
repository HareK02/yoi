use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::client::LlmClient;
use agen::llm_client::error::ClientError;
use agen::llm_client::event::{Event as LlmEvent, StopReason};
use agen::llm_client::types::Request;
use async_trait::async_trait;
use client::Client;
use client::transport::in_process::Socket as InProcessSocket;
use futures::{Stream, stream};
use protocol::{Event, Method};
use standalone::{
    StaleLeasePolicy, StandaloneHost, StandaloneLaunchConfig, StandaloneListScope,
    StandaloneSessionStatus, StandaloneSessionStore, StandaloneStartupError, StandaloneStoreError,
};
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
    let mut protocol_client = host.connect();

    protocol_client
        .send(&Method::run_text("read the probe"))
        .await
        .expect("submit input");

    tokio::time::timeout(Duration::from_secs(30), async {
        let mut saw_user_message = false;
        let mut saw_text = false;
        let mut saw_tool_result = false;
        loop {
            match protocol_client
                .next_event()
                .await
                .expect("protocol event")
                .expect("worker event")
            {
                Event::UserMessage { segments }
                    if format!("{segments:?}").contains("read the probe") =>
                {
                    saw_user_message = true;
                }
                Event::TextDelta { text } if text.contains("standalone response") => {
                    saw_text = true;
                }
                Event::ToolResult { .. } => {
                    saw_tool_result = true;
                }
                Event::RunEnd { .. } => {
                    assert!(
                        saw_user_message,
                        "stream must expose the committed user message"
                    );
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
        "the standalone state store could not be opened or validated"
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

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn standalone_restore_preserves_history_tasks_notifications_and_cwd_scope() -> TestResult {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("project");
    let state_dir = temp.path().join("client").join("standalone-sessions");
    std::fs::create_dir_all(&cwd)?;
    let launch = StandaloneLaunchConfig::new(
        &cwd,
        &state_dir,
        manifest::ProfileSelector::Default,
        "display-name-is-not-session-identity",
    )
    .resolve()?;
    let first_client = ScriptedClient::new(vec![
        vec![
            LlmEvent::tool_use_start(0, "task-1", "TaskCreate"),
            LlmEvent::tool_input_delta(
                0,
                r#"{"subject":"persisted task","description":"survives restore"}"#,
            ),
            LlmEvent::tool_use_stop(0),
        ],
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "first answer"),
            LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
        ],
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "notification acknowledged"),
            LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
        ],
    ]);
    let host = StandaloneHost::start_with_model_client(launch, first_client).await?;
    let session_id = host.session_id();
    let mut protocol_client = host.connect();
    protocol_client
        .send(&Method::run_text("first request"))
        .await?;
    wait_for_run_end(&mut protocol_client).await?;
    protocol_client
        .send(&Method::Notify {
            message: "persisted notification".to_string(),
            auto_run: true,
        })
        .await?;
    wait_for_run_end(&mut protocol_client).await?;
    host.shutdown().await?;

    let store = StandaloneSessionStore::open(&state_dir)?;
    let current = store.list(&cwd, StandaloneListScope::CurrentCwd, 100)?;
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].session_id, session_id);
    assert_eq!(current[0].status, StandaloneSessionStatus::Stopped);
    let other_cwd = temp.path().join("other");
    std::fs::create_dir(&other_cwd)?;
    assert!(
        store
            .list(&other_cwd, StandaloneListScope::CurrentCwd, 100)?
            .is_empty()
    );
    assert_eq!(
        store.list(&other_cwd, StandaloneListScope::All, 100)?.len(),
        1
    );

    let second_client = ScriptedClient::new(vec![vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "second answer"),
        LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
    ]]);
    let second_inspection = second_client.clone();
    let host =
        StandaloneHost::restore_with_model_client(state_dir.clone(), session_id, second_client)
            .await?;
    let mut protocol_client = host.connect();
    let snapshot = format!(
        "{:?}",
        protocol_client
            .next_event()
            .await
            .expect("restored protocol stream")
            .expect("restored snapshot")
    );
    assert!(snapshot.contains("first request"), "{snapshot}");
    assert!(snapshot.contains("first answer"), "{snapshot}");
    assert!(snapshot.contains("persisted task"), "{snapshot}");
    assert!(snapshot.contains("persisted notification"), "{snapshot}");

    protocol_client
        .send(&Method::run_text("continue after restore"))
        .await?;
    wait_for_run_end(&mut protocol_client).await?;
    let request = second_inspection
        .requests()
        .into_iter()
        .next()
        .expect("restored run request");
    let projected = format!("{:?}", request.items);
    assert!(projected.contains("first answer"), "{projected}");
    assert!(projected.contains("persisted notification"), "{projected}");
    assert!(projected.contains("persisted task"), "{projected}");
    host.shutdown().await?;

    store.delete(session_id)?;
    assert!(cwd.exists(), "deleting session state must not mutate cwd");
    assert!(matches!(
        store.load(session_id),
        Err(StandaloneStoreError::SessionNotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn standalone_restore_rejects_concurrent_lease_and_missing_cwd() -> TestResult {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("project");
    let moved = temp.path().join("moved-project");
    let state_dir = temp.path().join("state");
    std::fs::create_dir(&cwd)?;
    let launch = StandaloneLaunchConfig::new(
        &cwd,
        &state_dir,
        manifest::ProfileSelector::Default,
        "standalone-lease-test",
    )
    .resolve()?;
    let host =
        StandaloneHost::start_with_model_client(launch, ScriptedClient::new(Vec::new())).await?;
    let session_id = host.session_id();
    let store = StandaloneSessionStore::open(&state_dir)?;
    assert!(matches!(
        store.acquire_lease(session_id, StaleLeasePolicy::Recover),
        Err(StandaloneStoreError::SessionLeased(id)) if id == session_id
    ));
    let restore = StandaloneHost::restore_with_model_client(
        state_dir.clone(),
        session_id,
        ScriptedClient::new(Vec::new()),
    )
    .await;
    assert!(matches!(
        restore,
        Err(StandaloneStartupError::SessionActive)
    ));
    host.shutdown().await?;

    std::fs::rename(&cwd, &moved)?;
    let restore = StandaloneHost::restore_with_model_client(
        state_dir,
        session_id,
        ScriptedClient::new(Vec::new()),
    )
    .await;
    assert!(matches!(
        restore,
        Err(StandaloneStartupError::WorkingDirectoryUnavailable)
    ));
    Ok(())
}

#[tokio::test]
async fn standalone_restore_recovers_only_a_proven_stale_lease() -> TestResult {
    let temp = tempfile::tempdir()?;
    let state_dir = temp.path().join("state");
    let mut launch = StandaloneLaunchConfig::new(
        temp.path(),
        &state_dir,
        manifest::ProfileSelector::Default,
        "standalone-stale-lease-test",
    )
    .resolve()?;
    launch.profile.manifest.profile = Some(manifest::ProfileManifestSnapshot {
        source: manifest::ProfileSource::Registry {
            source: manifest::ProfileRegistrySource::User,
            name: "user-standalone".to_string(),
            path: None,
            provenance: Some("user-config-revision-7".to_string()),
        },
        profile: Some(manifest::ProfileMetadata {
            name: Some("User standalone".to_string()),
            description: None,
            format: None,
        }),
    });
    let host =
        StandaloneHost::start_with_model_client(launch, ScriptedClient::new(Vec::new())).await?;
    let session_id = host.session_id();
    host.shutdown().await?;
    let store = StandaloneSessionStore::open(&state_dir)?;
    assert!(matches!(
        store.load(session_id)?.manifest.profile,
        Some(manifest::ProfileManifestSnapshot {
            source: manifest::ProfileSource::Registry {
                source: manifest::ProfileRegistrySource::User,
                ..
            },
            ..
        })
    ));
    let session_dir = state_dir.join(session_id.to_string());
    std::fs::write(
        session_dir.join("lease.json"),
        serde_json::to_vec(&serde_json::json!({
            "lease_id": uuid::Uuid::now_v7(),
            "pid": u32::MAX,
            "process_start_marker": 1,
            "acquired_at_unix_ms": 1
        }))?,
    )?;

    let host = StandaloneHost::restore_with_model_client(
        state_dir,
        session_id,
        ScriptedClient::new(Vec::new()),
    )
    .await?;
    host.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn standalone_restore_rejects_lease_with_missing_start_marker() -> TestResult {
    let temp = tempfile::tempdir()?;
    let state_dir = temp.path().join("state");
    let launch = StandaloneLaunchConfig::new(
        temp.path(),
        &state_dir,
        manifest::ProfileSelector::Default,
        "standalone-unknown-lease-test",
    )
    .resolve()?;
    let host =
        StandaloneHost::start_with_model_client(launch, ScriptedClient::new(Vec::new())).await?;
    let session_id = host.session_id();
    host.shutdown().await?;
    let session_dir = state_dir.join(session_id.to_string());
    std::fs::write(
        session_dir.join("lease.json"),
        serde_json::to_vec(&serde_json::json!({
            "lease_id": uuid::Uuid::now_v7(),
            "pid": std::process::id(),
            "acquired_at_unix_ms": 1
        }))?,
    )?;

    let store = StandaloneSessionStore::open(&state_dir)?;
    assert!(matches!(
        store.acquire_lease(session_id, StaleLeasePolicy::Recover),
        Err(StandaloneStoreError::LeaseLivenessUnknown(id)) if id == session_id
    ));
    let restore = StandaloneHost::restore_with_model_client(
        state_dir,
        session_id,
        ScriptedClient::new(Vec::new()),
    )
    .await;
    assert!(matches!(
        restore,
        Err(StandaloneStartupError::LeaseLivenessUnknown)
    ));
    Ok(())
}

#[tokio::test]
async fn standalone_metadata_fails_closed_on_incomplete_or_newer_records() -> TestResult {
    let temp = tempfile::tempdir()?;
    let state_dir = temp.path().join("state");
    let launch = StandaloneLaunchConfig::new(
        temp.path(),
        &state_dir,
        manifest::ProfileSelector::Default,
        "standalone-schema-test",
    )
    .resolve()?;
    let host =
        StandaloneHost::start_with_model_client(launch, ScriptedClient::new(Vec::new())).await?;
    let session_id = host.session_id();
    host.shutdown().await?;
    let store = StandaloneSessionStore::open(&state_dir)?;
    let session_dir = state_dir.join(session_id.to_string());
    std::fs::write(session_dir.join("commit.pending"), b"interrupted\n")?;
    assert!(matches!(
        store.load(session_id),
        Err(StandaloneStoreError::IncompleteCommit(id)) if id == session_id
    ));
    std::fs::remove_file(session_dir.join("commit.pending"))?;
    let record_path = session_dir.join("record.json");
    let mut record: serde_json::Value = serde_json::from_slice(&std::fs::read(&record_path)?)?;
    record["schema_version"] = serde_json::json!(u32::MAX);
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record)?)?;
    assert!(matches!(
        store.load(session_id),
        Err(StandaloneStoreError::NewerSchema { id, .. }) if id == session_id
    ));
    Ok(())
}

async fn wait_for_run_end(client: &mut Client<InProcessSocket>) -> TestResult {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(client.next_event().await, Ok(Some(Event::RunEnd { .. }))) {
                break;
            }
        }
    })
    .await?;
    Ok(())
}
