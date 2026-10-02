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
} from "$lib/generated/protocol";
import WorkersNavSection from "./WorkersNavSection.svelte";
import { disposeWorkspaceWorkersStore } from "./worker-subscription";

const transport = vi.hoisted(() => ({
  listener: null as { onFrame(frame: SubscriptionFrame): void } | null,
  close: vi.fn(),
}));
vi.mock("$lib/workspace/multiplexer", () => ({
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
  transport.listener = null;
  vi.unstubAllGlobals();
});

function snapshot(
  state: SubscriptionWorker["state"],
  availability: SubscriptionWorker["availability"],
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
              workspace_id: "stop-test",
              workdir_attachments: [],
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
