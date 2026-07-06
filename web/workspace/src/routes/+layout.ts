import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type {
  RepositoryListResponse,
  WorkspaceResponse,
} from "$lib/workspace-sidebar/types";
import type { LayoutLoad } from "./$types";

export const ssr = false;
export const prerender = false;

export const load: LayoutLoad = async ({ fetch, params }) => {
  const workspaceId = params.workspaceId;
  const apiPath = (path: string) =>
    workspaceId ? workspaceApiPath(workspaceId, path) : `/api${path}`;

  const [workspace, repositories] = await Promise.all([
    loadJson<WorkspaceResponse>(fetch, apiPath("/workspace")),
    loadJson<RepositoryListResponse>(fetch, apiPath("/repositories")),
  ]);

  return {
    workspace: workspace.data,
    workspaceError: workspace.error,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
