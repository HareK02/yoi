import type {
  SetTicketRoleAssignmentRequest,
  TicketAssignmentAnswer,
  TicketAssignmentAnswerReceipt,
  TicketAssignmentDiagnosticsPage,
  TicketAssignmentsPage,
  TicketAssignmentTrackingHistoryPage,
  TicketAssignmentTrackingProjection,
  TicketDetail,
  WorkspaceTag,
} from "#lib/generated/ticket-api.ts";
import { workspaceApiPath } from "./http.ts";
import {
  parseAssignmentTracking,
  parseAssignmentTrackingProjection,
  parseWorkspaceTag,
} from "./ticket-browser.ts";

export function parseTags(value: unknown): WorkspaceTag[] {
  if (!Array.isArray(value) || value.length > 1000) {
    throw new Error("Tag list is invalid or unbounded");
  }
  return value.map((tag, index) => parseWorkspaceTag(tag, `Tags[${index}]`));
}
export function parseTracking(
  value: unknown,
): TicketAssignmentTrackingProjection[] {
  if (!Array.isArray(value) || value.length > 1000) {
    throw new Error("Assignment list is invalid or unbounded");
  }
  return value.map((item, index) =>
    parseAssignmentTrackingProjection(item, `Assignments[${index}]`)
  );
}
export function parseTrackingHistory(
  value: unknown,
): TicketAssignmentTrackingHistoryPage {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("History must be an object");
  }
  const page = value as Record<string, unknown>;
  if (
    Object.keys(page).some((key) =>
      !["items", "next_event_id"].includes(key)
    ) || !Array.isArray(page.items) || page.items.length > 100
  ) throw new Error("History page is invalid");
  const items = page.items.map((value, index) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("History entry must be an object");
    }
    const item = value as Record<string, unknown>;
    if (
      Object.keys(item).some((key) =>
        !["at", "event_id", "operation_key", "source_context", "tracking"]
          .includes(key)
      )
    ) throw new Error("Unexpected history field");
    for (const key of ["at", "event_id", "operation_key"]) {
      if (
        typeof item[key] !== "string" || (item[key] as string).length > 1024
      ) throw new Error(`History ${key} is invalid`);
    }
    return {
      at: item.at as string,
      event_id: item.event_id as string,
      operation_key: item.operation_key as string,
      source_context: item.source_context,
      tracking: parseAssignmentTracking(item.tracking, `History[${index}]`),
    };
  });
  if (
    page.next_event_id != null &&
    (typeof page.next_event_id !== "string" || page.next_event_id.length > 1024)
  ) throw new Error("History cursor is invalid");
  return {
    items,
    next_event_id: page.next_event_id as string | null | undefined,
  };
}
export function trackingPath(workspaceId: string, ticketId: string): string {
  return `/workspaces/${encodeURIComponent(workspaceId)}/tickets/${
    encodeURIComponent(ticketId)
  }/assignment-tracking`;
}
export type ObservedOperation = Readonly<
  { path: string; method: "POST" | "PUT" | "DELETE"; body?: string }
>;
// Keep the exact serialized input and comparison subject for explicit retry. A new
// observation must never silently rebase an ambiguous operation onto a new assignment.
export function observeOperation(
  path: string,
  method: ObservedOperation["method"],
  body?: unknown,
): ObservedOperation {
  return Object.freeze({
    path,
    method,
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}
export function assignmentOperation(
  workspaceId: string,
  ticketId: string,
  observed: Pick<TicketDetail, "assignments">,
  operationId: string,
  clear = false,
  principal?: Extract<
    SetTicketRoleAssignmentRequest["principal"],
    { kind: "subject" }
  >,
): ObservedOperation {
  const current = observed.assignments.find((row) =>
    row.role === "orchestrator"
  );
  const path = workspaceApiPath(
    workspaceId,
    `/tickets/${encodeURIComponent(ticketId)}/assignments/orchestrator`,
  );
  if (clear) {
    if (!current) throw new Error("No observed current assignment to release");
    return observeOperation(
      `${path}?${new URLSearchParams({
        operation_id: operationId,
        assignment_id: current.assignment_id,
      })}`,
      "DELETE",
    );
  }
  if (
    !principal || principal.kind !== "subject" || !principal.subject_id.trim()
  ) {
    throw new Error("Select a Subject for the new assignment");
  }
  const request: SetTicketRoleAssignmentRequest = {
    operation_id: operationId,
    expected_assignment_id: current?.assignment_id ?? null,
    principal,
  };
  return observeOperation(path, "PUT", request);
}
export function operationError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  if (/\(409\)/.test(text)) {
    return `Stale observation / conflict. Refresh and reconcile; do not overwrite or retry as a new operation. ${text}`;
  }
  if (/\(401\)|\(403\)/.test(text)) {
    return `Permission denied. No access was granted. ${text}`;
  }
  if (/\(404\)/.test(text)) {
    return `Resource or assignment unavailable. ${text}`;
  }
  if (/\(400\)|\(422\)/.test(text)) return `Input rejected. ${text}`;
  return `Outcome unknown. This may already be saved; retain the same operation key for exact retry. ${text}`;
}

function record(value: unknown, keys: string[]): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Expected API object");
  }
  const item = value as Record<string, unknown>;
  if (Object.keys(item).some((key) => !keys.includes(key))) {
    throw new Error("Unexpected API field");
  }
  return item;
}
function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 16384) {
    throw new Error("Expected bounded string");
  }
  return value;
}
function optionalText(value: unknown): string | null | undefined {
  return value == null ? value as null | undefined : text(value);
}
function flag(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Expected boolean");
  return value;
}
function pageItems<T>(value: unknown, parse: (value: unknown) => T): T[] {
  if (!Array.isArray(value) || value.length > 100) {
    throw new Error("Expected bounded page");
  }
  return value.map(parse);
}
export function parseAssignmentsPage(value: unknown): TicketAssignmentsPage {
  const page = record(value, ["items", "next_assignment_id"]);
  return {
    items: pageItems(
      page.items,
      (item) => parseAssignmentTrackingProjection(item, "Assignment page"),
    ),
    next_assignment_id: optionalText(page.next_assignment_id),
  };
}
export function parseAnswerReceipt(
  value: unknown,
): TicketAssignmentAnswerReceipt {
  const item = record(value, [
    "assignment_id",
    "input_event_id",
    "event_index",
  ]);
  if (
    !Number.isSafeInteger(item.event_index) || (item.event_index as number) < 0
  ) throw new Error("Answer event index invalid");
  return {
    assignment_id: text(item.assignment_id),
    input_event_id: text(item.input_event_id),
    event_index: item.event_index as number,
  };
}
export function answerOperation(
  workspaceId: string,
  ticketId: string,
  observed: TicketAssignmentTrackingProjection,
  body: string,
  operationKey: string,
): ObservedOperation {
  if (
    observed.principal.kind !== "subject" || !observed.current ||
    !observed.last_event_id || !observed.tracking ||
    observed.tracking.status === "done"
  ) throw new Error("Read actionable current assignment before answering");
  const request: TicketAssignmentAnswer = {
    assignment_id: observed.assignment_id,
    expected_last_event_id: observed.last_event_id,
    body,
    operation_key: operationKey,
  };
  return observeOperation(
    workspaceApiPath(
      workspaceId,
      `/tickets/${encodeURIComponent(ticketId)}/assignment-answer`,
    ),
    "POST",
    request,
  );
}
export function parseAssignmentDiagnostics(
  value: unknown,
): TicketAssignmentDiagnosticsPage {
  const page = record(value, [
    "assignment_id",
    "current",
    "input_origin_known",
    "actionable",
    "inputs",
    "next_input_event_id",
    "starts",
    "next_start_operation_id",
  ]);
  return {
    assignment_id: text(page.assignment_id),
    current: flag(page.current),
    input_origin_known: flag(page.input_origin_known),
    actionable: flag(page.actionable),
    next_input_event_id: optionalText(page.next_input_event_id),
    next_start_operation_id: optionalText(page.next_start_operation_id),
    inputs: pageItems(page.inputs, (value) => {
      const item = record(value, [
        "input_event_id",
        "delivery_id",
        "notification",
        "acceptance_outcome_unknown",
      ]);
      if (
        !["not_reserved", "reserved", "request_persisted", "runtime_accepted"]
          .includes(item.notification as string)
      ) throw new Error("Unexpected notification state");
      return {
        input_event_id: text(item.input_event_id),
        delivery_id: optionalText(item.delivery_id),
        notification: item
          .notification as TicketAssignmentDiagnosticsPage["inputs"][number][
            "notification"
          ],
        acceptance_outcome_unknown: flag(item.acceptance_outcome_unknown),
      };
    }),
    starts: pageItems(page.starts, (value) => {
      const item = record(value, [
        "operation_id",
        "claim_state",
        "assignment_id",
        "runtime_id",
        "worker_id",
        "actual_processing_unknown",
      ]);
      return {
        operation_id: text(item.operation_id),
        claim_state: text(item.claim_state),
        assignment_id: optionalText(item.assignment_id),
        runtime_id: optionalText(item.runtime_id),
        worker_id: optionalText(item.worker_id),
        actual_processing_unknown: flag(item.actual_processing_unknown),
      };
    }),
  };
}
