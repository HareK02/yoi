import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
  const repositories = await loadWorkspaceRepositoryList(
    fetch,
    params.workspaceId,
  );
  return {
    workspaceId: params.workspaceId,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
});
