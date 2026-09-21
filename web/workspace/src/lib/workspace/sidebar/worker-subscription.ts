import { type Readable, readable } from "svelte/store";
import type { WorkingDirectorySummary } from "$lib/generated/workdir-api";
import type { SubscriptionWorker } from "$lib/generated/protocol";
import { loadJson, workspaceApiPath } from "$lib/workspace/api/http";
import { parseWorkingDirectoryListResponse } from "$lib/workspace/api/workdirs";
import { workspaceMultiplexer } from "$lib/workspace/multiplexer";
import {
  applyWorkspaceWorkersFrame,
  createWorkspaceWorkersProjection,
} from "./worker-subscription-model";
import { liveWorkerState } from "./worker-state";
import type { SidebarWorkdirAttachment } from "./worker-workdir-meta";
import { compareWorkersForSidebar } from "./workers";
import type { Worker } from "./types";

export type SidebarWorker = Omit<Worker, "workdir_attachments"> & {
  workdir_attachments: SidebarWorkdirAttachment[];
  has_running_internal_workers: boolean;
};

export type WorkspaceWorkersState = {
  loading: boolean;
  error: string | null;
  workers: SidebarWorker[];
};

const stores = new Map<string, Readable<WorkspaceWorkersState>>();

export function disposeWorkspaceWorkersStore(workspaceId: string): void {
  stores.delete(workspaceId);
}

export function workspaceWorkersStore(
  workspaceId: string,
): Readable<WorkspaceWorkersState> {
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
      let workdirs = new Map<string, WorkingDirectorySummary>();
      let loading = true;
      let error: string | null = null;
      let disposed = false;
      let workdirRequest = 0;
      const publish = () => {
        const workers = [...projection.workers.values()]
          .map((worker) => projectWorker(worker, workdirs))
          .sort(compareWorkersForSidebar);
        set({ loading, error, workers });
      };
      const refreshWorkdirs = async () => {
        const request = ++workdirRequest;
        const result = await loadJson(
          fetch,
          workspaceApiPath(workspaceId, "/working-directories"),
          undefined,
          parseWorkingDirectoryListResponse,
        );
        if (disposed || request !== workdirRequest || !result.data) return;
        workdirs = new Map(
          result.data.items.map((
            workdir,
          ) => [workdir.working_directory_id, workdir]),
        );
        publish();
      };
      const multiplexer = workspaceMultiplexer(workspaceId);
      const subscription = multiplexer.subscribe(
        { topic: "workspace_workers" },
        {
          onFrame: (frame) => {
            try {
              if (
                frame.frame === "event" &&
                frame.message.event === "subscription_closed"
              ) {
                throw new Error(frame.message.data.message);
              }
              if (
                frame.frame === "response" &&
                frame.message.result === "subscription_rejected"
              ) {
                throw new Error(frame.message.payload.message);
              }
              applyWorkspaceWorkersFrame(projection, frame);
              loading = false;
              error = null;
              publish();
            } catch (cause) {
              loading = false;
              error = cause instanceof Error
                ? cause.message
                : "invalid Worker subscription frame";
              publish();
            }
          },
          onStatus: (status, message) => {
            if (status === "connecting") {
              loading = projection.workers.size === 0;
              error = null;
              publish();
            }
            if (status === "closed") {
              loading = projection.workers.size === 0;
              error = message ?? null;
              publish();
            }
          },
        },
      );
      const workdirSubscription = multiplexer.subscribe(
        { topic: "workspace_workdirs" },
        {
          onFrame: (frame) => {
            const isSnapshot = frame.frame === "response" &&
              frame.message.result === "subscribed";
            const isUpdate = frame.frame === "event" &&
              frame.message.event === "event" &&
              (frame.message.data.payload.event === "workdir_upserted" ||
                frame.message.data.payload.event === "workdir_removed");
            if (isSnapshot || isUpdate) void refreshWorkdirs();
          },
        },
      );
      void refreshWorkdirs();
      return () => {
        disposed = true;
        subscription.close();
        workdirSubscription.close();
      };
    },
  );
  stores.set(workspaceId, store);
  return store;
}

function projectWorker(
  worker: SubscriptionWorker,
  workdirs: ReadonlyMap<string, WorkingDirectorySummary>,
): SidebarWorker {
  if (!worker.runtime_id) {
    throw new Error("Workspace Worker projection is missing runtime_id");
  }
  if (!worker.resource_key) {
    throw new Error("Workspace Worker projection is missing resource_key");
  }
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
    workspace: {
      visibility: "workspace",
      identity: "runtime_subscription_worker",
    },
    state: liveWorkerState(worker),
    worker_state: worker.worker_state,
    pinned: false,
    retention_state: "transient",
    implementation: {
      kind: "runtime_subscription_worker",
      display_hint: "Workspace-authorized Runtime Worker",
    },
    capabilities: {
      can_stop: worker.availability !== "unavailable" &&
        worker.state !== "stopped",
      can_spawn_followup: false,
    },
    workdir_attachments: (worker.workdir_attachments ?? []).map((
      attachment,
    ) => ({
      ...attachment,
      working_directory: workdirs.get(attachment.working_directory_id),
    })),
    has_running_internal_workers: worker.has_running_internal_workers,
    diagnostics: [],
  };
}
