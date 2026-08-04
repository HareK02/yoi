import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { WorkspaceOrchestratorStatus } from "$lib/workspace/tickets/ticket-panel";
import type {
  RepositoryListResponse,
  TicketDetail,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const [ticket, repositories, orchestrator] = await Promise.all([
    loadJson<TicketDetail>(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/tickets/${encodeURIComponent(params.ticketId)}`,
      ),
    ),
    loadJson<RepositoryListResponse>(
      fetch,
      workspaceApiPath(params.workspaceId, "/repositories"),
    ),
    loadJson<WorkspaceOrchestratorStatus>(
      fetch,
      workspaceApiPath(params.workspaceId, "/orchestrator"),
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    ticketId: params.ticketId,
    ticket,
    repositories,
    orchestrator,
  };
}) satisfies PageLoad;
