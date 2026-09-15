//! Tracks per-LLM-request Usage measurements within a Worker run.
//!
//! Bridge between two sync touchpoints in the Engine lifecycle:
//!
//! - **`pre_llm_request` hook** (async, but synchronously accessed via the
//!   tracker): captures `history.len()` at the moment a request goes out.
//! - **`on_usage` callback** (sync closure): receives the aggregated final
//!   `UsageEvent` for that request after the stream completes.
//!
//! Pairing the two yields one `UsageRecord` per LLM call. Worker drains them
//! in `persist_turn` and writes them as `LogEntry::LlmUsage` entries.
//!
//! Multiple LLM calls per Worker run (tool loop) are supported: each call
//! produces its own `(history_len, UsageEvent)` pair, and the records are
//! buffered in chronological order.

use std::sync::Mutex;

use agen::UsageRecord;
use agen::timeline::event::UsageEvent;

/// The metric emitted after the next measured provider request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostRequestMetric {
    Prune,
    Compaction,
}

impl PostRequestMetric {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Prune => "prune.post_request",
            Self::Compaction => "compact.post_request",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PostRequestLink {
    pub(crate) correlation_id: String,
    pub(crate) metric: PostRequestMetric,
}

/// One drained measurement and its causal metric links.
#[derive(Debug, Clone)]
pub(crate) struct RecordedUsage {
    pub(crate) record: UsageRecord,
    pub(crate) post_requests: Vec<PostRequestLink>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageSnapshot {
    pub(crate) input_total_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) output_tokens: u64,
}

/// Shared between the pre-request hook, the `on_usage` callback, and Worker.
pub(crate) struct UsageTracker {
    /// `history.len()` captured at the most recent `pre_llm_request`.
    /// Cleared when paired with an incoming `on_usage` event.
    pending_history_len: Mutex<Option<usize>>,
    /// Optional causal link consumed by the next measured request.
    pending_correlations: Mutex<Vec<PostRequestLink>>,
    /// Records accumulated during the current run; drained by Worker.
    pending_records: Mutex<Vec<RecordedUsage>>,
}

impl UsageTracker {
    pub(crate) fn new() -> Self {
        Self {
            pending_history_len: Mutex::new(None),
            pending_correlations: Mutex::new(Vec::new()),
            pending_records: Mutex::new(Vec::new()),
        }
    }

    /// Called from a `pre_llm_request` hook with the current history length.
    pub(crate) fn note_request(&self, history_len: usize) {
        *self.pending_history_len.lock().unwrap() = Some(history_len);
    }

    /// Pair a prune event with the next provider request.
    pub(crate) fn note_correlation_id(&self, id: String) {
        self.note_post_request(id, PostRequestMetric::Prune);
    }

    /// Pair a completed compaction with the next normal provider request.
    pub(crate) fn note_compaction_correlation_id(&self, id: String) {
        self.note_post_request(id, PostRequestMetric::Compaction);
    }

    fn note_post_request(&self, id: String, metric: PostRequestMetric) {
        let mut pending = self.pending_correlations.lock().unwrap();
        pending.retain(|link| link.metric != metric);
        pending.push(PostRequestLink {
            correlation_id: id,
            metric,
        });
    }

    /// Called from the `on_usage` callback with the aggregated final
    /// UsageEvent. If a `history_len` was previously stashed via
    /// `note_request`, builds a `RecordedUsage` and pushes it onto the
    /// buffer. If not (e.g. test code that fires Usage outside a request),
    /// drops the event.
    pub(crate) fn record_usage(&self, event: &UsageEvent) {
        let history_len = match self.pending_history_len.lock().unwrap().take() {
            Some(n) => n,
            None => return,
        };
        let post_requests = std::mem::take(&mut *self.pending_correlations.lock().unwrap());
        // UsageEvent.input_tokens は scheme 層で「占有量（プロンプト全長）」に
        // 正規化済みである前提（Anthropic は cache_read + cache_creation を
        // 加算して emit する）。
        let input_total = event.input_tokens.unwrap_or(0);
        let cache_read = event.cache_read_input_tokens.unwrap_or(0);
        let cache_write = event.cache_creation_input_tokens.unwrap_or(0);
        let output = event.output_tokens.unwrap_or(0);
        self.pending_records.lock().unwrap().push(RecordedUsage {
            record: UsageRecord {
                history_len,
                input_total_tokens: input_total,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                output_tokens: output,
            },
            post_requests,
        });
    }

    /// Return a clone of the accumulated `UsageRecord`s without clearing them.
    /// Used by request-time circuit breakers that need the same occupancy
    /// projection as Worker persistence while the run is still active.
    pub(crate) fn records(&self) -> Vec<UsageRecord> {
        self.pending_records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.record.clone())
            .collect()
    }

    /// Drain accumulated records. Called by Worker after a run completes,
    /// before persisting the turn.
    pub(crate) fn drain(&self) -> Vec<RecordedUsage> {
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
        assert_eq!(records[0].record.history_len, 5);
        assert_eq!(records[0].record.input_total_tokens, 1000);
        assert_eq!(records[0].record.cache_read_tokens, 800);
        assert_eq!(records[0].record.cache_write_tokens, 100);
        assert_eq!(records[0].record.output_tokens, 42);
        assert!(records[0].post_requests.is_empty());
    }

    #[test]
    fn records_clones_without_clearing() {
        let tracker = UsageTracker::new();
        tracker.note_request(1);
        tracker.record_usage(&make_event(10, 0, 0, 5));

        let records = tracker.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].history_len, 1);
        assert_eq!(records[0].input_total_tokens, 10);
        assert_eq!(tracker.records().len(), 1);
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
        assert_eq!(records[0].record.history_len, 5);
        assert_eq!(records[1].record.history_len, 10);
        assert_eq!(records[1].record.cache_read_tokens, 50);
    }

    #[test]
    fn prune_and_compaction_links_share_the_next_request() {
        let tracker = UsageTracker::new();
        tracker.note_compaction_correlation_id("compact-id".into());
        tracker.note_correlation_id("prune-id".into());
        tracker.note_request(5);
        tracker.record_usage(&make_event(100, 10, 2, 20));

        let records = tracker.drain();
        assert_eq!(records[0].post_requests.len(), 2);
        assert!(records[0].post_requests.iter().any(|link| {
            link.correlation_id == "compact-id" && link.metric == PostRequestMetric::Compaction
        }));
        assert!(records[0].post_requests.iter().any(|link| {
            link.correlation_id == "prune-id" && link.metric == PostRequestMetric::Prune
        }));
    }

    #[test]
    fn correlation_id_pairs_with_next_record_only() {
        let tracker = UsageTracker::new();
        // Stash an ID, then run a request → the ID should land on this record.
        tracker.note_correlation_id("abc".into());
        tracker.note_request(5);
        tracker.record_usage(&make_event(100, 0, 0, 20));
        // Next request without a fresh stash → no correlation_id.
        tracker.note_request(10);
        tracker.record_usage(&make_event(200, 50, 0, 30));

        let records = tracker.drain();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].post_requests.len(), 1);
        assert_eq!(records[0].post_requests[0].correlation_id, "abc");
        assert_eq!(records[0].post_requests[0].metric, PostRequestMetric::Prune);
        assert!(records[1].post_requests.is_empty());
    }
}
