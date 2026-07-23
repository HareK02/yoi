import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceApiPath, workspaceRoute } from "$lib/workspace/api/http";
import type { RepositoryListResponse, WorkspaceResponse } from "$lib/workspace/sidebar/types";
import type { LayoutLoad } from "./$types";

export const ssr = false;
export const prerender = false;

export const load: LayoutLoad = async ({ fetch, params, url }) => {
  const workspaceId = params.workspaceId;

  if (!workspaceId) {
    const publicRoutes = new Set(["/account", "/login/device"]);
    if (publicRoutes.has(url.pathname)) {
      return {
        workspace: null,
        workspaceError: null,
        repositories: null,
        repositoriesError: null,
      };
    }

    const workspace = await loadJson<WorkspaceResponse>(fetch, "/api/workspace");
    if (workspace.data) {
      const scopedPath = workspaceRoute(workspace.data.workspace_id);
      throw redirect(307, `${scopedPath}${url.search}`);
    }
    return {
      workspace: null,
      workspaceError: workspace.error,
      repositories: null,
      repositoriesError: null,
    };
  }

  const apiPath = (path: string) => workspaceApiPath(workspaceId, path);

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
