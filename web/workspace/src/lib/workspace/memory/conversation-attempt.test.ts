import {
  clearConversationAttempt,
  type ConversationAttempt,
  conversationAttemptKey,
  conversationOperationId,
  type ConversationScope,
  readConversationAttempt,
  saveConversationAttempt,
} from "./conversation-attempt.ts";

declare const Deno: { test(name: string, fn: () => void): void };
function assert(condition: unknown): asserts condition {
  if (!condition) throw new Error("Assertion failed");
}
function rejects(fn: () => void): void {
  let threw = false;
  try {
    fn();
  } catch {
    threw = true;
  }
  assert(threw);
}
function storage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
    removeItem: (key: string) => {
      values.delete(key);
    },
  };
}
const scope: ConversationScope = {
  workspaceId: "workspace",
  subjectId: "subject",
  accountId: "account",
};
const attempt: ConversationAttempt = {
  conversationId: "conversation",
  operationId: "operation",
  text: "  original\n本文  ",
  replyTo: null,
  workRef: null,
};

Deno.test("reload and explicit retry preserve the entire original send, not the edited draft", () => {
  const s = storage();
  saveConversationAttempt(s, scope, attempt);
  const restored = readConversationAttempt(s, scope);
  assert(JSON.stringify(restored) === JSON.stringify(attempt));
  assert(Object.isFrozen(restored));
  saveConversationAttempt(s, scope, { ...attempt });
  assert(readConversationAttempt(s, scope)?.text === "  original\n本文  ");
  rejects(() =>
    saveConversationAttempt(s, scope, { ...attempt, text: "edited" })
  );
  assert(readConversationAttempt(s, scope)?.text === attempt.text);
});

Deno.test("unresolved input cannot be overwritten by a new ID or changed reply/work/conversation", () => {
  const s = storage();
  saveConversationAttempt(s, scope, attempt);
  for (
    const changed of [
      { ...attempt, operationId: "new" },
      { ...attempt, conversationId: "other" },
      { ...attempt, replyTo: "reply" },
      { ...attempt, workRef: "work" },
    ]
  ) {
    rejects(() => saveConversationAttempt(s, scope, changed));
    rejects(() => clearConversationAttempt(s, scope, changed));
    assert(
      readConversationAttempt(s, scope)?.operationId === attempt.operationId,
    );
  }
  clearConversationAttempt(s, scope, { ...attempt });
  assert(readConversationAttempt(s, scope) === null);
});

Deno.test("Workspace Subject and authenticated Account have separate retained input", () => {
  const s = storage();
  saveConversationAttempt(s, scope, attempt);
  for (
    const other of [
      { ...scope, workspaceId: "other" },
      { ...scope, subjectId: "other" },
      { ...scope, accountId: "other" },
    ]
  ) {
    assert(readConversationAttempt(s, other) === null);
    // 区切り文字を含む識別子でも別scopeに衝突しない。
    assert(conversationAttemptKey(other) !== conversationAttemptKey(scope));
  }
  assert(
    conversationAttemptKey({
      workspaceId: "a:b",
      subjectId: "c",
      accountId: "d",
    }) !==
      conversationAttemptKey({
        workspaceId: "a",
        subjectId: "b:c",
        accountId: "d",
      }),
  );
  assert(
    conversationOperationId(scope.subjectId) ===
      conversationOperationId(scope.subjectId),
  );
  assert(
    conversationOperationId("other") !==
      conversationOperationId(scope.subjectId),
  );
});

Deno.test("corrupt or cross-scope retained input fails closed without replacement or clearing", () => {
  const s = storage();
  const key = conversationAttemptKey(scope);
  for (
    const bad of [
      "not json",
      JSON.stringify({
        version: 1,
        scope: { ...scope, accountId: "other" },
        attempt,
      }),
      JSON.stringify({
        version: 1,
        scope,
        attempt: { ...attempt, operationId: "" },
      }),
      JSON.stringify({
        version: 1,
        scope,
        attempt: { ...attempt, replyTo: undefined },
      }),
      JSON.stringify({
        version: 1,
        scope,
        attempt: { ...attempt, attachments: [] },
      }),
    ]
  ) {
    s.setItem(key, bad);
    rejects(() => readConversationAttempt(s, scope));
    rejects(() => saveConversationAttempt(s, scope, attempt));
    rejects(() => clearConversationAttempt(s, scope, attempt));
    assert(s.getItem(key) === bad);
  }
});

Deno.test("storage refusal is not a retained send or successful clear", () => {
  const s = storage();
  rejects(() =>
    saveConversationAttempt(
      {
        ...s,
        setItem: () => {
          throw new Error("quota");
        },
      },
      scope,
      attempt,
    )
  );
  assert(readConversationAttempt(s, scope) === null);
  saveConversationAttempt(s, scope, attempt);
  rejects(() =>
    clearConversationAttempt(
      {
        ...s,
        removeItem: () => {
          throw new Error("blocked");
        },
      },
      scope,
      attempt,
    )
  );
  assert(
    readConversationAttempt(s, scope)?.operationId === attempt.operationId,
  );
  rejects(() =>
    readConversationAttempt({
      getItem: () => {
        throw new Error("blocked");
      },
    }, scope)
  );
});
