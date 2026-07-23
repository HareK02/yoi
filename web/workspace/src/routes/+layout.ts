import { redirect } from "@sveltejs/kit";
import { loadJson, workspaceRoute } from "$lib/workspace/api/http";
import type { WorkspaceResponse } from "$lib/workspace/sidebar/types";
import type { LayoutLoad } from "./$types";

export const ssr = false;
export const prerender = false;

export const load: LayoutLoad = async ({ fetch, params, url }) => {
  if (params.workspaceId) {
    return { workspaceScoped: true };
  }

  const publicRoutes = new Set(["/account", "/login/device"]);
  if (publicRoutes.has(url.pathname)) {
    return { workspaceScoped: false };
  }

  const workspace = await loadJson<WorkspaceResponse>(fetch, "/api/workspace");
  if (workspace.data) {
    const scopedPath = workspaceRoute(workspace.data.workspace_id);
    throw redirect(307, `${scopedPath}${url.search}`);
  }
  return { workspaceScoped: false };
};
