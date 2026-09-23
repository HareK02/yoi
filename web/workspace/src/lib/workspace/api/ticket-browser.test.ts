declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

import {
  parseMergeRequestDetailResponse,
  parseObjectiveListResponse,
  parseTicketListResponse,
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
