//! Tracks per-LLM-request Usage measurements within a Pod run.
//!
//! Bridge between two sync touchpoints in the Worker lifecycle:
//!
//! - **`pre_llm_request` hook** (async, but synchronously accessed via the
//!   tracker): captures `history.len()` at the moment a request goes out.
//! - **`on_usage` callback** (sync closure): receives the aggregated final
//!   `UsageEvent` for that request after the stream completes.
//!
//! Pairing the two yields one `UsageRecord` per LLM call. Pod drains them
//! in `persist_turn` and writes them as `LogEntry::LlmUsage` entries.
//!
//! Multiple LLM calls per Pod run (tool loop) are supported: each call
//! produces its own `(history_len, UsageEvent)` pair, and the records are
//! buffered in chronological order.

use std::sync::Mutex;

use llm_worker::timeline::event::UsageEvent;
use session_store::UsageRecord;

/// Shared between the pre-request hook, the `on_usage` callback, and Pod.
pub(crate) struct UsageTracker {
    /// `history.len()` captured at the most recent `pre_llm_request`.
    /// Cleared when paired with an incoming `on_usage` event.
    pending_history_len: Mutex<Option<usize>>,
    /// Records accumulated during the current run; drained by Pod.
    pending_records: Mutex<Vec<UsageRecord>>,
}

impl UsageTracker {
    pub(crate) fn new() -> Self {
        Self {
            pending_history_len: Mutex::new(None),
            pending_records: Mutex::new(Vec::new()),
        }
    }

    /// Called from a `pre_llm_request` hook with the current history length.
    pub(crate) fn note_request(&self, history_len: usize) {
        *self.pending_history_len.lock().unwrap() = Some(history_len);
    }

    /// Called from the `on_usage` callback with the aggregated final
    /// UsageEvent. If a `history_len` was previously stashed via
    /// `note_request`, builds a `UsageRecord` and pushes it onto the buffer.
    /// If not (e.g. test code that fires Usage outside a request), drops
    /// the event.
    pub(crate) fn record_usage(&self, event: &UsageEvent) {
        let history_len = match self.pending_history_len.lock().unwrap().take() {
            Some(n) => n,
            None => return,
        };
        // UsageEvent.input_tokens は scheme 層で「占有量（プロンプト全長）」に
        // 正規化済みである前提（Anthropic は cache_read + cache_creation を
        // 加算して emit する）。
        let input_total = event.input_tokens.unwrap_or(0);
        let cache_read = event.cache_read_input_tokens.unwrap_or(0);
        let cache_write = event.cache_creation_input_tokens.unwrap_or(0);
        let output = event.output_tokens.unwrap_or(0);
        self.pending_records.lock().unwrap().push(UsageRecord {
            history_len,
            input_total_tokens: input_total,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            output_tokens: output,
        });
    }

    /// Drain accumulated records. Called by Pod after a run completes,
    /// before persisting the turn.
    pub(crate) fn drain(&self) -> Vec<UsageRecord> {
        std::mem::take(&mut *self.pending_records.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(input: u64, cache_read: u64, cache_write: u64, output: u64) -> UsageEvent {
        UsageEvent {
            input_tokens: Some(input),
            output_tokens: Some(output),
            total_tokens: Some(input + output),
            cache_read_input_tokens: Some(cache_read),
            cache_creation_input_tokens: Some(cache_write),
        }
    }

    #[test]
    fn pairs_history_len_with_usage_event() {
        let tracker = UsageTracker::new();
        tracker.note_request(5);
        tracker.record_usage(&make_event(1000, 800, 100, 42));

        let records = tracker.drain();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].history_len, 5);
        assert_eq!(records[0].input_total_tokens, 1000);
        assert_eq!(records[0].cache_read_tokens, 800);
        assert_eq!(records[0].cache_write_tokens, 100);
        assert_eq!(records[0].output_tokens, 42);
    }

    #[test]
    fn drain_clears_buffer() {
        let tracker = UsageTracker::new();
        tracker.note_request(1);
        tracker.record_usage(&make_event(10, 0, 0, 5));
        assert_eq!(tracker.drain().len(), 1);
        assert_eq!(tracker.drain().len(), 0);
    }

    #[test]
    fn usage_without_pending_history_len_is_dropped() {
        let tracker = UsageTracker::new();
        tracker.record_usage(&make_event(10, 0, 0, 5));
        assert_eq!(tracker.drain().len(), 0);
    }

    #[test]
    fn multiple_requests_in_one_run() {
        let tracker = UsageTracker::new();
        tracker.note_request(5);
        tracker.record_usage(&make_event(100, 0, 0, 20));
        tracker.note_request(10);
        tracker.record_usage(&make_event(200, 50, 0, 30));

        let records = tracker.drain();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].history_len, 5);
        assert_eq!(records[1].history_len, 10);
        assert_eq!(records[1].cache_read_tokens, 50);
    }
}
