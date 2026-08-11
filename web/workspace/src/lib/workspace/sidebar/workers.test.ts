import {
  canOpenWorkerConsole,
  canShowWorkerInSidebar,
  compareWorkersForSidebar,
} from "./workers.ts";
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

Deno.test("sidebar workers sort running then idle then stopped", () => {
  const workers = [
    worker({ worker_id: "3", display_name: "Stopped", state: "stopped" }),
    worker({ worker_id: "2", display_name: "Running", state: "running" }),
    worker({ worker_id: "4", display_name: "Idle B", state: "idle" }),
    worker({ worker_id: "1", display_name: "Idle A", state: "idle" }),
  ];
  workers.sort(compareWorkersForSidebar);
  assertEquals(workers.map((candidate) => candidate.worker_id).join(","), "2,1,4,3");
});
