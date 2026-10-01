import { listWorkspaces } from "#lib/workspace/api/workspace-catalog.ts";
import type { Load } from "@sveltejs/kit";

export const ssr = false;

export const load = (async ({ fetch }) => {
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
}) satisfies Load;
