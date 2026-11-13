// Generated from subjektiv-api. Do not edit by hand.
// Regenerate: cargo run -q -p subjektiv-api --features typescript --example generate_chat_api_types > web/workspace/src/lib/generated/chat-api.ts

export const CHAT_LIMITS = { maxTextBytes: 16384, maxPageLimit: 100, maxCounter: 9007199254740991 } as const;

export type ChatConversation = { id: string, subject_id: string, participant_account_id: string,
/**
 * Latest user input in this conversation; empty conversations use null.
 */
last_user_message_id: string | null, };

export type ChatSendMessageRequest = { conversation_id: string, operation_id: string, text: string, reply_to: string | null, work_ref: string | null,
/**
 * Observed user input, independent of the optional reply target. Empty conversations use null.
 */
expected_last_user_message_id: string | null, };

export type ChatWorkerRequest = { operation: ChatWorkerOperation, };

export type ChatWorkerOperation = { "kind": "read", conversation_id: string, query: ChatMessagePageQuery, } | { "kind": "send", request: ChatSendMessageRequest, };

export type ChatWorkerResponse = { "kind": "messages", page: ChatMessagePage, } | { "kind": "sent", message: ChatMessage, };

export type MessagingSelector = { subject_id: string, conversation_id: string, after_sequence: number, };

export type MessagingCommand = { "kind": "open", request: ChatCreateConversationRequest, } | { "kind": "history", selector: MessagingSelector, query: ChatMessagePageQuery, } | { "kind": "post", selector: MessagingSelector, request: ChatUserMessageRequest, } | { "kind": "subscribe", selector: MessagingSelector, } | { "kind": "unsubscribe", subscription_id: string, };

export type MessagingRequest = { request_id: string, command: MessagingCommand, };

export type MessagingResult = { "kind": "opened", conversation: ChatConversation, } | { "kind": "history", page: ChatMessagePage, } | { "kind": "saved", response: ChatReceiveResponse, } | { "kind": "subscribed", subscription_id: string, selector: MessagingSelector, page: ChatMessagePage, } | { "kind": "unsubscribed", subscription_id: string, } | { "kind": "rejected", status: number, };

export type MessagingResponse = { request_id: string, result: MessagingResult, };

export type MessagingEvent = { "kind": "messages", subscription_id: string, page: ChatMessagePage, } | { "kind": "closed", subscription_id: string, reason: MessagingTermination, };

export type MessagingTermination = "access_revoked" | "lagged" | "server_shutdown";

export type ChatMessageSender = { "kind": "user", account_id: string, } | { "kind": "subject", subject_id: string, };

export type ChatMessage = { id: string, conversation_id: string, sequence: number, sender: ChatMessageSender, text: string, reply_to: string | null,
/**
 * Opaque related-work reference, not a grant or an attachment path.
 */
work_ref: string | null, operation_id: string,
/**
 * User messages refer to themselves; Subject messages pin the latest observed user input.
 */
last_user_message_id: string | null, };

export type ChatCreateConversationRequest = { operation_id: string, subject_id: string, };

export type ChatUserMessageRequest = { operation_id: string, text: string, reply_to: string | null, work_ref: string | null, };

export type ChatMessagePageQuery = { after_sequence: number | null, limit: number | null, };

export type ChatMessagePage = { conversation: ChatConversation, messages: Array<ChatMessage>, has_more: boolean, };

export type ChatDelivery = { "state": "no_current_body" } | { "state": "accepted", runtime_id: string, worker_id: string, } | { "state": "rejected", runtime_id: string, worker_id: string, detail: string, } | { "state": "unknown", runtime_id: string, worker_id: string, detail: string, } | { "state": "unavailable", detail: string, };

export type ChatReceiveResponse = { message: ChatMessage, delivery: ChatDelivery, };
// Semantic guards paired with the Rust-owned messaging DTO/schema. The multiplexer
// checks structural shape first; these checks bind pages, identities and byte limits.
// Rust str::trim uses Unicode White_Space, not ECMAScript TrimString (which removes FEFF).
export function chatHasText(value: string): boolean {
  return /[^\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/u.test(value);
}
function chatIdentifier(value: string): boolean {
  return typeof value === 'string' && chatHasText(value) && new TextEncoder().encode(value).length <= 512 && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}
function chatReference(value: string | null): boolean {
  return value === null || chatIdentifier(value);
}
export function validChatConversation(c: ChatConversation): boolean {
  return [c.id, c.subject_id, c.participant_account_id].every(chatIdentifier) && chatReference(c.last_user_message_id);
}
export function validChatMessage(m: ChatMessage): boolean {
  return [m.id, m.conversation_id, m.operation_id].every(chatIdentifier) &&
    chatHasText(m.text) && new TextEncoder().encode(m.text).length <= CHAT_LIMITS.maxTextBytes &&
    Number.isSafeInteger(m.sequence) && m.sequence >= 1 && m.sequence <= CHAT_LIMITS.maxCounter &&
    [m.reply_to, m.work_ref, m.last_user_message_id].every(chatReference) &&
    chatIdentifier(m.sender.kind === 'user' ? m.sender.account_id : m.sender.subject_id);
}
export function validChatPage(page: ChatMessagePage, after = 0): boolean {
  let previous = after;
  const ids = new Set<string>();
  return validChatConversation(page.conversation) && page.messages.length <= CHAT_LIMITS.maxPageLimit &&
    (!page.has_more || page.messages.length > 0) && page.messages.every(m => {
      const valid = validChatMessage(m) && m.conversation_id === page.conversation.id && m.sequence > previous && !ids.has(m.id) &&
        (m.sender.kind === 'user' ? m.sender.account_id === page.conversation.participant_account_id : m.sender.subject_id === page.conversation.subject_id);
      previous = m.sequence; ids.add(m.id); return valid;
    });
}
export function validMessagingSelector(s: MessagingSelector): boolean {
  return chatIdentifier(s.subject_id) && chatIdentifier(s.conversation_id) && Number.isSafeInteger(s.after_sequence) && s.after_sequence >= 0 && s.after_sequence <= CHAT_LIMITS.maxCounter;
}
export function messagingPageMatchesSelector(page: ChatMessagePage, s: MessagingSelector): boolean {
  return validMessagingSelector(s) && validChatPage(page, s.after_sequence) && page.conversation.id === s.conversation_id && page.conversation.subject_id === s.subject_id;
}
export function validMessagingResponse(response: MessagingResponse): boolean {
  if (!chatIdentifier(response.request_id)) return false;
  const r = response.result;
  switch (r.kind) {
    case 'opened': return validChatConversation(r.conversation);
    case 'history': return validChatPage(r.page);
    case 'saved': {
      const d = r.response.delivery;
      return validChatMessage(r.response.message) && (d.state === 'no_current_body' || d.state === 'unavailable' || (chatIdentifier(d.runtime_id) && chatIdentifier(d.worker_id)));
    }
    case 'subscribed': return chatIdentifier(r.subscription_id) && messagingPageMatchesSelector(r.page, r.selector);
    case 'unsubscribed': return chatIdentifier(r.subscription_id);
    case 'rejected': return Number.isInteger(r.status) && r.status >= 400 && r.status <= 599;
  }
}
export function validMessagingEvent(event: MessagingEvent): boolean {
  return chatIdentifier(event.subscription_id) && (event.kind === 'closed' || validChatPage(event.page));
}
