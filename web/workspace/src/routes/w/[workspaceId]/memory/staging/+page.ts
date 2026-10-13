import { MEMORY_API_LIMITS } from "#lib/generated/memory-api.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseMemoryStagingListResponse } from "#lib/workspace/memory/api.ts";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";
import type { PageLoad } from "./$types";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
  return {
    workspaceId: params.workspaceId,
    staging: await loadJson(
      fetch,
      `${workspaceApiPath(params.workspaceId, "/memory/staging")}?limit=200`,
      undefined,
      parseMemoryStagingListResponse,
      {
        diagnosticLabel: "Memory API",
        maxResponseBytes: MEMORY_API_LIMITS.maxResponseBytes,
      },
    ),
  };
});
