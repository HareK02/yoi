import type { WorkerRestoreRequest } from "#lib/generated/runtime-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  parseRuntimeCleanupExecution,
  parseRuntimeCleanupPlan,
  parseRuntimeWorkerLifecycleResult,
  parseWorkerRestoreResponse,
} from "#lib/workspace/api/runtime-workers.ts";
import type { Diagnostic, Worker } from "./types";

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

async function responseError(response: Response): Promise<string> {
  const fallback = `${response.status} ${response.statusText}`.trim();
  try {
    const payload: unknown = await readBoundedJson(response, 64 * 1024);
    if (
      typeof payload !== "object" || payload === null || Array.isArray(payload)
    ) {
      return fallback;
    }
    const record = payload as Record<string, unknown>;
    if (typeof record.message === "string") return record.message.slice(0, 512);
    const nested = record.error;
    if (
      typeof nested === "object" && nested !== null && !Array.isArray(nested)
    ) {
      const message = (nested as Record<string, unknown>).message;
      if (typeof message === "string") return message.slice(0, 512);
    }
    return fallback;
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

export function canRestoreWorker(worker: {
  availability: Worker["availability"];
  state?: string;
  lifecycleState?: string;
  restore_observation_token?: string | null;
}): boolean {
  return worker.availability === "observed" &&
    (worker.lifecycleState ?? worker.state) === "stopped" &&
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

export class WorkerActionHttpError extends Error {
  constructor(public readonly status: number, message: string) {
    super(message);
  }
}

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
    throw new WorkerActionHttpError(
      response.status,
      await responseError(response),
    );
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
    message: diagnosticMessage(
      result.result.diagnostics,
      `Worker Restore was ${state}`,
    ) +
      (pending
        ? ". Outcome is unresolved. Refresh observations and explicitly retry the same Restore to reconcile; refreshing alone does not settle it."
        : ""),
    retry: pending,
  };
}

export function restoreErrorNotice(cause: unknown): RestoreNotice {
  const conflict = cause instanceof WorkerActionHttpError &&
    cause.status === 409;
  const uncertain = !(cause instanceof WorkerActionHttpError) ||
    cause.status >= 500;
  return {
    level: conflict || uncertain ? "warning" : "error",
    title: conflict
      ? "Worker observation changed"
      : uncertain
      ? "Worker Restore outcome unknown"
      : "Worker Restore failed",
    message:
      (cause instanceof Error
        ? cause.message
        : "Worker Restore request failed") +
      (conflict
        ? ". Workers refreshed; review the latest state and explicitly try again."
        : uncertain
        ? ". Outcome is unknown. Check the latest state and explicitly retry the same Restore; no automatic retry was made."
        : ""),
    retry: uncertain,
  };
}

export function canStopSidebarWorker(worker: {
  availability: Worker["availability"];
  lifecycleState: string;
}): boolean {
  return worker.availability === "observed" &&
    ["idle", "running", "paused"].includes(worker.lifecycleState);
}

export function canDeleteSidebarWorker(worker: WorkerActionTarget): boolean {
  return worker.state === "stopped";
}

export async function stopSidebarWorker(
  workspaceId: string,
  worker: WorkerActionTarget,
  fetchFn: FetchFn = fetch,
): Promise<void> {
  const response = await fetchFn(`${workerPath(workspaceId, worker)}/stop`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reason: "stopped from Workspace sidebar" }),
  });
  if (!response.ok) throw new Error(await responseError(response));

  const result = parseRuntimeWorkerLifecycleResult(await response.json());
  if (result.state !== "accepted") {
    throw new Error(
      diagnosticMessage(result.diagnostics, `Worker stop was ${result.state}`),
    );
  }
}

export async function deleteSidebarWorker(
  workspaceId: string,
  worker: WorkerActionTarget,
  fetchFn: FetchFn = fetch,
): Promise<void> {
  const runtimePath = `/runtimes/${encodeURIComponent(worker.runtime_id)}`;
  const planResponse = await fetchFn(
    workspaceApiPath(workspaceId, `${runtimePath}/cleanup-plan`),
  );
  if (!planResponse.ok) throw new Error(await responseError(planResponse));

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
    throw new Error(await responseError(executionResponse));
  }

  const execution = parseRuntimeCleanupExecution(
    await executionResponse.json(),
  );
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
