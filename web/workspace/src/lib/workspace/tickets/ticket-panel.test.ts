import {
  appendUniqueTicketSummaries,
  TICKET_LANE_PAGE_SIZE,
  ticketLaneDefinitions,
  ticketLaneQuery,
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
  readTextFile(path: string): Promise<string>;
};

function assertIncludes(actual: string, expected: string): void {
  if (!actual.includes(expected)) {
    throw new Error(`expected source to include ${JSON.stringify(expected)}`);
  }
}

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

Deno.test("ticket lane queries request independent pages of 30", () => {
  const [readyPlanning, inprogressQueued, doneClosed] = ticketLaneDefinitions();

  assertEquals(TICKET_LANE_PAGE_SIZE, 30);
  assertEquals(ticketLaneQuery(readyPlanning).states, ["ready", "planning"]);
  assertEquals(ticketLaneQuery(inprogressQueued).states, [
    "inprogress",
    "queued",
  ]);
  assertEquals(ticketLaneQuery(doneClosed, "next-page"), {
    attention: [],
    cursor: "next-page",
    event_kinds: [],
    evidence: [],
    limit: 30,
    linked_objective_id: null,
    query: null,
    related_ticket_id: null,
    relation_kind: null,
    review_status: null,
    sort: "updated_desc",
    states: ["done", "closed"],
    updated_after: null,
    updated_before: null,
  });
});

Deno.test("incremental Ticket pages preserve order and discard duplicate ids", () => {
  const current = [
    { id: "first", title: "First", state: "ready", priority: "1" },
    { id: "second", title: "Second", state: "planning", priority: "2" },
  ] as TicketSummary[];
  const incoming = [
    { id: "second", title: "Duplicate", state: "planning", priority: "2" },
    { id: "third", title: "Third", state: "planning", priority: "3" },
  ] as TicketSummary[];

  assertEquals(
    appendUniqueTicketSummaries(current, incoming).map((ticket) => ticket.id),
    ["first", "second", "third"],
  );
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

Deno.test("ticket panel starts the Orchestrator explicitly and gates orchestration actions", async () => {
  const panelSource = await Deno.readTextFile(
    "src/routes/w/[workspaceId]/tickets/+page.svelte",
  );
  const detailSource = await Deno.readTextFile(
    "src/routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
  );

  assertIncludes(
    panelSource,
    'workspaceApiPath(data.workspaceId, "/orchestrator")',
  );
  assertIncludes(panelSource, '{ method: "POST" }');
  assertIncludes(panelSource, "Start Orchestrator");
  assertIncludes(panelSource, "orchestrator.data?.online");
  assertIncludes(
    panelSource,
    'workspaceApiPath(data.workspaceId, "/tickets/query")',
  );
  assertIncludes(
    panelSource,
    "onscroll={(event) => handleLaneScroll(event, lane.id)}",
  );
  assertIncludes(panelSource, "Loading 30 more…");
  assertIncludes(detailSource, "{#if orchestratorOnline}");
  assertIncludes(detailSource, "!orchestratorOnline");
  assertIncludes(detailSource, "Orchestrator offline");
});
