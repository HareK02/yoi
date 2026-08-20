import type { PageLoad } from "./$types";
import { loadWorkspaceCatalog } from "$lib/workspace/api/workspace-catalog";

export const load: PageLoad = async ({ fetch }) => {
  try {
    return {
      workspaces: await loadWorkspaceCatalog(fetch),
      catalogError: null,
    };
  } catch (error) {
    return {
      workspaces: [],
      catalogError: error instanceof Error ? error.message : String(error),
    };
  }
};
