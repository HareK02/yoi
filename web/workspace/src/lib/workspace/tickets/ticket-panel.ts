import type {
  TicketAssignmentPrincipalSnapshot,
  TicketAssignmentTrackingProjection,
  TicketAssignmentTrackingStatus,
  TicketClosureReason,
  TicketDetail,
  TicketQueryItem,
  TicketQueryRequest,
} from "#lib/generated/ticket-api.ts";

export const TICKET_STATES = ["open", "closed"] as const;
export type TicketState = (typeof TICKET_STATES)[number];
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
// 読取り専用の旧割当filter。新規割当へは使用しない。
export const LEGACY_ORCHESTRATOR_PRINCIPAL = {
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
  if (principal.kind === "subject") return `Subject: ${principal.subject_id}`;
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
  principal: "all" | "subject" | "orchestrator" | "user" | "worker";
  subjectId: string;
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
    principal: principal === "subject" || principal === "orchestrator" ||
        principal === "user" ||
        principal === "worker"
      ? principal
      : "all",
    subjectId: search.get("subject") ?? "",
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
  if (filters.principal === "subject") {
    if (!filters.subjectId.trim()) {
      throw new Error("Select a Subject for the assignment filter");
    }
    return { kind: "subject", subject_id: filters.subjectId.trim() };
  }
  if (filters.principal === "orchestrator") {
    return LEGACY_ORCHESTRATOR_PRINCIPAL;
  }
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
    limit: 30 as TicketQueryRequest["limit"],
    cursor,
  };
}

export function relationLabel(kind: string): string {
  return kind.replaceAll("_", " ");
}
