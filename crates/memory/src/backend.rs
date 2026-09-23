//! Typed transport boundary for Workspace-backed Memory operations.
//!
//! Runtime Workers serialize these DTOs to the Workspace API. Persistence and
//! validation are owned by the Workspace authority; this crate deliberately has
//! no repository-local filesystem executor.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::AuditEvent;
use crate::extract::{ExtractedCandidate, ExtractedPayload, StagingEvidence};
use crate::schema::{SourceEvidenceRef, SourceRef};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryBackendOperation {
    Query(MemoryQueryOperation),
    ReadDocument(MemoryDocumentReadOperation),
    UpdateDocument(MemoryDocumentUpdateOperation),
    ResidentSummary(MemoryResidentSummaryOperation),
    AppendAudit(MemoryAppendAuditOperation),
    StageCandidate(MemoryStageCandidateOperation),
    StageExtracted(MemoryStageExtractedOperation),
    StagingList(MemoryStagingListOperation),
    StagingRead(MemoryStagingReadOperation),
    StagingClose(MemoryStagingCloseOperation),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryBackendHttpResponse {
    Ok {
        result: MemoryBackendOperationResult,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryBackendOperationResult {
    ToolOutput(MemoryToolOutput),
    Acknowledged(MemoryBackendAckOutput),
    StagingWritten(MemoryStagingWriteOutput),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryToolOutput {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryQueryOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDocumentReadOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDocumentUpdateOperation {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct MemoryResidentSummaryOperation {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAppendAuditOperation {
    pub event: AuditEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStageCandidateOperation {
    pub source: SourceRef,
    pub extract_run_id: String,
    pub candidate: ExtractedCandidate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StagingEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceEvidenceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStageExtractedOperation {
    pub source: SourceRef,
    pub payload: ExtractedPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingListOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingReadOperation {
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingCloseOperation {
    pub candidate_id: String,
    pub action: MemoryStagingCloseAction,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_memory: Vec<MemoryStagingAffectedMemory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStagingCloseAction {
    Applied,
    Discarded,
    Invalid,
    Duplicate,
    AlreadyCovered,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingAffectedMemory {
    pub operation: MemoryStagingAffectedMemoryOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStagingAffectedMemoryOperation {
    Edit,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryConsolidateStagingOperation {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConsolidationOutput {
    pub status: String,
    pub summary: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub candidate_count: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryBackendAckOutput {
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryStagingWriteOutput {
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub staging_count: usize,
    pub staging_ids: Vec<String>,
}
