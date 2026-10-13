import { listWorkspaces } from "#lib/workspace/api/workspace-catalog.ts";
import { settle } from "#lib/workspace/navigation/deferred.ts";
import type { Load } from "@sveltejs/kit";

export const ssr = false;

export const load = ({ fetch }: Parameters<Load>[0]) => ({
  deferredCatalog: settle((async () => {
    try {
      return {
        accessibleWorkspaces: await listWorkspaces(fetch),
        workspaceCatalogError: null,
      };
    } catch (error) {
      return {
        accessibleWorkspaces: [],
        workspaceCatalogError: error instanceof Error ? error.message : "Unable to load Workspaces",
      };
    }
  })()),
});
