// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  SubscriptionFrame,
  SubscriptionWorker,
} from "#lib/generated/protocol.ts";
import type { WorkspaceWorkersState } from "./worker-subscription";
import { canRestoreWorker, canStopSidebarWorker } from "./worker-actions";

const mock = vi.hoisted(() => ({ handlers: null as any }));
vi.mock("#lib/workspace/multiplexer.ts", () => ({
  workspaceMultiplexer: () => ({
    subscribe: (_: unknown, handlers: unknown) => {
      mock.handlers = handlers;
      return { close: vi.fn() };
    },
  }),
}));
vi.mock(
  "#lib/workspace/alerts/store.ts",
  () => ({ pushWorkspaceAlert: vi.fn(), dismissWorkspaceAlert: vi.fn() }),
);
import {
  disposeWorkspaceWorkersStore,
  refreshWorkspaceWorkers,
  workspaceWorkersStore,
} from "./worker-subscription";

function summary(state = "stopped", token = "generation-1") {
  return {
    runtime_id: "runtime",
    worker_id: "worker",
    resource_key: "W-1",
    host_id: "runtime",
    display_name: "test",
    label: "test",
    availability: "observed",
    state,
    lifecycle: state === "stopped" ? "stopped" : "active",
    restore_observation_token: token,
    workspace: {
      visibility: "workspace",
      identity: "registered",
      workspace_id: "team",
    },
    implementation: { kind: "runtime", display_hint: "Runtime" },
    pinned: true,
    retention_state: "pinned",
    workdir_attachments: [],
    diagnostics: [],
  };
}
function response(state = "stopped", token = "generation-1") {
  return new Response(
    JSON.stringify({
      workspace_id: "team",
      limit: 100,
      items: [summary(state, token)],
      source: "catalog",
      diagnostics: [],
    }),
  );
}
function snapshot(
  state = "stopped",
  token = "generation-1",
): SubscriptionFrame {
  const worker = {
    worker_id: "worker",
    runtime_id: "runtime",
    resource_key: "W-1",
    availability: "observed",
    state,
    lifecycle: state === "stopped" ? "stopped" : "active",
    restore_observation_token: token,
    has_running_internal_workers: false,
    workspace_id: "team",
    workdir_attachments: [],
  } as SubscriptionWorker;
  return {
    protocol_version: 2,
    frame: "response",
    message: {
      result: "subscribed",
      payload: {
        request_id: "request",
        subscription_id: "sub",
        selector: { topic: "workspace_workers" },
        snapshot: { topic: "workers", data: { workers: [worker] } },
      },
    },
  };
}
const settle = async () => {
  for (let i = 0; i < 12; i++) await Promise.resolve();
};
let unsubscribe: (() => void) | null;
beforeEach(() => {
  mock.handlers = null;
  unsubscribe = null;
});
afterEach(() => {
  unsubscribe?.();
  disposeWorkspaceWorkersStore("team");
  vi.unstubAllGlobals();
});

describe("shared catalog observation refresh", () => {
  it("keeps full catalog metadata while exposing catalog lifecycle separately from foreground display", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) =>
        url.endsWith("/workers")
          ? response()
          : new Response(JSON.stringify({ items: [] }))
      ),
    );
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      latest = state
    );
    mock.handlers.onFrame(snapshot());
    await settle();
    expect(latest.catalogWorkers?.[0].pinned).toBe(true);
    expect(latest.catalogWorkers?.[0].retention_state).toBe("pinned");
    expect(latest.workers[0].lifecycleState).toBe("stopped");
    expect(latest.workers[0].restore_observation_token).toBe("generation-1");
  });
  it("keeps observed Stop restorable without a foreground snapshot, but fences a disconnected source", async () => {
    let availability: SubscriptionWorker["availability"] = "observed";
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) => {
        if (!url.endsWith("/workers")) return Response.json({ items: [] });
        return Response.json({
          workspace_id: "team",
          limit: 100,
          source: "catalog",
          diagnostics: [],
          items: [{
            ...summary(),
            availability,
            lifecycle: "stopped",
            state: availability === "observed" ? "stopped" : "unavailable",
            worker_state: null,
            restore_observation_token: availability === "observed"
              ? "generation-1"
              : null,
          }],
        });
      }),
    );
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      latest = state
    );
    for (const current of ["observed", "unavailable"] as const) {
      availability = current;
      const frame = snapshot();
      if (
        frame.frame !== "response" || frame.message.result !== "subscribed" ||
        frame.message.payload.snapshot.topic !== "workers"
      ) throw new Error("expected Worker snapshot");
      Object.assign(frame.message.payload.snapshot.data.workers[0], {
        availability,
        lifecycle: "stopped",
        worker_state: null,
      });
      mock.handlers.onFrame(frame);
      expect(canRestoreWorker(latest.workers[0])).toBe(current === "observed");
      await refreshWorkspaceWorkers("team");
      expect(latest.catalogRefreshing).toBe(false);
      expect(latest.workers[0].state).toBe(
        current === "observed" ? "stopped" : "unknown",
      );
      expect(latest.workers[0].worker_state).toBeNull();
      expect(canRestoreWorker(latest.workers[0])).toBe(current === "observed");
      expect(canStopSidebarWorker(latest.workers[0])).toBe(false);
    }
  });
  it("projects canonical recovery lifecycle from subscription and catalog without stopped fallback", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) =>
      url.endsWith("/workers")
        ? Response.json({
          workspace_id: "team", limit: 100, source: "catalog", diagnostics: [],
          items: [{ ...summary(), lifecycle: "recovery_required" }],
        })
        : Response.json({ items: [] })
    ));
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) => latest = state);
    const frame = snapshot();
    if (frame.frame !== "response" || frame.message.result !== "subscribed" ||
      frame.message.payload.snapshot.topic !== "workers") throw new Error("invalid fixture");
    frame.message.payload.snapshot.data.workers[0].lifecycle = "stopping";
    mock.handlers.onFrame(frame);
    expect(latest.workers[0].lifecycleState).toBe("stopping");
    expect(latest.workers[0].state).toBe("stopping");
    await settle();
    expect(latest.workers[0].lifecycleState).toBe("recovery_required");
    expect(latest.workers[0].state).toBe("recovery_required");
    expect(latest.catalogWorkers?.[0].state).toBe("stopped");
  });
  it("invalidates dependent plans before refresh and discards a GET superseded by a newer catalog event", async () => {
    const pending: Array<(response: Response) => void> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        url.endsWith("/workers")
          ? new Promise<Response>((resolve) => pending.push(resolve))
          : Promise.resolve(new Response(JSON.stringify({ items: [] })))
      ),
    );
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      latest = state
    );
    mock.handlers.onFrame(snapshot());
    const before = latest.catalogRequest;
    const old = pending.at(-1)!;
    mock.handlers.onFrame(snapshot("idle", "generation-2"));
    expect(latest.catalogRequest).not.toBe(before);
    expect(before?.aborted).toBe(true);
    expect(latest.workers[0].lifecycleState).toBe("active");
    pending.at(-1)!(response("idle", "generation-2"));
    await settle();
    old(response("stopped", "generation-1"));
    await settle();
    expect(latest.catalogWorkers?.[0].state).toBe("idle");
    expect(latest.workers[0].restore_observation_token).toBe("generation-2");
  });
  it("explicit uncertain-outcome refresh is only GET and cannot settle or replay Restore", async () => {
    const fetchMock = vi.fn(async (url: string) =>
      url.endsWith("/workers")
        ? response()
        : new Response(JSON.stringify({ items: [] }))
    );
    vi.stubGlobal("fetch", fetchMock);
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      latest = state
    );
    mock.handlers.onFrame(snapshot());
    await settle();
    const before = latest.catalogRequest;
    await refreshWorkspaceWorkers("team");
    expect(latest.catalogRequest).not.toBe(before);
    expect(before?.aborted).toBe(true);
    expect(fetchMock.mock.calls.every(([url]) => !url.endsWith("/restore")))
      .toBe(true);
  });
  it("does not filter catalog-only targets on a stream update while its metadata refresh is pending", async () => {
    let hold = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) => {
        if (!url.endsWith("/workers")) {
          return new Response(JSON.stringify({ items: [] }));
        }
        if (hold) return new Promise<Response>(() => {});
        return Response.json({
          workspace_id: "team",
          limit: 100,
          source: "catalog",
          diagnostics: [],
          items: [summary(), {
            ...summary(),
            worker_id: "registry-only",
            resource_key: "W-2",
            implementation: {
              kind: "backend_worker_registry",
              display_hint: "registry",
            },
          }],
        });
      }),
    );
    let latest!: WorkspaceWorkersState;
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      latest = state
    );
    mock.handlers.onFrame(snapshot());
    await settle();
    expect(latest.catalogWorkers).toHaveLength(2);
    hold = true;
    mock.handlers.onFrame(snapshot("idle", "generation-2"));
    expect(latest.catalogRefreshing).toBe(true);
    expect(latest.catalogWorkers).toHaveLength(2);
    expect(latest.catalogWorkers?.[1].implementation.kind).toBe(
      "backend_worker_registry",
    );
  });
  it("does not publish a late catalog GET after disposal", async () => {
    let resolve!: (response: Response) => void;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        url.endsWith("/workers")
          ? new Promise<Response>((r) => resolve = r)
          : Promise.resolve(new Response(JSON.stringify({ items: [] })))
      ),
    );
    const seen: WorkspaceWorkersState[] = [];
    unsubscribe = workspaceWorkersStore("team").subscribe((state) =>
      seen.push(state)
    );
    const before = seen.length;
    disposeWorkspaceWorkersStore("team");
    resolve(response("idle", "generation-2"));
    await settle();
    expect(seen.length).toBe(before);
  });
});
