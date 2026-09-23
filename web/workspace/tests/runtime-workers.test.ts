declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseRuntimeCleanupExecution,
  parseRuntimeCleanupPlan,
  parseRuntimeWorkerLifecycleResult,
  parseWorkerRetentionResponse,
} from "../src/lib/workspace/api/runtime-workers.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertThrows(operation: () => unknown, expected: string): void {
  try {
    operation();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes(expected)) return;
    throw new Error(`expected error containing ${expected}, received ${message}`);
  }
  throw new Error("expected operation to throw");
}

function plan() {
  return {
    workspace_id: "workspace-a",
    runtime_id: "runtime-a",
    generated_at: "2026-09-01T00:00:00Z",
    revision: "revision-a",
    digest: "digest-a",
    workers: [{
      target_id: "worker-target",
      action: "worker_delete",
      worker_id: "W-1",
      runtime_worker_id: "worker-1",
      runtime_id: "runtime-a",
      reason: "stopped",
      blocking_reason: null,
      pinned: false,
      retention_state: "active",
      linked_workdir_ids: [],
      running_linked: false,
      estimated_reclaim_bytes: 10,
    }],
    workdirs: [{
      target_id: "workdir-target",
      action: "workdir_clean_cleanup",
      workdir_id: "workdir-1",
      runtime_id: "runtime-a",
      repository_key: "main",
      reason: "unused",
      blocking_reason: null,
      linked_worker_ids: [],
      linked_running_worker_ids: [],
      running_linked: false,
      pinned_linked: false,
      file_status: "active",
      cleanliness: "clean",
      estimated_reclaim_bytes: null,
    }],
    diagnostics: [],
  };
}

Deno.test("Runtime cleanup parsers accept the generated exact response shapes", () => {
  const parsedPlan = parseRuntimeCleanupPlan(plan());
  assert(parsedPlan.workers[0]?.runtime_worker_id === "worker-1", "worker candidate missing");

  const execution = parseRuntimeCleanupExecution({
    workspace_id: "workspace-a",
    runtime_id: "runtime-a",
    executed_at: "2026-09-01T00:01:00Z",
    results: [{
      target_id: "worker-target",
      action: "worker_delete",
      status: "deleted",
      message: "deleted",
    }],
    plan_after: plan(),
    diagnostics: [],
  });
  assert(execution.results[0]?.status === "deleted", "execution result missing");

  const lifecycle = parseRuntimeWorkerLifecycleResult({
    state: "accepted",
    runtime_id: "runtime-a",
    worker_id: "worker-1",
    diagnostics: [],
  });
  assert(lifecycle.state === "accepted", "lifecycle state missing");

  const retention = parseWorkerRetentionResponse({
    workspace_id: "workspace-a",
    runtime_id: "runtime-a",
    worker_id: "worker-1",
    pinned: true,
    retention_state: "pinned",
  });
  assert(retention.pinned, "retention state missing");
});

Deno.test("Runtime cleanup parsers reject unknown fields and unsafe values", () => {
  assertThrows(
    () => parseRuntimeCleanupPlan({ ...plan(), unexpected: true }),
    "not part of the wire contract",
  );
  assertThrows(
    () => parseRuntimeCleanupPlan({ ...plan(), workers: new Array(1_001).fill(plan().workers[0]) }),
    "at most 1000",
  );
  assertThrows(
    () => parseRuntimeCleanupPlan({ ...plan(), workers: [{ ...plan().workers[0], estimated_reclaim_bytes: Number.MAX_SAFE_INTEGER + 1 }] }),
    "safe integer",
  );
  assertThrows(
    () => parseRuntimeWorkerLifecycleResult({ state: "future", runtime_id: "runtime-a", worker_id: "worker-1", diagnostics: [] }),
    "unknown value",
  );
});
