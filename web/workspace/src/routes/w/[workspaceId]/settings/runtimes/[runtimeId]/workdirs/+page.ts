import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseWorkspaceRuntimeList } from "#lib/workspace/api/runtime-management.ts";
import { parseRuntimeCleanupPlan } from "#lib/workspace/api/runtime-workers.ts";
import { parseWorkingDirectoryListResponse } from "#lib/workspace/api/workdirs.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const runtimeId = params.runtimeId;
  const [runtimes, workdirs, cleanupPlan] = await Promise.all([
    loadJson(
      fetch,
      workspaceApiPath(params.workspaceId, "/runtimes"),
      undefined,
      parseWorkspaceRuntimeList,
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
    loadJson(
      fetch,
      workspaceApiPath(
        params.workspaceId,
        `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`,
      ),
      undefined,
      parseRuntimeCleanupPlan,
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
