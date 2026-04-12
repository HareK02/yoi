//! Shared state for compaction decisions.
//!
//! Holds atomic counters shared between:
//! - `on_usage` callback (writes `last_input_tokens`)
//! - `CompactInterceptor` (reads token count, checks thresholds)
//! - `Pod::run()`/`resume()` (circuit breaker, thrash detection)

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const MAX_COMPACT_FAILURES: usize = 3;

/// Shared mutable state for compaction decisions.
pub(crate) struct CompactState {
    /// Last observed input_tokens from `on_usage` callback.
    last_input_tokens: AtomicU64,
    /// Proactive threshold — checked in `pre_llm_request` (between turns).
    turn_threshold: u64,
    /// Post-run threshold — checked by Controller after run completes.
    post_run_threshold: u64,
    /// Number of recent turns to retain after compaction.
    retained_turns: usize,
    /// Consecutive compact failures. At `MAX_COMPACT_FAILURES`, compaction is disabled.
    consecutive_failures: AtomicUsize,
    /// `true` immediately after a successful compact, cleared on next normal completion.
    just_compacted: AtomicBool,
    /// `true` when circuit breaker has tripped.
    disabled: AtomicBool,
}

impl CompactState {
    /// Create a new CompactState.
    ///
    /// `turn_threshold` is the proactive (80%) threshold from the manifest.
    /// `post_run_threshold` is derived as `turn_threshold * 9 / 8` (≈90%).
    pub(crate) fn new(turn_threshold: u64, retained_turns: usize) -> Self {
        Self {
            last_input_tokens: AtomicU64::new(0),
            turn_threshold,
            post_run_threshold: turn_threshold * 9 / 8,
            retained_turns,
            consecutive_failures: AtomicUsize::new(0),
            just_compacted: AtomicBool::new(false),
            disabled: AtomicBool::new(false),
        }
    }

    /// Update the last observed input_tokens (called from `on_usage`).
    pub(crate) fn update_input_tokens(&self, tokens: u64) {
        self.last_input_tokens.store(tokens, Ordering::Relaxed);
    }

    /// Read the last observed input_tokens.
    pub(crate) fn last_input_tokens(&self) -> u64 {
        self.last_input_tokens.load(Ordering::Relaxed)
    }

    /// The between-turns threshold value.
    pub(crate) fn turn_threshold(&self) -> u64 {
        self.turn_threshold
    }

    /// Number of turns to retain after compaction.
    pub(crate) fn retained_turns(&self) -> usize {
        self.retained_turns
    }

    /// Whether compaction has been disabled by the circuit breaker.
    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Whether `last_input_tokens` exceeds the between-turns threshold.
    pub(crate) fn exceeds_turn(&self) -> bool {
        self.last_input_tokens() > self.turn_threshold
    }

    /// Whether `last_input_tokens` exceeds the post-run threshold.
    pub(crate) fn exceeds_post_run(&self) -> bool {
        self.last_input_tokens() > self.post_run_threshold
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
    fn threshold_derivation() {
        let state = CompactState::new(80_000, 2);
        assert_eq!(state.turn_threshold, 80_000);
        assert_eq!(state.post_run_threshold, 90_000);
        assert_eq!(state.retained_turns(), 2);
    }

    #[test]
    fn exceeds_checks() {
        let state = CompactState::new(80_000, 2);
        assert!(!state.exceeds_turn());
        assert!(!state.exceeds_post_run());

        state.update_input_tokens(85_000);
        assert!(state.exceeds_turn());
        assert!(!state.exceeds_post_run());

        state.update_input_tokens(95_000);
        assert!(state.exceeds_turn());
        assert!(state.exceeds_post_run());
    }

    #[test]
    fn circuit_breaker_trips_after_max_failures() {
        let state = CompactState::new(80_000, 2);
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
        let state = CompactState::new(80_000, 2);
        state.record_compact_failure();
        state.record_compact_failure();
        assert!(!state.is_disabled());

        state.record_compact_success();
        assert!(state.just_compacted());

        // After success + 2 more failures, still not disabled (count was reset).
        state.record_compact_failure();
        state.record_compact_failure();
        assert!(!state.is_disabled());
    }

    #[test]
    fn just_compacted_lifecycle() {
        let state = CompactState::new(80_000, 2);
        assert!(!state.just_compacted());

        state.record_compact_success();
        assert!(state.just_compacted());

        state.set_just_compacted(false);
        assert!(!state.just_compacted());
    }
}
