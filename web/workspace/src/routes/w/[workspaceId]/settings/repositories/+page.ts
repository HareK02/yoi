import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseRepositoryListResponse } from "$lib/workspace/api/workspace-model";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const repositories = await loadJson(
    fetch,
    workspaceApiPath(params.workspaceId, "/repositories"),
    undefined,
    parseRepositoryListResponse,
  );
  return {
    workspaceId: params.workspaceId,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
