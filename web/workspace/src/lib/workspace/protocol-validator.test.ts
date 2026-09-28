import type { EventSubscriptionSelector, SubscriptionFrame } from '$lib/generated/protocol';
import {
  decodeSubscriptionFrame,
  subscriptionFrameMatchesSelector,
} from '$lib/generated/protocol-validator';

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function subscribedFrame(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    protocol_version: 1,
    frame: 'response',
    message: {
      result: 'subscribed',
      payload: {
        request_id: 'request-1',
        subscription_id: 'subscription-1',
        selector: { topic: 'workspace_workers' },
        snapshot_revision: 1,
        snapshot: { topic: 'workers', data: { workers: [] } },
        ...overrides,
      },
    },
  };
}

function decode(value: unknown) {
  return decodeSubscriptionFrame(JSON.stringify(value));
}

Deno.test('generated subscription validator accepts a current exact frame', () => {
  const result = decode(subscribedFrame());
  assert(result.ok, `expected valid frame, got ${JSON.stringify(result)}`);
  assert(result.value.frame === 'response', 'expected response frame');
});

Deno.test('generated subscription validator rejects malformed and newer shapes', () => {
  const cases: Array<[string, unknown]> = [
    ['malformed json', '{'],
    ['invalid discriminator', { ...subscribedFrame(), frame: 'future' }],
    ['missing required field', { frame: 'response', message: subscribedFrame().message }],
    ['unknown root field', { ...subscribedFrame(), secret: 'must-not-reflect' }],
    [
      'unknown nested field',
      subscribedFrame({ snapshot: { topic: 'workers', data: { workers: [], future: true } } }),
    ],
    ['wrong field type', subscribedFrame({ snapshot_revision: '1' })],
    ['unsafe integer', subscribedFrame({ snapshot_revision: Number.MAX_SAFE_INTEGER + 1 })],
    ['unsupported version', { ...subscribedFrame(), protocol_version: 2 }],
    [
      'newer Worker event',
      {
        protocol_version: 1,
        frame: 'event',
        message: {
          event: 'event',
          data: {
            subscription_id: 'subscription-1',
            subject_revision: 2,
            payload: {
              event: 'worker_protocol',
              data: { worker_id: 'worker-1', event: { event: 'future_event' } },
            },
          },
        },
      },
    ],
  ];

  for (const [name, value] of cases) {
    const result = typeof value === 'string' ? decodeSubscriptionFrame(value) : decode(value);
    assert(!result.ok, `${name} unexpectedly passed`);
    assert(!JSON.stringify(result).includes('must-not-reflect'), `${name} reflected frame content`);
  }
});

Deno.test('generated subscription validator enforces Rust semantic bounds and selector pairing', () => {
  const oversizedIds = Array.from({ length: 257 }, (_, index) => `worker-${index}`);
  const cases: Array<[string, unknown]> = [
    [
      'oversized correlation id',
      subscribedFrame({ subscription_id: 'x'.repeat(129) }),
    ],
    [
      'oversized rejection message',
      {
        protocol_version: 1,
        frame: 'response',
        message: {
          result: 'subscription_rejected',
          payload: {
            request_id: 'request-1',
            code: 'invalid_request',
            message: 'x'.repeat(1025),
          },
        },
      },
    ],
    [
      'oversized selector collection',
      subscribedFrame({
        selector: { topic: 'worker_lifecycle', worker_ids: oversizedIds },
      }),
    ],
    [
      'selector snapshot mismatch',
      subscribedFrame({
        selector: { topic: 'worker_protocol', worker_id: 'worker-1' },
      }),
    ],
    [
      'duplicate snapshot worker',
      subscribedFrame({
        snapshot: {
          topic: 'workers',
          data: {
            workers: [
              {
                worker_id: 'worker-1',
                runtime_id: 'runtime-1',
                availability: 'observed',
                subject_revision: 1,
                state: 'idle',
                has_running_internal_workers: false,
              },
              {
                worker_id: 'worker-1',
                runtime_id: 'runtime-1',
                availability: 'observed',
                subject_revision: 2,
                state: 'running',
                has_running_internal_workers: false,
              },
            ],
          },
        },
      }),
    ],
    [
      'server-to-Browser direction mismatch',
      {
        protocol_version: 1,
        frame: 'request',
        message: {
          method: 'unsubscribe_events',
          params: { request_id: 'request-1', subscription_id: 'subscription-1' },
        },
      },
    ],
  ];

  for (const [name, value] of cases) {
    const result = decode(value);
    assert(!result.ok, `${name} unexpectedly passed`);
  }
});

Deno.test('stateful selector validation fences a routed event before projection', () => {
  const frame: SubscriptionFrame = {
    protocol_version: 1,
    frame: 'event',
    message: {
      event: 'event',
      data: {
        subscription_id: 'subscription-1',
        subject_revision: 2,
        payload: {
          event: 'worker_protocol',
          data: { worker_id: 'worker-2', event: { event: 'thinking_start' } },
        },
      },
    },
  };
  const decoded = decode(frame);
  assert(decoded.ok, `expected structurally valid event, got ${JSON.stringify(decoded)}`);
  const selector: EventSubscriptionSelector = {
    topic: 'worker_protocol',
    worker_id: 'worker-1',
  };
  assert(
    !subscriptionFrameMatchesSelector(decoded.value, selector),
    'mismatched Worker event crossed its subscription selector',
  );
});
