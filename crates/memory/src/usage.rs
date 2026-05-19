//! Workspace-local usage event log for memory / knowledge / workflow records.
//!
//! The log is append-only JSONL under the workspace's `.insomnia/` tree. It is
//! intentionally evidence-only: aggregation reports explicit context reads and
//! resident exposure cost telemetry, but it does not classify records as
//! Knowledge candidates or tidy-protected records.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::workspace::{RecordKind, WorkspaceLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventKind {
    Use,
    ResidentExposure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum UsageSource {
    MemoryRead,
    KnowledgeRef,
    WorkflowInvoke,
    ResidentInjection,
}

impl UsageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryRead => "MemoryRead",
            Self::KnowledgeRef => "KnowledgeRef",
            Self::WorkflowInvoke => "WorkflowInvoke",
            Self::ResidentInjection => "ResidentInjection",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecordSnapshot {
    pub kind: String,
    pub slug: String,
    pub file_bytes: u64,
    pub file_tokens_estimate: u64,
}

impl UsageRecordSnapshot {
    pub fn from_bytes(kind: RecordKind, slug: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            kind: kind.as_str().to_string(),
            slug: slug.into(),
            file_bytes: bytes.len() as u64,
            file_tokens_estimate: estimate_tokens(bytes.len()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub segment_id: String,
    pub event: UsageEventKind,
    pub source: UsageSource,
    pub records: Vec<UsageRecordSnapshot>,
}

impl UsageEvent {
    pub fn new(
        segment_id: impl Into<String>,
        event: UsageEventKind,
        source: UsageSource,
        records: Vec<UsageRecordSnapshot>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            occurred_at: Utc::now(),
            segment_id: segment_id.into(),
            event,
            source,
            records,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    pub records: Vec<UsageReportRecord>,
}

impl UsageReport {
    pub fn empty() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReportRecord {
    pub kind: String,
    pub slug: String,
    pub use_count: u64,
    pub last_used_at: Option<DateTime<Utc>>,
    pub source_breakdown: BTreeMap<String, u64>,
    pub resident_exposure_count: u64,
    pub estimated_tokens_per_injection: u64,
    pub estimated_total_resident_exposure_tokens: u64,
}

#[derive(Debug, Default)]
struct Accumulator {
    kind: String,
    slug: String,
    use_count: u64,
    last_used_at: Option<DateTime<Utc>>,
    source_breakdown: BTreeMap<String, u64>,
    resident_exposure_count: u64,
    last_resident_tokens: u64,
}

/// Append one usage event to the workspace-local JSONL log.
pub fn append_usage_event(layout: &WorkspaceLayout, event: &UsageEvent) -> io::Result<()> {
    let path = layout.usage_events_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(event)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Convenience for a successful explicit record read.
pub fn append_use_event(
    layout: &WorkspaceLayout,
    segment_id: impl Into<String>,
    source: UsageSource,
    records: Vec<UsageRecordSnapshot>,
) -> io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    append_usage_event(
        layout,
        &UsageEvent::new(segment_id, UsageEventKind::Use, source, records),
    )
}

/// Convenience for resident model-invocation exposure cost telemetry.
pub fn append_resident_exposure_event(
    layout: &WorkspaceLayout,
    segment_id: impl Into<String>,
    records: Vec<UsageRecordSnapshot>,
) -> io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    append_usage_event(
        layout,
        &UsageEvent::new(
            segment_id,
            UsageEventKind::ResidentExposure,
            UsageSource::ResidentInjection,
            records,
        ),
    )
}

/// Read a record from the workspace and build the snapshot stored in usage
/// events. `slug` should be `"summary"` for [`RecordKind::Summary`].
pub fn snapshot_record_from_layout(
    layout: &WorkspaceLayout,
    kind: RecordKind,
    slug: &str,
) -> io::Result<UsageRecordSnapshot> {
    let path = record_path(layout, kind, slug)?;
    let bytes = fs::read(path)?;
    Ok(UsageRecordSnapshot::from_bytes(
        kind,
        slug.to_string(),
        &bytes,
    ))
}

pub fn snapshot_record_from_bytes(
    kind: RecordKind,
    slug: impl Into<String>,
    bytes: &[u8],
) -> UsageRecordSnapshot {
    UsageRecordSnapshot::from_bytes(kind, slug, bytes)
}

fn record_path(
    layout: &WorkspaceLayout,
    kind: RecordKind,
    slug: &str,
) -> io::Result<std::path::PathBuf> {
    match kind {
        RecordKind::Summary => Ok(layout.summary_path()),
        RecordKind::Decision => {
            let slug = crate::Slug::parse(slug.to_string()).map_err(invalid_slug_error)?;
            Ok(layout.decision_path(&slug))
        }
        RecordKind::Request => {
            let slug = crate::Slug::parse(slug.to_string()).map_err(invalid_slug_error)?;
            Ok(layout.request_path(&slug))
        }
        RecordKind::Workflow => {
            let slug = crate::Slug::parse(slug.to_string()).map_err(invalid_slug_error)?;
            Ok(layout.workflow_path(&slug))
        }
        RecordKind::Knowledge => {
            let slug = crate::Slug::parse(slug.to_string()).map_err(invalid_slug_error)?;
            Ok(layout.knowledge_path(&slug))
        }
    }
}

fn invalid_slug_error(err: lint_common::RecordLintError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, err)
}

/// Aggregate the append-only usage log into per-record evidence.
pub fn build_usage_report(layout: &WorkspaceLayout) -> io::Result<UsageReport> {
    let path = layout.usage_events_path();
    if !path.exists() {
        return Ok(UsageReport::empty());
    }

    let file = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut acc: HashMap<(String, String), Accumulator> = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: UsageEvent = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!(error = %err, "Skipping malformed memory usage event log line");
                continue;
            }
        };
        for record in event.records {
            let key = (record.kind.clone(), record.slug.clone());
            let entry = acc.entry(key).or_insert_with(|| Accumulator {
                kind: record.kind.clone(),
                slug: record.slug.clone(),
                ..Accumulator::default()
            });
            match event.event {
                UsageEventKind::Use => {
                    entry.use_count += 1;
                    let source = event.source.as_str().to_string();
                    *entry.source_breakdown.entry(source).or_insert(0) += 1;
                    entry.last_used_at = Some(
                        entry
                            .last_used_at
                            .map(|prev| prev.max(event.occurred_at))
                            .unwrap_or(event.occurred_at),
                    );
                }
                UsageEventKind::ResidentExposure => {
                    entry.resident_exposure_count += 1;
                    entry.last_resident_tokens = record.file_tokens_estimate;
                }
            }
        }
    }

    let mut records: Vec<UsageReportRecord> = acc
        .into_values()
        .map(|a| UsageReportRecord {
            kind: a.kind,
            slug: a.slug,
            use_count: a.use_count,
            last_used_at: a.last_used_at,
            source_breakdown: a.source_breakdown,
            resident_exposure_count: a.resident_exposure_count,
            estimated_tokens_per_injection: a.last_resident_tokens,
            estimated_total_resident_exposure_tokens: a
                .last_resident_tokens
                .saturating_mul(a.resident_exposure_count),
        })
        .collect();
    records.sort_by(|a, b| (&a.kind, &a.slug).cmp(&(&b.kind, &b.slug)));
    Ok(UsageReport { records })
}

fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
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
    fn aggregates_use_and_resident_exposure_separately() {
        let (_dir, layout) = setup();
        let decision = snapshot_record_from_bytes(RecordKind::Decision, "alpha", b"abcd");
        let knowledge = snapshot_record_from_bytes(RecordKind::Knowledge, "policy", b"abcdefgh");

        append_use_event(
            &layout,
            "session-a",
            UsageSource::MemoryRead,
            vec![decision.clone()],
        )
        .unwrap();
        append_use_event(
            &layout,
            "session-a",
            UsageSource::KnowledgeRef,
            vec![knowledge.clone()],
        )
        .unwrap();
        append_use_event(
            &layout,
            "session-b",
            UsageSource::KnowledgeRef,
            vec![knowledge.clone()],
        )
        .unwrap();
        append_resident_exposure_event(&layout, "session-b", vec![knowledge]).unwrap();

        let report = build_usage_report(&layout).unwrap();
        let decision = report
            .records
            .iter()
            .find(|r| r.kind == "decision" && r.slug == "alpha")
            .unwrap();
        assert_eq!(decision.use_count, 1);
        assert_eq!(decision.source_breakdown["MemoryRead"], 1);
        assert_eq!(decision.resident_exposure_count, 0);
        assert!(decision.last_used_at.is_some());

        let knowledge = report
            .records
            .iter()
            .find(|r| r.kind == "knowledge" && r.slug == "policy")
            .unwrap();
        assert_eq!(knowledge.use_count, 2);
        assert_eq!(knowledge.source_breakdown["KnowledgeRef"], 2);
        assert_eq!(knowledge.resident_exposure_count, 1);
        assert_eq!(knowledge.estimated_tokens_per_injection, 2);
        assert_eq!(knowledge.estimated_total_resident_exposure_tokens, 2);
    }

    #[test]
    fn resident_only_record_does_not_increment_use_count() {
        let (_dir, layout) = setup();
        let snapshot = snapshot_record_from_bytes(RecordKind::Knowledge, "policy", b"abcdefgh");
        append_resident_exposure_event(&layout, "session", vec![snapshot]).unwrap();

        let report = build_usage_report(&layout).unwrap();
        let record = &report.records[0];
        assert_eq!(record.use_count, 0);
        assert!(record.last_used_at.is_none());
        assert!(record.source_breakdown.is_empty());
        assert_eq!(record.resident_exposure_count, 1);
    }
}
