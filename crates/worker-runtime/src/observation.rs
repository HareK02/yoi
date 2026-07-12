use crate::identity::{RuntimeId, WorkerRef};
use serde::{Deserialize, Serialize};

/// Event cursor.  `next_event_id` is the first event id that should be returned
/// by the next poll.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCursor {
    pub runtime_id: RuntimeId,
    pub next_event_id: u64,
}

/// Placeholder subscription handle for future streaming APIs.  v0 is explicit
/// poll-only so HTTP/WS/SSE dependencies are not pulled into this crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub runtime_id: RuntimeId,
    pub cursor: EventCursor,
    pub mode: EventSubscriptionMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSubscriptionMode {
    PollOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_ref: Option<WorkerRef>,
    pub kind: RuntimeEventKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    RuntimeStarted,
    RuntimeStopped,
    WorkerCreated,
    WorkerInputAccepted,
    WorkerStopped,
    WorkerCancelled,
    WorkerDeleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEventBatch {
    pub runtime_id: RuntimeId,
    pub cursor: EventCursor,
    pub events: Vec<RuntimeEvent>,
    pub has_more: bool,
}

/// Runtime-local cursor for worker-scoped WebSocket observation.
#[cfg(feature = "ws-server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkerObservationCursor {
    pub sequence: u64,
}

#[cfg(feature = "ws-server")]
impl WorkerObservationCursor {
    pub const PREFIX: &'static str = "wo";

    pub fn new(sequence: u64) -> Self {
        Self { sequence }
    }

    pub fn zero() -> Self {
        Self { sequence: 0 }
    }

    pub fn encode(self) -> String {
        format!("{}_{:016x}", Self::PREFIX, self.sequence)
    }

    pub fn decode(value: &str) -> Option<Self> {
        let encoded = value.strip_prefix("wo_")?;
        if encoded.len() != 16 {
            return None;
        }
        u64::from_str_radix(encoded, 16)
            .ok()
            .map(|sequence| Self { sequence })
    }
}

/// One protocol event observed from a runtime Worker.
#[cfg(feature = "ws-server")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerObservationEvent {
    pub cursor: String,
    pub event_id: String,
    pub sequence: u64,
    pub worker_ref: WorkerRef,
    pub payload: protocol::Event,
}

#[cfg(feature = "ws-server")]
impl WorkerObservationEvent {
    pub fn new(sequence: u64, worker_ref: WorkerRef, payload: protocol::Event) -> Self {
        let cursor = WorkerObservationCursor::new(sequence).encode();
        Self {
            event_id: cursor.clone(),
            cursor,
            sequence,
            worker_ref,
            payload,
        }
    }
}
