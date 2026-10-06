// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { writable } from "svelte/store";
import type { ComponentProps } from "svelte";
import type {
  RuntimeCleanupPlanResponse,
  Worker,
} from "#lib/workspace/sidebar/types.ts";
import type { WorkspaceWorkersState } from "#lib/workspace/sidebar/worker-subscription.ts";
import {
  clearWorkspaceAlerts,
  workspaceAlerts,
} from "#lib/workspace/alerts/store.ts";
import WorkersPage from "./+page.svelte";

const shared = vi.hoisted(() => ({
  stores: new Map(),
  refresh: vi.fn(async (_workspaceId: string) => {}),
}));
vi.mock("#lib/workspace/sidebar/worker-subscription.ts", () => ({
  workspaceWorkersStore: (id: string) => shared.stores.get(id),
  refreshWorkspaceWorkers: shared.refresh,
}));

type Data = ComponentProps<typeof WorkersPage>["data"];
function worker(overrides: Partial<Worker> = {}): Worker {
  return {
    runtime_id: "runtime-a",
    worker_id: "worker-a",
    resource_key: "W-1",
    host_id: "host-a",
    display_name: "Catalog worker",
    label: "worker-a",
    profile: "builtin:coder",
    tags: [],
    workspace: { visibility: "workspace", identity: "workspace-a" },
    availability: "observed",
    state: "stopped",
    pinned: true,
    retention_state: "pinned",
    implementation: { kind: "runtime", display_hint: "Runtime Worker" },
    workdir_attachments: [],
    diagnostics: [],
    ...{ restore_observation_token: "observation-1" },
    ...overrides,
  };
}
function plan(revision = "plan-1"): RuntimeCleanupPlanResponse {
  return {
    workspace_id: "page-test",
    generated_at: "2026-10-06T00:00:00Z",
    runtime_id: "runtime-a",
    revision,
    digest: `digest-${revision}`,
    diagnostics: [],
    workdirs: [],
    workers: [{
      target_id: `target-${revision}`,
      action: "worker_delete",
      worker_id: "W-1",
      runtime_worker_id: "worker-a",
      runtime_id: "runtime-a",
      reason: "stopped",
      pinned: false,
      retention_state: "normal",
      linked_workdir_ids: [],
      running_linked: false,
    }],
  };
}
function catalog(id = "page-test") {
  const state = writable<WorkspaceWorkersState>({
    loading: true,
    workers: [],
    catalogWorkers: null,
    observationVersion: 0,
    catalogRefreshing: true,
  });
  shared.stores.set(id, state);
  return state;
}
function data(id = "page-test", item = worker()): Data {
  return {
    workspaceId: id,
    workers: { items: [item] },
    workersError: null,
    cleanupPlans: { "runtime-a": plan() },
    cleanupPlanErrors: {},
  } as unknown as Data;
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function publish(store: ReturnType<typeof catalog>, items: Worker[]) {
  store.update((previous) => ({
    loading: false,
    workers: [],
    catalogWorkers: items,
    observationVersion: previous.observationVersion + 1,
    catalogRefreshing: false,
  }));
}
afterEach(() => {
  cleanup();
  shared.stores.clear();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  clearWorkspaceAlerts();
});

test("catalog updates preserve retention and workdirs, and stale page data never replaces a ready catalog", async () => {
  const store = catalog();
  vi.stubGlobal("fetch", vi.fn(async () => Response.json(plan())));
  const view = render(WorkersPage, {
    params: { workspaceId: "page-test" },
    data: data(),
  });
  const attachment: NonNullable<Worker["workdir_attachments"]>[number] = {
    alias: "source",
    effective_permissions: { read: true, write: false, command: false },
    working_directory: {
      working_directory_id: "dir-a",
      display_name: "Retained Workdir",
      status: "active",
      materializer_kind: "client_hosted_external",
      source: {
        kind: "external_grant",
        grant_id: "grant-a",
        grant_permissions: { read: true, write: false, command: false },
      },
    },
  };
  publish(store, [
    worker({
      display_name: "Live catalog",
      retention_state: "retained",
      workdir_attachments: [attachment],
    }),
  ]);
  expect(await screen.findByText("Live catalog")).toBeTruthy();
  expect(screen.getByText("retained")).toBeTruthy();
  expect(screen.getByText(/Retained Workdir/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Unpin Live catalog" }))
    .toBeTruthy();
  expect(
    screen.getByRole("link", { name: "Live catalog" }).getAttribute("href"),
  ).toBe("/w/page-test/workers/W-1-live-catalog");
  await view.rerender({
    data: data("page-test", worker({ display_name: "Stale loader worker" })),
  });
  expect(screen.getByText("Live catalog")).toBeTruthy();
  expect(screen.queryByText("Stale loader worker")).toBeNull();
});

test("catalog change invalidates Delete immediately and an older cleanup response cannot restore a stale action", async () => {
  const store = catalog();
  const older = deferred<Response>();
  const newer = deferred<Response>();
  const request = vi.fn().mockReturnValueOnce(older.promise)
    .mockReturnValueOnce(newer.promise);
  vi.stubGlobal("fetch", request);
  render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
  publish(store, [worker({ pinned: false })]);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole("button", { name: "Delete Catalog worker" }))
    .toBeNull();
  publish(store, [worker({ pinned: false, state: "idle" })]);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(2));
  newer.resolve(Response.json({ ...plan("new"), workers: [] }));
  older.resolve(Response.json(plan("old")));
  await new Promise((done) => setTimeout(done, 0));
  expect(screen.queryByRole("button", { name: "Delete Catalog worker" }))
    .toBeNull();
  expect(
    request.mock.calls.every(([url]) =>
      !String(url).endsWith("/cleanup-executions")
    ),
  ).toBe(true);
});

test("refresh start invalidates an installed Delete candidate without requesting cleanup from the old catalog", async () => {
  const store = catalog();
  const request = vi.fn(async () => Response.json(plan()));
  vi.stubGlobal("fetch", request);
  render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
  publish(store, [worker({ pinned: false })]);
  expect(await screen.findByRole("button", { name: "Delete Catalog worker" }))
    .toBeTruthy();
  expect(request).toHaveBeenCalledTimes(1);

  // The flag itself invalidates plans, independently of the observation version.
  store.update((previous) => ({ ...previous, catalogRefreshing: true }));
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: "Delete Catalog worker" }))
      .toBeNull()
  );
  store.update((previous) => ({ ...previous }));
  await new Promise((done) => setTimeout(done, 0));
  expect(request).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("button", { name: "Delete Catalog worker" }))
    .toBeNull();

  // No catalog or version change on completion: the transition still reloads.
  store.update((previous) => ({ ...previous, catalogRefreshing: false }));
  expect(await screen.findByRole("button", { name: "Delete Catalog worker" }))
    .toBeTruthy();
  expect(request).toHaveBeenCalledTimes(2);
});

test.each(["during refresh", "after completion"] as const)(
  "a previous cleanup GET arriving %s cannot reinstall a stale candidate; completion reschedules cleanup",
  async (settlement) => {
    const store = catalog();
    const older = deferred<Response>();
    const newer = deferred<Response>();
    const request = vi.fn().mockReturnValueOnce(older.promise)
      .mockReturnValueOnce(newer.promise);
    vi.stubGlobal("fetch", request);
    render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
    publish(store, [worker({ pinned: false })]);
    await waitFor(() => expect(request).toHaveBeenCalledTimes(1));

    store.update((previous) => ({
      ...previous,
      catalogRefreshing: true,
      observationVersion: previous.observationVersion + 1,
      catalogWorkers: [
        worker({ pinned: false, display_name: "Overlay worker" }),
      ],
    }));
    expect(await screen.findByText("Overlay worker")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete Overlay worker" }))
      .toBeNull();
    expect(request).toHaveBeenCalledTimes(1);
    if (settlement === "during refresh") {
      older.resolve(Response.json(plan("stale")));
      await new Promise((done) => setTimeout(done, 0));
      expect(screen.queryByRole("button", { name: "Delete Overlay worker" }))
        .toBeNull();
    }

    // Successful catalog GET retains exactly the same catalog and observationVersion.
    store.update((previous) => ({ ...previous, catalogRefreshing: false }));
    await waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    if (settlement === "after completion") {
      older.resolve(Response.json(plan("stale")));
      await new Promise((done) => setTimeout(done, 0));
      expect(screen.queryByRole("button", { name: "Delete Overlay worker" }))
        .toBeNull();
    }
    newer.resolve(Response.json({ ...plan("fresh"), workers: [] }));
    await new Promise((done) => setTimeout(done, 0));
    expect(screen.queryByRole("button", { name: "Delete Overlay worker" }))
      .toBeNull();
    expect(request).toHaveBeenCalledTimes(2);
    expect(
      request.mock.calls.every(([url]) =>
        String(url).endsWith("/cleanup-plan")
      ),
    ).toBe(true);
  },
);

test("restore follow-up cannot request cleanup while the authoritative catalog refresh remains unresolved", async () => {
  const store = catalog();
  const request = vi.fn(async (input: RequestInfo | URL) =>
    String(input).endsWith("/restore")
      ? restoreResponse("accepted")
      : Response.json(plan())
  );
  vi.stubGlobal("fetch", request);
  render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
  publish(store, [worker({ pinned: false })]);
  expect(await screen.findByRole("button", { name: "Delete Catalog worker" }))
    .toBeTruthy();
  shared.refresh.mockImplementationOnce(async () => {
    store.update((previous) => ({
      ...previous,
      catalogRefreshing: true,
      observationVersion: previous.observationVersion + 1,
    }));
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Restore Catalog worker" }),
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Pin Catalog worker",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  expect(screen.queryByRole("button", { name: "Delete Catalog worker" }))
    .toBeNull();
  expect(
    request.mock.calls.filter(([url]) => String(url).endsWith("/cleanup-plan")),
  ).toHaveLength(1);
});

test.each(["switch", "dispose"] as const)(
  "late pin response has no UI, alert, refresh, or cleanup effects after %s",
  async (mode) => {
    catalog();
    catalog("next-workspace");
    const pending = deferred<Response>();
    const request = vi.fn((_input: RequestInfo | URL) => pending.promise);
    vi.stubGlobal("fetch", request);
    let alerts: unknown[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    const view = render(WorkersPage, {
      params: { workspaceId: "page-test" },
      data: data(),
    });
    await fireEvent.click(
      screen.getByRole("button", { name: "Unpin Catalog worker" }),
    );
    expect(String(request.mock.calls[0][0])).toContain("/api/w/page-test/");
    if (mode === "switch") {
      await view.rerender({
        data: data("next-workspace", worker({ display_name: "Next worker" })),
      });
    } else view.unmount();
    pending.resolve(
      Response.json({ message: "old pin failure" }, { status: 409 }),
    );
    await pending.promise;
    await new Promise((done) => setTimeout(done, 0));
    expect(shared.refresh).not.toHaveBeenCalled();
    expect(request).toHaveBeenCalledTimes(1);
    expect(alerts).toEqual([]);
    if (mode === "switch") {
      expect(screen.getByRole("button", { name: "Unpin Next worker" }))
        .toBeTruthy();
    }
    unsubscribe();
  },
);

test("pin response does not overwrite authoritative catalog retention", async () => {
  const store = catalog();
  const pending = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) =>
      String(input).endsWith("/pin")
        ? pending.promise
        : Promise.resolve(Response.json(plan()))
    ),
  );
  render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
  publish(store, [worker()]);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Unpin Catalog worker" }),
  );
  publish(store, [worker({ retention_state: "newer catalog", pinned: true })]);
  pending.resolve(
    Response.json({
      workspace_id: "page-test",
      runtime_id: "runtime-a",
      worker_id: "worker-a",
      pinned: false,
      retention_state: "old response",
    }),
  );
  await waitFor(() => expect(shared.refresh).toHaveBeenCalledWith("page-test"));
  expect(screen.getByText("newer catalog")).toBeTruthy();
  expect(screen.queryByText("old response")).toBeNull();
});

test.each(["switch", "dispose"] as const)(
  "late Delete response is ignored after %s",
  async (mode) => {
    const store = catalog();
    catalog("next-workspace");
    const pending = deferred<Response>();
    const request = vi.fn((input: RequestInfo | URL) =>
      String(input).endsWith("/cleanup-executions")
        ? pending.promise
        : Promise.resolve(Response.json(plan()))
    );
    vi.stubGlobal("fetch", request);
    const view = render(WorkersPage, {
      params: { workspaceId: "page-test" },
      data: data(),
    });
    publish(store, [worker({ pinned: false })]);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Delete Catalog worker" }),
    );
    const execution = request.mock.calls.find(([url]) =>
      String(url).endsWith("/cleanup-executions")
    );
    expect(String(execution?.[0])).toContain("/api/w/page-test/");
    if (mode === "switch") {
      await view.rerender({
        data: data("next-workspace", worker({ display_name: "Next worker" })),
      });
    } else view.unmount();
    const countBeforeCompletion = request.mock.calls.length;
    let alerts: unknown[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    pending.resolve(
      Response.json({ message: "old deletion failure" }, { status: 409 }),
    );
    await pending.promise;
    await new Promise((done) => setTimeout(done, 0));
    expect(shared.refresh).not.toHaveBeenCalled();
    expect(request).toHaveBeenCalledTimes(countBeforeCompletion);
    expect(alerts).toEqual([]);
    if (mode === "switch") {
      expect(screen.getByRole("button", { name: "Unpin Next worker" }))
        .toBeTruthy();
    }
    unsubscribe();
  },
);

function restoreResponse(
  state: "accepted" | "rejected" | "rolled_back" | "reconciliation_required",
  extra = {},
) {
  return Response.json({
    workspace_id: "page-test",
    runtime_id: "runtime-a",
    worker_id: "worker-a",
    result: { state, diagnostics: [], ...extra },
  });
}

test.each(["stopped", "idle", "running", "paused", "unavailable"] as const)(
  "Restore eligibility uses observed stopped catalog lifecycle (%s)",
  async (state) => {
    catalog();
    vi.stubGlobal("fetch", vi.fn(async () => Response.json(plan())));
    render(WorkersPage, {
      params: { workspaceId: "page-test" },
      data: data(
        "page-test",
        worker({
          state,
          availability: state === "unavailable" ? "unavailable" : "observed",
        }),
      ),
    });
    expect(
      (screen.getByRole("button", {
        name: "Restore Catalog worker",
      }) as HTMLButtonElement).disabled,
    ).toBe(state !== "stopped");
  },
);

test.each(
  ["accepted", "rejected", "rolled_back", "reconciliation_required"] as const,
)(
  "Restore classifies %s through the shared helper, without treating HTTP 200 as success",
  async (state) => {
    catalog();
    let alerts: { title?: string; level: string }[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    const request = vi.fn(async (input: RequestInfo | URL) =>
      String(input).endsWith("/restore")
        ? restoreResponse(state)
        : Response.json(plan())
    );
    vi.stubGlobal("fetch", request);
    render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
    await fireEvent.click(
      screen.getByRole("button", { name: "Restore Catalog worker" }),
    );
    await waitFor(() =>
      expect(shared.refresh).toHaveBeenCalledWith("page-test")
    );
    expect(alerts.at(-1)?.level).toBe(
      state === "accepted"
        ? "info"
        : state === "reconciliation_required"
        ? "warning"
        : "error",
    );
    expect(alerts.some((alert) => alert.title === "Worker restored")).toBe(
      state === "accepted",
    );
    expect(
      Boolean(
        screen.queryByRole("button", { name: "Retry Restore Catalog worker" }),
      ),
    ).toBe(state === "reconciliation_required");
    // The response is never projected into the catalog by the action.
    expect(screen.getByRole("cell", { name: "stopped" })).toBeTruthy();
    unsubscribe();
  },
);

test.each(["transport", "reconciliation"] as const)(
  "explicit %s retry reuses the exact guarded request despite newer observations",
  async (failure) => {
    const store = catalog();
    let attempts = 0;
    const request = vi.fn(
      async (input: RequestInfo | URL, _init?: RequestInit) => {
        if (!String(input).endsWith("/restore")) return Response.json(plan());
        if (++attempts === 1) {
          if (failure === "transport") throw new TypeError("connection lost");
          return restoreResponse("reconciliation_required");
        }
        return restoreResponse("accepted");
      },
    );
    vi.stubGlobal("fetch", request);
    render(WorkersPage, { params: { workspaceId: "page-test" }, data: data() });
    await fireEvent.click(
      screen.getByRole("button", { name: "Restore Catalog worker" }),
    );
    const retry = await screen.findByRole("button", {
      name: "Retry Restore Catalog worker",
    });
    publish(store, [
      worker({
        pinned: true,
        ...{ restore_observation_token: "observation-2" },
      }),
    ]);
    await waitFor(() =>
      expect((retry as HTMLButtonElement).disabled).toBe(false)
    );
    expect(attempts).toBe(1);
    expect(
      (screen.getByRole("button", {
        name: "Restore Catalog worker",
      }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await fireEvent.click(retry);
    await waitFor(() => expect(attempts).toBe(2));
    const operations = request.mock.calls.filter(([url]) =>
      String(url).endsWith("/restore")
    );
    expect(operations[1][1]?.body).toBe(operations[0][1]?.body);
    expect(JSON.parse(String(operations[0][1]?.body))).toEqual({
      expected_observation_token: "observation-1",
      request_id: expect.any(String),
    });
  },
);

test.each(["switch", "dispose"] as const)(
  "late restore result is ignored after %s",
  async (mode) => {
    catalog();
    catalog("next-workspace");
    const pending = deferred<Response>();
    const request = vi.fn((_input: RequestInfo | URL) => pending.promise);
    vi.stubGlobal("fetch", request);
    let alerts: unknown[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    const view = render(WorkersPage, {
      params: { workspaceId: "page-test" },
      data: data(),
    });
    await fireEvent.click(
      screen.getByRole("button", { name: "Restore Catalog worker" }),
    );
    expect(
      (screen.getByRole("button", {
        name: "Unpin Catalog worker",
      }) as HTMLButtonElement).disabled,
    ).toBe(true);
    if (mode === "switch") {
      await view.rerender({
        data: data("next-workspace", worker({ display_name: "Next worker" })),
      });
    } else view.unmount();
    pending.resolve(restoreResponse("accepted"));
    await pending.promise;
    await new Promise((done) => setTimeout(done, 0));
    expect(shared.refresh).not.toHaveBeenCalled();
    expect(request).toHaveBeenCalledTimes(1);
    expect(alerts).toEqual([]);
    unsubscribe();
  },
);
