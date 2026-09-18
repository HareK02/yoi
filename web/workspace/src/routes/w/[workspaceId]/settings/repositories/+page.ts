import { loadWorkspaceRepositoryList } from "$lib/workspace/api/repositories";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const repositories = await loadWorkspaceRepositoryList(
    fetch,
    params.workspaceId,
  );
  return {
    workspaceId: params.workspaceId,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
