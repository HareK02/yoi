import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { MemoryStagingListResponse } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    staging: await loadJson<MemoryStagingListResponse>(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/memory/staging")}?limit=200`,
    ),
  };
};
