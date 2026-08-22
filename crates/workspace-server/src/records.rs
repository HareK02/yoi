use project_record::validate_record_id;
use serde::{Deserialize, Serialize};
pub use workspace_api::{
    ObjectiveDetail, ObjectiveEventDetail, ObjectiveLinkedTicketSummary, ObjectiveResourceSummary,
    ObjectiveSummary, QueryPage,
};

use crate::{Error, Result};

const SUMMARY_BODY_LIMIT: usize = 240;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRecordList<T> {
    pub items: Vec<T>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketSummaryPage {
    pub items: Vec<TicketSummary>,
    pub page: QueryPage,
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
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub workspace_action_priority: String,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TicketListPageRequest {
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketListResponse {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<TicketSummary>,
    pub page: QueryPage,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketDetail {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub readiness: Option<String>,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub item_revision: String,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub repository_id: Option<String>,
    pub ref_selector: Option<String>,
    pub risk_flags: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    pub event_count: usize,
    pub events: Vec<TicketEventDetail>,
    pub event_page: QueryPage,
    pub artifact_count: usize,
    pub artifacts: Vec<String>,
    pub relations: TicketRelationView,
    pub linked_objectives: Vec<ObjectiveLinkSummary>,
    pub implementation_reports: Vec<TicketEvidenceEvent>,
    pub assignments: Vec<TicketRoleAssignmentSummary>,
    pub current_coder: Option<TicketAssignmentSummary>,
    pub assignment_diagnostics: Vec<String>,
    pub action_eligibility: TicketActionEligibility,
    pub merge_request: Option<TicketMergeRequestSummary>,
    pub evidence: TicketEvidenceSummary,
    pub resolution: Option<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketEventDetail {
    pub sequence: usize,
    pub event_ref: String,
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
    pub attributes: std::collections::BTreeMap<String, String>,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRelation {
    pub ticket_id: String,
    pub kind: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_resource_key: Option<String>,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct DerivedTicketRelation {
    pub source_ticket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_resource_key: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_resource_key: Option<String>,
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
                    target_resource_key: None,
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
                    source_resource_key: None,
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
                    blocking_resource_key: None,
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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ObjectiveLinkSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketEvidenceEvent {
    pub event_ref: String,
    pub sequence: usize,
    pub kind: String,
    pub at: Option<String>,
    pub author: Option<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketAssignmentSummary {
    pub assignment_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketRoleAssignmentSummary {
    pub assignment_id: String,
    pub role: String,
    pub principal: TicketAssignmentPrincipalSummary,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(tag = "kind", rename_all = "snake_case"))]
pub enum TicketAssignmentPrincipalSummary {
    User {
        account_id: String,
    },
    Worker {
        runtime_id: String,
        worker_id: String,
    },
    WorkspaceAgent {
        agent_key: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketActionEligibility {
    pub can_assign_orchestrator: bool,
    pub can_unassign_orchestrator: bool,
    pub can_queue: bool,
    pub can_start_manual_coder: bool,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketMergeRequestSummary {
    pub merge_request_id: String,
    pub repository_id: String,
    pub state: String,
    pub review_status: String,
    pub selector_from: Option<String>,
    pub selector_to: String,
    pub updated_at: String,
    pub current_subject_ref: Option<String>,
    pub review_subject_ref: Option<String>,
    pub review_requested_at: Option<String>,
    pub review_submitted_at: Option<String>,
    pub review_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct MergeRequestListItem {
    pub summary: TicketMergeRequestSummary,
    pub ticket_ids: Vec<String>,
    pub thread_event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct MergeRequestListResponse {
    pub items: Vec<MergeRequestListItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketEvidenceSummary {
    pub has_merge_request: bool,
    pub has_current_subject_ref: bool,
    pub has_review_request: bool,
    pub has_commit: bool,
    pub review_status: Option<String>,
    pub approved_current_subject: bool,
    pub review_after_rescope: bool,
    pub unresolved_request_changes: bool,
    pub complete_for_integration: bool,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketQueryRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub event_kinds: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    pub review_status: Option<String>,
    #[serde(default)]
    pub attention: Vec<String>,
    pub related_ticket_id: Option<String>,
    pub relation_kind: Option<String>,
    pub linked_objective_id: Option<String>,
    pub updated_after: Option<String>,
    pub updated_before: Option<String>,
    pub sort: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketQueryItem {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub readiness: Option<String>,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub item_revision: String,
    pub workspace_action_priority: String,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
    pub matching_event: Option<TicketEvidenceEvent>,
    pub linked_objective_ids: Vec<String>,
    pub relation_count: usize,
    pub blocker_count: usize,
    pub unresolved_blocker_count: usize,
    pub unresolved_review_count: usize,
    pub evidence: TicketEvidenceSummary,
    pub merge_request: Option<TicketMergeRequestSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketQueryResponse {
    pub items: Vec<TicketQueryItem>,
    pub page: QueryPage,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct TicketShowRequest {
    pub event_limit: Option<usize>,
    pub event_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ObjectiveQueryRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    pub linked_ticket_id: Option<String>,
    pub updated_after: Option<String>,
    pub updated_before: Option<String>,
    pub sort: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveQueryItem {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
    pub linked_ticket_count: usize,
    pub linked_tickets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveQueryResponse {
    pub items: Vec<ObjectiveQueryItem>,
    pub page: QueryPage,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ObjectiveShowRequest {
    pub event_limit: Option<usize>,
    pub event_cursor: Option<String>,
}

#[cfg(feature = "typescript")]
pub fn ticket_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        InvalidProjectRecord::decl(&config),
        TicketSummary::decl(&config),
        TicketListResponse::decl(&config),
        QueryPage::decl(&config),
        TicketEventDetail::decl(&config),
        ObjectiveLinkSummary::decl(&config),
        TicketEvidenceEvent::decl(&config),
        TicketAssignmentSummary::decl(&config),
        TicketRoleAssignmentSummary::decl(&config),
        TicketAssignmentPrincipalSummary::decl(&config),
        TicketActionEligibility::decl(&config),
        TicketMergeRequestSummary::decl(&config),
        MergeRequestListItem::decl(&config),
        MergeRequestListResponse::decl(&config),
        TicketEvidenceSummary::decl(&config),
        TicketQueryRequest::decl(&config),
        TicketQueryItem::decl(&config),
        TicketQueryResponse::decl(&config),
        TicketShowRequest::decl(&config),
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
