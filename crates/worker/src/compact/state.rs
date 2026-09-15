use std::sync::Mutex;

use super::telemetry::CompactFailureCategory;

/// The process-local state of automatic compaction for one logical run.
///
/// This is deliberately not persisted: restoring a worker starts from
/// [`AutomaticCompactState::NotAttempted`]. The failure counter is likewise a
/// guard for one live worker process, not session authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticCompactState {
    NotAttempted,
    Attempted,
    Completed(CompactionOutcome),
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
    /// Compaction succeeded but the replacement still exceeds the request
    /// safety threshold before a post-compaction request was durably recorded.
    Thrash,
    /// This logical run already used its automatic attempt and it failed.
    Failed(CompactFailureCategory),
    /// This logical run already used its automatic attempt and it was
    /// cancelled. Cancellation is not counted as a compaction failure.
    Cancelled,
    /// Consecutive automatic failures disabled further attempts until a
    /// successful manual compaction or explicit re-enable.
    Disabled(CompactFailureCategory),
}

#[derive(Debug)]
struct AutomaticCompactRuntimeState {
    attempt: AutomaticCompactState,
    consecutive_failures: u32,
    automatic_disabled: bool,
    last_failure: Option<CompactFailureCategory>,
    pending_request_block: Option<AutomaticCompactBlock>,
}

const MAX_CONSECUTIVE_AUTOMATIC_FAILURES: u32 = 2;

/// Tracks automatic compaction thresholds and process-local loop guards.
#[derive(Debug)]
pub(crate) struct CompactState {
    /// Post-run threshold. Checked before a new user run starts.
    compact_threshold: Option<u64>,
    /// In-request safety threshold. Checked immediately before each provider
    /// request, both before and after pre-request hooks.
    request_threshold: Option<u64>,
    retained_tokens: u64,
    max_consecutive_failures: u32,
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
            max_consecutive_failures: MAX_CONSECUTIVE_AUTOMATIC_FAILURES,
            runtime: Mutex::new(AutomaticCompactRuntimeState {
                attempt: AutomaticCompactState::NotAttempted,
                consecutive_failures: 0,
                automatic_disabled: false,
                last_failure: None,
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
        !runtime.automatic_disabled && runtime.attempt == AutomaticCompactState::NotAttempted
    }

    /// Starts a fresh logical run. Pause/resume paths must not call this.
    pub(crate) fn begin_logical_run(&self) {
        let mut runtime = self.lock_runtime();
        runtime.attempt = AutomaticCompactState::NotAttempted;
        runtime.pending_request_block = None;
    }

    /// Clears per-run state after a terminal run outcome. The consecutive
    /// failure circuit breaker intentionally survives logical-run boundaries.
    pub(crate) fn finish_logical_run(&self) {
        let mut runtime = self.lock_runtime();
        runtime.attempt = AutomaticCompactState::NotAttempted;
        runtime.pending_request_block = None;
    }

    /// Atomically evaluates the pre-run threshold and claims this logical
    /// run's one automatic compaction attempt when eligible.
    pub(crate) fn evaluate_pre_run(&self, total_tokens: u64) -> AutomaticCompactDecision {
        if !self
            .compact_threshold
            .is_some_and(|threshold| total_tokens > threshold)
        {
            return AutomaticCompactDecision::Continue;
        }
        self.claim_attempt(AutomaticCompactTrigger::PreRun)
    }

    /// Atomically evaluates the request safety threshold and either claims the
    /// run's one automatic attempt or records a typed request-block reason.
    pub(crate) fn evaluate_request(&self, total_tokens: u64) -> AutomaticCompactDecision {
        if !self
            .request_threshold
            .is_some_and(|threshold| total_tokens > threshold)
        {
            return AutomaticCompactDecision::Continue;
        }

        self.claim_attempt(AutomaticCompactTrigger::RequestThreshold)
    }

    pub(crate) fn record_request_block(&self, block: AutomaticCompactBlock) {
        self.lock_runtime().pending_request_block = Some(block);
    }

    /// Claims a hook-originated compaction yield under the same one-attempt
    /// guard used by threshold evaluation.
    pub(crate) fn claim_hook_yield(&self) -> AutomaticCompactDecision {
        self.claim_attempt(AutomaticCompactTrigger::RequestThreshold)
    }

    pub(crate) fn has_claimed_attempt(&self) -> bool {
        self.lock_runtime().attempt == AutomaticCompactState::Attempted
    }

    /// Completes the currently claimed automatic attempt exactly once.
    pub(crate) fn complete_automatic(&self, outcome: CompactionOutcome) -> bool {
        let mut runtime = self.lock_runtime();
        if runtime.attempt != AutomaticCompactState::Attempted {
            return false;
        }

        runtime.attempt = AutomaticCompactState::Completed(outcome);
        match outcome {
            CompactionOutcome::Succeeded => {
                runtime.consecutive_failures = 0;
                runtime.last_failure = None;
            }
            CompactionOutcome::Failed(category) => {
                runtime.consecutive_failures = runtime.consecutive_failures.saturating_add(1);
                runtime.last_failure = Some(category);
                if runtime.consecutive_failures >= self.max_consecutive_failures {
                    runtime.automatic_disabled = true;
                }
            }
            CompactionOutcome::Cancelled => {}
        }
        true
    }

    /// Re-arms automatic compaction only after the first real provider request
    /// following successful compaction has a durably committed UsageRecord.
    pub(crate) fn post_compact_request_committed(&self) {
        let mut runtime = self.lock_runtime();
        if runtime.attempt == AutomaticCompactState::Completed(CompactionOutcome::Succeeded) {
            runtime.attempt = AutomaticCompactState::NotAttempted;
        }
    }

    /// A successful manual compaction is the recovery path for a disabled
    /// automatic circuit breaker and does not count as an automatic attempt.
    pub(crate) fn reenable_automatic(&self) {
        let mut runtime = self.lock_runtime();
        runtime.attempt = AutomaticCompactState::NotAttempted;
        runtime.consecutive_failures = 0;
        runtime.automatic_disabled = false;
        runtime.last_failure = None;
        runtime.pending_request_block = None;
    }

    pub(crate) fn take_pending_request_block(&self) -> Option<AutomaticCompactBlock> {
        self.lock_runtime().pending_request_block.take()
    }

    fn claim_attempt(&self, trigger: AutomaticCompactTrigger) -> AutomaticCompactDecision {
        let mut runtime = self.lock_runtime();
        if runtime.automatic_disabled {
            let category = runtime
                .last_failure
                .expect("disabled automatic compaction must retain its failure category");
            return AutomaticCompactDecision::Block(AutomaticCompactBlock::Disabled(category));
        }

        match runtime.attempt {
            AutomaticCompactState::NotAttempted => {
                runtime.attempt = AutomaticCompactState::Attempted;
                AutomaticCompactDecision::Start(trigger)
            }
            AutomaticCompactState::Attempted => AutomaticCompactDecision::Continue,
            AutomaticCompactState::Completed(CompactionOutcome::Succeeded) => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Thrash)
            }
            AutomaticCompactState::Completed(CompactionOutcome::Failed(category)) => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Failed(category))
            }
            AutomaticCompactState::Completed(CompactionOutcome::Cancelled) => {
                AutomaticCompactDecision::Block(AutomaticCompactBlock::Cancelled)
            }
        }
    }

    fn lock_runtime(&self) -> std::sync::MutexGuard<'_, AutomaticCompactRuntimeState> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn attempt_state(&self) -> AutomaticCompactState {
        self.lock_runtime().attempt
    }

    #[cfg(test)]
    pub(crate) fn consecutive_failures(&self) -> u32 {
        self.lock_runtime().consecutive_failures
    }

    #[cfg(test)]
    pub(crate) fn automatic_disabled(&self) -> bool {
        self.lock_runtime().automatic_disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILURE: CompactFailureCategory = CompactFailureCategory::Storage;

    #[test]
    fn same_logical_run_claims_only_one_automatic_attempt() {
        let state = CompactState::new(Some(10), Some(10), 2);

        assert_eq!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Start(AutomaticCompactTrigger::PreRun)
        );
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Continue
        );
        assert_eq!(state.attempt_state(), AutomaticCompactState::Attempted);
    }

    #[test]
    fn completed_attempt_blocks_retry_and_preserves_failure_category() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Failed(FAILURE)));

        let block = AutomaticCompactBlock::Failed(FAILURE);
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(block)
        );
        state.record_request_block(block);
        assert_eq!(state.take_pending_request_block(), Some(block));
        assert_eq!(state.consecutive_failures(), 1);
    }

    #[test]
    fn two_failed_automatic_attempts_disable_until_manual_success() {
        let state = CompactState::new(Some(10), Some(10), 2);
        for expected in 1..=2 {
            state.begin_logical_run();
            assert!(matches!(
                state.evaluate_pre_run(11),
                AutomaticCompactDecision::Start(_)
            ));
            assert!(state.complete_automatic(CompactionOutcome::Failed(FAILURE)));
            assert_eq!(state.consecutive_failures(), expected);
        }
        assert!(state.automatic_disabled());

        state.begin_logical_run();
        assert_eq!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Disabled(FAILURE))
        );

        state.reenable_automatic();
        assert!(!state.automatic_disabled());
        assert_eq!(state.consecutive_failures(), 0);
        assert!(matches!(
            state.evaluate_pre_run(11),
            AutomaticCompactDecision::Start(_)
        ));
    }

    #[test]
    fn cancellation_consumes_run_attempt_without_counting_as_failure() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        assert!(state.complete_automatic(CompactionOutcome::Cancelled));

        assert_eq!(state.consecutive_failures(), 0);
        assert!(!state.automatic_disabled());
        assert_eq!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Block(AutomaticCompactBlock::Cancelled)
        );
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
        assert_eq!(state.attempt_state(), AutomaticCompactState::NotAttempted);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
    }

    #[test]
    fn pause_resume_preserves_attempt_while_terminal_finish_clears_it() {
        let state = CompactState::new(None, Some(10), 2);
        assert!(matches!(
            state.evaluate_request(11),
            AutomaticCompactDecision::Start(_)
        ));
        // Pause/resume deliberately performs no state transition.
        assert_eq!(state.attempt_state(), AutomaticCompactState::Attempted);

        state.finish_logical_run();
        assert_eq!(state.attempt_state(), AutomaticCompactState::NotAttempted);
    }
}
