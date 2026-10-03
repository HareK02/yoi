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
  expect(onStatus).toHaveBeenLastCalledWith('closed', 'Workspace protocol frame rejected');
  expect(JSON.stringify(onStatus.mock.calls)).not.toContain('untrusted detail');

  vi.advanceTimersByTime(500);
  const second = FakeWebSocket.instances[1];
  expect(second).toBeDefined();
  second.open();
  expect(second.sent).toHaveLength(1);
  expect(JSON.parse(second.sent[0]).message.method).toBe('subscribe_events');
  multiplexer.dispose();
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
  expect(workersStatus).toHaveBeenLastCalledWith(
    'connecting',
    'resubscribe for a fresh snapshot',
  );
  expect(workdirsFrame).toHaveBeenCalledTimes(1);
  expect(workdirsStatus).not.toHaveBeenCalledWith(
    'connecting',
    'resubscribe for a fresh snapshot',
  );
  expect(socket.sent).toHaveLength(sentBeforeClosure + 1);
  expect(JSON.parse(socket.sent.at(-1)!).message.params.selector).toEqual({
    topic: 'workspace_workers',
  });
  expect(socket.closes).toEqual([]);
  multiplexer.dispose();
});
