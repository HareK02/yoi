use std::sync::RwLock;

use llm_worker::llm_client::types::Item;
use llm_worker_persistence::SessionId;
use serde::{Deserialize, Serialize};

/// Shared state between PodController and runtime directory.
///
/// Controller updates this in-memory; RuntimeDir writes it to disk.
/// Wrapped in `Arc` for sharing.
pub struct PodSharedState {
    pub pod_name: String,
    pub session_id: SessionId,
    pub manifest_toml: String,
    pub status: RwLock<PodStatus>,
    pub history: RwLock<Vec<Item>>,
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
    ) -> Self {
        Self {
            pod_name,
            session_id,
            manifest_toml,
            status: RwLock::new(PodStatus::Idle),
            history: RwLock::new(Vec::new()),
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
            llm_worker_persistence::new_session_id(),
            "[pod]\nname = \"test-pod\"".into(),
        )
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
