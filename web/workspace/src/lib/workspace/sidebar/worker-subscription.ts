import { readable, type Readable } from 'svelte/store';
import type { SubscriptionWorker } from '$lib/generated/protocol';
import { workspaceMultiplexer } from '$lib/workspace/multiplexer';
import {
  applyWorkspaceWorkersFrame,
  createWorkspaceWorkersProjection,
} from './worker-subscription-model';
import { compareWorkersForSidebar } from './workers';
import type { Worker } from './types';

export type SidebarWorker = Worker & {
  repository_id: string | null;
  working_directory_id: string | null;
  has_running_internal_workers: boolean;
};

export type WorkspaceWorkersState = {
  loading: boolean;
  error: string | null;
  workers: SidebarWorker[];
};

const stores = new Map<string, Readable<WorkspaceWorkersState>>();

export function workspaceWorkersStore(workspaceId: string): Readable<WorkspaceWorkersState> {
  const cached = stores.get(workspaceId);
  if (cached) return cached;
  const store = readable<WorkspaceWorkersState>(
    { loading: true, error: null, workers: [] },
    (set) => {
      if (!workspaceId) {
        set({ loading: false, error: null, workers: [] });
        return;
      }
      const projection = createWorkspaceWorkersProjection();
      const publish = (loading = false, error: string | null = null) => {
        const workers = [...projection.workers.values()]
          .map(projectWorker)
          .sort(compareWorkersForSidebar);
        set({ loading, error, workers });
      };
      const subscription = workspaceMultiplexer(workspaceId).subscribe(
        { topic: 'workspace_workers' },
        {
          onFrame: (frame) => {
            try {
              if (frame.frame === 'event' && frame.message.event === 'subscription_closed') {
                throw new Error(frame.message.data.message);
              }
              if (
                frame.frame === 'response' &&
                frame.message.result === 'subscription_rejected'
              ) {
                throw new Error(frame.message.payload.message);
              }
              applyWorkspaceWorkersFrame(projection, frame);
              publish(false, null);
            } catch (error) {
              publish(false, error instanceof Error ? error.message : 'invalid Worker subscription frame');
            }
          },
          onStatus: (status, message) => {
            if (status === 'connecting') publish(projection.workers.size === 0, null);
            if (status === 'closed') publish(projection.workers.size === 0, message ?? null);
          },
        },
      );
      return () => subscription.close();
    },
  );
  stores.set(workspaceId, store);
  return store;
}

function projectWorker(worker: SubscriptionWorker): SidebarWorker {
  if (!worker.runtime_id) throw new Error('Workspace Worker projection is missing runtime_id');
  if (!worker.resource_key) throw new Error('Workspace Worker projection is missing resource_key');
  const displayName = worker.display_name ?? `Worker ${worker.worker_id}`;
  return {
    runtime_id: worker.runtime_id,
    worker_id: worker.worker_id,
    resource_key: worker.resource_key,
    host_id: worker.runtime_id,
    display_name: displayName,
    label: displayName,
    profile: worker.profile ?? null,
    tags: [],
    workspace: { visibility: 'workspace', identity: 'runtime_subscription_worker' },
    state: worker.state,
    pinned: false,
    retention_state: 'transient',
    implementation: {
      kind: 'runtime_subscription_worker',
      display_hint: 'Workspace-authorized Runtime Worker',
    },
    capabilities: {
      can_stop: worker.state !== 'stopped' && worker.state !== 'cancelled',
      can_spawn_followup: false,
    },
    repository_id: worker.repository_id ?? null,
    working_directory_id: worker.working_directory_id ?? null,
    has_running_internal_workers: worker.has_running_internal_workers,
    working_directory: null,
    diagnostics: [],
  };
}
