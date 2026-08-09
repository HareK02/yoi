use crate::catalog::WorkerStatus;
use crate::identity::WorkerRef;
use protocol::Segment;
use serde::{Deserialize, Serialize};

/// Input kind accepted by the embedded interaction API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    Notify,
    Compact,
    ListRewindTargets,
    RegisterPeer,
}

impl WorkerInputKind {
    pub fn is_empty_content_allowed(&self) -> bool {
        matches!(self, Self::Compact | Self::ListRewindTargets)
    }
}

/// Worker input request accepted by a Runtime Worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInput {
    pub kind: WorkerInputKind,
    pub content: String,
    /// Runtime-generated correlation id. This is never accepted from public
    /// JSON input and is consumed only by the execution backend.
    #[serde(skip)]
    pub submission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
}

impl WorkerInput {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::User,
            content: content.into(),
            submission_id: None,
            segments: None,
        }
    }

    pub fn notify(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::Notify,
            content: content.into(),
            submission_id: None,
            segments: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerInput;

    #[test]
    fn notify_is_an_operation_and_legacy_system_kind_is_rejected() {
        assert_eq!(
            serde_json::to_value(WorkerInput::notify("message")).unwrap(),
            serde_json::json!({ "kind": "notify", "content": "message" })
        );
        assert!(
            serde_json::from_value::<WorkerInput>(serde_json::json!({
                "kind": "system",
                "content": "message"
            }))
            .is_err()
        );
    }
}

/// Acknowledgement returned after input is accepted into the Worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInteractionAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
}
