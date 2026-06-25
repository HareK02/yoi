use crate::catalog::WorkerStatus;
use crate::identity::WorkerRef;
use serde::{Deserialize, Serialize};

/// Input kind accepted by the embedded interaction API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerInputKind {
    User,
    System,
}

/// Worker input request.  v0 stores the input in an in-memory transcript and
/// does not execute providers/tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInput {
    pub kind: WorkerInputKind,
    pub content: String,
}

impl WorkerInput {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::User,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            kind: WorkerInputKind::System,
            content: content.into(),
        }
    }
}

/// Acknowledgement returned after input is accepted into the in-memory Worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInteractionAck {
    pub worker_ref: WorkerRef,
    pub status: WorkerStatus,
    pub transcript_sequence: u64,
    pub event_id: u64,
}
