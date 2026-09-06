import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkspaceRuntimeDetail } from "$lib/workspace/api/runtime-management";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
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
};
