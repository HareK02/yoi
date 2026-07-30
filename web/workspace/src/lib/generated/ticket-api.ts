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

export type TicketEventDetail = {
  sequence: number;
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
  priority: string;
  created_at: string | null;
  updated_at: string | null;
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
  artifact_count: number;
  artifacts: Array<string>;
  relations: TicketRelationView;
  resolution: string | null;
  record_source: string;
};
