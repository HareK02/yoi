use project_record::validate_record_id;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const SUMMARY_BODY_LIMIT: usize = 240;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRecordList<T> {
    pub items: Vec<T>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvalidProjectRecord {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketSummary {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub workspace_action_priority: String,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub repository_id: Option<String>,
    pub ref_selector: Option<String>,
    pub risk_flags: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    pub event_count: usize,
    pub events: Vec<TicketEventDetail>,
    pub artifact_count: usize,
    pub artifacts: Vec<String>,
    pub resolution: Option<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketEventDetail {
    pub sequence: usize,
    pub kind: String,
    pub author: Option<String>,
    pub at: Option<String>,
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub reason: Option<String>,
    pub state_field: Option<String>,
    pub heading: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveSummary {
    pub id: String,
    pub title: String,
    pub state: String,
    pub updated_at: Option<String>,
    pub summary: String,
    pub linked_tickets: Vec<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub linked_tickets: Vec<String>,
    pub resources: Vec<ObjectiveResourceSummary>,
    pub body: String,
    pub body_truncated: bool,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveResourceSummary {
    pub path: String,
    pub media_type: Option<String>,
    pub bytes: usize,
    pub updated_at: String,
}

pub(crate) fn validate_project_id(id: &str) -> Result<()> {
    validate_record_id(id).map_err(|_| Error::InvalidRecordId(id.to_string()))
}

pub(crate) fn summarize_body(body: &str) -> String {
    let summary = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default();
    let (summary, truncated) = truncate_body(summary, SUMMARY_BODY_LIMIT);
    if truncated {
        format!("{summary}…")
    } else {
        summary
    }
}

pub(crate) fn truncate_body(body: &str, limit: usize) -> (String, bool) {
    if body.len() <= limit {
        return (body.to_string(), false);
    }
    let mut end = limit;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    (body[..end].to_string(), true)
}
