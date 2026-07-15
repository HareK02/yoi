import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import type { ListResponse, RuntimeCleanupPlanResponse, Worker } from "$lib/workspace/sidebar/types";
import type { PageLoad } from "./$types";

export const load: PageLoad = async ({ fetch, params }) => {
  const workers = await loadJson<ListResponse<Worker>>(
    fetch,
    workspaceApiPath(params.workspaceId, "/workers"),
  );
  const runtimeIds = Array.from(new Set(workers.data?.items.map((worker) => worker.runtime_id) ?? []));
  const cleanupPlanEntries = await Promise.all(
    runtimeIds.map(async (runtimeId) => {
      const cleanupPlan = await loadJson<RuntimeCleanupPlanResponse>(
        fetch,
        workspaceApiPath(params.workspaceId, `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`),
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
