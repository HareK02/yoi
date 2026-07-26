import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { MemoryDocumentResponse } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  return {
    workspaceId: params.workspaceId,
    memory: await loadJson<MemoryDocumentResponse>(
      fetch,
      workspaceApiPath(params.workspaceId, "/memory"),
    ),
  };
};
