import { error } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { loadWorkspaceRepositoryList } from "$lib/workspace/api/repositories";
import { parseWorkspaceResponse } from "$lib/workspace/api/workspace-model";
import type { LayoutLoad } from "./$types";

export const load: LayoutLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [workspaceResult, repositoryResult] = await Promise.all([
    loadJson<unknown>(fetch, workspaceApiPath(workspaceId, "/workspace")),
    loadWorkspaceRepositoryList(fetch, workspaceId),
  ]);

  let workspace = null;
  let workspaceError = workspaceResult.error;
  if (workspaceResult.data !== null) {
    try {
      workspace = parseWorkspaceResponse(workspaceResult.data);
    } catch (cause) {
      workspaceError = cause instanceof Error
        ? cause.message
        : "invalid workspace response";
    }
  }
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
