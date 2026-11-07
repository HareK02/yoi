declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function ordered(value: unknown): unknown {
  return Array.isArray(value)
    ? value.map(ordered)
    : value && typeof value === "object"
    ? Object.fromEntries(
      Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map((
        [key, item],
      ) => [key, ordered(item)]),
    )
    : value;
}
function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(ordered(actual)) !== JSON.stringify(ordered(expected))) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}
function assertThrows(operation: () => unknown): void {
  try {
    operation();
  } catch {
    return;
  }
  throw new Error("Expected rejection");
}
function assertStringIncludes(actual: string, expected: string): void {
  if (!actual.includes(expected)) {
    throw new Error(`${actual} does not include ${expected}`);
  }
}
import type {
  TicketAssignmentTrackingProjection,
  TicketDetail,
} from "#lib/generated/ticket-api.ts";
import {
  answerOperation,
  assignmentOperation,
  observeOperation,
  operationError,
  parseAnswerReceipt,
  parseAssignmentDiagnostics,
  parseAssignmentsPage,
  parseTrackingHistory,
} from "./ticket-operations.ts";

const current: TicketAssignmentTrackingProjection = {
  workspace_id: "team one",
  ticket_id: "ticket/one",
  assignment_id: "assignment-1",
  role: "orchestrator",
  current: true,
  principal: { kind: "subject", subject_id: "subject-a" },
  assigned_by: "account/user",
  assigned_at: "2026-01-01T00:00:00Z",
  tracking: {
    status: "waiting",
    waiting_reason: "User decision needed",
    next_action: "Read answer",
  },
  last_event_id: "event-1",
  updated_at: "2026-01-02T00:00:00Z",
};
function observed(): Pick<TicketDetail, "assignments"> {
  return {
    assignments: [{
      assignment_id: current.assignment_id,
      role: "orchestrator",
      principal: current.principal,
      assigned_at: current.assigned_at,
      assigned_by: current.assigned_by,
    }],
  };
}
Deno.test("assignment retry retains the exact observed pointer, operation key and payload after new data arrives", () => {
  const old = observed();
  const operation = assignmentOperation(
    "team one",
    "ticket/one",
    old,
    "operation-1",
    false,
    { kind: "subject", subject_id: "subject-a" },
  );
  old.assignments[0].assignment_id = "replacement";
  assertEquals(JSON.parse(operation.body!), {
    operation_id: "operation-1",
    expected_assignment_id: "assignment-1",
    principal: { kind: "subject", subject_id: "subject-a" },
  });
  assertEquals(
    operation.path,
    "/api/w/team%20one/tickets/ticket%2Fone/assignments/orchestrator",
  );
  assertEquals(Object.isFrozen(operation), true);
  const fresh = assignmentOperation(
    "team one",
    "ticket/one",
    old,
    "operation-2",
    false,
    { kind: "subject", subject_id: "subject-b" },
  );
  assertEquals(JSON.parse(fresh.body!).expected_assignment_id, "replacement");
  const clear = assignmentOperation(
    "team one",
    "ticket/one",
    old,
    "release",
    true,
  );
  assertStringIncludes(
    clear.path,
    "operation_id=release&assignment_id=replacement",
  );
  assertEquals(clear.method, "DELETE");
  assertThrows(() =>
    assignmentOperation("team", "ticket", { assignments: [] }, "clear", true)
  );
});
Deno.test("waiting answers carry the same assignment and observed event without status or Assign mutation", () => {
  const operation = answerOperation(
    "team one",
    "ticket/one",
    current,
    "My answer",
    "answer-key",
  );
  assertEquals(JSON.parse(operation.body!), {
    assignment_id: "assignment-1",
    expected_last_event_id: "event-1",
    body: "My answer",
    operation_key: "answer-key",
  });
  assertEquals(
    operation.path,
    "/api/w/team%20one/tickets/ticket%2Fone/assignment-answer",
  );
  for (
    const unavailable of [{ ...current, current: false }, {
      ...current,
      last_event_id: null,
    }, { ...current, tracking: { status: "done" as const } }]
  ) {
    assertThrows(() =>
      answerOperation("team", "ticket", unavailable, "answer", "new-op")
    );
  }
});
Deno.test("operation snapshots are serialized once rather than silently rebased for exact retry", () => {
  const body = {
    operation_key: "saved",
    tag_ids: ["stable-id"],
    expected_tag_ids: [] as string[],
  };
  const operation = observeOperation("/tags", "PUT", body);
  body.tag_ids[0] = "renamed-id";
  assertEquals(JSON.parse(operation.body!).tag_ids, ["stable-id"]);
});
Deno.test("delivery diagnostics keep saved notification, acceptance uncertainty and actual processing separate", () => {
  const wire = {
    assignment_id: "assignment-1",
    current: true,
    input_origin_known: true,
    actionable: true,
    inputs: [{
      input_event_id: "input-1",
      delivery_id: "delivery-1",
      notification: "request_persisted" as const,
      acceptance_outcome_unknown: true,
    }],
    starts: [{
      operation_id: "start-1",
      claim_state: "pending",
      assignment_id: null,
      runtime_id: null,
      worker_id: null,
      actual_processing_unknown: true,
    }],
    next_input_event_id: null,
    next_start_operation_id: null,
  };
  assertEquals(parseAssignmentDiagnostics(wire), wire);
  assertThrows(() =>
    parseAssignmentDiagnostics({ ...wire, rendered_content: "must not leak" })
  );
  assertThrows(() =>
    parseAssignmentDiagnostics({
      ...wire,
      inputs: [{ ...wire.inputs[0], notification: "done" }],
    })
  );
  assertThrows(() =>
    parseAssignmentDiagnostics({
      ...wire,
      inputs: Array(101).fill(wire.inputs[0]),
    })
  );
});
Deno.test("retained assignment page preserves result and release rather than rewriting handling to done", () => {
  const row = {
    ...current,
    current: false,
    release: {
      action: "replaced",
      event_id: "release-event",
      actor: "account/user",
      at: current.assigned_at,
      replacement_assignment_id: "assignment-2",
      last_tracking: current.tracking,
    },
  };
  const page = parseAssignmentsPage({
    items: [row],
    next_assignment_id: "assignment-1",
  });
  assertEquals(page.items[0].tracking?.status, "waiting");
  assertEquals(page.items[0].release?.action, "replaced");
  assertThrows(() =>
    parseAssignmentsPage({
      items: Array(101).fill(row),
      next_assignment_id: null,
    })
  );
  assertThrows(() =>
    parseAssignmentsPage({
      items: [row],
      next_assignment_id: null,
      guessed_status: "done",
    })
  );
  assertThrows(() => parseTrackingHistory({ items: [], next_event_id: 1 }));
});
Deno.test("answer receipt rejects unsafe event indices and extra inferred result fields", () => {
  assertEquals(
    parseAnswerReceipt({
      assignment_id: "a",
      input_event_id: "event:1",
      event_index: 1,
    }),
    { assignment_id: "a", input_event_id: "event:1", event_index: 1 },
  );
  assertThrows(() =>
    parseAnswerReceipt({
      assignment_id: "a",
      input_event_id: "event:1",
      event_index: 2 ** 54,
    })
  );
  assertThrows(() =>
    parseAnswerReceipt({
      assignment_id: "a",
      input_event_id: "event:1",
      event_index: 1,
      status: "inprogress",
    })
  );
});
Deno.test("conflict, permission, unavailable and unknown errors never claim successful new delegation", () => {
  assertStringIncludes(
    operationError(new Error("request failed (409)")),
    "Stale observation",
  );
  assertStringIncludes(
    operationError(new Error("request failed (403)")),
    "Permission denied",
  );
  assertStringIncludes(
    operationError(new Error("request failed (404)")),
    "unavailable",
  );
  assertStringIncludes(
    operationError(new Error("network response lost")),
    "Outcome unknown",
  );
});

Deno.test("new Assign requires an explicit Subject; legacy assignments remain releasable audit", () => {
  assertThrows(() =>
    assignmentOperation("team", "ticket", observed(), "missing")
  );
  assertThrows(() =>
    assignmentOperation("team", "ticket", observed(), "empty", false, {
      kind: "subject",
      subject_id: " ",
    })
  );
  for (
    const principal of [{ kind: "user", account_id: "alice" }, {
      kind: "worker",
      runtime_id: "r",
      worker_id: "w",
    }, {
      kind: "workspace_agent",
      agent_key: "workspace-orchestrator",
    }] as const
  ) {
    assertThrows(() =>
      answerOperation("team", "ticket", { ...current, principal }, "old", "op")
    );
    const operation = assignmentOperation(
      "team",
      "ticket",
      { assignments: [{ ...observed().assignments[0], principal }] },
      "release",
      true,
    );
    assertEquals(operation.method, "DELETE");
    assertStringIncludes(operation.path, "assignment_id=assignment-1");
  }
});
