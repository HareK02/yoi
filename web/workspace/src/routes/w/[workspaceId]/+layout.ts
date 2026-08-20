import { error } from "@sveltejs/kit";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { LayoutLoad } from "./$types";
import type {
  RepositoryListResponse,
  WorkspaceResponse,
} from "$lib/workspace/sidebar/types";

export const load: LayoutLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const [workspace, repositories] = await Promise.all([
    loadJson<WorkspaceResponse>(
      fetch,
      workspaceApiPath(workspaceId, "/workspace"),
    ),
    loadJson<RepositoryListResponse>(
      fetch,
      workspaceApiPath(workspaceId, "/repositories"),
    ),
  ]);

  if (!workspace.data) {
    error(404, {
      message: workspace.error ?? `Workspace ${workspaceId} is unavailable`,
    });
  }

  return {
    workspace: workspace.data,
    workspaceError: null,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
