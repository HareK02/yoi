import { parseBrowserWorkspaceOrchestratorResponse } from "#lib/workspace/api/workers.ts";
import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseTicketQueryResponse,
  TICKET_BROWSER_API_LOAD_POLICY,
} from "#lib/workspace/api/ticket-browser.ts";
import { parseTags } from "#lib/workspace/api/ticket-operations.ts";
import {
  readTicketFilters,
  ticketQueryRequest,
} from "#lib/workspace/tickets/ticket-panel.ts";
import type { TicketState } from "#lib/generated/ticket-api.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params, url }) => {
  const workspaceId = params.workspaceId;
  const filters = readTicketFilters(url.searchParams);
  const loadLane = async (state: TicketState) => {
    if (filters.caseState !== "all" && filters.caseState !== state) return null;
    try {
      return await loadJson(
        fetch,
        workspaceApiPath(workspaceId, "/tickets/query"),
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(ticketQueryRequest(filters, [state])),
        },
        parseTicketQueryResponse,
        TICKET_BROWSER_API_LOAD_POLICY,
      );
    } catch (error) {
      return {
        data: null,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  };
  const [open, closed, tags, orchestrator, repositories] = await Promise.all([
    loadLane("open"),
    loadLane("closed"),
    loadJson(
      fetch,
      workspaceApiPath(workspaceId, "/ticket-tags"),
      undefined,
      parseTags,
      TICKET_BROWSER_API_LOAD_POLICY,
    ),
    loadJson(
      fetch,
      workspaceApiPath(workspaceId, "/orchestrator"),
      undefined,
      parseBrowserWorkspaceOrchestratorResponse,
    ),
    loadWorkspaceRepositoryList(fetch, workspaceId),
  ]);
  return {
    workspaceId,
    filters,
    ticketLanes: { open, closed },
    tags,
    orchestrator,
    repositories,
  };
};
