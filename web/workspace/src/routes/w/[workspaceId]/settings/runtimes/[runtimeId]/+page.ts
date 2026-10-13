import { deferPage } from "#lib/workspace/navigation/deferred.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseWorkspaceRuntimeDetail } from "#lib/workspace/api/runtime-management.ts";
import type { PageLoad } from "./$types";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
  const detail = await loadJson(
    fetch,
    workspaceApiPath(
      params.workspaceId,
      `/runtimes/${encodeURIComponent(params.runtimeId)}`,
    ),
    undefined,
    (value) => {
      const response = parseWorkspaceRuntimeDetail(value);
      if (
        response.workspace_id !== params.workspaceId ||
        response.runtime.runtime_id !== params.runtimeId
      ) {
        throw new Error("Runtime detail did not match the route");
      }
      return response;
    },
  );

  return {
    workspaceId: params.workspaceId,
    runtimeId: params.runtimeId,
    runtimeDetail: detail.data,
    runtimeDetailError: detail.error,
  };
});
