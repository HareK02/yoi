//! Parent-facing tools for in-process Internal SubWorker sessions.
//!
//! All four tools share the same parent-owned `SpawnedWorkerRegistry` of typed session handles.
//! There is no Runtime catalog lookup or child socket transport, so a Worker can operate only on
//! its direct Internal children. The socket helper at the bottom remains solely for the legacy
//! top-level Worker callback protocol and is not part of SubWorker communication.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{Event, Method};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

use crate::spawn::registry::SpawnedWorkerRegistry;

/// Timeout applied to each socket-level operation — connect, write,
/// read. Kept short so a stuck child doesn't block the spawner's turn.
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Shared input types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct NameInput {
    /// Name of a previously spawned SubWorker.
    name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubWorkerListInput {}

#[derive(Debug, Serialize)]
struct SubWorkerListItem {
    name: String,
}

struct SubWorkerListTool {
    registry: Arc<SpawnedWorkerRegistry>,
}

#[async_trait]
impl Tool for SubWorkerListTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let _input: SubWorkerListInput = serde_json::from_str(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid SubWorkerList input: {error}"))
        })?;
        let items = self
            .registry
            .list_internal()
            .into_iter()
            .map(|record| SubWorkerListItem {
                name: record.worker_name,
            })
            .collect::<Vec<_>>();
        let count = items.len();
        let content = serde_json::to_string_pretty(&serde_json::json!({ "sub_workers": items }))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        Ok(ToolOutput {
            summary: format!("listed {count} child SubWorker(s)"),
            content: Some(content),
        })
    }
}

pub fn sub_worker_list_tool(registry: Arc<SpawnedWorkerRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SubWorkerListInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SubWorkerList")
            .description("List child SubWorkers owned by this Worker. Peer Workers and general Runtime Workers are excluded.")
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SubWorkerListTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// SubWorkerSend
// ---------------------------------------------------------------------------

const SEND_TO_POD_DESCRIPTION: &str = "Send a text message to a previously spawned SubWorker. The SubWorker \
processes it as a user turn. Fails if the SubWorker is already executing a \
turn — retry after it finishes. Does not wait for the turn to complete; \
use worker-observation tools to inspect its committed session.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SubWorkerSendInput {
    /// Target SubWorker name.
    name: String,
    /// Text delivered to the SubWorker as the next user message.
    message: String,
}

struct SubWorkerSendTool {
    registry: Arc<SpawnedWorkerRegistry>,
}

#[async_trait]
impl Tool for SubWorkerSendTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SubWorkerSendInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SubWorkerSend input: {e}")))?;
        if let Some(record) = self.registry.get_internal(&input.name) {
            record.session.send(input.message).await.map_err(|error| {
                ToolError::ExecutionFailed(format!("send to `{}`: {error}", input.name))
            })?;
            return Ok(ToolOutput {
                summary: format!("sent message to `{}`", input.name),
                content: None,
            });
        }
        Err(unknown_worker_err(&input.name))
    }
}

pub fn sub_worker_send_tool(registry: Arc<SpawnedWorkerRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SubWorkerSendInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SubWorkerSend")
            .description(SEND_TO_POD_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SubWorkerSendTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// SubWorkerStop
// ---------------------------------------------------------------------------

const STOP_POD_DESCRIPTION: &str = "Cancel and stop a spawned Internal SubWorker session, remove it from the parent's direct-child registry, and reclaim delegated Write scope.";

struct SubWorkerStopTool {
    registry: Arc<SpawnedWorkerRegistry>,
}

#[async_trait]
impl Tool for SubWorkerStopTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: NameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SubWorkerStop input: {e}")))?;
        if let Some(record) = self.registry.get_internal(&input.name) {
            record.session.stop().await.map_err(|error| {
                ToolError::ExecutionFailed(format!("stop `{}`: {error}", input.name))
            })?;
            self.registry
                .remove_internal(&input.name)
                .await
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
            return Ok(ToolOutput {
                summary: format!(
                    "stopped worker `{}` and reclaimed delegated scope",
                    input.name
                ),
                content: None,
            });
        }
        Err(unknown_worker_err(&input.name))
    }
}

pub fn sub_worker_stop_tool(registry: Arc<SpawnedWorkerRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(NameInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SubWorkerStop")
            .description(STOP_POD_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SubWorkerStopTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unknown_worker_err(name: &str) -> ToolError {
    ToolError::InvalidArgument(format!("no spawned worker named `{name}`"))
}

/// Connect with a timeout, drain the server's connect-time snapshot,
/// write one `Method` line, flush, and close.
///
/// The Worker socket protocol sends replayed alerts and an initial
/// `Event::Snapshot` before it starts reading client methods. Send-only
/// callers must consume that prefix; otherwise a large snapshot can block
/// the server's writer before it reaches the method-read branch. Any
/// socket error maps to an `io::Error`; the caller decides whether to
/// surface it to the LLM or treat it as "worker stopped".
pub(crate) async fn connect_and_send(socket: &Path, method: &Method) -> std::io::Result<()> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"))??;
    let (r, w) = stream.into_split();
    let mut reader = JsonLineReader::new(r);
    let mut writer = JsonLineWriter::new(w);

    drain_initial_snapshot(&mut reader).await?;

    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(method))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write timed out"))??;
    Ok(())
}

async fn drain_initial_snapshot<R>(reader: &mut JsonLineReader<R>) -> std::io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out"))??;
        match event {
            Some(Event::Snapshot { .. }) => return Ok(()),
            Some(_) => continue,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "worker closed connection before Snapshot event",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use protocol::{Alert, AlertLevel, AlertSource, Greeting, WorkerEvent, WorkerStatus};
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio::task::JoinHandle;

    fn snapshot(entries: Vec<serde_json::Value>) -> Event {
        Event::Snapshot {
            entries,
            greeting: Greeting {
                worker_name: "server".into(),
                cwd: "/tmp".into(),
                provider: "test".into(),
                model: "test".into(),
                scope_summary: String::new(),
                tools: Vec::new(),
                context_window: 200_000,
                context_tokens: 0,
            },
            status: WorkerStatus::Idle,
            in_flight: Default::default(),
        }
    }

    fn serve_initial_events_then_method(
        listener: UnixListener,
        events: Vec<Event>,
    ) -> JoinHandle<Option<Method>> {
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut reader = JsonLineReader::new(r);
            let mut writer = JsonLineWriter::new(w);
            for event in events {
                writer.write(&event).await.ok()?;
            }
            reader.next::<Method>().await.ok().flatten()
        })
    }

    #[tokio::test]
    async fn connect_and_send_drains_initial_alert_and_snapshot_before_method() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let received = serve_initial_events_then_method(
            listener,
            vec![
                Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message: "replayed alert".into(),
                    timestamp_ms: 0,
                }),
                snapshot(Vec::new()),
            ],
        );

        connect_and_send(&socket, &Method::Shutdown).await.unwrap();

        let method = received.await.unwrap().expect("expected method");
        assert!(matches!(method, Method::Shutdown));
    }

    #[tokio::test]
    async fn connect_and_send_delivers_method_after_large_initial_snapshot() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let large_payload = "x".repeat(2 * 1024 * 1024);
        let received = serve_initial_events_then_method(
            listener,
            vec![snapshot(vec![
                serde_json::json!({ "payload": large_payload }),
            ])],
        );
        let expected = Method::WorkerEvent(WorkerEvent::TurnEnded {
            worker_name: "child".into(),
        });

        connect_and_send(&socket, &expected).await.unwrap();

        let method = received.await.unwrap().expect("expected method");
        match method {
            Method::WorkerEvent(WorkerEvent::TurnEnded { worker_name }) => {
                assert_eq!(worker_name, "child")
            }
            other => panic!("expected TurnEnded WorkerEvent, got {other:?}"),
        }
    }
}
