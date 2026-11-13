declare const Deno: { test(name: string, fn: () => void | Promise<void>): void };
import { deepStrictEqual as assertEquals, rejects as assertRejects } from 'node:assert/strict';
import type { ChatMessage, ChatMessagePage, MessagingCommand, MessagingEvent, MessagingResult } from '#lib/generated/chat-api.ts';
import { ConversationMessagingClient } from './conversation-messaging.ts';

const scope = { workspaceId: 'w', subjectId: 's', accountId: 'a' };
const conversation = { id: 'c', subject_id: 's', participant_account_id: 'a', last_user_message_id: null };
function message(sequence: number): ChatMessage {
  return { id: `m-${sequence}`, conversation_id: 'c', sequence, sender: { kind: 'subject', subject_id: 's' }, text: `reply ${sequence}`, reply_to: null, work_ref: null, operation_id: `op-${sequence}`, last_user_message_id: null };
}
function page(messages: ChatMessage[] = []): ChatMessagePage { return { conversation, messages, has_more: false }; }
async function flush() { for (let n = 0; n < 12; n++) await Promise.resolve(); }
function fixture() {
  let listener!: { onEvent(event: MessagingEvent): void; onStatus(status: 'connecting' | 'open' | 'closed'): void };
  const commands: MessagingCommand[] = [];
  const pages: ChatMessage[][] = [];
  const errors: number[] = [];
  let closed = 0;
  let handler = async (command: MessagingCommand): Promise<MessagingResult> => {
    if (command.kind === 'open') return { kind: 'opened', conversation };
    if (command.kind === 'subscribe') return { kind: 'subscribed', subscription_id: 'sub', selector: command.selector, page: page() };
    throw new Error('unexpected command');
  };
  const controller = new AbortController();
  const client = new ConversationMessagingClient(scope, controller.signal, {
    onPage: (_, restored) => pages.push(restored), onStatus: () => {}, onError: e => errors.push(e.status ?? 0),
  }, l => {
    listener = l;
    return { request: command => { commands.push(structuredClone(command)); return handler(command); }, close: () => { closed++; } };
  });
  return { client, controller, commands, pages, errors, get closed() { return closed; }, get listener() { return listener; }, set handler(value: typeof handler) { handler = value; } };
}
Deno.test('messaging restores sparse saved sequences, deduplicates repeated pages and resumes from the last stored page', async () => {
  const f = fixture(); f.listener.onStatus('open'); await flush();
  f.listener.onEvent({ kind: 'messages', subscription_id: 'sub', page: page([message(3), message(9)]) });
  f.listener.onEvent({ kind: 'messages', subscription_id: 'sub', page: page([message(3), message(9)]) });
  assertEquals(f.pages.at(-1)?.map(m => m.sequence), [3, 9]);
  f.listener.onStatus('closed'); f.listener.onStatus('open'); await flush();
  assertEquals(f.commands.at(-1), { kind: 'subscribe', selector: { subject_id: 's', conversation_id: 'c', after_sequence: 9 } });
  f.listener.onEvent({ kind: 'closed', subscription_id: 'sub', reason: 'lagged' }); await flush();
  assertEquals(f.commands.at(-1)?.kind, 'subscribe');
  assertEquals(f.errors, []); f.client.close();
});
Deno.test('saved ACK never skips an unobserved reply and post retry retains the complete request', async () => {
  const f = fixture(); f.listener.onStatus('open'); await flush();
  const attempt = { conversationId: 'c', operationId: 'retry-op', text: 'same input', replyTo: null, workRef: null };
  const saved = { ...message(10), sender: { kind: 'user' as const, account_id: 'a' }, operation_id: attempt.operationId, text: attempt.text };
  f.handler = async command => {
    if (command.kind === 'post') return { kind: 'saved', response: { message: saved, delivery: { state: 'no_current_body' } } };
    throw new Error('not subscribed');
  };
  await f.client.post(attempt); await f.client.post(attempt);
  assertEquals(f.commands.at(-1), f.commands.at(-2));
  assertEquals(f.pages.at(-1), []);
  f.listener.onEvent({ kind: 'messages', subscription_id: 'sub', page: page([message(7), saved]) });
  assertEquals(f.pages.at(-1)?.map(m => m.sequence), [7, 10]); f.client.close();
});
Deno.test('a late subscription result after abort cannot adopt another Account history', async () => {
  const f = fixture(); let release!: (result: MessagingResult) => void;
  f.handler = command => command.kind === 'open' ? Promise.resolve({ kind: 'opened', conversation }) : new Promise(resolve => { release = resolve; });
  f.listener.onStatus('open'); await flush(); f.controller.abort();
  release({ kind: 'subscribed', subscription_id: 'sub', selector: { subject_id: 's', conversation_id: 'c', after_sequence: 0 }, page: page([message(1)]) }); await flush();
  assertEquals(f.pages, []); assertEquals(f.closed, 1);
  await assertRejects(() => f.client.post({ conversationId: 'c', operationId: 'op', text: 'text', replyTo: null, workRef: null }));
});
Deno.test('Subject or Account mismatch and revoked access withdraw only this conversation', async () => {
  for (const kind of ['subject', 'account', 'revoked']) {
    const f = fixture(); f.listener.onStatus('open'); await flush();
    if (kind === 'revoked') f.listener.onEvent({ kind: 'closed', subscription_id: 'sub', reason: 'access_revoked' });
    else f.listener.onEvent({ kind: 'messages', subscription_id: 'sub', page: { ...page([message(1)]), conversation: { ...conversation, ...(kind === 'subject' ? { subject_id: 'other' } : { participant_account_id: 'other' }) } } });
    assertEquals(f.pages.at(-1), []); assertEquals(f.errors.length, 1); f.client.close();
  }
});
