import { listWorkspaces } from "$lib/workspace/api/workspace-catalog";
import type { LayoutLoad } from "./$types";

export const load: LayoutLoad = async ({ fetch }) => {
  try {
    return {
      accessibleWorkspaces: await listWorkspaces(fetch),
      workspaceCatalogError: null,
    };
  } catch (error) {
    return {
      accessibleWorkspaces: [],
      workspaceCatalogError: error instanceof Error
        ? error.message
        : "Unable to load Workspaces",
    };
  }
};
