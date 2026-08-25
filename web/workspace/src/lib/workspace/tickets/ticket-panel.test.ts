import {
  nextTicketLaneVisibleCount,
  TICKET_LANE_PAGE_SIZE,
  ticketLanes,
  ticketWorkerLaunchHref,
  ticketWorkerMessage,
} from "./ticket-panel.ts";
import type {
  TicketDetail,
  TicketSummary,
} from "../../generated/ticket-api.ts";

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

Deno.test("ticketLanes combines workflow states and sorts by state then update time", () => {
  const tickets = [
    {
      id: "planning",
      state: "planning",
      title: "Planning",
      updated_at: "2026-07-30T12:00:00Z",
    },
    {
      id: "ready-old",
      state: "ready",
      title: "Ready old",
      updated_at: "2026-07-29T12:00:00Z",
    },
    {
      id: "ready-new",
      state: "ready",
      title: "Ready new",
      updated_at: "2026-07-30T12:00:00Z",
    },
    { id: "queued", state: "queued", title: "Queued", updated_at: null },
    {
      id: "inprogress",
      state: "inprogress",
      title: "In progress",
      updated_at: null,
    },
    { id: "closed", state: "closed", title: "Closed", updated_at: null },
    { id: "done", state: "done", title: "Done", updated_at: null },
  ] as TicketSummary[];

  const lanes = ticketLanes(tickets);
  assertEquals(lanes.map((lane) => lane.id), [
    "ready-planning",
    "inprogress-queued",
    "done-closed",
  ]);
  assertEquals(lanes[0].tickets.map((ticket) => ticket.id), [
    "ready-new",
    "ready-old",
    "planning",
  ]);
  assertEquals(lanes[1].tickets.map((ticket) => ticket.id), [
    "inprogress",
    "queued",
  ]);
  assertEquals(lanes[2].tickets.map((ticket) => ticket.id), [
    "done",
    "closed",
  ]);
});

Deno.test("ticket lane visibility advances in bounded pages of 30", () => {
  assertEquals(TICKET_LANE_PAGE_SIZE, 30);
  assertEquals(nextTicketLaneVisibleCount(0, 95), 30);
  assertEquals(nextTicketLaneVisibleCount(30, 95), 60);
  assertEquals(nextTicketLaneVisibleCount(60, 95), 90);
  assertEquals(nextTicketLaneVisibleCount(90, 95), 95);
  assertEquals(nextTicketLaneVisibleCount(95, 95), 95);
});

Deno.test("ticket worker launch uses the common Worker route and bounded Ticket context", () => {
  const ticket = {
    id: "00001KYRRDVH9",
    title: "Ticket panel API",
    repository_id: "main repo",
    ref_selector: "work/ticket",
  } as TicketDetail;

  assertEquals(
    ticketWorkerMessage(ticket.id, "coder"),
    "Work on Ticket 00001KYRRDVH9 as its coder.",
  );

  const href = ticketWorkerLaunchHref("workspace one", ticket, "reviewer");
  const url = new URL(href, "https://example.test");
  assertEquals(url.pathname, "/w/workspace%20one/workers/new");
  assertEquals(url.searchParams.get("ticketId"), ticket.id);
  assertEquals(url.searchParams.get("ticketRole"), "reviewer");
  assertEquals(url.searchParams.get("repositoryId"), "main repo");
  assertEquals(url.searchParams.get("refSelector"), "work/ticket");
  assertEquals(
    url.searchParams.get("initialInput"),
    "Work on Ticket 00001KYRRDVH9 as its reviewer.",
  );
});

Deno.test("ticket detail uses server-derived role assignment actions", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../../routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
      import.meta.url,
    ),
  );

  assertEquals(source.includes("ticket.action_eligibility.can_queue"), true);
  assertEquals(source.includes("ticket.relations.blockers.length > 0"), true);
  assertEquals(source.includes("ticket.action_eligibility.queue_tickets"), true);
  assertEquals(source.includes("This operation queues:"), true);
  assertEquals(source.includes("outcome.queued_tickets.join"), true);
  assertEquals(
    source.includes("resolve the listed blockers before Queue"),
    false,
  );
  assertEquals(
    source.includes("ticket.action_eligibility.can_assign_orchestrator"),
    true,
  );
  assertEquals(source.includes("/assignments/${role}"), true);
  assertEquals(source.includes('kind: "workspace_agent"'), true);
  assertEquals(source.includes('kind: "worker"'), true);
  assertEquals(source.includes("ticket.assignee"), false);
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
