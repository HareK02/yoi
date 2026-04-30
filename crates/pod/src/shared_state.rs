use std::sync::{OnceLock, RwLock};

use llm_worker::llm_client::types::Item;
use protocol::Segment;
use serde::{Deserialize, Serialize};
use session_store::SessionId;

use crate::fs_view::PodFsView;

/// Shared state between PodController and runtime directory.
///
/// Controller updates this in-memory; RuntimeDir writes it to disk.
/// Wrapped in `Arc` for sharing.
pub struct PodSharedState {
    pub pod_name: String,
    pub session_id: SessionId,
    pub manifest_toml: String,
    pub greeting: protocol::Greeting,
    pub status: RwLock<PodStatus>,
    pub history: RwLock<Vec<Item>>,
    /// Typed user submissions in submit order. The K-th entry corresponds
    /// to the K-th `Item::user_message` in `history` (modulo seed history
    /// loaded from a pre-compaction `SessionStart.history`, whose original
    /// segments are not preserved). Surfaced via `Event::History` so
    /// clients can re-render typed atoms on session restore.
    pub user_segments: RwLock<Vec<Vec<Segment>>>,
    /// Pod-from-the-inside view of the filesystem. Set once in
    /// `PodController::start` after the `ScopedFs` is materialised, and
    /// read from the IPC server layer to answer `ListCompletions`
    /// queries without going through the controller. `None` until set
    /// (only relevant for unit tests that build a `PodSharedState`
    /// directly without spinning up a controller).
    fs_view: OnceLock<PodFsView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PodStatus {
    Idle,
    Running,
    Paused,
}

impl PodSharedState {
    pub fn new(
        pod_name: String,
        session_id: SessionId,
        manifest_toml: String,
        greeting: protocol::Greeting,
    ) -> Self {
        Self {
            pod_name,
            session_id,
            manifest_toml,
            greeting,
            status: RwLock::new(PodStatus::Idle),
            history: RwLock::new(Vec::new()),
            user_segments: RwLock::new(Vec::new()),
            fs_view: OnceLock::new(),
        }
    }

    /// Attach the Pod's filesystem view. Called once during controller
    /// startup. Subsequent calls are silently ignored (`OnceLock`).
    pub fn set_fs_view(&self, view: PodFsView) {
        let _ = self.fs_view.set(view);
    }

    /// Borrow the attached `PodFsView`, if any. Returns `None` for unit
    /// tests that didn't wire one up.
    pub fn fs_view(&self) -> Option<&PodFsView> {
        self.fs_view.get()
    }

    pub fn user_segments(&self) -> Vec<Vec<Segment>> {
        self.user_segments
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn set_user_segments(&self, segments: Vec<Vec<Segment>>) {
        if let Ok(mut s) = self.user_segments.write() {
            *s = segments;
        }
    }

    pub fn push_user_segments(&self, segments: Vec<Segment>) {
        if let Ok(mut s) = self.user_segments.write() {
            s.push(segments);
        }
    }

    pub fn set_status(&self, status: PodStatus) {
        if let Ok(mut s) = self.status.write() {
            *s = status;
        }
    }

    pub fn get_status(&self) -> PodStatus {
        self.status.read().map(|s| *s).unwrap_or(PodStatus::Idle)
    }

    pub fn history(&self) -> Vec<Item> {
        self.history.read().map(|h| h.clone()).unwrap_or_default()
    }

    pub fn update_history(&self, items: Vec<Item>) {
        if let Ok(mut h) = self.history.write() {
            *h = items;
        }
    }

    /// Serialize status as JSON.
    pub fn status_json(&self) -> String {
        let status = self.get_status();
        serde_json::json!({
            "state": status,
            "session_id": self.session_id.to_string(),
            "pod_name": self.pod_name,
        })
        .to_string()
    }

    /// Serialize history as JSON.
    pub fn history_json(&self) -> String {
        if let Ok(h) = self.history.read() {
            serde_json::to_string(&*h).unwrap_or_else(|_| "[]".into())
        } else {
            "[]".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_worker::llm_client::types::{ContentPart, Item, Role};

    fn test_state() -> PodSharedState {
        PodSharedState::new(
            "test-pod".into(),
            session_store::new_session_id(),
            "[pod]\nname = \"test-pod\"".into(),
            test_greeting(),
        )
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            pod_name: "test-pod".into(),
            cwd: "/tmp".into(),
            provider: "anthropic".into(),
            model: "claude".into(),
            scope_summary: String::new(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn initial_status_is_idle() {
        let state = test_state();
        assert_eq!(state.get_status(), PodStatus::Idle);
    }

    #[test]
    fn set_and_get_status() {
        let state = test_state();
        state.set_status(PodStatus::Running);
        assert_eq!(state.get_status(), PodStatus::Running);
        state.set_status(PodStatus::Paused);
        assert_eq!(state.get_status(), PodStatus::Paused);
    }

    #[test]
    fn status_json_contains_fields() {
        let state = test_state();
        let json = state.status_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["state"], "idle");
        assert_eq!(parsed["pod_name"], "test-pod");
        assert!(parsed["session_id"].is_string());
    }

    #[test]
    fn status_json_reflects_changes() {
        let state = test_state();
        state.set_status(PodStatus::Running);
        let json = state.status_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["state"], "running");
    }

    #[test]
    fn history_json_empty_initially() {
        let state = test_state();
        assert_eq!(state.history_json(), "[]");
    }

    #[test]
    fn history_json_after_update() {
        let state = test_state();
        let items = vec![Item::Message {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::Text {
                text: "Hello".into(),
            }],
            status: None,
        }];
        state.update_history(items);
        let json = state.history_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["role"], "assistant");
    }
}
