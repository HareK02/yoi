use std::sync::Mutex;

use super::telemetry::CompactFailureCategory;

/// Process-local automatic compaction guard for the current logical run.
///
/// This guard is deliberately not persisted or reconstructed from session
/// history, compaction metrics, or replacement-segment state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticCompactGuard {
    Ready,
    SuppressedForCurrentRun {
        failure_category: CompactFailureCategory,
    },
    AwaitingPostCompactRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionOutcome {
    Succeeded,
    Failed(CompactFailureCategory),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticCompactTrigger {
    PreRun,
    RequestThreshold,
}

/// Decision returned by an atomic threshold/attempt-state evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticCompactDecision {
    Continue,
    Start(AutomaticCompactTrigger),
    Block(AutomaticCompactBlock),
}

/// Typed reason why a provider request may not proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticCompactBlock {
    /// An automatic attempt was already claimed for this logical run and has
    /// not yet produced an outcome.
    Attempted,
    /// Compaction succeeded, but no post-compaction provider request has yet
    /// committed a new occupancy UsageRecord.
    Thrash,
    /// This logical run already used its automatic attempt and it failed.
    Failed(CompactFailureCategory),
    /// This logical run's automatic attempt was cancelled. Cancellation is not
    /// classified or counted as a compaction failure.
    Cancelled,
}

#[derive(Debug)]
struct AutomaticCompactRuntimeState {
    guard: AutomaticCompactGuard,
    attempt_claimed: bool,
    cancelled_attempt: bool,
    pending_request_block: Option<AutomaticCompactBlock>,
}

/// Tracks automatic compaction thresholds and the current logical-run guard.
#[derive(Debug)]
pub(crate) struct CompactState {
    /// Proactive threshold checked before a fresh user run starts.
    compact_threshold: Option<u64>,
    /// Safety threshold checked immediately before every provider request.
    request_threshold: Option<u64>,
    retained_tokens: u64,
    runtime: Mutex<AutomaticCompactRuntimeState>,
}

impl CompactState {
    pub(crate) fn new(
        compact_threshold: Option<u64>,
        request_threshold: Option<u64>,
        retained_tokens: u64,
    ) -> Self {
        Self {
            compact_threshold,
            request_threshold,
            retained_tokens,
            runtime: Mutex::new(AutomaticCompactRuntimeState {
                guard: AutomaticCompactGuard::Ready,
                attempt_claimed: false,
                cancelled_attempt: false,
                pending_request_block: None,
            }),
        }
    }

    pub(crate) fn retained_tokens(&self) -> u64 {
        self.retained_tokens
    }

    pub(crate) fn pre_run_eligible(&self, total_tokens: u64) -> bool {
        if !self
            .compact_threshold
            .is_some_and(|threshold| total_tokens > threshold)
        {
            return false;
        }
        let runtime = self.lock_runtime();
        runtime.guard == AutomaticCompactGuard::Ready && !runtime.attempt_claimed
    }

    /// Starts a fresh logical run. Pause/resume paths must not call this.
    pub(crate) fn begin_logical_run(&self) {
        self.clear_logical_run();
    }

    /// Clears per-run state after a terminal run outcome.
    pub(crate) fn finish_logical_run(&self) {
        self.clear_logical_run();
    }

    /// Atomically evaluates the proactive threshold and claims this logical
    /// run's automatic attempt when eligible.
    pub(crate) fn evaluate_pre_run(&self, total_tokens: u64) -> AutomaticCompactDecision {
        if !self
            .compact_threshold
            .is_some_and(|threshold| total_tokens > threshold)
        {
            return AutomaticCompactDecision::Continue;
        }
        self.claim_attempt(AutomaticCompactTrigger::PreRun)
    }

    /// Atomically evaluates the request safety threshold and either claims an
    /// automatic attempt or returns the typed reason the request must stop.
    pub(crate) fn evaluate_request(&self, total_tokens: u64) -> AutomaticCompactDecision {
        if !self
            .request_threshold
            .is_some_and(|threshold| total_tokens > threshold)
        {
            return AutomaticCompactDecision::Continue;
        }
        self.claim_attempt(AutomaticCompactTrigger::RequestThreshold)
    }

    /// Claims a hook-originated compaction yield under the same guard used by
    /// threshold evaluation. This exists even in manual-only configurations.
    pub(crate) fn claim_hook_yield(&self) -> AutomaticCompactDecision {
        self.claim_attempt(AutomaticCompactTrigger::RequestThreshold)
    }

    pub(crate) fn has_claimed_attempt(&self) -> bool {
        self.lock_runtime().attempt_claimed
    }

    pub(crate) fn record_request_block(&self, block: AutomaticCompactBlock) {
        self.lock_runtime().pending_request_block = Some(block);
    }

    /// Completes the currently claimed automatic attempt exactly once.
    pub(crate) fn complete_automatic(&self, outcome: CompactionOutcome) -> bool {
        let mut runtime = self.lock_runtime();
        if !runtime.attempt_claimed
            || runtime.guard != AutomaticCompactGuard::Ready
            || runtime.cancelled_attempt
        {
            return false;
        }

        match outcome {
            CompactionOutcome::Succeeded => {
                runtime.guard = AutomaticCompactGuard::AwaitingPostCompactRequest;
            }
            CompactionOutcome::Failed(failure_category) => {
                runtime.guard = AutomaticCompactGuard::SuppressedForCurrentRun { failure_category };
            }
            CompactionOutcome::Cancelled => {
                runtime.cancelled_attempt = true;
            }
        }
        true
    }

    /// Re-arms automatic compaction only after the first real provider request
    /// following successful compaction has a durably committed UsageRecord.
    pub(crate) fn post_compact_request_committed(&self) {
        let mut runtime = self.lock_runtime();
        if runtime.guard == AutomaticCompactGuard::AwaitingPostCompactRequest {
            runtime.guard = AutomaticCompactGuard::Ready;
            runtime.attempt_claimed = false;
            runtime.cancelled_attempt = false;
        }
    }

    pub(crate) fn take_pending_request_block(&self) -> Option<AutomaticCompactBlock> {
        self.lock_runtime().pending_request_block.take()
    }

    fn claim_attempt(&self, trigger: AutomaticCompactTrigger) -> AutomaticCompactDecision {
        let mut runtime = self.lock_runtime();
        match runtime.guard {
            AutomaticCompactGuard::Ready if !runtime.attempt_claimed => {
                runtime.attempt_claimed = true;
                AutomaticCompactDecision::Start(trigger)
            }
            AutomaticCompactGuard::Ready if runtime.cancelled_attempt => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Cancelled)
            }
            AutomaticCompactGuard::Ready => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Attempted)
            }
            AutomaticCompactGuard::SuppressedForCurrentRun { failure_category } => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Failed(failure_category))
            }
            AutomaticCompactGuard::AwaitingPostCompactRequest => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Thrash)
            }
        }
    }

    fn clear_logical_run(&self) {
        let mut runtime = self.lock_runtime();
        runtime.guard = AutomaticCompactGuard::Ready;
        runtime.attempt_claimed = false;
        runtime.cancelled_attempt = false;
        runtime.pending_request_block = None;
    }

    fn lock_runtime(&self) -> std::sync::MutexGuard<'_, AutomaticCompactRuntimeState> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn guard(&self) -> AutomaticCompactGuard {
        self.lock_runtime().guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILURE: CompactFailureCategory = CompactFailureCategory::Storage;

    #[test]
    fn automatic_failure_suppresses_only_current_logical_run() {
        let state = CompactState::new(Some(10), Some(10), 2);
        assert_eq!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Start(AutomaticCompactTrigger::PreRun)
        );
        assert!(state.complete_automatic(CompactionOutcome::Failed(FAILURE)));
        assert_eq!(
            state.guard(),
            AutomaticCompactGuard::SuppressedForCurrentRun {
                failure_category: FAILURE
            }
        );
        assert_eq!(
            state.evaluate_request(10),
            AutomaticCompactDecision::Continue,
            "a failed proactive compact still permits a request below the safety threshold"
        );
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Failed(FAILURE))
        );

        state.begin_logical_run();
        assert_eq!(state.guard(), AutomaticCompactGuard::Ready);
        assert_eq!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Start(AutomaticCompactTrigger::PreRun)
        );
    }

    #[test]
    fn claimed_attempt_cannot_be_started_twice() {
        let state = CompactState::new(Some(10), Some(10), 2);
        assert!(matches!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Attempted)
        );
    }

    #[test]
    fn cancellation_consumes_run_attempt_without_becoming_failure() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Cancelled));

        assert_eq!(state.guard(), AutomaticCompactGuard::Ready);
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Cancelled)
        );
        state.begin_logical_run();
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
    }

    #[test]
    fn successful_compaction_requires_committed_request_before_rearming() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Succeeded));

        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Thrash)
        );
        state.post_compact_request_committed();
        assert_eq!(state.guard(), AutomaticCompactGuard::Ready);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
    }

    #[test]
    fn pause_resume_preserves_guard_while_terminal_finish_clears_it() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Failed(FAILURE)));
        // Pause/resume deliberately performs no state transition.
        assert!(matches!(
            state.guard(),
            AutomaticCompactGuard::SuppressedForCurrentRun { .. }
        ));

        state.finish_logical_run();
        assert_eq!(state.guard(), AutomaticCompactGuard::Ready);
    }

    #[test]
    fn hook_yield_is_guarded_without_threshold_configuration() {
        let state = CompactState::new(None, None, 2);
        assert!(matches!(
            state.claim_hook_yield(),
            AutomaticCompactDecision::Start(_)
        ));
        assert_eq!(
            state.claim_hook_yield(),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Attempted)
        );
    }
}
