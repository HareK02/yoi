import { loadJson } from "$lib/workspace-api/http";
import type {
  RepositoryListResponse,
  WorkspaceResponse,
} from "$lib/workspace-sidebar/types";
import type { LayoutLoad } from "./$types";

export const ssr = false;
export const prerender = false;

export const load: LayoutLoad = async ({ fetch }) => {
  const [workspace, repositories] = await Promise.all([
    loadJson<WorkspaceResponse>(fetch, "/api/workspace"),
    loadJson<RepositoryListResponse>(fetch, "/api/repositories"),
  ]);

  return {
    workspace: workspace.data,
    workspaceError: workspace.error,
    repositories: repositories.data,
    repositoriesError: repositories.error,
  };
};
