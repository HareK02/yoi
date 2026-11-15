import type { ConversationScope } from "./conversation-attempt.ts";

export function conversationDraftKey(scope: ConversationScope, conversationId: string): string {
  return `yoi.conversation-draft:${
    JSON.stringify([scope.accountId, scope.workspaceId, scope.subjectId, conversationId])
  }`;
}

export function readConversationDraft(storage: Pick<Storage, "getItem">, key: string): string {
  const raw = storage.getItem(key);
  if (raw === null) return "";
  const value: unknown = JSON.parse(raw);
  if (typeof value !== "string") throw new Error("Invalid conversation draft");
  return value;
}

export function saveConversationDraft(
  storage: Pick<Storage, "setItem" | "removeItem">,
  key: string,
  text: string,
): void {
  if (text === "") storage.removeItem(key);
  else storage.setItem(key, JSON.stringify(text));
}
