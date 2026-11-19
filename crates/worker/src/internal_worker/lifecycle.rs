//! Direct-child facts are owned by the actor, not by registry removal or UI status.
//! Sources and retry tasks live only in this process. A retained parent receipt does
//! not retain the child's ephemeral session after registry removal, or prove model
//! processing. No child restoration or Subject-body routing is implied.

use super::*;

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct InternalWorkerLifecycleEvent {
    pub session_id: String,
    pub source_id: String,
    pub input_id: Option<String>,
    pub timestamp_ms: u64,
    pub fact: InternalWorkerLifecycleFact,
    pub terminal_entry_id: Option<String>,
    pub source_recorded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) enum InternalWorkerLifecycleFact {
    RunFinished,
    RunCancelled,
    RunRolledBack,
    RunLimitReached,
    RunInterrupted,
    RunFailed,
    ExecutionUnknown,
    Stopped { reason: &'static str },
    StopUnconfirmed,
}

impl InternalWorkerLifecycleFact {
    fn from_outcome(outcome: &InternalWorkerTurnOutcome) -> Option<Self> {
        Some(match outcome {
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Finished) => Self::RunFinished,
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Paused) => return None,
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Cancelled) => Self::RunCancelled,
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::RolledBack) => {
                Self::RunRolledBack
            }
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::LimitReached) => {
                Self::RunLimitReached
            }
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Interrupted { .. }) => {
                Self::RunInterrupted
            }
            InternalWorkerTurnOutcome::Failed { .. } => Self::RunFailed,
        })
    }
}

pub(crate) type InternalWorkerLifecycleCallback =
    Arc<dyn Fn(InternalWorkerLifecycleEvent) + Send + Sync>;

pub(super) struct LifecyclePublisher {
    pub(super) session_id: String,
    pub(super) writer: crate::worker::LogWriterHandle<EphemeralSessionStore>,
    pub(super) callback: Option<InternalWorkerLifecycleCallback>,
}

impl LifecyclePublisher {
    pub(super) fn publish(
        &self,
        input_id: Option<&str>,
        fact: InternalWorkerLifecycleFact,
        terminal_entry_id: Option<String>,
    ) {
        let boundary = if matches!(
            fact,
            InternalWorkerLifecycleFact::Stopped { .. }
                | InternalWorkerLifecycleFact::StopUnconfirmed
        ) {
            "stop"
        } else {
            input_id.unwrap_or("actor")
        };
        let mut event = InternalWorkerLifecycleEvent {
            session_id: self.session_id.clone(),
            source_id: format!("internal-lifecycle:{}:{boundary}", self.session_id),
            input_id: input_id.map(str::to_owned),
            timestamp_ms: session_store::segment_log::now_millis(),
            fact,
            terminal_entry_id,
            source_recorded: true,
        };
        if let Some(callback) = &self.callback {
            // The same ephemeral Store owns source facts and terminal history.
            // Failure is visible in the delivery, never presented as saved evidence.
            if self
                .writer
                .append_entry(LogEntry::Extension {
                    ts: event.timestamp_ms,
                    domain: "internal_worker_lifecycle".into(),
                    payload: serde_json::to_value(&event).expect("lifecycle fact is serializable"),
                })
                .is_err()
            {
                event.source_recorded = false;
                tracing::warn!(source_id = %event.source_id, "Internal lifecycle source persistence failed");
            }
            callback(event);
        }
    }

    pub(super) fn run_ended(
        &self,
        input_id: &str,
        outcome: &InternalWorkerTurnOutcome,
        terminal_entry_id: Option<String>,
    ) {
        if let Some(fact) = InternalWorkerLifecycleFact::from_outcome(outcome) {
            self.publish(Some(input_id), fact, terminal_entry_id);
        }
    }
}

pub(super) fn latest_terminal_id(sink: &SegmentLogSink) -> Option<String> {
    sink.subscribe_with_snapshot()
        .0
        .iter()
        .rev()
        .find_map(|entry| match entry {
            LogEntry::RunCompleted {
                entry_id, result, ..
            } if !matches!(result, agen::EngineResult::Paused) => {
                entry_id.as_ref().map(|id| id.0.clone())
            }
            LogEntry::RunCancelled { entry_id, .. } | LogEntry::RunErrored { entry_id, .. } => {
                entry_id.as_ref().map(|id| id.0.clone())
            }
            _ => None,
        })
}
