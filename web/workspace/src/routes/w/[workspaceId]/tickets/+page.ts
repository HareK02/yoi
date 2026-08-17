import type { TicketListResponse } from "$lib/generated/ticket-api";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [tickets, orchestrator] = await Promise.all([
    loadJson<TicketListResponse>(
      fetch,
      `${workspaceApiPath(workspaceId, "/tickets")}?limit=1000`,
    ),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(workspaceId, "/orchestrator"),
    ),
  ]);

  return { workspaceId, tickets, orchestrator };
}) satisfies PageLoad;
