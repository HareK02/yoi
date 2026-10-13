import { error } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { loadWorkspaceRepositoryList } from "#lib/workspace/api/repositories.ts";
import { parseWorkspaceResponse } from "#lib/workspace/api/workspace-model.ts";
import { settle } from "#lib/workspace/navigation/deferred.ts";
import type { LayoutLoad } from "./$types";

export const load = ({ fetch, params }: Parameters<LayoutLoad>[0]) => {
  const workspaceId = params.workspaceId;
  return { deferredWorkspace: settle(loadWorkspace(fetch, workspaceId)) };
};

async function loadWorkspace(fetch: Parameters<LayoutLoad>[0]["fetch"], workspaceId: string) {
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
