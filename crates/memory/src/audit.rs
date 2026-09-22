//! Typed Memory audit events sent to the Workspace authority.
//!
//! Runtime Workers construct these DTOs and submit them through the Memory
//! backend operation boundary. Repository-local audit logs are not an authority.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditWorker {
    MemoryExtract,
    MemoryConsolidation,
}

impl AuditWorker {
    pub fn label(self) -> &'static str {
        match self {
            Self::MemoryExtract => "extract",
            Self::MemoryConsolidation => "consolidation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLifecycleStatus {
    Started,
    Completed,
    Skipped,
    Failed,
    Cancelled,
}

impl WorkerLifecycleStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Started => "running",
            Self::Completed => "done",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTrigger {
    SessionEnd,
    TurnThreshold,
    TokenThreshold,
    StagingBacklog,
    Idle,
    Manual,
    StartupRecovery,
    Unknown,
}

impl AuditTrigger {
    pub fn label(self) -> &'static str {
        match self {
            Self::SessionEnd => "session_end",
            Self::TurnThreshold => "turn_threshold",
            Self::TokenThreshold => "token_threshold",
            Self::StagingBacklog => "staging_backlog",
            Self::Idle => "idle",
            Self::Manual => "manual",
            Self::StartupRecovery => "startup_recovery",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAudit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAudit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractAudit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_range: Option<[u64; 2]>,
    #[serde(default)]
    pub staging_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub staging_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub staging_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationAudit {
    #[serde(default)]
    pub staging_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub invalid_staging_count: usize,
    #[serde(default)]
    pub staging_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed_staging_ids: Vec<String>,
    #[serde(default)]
    pub operations: OperationCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationCounts {
    #[serde(default)]
    pub write: usize,
    #[serde(default)]
    pub edit: usize,
    #[serde(default)]
    pub delete: usize,
    #[serde(default)]
    pub drop: usize,
    #[serde(default)]
    pub merge: usize,
    #[serde(default)]
    pub trim: usize,
}

impl OperationCounts {
    pub fn total_record_changes(&self) -> usize {
        self.write + self.edit + self.delete + self.drop + self.merge + self.trim
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySettingsAudit {
    pub workspace_id: String,
    pub settings_revision: u64,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLifecycleAudit {
    pub run_id: Uuid,
    pub worker: AuditWorker,
    pub status: WorkerLifecycleStatus,
    pub trigger: AuditTrigger,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_settings: Option<MemorySettingsAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtractAudit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consolidation: Option<ConsolidationAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordOperationAudit {
    pub op: String,
    pub status: AuditStatus,
    pub kind: String,
    pub slug: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordUsageAudit {
    pub op: String,
    pub status: AuditStatus,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditPayload {
    WorkerLifecycle(WorkerLifecycleAudit),
    RecordOperation(RecordOperationAudit),
    RecordUsage(RecordUsageAudit),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    #[serde(flatten)]
    pub payload: AuditPayload,
}

impl AuditEvent {
    pub fn new(payload: AuditPayload) -> Self {
        Self {
            id: Uuid::now_v7(),
            occurred_at: Utc::now(),
            payload,
        }
    }
}
