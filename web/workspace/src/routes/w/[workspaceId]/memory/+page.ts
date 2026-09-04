import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseMemoryDocumentResponse } from "$lib/workspace/memory/api";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    memory: await loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, "/memory"),
      undefined,
      parseMemoryDocumentResponse,
    ),
  };
};
