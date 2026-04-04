//! Debug-only raw stream event recording.
//!
//! [`TraceEntry`] captures every LLM stream event verbatim for debugging
//! and replay analysis. Written to a separate `.trace.jsonl` file,
//! completely independent of the session log used for state restoration.
//!
//! Disabled by default. Enable via `SessionConfig::record_event_trace`.

use llm_worker::llm_client::event::Event;
use serde::{Deserialize, Serialize};

/// A single trace entry recording a raw stream event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Timestamp in milliseconds since Unix epoch.
    pub ts: u64,
    /// Turn number at the time of recording.
    pub turn: usize,
    /// The raw stream event.
    pub event: Event,
}
