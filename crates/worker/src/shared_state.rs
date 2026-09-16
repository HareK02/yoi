use std::collections::VecDeque;
use std::sync::{
    OnceLock, RwLock,
    atomic::{AtomicBool, Ordering},
};

use protocol::{
    WorkerBusyState, WorkerCommandDisposition, WorkerCommandEnvelope, WorkerCommandKind,
    WorkerMaintenanceState, WorkerRunState, WorkerState, WorkerStateSnapshot, WorkerStatus,
};
use serde_json::json;
use session_store::SegmentId;

use crate::fs_view::WorkerFsView;

const COMPLETED_COMMAND_RETENTION: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AcceptedWorkerCommand {
    envelope: WorkerCommandEnvelope,
    kind: WorkerCommandKind,
    disposition: Option<WorkerCommandDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerCommandAdmission {
    Accepted,
    Retry,
    Conflict,
    StaleCommandId,
}

/// Shared state between WorkerController and runtime directory.
///
/// `WorkerStateSnapshot` is the sole live execution-state authority. Runtime
/// catalog status remains a separate lifecycle projection because `Stopped`
/// describes the execution handle rather than a live controller state.
pub struct WorkerSharedState {
    pub worker_name: String,
    pub segment_id: SegmentId,
    pub manifest_toml: String,
    pub greeting: protocol::Greeting,
    state: RwLock<WorkerStateSnapshot>,
    accepted_commands: RwLock<VecDeque<AcceptedWorkerCommand>>,
    /// Worker-from-the-inside view of the filesystem. Set once in
    /// `WorkerController::start` after the local WorkdirSession provider is
    /// materialised, and read from the IPC server layer to answer
    /// `ListCompletions` queries without going through the controller. It is
    /// unset only in unit tests that construct `WorkerSharedState` directly.
    fs_view: OnceLock<WorkerFsView>,
    flow_transition_enabled: AtomicBool,
}

impl WorkerSharedState {
    pub fn new(
        worker_name: String,
        segment_id: SegmentId,
        manifest_toml: String,
        greeting: protocol::Greeting,
    ) -> Self {
        Self {
            worker_name,
            segment_id,
            manifest_toml,
            greeting,
            state: RwLock::new(WorkerStateSnapshot::initial()),
            accepted_commands: RwLock::new(VecDeque::new()),
            fs_view: OnceLock::new(),
            flow_transition_enabled: AtomicBool::new(false),
        }
    }

    /// Attach the Worker's filesystem view. Called once during controller
    /// startup. Subsequent calls are silently ignored (`OnceLock`).
    pub fn set_fs_view(&self, view: WorkerFsView) {
        let _ = self.fs_view.set(view);
    }

    /// Borrow the attached `WorkerFsView`, if any. Returns `None` for unit
    /// tests that didn't wire one up.
    pub fn fs_view(&self) -> Option<&WorkerFsView> {
        self.fs_view.get()
    }

    pub fn enable_flow_transition(&self) {
        self.flow_transition_enabled.store(true, Ordering::Release);
    }

    pub fn flow_transition_enabled(&self) -> bool {
        self.flow_transition_enabled.load(Ordering::Acquire)
    }

    pub fn transition(&self, state: WorkerState) -> WorkerStateSnapshot {
        let mut snapshot = self
            .state
            .write()
            .expect("worker state lock poisoned; refusing an inferred fallback state");
        if snapshot.state != state {
            snapshot.state = state;
        }
        snapshot.clone()
    }

    pub(crate) fn admit_command(
        &self,
        envelope: WorkerCommandEnvelope,
        kind: WorkerCommandKind,
    ) -> WorkerCommandAdmission {
        let mut snapshot = self
            .state
            .write()
            .expect("worker state lock poisoned; refusing command admission");
        let mut accepted = self
            .accepted_commands
            .write()
            .expect("worker command ledger lock poisoned; refusing command admission");
        if let Some(existing) = accepted
            .iter()
            .find(|accepted| accepted.envelope.command_id == envelope.command_id)
        {
            return if existing.envelope == envelope && existing.kind == kind {
                WorkerCommandAdmission::Retry
            } else {
                WorkerCommandAdmission::Conflict
            };
        }
        if envelope.command_id <= snapshot.last_command_id {
            return WorkerCommandAdmission::StaleCommandId;
        }

        snapshot.last_command_id = envelope.command_id;
        accepted.push_back(AcceptedWorkerCommand {
            envelope,
            kind,
            disposition: None,
        });
        WorkerCommandAdmission::Accepted
    }

    pub(crate) fn complete_command(
        &self,
        command_id: u64,
        kind: WorkerCommandKind,
        disposition: WorkerCommandDisposition,
    ) {
        if !matches!(
            disposition,
            WorkerCommandDisposition::Accepted | WorkerCommandDisposition::InvalidState
        ) {
            return;
        }
        let mut accepted = self
            .accepted_commands
            .write()
            .expect("worker command ledger lock poisoned; refusing command completion");
        if let Some(command) = accepted
            .iter_mut()
            .find(|command| command.envelope.command_id == command_id && command.kind == kind)
        {
            command.disposition.get_or_insert(disposition);
        }
        while accepted
            .iter()
            .filter(|command| command.disposition.is_some())
            .count()
            > COMPLETED_COMMAND_RETENTION
        {
            let Some(index) = accepted
                .iter()
                .position(|command| command.disposition.is_some())
            else {
                break;
            };
            accepted.remove(index);
        }
    }

    #[cfg(test)]
    pub(crate) fn command_result(
        &self,
        command_id: u64,
    ) -> Option<Option<WorkerCommandDisposition>> {
        self.accepted_commands
            .read()
            .expect("worker command ledger lock poisoned")
            .iter()
            .find(|command| command.envelope.command_id == command_id)
            .map(|command| command.disposition)
    }

    pub fn snapshot(&self) -> WorkerStateSnapshot {
        self.state
            .read()
            .expect("worker state lock poisoned; refusing an inferred fallback state")
            .clone()
    }

    /// Runtime catalog projection. This must not be used as live command
    /// admission authority.
    pub fn catalog_status(&self) -> WorkerStatus {
        match self.snapshot().state {
            WorkerState::Idle => WorkerStatus::Idle,
            WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused)) => WorkerStatus::Paused,
            WorkerState::Busy(WorkerBusyState::Run(_))
            | WorkerState::Busy(WorkerBusyState::Maintenance(WorkerMaintenanceState::Compacting)) => {
                WorkerStatus::Running
            }
        }
    }

    /// Serialize the runtime-directory lifecycle projection as JSON while
    /// retaining the full state snapshot for diagnostics and reconnects.
    pub fn status_json(&self) -> String {
        let snapshot = self.snapshot();
        json!({
            "state": self.catalog_status(),
            "worker_state": snapshot,
            "segment_id": self.segment_id.to_string(),
            "worker_name": self.worker_name,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> WorkerSharedState {
        WorkerSharedState::new(
            "test-worker".into(),
            session_store::new_segment_id(),
            "[engine]\nname = \"test-worker\"".into(),
            test_greeting(),
        )
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            worker_name: "test-worker".into(),
            cwd: "/tmp".into(),
            provider: "anthropic".into(),
            model: "claude".into(),
            scope_summary: String::new(),
            tools: Vec::new(),
            context_window: 200_000,
            context_tokens: 0,
        }
    }

    #[test]
    fn initial_snapshot_is_idle() {
        let state = test_state();
        assert_eq!(state.snapshot(), WorkerStateSnapshot::initial());
        assert_eq!(state.catalog_status(), WorkerStatus::Idle);
    }

    #[test]
    fn transitions_publish_full_state() {
        let state = test_state();
        let running = WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running));
        assert_eq!(state.transition(running.clone()).state, running);

        let paused = WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Paused));
        let snapshot = state.transition(paused.clone());
        assert_eq!(snapshot.state, paused);
        assert_eq!(snapshot.last_command_id, 0);
        assert_eq!(state.catalog_status(), WorkerStatus::Paused);
    }

    #[test]
    fn accepted_command_identity_advances_last_id_and_detects_reuse_conflicts() {
        let state = test_state();
        let envelope = WorkerCommandEnvelope { command_id: 9 };
        assert_eq!(
            state.admit_command(envelope, WorkerCommandKind::Pause),
            WorkerCommandAdmission::Accepted
        );
        assert_eq!(
            state.snapshot(),
            WorkerStateSnapshot {
                last_command_id: 9,
                state: WorkerState::Idle,
            }
        );
        assert_eq!(state.command_result(9), Some(None));
        state.complete_command(
            9,
            WorkerCommandKind::Pause,
            WorkerCommandDisposition::Accepted,
        );
        assert_eq!(
            state.command_result(9),
            Some(Some(WorkerCommandDisposition::Accepted))
        );
        assert_eq!(
            state.admit_command(envelope, WorkerCommandKind::Pause),
            WorkerCommandAdmission::Retry
        );
        assert_eq!(
            state.admit_command(envelope, WorkerCommandKind::Cancel),
            WorkerCommandAdmission::Conflict
        );
        assert_eq!(state.snapshot().last_command_id, 9);
    }

    #[test]
    fn status_json_contains_full_snapshot_and_catalog_projection() {
        let state = test_state();
        state.transition(WorkerState::Busy(WorkerBusyState::Maintenance(
            WorkerMaintenanceState::Compacting,
        )));
        let parsed: serde_json::Value = serde_json::from_str(&state.status_json()).unwrap();
        assert_eq!(parsed["state"], "running");
        assert!(parsed["worker_state"].get("execution_generation").is_none());
        assert!(parsed["worker_state"].get("revision").is_none());
        assert_eq!(parsed["worker_state"]["state"]["kind"], "busy");
        assert_eq!(parsed["worker_name"], "test-worker");
        assert!(parsed["segment_id"].is_string());
    }
}
