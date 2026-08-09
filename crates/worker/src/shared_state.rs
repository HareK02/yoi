use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use protocol::WorkerStatus;
use serde_json::json;
use session_store::SegmentId;

use crate::fs_view::WorkerFsView;

/// Shared state between WorkerController and runtime directory.
///
/// Controller updates this in-memory; RuntimeDir writes the status
/// snapshot to disk. Wrapped in `Arc` for sharing.
///
/// History and typed user-segment mirrors used to live here so the
/// IPC layer could answer `Method::GetHistory`. Those reads now go
/// directly through the session-log sink (`Event::Snapshot` +
/// live events), so this struct holds only status, identity,
/// greeting, and filesystem completion lookup hubs.
pub struct WorkerSharedState {
    pub worker_name: String,
    pub segment_id: SegmentId,
    pub manifest_toml: String,
    pub greeting: protocol::Greeting,
    pub status: RwLock<WorkerStatus>,
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
            status: RwLock::new(WorkerStatus::Idle),
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

    pub fn set_status(&self, status: WorkerStatus) {
        if let Ok(mut s) = self.status.write() {
            *s = status;
        }
    }

    pub fn get_status(&self) -> WorkerStatus {
        self.status.read().map(|s| *s).unwrap_or(WorkerStatus::Idle)
    }

    /// Serialize status as JSON.
    pub fn status_json(&self) -> String {
        let status = self.get_status();
        json!({
            "state": status,
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
    fn initial_status_is_idle() {
        let state = test_state();
        assert_eq!(state.get_status(), WorkerStatus::Idle);
    }

    #[test]
    fn set_and_get_status() {
        let state = test_state();
        state.set_status(WorkerStatus::Running);
        assert_eq!(state.get_status(), WorkerStatus::Running);
        state.set_status(WorkerStatus::Paused);
        assert_eq!(state.get_status(), WorkerStatus::Paused);
    }

    #[test]
    fn status_json_contains_fields() {
        let state = test_state();
        let json = state.status_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["state"], "idle");
        assert_eq!(parsed["worker_name"], "test-worker");
        assert!(parsed["segment_id"].is_string());
    }

    #[test]
    fn status_json_reflects_changes() {
        let state = test_state();
        state.set_status(WorkerStatus::Running);
        let json = state.status_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["state"], "running");
    }
}
