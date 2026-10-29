import {
  closureReasonLabel,
  currentOrchestratorAssignment,
  handlingLabel,
  handlingSummary,
  readTicketFilters,
  ticketQueryRequest,
  ticketWorkerLaunchHref,
  ticketWorkerMessage,
} from "./ticket-panel.ts";
import type { TicketDetail } from "../../generated/ticket-api.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("generic Ticket launch carries natural text without role or resource claims", () => {
  const requests = [
    "Review is unnecessary; complete the requested work.",
    "Review the change, then merge it if approved.",
    "Investigate and return without a conclusion.",
  ];
  const targetSets: TicketDetail["targets"][] = [
    [],
    [{ repository_key: "docs", ref_selector: "main", access: "read_only" }],
    [{ repository_key: "repo", ref_selector: "main", access: "read_write" }],
  ];
  assertEquals(ticketWorkerMessage("T-720"), "Work on Ticket T-720.");
  for (const targets of targetSets) {
    const ticket = { id: "T-720", title: "Flexible launch", targets };
    for (const request of requests) {
      const url = new URL(
        ticketWorkerLaunchHref("workspace one", ticket, request),
        "https://example.test",
      );
      assertEquals(url.pathname, "/w/workspace%20one/workers/new");
      assertEquals(url.searchParams.get("ticketId"), ticket.id);
      assertEquals(url.searchParams.get("ticketTitle"), ticket.title);
      assertEquals(url.searchParams.get("initialInput"), request);
      for (
        const field of [
          "ticketRole",
          "profile",
          "repositoryKey",
          "refSelector",
          "flow",
        ]
      ) {
        assertEquals(url.searchParams.has(field), false);
      }
    }
  }
});

Deno.test("case, stable tags and principal filters survive every server-query page", () => {
  const filters = readTicketFilters(
    new URLSearchParams(
      "q=options&case=open&tag=tag-1&tag=tag-2&principal=subject&subject=subject-a",
    ),
  );
  for (const cursor of [null, "next-page"]) {
    const request = JSON.parse(
      JSON.stringify(ticketQueryRequest(filters, ["open"], cursor)),
    );
    assertEquals(request, {
      query: "options",
      states: ["open"],
      tag_ids: ["tag-1", "tag-2"],
      assigned_principal: {
        kind: "subject",
        subject_id: "subject-a",
      },
      handling_status: null,
      sort: "updated_desc",
      limit: 30,
      cursor,
    });
    const handling = JSON.parse(
      JSON.stringify(ticketQueryRequest(filters, [], cursor, "done")),
    );
    assertEquals(handling.handling_status, "done");
    assertEquals(handling.states, []);
  }
});

Deno.test("current close-reason queries preserve known and custom reasons on every page without changing case intent", () => {
  for (
    const [input, expected] of [["completed", "completed"], [
      "other&reason_text=Superseded%20plan",
      { other: "Superseded plan" },
    ]] as const
  ) {
    const filters = readTicketFilters(
      new URLSearchParams(`case=closed&reason=${input}`),
    );
    for (const cursor of [null, "next-page"]) {
      const request = JSON.parse(
        JSON.stringify(ticketQueryRequest(filters, ["closed"], cursor)),
      );
      assertEquals(request.closure_reason, expected);
      assertEquals(request.states, ["closed"]);
      assertEquals(request.cursor, cursor);
    }
  }
});

Deno.test("handling summary uses only the current self-report and never Worker or case status", () => {
  const row = {
    assignment_id: "a",
    current: true,
    role: "orchestrator",
    tracking: { status: "waiting", waiting_reason: "Awaiting user choice" },
  } as TicketDetail["assignment_tracking"][number];
  for (const state of ["open", "closed"] as const) {
    for (const workerState of ["Running", "Stopped"]) {
      const value = {
        state,
        workerState,
        assignment_tracking: [{
          ...row,
          assignment_id: "released",
          current: false,
          tracking: { status: "done" as const, result: "Old result" },
        }, row],
      };
      const current = currentOrchestratorAssignment(value);
      assertEquals(current?.assignment_id, "a");
      assertEquals(handlingLabel(current?.tracking?.status), "Waiting");
      assertEquals(handlingSummary(current), "Awaiting user choice");
    }
  }
  assertEquals(
    handlingSummary({
      ...row,
      tracking: { status: "done", result: "Compared options" },
    }),
    "Compared options",
  );
  assertEquals(
    handlingSummary({
      ...row,
      tracking: null,
      migration_diagnostic: "Unknown historical handling",
    }),
    "Unknown historical handling",
  );
  assertEquals(handlingLabel(null), "Unknown");
  assertEquals(
    currentOrchestratorAssignment({
      assignment_tracking: [{ ...row, current: false }],
    }),
    null,
  );
  assertEquals(handlingSummary(null), "Not assigned");
  assertEquals(closureReasonLabel({ other: "Superseded" }), "Superseded");
});

Deno.test("Ticket authoring creates the target collection without singular authority", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../../routes/w/[workspaceId]/tickets/PageContent.svelte",
      import.meta.url,
    ),
  );

  assertEquals(source.includes("const request: NewTicket"), true);
  assertEquals(source.includes("targets: normalizedCreateTargets()"), true);
  assertEquals(source.includes('value="read_write"'), true);
  assertEquals(source.includes('value="read_only"'), true);
  assertEquals(source.includes("repository_key: create"), false);
  assertEquals(source.includes("ref_selector: create"), false);
});

Deno.test("ticket detail keeps the operation rail outside main content", async () => {
  const css = await Deno.readTextFile(
    new URL("../styles/tickets.css", import.meta.url),
  );

  assertEquals(css.includes("container: ticket-detail / inline-size;"), true);
  assertEquals(
    css.includes(".ticket-detail-main {\n    overflow-wrap: anywhere;"),
    true,
  );
  assertEquals(
    css.includes("@container ticket-detail (max-width: 48rem)"),
    true,
  );
  assertEquals(css.includes("min-width: 0;"), true);
});

Deno.test("Subject identity survives every query page without singleton normalization", () => {
  for (const subject of ["subject-a", "subject-b"]) {
    const filters = readTicketFilters(
      new URLSearchParams({ principal: "subject", subject }),
    );
    for (const cursor of [null, "next"]) {
      assertEquals(ticketQueryRequest(filters, [], cursor).assigned_principal, {
        kind: "subject",
        subject_id: subject,
      });
    }
  }
  const legacy = readTicketFilters(
    new URLSearchParams("principal=orchestrator"),
  );
  assertEquals(ticketQueryRequest(legacy, []).assigned_principal, {
    kind: "workspace_agent",
    agent_key: "workspace-orchestrator",
  });
});
