//! Shared subscription-multiplexer wire contract.
//!
//! This protocol is distinct from the single-Worker [`crate::Method`] / [`crate::Event`]
//! protocol. A connection carries many independently managed subscriptions. The
//! connection endpoint owns authorization and maps the typed selectors below to
//! visible resources; clients cannot provide Workspace scope as selector input.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::Event as WorkerProtocolEvent;

pub const SUBSCRIPTION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_CORRELATION_ID_BYTES: usize = 128;
pub const MAX_RESOURCE_ID_BYTES: usize = 256;
pub const MAX_WORKER_IDS_PER_SELECTOR: usize = 256;
pub const MAX_REJECTION_MESSAGE_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionValidationError {
    UnsupportedProtocolVersion {
        actual: u16,
    },
    EmptyIdentifier {
        field: &'static str,
    },
    IdentifierTooLong {
        field: &'static str,
        max_bytes: usize,
    },
    InvalidIdentifier {
        field: &'static str,
    },
    EmptyWorkerSelection,
    TooManyWorkers {
        max: usize,
    },
    DuplicateWorkerId {
        worker_id: String,
    },
    EmptyRejectionMessage,
    RejectionMessageTooLong {
        max_bytes: usize,
    },
    SelectorSnapshotMismatch,
    SelectorEventMismatch,
    UnselectedWorker {
        worker_id: String,
    },
}

impl fmt::Display for SubscriptionValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion { actual } => write!(
                formatter,
                "unsupported subscription protocol version {actual}; expected {SUBSCRIPTION_PROTOCOL_VERSION}"
            ),
            Self::EmptyIdentifier { field } => write!(formatter, "{field} must not be empty"),
            Self::IdentifierTooLong { field, max_bytes } => {
                write!(formatter, "{field} exceeds {max_bytes} bytes")
            }
            Self::InvalidIdentifier { field } => write!(
                formatter,
                "{field} must be trimmed and must not contain control characters"
            ),
            Self::EmptyWorkerSelection => {
                formatter.write_str("worker_lifecycle requires at least one worker id")
            }
            Self::TooManyWorkers { max } => {
                write!(formatter, "worker selector exceeds {max} worker ids")
            }
            Self::DuplicateWorkerId { worker_id } => {
                write!(
                    formatter,
                    "worker selector contains duplicate id {worker_id:?}"
                )
            }
            Self::EmptyRejectionMessage => {
                formatter.write_str("subscription rejection message must not be empty")
            }
            Self::RejectionMessageTooLong { max_bytes } => write!(
                formatter,
                "subscription rejection message exceeds {max_bytes} bytes"
            ),
            Self::SelectorSnapshotMismatch => {
                formatter.write_str("subscription snapshot does not match its selector")
            }
            Self::SelectorEventMismatch => {
                formatter.write_str("subscription event does not match its selector")
            }
            Self::UnselectedWorker { worker_id } => {
                write!(formatter, "worker {worker_id:?} is outside the selector")
            }
        }
    }
}

impl std::error::Error for SubscriptionValidationError {}

macro_rules! bounded_identifier {
    ($name:ident, $field:literal, $max:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, SubscriptionValidationError> {
                let value = value.into();
                validate_identifier($field, &value, $max)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }

            pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
                validate_identifier($field, &self.0, $max)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

bounded_identifier!(
    SubscriptionRequestId,
    "request_id",
    MAX_CORRELATION_ID_BYTES
);
bounded_identifier!(SubscriptionId, "subscription_id", MAX_CORRELATION_ID_BYTES);
bounded_identifier!(SubscriptionWorkerId, "worker_id", MAX_RESOURCE_ID_BYTES);
bounded_identifier!(
    SubscriptionWorkdirId,
    "working_directory_id",
    MAX_RESOURCE_ID_BYTES
);

fn validate_identifier(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SubscriptionValidationError> {
    if value.is_empty() {
        return Err(SubscriptionValidationError::EmptyIdentifier { field });
    }
    if value.len() > max_bytes {
        return Err(SubscriptionValidationError::IdentifierTooLong { field, max_bytes });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(SubscriptionValidationError::InvalidIdentifier { field });
    }
    Ok(())
}

fn validate_rejection_message(message: &str) -> Result<(), SubscriptionValidationError> {
    if message.is_empty() {
        return Err(SubscriptionValidationError::EmptyRejectionMessage);
    }
    if message.len() > MAX_REJECTION_MESSAGE_BYTES {
        return Err(SubscriptionValidationError::RejectionMessageTooLong {
            max_bytes: MAX_REJECTION_MESSAGE_BYTES,
        });
    }
    Ok(())
}

/// A bounded, canonical worker-id set. Ordering on the wire and in equality/hash
/// semantics is lexical, so equivalent selectors aggregate to one upstream key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubscriptionWorkerIds(Vec<SubscriptionWorkerId>);

impl SubscriptionWorkerIds {
    pub fn new(
        worker_ids: impl IntoIterator<Item = SubscriptionWorkerId>,
    ) -> Result<Self, SubscriptionValidationError> {
        let mut worker_ids = worker_ids.into_iter().collect::<Vec<_>>();
        if worker_ids.is_empty() {
            return Err(SubscriptionValidationError::EmptyWorkerSelection);
        }
        if worker_ids.len() > MAX_WORKER_IDS_PER_SELECTOR {
            return Err(SubscriptionValidationError::TooManyWorkers {
                max: MAX_WORKER_IDS_PER_SELECTOR,
            });
        }
        for worker_id in &worker_ids {
            worker_id.validate()?;
        }
        worker_ids.sort_unstable();
        for pair in worker_ids.windows(2) {
            if pair[0] == pair[1] {
                return Err(SubscriptionValidationError::DuplicateWorkerId {
                    worker_id: pair[0].to_string(),
                });
            }
        }
        Ok(Self(worker_ids))
    }

    pub fn as_slice(&self) -> &[SubscriptionWorkerId] {
        &self.0
    }

    pub fn contains(&self, worker_id: &SubscriptionWorkerId) -> bool {
        self.0.binary_search(worker_id).is_ok()
    }

    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

impl<'de> Deserialize<'de> for SubscriptionWorkerIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let worker_ids = Vec::<SubscriptionWorkerId>::deserialize(deserializer)?;
        Self::new(worker_ids).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "topic", rename_all = "snake_case")]
pub enum EventSubscriptionSelector {
    RuntimeWorkers,
    WorkerLifecycle {
        worker_ids: SubscriptionWorkerIds,
    },
    WorkerProtocol {
        worker_id: SubscriptionWorkerId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_id: Option<String>,
    },
    /// Server-derived Workspace projection. Workspace identity comes from the
    /// authenticated connection and is deliberately absent from this selector.
    WorkspaceWorkers,
    /// Typed extension point for the later Workdir subscription slice.
    WorkspaceWorkdirs,
}

impl EventSubscriptionSelector {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::WorkerLifecycle { worker_ids } => worker_ids.validate(),
            Self::WorkerProtocol {
                worker_id,
                runtime_id,
            } => {
                worker_id.validate()?;
                if let Some(runtime_id) = runtime_id {
                    validate_identifier("runtime_id", runtime_id, MAX_RESOURCE_ID_BYTES)?;
                }
                Ok(())
            }
            Self::RuntimeWorkers | Self::WorkspaceWorkers | Self::WorkspaceWorkdirs => Ok(()),
        }
    }

    pub fn selects_worker(&self, worker_id: &SubscriptionWorkerId) -> bool {
        match self {
            Self::RuntimeWorkers | Self::WorkspaceWorkers => true,
            Self::WorkerLifecycle { worker_ids } => worker_ids.contains(worker_id),
            Self::WorkerProtocol {
                worker_id: selected,
                ..
            } => selected == worker_id,
            Self::WorkspaceWorkdirs => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubscriptionFrame {
    pub protocol_version: u16,
    #[serde(flatten)]
    pub payload: SubscriptionFramePayload,
}

impl SubscriptionFrame {
    pub fn new(payload: SubscriptionFramePayload) -> Self {
        Self {
            protocol_version: SUBSCRIPTION_PROTOCOL_VERSION,
            payload,
        }
    }

    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        if self.protocol_version != SUBSCRIPTION_PROTOCOL_VERSION {
            return Err(SubscriptionValidationError::UnsupportedProtocolVersion {
                actual: self.protocol_version,
            });
        }
        self.payload.validate()
    }
}

/// Direction is explicit at the outer envelope, while concrete message kinds
/// remain typed within each lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "frame", content = "message", rename_all = "snake_case")]
pub enum SubscriptionFramePayload {
    Request(SubscriptionRequest),
    Response(SubscriptionResponse),
    Event(SubscriptionEvent),
    WorkerProtocol(SubscriptionWorkerProtocolMethod),
}

impl SubscriptionFramePayload {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::Request(request) => request.validate(),
            Self::Response(response) => response.validate(),
            Self::Event(event) => event.validate(),
            Self::WorkerProtocol(message) => message.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubscriptionWorkerProtocolMethod {
    pub subscription_id: SubscriptionId,
    pub method: crate::Method,
}

impl SubscriptionWorkerProtocolMethod {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        self.subscription_id.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum SubscriptionRequest {
    SubscribeEvents {
        request_id: SubscriptionRequestId,
        selector: EventSubscriptionSelector,
    },
    UnsubscribeEvents {
        request_id: SubscriptionRequestId,
        subscription_id: SubscriptionId,
    },
}

impl SubscriptionRequest {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::SubscribeEvents {
                request_id,
                selector,
            } => {
                request_id.validate()?;
                selector.validate()
            }
            Self::UnsubscribeEvents {
                request_id,
                subscription_id,
            } => {
                request_id.validate()?;
                subscription_id.validate()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "result", content = "payload", rename_all = "snake_case")]
pub enum SubscriptionResponse {
    Subscribed {
        request_id: SubscriptionRequestId,
        subscription_id: SubscriptionId,
        selector: EventSubscriptionSelector,
        snapshot_revision: u64,
        snapshot: SubscriptionSnapshot,
    },
    Unsubscribed {
        request_id: SubscriptionRequestId,
        subscription_id: SubscriptionId,
    },
    SubscriptionRejected {
        request_id: SubscriptionRequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subscription_id: Option<SubscriptionId>,
        code: SubscriptionRejectionCode,
        message: String,
    },
}

impl SubscriptionResponse {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::Subscribed {
                request_id,
                subscription_id,
                selector,
                snapshot,
                ..
            } => {
                request_id.validate()?;
                subscription_id.validate()?;
                selector.validate()?;
                snapshot.validate_for_selector(selector)
            }
            Self::Unsubscribed {
                request_id,
                subscription_id,
            } => {
                request_id.validate()?;
                subscription_id.validate()
            }
            Self::SubscriptionRejected {
                request_id,
                subscription_id,
                message,
                ..
            } => {
                request_id.validate()?;
                if let Some(subscription_id) = subscription_id {
                    subscription_id.validate()?;
                }
                validate_rejection_message(message)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionRejectionCode {
    InvalidRequest,
    UnsupportedProtocolVersion,
    UnsupportedSelector,
    Unauthorized,
    ResourceNotFound,
    CapacityExceeded,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionTerminationCode {
    Lagged,
    ResourceGone,
    Unauthorized,
    ServerShutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SubscriptionEvent {
    Event {
        subscription_id: SubscriptionId,
        subject_revision: u64,
        payload: SubscriptionEventPayload,
    },
    SubscriptionClosed {
        subscription_id: SubscriptionId,
        code: SubscriptionTerminationCode,
        message: String,
    },
}

impl SubscriptionEvent {
    pub fn subscription_id(&self) -> &SubscriptionId {
        match self {
            Self::Event {
                subscription_id, ..
            }
            | Self::SubscriptionClosed {
                subscription_id, ..
            } => subscription_id,
        }
    }

    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::Event {
                subscription_id,
                payload,
                ..
            } => {
                subscription_id.validate()?;
                payload.validate()
            }
            Self::SubscriptionClosed {
                subscription_id,
                message,
                ..
            } => {
                subscription_id.validate()?;
                validate_rejection_message(message)
            }
        }
    }

    pub fn validate_for_selector(
        &self,
        selector: &EventSubscriptionSelector,
    ) -> Result<(), SubscriptionValidationError> {
        self.validate()?;
        match self {
            Self::Event { payload, .. } => payload.validate_for_selector(selector),
            Self::SubscriptionClosed { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionWorkerState {
    Idle,
    Running,
    Paused,
    Stopped,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubscriptionWorker {
    pub worker_id: SubscriptionWorkerId,
    /// Set by the Workspace Server when projecting a Runtime-owned Worker to clients.
    /// Runtime producers leave this unset because the connection identifies the Runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    /// Producer-owned monotonic revision for this Worker subject.
    pub subject_revision: u64,
    pub state: SubscriptionWorkerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_id: Option<SubscriptionWorkdirId>,
}

impl SubscriptionWorker {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        self.worker_id.validate()?;
        if let Some(runtime_id) = &self.runtime_id {
            validate_identifier("runtime_id", runtime_id, MAX_RESOURCE_ID_BYTES)?;
        }
        if let Some(repository_id) = &self.repository_id {
            validate_identifier("repository_id", repository_id, MAX_RESOURCE_ID_BYTES)?;
        }
        if let Some(working_directory_id) = &self.working_directory_id {
            working_directory_id.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SubscriptionWorkdir {
    pub working_directory_id: SubscriptionWorkdirId,
    pub repository_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<SubscriptionWorkerId>,
}

impl SubscriptionWorkdir {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        self.working_directory_id.validate()?;
        validate_identifier("repository_id", &self.repository_id, MAX_RESOURCE_ID_BYTES)?;
        validate_identifier("workdir_state", &self.state, MAX_RESOURCE_ID_BYTES)?;
        if let Some(worker_id) = &self.primary_worker_id {
            worker_id.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "topic", content = "data", rename_all = "snake_case")]
pub enum SubscriptionSnapshot {
    Workers {
        workers: Vec<SubscriptionWorker>,
    },
    WorkerProtocol {
        worker_id: SubscriptionWorkerId,
        events: Vec<WorkerProtocolEvent>,
    },
    WorkspaceWorkdirs {
        workdirs: Vec<SubscriptionWorkdir>,
    },
}

impl SubscriptionSnapshot {
    pub fn validate_for_selector(
        &self,
        selector: &EventSubscriptionSelector,
    ) -> Result<(), SubscriptionValidationError> {
        selector.validate()?;
        match (selector, self) {
            (
                EventSubscriptionSelector::RuntimeWorkers
                | EventSubscriptionSelector::WorkspaceWorkers,
                Self::Workers { workers },
            ) => validate_workers(workers),
            (
                EventSubscriptionSelector::WorkerLifecycle { worker_ids },
                Self::Workers { workers },
            ) => {
                validate_workers(workers)?;
                for worker in workers {
                    if !worker_ids.contains(&worker.worker_id) {
                        return Err(SubscriptionValidationError::UnselectedWorker {
                            worker_id: worker.worker_id.to_string(),
                        });
                    }
                }
                Ok(())
            }
            (
                EventSubscriptionSelector::WorkerProtocol {
                    worker_id: selected,
                    ..
                },
                Self::WorkerProtocol { worker_id, .. },
            ) if selected == worker_id => worker_id.validate(),
            (
                EventSubscriptionSelector::WorkspaceWorkdirs,
                Self::WorkspaceWorkdirs { workdirs },
            ) => {
                for workdir in workdirs {
                    workdir.validate()?;
                }
                Ok(())
            }
            _ => Err(SubscriptionValidationError::SelectorSnapshotMismatch),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SubscriptionEventPayload {
    WorkerUpserted {
        worker: SubscriptionWorker,
    },
    WorkerRemoved {
        worker_id: SubscriptionWorkerId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_id: Option<String>,
    },
    WorkerProtocol {
        worker_id: SubscriptionWorkerId,
        event: WorkerProtocolEvent,
    },
    WorkdirUpserted {
        workdir: SubscriptionWorkdir,
    },
    WorkdirRemoved {
        working_directory_id: SubscriptionWorkdirId,
    },
}

impl SubscriptionEventPayload {
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        match self {
            Self::WorkerUpserted { worker } => worker.validate(),
            Self::WorkerRemoved {
                worker_id,
                runtime_id,
            } => {
                worker_id.validate()?;
                if let Some(runtime_id) = runtime_id {
                    validate_identifier("runtime_id", runtime_id, MAX_RESOURCE_ID_BYTES)?;
                }
                Ok(())
            }
            Self::WorkerProtocol { worker_id, .. } => worker_id.validate(),
            Self::WorkdirUpserted { workdir } => workdir.validate(),
            Self::WorkdirRemoved {
                working_directory_id,
            } => working_directory_id.validate(),
        }
    }

    pub fn validate_for_selector(
        &self,
        selector: &EventSubscriptionSelector,
    ) -> Result<(), SubscriptionValidationError> {
        selector.validate()?;
        self.validate()?;
        match (selector, self) {
            (
                EventSubscriptionSelector::RuntimeWorkers
                | EventSubscriptionSelector::WorkspaceWorkers,
                Self::WorkerUpserted { .. } | Self::WorkerRemoved { .. },
            ) => Ok(()),
            (
                EventSubscriptionSelector::WorkerLifecycle { worker_ids },
                Self::WorkerUpserted { worker },
            ) if worker_ids.contains(&worker.worker_id) => Ok(()),
            (
                EventSubscriptionSelector::WorkerLifecycle { worker_ids },
                Self::WorkerRemoved { worker_id, .. },
            ) if worker_ids.contains(worker_id) => Ok(()),
            (
                EventSubscriptionSelector::WorkerProtocol {
                    worker_id: selected,
                    ..
                },
                Self::WorkerProtocol { worker_id, .. },
            ) if selected == worker_id => Ok(()),
            (
                EventSubscriptionSelector::WorkspaceWorkdirs,
                Self::WorkdirUpserted { .. } | Self::WorkdirRemoved { .. },
            ) => Ok(()),
            (
                EventSubscriptionSelector::WorkerLifecycle { .. },
                Self::WorkerUpserted { worker },
            ) => Err(SubscriptionValidationError::UnselectedWorker {
                worker_id: worker.worker_id.to_string(),
            }),
            (
                EventSubscriptionSelector::WorkerLifecycle { .. },
                Self::WorkerRemoved { worker_id, .. },
            ) => Err(SubscriptionValidationError::UnselectedWorker {
                worker_id: worker_id.to_string(),
            }),
            _ => Err(SubscriptionValidationError::SelectorEventMismatch),
        }
    }
}

fn validate_workers(workers: &[SubscriptionWorker]) -> Result<(), SubscriptionValidationError> {
    let mut seen = HashSet::with_capacity(workers.len());
    for worker in workers {
        worker.validate()?;
        if !seen.insert((worker.runtime_id.as_deref(), &worker.worker_id)) {
            return Err(SubscriptionValidationError::DuplicateWorkerId {
                worker_id: worker.worker_id.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker_id(value: &str) -> SubscriptionWorkerId {
        SubscriptionWorkerId::new(value).unwrap()
    }

    fn request_id() -> SubscriptionRequestId {
        SubscriptionRequestId::new("request-1").unwrap()
    }

    fn subscription_id() -> SubscriptionId {
        SubscriptionId::new("subscription-1").unwrap()
    }

    fn worker(value: &str) -> SubscriptionWorker {
        SubscriptionWorker {
            worker_id: worker_id(value),
            runtime_id: None,
            subject_revision: 0,
            state: SubscriptionWorkerState::Idle,
            workspace_id: Some("workspace-1".to_string()),
            display_name: Some(format!("Worker {value}")),
            profile: Some("builtin:coder".to_string()),
            repository_id: None,
            working_directory_id: None,
        }
    }

    #[test]
    fn subscribe_frame_has_stable_versioned_json_shape() {
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::Request(
            SubscriptionRequest::SubscribeEvents {
                request_id: request_id(),
                selector: EventSubscriptionSelector::WorkerLifecycle {
                    worker_ids: SubscriptionWorkerIds::new([
                        worker_id("worker-2"),
                        worker_id("worker-1"),
                    ])
                    .unwrap(),
                },
            },
        ));

        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "protocol_version": 1,
                "frame": "request",
                "message": {
                    "method": "subscribe_events",
                    "params": {
                        "request_id": "request-1",
                        "selector": {
                            "topic": "worker_lifecycle",
                            "worker_ids": ["worker-1", "worker-2"]
                        }
                    }
                }
            })
        );
        frame.validate().unwrap();
        let decoded: SubscriptionFrame = serde_json::from_value(json).unwrap();
        decoded.validate().unwrap();
    }

    #[test]
    fn subscribed_snapshot_round_trips_and_matches_selector() {
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::Response(
            SubscriptionResponse::Subscribed {
                request_id: request_id(),
                subscription_id: subscription_id(),
                selector: EventSubscriptionSelector::RuntimeWorkers,
                snapshot_revision: 7,
                snapshot: SubscriptionSnapshot::Workers {
                    workers: vec![worker("worker-1")],
                },
            },
        ));

        frame.validate().unwrap();
        let json = serde_json::to_string(&frame).unwrap();
        let decoded: SubscriptionFrame = serde_json::from_str(&json).unwrap();
        decoded.validate().unwrap();
    }

    #[test]
    fn worker_selector_is_canonical_and_rejects_duplicates() {
        let first = SubscriptionWorkerIds::new([worker_id("b"), worker_id("a")]).unwrap();
        let second = SubscriptionWorkerIds::new([worker_id("a"), worker_id("b")]).unwrap();
        assert_eq!(first, second);

        let error = SubscriptionWorkerIds::new([worker_id("a"), worker_id("a")]).unwrap_err();
        assert!(matches!(
            error,
            SubscriptionValidationError::DuplicateWorkerId { .. }
        ));
        assert!(serde_json::from_str::<SubscriptionWorkerIds>("[]").is_err());
    }

    #[test]
    fn validation_rejects_invalid_identifiers_and_versions() {
        assert!(matches!(
            SubscriptionRequestId::new(" "),
            Err(SubscriptionValidationError::InvalidIdentifier { .. })
        ));
        let mut frame = SubscriptionFrame::new(SubscriptionFramePayload::Request(
            SubscriptionRequest::UnsubscribeEvents {
                request_id: request_id(),
                subscription_id: subscription_id(),
            },
        ));
        frame.protocol_version = 2;
        assert!(matches!(
            frame.validate(),
            Err(SubscriptionValidationError::UnsupportedProtocolVersion { actual: 2 })
        ));
    }

    #[test]
    fn snapshot_and_events_must_match_selector() {
        let selected = EventSubscriptionSelector::WorkerLifecycle {
            worker_ids: SubscriptionWorkerIds::new([worker_id("worker-1")]).unwrap(),
        };
        let snapshot = SubscriptionSnapshot::Workers {
            workers: vec![worker("worker-2")],
        };
        assert!(matches!(
            snapshot.validate_for_selector(&selected),
            Err(SubscriptionValidationError::UnselectedWorker { .. })
        ));

        let event = SubscriptionEvent::Event {
            subscription_id: subscription_id(),
            subject_revision: 8,
            payload: SubscriptionEventPayload::WorkerRemoved {
                worker_id: worker_id("worker-2"),
                runtime_id: None,
            },
        };
        assert!(matches!(
            event.validate_for_selector(&selected),
            Err(SubscriptionValidationError::UnselectedWorker { .. })
        ));
        assert!(matches!(
            event.validate_for_selector(&EventSubscriptionSelector::WorkspaceWorkdirs),
            Err(SubscriptionValidationError::SelectorEventMismatch)
        ));
    }

    #[test]
    fn worker_protocol_method_uses_subscription_lane() {
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::WorkerProtocol(
            SubscriptionWorkerProtocolMethod {
                subscription_id: subscription_id(),
                method: crate::Method::ListCompletions {
                    kind: crate::CompletionKind::File,
                    prefix: "src/".to_string(),
                },
            },
        ));
        frame.validate().unwrap();
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            serde_json::json!({
                "protocol_version": 1,
                "frame": "worker_protocol",
                "message": {
                    "subscription_id": "subscription-1",
                    "method": {
                        "method": "list_completions",
                        "params": {
                            "kind": "file",
                            "prefix": "src/"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn workspace_snapshot_allows_equal_local_worker_ids_from_distinct_runtimes() {
        let mut first = worker("1");
        first.runtime_id = Some("runtime-a".to_string());
        let mut second = worker("1");
        second.runtime_id = Some("runtime-b".to_string());
        SubscriptionSnapshot::Workers {
            workers: vec![first, second],
        }
        .validate_for_selector(&EventSubscriptionSelector::WorkspaceWorkers)
        .unwrap();
    }

    #[test]
    fn subscription_closed_is_a_typed_server_event() {
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::Event(
            SubscriptionEvent::SubscriptionClosed {
                subscription_id: subscription_id(),
                code: SubscriptionTerminationCode::Lagged,
                message: "resubscribe".to_string(),
            },
        ));
        frame.validate().unwrap();
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            serde_json::json!({
                "protocol_version": 1,
                "frame": "event",
                "message": {
                    "event": "subscription_closed",
                    "data": {
                        "subscription_id": "subscription-1",
                        "code": "lagged",
                        "message": "resubscribe"
                    }
                }
            })
        );
    }

    #[test]
    fn client_selector_has_no_workspace_scope_field() {
        let json = serde_json::to_value(EventSubscriptionSelector::WorkspaceWorkers).unwrap();
        assert_eq!(json, serde_json::json!({ "topic": "workspace_workers" }));
        assert!(json.get("workspace_id").is_none());
    }
}
