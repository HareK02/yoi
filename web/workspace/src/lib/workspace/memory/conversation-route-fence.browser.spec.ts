// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import SubjectConversation from "./SubjectConversation.svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";
import { readConversationAttempt } from "./conversation-attempt.ts";
import type { ChatMessagePage, MessagingCommand, MessagingResult } from '#lib/generated/chat-api.ts';
const transport = vi.hoisted(() => ({ status: 'open', request: vi.fn<(command: MessagingCommand) => Promise<MessagingResult>>() }));
vi.mock('#lib/workspace/multiplexer.ts', () => ({ workspaceMultiplexer: () => ({
  connectMessaging(listener: { onStatus(status: string): void }) {
    queueMicrotask(() => listener.onStatus(transport.status));
    return { request: (command: MessagingCommand) => transport.request(command), close: vi.fn() };
  },
}) }));
const props = {
  workspaceId: "w",
  subjectId: "s",
  accountId: authUser.account_id,
  subjectName: "Companion",
};
beforeEach(() => {
  sessionStorage.clear();
  localStorage.clear();
  transport.request.mockReset();
  transport.status = 'open';
  browserSession.snapshot = {
    kind: "ready",
    value: {
      actor: { ...authUser, auth_method: "browser_session" },
      authentication_reference: "fixture-session",
      session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
    },
  };
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  sessionStorage.clear();
  localStorage.clear();
});
function conversation(subject: string) {
  return {
    id: `c-${subject}`,
    subject_id: subject,
    participant_account_id: props.accountId,
    last_user_message_id: null,
  };
}
function page(subject: string, text: string): ChatMessagePage {
  return {
    conversation: conversation(subject),
    messages: [{
      id: `m-${subject}`,
      conversation_id: `c-${subject}`,
      sequence: 1,
      sender: { kind: "subject", subject_id: subject },
      text,
      reply_to: null,
      work_ref: null,
      operation_id: "publish",
      last_user_message_id: null,
    }],
    has_more: false,
  };
}
for (const status of [401, 403, 404]) {
  test(`${status} withdraws retained text and fences a late POST ACK without deleting its retry record`, async () => {
    let release!: (response: MessagingResult) => void;
    let posted = false;
    let denied = false;
    vi.stubGlobal('fetch', vi.fn(async () => Response.json(browserSession.snapshot.kind === 'ready' ? browserSession.snapshot.value : {})));
    transport.request.mockImplementation(async command => {
      if (command.kind === 'open') return { kind: 'opened', conversation: conversation('s') };
      if (command.kind === 'post') { posted = true; return await new Promise(resolve => { release = resolve; }); }
      if (denied) return { kind: 'rejected', status };
      const empty = { conversation: conversation('s'), messages: [], has_more: false };
      if (command.kind === 'subscribe') return { kind: 'subscribed', subscription_id: 'sub', selector: command.selector, page: empty };
      return { kind: 'history', page: empty };
    });
    render(SubjectConversation, props);
    await screen.findByText("No public messages yet.");
    EditorView.findFromDOM(screen.getByRole("textbox"))!.dispatch({ changes: { from: 0, insert: "retained private text" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(posted).toBe(true));
    const attempt = readConversationAttempt(sessionStorage, props)!;
    denied = true;
    await fireEvent.click(screen.getByRole("button", { name: "Refresh history" }));
    const deniedMessage = (await screen.findByRole("alert")).textContent;
    await browserSession.refresh();
    expect(screen.getByRole("alert").textContent).toBe(deniedMessage);
    expect(screen.queryByText("retained private text")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    release({ kind: "saved", response: {
      message: {
        id: "user",
        conversation_id: attempt.conversationId,
        sequence: 1,
        sender: { kind: "user", account_id: props.accountId },
        text: attempt.text,
        reply_to: null,
        work_ref: null,
        operation_id: attempt.operationId,
        last_user_message_id: "user",
      },
      delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
    }});
    await Promise.resolve();
    expect(readConversationAttempt(sessionStorage, props)).toEqual(attempt);
    expect(
      screen.queryByText("Runtime accepted this input. Processing and replies are not confirmed."),
    ).toBeNull();
    expect(screen.queryByText("Loading conversation…")).toBeNull();
  });
}

for (const changed of [{ subjectId: "other" }, { workspaceId: "other" }]) {
  test(`late history cannot populate a different ${Object.keys(changed)[0]} route`, async () => {
    let release!: (response: MessagingResult) => void;
    let first = true;
    let readingOld = false;
    transport.request.mockImplementation(async command => {
      if (command.kind === 'open') return { kind: 'opened', conversation: conversation(command.request.subject_id) };
      if (command.kind !== 'subscribe') throw new Error('unexpected command');
      if (first) {
        first = false; readingOld = true;
        return await new Promise(resolve => { release = resolve; });
      }
      return { kind: 'subscribed', subscription_id: 'new', selector: command.selector, page: page(changed.subjectId ?? 's', 'current route public reply') };
    });
    const view = render(SubjectConversation, props);
    await waitFor(() => expect(readingOld).toBe(true));
    await view.rerender({ ...props, ...changed });
    await screen.findByText("current route public reply");
    release({ kind: "subscribed", subscription_id: "old", selector: { subject_id: "s", conversation_id: "c-s", after_sequence: 0 }, page: page("s", "old route secret reply") });
    await Promise.resolve();
    expect(screen.queryByText("old route secret reply")).toBeNull();
  });
}

test("authentication change while POST is in flight never clears the former Account's retained input or adopts its ACK", async () => {
  let release!: (response: MessagingResult) => void;
  let posted = false;
  transport.request.mockImplementation(async command => {
    if (command.kind === 'open') return { kind: 'opened', conversation: conversation('s') };
    if (command.kind === 'post') { posted = true; return await new Promise(resolve => { release = resolve; }); }
    if (command.kind !== 'subscribe') throw new Error('unexpected command');
    return { kind: 'subscribed', subscription_id: 'sub', selector: command.selector, page: { conversation: conversation('s'), messages: [], has_more: false } };
  });
  render(SubjectConversation, props);
  await screen.findByText("No public messages yet.");
  EditorView.findFromDOM(screen.getByRole("textbox"))!.dispatch({ changes: { from: 0, insert: "former account input" } });
  await fireEvent.click(screen.getByRole("button", { name: "Send message" }));
  await waitFor(() => expect(posted).toBe(true));
  const attempt = readConversationAttempt(sessionStorage, props)!;
  browserSession.invalidate();
  release({ kind: "saved", response: {
    message: {
      id: "user",
      conversation_id: attempt.conversationId,
      sequence: 1,
      sender: { kind: "user", account_id: props.accountId },
      text: attempt.text,
      reply_to: null,
      work_ref: null,
      operation_id: attempt.operationId,
      last_user_message_id: "user",
    },
    delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
  }});
  await Promise.resolve();
  expect(readConversationAttempt(sessionStorage, props)).toEqual(attempt);
  expect(
    screen.queryByText("Runtime accepted this input. Processing and replies are not confirmed."),
  ).toBeNull();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(readConversationAttempt(sessionStorage, { ...props, accountId: "other" })).toBeNull();
});

test('an unavailable initial socket stops loading and leaves explicit recovery available without claiming an auth verdict', async () => {
  transport.status = 'closed';
  transport.request.mockImplementation(async command => {
    if (command.kind === 'open') return { kind: 'opened', conversation: conversation('s') };
    if (command.kind === 'subscribe') return { kind: 'subscribed', subscription_id: 'recovered', selector: command.selector, page: page('s', 'recovered public reply') };
    throw new Error('unexpected command');
  });
  render(SubjectConversation, props);
  await screen.findByRole('alert');
  expect(screen.queryByText('Loading conversation…')).toBeNull();
  expect(screen.getByRole('button', { name: 'Refresh history' }).hasAttribute('disabled')).toBe(false);
  await fireEvent.click(screen.getByRole('button', { name: 'Refresh history' }));
  await screen.findByText('recovered public reply');
  expect(screen.queryByRole('alert')).toBeNull();
});
