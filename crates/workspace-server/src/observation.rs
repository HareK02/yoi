use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as TungsteniteError, Message as TungsteniteMessage};
use worker_runtime::http_server::{RuntimeWorkerEventWsEnvelope, RuntimeWorkerEventWsFrame};

/// Backend-private source for a runtime worker observation stream.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeObservationSourceConfig {
    pub runtime_id: String,
    pub worker_id: String,
    pub endpoint: String,
    pub bearer_token: Option<String>,
}

impl std::fmt::Debug for RuntimeObservationSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeObservationSourceConfig")
            .field("runtime_id", &self.runtime_id)
            .field("worker_id", &self.worker_id)
            .field("endpoint", &"<backend-private>")
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Event consumed from a Runtime-owned worker observation WebSocket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeObservationUpstreamEvent {
    pub runtime_id: String,
    pub worker_id: String,
    pub runtime_cursor: String,
    pub payload: protocol::Event,
}

/// Backend-local frame exposed to browser/future-TUI clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientWorkerEventWsFrame {
    Event {
        envelope: ClientWorkerEventWsEnvelope,
    },
    Diagnostic {
        diagnostic: ClientWorkerEventWsDiagnostic,
    },
}

/// Backend-owned opaque event envelope. It intentionally omits Runtime endpoints,
/// credentials, sockets and session paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientWorkerEventWsEnvelope {
    pub cursor: String,
    pub event_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub payload: protocol::Event,
}

/// Client-facing typed observation diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientWorkerEventWsDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ClientWorkerEventsWsQuery {
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationProxyError {
    RuntimeUnavailable(String),
    WorkerNotFound(String),
    CursorMalformed(String),
    CursorUnknownOrExpired(String),
    UpstreamDisconnect(String),
    MalformedFrame(String),
    ObservationOnly,
}

impl ObservationProxyError {
    pub fn code(&self) -> &'static str {
        match self {
            ObservationProxyError::RuntimeUnavailable(_) => "backend.runtime_unavailable",
            ObservationProxyError::WorkerNotFound(_) => "backend.worker_not_found",
            ObservationProxyError::CursorMalformed(_) => "backend.cursor_malformed",
            ObservationProxyError::CursorUnknownOrExpired(_) => "backend.cursor_unknown_or_expired",
            ObservationProxyError::UpstreamDisconnect(_) => "backend.upstream_disconnect",
            ObservationProxyError::MalformedFrame(_) => "backend.malformed_frame",
            ObservationProxyError::ObservationOnly => "backend.observation_only",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ObservationProxyError::RuntimeUnavailable(message)
            | ObservationProxyError::WorkerNotFound(message)
            | ObservationProxyError::CursorMalformed(message)
            | ObservationProxyError::CursorUnknownOrExpired(message)
            | ObservationProxyError::UpstreamDisconnect(message)
            | ObservationProxyError::MalformedFrame(message) => message,
            ObservationProxyError::ObservationOnly => {
                "backend worker event WebSocket is observation-only"
            }
        }
    }
}

impl ClientWorkerEventWsFrame {
    pub fn event(envelope: ClientWorkerEventWsEnvelope) -> Self {
        Self::Event { envelope }
    }

    pub fn diagnostic(error: ObservationProxyError) -> Self {
        Self::Diagnostic {
            diagnostic: ClientWorkerEventWsDiagnostic {
                code: error.code().to_string(),
                message: error.message().to_string(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BackendObservationCursor {
    pub sequence: u64,
}

impl BackendObservationCursor {
    pub fn new(sequence: u64) -> Self {
        Self { sequence }
    }

    pub fn zero() -> Self {
        Self { sequence: 0 }
    }

    pub fn encode(self) -> String {
        format!("bo_{:016x}", self.sequence)
    }

    pub fn decode(value: &str) -> Option<Self> {
        let encoded = value.strip_prefix("bo_")?;
        if encoded.len() != 16 {
            return None;
        }
        u64::from_str_radix(encoded, 16)
            .ok()
            .map(|sequence| Self { sequence })
    }
}

#[derive(Debug, Default)]
struct BackendObservationState {
    next_sequence: u64,
    history: BTreeMap<ObservationKey, VecDeque<StoredBackendEvent>>,
}

impl BackendObservationState {
    fn new() -> Self {
        Self {
            next_sequence: 1,
            history: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservationKey {
    runtime_id: String,
    worker_id: String,
}

#[derive(Clone, Debug)]
struct StoredBackendEvent {
    sequence: u64,
    runtime_cursor: String,
    envelope: ClientWorkerEventWsEnvelope,
}

#[derive(Clone, Debug)]
pub struct BackendObservationOpen {
    pub replay: Vec<ClientWorkerEventWsEnvelope>,
    pub runtime_cursor: Option<String>,
    pub backend_cursor: BackendObservationCursor,
}

/// Backend-owned in-memory v0 observation proxy state.
#[derive(Clone)]
pub struct BackendObservationProxy {
    sources: Arc<BTreeMap<ObservationKey, RuntimeObservationSourceConfig>>,
    state: Arc<Mutex<BackendObservationState>>,
}

impl std::fmt::Debug for BackendObservationProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendObservationProxy")
            .field("source_count", &self.sources.len())
            .field("state", &"<omitted>")
            .finish()
    }
}

impl BackendObservationProxy {
    pub fn new(sources: Vec<RuntimeObservationSourceConfig>) -> Self {
        let sources = sources
            .into_iter()
            .map(|source| {
                (
                    ObservationKey {
                        runtime_id: source.runtime_id.clone(),
                        worker_id: source.worker_id.clone(),
                    },
                    source,
                )
            })
            .collect();
        Self {
            sources: Arc::new(sources),
            state: Arc::new(Mutex::new(BackendObservationState::new())),
        }
    }

    pub fn source(
        &self,
        runtime_id: &str,
        worker_id: &str,
    ) -> Result<RuntimeObservationSourceConfig, ObservationProxyError> {
        self.sources
            .get(&ObservationKey {
                runtime_id: runtime_id.to_string(),
                worker_id: worker_id.to_string(),
            })
            .cloned()
            .ok_or_else(|| {
                ObservationProxyError::WorkerNotFound(format!(
                    "worker {worker_id} is not registered for runtime {runtime_id}"
                ))
            })
    }

    pub fn open(
        &self,
        runtime_id: &str,
        worker_id: &str,
        cursor: Option<&str>,
    ) -> Result<BackendObservationOpen, ObservationProxyError> {
        let key = ObservationKey {
            runtime_id: runtime_id.to_string(),
            worker_id: worker_id.to_string(),
        };
        let cursor = match cursor {
            Some(raw) => BackendObservationCursor::decode(raw).ok_or_else(|| {
                ObservationProxyError::CursorMalformed(format!(
                    "malformed backend observation cursor: {raw}"
                ))
            })?,
            None => BackendObservationCursor::zero(),
        };
        let state = self.state.lock().map_err(|_| {
            ObservationProxyError::RuntimeUnavailable(
                "backend observation state lock poisoned".into(),
            )
        })?;
        let history = state.history.get(&key);
        let replay: Vec<_> = history
            .into_iter()
            .flat_map(|events| events.iter())
            .filter(|event| event.sequence > cursor.sequence)
            .cloned()
            .collect();
        if cursor.sequence != 0 {
            let found = history
                .into_iter()
                .flat_map(|events| events.iter())
                .any(|event| event.sequence == cursor.sequence);
            if !found {
                return Err(ObservationProxyError::CursorUnknownOrExpired(format!(
                    "backend observation cursor {} is unknown or expired for runtime {runtime_id} worker {worker_id}",
                    cursor.encode()
                )));
            }
        }
        let runtime_cursor = replay
            .last()
            .map(|event| event.runtime_cursor.clone())
            .or_else(|| {
                history.and_then(|events| {
                    events
                        .iter()
                        .find(|event| event.sequence == cursor.sequence)
                        .map(|event| event.runtime_cursor.clone())
                })
            });
        Ok(BackendObservationOpen {
            replay: replay.into_iter().map(|event| event.envelope).collect(),
            runtime_cursor,
            backend_cursor: cursor,
        })
    }

    pub fn store(
        &self,
        event: RuntimeObservationUpstreamEvent,
    ) -> Result<ClientWorkerEventWsEnvelope, ObservationProxyError> {
        let mut state = self.state.lock().map_err(|_| {
            ObservationProxyError::RuntimeUnavailable(
                "backend observation state lock poisoned".into(),
            )
        })?;
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        let cursor = BackendObservationCursor::new(sequence).encode();
        let envelope = ClientWorkerEventWsEnvelope {
            cursor: cursor.clone(),
            event_id: cursor,
            runtime_id: event.runtime_id.clone(),
            worker_id: event.worker_id.clone(),
            payload: event.payload,
        };
        let key = ObservationKey {
            runtime_id: event.runtime_id,
            worker_id: event.worker_id,
        };
        let history = state.history.entry(key).or_default();
        history.push_back(StoredBackendEvent {
            sequence,
            runtime_cursor: event.runtime_cursor,
            envelope: envelope.clone(),
        });
        while history.len() > 1024 {
            history.pop_front();
        }
        Ok(envelope)
    }
}

fn map_runtime_connect_error(error: TungsteniteError) -> ObservationProxyError {
    match error {
        TungsteniteError::Http(response) if response.status() == StatusCode::NOT_FOUND => {
            ObservationProxyError::WorkerNotFound(
                "runtime worker observation endpoint returned 404 not found".into(),
            )
        }
        TungsteniteError::Http(response) if response.status() == StatusCode::BAD_REQUEST => {
            ObservationProxyError::CursorMalformed(
                "runtime worker observation endpoint rejected the request as malformed".into(),
            )
        }
        TungsteniteError::Http(response) => ObservationProxyError::RuntimeUnavailable(format!(
            "runtime worker observation endpoint rejected WebSocket upgrade with status {}",
            response.status()
        )),
        error => ObservationProxyError::RuntimeUnavailable(format!(
            "failed to connect runtime WebSocket: {error}"
        )),
    }
}

fn map_runtime_diagnostic(code: String, message: String) -> ObservationProxyError {
    match code.as_str() {
        "runtime.worker_not_found" => ObservationProxyError::WorkerNotFound(message),
        "runtime.cursor_malformed" => ObservationProxyError::CursorMalformed(message),
        "runtime.cursor_unknown_or_expired" | "runtime.cursor_expired" => {
            ObservationProxyError::CursorUnknownOrExpired(message)
        }
        "runtime.unavailable" => ObservationProxyError::RuntimeUnavailable(message),
        "runtime.upstream_closed" | "runtime.websocket_error" => {
            ObservationProxyError::UpstreamDisconnect(message)
        }
        "runtime.serialize_failed" => ObservationProxyError::MalformedFrame(message),
        "runtime.observation_only" => ObservationProxyError::ObservationOnly,
        _ => ObservationProxyError::RuntimeUnavailable(format!(
            "runtime diagnostic {code}: {message}"
        )),
    }
}

pub struct RuntimeWsObservationClient {
    runtime_id: String,
    worker_id: String,
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl RuntimeWsObservationClient {
    pub async fn connect(
        source: &RuntimeObservationSourceConfig,
        runtime_cursor: Option<&str>,
    ) -> Result<Self, ObservationProxyError> {
        let mut endpoint = source.endpoint.clone();
        if let Some(cursor) = runtime_cursor {
            let separator = if endpoint.contains('?') { '&' } else { '?' };
            endpoint.push(separator);
            endpoint.push_str("cursor=");
            endpoint.push_str(cursor);
        }
        let mut request = endpoint.into_client_request().map_err(|error| {
            ObservationProxyError::RuntimeUnavailable(format!(
                "failed to build runtime WebSocket request: {error}"
            ))
        })?;
        if let Some(token) = &source.bearer_token {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {token}").parse().map_err(|error| {
                    ObservationProxyError::RuntimeUnavailable(format!(
                        "failed to build runtime authorization header: {error}"
                    ))
                })?,
            );
        }
        let (stream, _) = connect_async(request)
            .await
            .map_err(map_runtime_connect_error)?;
        Ok(Self {
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
            stream,
        })
    }

    pub async fn next_event(
        &mut self,
    ) -> Result<RuntimeObservationUpstreamEvent, ObservationProxyError> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Err(ObservationProxyError::UpstreamDisconnect(
                    "runtime WebSocket closed".into(),
                ));
            };
            let message = message.map_err(|error| {
                ObservationProxyError::UpstreamDisconnect(format!(
                    "runtime WebSocket receive error: {error}"
                ))
            })?;
            let text = match message {
                TungsteniteMessage::Text(text) => text,
                TungsteniteMessage::Close(_) => {
                    return Err(ObservationProxyError::UpstreamDisconnect(
                        "runtime WebSocket closed".into(),
                    ));
                }
                TungsteniteMessage::Ping(payload) => {
                    self.stream
                        .send(TungsteniteMessage::Pong(payload))
                        .await
                        .map_err(|error| {
                            ObservationProxyError::UpstreamDisconnect(format!(
                                "failed to reply to runtime ping: {error}"
                            ))
                        })?;
                    continue;
                }
                TungsteniteMessage::Pong(_) => continue,
                TungsteniteMessage::Binary(_) | TungsteniteMessage::Frame(_) => {
                    return Err(ObservationProxyError::MalformedFrame(
                        "runtime sent a non-text observation frame".into(),
                    ));
                }
            };
            let frame: RuntimeWorkerEventWsFrame =
                serde_json::from_str(&text).map_err(|error| {
                    ObservationProxyError::MalformedFrame(format!(
                        "failed to decode runtime observation frame: {error}"
                    ))
                })?;
            match frame {
                RuntimeWorkerEventWsFrame::Event { envelope } => {
                    return Ok(self.map_envelope(envelope));
                }
                RuntimeWorkerEventWsFrame::Diagnostic { diagnostic } => {
                    return Err(map_runtime_diagnostic(diagnostic.code, diagnostic.message));
                }
            }
        }
    }

    fn map_envelope(
        &self,
        envelope: RuntimeWorkerEventWsEnvelope,
    ) -> RuntimeObservationUpstreamEvent {
        RuntimeObservationUpstreamEvent {
            runtime_id: self.runtime_id.clone(),
            worker_id: self.worker_id.clone(),
            runtime_cursor: envelope.cursor,
            payload: envelope.payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensitive_source() -> RuntimeObservationSourceConfig {
        RuntimeObservationSourceConfig {
            runtime_id: "remote-runtime".to_string(),
            worker_id: "worker-1".to_string(),
            endpoint: "wss://remote.example.invalid/private/workers/worker-1/events/ws".to_string(),
            bearer_token: Some("top-secret-bearer-token".to_string()),
        }
    }

    #[test]
    fn runtime_observation_source_debug_redacts_endpoint_and_token() {
        let debug = format!("{:?}", sensitive_source());

        assert!(debug.contains("remote-runtime"));
        assert!(debug.contains("worker-1"));
        assert!(debug.contains("<backend-private>"));
        assert!(debug.contains("<redacted>"));
        for forbidden in [
            "remote.example.invalid",
            "/private/workers/worker-1/events/ws",
            "top-secret-bearer-token",
        ] {
            assert!(
                !debug.contains(forbidden),
                "debug leaked {forbidden}: {debug}"
            );
        }
    }

    #[test]
    fn backend_observation_proxy_debug_redacts_contained_sources() {
        let proxy = BackendObservationProxy::new(vec![sensitive_source()]);
        let debug = format!("{proxy:?}");

        assert!(debug.contains("BackendObservationProxy"));
        assert!(debug.contains("source_count"));
        for forbidden in [
            "remote.example.invalid",
            "/private/workers/worker-1/events/ws",
            "top-secret-bearer-token",
        ] {
            assert!(
                !debug.contains(forbidden),
                "debug leaked {forbidden}: {debug}"
            );
        }
    }
}
