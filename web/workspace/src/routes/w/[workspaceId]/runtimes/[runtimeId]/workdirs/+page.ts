import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type {
  BrowserWorkingDirectoryListResponse,
  ListResponse,
  Runtime,
  RuntimeCleanupPlanResponse,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimeId = params.runtimeId;
  const [runtimes, workdirs, cleanupPlan] = await Promise.all([
    loadJson<ListResponse<Runtime>>(fetch, workspaceApiPath(params.workspaceId, "/runtimes")),
    loadJson<BrowserWorkingDirectoryListResponse>(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/runtimes/${encodeURIComponent(runtimeId)}/working-directories`,
      ),
    ),
    loadJson<RuntimeCleanupPlanResponse>(
      fetch,
      workspaceApiPath(params.workspaceId, `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`),
    ),
  ]);

  return {
    workspaceId: params.workspaceId,
    runtimeId,
    runtimes: runtimes.data,
    runtimesError: runtimes.error,
    workdirs: workdirs.data,
    workdirsError: workdirs.error,
    cleanupPlan: cleanupPlan.data,
    cleanupPlanError: cleanupPlan.error,
  };
};
