import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type { ListResponse, Runtime } from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimes = await loadJson<ListResponse<Runtime>>(
    fetch,
    workspaceApiPath(params.workspaceId, "/runtimes"),
  );

  return {
    workspaceId: params.workspaceId,
    runtimes: runtimes.data,
    runtimesError: runtimes.error,
  };
};
