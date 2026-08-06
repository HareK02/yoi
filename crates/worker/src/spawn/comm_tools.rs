//! Worker-to-Worker communication tools.
//!
//! Three tools in one module: `SubWorkerSend`, `SubWorkerReadOutput`, `SubWorkerStop`,
//! all built on the same `SpawnedWorkerRegistry` handed in by
//! the controller. Each operation is request-response: connect to the
//! target's Unix socket, perform one method exchange, disconnect.
//!
//! These tools only touch Workers listed in the spawner's
//! `SpawnedWorkerRegistry`; there is no machine-wide directory lookup, so
//! the spawner can only reach its own descendants.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_engine::llm_client::types::{ContentPart, Item, Role};
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, InvokeKind, Method};
use serde::{Deserialize, Serialize};
use session_store::LogEntry;
use tokio::net::UnixStream;

use crate::runtime::dir::SpawnedWorkerRecord;
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
        let mut items = self
            .registry
            .list_internal()
            .into_iter()
            .map(|record| SubWorkerListItem {
                name: record.worker_name,
            })
            .collect::<Vec<_>>();
        items.extend(
            self.registry
                .list()
                .await
                .into_iter()
                .map(|record| SubWorkerListItem {
                    name: record.worker_name,
                }),
        );
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
use `SubWorkerReadOutput` to fetch results afterwards.";

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
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_worker_err(&input.name))?;

        send_run_and_confirm(&record.socket_path, input.message)
            .await
            .map_err(|e| match e {
                SendRunError::AlreadyRunning => ToolError::ExecutionFailed(format!(
                    "worker `{}` is already running a turn; wait for it to finish and retry",
                    input.name
                )),
                SendRunError::Rejected { code, message } => ToolError::ExecutionFailed(format!(
                    "worker `{}` rejected the run with {code:?}: {message}",
                    input.name
                )),
                SendRunError::Io(msg) => {
                    ToolError::ExecutionFailed(format!("send to `{}`: {msg}", input.name))
                }
            })?;

        Ok(ToolOutput {
            summary: format!("sent message to `{}`", input.name),
            content: None,
        })
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
// SubWorkerReadOutput
// ---------------------------------------------------------------------------

const READ_POD_OUTPUT_DESCRIPTION: &str = "Fetch new assistant text from a SubWorker since the last read. \
Uses an internal cursor per-SubWorker so consecutive calls return only \
newly-produced output. Returns the SubWorker's current status and the new \
text, or reports `stopped` if the SubWorker can no longer be reached.";

struct SubWorkerReadOutputTool {
    registry: Arc<SpawnedWorkerRegistry>,
}

#[async_trait]
impl Tool for SubWorkerReadOutputTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: NameInput = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid SubWorkerReadOutput input: {e}"))
        })?;
        if let Some(record) = self.registry.get_internal(&input.name) {
            let entries = record.session.entries();
            let cursor = self.registry.cursor(&input.name).await;
            let new_entries = if cursor >= entries.len() {
                &[] as &[LogEntry]
            } else {
                &entries[cursor..]
            };
            let values = new_entries
                .iter()
                .filter_map(|entry| serde_json::to_value(entry).ok())
                .collect::<Vec<_>>();
            let new_text = extract_assistant_text(&values);
            self.registry.set_cursor(&input.name, entries.len()).await;
            let status = format!("{:?}", record.session.status()).to_lowercase();
            let summary = if new_text.is_empty() {
                format!("worker `{}` {status}; no new assistant text", input.name)
            } else {
                format!(
                    "worker `{}` {status}: {} new line(s) of assistant text",
                    input.name,
                    new_text.lines().count()
                )
            };
            return Ok(ToolOutput {
                summary,
                content: (!new_text.is_empty()).then_some(new_text),
            });
        }
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_worker_err(&input.name))?;

        let items = match fetch_history(&record.socket_path).await {
            Ok(items) => items,
            Err(_) => {
                return Ok(ToolOutput {
                    summary: format!("worker `{}` is stopped (unreachable)", input.name),
                    content: None,
                });
            }
        };

        let cursor = self.registry.cursor(&input.name).await;
        let new_items = if cursor >= items.len() {
            &[] as &[serde_json::Value]
        } else {
            &items[cursor..]
        };
        let new_text = extract_assistant_text(new_items);
        self.registry.set_cursor(&input.name, items.len()).await;

        let summary = if new_text.is_empty() {
            format!("worker `{}` running; no new assistant text", input.name)
        } else {
            let lines = new_text.lines().count();
            format!(
                "worker `{}`: {lines} new line(s) of assistant text",
                input.name
            )
        };
        let content = if new_text.is_empty() {
            None
        } else {
            Some(new_text)
        };
        Ok(ToolOutput { summary, content })
    }
}

pub fn sub_worker_read_output_tool(registry: Arc<SpawnedWorkerRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(NameInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SubWorkerReadOutput")
            .description(READ_POD_OUTPUT_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SubWorkerReadOutputTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// SubWorkerStop
// ---------------------------------------------------------------------------

const STOP_POD_DESCRIPTION: &str = "Terminate a spawned SubWorker and reclaim the delegated scope. The SubWorker \
receives `Shutdown`; its scope entry is released in the machine-wide \
registry so the parent Worker can spawn a new SubWorker over the same paths.";

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
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_worker_err(&input.name))?;

        // Best-effort Shutdown. The child's own `ScopeAllocationGuard`
        // releases its entry on clean exit; the parent reclaim below is the
        // authoritative operation for removing the child record and returning
        // delegated Write scope to the spawner.
        let _ = connect_and_send(&record.socket_path, &Method::Shutdown).await;

        let scope_summary = summarize_scope(&record);

        self.registry
            .remove(&record.worker_name)
            .await
            .map_err(|e| {
                ToolError::ExecutionFailed(format!("update spawned worker registry: {e}"))
            })?;

        Ok(ToolOutput {
            summary: format!(
                "stopped worker `{}`; reclaimed scope: {scope_summary}",
                record.worker_name
            ),
            content: None,
        })
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

fn summarize_scope(record: &SpawnedWorkerRecord) -> String {
    if record.scope_delegated.is_empty() {
        return "(none)".into();
    }
    let parts: Vec<String> = record
        .scope_delegated
        .iter()
        .map(|rule| {
            let perm = match rule.permission {
                manifest::Permission::Read => "read",
                manifest::Permission::Write => "write",
            };
            let recursive = if rule.recursive {
                ""
            } else {
                " [non-recursive]"
            };
            format!("{perm}:{}{recursive}", rule.target.display())
        })
        .collect();
    parts.join(", ")
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

/// Failure modes distinguished by `SubWorkerSend`.
#[derive(Debug)]
pub(crate) enum SendRunError {
    /// Target SubWorker responded with `Error { AlreadyRunning }` — the
    /// caller can retry once the current turn ends.
    AlreadyRunning,
    /// Target SubWorker explicitly rejected the run after delivery reached the
    /// controller.
    Rejected { code: ErrorCode, message: String },
    /// Transport, protocol, timeout, or unexpected EOF before acceptance
    /// evidence was observed.
    Io(String),
}

/// Write `Method::Run` to the target and read back events until we see
/// evidence that the controller accepted the run (`UserMessage`,
/// `TurnStart`, or a user-send `InvokeStart`) or rejected it. The connect-time
/// event prelude is drained before sending the method so large Snapshots and
/// large Run payloads cannot block each other on the same socket. Times out
/// per operation so a stuck Worker doesn't hang the tool.
pub(crate) async fn send_run_and_confirm(socket: &Path, input: String) -> Result<(), SendRunError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| SendRunError::Io("connect timed out".into()))?
        .map_err(|e| SendRunError::Io(format!("connect: {e}")))?;
    let (r, w) = stream.into_split();
    let mut writer = JsonLineWriter::new(w);
    let mut reader = JsonLineReader::new(r);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| SendRunError::Io("read initial Snapshot timed out".into()))?
            .map_err(|e| SendRunError::Io(format!("read initial Snapshot: {e}")))?;
        match event {
            Some(Event::Snapshot { .. }) => break,
            Some(Event::Alert(_)) => continue,
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(SendRunError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(SendRunError::Rejected { code, message });
            }
            Some(_) => continue,
            None => {
                return Err(SendRunError::Io(
                    "connection closed before initial Snapshot".into(),
                ));
            }
        }
    }

    tokio::time::timeout(
        SOCKET_OP_TIMEOUT,
        writer.write(&Method::Run {
            input: vec![protocol::Segment::text(input)],
        }),
    )
    .await
    .map_err(|_| SendRunError::Io("write timed out".into()))?
    .map_err(|e| SendRunError::Io(format!("write: {e}")))?;
    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| SendRunError::Io("read response timed out".into()))?
            .map_err(|e| SendRunError::Io(format!("read response: {e}")))?;
        match event {
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(SendRunError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(SendRunError::Rejected { code, message });
            }
            Some(Event::InvokeStart {
                kind: InvokeKind::UserSend,
            })
            | Some(Event::UserMessage { .. })
            | Some(Event::TurnStart { .. }) => return Ok(()),
            // Other post-Snapshot events can race with the controller's
            // response; keep reading until the Run is accepted or rejected.
            Some(_) => continue,
            None => return Err(SendRunError::Io("connection closed before response".into())),
        }
    }
}

/// Connect to a Worker's socket and read the connect-time `Event::Snapshot`.
///
/// Workers deliver the session-log mirror as the first non-Alert event on
/// every new connection, so consuming it is sufficient — no explicit
/// `GetHistory` method round trip. Returns the entries as raw JSON
/// values; callers deserialize as `session_store::LogEntry` if they
/// need typed access.
async fn fetch_history(socket: &Path) -> std::io::Result<Vec<serde_json::Value>> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"))??;
    let (r, _w) = stream.into_split();
    let mut reader = JsonLineReader::new(r);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out"))??;
        match event {
            Some(Event::Snapshot { entries, .. }) => return Ok(entries),
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

fn extract_assistant_text(entries: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for value in entries {
        // The wire payload is the JSON form of `session_store::LogEntry`.
        // Walk current singular assistant items and the seeded history in
        // post-compaction `SegmentStart` entries.
        let Ok(entry) = serde_json::from_value::<LogEntry>(value.clone()) else {
            continue;
        };
        match entry {
            LogEntry::SegmentStart { history, .. } => {
                for logged in history {
                    push_assistant_text(&mut out, logged);
                }
            }
            LogEntry::AssistantItem { item, .. } => push_assistant_text(&mut out, item),
            _ => continue,
        }
    }
    out
}

fn push_assistant_text(out: &mut String, logged: session_store::LoggedItem) {
    let item: Item = logged.into();
    if let Item::Message {
        role: Role::Assistant,
        content,
        ..
    } = item
    {
        for part in content {
            if let ContentPart::Text { text } = part {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&text);
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

    fn serve_initial_events_then_run_ack(
        listener: UnixListener,
        initial_events: Vec<Event>,
        ack: Event,
    ) -> JoinHandle<Option<Method>> {
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.ok()?;
            let (r, w) = stream.into_split();
            let mut reader = JsonLineReader::new(r);
            let mut writer = JsonLineWriter::new(w);
            for event in initial_events {
                writer.write(&event).await.ok()?;
            }
            let method = reader.next::<Method>().await.ok().flatten()?;
            writer.write(&ack).await.ok()?;
            Some(method)
        })
    }

    #[tokio::test]
    async fn send_run_and_confirm_keeps_connection_open_until_user_message_ack() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let received = serve_initial_events_then_run_ack(
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
            Event::UserMessage {
                segments: vec![protocol::Segment::text("hello")],
            },
        );

        send_run_and_confirm(&socket, "hello".into()).await.unwrap();

        let method = received.await.unwrap().expect("expected method");
        match method {
            Method::Run { input } => {
                assert_eq!(protocol::Segment::flatten_to_text(&input), "hello");
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_run_and_confirm_drains_alert_and_large_snapshot_before_large_run() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let large_snapshot_payload = "s".repeat(2 * 1024 * 1024);
        let large_run_payload = "r".repeat(2 * 1024 * 1024);
        let received = serve_initial_events_then_run_ack(
            listener,
            vec![
                Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message: "replayed alert".into(),
                    timestamp_ms: 0,
                }),
                snapshot(vec![
                    serde_json::json!({ "payload": large_snapshot_payload }),
                ]),
            ],
            Event::InvokeStart {
                kind: InvokeKind::UserSend,
            },
        );

        send_run_and_confirm(&socket, large_run_payload.clone())
            .await
            .unwrap();

        let method = received.await.unwrap().expect("expected method");
        match method {
            Method::Run { input } => {
                assert_eq!(
                    protocol::Segment::flatten_to_text(&input),
                    large_run_payload
                );
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_run_and_confirm_reports_already_running() {
        let tmp = TempDir::new().unwrap();
        let socket = tmp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let received = serve_initial_events_then_run_ack(
            listener,
            vec![snapshot(Vec::new())],
            Event::Error {
                code: ErrorCode::AlreadyRunning,
                message: "busy".into(),
            },
        );

        let err = send_run_and_confirm(&socket, "hello".into())
            .await
            .expect_err("expected AlreadyRunning");
        assert!(matches!(err, SendRunError::AlreadyRunning));
        assert!(matches!(received.await.unwrap(), Some(Method::Run { .. })));
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
