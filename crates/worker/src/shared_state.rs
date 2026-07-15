use std::sync::{OnceLock, RwLock};

use protocol::WorkerStatus;
use serde_json::json;
use session_store::SegmentId;

use crate::fs_view::WorkerFsView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeCandidate {
    pub slug: String,
}

/// Shared state between WorkerController and runtime directory.
///
/// Controller updates this in-memory; RuntimeDir writes the status
/// snapshot to disk. Wrapped in `Arc` for sharing.
///
/// History and typed user-segment mirrors used to live here so the
/// IPC layer could answer `Method::GetHistory`. Those reads now go
/// directly through the session-log sink (`Event::Snapshot` +
/// live events), so this struct holds only status, identity,
/// greeting, and completion lookup hubs.
pub struct WorkerSharedState {
    pub worker_name: String,
    pub segment_id: SegmentId,
    pub manifest_toml: String,
    pub greeting: protocol::Greeting,
    pub status: RwLock<WorkerStatus>,
    /// Worker-from-the-inside view of the filesystem. Set once in
    /// `WorkerController::start` after the `ScopedFs` is materialised, and
    /// read from the IPC server layer to answer `ListCompletions`
    /// queries without going through the controller. `None` until set
    /// (only relevant for unit tests that build a `WorkerSharedState`
    /// directly without spinning up a controller).
    fs_view: OnceLock<WorkerFsView>,
    knowledge: OnceLock<Vec<KnowledgeCandidate>>,
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
            knowledge: OnceLock::new(),
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

    pub fn set_knowledge(&self, knowledge: Vec<KnowledgeCandidate>) {
        let _ = self.knowledge.set(knowledge);
    }

    pub fn list_knowledge_completions(&self, prefix: &str) -> Vec<KnowledgeCandidate> {
        self.knowledge
            .get()
            .map(|items| {
                items
                    .iter()
                    .filter(|candidate| candidate.slug.starts_with(prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
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

    #[test]
    fn knowledge_completions_empty_when_unset() {
        let state = test_state();
        assert!(state.list_knowledge_completions("").is_empty());
        assert!(state.list_knowledge_completions("foo").is_empty());
    }

    #[test]
    fn knowledge_completions_filter_by_prefix() {
        let state = test_state();
        state.set_knowledge(vec![
            KnowledgeCandidate {
                slug: "alpha".into(),
            },
            KnowledgeCandidate {
                slug: "alphabet".into(),
            },
            KnowledgeCandidate {
                slug: "beta".into(),
            },
        ]);
        let all = state.list_knowledge_completions("");
        assert_eq!(all.len(), 3);
        let alpha = state.list_knowledge_completions("alpha");
        assert_eq!(
            alpha.iter().map(|c| c.slug.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "alphabet"]
        );
        assert!(state.list_knowledge_completions("zzz").is_empty());
    }
}
