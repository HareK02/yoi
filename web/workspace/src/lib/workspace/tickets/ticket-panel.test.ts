import {
  ticketLanes,
  ticketWorkerLaunchHref,
  ticketWorkerMessage,
} from "./ticket-panel.ts";
import type {
  TicketDetail,
  TicketSummary,
} from "../../generated/ticket-api.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
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
