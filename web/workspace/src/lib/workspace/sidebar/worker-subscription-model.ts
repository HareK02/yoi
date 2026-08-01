import type {
  SubscriptionEventPayload,
  SubscriptionFrame,
  SubscriptionWorker,
} from '$lib/generated/protocol';

export type WorkspaceWorkersProjection = {
  workers: Map<string, SubscriptionWorker>;
  revisions: Map<string, number>;
};

export function createWorkspaceWorkersProjection(): WorkspaceWorkersProjection {
  return { workers: new Map(), revisions: new Map() };
}

export function applyWorkspaceWorkersFrame(
  projection: WorkspaceWorkersProjection,
  frame: SubscriptionFrame,
): void {
  if (frame.protocol_version !== 1) throw new Error('unsupported Worker subscription protocol');
  if (frame.frame === 'response' && frame.message.result === 'subscribed') {
    if (frame.message.payload.selector.topic !== 'workspace_workers') return;
    const snapshot = frame.message.payload.snapshot;
    if (snapshot.topic !== 'workers') throw new Error('workspace_workers returned a non-Worker snapshot');
    projection.workers.clear();
    projection.revisions.clear();
    for (const worker of snapshot.data.workers) {
      const key = workerKey(worker.runtime_id, worker.worker_id);
      projection.workers.set(key, worker);
      projection.revisions.set(key, worker.subject_revision);
    }
    return;
  }
  if (frame.frame !== 'event' || frame.message.event !== 'event') return;
  applyPayload(projection, frame.message.data.subject_revision, frame.message.data.payload);
}

function applyPayload(
  projection: WorkspaceWorkersProjection,
  subjectRevision: number,
  payload: SubscriptionEventPayload,
): void {
  if (payload.event === 'worker_upserted') {
    const worker = payload.data.worker;
    const key = workerKey(worker.runtime_id, worker.worker_id);
    if (subjectRevision <= (projection.revisions.get(key) ?? 0)) return;
    projection.revisions.set(key, subjectRevision);
    projection.workers.set(key, worker);
  } else if (payload.event === 'worker_removed') {
    const key = workerKey(payload.data.runtime_id, payload.data.worker_id);
    if (subjectRevision <= (projection.revisions.get(key) ?? 0)) return;
    projection.revisions.set(key, subjectRevision);
    projection.workers.delete(key);
  }
}

function workerKey(runtimeId: string | null | undefined, workerId: string): string {
  if (!runtimeId) throw new Error('Workspace Worker projection is missing runtime_id');
  return `${runtimeId}:${workerId}`;
}
