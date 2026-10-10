// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ComponentProps } from "svelte";
import type {
  SubscriptionFrame,
  SubscriptionWorker,
} from "#lib/generated/protocol.ts";
import {
  clearWorkspaceAlerts,
  workspaceAlerts,
} from "#lib/workspace/alerts/store.ts";
import { disposeWorkspaceWorkersStore } from "#lib/workspace/sidebar/worker-subscription.ts";
import WorkersNavSection from "#lib/workspace/sidebar/WorkersNavSection.svelte";
import WorkersPage from "./+page.svelte";

const transport = vi.hoisted(() => ({
  listener: null as null | { onFrame(frame: SubscriptionFrame): void },
}));
vi.mock("#lib/workspace/multiplexer.ts", () => ({
  workspaceMultiplexer: () => ({
    subscribe: (_: unknown, listener: typeof transport.listener) => {
      transport.listener = listener;
      return { close: vi.fn() };
    },
  }),
}));
const workspaceId = "team /";
const runtimeId = "runtime /";
const workerId = "worker /";
const restoreUrl =
  "/api/w/team%20%2F/runtimes/runtime%20%2F/workers/worker%20%2F/restore";
function worker(state = "stopped", token = "generation-1") {
  return {
    runtime_id: runtimeId,
    worker_id: workerId,
    resource_key: "W-1",
    host_id: runtimeId,
    display_name: "Team Worker",
    label: "Team Worker",
    availability: "observed" as const,
    state,
    restore_observation_token: token,
    tags: [],
    diagnostics: [],
    pinned: false,
    retention_state: "normal",
    workspace: {
      visibility: "workspace",
      identity: "registered",
      workspace_id: workspaceId,
    },
    implementation: { kind: "runtime", display_hint: "Runtime" },
    workdir_attachments: [],
    worker_state: state === "stopped"
      ? null
      : { last_command_id: 0, state: { kind: "idle" as const } },
  };
}
function frame(current: ReturnType<typeof worker>): SubscriptionFrame {
  const subscriptionWorker = {
    ...current,
    has_running_internal_workers: false,
    workspace_id: workspaceId,
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
        snapshot: { topic: "workers", data: { workers: [subscriptionWorker] } },
      },
    },
  };
}
function plan(state: string) {
  return {
    workspace_id: workspaceId,
    runtime_id: runtimeId,
    generated_at: "2026-10-06T00:00:00Z",
    digest: state,
    diagnostics: [],
    workdirs: [],
    workers: state === "stopped"
      ? [{
        target_id: "target",
        action: "worker_delete",
        worker_id: "W-1",
        runtime_worker_id: workerId,
        runtime_id: runtimeId,
        reason: "stopped",
        pinned: false,
        retention_state: "normal",
        linked_workdir_ids: [],
        running_linked: false,
      }]
      : [],
  };
}
function response(state: string, summary?: ReturnType<typeof worker>) {
  return Response.json({
    workspace_id: workspaceId,
    runtime_id: runtimeId,
    worker_id: workerId,
    result: { state, worker: summary, diagnostics: [] },
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => resolve = r);
  return { promise, resolve };
}
function mount() {
  const row = worker();
  render(WorkersNavSection, {
    workspaceId,
    currentPath: `/w/${workspaceId}/workers`,
  });
  render(WorkersPage, {
    params: { workspaceId },
    data: {
      workspaceId,
      workers: { items: [row] },
      workersError: null,
      cleanupPlans: {},
      cleanupPlanErrors: {},
    } as unknown as ComponentProps<typeof WorkersPage>["data"],
  });
}
async function sidebarRestore() {
  await fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Team Worker" }),
  );
  return screen.findByRole("menuitem", { name: "Restore" });
}
function pageRestore() {
  return screen.getByRole("button", { name: "Restore Team Worker" });
}
let current = worker();
let handleRestore: (init?: RequestInit) => Promise<Response>;
let requests: ReturnType<typeof vi.fn>;
beforeEach(() => {
  transport.listener = null;
  current = worker();
  handleRestore = async () => response("accepted");
  requests = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url === restoreUrl) return handleRestore(init);
    if (url.endsWith("/workers")) {
      return Response.json({
        workspace_id: workspaceId,
        limit: 100,
        source: "catalog",
        items: [current],
        diagnostics: [],
      });
    }
    if (url.endsWith("/cleanup-plan")) {
      return Response.json(plan(current.state));
    }
    if (url.endsWith("/working-directories")) {
      return Response.json({
        workspace_id: workspaceId,
        items: [],
        diagnostics: [],
      });
    }
    throw new Error(`Unexpected request ${url}`);
  });
  vi.stubGlobal("fetch", requests);
  clearWorkspaceAlerts();
});
afterEach(() => {
  cleanup();
  disposeWorkspaceWorkersStore(workspaceId);
  vi.unstubAllGlobals();
  clearWorkspaceAlerts();
});
async function ready() {
  await waitFor(() => expect(transport.listener).not.toBeNull());
  transport.listener!.onFrame(frame(current));
  await waitFor(() =>
    expect((pageRestore() as HTMLButtonElement).disabled).toBe(false)
  );
  await screen.findByRole("button", { name: "Delete Team Worker" });
}

test.each(["sidebar", "page"] as const)(
  "%s Restore updates both actual components through the real store and invalidates cleanup without reload",
  async (entrance) => {
    const pending = deferred<Response>();
    handleRestore = () => pending.promise;
    mount();
    await ready();
    await fireEvent.click(
      entrance === "sidebar" ? await sidebarRestore() : pageRestore(),
    );
    const operations = requests.mock.calls.filter(([url]) =>
      String(url) === restoreUrl
    );
    expect(operations).toHaveLength(1);
    expect(operations[0][1].method).toBe("POST");
    expect(JSON.parse(operations[0][1].body)).toEqual({
      expected_observation_token: "generation-1",
      request_id: expect.any(String),
    });
    current = worker("idle", "generation-2");
    transport.listener!.onFrame(frame(current));
    await screen.findByRole("cell", { name: "idle" });
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Delete Team Worker" }))
        .toBeNull()
    );
    // A late old response cannot put either component back into stopped/running.
    pending.resolve(response("accepted", worker("stopped", "generation-1")));
    await waitFor(() =>
      expect((pageRestore() as HTMLButtonElement).disabled).toBe(true)
    );
    expect(screen.queryByRole("cell", { name: "stopped" })).toBeNull();
    const menuRestore = await sidebarRestore();
    expect((menuRestore as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getAllByRole("link", { name: /Team Worker/ })).toHaveLength(
      2,
    );
    expect(requests.mock.calls.filter(([url]) => String(url) === restoreUrl))
      .toHaveLength(1);
  },
);

test("separate entrances remain independent while Runtime conflict triggers shared refresh, not automatic replay", async () => {
  const first = deferred<Response>();
  let count = 0;
  handleRestore = async () =>
    ++count === 1 ? first.promise : Response.json({
      error: "restore_observation_conflict",
      message: "Observation changed",
    }, { status: 409 });
  const alerts: Array<unknown[]> = [];
  const unsubscribe = workspaceAlerts.subscribe((value) => alerts.push(value));
  mount();
  await ready();
  await fireEvent.click(await sidebarRestore());
  expect((pageRestore() as HTMLButtonElement).disabled).toBe(false); // not a cross-screen lock
  await fireEvent.click(pageRestore());
  await waitFor(() =>
    expect(JSON.stringify(alerts)).toContain("Worker observation changed")
  );
  const operations = requests.mock.calls.filter(([url]) =>
    String(url) === restoreUrl
  );
  expect(operations).toHaveLength(2);
  const [a, b] = operations.map(([, init]) => JSON.parse(init.body));
  expect(a.expected_observation_token).toBe(b.expected_observation_token);
  expect(a.request_id).not.toBe(b.request_id);
  expect(screen.queryByRole("button", { name: "Retry Restore Team Worker" }))
    .toBeNull();
  current = worker("idle", "generation-2");
  transport.listener!.onFrame(frame(current));
  first.resolve(response("accepted"));
  await screen.findByRole("cell", { name: "idle" });
  await waitFor(() =>
    expect((pageRestore() as HTMLButtonElement).disabled).toBe(true)
  );
  expect(count).toBe(2);
  unsubscribe();
});

test.each(["http", "malformed"] as const)(
  "%s Restore response never produces success in either entrance",
  async (failure) => {
    handleRestore = async () =>
      failure === "http"
        ? Response.json({ message: "forbidden restore" }, { status: 403 })
        : Response.json({
          workspace_id: workspaceId,
          runtime_id: runtimeId,
          worker_id: workerId,
          result: { state: "new-state" },
        });
    const alerts: Array<unknown[]> = [];
    const unsubscribe = workspaceAlerts.subscribe((value) =>
      alerts.push(value)
    );
    mount();
    await ready();
    await fireEvent.click(pageRestore());
    await waitFor(() =>
      expect(JSON.stringify(alerts)).toContain(
        failure === "http" ? "forbidden restore" : "outcome unknown",
      )
    );
    expect(JSON.stringify(alerts)).not.toContain("Worker restored");
    expect(screen.getByRole("cell", { name: "stopped" })).toBeTruthy();
    // The sidebar is a second explicit entrance, not an automatic retry.
    await fireEvent.click(await sidebarRestore());
    await waitFor(() =>
      expect(requests.mock.calls.filter(([url]) => String(url) === restoreUrl))
        .toHaveLength(2)
    );
    expect(JSON.stringify(alerts)).not.toContain("Worker restored");
    unsubscribe();
  },
);
