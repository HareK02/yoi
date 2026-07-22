import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { TicketListResponse } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load = (async ({ fetch, params }) => {
  const tickets = await loadJson<TicketListResponse>(
    fetch,
    workspaceApiPath(params.workspaceId, "/tickets"),
  );

  return {
    workspaceId: params.workspaceId,
    tickets,
  };
}) satisfies PageLoad;
