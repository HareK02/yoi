import type {
  SubscriptionEventPayload,
  SubscriptionFrame,
  SubscriptionWorker,
} from '$lib/generated/protocol';

import { liveWorkerState } from './worker-state';
import {
  applyWorkspaceWorkersFrame,
  createWorkspaceWorkersProjection,
} from './worker-subscription-model';

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function assertThrows(fn: () => void, message: string): void {
  try {
    fn();
  } catch {
    return;
  }
  throw new Error(message);
}

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function worker(
  runtimeId: string,
  workerId: string,
  hasRunningInternalWorkers = false,
  availability: 'observed' | 'unavailable' = 'observed',
): SubscriptionWorker {
  return {
    worker_id: workerId,
    runtime_id: runtimeId,
    availability,
    state: 'idle',
    has_running_internal_workers: hasRunningInternalWorkers,
    workspace_id: 'workspace-test',
    display_name: null,
    profile: null,
    workdir_attachments: [],
  };
}

function key(runtimeId: string, workerId: string): string {
  return JSON.stringify([runtimeId, workerId]);
}

function snapshot(
  subscriptionId: string,
  workers: SubscriptionWorker[],
): SubscriptionFrame {
  return {
    protocol_version: 2,
    frame: 'response',
    message: {
      result: 'subscribed',
      payload: {
        request_id: `request-${subscriptionId}`,
        subscription_id: subscriptionId,
        selector: { topic: 'workspace_workers' },
        snapshot: { topic: 'workers', data: { workers } },
      },
    },
  };
}

function event(
  subscriptionId: string,
  payload: SubscriptionEventPayload,
): SubscriptionFrame {
  return {
    protocol_version: 2,
    frame: 'event',
    message: {
      event: 'event',
      data: { subscription_id: subscriptionId, payload },
    },
  };
}

Deno.test('Worker list state uses the authoritative live snapshot separately from lifecycle', () => {
  const active = worker('runtime-a', 'worker-1');
  active.worker_state = {
    last_command_id: 1,
    state: { kind: 'busy', state: { kind: 'run', state: 'paused' } },
  };
  assertEquals(liveWorkerState(active), 'paused');

  const unavailable = worker('runtime-a', 'worker-2', false, 'unavailable');
  unavailable.worker_state = {
    last_command_id: 2,
    state: { kind: 'busy', state: { kind: 'run', state: 'running' } },
  };
  assertEquals(liveWorkerState(unavailable), 'unknown');
  assertEquals(
    liveWorkerState({
      ...unavailable,
      availability: 'observed',
      worker_state: null,
      state: 'missing',
    }),
    'missing',
  );
  unavailable.availability = 'observed';
  unavailable.worker_state = null;
  unavailable.state = 'stopped';
  assertEquals(liveWorkerState(unavailable), 'stopped');
});

Deno.test('workspace Worker snapshot keeps equal local ids from different Runtimes', () => {
  const projection = createWorkspaceWorkersProjection();
  applyWorkspaceWorkersFrame(
    projection,
    snapshot('subscription-1', [worker('runtime-a', '1'), worker('runtime-b', '1')]),
  );
  assertEquals([...projection.workers.keys()].sort(), [key('runtime-a', '1'), key('runtime-b', '1')]);
});

Deno.test('workspace Worker snapshot and ordered live update preserve canonical Job metadata', () => {
  const projection = createWorkspaceWorkersProjection();
  const snapshotWorker = worker('runtime-a', 'job-worker');
  snapshotWorker.job = {
    job_id: 'check:T-1:r1',
    attempt_id: 'check:T-1:r1:attempt:1',
    purpose: 'ticket_item_check',
  };
  applyWorkspaceWorkersFrame(projection, snapshot('subscription-1', [snapshotWorker]));
  assertEquals(projection.workers.get(key('runtime-a', 'job-worker'))?.job, snapshotWorker.job);

  const updated = { ...snapshotWorker };
  updated.job = { ...snapshotWorker.job, attempt_id: 'check:T-1:r1:attempt:2' };
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-1', { event: 'worker_upserted', data: { worker: updated } }),
  );
  assertEquals(projection.workers.get(key('runtime-a', 'job-worker'))?.job, updated.job);
});

Deno.test('fresh subscription snapshot replaces old state and fences delayed old events', () => {
  const projection = createWorkspaceWorkersProjection();
  applyWorkspaceWorkersFrame(
    projection,
    snapshot('subscription-old', [worker('runtime-a', 'old'), worker('runtime-b', 'same')]),
  );
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-old', {
      event: 'worker_upserted',
      data: { worker: worker('runtime-a', 'before-reconnect') },
    }),
  );

  applyWorkspaceWorkersFrame(
    projection,
    snapshot('subscription-new', [worker('runtime-a', 'current')]),
  );
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-old', {
      event: 'worker_upserted',
      data: { worker: worker('runtime-a', 'late-old-event') },
    }),
  );

  assertEquals(projection.subscriptionId, 'subscription-new');
  assertEquals([...projection.workers.keys()], [key('runtime-a', 'current')]);
});

Deno.test('workspace Worker reducer applies ordered updates and composite removal', () => {
  const projection = createWorkspaceWorkersProjection();
  applyWorkspaceWorkersFrame(
    projection,
    snapshot('subscription-1', [worker('runtime-a', '1'), worker('runtime-b', '1')]),
  );

  const updated = worker('runtime-a', '1');
  updated.state = 'running';
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-1', { event: 'worker_upserted', data: { worker: updated } }),
  );
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-1', {
      event: 'worker_removed',
      data: { worker_id: '1', runtime_id: 'runtime-a' },
    }),
  );

  assertEquals([...projection.workers.keys()], [key('runtime-b', '1')]);
});

Deno.test('workspace Worker event before a snapshot is rejected', () => {
  const projection = createWorkspaceWorkersProjection();
  assertThrows(
    () =>
      applyWorkspaceWorkersFrame(
        projection,
        event('subscription-1', {
          event: 'worker_upserted',
          data: { worker: worker('runtime-a', '1') },
        }),
      ),
    'event-before-snapshot must be rejected',
  );
});

Deno.test('fatal child stop replaces the running-child sidebar projection', () => {
  const projection = createWorkspaceWorkersProjection();
  applyWorkspaceWorkersFrame(
    projection,
    snapshot('subscription-1', [worker('runtime-a', '1', true)]),
  );
  applyWorkspaceWorkersFrame(
    projection,
    event('subscription-1', {
      event: 'worker_upserted',
      data: { worker: worker('runtime-a', '1', false) },
    }),
  );

  assertEquals(
    projection.workers.get(key('runtime-a', '1'))?.has_running_internal_workers,
    false,
  );
});
