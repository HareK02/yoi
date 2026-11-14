import type { MessagingCommand, MessagingResult } from '#lib/generated/chat-api.ts';

export type WorkspaceMessagingConnection = {
  request(command: MessagingCommand, signal: AbortSignal): Promise<MessagingResult>;
  close(): void;
};
