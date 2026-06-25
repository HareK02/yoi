//! Shared state for compaction decisions.
//!
//! Holds the two configured thresholds and circuit-breaker / thrash-detection
//! flags shared between:
//! - `WorkerInterceptor` (reads `request_threshold` — the *safety net* for
//!   between-requests yielding)
//! - `Worker::try_pre_run_compact` (reads `post_run_threshold` — the
//!   *proactive* check before the next turn starts)
//! - `Worker::run()` / `resume()` (circuit breaker, thrash detection)
//!
//! Current occupancy (input-token count) is **not** stored here. The single
//! source of truth is `session_store::UsageRecord` (persisted per LLM call)
//! projected through `Worker::total_tokens()`. Callers pass the current
//! occupancy to `exceeds_*` at check time.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const MAX_COMPACT_FAILURES: usize = 3;

/// Shared mutable state for compaction decisions.
pub(crate) struct CompactState {
    /// Between-turns threshold (proactive). Checked before the next turn
    /// starts. `None` disables the pre-run check.
    post_run_threshold: Option<u64>,
    /// Between-requests threshold (safety net). Checked inside a turn
    /// before each LLM request. `None` disables the request check.
    request_threshold: Option<u64>,
    /// Token budget retained verbatim at the tail after compaction.
    retained_tokens: u64,
    /// Consecutive compact failures. At `MAX_COMPACT_FAILURES`, compaction is disabled.
    consecutive_failures: AtomicUsize,
    /// `true` immediately after a successful compact, cleared on next normal completion.
    just_compacted: AtomicBool,
    /// `true` when circuit breaker has tripped.
    disabled: AtomicBool,
}

impl CompactState {
    pub(crate) fn new(
        post_run_threshold: Option<u64>,
        request_threshold: Option<u64>,
        retained_tokens: u64,
    ) -> Self {
        Self {
            post_run_threshold,
            request_threshold,
            retained_tokens,
            consecutive_failures: AtomicUsize::new(0),
            just_compacted: AtomicBool::new(false),
            disabled: AtomicBool::new(false),
        }
    }

    /// Configured between-requests threshold (if any).
    pub(crate) fn request_threshold(&self) -> Option<u64> {
        self.request_threshold
    }

    /// Token budget retained verbatim at the tail after compaction.
    pub(crate) fn retained_tokens(&self) -> u64 {
        self.retained_tokens
    }

    /// Whether compaction has been disabled by the circuit breaker.
    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Whether `current_tokens` exceeds the between-requests threshold.
    /// Returns `false` when `request_threshold` is unset.
    pub(crate) fn exceeds_request(&self, current_tokens: u64) -> bool {
        self.request_threshold
            .map(|t| current_tokens > t)
            .unwrap_or(false)
    }

    /// Whether `current_tokens` exceeds the post-run threshold.
    /// Returns `false` when `post_run_threshold` is unset.
    pub(crate) fn exceeds_post_run(&self, current_tokens: u64) -> bool {
        self.post_run_threshold
            .map(|t| current_tokens > t)
            .unwrap_or(false)
    }

    /// Whether a compact just completed (for thrash detection).
    pub(crate) fn just_compacted(&self) -> bool {
        self.just_compacted.load(Ordering::Relaxed)
    }

    /// Set or clear the just_compacted flag.
    pub(crate) fn set_just_compacted(&self, val: bool) {
        self.just_compacted.store(val, Ordering::Relaxed);
    }

    /// Record a successful compaction: reset failure counter, set just_compacted.
    pub(crate) fn record_compact_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.just_compacted.store(true, Ordering::Relaxed);
    }

    /// Record a compaction failure. Disables compaction after MAX_COMPACT_FAILURES.
    pub(crate) fn record_compact_failure(&self) {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if prev + 1 >= MAX_COMPACT_FAILURES {
            self.disabled.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_thresholds_configured() {
        let state = CompactState::new(Some(80_000), Some(90_000), 8_000);
        assert_eq!(state.request_threshold(), Some(90_000));
        assert_eq!(state.retained_tokens(), 8_000);

        assert!(!state.exceeds_request(70_000));
        assert!(!state.exceeds_post_run(70_000));

        assert!(!state.exceeds_request(85_000));
        assert!(state.exceeds_post_run(85_000));

        assert!(state.exceeds_request(95_000));
        assert!(state.exceeds_post_run(95_000));
    }

    #[test]
    fn post_run_only() {
        let state = CompactState::new(Some(80_000), None, 8_000);
        // request check always false when threshold is None.
        assert!(!state.exceeds_request(1_000_000));
        assert!(state.exceeds_post_run(85_000));
    }

    #[test]
    fn request_only() {
        let state = CompactState::new(None, Some(90_000), 8_000);
        assert!(!state.exceeds_post_run(1_000_000));
        assert!(state.exceeds_request(95_000));
    }

    #[test]
    fn both_none_disables_all_checks() {
        let state = CompactState::new(None, None, 8_000);
        assert!(!state.exceeds_request(1_000_000));
        assert!(!state.exceeds_post_run(1_000_000));
    }

    #[test]
    fn circuit_breaker_trips_after_max_failures() {
        let state = CompactState::new(Some(80_000), Some(90_000), 8_000);
        assert!(!state.is_disabled());

        state.record_compact_failure();
        assert!(!state.is_disabled());
        state.record_compact_failure();
        assert!(!state.is_disabled());
        state.record_compact_failure();
        assert!(state.is_disabled());
    }

    #[test]
    fn success_resets_failure_count() {
        let state = CompactState::new(Some(80_000), Some(90_000), 8_000);
        state.record_compact_failure();
        state.record_compact_failure();
        assert!(!state.is_disabled());

        state.record_compact_success();
        assert!(state.just_compacted());

        state.record_compact_failure();
        state.record_compact_failure();
        assert!(!state.is_disabled());
    }

    #[test]
    fn just_compacted_lifecycle() {
        let state = CompactState::new(Some(80_000), Some(90_000), 8_000);
        assert!(!state.just_compacted());

        state.record_compact_success();
        assert!(state.just_compacted());

        state.set_just_compacted(false);
        assert!(!state.just_compacted());
    }
}
