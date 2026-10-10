declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};
function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, received ${
        JSON.stringify(actual)
      }`,
    );
  }
}
async function assertRejects(operation: () => Promise<unknown>): Promise<void> {
  try {
    await operation();
  } catch (error) {
    if (error instanceof Error && error.message.includes("409")) return;
    throw error;
  }
  throw new Error("Expected stale CAS rejection");
}
import type { TicketCaseAction } from "#lib/generated/ticket-api.ts";
import { ticketCaseRequest, updateTicketCase } from "./ticket-case.ts";

const observed = {
  content_digest: "a".repeat(64),
  state: "open",
  closure_operation_key: "last-correction",
} as const;
const summary = {
  id: "ticket-1",
  resource_key: "T-1",
  slug: "case",
  title: "Case",
  kind: "task",
  labels: [],
  priority: "P2",
  status: "Closed",
  workflow_state: "closed",
  workflow_state_explicit: true,
};

Deno.test("case actions carry content/state/last-closure CAS without browser author or unrelated lifecycle fields", async () => {
  const actions: TicketCaseAction[] = [
    ...["completed", "not_planned", "duplicate", "unknown", {
      other: "Deferred",
    }].map((reason) => ({ action: "close", reason } as TicketCaseAction)),
    { action: "reopen" },
    { action: "correct_closure", reason: "unknown" },
  ];
  const original = globalThis.fetch;
  try {
    for (const action of actions) {
      const request = ticketCaseRequest(
        observed,
        action,
        "  Case judgment  ",
        "operation-1",
      );
      assertEquals(request, {
        expected_content_digest: observed.content_digest,
        expected_state: "open",
        expected_closure_operation_key: "last-correction",
        operation_key: "operation-1",
        action,
        explanation: "Case judgment",
        references: [],
      });
      globalThis.fetch = (input, init) => {
        assertEquals(input, "/workspaces/workspace%20one/tickets/T%2F1/case");
        const options = init as { method?: string; body?: string };
        assertEquals(options.method, "POST");
        assertEquals(JSON.parse(String(options.body)), request);
        return Promise.resolve(Response.json(summary));
      };
      assertEquals(
        (await updateTicketCase("workspace one", "T/1", request))
          .workflow_state,
        "closed",
      );
    }
  } finally {
    globalThis.fetch = original;
  }
  assertEquals(
    ticketCaseRequest(
      { ...observed, closure_operation_key: null },
      { action: "close", reason: "unknown" },
      "Close",
      "first",
    ).expected_closure_operation_key,
    null,
  );
});

Deno.test("case API propagates stale CAS failure rather than reporting success", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = () =>
    Promise.resolve(Response.json({ message: "stale case" }, { status: 409 }));
  try {
    await assertRejects(() =>
      updateTicketCase(
        "w",
        "T-1",
        ticketCaseRequest(
          observed,
          { action: "reopen" },
          "Reopen",
          "operation-1",
        ),
      )
    );
  } finally {
    globalThis.fetch = original;
  }
});
