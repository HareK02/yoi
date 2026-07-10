use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use futures::StreamExt;
use protocol::{ErrorCode, Event, Method, Segment};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

const RECONNECT_DELAY: Duration = Duration::from_millis(500);
const MAX_RECONNECT_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRuntimeTarget {
    /// Workspace Backend API root URL, for example `http://127.0.0.1:8787`.
    /// This is intentionally the Backend endpoint, not a Runtime endpoint.
    pub base_url: String,
    /// Backend-owned Runtime identity used as path authority.
    pub runtime_id: String,
    /// Backend-owned Worker identity used as path authority.
    pub worker_id: String,
}

impl BackendRuntimeTarget {
    pub fn new(
        base_url: impl Into<String>,
        runtime_id: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
        }
    }

    pub fn display_label(&self) -> String {
        format!("{}:{}", self.runtime_id, self.worker_id)
    }
}

#[derive(Debug)]
pub struct BackendRuntimeClient {
    target: BackendRuntimeTarget,
    http: reqwest::Client,
    events: mpsc::UnboundedReceiver<Event>,
    diagnostics: VecDeque<Event>,
    _observation_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub enum BackendRuntimeClientError {
    InvalidTarget(String),
    Http(reqwest::Error),
}

impl fmt::Display for BackendRuntimeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BackendRuntimeClientError {}

impl From<reqwest::Error> for BackendRuntimeClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl BackendRuntimeClient {
    pub async fn connect(target: BackendRuntimeTarget) -> Result<Self, BackendRuntimeClientError> {
        validate_target(&target)?;
        let http = reqwest::Client::new();
        let (tx, rx) = mpsc::unbounded_channel();

        let observation_target = target.clone();
        let observation_tx = tx.clone();
        let observation_task = tokio::spawn(async move {
            observe_worker_events(observation_target, observation_tx).await;
        });

        Ok(Self {
            target,
            http,
            events: rx,
            diagnostics: VecDeque::new(),
            _observation_task: observation_task,
        })
    }

    pub fn try_next_event(&mut self) -> Option<Event> {
        if let Some(event) = self.diagnostics.pop_front() {
            return Some(event);
        }
        self.events.try_recv().ok()
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        if let Some(event) = self.diagnostics.pop_front() {
            return Some(event);
        }
        self.events.recv().await
    }

    pub async fn send(&mut self, method: &Method) -> Result<(), BackendRuntimeClientError> {
        match backend_command_from_method(method) {
            BackendCommand::Input { kind, content } => {
                let url = self.worker_api_url("input");
                match self
                    .http
                    .post(url)
                    .json(&WorkerInputRequest { kind, content })
                    .send()
                    .await
                    .and_then(|response| response.error_for_status())
                {
                    Ok(response) => match response.json::<WorkerInputResult>().await {
                        Ok(result) => self.enqueue_operation_diagnostics(
                            "input",
                            result.state,
                            result.diagnostics,
                        ),
                        Err(error) => self.enqueue_diagnostic(format!(
                            "Backend runtime input response could not be decoded for {}: {error}",
                            self.target.display_label()
                        )),
                    },
                    Err(error) => self.enqueue_diagnostic(format!(
                        "Backend runtime input failed for {}: {error}",
                        self.target.display_label()
                    )),
                }
            }
            BackendCommand::Lifecycle { action, reason } => {
                let url = self.worker_api_url(action);
                match self
                    .http
                    .post(url)
                    .json(&WorkerLifecycleRequest { reason })
                    .send()
                    .await
                    .and_then(|response| response.error_for_status())
                {
                    Ok(response) => match response.json::<WorkerLifecycleResult>().await {
                        Ok(result) => self.enqueue_operation_diagnostics(
                            action,
                            result.state,
                            result.diagnostics,
                        ),
                        Err(error) => self.enqueue_diagnostic(format!(
                            "Backend runtime {action} response could not be decoded for {}: {error}",
                            self.target.display_label()
                        )),
                    },
                    Err(error) => self.enqueue_diagnostic(format!(
                        "Backend runtime {action} failed for {}: {error}",
                        self.target.display_label()
                    )),
                }
            }
            BackendCommand::Unsupported(message) => {
                self.enqueue_diagnostic(message);
            }
        }
        Ok(())
    }

    fn worker_api_url(&self, suffix: &str) -> String {
        let path = format!(
            "/api/runtimes/{}/workers/{}/{}",
            path_segment_encode(&self.target.runtime_id),
            path_segment_encode(&self.target.worker_id),
            suffix
        );
        join_base_and_path(&self.target.base_url, &path)
    }

    fn enqueue_operation_diagnostics(
        &mut self,
        operation: &str,
        state: String,
        diagnostics: Vec<BackendDiagnostic>,
    ) {
        if state != "accepted" {
            self.enqueue_diagnostic(format!(
                "Backend runtime {operation} was {state} for {}",
                self.target.display_label()
            ));
        }
        for diagnostic in diagnostics {
            self.enqueue_diagnostic(format!(
                "Backend runtime {operation} diagnostic [{}]: {}",
                diagnostic.code, diagnostic.message
            ));
        }
    }

    fn enqueue_diagnostic(&mut self, message: impl Into<String>) {
        self.diagnostics.push_back(diagnostic_event(message));
    }
}

impl Drop for BackendRuntimeClient {
    fn drop(&mut self) {
        self._observation_task.abort();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BackendCommand {
    Input {
        kind: WorkerInputKind,
        content: String,
    },
    Lifecycle {
        action: &'static str,
        reason: Option<String>,
    },
    Unsupported(String),
}

fn backend_command_from_method(method: &Method) -> BackendCommand {
    match method {
        Method::Run { input } => BackendCommand::Input {
            kind: WorkerInputKind::User,
            content: Segment::flatten_to_text(input),
        },
        Method::Notify { message, .. } => BackendCommand::Input {
            kind: WorkerInputKind::System,
            content: message.clone(),
        },
        Method::Cancel => BackendCommand::Lifecycle {
            action: "cancel",
            reason: Some("requested from TUI Backend Runtime API client".to_string()),
        },
        Method::Shutdown => BackendCommand::Lifecycle {
            action: "stop",
            reason: Some("requested from TUI Backend Runtime API client".to_string()),
        },
        Method::Pause => BackendCommand::Unsupported(
            "Backend Runtime API does not expose pause/resume for the TUI client yet; command was not sent".to_string(),
        ),
        Method::Resume => BackendCommand::Unsupported(
            "Backend Runtime API does not expose resume for the TUI client yet; command was not sent".to_string(),
        ),
        Method::Compact => BackendCommand::Unsupported(
            "Backend Runtime API does not expose compaction for the TUI client yet; command was not sent".to_string(),
        ),
        Method::ListCompletions { .. } => BackendCommand::Unsupported(
            "Backend Runtime API does not expose completion lookup for the TUI client yet".to_string(),
        ),
        Method::ListRewindTargets | Method::RewindTo { .. } => BackendCommand::Unsupported(
            "Backend Runtime API does not expose rewind controls for the TUI client yet; command was not sent".to_string(),
        ),
        Method::ListWorkers | Method::RestoreWorker { .. } | Method::RegisterPeer { .. } => {
            BackendCommand::Unsupported(
                "Backend Runtime API worker-management controls are not available from this Console connection".to_string(),
            )
        }
        Method::WorkerEvent(_) => BackendCommand::Unsupported(
            "Backend Runtime API does not accept child Worker lifecycle events from this Console connection".to_string(),
        ),
    }
}

async fn observe_worker_events(target: BackendRuntimeTarget, tx: mpsc::UnboundedSender<Event>) {
    let mut cursor: Option<String> = None;
    let mut last_sequence = 0_u64;
    let mut attempts = 0_usize;

    loop {
        let url = observation_ws_url(&target, cursor.as_deref());
        match connect_async(&url).await {
            Ok((mut ws, _)) => {
                attempts = 0;
                while let Some(frame) = ws.next().await {
                    match frame {
                        Ok(TungsteniteMessage::Text(text)) => {
                            match serde_json::from_str::<ClientWorkerEventWsFrame>(&text) {
                                Ok(ClientWorkerEventWsFrame::Event { envelope }) => {
                                    if envelope.runtime_id != target.runtime_id
                                        || envelope.worker_id != target.worker_id
                                    {
                                        let _ = tx.send(diagnostic_event(format!(
                                            "Backend observation frame target mismatch: got {}:{}, expected {}",
                                            envelope.runtime_id,
                                            envelope.worker_id,
                                            target.display_label()
                                        )));
                                        continue;
                                    }
                                    if let Some(sequence) = decode_backend_cursor(&envelope.cursor)
                                    {
                                        if sequence <= last_sequence {
                                            continue;
                                        }
                                        last_sequence = sequence;
                                    } else {
                                        let _ = tx.send(diagnostic_event(format!(
                                            "Backend observation cursor was malformed: {}",
                                            envelope.cursor
                                        )));
                                    }
                                    cursor = Some(envelope.cursor.clone());
                                    let _ = tx.send(envelope.payload);
                                }
                                Ok(ClientWorkerEventWsFrame::Diagnostic { diagnostic }) => {
                                    let message = format!(
                                        "Backend observation diagnostic [{}]: {}",
                                        diagnostic.code, diagnostic.message
                                    );
                                    let _ = tx.send(diagnostic_event(message));
                                    if diagnostic.code == "backend.cursor_unknown_or_expired" {
                                        cursor = None;
                                        last_sequence = 0;
                                        break;
                                    }
                                }
                                Err(error) => {
                                    let _ = tx.send(diagnostic_event(format!(
                                        "Backend observation frame was not valid JSON: {error}"
                                    )));
                                }
                            }
                        }
                        Ok(TungsteniteMessage::Close(_)) => break,
                        Ok(TungsteniteMessage::Ping(_))
                        | Ok(TungsteniteMessage::Pong(_))
                        | Ok(TungsteniteMessage::Binary(_))
                        | Ok(TungsteniteMessage::Frame(_)) => {}
                        Err(error) => {
                            let _ = tx.send(diagnostic_event(format!(
                                "Backend observation WebSocket error for {}: {error}",
                                target.display_label()
                            )));
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = tx.send(diagnostic_event(format!(
                    "Backend observation WebSocket connect failed for {}: {error}",
                    target.display_label()
                )));
            }
        }

        attempts += 1;
        if attempts > MAX_RECONNECT_ATTEMPTS {
            let _ = tx.send(diagnostic_event(format!(
                "Backend observation stream for {} stopped after {MAX_RECONNECT_ATTEMPTS} reconnect attempts",
                target.display_label()
            )));
            break;
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn diagnostic_event(message: impl Into<String>) -> Event {
    Event::Error {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

fn validate_target(target: &BackendRuntimeTarget) -> Result<(), BackendRuntimeClientError> {
    if target.base_url.trim().is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL is required".to_string(),
        ));
    }
    if !(target.base_url.starts_with("http://") || target.base_url.starts_with("https://")) {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "Backend API base URL must start with http:// or https://".to_string(),
        ));
    }
    if target.runtime_id.is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "runtime_id is required".to_string(),
        ));
    }
    if target.worker_id.is_empty() {
        return Err(BackendRuntimeClientError::InvalidTarget(
            "worker_id is required".to_string(),
        ));
    }
    Ok(())
}

fn observation_ws_url(target: &BackendRuntimeTarget, cursor: Option<&str>) -> String {
    let path = format!(
        "/api/runtimes/{}/workers/{}/events/ws",
        path_segment_encode(&target.runtime_id),
        path_segment_encode(&target.worker_id)
    );
    let mut url = join_base_and_path(&http_base_to_ws(&target.base_url), &path);
    if let Some(cursor) = cursor {
        url.push_str("?cursor=");
        url.push_str(&query_value_encode(cursor));
    }
    url
}

fn http_base_to_ws(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

fn join_base_and_path(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn decode_backend_cursor(cursor: &str) -> Option<u64> {
    let encoded = cursor.strip_prefix("bo_")?;
    if encoded.len() != 16 {
        return None;
    }
    u64::from_str_radix(encoded, 16).ok()
}

fn path_segment_encode(input: &str) -> String {
    percent_encode(input, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
    })
}

fn query_value_encode(input: &str) -> String {
    percent_encode(input, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
    })
}

fn percent_encode(input: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        if keep(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkerInputKind {
    User,
    System,
}

#[derive(Debug, Serialize)]
struct WorkerInputRequest {
    kind: WorkerInputKind,
    content: String,
}

#[derive(Debug, Serialize)]
struct WorkerLifecycleRequest {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkerInputResult {
    state: String,
    #[serde(default)]
    diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct WorkerLifecycleResult {
    state: String,
    #[serde(default)]
    diagnostics: Vec<BackendDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct BackendDiagnostic {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ClientWorkerEventWsFrame {
    Event {
        envelope: ClientWorkerEventWsEnvelope,
    },
    Diagnostic {
        diagnostic: ClientWorkerEventWsDiagnostic,
    },
}

#[derive(Debug, Deserialize)]
struct ClientWorkerEventWsEnvelope {
    cursor: String,
    runtime_id: String,
    worker_id: String,
    payload: Event,
}

#[derive(Debug, Deserialize)]
struct ClientWorkerEventWsDiagnostic {
    code: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_command_maps_run_to_user_input_without_runtime_endpoint() {
        let method = Method::Run {
            input: vec![
                Segment::text("hello"),
                Segment::FileRef {
                    path: "src/lib.rs".into(),
                },
            ],
        };
        assert_eq!(
            backend_command_from_method(&method),
            BackendCommand::Input {
                kind: WorkerInputKind::User,
                content: "hello@src/lib.rs".to_string(),
            }
        );
    }

    #[test]
    fn observation_url_uses_backend_runtime_worker_identity() {
        let target =
            BackendRuntimeTarget::new("http://127.0.0.1:8787/", "runtime/one", "worker one");
        assert_eq!(
            observation_ws_url(&target, Some("bo_0000000000000001")),
            "ws://127.0.0.1:8787/api/runtimes/runtime%2Fone/workers/worker%20one/events/ws?cursor=bo_0000000000000001"
        );
    }
}
