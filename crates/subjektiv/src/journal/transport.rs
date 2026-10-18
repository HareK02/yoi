//! Bounded, Host-bound Journal recording and sequential Job transport.
//!
//! Subject, Journal and generation authority never come from these model inputs.
//! Hosts validate live execution and public source bytes before selection; reading a
//! preview must not select the enclosing entry. Operation retries retain the entire
//! JournalOperation, including its original expected_state and covered_ranges.

use serde::{Deserialize, Serialize};

use super::{
    JournalEntrySummary, JournalMetadata, JournalOperation, JournalOperationStatus, JournalPage,
    JournalReceipt, JournalSourcePointer, JournalSourceRange, JournalTextPage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalRecordingReason {
    RunCommitted,
    TaskCompleted,
    BeforeCompact,
    WorkerRestored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalCompactionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecordingRequest {
    pub reason: JournalRecordingReason,
}

/// Admission is durable Host evidence, not proof that generation finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalRecordingResponse {
    NoNewSources,
    Accepted,
}

/// No unbounded initial/current body or position is included in model state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalJobState {
    pub generation_id: String,
    pub initial_journal: JournalMetadata,
    pub initial_open: Option<JournalEntrySummary>,
    pub current_journal: JournalMetadata,
    pub current_open: Option<JournalEntrySummary>,
    pub previous_fixed_end: Option<JournalSourcePointer>,
    /// Frozen permission to fetch this prefix, not evidence of delivery or reflection.
    /// The sequential service gates mutations on its durable Source-read delivery ledger.
    pub selected_ranges: Vec<JournalSourceRange>,
}

/// Host-verified saved progress only; not an aggregate record submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalJobCompletion {
    pub generation_id: String,
    pub state_token: String,
    pub process_order: u64,
    pub process_byte: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalSnapshot {
    Initial,
    Current,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalTextTarget {
    Position {
        snapshot: JournalSnapshot,
    },
    OpenBody {
        snapshot: JournalSnapshot,
    },
    OpenOngoing {
        snapshot: JournalSnapshot,
    },
    /// The Host restricts this pointer to this Job's frozen public source input.
    Source {
        pointer: JournalSourcePointer,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalTextRead {
    pub target: JournalTextTarget,
    pub offset: usize,
    pub limit: usize,
    /// Required continuation condition; None only for the first chunk.
    pub content_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalJobRequest {
    Execute {
        operation: JournalOperation,
    },
    ReadState,
    QueryOperation {
        request_id: String,
    },
    ReadOperationCoverage {
        request_id: String,
        offset: usize,
        limit: usize,
    },
    ListOperations {
        after: Option<String>,
        limit: usize,
    },
    ReadText {
        read: JournalTextRead,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum JournalJobResponse {
    Saved(JournalReceipt),
    State(JournalJobState),
    Operation(Option<JournalOperationStatus>),
    Coverage(JournalPage<JournalSourceRange>),
    Operations(JournalPage<JournalOperationStatus>),
    Text(JournalTextPage),
}
