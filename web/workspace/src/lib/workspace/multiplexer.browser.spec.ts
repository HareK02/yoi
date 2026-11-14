// @vitest-environment happy-dom

import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { WorkspaceMultiplexer } from './multiplexer';

type SocketEvent = 'open' | 'message' | 'error' | 'close';
type SocketListener = (event: { data?: unknown }) => void;

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];

  readyState = FakeWebSocket.CONNECTING;
  readonly sent: string[] = [];
  readonly closes: Array<[number | undefined, string | undefined]> = [];
  readonly #listeners = new Map<SocketEvent, SocketListener[]>();

  constructor(readonly url: URL) {
    FakeWebSocket.instances.push(this);
  }

  addEventListener(kind: SocketEvent, listener: SocketListener): void {
    const listeners = this.#listeners.get(kind) ?? [];
    listeners.push(listener);
    this.#listeners.set(kind, listeners);
  }

  send(value: string): void {
    this.sent.push(value);
  }

  close(code?: number, reason?: string): void {
    if (
      code !== undefined &&
      code !== 1000 &&
      (code < 3000 || code > 4999)
    ) {
      throw new DOMException('Invalid WebSocket close code', 'InvalidAccessError');
    }
    this.closes.push([code, reason]);
    if (this.readyState === FakeWebSocket.CLOSED) return;
    this.readyState = FakeWebSocket.CLOSED;
    this.#emit('close');
  }

  open(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.#emit('open');
  }

  message(value: unknown): void {
    this.#emit('message', { data: value });
  }

  #emit(kind: SocketEvent, event: { data?: unknown } = {}): void {
    for (const listener of this.#listeners.get(kind) ?? []) listener(event);
  }
}

beforeEach(() => {
  FakeWebSocket.instances = [];
  vi.stubGlobal('WebSocket', FakeWebSocket);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test('invalid inbound frame closes before dispatch and reconnects for a fresh snapshot', () => {
  vi.useFakeTimers();
  const multiplexer = new WorkspaceMultiplexer('workspace-validation-test');
  const onFrame = vi.fn();
  const onStatus = vi.fn();
  multiplexer.subscribe({ topic: 'workspace_workers' }, { onFrame, onStatus });

  const first = FakeWebSocket.instances[0];
  expect(first).toBeDefined();
  first.open();
  expect(first.sent).toHaveLength(1);

  first.message(
    JSON.stringify({
      protocol_version: 2,
      frame: 'event',
      message: {
        event: 'subscription_closed',
        data: {
          subscription_id: 'subscription-1',
          code: 'lagged',
          message: 'untrusted detail',
          newer_field: true,
        },
      },
    }),
  );

  expect(onFrame).not.toHaveBeenCalled();
  expect(first.closes).toEqual([[4002, 'Invalid workspace protocol frame']]);
  expect(onStatus).toHaveBeenLastCalledWith('closed', {
    kind: 'protocol',
    message: 'Workspace protocol frame rejected',
  });
  expect(JSON.stringify(onStatus.mock.calls)).not.toContain('untrusted detail');

  vi.advanceTimersByTime(500);
  const second = FakeWebSocket.instances[1];
  expect(second).toBeDefined();
  second.open();
  expect(second.sent).toHaveLength(1);
  expect(JSON.parse(second.sent[0]).message.method).toBe('subscribe_events');

  first.open();
  first.message('{"protocol_version":2,"frame":"invalid"}');
  expect(second.sent).toHaveLength(1);
  expect(second.closes).toEqual([]);
  multiplexer.dispose();
});

test('normal disposal closes silently and never schedules a reconnect', () => {
  vi.useFakeTimers();
  const multiplexer = new WorkspaceMultiplexer('workspace-dispose-test');
  const onStatus = vi.fn();
  multiplexer.subscribe(
    { topic: 'workspace_workers' },
    { onFrame: vi.fn(), onStatus },
  );

  const socket = FakeWebSocket.instances[0];
  socket.open();
  onStatus.mockClear();
  multiplexer.dispose();

  expect(socket.closes).toEqual([[undefined, undefined]]);
  expect(onStatus).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1_000);
  expect(FakeWebSocket.instances).toHaveLength(1);
});

test('terminal subscription closure does not retry or report normal socket cleanup as transport failure', () => {
  vi.useFakeTimers();
  const multiplexer = new WorkspaceMultiplexer('workspace-terminal-test');
  const onFrame = vi.fn();
  const onStatus = vi.fn();
  multiplexer.subscribe(
    { topic: 'workspace_workers' },
    { onFrame, onStatus },
  );

  const socket = FakeWebSocket.instances[0];
  socket.open();
  const request = JSON.parse(socket.sent[0]);
  socket.message(JSON.stringify({
    protocol_version: 2,
    frame: 'response',
    message: {
      result: 'subscribed',
      payload: {
        request_id: request.message.params.request_id,
        subscription_id: 'subscription-terminal',
        selector: { topic: 'workspace_workers' },
        snapshot: { topic: 'workers', data: { workers: [] } },
      },
    },
  }));
  onStatus.mockClear();
  const sentBeforeClosure = socket.sent.length;

  socket.message(JSON.stringify({
    protocol_version: 2,
    frame: 'event',
    message: {
      event: 'subscription_closed',
      data: {
        subscription_id: 'subscription-terminal',
        code: 'unauthorized',
        message: 'Workspace Worker subscription is no longer authorized',
      },
    },
  }));

  expect(onFrame).toHaveBeenCalledTimes(2);
  expect(onStatus).not.toHaveBeenCalled();
  expect(socket.sent).toHaveLength(sentBeforeClosure);
  expect(socket.closes).toEqual([[undefined, undefined]]);
  vi.advanceTimersByTime(1_000);
  expect(FakeWebSocket.instances).toHaveLength(1);
});

test('valid subscription closure remains isolated and resubscribes only its selector', () => {
  const multiplexer = new WorkspaceMultiplexer('workspace-isolation-test');
  const workersFrame = vi.fn();
  const workersStatus = vi.fn();
  const workdirsFrame = vi.fn();
  const workdirsStatus = vi.fn();
  multiplexer.subscribe(
    { topic: 'workspace_workers' },
    { onFrame: workersFrame, onStatus: workersStatus },
  );
  multiplexer.subscribe(
    { topic: 'workspace_workdirs' },
    { onFrame: workdirsFrame, onStatus: workdirsStatus },
  );

  const socket = FakeWebSocket.instances[0];
  socket.open();
  const requests = socket.sent.map((value) => JSON.parse(value));
  expect(requests).toHaveLength(2);
  for (const [index, request] of requests.entries()) {
    if (request.frame !== 'request' || request.message.method !== 'subscribe_events') {
      throw new Error('expected subscribe request');
    }
    const selector = request.message.params.selector;
    const snapshot =
      selector.topic === 'workspace_workers'
        ? { topic: 'workers', data: { workers: [] } }
        : { topic: 'workspace_workdirs', data: { workdirs: [] } };
    socket.message(
      JSON.stringify({
        protocol_version: 2,
        frame: 'response',
        message: {
          result: 'subscribed',
          payload: {
            request_id: request.message.params.request_id,
            subscription_id: `subscription-${index + 1}`,
            selector,
            snapshot,
          },
        },
      }),
    );
  }

  const sentBeforeClosure = socket.sent.length;
  socket.message(
    JSON.stringify({
      protocol_version: 2,
      frame: 'event',
      message: {
        event: 'subscription_closed',
        data: {
          subscription_id: 'subscription-1',
          code: 'lagged',
          message: 'resubscribe for a fresh snapshot',
        },
      },
    }),
  );

  expect(workersFrame).toHaveBeenCalledTimes(2);
  expect(workersStatus).toHaveBeenLastCalledWith('connecting');
  expect(workdirsFrame).toHaveBeenCalledTimes(1);
  expect(workdirsStatus).not.toHaveBeenCalledWith('connecting');
  expect(socket.sent).toHaveLength(sentBeforeClosure + 1);
  expect(JSON.parse(socket.sent.at(-1)!).message.params.selector).toEqual({
    topic: 'workspace_workers',
  });
  expect(socket.closes).toEqual([]);
  multiplexer.dispose();
});

test('messaging and Worker subscriptions share one socket and terminate independently', async () => {
  const multiplexer = new WorkspaceMultiplexer('workspace-messaging');
  const worker = multiplexer.subscribe({ topic: 'workspace_workers' }, { onFrame: vi.fn() });
  const onEvent = vi.fn(); const onStatus = vi.fn();
  const chat = multiplexer.connectMessaging({ onEvent, onStatus });
  const socket = FakeWebSocket.instances[0]; socket.open();
  expect(FakeWebSocket.instances).toHaveLength(1);
  const selector = { subject_id: 's', conversation_id: 'c', after_sequence: 0 };
  const pending = chat.request({ kind: 'subscribe', selector }, new AbortController().signal);
  const request = JSON.parse(socket.sent.at(-1)!);
  socket.message(JSON.stringify({ protocol_version: 2, frame: 'messaging_response', message: { request_id: request.message.request_id, result: { kind: 'subscribed', subscription_id: 'chat-1', selector, page: { conversation: { id: 'c', subject_id: 's', participant_account_id: 'a', last_user_message_id: null }, messages: [], has_more: false } } } }));
  expect((await pending).kind).toBe('subscribed');
  worker.close(); expect(socket.closes).toEqual([]);
  socket.message(JSON.stringify({ protocol_version: 2, frame: 'messaging_event', message: { kind: 'closed', subscription_id: 'chat-1', reason: 'lagged' } }));
  expect(onEvent).toHaveBeenCalledWith({ kind: 'closed', subscription_id: 'chat-1', reason: 'lagged' });
  expect(socket.closes).toEqual([]);
  chat.close(); expect(socket.closes).toEqual([[undefined, undefined]]);
});

test('messaging-only disconnect reconnects and an aborted late subscribe is explicitly released', async () => {
  vi.useFakeTimers();
  const multiplexer = new WorkspaceMultiplexer('messaging-reconnect');
  const chat = multiplexer.connectMessaging({ onEvent: vi.fn(), onStatus: vi.fn() });
  const first = FakeWebSocket.instances[0]; first.open(); first.close();
  vi.advanceTimersByTime(500);
  const socket = FakeWebSocket.instances[1]; expect(socket).toBeDefined(); socket.open();
  const controller = new AbortController();
  const selector = { subject_id: 's', conversation_id: 'c', after_sequence: 0 };
  const pending = chat.request({ kind: 'subscribe', selector }, controller.signal);
  const request = JSON.parse(socket.sent.at(-1)!);
  controller.abort(); await expect(pending).rejects.toThrow('outcome unknown');
  socket.message(JSON.stringify({ protocol_version: 2, frame: 'messaging_response', message: { request_id: request.message.request_id, result: { kind: 'subscribed', subscription_id: 'orphan', selector, page: { conversation: { id: 'c', subject_id: 's', participant_account_id: 'a', last_user_message_id: null }, messages: [], has_more: false } } } }));
  expect(JSON.parse(socket.sent.at(-1)!).message.command).toEqual({ kind: 'unsubscribe', subscription_id: 'orphan' });
  chat.close();
});

test('a timed-out messaging subscribe cleans up its late ACK and a fresh subscribe remains live beside Workers', async () => {
  vi.useFakeTimers();
  const mux = new WorkspaceMultiplexer('workspace-messaging-timeout');
  const workerFrames = vi.fn();
  mux.subscribe({ topic: 'workspace_workers' }, { onFrame: workerFrames, onStatus: vi.fn() });
  const events = vi.fn();
  const lane = mux.connectMessaging({ onEvent: events, onStatus: vi.fn() });
  const socket = FakeWebSocket.instances[0]; socket.open();
  const selector = { subject_id: 's', conversation_id: 'c', after_sequence: 0 };
  const expired = lane.request({ kind: 'subscribe', selector }, new AbortController().signal);
  const outcome = expect(expired).rejects.toThrow('Messaging outcome unknown');
  const old = JSON.parse(socket.sent.at(-1)!);
  await vi.advanceTimersByTimeAsync(15001); await outcome;
  const page = { conversation: { id: 'c', subject_id: 's', participant_account_id: 'a', last_user_message_id: null }, messages: [], has_more: false };
  const subscribed = (request_id: string, subscription_id: string) => ({ protocol_version: 2, frame: 'messaging_response', message: { request_id, result: { kind: 'subscribed', selector, subscription_id, page } } });
  socket.message(JSON.stringify(subscribed(old.message.request_id, 'orphan')));
  expect(JSON.parse(socket.sent.at(-1)!).message.command).toEqual({ kind: 'unsubscribe', subscription_id: 'orphan' });
  const fresh = lane.request({ kind: 'subscribe', selector }, new AbortController().signal);
  const next = JSON.parse(socket.sent.at(-1)!);
  socket.message(JSON.stringify(subscribed(next.message.request_id, 'fresh'))); await fresh;
  socket.message(JSON.stringify({ protocol_version: 2, frame: 'messaging_event', message: { kind: 'messages', subscription_id: 'fresh', page } }));
  expect(events).toHaveBeenCalledOnce();
  expect(socket.closes).toEqual([]); expect(FakeWebSocket.instances).toHaveLength(1);
  const workers = JSON.parse(socket.sent[0]);
  socket.message(JSON.stringify({ protocol_version: 2, frame: 'response', message: { result: 'subscribed', payload: { request_id: workers.message.params.request_id, subscription_id: 'workers', selector: { topic: 'workspace_workers' }, snapshot: { topic: 'workers', data: { workers: [] } } } } }));
  expect(workerFrames).toHaveBeenCalledOnce();
  mux.dispose();
});
