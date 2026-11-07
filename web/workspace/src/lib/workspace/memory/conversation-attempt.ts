// 会話の正本や配送queueではなく、同じtabでの明示再送に必要な全入力だけを保持する。
// 送信結果が不明な間は次の要求で上書きせず、本文の後続編集とは分離する。
export type ConversationScope = Readonly<{
  workspaceId: string;
  subjectId: string;
  accountId: string;
}>;

export type ConversationAttempt = Readonly<{
  conversationId: string;
  operationId: string;
  text: string;
  replyTo: string | null;
  workRef: string | null;
}>;

type AttemptStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function conversationOperationId(subjectId: string): string {
  // createのoperationはAccount/Workspaceに束縛される。再訪・別tabでも同じ会話を解決する。
  return `web-subject-conversation/v1/${subjectId}`;
}

export function conversationAttemptKey(scope: ConversationScope): string {
  return `yoi.conversation-attempt:${
    JSON.stringify([scope.workspaceId, scope.subjectId, scope.accountId])
  }`;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exact(value: Record<string, unknown>, keys: string[]): boolean {
  return Object.keys(value).length === keys.length &&
    keys.every((key) => key in value);
}

function id(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0 &&
    new TextEncoder().encode(value).length <= 512 &&
    !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function nullableId(value: unknown): value is string | null {
  return value === null || id(value);
}

export function readConversationAttempt(
  storage: Pick<AttemptStorage, "getItem">,
  scope: ConversationScope,
): ConversationAttempt | null {
  const raw = storage.getItem(conversationAttemptKey(scope));
  if (raw === null) return null;
  const value: unknown = JSON.parse(raw);
  if (
    !record(value) || !exact(value, ["version", "scope", "attempt"]) ||
    value.version !== 1 ||
    !record(value.scope) ||
    !exact(value.scope, ["workspaceId", "subjectId", "accountId"]) ||
    value.scope.workspaceId !== scope.workspaceId ||
    value.scope.subjectId !== scope.subjectId ||
    value.scope.accountId !== scope.accountId || !record(value.attempt)
  ) {
    throw new Error(
      "Saved send input cannot be read. Do not replace it with a new send.",
    );
  }
  const attempt = value.attempt;
  if (
    !exact(attempt, [
      "conversationId",
      "operationId",
      "text",
      "replyTo",
      "workRef",
    ]) ||
    !id(attempt.conversationId) || !id(attempt.operationId) ||
    typeof attempt.text !== "string" ||
    !attempt.text.trim() || !nullableId(attempt.replyTo) ||
    !nullableId(attempt.workRef)
  ) {
    throw new Error(
      "Saved send input is invalid. Do not replace it with a new send.",
    );
  }
  return Object.freeze({
    conversationId: attempt.conversationId,
    operationId: attempt.operationId,
    text: attempt.text,
    replyTo: attempt.replyTo,
    workRef: attempt.workRef,
  });
}

function sameAttempt(a: ConversationAttempt, b: ConversationAttempt): boolean {
  return a.conversationId === b.conversationId &&
    a.operationId === b.operationId &&
    a.text === b.text && a.replyTo === b.replyTo && a.workRef === b.workRef;
}

export function saveConversationAttempt(
  storage: AttemptStorage,
  scope: ConversationScope,
  attempt: ConversationAttempt,
): void {
  const saved = readConversationAttempt(storage, scope);
  if (saved !== null && !sameAttempt(saved, attempt)) {
    throw new Error("Resolve the saved send before starting another send.");
  }
  storage.setItem(
    conversationAttemptKey(scope),
    JSON.stringify({ version: 1, scope, attempt }),
  );
}

export function clearConversationAttempt(
  storage: AttemptStorage,
  scope: ConversationScope,
  expected: ConversationAttempt,
): void {
  const saved = readConversationAttempt(storage, scope);
  if (saved !== null && !sameAttempt(saved, expected)) {
    throw new Error("The saved send changed. It has not been cleared.");
  }
  storage.removeItem(conversationAttemptKey(scope));
}
