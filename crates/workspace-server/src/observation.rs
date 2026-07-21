use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use worker_runtime::identity::WorkerRef;
use worker_runtime::observation::{WorkerObservationCursor, WorkerObservationEvent};

use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as TungsteniteError, Message as TungsteniteMessage};

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
    pub runtime_event_id: String,
    pub payload: protocol::Event,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationProxyError {
    RuntimeUnavailable(String),
    WorkerNotFound(String),
    UpstreamDisconnect(String),
    MalformedFrame(String),
}

impl ObservationProxyError {
    pub fn code(&self) -> &'static str {
        match self {
            ObservationProxyError::RuntimeUnavailable(_) => "backend.runtime_unavailable",
            ObservationProxyError::WorkerNotFound(_) => "backend.worker_not_found",
            ObservationProxyError::UpstreamDisconnect(_) => "backend.upstream_disconnect",
            ObservationProxyError::MalformedFrame(_) => "backend.malformed_frame",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ObservationProxyError::RuntimeUnavailable(message)
            | ObservationProxyError::WorkerNotFound(message)
            | ObservationProxyError::UpstreamDisconnect(message)
            | ObservationProxyError::MalformedFrame(message) => message,
        }
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
}

impl std::fmt::Debug for BackendObservationProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendObservationProxy")
            .field("source_count", &self.sources.len())
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
}

fn map_runtime_connect_error(error: TungsteniteError) -> ObservationProxyError {
    match error {
        TungsteniteError::Http(response) if response.status() == StatusCode::NOT_FOUND => {
            ObservationProxyError::WorkerNotFound(
                "runtime worker observation endpoint returned 404 not found".into(),
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
    ) -> Result<Self, ObservationProxyError> {
        let endpoint = source.endpoint.clone();
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
            let payload: protocol::Event = serde_json::from_str(&text).map_err(|error| {
                ObservationProxyError::MalformedFrame(format!(
                    "failed to decode runtime protocol event frame: {error}"
                ))
            })?;
            return Ok(RuntimeObservationUpstreamEvent {
                runtime_id: self.runtime_id.clone(),
                worker_id: self.worker_id.clone(),
                runtime_event_id: "protocol".to_string(),
                payload,
            });
        }
    }
}

pub enum RuntimeObservationClient {
    RemoteWs(RuntimeWsObservationClient),
    Embedded(EmbeddedObservationClient),
}

impl RuntimeObservationClient {
    pub async fn connect(source: &RuntimeObservationSource) -> Result<Self, ObservationProxyError> {
        match source {
            RuntimeObservationSource::RemoteWs(config) => {
                RuntimeWsObservationClient::connect(config)
                    .await
                    .map(Self::RemoteWs)
            }
            RuntimeObservationSource::Embedded(source) => {
                EmbeddedObservationClient::connect(source).map(Self::Embedded)
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
    fn connect(source: &EmbeddedRuntimeObservationSource) -> Result<Self, ObservationProxyError> {
        let cursor = source
            .runtime
            .worker_observation_cursor_now(&source.worker_ref)
            .map_err(|err| {
                ObservationProxyError::WorkerNotFound(format!(
                    "embedded Worker '{}' is not observable: {err}",
                    source.worker_id
                ))
            })?;
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
            runtime_event_id: "snapshot".to_string(),
            payload: snapshot,
        });
        for event in source
            .runtime
            .read_worker_observation_events(&source.worker_ref, cursor)
            .map_err(|err| {
                ObservationProxyError::RuntimeUnavailable(format!(
                    "embedded Worker '{}' observation cursor is unavailable: {err}",
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
                            ObservationProxyError::RuntimeUnavailable(
                                "embedded runtime emitted a malformed cursor".into(),
                            )
                        })?;
                    return Ok(Self::map_event(&self.runtime_id, &self.worker_id, event));
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    return Err(ObservationProxyError::RuntimeUnavailable(
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
            runtime_event_id: event.cursor.clone(),
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
            endpoint: "wss://remote.example.invalid/private/workers/worker-1/protocol/ws"
                .to_string(),
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
            "/private/workers/worker-1/protocol/ws",
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
            "/private/workers/worker-1/protocol/ws",
            "top-secret-bearer-token",
        ] {
            assert!(
                !debug.contains(forbidden),
                "debug leaked {forbidden}: {debug}"
            );
        }
    }
}
