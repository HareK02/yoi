use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use worker_runtime::identity::WorkerRef;
use worker_runtime::observation::{WorkerObservationCursor, WorkerObservationEvent};

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

#[derive(Clone)]
pub struct EmbeddedRuntimeObservationSource {
    pub runtime_id: String,
    pub worker_id: String,
    pub runtime: worker_runtime::Runtime,
    pub worker_ref: WorkerRef,
}

#[derive(Clone)]
pub enum RuntimeObservationSource {
    RemoteWs(RuntimeObservationSourceConfig),
    Embedded(EmbeddedRuntimeObservationSource),
}

impl RuntimeObservationSource {
    pub fn remote_ws(config: RuntimeObservationSourceConfig) -> Self {
        Self::RemoteWs(config)
    }

    pub fn embedded(source: EmbeddedRuntimeObservationSource) -> Self {
        Self::Embedded(source)
    }

    pub fn runtime_id(&self) -> &str {
        match self {
            Self::RemoteWs(config) => &config.runtime_id,
            Self::Embedded(source) => &source.runtime_id,
        }
    }

    pub fn worker_id(&self) -> &str {
        match self {
            Self::RemoteWs(config) => &config.worker_id,
            Self::Embedded(source) => &source.worker_id,
        }
    }
}

impl std::fmt::Debug for RuntimeObservationSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteWs(config) => formatter
                .debug_struct("RemoteRuntimeObservationSource")
                .field("runtime_id", &config.runtime_id)
                .field("worker_id", &config.worker_id)
                .field("endpoint", &"<backend-private>")
                .field(
                    "bearer_token",
                    &config.bearer_token.as_ref().map(|_| "<redacted>"),
                )
                .finish(),
            Self::Embedded(source) => formatter
                .debug_struct("EmbeddedRuntimeObservationSource")
                .field("runtime_id", &source.runtime_id)
                .field("worker_id", &source.worker_id)
                .finish(),
        }
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
}

impl BackendObservationState {
    fn new() -> Self {
        Self { next_sequence: 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservationKey {
    runtime_id: String,
    worker_id: String,
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
    ) -> Result<RuntimeObservationSource, ObservationProxyError> {
        self.sources
            .get(&ObservationKey {
                runtime_id: runtime_id.to_string(),
                worker_id: worker_id.to_string(),
            })
            .cloned()
            .map(RuntimeObservationSource::remote_ws)
            .ok_or_else(|| {
                ObservationProxyError::WorkerNotFound(format!(
                    "worker {worker_id} is not registered for runtime {runtime_id}"
                ))
            })
    }

    pub fn open(
        &self,
        _runtime_id: &str,
        _worker_id: &str,
        cursor: Option<&str>,
    ) -> Result<(), ObservationProxyError> {
        if let Some(raw) = cursor {
            BackendObservationCursor::decode(raw).ok_or_else(|| {
                ObservationProxyError::CursorMalformed(format!(
                    "malformed backend observation cursor: {raw}"
                ))
            })?;
        }
        Ok(())
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
        Ok(ClientWorkerEventWsEnvelope {
            cursor: cursor.clone(),
            event_id: cursor,
            runtime_id: event.runtime_id,
            worker_id: event.worker_id,
            payload: event.payload,
        })
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

pub enum RuntimeObservationClient {
    RemoteWs(RuntimeWsObservationClient),
    Embedded(EmbeddedObservationClient),
}

impl RuntimeObservationClient {
    pub async fn connect(
        source: &RuntimeObservationSource,
        runtime_cursor: Option<&str>,
    ) -> Result<Self, ObservationProxyError> {
        match source {
            RuntimeObservationSource::RemoteWs(config) => {
                RuntimeWsObservationClient::connect(config, runtime_cursor)
                    .await
                    .map(Self::RemoteWs)
            }
            RuntimeObservationSource::Embedded(source) => {
                EmbeddedObservationClient::connect(source, runtime_cursor).map(Self::Embedded)
            }
        }
    }

    pub async fn next_event(
        &mut self,
    ) -> Result<RuntimeObservationUpstreamEvent, ObservationProxyError> {
        match self {
            Self::RemoteWs(client) => client.next_event().await,
            Self::Embedded(client) => client.next_event().await,
        }
    }
}

pub struct EmbeddedObservationClient {
    runtime_id: String,
    worker_id: String,
    worker_ref: WorkerRef,
    cursor: WorkerObservationCursor,
    receiver: tokio::sync::broadcast::Receiver<WorkerObservationEvent>,
    queued: VecDeque<RuntimeObservationUpstreamEvent>,
}

impl EmbeddedObservationClient {
    fn connect(
        source: &EmbeddedRuntimeObservationSource,
        runtime_cursor: Option<&str>,
    ) -> Result<Self, ObservationProxyError> {
        let cursor = match runtime_cursor {
            Some(raw) => WorkerObservationCursor::decode(raw).ok_or_else(|| {
                ObservationProxyError::CursorMalformed(
                    "embedded runtime cursor is malformed".into(),
                )
            })?,
            None => source
                .runtime
                .worker_observation_cursor_now(&source.worker_ref)
                .map_err(|err| {
                    ObservationProxyError::WorkerNotFound(format!(
                        "embedded Worker '{}' is not observable: {err}",
                        source.worker_id
                    ))
                })?,
        };
        let receiver = source
            .runtime
            .subscribe_worker_observation()
            .map_err(|err| {
                ObservationProxyError::WorkerNotFound(format!(
                    "embedded Worker '{}' observation subscription is unavailable: {err}",
                    source.worker_id
                ))
            })?;
        let mut queued = VecDeque::new();
        if runtime_cursor.is_none() {
            let snapshot = source
                .runtime
                .worker_observation_snapshot(&source.worker_ref)
                .map_err(|err| {
                    ObservationProxyError::WorkerNotFound(format!(
                        "embedded Worker '{}' snapshot is unavailable: {err}",
                        source.worker_id
                    ))
                })?;
            queued.push_back(RuntimeObservationUpstreamEvent {
                runtime_id: source.runtime_id.clone(),
                worker_id: source.worker_id.clone(),
                runtime_cursor: cursor.encode(),
                payload: snapshot,
            });
        }
        for event in source
            .runtime
            .read_worker_observation_events(&source.worker_ref, cursor)
            .map_err(|err| {
                ObservationProxyError::CursorUnknownOrExpired(format!(
                    "embedded Worker '{}' cursor is unavailable: {err}",
                    source.worker_id
                ))
            })?
        {
            queued.push_back(Self::map_event(
                &source.runtime_id,
                &source.worker_id,
                event,
            ));
        }
        Ok(Self {
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
            worker_ref: source.worker_ref.clone(),
            cursor,
            receiver,
            queued,
        })
    }

    async fn next_event(
        &mut self,
    ) -> Result<RuntimeObservationUpstreamEvent, ObservationProxyError> {
        if let Some(event) = self.queued.pop_front() {
            if let Some(cursor) = WorkerObservationCursor::decode(&event.runtime_cursor) {
                self.cursor = cursor;
            }
            return Ok(event);
        }
        loop {
            match self.receiver.recv().await {
                Ok(event)
                    if event.worker_ref == self.worker_ref
                        && event.sequence > self.cursor.sequence =>
                {
                    self.cursor =
                        WorkerObservationCursor::decode(&event.cursor).ok_or_else(|| {
                            ObservationProxyError::CursorMalformed(
                                "embedded runtime emitted a malformed cursor".into(),
                            )
                        })?;
                    return Ok(Self::map_event(&self.runtime_id, &self.worker_id, event));
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    return Err(ObservationProxyError::CursorUnknownOrExpired(
                        "embedded runtime observation backlog was exceeded".into(),
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err(ObservationProxyError::UpstreamDisconnect(
                        "embedded runtime observation stream closed".into(),
                    ));
                }
            }
        }
    }

    fn map_event(
        runtime_id: &str,
        worker_id: &str,
        event: WorkerObservationEvent,
    ) -> RuntimeObservationUpstreamEvent {
        RuntimeObservationUpstreamEvent {
            runtime_id: runtime_id.to_string(),
            worker_id: worker_id.to_string(),
            runtime_cursor: event.cursor,
            payload: event.payload,
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
