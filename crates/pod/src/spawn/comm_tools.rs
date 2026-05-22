//! Pod-to-Pod communication tools.
//!
//! Four tools in one module — `SendToPod`, `ReadPodOutput`, `StopPod`,
//! `ListPods` — all built on the same `SpawnedPodRegistry` handed in by
//! the controller. Each operation is request-response: connect to the
//! target's Unix socket, perform one method exchange, disconnect.
//!
//! These tools only touch Pods listed in the spawner's
//! `SpawnedPodRegistry`; there is no machine-wide directory lookup, so
//! the spawner can only reach its own descendants.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_worker::llm_client::types::{ContentPart, Item, Role};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, Method};
use serde::Deserialize;
use session_store::LogEntry;
use tokio::net::UnixStream;

use crate::runtime::dir::SpawnedPodRecord;
use crate::runtime::pod_registry::{self, LockFileGuard};
use crate::spawn::registry::SpawnedPodRegistry;

/// Timeout applied to each socket-level operation — connect, write,
/// read. Kept short so a stuck child doesn't block the spawner's turn.
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Shared input types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct NameInput {
    /// Name of a previously spawned Pod.
    name: String,
}

// ---------------------------------------------------------------------------
// SendToPod
// ---------------------------------------------------------------------------

const SEND_TO_POD_DESCRIPTION: &str = "Send a text message to a previously spawned Pod. The spawned Pod \
processes it as a user turn. Fails if the Pod is already executing a \
turn — retry after it finishes. Does not wait for the turn to complete; \
use `ReadPodOutput` to fetch results afterwards.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SendToPodInput {
    /// Target Pod name.
    name: String,
    /// Text delivered to the Pod as the next user message.
    message: String,
}

struct SendToPodTool {
    registry: Arc<SpawnedPodRegistry>,
}

#[async_trait]
impl Tool for SendToPodTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: SendToPodInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SendToPod input: {e}")))?;
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_pod_err(&input.name))?;

        send_run_and_confirm(&record.socket_path, input.message)
            .await
            .map_err(|e| match e {
                SendRunError::AlreadyRunning => ToolError::ExecutionFailed(format!(
                    "pod `{}` is already running a turn; wait for it to finish and retry",
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

pub fn send_to_pod_tool(registry: Arc<SpawnedPodRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SendToPodInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SendToPod")
            .description(SEND_TO_POD_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SendToPodTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// ReadPodOutput
// ---------------------------------------------------------------------------

const READ_POD_OUTPUT_DESCRIPTION: &str = "Fetch new assistant text from a spawned Pod since the last read. \
Uses an internal cursor per-Pod so consecutive calls return only \
newly-produced output. Returns the Pod's current status and the new \
text, or reports `stopped` if the Pod can no longer be reached.";

struct ReadPodOutputTool {
    registry: Arc<SpawnedPodRegistry>,
}

#[async_trait]
impl Tool for ReadPodOutputTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: NameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid ReadPodOutput input: {e}")))?;
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_pod_err(&input.name))?;

        let items = match fetch_history(&record.socket_path).await {
            Ok(items) => items,
            Err(_) => {
                return Ok(ToolOutput {
                    summary: format!("pod `{}` is stopped (unreachable)", input.name),
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
            format!("pod `{}` running; no new assistant text", input.name)
        } else {
            let lines = new_text.lines().count();
            format!(
                "pod `{}`: {lines} new line(s) of assistant text",
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

pub fn read_pod_output_tool(registry: Arc<SpawnedPodRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(NameInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("ReadPodOutput")
            .description(READ_POD_OUTPUT_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadPodOutputTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// StopPod
// ---------------------------------------------------------------------------

const STOP_POD_DESCRIPTION: &str = "Terminate a spawned Pod and reclaim the delegated scope. The Pod \
receives `Shutdown`; its scope entry is released in the machine-wide \
registry so the spawner can spawn a new Pod over the same paths.";

struct StopPodTool {
    registry: Arc<SpawnedPodRegistry>,
}

#[async_trait]
impl Tool for StopPodTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: NameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid StopPod input: {e}")))?;
        let record = self
            .registry
            .get(&input.name)
            .await
            .ok_or_else(|| unknown_pod_err(&input.name))?;

        // Best-effort Shutdown. The child's own `ScopeAllocationGuard`
        // releases the entry on clean exit; we also release explicitly
        // below so callers can't observe a window where the scope is
        // still registered but StopPod has returned. Duplicate release
        // is harmless — `ScopeAllocationGuard`'s drop path swallows
        // `UnknownPod` errors.
        let _ = connect_and_send(&record.socket_path, &Method::Shutdown).await;

        let scope_summary = summarize_scope(&record);
        release_scope(&record.pod_name);

        self.registry
            .remove(&record.pod_name)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("update spawned pod registry: {e}")))?;

        Ok(ToolOutput {
            summary: format!(
                "stopped pod `{}`; reclaimed scope: {scope_summary}",
                record.pod_name
            ),
            content: None,
        })
    }
}

pub fn stop_pod_tool(registry: Arc<SpawnedPodRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(NameInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("StopPod")
            .description(STOP_POD_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(StopPodTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// ListPods
// ---------------------------------------------------------------------------

const LIST_PODS_DESCRIPTION: &str = "List all Pods spawned by this Pod along with their reachability \
status (`alive` / `stopped`) and the scope each was granted.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EmptyInput {}

struct ListPodsTool {
    registry: Arc<SpawnedPodRegistry>,
}

#[async_trait]
impl Tool for ListPodsTool {
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
        let records = self.registry.list().await;
        if records.is_empty() {
            return Ok(ToolOutput {
                summary: "no spawned pods".into(),
                content: None,
            });
        }

        let mut lines: Vec<String> = Vec::with_capacity(records.len());
        let mut stale_names: Vec<String> = Vec::new();
        for record in &records {
            let alive = is_reachable(&record.socket_path).await;
            let status = if alive { "alive" } else { "stopped" };
            let scope = summarize_scope(record);
            lines.push(format!("{} [{status}] scope={scope}", record.pod_name));
            if !alive {
                stale_names.push(record.pod_name.clone());
            }
        }

        // Trigger stale reclaim on unreachable pods so the lock file's
        // allocation table doesn't keep growing indefinitely when
        // children crash without a clean exit path.
        if !stale_names.is_empty() {
            if let Ok(lock_path) = pod_registry::default_registry_path()
                && let Ok(mut guard) = LockFileGuard::open(&lock_path)
            {
                pod_registry::reclaim_stale(&mut guard);
            }
        }

        let summary = format!("{} pod(s) known", records.len());
        Ok(ToolOutput {
            summary,
            content: Some(lines.join("\n")),
        })
    }
}

pub fn list_pods_tool(registry: Arc<SpawnedPodRegistry>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(EmptyInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("ListPods")
            .description(LIST_PODS_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ListPodsTool {
            registry: registry.clone(),
        });
        (meta, tool)
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unknown_pod_err(name: &str) -> ToolError {
    ToolError::InvalidArgument(format!("no spawned pod named `{name}`"))
}

/// Connect with a timeout, write one `Method` line, flush, and close.
/// Any socket error maps to an `io::Error`; the caller decides whether
/// to surface it to the LLM or treat it as "pod stopped".
pub(crate) async fn connect_and_send(socket: &Path, method: &Method) -> std::io::Result<()> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"))??;
    let (_r, w) = stream.into_split();
    let mut writer = JsonLineWriter::new(w);
    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(method))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write timed out"))??;
    Ok(())
}

/// Failure modes distinguished by `SendToPod`.
enum SendRunError {
    /// Target Pod responded with `Error { AlreadyRunning }` — the
    /// caller can retry once the current turn ends.
    AlreadyRunning,
    /// Any other failure (connect / write / read / unexpected EOF).
    Io(String),
}

/// Write `Method::Run` to the target and read back events until we see
/// either `TurnStart` (accepted) or `Error { AlreadyRunning }`
/// (rejected). Any replayed alerts that precede the response are
/// skipped. Times out per-read so a stuck Pod doesn't hang the tool.
async fn send_run_and_confirm(socket: &Path, input: String) -> Result<(), SendRunError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| SendRunError::Io("connect timed out".into()))?
        .map_err(|e| SendRunError::Io(format!("connect: {e}")))?;
    let (r, w) = stream.into_split();
    let mut writer = JsonLineWriter::new(w);
    let mut reader = JsonLineReader::new(r);
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
            .map_err(|_| SendRunError::Io("read timed out".into()))?
            .map_err(|e| SendRunError::Io(format!("read: {e}")))?;
        match event {
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(SendRunError::AlreadyRunning),
            Some(Event::TurnStart { .. }) => return Ok(()),
            // Alerts and other pre-turn events are replayed to new
            // subscribers; keep reading until the controller's response
            // to our `Run` shows up.
            Some(_) => continue,
            None => return Err(SendRunError::Io("connection closed before response".into())),
        }
    }
}

/// Connect to a Pod's socket and read the connect-time `Event::Snapshot`.
///
/// Pods deliver the session-log mirror as the first non-Alert event on
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
                    "pod closed connection before Snapshot event",
                ));
            }
        }
    }
}

/// Probe-connect test. Connection accepted within timeout → alive.
async fn is_reachable(socket: &Path) -> bool {
    tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

fn extract_assistant_text(entries: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for value in entries {
        // The wire payload is the JSON form of `session_store::LogEntry`.
        // Walk Assistant items inside each entry that can carry them:
        // post-compaction `SegmentStart.history` (seed) and per-LLM-call
        // `AssistantItems` deltas.
        let Ok(entry) = serde_json::from_value::<LogEntry>(value.clone()) else {
            continue;
        };
        let logged_items = match entry {
            LogEntry::SegmentStart { history, .. } => history,
            LogEntry::AssistantItems { items, .. } => items,
            _ => continue,
        };
        for logged in logged_items {
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
    }
    out
}

fn summarize_scope(record: &SpawnedPodRecord) -> String {
    if record.scope_delegated.is_empty() {
        return "(none)".into();
    }
    let parts: Vec<String> = record
        .scope_delegated
        .iter()
        .map(|r| {
            let perm = match r.permission {
                manifest::Permission::Read => "read",
                manifest::Permission::Write => "write",
            };
            let tag = if r.recursive { "" } else { " [non-recursive]" };
            format!("{perm}:{}{tag}", r.target.display())
        })
        .collect();
    parts.join(", ")
}

/// Best-effort release of the pod's scope allocation. Swallows every
/// error: the caller has already completed its user-visible side
/// effects (Method::Shutdown was sent), and stale-reclaim will clean
/// up whatever we couldn't.
fn release_scope(pod_name: &str) {
    let Ok(lock_path) = pod_registry::default_registry_path() else {
        return;
    };
    let Ok(mut guard) = LockFileGuard::open(&lock_path) else {
        return;
    };
    let _ = pod_registry::release_pod(&mut guard, pod_name);
}
