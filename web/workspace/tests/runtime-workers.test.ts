declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

import {
  decodeRuntimeControl,
  runtimeControlMessage,
} from "../src/lib/workspace/failure/runtime-control.ts";
import { decodeProblem } from "../src/lib/workspace/failure/receive.ts";
import {
  controlProblem,
  startupObservations,
} from "../src/lib/workspace/failure/runtime-control.test-fixtures.ts";
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
Deno.test("Restore parser preserves typed outcomes and optional Workspace summary", () => {
  for (
    const state of [
      "accepted",
      "rejected",
      "rolled_back",
      "superseded",
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
  assertThrows(() =>
    parseWorkerRestoreResponse(restore("accepted", {
      ...restoredWorker(),
      restore_observation_token: "😀".repeat(65),
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

Deno.test("lifecycle replay metadata is optional but validated when present", () => {
  const receipt = {
    state: "accepted",
    runtime_id: "runtime-a",
    worker_id: "worker-1",
    diagnostics: [],
  };
  assert(
    parseRuntimeWorkerLifecycleResult(receipt).replayed === false,
    "missing replay is not a replay",
  );
  assert(
    parseRuntimeWorkerLifecycleResult({ ...receipt, replayed: true })
      .replayed === true,
    "replay flag lost",
  );
  assertThrows(
    () => parseRuntimeWorkerLifecycleResult({ ...receipt, replayed: "true" }),
    "must be a boolean",
  );
});

Deno.test("Runtime control adapter accepts enum details only for the supported domain, reason, version and category", () => {
  const decode = (problem: unknown) =>
    decodeProblem(
      JSON.stringify(problem),
      409,
      "application/problem+json",
      decodeRuntimeControl,
    );
  const known = controlProblem();
  assert(decode(known).kind === "problem", "known control rejected");
  for (
    const patch of [{ domain: "other" }, {
      reason: "future_reason",
      details: "future_reason",
    }, { details_version: 2 }]
  ) {
    assert(
      decode({ ...known, yoi_failure: { ...known.yoi_failure, ...patch } })
        .kind === "unsupported",
      "unknown control treated as known",
    );
  }
  for (
    const patch of [{ details: { cause: "execution_rejected" } }, {
      details: "worker_missing",
    }, { category: "dependency" }]
  ) {
    assert(
      decode({ ...known, yoi_failure: { ...known.yoi_failure, ...patch } })
        .kind === "invalid",
      "incoherent identity/details accepted",
    );
  }
});

Deno.test("Runtime missing is a dependency cause at the HTTP and primary result boundaries without changing settlement", () => {
  const explanation =
    "The requested Runtime is not registered in this Workspace.";
  const problem = controlProblem("runtime_missing", explanation, 404);
  problem.yoi_failure.category = "dependency";
  const receipt = decodeProblem(
    JSON.stringify(problem),
    404,
    "application/problem+json",
    decodeRuntimeControl,
  );
  assert(receipt.kind === "problem", "Runtime missing dependency was rejected");
  assert(
    decodeProblem(
      JSON.stringify({
        ...problem,
        yoi_failure: { ...problem.yoi_failure, category: "conflict" },
      }),
      404,
      "application/problem+json",
      decodeRuntimeControl,
    ).kind === "invalid",
    "Runtime missing was accepted with an incoherent category",
  );
  const primary = {
    control: "runtime_missing",
    control_explanation: explanation,
    diagnostics: [],
  };
  const lifecycle = parseRuntimeWorkerLifecycleResult({
    ...primary,
    state: "rejected",
    replayed: false,
    runtime_id: "runtime-a",
    worker_id: "worker-1",
  });
  const restored = parseWorkerRestoreResponse({
    ...restore("rolled_back"),
    result: { ...primary, state: "rolled_back" },
  }).result;
  assert(
    lifecycle.state === "rejected",
    "missing Runtime changed Stop settlement",
  );
  assert(
    restored.state === "rolled_back",
    "missing Runtime changed Restore settlement",
  );
  for (const result of [lifecycle, restored]) {
    assert(
      result.control === "runtime_missing",
      "primary Runtime cause was lost",
    );
    assert(
      runtimeControlMessage(result, "Operation failed") === explanation,
      "paired owner explanation was lost",
    );
  }
});

Deno.test("primary control explanation is paired with a known cause without changing authoritative lifecycle state", () => {
  for (
    const row of [
      {
        control: "execution_rejected",
        control_explanation: "Owner public explanation",
        expected: "Owner public explanation",
      },
      { control: "execution_rejected", expected: "explanation is unavailable" },
      {
        control: "execution_rejected",
        control_explanation: null,
        expected: "explanation is unavailable",
      },
      {
        control: "execution_rejected",
        control_explanation: "",
        expected: "explanation is unavailable",
      },
      {
        control: "execution_rejected",
        control_explanation: " ",
        expected: "explanation is unavailable",
      },
      {
        control: "execution_rejected",
        control_explanation: { private: "not text" },
        expected: "explanation is unavailable",
      },
      {
        control: "execution_rejected",
        control_explanation: "x".repeat(5000),
        expected: "explanation is unavailable",
      },
      {
        control: "future_cause",
        control_explanation: "private unsupported explanation",
        expected: "unsupported control cause",
      },
      {
        control: null,
        control_explanation: "unpaired legacy explanation",
        expected: "No supported Runtime control cause",
      },
      {
        control_explanation: "unpaired legacy explanation",
        expected: "No supported Runtime control cause",
      },
    ]
  ) {
    for (const state of ["accepted", "rejected"] as const) {
      const { expected, ...fields } = row;
      const result = parseRuntimeWorkerLifecycleResult({
        state,
        runtime_id: "runtime-a",
        worker_id: "worker-a",
        diagnostics: [{
          code: "execution_rejected",
          severity: "error",
          message: "This diagnostic must not become primary",
        }],
        ...fields,
      });
      assert(
        result.state === state,
        "explanation validation changed settlement",
      );
      const message = runtimeControlMessage(result, "Worker operation failed");
      assert(
        message.includes(expected),
        `missing primary explanation/fallback: ${message}`,
      );
      assert(
        !message.includes("diagnostic") && !message.includes("private") &&
          !message.includes("legacy"),
        "unpaired explanation or diagnostic leaked as primary",
      );
      if (fields.control === "execution_rejected") {
        assert(
          result.control === "execution_rejected",
          "invalid explanation discarded a known cause",
        );
      }
    }
  }
});

Deno.test("Worker result diagnostics reject a primary control cause on the generic Diagnostic contract", () => {
  assertThrows(() =>
    parseRuntimeWorkerLifecycleResult({
      state: "rejected",
      runtime_id: "runtime-a",
      worker_id: "worker-a",
      diagnostics: [{
        code: "supplement",
        severity: "info",
        message: "Not a primary cause",
        control: "execution_rejected",
      }],
    }), "is not part of the wire contract");
});

Deno.test("Restore retains startup observations without promoting Session, cleanup or persistence evidence to settlement", () => {
  const startup = startupObservations();
  for (
    const state of [
      "accepted",
      "rejected",
      "rolled_back",
      "superseded",
      "reconciliation_required",
    ]
  ) {
    const result = parseWorkerRestoreResponse({
      ...restore(state),
      result: {
        state,
        startup,
        control: "execution_failed",
        control_explanation: "Session registration permission denied.",
        diagnostics: [{
          code: "cleanup",
          severity: "error",
          message: "Supplement is not primary",
        }],
      },
    }).result;
    assert(
      result.state === state,
      "startup observations changed Restore settlement",
    );
    assert(
      JSON.stringify(result.startup) === JSON.stringify(startup),
      "startup evidence changed",
    );
    assert(
      !result.startupEvidenceUnavailable,
      "actual startup object was not accepted",
    );
    assert(
      result.control === "execution_failed",
      "Session evidence replaced owner control",
    );
    assert(
      runtimeControlMessage(result, "Restore failed") ===
        "Session registration permission denied.",
      "fresh top-level public explanation was not used",
    );
  }
});

Deno.test("Restore startup accepts nullable and omitted evidence and preserves opaque attachments without decoding them for display", () => {
  for (
    const startup of [undefined, null, { primary: "opaque future attachment" }]
  ) {
    const result = parseWorkerRestoreResponse({
      ...restore("reconciliation_required"),
      result: {
        state: "reconciliation_required",
        startup,
        control: "execution_failed",
      },
    }).result;
    assert(
      result.state === "reconciliation_required",
      "optional evidence changed settlement",
    );
    assert(
      !result.startupEvidenceUnavailable,
      "legacy or nullable evidence was rejected",
    );
    assert(
      runtimeControlMessage(result, "Restore failed").includes(
        "explanation is unavailable",
      ),
      "attachment became the primary explanation",
    );
    if (startup && typeof startup === "object") {
      assert(
        result.startup?.primary === startup.primary,
        "opaque attachment was lost",
      );
      assert(
        result.startup.secondary.length === 0 &&
          result.startup.resources === null &&
          result.startup.persistence === null &&
          result.startup.io_kind === null,
        "omitted optional observations were not normalized",
      );
    } else {
      assert(result.startup === startup, "null/absent startup changed");
    }
  }
  // The attachment budget is the shared envelope budget, not a short diagnostic string.
  const primary = "x".repeat(8000);
  const result = parseWorkerRestoreResponse({
    ...restore(),
    result: { state: "accepted", startup: { primary } },
  }).result;
  assert(
    result.startup?.primary === primary,
    "bounded large opaque attachment rejected",
  );
});

Deno.test("Restore startup validates enum observations including externally tagged cleanup failures", () => {
  const startup = startupObservations();
  const rows = [
    ...["persisted", "not_saved", "conflict", "outcome_unknown"].map((
      persistence,
    ) => ({ ...startup, persistence })),
    ...[
      "not_found",
      "permission_denied",
      "already_exists",
      "invalid_input",
      "invalid_data",
      "interrupted",
      "timed_out",
      "would_block",
      "unexpected_eof",
      "other",
    ].map((io_kind) => ({ ...startup, io_kind })),
    ...["none", "worker_startup", "unproven"].map((resources) => ({
      ...startup,
      resources: { ...startup.resources, resources },
    })),
    ...[
      "not_needed",
      "required",
      "confirmed",
      ...[
        "ownership",
        "workdir",
        "artifacts",
        "allocation",
        "feature",
        "unsupported",
      ].map((failed) => ({ failed })),
    ].map((cleanup) => ({
      ...startup,
      resources: { ...startup.resources, cleanup },
    })),
    ...[
      "cleanup",
      "runtime_record_delete",
      "runtime_snapshot_save",
      "runtime_removal_publish",
      "runtime_reconciliation_record_save",
      "runtime_reconciliation_publish",
      "metadata_save",
    ].map((phase) => ({
      ...startup,
      secondary: [{ kind: "projection_unavailable", phase }],
    })),
  ];
  for (const value of rows) {
    const result = parseWorkerRestoreResponse({
      ...restore("rolled_back"),
      result: { state: "rolled_back", startup: value },
    }).result;
    assert(
      !result.startupEvidenceUnavailable &&
        JSON.stringify(result.startup) === JSON.stringify(value),
      "known observation enum rejected or changed",
    );
    assert(
      result.state === "rolled_back",
      "observation enum changed settlement",
    );
  }
});

Deno.test("Restore drops malformed or unsupported optional startup evidence without losing authoritative state or disclosing raw fields", () => {
  const startup = startupObservations();
  const invalid = [
    "not an object",
    [],
    {},
    { ...startup, primary: {} },
    { ...startup, primary: "" },
    { ...startup, primary: "bad\u0000attachment" },
    { ...startup, primary: "界".repeat(11000) },
    { ...startup, secondary: null },
    {
      ...startup,
      secondary: new Array(65).fill({ kind: "cleanup", failure: "workdir" }),
    },
    { ...startup, secondary: [{ kind: "future" }] },
    {
      ...startup,
      secondary: [{ kind: "projection_unavailable", phase: "future" }],
    },
    { ...startup, secondary: [{ kind: "cleanup", failure: "future" }] },
    {
      ...startup,
      secondary: [{
        kind: "persistence",
        phase: "metadata_save",
        outcome: "future",
      }],
    },
    {
      ...startup,
      resources: {
        resources: "future",
        cleanup: "confirmed",
        durable_effects_possible: false,
      },
    },
    {
      ...startup,
      resources: {
        resources: "none",
        cleanup: "failed",
        durable_effects_possible: false,
      },
    },
    {
      ...startup,
      resources: {
        resources: "none",
        cleanup: { failed: "future" },
        durable_effects_possible: false,
      },
    },
    {
      ...startup,
      resources: {
        resources: "none",
        cleanup: "confirmed",
        durable_effects_possible: "false",
      },
    },
    { ...startup, persistence: "future" },
    { ...startup, io_kind: "future" },
    { ...startup, private_raw: "not public" },
  ];
  for (const value of invalid) {
    const result = parseWorkerRestoreResponse({
      ...restore("superseded"),
      result: {
        state: "superseded",
        startup: value,
        control: "execution_failed",
        control_explanation: "Public owner explanation",
      },
    }).result;
    assert(
      result.state === "superseded" && result.control === "execution_failed",
      "unreadable evidence changed authoritative result",
    );
    assert(
      result.startup === undefined && result.startupEvidenceUnavailable,
      "unsafe startup fields were retained without a marker",
    );
    assert(
      runtimeControlMessage(result, "Restore failed") ===
        "Public owner explanation",
      "unreadable startup overrode primary display",
    );
  }
});

Deno.test("generated Runtime primary control contract includes cause and owner explanation on all three results, never Diagnostic", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/lib/generated/runtime-api.ts", import.meta.url),
  );
  for (
    const name of [
      "RuntimeWorkerInputResult",
      "RuntimeWorkerLifecycleResult",
      "WorkerRestoreResult",
    ]
  ) {
    const declaration = source.match(
      new RegExp(`export type ${name} = \\{([^}]+)\\};`),
    )?.[1];
    assert(
      declaration &&
        /control\?\s*:\s*RuntimeControlCause\s*\|\s*null/.test(declaration),
      `${name} generated primary cause is stale`,
    );
    assert(
      /control_explanation\?\s*:\s*string\s*\|\s*null/.test(declaration),
      `${name} generated owner explanation is stale`,
    );
  }
  const diagnostic = source.match(/export type Diagnostic = \{([^}]+)\};/)?.[1];
  assert(
    diagnostic && !/\bcontrol(?:_explanation)?\??\s*:/.test(diagnostic),
    "generic Diagnostic gained primary control fields",
  );
});

Deno.test("Restore owner-produced wire preserves Session cause, cleanup and persistence independently", async () => {
  const payload = JSON.parse(
    await Deno.readTextFile(
      new URL(
        "../../../crates/error-contract/fixtures/runtime-restore-session-denied.json",
        import.meta.url,
      ),
    ),
  );
  const response = parseWorkerRestoreResponse(payload);
  assert(
    response.result.state === "rolled_back",
    "operation settlement was reclassified",
  );
  assert(
    response.result.control === "execution_rejected",
    "coarse control cause was lost",
  );
  const startup = response.result.startup;
  assert(
    startup != null && !response.result.startupEvidenceUnavailable,
    "actual Serde startup shape was rejected",
  );
  const primary = JSON.parse(startup.primary);
  assert(
    primary.domain === "session_registration" &&
      primary.reason === "permission_denied",
    "specific Session primary was replaced",
  );
  assert(
    startup.persistence === "outcome_unknown",
    "independent persistence outcome was lost",
  );
  assert(
    startup.resources?.cleanup &&
      typeof startup.resources.cleanup === "object" &&
      startup.resources.cleanup.failed === "workdir",
    "cleanup was merged into the primary",
  );
  assert(
    runtimeControlMessage(response.result, "fallback") ===
      payload.result.control_explanation,
    "owner explanation did not reach display",
  );
});
