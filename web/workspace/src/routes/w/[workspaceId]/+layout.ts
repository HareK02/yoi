import { error } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import {
  parseRepositoryListResponse,
  parseWorkspaceResponse,
} from "$lib/workspace/api/workspace-model";
import type { LayoutLoad } from "./$types";

export const load: LayoutLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [workspaceResult, repositoryResult] = await Promise.all([
    loadJson<unknown>(fetch, workspaceApiPath(workspaceId, "/workspace")),
    loadJson<unknown>(fetch, workspaceApiPath(workspaceId, "/repositories")),
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

  let repositories = null;
  let repositoriesError = repositoryResult.error;
  if (repositoryResult.data !== null) {
    try {
      repositories = parseRepositoryListResponse(repositoryResult.data);
    } catch (cause) {
      repositoriesError = cause instanceof Error
        ? cause.message
        : "invalid repository list response";
    }
  }

  return {
    workspace,
    workspaceError: null,
    repositories,
    repositoriesError,
  };
};
