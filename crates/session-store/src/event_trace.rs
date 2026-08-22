//! Debug-only LLM request/stream trace recording.
//!
//! [`TraceEntry`] captures stream lifecycle markers and normalized provider
//! stream events for debugging stalls. It is not a byte-for-byte raw SSE
//! capture. Written to a separate `.trace.jsonl` file,
//! completely independent of the segment log used for state restoration.
//!
//! Disabled by default. Enable via `SessionConfig::record_event_trace`.

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
