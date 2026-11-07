// @vitest-environment happy-dom
// 本番component/API adapterと接続し、HTTPとbrowser storage障害だけを代替する。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import SubjectConversation from "./SubjectConversation.svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
import { conversationAttemptKey, readConversationAttempt } from "./conversation-attempt.ts";
import type { ChatMessage, ChatUserMessageRequest } from "#lib/generated/chat-api.ts";

const props = {
  workspaceId: "w",
  subjectId: "s",
  accountId: authUser.account_id,
  subjectName: "Research companion",
};
function authenticate(account = props.accountId) {
  browserSession.snapshot = {
    kind: "ready",
    value: {
      actor: {
        ...authUser,
        account_id: account,
        auth_method: "browser_session",
      },
      authentication_reference: "fixture-session",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
    },
  };
}
beforeEach(() => {
  sessionStorage.clear();
  localStorage.clear();
  authenticate();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
  localStorage.clear();
});
function harness() {
  const messages: ChatMessage[] = [];
  const posts: ChatUserMessageRequest[] = [];
  let mode:
    | "accepted"
    | "lost"
    | "conflict"
    | "failed"
    | "no_current_body"
    | "unknown"
    | "rejected" = "accepted";
  let readStatus = 200;
  const conversation = {
    id: "c",
    subject_id: props.subjectId,
    participant_account_id: props.accountId,
    last_user_message_id: null as string | null,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = new URL(String(input), "http://fixture.invalid");
      if (url.pathname.endsWith("/conversations")) {
        return Response.json({ ...conversation, last_user_message_id: null });
      }
      if (init?.method === "POST") {
        const body = JSON.parse(String(init.body)) as ChatUserMessageRequest;
        posts.push(body);
        if (mode === "conflict" || mode === "failed") {
          return new Response(null, {
            status: mode === "conflict" ? 409 : 500,
          });
        }
        let message = messages.find((m) => m.operation_id === body.operation_id);
        if (!message) {
          message = {
            id: `m${messages.length + 1}`,
            conversation_id: "c",
            sequence: messages.length + 1,
            sender: { kind: "user", account_id: props.accountId },
            ...body,
            last_user_message_id: `m${messages.length + 1}`,
          };
          messages.push(message);
        }
        conversation.last_user_message_id = message.id;
        if (mode === "lost") throw new TypeError("ACK lost after persistence");
        const delivery = mode === "no_current_body"
          ? { state: mode }
          : mode === "accepted"
          ? { state: mode, runtime_id: "r", worker_id: "w" }
          : { state: mode, runtime_id: "r", worker_id: "w", detail: "fixture" };
        return Response.json({ message, delivery });
      }
      if (readStatus !== 200) return new Response(null, { status: readStatus });
      const after = Number(url.searchParams.get("after_sequence") ?? 0);
      const newer = messages.filter((m) => m.sequence > after);
      return Response.json({
        conversation,
        messages: newer.slice(0, 100),
        has_more: newer.length > 100,
      });
    }),
  );
  function reply(text: string) {
    messages.push({
      id: `m${messages.length + 1}`,
      conversation_id: "c",
      sequence: messages.length + 1,
      sender: { kind: "subject", subject_id: props.subjectId },
      text,
      reply_to: null,
      work_ref: null,
      operation_id: `reply-${messages.length + 1}`,
      last_user_message_id: conversation.last_user_message_id,
    });
  }
  return {
    messages,
    posts,
    reply,
    mode: (next: typeof mode) => {
      mode = next;
    },
    deny: (status: number) => {
      readStatus = status;
    },
  };
}
const input = () => screen.getByRole("textbox", { name: "Message to Research companion" });
const send = () => screen.getByRole("button", { name: "Send message" });
async function compose(text: string) {
  await fireEvent.input(input(), { target: { value: text } });
  await fireEvent.click(send());
}
async function ready() {
  await screen.findByText("No public messages yet.");
}
async function refresh() {
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh history" }),
  );
}

test("acceptance allows Busy follow-up without inventing a reply; multiple and delayed publications appear only from saved pages", async () => {
  const h = harness();
  render(SubjectConversation, props);
  await ready();
  await compose("first input while working");
  await screen.findByText(
    "Runtime accepted this input. Processing and replies are not confirmed.",
  );
  await compose("second input before any reply");
  await waitFor(() => expect(h.posts).toHaveLength(2));
  await waitFor(() => expect(readConversationAttempt(sessionStorage, props)).toBeNull());
  expect(screen.queryByText("Explicit first reply")).toBeNull();
  await waitFor(() =>
    expect((screen.getByRole("button", { name: "Refresh history" }) as HTMLButtonElement).disabled)
      .toBe(false)
  );
  await refresh();
  await waitFor(() => expect(screen.getAllByText("You", { exact: true })).toHaveLength(2));
  h.reply("Explicit first reply");
  h.reply("Explicit second reply");
  await refresh();
  await screen.findByText("Explicit second reply");
  h.reply("Later evaluated background result");
  await fireEvent(window, new Event("online"));
  await screen.findByText("Later evaluated background result");
  expect(screen.queryByText("internal assistant output")).toBeNull();
  expect(screen.getAllByText("Research companion", { exact: true }))
    .toHaveLength(3);
});

test("ACK loss keeps full input across reload while the next draft stays editable; exact retry does not duplicate public rows", async () => {
  const h = harness();
  h.mode("lost");
  const view = render(SubjectConversation, props);
  await ready();
  const original = "  original input\n日本語  ";
  await compose(original);
  await screen.findByText(/request outcome could not be confirmed/i);
  await fireEvent.input(input(), { target: { value: "edited next draft" } });
  expect((send() as HTMLButtonElement).disabled).toBe(true);
  expect(readConversationAttempt(sessionStorage, props)?.text).toBe(original);
  view.unmount();
  render(SubjectConversation, props);
  await screen.findByText(
    "Previous send outcome is unknown. Its original input is retained.",
  );
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry same input",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  h.mode("accepted");
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry same input" }),
  );
  await screen.findByText(
    "Runtime accepted this input. Processing and replies are not confirmed.",
  );
  expect(h.posts).toHaveLength(2);
  expect(h.posts[0]).toEqual(h.posts[1]);
  expect(h.messages).toHaveLength(1);
  await waitFor(() => expect(screen.getAllByText("You", { exact: true })).toHaveLength(1));
  expect(readConversationAttempt(sessionStorage, props)).toBeNull();
});

for (
  const mode of [
    "no_current_body",
    "unknown",
    "rejected",
    "conflict",
    "failed",
  ] as const
) {
  test(`${mode} retains original input and never shows an accepted or processed result`, async () => {
    const h = harness();
    h.mode(mode);
    render(SubjectConversation, props);
    await ready();
    await compose("retained");
    await waitFor(() =>
      expect(
        (screen.getByRole("button", {
          name: "Retry same input",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    expect(readConversationAttempt(sessionStorage, props)?.text).toBe(
      "retained",
    );
    expect(
      screen.queryByText(
        "Runtime accepted this input. Processing and replies are not confirmed.",
      ),
    ).toBeNull();
    await fireEvent.input(input(), { target: { value: "do not replace" } });
    expect((send() as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(
      screen.getByRole("button", { name: "Retry same input" }),
    );
    await waitFor(() => expect(h.posts).toHaveLength(2));
    expect(h.posts[0]).toEqual(h.posts[1]);
  });
}

test("storage write refusal dispatches nothing, preserves the draft and can be rechecked without sending", async () => {
  const h = harness();
  render(SubjectConversation, props);
  await ready();
  const originalStorage = sessionStorage;
  vi.stubGlobal("sessionStorage", {
    getItem: originalStorage.getItem.bind(originalStorage),
    setItem: () => {
      throw new Error("quota");
    },
    removeItem: originalStorage.removeItem.bind(originalStorage),
  });
  await compose("unsent draft");
  await screen.findByText(
    "Send input could not be retained safely. Nothing new was dispatched.",
  );
  expect(h.posts).toHaveLength(0);
  expect((input() as HTMLTextAreaElement).value).toBe("unsent draft");
  vi.stubGlobal("sessionStorage", originalStorage);
  await fireEvent.click(
    screen.getByRole("button", { name: "Recheck retry storage" }),
  );
  expect(h.posts).toHaveLength(0);
  await fireEvent.click(send());
  await screen.findByText(
    "Runtime accepted this input. Processing and replies are not confirmed.",
  );
});

test("accepted clear failure blocks new input; local recheck clears only the matching retry record without redelivery", async () => {
  const h = harness();
  render(SubjectConversation, props);
  await ready();
  const originalStorage = sessionStorage;
  vi.stubGlobal("sessionStorage", {
    getItem: originalStorage.getItem.bind(originalStorage),
    setItem: originalStorage.setItem.bind(originalStorage),
    removeItem: () => {
      throw new Error("blocked");
    },
  });
  await compose("accepted once");
  await screen.findByText(/local retry record could not be cleared/);
  expect(readConversationAttempt(sessionStorage, props)).not.toBeNull();
  expect(h.posts).toHaveLength(1);
  vi.stubGlobal("sessionStorage", originalStorage);
  await fireEvent.click(
    screen.getByRole("button", { name: "Recheck retry storage" }),
  );
  expect(readConversationAttempt(sessionStorage, props)).toBeNull();
  expect(h.posts).toHaveLength(1);
});

test("more than one page follows saved sequence, reconnect adds only unseen messages, and permission loss removes composer/history", async () => {
  const h = harness();
  for (let i = 1; i <= 105; ++i) h.reply(`Public reply ${i}`);
  render(SubjectConversation, props);
  await screen.findByText("Public reply 100");
  expect(screen.queryByText("Public reply 101")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Load newer messages" }),
  );
  await screen.findByText("Public reply 105");
  await fireEvent(window, new Event("online"));
  await waitFor(() =>
    expect(screen.getAllByText("Research companion", { exact: true }))
      .toHaveLength(105)
  );
  h.deny(403);
  await refresh();
  await screen.findByText("You do not have access to this conversation.");
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByText("Public reply 1", { exact: true })).toBeNull();
});

test("corrupt or mismatched stored input fails closed rather than silently issuing a new operation", async () => {
  const h = harness();
  sessionStorage.setItem(conversationAttemptKey(props), "corrupt");
  render(SubjectConversation, props);
  await ready();
  await compose("must not send");
  expect(h.posts).toHaveLength(0);
  expect((send() as HTMLButtonElement).disabled).toBe(true);
  expect(sessionStorage.getItem(conversationAttemptKey(props))).toBe("corrupt");
});
