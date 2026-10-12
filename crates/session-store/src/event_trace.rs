//! Diagnostic records of LLM transport, independent of history replay.
//!
//! [`TraceEntry`] records normalized events and lifecycle boundaries, not a complete
//! copy of raw SSE. Stored separately from Segment logs, it is not a source for
//! restored model input. This separates observed transport state from input/output
//! committed to history.

use agen::llm_client::event::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single trace entry recording either a lifecycle marker or normalized stream event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceEntry {
    /// Timestamp in milliseconds since Unix epoch.
    pub ts: u64,
    /// Turn number at the time of recording.
    pub turn: usize,
    /// LLM call index within the worker, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_call: Option<usize>,
    #[serde(flatten)]
    pub payload: TracePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TracePayload {
    /// Normalized provider stream event.
    StreamEvent { event: Event },
    /// Marker for code that runs before/around provider stream events.
    Lifecycle {
        label: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        data: Value,
    },
}
