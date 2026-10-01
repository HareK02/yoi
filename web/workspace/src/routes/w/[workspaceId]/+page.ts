import { loadDashboard } from "$lib/workspace/home/dashboard";
import type { PageLoad } from "./$types";

export const load: PageLoad = ({ fetch, params, depends }) => {
  depends("workspace:home");
  return {
    workspaceId: params.workspaceId,
    dashboard: loadDashboard(fetch, params.workspaceId),
  };
};
