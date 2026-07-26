import { canOpenWorkerConsole, canShowWorkerInSidebar } from "./workers.ts";
import type { Worker } from "./types.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, got ${String(actual)}`);
  }
}

function worker(overrides: Partial<Worker>): Worker {
  return {
    runtime_id: "arc",
    worker_id: "1",
    host_id: "host",
    display_name: "Worker 1",
    label: "Worker 1",
    role: null,
    profile: null,
    singleton_key: null,
    tags: [],
    workspace: { visibility: "workspace", identity: "workspace" },
    state: "running",
    pinned: false,
    retention_state: "normal",
    last_seen_at: null,
    implementation: {
      kind: "runtime_worker",
      display_hint: "Runtime Worker",
    },
    capabilities: {
      can_stop: true,
      can_spawn_followup: false,
    },
    working_directory: null,
    diagnostics: [],
    ...overrides,
  };
}

Deno.test("registry-only workers are not sidebar targets or console targets", () => {
  const registryOnly = worker({
    state: "missing",
    implementation: {
      kind: "backend_worker_registry",
      display_hint: "Missing Worker",
    },
    capabilities: {
      can_stop: false,
      can_spawn_followup: false,
    },
  });
  assertEquals(canShowWorkerInSidebar(registryOnly), false);
  assertEquals(canOpenWorkerConsole(registryOnly), false);
});

Deno.test("live runtime workers are sidebar targets and console targets", () => {
  const liveWorker = worker({ state: "running" });
  assertEquals(canShowWorkerInSidebar(liveWorker), true);
  assertEquals(canOpenWorkerConsole(liveWorker), true);
});
