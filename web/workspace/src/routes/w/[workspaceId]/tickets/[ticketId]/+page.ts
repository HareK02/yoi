import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { TicketDetail } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const ticketId = params.ticketId;
  const ticket = await loadJson<TicketDetail>(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/tickets/${encodeURIComponent(ticketId)}`,
    ),
  );

  return {
    workspaceId: params.workspaceId,
    ticketId,
    ticket,
  };
}) satisfies PageLoad;
