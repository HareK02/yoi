import { MEMORY_API_LIMITS } from "#lib/generated/memory-api.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseMemoryDocumentResponse } from "#lib/workspace/memory/api.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    memory: await loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, "/memory"),
      undefined,
      parseMemoryDocumentResponse,
      {
        diagnosticLabel: "Memory API",
        maxResponseBytes: MEMORY_API_LIMITS.maxResponseBytes,
      },
    ),
  };
};
