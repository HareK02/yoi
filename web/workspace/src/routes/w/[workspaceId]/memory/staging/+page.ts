import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseMemoryStagingListResponse } from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    staging: await loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/memory/staging")}?limit=200`,
      undefined,
      parseMemoryStagingListResponse,
    ),
  };
};
