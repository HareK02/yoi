import type { TicketQueryResponse } from "$lib/generated/ticket-api";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  ticketLaneDefinitions,
  ticketLaneQuery,
  ticketSummaryFromQueryItem,
  type WorkspaceOrchestratorStatus,
} from "$lib/workspace/tickets/ticket-panel";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const ticketLanePagesPromise = Promise.all(
    ticketLaneDefinitions().map(async (lane) => {
      try {
        const page = await loadJson<TicketQueryResponse>(
          fetch,
          workspaceApiPath(workspaceId, "/tickets/query"),
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(ticketLaneQuery(lane)),
          },
        );
        return {
          id: lane.id,
          label: lane.label,
          states: [...lane.states],
          tickets: page.data?.items.map(ticketSummaryFromQueryItem) ?? [],
          nextCursor: page.data?.page.next_cursor ?? null,
          hasMore: page.data?.page.has_more ?? false,
          error: page.error,
        };
      } catch (error) {
        return {
          id: lane.id,
          label: lane.label,
          states: [...lane.states],
          tickets: [],
          nextCursor: null,
          hasMore: false,
          error: error instanceof Error
            ? error.message
            : "Unable to load Tickets.",
        };
      }
    }),
  );

  const [ticketLanePages, orchestrator] = await Promise.all([
    ticketLanePagesPromise,
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(workspaceId, "/orchestrator"),
    ),
  ]);

  return {
    workspaceId,
    ticketLanePages,
    orchestrator,
  };
};
