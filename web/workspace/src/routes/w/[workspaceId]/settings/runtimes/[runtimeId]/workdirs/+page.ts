import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkingDirectoryListResponse } from "$lib/workspace/api/workdirs";
import type {
  ListResponse,
  Runtime,
  RuntimeCleanupPlanResponse,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimeId = params.runtimeId;
  const [runtimes, workdirs, cleanupPlan] = await Promise.all([
    loadJson<ListResponse<Runtime>>(
      fetch,
      workspaceApiPath(params.workspaceId, "/runtimes"),
    ),
    loadJson(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/runtimes/${encodeURIComponent(runtimeId)}/working-directories`,
      ),
      undefined,
      parseWorkingDirectoryListResponse,
    ),
    loadJson<RuntimeCleanupPlanResponse>(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`,
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
    cleanupPlan: cleanupPlan.data,
    cleanupPlanError: cleanupPlan.error,
  };
};
