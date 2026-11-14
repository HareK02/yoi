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
