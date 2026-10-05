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
} from "#lib/generated/protocol.ts";
import type { Worker } from "#lib/workspace/sidebar/types.ts";
import { EditorView } from "@codemirror/view";
import * as alerts from "#lib/workspace/alerts/store.ts";
import ConsolePage from "./+page.svelte";

// happy-dom does not implement Web Animations. Motion itself is covered in Chromium.
vi.mock("svelte/motion", () => ({ prefersReducedMotion: { current: true } }));

const multiplexer = vi.hoisted(() => {
  type Listener = {
    onFrame(frame: unknown): void;
    onStatus?(
      status: "connecting" | "open" | "closed",
      failure?: { kind: "protocol" | "transport"; message: string },
    ): void;
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

vi.mock("#lib/workspace/multiplexer.ts", () => ({
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

test.each(["live_protocol", "retained_snapshot"] as const)("overlapping history and %s render tool activity once in both modes", async (availability) => {
  const session = sessionWithUserMessage("inspect tools");
  for (const [callId, name] of [["call-a", "Bash"], ["call-b", "Grep"]]) {
    session.entries.push({
      kind: "tool_call", entry_id: `entry-${callId}`, timestamp: 2,
      provenance: "model_output", call_id: callId, name, arguments: "{}",
    }, {
      kind: "tool_result", entry_id: `result-${callId}`, timestamp: 3,
      provenance: "tool_output", call_id: callId, summary: "done", content: "done", is_error: false,
    });
  }
  const historyResponse = deferred<Response>();
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
    if (String(input).includes("/session/history")) return historyResponse.promise;
    return Response.json({ availability, snapshot: session });
  }));
  const view = render(ConsolePage, { data: pageData() });
  if (availability === "live_protocol") {
    await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
    latestListener().onFrame(subscribedFrame(session));
  }
  await screen.findByText("searched 1 time・ran 1 command");
  historyResponse.resolve(Response.json({
    availability: "page", page: {
      ...emptyHistoryPage().page,
      turns: [{ turn_id: session.entries[0].entry_id, entries: session.entries }],
    },
  }));
  await screen.findByText("Start of conversation");
  expect(view.container.querySelectorAll(".activity-summary")).toHaveLength(1);
  expect(screen.getAllByText("searched 1 time・ran 1 command")).toHaveLength(1);
  const footer = screen.getByLabelText("Worker model and context");
  const overview = screen.getByRole("switch", { name: "Overview" });
  expect(screen.queryByRole("button", { name: "Normal" })).toBeNull();
  const details = screen.getByRole("button", { name: "Details" });
  expect(view.container.querySelector(".console-header")).toBeNull();
  expect(footer.contains(overview)).toBe(true);
  expect(overview.nextElementSibling).toBe(details);
  expect(overview.closest("form")).toBeNull();
  expect(view.container.querySelector(".console-composer")?.nextElementSibling).toBe(footer);
  expect(overview.getAttribute("aria-checked")).toBe("true");
  await fireEvent.click(overview);
  expect(overview.getAttribute("aria-checked")).toBe("false");
  expect(overview.textContent?.trim()).toBe("Overview");
  const transcript = screen.getByRole("article", { name: "main transcript" });
  const ids = [...transcript.querySelectorAll("[data-console-line-id]")].map((line) => line.getAttribute("data-console-line-id"));
  expect(ids).toHaveLength(3);
  expect(new Set(ids).size).toBe(ids.length);
  await fireEvent.click(overview);
  expect(overview.getAttribute("aria-checked")).toBe("true");
  expect(overview.textContent?.trim()).toBe("Overview");
  expect(view.container.querySelectorAll(".activity-summary")).toHaveLength(1);
  expect(multiplexer.sendWorkerMethod).not.toHaveBeenCalled();
});

test.each([
  { text: ":peer", message: "Invalid arguments. Usage: :peer <worker-name>", transportFailure: false },
  { text: "keep this draft", message: "test transport failure", transportFailure: true },
])("Composer keeps $text on failure without adding any footer message", async ({ text, message, transportFailure }) => {
  const alert = vi.spyOn(alerts, "pushWorkspaceAlert");
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame(emptySession()));
  await screen.findByText("No conversation to display");
  const cm = EditorView.findFromDOM(view.container.querySelector(".cm-editor") as HTMLElement)!;
  const form = view.container.querySelector(".console-composer") as HTMLFormElement;
  cm.dispatch({ changes: { from: 0, insert: text }, selection: { anchor: text.length } });
  if (transportFailure) multiplexer.sendWorkerMethod.mockImplementationOnce(() => { throw new Error(message); });
  await fireEvent.submit(form);
  await waitFor(() => expect(alert).toHaveBeenCalledWith("error", message, expect.any(Object)));
  expect(cm.state.doc.toString()).toBe(text);
  expect(multiplexer.sendWorkerMethod).toHaveBeenCalledTimes(transportFailure ? 1 : 0);
  expect(form.children).toHaveLength(1);
  expect(form.firstElementChild?.classList.contains("composer-input-shell")).toBe(true);
  expect(form.querySelector('[role="alert"]')).toBeNull();
  expect(form.textContent).not.toContain(message);

  cm.dispatch({ changes: { from: 0, to: cm.state.doc.length, insert: ":peer teammate" } });
  await fireEvent.submit(form);
  await waitFor(() => expect(multiplexer.sendWorkerMethod).toHaveBeenLastCalledWith({
    method: "register_peer", params: { name: "teammate" },
  }));
  expect(cm.state.doc.toString()).toBe("");
  expect(form.children).toHaveLength(1);
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
  expect(screen.queryByRole("group", { name: "Queued inputs" })).toBeNull();
  expect(screen.queryByRole("heading", { name: "0 Queued" })).toBeNull();
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

test("queue-only state hides notifications and disables cancellation when disconnected", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.pending_submissions.submissions = [{ submission_id: "queue-1", preview: "送信内容", accepted_at_ms: 1, segment_count: 1, byte_len: 12 }];
  latestListener().onFrame(subscribedFrame(session));
  await screen.findByText("送信内容");
  expect(screen.queryByRole("group", { name: "Notifications" })).toBeNull();
  expect(screen.queryByRole("heading", { name: "Notifications" })).toBeNull();
  latestListener().onStatus?.("closed", { kind: "transport", message: "connection lost" });
  await waitFor(() => expect((screen.getByRole("button", { name: "Cancel queued input 1" }) as HTMLButtonElement).disabled).toBe(true));
});

test.each(["idle", "running", "paused"] as const)("Composer completion executes Compact/Rewind without header controls or Submit (%s)", async (state) => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const frame = subscribedFrame(emptySession());
  const snapshot = frame.message.payload.snapshot.data.events[0];
  if (snapshot.event === "snapshot" && state !== "idle") {
    snapshot.data.state.state = { kind: "busy", state: { kind: "run", state } };
  }
  latestListener().onFrame(frame);
  await screen.findByText("No conversation to display");
  expect(screen.queryByRole("button", { name: "Compact" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Rewind" })).toBeNull();
  const cm = EditorView.findFromDOM(view.container.querySelector(".cm-editor") as HTMLElement)!;
  cm.focus();
  cm.dispatch({ changes: { from: 0, insert: ":comp" }, selection: { anchor: 5 } });
  await screen.findByRole("option");
  await fireEvent.keyDown(cm.contentDOM, { key: "Enter" });
  await waitFor(() => expect(multiplexer.sendWorkerMethod).toHaveBeenCalledWith({
    method: "compact", params: { command: expect.objectContaining({ command_id: 1 }) },
  }));
  await waitFor(() => expect(cm.state.doc.toString()).toBe(""));
  cm.dispatch({ changes: { from: 0, insert: ":rew" }, selection: { anchor: 4 } });
  const option = await screen.findByRole("option");
  await fireEvent.pointerDown(option);
  await fireEvent.click(option);
  await waitFor(() => expect(multiplexer.sendWorkerMethod).toHaveBeenCalledWith({ method: "list_rewind_targets" }));
  expect(multiplexer.sendWorkerMethod.mock.calls.every(([method]) => method.method !== "submit" && method.method !== "cancel")).toBe(true);
});

test("file completions consume stale responses before the latest prefix and close with the connection", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame(emptySession()));
  await screen.findByText("No conversation to display");
  const cm = EditorView.findFromDOM(view.container.querySelector(".cm-editor") as HTMLElement)!;
  cm.focus();
  cm.dispatch({ changes: { from: 0, insert: "@old" }, selection: { anchor: 4 } });
  await waitFor(() => expect(multiplexer.sendWorkerMethod).toHaveBeenCalledWith({ method: "list_completions", params: { kind: "file", prefix: "old" } }));
  cm.dispatch({ changes: { from: 1, to: 4, insert: "new" }, selection: { anchor: 4 } });
  await settleMicrotasks();
  expect(multiplexer.sendWorkerMethod).toHaveBeenCalledOnce();
  const receive = (value: string) => latestListener().onFrame({
    frame: "event", message: { event: "event", data: { payload: {
      event: "worker_protocol", data: { event: { event: "completions", data: { kind: "file", entries: [{ value, is_dir: false }] } } },
    } } },
  });
  receive("old-result");
  await waitFor(() => expect(multiplexer.sendWorkerMethod).toHaveBeenCalledWith({ method: "list_completions", params: { kind: "file", prefix: "new" } }));
  expect(screen.queryByRole("option")).toBeNull();
  receive("new-result");
  expect((await screen.findByRole("option")).textContent).toContain("@new-result");
  latestListener().onStatus?.("closed", { kind: "transport", message: "connection lost" });
  await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  receive("late-result");
  await settleMicrotasks();
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("mini task summary opens and closes details without a header Tasks button", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.entries.push({
    kind: "tool_call", entry_id: "task-create", timestamp: 1, provenance: "model_output", derived_from: [],
    call_id: "task-call", name: "TaskCreate",
    arguments: JSON.stringify({ subject: "Inspect layout", description: "Detailed task description" }),
  });
  latestListener().onFrame(subscribedFrame(session));
  const summary = await screen.findByRole("button", { name: /1 task — pending/ });
  expect(screen.queryByRole("complementary", { name: "Worker tasks" })).toBeNull();
  expect(view.container.querySelector(".console-header")).toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Details" }));
  const details = await screen.findByRole("complementary", { name: "Worker detail" });
  expect(details.textContent).not.toContain("Capabilities");
  expect(details.textContent).not.toContain("follow-up spawn");
  await fireEvent.click(summary);
  const pane = await screen.findByRole("complementary", { name: "Worker tasks" });
  expect(summary.getAttribute("aria-expanded")).toBe("true");
  expect(summary.getAttribute("aria-controls")).toBe(pane.id);
  expect(pane.textContent).toContain("Detailed task description");
  expect(screen.queryByRole("complementary", { name: "Worker detail" })).toBeNull();
  await fireEvent.click(summary);
  await waitFor(() => expect(screen.queryByRole("complementary", { name: "Worker tasks" })).toBeNull());
  expect(summary.getAttribute("aria-expanded")).toBe("false");
});

test("latest jump is icon-only, hidden at the bottom, and resumes following streamed output", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  const view = render(ConsolePage, { data: pageData() });
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  latestListener().onFrame(subscribedFrame(sessionWithUserMessage("Follow question")));
  await screen.findByRole("button", { name: "Turn 1: Follow question" });
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  const scroll = view.container.querySelector<HTMLElement>(".console-scroll")!;
  let height = 2000;
  Object.defineProperties(scroll, {
    scrollHeight: { configurable: true, get: () => height },
    clientHeight: { configurable: true, value: 400 },
  });
  scroll.scrollTop = 400;
  await fireEvent.scroll(scroll);
  const jump = await screen.findByRole("button", { name: "Jump to latest" });
  expect(jump.textContent?.trim()).toBe("");
  expect(jump.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
  expect(jump.closest(".console-body")).not.toBeNull();
  expect(jump.closest(".console-scroll, .console-composer")).toBeNull();
  const emitDelta = (text: string) => latestListener().onFrame({
    frame: "event", message: { event: "event", data: { payload: {
      event: "worker_protocol", data: { event: { event: "text_delta", data: { text } } },
    } } },
  });
  height = 2500;
  emitDelta("new output");
  await screen.findByText("new output");
  expect(scroll.scrollTop).toBe(400);
  await fireEvent.click(jump);
  await waitFor(() => expect(scroll.scrollTop).toBe(2500));
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  height = 3000;
  emitDelta(" continues");
  await waitFor(() => expect(scroll.scrollTop).toBe(3000));
  scroll.scrollTop = 400;
  await fireEvent.scroll(scroll);
  await screen.findByRole("button", { name: "Jump to latest" });
  scroll.scrollTop = height - 400;
  await fireEvent.scroll(scroll);
  expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  height = 3500;
  emitDelta(" after manual return");
  await waitFor(() => expect(scroll.scrollTop).toBe(3500));
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
  expect(screen.getByRole("button", { name: "Jump to latest" })).not.toBeNull();
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
  latestListener().onStatus?.("closed", {
    kind: "transport",
    message: "subscription ended before snapshot",
  });

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

  latestListener().onStatus?.("closed", {
    kind: "transport",
    message: "Workspace subscription disconnected",
  });
  expect(await screen.findByText("Conversation updates are unavailable")).not
    .toBeNull();
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "kept message",
    ),
  ).not.toBeNull();
  expect(screen.queryByText("No conversation to display")).toBeNull();

  latestListener().onStatus?.("connecting");
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
    expectHistoryTurn("question 1")
  );

  latestListener().onStatus?.("closed", {
    kind: "transport",
    message: "disconnected during rewind",
  });
  latestListener().onStatus?.("connecting");
  latestListener().onFrame(
    subscribedFrame(sessionWithUserMessage("lineage B current")),
  );

  await waitFor(() =>
    expectHistoryTurn("question 2")
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
    expectHistoryTurn("question 2")
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
    expectHistoryTurn("question 50")
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
    expectHistoryTurn("question 101")
  );
  expectHistoryTurn("question 48");
  expect(screen.queryByText("question 49")).toBeNull();
  expectHistoryTurn("question 41");
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

  latestListener().onStatus?.("closed", {
    kind: "transport",
    message: "reload old worker",
  });
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
    expectHistoryTurn("question 8")
  );
  expect(screen.getAllByText("done 12")).toHaveLength(1);
  const transcriptScroller = screen.getByLabelText("main transcript")
    .parentElement as HTMLElement;
  transcriptScroller.scrollTop = 0;
  await fireEvent.scroll(transcriptScroller);
  await waitFor(() =>
    expectHistoryTurn("question 3")
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
    expectHistoryTurn("question 1")
  );
  expect(screen.getAllByText("Start of conversation")).toHaveLength(1);
  expect(historyRequests).toBe(3);

  const target = screen.getByText("question 8").closest(".console-line") as HTMLElement;
  vi.spyOn(target, "getBoundingClientRect").mockReturnValue({ top: 600 } as DOMRect);
  vi.spyOn(transcriptScroller, "getBoundingClientRect").mockReturnValue({ top: 200 } as DOMRect);
  Object.defineProperties(transcriptScroller, {
    scrollHeight: { configurable: true, value: 2000 },
    clientHeight: { configurable: true, value: 400 },
  });
  transcriptScroller.scrollTop = 400;
  await fireEvent.click(
    screen.getByRole("button", {
      name: /^Turn \d+: question 8$/,
    }),
  );
  expect(transcriptScroller.scrollTop).toBe(800);
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
    expectHistoryTurn("question 1")
  );
  expect(historyRequests).toBe(2);
  expect(
    within(screen.getByRole("article", { name: "main transcript" })).getByText(
      "current response remains",
    ),
  ).not.toBeNull();
  expect(screen.getAllByText("Start of conversation")).toHaveLength(1);
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
    expectHistoryTurn("question 21")
  );

  stale.resolve(Response.json(historyPage([99], null)));
  await settleMicrotasks();
  expect(screen.queryByText("question 99")).toBeNull();
  expectHistoryTurn("question 21");
});

function expectHistoryTurn(text: string) {
  expect(screen.getAllByText(text)).toHaveLength(1);
  expect(screen.getByRole("button", { name: new RegExp("^Turn \\d+: " + text + "$") })).not.toBeNull();
}

test("pending groups independently follow authoritative changes, including zero count with stale previews", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ availability: "live_protocol" })));
  render(ConsolePage, { data: pageData() });
  await waitFor(() => expect(multiplexer.subscribe).toHaveBeenCalledOnce());
  const session = emptySession();
  session.pending_submissions.submissions = [{ submission_id: "queue-1", preview: "Queued", accepted_at_ms: 1, segment_count: 1, byte_len: 6 }];
  session.pending_submissions.notification_count = 1;
  session.pending_submissions.notification_previews = ["Notice"];
  latestListener().onFrame(subscribedFrame(session));
  await screen.findByRole("group", { name: "Queued inputs" });
  expect(screen.getByRole("group", { name: "Notifications" })).toBeTruthy();
  session.pending_submissions.notification_count = 0;
  latestListener().onFrame(subscribedFrame(session));
  await waitFor(() => expect(screen.queryByRole("group", { name: "Notifications" })).toBeNull());
  expect(screen.getByRole("group", { name: "Queued inputs" })).toBeTruthy();
  session.pending_submissions.submissions = [];
  session.pending_submissions.notification_count = 1;
  latestListener().onFrame(subscribedFrame(session));
  await waitFor(() => expect(screen.queryByRole("group", { name: "Queued inputs" })).toBeNull());
  expect(screen.getByRole("group", { name: "Notifications" })).toBeTruthy();
  session.pending_submissions.notification_count = 0;
  latestListener().onFrame(subscribedFrame(session));
  await waitFor(() => expect(screen.queryByRole("region", { name: "Pending activations" })).toBeNull());
});
