//! Lightweight Tickets browser contract, separate from authoritative evidence queries.
use crate::{BrowserTicketState, QueryPage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketBrowseQueryRequest {
    pub query: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(type = "Array<BrowserTicketState>"))]
    pub states: Vec<ticket::TicketState>,
    /// Every supplied active tag ID must match (AND).
    #[serde(default)]
    pub tag_ids: Vec<String>,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "TicketAssignmentPrincipalSnapshot | null")
    )]
    pub assigned_principal: Option<ticket::TicketAssignmentPrincipalSnapshot>,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "TicketAssignmentTrackingStatus | null")
    )]
    pub handling_status: Option<ticket::TicketAssignmentTrackingStatus>,
    /// Exact current closed-case reason; historical reasons do not match this filter.
    #[cfg_attr(feature = "typescript", ts(type = "TicketClosureReason | null"))]
    pub closure_reason: Option<ticket::TicketClosureReason>,
    /// Only updated_desc is supported; defaults to updated_desc.
    pub sort: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
    /// Browse-only cursor bound to the Workspace, canonical filters, and sort.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketBrowseQueryItem {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: BrowserTicketState,
    pub updated_at: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "Array<WorkspaceTag>"))]
    pub tags: Vec<ticket::WorkspaceTag>,
    #[cfg_attr(feature = "typescript", ts(type = "TicketClosureJudgment | null"))]
    pub current_closure: Option<ticket::TicketClosureJudgment>,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "Array<TicketAssignmentTrackingProjection>")
    )]
    pub assignment_tracking: Vec<ticket::TicketAssignmentTrackingProjection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketBrowseQueryResponse {
    pub items: Vec<TicketBrowseQueryItem>,
    pub page: QueryPage,
    pub record_authority: String,
}
