import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseTicketQueryResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import {
  HANDLING_STATES,
  readTicketFilters,
  ticketQueryRequest,
} from "#lib/workspace/tickets/ticket-panel.ts";
import type { TicketAssignmentTrackingStatus } from "#lib/generated/ticket-api.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ fetch, params, url }: Parameters<PageLoad>[0]) => {
  const workspaceId = params.workspaceId;
  const filters = {
    ...readTicketFilters(url.searchParams),

    tagIds: [],
    closureReason: "all" as const,
    closureText: "",
  };
  const input = url.searchParams.get("handling");
  const handling =
    HANDLING_STATES.includes(input as TicketAssignmentTrackingStatus)
      ? input as TicketAssignmentTrackingStatus
      : null;
  const request = ticketQueryRequest(
    filters,
    filters.caseState === "all" ? [] : [filters.caseState],
    null,
    handling,
  );
  const tickets = await loadJson(
      fetch,
      workspaceApiPath(workspaceId, "/tickets/query"),
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(request),
      },
      parseTicketQueryResponse,
      TICKET_BROWSER_API_LOAD_POLICY,
    );

  return { workspaceId, filters, handling, tickets };
});
