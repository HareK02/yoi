import { listWorkspaces } from "#lib/workspace/api/workspace-catalog.ts";
import { settle } from "#lib/workspace/navigation/deferred.ts";
import type { Load } from "@sveltejs/kit";

export const ssr = false;

export const load = ({ fetch, url }: Parameters<Load>[0]) => ({
  deferredCatalog: settle((async () => {
    // 初回有効化とhosting管理はWorkspace未作成でも利用する。
    if (url.pathname === "/activate" || url.pathname.startsWith("/hosting/")) {
      return { accessibleWorkspaces: [], workspaceCatalogError: null };
    }
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
  })()),
});
