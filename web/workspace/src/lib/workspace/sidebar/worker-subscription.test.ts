import type { SubscriptionFrame, SubscriptionWorker } from '$lib/generated/protocol';

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
import {
  applyWorkspaceWorkersFrame,
  createWorkspaceWorkersProjection,
} from './worker-subscription-model';

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function worker(runtimeId: string, workerId: string, revision: number): SubscriptionWorker {
  return {
    worker_id: workerId,
    runtime_id: runtimeId,
    subject_revision: revision,
    state: 'idle',
    workspace_id: 'workspace-test',
    display_name: null,
    profile: null,
    working_directory_id: null,
  };
}

Deno.test('workspace Worker snapshot keeps equal local ids from different Runtimes', () => {
  const projection = createWorkspaceWorkersProjection();
  const frame: SubscriptionFrame = {
    protocol_version: 1,
    frame: 'response',
    message: {
      result: 'subscribed',
      payload: {
        request_id: 'request-1',
        subscription_id: 'subscription-1',
        selector: { topic: 'workspace_workers' },
        snapshot_revision: 1,
        snapshot: {
          topic: 'workers',
          data: { workers: [worker('runtime-a', '1', 1), worker('runtime-b', '1', 1)] },
        },
      },
    },
  };
  applyWorkspaceWorkersFrame(projection, frame);
  assertEquals([...projection.workers.keys()].sort(), ['runtime-a:1', 'runtime-b:1']);
});

Deno.test('workspace Worker reducer ignores stale events and removes composite subject', () => {
  const projection = createWorkspaceWorkersProjection();
  projection.workers.set('runtime-a:1', worker('runtime-a', '1', 3));
  projection.revisions.set('runtime-a:1', 3);

  applyWorkspaceWorkersFrame(projection, {
    protocol_version: 1,
    frame: 'event',
    message: {
      event: 'event',
      data: {
        subscription_id: 'subscription-1',
        subject_revision: 2,
        payload: { event: 'worker_upserted', data: { worker: worker('runtime-a', '1', 2) } },
      },
    },
  });
  assertEquals(projection.revisions.get('runtime-a:1'), 3);

  applyWorkspaceWorkersFrame(projection, {
    protocol_version: 1,
    frame: 'event',
    message: {
      event: 'event',
      data: {
        subscription_id: 'subscription-1',
        subject_revision: 4,
        payload: {
          event: 'worker_removed',
          data: { worker_id: '1', runtime_id: 'runtime-a' },
        },
      },
    },
  });
  assertEquals(projection.workers.size, 0);
  assertEquals(projection.revisions.get('runtime-a:1'), 4);
});
