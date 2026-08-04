import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type { TicketListResponse } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const [tickets, orchestrator] = await Promise.all([
    loadJson<TicketListResponse>(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/tickets")}?limit=1000`,
    ),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(params.workspaceId, "/orchestrator"),
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    tickets,
    orchestrator,
  };
}) satisfies PageLoad;
