// @vitest-environment happy-dom
// 本番component/messaging clientと接続し、multiplexerの外側とbrowser storage障害だけを代替する。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import { tick } from 'svelte';
import type { MessagingCommand, MessagingEvent, MessagingResult } from "#lib/generated/chat-api.ts";
const transport = vi.hoisted(() => ({ request: vi.fn<(command: MessagingCommand) => Promise<MessagingResult>>(), listener: null as null | { onStatus(status: string): void; onEvent(event: MessagingEvent): void } }));
vi.mock("#lib/workspace/multiplexer.ts", () => ({ workspaceMultiplexer: () => ({
  connectMessaging(listener: NonNullable<typeof transport.listener>) {
    transport.listener = listener;
    queueMicrotask(() => listener.onStatus("open"));
    return { request: (command: MessagingCommand) => transport.request(command), close: vi.fn() };
  },
}) }));
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
function harness(scope = props) {
  const messages: ChatMessage[] = [];
  const posts: ChatUserMessageRequest[] = [];
  let publishOnPost = true;
  let postGate: Promise<void> | null = null;
  let mode:
    | "accepted"
    | "lost"
    | "conflict"
    | "failed"
    | "no_current_body"
    | "unknown"
    | "unavailable"
    | "rejected" = "accepted";
  let readStatus = 200;
  const conversation = {
    id: "c",
    subject_id: scope.subjectId,
    participant_account_id: scope.accountId,
    last_user_message_id: null as string | null,
  };
  const page = (after = 0) => {
    const newer = messages.filter(m => m.sequence > after);
    return { conversation: { ...conversation }, messages: structuredClone(newer.slice(0, 100)), has_more: newer.length > 100 };
  };
  transport.request.mockImplementation(async command => {
    if (command.kind === "open") return { kind: "opened", conversation: { ...conversation } };
    if (command.kind === "post") {
      const body = command.request;
      posts.push(structuredClone(body));
      if (mode === "conflict" || mode === "failed") return { kind: "rejected", status: mode === "conflict" ? 409 : 500 };
      let message = messages.find(m => m.operation_id === body.operation_id);
      if (!message) {
        message = { id: "m" + (messages.length + 1), conversation_id: "c", sequence: messages.length + 1,
          sender: { kind: "user", account_id: scope.accountId }, ...body, last_user_message_id: "m" + (messages.length + 1) };
        messages.push(message);
      }
      conversation.last_user_message_id = message.id;
      if (mode === "lost") throw new Error("ACK lost after persistence");
      const delivery = mode === "no_current_body" ? { state: mode } : mode === 'unavailable' ? { state: mode, detail: 'fixture' } : mode === "accepted"
        ? { state: mode, runtime_id: "r", worker_id: "w" }
        : { state: mode, runtime_id: "r", worker_id: "w", detail: "fixture" };
      if (publishOnPost) transport.listener?.onEvent({ kind: "messages", subscription_id: "sub", page: page() });
      if (postGate) await postGate;
      return { kind: "saved", response: { message, delivery } };
    }
    if (readStatus !== 200) return { kind: "rejected", status: readStatus };
    if (command.kind === "subscribe") return { kind: "subscribed", subscription_id: "sub", selector: command.selector, page: page(command.selector.after_sequence) };
    if (command.kind === "history") return { kind: "history", page: page(command.query.after_sequence ?? 0) };
    throw new Error("unexpected command");
  });
  function reply(text: string) {
    messages.push({
      id: `m${messages.length + 1}`,
      conversation_id: "c",
      sequence: messages.length + 1,
      sender: { kind: "subject", subject_id: scope.subjectId },
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
    publish: () => transport.listener?.onEvent({ kind: 'messages', subscription_id: 'sub', page: page() }),
    deferPublication: () => { publishOnPost = false; },
    holdPost: () => {
      let release!: () => void;
      postGate = new Promise<void>(resolve => { release = resolve; });
      return release;
    },
    reconnect: () => { transport.listener?.onStatus("closed"); transport.listener?.onStatus("open"); },
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
function typeDraft(text: string) {
  const editor = EditorView.findFromDOM(input())!;
  editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: text } });
}
async function compose(text: string) {
  typeDraft(text);
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
    "delivered",
  );
  await compose("second input before any reply");
  await waitFor(() => expect(h.posts).toHaveLength(2));
  await waitFor(() => expect(readConversationAttempt(sessionStorage, props)).toBeNull());
  await waitFor(() => expect(screen.getAllByText('delivered')).toHaveLength(2));
  expect(screen.queryByText(/Runtime accepted|Processing and replies|Retained send input|Retry uses the same ID|Resolve the retained send/)).toBeNull();
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
  h.reconnect();
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
  await screen.findByText('Delivery could not be confirmed.');
  expect(screen.getAllByText('You', { exact: true })).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Retry message' }).closest('li')?.dataset.local).toBe('true');
  expect(screen.queryByText('delivered')).toBeNull();
  typeDraft("edited next draft");
  expect((send() as HTMLButtonElement).disabled).toBe(true);
  expect(readConversationAttempt(sessionStorage, props)?.text).toBe(original);
  view.unmount();
  render(SubjectConversation, props);
  await screen.findByRole('button', { name: 'Retry message' });
  await waitFor(() => expect(screen.getAllByText('You', { exact: true })).toHaveLength(1));
  expect(screen.getByRole('button', { name: 'Retry message' }).closest('li')?.dataset.messageId).toBe(h.messages[0].id);
  expect(screen.queryByText('delivered')).toBeNull();
  expect(EditorView.findFromDOM(input())!.state.doc.toString()).toBe('edited next draft');
  await waitFor(() =>
    expect(
      (screen.getByRole("button", {
        name: "Retry message",
      }) as HTMLButtonElement).disabled,
    ).toBe(false)
  );
  h.mode("accepted");
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry message" }),
  );
  await screen.findByText(
    "delivered",
  );
  expect(h.posts).toHaveLength(2);
  expect(h.posts[0]).toEqual(h.posts[1]);
  expect(h.messages).toHaveLength(1);
  await waitFor(() => expect(screen.getAllByText("You", { exact: true })).toHaveLength(1));
  expect(readConversationAttempt(sessionStorage, props)).toBeNull();
  expect(EditorView.findFromDOM(input())!.state.doc.toString()).toBe('edited next draft');
});

for (
  const mode of [
    "no_current_body",
    "unknown",
    "unavailable",
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
          name: "Retry message",
        }) as HTMLButtonElement).disabled,
      ).toBe(false)
    );
    expect(readConversationAttempt(sessionStorage, props)?.text).toBe(
      "retained",
    );
    expect(
      screen.queryByText(
        "delivered",
      ),
    ).toBeNull();
    const retry = screen.getByRole('button', { name: 'Retry message' });
    expect(retry.closest('li')?.textContent).toContain('retained');
    expect(retry.querySelector('svg')).not.toBeNull();
    expect(retry.textContent?.trim()).toBe('');
    expect(screen.getAllByText('retained', { exact: true })).toHaveLength(1);
    expect(screen.queryByText(/Runtime accepted|same ID|original request|Retained send input/)).toBeNull();
    typeDraft("do not replace");
    expect((send() as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(
      screen.getByRole("button", { name: "Retry message" }),
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
    "Message could not be saved in this tab. Nothing was sent.",
  );
  expect(h.posts).toHaveLength(0);
  expect(EditorView.findFromDOM(input())!.state.doc.toString()).toBe("unsent draft");
  vi.stubGlobal("sessionStorage", originalStorage);
  await fireEvent.click(
    screen.getByRole("button", { name: "Recheck local storage" }),
  );
  expect(h.posts).toHaveLength(0);
  await fireEvent.click(send());
  await screen.findByText(
    "delivered",
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
  await screen.findByText('Local storage could not be updated. Sending is paused.');
  expect(screen.getByText('delivered').closest('li')?.dataset.messageId).toBe(h.messages[0].id);
  expect(screen.queryByRole('button', { name: 'Retry message' })).toBeNull();
  expect(readConversationAttempt(sessionStorage, props)).not.toBeNull();
  expect(h.posts).toHaveLength(1);
  vi.stubGlobal("sessionStorage", originalStorage);
  await fireEvent.click(
    screen.getByRole("button", { name: "Recheck local storage" }),
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
  h.reconnect();
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

for (const order of ['page-first', 'response-first'] as const) {
  test(`${order} reconciles the sending bubble by operation without losing the next draft or duplicating saved messages`, async () => {
    const h = harness();
    h.deferPublication();
    const release = h.holdPost();
    const ui = render(SubjectConversation, props);
    await ready();
    await compose('pending input');
    const sending = await screen.findByText('sending');
    expect(sending.closest('li')?.dataset.local).toBe('true');
    expect(sending.closest('li')?.textContent).toContain('pending input');
    expect(screen.queryByRole('button', { name: 'Retry message' })).toBeNull();
    typeDraft('next draft stays');
    if (order === 'page-first') {
      h.publish();
      await waitFor(() => expect(screen.getByText('sending').closest('li')?.dataset.messageId).toBe(h.messages[0].id));
      expect(screen.queryByText('delivered')).toBeNull();
      expect(ui.container.querySelector('[data-local]')).toBeNull();
    }
    release();
    const delivered = await screen.findByText('delivered');
    expect(readConversationAttempt(sessionStorage, props)).toBeNull();
    if (order === 'response-first') {
      expect(delivered.closest('li')?.dataset.local).toBe('true');
      h.publish();
    }
    await waitFor(() => expect(screen.getByText('delivered').closest('li')?.dataset.messageId).toBe(h.messages[0].id));
    expect(screen.getAllByText('pending input', { exact: true })).toHaveLength(1);
    expect(ui.container.querySelector('[data-local]')).toBeNull();
    expect(EditorView.findFromDOM(input())!.state.doc.toString()).toBe('next draft stays');
    expect((send() as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByText(/read|processed|Processing and replies/i, { selector: '.chat-delivery' })).toBeNull();
  });
}

test('a second send does not replace an accepted bubble awaiting its saved page', async () => {
  const h = harness();
  h.deferPublication();
  const ui = render(SubjectConversation, props);
  await ready();
  await compose('first unpublished');
  await screen.findByText('delivered');
  await compose('second unpublished');
  await waitFor(() => expect(screen.getAllByText('delivered')).toHaveLength(2));
  expect(ui.container.querySelectorAll('[data-local]')).toHaveLength(2);
  h.publish();
  await waitFor(() => expect(ui.container.querySelectorAll('[data-local]')).toHaveLength(0));
  expect(screen.getAllByText('delivered')).toHaveLength(2);
  expect([...ui.container.querySelectorAll('[data-message-id]')].map(row => row.getAttribute('data-message-id'))).toEqual(h.messages.map(message => message.id));
});

for (const boundary of ['accountId', 'workspaceId', 'subjectId'] as const) {
  test(`${boundary} change fences local bubbles and a late accepted response without clearing the original retry`, async () => {
    const h = harness();
    h.deferPublication();
    const release = h.holdPost();
    const ui = render(SubjectConversation, props);
    await ready();
    await compose('old pending input');
    await screen.findByText('sending');
    typeDraft('old next draft');
    const original = readConversationAttempt(sessionStorage, props);
    const next = { ...props, [boundary]: 'another' };
    if (boundary === 'accountId') authenticate(next.accountId);
    const other = harness(next);
    await ui.rerender(next);
    await ready();
    release();
    // 応答のPromise継続とSvelte描画を処理し、旧contextの遅延結果が混入しないことを確認する。
    await tick();
    expect(screen.queryByText('old pending input')).toBeNull();
    expect(screen.queryByText('delivered')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Retry message' })).toBeNull();
    expect(EditorView.findFromDOM(input())!.state.doc.toString()).toBe('');
    expect(readConversationAttempt(sessionStorage, props)).toEqual(original);
    expect(readConversationAttempt(sessionStorage, next)).toBeNull();
    await compose('new conversation input');
    await screen.findByText('delivered');
    expect(other.posts).toHaveLength(1);
    expect(other.posts[0].operation_id).not.toBe(original?.operationId);
    expect(readConversationAttempt(sessionStorage, props)).toEqual(original);
  });
}
