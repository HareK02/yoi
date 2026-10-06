declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import {
  parseRuntimeCleanupExecution,
  parseRuntimeCleanupPlan,
  parseRuntimeWorkerLifecycleResult,
  parseWorkerRestoreResponse,
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
    throw new Error(
      `expected error containing ${expected}, received ${message}`,
    );
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
  assert(
    parsedPlan.workers[0]?.runtime_worker_id === "worker-1",
    "worker candidate missing",
  );

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
  assert(
    execution.results[0]?.status === "deleted",
    "execution result missing",
  );

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
    () =>
      parseRuntimeCleanupPlan({
        ...plan(),
        workers: new Array(1_001).fill(plan().workers[0]),
      }),
    "at most 1000",
  );
  assertThrows(
    () =>
      parseRuntimeCleanupPlan({
        ...plan(),
        workers: [{
          ...plan().workers[0],
          estimated_reclaim_bytes: Number.MAX_SAFE_INTEGER + 1,
        }],
      }),
    "safe integer",
  );
  assertThrows(
    () =>
      parseRuntimeWorkerLifecycleResult({
        state: "future",
        runtime_id: "runtime-a",
        worker_id: "worker-1",
        diagnostics: [],
      }),
    "unknown value",
  );
});

function restore(state = "accepted", worker?: unknown) {
  return {
    workspace_id: "workspace-a",
    runtime_id: "runtime-a",
    worker_id: "worker-1",
    result: { state, diagnostics: [], worker },
  };
}
function restoredWorker() {
  return {
    runtime_id: "runtime-a",
    worker_id: "worker-1",
    resource_key: "W-1",
    host_id: "runtime-a",
    label: "Worker",
    availability: "observed",
    state: "idle",
    workspace: {
      visibility: "workspace",
      identity: "registered",
      workspace_id: "workspace-a",
    },
    implementation: { kind: "runtime", display_hint: "Runtime" },
    restore_observation_token: "generation-b",
  };
}
Deno.test("Restore parser preserves all four states and optional Workspace summary", () => {
  for (
    const state of [
      "accepted",
      "rejected",
      "rolled_back",
      "reconciliation_required",
    ]
  ) {
    assert(
      parseWorkerRestoreResponse(restore(state)).result.state === state,
      "state lost",
    );
    assert(
      parseWorkerRestoreResponse(restore(state, null)).result.worker === null,
      "null lost",
    );
  }
  const parsed = parseWorkerRestoreResponse(
    restore("accepted", restoredWorker()),
  );
  assert(
    parsed.result.worker?.resource_key === "W-1",
    "Workspace summary lost",
  );
  assert(parsed.result.worker?.state === "idle", "must not force running");
});
Deno.test("Restore parser bounds diagnostics, validates wrapper/summary identity and rejects malformed results", () => {
  assertThrows(() => parseWorkerRestoreResponse(restore("accepted", {
    ...restoredWorker(), restore_observation_token: "😀".repeat(65),
  })), "256 UTF-8 byte limit");
  assertThrows(
    () => parseWorkerRestoreResponse(restore("future")),
    "unknown value",
  );
  assertThrows(
    () => parseWorkerRestoreResponse({ ...restore(), runtime_id: null }),
    "must be a string",
  );
  assertThrows(
    () => parseWorkerRestoreResponse({ ...restore(), extra: true }),
    "not part of the wire contract",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse({
        ...restore(),
        result: {
          state: "accepted",
          diagnostics: new Array(65).fill({
            code: "e",
            severity: "error",
            message: "e",
          }),
        },
      }),
    "at most 64",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse({
        ...restore(),
        result: {
          state: "rejected",
          diagnostics: [{
            code: "e",
            severity: "error",
            message: "x".repeat(4097),
          }],
        },
      }),
    "4096",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse({
        ...restore(),
        result: {
          state: "rejected",
          diagnostics: [{ code: "e", severity: "invalid", message: "e" }],
        },
      }),
    "unknown value",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse(
        restore("accepted", { ...restoredWorker(), worker_id: "other" }),
      ),
    "identity",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse(
        restore("accepted", {
          ...restoredWorker(),
          workspace: {
            visibility: "workspace",
            identity: "registered",
            workspace_id: "other",
          },
        }),
      ),
    "identity",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse(
        restore("accepted", { worker_id: "worker-1", state: "idle" }),
      ),
    "availability",
  );
  assertThrows(
    () =>
      parseWorkerRestoreResponse({
        state: "accepted",
        runtime_id: "runtime-a",
        worker_id: "worker-1",
        diagnostics: [],
      }),
    "wire contract",
  );
});
