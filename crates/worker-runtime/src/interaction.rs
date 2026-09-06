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
    /// Authenticated client-generated idempotency key. Runtime generates one
    /// only for trusted internal callers that omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
}

impl WorkerInput {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::User,
            content: content.into(),
            submission_request_id: None,
            segments: None,
        }
    }

    pub fn notify(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::Notify,
            content: content.into(),
            submission_request_id: None,
            segments: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerInput;

    #[test]
    fn submission_request_id_round_trips_for_authenticated_client_retry() {
        let input: WorkerInput = serde_json::from_value(serde_json::json!({
            "kind": "user",
            "content": "message",
            "submission_request_id": "request-1"
        }))
        .unwrap();
        assert_eq!(input.submission_request_id.as_deref(), Some("request-1"));
        assert_eq!(
            serde_json::to_value(input).unwrap()["submission_request_id"],
            "request-1"
        );
    }

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
    /// Present for User Submit and absent for non-Submit interactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission: Option<crate::execution::WorkerSubmissionAck>,
}
