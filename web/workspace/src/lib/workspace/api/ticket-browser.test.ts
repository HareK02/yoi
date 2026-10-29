declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

import type { TicketBrowseQueryResponse } from "#lib/generated/ticket-api.ts";
import { loadJson } from "./http.ts";
import { fixturePost } from "../home/dashboard.test-fixtures.ts";
import {
  TICKET_BROWSER_API_LOAD_POLICY,
  parseTicketEvent,
  parseAssignmentTrackingProjection,
  parseMergeRequestDetailResponse,
  parseMergeRequestListResponse,
  parseObjectiveListResponse,
  parseTicketDetail,
  parseTicketListResponse,
  parseTicketBrowseQueryResponse,
  parseTicketRecordRef,
  parseTicketRoleAssignmentMutationResponse,
  parseTicketSummary,
} from "./ticket-browser.ts";

function assertEquals(actual: unknown, expected: unknown): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, received ${actualJson}`);
  }
}

function assertThrows(operation: () => unknown, message: string): void {
  try {
    operation();
  } catch (error) {
    if (error instanceof Error && error.message.includes(message)) return;
    throw error;
  }
  throw new Error(`Expected operation to throw: ${message}`);
}

const page = {
  has_more: false,
  limit: 30,
  next_cursor: null,
  returned: 1,
  sort: "updated_desc",
  source_limit: null,
  source_truncated: false,
};

function browseFixture(): TicketBrowseQueryResponse {
  return {
    record_authority: "sqlite",
    page: { ...page, has_more: true, next_cursor: "opaque-cursor" },
    items: [{
      id: "ticket-id", resource_key: "T-1", title: "Minimal browse row",
      state: "open", updated_at: null, tags: [], current_closure: null,
      assignment_tracking: [],
    }],
  };
}

Deno.test("Ticket browse accepts the generated minimal row and preserves cursor metadata without enriched query fields", () => {
  const fixture = browseFixture();
  const parsed = parseTicketBrowseQueryResponse(fixture);
  assertEquals(parsed, { page: fixture.page, record_authority: "sqlite", items: fixture.items });
  for (const field of ["evidence", "merge_requests", "content_digest", "snippet", "blocker_count", "current_worker"]) {
    assertEquals(field in parsed.items[0], false);
    assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: [{ ...fixture.items[0], [field]: null }] }), "unknown field");
  }
  assertEquals(parseTicketBrowseQueryResponse({ ...fixture, items: [], page: { ...page, returned: 0 } }).items, []);
});

Deno.test("Ticket browse preserves case, tags and self-reported handling independently", () => {
  const fixture = browseFixture();
  const assignment = {
    workspace_id: "workspace-a", ticket_id: "ticket-id", assignment_id: "delegation-1",
    role: "orchestrator", principal: { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
    assigned_by: "user:alice", assigned_at: "2026-10-15T00:00:00Z", current: true,
    release: null, last_event_id: "event-1", updated_at: null, migration_diagnostic: null,
    tracking: { status: "done", waiting_reason: null, progress: "Investigated", next_action: "Human decision", result: "Options returned" },
  };
  for (const state of ["open", "closed"]) {
    const row = parseTicketBrowseQueryResponse({
      ...fixture,
      items: [{ ...fixture.items[0], state, tags, current_closure: state === "closed" ? closure : null, assignment_tracking: [assignment] }],
    }).items[0];
    assertEquals(row.state, state);
    assertEquals(row.tags, tags);
    for (const [field, expected] of Object.entries(assignment)) {
      assertEquals((row.assignment_tracking[0] as unknown as Record<string, unknown>)[field], expected);
    }
    assertEquals(row.current_closure, state === "closed" ? closure : null);
  }
  assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: [{ ...fixture.items[0], assignment_tracking: [{ ...assignment, tracking: { status: "running" } }] }] }), "status is invalid");
});

Deno.test("Ticket browse rejects missing required fields, legacy states and malformed row values", () => {
  const fixture = browseFixture();
  for (const field of ["id", "resource_key", "title", "state", "tags", "assignment_tracking"]) {
    const row: Record<string, unknown> = { ...fixture.items[0] };
    delete row[field];
    assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: [row] }), `items[0].${field}`);
  }
  for (const [field, value] of [
    ["id", 1], ["title", false], ["state", "done"], ["state", "queued"],
    ["updated_at", 7], ["tags", null], ["tags", [{ ...tags[0], id: " " }]],
    ["assignment_tracking", {}], ["current_closure", { ...closure, reason: "done" }],
  ] as const) {
    assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: [{ ...fixture.items[0], [field]: value }] }), `items[0].${field}`);
  }
});

Deno.test("Ticket browse validates envelope, pagination and collection bounds", () => {
  const fixture = browseFixture();
  for (const field of ["items", "page", "record_authority"]) {
    const wire: Record<string, unknown> = { ...fixture };
    delete wire[field];
    assertThrows(() => parseTicketBrowseQueryResponse(wire), `response.${field}`);
  }
  assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, unexpected: true }), "unknown field");
  for (const [field, value] of [["has_more", "yes"], ["next_cursor", 42], ["limit", -1], ["returned", 1.5], ["source_truncated", null]] as const) {
    assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, page: { ...fixture.page, [field]: value } }), `page.${field}`);
  }
  assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, page: { ...fixture.page, extra: true } }), "unknown field");
  assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: Array(4097).fill(fixture.items[0]) }), "items exceeds its item limit");
  assertThrows(() => parseTicketBrowseQueryResponse({ ...fixture, items: [{ ...fixture.items[0], title: "x".repeat(262145) }] }), "title exceeds its length limit");
});

Deno.test("Ticket browse load diagnostics identify the malformed field without exposing row content", async () => {
  const fixture = browseFixture();
  const result = await loadJson(
    (async () => Response.json({ ...fixture, items: [{ ...fixture.items[0], title: "PRIVATE_TITLE", tags: null }] })) as typeof fetch,
    "/api/w/workspace-a/tickets/browse", { method: "POST" },
    parseTicketBrowseQueryResponse, TICKET_BROWSER_API_LOAD_POLICY,
  );
  assertEquals(result.data, null);
  assertEquals(result.error, "Ticket Browser API: Ticket browse response.items[0].tags must be an array");
  assertEquals(result.error?.includes("PRIVATE_TITLE"), false);
});

Deno.test("Ticket post parser preserves author and edit metadata independently of pagination", () => {
  const initial = fixturePost("Current initial post", { edited_at: "2026-10-13T10:00:00Z", edited_by: "Rin" });
  const parsed = parseTicketDetail({ ...ticketDetailFixture(), initial_post: initial, events: [], event_page: { ...page, returned: 0, has_more: true } });
  assertEquals(parsed.initial_post.author, initial.author);
  assertEquals(parsed.initial_post.at, initial.at);
  assertEquals(parsed.initial_post.edited_at, initial.edited_at);
  assertEquals(parsed.initial_post.edited_by, "Rin");
  assertEquals(parsed.initial_post.body_digest, initial.body_digest);
  assertEquals(parsed.initial_post.is_initial, true);
  assertEquals(parsed.created_by, null);
  assertEquals(parsed.events, []);
});

Deno.test("Ticket post parser rejects absent safety fields and unknown wire fields", () => {
  for (const field of ["is_initial", "body_digest", "body_truncated", "editable"]) {
    const wire: Record<string, unknown> = { ...fixturePost("Post") };
    delete wire[field];
    assertThrows(() => parseTicketEvent(wire), field);
  }
  assertThrows(() => parseTicketEvent({ ...fixturePost("Post"), editable: "yes" }), "editable");
  assertThrows(() => parseTicketEvent({ ...fixturePost("Post"), edited_by: 7 }), "edited_by");
  assertThrows(() => parseTicketEvent({ ...fixturePost("Post"), surprise: true }), "unknown field");
  const detail: Record<string, unknown> = { ...ticketDetailFixture() };
  delete detail.initial_post;
  assertThrows(() => parseTicketDetail(detail), "initial_post");
});

Deno.test("Ticket load diagnostics retain the failed field without leaking response contents", async () => {
  const secret = "PRIVATE_RESPONSE_TEXT";
  const wire: Record<string, unknown> = { ...ticketDetailFixture(), body: secret };
  delete wire.initial_post;
  const load = (payload: unknown) => loadJson(
    (async () => Response.json(payload)) as typeof fetch,
    "/api/w/workspace-a/tickets/T-1",
    undefined,
    parseTicketDetail,
    TICKET_BROWSER_API_LOAD_POLICY,
  );
  const missing = await load(wire);
  assertEquals(missing.data, null);
  assertEquals(missing.error, "Ticket Browser API: Ticket detail.initial_post must be an object");
  assertEquals(missing.status, undefined);
  const unknown = await load({ ...ticketDetailFixture(), [secret]: true });
  assertEquals(unknown.error, "Ticket Browser API: Ticket detail contains unknown field");
  const nested = ticketDetailFixture();
  const malformed = await load({ ...nested, initial_post: { ...nested.initial_post, attributes: { [secret]: 5 } } });
  assertEquals(malformed.error, "Ticket Browser API: Ticket detail.initial_post.attributes value must be a string");
  for (const result of [missing, unknown, malformed]) {
    assertEquals(result.error?.includes(secret), false);
  }
});

Deno.test("Ticket Browser parser accepts the generated Ticket list shape", () => {
  const parsed = parseTicketListResponse({
    workspace_id: "workspace-a",
    limit: 30,
    items: [{
      id: "ticket-id",
      resource_key: "T-1",
      title: "Ticket",
      tags: [],
      state: "open",
      priority: "P2",
      updated_at: null,
      queued_by: null,
      queued_at: null,
      workspace_action_priority: "ready",
      record_source: "sqlite",
    }],
    page,
    invalid_records: [],
    record_authority: "sqlite",
  });
  assertEquals(parsed.items[0]?.resource_key, "T-1");
});

Deno.test("Ticket Browser parser rejects unknown fields and out-of-range limits", () => {
  const fixture = {
    workspace_id: "workspace-a",
    limit: 30,
    items: [],
    page: { ...page, returned: 0 },
    invalid_records: [],
    record_authority: "sqlite",
  };
  assertThrows(
    () => parseTicketListResponse({ ...fixture, stale: true }),
    "unknown field",
  );
  assertThrows(
    () => parseTicketListResponse({ ...fixture, limit: 1001 }),
    "0 through 1000",
  );
});

Deno.test("Ticket Browser accepts generic Worker eligibility and assignments", () => {
  const fixture = ticketDetailFixture();
  const worker = {
    assignment_id: "assignment-1",
    runtime_id: "runtime-1",
    worker_id: "worker-1",
    worker_resource_key: "W-1",
  };
  const parsed = parseTicketDetail({
    ...fixture,
    current_worker: worker,
    action_eligibility: {
      ...fixture.action_eligibility,
      can_start_manual_worker: true,
    },
  });
  assertEquals(parsed.current_worker, worker);
  assertEquals(parsed.action_eligibility.can_start_manual_worker, true);

  const assignment = {
    assigned_at: "2026-10-06T00:00:00Z",
    assigned_by: "user-1",
    assignment_id: "assignment-1",
    principal: {
      kind: "worker",
      runtime_id: "runtime-1",
      worker_id: "worker-1",
    },
    role: "worker",
    ticket_id: "ticket-id",
    workspace_id: "workspace-a",
  };
  assertEquals(
    parseTicketRoleAssignmentMutationResponse({
      assignment,
      ticket_id: "ticket-id",
      workspace_id: "workspace-a",
    }).assignment,
    assignment,
  );
});

function ticketDetailFixture() {
  return {
    action_eligibility: {
      blockers: [],
      can_assign_orchestrator: false,
      can_queue: false,
      can_start_manual_worker: false,
      can_unassign_orchestrator: false,
      queue_tickets: [],
    },
    artifact_count: 0,
    artifacts: [],
    assignment_diagnostics: [],
    assignment_tracking: [],
    assignments: [],
    closure_history: [],
    closure_operation_key: null,
    current_closure: null,
    legacy_handoff: null,
    created_by: null,
    initial_post: fixturePost("Body"),
    body: "Body",
    body_truncated: false,
    current_worker: null,
    event_count: 0,
    event_page: { ...page, returned: 0 },
    events: [],
    evidence: {
      approved_current_subject: false,
      complete_for_integration: false,
      has_commit: false,
      has_current_subject_ref: false,
      has_merge_request: false,
      has_review_request: false,
      missing: [],
      review_after_rescope: false,
      review_status: null,
      unresolved_request_changes: false,
    },
    id: "ticket-id",
    implementation_reports: [],
    content_digest: "a".repeat(64),
    linked_objectives: [],
    merge_request: null,
    merge_requests: [],
    priority: "P2",
    queued_at: null,
    queued_by: null,
    readiness: null,
    record_source: "sqlite",
    relations: { blockers: [], incoming: [], notices: [], outgoing: [] },
    resolution: null,
    resource_key: "T-1",
    risk_flags: [],
    state: "open",
    tags: [],
    targets: [
      {
        repository_key: "docs",
        ref_selector: "main",
        access: "read_only",
      },
      {
        repository_key: "main",
        ref_selector: "develop",
        access: "read_write",
      },
    ],
    title: "Ticket",
  };
}

Deno.test("Ticket Browser keeps self-reported delegation independent of case and Worker state", () => {
  const tracking = {
    workspace_id: "workspace-a",
    ticket_id: "ticket-id",
    assignment_id: "delegation",
    role: "orchestrator",
    principal: { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
    assigned_by: "workspace-user:alice",
    assigned_at: "2026-01-01T00:00:00Z",
    release: null,
    current: true,
    last_event_id: "report-event",
    updated_at: null,
    migration_diagnostic: null,
    tracking: {
      status: "done",
      waiting_reason: null,
      progress: "Investigated",
      next_action: "Human decision",
      result: "Alternatives returned",
    },
  };
  for (const state of ["open", "closed"]) {
    const parsed = parseTicketDetail({
      ...ticketDetailFixture(),
      state,
      assignment_tracking: [tracking],
    });
    assertEquals(parsed.assignment_tracking[0]?.tracking?.status, "done");
    assertEquals(parsed.assignment_tracking[0]?.last_event_id, "report-event");
    assertEquals(parsed.state, state);
    assertEquals(parsed.current_worker, null);
  }
  assertThrows(
    () =>
      parseTicketDetail({
        ...ticketDetailFixture(),
        assignment_tracking: [{
          ...tracking,
          tracking: { ...tracking.tracking, status: "running" },
        }],
      }),
    ".status is invalid",
  );
});

Deno.test("Ticket Browser parser accepts target collections and rejects singular target authority", () => {
  const fixture = ticketDetailFixture();
  const parsed = parseTicketDetail(fixture);
  assertEquals(parsed.targets.length, 2);
  assertEquals(parsed.targets[0]?.repository_key, "docs");
  assertEquals(parsed.targets[0]?.access, "read_only");
  assertEquals(parsed.targets[1]?.repository_key, "main");
  assertEquals(parsed.targets[1]?.access, "read_write");
  assertThrows(
    () =>
      parseTicketDetail({
        ...fixture,
        targets: [{ ...fixture.targets[0], access: "write" }],
      }),
    "targets[0].access is invalid",
  );
  assertThrows(
    () =>
      parseTicketDetail({
        ...fixture,
        repository_key: "main",
        ref_selector: "develop",
      }),
    "unknown field",
  );
});

const mergeRequestSummary = {
  merge_request_id: "mr-1",
  repository_key: "main",
  state: "open",
  selector_from: "work/T-1",
  selector_to: "develop",
  updated_at: "2026-10-08T00:00:00Z",
  current_subject_ref: "source-1",
  source_ref_observation: { status: "observed" },
  integration_evidence_error: null,
  review_status: "approved",
  review_subject_ref: "source-1",
  review_requested_at: null,
  review_submitted_at: null,
  review_excerpt: null,
};

function mergeRequestListFixture(summary: unknown) {
  return {
    items: [{ summary, ticket_ids: ["ticket-id"], thread_event_count: 2 }],
    next_cursor: null,
  };
}

Deno.test("Merge Request summaries preserve live observation and immutable merged evidence separately", () => {
  for (
    const summary of [
      mergeRequestSummary,
      {
        ...mergeRequestSummary,
        current_subject_ref: null,
        source_ref_observation: {
          status: "unavailable",
          code: "repository_unavailable",
        },
        review_status: "pending",
      },
      {
        ...mergeRequestSummary,
        state: "merged",
        source_ref_observation: { status: "not_required" },
      },
      {
        ...mergeRequestSummary,
        state: "merged",
        source_ref_observation: { status: "not_required" },
        integration_evidence_error: "approval_event_missing",
        review_status: "none",
      },
    ]
  ) {
    const listed = parseMergeRequestListResponse(
      mergeRequestListFixture(summary),
    );
    const detailed = parseTicketDetail({
      ...ticketDetailFixture(),
      merge_requests: [summary],
      merge_request: summary,
    });
    for (
      const parsed of [
        listed.items[0]?.summary,
        detailed.merge_requests[0],
        detailed.merge_request,
      ]
    ) {
      assertEquals(
        parsed?.source_ref_observation,
        summary.source_ref_observation,
      );
      assertEquals(parsed?.current_subject_ref, summary.current_subject_ref);
      assertEquals(
        parsed?.integration_evidence_error,
        summary.integration_evidence_error,
      );
      assertEquals(parsed?.review_subject_ref, summary.review_subject_ref);
    }
  }
});

Deno.test("Merge Request summaries reject missing and malformed tagged source observations", () => {
  for (
    const [observation, message] of [
      [undefined, "source_ref_observation must be an object"],
      [null, "source_ref_observation must be an object"],
      ["observed", "source_ref_observation must be an object"],
      [{ status: "unknown" }, "status is invalid"],
      [{ status: "unavailable" }, "code must be a string"],
      [{ status: "unavailable", code: 42 }, "code must be a string"],
      [
        { status: "unavailable", code: "x".repeat(262_145) },
        "code exceeds its length limit",
      ],
      [{ status: "observed", code: "stale" }, "unknown field"],
      [{ status: "not_required", code: "stale" }, "unknown field"],
      [
        { status: "unavailable", code: "missing", extra: true },
        "unknown field",
      ],
    ] as const
  ) {
    assertThrows(
      () =>
        parseMergeRequestListResponse(mergeRequestListFixture({
          ...mergeRequestSummary,
          source_ref_observation: observation,
        })),
      message,
    );
  }
});

Deno.test("Merge Request summaries require a nullable bounded integration evidence error", () => {
  for (
    const [error, message] of [
      [undefined, "integration_evidence_error must be a string"],
      [false, "integration_evidence_error must be a string"],
      [
        "x".repeat(262_145),
        "integration_evidence_error exceeds its length limit",
      ],
    ] as const
  ) {
    assertThrows(
      () =>
        parseMergeRequestListResponse(mergeRequestListFixture({
          ...mergeRequestSummary,
          integration_evidence_error: error,
        })),
      message,
    );
  }
});

Deno.test("closed Ticket state is preserved when current requirement evidence is incomplete", () => {
  const fixture = ticketDetailFixture();
  const parsed = parseTicketDetail({
    ...fixture,
    state: "closed",
    evidence: { ...fixture.evidence, missing: ["review_after_rescope"] },
    merge_requests: [{
      ...mergeRequestSummary,
      state: "merged",
      source_ref_observation: { status: "not_required" },
    }],
  });
  assertEquals(parsed.state, "closed");
  assertEquals(parsed.evidence.complete_for_integration, false);
  assertEquals(parsed.merge_requests[0]?.review_status, "approved");
});

Deno.test("Ticket Browser parser validates Ticket create responses", () => {
  const parsed = parseTicketRecordRef({
    id: "ticket-id",
    resource_key: "T-1",
    slug: "ticket",
    status: "Open",
  });
  assertEquals(parsed.resource_key, "T-1");
  assertThrows(
    () => parseTicketRecordRef({ ...parsed, status: "open" }),
    "status is invalid",
  );
});

Deno.test("Ticket Browser parser accepts the generated Objective list shape", () => {
  const parsed = parseObjectiveListResponse({
    workspace_id: "workspace-a",
    limit: 100,
    items: [{
      id: "objective-id",
      resource_key: "O-1",
      title: "Objective",
      state: "active",
      created_at: null,
      updated_at: null,
      summary: "Summary",
      linked_tickets: ["T-1"],
      record_source: "sqlite",
    }],
    invalid_records: [],
    record_authority: "sqlite",
  });
  assertEquals(parsed.items[0]?.resource_key, "O-1");
});

Deno.test("Ticket Browser parser validates the canonical Merge Request thread", () => {
  const parsed = parseMergeRequestDetailResponse({
    workspace_id: "workspace-a",
    merge_request_id: "mr-1",
    repository_key: "main",
    state: "open",
    selector_from: "work/T-1",
    selector_to: "develop",
    ticket_ids: ["ticket-id"],
    created_at: "2026-09-22T00:00:00Z",
    updated_at: "2026-09-22T00:00:00Z",
    thread: [{
      kind: "review_requested",
      event_id: "event-1",
      sequence: 1,
      subject_ref: "abc123",
      ticket_content_digest: "b".repeat(64),
      ticket_merge_request_subjects: [
        { merge_request_id: "mr-1", subject_ref: "abc123" },
      ],
      requested_by: { runtime_id: "runtime-a", worker_id: "worker-a" },
      reviewer: { runtime_id: "runtime-a", worker_id: "reviewer-a" },
      created_at: "2026-09-22T00:00:00Z",
    }],
    source: {
      status: "resolved",
      ref: "abc123",
      observed_at: "2026-09-22T00:00:00Z",
    },
    target: {
      status: "resolved",
      ref: "def456",
      observed_at: "2026-09-22T00:00:00Z",
    },
    linked_tickets: [{ ticket_id: "ticket-id", key: "T-1" }],
  });
  assertEquals(parsed.thread[0]?.kind, "review_requested");
  assertThrows(
    () =>
      parseMergeRequestDetailResponse({
        ...parsed,
        thread: [{ ...parsed.thread[0], unexpected: true }],
      }),
    "unknown field",
  );
});

const tags = ["ready", "closed"].map((name) => ({
  workspace_id: "workspace-a",
  id: `tag-${name}`,
  name,
  last_operation_key: `create-${name}`,
  deleted: false,
}));

const closure = {
  operation_key: "close-1",
  reason: "not_planned",
  explanation: "Not planned for this scope.",
  references: [{ kind: "document", target: "result.md" }],
  author: "user:alice",
  at: "2026-10-10T00:00:00Z",
};
const closureHistory = [
  {
    ...closure,
    reason: undefined,
    previous_closure_operation_key: null,
    action: { action: "close", reason: "not_planned" },
  },
  {
    ...closure,
    reason: undefined,
    operation_key: "correct-1",
    previous_closure_operation_key: "close-1",
    action: { action: "correct_closure", reason: { other: "Deferred" } },
  },
  {
    ...closure,
    reason: undefined,
    operation_key: "reopen-1",
    previous_closure_operation_key: "correct-1",
    action: { action: "reopen" },
  },
].map(({ reason: _, ...entry }) => ({
  operation_key: entry.operation_key,
  previous_closure_operation_key: entry.previous_closure_operation_key,
  action: entry.action,
  explanation: entry.explanation,
  references: entry.references,
  author: entry.author,
  at: entry.at,
}));
const legacyHandoff = {
  workflow_state: "done",
  workflow_state_explicit: true,
  status: "Closed",
  queued_by: "workspace-web",
  queued_at: "2026-10-09T00:00:00Z",
  resolution: "Historical result, not a current case state",
};

Deno.test("tags named ready and closed coexist with case judgment and retained reopen history without inferring state", () => {
  const fixture = {
    ...ticketDetailFixture(),
    state: "open",
    tags,
    closure_operation_key: "correct-1",
    current_closure: null,
    closure_history: closureHistory,
    legacy_handoff: legacyHandoff,
  };
  const parsed = parseTicketDetail(fixture);
  assertEquals(parsed.state, "open");
  assertEquals(parsed.closure_operation_key, "correct-1");
  assertEquals(parsed.closure_history, closureHistory);
  assertEquals(parsed.legacy_handoff, legacyHandoff);
  assertEquals(parsed.tags, tags);
  const closed = parseTicketDetail({
    ...fixture,
    state: "closed",
    current_closure: closure,
  });
  assertEquals(closed.state, "closed");
  assertEquals(closed.current_closure, closure);
  assertEquals(closed.closure_history, closureHistory);
  assertEquals(closed.tags, tags);
  const list = parseTicketListResponse({
    workspace_id: "workspace-a",
    limit: 30,
    page,
    invalid_records: [],
    record_authority: "sqlite",
    items: [{
      id: "ticket-id",
      resource_key: "T-1",
      title: "Ticket",
      state: "closed",
      tags,
      priority: "P2",
      record_source: "sqlite",
      workspace_action_priority: "background",
      closure_operation_key: "close-1",
      current_closure: closure,
      legacy_handoff: legacyHandoff,
    }],
  });
  assertEquals(list.items[0].state, "closed");
  assertEquals(list.items[0].tags, tags);
  assertEquals(list.items[0].current_closure, closure);
  assertEquals(list.items[0].legacy_handoff, legacyHandoff);
});

Deno.test("Ticket tag projections reject missing collections and malformed identities, types and unknown fields", () => {
  const parsers = [
    (tags: unknown) => parseTicketDetail({ ...ticketDetailFixture(), tags }),
    (tags: unknown) =>
      parseTicketSummary({
        id: "ticket-id",
        slug: "ticket",
        title: "Ticket",
        kind: "task",
        labels: [],
        priority: "P2",
        status: "Open",
        workflow_state: "open",
        workflow_state_explicit: true,
        tags,
      }),
    (tags: unknown) =>
      parseTicketListResponse({
        workspace_id: "workspace-a",
        limit: 30,
        page,
        invalid_records: [],
        record_authority: "sqlite",
        items: [{
          id: "ticket-id",
          resource_key: "T-1",
          title: "Ticket",
          state: "open",
          priority: "P2",
          record_source: "sqlite",
          workspace_action_priority: "background",
          tags,
        }],
      }),
  ];
  const tag = tags[0];
  for (const parse of parsers) {
    for (const value of [undefined, null, {}, "ready"]) {
      assertThrows(() => parse(value), "tags must be an array");
    }
    for (
      const [value, message] of [
        [{ ...tag, id: undefined }, "id must be a string"],
        [{ ...tag, id: 1 }, "id must be a string"],
        [{ ...tag, id: " " }, "id must be nonempty"],
        [{ ...tag, id: "é".repeat(65) }, "at most 128 UTF-8 bytes"],
        [{ ...tag, workspace_id: false }, "workspace_id must be a string"],
        [{ ...tag, name: null }, "name must be a string"],
        [
          { ...tag, last_operation_key: 1 },
          "last_operation_key must be a string",
        ],
        [{ ...tag, deleted: "false" }, "deleted must be a boolean"],
        [{ ...tag, state: "closed" }, "unknown field"],
      ] as const
    ) {
      assertThrows(() => parse([value]), message);
    }
  }
});

Deno.test("case decoders reject legacy current states and malformed closure judgments", () => {
  const fixture = ticketDetailFixture();
  for (const state of ["planning", "ready", "queued", "inprogress", "done"]) {
    assertThrows(
      () => parseTicketDetail({ ...fixture, state }),
      "state is invalid",
    );
  }
  for (const reason of ["done", { other: 1 }, { other: "ok", extra: true }]) {
    assertThrows(
      () =>
        parseTicketDetail({
          ...fixture,
          current_closure: { ...closure, reason },
        }),
      "Ticket detail.current_closure.reason",
    );
  }
  for (
    const action of ["reopen", { action: "close" }, {
      action: "reopen",
      reason: "completed",
    }, { action: "complete", reason: "completed" }]
  ) {
    assertThrows(
      () =>
        parseTicketDetail({
          ...fixture,
          closure_history: [{ ...closureHistory[0], action }],
        }),
      "Ticket detail.closure_history[0].action",
    );
  }
  assertThrows(
    () => parseTicketDetail({ ...fixture, closure_history: {} }),
    "closure_history must be an array",
  );
  assertThrows(
    () => parseTicketDetail({ ...fixture, closure_operation_key: 1 }),
    "closure_operation_key must be a string",
  );
});

Deno.test("Subject assignment projection preserves identity and legacy audit without singleton normalization", () => {
  const base = {
    workspace_id: "w",
    ticket_id: "t",
    assignment_id: "a",
    role: "orchestrator",
    assigned_by: "Mika",
    assigned_at: "2026-01-01T00:00:00Z",
    current: true,
    tracking: { status: "waiting" },
    last_event_id: "e",
  };
  for (
    const principal of [
      { kind: "subject", subject_id: "subject-a" },
      { kind: "subject", subject_id: "subject-b" },
      { kind: "user", account_id: "alice" },
      { kind: "worker", runtime_id: "r", worker_id: "w" },
      { kind: "workspace_agent", agent_key: "workspace-orchestrator" },
    ]
  ) {
    assertEquals(
      parseAssignmentTrackingProjection({ ...base, principal }, "assignment")
        .principal,
      principal,
    );
  }
  for (
    const principal of [
      { kind: "subject" },
      { kind: "subject", subject_id: 3 },
      { kind: "subject", subject_id: "a", worker_id: "w" },
    ]
  ) {
    assertThrows(
      () =>
        parseAssignmentTrackingProjection({ ...base, principal }, "assignment"),
      "assignment.principal",
    );
  }
});
