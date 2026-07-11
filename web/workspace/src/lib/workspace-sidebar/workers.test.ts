import { canOpenWorkerConsole } from "./workers.ts";
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
    worker_id: "worker-1",
    host_id: "host",
    label: "worker-1",
    role: null,
    profile: null,
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
      can_accept_input: true,
      can_stop: true,
      can_spawn_followup: false,
    },
    working_directory: null,
    diagnostics: [],
    ...overrides,
  };
}

Deno.test("canOpenWorkerConsole rejects archived registry-only workers", () => {
  assertEquals(
    canOpenWorkerConsole(worker({
      state: "archived",
      implementation: {
        kind: "backend_worker_registry",
        display_hint: "Archived Worker",
      },
      capabilities: {
        can_accept_input: false,
        can_stop: false,
        can_spawn_followup: false,
      },
    })),
    false,
  );
});

Deno.test("canOpenWorkerConsole accepts live runtime workers", () => {
  assertEquals(canOpenWorkerConsole(worker({ state: "running" })), true);
});
