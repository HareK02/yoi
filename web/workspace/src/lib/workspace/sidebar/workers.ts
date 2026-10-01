import { workerHref } from '../resource-links';
import { ownsRoutePath } from './route-ownership';
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

export function canShowWorkerInSidebar(
  worker: Pick<Worker, 'implementation' | 'job'>,
): boolean {
  return !worker.job && worker.implementation.kind !== 'backend_worker_registry';
}

export function canOpenWorkerConsole(
  worker: Pick<Worker, 'implementation'>,
): boolean {
  return worker.implementation.kind !== 'backend_worker_registry';
}

type SidebarWorkerLink = Pick<Worker, 'resource_key' | 'display_name'>;

export function workerOwnsSidebarPath(
  workspaceId: string,
  worker: SidebarWorkerLink,
  currentPath: string,
): boolean {
  return ownsRoutePath(workerHref(workspaceId, worker), currentPath);
}

export function visibleWorkersForSidebar<T extends SidebarWorkerLink>(
  workers: readonly T[],
  options: {
    workspaceId: string;
    currentPath: string;
    expanded: boolean;
    limit: number;
  },
): readonly T[] {
  if (options.expanded || workers.length <= options.limit) return workers;
  if (options.limit <= 0) return [];

  const visible = workers.slice(0, options.limit);
  const currentIndex = workers.findIndex((worker) =>
    workerOwnsSidebarPath(options.workspaceId, worker, options.currentPath)
  );
  if (currentIndex < options.limit) return visible;

  visible[visible.length - 1] = workers[currentIndex];
  return visible;
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
