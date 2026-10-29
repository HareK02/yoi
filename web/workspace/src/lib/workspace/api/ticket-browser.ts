import type {
  BrowserTicketState,
  InvalidProjectRecord,
  MergeRequestDetailResponse,
  MergeRequestLinkedTicketResponse,
  MergeRequestListItem,
  MergeRequestListResponse,
  MergeRequestRefDiagnostic,
  MergeRequestRefResponse,
  MergeRequestReviewSubject,
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
  TicketAssignmentRelease,
  TicketAssignmentSummary,
  TicketAssignmentTracking,
  TicketAssignmentTrackingProjection,
  TicketCaseAction,
  TicketCaseHistoryEntry,
  TicketClosureJudgment,
  TicketClosureReason,
  TicketDetail,
  TicketDetailDerivedRelation,
  TicketDetailRelation,
  TicketDetailRelationBlocker,
  TicketDetailRelationNotice,
  TicketDetailRelationView,
  TicketEventDetail,
  TicketEvidenceEvent,
  TicketEvidenceSummary,
  TicketLegacyWorkflowSnapshot,
  TicketListItemSummary,
  TicketListResponse,
  TicketMergeRequestSummary,
  TicketBrowseQueryItem,
  TicketBrowseQueryResponse,
  TicketQueryItem,
  TicketQueryResponse,
  TicketQueueOutcome,
  TicketRef,
  TicketReference,
  TicketRoleAssignmentMutationResponse,
  TicketRoleAssignmentRecord,
  TicketRoleAssignmentSummary,
  TicketSummary,
  TicketTarget,
  WorkspaceTag,
} from "#lib/generated/ticket-api.ts";

export class TicketBrowserContractError extends Error {}

export const TICKET_BROWSER_API_MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
export const TICKET_BROWSER_API_LOAD_POLICY = {
  diagnosticLabel: "Ticket Browser API",
  maxResponseBytes: TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
  describeValidationError: (error: unknown) =>
    error instanceof TicketBrowserContractError ? error.message : undefined,
} as const;

const MAX_STRING_LENGTH = 262_144;
const MAX_COLLECTION_LENGTH = 4_096;
const MAX_MAP_ENTRIES = 4_096;

function object(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TicketBrowserContractError(`${label} must be an object`);
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
    throw new TicketBrowserContractError(`${label} contains unknown field`);
  }
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string") throw new TicketBrowserContractError(`${label} must be a string`);
  if (value.length > MAX_STRING_LENGTH) {
    throw new TicketBrowserContractError(`${label} exceeds its length limit`);
  }
  return value;
}

function boolean(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") throw new TicketBrowserContractError(`${label} must be a boolean`);
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
    throw new TicketBrowserContractError(
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
  if (!Array.isArray(value)) throw new TicketBrowserContractError(`${label} must be an array`);
  if (value.length > MAX_COLLECTION_LENGTH) {
    throw new TicketBrowserContractError(`${label} exceeds its item limit`);
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
    throw new TicketBrowserContractError(`${label} exceeds its entry limit`);
  }
  return Object.fromEntries(
    entries.map(([key, item]) => [
      string(key, `${label} key`),
      string(item, `${label} value`),
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

function parseTicketState(value: unknown, label: string): BrowserTicketState {
  if (value !== "open" && value !== "closed") {
    throw new TicketBrowserContractError(`${label} is invalid`);
  }
  return value;
}

function parseClosureReason(
  value: unknown,
  label: string,
): TicketClosureReason {
  if (
    typeof value === "string" &&
    ["completed", "not_planned", "duplicate", "unknown"].includes(value)
  ) {
    return value as TicketClosureReason;
  }
  const item = object(value, label);
  exact(item, ["other"], label);
  return { other: string(item.other, `${label}.other`) };
}

function parseCaseAction(value: unknown, label: string): TicketCaseAction {
  const item = object(value, label);
  if (item.action === "reopen") {
    exact(item, ["action"], label);
    return { action: "reopen" };
  }
  if (item.action !== "close" && item.action !== "correct_closure") {
    throw new TicketBrowserContractError(`${label}.action is invalid`);
  }
  exact(item, ["action", "reason"], label);
  return {
    action: item.action,
    reason: parseClosureReason(item.reason, `${label}.reason`),
  };
}

function parseReference(value: unknown, label: string): TicketReference {
  const item = object(value, label);
  exact(item, ["kind", "target"], label);
  return {
    kind: string(item.kind, `${label}.kind`),
    target: string(item.target, `${label}.target`),
  };
}

function parseClosure(value: unknown, label: string): TicketClosureJudgment {
  const item = object(value, label);
  exact(item, [
    "operation_key",
    "reason",
    "explanation",
    "references",
    "author",
    "at",
  ], label);
  return {
    operation_key: string(item.operation_key, `${label}.operation_key`),
    reason: parseClosureReason(item.reason, `${label}.reason`),
    explanation: string(item.explanation, `${label}.explanation`),
    references: array(item.references, `${label}.references`, parseReference),
    author: string(item.author, `${label}.author`),
    at: string(item.at, `${label}.at`),
  };
}

function parseCaseHistory(
  value: unknown,
  label: string,
): TicketCaseHistoryEntry {
  const item = object(value, label);
  exact(item, [
    "operation_key",
    "previous_closure_operation_key",
    "action",
    "explanation",
    "references",
    "author",
    "at",
  ], label);
  return {
    operation_key: string(item.operation_key, `${label}.operation_key`),
    previous_closure_operation_key: optionalNullableString(
      item.previous_closure_operation_key,
      `${label}.previous_closure_operation_key`,
    ),
    action: parseCaseAction(item.action, `${label}.action`),
    explanation: string(item.explanation, `${label}.explanation`),
    references: array(item.references, `${label}.references`, parseReference),
    author: string(item.author, `${label}.author`),
    at: string(item.at, `${label}.at`),
  };
}

function parseLegacyHandoff(
  value: unknown,
  label: string,
): TicketLegacyWorkflowSnapshot {
  const item = object(value, label);
  exact(item, [
    "workflow_state",
    "workflow_state_explicit",
    "status",
    "queued_by",
    "queued_at",
    "resolution",
  ], label);
  return {
    workflow_state: string(item.workflow_state, `${label}.workflow_state`),
    workflow_state_explicit: boolean(
      item.workflow_state_explicit,
      `${label}.workflow_state_explicit`,
    ),
    status: string(item.status, `${label}.status`),
    queued_by: optionalNullableString(item.queued_by, `${label}.queued_by`),
    queued_at: optionalNullableString(item.queued_at, `${label}.queued_at`),
    resolution: optionalNullableString(item.resolution, `${label}.resolution`),
  };
}

function caseProjection(item: Record<string, unknown>, label: string) {
  return {
    closure_operation_key: optionalNullableString(
      item.closure_operation_key,
      `${label}.closure_operation_key`,
    ),
    current_closure: item.current_closure === undefined
      ? undefined
      : item.current_closure === null
      ? null
      : parseClosure(item.current_closure, `${label}.current_closure`),
    legacy_handoff: item.legacy_handoff === undefined
      ? undefined
      : item.legacy_handoff === null
      ? null
      : parseLegacyHandoff(item.legacy_handoff, `${label}.legacy_handoff`),
  };
}

export function parseWorkspaceTag(
  value: unknown,
  label = "Workspace tag",
): WorkspaceTag {
  const item = object(value, label);
  exact(item, [
    "workspace_id",
    "id",
    "name",
    "last_operation_key",
    "deleted",
  ], label);
  const id = string(item.id, `${label}.id`);
  if (id.trim().length === 0 || new TextEncoder().encode(id).length > 128) {
    throw new TicketBrowserContractError(`${label}.id must be nonempty and at most 128 UTF-8 bytes`);
  }
  return {
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
    id,
    name: string(item.name, `${label}.name`),
    last_operation_key: string(
      item.last_operation_key,
      `${label}.last_operation_key`,
    ),
    deleted: boolean(item.deleted, `${label}.deleted`),
  };
}

export function parseTicketSummary(value: unknown): TicketSummary {
  const label = "Ticket summary";
  const item = object(value, label);
  exact(item, [
    "id",
    "kind",
    "labels",
    "priority",
    "queued_at",
    "queued_by",
    "readiness",
    "resource_key",
    "slug",
    "status",
    "tags",
    "title",
    "updated_at",
    "workflow_state",
    "workflow_state_explicit",
  ], label);
  let status: TicketSummary["status"];
  if (item.status === "Open" || item.status === "Closed") status = item.status;
  else {
    const other = object(item.status, `${label}.status`);
    exact(other, ["Other"], `${label}.status`);
    status = { Other: string(other.Other, `${label}.status.Other`) };
  }
  return {
    id: string(item.id, `${label}.id`),
    kind: string(item.kind, `${label}.kind`),
    labels: strings(item.labels, `${label}.labels`),
    priority: string(item.priority, `${label}.priority`),
    queued_at: optionalNullableString(item.queued_at, `${label}.queued_at`),
    queued_by: optionalNullableString(item.queued_by, `${label}.queued_by`),
    readiness: optionalNullableString(item.readiness, `${label}.readiness`),
    resource_key: optionalNullableString(
      item.resource_key,
      `${label}.resource_key`,
    ),
    slug: string(item.slug, `${label}.slug`),
    status,
    tags: array(item.tags, `${label}.tags`, parseWorkspaceTag),
    title: string(item.title, `${label}.title`),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
    workflow_state: parseTicketState(
      item.workflow_state,
      `${label}.workflow_state`,
    ),
    workflow_state_explicit: boolean(
      item.workflow_state_explicit,
      `${label}.workflow_state_explicit`,
    ),
  };
}

function parseTicketListItem(
  value: unknown,
  label: string,
): TicketListItemSummary {
  const item = object(value, label);
  exact(item, [
    "closure_operation_key",
    "current_closure",
    "legacy_handoff",
    "id",
    "priority",
    "queued_at",
    "queued_by",
    "record_source",
    "resource_key",
    "state",
    "tags",
    "title",
    "updated_at",
    "workspace_action_priority",
  ], label);
  return {
    ...caseProjection(item, label),
    id: string(item.id, `${label}.id`),
    priority: string(item.priority, `${label}.priority`),
    queued_at: optionalNullableString(item.queued_at, `${label}.queued_at`),
    queued_by: optionalNullableString(item.queued_by, `${label}.queued_by`),
    record_source: string(item.record_source, `${label}.record_source`),
    resource_key: string(item.resource_key, `${label}.resource_key`),
    state: parseTicketState(item.state, `${label}.state`),
    tags: array(item.tags, `${label}.tags`, parseWorkspaceTag),
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

export function parseTicketEvent(value: unknown, label = "Ticket post"): TicketEventDetail {
  const item = object(value, label);
  exact(item, [
    "is_initial",
    "edited_at",
    "edited_by",
    "body_digest",
    "body_truncated",
    "editable",
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
    is_initial: boolean(item.is_initial, `${label}.is_initial`),
    edited_at: optionalNullableString(item.edited_at, `${label}.edited_at`),
    edited_by: optionalNullableString(item.edited_by, `${label}.edited_by`),
    body_digest: string(item.body_digest, `${label}.body_digest`),
    body_truncated: boolean(item.body_truncated, `${label}.body_truncated`),
    editable: boolean(item.editable, `${label}.editable`),
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
  if (kind === "subject") {
    exact(item, ["subject_id", "kind"], label);
    return { kind, subject_id: string(item.subject_id, `${label}.subject_id`) };
  }
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
  throw new TicketBrowserContractError(`${label}.kind is invalid`);
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
  if (!["orchestrator", "worker", "owner", "contributor"].includes(role)) {
    throw new TicketBrowserContractError(`${label}.role is invalid`);
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

export function parseAssignmentTracking(
  value: unknown,
  label: string,
): TicketAssignmentTracking {
  const item = object(value, label);
  exact(
    item,
    ["status", "waiting_reason", "progress", "next_action", "result"],
    label,
  );
  const status = string(item.status, `${label}.status`);
  if (status !== "waiting" && status !== "inprogress" && status !== "done") {
    throw new TicketBrowserContractError(`${label}.status is invalid`);
  }
  return {
    status,
    waiting_reason: optionalNullableString(
      item.waiting_reason,
      `${label}.waiting_reason`,
    ),
    progress: optionalNullableString(item.progress, `${label}.progress`),
    next_action: optionalNullableString(
      item.next_action,
      `${label}.next_action`,
    ),
    result: optionalNullableString(item.result, `${label}.result`),
  };
}

function parseAssignmentRelease(
  value: unknown,
  label: string,
): TicketAssignmentRelease {
  const item = object(value, label);
  exact(item, [
    "action",
    "event_id",
    "operation_key",
    "actor",
    "at",
    "reason",
    "replacement_assignment_id",
    "last_tracking",
  ], label);
  const action = string(item.action, `${label}.action`);
  if (action !== "unassigned" && action !== "replaced") {
    throw new TicketBrowserContractError(`${label}.action is invalid`);
  }
  return {
    action,
    event_id: string(item.event_id, `${label}.event_id`),
    operation_key: optionalNullableString(
      item.operation_key,
      `${label}.operation_key`,
    ),
    actor: string(item.actor, `${label}.actor`),
    at: string(item.at, `${label}.at`),
    reason: optionalNullableString(item.reason, `${label}.reason`),
    replacement_assignment_id: optionalNullableString(
      item.replacement_assignment_id,
      `${label}.replacement_assignment_id`,
    ),
    last_tracking: item.last_tracking === undefined
      ? undefined
      : item.last_tracking === null
      ? null
      : parseAssignmentTracking(item.last_tracking, `${label}.last_tracking`),
  };
}

export function parseAssignmentTrackingProjection(
  value: unknown,
  label: string,
): TicketAssignmentTrackingProjection {
  const item = object(value, label);
  exact(item, [
    "workspace_id",
    "ticket_id",
    "assignment_id",
    "role",
    "principal",
    "assigned_by",
    "assigned_at",
    "release",
    "current",
    "tracking",
    "last_event_id",
    "updated_at",
    "migration_diagnostic",
  ], label);
  return {
    workspace_id: string(item.workspace_id, `${label}.workspace_id`),
    ticket_id: string(item.ticket_id, `${label}.ticket_id`),
    assignment_id: string(item.assignment_id, `${label}.assignment_id`),
    role: string(item.role, `${label}.role`),
    principal: parseTicketAssignmentPrincipal(
      item.principal,
      `${label}.principal`,
    ),
    assigned_by: string(item.assigned_by, `${label}.assigned_by`),
    assigned_at: string(item.assigned_at, `${label}.assigned_at`),
    release: item.release === undefined
      ? undefined
      : item.release === null
      ? null
      : parseAssignmentRelease(item.release, `${label}.release`),
    current: boolean(item.current, `${label}.current`),
    tracking: item.tracking === undefined
      ? undefined
      : item.tracking === null
      ? null
      : parseAssignmentTracking(item.tracking, `${label}.tracking`),
    last_event_id: optionalNullableString(
      item.last_event_id,
      `${label}.last_event_id`,
    ),
    updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
    migration_diagnostic: optionalNullableString(
      item.migration_diagnostic,
      `${label}.migration_diagnostic`,
    ),
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
    "can_start_manual_worker",
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
    can_start_manual_worker: boolean(
      item.can_start_manual_worker,
      `${label}.can_start_manual_worker`,
    ),
    can_unassign_orchestrator: boolean(
      item.can_unassign_orchestrator,
      `${label}.can_unassign_orchestrator`,
    ),
    queue_tickets: strings(item.queue_tickets, `${label}.queue_tickets`),
  };
}

function parseSourceRefObservation(
  value: unknown,
  label: string,
): TicketMergeRequestSummary["source_ref_observation"] {
  const item = object(value, label);
  const status = string(item.status, `${label}.status`);
  if (status === "observed" || status === "not_required") {
    exact(item, ["status"], label);
    return { status };
  }
  if (status === "unavailable") {
    exact(item, ["status", "code"], label);
    return { status, code: string(item.code, `${label}.code`) };
  }
  throw new TicketBrowserContractError(`${label}.status is invalid`);
}

function parseTicketMergeRequestSummary(
  value: unknown,
  label: string,
): TicketMergeRequestSummary {
  const item = object(value, label);
  exact(item, [
    "current_subject_ref",
    "integration_evidence_error",
    "source_ref_observation",
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
    integration_evidence_error: nullableString(
      item.integration_evidence_error,
      `${label}.integration_evidence_error`,
    ),
    source_ref_observation: parseSourceRefObservation(
      item.source_ref_observation,
      `${label}.source_ref_observation`,
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
    throw new TicketBrowserContractError(`${label}.access is invalid`);
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
    "assignment_tracking",
    "assignments",
    "closure_history",
    "body",
    "body_truncated",
    "created_by",
    "initial_post",
    "created_at",
    "current_worker",
    "event_count",
    "event_page",
    "events",
    "evidence",
    "closure_operation_key",
    "current_closure",
    "legacy_handoff",
    "id",
    "implementation_reports",
    "content_digest",
    "linked_objectives",
    "merge_request",
    "merge_requests",
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
    "tags",
    "targets",
    "title",
    "updated_at",
  ], label);
  return {
    ...caseProjection(item, label),
    closure_history: array(
      item.closure_history,
      `${label}.closure_history`,
      parseCaseHistory,
    ),
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
    assignment_tracking: array(
      item.assignment_tracking,
      `${label}.assignment_tracking`,
      parseAssignmentTrackingProjection,
    ),
    assignments: array(
      item.assignments,
      `${label}.assignments`,
      parseTicketRoleAssignmentSummary,
    ),
    body: string(item.body, `${label}.body`),
    body_truncated: boolean(item.body_truncated, `${label}.body_truncated`),
    created_by: optionalNullableString(item.created_by, `${label}.created_by`),
    initial_post: parseTicketEvent(item.initial_post, `${label}.initial_post`),
    created_at: optionalNullableString(item.created_at, `${label}.created_at`),
    current_worker: item.current_worker === undefined
      ? undefined
      : item.current_worker === null
      ? null
      : parseTicketAssignmentSummary(
        item.current_worker,
        `${label}.current_worker`,
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
    content_digest: string(item.content_digest, `${label}.content_digest`),
    linked_objectives: array(
      item.linked_objectives,
      `${label}.linked_objectives`,
      parseObjectiveLink,
    ),
    merge_requests: array(
      item.merge_requests,
      `${label}.merge_requests`,
      parseTicketMergeRequestSummary,
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
    state: parseTicketState(item.state, `${label}.state`),
    tags: array(item.tags, `${label}.tags`, parseWorkspaceTag),
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
    throw new TicketBrowserContractError(`${label}.status is invalid`);
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

function parseMergeRequestReviewSubject(
  value: unknown,
  label: string,
): MergeRequestReviewSubject {
  const item = object(value, label);
  exact(item, ["merge_request_id", "subject_ref"], label);
  return {
    merge_request_id: string(
      item.merge_request_id,
      `${label}.merge_request_id`,
    ),
    subject_ref: string(item.subject_ref, `${label}.subject_ref`),
  };
}

function parseReviewFinding(value: unknown, label: string): ReviewFinding {
  const item = object(value, label);
  exact(item, ["body", "code", "line", "path", "severity"], label);
  const severity = string(item.severity, `${label}.severity`);
  if (!["blocker", "major", "minor", "note"].includes(severity)) {
    throw new TicketBrowserContractError(`${label}.severity is invalid`);
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
    exact(
      item,
      [
        ...base,
        "requested_by",
        "reviewer",
        "subject_ref",
        "ticket_content_digest",
        "ticket_merge_request_subjects",
      ],
      label,
    );
    return {
      ...eventBase,
      kind,
      requested_by: parseMergeRequestWorker(
        item.requested_by,
        `${label}.requested_by`,
      ),
      reviewer: parseMergeRequestWorker(item.reviewer, `${label}.reviewer`),
      subject_ref: string(item.subject_ref, `${label}.subject_ref`),
      ticket_content_digest: string(
        item.ticket_content_digest,
        `${label}.ticket_content_digest`,
      ),
      ticket_merge_request_subjects: array(
        item.ticket_merge_request_subjects,
        `${label}.ticket_merge_request_subjects`,
        parseMergeRequestReviewSubject,
      ),
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
      "ticket_content_digest",
      "ticket_merge_request_subjects",
    ], label);
    const decision = string(item.decision, `${label}.decision`);
    if (!["approve", "request_changes"].includes(decision)) {
      throw new TicketBrowserContractError(`${label}.decision is invalid`);
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
      ticket_content_digest: string(
        item.ticket_content_digest,
        `${label}.ticket_content_digest`,
      ),
      ticket_merge_request_subjects: array(
        item.ticket_merge_request_subjects,
        `${label}.ticket_merge_request_subjects`,
        parseMergeRequestReviewSubject,
      ),
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
      throw new TicketBrowserContractError(`${label}.strategy is invalid`);
    }
    if (!["none", "clean", "conflicts_resolved"].includes(resolution)) {
      throw new TicketBrowserContractError(`${label}.resolution is invalid`);
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
  throw new TicketBrowserContractError(`${label}.kind is invalid`);
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
    throw new TicketBrowserContractError(`${label}.state is invalid`);
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

export function parseTicketBrowseQueryResponse(value: unknown): TicketBrowseQueryResponse {
  const label = "Ticket browse response";
  const page = object(value, label);
  exact(page, ["items", "page", "record_authority"], label);
  return {
    page: parseQueryPage(page.page, `${label}.page`),
    record_authority: string(page.record_authority, `${label}.record_authority`),
    items: array(page.items, `${label}.items`, (value, label): TicketBrowseQueryItem => {
      const item = object(value, label);
      exact(item, [
        "id", "resource_key", "title", "state", "updated_at", "tags",
        "current_closure", "assignment_tracking",
      ], label);
      return {
        id: string(item.id, `${label}.id`),
        resource_key: string(item.resource_key, `${label}.resource_key`),
        title: string(item.title, `${label}.title`),
        state: parseTicketState(item.state, `${label}.state`),
        updated_at: optionalNullableString(item.updated_at, `${label}.updated_at`),
        tags: array(item.tags, `${label}.tags`, parseWorkspaceTag),
        current_closure: item.current_closure == null
          ? item.current_closure as null | undefined
          : parseClosure(item.current_closure, `${label}.current_closure`),
        assignment_tracking: array(
          item.assignment_tracking, `${label}.assignment_tracking`, parseAssignmentTrackingProjection,
        ),
      };
    }),
  };
}

export function parseTicketQueryResponse(value: unknown): TicketQueryResponse {
  const label = "Ticket query response";
  const page = object(value, label);
  exact(page, ["items", "page", "record_authority"], label);
  return {
    page: parseQueryPage(page.page, `${label}.page`),
    record_authority: string(
      page.record_authority,
      `${label}.record_authority`,
    ),
    items: array(
      page.items,
      `${label}.items`,
      (value, label): TicketQueryItem => {
        const item = object(value, label);
        exact(item, [
          "assignment_tracking",
          "blocker_count",
          "closure_operation_key",
          "content_digest",
          "created_at",
          "current_closure",
          "current_worker",
          "evidence",
          "id",
          "legacy_handoff",
          "linked_objective_ids",
          "linked_objective_keys",
          "matched_fields",
          "matching_event",
          "merge_request",
          "merge_requests",
          "priority",
          "readiness",
          "relation_count",
          "resource_key",
          "snippet",
          "state",
          "tags",
          "title",
          "unresolved_blocker_count",
          "unresolved_review_count",
          "updated_at",
          "workspace_action_priority",
        ], label);
        return {
          id: string(item.id, `${label}.id`),
          resource_key: string(item.resource_key, `${label}.resource_key`),
          title: string(item.title, `${label}.title`),
          state: parseTicketState(item.state, `${label}.state`),
          priority: string(item.priority, `${label}.priority`),
          assignment_tracking: array(
            item.assignment_tracking,
            `${label}.assignment_tracking`,
            parseAssignmentTrackingProjection,
          ),
          tags: array(item.tags, `${label}.tags`, parseWorkspaceTag),
          blocker_count: integer(item.blocker_count, `${label}.blocker_count`),
          unresolved_blocker_count: integer(
            item.unresolved_blocker_count,
            `${label}.unresolved_blocker_count`,
          ),
          unresolved_review_count: integer(
            item.unresolved_review_count,
            `${label}.unresolved_review_count`,
          ),
          relation_count: integer(
            item.relation_count,
            `${label}.relation_count`,
          ),
          content_digest: string(
            item.content_digest,
            `${label}.content_digest`,
          ),
          created_at: optionalNullableString(
            item.created_at,
            `${label}.created_at`,
          ),
          updated_at: optionalNullableString(
            item.updated_at,
            `${label}.updated_at`,
          ),
          closure_operation_key: optionalNullableString(
            item.closure_operation_key,
            `${label}.closure_operation_key`,
          ),
          current_closure: item.current_closure == null
            ? item.current_closure as null | undefined
            : parseClosure(item.current_closure, `${label}.current_closure`),
          current_worker: item.current_worker == null
            ? item.current_worker as null | undefined
            : parseTicketAssignmentSummary(
              item.current_worker,
              `${label}.current_worker`,
            ),
          evidence: parseTicketEvidenceSummary(
            item.evidence,
            `${label}.evidence`,
          ),
          legacy_handoff: item.legacy_handoff == null
            ? item.legacy_handoff as null | undefined
            : parseLegacyHandoff(
              item.legacy_handoff,
              `${label}.legacy_handoff`,
            ),
          linked_objective_ids: strings(
            item.linked_objective_ids,
            `${label}.linked_objective_ids`,
          ),
          linked_objective_keys: strings(
            item.linked_objective_keys,
            `${label}.linked_objective_keys`,
          ),
          matched_fields: strings(
            item.matched_fields,
            `${label}.matched_fields`,
          ),
          matching_event: item.matching_event == null
            ? item.matching_event as null | undefined
            : parseTicketEvidenceEvent(
              item.matching_event,
              `${label}.matching_event`,
            ),
          merge_request: item.merge_request == null
            ? item.merge_request as null | undefined
            : parseTicketMergeRequestSummary(
              item.merge_request,
              `${label}.merge_request`,
            ),
          merge_requests: array(
            item.merge_requests,
            `${label}.merge_requests`,
            parseTicketMergeRequestSummary,
          ),
          readiness: optionalNullableString(
            item.readiness,
            `${label}.readiness`,
          ),
          snippet: optionalNullableString(item.snippet, `${label}.snippet`),
          workspace_action_priority: string(
            item.workspace_action_priority,
            `${label}.workspace_action_priority`,
          ),
        };
      },
    ),
  };
}
