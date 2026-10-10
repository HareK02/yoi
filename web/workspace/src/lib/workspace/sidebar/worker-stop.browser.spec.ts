// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type {
  SubscriptionFrame,
  SubscriptionWorker,
} from "#lib/generated/protocol.ts";
import WorkersNavSection from "./WorkersNavSection.svelte";
import { disposeWorkspaceWorkersStore } from "./worker-subscription";
import {
  clearWorkspaceAlerts,
  workspaceAlerts,
} from "#lib/workspace/alerts/store.ts";

const transport = vi.hoisted(() => ({
  listener: null as { onFrame(frame: SubscriptionFrame): void } | null,
  close: vi.fn(),
}));
vi.mock("#lib/workspace/multiplexer.ts", () => ({
  workspaceMultiplexer: () => ({
    subscribe: (
      _selector: unknown,
      listener: NonNullable<typeof transport.listener>,
    ) => {
      transport.listener = listener;
      return { close: transport.close };
    },
  }),
}));

afterEach(() => {
  cleanup();
  disposeWorkspaceWorkersStore("stop-test");
  disposeWorkspaceWorkersStore("next-test");
  clearWorkspaceAlerts();
  transport.listener = null;
  vi.unstubAllGlobals();
});

function snapshot(
  state: SubscriptionWorker["state"],
  availability: SubscriptionWorker["availability"],
  workspaceId = "stop-test",
): SubscriptionFrame {
  return {
    protocol_version: 2,
    frame: "response",
    message: {
      result: "subscribed",
      payload: {
        request_id: "request-1",
        subscription_id: "subscription-1",
        selector: { topic: "workspace_workers" },
        snapshot: {
          topic: "workers",
          data: {
            workers: [{
              worker_id: "worker-a",
              runtime_id: "runtime-a",
              resource_key: "W-1",
              availability,
              state,
              // An absent foreground snapshot must not prevent stopping an active catalog Worker.
              worker_state: null,
              has_running_internal_workers: false,
              display_name: "Test worker",
              workspace_id: workspaceId,
              workdir_attachments: [],
              ...{ restore_observation_token: "observation-1" },
            }],
          },
        },
      },
    },
  };
}

test.each(["idle", "running", "paused", "stopped"] as const)(
  "Stop uses observed catalog state without capabilities (%s)",
  async (state) => {
    const request = vi.fn(async (input: RequestInfo | URL) =>
      String(input).endsWith("/stop")
        ? Response.json({
          state: "accepted",
          runtime_id: "runtime-a",
          worker_id: "worker-a",
          diagnostics: [],
        })
        : new Response(null, { status: 404 })
    );
    vi.stubGlobal("fetch", request);
    render(WorkersNavSection, { workspaceId: "stop-test" });
    await waitFor(() => expect(transport.listener).not.toBeNull());
    transport.listener!.onFrame(snapshot(state, "observed"));
    await fireEvent.click(
      await screen.findByRole("button", { name: "Actions for Test worker" }),
    );
    const stop = screen.getByRole("menuitem", {
      name: "Stop",
    }) as HTMLButtonElement;
    expect(stop.disabled).toBe(state === "stopped");
    if (state === "stopped") return;
    await fireEvent.click(stop);
    await waitFor(() =>
      expect(request.mock.calls.some(([url]) => String(url).endsWith("/stop")))
        .toBe(true)
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Actions for Test worker" }),
    );
    await waitFor(() =>
      expect(
        (screen.getByRole("menuitem", { name: "Stop" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false)
    );
    // A successful POST is not a lifecycle observation. Only a newer authoritative frame disables Stop.
    transport.listener!.onFrame(snapshot("stopped", "observed"));
    await waitFor(() =>
      expect(
        (screen.getByRole("menuitem", { name: "Stop" }) as HTMLButtonElement)
          .disabled,
      ).toBe(true)
    );
  },
);

test("Unavailable observation disables Stop even when catalog state is running", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(null, { status: 404 })),
  );
  render(WorkersNavSection, { workspaceId: "stop-test" });
  await waitFor(() => expect(transport.listener).not.toBeNull());
  transport.listener!.onFrame(snapshot("running", "unavailable"));
  await fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Test worker" }),
  );
  expect(
    (screen.getByRole("menuitem", { name: "Stop" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
});

test.each(["switch", "dispose"] as const)(
  "late Stop completion is ignored after workspace %s",
  async (mode) => {
    let resolve!: (response: Response) => void;
    const pending = new Promise<Response>((done) => {
      resolve = done;
    });
    const request = vi.fn((input: RequestInfo | URL) =>
      String(input).endsWith("/stop")
        ? pending
        : Promise.resolve(new Response(null, { status: 404 }))
    );
    vi.stubGlobal("fetch", request);
    const view = render(WorkersNavSection, { workspaceId: "stop-test" });
    await waitFor(() => expect(transport.listener).not.toBeNull());
    transport.listener!.onFrame(snapshot("running", "observed"));
    await fireEvent.click(
      await screen.findByRole("button", { name: "Actions for Test worker" }),
    );
    await fireEvent.click(screen.getByRole("menuitem", { name: "Stop" }));
    const operation = request.mock.calls.find(([url]) =>
      String(url).endsWith("/stop")
    );
    expect(String(operation?.[0])).toContain("/api/w/stop-test/");
    if (mode === "switch") {
      await view.rerender({ workspaceId: "next-test" });
      transport.listener!.onFrame(snapshot("idle", "observed", "next-test"));
      await fireEvent.click(
        await screen.findByRole("button", { name: "Actions for Test worker" }),
      );
    } else view.unmount();
    clearWorkspaceAlerts();
    let alerts: unknown[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    const readsBeforeCompletion =
      request.mock.calls.filter(([url]) => String(url).endsWith("/workers"))
        .length;
    resolve(
      Response.json({
        state: "accepted",
        runtime_id: "runtime-a",
        worker_id: "worker-a",
        diagnostics: [],
      }),
    );
    await pending;
    await new Promise((done) => setTimeout(done, 0));
    expect(alerts).toEqual([]);
    expect(
      request.mock.calls.filter(([url]) => String(url).endsWith("/workers"))
        .length,
    ).toBe(readsBeforeCompletion);
    if (mode === "switch") {
      expect(
        (screen.getByRole("menuitem", { name: "Stop" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false);
    }
    unsubscribe();
  },
);

test.each(["idle", "running", "paused", "stopped"] as const)(
  "sidebar Restore eligibility (%s)",
  async (state) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(null, { status: 404 })),
    );
    render(WorkersNavSection, { workspaceId: "stop-test" });
    await waitFor(() => expect(transport.listener).not.toBeNull());
    transport.listener!.onFrame(snapshot(state, "observed"));
    await fireEvent.click(
      await screen.findByRole("button", { name: "Actions for Test worker" }),
    );
    expect(
      (screen.getByRole("menuitem", { name: "Restore" }) as HTMLButtonElement)
        .disabled,
    ).toBe(state !== "stopped");
  },
);

test("sidebar Restore pins the target and preserves an uncertain request for explicit retry", async () => {
  let attempts = 0;
  const request = vi.fn(
    async (input: RequestInfo | URL, _init?: RequestInit) => {
      if (!String(input).endsWith("/restore")) {
        return new Response(null, { status: 404 });
      }
      return Response.json({
        workspace_id: "stop-test",
        runtime_id: "runtime-a",
        worker_id: "worker-a",
        result: {
          state: ++attempts === 1 ? "reconciliation_required" : "accepted",
          diagnostics: [],
        },
      });
    },
  );
  vi.stubGlobal("fetch", request);
  render(WorkersNavSection, { workspaceId: "stop-test" });
  await waitFor(() => expect(transport.listener).not.toBeNull());
  transport.listener!.onFrame(snapshot("stopped", "observed"));
  await fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Test worker" }),
  );
  await fireEvent.click(screen.getByRole("menuitem", { name: "Restore" }));
  await waitFor(() => expect(attempts).toBe(1));
  await fireEvent.click(
    screen.getByRole("button", { name: "Actions for Test worker" }),
  );
  const retry = await screen.findByRole("menuitem", { name: "Retry Restore" });
  await waitFor(() =>
    expect((retry as HTMLButtonElement).disabled).toBe(false)
  );
  expect(
    (screen.getByRole("menuitem", { name: "Restore" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await fireEvent.click(retry);
  await waitFor(() => expect(attempts).toBe(2));
  const operations = request.mock.calls.filter(([url]) =>
    String(url).endsWith("/restore")
  );
  expect(String(operations[0][0])).toContain(
    "/api/w/stop-test/runtimes/runtime-a/workers/worker-a/restore",
  );
  expect(operations[1][1]?.body).toBe(operations[0][1]?.body);
});

test.each(["switch", "dispose"] as const)(
  "late sidebar Restore completion is ignored after %s",
  async (mode) => {
    let resolve!: (response: Response) => void;
    const pending = new Promise<Response>((done) => {
      resolve = done;
    });
    const request = vi.fn((input: RequestInfo | URL) =>
      String(input).endsWith("/restore")
        ? pending
        : Promise.resolve(new Response(null, { status: 404 }))
    );
    vi.stubGlobal("fetch", request);
    const view = render(WorkersNavSection, { workspaceId: "stop-test" });
    await waitFor(() => expect(transport.listener).not.toBeNull());
    transport.listener!.onFrame(snapshot("stopped", "observed"));
    await fireEvent.click(
      await screen.findByRole("button", { name: "Actions for Test worker" }),
    );
    await fireEvent.click(screen.getByRole("menuitem", { name: "Restore" }));
    if (mode === "switch") {
      await view.rerender({ workspaceId: "next-test" });
      transport.listener!.onFrame(snapshot("stopped", "observed", "next-test"));
      await fireEvent.click(
        await screen.findByRole("button", { name: "Actions for Test worker" }),
      );
    } else view.unmount();
    clearWorkspaceAlerts();
    let alerts: unknown[] = [];
    const unsubscribe = workspaceAlerts.subscribe((value) => {
      alerts = value;
    });
    const readsBeforeCompletion =
      request.mock.calls.filter(([url]) => String(url).endsWith("/workers"))
        .length;
    resolve(
      Response.json({
        workspace_id: "stop-test",
        runtime_id: "runtime-a",
        worker_id: "worker-a",
        result: { state: "accepted", diagnostics: [] },
      }),
    );
    await pending;
    await new Promise((done) => setTimeout(done, 0));
    expect(alerts).toEqual([]);
    expect(
      request.mock.calls.filter(([url]) => String(url).endsWith("/workers"))
        .length,
    ).toBe(readsBeforeCompletion);
    if (mode === "switch") {
      expect(
        (screen.getByRole("menuitem", { name: "Restore" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false);
    }
    unsubscribe();
  },
);

test("sidebar Delete cannot proceed to execution when its cleanup plan arrives after workspace switch", async () => {
  let resolve!: (response: Response) => void;
  const pending = new Promise<Response>((done) => {
    resolve = done;
  });
  const request = vi.fn((input: RequestInfo | URL) =>
    String(input).endsWith("/cleanup-plan")
      ? pending
      : Promise.resolve(new Response(null, { status: 404 }))
  );
  vi.stubGlobal("fetch", request);
  const view = render(WorkersNavSection, { workspaceId: "stop-test" });
  await waitFor(() => expect(transport.listener).not.toBeNull());
  transport.listener!.onFrame(snapshot("stopped", "observed"));
  await fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Test worker" }),
  );
  await fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
  expect(
    request.mock.calls.some(([url]) => String(url).endsWith("/cleanup-plan")),
  ).toBe(true);
  await view.rerender({ workspaceId: "next-test" });
  clearWorkspaceAlerts();
  resolve(Response.json({
    workspace_id: "stop-test",
    runtime_id: "runtime-a",
    generated_at: "2026-10-06T00:00:00Z",

    digest: "old-digest",
    workdirs: [],
    diagnostics: [],
    workers: [{
      target_id: "worker-target",
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
  }));
  await pending;
  await new Promise((done) => setTimeout(done, 0));
  expect(
    request.mock.calls.some(([url]) =>
      String(url).endsWith("/cleanup-executions")
    ),
  ).toBe(false);
});
