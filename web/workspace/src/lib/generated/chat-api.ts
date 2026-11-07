// Generated from server-api. Do not edit by hand.
// Regenerate: cargo run -q -p server-api --features typescript --example generate_chat_api_types > web/workspace/src/lib/generated/chat-api.ts

export const CHAT_LIMITS = {
  maxTextBytes: 16384,
  maxPageLimit: 100,
  maxCounter: 9007199254740991,
} as const;

export type ChatConversation = {
  id: string;
  subject_id: string;
  participant_account_id: string;
  /**
   * Latest user input in this conversation; empty conversations use null.
   */
  last_user_message_id: string | null;
};

export type ChatMessageSender = { "kind": "user"; account_id: string } | {
  "kind": "subject";
  subject_id: string;
};

export type ChatMessage = {
  id: string;
  conversation_id: string;
  sequence: number;
  sender: ChatMessageSender;
  text: string;
  reply_to: string | null;
  /**
   * Opaque related-work reference, not a grant or an attachment path.
   */
  work_ref: string | null;
  operation_id: string;
  /**
   * User messages refer to themselves; Subject messages pin the latest observed user input.
   */
  last_user_message_id: string | null;
};

export type ChatCreateConversationRequest = {
  operation_id: string;
  subject_id: string;
};

export type ChatUserMessageRequest = {
  operation_id: string;
  text: string;
  reply_to: string | null;
  work_ref: string | null;
};

export type ChatMessagePageQuery = {
  after_sequence: number | null;
  limit: number | null;
};

export type ChatMessagePage = {
  conversation: ChatConversation;
  messages: Array<ChatMessage>;
  has_more: boolean;
};

export type ChatDelivery = { "state": "no_current_body" } | {
  "state": "accepted";
  runtime_id: string;
  worker_id: string;
} | {
  "state": "rejected";
  runtime_id: string;
  worker_id: string;
  detail: string;
} | {
  "state": "unknown";
  runtime_id: string;
  worker_id: string;
  detail: string;
} | { "state": "unavailable"; detail: string };

export type ChatReceiveResponse = {
  message: ChatMessage;
  delivery: ChatDelivery;
};
