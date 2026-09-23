declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

import {
  parseMergeRequestDetailResponse,
  parseObjectiveListResponse,
  parseTicketDetail,
  parseTicketListResponse,
  parseTicketRecordRef,
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

Deno.test("Ticket Browser parser accepts the generated Ticket list shape", () => {
  const parsed = parseTicketListResponse({
    workspace_id: "workspace-a",
    limit: 30,
    items: [{
      id: "ticket-id",
      resource_key: "T-1",
      title: "Ticket",
      state: "ready",
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
    "unknown field stale",
  );
  assertThrows(
    () => parseTicketListResponse({ ...fixture, limit: 1001 }),
    "0 through 1000",
  );
});

Deno.test("Ticket Browser parser accepts target collections and rejects singular target authority", () => {
  const fixture = {
    action_eligibility: {
      blockers: [],
      can_assign_orchestrator: false,
      can_queue: false,
      can_start_manual_coder: false,
      can_unassign_orchestrator: false,
      queue_tickets: [],
    },
    artifact_count: 0,
    artifacts: [],
    assignment_diagnostics: [],
    assignments: [],
    body: "Body",
    body_truncated: false,
    current_coder: null,
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
    item_revision: "revision-1",
    linked_objectives: [],
    merge_request: null,
    priority: "P2",
    queued_at: null,
    queued_by: null,
    readiness: null,
    record_source: "sqlite",
    relations: { blockers: [], incoming: [], notices: [], outgoing: [] },
    resolution: null,
    resource_key: "T-1",
    risk_flags: [],
    state: "planning",
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
    "unknown field repository_key",
  );
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
    "unknown field unexpected",
  );
});
