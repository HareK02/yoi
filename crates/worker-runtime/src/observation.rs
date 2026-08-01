use crate::identity::WorkerRef;
use serde::{Deserialize, Serialize};

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
