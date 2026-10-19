import { loadDashboard } from "#lib/workspace/home/dashboard.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params, depends, parent }) => {
  await parent();
  depends("workspace:home");
  return {
    workspaceId: params.workspaceId,
    dashboard: loadDashboard(fetch, params.workspaceId),
  };
};
