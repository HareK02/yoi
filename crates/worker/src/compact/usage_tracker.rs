//! Tracks per-LLM-request Usage measurements within a Worker run.
//!
//! Bridge between two sync touchpoints in the Engine lifecycle:
//!
//! - **`pre_llm_request` hook** (async, but synchronously accessed via the
//!   tracker): captures `history.len()` at the moment a request goes out.
//! - **`on_usage` callback** (sync closure): receives the aggregated final
//!   `UsageEvent` for that request after the stream completes.
//!
//! Pairing the two yields one `UsageRecord` per LLM call. Worker commits
//! successful requests from the assistant-turn boundary and uses terminal
//! `persist_turn` only for records that have not already crossed that boundary.
//!
//! Multiple LLM calls per Worker run (tool loop) are supported: each call
//! produces its own `(history_len, UsageEvent)` pair, and the records are
//! buffered in chronological order.

use std::collections::VecDeque;
use std::sync::Mutex;

use agen::UsageRecord;
use agen::timeline::event::UsageEvent;
use session_metrics::Metric;
use session_store::{LogEntry, StoreError, segment_log};

use crate::compact::state::CompactState;
use crate::compact::telemetry::correlated_post_request_metric;

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

/// One measured request and its causal metric links.
#[derive(Debug, Clone)]
pub(crate) struct RecordedUsage {
    pub(crate) record: UsageRecord,
    pub(crate) post_requests: Vec<PostRequestLink>,
    /// True only when the provider stream completed normally and reported an
    /// input occupancy. Partial/error/cancelled streams remain billable usage
    /// records, but cannot satisfy a post-compaction rearm.
    pub(crate) completed_with_occupancy: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UsageSnapshot {
    pub(crate) input_total_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) output_tokens: u64,
}

#[derive(Debug)]
struct PendingRequest {
    id: u64,
    history_len: usize,
    usage_recorded: bool,
}

#[derive(Debug)]
struct PendingRecordedUsage {
    request_id: u64,
    has_input_occupancy: bool,
    usage: RecordedUsage,
}

#[derive(Debug, Default)]
struct UsageTrackerState {
    next_request_id: u64,
    pending_request: Option<PendingRequest>,
    pending_correlations: Vec<PostRequestLink>,
    pending_records: VecDeque<PendingRecordedUsage>,
}

/// Shared between the pre-request hook, the `on_usage` callback, and Worker.
pub(crate) struct UsageTracker {
    state: Mutex<UsageTrackerState>,
}

impl UsageTracker {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(UsageTrackerState::default()),
        }
    }

    /// Called from a `pre_llm_request` hook with the current history length.
    /// Replacing an older pending request classifies that request as incomplete;
    /// any partial usage already recorded for it remains available for billing.
    pub(crate) fn note_request(&self, history_len: usize) {
        let mut state = self.state.lock().unwrap();
        let id = state.next_request_id;
        state.next_request_id = state.next_request_id.wrapping_add(1);
        state.pending_request = Some(PendingRequest {
            id,
            history_len,
            usage_recorded: false,
        });
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
        let mut state = self.state.lock().unwrap();
        state
            .pending_correlations
            .retain(|link| link.metric != metric);
        state.pending_correlations.push(PostRequestLink {
            correlation_id: id,
            metric,
        });
    }

    /// Called from the `on_usage` callback with the aggregated final
    /// UsageEvent. The record remains associated with the current request until
    /// [`Self::request_completed`] proves that the provider stream completed
    /// normally. Error/cancel paths still retain the measurement for billing.
    pub(crate) fn record_usage(&self, event: &UsageEvent) {
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending_request.as_mut() else {
            return;
        };
        if pending.usage_recorded {
            return;
        }
        pending.usage_recorded = true;
        let request_id = pending.id;
        let history_len = pending.history_len;
        // UsageEvent.input_tokens は scheme 層で「占有量（プロンプト全長）」に
        // 正規化済みである前提（Anthropic は cache_read + cache_creation を
        // 加算して emit する）。
        let input_total = event.input_tokens.unwrap_or(0);
        let cache_read = event.cache_read_input_tokens.unwrap_or(0);
        let cache_write = event.cache_creation_input_tokens.unwrap_or(0);
        let output = event.output_tokens.unwrap_or(0);
        state.pending_records.push_back(PendingRecordedUsage {
            request_id,
            has_input_occupancy: event.input_tokens.is_some(),
            usage: RecordedUsage {
                record: UsageRecord {
                    history_len,
                    input_total_tokens: input_total,
                    cache_read_tokens: cache_read,
                    cache_write_tokens: cache_write,
                    output_tokens: output,
                },
                post_requests: Vec::new(),
                completed_with_occupancy: false,
            },
        });
    }

    /// Mark the current request as normally completed. A post-request causal
    /// link is consumed only when that exact request also reported a valid
    /// input occupancy; callbacks from partial/error/cancelled streams cannot
    /// rearm the compaction guard.
    pub(crate) fn request_completed(&self) {
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending_request.take() else {
            return;
        };
        let Some(recorded_index) = state
            .pending_records
            .iter()
            .rposition(|recorded| recorded.request_id == pending.id)
        else {
            return;
        };
        let has_occupancy = state.pending_records[recorded_index].has_input_occupancy;
        if !has_occupancy {
            return;
        }
        let post_requests = std::mem::take(&mut state.pending_correlations);
        let recorded = &mut state.pending_records[recorded_index].usage;
        recorded.completed_with_occupancy = true;
        recorded.post_requests = post_requests;
    }

    /// Return a clone of accumulated measurements without clearing them.
    /// Used by request-time circuit breakers that need the same occupancy
    /// projection as Worker persistence while the run is still active.
    pub(crate) fn records(&self) -> Vec<UsageRecord> {
        self.state
            .lock()
            .unwrap()
            .pending_records
            .iter()
            .map(|r| r.usage.record.clone())
            .collect()
    }

    /// Number of measurements that have not crossed the durable commit
    /// boundary. Session rewrites must not activate a replacement Segment while
    /// this is non-zero, because their history coordinates belong to the current
    /// Segment.
    pub(crate) fn pending_record_count(&self) -> usize {
        self.state.lock().unwrap().pending_records.len()
    }

    /// Remove the oldest measurement for one durable commit attempt.
    pub(crate) fn take_next(&self) -> Option<RecordedUsage> {
        self.state
            .lock()
            .unwrap()
            .pending_records
            .pop_front()
            .map(|recorded| recorded.usage)
    }

    /// Restore a measurement after its durable commit failed.
    pub(crate) fn restore_front(&self, usage: RecordedUsage) {
        self.state
            .lock()
            .unwrap()
            .pending_records
            .push_front(PendingRecordedUsage {
                // Persist ordering is the only remaining identity requirement.
                request_id: u64::MAX,
                has_input_occupancy: usage.completed_with_occupancy,
                usage,
            });
    }

    /// Drain accumulated records at a terminal Worker boundary.
    pub(crate) fn drain(&self) -> Vec<RecordedUsage> {
        let mut state = self.state.lock().unwrap();
        state.pending_request = None;
        state
            .pending_records
            .drain(..)
            .map(|recorded| recorded.usage)
            .collect()
    }
}

/// Commit all currently measured usage records in request order.
///
/// A record is removed from the tracker only after its `LlmUsage` append
/// succeeds. This is the single accounting boundary shared by the in-run
/// interceptor and terminal `persist_turn`, preventing both loss on append
/// failure and duplicate terminal writes. Correlated metrics are handed to the
/// same commit boundary so they stay on the usage record's Segment; the return
/// value is only useful as test/diagnostic evidence and must not be re-emitted.
pub(crate) fn persist_pending_usage(
    tracker: &UsageTracker,
    usage_history: &Mutex<Vec<UsageRecord>>,
    compact_state: Option<&CompactState>,
    mut commit: impl FnMut(LogEntry, &mut dyn FnMut(), &[Metric]) -> Result<(), StoreError>,
) -> Result<Vec<Metric>, StoreError> {
    let mut metrics = Vec::new();
    while let Some(recorded) = tracker.take_next() {
        let RecordedUsage {
            record,
            post_requests,
            completed_with_occupancy,
        } = recorded;
        let post_metrics = post_requests
            .iter()
            .map(|link| correlated_post_request_metric(link.metric, &link.correlation_id, &record))
            .collect::<Vec<_>>();
        let usage_entry = LogEntry::LlmUsage {
            ts: segment_log::now_millis(),
            history_len: record.history_len,
            input_total_tokens: record.input_total_tokens,
            cache_read_tokens: record.cache_read_tokens,
            cache_write_tokens: record.cache_write_tokens,
            output_tokens: record.output_tokens,
        };
        let mut publish_committed_usage = || {
            usage_history
                .lock()
                .expect("usage_history poisoned")
                .push(record.clone());

            if completed_with_occupancy
                && post_requests
                    .iter()
                    .any(|link| link.metric == PostRequestMetric::Compaction)
            {
                if let Some(state) = compact_state {
                    state.post_compact_request_committed();
                }
            }
        };
        if let Err(error) = commit(usage_entry, &mut publish_committed_usage, &post_metrics) {
            tracker.restore_front(RecordedUsage {
                record,
                post_requests,
                completed_with_occupancy,
            });
            return Err(error);
        }
        metrics.extend(post_metrics);
    }
    Ok(metrics)
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
        tracker.request_completed();

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
        tracker.request_completed();
        // Next request without a fresh stash → no correlation_id.
        tracker.note_request(10);
        tracker.record_usage(&make_event(200, 50, 0, 30));
        tracker.request_completed();

        let records = tracker.drain();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].post_requests.len(), 1);
        assert_eq!(records[0].post_requests[0].correlation_id, "abc");
        assert_eq!(records[0].post_requests[0].metric, PostRequestMetric::Prune);
        assert!(records[1].post_requests.is_empty());
    }

    #[test]
    fn partial_usage_does_not_consume_post_compact_link() {
        let tracker = UsageTracker::new();
        tracker.note_compaction_correlation_id("compact-id".into());
        tracker.note_request(5);
        tracker.record_usage(&make_event(90, 0, 0, 2));

        // Starting another request proves the prior stream did not complete.
        tracker.note_request(6);
        tracker.record_usage(&make_event(100, 0, 0, 3));
        tracker.request_completed();

        let records = tracker.drain();
        assert_eq!(records.len(), 2);
        assert!(!records[0].completed_with_occupancy);
        assert!(records[0].post_requests.is_empty());
        assert!(records[1].completed_with_occupancy);
        assert_eq!(records[1].post_requests.len(), 1);
        assert_eq!(records[1].post_requests[0].correlation_id, "compact-id");
    }

    #[test]
    fn completed_request_without_input_occupancy_does_not_consume_link() {
        let tracker = UsageTracker::new();
        tracker.note_compaction_correlation_id("compact-id".into());
        tracker.note_request(5);
        let mut missing_input = make_event(0, 0, 0, 2);
        missing_input.input_tokens = None;
        tracker.record_usage(&missing_input);
        tracker.request_completed();

        tracker.note_request(6);
        tracker.record_usage(&make_event(100, 0, 0, 3));
        tracker.request_completed();

        let records = tracker.drain();
        assert!(!records[0].completed_with_occupancy);
        assert!(records[0].post_requests.is_empty());
        assert!(records[1].completed_with_occupancy);
        assert_eq!(records[1].post_requests[0].correlation_id, "compact-id");
    }

    #[test]
    fn completed_request_without_usage_does_not_complete_previous_partial_record() {
        let tracker = UsageTracker::new();
        tracker.note_compaction_correlation_id("compact-id".into());
        tracker.note_request(5);
        tracker.record_usage(&make_event(90, 0, 0, 2));

        tracker.note_request(6);
        tracker.request_completed();

        let records = tracker.drain();
        assert_eq!(records.len(), 1);
        assert!(!records[0].completed_with_occupancy);
        assert!(records[0].post_requests.is_empty());
    }

    #[test]
    fn failed_commit_can_restore_record_without_reordering() {
        let tracker = UsageTracker::new();
        tracker.note_request(1);
        tracker.record_usage(&make_event(10, 0, 0, 1));
        tracker.request_completed();
        tracker.note_request(2);
        tracker.record_usage(&make_event(20, 0, 0, 2));
        tracker.request_completed();

        let first = tracker.take_next().expect("first record");
        tracker.restore_front(first);
        let records = tracker.drain();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record.input_total_tokens, 10);
        assert_eq!(records[1].record.input_total_tokens, 20);
    }

    #[test]
    fn compaction_rearms_only_after_usage_append_succeeds() {
        use crate::compact::state::{AutomaticCompactDecision, CompactionOutcome};

        let tracker = UsageTracker::new();
        let state = CompactState::new(None, Some(10), 0);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Succeeded));

        tracker.note_compaction_correlation_id("compact-id".into());
        tracker.note_request(1);
        tracker.record_usage(&make_event(5, 0, 0, 1));
        tracker.request_completed();
        let usage_history = Mutex::new(Vec::new());

        let error = persist_pending_usage(&tracker, &usage_history, Some(&state), |_, _, _| {
            Err(StoreError::Io(std::io::Error::other("synthetic failure")))
        })
        .expect_err("usage append must fail");
        assert!(error.to_string().contains("synthetic failure"));
        assert!(usage_history.lock().unwrap().is_empty());
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(_)
        ));

        let mut entries = Vec::new();
        let metrics = persist_pending_usage(
            &tracker,
            &usage_history,
            Some(&state),
            |entry, after_commit, _post_metrics| {
                entries.push(entry);
                after_commit();
                Ok(())
            },
        )
        .expect("retry should commit");
        assert_eq!(entries.len(), 1);
        assert_eq!(usage_history.lock().unwrap().len(), 1);
        assert_eq!(metrics.len(), 1);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
    }
}
