import type {
  SubscriptionEventPayload,
  SubscriptionFrame,
  SubscriptionWorker,
} from '$lib/generated/protocol';

export type WorkspaceWorkersProjection = {
  workers: Map<string, SubscriptionWorker>;
  subscriptionId: string | null;
};

export function createWorkspaceWorkersProjection(): WorkspaceWorkersProjection {
  return { workers: new Map(), subscriptionId: null };
}

export function applyWorkspaceWorkersFrame(
  projection: WorkspaceWorkersProjection,
  frame: SubscriptionFrame,
): void {
  if (frame.protocol_version !== 2) throw new Error('unsupported Worker subscription protocol');
  if (frame.frame === 'response' && frame.message.result === 'subscribed') {
    if (frame.message.payload.selector.topic !== 'workspace_workers') return;
    const snapshot = frame.message.payload.snapshot;
    if (snapshot.topic !== 'workers') throw new Error('workspace_workers returned a non-Worker snapshot');

    // Build the replacement before mutating the live projection so one malformed
    // Worker cannot partially clear the previous subscription lifetime.
    const workers = new Map<string, SubscriptionWorker>();
    for (const worker of snapshot.data.workers) {
      workers.set(workerKey(worker.runtime_id, worker.worker_id), worker);
    }
    projection.workers = workers;
    projection.subscriptionId = frame.message.payload.subscription_id;
    return;
  }
  if (frame.frame !== 'event' || frame.message.event !== 'event') return;
  const subscriptionId = frame.message.data.subscription_id;
  if (!projection.subscriptionId) {
    throw new Error('workspace_workers event arrived before its snapshot');
  }
  if (subscriptionId !== projection.subscriptionId) return;
  applyPayload(projection, frame.message.data.payload);
}

function applyPayload(
  projection: WorkspaceWorkersProjection,
  payload: SubscriptionEventPayload,
): void {
  if (payload.event === 'worker_upserted') {
    const worker = payload.data.worker;
    projection.workers.set(workerKey(worker.runtime_id, worker.worker_id), worker);
  } else if (payload.event === 'worker_removed') {
    projection.workers.delete(workerKey(payload.data.runtime_id, payload.data.worker_id));
  }
}

function workerKey(runtimeId: string | null | undefined, workerId: string): string {
  if (!runtimeId) throw new Error('Workspace Worker projection is missing runtime_id');
  return JSON.stringify([runtimeId, workerId]);
}
