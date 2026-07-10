import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type { ListResponse, Worker } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workers = await loadJson<ListResponse<Worker>>(
    fetch,
    workspaceApiPath(params.workspaceId, "/workers"),
  );

  return {
    workspaceId: params.workspaceId,
    workers: workers.data,
    workersError: workers.error,
  };
};
