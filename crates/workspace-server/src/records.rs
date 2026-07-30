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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct InvalidProjectRecord {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketListResponse {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<TicketSummary>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub assignee: Option<String>,
    pub repository_id: Option<String>,
    pub ref_selector: Option<String>,
    pub risk_flags: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    pub event_count: usize,
    pub events: Vec<TicketEventDetail>,
    pub artifact_count: usize,
    pub artifacts: Vec<String>,
    pub relations: TicketRelationView,
    pub resolution: Option<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRelation {
    pub ticket_id: String,
    pub kind: String,
    pub target: String,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct DerivedTicketRelation {
    pub source_ticket: String,
    pub inverse_kind: String,
    pub forward_kind: String,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRelationBlocker {
    pub blocking_ticket: String,
    pub reason_kind: String,
    pub relation_kind: String,
    pub note: Option<String>,
    pub blocking_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRelationNotice {
    pub related_ticket: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRelationView {
    pub outgoing: Vec<TicketRelation>,
    pub incoming: Vec<DerivedTicketRelation>,
    pub blockers: Vec<TicketRelationBlocker>,
    pub notices: Vec<TicketRelationNotice>,
}

impl From<ticket::TicketRelationView> for TicketRelationView {
    fn from(value: ticket::TicketRelationView) -> Self {
        Self {
            outgoing: value
                .outgoing
                .into_iter()
                .map(|relation| TicketRelation {
                    ticket_id: relation.ticket_id,
                    kind: relation.kind.as_str().to_string(),
                    target: relation.target,
                    note: relation.note,
                    author: relation.author,
                    at: relation.at,
                })
                .collect(),
            incoming: value
                .incoming
                .into_iter()
                .map(|relation| DerivedTicketRelation {
                    source_ticket: relation.source_ticket,
                    inverse_kind: relation.inverse_kind,
                    forward_kind: relation.forward_kind.as_str().to_string(),
                    note: relation.note,
                    author: relation.author,
                    at: relation.at,
                })
                .collect(),
            blockers: value
                .blockers
                .into_iter()
                .map(|blocker| TicketRelationBlocker {
                    blocking_ticket: blocker.blocking_ticket,
                    reason_kind: blocker.reason_kind,
                    relation_kind: blocker.relation_kind.as_str().to_string(),
                    note: blocker.note,
                    blocking_state: blocker.blocking_state.as_str().to_string(),
                })
                .collect(),
            notices: value
                .notices
                .into_iter()
                .map(|notice| TicketRelationNotice {
                    related_ticket: notice.related_ticket,
                    kind: notice.kind.as_str().to_string(),
                    message: notice.message,
                })
                .collect(),
        }
    }
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

#[cfg(feature = "typescript")]
pub fn ticket_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        InvalidProjectRecord::decl(&config),
        TicketSummary::decl(&config),
        TicketListResponse::decl(&config),
        TicketEventDetail::decl(&config),
        TicketRelation::decl(&config),
        DerivedTicketRelation::decl(&config),
        TicketRelationBlocker::decl(&config),
        TicketRelationNotice::decl(&config),
        TicketRelationView::decl(&config),
        TicketDetail::decl(&config),
    ];
    format!(
        "// Generated from yoi-workspace-server. Do not edit by hand.\n// Regenerate: cargo run -q -p yoi-workspace-server --features typescript --example generate_ticket_api_types > web/workspace/src/lib/generated/ticket-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn generated_ticket_api_contract_is_current() {
        let expected = super::ticket_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/ticket-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Ticket API TypeScript types with `cargo run -q -p yoi-workspace-server --features typescript --example generate_ticket_api_types > web/workspace/src/lib/generated/ticket-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace(";}", "}")
    }
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
