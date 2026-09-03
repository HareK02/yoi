import { workspaceApiPath } from "$lib/workspace/api/http";
import type {
  Diagnostic,
  RuntimeCleanupExecutionResponse,
  RuntimeCleanupPlanResponse,
  Worker,
} from "./types";

type FetchFn = typeof fetch;

type WorkerLifecycleResponse = {
  state: string;
  diagnostics?: Diagnostic[];
};

function workerPath(workspaceId: string, worker: Worker): string {
  return workspaceApiPath(
    workspaceId,
    `/runtimes/${encodeURIComponent(worker.runtime_id)}/workers/${
      encodeURIComponent(worker.worker_id)
    }`,
  );
}

async function responseError(response: Response): Promise<string> {
  const fallback = `${response.status} ${response.statusText}`.trim();
  try {
    const payload = await response.json() as {
      message?: string;
      error?: { message?: string };
    };
    return payload.error?.message ?? payload.message ?? fallback;
  } catch {
    return fallback;
  }
}

function diagnosticMessage(
  diagnostics: Diagnostic[] | undefined,
  fallback: string,
): string {
  return diagnostics?.find((diagnostic) => diagnostic.severity === "error")
    ?.message ??
    diagnostics?.[0]?.message ??
    fallback;
}

export function canDeleteSidebarWorker(worker: Worker): boolean {
  return worker.state === "stopped" || worker.state === "cancelled";
}

export async function stopSidebarWorker(
  workspaceId: string,
  worker: Worker,
  fetchFn: FetchFn = fetch,
): Promise<void> {
  const response = await fetchFn(`${workerPath(workspaceId, worker)}/stop`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reason: "stopped from Workspace sidebar" }),
  });
  if (!response.ok) throw new Error(await responseError(response));

  const result = await response.json() as WorkerLifecycleResponse;
  if (result.state !== "accepted") {
    throw new Error(
      diagnosticMessage(result.diagnostics, `Worker stop was ${result.state}`),
    );
  }
}

export async function deleteSidebarWorker(
  workspaceId: string,
  worker: Worker,
  fetchFn: FetchFn = fetch,
): Promise<void> {
  const runtimePath = `/runtimes/${encodeURIComponent(worker.runtime_id)}`;
  const planResponse = await fetchFn(
    workspaceApiPath(workspaceId, `${runtimePath}/cleanup-plan`),
  );
  if (!planResponse.ok) throw new Error(await responseError(planResponse));

  const plan = await planResponse.json() as RuntimeCleanupPlanResponse;
  const candidate = plan.workers.find((item) =>
    item.runtime_id === worker.runtime_id &&
    item.runtime_worker_id === worker.worker_id
  );
  if (!candidate) throw new Error("Worker is not available for deletion");
  if (candidate.blocking_reason) throw new Error(candidate.blocking_reason);

  const executionResponse = await fetchFn(
    workspaceApiPath(workspaceId, `${runtimePath}/cleanup-executions`),
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        expected_plan_revision: plan.revision,
        expected_plan_digest: plan.digest,
        worker_target_ids: [candidate.target_id],
        workdir_target_ids: [],
        confirm_dirty_discard_target_ids: [],
      }),
    },
  );
  if (!executionResponse.ok) {
    throw new Error(await responseError(executionResponse));
  }

  const execution = await executionResponse
    .json() as RuntimeCleanupExecutionResponse;
  const outcome = execution.results.find((result) =>
    result.target_id === candidate.target_id
  );
  if (!outcome || outcome.status !== "deleted") {
    throw new Error(
      outcome?.message ??
        diagnosticMessage(
          execution.diagnostics,
          "Worker deletion was not completed",
        ),
    );
  }
}
