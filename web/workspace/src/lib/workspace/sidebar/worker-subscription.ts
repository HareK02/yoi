import { browser } from '$app/environment';
import { readable, type Readable } from 'svelte/store';
import type { SubscriptionFrame, SubscriptionWorker } from '$lib/generated/protocol';
import { workspaceApiPath } from '$lib/workspace/api/http';
import {
  applyWorkspaceWorkersFrame,
  createWorkspaceWorkersProjection,
} from './worker-subscription-model';
import type { Worker } from './types';

export type WorkspaceWorkersState = {
  loading: boolean;
  error: string | null;
  workers: Worker[];
};

const stores = new Map<string, Readable<WorkspaceWorkersState>>();

export function workspaceWorkersStore(workspaceId: string): Readable<WorkspaceWorkersState> {
  const cached = stores.get(workspaceId);
  if (cached) return cached;
  const store = readable<WorkspaceWorkersState>(
    { loading: true, error: null, workers: [] },
    (set) => {
      if (!browser || !workspaceId) {
        set({ loading: false, error: null, workers: [] });
        return;
      }
      let closed = false;
      let socket: WebSocket | null = null;
      let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
      const projection = createWorkspaceWorkersProjection();

      const publish = (loading = false, error: string | null = null) => {
        const workers = [...projection.workers.values()]
          .map(projectWorker)
          .sort((left, right) =>
            left.runtime_id.localeCompare(right.runtime_id) ||
            left.worker_id.localeCompare(right.worker_id)
          );
        set({ loading, error, workers });
      };
      const connect = () => {
        if (closed) return;
        const url = new URL(workspaceApiPath(workspaceId, '/protocol/ws'), window.location.origin);
        url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
        socket = new WebSocket(url);
        socket.addEventListener('open', () => {
          const frame: SubscriptionFrame = {
            protocol_version: 1,
            frame: 'request',
            message: {
              method: 'subscribe_events',
              params: {
                request_id: crypto.randomUUID(),
                selector: { topic: 'workspace_workers' },
              },
            },
          };
          socket?.send(JSON.stringify(frame));
        });
        socket.addEventListener('message', (message) => {
          try {
            const frame = JSON.parse(String(message.data)) as SubscriptionFrame;
            let closedMessage: string | null = null;
            if (frame.frame === 'event' && frame.message.event === 'subscription_closed') {
              closedMessage = frame.message.data.message;
            } else if (
              frame.frame === 'response' &&
              frame.message.result === 'subscription_rejected'
            ) {
              closedMessage = frame.message.payload.message;
            }
            if (closedMessage) {
              socket?.close();
              throw new Error(closedMessage);
            }
            applyWorkspaceWorkersFrame(projection, frame);
            publish(false, null);
          } catch (error) {
            publish(false, error instanceof Error ? error.message : 'invalid Worker subscription frame');
          }
        });
        socket.addEventListener('close', () => {
          socket = null;
          if (closed) return;
          publish(projection.workers.size === 0, 'Worker subscription disconnected; reconnecting…');
          reconnectTimer = setTimeout(connect, 500);
        });
        socket.addEventListener('error', () => socket?.close());
      };
      connect();
      return () => {
        closed = true;
        if (reconnectTimer) clearTimeout(reconnectTimer);
        socket?.close();
      };
    },
  );
  stores.set(workspaceId, store);
  return store;
}

function projectWorker(worker: SubscriptionWorker): Worker {
  if (!worker.runtime_id) throw new Error('Workspace Worker projection is missing runtime_id');
  const displayName = worker.display_name ?? `Worker ${worker.worker_id}`;
  return {
    runtime_id: worker.runtime_id,
    worker_id: worker.worker_id,
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
    working_directory: null,
    diagnostics: [],
  };
}
