import {
  controlProblem,
  controlProblemResponse,
  startupObservations,
} from "../../src/lib/workspace/failure/runtime-control.test-fixtures.ts";
import { RuntimeControlHttpError } from "../../src/lib/workspace/failure/runtime-control.ts";
import {
  canDeleteSidebarWorker,
  canRestoreWorker,
  canStopSidebarWorker,
  createRestoreRequest,
  deleteSidebarWorker,
  restoreErrorNotice,
  restoreNotice,
  restoreWorkspaceWorker,
  stopCleanupMessage,
  stopSidebarWorker,
} from "../../src/lib/workspace/sidebar/worker-actions.ts";
import type { Worker } from "../../src/lib/workspace/sidebar/types.ts";
import {
  deletionNotice,
  workerDeletionOutcome,
} from "../../src/lib/workspace/workers/worker-removal.ts";
import { parseRuntimeCleanupExecution } from "../../src/lib/workspace/api/runtime-workers.ts";

function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals(actual: unknown, expected: unknown): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`expected ${expectedJson}, received ${actualJson}`);
  }
}

async function assertRejects(
  operation: () => Promise<unknown>,
  message: string,
): Promise<void> {
  try {
    await operation();
  } catch (cause) {
    assert(cause instanceof Error, "expected an Error");
    assert(
      cause.message.includes(message),
      `expected error containing ${message}`,
    );
    return;
  }
  throw new Error("expected operation to reject");
}

const worker = {
  runtime_id: "runtime /",
  worker_id: "worker /",
  state: "running",
} as Worker;

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "content-type": "application/json" },
  });
}

Deno.test("sidebar actions use canonical lifecycle and observed availability, never legacy status", () => {
  const cases = [
    ["not_started", true, false],
    ["stopped", true, false],
    ["active", false, true],
    ["awaiting_restore", false, false],
    ["creating", false, false],
    ["restoring", false, false],
    ["stopping", false, false],
    ["recovery_required", false, false],
    [null, false, false],
    [undefined, false, false],
  ] as const;
  for (const [lifecycle, settled, active] of cases) {
    for (const state of ["stopped", "idle", "running", "unknown"]) {
      for (const availability of ["observed", "unavailable"] as const) {
        const target = {
          availability,
          lifecycle,
          state,
          restore_observation_token: "generation",
        };
        assertEquals(canRestoreWorker(target), availability === "observed" && settled);
        assertEquals(canDeleteSidebarWorker(target), availability === "observed" && settled);
        assertEquals(canStopSidebarWorker(target), availability === "observed" && active);
        assert(!canRestoreWorker({ ...target, restore_observation_token: null }));
      }
    }
  }
  for (const availability of [undefined, null, "unknown", "stale"]) {
    const target = {
      availability,
      lifecycle: "stopped",
      state: "stopped",
      restore_observation_token: "generation",
    } as Worker;
    assert(!canRestoreWorker(target));
    assert(!canDeleteSidebarWorker(target));
    assert(!canStopSidebarWorker({ ...target, lifecycle: "active" }));
  }
  for (const lifecycle of ["inactive", "unknown", "stale", "future_lifecycle"]) {
    const target = {
      availability: "observed",
      lifecycle,
      state: "stopped",
      restore_observation_token: "generation",
    } as Worker;
    assert(!canRestoreWorker(target));
    assert(!canDeleteSidebarWorker(target));
    assert(!canStopSidebarWorker(target));
  }
});

Deno.test("sidebar Stop uses the workspace-scoped Worker lifecycle endpoint", async () => {
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: input.toString(), init });
    return jsonResponse({
      state: "accepted",
      runtime_id: worker.runtime_id,
      worker_id: worker.worker_id,
      diagnostics: [],
    });
  }) as typeof fetch;

  await stopSidebarWorker("team space", worker, fetchFn);

  assertEquals(requests.length, 1);
  assertEquals(
    requests[0]?.url,
    "/api/w/team%20space/runtimes/runtime%20%2F/workers/worker%20%2F/stop",
  );
  assertEquals(requests[0]?.init?.method, "POST");
  assertEquals(JSON.parse(String(requests[0]?.init?.body)), {
    reason: "stopped from Workspace sidebar",
  });
});

Deno.test("accepted Stop retains cleanup cause without turning its explanation into retry authority", async () => {
  for (
    const explanation of ["provider session closing failed", "different authorized explanation"]
  ) {
    let requests = 0;
    const cleanup = JSON.stringify({
      version: 1,
      required_extensions: [],
      domain: "workdir",
      reason: "provider_unavailable",
      category: "dependency",
      details_version: 1,
      details: {
        operation: "session_close",
        classification: "unavailable",
        denial_reason: null,
        failure_reason: "provider_unavailable",
      },
      explanation,
      context: {
        operations: [],
        causes: [],
        diagnostics: [],
        correlation: null,
        supplementary_omitted: false,
      },
    });
    const fetchFn = (() => {
      requests++;
      return Promise.resolve(
        jsonResponse({
          state: "accepted",
          runtime_id: worker.runtime_id,
          worker_id: worker.worker_id,
          diagnostics: [],
          cleanup_failures: [cleanup],
        }),
      );
    }) as typeof fetch;
    const receipts = await stopSidebarWorker("team", worker, fetchFn);
    assertEquals(receipts.length, 1);
    const received = receipts[0];
    assert(received.kind === "known");
    assertEquals(received.failure.reason, "provider_unavailable");
    assertEquals(received.failure.details, {
      operation: "session_close",
      classification: "unavailable",
      denial_reason: null,
      failure_reason: "provider_unavailable",
    });
    assert(stopCleanupMessage(receipts).includes(explanation));
    assert(stopCleanupMessage(receipts).includes("Stop remains accepted"));
    assert(stopCleanupMessage(receipts).includes("do not repeat Stop"));
    assertEquals(requests, 1);
  }
});

Deno.test("unreadable optional cleanup evidence preserves accepted Stop without exposing received bytes", async () => {
  const future = JSON.stringify({
    version: 2,
    required_extensions: [],
    domain: "future-domain",
    reason: "future-reason",
    category: "dependency",
    details_version: 2,
    details: {},
    explanation: "provider-private /secret/path",
    context: {
      operations: [],
      causes: [],
      diagnostics: [],
      correlation: null,
      supplementary_omitted: false,
    },
  });
  for (
    const cleanup_failures of [
      ["provider-private /secret/path"],
      [future],
      [{}],
      null,
      Array(65).fill("private"),
    ]
  ) {
    let requests = 0;
    const fetchFn = (() => {
      requests++;
      return Promise.resolve(
        jsonResponse({
          state: "accepted",
          runtime_id: worker.runtime_id,
          worker_id: worker.worker_id,
          diagnostics: [],
          cleanup_failures,
        }),
      );
    }) as typeof fetch;
    const receipts = await stopSidebarWorker("team", worker, fetchFn);
    assert(receipts.length > 0);
    assert(receipts.every((receipt) => receipt.kind !== "known"));
    const message = stopCleanupMessage(receipts);
    assert(message.includes("Stop remains accepted"));
    assert(!message.includes("private"));
    assert(!message.includes("/secret/path"));
    assertEquals(requests, 1);
  }
});

Deno.test("sidebar Stop rejects a response for another Worker without sending another mutation", async () => {
  let requests = 0;
  const fetchFn = (() => {
    requests++;
    return Promise.resolve(
      jsonResponse({
        state: "accepted",
        runtime_id: "another-runtime",
        worker_id: worker.worker_id,
        diagnostics: [],
      }),
    );
  }) as typeof fetch;
  await assertRejects(() => stopSidebarWorker("team", worker, fetchFn), "identity does not match");
  assertEquals(requests, 1);
});

Deno.test("sidebar Stop rejects non-accepted lifecycle responses", async () => {
  const fetchFn = (() =>
    Promise.resolve(
      jsonResponse({
        state: "rejected",
        runtime_id: worker.runtime_id,
        worker_id: worker.worker_id,
        diagnostics: [{
          code: "cannot_stop",
          severity: "error",
          message: "Worker cannot stop",
        }],
      }),
    )) as typeof fetch;

  await assertRejects(
    () => stopSidebarWorker("workspace", worker, fetchFn),
    "Worker stop was rejected",
  );
});

Deno.test("sidebar Delete executes the authoritative runtime cleanup plan", async () => {
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: input.toString(), init });
    if (requests.length === 1) {
      return jsonResponse({
        workspace_id: "team",
        runtime_id: worker.runtime_id,
        generated_at: "2026-01-01T00:00:00Z",

        digest: "digest-7",
        workers: [{
          target_id: "worker-target",
          action: "worker_delete",
          worker_id: worker.worker_id,
          runtime_id: worker.runtime_id,
          runtime_worker_id: worker.worker_id,
          reason: "stopped worker",
          blocking_reason: null,
          pinned: false,
          retention_state: "stopped",
          linked_workdir_ids: [],
          running_linked: false,
        }],
        workdirs: [],
        diagnostics: [],
      });
    }
    return jsonResponse({
      workspace_id: "team",
      runtime_id: worker.runtime_id,
      executed_at: "2026-01-01T00:00:01Z",
      results: [{
        target_id: "worker-target",
        action: "worker_delete",
        status: "deleted",
        message: "",
      }],
      plan_after: {
        workspace_id: "team",
        runtime_id: worker.runtime_id,
        generated_at: "2026-01-01T00:00:01Z",

        digest: "digest-8",
        workers: [],
        workdirs: [],
        diagnostics: [],
      },
      diagnostics: [],
    });
  }) as typeof fetch;

  await deleteSidebarWorker("team", { ...worker, state: "stopped" }, fetchFn);

  assertEquals(requests.map((request) => request.url), [
    "/api/w/team/runtimes/runtime%20%2F/cleanup-plan",
    "/api/w/team/runtimes/runtime%20%2F/cleanup-executions",
  ]);
  assertEquals(requests[1]?.init?.method, "POST");
  assertEquals(JSON.parse(String(requests[1]?.init?.body)), {
    expected_plan_digest: "digest-7",
    worker_target_ids: ["worker-target"],
    workdir_target_ids: [],
    confirm_dirty_discard_target_ids: [],
  });
});

Deno.test("sidebar Delete reports cleanup-plan blocking reasons", async () => {
  const fetchFn = (() =>
    Promise.resolve(
      jsonResponse({
        workspace_id: "team",
        runtime_id: worker.runtime_id,
        generated_at: "2026-01-01T00:00:00Z",

        digest: "digest-8",
        workers: [{
          target_id: "worker-target",
          action: "worker_delete",
          worker_id: worker.worker_id,
          runtime_id: worker.runtime_id,
          runtime_worker_id: worker.worker_id,
          reason: "stopped worker",
          blocking_reason: "Worker is pinned",
          pinned: true,
          retention_state: "pinned",
          linked_workdir_ids: [],
          running_linked: false,
        }],
        workdirs: [],
        diagnostics: [],
      }),
    )) as typeof fetch;

  await assertRejects(
    () => deleteSidebarWorker("team", { ...worker, state: "stopped" }, fetchFn),
    "Worker is pinned",
  );
});

Deno.test("Worker navigation exposes an accessible hover action menu", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../../src/lib/workspace/sidebar/WorkersNavSection.svelte",
      import.meta.url,
    ),
  );
  const styles = await Deno.readTextFile(
    new URL("../../src/lib/workspace/sidebar/sidebar.css", import.meta.url),
  );

  assert(source.includes('aria-haspopup="menu"'));
  assert(source.includes('role="menuitem"'));
  assert(source.includes("stopSidebarWorker"));
  assert(source.includes("deleteSidebarWorker"));
  assert(styles.includes(".worker-nav-item:hover .worker-actions-trigger"));
});

Deno.test("Restore scoped POST encodes identities and sends immutable intent without assignment query", async () => {
  const request = createRestoreRequest({
    restore_observation_token: "generation",
  });
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: input.toString(), init });
    return jsonResponse({
      workspace_id: "team space",
      runtime_id: worker.runtime_id,
      worker_id: worker.worker_id,
      result: { state: "accepted" },
    });
  }) as typeof fetch;
  await restoreWorkspaceWorker("team space", worker, request, fetchFn);
  await restoreWorkspaceWorker("team space", worker, request, fetchFn);
  assertEquals(
    requests[0].url,
    "/api/w/team%20space/runtimes/runtime%20%2F/workers/worker%20%2F/restore",
  );
  assertEquals(requests[0].init?.method, "POST");
  assertEquals(JSON.parse(String(requests[0].init?.body)), request);
  assertEquals(requests[1].init?.body, requests[0].init?.body);
  assert(request.request_id.length > 0);
});
Deno.test("Restore classification never promotes nonaccepted results and preserves unknown outcome recovery", async () => {
  for (
    const state of [
      "accepted",
      "rejected",
      "rolled_back",
      "superseded",
      "reconciliation_required",
    ] as const
  ) {
    const response = await restoreWorkspaceWorker(
      "team",
      worker,
      { expected_observation_token: "g", request_id: "id" },
      (() =>
        Promise.resolve(jsonResponse({
          workspace_id: "team",
          runtime_id: worker.runtime_id,
          worker_id: worker.worker_id,
          result: {
            state,
            diagnostics: [{
              code: "result",
              severity: "warning",
              message: "backend reason",
            }],
          },
        }))) as typeof fetch,
    );
    const notice = restoreNotice(response);
    assertEquals(notice.level === "info", state === "accepted");
    assertEquals(notice.retry, state === "reconciliation_required");
    if (state !== "accepted") {
      assert(notice.message.includes(`Worker Restore was ${state}`));
    }
  }
  for (const status of [400, 403, 409, 500]) {
    try {
      await restoreWorkspaceWorker(
        "team",
        worker,
        { expected_observation_token: "g", request_id: "id" },
        (() =>
          Promise.resolve(
            jsonResponse({ error: "conflict", message: "changed" }, status),
          )) as typeof fetch,
      );
      throw new Error("must reject");
    } catch (error) {
      const notice = restoreErrorNotice(error);
      assertEquals(notice.retry, status >= 500);
      if (status === 409) {
        assertEquals(notice.title, "Worker observation changed");
      }
    }
  }
  assert(restoreErrorNotice(new TypeError("network failed")).retry);
  await assertRejects(
    () =>
      restoreWorkspaceWorker(
        "team",
        worker,
        { expected_observation_token: "g", request_id: "id" },
        (() =>
          Promise.resolve(
            jsonResponse({
              workspace_id: "other",
              runtime_id: worker.runtime_id,
              worker_id: worker.worker_id,
              result: { state: "accepted" },
            }),
          )) as typeof fetch,
      ),
    "requested target",
  );
  await assertRejects(
    () =>
      restoreWorkspaceWorker(
        "team",
        worker,
        { expected_observation_token: "g", request_id: "id" },
        (() =>
          Promise.resolve(
            jsonResponse({
              workspace_id: "team",
              runtime_id: worker.runtime_id,
              worker_id: worker.worker_id,
              result: { state: "future" },
            }),
          )) as typeof fetch,
      ),
    "unknown value",
  );
});

Deno.test("Restore bounds the response before decoding and retains uncertain outcome instead of success", async () => {
  let response = new Response("{}", {
    headers: { "content-length": String(8 * 1024 * 1024 + 1) },
  });
  let caught: unknown;
  try {
    await restoreWorkspaceWorker("team", worker, {
      expected_observation_token: "g",
      request_id: "id",
    }, (() => Promise.resolve(response)) as typeof fetch);
  } catch (error) {
    caught = error;
  }
  assert(caught instanceof Error);
  assert(restoreErrorNotice(caught).retry);
  response = new Response("{}", {
    status: 409,
    headers: { "content-length": String(64 * 1024 + 1) },
  });
  try {
    await restoreWorkspaceWorker("team", worker, {
      expected_observation_token: "g",
      request_id: "id",
    }, (() => Promise.resolve(response)) as typeof fetch);
  } catch (error) {
    caught = error;
  }
  assertEquals(restoreErrorNotice(caught).title, "Worker observation changed");
  assertEquals(restoreErrorNotice(caught).retry, false);
});

Deno.test("sidebar Delete accepts HTTP 202 but retains the Worker and saved operation", async () => {
  for (const state of ["waiting", "held"] as const) {
    const plan = {
      workspace_id: "team",
      runtime_id: worker.runtime_id,
      generated_at: "2026-01-01T00:00:00Z",
      digest: "saved-digest",
      workers: [{
        target_id: "target",
        action: "worker_delete",
        worker_id: worker.worker_id,
        runtime_id: worker.runtime_id,
        runtime_worker_id: worker.worker_id,
        reason: "stopped",
        blocking_reason: null,
        pinned: false,
        retention_state: "stopped",
        linked_workdir_ids: [],
        running_linked: false,
      }],
      workdirs: [],
      diagnostics: [],
    };
    const removal = {
      operation_id: "original-removal-operation",
      state,
      phase: "input_handoff" as const,
      cause: state === "held" ? "job_failed" as const : "job_pending" as const,
    };
    const receipt = {
      workspace_id: "team",
      runtime_id: worker.runtime_id,
      executed_at: "2026-01-01T00:00:01Z",
      results: [{
        target_id: "target",
        action: "worker_delete",
        status: "accepted",
        message: "",
        removal,
      }],
      plan_after: plan,
      diagnostics: [],
    };
    const requests: string[] = [];
    const fetchFn = ((input: RequestInfo | URL) => {
      requests.push(String(input));
      return Promise.resolve(
        Response.json(requests.length === 1 ? plan : receipt, {
          status: requests.length === 1 ? 200 : 202,
        }),
      );
    }) as typeof fetch;
    const outcome = await deleteSidebarWorker("team", {
      ...worker,
      state: "stopped",
    }, fetchFn);
    assertEquals(outcome, { removed: false, removal });
    assertEquals(requests.length, 2); // No polling or retrying mutations.
    const notice = deletionNotice(outcome);
    assertEquals(notice.title, "Worker removal accepted");
    assert(notice.message.includes(state));
    assert(notice.message.includes("Removal operation"));
    assert(notice.message.includes("Worker retained, not deleted"));
    assertEquals(
      parseRuntimeCleanupExecution(receipt).results[0].removal,
      removal,
    );
    // An observation must never be promoted by a status string alone.
    assertEquals(
      workerDeletionOutcome({
        ...parseRuntimeCleanupExecution(receipt).results[0],
        status: "deleted",
      }).removed,
      false,
    );
  }
  assertEquals(
    workerDeletionOutcome({
      target_id: "t",
      action: "worker_delete",
      status: "deleted",
      message: "",
    }),
    { removed: true },
  );
});

Deno.test("Stop interruption and post-termination failures preserve owner evidence without retrying", async () => {
  for (
    const [reason, category, status] of [
      ["stop_request_interrupted", "conflict", 409],
      ["stop_terminated_persistence_failed", "internal", 500],
      ["stop_terminated_persistence_outcome_unknown", "unknown", 500],
      ["stop_terminated_cleanup_failed", "dependency", 500],
    ] as const
  ) {
    const explanation =
      `Owner explanation for ${reason}; inspect before recovery.`;
    const problem = controlProblem(reason, explanation, status);
    problem.yoi_failure.category = category;
    for (const validCategory of [true, false]) {
      let calls = 0;
      try {
        await stopSidebarWorker(
          "team",
          worker,
          (() => {
            calls++;
            return Promise.resolve(controlProblemResponse({
              ...problem,
              yoi_failure: {
                ...problem.yoi_failure,
                category: validCategory ? category : "authorization",
              },
            }));
          }) as typeof fetch,
        );
        throw new Error("must reject");
      } catch (error) {
        assert(error instanceof RuntimeControlHttpError);
        assertEquals(error.receipt.kind, validCategory ? "problem" : "invalid");
        if (error.receipt.kind === "problem") {
          assertEquals(error.receipt.problem.yoi_failure.details, reason);
          assertEquals(error.receipt.problem.yoi_failure.category, category);
          assertEquals(error.message, explanation);
        } else {
          assert(!error.message.includes(explanation));
        }
      }
      assertEquals(calls, 1);
    }
    {
      let calls = 0;
      await assertRejects(
        () =>
          stopSidebarWorker(
            "team",
            worker,
            (() => {
              calls++;
              return Promise.resolve(jsonResponse({
                state: "rejected",
                runtime_id: worker.runtime_id,
                worker_id: worker.worker_id,
                control: reason,
                control_explanation: explanation,
                diagnostics: [],
              }));
            }) as typeof fetch,
          ),
        explanation,
      );
      assertEquals(calls, 1);
    }
  }
});

Deno.test("Restore and Stop HTTP failures display the typed primary cause, not legacy bodies or supplemental order", async () => {
  for (
    const explanation of [
      "Admission rejected.",
      "Different public wording <b>plain text</b>.",
    ]
  ) {
    const problem = controlProblem("execution_rejected", explanation);
    for (
      const operation of [
        (fetchFn: typeof fetch) =>
          restoreWorkspaceWorker("team", worker, {
            expected_observation_token: "g",
            request_id: "same-id",
          }, fetchFn),
        (fetchFn: typeof fetch) => stopSidebarWorker("team", worker, fetchFn),
      ]
    ) {
      let calls = 0;
      try {
        await operation(
          (() => {
            calls++;
            return Promise.resolve(controlProblemResponse(problem));
          }) as typeof fetch,
        );
        throw new Error("must reject");
      } catch (error) {
        assert(error instanceof RuntimeControlHttpError);
        assertEquals(error.message, explanation);
        assert(error.receipt.kind === "problem");
        assertEquals(
          error.receipt.problem.yoi_failure.details,
          "execution_rejected",
        );
        assertEquals(restoreErrorNotice(error).retry, false);
      }
      assertEquals(calls, 1);
    }
  }
  for (
    const response of [
      () => jsonResponse({ message: "private raw legacy body" }, 500),
      () =>
        controlProblemResponse(
          controlProblem("future_reason", "private future explanation", 500),
        ),
      () =>
        new Response("{not json", {
          status: 500,
          headers: { "content-type": "application/problem+json" },
        }),
    ]
  ) {
    try {
      await restoreWorkspaceWorker("team", worker, {
        expected_observation_token: "g",
        request_id: "same-id",
      }, (() => Promise.resolve(response())) as typeof fetch);
      throw new Error("must reject");
    } catch (error) {
      assert(error instanceof RuntimeControlHttpError);
      assert(error.receipt.kind !== "problem");
      assert(!error.message.includes("private"));
      assert(restoreErrorNotice(error).retry);
    }
  }
});

Deno.test("Restore settlement and same-request reconciliation ignore diagnostic severity, text and position", async () => {
  for (
    const state of [
      "accepted",
      "rejected",
      "rolled_back",
      "superseded",
      "reconciliation_required",
    ] as const
  ) {
    for (const reverse of [false, true]) {
      const diagnostics = [
        {
          code: "worker_missing_stop_busy",
          severity: "error",
          message: "cleanup wording must not become the primary cause",
        },
        {
          code: "unrelated",
          severity: "info",
          message: reverse
            ? "Changed diagnostic wording"
            : "Supplementary observation only",
        },
      ];
      const parsed = await restoreWorkspaceWorker(
        "team",
        worker,
        { expected_observation_token: "g", request_id: "same-id" },
        (() =>
          Promise.resolve(jsonResponse({
            workspace_id: "team",
            runtime_id: worker.runtime_id,
            worker_id: worker.worker_id,
            result: {
              state,
              control: "restore_admission_busy",
              control_explanation: "Public admission explanation",
              diagnostics: reverse ? diagnostics.reverse() : diagnostics,
            },
          }))) as typeof fetch,
      );
      const notice = restoreNotice(parsed);
      assertEquals(parsed.result.state, state);
      assertEquals(notice.retry, state === "reconciliation_required");
      assertEquals(notice.level === "info", state === "accepted");
      if (state !== "accepted") {
        assert(notice.message.startsWith("Public admission explanation"));
        assert(!notice.message.includes("cleanup wording"));
      }
    }
  }
  const unsupported = await restoreWorkspaceWorker(
    "team",
    worker,
    { expected_observation_token: "g", request_id: "same-id" },
    (() =>
      Promise.resolve(jsonResponse({
        workspace_id: "team",
        runtime_id: worker.runtime_id,
        worker_id: worker.worker_id,
        result: {
          state: "reconciliation_required",
          control: "future_cause",
          diagnostics: [],
        },
      }))) as typeof fetch,
  );
  assert(
    restoreNotice(unsupported).message.includes("unsupported control cause"),
  );
  assert(restoreNotice(unsupported).retry);
});

Deno.test("Restore real caller accepts startup Session observations and uses only the fresh paired public explanation", async () => {
  const request = {
    expected_observation_token: "same-observation",
    request_id: "same-restore",
  };
  for (const state of ["rolled_back", "reconciliation_required"] as const) {
    for (
      const primary of [
        startupObservations().primary,
        "unsupported opaque attachment",
      ]
    ) {
      let requests = 0;
      const startup = { ...startupObservations(), primary };
      const parsed = await restoreWorkspaceWorker(
        "team",
        worker,
        request,
        (async (_input: RequestInfo | URL, init?: RequestInit) => {
          requests++;
          assertEquals(JSON.parse(String(init?.body)), request);
          return jsonResponse({
            workspace_id: "team",
            runtime_id: worker.runtime_id,
            worker_id: worker.worker_id,
            result: {
              state,
              startup,
              control: "execution_failed",
              control_explanation: "Session registration permission denied.",
              diagnostics: [{
                code: "cleanup",
                severity: "error",
                message: "Do not show this as primary",
              }],
            },
          });
        }) as typeof fetch,
      );
      assertEquals(requests, 1);
      assertEquals(parsed.result.state, state);
      assertEquals(parsed.result.startup, startup);
      assertEquals(parsed.result.control, "execution_failed");
      const notice = restoreNotice(parsed);
      assert(
        notice.message.startsWith("Session registration permission denied."),
      );
      assert(
        !notice.message.includes("opaque") &&
          !notice.message.includes("Do not show"),
      );
      assertEquals(notice.retry, state === "reconciliation_required");
    }
  }
});

Deno.test("Stop acceptance and primary explanation are independent of diagnostic text, severity, code and order", async () => {
  for (const state of ["accepted", "rejected"] as const) {
    for (const reverse of [false, true]) {
      const diagnostics = [
        {
          code: "worker_missing",
          severity: "error",
          message: "Unrelated cleanup failure",
        },
        {
          code: "changed text and code",
          severity: "info",
          message: reverse ? "Changed diagnostic wording" : "Supplement only",
        },
      ];
      const fetchFn = (() =>
        Promise.resolve(
          jsonResponse({
            state,
            runtime_id: worker.runtime_id,
            worker_id: worker.worker_id,
            control: "stop_admission_busy",
            control_explanation: "Owner rejected Stop admission",
            diagnostics: reverse ? diagnostics.reverse() : diagnostics,
          }),
        )) as typeof fetch;
      if (state === "accepted") {
        await stopSidebarWorker("team", worker, fetchFn);
      } else {await assertRejects(
          () => stopSidebarWorker("team", worker, fetchFn),
          "Owner rejected Stop admission",
        );}
    }
  }
});
