import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type {
  RepositoryListResponse,
  TicketDetail,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const [ticket, repositories] = await Promise.all([
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
  ]);

  return {
    workspaceId: params.workspaceId,
    ticketId: params.ticketId,
    ticket,
    repositories,
  };
}) satisfies PageLoad;
