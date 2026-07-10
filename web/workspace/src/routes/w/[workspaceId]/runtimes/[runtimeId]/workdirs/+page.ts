import { loadJson, workspaceApiPath } from "$lib/workspace-api/http";
import type {
  BrowserWorkingDirectoryListResponse,
  ListResponse,
  Runtime,
} from "$lib/workspace-sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimeId = params.runtimeId;
  const [runtimes, workdirs] = await Promise.all([
    loadJson<ListResponse<Runtime>>(fetch, workspaceApiPath(params.workspaceId, "/runtimes")),
    loadJson<BrowserWorkingDirectoryListResponse>(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/runtimes/${encodeURIComponent(runtimeId)}/working-directories`,
      ),
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    runtimeId,
    runtimes: runtimes.data,
    runtimesError: runtimes.error,
    workdirs: workdirs.data,
    workdirsError: workdirs.error,
  };
};
