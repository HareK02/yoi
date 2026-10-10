import type { BrowserWorkspaceOrchestratorResponse } from "#lib/generated/worker-launch-api.ts";
import type {
  TicketDetail,
  TicketListItemSummary,
} from "#lib/generated/ticket-api.ts";

export const TICKET_STATES = ["open", "closed"] as const;
export type TicketState = (typeof TICKET_STATES)[number];
export type WorkspaceOrchestratorStatus = BrowserWorkspaceOrchestratorResponse;
const LANE_DEFINITIONS = [
  { id: "open", label: "Open", states: ["open"] },
  { id: "closed", label: "Closed", states: ["closed"] },
] as const;

export const TICKET_LANE_PAGE_SIZE = 30;

export type TicketLaneDefinition = (typeof LANE_DEFINITIONS)[number];
export type TicketLaneId = TicketLaneDefinition["id"];
export type TicketCardSummary = Pick<
  TicketListItemSummary,
  "id" | "resource_key" | "title" | "state" | "priority" | "updated_at"
>;

export type TicketLane = {
  id: TicketLaneId;
  label: string;
  states: readonly TicketState[];
  tickets: TicketCardSummary[];
};

export function nextTicketLaneVisibleCount(
  current: number,
  total: number,
): number {
  return Math.min(total, current + TICKET_LANE_PAGE_SIZE);
}

function updatedAt(ticket: TicketCardSummary): number {
  if (!ticket.updated_at) return 0;
  const parsed = Date.parse(ticket.updated_at);
  return Number.isNaN(parsed) ? 0 : parsed;
}

export function sortTickets(
  tickets: TicketCardSummary[],
): TicketCardSummary[] {
  return [...tickets].sort((left, right) => {
    const updatedDelta = updatedAt(right) - updatedAt(left);
    if (updatedDelta !== 0) return updatedDelta;
    return left.id.localeCompare(right.id);
  });
}

export function ticketLanes(tickets: TicketCardSummary[]): TicketLane[] {
  return LANE_DEFINITIONS.map((definition) => ({
    ...definition,
    tickets: sortTickets(
      tickets.filter((ticket) =>
        (definition.states as readonly string[]).includes(ticket.state)
      ),
    ),
  }));
}

export function ticketWorkerMessage(ticketId: string): string {
  return `Work on Ticket ${ticketId}.`;
}

export function ticketWorkerLaunchHref(
  workspaceId: string,
  ticket: Pick<TicketDetail, "id" | "title">,
  request = ticketWorkerMessage(ticket.id),
): string {
  const params = new URLSearchParams({
    ticketId: ticket.id,
    ticketTitle: ticket.title,
    initialInput: request,
  });
  return `/w/${
    encodeURIComponent(workspaceId)
  }/workers/new?${params.toString()}`;
}

export function relationLabel(kind: string): string {
  return kind.replaceAll("_", " ");
}
