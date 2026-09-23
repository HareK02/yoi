pub use server_api::{
    InvalidProjectRecord, MergeRequestListItem, MergeRequestListResponse,
    MergeRequestRefDiagnostic, ObjectiveDetail, ObjectiveEventDetail, ObjectiveLinkSummary,
    ObjectiveLinkedTicketSummary, ObjectiveQueryItem, ObjectiveQueryRequest,
    ObjectiveQueryResponse, ObjectiveResourceSummary, ObjectiveShowRequest, ObjectiveSummary,
    QueryPage, TicketActionEligibility,
    TicketAssignmentPrincipal as TicketAssignmentPrincipalSummary, TicketAssignmentSummary,
    TicketDetail, TicketDetailDerivedRelation as DerivedTicketRelation,
    TicketDetailRelation as TicketRelation, TicketDetailRelationBlocker as TicketRelationBlocker,
    TicketDetailRelationNotice as TicketRelationNotice,
    TicketDetailRelationView as TicketRelationView, TicketEventDetail, TicketEvidenceEvent,
    TicketEvidenceSummary, TicketListItemSummary as TicketSummary, TicketListResponse,
    TicketMergeRequestSummary, TicketQueryItem, TicketQueryRequest, TicketQueryResponse,
    TicketRoleAssignmentSummary, TicketShowRequest,
};

const SUMMARY_BODY_LIMIT: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecordList<T> {
    pub items: Vec<T>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketSummaryPage {
    pub items: Vec<TicketSummary>,
    pub page: QueryPage,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketListProjectionRequest {
    pub states: Vec<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

pub(crate) fn ticket_relation_view_from_domain(
    value: ticket::TicketRelationView,
) -> TicketRelationView {
    TicketRelationView {
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
