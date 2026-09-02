import {
  canDeleteSidebarWorker,
  deleteSidebarWorker,
  stopSidebarWorker,
} from "../../src/lib/workspace/sidebar/worker-actions.ts";
import type { Worker } from "../../src/lib/workspace/sidebar/types.ts";

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
  capabilities: { can_stop: true },
} as Worker;

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "content-type": "application/json" },
  });
}

Deno.test("sidebar Stop uses the workspace-scoped Worker lifecycle endpoint", async () => {
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: input.toString(), init });
    return jsonResponse({ state: "accepted", diagnostics: [] });
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

Deno.test("sidebar Stop rejects non-accepted lifecycle responses", async () => {
  const fetchFn = (() =>
    Promise.resolve(
      jsonResponse({
        state: "rejected",
        diagnostics: [{ severity: "error", message: "Worker cannot stop" }],
      }),
    )) as typeof fetch;

  await assertRejects(
    () => stopSidebarWorker("workspace", worker, fetchFn),
    "Worker cannot stop",
  );
});

Deno.test("sidebar Delete executes the authoritative runtime cleanup plan", async () => {
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: input.toString(), init });
    if (requests.length === 1) {
      return jsonResponse({
        revision: 7,
        digest: "digest-7",
        candidates: [],
        workers: [{
          target_id: "worker-target",
          runtime_id: worker.runtime_id,
          runtime_worker_id: worker.worker_id,
          blocking_reason: null,
        }],
        workdirs: [],
        diagnostics: [],
      });
    }
    return jsonResponse({
      results: [{
        target_id: "worker-target",
        status: "deleted",
        message: null,
      }],
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
    expected_plan_revision: 7,
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
        revision: 8,
        digest: "digest-8",
        candidates: [],
        workers: [{
          target_id: "worker-target",
          runtime_id: worker.runtime_id,
          runtime_worker_id: worker.worker_id,
          blocking_reason: "Worker is pinned",
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

Deno.test("sidebar Delete is enabled only for terminal Worker states", () => {
  assert(!canDeleteSidebarWorker(worker));
  assert(canDeleteSidebarWorker({ ...worker, state: "stopped" }));
  assert(canDeleteSidebarWorker({ ...worker, state: "cancelled" }));
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
  assert(source.includes("stopSidebarWorker(workspaceId, worker)"));
  assert(source.includes("deleteSidebarWorker(workspaceId, worker)"));
  assert(styles.includes(".worker-nav-item:hover .worker-actions-trigger"));
});
