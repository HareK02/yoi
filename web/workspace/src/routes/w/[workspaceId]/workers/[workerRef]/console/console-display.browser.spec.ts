// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  Event as ProtocolEvent,
  SessionSnapshot,
} from "$lib/generated/protocol";
import type { Worker } from "$lib/workspace/sidebar/types";
import ConsolePage from "./+page.svelte";

const multiplexer = vi.hoisted(() => {
  type Listener = {
    onFrame(frame: unknown): void;
    onStatus?(status: "connecting" | "open" | "closed", message?: string): void;
  };
  const listeners: Listener[] = [];
  return {
    listeners,
    close: vi.fn(),
    sendWorkerMethod: vi.fn(),
    subscribe: vi.fn((_selector: unknown, listener: Listener) => {
      listeners.push(listener);
      listener.onStatus?.("connecting");
      return {
        close: multiplexer.close,
        sendWorkerMethod: multiplexer.sendWorkerMethod,
      };
    }),
  };
});

vi.mock("$lib/workspace/multiplexer", () => ({
  workspaceMultiplexer: () => ({ subscribe: multiplexer.subscribe }),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function worker(workerId = "worker-a", runtimeId = "runtime-a"): Worker {
  return {
    worker_id: workerId,
    runtime_id: runtimeId,
    resource_key: `worker:${workerId}`,
    display_name: workerId,
    label: workerId,
    state: "idle",
    host_id: "host-a",
    profile: "default",
    tags: [],
    diagnostics: [],
    capabilities: {
      can_stop: true,
      can_spawn_followup: true,
    },
    workspace: { visibility: "workspace", identity: "workspace-a" },
    implementation: { kind: "builtin", display_hint: "test" },
    workdir_attachments: [],
  } as unknown as Worker;
}

function pageData(workerId = "worker-a", runtimeId = "runtime-a") {
  return {
    workspaceId: "workspace-a",
    runtimeId,
    workerId,
    worker: worker(workerId, runtimeId),
    workerError: null,
  };
}

function emptySession(): SessionSnapshot {
  return {
    pending_submissions: {
      revision: 0,
      notification_count: 0,
      head_id: null,
      submissions: [],
    },
    entries: [],
  };
}

function sessionWithUserMessage(content: string): SessionSnapshot {
  return {
    ...emptySession(),
    entries: [{
      kind: "user_input",
      segments: [{ kind: "text", content }],
      entry_id: `entry-${content}`,
      timestamp: 1,
      provenance: "human_input",
      derived_from: [],
    }],
  };
}

function snapshotEvent(session = emptySession()): ProtocolEvent {
  return {
    event: "snapshot",
    data: {
      session,
      greeting: {
        worker_name: "worker-a",
        cwd: "",
        provider: "test",
        model: "test",
        scope_summary: "test",
        tools: [],
        context_window: 0,
        context_tokens: 0,
      },
      state: { last_command_id: 0, state: { kind: "idle" } },
      in_flight: { blocks: [], commands: [], compaction: null },
      internal_workers: [],
    },
  };
}

function subscribedFrame(session = emptySession()) {
  return {
    frame: "response",
    message: {
      result: "subscribed",
      payload: {
        snapshot: {
          topic: "worker_protocol",
          data: { events: [snapshotEvent(session)] },
        },
      },
    },
  };
}

function latestListener() {
  const listener = multiplexer.listeners.at(-1);
  if (!listener) throw new Error("missing Worker protocol listener");
  return listener;
}

async function settleMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  multiplexer.listeners.length = 0;
  multiplexer.close.mockClear();
  multiplexer.sendWorkerMethod.mockClear();
  multiplexer.subscribe.mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

test("pending inputs show literal previews above Composer with revision-fenced icon actions", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.pending_submissions = {
    revision: 7, head_id: "private-submission-id", notification_count: 1,
    notification_previews: ["<b>通知内容</b>"],
    submissions: [
      { submission_id: "private-submission-id", preview: "<b>実際の入力</b> " + "長文".repeat(100), accepted_at_ms: 1, segment_count: 1, byte_len: 500 },
      { submission_id: "legacy-submission-id", accepted_at_ms: 2, segment_count: 1, byte_len: 10 },
    ],
  };
  latestListener().onFrame(subscribedFrame(session));
  const region = await screen.findByRole("region", { name: "Pending activations" });
  expect(region.querySelector("details")).toBeNull();
  expect(region.textContent).not.toContain("private-submission-id");
  expect(region.textContent).not.toContain("legacy-submission-id");
  expect(region.textContent).toContain("<b>実際の入力</b>");
  expect(region.querySelector("b")).toBeNull();
  expect(region.textContent).toContain("Preview unavailable");
  const queue = screen.getByRole("list", { name: "Queued inputs" });
  const notifications = screen.getByRole("group", { name: "Notifications" });
  expect(queue.querySelectorAll("li")).toHaveLength(2);
  expect(notifications.textContent).toContain("<b>通知内容</b>");
  expect(notifications.querySelector("button")).toBeNull();
  expect(notifications.querySelector("b")).toBeNull();
  expect(region.textContent).toContain("2 Queued");
  expect(screen.queryByRole("button", { name: /Clear all/ })).toBeNull();
  expect(region.nextElementSibling).toBe(view.container.querySelector(".console-composer"));
  expect(screen.queryByRole("button", { name: "Continue next" })).toBeNull();
  const cancel = screen.getByRole("button", { name: "Cancel queued input 1" });
  expect(cancel.textContent?.trim()).toBe("");
  expect(cancel.querySelector("path")?.getAttribute("d")).toBe("M5 12h14");
  await fireEvent.focus(cancel);
  await fireEvent.click(cancel);
  expect(multiplexer.sendWorkerMethod).toHaveBeenLastCalledWith({
    method: "cancel_pending_submission",
    params: { submission_id: "private-submission-id", expected_revision: 7 },
  });
  // Do not remove a row optimistically before the authoritative snapshot arrives.
  expect(queue.querySelectorAll("li")).toHaveLength(2);
  latestListener().onFrame({ frame: "event", message: { event: "event", data: {
    payload: { event: "worker_protocol", data: { worker_id: "worker-a", event: {
      event: "pending_submissions_changed", data: { pending: {
        ...session.pending_submissions, revision: 8, head_id: "legacy-submission-id",
        submissions: session.pending_submissions.submissions.slice(1),
      } },
    } } },
  } } });
  await waitFor(() => expect(queue.querySelectorAll("li")).toHaveLength(1));
  expect(notifications.textContent).toContain("<b>通知内容</b>");
  await fireEvent.click(screen.getByRole("button", { name: "Cancel queued input 1" }));
  expect(multiplexer.sendWorkerMethod).toHaveBeenLastCalledWith({
    method: "cancel_pending_submission", params: { submission_id: "legacy-submission-id", expected_revision: 8 },
  });
  latestListener().onFrame(subscribedFrame(emptySession()));
  await waitFor(() => expect(screen.queryByRole("region", { name: "Pending activations" })).toBeNull());
});

test("notification-only state displays previews without actions and supports legacy runtimes", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.pending_submissions.notification_count = 2;
  session.pending_submissions.notification_previews = ["一件目の通知", "二件目の通知"];
  session.pending_submissions.head_id = "notification-head";
  latestListener().onFrame(subscribedFrame(session));
  const region = await screen.findByRole("region", { name: "Pending activations" });
  expect(region.textContent).toContain("0 Queued");
  expect(region.textContent).toContain("一件目の通知");
  expect(region.textContent).toContain("二件目の通知");
  expect(region.querySelectorAll("li")).toHaveLength(2);
  expect(region.querySelector("button")).toBeNull();
  delete session.pending_submissions.notification_previews;
  latestListener().onFrame(subscribedFrame(session));
  await waitFor(() => expect(region.textContent).toContain("2 pending · Preview unavailable"));
  expect(region.querySelector("button")).toBeNull();
  latestListener().onFrame(subscribedFrame(emptySession()));
  await waitFor(() => expect(screen.queryByRole("region", { name: "Pending activations" })).toBeNull());
});

test("queue-only state keeps the notification column empty and disables cancellation when disconnected", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.pending_submissions.submissions = [{ submission_id: "queue-1", preview: "送信内容", accepted_at_ms: 1, segment_count: 1, byte_len: 12 }];
  latestListener().onFrame(subscribedFrame(session));
  await screen.findByText("送信内容");
  expect(screen.getByRole("group", { name: "Notifications" }).querySelector("ol")).toBeNull();
  latestListener().onStatus?.("closed", "connection lost");
  await waitFor(() => expect((screen.getByRole("button", { name: "Cancel queued input 1" }) as HTMLButtonElement).disabled).toBe(true));
});

test("turn navigation jumps only the transcript to its user message and stops bottom-follow", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame(sessionWithUserMessage("Navigation question")));
  const button = await screen.findByRole("button", { name: "Turn 1: Navigation question" });
  const scroll = view.container.querySelector<HTMLElement>(".console-scroll")!;
  const user = view.container.querySelector<HTMLElement>(".console-line.user")!;
  Object.defineProperties(scroll, {
    scrollHeight: { configurable: true, value: 2000 },
    clientHeight: { configurable: true, value: 400 },
  });
  scroll.scrollTop = 400;
  vi.spyOn(scroll, "getBoundingClientRect").mockReturnValue({ top: 200 } as DOMRect);
  vi.spyOn(user, "getBoundingClientRect").mockReturnValue({ top: 600 } as DOMRect);
  await fireEvent.click(button);
  expect(scroll.scrollTop).toBe(800);
  await settleMicrotasks();
  expect(scroll.scrollTop).toBe(800);
});

test("does not call Runtime APIs before the Worker execution target resolves", async () => {
  const request = vi.fn();
  vi.stubGlobal("fetch", request);
  render(ConsolePage, {
    data: {
      workspaceId: "workspace-a",
      runtimeId: null,
      workerId: null,
      worker: null,
      workerError: "Worker API request failed with HTTP 404",
    },
  });

  const unavailable = await screen.findByRole("alert");
  expect(unavailable.textContent).toContain("Conversation unavailable");
  expect(unavailable.textContent).toContain(
    "Worker API request failed with HTTP 404",
  );
  await settleMicrotasks();
  expect(request).not.toHaveBeenCalled();
  expect(multiplexer.subscribe).not.toHaveBeenCalled();
});

test("keeps delayed Session and live initial snapshot work in loading state until projection applies", async () => {
  const sessionResponse = deferred<Response>();
  vi.stubGlobal("fetch", vi.fn(() => sessionResponse.promise));
  render(ConsolePage, { data: pageData() });

  expect(screen.getByRole("status").textContent).toContain(
    "Loading conversation",
  );
  expect(screen.queryByText("No conversation to display")).toBeNull();

  sessionResponse.resolve(Response.json({ availability: "live_protocol" }));
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  expect(screen.getByRole("status").textContent).toContain(
    "Waiting for the initial conversation snapshot",
  );
  expect(screen.queryByText("No conversation to display")).toBeNull();

  latestListener().onFrame(subscribedFrame(emptySession()));
  expect(await screen.findByText("No conversation to display")).not.toBeNull();
  expect(screen.getByText("This Worker view has no conversation history yet."))
    .not
    .toBeNull();
});

test("applies retained empty and nonempty snapshots before showing their read-only result", async () => {
  const response = deferred<Response>();
  const nextResponse = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const pending = String(input).includes("worker-b")
        ? nextResponse.promise
        : response.promise;
      return pending.then((value) => value.clone());
    }),
  );
  const view = render(ConsolePage, { data: pageData() });

  expect(screen.getByRole("status").textContent).toContain(
    "Loading conversation",
  );
  expect(screen.queryByText("No conversation to display")).toBeNull();
  response.resolve(Response.json({
    availability: "retained_snapshot",
    snapshot: emptySession(),
  }));
  expect(await screen.findByText("Read-only retained conversation")).not
    .toBeNull();
  expect(screen.getByText("No conversation to display")).not.toBeNull();

  await view.rerender({ data: pageData("worker-b", "runtime-b") });
  expect(screen.getByRole("status").textContent).toContain(
    "Loading conversation",
  );
  expect(screen.queryByText("No conversation to display")).toBeNull();
  nextResponse.resolve(Response.json({
    availability: "retained_snapshot",
    snapshot: sessionWithUserMessage("retained message"),
  }));

  expect(await screen.findByText("retained message")).not.toBeNull();
  expect(screen.getByText("Read-only retained conversation")).not.toBeNull();
  expect(screen.queryByText("No conversation to display")).toBeNull();
});

test("shows bounded initial failures and retries explicitly", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(new Response(null, { status: 503 }))
    .mockResolvedValueOnce(Response.json({
      availability: "retained_snapshot",
      snapshot: emptySession(),
    }));
  vi.stubGlobal("fetch", fetchMock);
  render(ConsolePage, { data: pageData() });

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Unable to load conversation");
  expect(alert.textContent).toContain("failed to observe Worker Session (503)");
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));

  expect(await screen.findByText("No conversation to display")).not.toBeNull();
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

test("distinguishes typed unavailability and an initial live connection closure", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(Response.json({
      availability: "unavailable",
      message: "retained state expired",
    }))
    .mockResolvedValueOnce(Response.json({ availability: "live_protocol" }));
  vi.stubGlobal("fetch", fetchMock);
  render(ConsolePage, { data: pageData() });

  expect((await screen.findByRole("alert")).textContent).toContain(
    "Conversation unavailable retained state expired",
  );
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onStatus?.("closed", "subscription ended before snapshot");

  expect((await screen.findByRole("alert")).textContent).toContain(
    "subscription ended before snapshot",
  );
  expect(screen.queryByText("No conversation to display")).toBeNull();
});

test("preserves rendered content while reconnecting", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(Response.json({ availability: "live_protocol" }))
    ),
  );
  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("kept message")),
  );
  expect(await screen.findByText("kept message")).not.toBeNull();

  latestListener().onStatus?.("closed", "Workspace subscription disconnected");
  expect(await screen.findByText("Conversation updates are unavailable")).not
    .toBeNull();
  expect(screen.getByText("kept message")).not.toBeNull();
  expect(screen.queryByText("No conversation to display")).toBeNull();

  latestListener().onStatus?.("connecting", "resubscribing");
  expect(await screen.findByText("Refreshing conversation")).not.toBeNull();
  expect(screen.getByText("kept message")).not.toBeNull();
});

test("route changes clear prior content and fence stale Session responses", async () => {
  const staleReload = deferred<Response>();
  const current = deferred<Response>();
  let firstWorkerLoads = 0;
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    if (String(input).includes("worker-b")) {
      return current.promise.then((value) => value.clone());
    }
    firstWorkerLoads += 1;
    if (firstWorkerLoads === 1) {
      return Promise.resolve(Response.json({ availability: "live_protocol" }));
    }
    return staleReload.promise.then((value) => value.clone());
  });
  vi.stubGlobal("fetch", fetchMock);
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("previous worker content")),
  );
  expect(await screen.findByText("previous worker content")).not.toBeNull();

  latestListener().onStatus?.("closed", "reload old worker");
  await fireEvent.click(
    await screen.findByRole("button", { name: "Retry" }),
  );
  await waitFor(() => expect(firstWorkerLoads).toBe(2));
  await view.rerender({ data: pageData("worker-b", "runtime-b") });
  expect(screen.queryByText("previous worker content")).toBeNull();
  expect(screen.getByRole("status").textContent).toContain(
    "Loading conversation",
  );

  current.resolve(Response.json({
    availability: "retained_snapshot",
    snapshot: sessionWithUserMessage("current worker content"),
  }));
  expect(await screen.findByText("current worker content")).not.toBeNull();

  staleReload.resolve(Response.json({
    availability: "unavailable",
    message: "stale worker error",
  }));
  await settleMicrotasks();
  expect(screen.queryByText("stale worker error")).toBeNull();
  expect(screen.queryByText("previous worker content")).toBeNull();
  expect(screen.getByText("current worker content")).not.toBeNull();
});
