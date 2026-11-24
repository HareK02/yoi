import type { WorkerRestoreRequest } from "#lib/generated/runtime-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  RuntimeControlHttpError,
  runtimeControlHttpError as responseError,
  runtimeControlMessage,
} from "#lib/workspace/failure/runtime-control.ts";
import {
  parseRuntimeCleanupExecution,
  parseRuntimeCleanupPlan,
  parseRuntimeWorkerLifecycleResult,
  parseWorkerRestoreResponse,
  type WorkdirCleanupReceipt,
} from "#lib/workspace/api/runtime-workers.ts";
import type { Worker } from "./types";
import {
  type WorkerDeletionOutcome,
  workerDeletionOutcome,
} from "#lib/workspace/workers/worker-removal.ts";

type FetchFn = typeof fetch;
type WorkerActionTarget = Pick<Worker, "runtime_id" | "worker_id" | "state">;

function workerPath(workspaceId: string, worker: WorkerActionTarget): string {
  return workspaceApiPath(
    workspaceId,
    `/runtimes/${encodeURIComponent(worker.runtime_id)}/workers/${
      encodeURIComponent(worker.worker_id)
    }`,
  );
}

type WorkerLifecycleObservation = Pick<Worker, "availability" | "lifecycle"> & {
  state?: string;
};

function isSettledInactiveWorker(worker: WorkerLifecycleObservation): boolean {
  return worker.availability === "observed" &&
    (worker.lifecycle === "not_started" || worker.lifecycle === "stopped");
}

export function canRestoreWorker(
  worker: WorkerLifecycleObservation & { restore_observation_token?: string | null },
): boolean {
  return isSettledInactiveWorker(worker) &&
    Boolean(worker.restore_observation_token);
}

/** Immutable intent: retained unchanged for explicit uncertain-result recovery. */
export type RestoreRequest = WorkerRestoreRequest;

export function createRestoreRequest(
  worker: { restore_observation_token?: string | null },
): RestoreRequest {
  if (!worker.restore_observation_token) {
    throw new Error(
      "Worker Restore observation is unavailable; refresh Workers",
    );
  }
  return {
    expected_observation_token: worker.restore_observation_token,
    request_id: crypto.randomUUID(),
  };
}

export { RuntimeControlHttpError as WorkerActionHttpError };

export async function restoreWorkspaceWorker(
  workspaceId: string,
  worker: WorkerActionTarget,
  request: RestoreRequest,
  fetchFn: FetchFn = fetch,
) {
  const response = await fetchFn(`${workerPath(workspaceId, worker)}/restore`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request),
  });
  if (!response.ok) {
    throw await responseError(response);
  }
  const result = parseWorkerRestoreResponse(
    await readBoundedJson(response, 8 * 1024 * 1024),
  );
  if (
    result.workspace_id !== workspaceId ||
    result.runtime_id !== worker.runtime_id ||
    result.worker_id !== worker.worker_id
  ) {
    throw new Error(
      "Worker Restore response does not match the requested target",
    );
  }
  return result;
}

export type RestoreNotice = {
  level: "info" | "warning" | "error";
  title: string;
  message: string;
  retry: boolean;
};

/** Shared classification: neither an HTTP 200 nor a GET settles an unknown operation. */
export function restoreNotice(
  result: Awaited<ReturnType<typeof restoreWorkspaceWorker>>,
): RestoreNotice {
  const state = result.result.state;
  if (state === "accepted") {
    return {
      level: "info",
      title: "Worker restored",
      message: "Worker restored. Open Console to continue.",
      retry: false,
    };
  }
  const pending = state === "reconciliation_required";
  return {
    level: pending ? "warning" : "error",
    title: pending
      ? "Worker Restore needs reconciliation"
      : "Worker Restore failed",
    message: runtimeControlMessage(
      result.result,
      `Worker Restore was ${state}`,
    ) +
      (pending
        ? ". Outcome is unresolved. Refresh observations and explicitly retry the same Restore to reconcile; refreshing alone does not settle it."
        : ""),
    retry: pending,
  };
}

export function restoreErrorNotice(cause: unknown): RestoreNotice {
  const conflict = cause instanceof RuntimeControlHttpError &&
    cause.status === 409;
  const uncertain = !(cause instanceof RuntimeControlHttpError) ||
    cause.status >= 500;
  return {
    level: conflict || uncertain ? "warning" : "error",
    title: conflict
      ? "Worker observation changed"
      : uncertain
      ? "Worker Restore outcome unknown"
      : "Worker Restore failed",
    message: (cause instanceof Error ? cause.message : "Worker Restore request failed") +
      (conflict
        ? ". Workers refreshed; review the latest state and explicitly try again."
        : uncertain
        ? ". Outcome is unknown. Check the latest state and explicitly retry the same Restore; no automatic retry was made."
        : ""),
    retry: uncertain,
  };
}

export function canStopSidebarWorker(worker: WorkerLifecycleObservation): boolean {
  return worker.availability === "observed" && worker.lifecycle === "active";
}

export function canDeleteSidebarWorker(worker: WorkerLifecycleObservation): boolean {
  return isSettledInactiveWorker(worker);
}

export async function stopSidebarWorker(
  workspaceId: string,
  worker: WorkerActionTarget & { restore_observation_token?: string | null },
  fetchFn: FetchFn = fetch,
): Promise<WorkdirCleanupReceipt[]> {
  const response = await fetchFn(`${workerPath(workspaceId, worker)}/stop`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      reason: "stopped from Workspace sidebar",
      ...(worker.restore_observation_token
        ? {
          stop_intent: {
            request_id: crypto.randomUUID(),
            expected_observation_token: worker.restore_observation_token,
          },
        }
        : {}),
    }),
  });
  if (!response.ok) throw await responseError(response);

  const result = parseRuntimeWorkerLifecycleResult(await readBoundedJson(response, 1024 * 1024));
  if (result.runtime_id !== worker.runtime_id || result.worker_id !== worker.worker_id) {
    throw new Error(
      "Worker Stop response identity does not match the requested Worker; inspect current state before recovery",
    );
  }
  if (result.state !== "accepted") {
    throw new Error(
      runtimeControlMessage(result, `Worker stop was ${result.state}`),
    );
  }
  return result.cleanup_failures;
}

export function stopCleanupMessage(receipts: WorkdirCleanupReceipt[]): string {
  const explanations = receipts.map((receipt) =>
    receipt.kind === "known"
      ? receipt.failure.explanation
      : "Cleanup failure details could not be interpreted safely"
  );
  return `${
    [...new Set(explanations)].join("; ")
  }. Stop remains accepted. Inspect retained Workdir sessions before recovery; do not repeat Stop to retry cleanup.`;
}

export async function deleteSidebarWorker(
  workspaceId: string,
  worker: WorkerActionTarget,
  fetchFn: FetchFn = fetch,
): Promise<WorkerDeletionOutcome> {
  const runtimePath = `/runtimes/${encodeURIComponent(worker.runtime_id)}`;
  const planResponse = await fetchFn(
    workspaceApiPath(workspaceId, `${runtimePath}/cleanup-plan`),
  );
  if (!planResponse.ok) throw await responseError(planResponse);

  const plan = parseRuntimeCleanupPlan(await planResponse.json());
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
        expected_plan_digest: plan.digest,
        worker_target_ids: [candidate.target_id],
        workdir_target_ids: [],
        confirm_dirty_discard_target_ids: [],
      }),
    },
  );
  if (!executionResponse.ok) {
    throw await responseError(executionResponse);
  }

  const execution = parseRuntimeCleanupExecution(
    await executionResponse.json(),
  );
  const outcome = execution.results.find((result) =>
    result.target_id === candidate.target_id
  );
  return workerDeletionOutcome(outcome);
}
