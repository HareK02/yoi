import type { BrowserWorkspaceOrchestratorResponse } from "#lib/generated/worker-launch-api.ts";
import type {
  TicketAssignmentPrincipalSnapshot,
  TicketAssignmentTrackingProjection,
  TicketAssignmentTrackingStatus,
  TicketClosureReason,
  TicketDetail,
  TicketListItemSummary,
  TicketQueryItem,
  TicketQueryRequest,
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
export type TicketCardSummary =
  & Pick<
    TicketListItemSummary,
    "id" | "resource_key" | "title" | "state" | "priority" | "updated_at"
  >
  & Partial<
    Pick<TicketQueryItem, "tags" | "current_closure" | "assignment_tracking">
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

export const HANDLING_STATES = ["waiting", "inprogress", "done"] as const;
export const ORCHESTRATOR_PRINCIPAL = {
  kind: "workspace_agent",
  agent_key: "workspace-orchestrator",
} as const;

export function currentOrchestratorAssignment(
  ticket: Pick<TicketQueryItem, "assignment_tracking">,
): TicketAssignmentTrackingProjection | null {
  return ticket.assignment_tracking.find((row) =>
    row.current && row.role === "orchestrator"
  ) ?? null;
}
export function handlingLabel(
  status?: TicketAssignmentTrackingStatus | null,
): string {
  return status === "waiting"
    ? "Waiting"
    : status === "inprogress"
    ? "In progress"
    : status === "done"
    ? "Done"
    : "Unknown";
}
export function assignmentPrincipalLabel(
  principal: TicketAssignmentPrincipalSnapshot,
): string {
  if (principal.kind === "workspace_agent") {
    return `Workspace agent: ${principal.agent_key}`;
  }
  if (principal.kind === "user") return `User account: ${principal.account_id}`;
  return `Worker: ${principal.runtime_id}/${principal.worker_id}`;
}
export function handlingSummary(
  row: TicketAssignmentTrackingProjection | null,
): string {
  const tracking = row?.tracking;
  if (!row) return "Not assigned";
  if (!tracking) return row.migration_diagnostic ?? "Handling unavailable";
  return tracking.status === "waiting"
    ? tracking.waiting_reason ?? "Waiting reason not reported"
    : tracking.status === "inprogress"
    ? tracking.progress ?? "Progress not reported"
    : tracking.result ?? "Result not reported";
}
export function closureReasonLabel(reason: TicketClosureReason): string {
  return typeof reason === "object" ? reason.other : ({
    completed: "Completed",
    not_planned: "Not planned",
    duplicate: "Duplicate",
    unknown: "Unknown",
  })[reason];
}
export type TicketFilters = {
  query: string;
  caseState: "all" | TicketState;
  tagIds: string[];
  principal: "all" | "orchestrator" | "user" | "worker";
  accountId: string;
  runtimeId: string;
  workerId: string;
  closureReason:
    | "all"
    | "completed"
    | "not_planned"
    | "duplicate"
    | "unknown"
    | "other";
  closureText: string;
};
export function readTicketFilters(search: URLSearchParams): TicketFilters {
  const principal = search.get("principal");
  const state = search.get("case");
  const reason = search.get("reason");
  return {
    query: search.get("q") ?? "",
    caseState: state === "open" || state === "closed" ? state : "all",
    tagIds: [...new Set(search.getAll("tag"))],
    principal: principal === "orchestrator" || principal === "user" ||
        principal === "worker"
      ? principal
      : "all",
    accountId: search.get("account") ?? "",
    runtimeId: search.get("runtime") ?? "",
    workerId: search.get("worker") ?? "",
    closureReason:
      ["completed", "not_planned", "duplicate", "unknown", "other"].includes(
          reason ?? "",
        )
        ? reason as TicketFilters["closureReason"]
        : "all",
    closureText: search.get("reason_text") ?? "",
  };
}
export function filterPrincipal(
  filters: TicketFilters,
): TicketAssignmentPrincipalSnapshot | null {
  if (filters.principal === "orchestrator") return ORCHESTRATOR_PRINCIPAL;
  if (filters.principal === "user") {
    if (!filters.accountId.trim()) {
      throw new Error("Enter an account ID for the user principal filter");
    }
    return { kind: "user", account_id: filters.accountId.trim() };
  }
  if (filters.principal === "worker") {
    if (!filters.runtimeId.trim() || !filters.workerId.trim()) {
      throw new Error(
        "Enter Runtime and Worker IDs for the Worker principal filter",
      );
    }
    return {
      kind: "worker",
      runtime_id: filters.runtimeId.trim(),
      worker_id: filters.workerId.trim(),
    };
  }
  return null;
}
// Every page repeats the same server-side filters. Never search a loaded slice.
export function ticketQueryRequest(
  filters: TicketFilters,
  states: TicketState[],
  cursor: string | null = null,
  handlingStatus: TicketAssignmentTrackingStatus | null = null,
): TicketQueryRequest {
  return {
    query: filters.query.trim() || null,
    states,
    ...(filters.closureReason === "all" ? {} : {
      closure_reason: filters.closureReason === "other"
        ? { other: filters.closureText.trim() }
        : filters.closureReason,
    }),
    tag_ids: filters.tagIds,
    assigned_principal: filterPrincipal(filters),
    handling_status: handlingStatus,
    sort: "updated_desc",
    limit: TICKET_LANE_PAGE_SIZE as TicketQueryRequest["limit"],
    cursor,
  };
}

export function relationLabel(kind: string): string {
  return kind.replaceAll("_", " ");
}
