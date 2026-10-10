declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

import {
  parseMergeRequestDetailResponse,
  parseMergeRequestListResponse,
  parseObjectiveListResponse,
  parseTicketDetail,
  parseTicketListResponse,
  parseTicketRecordRef,
  parseTicketRoleAssignmentMutationResponse,
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
    principal: { kind: "worker", runtime_id: "runtime-1", worker_id: "worker-1" },
    role: "worker",
    ticket_id: "ticket-id",
    workspace_id: "workspace-a",
  };
  assertEquals(parseTicketRoleAssignmentMutationResponse({
    assignment,
    ticket_id: "ticket-id",
    workspace_id: "workspace-a",
  }).assignment, assignment);
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
    assignments: [],
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
}

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
    "unknown field repository_key",
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
      [{ status: "observed", code: "stale" }, "unknown field code"],
      [{ status: "not_required", code: "stale" }, "unknown field code"],
      [
        { status: "unavailable", code: "missing", extra: true },
        "unknown field extra",
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

Deno.test("done Ticket state is preserved when current requirement evidence is incomplete", () => {
  const fixture = ticketDetailFixture();
  const parsed = parseTicketDetail({
    ...fixture,
    state: "done",
    evidence: { ...fixture.evidence, missing: ["review_after_rescope"] },
    merge_requests: [{
      ...mergeRequestSummary,
      state: "merged",
      source_ref_observation: { status: "not_required" },
    }],
  });
  assertEquals(parsed.state, "done");
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
    "unknown field unexpected",
  );
});
