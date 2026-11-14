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

Deno.test('explicit refresh recovers failed Open and Subscribe without a socket reconnect or a retry loop', async () => {
  for (const failure of ['open-timeout', 'subscribe-timeout', 'open-rejected', 'subscribe-rejected']) {
    const f = fixture();
    let fail = true;
    f.handler = async command => {
      if (fail && command.kind === (failure.startsWith('open') ? 'open' : 'subscribe')) {
        if (failure.endsWith('timeout')) throw new Error('ACK outcome unknown');
        return { kind: 'rejected', status: 503 };
      }
      if (command.kind === 'open') return { kind: 'opened', conversation };
      if (command.kind === 'subscribe') return { kind: 'subscribed', subscription_id: 'recovered', selector: command.selector, page: page([message(3)]) };
      throw new Error('must reinstall subscription, not just retrieve history');
    };
    f.listener.onStatus('open'); await flush();
    assertEquals(f.errors.length, 1);
    const count = f.commands.length; await flush(); assertEquals(f.commands.length, count);
    fail = false; await f.client.refresh();
    assertEquals(f.commands.at(-1)?.kind, 'subscribe');
    f.listener.onEvent({ kind: 'messages', subscription_id: 'recovered', page: page([message(9)]) });
    assertEquals(f.pages.at(-1)?.map(m => m.sequence), [3, 9]);
    f.client.close();
  }
});

Deno.test('abort during explicit recovery cannot revive a detached subscription', async () => {
  const f = fixture(); f.handler = async () => { throw new Error('open timeout'); };
  f.listener.onStatus('open'); await flush();
  let release!: (result: MessagingResult) => void;
  f.handler = () => new Promise(resolve => { release = resolve; });
  const recovery = f.client.refresh(); await flush(); f.controller.abort();
  release({ kind: 'opened', conversation }); await recovery;
  assertEquals(f.commands.map(c => c.kind), ['open', 'open']);
  assertEquals(f.pages, []); assertEquals(f.closed, 1);
});

import whitespace from '../../../../../../crates/subjektiv-api/fixtures/chat-whitespace.json' with { type: 'json' };
import { chatHasText } from '#lib/generated/chat-api.ts';
import { decodeSubscriptionFrame } from '#lib/generated/protocol-validator.ts';
import { parseConversationMessage } from './conversation-api.ts';
import { saveConversationAttempt, readConversationAttempt } from './conversation-attempt.ts';
Deno.test('the shared Unicode corpus passes Rust-compatible frame, conversation parser and retry storage admission', () => {
  for (const { text, nonblank } of whitespace) {
    assertEquals(chatHasText(text), nonblank);
    const saved = { ...message(1), text };
    const frame = { protocol_version: 2, frame: 'messaging_event', message: { kind: 'messages', subscription_id: 'sub', page: page([saved]) } };
    assertEquals(decodeSubscriptionFrame(JSON.stringify(frame)).ok, nonblank);
    if (!nonblank) continue;
    assertEquals(parseConversationMessage(saved, scope, 'c').text, text);
    const entries = new Map<string, string>();
    const storage = { getItem: (key: string) => entries.get(key) ?? null, setItem: (key: string, value: string) => { entries.set(key, value); }, removeItem: (key: string) => { entries.delete(key); } };
    const attempt = { conversationId: 'c', operationId: 'unicode', text, replyTo: null, workRef: null };
    saveConversationAttempt(storage, scope, attempt);
    assertEquals(readConversationAttempt(storage, scope), attempt);
  }
});
