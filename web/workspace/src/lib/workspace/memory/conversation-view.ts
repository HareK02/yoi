import type { ChatMessage } from "#lib/generated/chat-api.ts";

export type ConversationMessageView = {
  id: string;
  text: string;
  own: boolean;
  author: string;
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
