//! Append-only JSONL audit log for memory workers and tools.
//!
//! The log is evidence-only observability data under
//! `.yoi/memory/_logs/current.log`. It is intentionally separate from
//! `_staging` and `_usage`, and consolidation never consumes it. Operators can
//! follow the latest stream with:
//!
//! ```text
//! tail -f .yoi/memory/_logs/current.log
//! ```

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::workspace::WorkspaceLayout;

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
pub struct WorkerLifecycleAudit {
    pub run_id: Uuid,
    pub worker: AuditWorker,
    pub status: WorkerLifecycleStatus,
    pub trigger: AuditTrigger,
    pub reason: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSnapshot {
    pub kind: String,
    pub slug: String,
    pub path: PathBuf,
    pub hash: String,
}

/// Append one audit event to `.yoi/memory/_logs/current.log`.
pub fn append_audit_event(layout: &WorkspaceLayout, event: &AuditEvent) -> io::Result<()> {
    let path = layout.audit_current_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(event)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn append_worker_lifecycle(
    layout: &WorkspaceLayout,
    audit: WorkerLifecycleAudit,
) -> io::Result<()> {
    append_audit_event(
        layout,
        &AuditEvent::new(AuditPayload::WorkerLifecycle(audit)),
    )
}

pub fn append_record_operation(
    layout: &WorkspaceLayout,
    audit: RecordOperationAudit,
) -> io::Result<()> {
    append_audit_event(
        layout,
        &AuditEvent::new(AuditPayload::RecordOperation(audit)),
    )
}

pub fn append_record_usage(layout: &WorkspaceLayout, audit: RecordUsageAudit) -> io::Result<()> {
    append_audit_event(layout, &AuditEvent::new(AuditPayload::RecordUsage(audit)))
}

pub fn file_hash(path: &Path) -> io::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(hash_bytes(&bytes))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn snapshot_records(layout: &WorkspaceLayout) -> BTreeMap<String, RecordSnapshot> {
    let mut out = BTreeMap::new();
    snapshot_one(&mut out, "summary", "summary", layout.summary_path());
    snapshot_dir(&mut out, "decision", layout.decisions_dir());
    snapshot_dir(&mut out, "request", layout.requests_dir());
    snapshot_dir(&mut out, "knowledge", layout.knowledge_dir());
    out
}

pub fn operation_counts_from_snapshots(
    before: &BTreeMap<String, RecordSnapshot>,
    after: &BTreeMap<String, RecordSnapshot>,
) -> OperationCounts {
    let mut counts = OperationCounts::default();
    for (key, after_record) in after {
        match before.get(key) {
            None => counts.write += 1,
            Some(before_record) if before_record.hash != after_record.hash => counts.edit += 1,
            Some(_) => {}
        }
    }
    for key in before.keys() {
        if !after.contains_key(key) {
            counts.delete += 1;
        }
    }
    counts
}

fn snapshot_dir(out: &mut BTreeMap<String, RecordSnapshot>, kind: &str, dir: PathBuf) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(slug) = name.strip_suffix(".md").map(str::to_string) else {
            continue;
        };
        snapshot_one(out, kind, &slug, path);
    }
}

fn snapshot_one(out: &mut BTreeMap<String, RecordSnapshot>, kind: &str, slug: &str, path: PathBuf) {
    if !path.is_file() {
        return;
    }
    let Ok(Some(hash)) = file_hash(&path) else {
        return;
    };
    out.insert(
        format!("{kind}/{slug}"),
        RecordSnapshot {
            kind: kind.to_string(),
            slug: slug.to_string(),
            path,
            hash,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        (dir, layout)
    }

    #[test]
    fn appends_jsonl_to_current_log() {
        let (_dir, layout) = setup();
        let run_id = Uuid::now_v7();
        append_worker_lifecycle(
            &layout,
            WorkerLifecycleAudit {
                run_id,
                worker: AuditWorker::MemoryExtract,
                status: WorkerLifecycleStatus::Started,
                trigger: AuditTrigger::TokenThreshold,
                reason: "tokens_threshold_reached".to_string(),
                model: None,
                usage: None,
                extract: None,
                consolidation: None,
            },
        )
        .unwrap();

        let text = fs::read_to_string(layout.audit_current_log_path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["event"], "worker_lifecycle");
        assert_eq!(value["worker"], "memory_extract");
        assert_eq!(value["status"], "started");
        assert_eq!(value["run_id"], run_id.to_string());
    }

    #[test]
    fn counts_created_edited_deleted_records() {
        let (dir, layout) = setup();
        let decision_dir = dir.path().join(".yoi/memory/decisions");
        fs::create_dir_all(&decision_dir).unwrap();
        fs::write(decision_dir.join("a.md"), "old").unwrap();
        fs::write(decision_dir.join("gone.md"), "old").unwrap();
        let before = snapshot_records(&layout);

        fs::write(decision_dir.join("a.md"), "new").unwrap();
        fs::remove_file(decision_dir.join("gone.md")).unwrap();
        fs::write(decision_dir.join("created.md"), "new").unwrap();
        let after = snapshot_records(&layout);

        let counts = operation_counts_from_snapshots(&before, &after);
        assert_eq!(counts.write, 1);
        assert_eq!(counts.edit, 1);
        assert_eq!(counts.delete, 1);
    }

    #[test]
    fn hash_has_sha256_prefix() {
        assert_eq!(hash_bytes(b"abc").len(), "sha256:".len() + 64);
        assert!(hash_bytes(b"abc").starts_with("sha256:"));
    }
}
