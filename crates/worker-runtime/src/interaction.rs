use crate::catalog::WorkerStatus;
use crate::identity::WorkerRef;
use protocol::Segment;
use serde::{Deserialize, Serialize};

/// Input kind accepted by the embedded interaction API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    System,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
}

impl WorkerInput {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::User,
            content: content.into(),
            segments: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::System,
            content: content.into(),
            segments: None,
        }
    }
}

/// Acknowledgement returned after input is accepted into the Worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInteractionAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    pub event_id: u64,
}
