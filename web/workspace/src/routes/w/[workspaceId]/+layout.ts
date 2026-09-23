import { error } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { loadWorkspaceRepositoryList } from "$lib/workspace/api/repositories";
import { parseWorkspaceResponse } from "$lib/workspace/api/workspace-model";
import type { LayoutLoad } from "./$types";

export const load: LayoutLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [workspaceResult, repositoryResult] = await Promise.all([
    loadJson(
      fetch,
      workspaceApiPath(workspaceId, "/workspace"),
      undefined,
      parseWorkspaceResponse,
      { diagnosticLabel: "Workspace API", maxResponseBytes: 1024 * 1024 },
    ),
    loadWorkspaceRepositoryList(fetch, workspaceId),
  ]);

  const workspace = workspaceResult.data;
  const workspaceError = workspaceResult.error;
  if (!workspace) {
    error(404, {
      message: workspaceError ?? `Workspace ${workspaceId} is unavailable`,
    });
  }

  const repositories = repositoryResult.data;
  const repositoriesError = repositoryResult.error;

  return {
    workspace,
    workspaceError: null,
    repositories,
    repositoriesError,
  };
};
