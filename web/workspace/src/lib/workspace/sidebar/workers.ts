import type { Worker } from './types';

export type SidebarWorkerActivity =
  | 'worker-running'
  | 'subworker-running'
  | 'idle'
  | 'none';

type WorkerActivitySource = Pick<Worker, 'state'> & {
  has_running_internal_workers: boolean;
};

export function sidebarWorkerActivity(
  worker: WorkerActivitySource,
): SidebarWorkerActivity {
  if (worker.state === 'running') return 'worker-running';
  if (worker.has_running_internal_workers) return 'subworker-running';
  if (worker.state === 'idle') return 'idle';
  return 'none';
}

export function canShowWorkerInSidebar(worker: Worker): boolean {
  return worker.implementation.kind !== 'backend_worker_registry';
}

export function canOpenWorkerConsole(worker: Worker): boolean {
  return canShowWorkerInSidebar(worker);
}

type SortableWorker = Pick<
  Worker,
  'state' | 'display_name' | 'runtime_id' | 'worker_id'
>;

function workerStateRank(state: Worker['state']): number {
  switch (state) {
    case 'running':
      return 0;
    case 'idle':
      return 1;
    case 'stopped':
      return 2;
    default:
      return 3;
  }
}

export function compareWorkersForSidebar(
  left: SortableWorker,
  right: SortableWorker,
): number {
  const stateOrder = workerStateRank(left.state) - workerStateRank(right.state);
  if (stateOrder !== 0) return stateOrder;
  return (left.display_name ?? left.worker_id).localeCompare(
    right.display_name ?? right.worker_id,
  ) || left.runtime_id.localeCompare(right.runtime_id) ||
    left.worker_id.localeCompare(right.worker_id);
}
