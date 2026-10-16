import { isAuthPage } from "#lib/workspace/auth/routes.ts";
import { listWorkspaces } from "#lib/workspace/api/workspace-catalog.ts";
import { settle } from "#lib/workspace/navigation/deferred.ts";
import type { Load } from "@sveltejs/kit";

export const ssr = false;

export const load = ({ fetch, url }: Parameters<Load>[0]) => ({
  deferredCatalog: settle((async () => {
    // 公開入口・認証・hostingの判断はWorkspace catalogを必要としない。
    // dashboardはactor確認後に自身の一覧を取得する。
    if (
      url.pathname === "/" || url.pathname === "/dashboard" ||
      isAuthPage(url.pathname) || url.pathname.startsWith("/hosting/")
    ) {
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
