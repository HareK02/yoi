import type {
  InvalidProjectRecord,
  MergeRequestDetailResponse,
  MergeRequestLinkedTicketResponse,
  MergeRequestListItem,
  MergeRequestListResponse,
  MergeRequestRefDiagnostic,
  MergeRequestRefResponse,
  MergeRequestThreadEvent,
  MergeRequestWorkerIdentity,
  ObjectiveDetail,
  ObjectiveEventDetail,
  ObjectiveLinkedTicketSummary,
  ObjectiveListResponse,
  ObjectiveResourceSummary,
  ObjectiveSummary,
  QueryPage,
  ReviewFinding,
  TicketActionEligibility,
  TicketAssignmentPrincipal,
  TicketAssignmentSummary,
  TicketDetail,
  TicketDetailDerivedRelation,
  TicketDetailRelation,
  TicketDetailRelationBlocker,
  TicketDetailRelationNotice,
  TicketDetailRelationView,
  TicketEventDetail,
  TicketEvidenceEvent,
  TicketEvidenceSummary,
  TicketListItemSummary,
  TicketListResponse,
  TicketMergeRequestSummary,
  TicketQueueOutcome,
  TicketRef,
  TicketRoleAssignmentMutationResponse,
  TicketRoleAssignmentRecord,
  TicketRoleAssignmentSummary,
  TicketTarget,
} from "$lib/generated/ticket-api.ts";

export const TICKET_BROWSER_API_MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
export const TICKET_BROWSER_API_LOAD_POLICY = {
  diagnosticLabel: "Ticket Browser API",
  maxResponseBytes: TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
} as const;

const MAX_STRING_LENGTH = 262_144;
const MAX_COLLECTION_LENGTH = 4_096;
const MAX_MAP_ENTRIES = 4_096;

function object(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function exact(
  value: Record<string, unknown>,
  allowed: readonly string[],
  label: string,
): void {
  const unexpected = Object.keys(value).find((key) => !allowed.includes(key));
  if (unexpected !== undefined) {
    throw new Error(`${label} contains unknown field ${unexpected}`);
  }
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string") throw new Error(`${label} must be a string`);
  if (value.length > MAX_STRING_LENGTH) {
    throw new Error(`${label} exceeds its length limit`);
  }
  return value;
}

function boolean(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${label} must be a boolean`);
  return value;
}

function integer(
  value: unknown,
  label: string,
  minimum = 0,
  maximum = Number.MAX_SAFE_INTEGER,
): number {
  if (
    typeof value !== "number" || !Number.isSafeInteger(value) ||
    value < minimum || value > maximum
  ) {
    throw new Error(
      `${label} must be an integer from ${minimum} through ${maximum}`,
    );
  }
  return value;
}

function nullableString(value: unknown, label: string): string | null {
  return value === null ? null : string(value, label);
}

function optionalNullableString(
  value: unknown,
  label: string,
): string | null | undefined {
  return value === undefined ? undefined : nullableString(value, label);
}

function array<T>(
  value: unknown,
  label: string,
  parse: (item: unknown, label: string) => T,
): T[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  if (value.length > MAX_COLLECTION_LENGTH) {
    throw new Error(`${label} exceeds its item limit`);
  }
  return value.map((item, index) => parse(item, `${label}[${index}]`));
}

function strings(value: unknown, label: string): string[] {
  return array(value, label, string);
}

function stringMap(value: unknown, label: string): Record<string, string> {
  const source = object(value, label);
  const entries = Object.entries(source);
  if (entries.length > MAX_MAP_ENTRIES) {
    throw new Error(`${label} exceeds its entry limit`);
  }
  return Object.fromEntries(
    entries.map(([key, item]) => [
      string(key, `${label} key`),
      string(item, `${label}.${key}`),
    ]),
  );
}

function parseQueryPage(value: unknown, label: string): QueryPage {
  const item = object(value, label);
  exact(item, [
    "has_more",
    "limit",
    "next_cursor",
    "returned",
    "sort",
    "source_limit",
    "source_truncated",
  ], label);
  return {
    has_more: boolean(item.has_more, `${label}.has_more`),
    limit: integer(item.limit, `${label}.limit`),
    next_cursor: optionalNullableString(
      item.next_cursor,
      `${label}.next_cursor`,
    ),
    returned: integer(item.returned, `${label}.returned`),
    sort: string(item.sort, `${label}.sort`),
    source_limit: item.source_limit === undefined
      ? undefined
      : item.source_limit === null
      ? null
      : integer(item.source_limit, `${label}.source_limit`),
    source_truncated: boolean(
      item.source_truncated,
      `${label}.source_truncated`,
    ),
  };
}

function parseInvalidProjectRecord(
  value: unknown,
  label: string,
): InvalidProjectRecord {
  const item = object(value, label);
  exact(item, ["label", "reason"], label);
  return {
    label: string(item.label, `${label}.label`),
    reason: string(item.reason, `${label}.reason`),
  };
}

function parseTicketListItem(
  value: unknown,
  label: string,
): TicketListItemSummary {
  const item = object(value, label);
  exact(item, [
    "id",
    "priority",
    "queued_at",
    "queued_by",
    "record_source",
    "resource_key",
    "state",
    "title",
    "updated_at",
    "workspace_action_priority",
  ], label);
  return {
    id: string(item.id, `${label}.id`),
    priority: string(item.priority, `${label}.priority`),
    queued_at: optionalNullableString(item.queued_at, `${label}.queued_at`),
    queued_by: optionalNullableString(item.queued_by, `${label}.queued_by`),
    record_source: string(item.record_source, `${label}.record_source`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    state: string(item.state, `${label}.state`),
    title: string(item.title, `${label}.title`),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
    workspace_action_priority: string(
      item.workspace_action_priority,
      `${label}.workspace_action_priority`,
    ),
  };
}

export function parseTicketListResponse(value: unknown): TicketListResponse {
  const item = object(value, "Ticket list response");
  exact(item, [
    "invalid_records",
    "items",
    "limit",
    "page",
    "record_authority",
    "workspace_id",
  ], "Ticket list response");
  const limit = integer(item.limit, "Ticket list response.limit", 0, 1000);
  return {
    invalid_records: array(
      item.invalid_records,
      "Ticket list response.invalid_records",
      parseInvalidProjectRecord,
    ),
    items: array(
      item.items,
      "Ticket list response.items",
      parseTicketListItem,
    ),
    limit: limit as TicketListResponse["limit"],
    page: parseQueryPage(item.page, "Ticket list response.page"),
    record_authority: string(
      item.record_authority,
      "Ticket list response.record_authority",
    ),
    workspace_id: string(
      item.workspace_id,
      "Ticket list response.workspace_id",
    ),
  };
}

function parseTicketEvent(value: unknown, label: string): TicketEventDetail {
  const item = object(value, label);
  exact(item, [
    "at",
    "attributes",
    "author",
    "body",
    "event_ref",
    "from",
    "heading",
    "kind",
    "reason",
    "references",
    "sequence",
    "state_field",
    "status",
    "to",
  ], label);
  return {
    at: optionalNullableString(item.at, `${label}.at`),
    attributes: stringMap(item.attributes, `${label}.attributes`),
    author: optionalNullableString(item.author, `${label}.author`),
    body: optionalNullableString(item.body, `${label}.body`),
    event_ref: string(item.event_ref, `${label}.event_ref`),
    from: optionalNullableString(item.from, `${label}.from`),
    heading: optionalNullableString(item.heading, `${label}.heading`),
    kind: string(item.kind, `${label}.kind`),
    reason: optionalNullableString(item.reason, `${label}.reason`),
    references: strings(item.references, `${label}.references`),
    sequence: integer(item.sequence, `${label}.sequence`),
    state_field: optionalNullableString(
      item.state_field,
      `${label}.state_field`,
    ),
    status: optionalNullableString(item.status, `${label}.status`),
    to: optionalNullableString(item.to, `${label}.to`),
  };
}

function parseTicketEvidenceEvent(
  value: unknown,
  label: string,
): TicketEvidenceEvent {
  const item = object(value, label);
  exact(
    item,
    ["at", "author", "event_ref", "excerpt", "kind", "sequence"],
    label,
  );
  return {
    at: optionalNullableString(item.at, `${label}.at`),
    author: optionalNullableString(item.author, `${label}.author`),
    event_ref: string(item.event_ref, `${label}.event_ref`),
    excerpt: string(item.excerpt, `${label}.excerpt`),
    kind: string(item.kind, `${label}.kind`),
    sequence: integer(item.sequence, `${label}.sequence`),
  };
}

function parseTicketAssignmentPrincipal(
  value: unknown,
  label: string,
): TicketAssignmentPrincipal {
  const item = object(value, label);
  const kind = string(item.kind, `${label}.kind`);
  if (kind === "user") {
    exact(item, ["account_id", "kind"], label);
    return { kind, account_id: string(item.account_id, `${label}.account_id`) };
  }
  if (kind === "worker") {
    exact(item, ["kind", "runtime_id", "worker_id"], label);
    return {
      kind,
      runtime_id: string(item.runtime_id, `${label}.runtime_id`),
      worker_id: string(item.worker_id, `${label}.worker_id`),
    };
  }
  if (kind === "workspace_agent") {
    exact(item, ["agent_key", "kind"], label);
    return { kind, agent_key: string(item.agent_key, `${label}.agent_key`) };
  }
  throw new Error(`${label}.kind is invalid`);
}

function parseTicketAssignmentSummary(
  value: unknown,
  label: string,
): TicketAssignmentSummary {
  const item = object(value, label);
  exact(item, [
    "assignment_id",
    "runtime_id",
    "worker_id",
    "worker_resource_key",
  ], label);
  return {
    assignment_id: string(item.assignment_id, `${label}.assignment_id`),
    runtime_id: string(item.runtime_id, `${label}.runtime_id`),
    worker_id: string(item.worker_id, `${label}.worker_id`),
    worker_resource_key: optionalNullableString(
      item.worker_resource_key,
      `${label}.worker_resource_key`,
    ),
  };
}

function parseTicketRoleAssignmentSummary(
  value: unknown,
  label: string,
): TicketRoleAssignmentSummary {
  const item = object(value, label);
  exact(item, [
    "assigned_at",
    "assigned_by",
    "assignment_id",
    "principal",
    "role",
  ], label);
  return {
    assigned_at: string(item.assigned_at, `${label}.assigned_at`),
    assigned_by: string(item.assigned_by, `${label}.assigned_by`),
    assignment_id: string(item.assignment_id, `${label}.assignment_id`),
    principal: parseTicketAssignmentPrincipal(
      item.principal,
      `${label}.principal`,
    ),
    role: string(item.role, `${label}.role`),
  };
}

function parseTicketRoleAssignmentRecord(
  value: unknown,
  label: string,
): TicketRoleAssignmentRecord {
  const item = object(value, label);
  exact(item, [
    "assigned_at",
    "assigned_by",
    "assignment_id",
    "principal",
    "role",
    "ticket_id",
    "workspace_id",
  ], label);
  const role = string(item.role, `${label}.role`);
  if (!["orchestrator", "coder", "owner", "contributor"].includes(role)) {
    throw new Error(`${label}.role is invalid`);
  }
  return {
    assigned_at: string(item.assigned_at, `${label}.assigned_at`),
    assigned_by: string(item.assigned_by, `${label}.assigned_by`),
    assignment_id: string(item.assignment_id, `${label}.assignment_id`),
    principal: parseTicketAssignmentPrincipal(
      item.principal,
      `${label}.principal`,
    ),
    role: role as TicketRoleAssignmentRecord["role"],
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
  };
}

function parseActionEligibility(
  value: unknown,
  label: string,
): TicketActionEligibility {
  const item = object(value, label);
  exact(item, [
    "blockers",
    "can_assign_orchestrator",
    "can_queue",
    "can_start_manual_coder",
    "can_unassign_orchestrator",
    "queue_tickets",
  ], label);
  return {
    blockers: strings(item.blockers, `${label}.blockers`),
    can_assign_orchestrator: boolean(
      item.can_assign_orchestrator,
      `${label}.can_assign_orchestrator`,
    ),
    can_queue: boolean(item.can_queue, `${label}.can_queue`),
    can_start_manual_coder: boolean(
      item.can_start_manual_coder,
      `${label}.can_start_manual_coder`,
    ),
    can_unassign_orchestrator: boolean(
      item.can_unassign_orchestrator,
      `${label}.can_unassign_orchestrator`,
    ),
    queue_tickets: strings(item.queue_tickets, `${label}.queue_tickets`),
  };
}

function parseTicketMergeRequestSummary(
  value: unknown,
  label: string,
): TicketMergeRequestSummary {
  const item = object(value, label);
  exact(item, [
    "current_subject_ref",
    "merge_request_id",
    "repository_key",
    "review_excerpt",
    "review_requested_at",
    "review_status",
    "review_subject_ref",
    "review_submitted_at",
    "selector_from",
    "selector_to",
    "state",
    "updated_at",
  ], label);
  return {
    current_subject_ref: optionalNullableString(
      item.current_subject_ref,
      `${label}.current_subject_ref`,
    ),
    merge_request_id: string(
      item.merge_request_id,
      `${label}.merge_request_id`,
    ),
    repository_key: string(item.repository_key, `${label}.repository_key`),
    review_excerpt: optionalNullableString(
      item.review_excerpt,
      `${label}.review_excerpt`,
    ),
    review_requested_at: optionalNullableString(
      item.review_requested_at,
      `${label}.review_requested_at`,
    ),
    review_status: string(item.review_status, `${label}.review_status`),
    review_subject_ref: optionalNullableString(
      item.review_subject_ref,
      `${label}.review_subject_ref`,
    ),
    review_submitted_at: optionalNullableString(
      item.review_submitted_at,
      `${label}.review_submitted_at`,
    ),
    selector_from: optionalNullableString(
      item.selector_from,
      `${label}.selector_from`,
    ),
    selector_to: string(item.selector_to, `${label}.selector_to`),
    state: string(item.state, `${label}.state`),
    updated_at: string(item.updated_at, `${label}.updated_at`),
  };
}

function parseTicketEvidenceSummary(
  value: unknown,
  label: string,
): TicketEvidenceSummary {
  const item = object(value, label);
  exact(item, [
    "approved_current_subject",
    "complete_for_integration",
    "has_commit",
    "has_current_subject_ref",
    "has_merge_request",
    "has_review_request",
    "missing",
    "review_after_rescope",
    "review_status",
    "unresolved_request_changes",
  ], label);
  return {
    approved_current_subject: boolean(
      item.approved_current_subject,
      `${label}.approved_current_subject`,
    ),
    complete_for_integration: boolean(
      item.complete_for_integration,
      `${label}.complete_for_integration`,
    ),
    has_commit: boolean(item.has_commit, `${label}.has_commit`),
    has_current_subject_ref: boolean(
      item.has_current_subject_ref,
      `${label}.has_current_subject_ref`,
    ),
    has_merge_request: boolean(
      item.has_merge_request,
      `${label}.has_merge_request`,
    ),
    has_review_request: boolean(
      item.has_review_request,
      `${label}.has_review_request`,
    ),
    missing: strings(item.missing, `${label}.missing`),
    review_after_rescope: boolean(
      item.review_after_rescope,
      `${label}.review_after_rescope`,
    ),
    review_status: optionalNullableString(
      item.review_status,
      `${label}.review_status`,
    ),
    unresolved_request_changes: boolean(
      item.unresolved_request_changes,
      `${label}.unresolved_request_changes`,
    ),
  };
}

function parseRelation(value: unknown, label: string): TicketDetailRelation {
  const item = object(value, label);
  exact(item, [
    "at",
    "author",
    "kind",
    "note",
    "target",
    "target_resource_key",
    "ticket_id",
  ], label);
  return {
    at: string(item.at, `${label}.at`),
    author: string(item.author, `${label}.author`),
    kind: string(item.kind, `${label}.kind`),
    note: optionalNullableString(item.note, `${label}.note`),
    target: string(item.target, `${label}.target`),
    target_resource_key: optionalNullableString(
      item.target_resource_key,
      `${label}.target_resource_key`,
    ),
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
  };
}

function parseDerivedRelation(
  value: unknown,
  label: string,
): TicketDetailDerivedRelation {
  const item = object(value, label);
  exact(item, [
    "at",
    "author",
    "forward_kind",
    "inverse_kind",
    "note",
    "source_resource_key",
    "source_ticket",
  ], label);
  return {
    at: string(item.at, `${label}.at`),
    author: string(item.author, `${label}.author`),
    forward_kind: string(item.forward_kind, `${label}.forward_kind`),
    inverse_kind: string(item.inverse_kind, `${label}.inverse_kind`),
    note: optionalNullableString(item.note, `${label}.note`),
    source_resource_key: optionalNullableString(
      item.source_resource_key,
      `${label}.source_resource_key`,
    ),
    source_ticket: string(item.source_ticket, `${label}.source_ticket`),
  };
}

function parseRelationBlocker(
  value: unknown,
  label: string,
): TicketDetailRelationBlocker {
  const item = object(value, label);
  exact(item, [
    "blocking_resource_key",
    "blocking_state",
    "blocking_ticket",
    "note",
    "reason_kind",
    "relation_kind",
  ], label);
  return {
    blocking_resource_key: optionalNullableString(
      item.blocking_resource_key,
      `${label}.blocking_resource_key`,
    ),
    blocking_state: string(item.blocking_state, `${label}.blocking_state`),
    blocking_ticket: string(item.blocking_ticket, `${label}.blocking_ticket`),
    note: optionalNullableString(item.note, `${label}.note`),
    reason_kind: string(item.reason_kind, `${label}.reason_kind`),
    relation_kind: string(item.relation_kind, `${label}.relation_kind`),
  };
}

function parseRelationNotice(
  value: unknown,
  label: string,
): TicketDetailRelationNotice {
  const item = object(value, label);
  exact(item, ["kind", "message", "related_ticket"], label);
  return {
    kind: string(item.kind, `${label}.kind`),
    message: string(item.message, `${label}.message`),
    related_ticket: string(item.related_ticket, `${label}.related_ticket`),
  };
}

function parseRelationView(
  value: unknown,
  label: string,
): TicketDetailRelationView {
  const item = object(value, label);
  exact(item, ["blockers", "incoming", "notices", "outgoing"], label);
  return {
    blockers: array(item.blockers, `${label}.blockers`, parseRelationBlocker),
    incoming: array(item.incoming, `${label}.incoming`, parseDerivedRelation),
    notices: array(item.notices, `${label}.notices`, parseRelationNotice),
    outgoing: array(item.outgoing, `${label}.outgoing`, parseRelation),
  };
}

function parseObjectiveLink(value: unknown, label: string) {
  const item = object(value, label);
  exact(item, ["id", "resource_key", "state", "title"], label);
  return {
    id: string(item.id, `${label}.id`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    state: string(item.state, `${label}.state`),
    title: string(item.title, `${label}.title`),
  };
}

function parseTicketTarget(value: unknown, label: string): TicketTarget {
  const item = object(value, label);
  exact(item, ["access", "ref_selector", "repository_key"], label);
  const access = string(item.access, `${label}.access`);
  if (access !== "read_only" && access !== "read_write") {
    throw new Error(`${label}.access is invalid`);
  }
  return {
    access,
    ref_selector: optionalNullableString(
      item.ref_selector,
      `${label}.ref_selector`,
    ),
    repository_key: string(item.repository_key, `${label}.repository_key`),
  };
}

export function parseTicketDetail(value: unknown): TicketDetail {
  const label = "Ticket detail";
  const item = object(value, label);
  exact(item, [
    "action_eligibility",
    "artifact_count",
    "artifacts",
    "assignment_diagnostics",
    "assignments",
    "body",
    "body_truncated",
    "created_at",
    "current_coder",
    "event_count",
    "event_page",
    "events",
    "evidence",
    "id",
    "implementation_reports",
    "item_revision",
    "linked_objectives",
    "merge_request",
    "priority",
    "queued_at",
    "queued_by",
    "readiness",
    "record_source",
    "relations",
    "resolution",
    "resource_key",
    "risk_flags",
    "state",
    "targets",
    "title",
    "updated_at",
  ], label);
  return {
    action_eligibility: parseActionEligibility(
      item.action_eligibility,
      `${label}.action_eligibility`,
    ),
    artifact_count: integer(item.artifact_count, `${label}.artifact_count`),
    artifacts: strings(item.artifacts, `${label}.artifacts`),
    assignment_diagnostics: strings(
      item.assignment_diagnostics,
      `${label}.assignment_diagnostics`,
    ),
    assignments: array(
      item.assignments,
      `${label}.assignments`,
      parseTicketRoleAssignmentSummary,
    ),
    body: string(item.body, `${label}.body`),
    body_truncated: boolean(item.body_truncated, `${label}.body_truncated`),
    created_at: optionalNullableString(item.created_at, `${label}.created_at`),
    current_coder: item.current_coder === undefined
      ? undefined
      : item.current_coder === null
      ? null
      : parseTicketAssignmentSummary(
        item.current_coder,
        `${label}.current_coder`,
      ),
    event_count: integer(item.event_count, `${label}.event_count`),
    event_page: parseQueryPage(item.event_page, `${label}.event_page`),
    events: array(item.events, `${label}.events`, parseTicketEvent),
    evidence: parseTicketEvidenceSummary(item.evidence, `${label}.evidence`),
    id: string(item.id, `${label}.id`),
    implementation_reports: array(
      item.implementation_reports,
      `${label}.implementation_reports`,
      parseTicketEvidenceEvent,
    ),
    item_revision: string(item.item_revision, `${label}.item_revision`),
    linked_objectives: array(
      item.linked_objectives,
      `${label}.linked_objectives`,
      parseObjectiveLink,
    ),
    merge_request: item.merge_request === undefined
      ? undefined
      : item.merge_request === null
      ? null
      : parseTicketMergeRequestSummary(
        item.merge_request,
        `${label}.merge_request`,
      ),
    priority: string(item.priority, `${label}.priority`),
    queued_at: optionalNullableString(item.queued_at, `${label}.queued_at`),
    queued_by: optionalNullableString(item.queued_by, `${label}.queued_by`),
    readiness: optionalNullableString(item.readiness, `${label}.readiness`),
    record_source: string(item.record_source, `${label}.record_source`),
    relations: parseRelationView(item.relations, `${label}.relations`),
    resolution: optionalNullableString(item.resolution, `${label}.resolution`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    risk_flags: strings(item.risk_flags, `${label}.risk_flags`),
    state: string(item.state, `${label}.state`),
    targets: array(item.targets, `${label}.targets`, parseTicketTarget),
    title: string(item.title, `${label}.title`),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
  };
}

export function parseTicketRecordRef(value: unknown): TicketRef {
  const label = "Ticket create response";
  const item = object(value, label);
  exact(item, ["id", "resource_key", "slug", "status"], label);
  const status = string(item.status, `${label}.status`);
  if (status !== "Open" && status !== "Closed") {
    throw new Error(`${label}.status is invalid`);
  }
  return {
    id: string(item.id, `${label}.id`),
    resource_key: optionalNullableString(
      item.resource_key,
      `${label}.resource_key`,
    ),
    slug: string(item.slug, `${label}.slug`),
    status,
  };
}

export function parseTicketQueueOutcome(value: unknown): TicketQueueOutcome {
  const item = object(value, "Ticket queue outcome");
  exact(item, ["queued_tickets", "requested_ticket"], "Ticket queue outcome");
  return {
    queued_tickets: strings(
      item.queued_tickets,
      "Ticket queue outcome.queued_tickets",
    ),
    requested_ticket: string(
      item.requested_ticket,
      "Ticket queue outcome.requested_ticket",
    ),
  };
}

export function parseTicketRoleAssignmentMutationResponse(
  value: unknown,
): TicketRoleAssignmentMutationResponse {
  const label = "Ticket assignment response";
  const item = object(value, label);
  exact(item, ["assignment", "ticket_id", "workspace_id"], label);
  return {
    assignment: item.assignment === undefined
      ? undefined
      : item.assignment === null
      ? null
      : parseTicketRoleAssignmentRecord(item.assignment, `${label}.assignment`),
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
  };
}

function parseObjectiveSummary(
  value: unknown,
  label: string,
): ObjectiveSummary {
  const item = object(value, label);
  exact(item, [
    "created_at",
    "id",
    "linked_tickets",
    "record_source",
    "resource_key",
    "state",
    "summary",
    "title",
    "updated_at",
  ], label);
  return {
    created_at: optionalNullableString(item.created_at, `${label}.created_at`),
    id: string(item.id, `${label}.id`),
    linked_tickets: strings(item.linked_tickets, `${label}.linked_tickets`),
    record_source: string(item.record_source, `${label}.record_source`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    state: string(item.state, `${label}.state`),
    summary: string(item.summary, `${label}.summary`),
    title: string(item.title, `${label}.title`),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
  };
}

export function parseObjectiveListResponse(
  value: unknown,
): ObjectiveListResponse {
  const label = "Objective list response";
  const item = object(value, label);
  exact(item, [
    "invalid_records",
    "items",
    "limit",
    "record_authority",
    "workspace_id",
  ], label);
  const limit = integer(item.limit, `${label}.limit`, 0, 1000);
  return {
    invalid_records: array(
      item.invalid_records,
      `${label}.invalid_records`,
      parseInvalidProjectRecord,
    ),
    items: array(item.items, `${label}.items`, parseObjectiveSummary),
    limit: limit as ObjectiveListResponse["limit"],
    record_authority: string(
      item.record_authority,
      `${label}.record_authority`,
    ),
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
  };
}

function parseObjectiveEvent(
  value: unknown,
  label: string,
): ObjectiveEventDetail {
  const item = object(value, label);
  exact(item, ["body", "created_at", "event_ref", "kind"], label);
  return {
    body: optionalNullableString(item.body, `${label}.body`),
    created_at: string(item.created_at, `${label}.created_at`),
    event_ref: string(item.event_ref, `${label}.event_ref`),
    kind: string(item.kind, `${label}.kind`),
  };
}

function parseObjectiveLinkedTicket(
  value: unknown,
  label: string,
): ObjectiveLinkedTicketSummary {
  return parseObjectiveLink(value, label);
}

function parseObjectiveResource(
  value: unknown,
  label: string,
): ObjectiveResourceSummary {
  const item = object(value, label);
  exact(item, ["bytes", "media_type", "path", "updated_at"], label);
  return {
    bytes: integer(item.bytes, `${label}.bytes`),
    media_type: optionalNullableString(item.media_type, `${label}.media_type`),
    path: string(item.path, `${label}.path`),
    updated_at: string(item.updated_at, `${label}.updated_at`),
  };
}

export function parseObjectiveDetail(value: unknown): ObjectiveDetail {
  const label = "Objective detail";
  const item = object(value, label);
  exact(item, [
    "body",
    "body_truncated",
    "created_at",
    "event_page",
    "events",
    "id",
    "linked_ticket_summaries",
    "linked_tickets",
    "record_source",
    "resource_key",
    "resources",
    "revision",
    "state",
    "title",
    "updated_at",
  ], label);
  return {
    body: string(item.body, `${label}.body`),
    body_truncated: boolean(item.body_truncated, `${label}.body_truncated`),
    created_at: optionalNullableString(item.created_at, `${label}.created_at`),
    event_page: parseQueryPage(item.event_page, `${label}.event_page`),
    events: array(item.events, `${label}.events`, parseObjectiveEvent),
    id: string(item.id, `${label}.id`),
    linked_ticket_summaries: array(
      item.linked_ticket_summaries,
      `${label}.linked_ticket_summaries`,
      parseObjectiveLinkedTicket,
    ),
    linked_tickets: strings(item.linked_tickets, `${label}.linked_tickets`),
    record_source: string(item.record_source, `${label}.record_source`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    resources: array(
      item.resources,
      `${label}.resources`,
      parseObjectiveResource,
    ),
    revision: string(item.revision, `${label}.revision`),
    state: string(item.state, `${label}.state`),
    title: string(item.title, `${label}.title`),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
  };
}

function parseMergeRequestDiagnostic(
  value: unknown,
  label: string,
): MergeRequestRefDiagnostic {
  const item = object(value, label);
  exact(item, ["code", "message"], label);
  return {
    code: string(item.code, `${label}.code`),
    message: string(item.message, `${label}.message`),
  };
}

function parseMergeRequestListItem(
  value: unknown,
  label: string,
): MergeRequestListItem {
  const item = object(value, label);
  exact(item, [
    "ref_diagnostics",
    "summary",
    "thread_event_count",
    "ticket_ids",
  ], label);
  return {
    ref_diagnostics: item.ref_diagnostics === undefined ? undefined : array(
      item.ref_diagnostics,
      `${label}.ref_diagnostics`,
      parseMergeRequestDiagnostic,
    ),
    summary: parseTicketMergeRequestSummary(item.summary, `${label}.summary`),
    thread_event_count: integer(
      item.thread_event_count,
      `${label}.thread_event_count`,
    ),
    ticket_ids: strings(item.ticket_ids, `${label}.ticket_ids`),
  };
}

export function parseMergeRequestListResponse(
  value: unknown,
): MergeRequestListResponse {
  const label = "Merge Request list response";
  const item = object(value, label);
  exact(item, ["items", "next_cursor"], label);
  return {
    items: array(item.items, `${label}.items`, parseMergeRequestListItem),
    next_cursor: optionalNullableString(
      item.next_cursor,
      `${label}.next_cursor`,
    ),
  };
}

function parseMergeRequestWorker(
  value: unknown,
  label: string,
): MergeRequestWorkerIdentity {
  const item = object(value, label);
  exact(item, ["runtime_id", "worker_id"], label);
  return {
    runtime_id: string(item.runtime_id, `${label}.runtime_id`),
    worker_id: string(item.worker_id, `${label}.worker_id`),
  };
}

function parseReviewFinding(value: unknown, label: string): ReviewFinding {
  const item = object(value, label);
  exact(item, ["body", "code", "line", "path", "severity"], label);
  const severity = string(item.severity, `${label}.severity`);
  if (!["blocker", "major", "minor", "note"].includes(severity)) {
    throw new Error(`${label}.severity is invalid`);
  }
  return {
    body: string(item.body, `${label}.body`),
    code: optionalNullableString(item.code, `${label}.code`),
    line: item.line === undefined
      ? undefined
      : item.line === null
      ? null
      : integer(item.line, `${label}.line`, 0, 4_294_967_295),
    path: optionalNullableString(item.path, `${label}.path`),
    severity: severity as ReviewFinding["severity"],
  };
}

function parseMergeRequestThreadEvent(
  value: unknown,
  label: string,
): MergeRequestThreadEvent {
  const item = object(value, label);
  const kind = string(item.kind, `${label}.kind`);
  const base = ["created_at", "event_id", "kind", "sequence"];
  const eventBase = {
    created_at: string(item.created_at, `${label}.created_at`),
    event_id: string(item.event_id, `${label}.event_id`),
    sequence: integer(item.sequence, `${label}.sequence`),
  };
  if (kind === "review_requested") {
    exact(item, [...base, "requested_by", "reviewer", "subject_ref"], label);
    return {
      ...eventBase,
      kind,
      requested_by: parseMergeRequestWorker(
        item.requested_by,
        `${label}.requested_by`,
      ),
      reviewer: parseMergeRequestWorker(item.reviewer, `${label}.reviewer`),
      subject_ref: string(item.subject_ref, `${label}.subject_ref`),
    };
  }
  if (kind === "review") {
    exact(item, [
      ...base,
      "body",
      "decision",
      "findings",
      "request_event_id",
      "reviewer",
      "subject_ref",
    ], label);
    const decision = string(item.decision, `${label}.decision`);
    if (!["approve", "request_changes"].includes(decision)) {
      throw new Error(`${label}.decision is invalid`);
    }
    return {
      ...eventBase,
      kind,
      body: string(item.body, `${label}.body`),
      decision: decision as "approve" | "request_changes",
      findings: array(item.findings, `${label}.findings`, parseReviewFinding),
      request_event_id: string(
        item.request_event_id,
        `${label}.request_event_id`,
      ),
      reviewer: parseMergeRequestWorker(item.reviewer, `${label}.reviewer`),
      subject_ref: string(item.subject_ref, `${label}.subject_ref`),
    };
  }
  if (kind === "review_revoked") {
    exact(item, [
      ...base,
      "reason",
      "review_event_id",
      "revoked_by",
      "subject_ref",
    ], label);
    return {
      ...eventBase,
      kind,
      reason: string(item.reason, `${label}.reason`),
      review_event_id: string(item.review_event_id, `${label}.review_event_id`),
      revoked_by: parseMergeRequestWorker(
        item.revoked_by,
        `${label}.revoked_by`,
      ),
      subject_ref: string(item.subject_ref, `${label}.subject_ref`),
    };
  }
  if (kind === "review_cancelled") {
    exact(item, [...base, "reason", "request_event_id", "subject_ref"], label);
    return {
      ...eventBase,
      kind,
      reason: string(item.reason, `${label}.reason`),
      request_event_id: string(
        item.request_event_id,
        `${label}.request_event_id`,
      ),
      subject_ref: string(item.subject_ref, `${label}.subject_ref`),
    };
  }
  if (kind === "comment") {
    exact(item, [...base, "author", "body"], label);
    return {
      ...eventBase,
      kind,
      author: parseMergeRequestWorker(item.author, `${label}.author`),
      body: string(item.body, `${label}.body`),
    };
  }
  if (kind === "merge") {
    exact(item, [
      ...base,
      "approval_event_id",
      "approved_source_ref",
      "merged_by",
      "operation_id",
      "resolution",
      "strategy",
      "target_ref_after",
      "target_ref_before",
    ], label);
    const strategy = string(item.strategy, `${label}.strategy`);
    const resolution = string(item.resolution, `${label}.resolution`);
    if (!["fast_forward", "merge"].includes(strategy)) {
      throw new Error(`${label}.strategy is invalid`);
    }
    if (!["none", "clean", "conflicts_resolved"].includes(resolution)) {
      throw new Error(`${label}.resolution is invalid`);
    }
    return {
      ...eventBase,
      kind,
      approval_event_id: string(
        item.approval_event_id,
        `${label}.approval_event_id`,
      ),
      approved_source_ref: string(
        item.approved_source_ref,
        `${label}.approved_source_ref`,
      ),
      merged_by: parseMergeRequestWorker(item.merged_by, `${label}.merged_by`),
      operation_id: string(item.operation_id, `${label}.operation_id`),
      resolution: resolution as "none" | "clean" | "conflicts_resolved",
      strategy: strategy as "fast_forward" | "merge",
      target_ref_after: string(
        item.target_ref_after,
        `${label}.target_ref_after`,
      ),
      target_ref_before: string(
        item.target_ref_before,
        `${label}.target_ref_before`,
      ),
    };
  }
  throw new Error(`${label}.kind is invalid`);
}

function parseMergeRequestRef(
  value: unknown,
  label: string,
): MergeRequestRefResponse {
  const item = object(value, label);
  exact(item, ["diagnostic", "observed_at", "ref", "status"], label);
  return {
    diagnostic: item.diagnostic === undefined
      ? undefined
      : item.diagnostic === null
      ? null
      : parseMergeRequestDiagnostic(item.diagnostic, `${label}.diagnostic`),
    observed_at: string(item.observed_at, `${label}.observed_at`),
    ref: optionalNullableString(item.ref, `${label}.ref`),
    status: string(item.status, `${label}.status`),
  };
}

function parseLinkedTicket(
  value: unknown,
  label: string,
): MergeRequestLinkedTicketResponse {
  const item = object(value, label);
  exact(item, ["key", "ticket_id"], label);
  return {
    key: optionalNullableString(item.key, `${label}.key`),
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
  };
}

export function parseMergeRequestDetailResponse(
  value: unknown,
): MergeRequestDetailResponse {
  const label = "Merge Request detail";
  const item = object(value, label);
  exact(item, [
    "created_at",
    "linked_tickets",
    "merge_request_id",
    "repository_key",
    "selector_from",
    "selector_to",
    "source",
    "state",
    "target",
    "thread",
    "ticket_ids",
    "updated_at",
    "workspace_id",
  ], label);
  const state = string(item.state, `${label}.state`);
  if (!["open", "merged", "closed"].includes(state)) {
    throw new Error(`${label}.state is invalid`);
  }
  return {
    created_at: string(item.created_at, `${label}.created_at`),
    linked_tickets: array(
      item.linked_tickets,
      `${label}.linked_tickets`,
      parseLinkedTicket,
    ),
    merge_request_id: string(
      item.merge_request_id,
      `${label}.merge_request_id`,
    ),
    repository_key: string(item.repository_key, `${label}.repository_key`),
    selector_from: optionalNullableString(
      item.selector_from,
      `${label}.selector_from`,
    ),
    selector_to: string(item.selector_to, `${label}.selector_to`),
    source: parseMergeRequestRef(item.source, `${label}.source`),
    state: state as MergeRequestDetailResponse["state"],
    target: parseMergeRequestRef(item.target, `${label}.target`),
    thread: array(item.thread, `${label}.thread`, parseMergeRequestThreadEvent),
    ticket_ids: strings(item.ticket_ids, `${label}.ticket_ids`),
    updated_at: string(item.updated_at, `${label}.updated_at`),
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
  };
}
