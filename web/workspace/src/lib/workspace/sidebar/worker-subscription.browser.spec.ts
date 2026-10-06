// @vitest-environment happy-dom

import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  SubscriptionFrame,
  SubscriptionWorker,
} from "#lib/generated/protocol.ts";
import WorkspaceAlerts from "#lib/workspace/alerts/WorkspaceAlerts.svelte";
import {
  clearWorkspaceAlerts,
  workspaceAlerts,
} from "#lib/workspace/alerts/store.ts";
import WorkersNavSection from "./WorkersNavSection.svelte";
import {
  disposeWorkspaceWorkersStore,
  workspaceWorkersStore,
} from "./worker-subscription";

const transport = vi.hoisted(() => {
  type Listener = {
    onFrame(frame: SubscriptionFrame): void;
    onStatus?(
      status: "connecting" | "open" | "closed",
      failure?: { kind: "protocol" | "transport"; message: string },
    ): void;
  };
  const listeners = new Map<string, Listener[]>();
  const closes = new Map<string, ReturnType<typeof vi.fn>>();
  return {
    listeners,
    closes,
    subscribe: vi.fn((workspaceId: string, listener: Listener) => {
      const workspaceListeners = listeners.get(workspaceId) ?? [];
      workspaceListeners.push(listener);
      listeners.set(workspaceId, workspaceListeners);
      const close = closes.get(workspaceId) ?? vi.fn();
      closes.set(workspaceId, close);
      listener.onStatus?.("connecting");
      return { close, sendWorkerMethod: vi.fn() };
    }),
  };
});

vi.mock("#lib/workspace/multiplexer.ts", () => ({
  workspaceMultiplexer: (workspaceId: string) => ({
    subscribe: (
      _selector: unknown,
      listener: Parameters<typeof transport.subscribe>[1],
    ) => transport.subscribe(workspaceId, listener),
  }),
}));

beforeEach(() => {
  transport.listeners.clear();
  transport.closes.clear();
  transport.subscribe.mockClear();
  clearWorkspaceAlerts();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const workspaceId = decodeURIComponent(
        new URL(String(input), "https://example.test").pathname.split("/")[3] ??
          "",
      );
      return Response.json({
        workspace_id: workspaceId,
        ...(String(input).endsWith("/workers")
          ? { limit: 100, source: "catalog" }
          : {}),
        items: [],
        diagnostics: [],
      });
    }),
  );
});

afterEach(() => {
  cleanup();
  for (const workspaceId of transport.listeners.keys()) {
    disposeWorkspaceWorkersStore(workspaceId);
  }
  clearWorkspaceAlerts();
  vi.unstubAllGlobals();
});

function worker(workspaceId: string, workerId: string): SubscriptionWorker {
  return {
    worker_id: workerId,
    runtime_id: `runtime-${workspaceId}`,
    resource_key: `W-${workerId}`,
    availability: "observed",
    state: "idle",
    worker_state: null,
    has_running_internal_workers: false,
    display_name: `Worker ${workerId}`,
    workspace_id: workspaceId,
    workdir_attachments: [],
  };
}

function snapshot(
  workspaceId: string,
  subscriptionId: string,
  workers: SubscriptionWorker[],
): SubscriptionFrame {
  return {
    protocol_version: 2,
    frame: "response",
    message: {
      result: "subscribed",
      payload: {
        request_id: `request-${subscriptionId}`,
        subscription_id: subscriptionId,
        selector: { topic: "workspace_workers" },
        snapshot: { topic: "workers", data: { workers } },
      },
    },
  };
}

function latestListener(workspaceId: string) {
  const listeners = transport.listeners.get(workspaceId) ?? [];
  const listener = listeners.at(-1);
  if (!listener) throw new Error(`missing listener for ${workspaceId}`);
  return listener;
}

function mount(workspaceId = "workspace-a") {
  render(WorkspaceAlerts);
  return render(WorkersNavSection, {
    workspaceId,
    currentPath: `/w/${workspaceId}`,
  });
}

function alerts(): Element[] {
  return [...document.querySelectorAll(".workspace-alert")];
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("explicit store disposal closes and fences pending fetch and subscription callbacks", async () => {
  const pendingFetch = deferred<Response>();
  vi.stubGlobal("fetch", vi.fn(() => pendingFetch.promise));
  render(WorkspaceAlerts);
  const states = vi.fn();
  const unsubscribe = workspaceWorkersStore("workspace-a").subscribe(states);
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledOnce());
  const listener = latestListener("workspace-a");

  disposeWorkspaceWorkersStore("workspace-a");
  expect(transport.closes.get("workspace-a")).toHaveBeenCalledOnce();
  listener.onStatus?.("closed", {
    kind: "transport",
    message: "late transport failure",
  });
  listener.onFrame(
    snapshot("workspace-a", "subscription-late", [
      worker("workspace-a", "late"),
    ]),
  );
  pendingFetch.resolve(new Response("late fetch failure", { status: 503 }));
  await pendingFetch.promise;
  await Promise.resolve();

  expect(alerts()).toHaveLength(0);
  expect(JSON.stringify(states.mock.calls)).not.toContain("late");
  unsubscribe();
});

test("disconnect and reconnect keep Worker rows, avoid sidebar diagnostics, and dedupe the alert episode", async () => {
  mount();
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledOnce());
  const listener = latestListener("workspace-a");
  listener.onFrame(
    snapshot("workspace-a", "subscription-a", [worker("workspace-a", "old")]),
  );
  expect(await screen.findByText("Worker old")).toBeTruthy();

  const failure = { kind: "transport" as const, message: "connection lost" };
  const alertSnapshots = vi.fn();
  const unsubscribeAlerts = workspaceAlerts.subscribe(alertSnapshots);
  alertSnapshots.mockClear();
  listener.onStatus?.("closed", failure);
  listener.onStatus?.("closed", failure);

  expect(alertSnapshots).toHaveBeenCalledOnce();
  unsubscribeAlerts();

  expect(screen.getByText("Worker old")).toBeTruthy();
  expect(document.querySelector(".sidebar-nav-section")?.textContent).not
    .toContain(
      "connection lost",
    );
  await waitFor(() => expect(alerts()).toHaveLength(1));
  expect(alerts()[0]?.textContent).toContain("connection lost");

  listener.onStatus?.("connecting");
  listener.onFrame(
    snapshot("workspace-a", "subscription-b", [worker("workspace-a", "new")]),
  );
  await waitFor(() => expect(screen.getByText("Worker new")).toBeTruthy());
  expect(screen.queryByText("Worker old")).toBeNull();
  expect(alerts()).toHaveLength(0);
});

test("route switch silently fences delayed old Workspace callbacks and starts the new subscription", async () => {
  const view = mount("workspace-a");
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledOnce());
  const oldListener = latestListener("workspace-a");
  oldListener.onFrame(
    snapshot("workspace-a", "subscription-a", [worker("workspace-a", "a")]),
  );
  expect(await screen.findByText("Worker a")).toBeTruthy();

  await view.rerender({
    workspaceId: "workspace-a",
    currentPath: "/w/workspace-a/workers",
  });
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledTimes(2));
  expect(transport.closes.get("workspace-a")).toHaveBeenCalledTimes(1);
  latestListener("workspace-a").onFrame(
    snapshot("workspace-a", "subscription-a2", [worker("workspace-a", "same")]),
  );
  expect(await screen.findByText("Worker same")).toBeTruthy();
  expect(alerts()).toHaveLength(0);

  await view.rerender({
    workspaceId: "workspace-b",
    currentPath: "/w/workspace-b",
  });
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledTimes(3));
  expect(transport.closes.get("workspace-a")).toHaveBeenCalledTimes(2);

  oldListener.onStatus?.("closed", {
    kind: "protocol",
    message: "OLD WORKSPACE FAILURE",
  });
  oldListener.onFrame(
    snapshot("workspace-a", "subscription-late", [
      worker("workspace-a", "late"),
    ]),
  );
  latestListener("workspace-b").onFrame(
    snapshot("workspace-b", "subscription-b", [worker("workspace-b", "b")]),
  );

  expect(await screen.findByText("Worker b")).toBeTruthy();
  expect(screen.queryByText("Worker late")).toBeNull();
  expect(document.body.textContent).not.toContain("OLD WORKSPACE FAILURE");
  expect(alerts()).toHaveLength(0);
});

test("terminal subscription rejection is shown only through WorkspaceAlerts", async () => {
  mount();
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledOnce());
  latestListener("workspace-a").onFrame({
    protocol_version: 2,
    frame: "response",
    message: {
      result: "subscription_rejected",
      payload: {
        request_id: "request-rejected",
        code: "unauthorized",
        message: "Worker subscription denied",
      },
    },
  });

  expect(await screen.findByText("No Workers are active.")).toBeTruthy();
  expect(document.querySelector(".sidebar-nav-section")?.textContent).not
    .toContain(
      "Worker subscription denied",
    );
  expect(alerts()).toHaveLength(1);
  expect(alerts()[0]?.textContent).toContain("Worker subscription denied");
});

test("Workdir fetch failure preserves Worker updates and uses a separate stable alert", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) =>
      String(input).endsWith("/workers")
        ? Response.json({
          workspace_id: "workspace-a",
          limit: 100,
          source: "catalog",
          items: [],
          diagnostics: [],
        })
        : new Response("Workdir lookup failed", { status: 503 })
    ),
  );
  mount();
  await waitFor(() => expect(transport.subscribe).toHaveBeenCalledOnce());
  latestListener("workspace-a").onFrame(
    snapshot("workspace-a", "subscription-a", [
      worker("workspace-a", "visible"),
    ]),
  );

  expect(await screen.findByText("Worker visible")).toBeTruthy();
  await waitFor(() => expect(alerts()).toHaveLength(1));
  expect(alerts()[0]?.textContent).toContain("Workdir lookup failed");
  expect(document.querySelector(".sidebar-nav-section")?.textContent).not
    .toContain(
      "Workdir lookup failed",
    );
});
