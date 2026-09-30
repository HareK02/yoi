// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  Event as ProtocolEvent,
  SessionConversationTurn,
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

function emptyHistoryPage() {
  return {
    availability: "page",
    page: {
      session_id: "session-a",
      lineage_id: "lineage-a",
      compact_ancestor_lineage_ids: [],
      turns: [],
      next_cursor: null,
      has_more: false,
    },
  };
}

function historyPage(
  indices: number[],
  cursor: string | null,
  lineageId = "lineage-a",
  parentLineage?: {
    lineage_id: string;
    adopted_through_turn?: SessionConversationTurn | null;
  },
) {
  return {
    availability: "page",
    page: {
      session_id: "session-a",
      lineage_id: lineageId,
      compact_ancestor_lineage_ids: [],
      parent_lineage: parentLineage,
      turns: indices.map((index) => ({
        turn_id: `user-${index}`,
        entries: [
          {
            entry_id: `user-${index}`,
            timestamp: index,
            provenance: "human_input",
            kind: "message",
            role: "user",
            content: [{ kind: "text", text: `question ${index}` }],
          },
          {
            entry_id: `assistant-${index}`,
            timestamp: index,
            provenance: "model_output",
            kind: "message",
            role: "assistant",
            content: [{ kind: "text", text: `done ${index}` }],
          },
        ],
      })),
      next_cursor: cursor,
      has_more: cursor !== null,
    },
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

  expect(
    await within(screen.getByRole("article", { name: "main transcript" }))
      .findByText("retained message"),
  ).not.toBeNull();
  expect(screen.getByText("Read-only retained conversation")).not.toBeNull();
  expect(screen.queryByText("No conversation to display")).toBeNull();
});

test("shows bounded initial failures and retries explicitly", async () => {
  let sessionRequests = 0;
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    if (String(input).includes("/session/history")) {
      return Promise.resolve(Response.json(emptyHistoryPage()));
    }
    sessionRequests += 1;
    return Promise.resolve(
      sessionRequests === 1
        ? new Response(null, { status: 503 })
        : Response.json({
          availability: "retained_snapshot",
          snapshot: emptySession(),
        }),
    );
  });
  vi.stubGlobal("fetch", fetchMock);
  render(ConsolePage, { data: pageData() });

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Unable to load conversation");
  expect(alert.textContent).toContain("failed to observe Worker Session (503)");
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));

  expect(await screen.findByText("No conversation to display")).not.toBeNull();
  expect(sessionRequests).toBe(2);
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
  expect(
    await within(screen.getByRole("article", { name: "main transcript" }))
      .findByText("kept message"),
  ).not.toBeNull();

  latestListener().onStatus?.("closed", "Workspace subscription disconnected");
  expect(await screen.findByText("Conversation updates are unavailable")).not
    .toBeNull();
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "kept message",
    ),
  ).not.toBeNull();
  expect(screen.queryByText("No conversation to display")).toBeNull();

  latestListener().onStatus?.("connecting", "resubscribing");
  expect(await screen.findByText("Refreshing conversation")).not.toBeNull();
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "kept message",
    ),
  ).not.toBeNull();
});

test("reconnect snapshot refreshes lineage and removes stale sibling history", async () => {
  let historyRequests = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      if (!String(input).includes("/session/history")) {
        return Promise.resolve(
          Response.json({ availability: "live_protocol" }),
        );
      }
      historyRequests += 1;
      const response = historyPage([historyRequests], null);
      if (historyRequests === 2) {
        response.page.lineage_id = "lineage-b";
      }
      return Promise.resolve(Response.json(response));
    }),
  );

  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("lineage A current")),
  );
  await waitFor(() =>
    expect(screen.getAllByText("question 1")).toHaveLength(2)
  );

  latestListener().onStatus?.("closed", "disconnected during rewind");
  latestListener().onStatus?.("connecting", "reconnecting");
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("lineage B current")),
  );

  await waitFor(() =>
    expect(screen.getAllByText("question 2")).toHaveLength(2)
  );
  expect(screen.queryByText("question 1")).toBeNull();
  expect(historyRequests).toBe(2);
});

test("snapshot supersedes an in-flight page before stale lineage can render", async () => {
  const staleHistory = deferred<Response>();
  let historyRequests = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      if (!String(input).includes("/session/history")) {
        return Promise.resolve(
          Response.json({ availability: "live_protocol" }),
        );
      }
      historyRequests += 1;
      if (historyRequests === 1) return staleHistory.promise;
      const response = historyPage([2], null);
      response.page.lineage_id = "lineage-b";
      return Promise.resolve(Response.json(response));
    }),
  );

  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("lineage A current")),
  );
  await waitFor(() => expect(historyRequests).toBe(1));

  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("lineage B current")),
  );
  staleHistory.resolve(Response.json(historyPage([1], null)));

  await waitFor(() =>
    expect(screen.getAllByText("question 2")).toHaveLength(2)
  );
  expect(screen.queryByText("question 1")).toBeNull();
  expect(historyRequests).toBe(2);
});

test("fork refresh reconciles an in-flight older page to the adopted prefix", async () => {
  const staleOlder = deferred<Response>();
  let historyRequests = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      if (!String(input).includes("/session/history")) {
        return Promise.resolve(
          Response.json({ availability: "live_protocol" }),
        );
      }
      historyRequests += 1;
      if (historyRequests === 1) {
        return Promise.resolve(
          Response.json(historyPage([46, 47, 48, 49, 50], "older-46")),
        );
      }
      if (historyRequests === 2) return staleOlder.promise;
      return Promise.resolve(Response.json(historyPage(
        [101, 102, 103, 104, 105],
        "older-101",
        "lineage-fork",
        {
          lineage_id: "lineage-a",
          adopted_through_turn: historyPage([48], null).page
            .turns[0] as SessionConversationTurn,
        },
      )));
    }),
  );

  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame());
  await waitFor(() =>
    expect(screen.getAllByText("question 50")).toHaveLength(2)
  );

  const transcriptScroller = screen.getByLabelText("main transcript")
    .parentElement as HTMLElement;
  transcriptScroller.scrollTop = 0;
  await fireEvent.scroll(transcriptScroller);
  await waitFor(() => expect(historyRequests).toBe(2));

  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("fork current")),
  );
  staleOlder.resolve(Response.json(historyPage([41, 42, 43, 44, 45], null)));

  await waitFor(() =>
    expect(screen.getAllByText("question 101")).toHaveLength(2)
  );
  expect(screen.getAllByText("question 48")).toHaveLength(2);
  expect(screen.queryByText("question 49")).toBeNull();
  expect(screen.getAllByText("question 41")).toHaveLength(2);
  expect(historyRequests).toBe(3);
});

test("route changes clear prior content and fence stale Session responses", async () => {
  const staleReload = deferred<Response>();
  const current = deferred<Response>();
  let firstWorkerLoads = 0;
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    if (String(input).includes("/session/history")) {
      return Promise.resolve(Response.json(emptyHistoryPage()));
    }
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
  expect(
    await within(screen.getByRole("article", { name: "main transcript" }))
      .findByText("previous worker content"),
  ).not.toBeNull();

  latestListener().onStatus?.("closed", "reload old worker");
  await fireEvent.click(
    await screen.findByRole("button", { name: "Retry" }),
  );
  await waitFor(() => expect(firstWorkerLoads).toBe(2));
  await view.rerender({ data: pageData("worker-b", "runtime-b") });
  expect(
    within(screen.getByRole("article", { name: "main transcript" }))
      .queryByText("previous worker content"),
  ).toBeNull();
  expect(screen.getByRole("status").textContent).toContain(
    "Loading conversation",
  );

  current.resolve(Response.json({
    availability: "retained_snapshot",
    snapshot: sessionWithUserMessage("current worker content"),
  }));
  expect(
    await within(screen.getByRole("article", { name: "main transcript" }))
      .findByText(
        "current worker content",
      ),
  ).not.toBeNull();

  staleReload.resolve(Response.json({
    availability: "unavailable",
    message: "stale worker error",
  }));
  await settleMicrotasks();
  expect(screen.queryByText("stale worker error")).toBeNull();
  expect(
    within(screen.getByRole("article", { name: "main transcript" }))
      .queryByText("previous worker content"),
  ).toBeNull();
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "current worker content",
    ),
  ).not.toBeNull();
});

test("loads retained conversation turns five at a time into the log and turn bar", async () => {
  const historyPages = [
    {
      availability: "page",
      page: {
        session_id: "session-a",
        lineage_id: "lineage-a",
        compact_ancestor_lineage_ids: [],
        turns: [8, 9, 10, 11, 12].map((index) => ({
          turn_id: `user-${index}`,
          entries: [
            {
              entry_id: `user-${index}`,
              timestamp: index,
              provenance: "human_input",
              kind: "message",
              role: "user",
              content: [{ kind: "text", text: `question ${index}` }],
            },
            {
              entry_id: `assistant-${index}`,
              timestamp: index,
              provenance: "model_output",
              kind: "message",
              role: "assistant",
              content: [{ kind: "text", text: `done ${index}` }],
            },
          ],
        })),
        next_cursor: "older-8",
        has_more: true,
      },
    },
    {
      availability: "page",
      page: {
        session_id: "session-a",
        lineage_id: "lineage-a",
        compact_ancestor_lineage_ids: [],
        turns: [3, 4, 5, 6, 7].map((index) => ({
          turn_id: `user-${index}`,
          entries: [{
            entry_id: `user-${index}`,
            timestamp: index,
            provenance: "human_input",
            kind: "message",
            role: "user",
            content: [{ kind: "text", text: `question ${index}` }],
          }],
        })),
        next_cursor: "older-3",
        has_more: true,
      },
    },
    historyPage([1, 2], null),
  ];
  let historyRequests = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      if (String(input).includes("/session/history")) {
        return Promise.resolve(Response.json(historyPages[historyRequests++]));
      }
      return Promise.resolve(Response.json({ availability: "live_protocol" }));
    }),
  );

  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame());

  await waitFor(() =>
    expect(screen.getAllByText("question 8")).toHaveLength(2)
  );
  expect(screen.getAllByText("done 12")).toHaveLength(2);
  const transcriptScroller = screen.getByLabelText("main transcript")
    .parentElement as HTMLElement;
  transcriptScroller.scrollTop = 0;
  await fireEvent.scroll(transcriptScroller);
  await waitFor(() =>
    expect(screen.getAllByText("question 3")).toHaveLength(2)
  );
  expect(historyRequests).toBe(2);

  const navigation = screen.getByLabelText("Conversation turns");
  const navigationList = navigation.firstElementChild as HTMLElement;
  navigationList.scrollTop = 0;
  await fireEvent.scroll(navigationList);
  await fireEvent.scroll(navigationList);
  transcriptScroller.scrollTop = 0;
  await fireEvent.scroll(transcriptScroller);
  await settleMicrotasks();
  expect(historyRequests).toBe(2);

  await fireEvent.click(
    screen.getByRole("button", {
      name: "Earlier conversation available · load 5 turns",
    }),
  );
  await waitFor(() =>
    expect(screen.getAllByText("question 1")).toHaveLength(2)
  );
  expect(screen.getAllByText("Start of conversation")).toHaveLength(2);
  expect(historyRequests).toBe(3);

  const scrollIntoView = vi.fn();
  vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(
    scrollIntoView,
  );
  await fireEvent.click(
    screen.getByRole("button", {
      name: "Jump to conversation: question 8",
    }),
  );
  expect(scrollIntoView).toHaveBeenCalledOnce();
});

test("keeps live content when history fails and retries the bounded page explicitly", async () => {
  let historyRequests = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      if (String(input).includes("/session/history")) {
        historyRequests += 1;
        return Promise.resolve(
          historyRequests === 1
            ? new Response(null, { status: 503 })
            : Response.json(historyPage([1, 2, 3, 4], null)),
        );
      }
      return Promise.resolve(Response.json({ availability: "live_protocol" }));
    }),
  );

  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("current response remains")),
  );

  expect(
    await within(screen.getByRole("article", { name: "main transcript" }))
      .findByText("current response remains"),
  ).not.toBeNull();
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Earlier conversation unavailable · retry",
    }),
  );
  await waitFor(() =>
    expect(screen.getAllByText("question 1")).toHaveLength(2)
  );
  expect(historyRequests).toBe(2);
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "current response remains",
    ),
  ).not.toBeNull();
  expect(screen.getAllByText("Start of conversation")).toHaveLength(2);
});

test("fences delayed history pages when the selected Worker route changes", async () => {
  const stale = deferred<Response>();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = String(input);
      if (path.includes("/session/history")) {
        return path.includes("worker-b")
          ? Promise.resolve(Response.json(historyPage([21], null)))
          : stale.promise;
      }
      return Promise.resolve(Response.json({
        availability: "retained_snapshot",
        snapshot: emptySession(),
      }));
    }),
  );

  const view = render(ConsolePage, { data: pageData() });
  await view.rerender({ data: pageData("worker-b", "runtime-b") });
  await waitFor(() =>
    expect(screen.getAllByText("question 21")).toHaveLength(2)
  );

  stale.resolve(Response.json(historyPage([99], null)));
  await settleMicrotasks();
  expect(screen.queryByText("question 99")).toBeNull();
  expect(screen.getAllByText("question 21")).toHaveLength(2);
});
