import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseWorkerListResponse } from "#lib/workspace/api/workers.ts";
import { parseRuntimeCleanupPlan } from "#lib/workspace/api/runtime-workers.ts";
import type {
  RuntimeCleanupPlanResponse,
} from "#lib/workspace/sidebar/types.ts";
import type { PageLoad } from "./$types";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";

export const load = deferPage(async ({ fetch, params }: Parameters<PageLoad>[0]) => {
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
});
