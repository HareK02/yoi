// Generated from yoi-workspace-server. Do not edit by hand.
// Regenerate: cargo run -q -p yoi-workspace-server --features typescript --example generate_ticket_api_types > web/workspace/src/lib/generated/ticket-api.ts

export type InvalidProjectRecord = { label: string; reason: string };

export type TicketSummary = {
  id: string;
  title: string;
  state: string;
  priority: string;
  updated_at: string | null;
  queued_by: string | null;
  queued_at: string | null;
  workspace_action_priority: string;
  record_source: string;
};

export type TicketListResponse = {
  workspace_id: string;
  limit: number;
  items: Array<TicketSummary>;
  invalid_records: Array<InvalidProjectRecord>;
  record_authority: string;
};

export type QueryPage = {
  limit: number;
  returned: number;
  has_more: boolean;
  next_cursor: string | null;
  sort: string;
  source_limit: number | null;
  source_truncated: boolean;
};

export type TicketEventDetail = {
  sequence: number;
  event_ref: string;
  kind: string;
  author: string | null;
  at: string | null;
  status: string | null;
  from: string | null;
  to: string | null;
  reason: string | null;
  state_field: string | null;
  heading: string | null;
  body: string | null;
  attributes: { [key in string]: string };
  references: Array<string>;
};

export type ObjectiveLinkSummary = { id: string; title: string; state: string };

export type TicketEvidenceEvent = {
  event_ref: string;
  sequence: number;
  kind: string;
  at: string | null;
  author: string | null;
  excerpt: string;
};

export type TicketAssignmentSummary = {
  assignment_id: string;
  runtime_id: string;
  worker_id: string;
};

export type TicketMergeRequestSummary = {
  merge_request_id: string;
  state: string;
  review_status: string;
  revision_id: string;
  base_commit: string;
  head_commit: string;
  changed_paths: Array<string>;
  updated_at: string;
  review_submitted_at: string | null;
  review_excerpt: string | null;
};

export type TicketEvidenceSummary = {
  has_implementation_report: boolean;
  implementation_report_after_rescope: boolean;
  has_merge_request: boolean;
  has_commit: boolean;
  review_status: string | null;
  approved: boolean;
  unresolved_request_changes: boolean;
  complete_for_integration: boolean;
  missing: Array<string>;
};

export type TicketQueryRequest = {
  query: string | null;
  states: Array<string>;
  event_kinds: Array<string>;
  evidence: Array<string>;
  review_status: string | null;
  attention: Array<string>;
  related_ticket_id: string | null;
  relation_kind: string | null;
  linked_objective_id: string | null;
  updated_after: string | null;
  updated_before: string | null;
  sort: string | null;
  limit: number | null;
  cursor: string | null;
};

export type TicketQueryItem = {
  id: string;
  title: string;
  state: string;
  readiness: string | null;
  priority: string;
  created_at: string | null;
  updated_at: string | null;
  item_revision: string;
  workspace_action_priority: string;
  matched_fields: Array<string>;
  snippet: string | null;
  matching_event: TicketEvidenceEvent | null;
  linked_objective_ids: Array<string>;
  relation_count: number;
  blocker_count: number;
  unresolved_blocker_count: number;
  unresolved_review_count: number;
  evidence: TicketEvidenceSummary;
  merge_request: TicketMergeRequestSummary | null;
};

export type TicketQueryResponse = {
  items: Array<TicketQueryItem>;
  page: QueryPage;
  record_authority: string;
};

export type TicketShowRequest = {
  event_limit: number | null;
  event_cursor: string | null;
};

export type TicketRelation = {
  ticket_id: string;
  kind: string;
  target: string;
  note: string | null;
  author: string;
  at: string;
};

export type DerivedTicketRelation = {
  source_ticket: string;
  inverse_kind: string;
  forward_kind: string;
  note: string | null;
  author: string;
  at: string;
};

export type TicketRelationBlocker = {
  blocking_ticket: string;
  reason_kind: string;
  relation_kind: string;
  note: string | null;
  blocking_state: string;
};

export type TicketRelationNotice = {
  related_ticket: string;
  kind: string;
  message: string;
};

export type TicketRelationView = {
  outgoing: Array<TicketRelation>;
  incoming: Array<DerivedTicketRelation>;
  blockers: Array<TicketRelationBlocker>;
  notices: Array<TicketRelationNotice>;
};

export type TicketDetail = {
  id: string;
  title: string;
  state: string;
  readiness: string | null;
  priority: string;
  created_at: string | null;
  updated_at: string | null;
  item_revision: string;
  queued_by: string | null;
  queued_at: string | null;
  assignee: string | null;
  repository_id: string | null;
  ref_selector: string | null;
  risk_flags: Array<string>;
  body: string;
  body_truncated: boolean;
  event_count: number;
  events: Array<TicketEventDetail>;
  event_page: QueryPage;
  artifact_count: number;
  artifacts: Array<string>;
  relations: TicketRelationView;
  linked_objectives: Array<ObjectiveLinkSummary>;
  implementation_reports: Array<TicketEvidenceEvent>;
  current_assignment: TicketAssignmentSummary | null;
  merge_request: TicketMergeRequestSummary | null;
  evidence: TicketEvidenceSummary;
  resolution: string | null;
  record_source: string;
};
