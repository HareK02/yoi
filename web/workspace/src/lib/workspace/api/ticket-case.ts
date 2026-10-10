import type {
  TicketCaseAction,
  TicketCaseUpdateRequest,
  TicketDetail,
  TicketSummary,
} from "#lib/generated/ticket-api.ts";
import { workspaceApiJsonWithBody } from "./http.ts";
import {
  parseTicketSummary,
  TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
} from "./ticket-browser.ts";

export function ticketCaseRequest(
  ticket: Pick<
    TicketDetail,
    "content_digest" | "state" | "closure_operation_key"
  >,
  action: TicketCaseAction,
  explanation: string,
  operationKey: string,
): TicketCaseUpdateRequest {
  return {
    expected_content_digest: ticket.content_digest,
    expected_state: ticket.state,
    // The last judgment key survives reopening; do not derive it from current_closure.
    expected_closure_operation_key: ticket.closure_operation_key ?? null,
    operation_key: operationKey,
    action,
    explanation: explanation.trim(),
    references: [],
  };
}

export function updateTicketCase(
  workspaceId: string,
  ticketId: string,
  request: TicketCaseUpdateRequest,
): Promise<TicketSummary> {
  return workspaceApiJsonWithBody(
    `/workspaces/${encodeURIComponent(workspaceId)}/tickets/${
      encodeURIComponent(ticketId)
    }/case`,
    { method: "POST", body: JSON.stringify(request) },
    parseTicketSummary,
    TICKET_BROWSER_API_MAX_RESPONSE_BYTES,
  );
}
