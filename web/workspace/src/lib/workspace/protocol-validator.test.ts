import type { EventSubscriptionSelector, SubscriptionFrame } from '$lib/generated/protocol';
import {
  decodeSubscriptionFrame,
  MAX_SUBSCRIPTION_STRING_BYTES,
  RUST_SERIALIZED_SUBSCRIPTION_FRAME_FIXTURES,
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

Deno.test('pending submission previews accept new and legacy runtime payloads', () => {
  for (const preview of [undefined, '日本語 <b>literal</b>', null]) {
    const result = decode(subscribedFrame({
      selector: { topic: 'worker_protocol', worker_id: 'worker-1' },
      snapshot: { topic: 'worker_protocol', data: {
        worker_id: 'worker-1',
        events: [{ event: 'pending_submissions_changed', data: { pending: {
          revision: 1, notification_count: 0, head_id: 'queued-1',
          submissions: [{ submission_id: 'queued-1', accepted_at_ms: 1,
            segment_count: 1, byte_len: 10, ...(preview === undefined ? {} : { preview }) }],
        } } }],
      } },
    }));
    assert(result.ok, `pending preview rejected: ${JSON.stringify(result)}`);
  }
});

Deno.test('pending notification previews accept new and legacy runtime payloads', () => {
  for (const previews of [undefined, [], ['通知 <b>literal</b>']]) {
    const result = decode(subscribedFrame({
      selector: { topic: 'worker_protocol', worker_id: 'worker-1' },
      snapshot: { topic: 'worker_protocol', data: {
        worker_id: 'worker-1',
        events: [{ event: 'pending_submissions_changed', data: { pending: {
          revision: 1, notification_count: 1, head_id: 'notification-head', submissions: [],
          ...(previews === undefined ? {} : { notification_previews: previews }),
        } } }],
      } },
    }));
    assert(result.ok, `notification previews rejected: ${JSON.stringify(result)}`);
  }
});

Deno.test('generated subscription validator accepts an external Workdir Worker', () => {
  const result = decode(subscribedFrame({
    snapshot: {
      topic: 'workers',
      data: {
        workers: [
          {
            worker_id: 'worker-1',
            runtime_id: 'runtime-1',
            resource_key: 'worker-resource-1',
            availability: 'observed',
            subject_revision: 1,
            state: 'idle',
            has_running_internal_workers: false,
            workspace_id: 'workspace-1',
            display_name: 'Companion',
            profile: 'companion',
            workdir_attachments: [
              {
                alias: 'workspace',
                working_directory_id: 'external-workdir-1',
              },
            ],
          },
        ],
      },
    },
  }));
  assert(result.ok, `external Workdir Worker was rejected: ${JSON.stringify(result)}`);
});

Deno.test('Rust-serialized compatibility fixtures satisfy the generated Browser contract', () => {
  for (const fixture of RUST_SERIALIZED_SUBSCRIPTION_FRAME_FIXTURES) {
    const result = decode(fixture);
    assert(result.ok, `Rust fixture was rejected: ${JSON.stringify(result)}`);
  }
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
    [
      'missing serialized Worker defaults',
      subscribedFrame({
        snapshot: {
          topic: 'workers',
          data: {
            workers: [
              { worker_id: 'worker-1', subject_revision: 1, state: 'idle' },
            ],
          },
        },
      }),
    ],
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
    [
      'out-of-range Rust u32',
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
              data: {
                worker_id: 'worker-1',
                event: {
                  event: 'llm_retry',
                  data: {
                    llm_call: 1,
                    failed_attempt: 4_294_967_296,
                    max_attempts: 2,
                    wait_ms: 1,
                    elapsed_ms: 1,
                    error: 'retry',
                  },
                },
              },
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

Deno.test('aggregate limits include keys in unconstrained Worker JSON values', () => {
  const oversizedKey = 'k'.repeat(MAX_SUBSCRIPTION_STRING_BYTES + 1);
  const result = decode({
    protocol_version: 1,
    frame: 'event',
    message: {
      event: 'event',
      data: {
        subscription_id: 'subscription-1',
        subject_revision: 2,
        payload: {
          event: 'worker_protocol',
          data: {
            worker_id: 'worker-1',
            event: {
              event: 'system_item',
              data: { item: { [oversizedKey]: true } },
            },
          },
        },
      },
    },
  });
  assert(!result.ok && result.reason === 'aggregate_limit', 'oversized object key passed');
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
