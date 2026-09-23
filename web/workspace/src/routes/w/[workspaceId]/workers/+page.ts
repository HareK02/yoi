import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkerListResponse } from "$lib/workspace/api/workers";
import { parseRuntimeCleanupPlan } from "$lib/workspace/api/runtime-workers";
import type {
  RuntimeCleanupPlanResponse,
} from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workers = await loadJson(
    fetch,
    workspaceApiPath(params.workspaceId, "/workers"),
    undefined,
    parseWorkerListResponse,
    { diagnosticLabel: "Worker API", maxResponseBytes: 8 * 1024 * 1024 },
  );
  const runtimeIds = Array.from(
    new Set(workers.data?.items.map((worker) => worker.runtime_id) ?? []),
  );
  const cleanupPlanEntries = await Promise.all(
    runtimeIds.map(async (runtimeId) => {
      const cleanupPlan = await loadJson(
        fetch,
        workspaceApiPath(
          params.workspaceId,
          `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`,
        ),
        undefined,
        parseRuntimeCleanupPlan,
      );
      return [runtimeId, cleanupPlan] as const;
    }),
  );
  const cleanupPlans: Record<string, RuntimeCleanupPlanResponse> = {};
  const cleanupPlanErrors: Record<string, string> = {};
  for (const [runtimeId, cleanupPlan] of cleanupPlanEntries) {
    if (cleanupPlan.data) cleanupPlans[runtimeId] = cleanupPlan.data;
    if (cleanupPlan.error) cleanupPlanErrors[runtimeId] = cleanupPlan.error;
  }

  return {
    workspaceId: params.workspaceId,
    workers: workers.data,
    workersError: workers.error,
    cleanupPlans,
    cleanupPlanErrors,
  };
};
