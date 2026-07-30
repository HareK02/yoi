import type { TicketDetail, TicketSummary } from "$lib/generated/ticket-api";

export const TICKET_STATES = [
  "planning",
  "ready",
  "queued",
  "inprogress",
  "done",
  "closed",
] as const;

export type TicketState = (typeof TICKET_STATES)[number];
export type TicketWorkerRole = "coder" | "reviewer";

const LANE_DEFINITIONS = [
  {
    id: "ready-planning",
    label: "Ready + Planning",
    states: ["ready", "planning"],
  },
  {
    id: "inprogress-queued",
    label: "In progress + Queued",
    states: ["inprogress", "queued"],
  },
  {
    id: "done-closed",
    label: "Done + Closed",
    states: ["done", "closed"],
  },
] as const;

const STATE_SORT_ORDER = new Map<string, number>([
  ["ready", 0],
  ["planning", 1],
  ["inprogress", 0],
  ["queued", 1],
  ["done", 0],
  ["closed", 1],
]);

export type TicketLane = {
  id: string;
  label: string;
  states: readonly TicketState[];
  tickets: TicketSummary[];
};

function updatedAt(ticket: TicketSummary): number {
  if (!ticket.updated_at) return 0;
  const parsed = Date.parse(ticket.updated_at);
  return Number.isNaN(parsed) ? 0 : parsed;
}

export function sortTickets(tickets: TicketSummary[]): TicketSummary[] {
  return [...tickets].sort((left, right) => {
    const stateDelta = (STATE_SORT_ORDER.get(left.state) ?? 99) -
      (STATE_SORT_ORDER.get(right.state) ?? 99);
    if (stateDelta !== 0) return stateDelta;
    const updatedDelta = updatedAt(right) - updatedAt(left);
    if (updatedDelta !== 0) return updatedDelta;
    return left.id.localeCompare(right.id);
  });
}

export function ticketLanes(tickets: TicketSummary[]): TicketLane[] {
  return LANE_DEFINITIONS.map((definition) => ({
    ...definition,
    tickets: sortTickets(
      tickets.filter((ticket) =>
        (definition.states as readonly string[]).includes(ticket.state)
      ),
    ),
  }));
}

export function ticketWorkerMessage(
  ticketId: string,
  role: TicketWorkerRole,
): string {
  return `Work on Ticket ${ticketId} as its ${role}.`;
}

export function ticketWorkerLaunchHref(
  workspaceId: string,
  ticket: Pick<
    TicketDetail,
    "id" | "title" | "repository_id" | "ref_selector"
  >,
  role: TicketWorkerRole,
): string {
  const params = new URLSearchParams({
    ticketId: ticket.id,
    ticketTitle: ticket.title,
    ticketRole: role,
    initialInput: ticketWorkerMessage(ticket.id, role),
  });
  if (ticket.repository_id) {
    params.set("repositoryId", ticket.repository_id);
  }
  if (ticket.ref_selector) {
    params.set("refSelector", ticket.ref_selector);
  }
  return `/w/${
    encodeURIComponent(workspaceId)
  }/workers/new?${params.toString()}`;
}

export function relationLabel(kind: string): string {
  return kind.replaceAll("_", " ");
}
