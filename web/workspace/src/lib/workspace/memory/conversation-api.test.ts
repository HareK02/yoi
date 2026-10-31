// HTTP境界のfixture試験。実Backend/Runtime接続の証拠ではない。
import {
  appendConversationMessages,
  ConversationApiError,
  getConversationPage,
  openSubjectConversation,
  parseConversationMessage,
  parseConversationPage,
  sendConversationInput,
} from "./conversation-api.ts";
import { CHAT_LIMITS } from "#lib/generated/chat-api.ts";
import {
  readConversationAttempt,
  saveConversationAttempt,
} from "./conversation-attempt.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};
function assert(value: unknown): asserts value {
  if (!value) throw new Error("Assertion failed");
}
function rejects(fn: () => unknown) {
  let failed = false;
  try {
    fn();
  } catch {
    failed = true;
  }
  assert(failed);
}
const scope = {
  workspaceId: "w/1",
  subjectId: "subject",
  accountId: "account",
};
const conversation = {
  id: "c/1",
  subject_id: scope.subjectId,
  participant_account_id: scope.accountId,
  last_user_message_id: null,
};
const attempt = {
  conversationId: conversation.id,
  operationId: "send-id",
  text: "  exact\n本文  ",
  replyTo: null,
  workRef: null,
};
function message(sequence = 1) {
  return {
    id: `m${sequence}`,
    conversation_id: conversation.id,
    sequence,
    sender: { kind: "user", account_id: scope.accountId },
    text: attempt.text,
    reply_to: null,
    work_ref: null,
    operation_id: attempt.operationId,
    last_user_message_id: `m${sequence}`,
  };
}
function page(messages: unknown[], has_more = false) {
  return { conversation, messages, has_more };
}

Deno.test("public conversation boundary rejects wrong identity internal senders and unsafe cursor/order", () => {
  for (
    const invalid of [
      { ...message(), conversation_id: "other" },
      { ...message(), sender: { kind: "assistant" } },
      { ...message(), sender: { kind: "tool" } },
      { ...message(), sender: { kind: "subject", subject_id: "other" } },
      { ...message(), sender: { kind: "user", account_id: "other" } },
      { ...message(), sequence: Number.MAX_SAFE_INTEGER + 1 },
      { ...message(), sequence: 0 },
      { ...message(), attachments: [] },
    ]
  ) rejects(() => parseConversationMessage(invalid, scope, conversation.id));
  for (
    const invalid of [
      page([message(2), message(1)]),
      page([message(1), message(1)]),
      page([], true),
      { ...page([]), conversation: { ...conversation, subject_id: "other" } },
      {
        ...page([]),
        conversation: { ...conversation, participant_account_id: "other" },
      },
    ]
  ) rejects(() => parseConversationPage(invalid, scope, conversation.id, 0));
  rejects(() =>
    parseConversationPage(page([message(1)]), scope, conversation.id, 1)
  );
});

Deno.test("exclusive sequence pages append without duplicating an earlier public message", () => {
  const first = parseConversationPage(
    page([message(1), message(2)], true),
    scope,
    conversation.id,
    0,
  );
  const second = parseConversationPage(
    page([message(3)]),
    scope,
    conversation.id,
    2,
  );
  const merged = appendConversationMessages(first.messages, second.messages);
  assert(merged.map((m) => m.sequence).join() === "1,2,3");
  rejects(() => appendConversationMessages(merged, [merged[0]]));
  const repeat = { ...second.messages[0], sequence: 4 };
  rejects(() => appendConversationMessages(merged, [repeat]));
});

Deno.test("revisit uses the same account-scoped create input then reads authoritative pages", async () => {
  const requests: { path: string; body: unknown }[] = [];
  const fetchFn: typeof fetch = (input, init) => {
    requests.push({
      path: String(input),
      body: init?.body ? JSON.parse(String(init.body)) : null,
    });
    return Promise.resolve(
      Response.json(
        init?.method === "POST" ? conversation : page([message(1)]),
      ),
    );
  };
  const signal = new AbortController().signal;
  const first = await openSubjectConversation(fetchFn, scope, signal);
  const revisit = await openSubjectConversation(fetchFn, scope, signal);
  assert(first.id === revisit.id);
  assert(JSON.stringify(requests[0]) === JSON.stringify(requests[1]));
  await getConversationPage(fetchFn, scope, first.id, 1, signal);
  assert(
    requests[2].path ===
      `/api/w/w%2F1/chat/conversations/c%2F1/messages?after_sequence=1&limit=${CHAT_LIMITS.maxPageLimit}`,
  );
});

Deno.test("ACK loss retains exact request bytes for explicit replay and accepted is not a reply", async () => {
  const saved = new Map<string, string>();
  const storage = {
    getItem: (k: string) => saved.get(k) ?? null,
    setItem: (k: string, v: string) => {
      saved.set(k, v);
    },
    removeItem: (k: string) => {
      saved.delete(k);
    },
  };
  const bodies: string[] = [];
  const fetchFn: typeof fetch = (_input, init) => {
    bodies.push(String(init?.body));
    return bodies.length === 1
      ? Promise.reject(new TypeError("ACK lost"))
      : Promise.resolve(
        Response.json({
          message: message(),
          delivery: { state: "accepted", runtime_id: "r", worker_id: "w" },
        }),
      );
  };
  const signal = new AbortController().signal;
  saveConversationAttempt(storage, scope, attempt);
  try {
    await sendConversationInput(fetchFn, scope, attempt, signal);
    throw new Error("Expected unknown");
  } catch (error) {
    assert(error instanceof ConversationApiError && error.status === null);
  }
  const retained = readConversationAttempt(storage, scope);
  assert(retained !== null);
  const receipt = await sendConversationInput(fetchFn, scope, retained, signal);
  assert(bodies[0] === bodies[1]);
  assert(receipt.message.sender.kind === "user");
  assert(receipt.delivery.state === "accepted");
});

Deno.test("HTTP rejection and saved-message delivery failure never become an accepted reply", async () => {
  const signal = new AbortController().signal;
  for (const status of [401, 403, 404, 409, 500]) {
    const fetchFn: typeof fetch = () =>
      Promise.resolve(new Response(null, { status }));
    try {
      await sendConversationInput(fetchFn, scope, attempt, signal);
      throw new Error("Expected rejection");
    } catch (error) {
      assert(error instanceof ConversationApiError && error.status === status);
    }
  }
  for (
    const delivery of [
      { state: "no_current_body" },
      { state: "rejected", runtime_id: "r", worker_id: "w", detail: "stopped" },
      { state: "unknown", runtime_id: "r", worker_id: "w", detail: "lost" },
      { state: "unavailable", detail: "unavailable" },
    ]
  ) {
    const fetchFn: typeof fetch = () =>
      Promise.resolve(Response.json({ message: message(), delivery }));
    const result = await sendConversationInput(fetchFn, scope, attempt, signal);
    assert(result.delivery.state === delivery.state);
    assert(result.message.sender.kind === "user");
  }
});
