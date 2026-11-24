import type { ChatMessage } from "#lib/generated/chat-api.ts";
import type { ConversationAttempt } from "./conversation-attempt.ts";

export type ConversationOutgoing = {
  request: ConversationAttempt;
  state: "sending" | "delivered" | "retry";
};

export type ConversationMessageView = {
  id: string;
  text: string;
  own: boolean;
  author: string;
  local?: boolean;
  delivery?: { operationId: string; state: ConversationOutgoing["state"] };
};

export function conversationMessageView(
  message: ChatMessage,
  accountId: string,
  subjectName: string,
): ConversationMessageView {
  const own = message.sender.kind === "user" && message.sender.account_id === accountId;
  return {
    id: message.id,
    text: message.text,
    own,
    author: own ? "You" : message.sender.kind === "subject" ? subjectName : "Participant",
  };
}

// ローカル送信は保存履歴へ混ぜない。配送観測を同じ会話・本人・operationへ結び、
// pageとPOST応答の到着順にかかわらず保存済みバブルを一つだけ表示する。
export function conversationHistoryView(
  messages: ChatMessage[],
  accountId: string,
  subjectName: string,
  outgoing: ConversationOutgoing[],
): ConversationMessageView[] {
  const unmatched = new Set(outgoing);
  const rows = messages.map(message => {
    const row = conversationMessageView(message, accountId, subjectName);
    const sent = row.own ? outgoing.find(({ request }) =>
      request.conversationId === message.conversation_id && request.operationId === message.operation_id
    ) : undefined;
    if (sent) {
      row.delivery = { operationId: sent.request.operationId, state: sent.state };
      unmatched.delete(sent);
    }
    return row;
  });
  for (const { request, state } of unmatched) {
    rows.push({
      id: `local:${request.operationId}`,
      text: request.text,
      own: true,
      author: "You",
      local: true,
      delivery: { operationId: request.operationId, state },
    });
  }
  const latestOwn = rows.findLast(row => row.own);
  const repliedTo = new Set(messages.filter(message => message.sender.kind === 'subject')
    .flatMap(message => [message.last_user_message_id, message.reply_to]));
  for (const row of rows) {
    if (row.delivery?.state === 'delivered' && (row !== latestOwn || (!row.local && repliedTo.has(row.id)))) {
      delete row.delivery;
    }
  }
  return rows;
}
