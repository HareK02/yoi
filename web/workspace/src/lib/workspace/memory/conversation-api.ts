import { CHAT_LIMITS } from "#lib/generated/chat-api.ts";
import type {
  ChatConversation,
  ChatCreateConversationRequest,
  ChatDelivery,
  ChatMessage,
  ChatMessagePage,
  ChatReceiveResponse,
  ChatUserMessageRequest,
} from "#lib/generated/chat-api.ts";
import { readBoundedJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import {
  type ConversationAttempt,
  conversationOperationId,
  type ConversationScope,
} from "./conversation-attempt.ts";

export class ConversationApiError extends Error {
  constructor(readonly status: number | null) {
    super(
      status === 401 || status === 403
        ? "You do not have access to this conversation."
        : status === 404
        ? "The Subject or conversation is no longer available."
        : status === 409
        ? "The saved operation conflicts with another input. Keep the original request; do not send it under a new ID."
        : "The request outcome could not be confirmed. Keep the saved input and retry the same request.",
    );
  }
}

function invalid(): never {
  throw new ConversationApiError(null);
}
function object(value: unknown, keys: string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    invalid();
  }
  const record = value as Record<string, unknown>;
  if (
    Object.keys(record).length !== keys.length ||
    keys.some((key) => !(key in record))
  ) invalid();
  return record;
}
function text(value: unknown, maxBytes = 512): string {
  if (
    typeof value !== "string" || !value.trim() ||
    new TextEncoder().encode(value).length > maxBytes
  ) invalid();
  return value;
}
function reference(value: unknown): string | null {
  return value === null ? null : text(value);
}

export function parseConversation(
  value: unknown,
  scope: ConversationScope,
): ChatConversation {
  const v = object(value, [
    "id",
    "subject_id",
    "participant_account_id",
    "last_user_message_id",
  ]);
  if (
    v.subject_id !== scope.subjectId ||
    v.participant_account_id !== scope.accountId
  ) invalid();
  return {
    id: text(v.id),
    subject_id: scope.subjectId,
    participant_account_id: scope.accountId,
    last_user_message_id: reference(v.last_user_message_id),
  };
}

export function parseConversationMessage(
  value: unknown,
  scope: ConversationScope,
  conversationId: string,
): ChatMessage {
  const v = object(value, [
    "id",
    "conversation_id",
    "sequence",
    "sender",
    "text",
    "reply_to",
    "work_ref",
    "operation_id",
    "last_user_message_id",
  ]);
  if (
    v.conversation_id !== conversationId || typeof v.sequence !== "number" ||
    !Number.isSafeInteger(v.sequence) || v.sequence < 1 ||
    v.sequence > CHAT_LIMITS.maxCounter
  ) invalid();
  if (typeof v.sender !== "object" || v.sender === null) invalid();
  const kind = (v.sender as Record<string, unknown>).kind;
  let sender: ChatMessage["sender"];
  if (kind === "user") {
    const s = object(v.sender, ["kind", "account_id"]);
    if (s.account_id !== scope.accountId) invalid();
    sender = { kind, account_id: scope.accountId };
  } else if (kind === "subject") {
    const s = object(v.sender, ["kind", "subject_id"]);
    if (s.subject_id !== scope.subjectId) invalid();
    sender = { kind, subject_id: scope.subjectId };
  } else invalid();
  return {
    id: text(v.id),
    conversation_id: conversationId,
    sequence: v.sequence,
    sender,
    text: text(v.text, CHAT_LIMITS.maxTextBytes),
    reply_to: reference(v.reply_to),
    work_ref: reference(v.work_ref),
    operation_id: text(v.operation_id),
    last_user_message_id: reference(v.last_user_message_id),
  };
}

export function parseConversationPage(
  value: unknown,
  scope: ConversationScope,
  conversationId: string,
  after: number,
): ChatMessagePage {
  const v = object(value, ["conversation", "messages", "has_more"]);
  const conversation = parseConversation(v.conversation, scope);
  if (
    conversation.id !== conversationId || !Array.isArray(v.messages) ||
    v.messages.length > CHAT_LIMITS.maxPageLimit ||
    typeof v.has_more !== "boolean"
  ) invalid();
  const messages = v.messages.map((message) =>
    parseConversationMessage(message, scope, conversationId)
  );
  let previous = after;
  const ids = new Set<string>();
  for (const message of messages) {
    if (message.sequence <= previous || ids.has(message.id)) invalid();
    previous = message.sequence;
    ids.add(message.id);
  }
  if (v.has_more && messages.length === 0) invalid();
  return { conversation, messages, has_more: v.has_more };
}

export function appendConversationMessages(
  saved: ChatMessage[],
  incoming: ChatMessage[],
): ChatMessage[] {
  let previous = saved.at(-1)?.sequence ?? 0;
  const ids = new Set(saved.map((message) => message.id));
  for (const message of incoming) {
    if (message.sequence <= previous || ids.has(message.id)) invalid();
    previous = message.sequence;
    ids.add(message.id);
  }
  return [...saved, ...incoming];
}

function parseDelivery(value: unknown): ChatDelivery {
  if (typeof value !== "object" || value === null) invalid();
  const state = (value as Record<string, unknown>).state;
  if (state === "no_current_body") {
    object(value, ["state"]);
    return { state };
  }
  if (state === "unavailable") {
    const v = object(value, ["state", "detail"]);
    return {
      state,
      detail: typeof v.detail === "string" ? v.detail : invalid(),
    };
  }
  if (state !== "accepted" && state !== "rejected" && state !== "unknown") {
    invalid();
  }
  const v = object(
    value,
    state === "accepted"
      ? ["state", "runtime_id", "worker_id"]
      : ["state", "runtime_id", "worker_id", "detail"],
  );
  const target = {
    runtime_id: text(v.runtime_id),
    worker_id: text(v.worker_id),
  };
  if (state === "accepted") return { state, ...target };
  return {
    state,
    ...target,
    detail: typeof v.detail === "string" ? v.detail : invalid(),
  };
}

async function request(
  fetchFn: typeof fetch,
  path: string,
  signal: AbortSignal,
  body?: unknown,
): Promise<unknown> {
  let response: Response;
  try {
    response = await fetchFn(path, {
      signal,
      credentials: "same-origin",
      cache: "no-store",
      ...(body === undefined ? {} : {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    });
  } catch {
    throw new ConversationApiError(null);
  }
  if (!response.ok) {
    await response.body?.cancel();
    throw new ConversationApiError(response.status);
  }
  try {
    return await readBoundedJson(response, 2 * 1024 * 1024);
  } catch {
    throw new ConversationApiError(null);
  }
}

function base(scope: ConversationScope): string {
  return workspaceApiPath(scope.workspaceId, "/chat/conversations");
}
export async function openSubjectConversation(
  fetchFn: typeof fetch,
  scope: ConversationScope,
  signal: AbortSignal,
): Promise<ChatConversation> {
  const body: ChatCreateConversationRequest = {
    operation_id: conversationOperationId(scope.subjectId),
    subject_id: scope.subjectId,
  };
  return parseConversation(
    await request(fetchFn, base(scope), signal, body),
    scope,
  );
}
export async function getConversationPage(
  fetchFn: typeof fetch,
  scope: ConversationScope,
  conversationId: string,
  after: number,
  signal: AbortSignal,
): Promise<ChatMessagePage> {
  const query = new URLSearchParams({
    after_sequence: String(after),
    limit: String(CHAT_LIMITS.maxPageLimit),
  });
  return parseConversationPage(
    await request(
      fetchFn,
      `${base(scope)}/${encodeURIComponent(conversationId)}/messages?${query}`,
      signal,
    ),
    scope,
    conversationId,
    after,
  );
}
export async function sendConversationInput(
  fetchFn: typeof fetch,
  scope: ConversationScope,
  attempt: ConversationAttempt,
  signal: AbortSignal,
): Promise<ChatReceiveResponse> {
  const body: ChatUserMessageRequest = {
    operation_id: attempt.operationId,
    text: attempt.text,
    reply_to: attempt.replyTo,
    work_ref: attempt.workRef,
  };
  const v = object(
    await request(
      fetchFn,
      `${base(scope)}/${encodeURIComponent(attempt.conversationId)}/messages`,
      signal,
      body,
    ),
    ["message", "delivery"],
  );
  const message = parseConversationMessage(
    v.message,
    scope,
    attempt.conversationId,
  );
  if (
    message.sender.kind !== "user" ||
    message.operation_id !== body.operation_id || message.text !== body.text ||
    message.reply_to !== body.reply_to || message.work_ref !== body.work_ref
  ) invalid();
  return { message, delivery: parseDelivery(v.delivery) };
}
