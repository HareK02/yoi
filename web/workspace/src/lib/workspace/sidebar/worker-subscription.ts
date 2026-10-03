import { type Readable, readable } from "svelte/store";
import type { WorkingDirectorySummary } from "#lib/generated/workdir-api.ts";
import type { SubscriptionWorker } from "#lib/generated/protocol.ts";
import {
  dismissWorkspaceAlert,
  pushWorkspaceAlert,
} from "#lib/workspace/alerts/store.ts";
import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";
import { parseWorkingDirectoryListResponse } from "#lib/workspace/api/workdirs.ts";
import {
  workspaceMultiplexer,
  type WorkspaceMultiplexerFailure,
  type WorkspaceMultiplexerSubscription,
} from "#lib/workspace/multiplexer.ts";
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
  /** Catalog lifecycle, independent of the possibly unknown foreground display state. */
  lifecycleState: SubscriptionWorker["state"];
};

export type WorkspaceWorkersState = {
  loading: boolean;
  workers: SidebarWorker[];
};

type WorkerAlertKind = "subscription" | "workdirs";
type WorkspaceWorkersStoreEntry = {
  store: Readable<WorkspaceWorkersState>;
  dispose(): void;
};

const stores = new Map<string, WorkspaceWorkersStoreEntry>();

function workerAlertId(workspaceId: string, kind: WorkerAlertKind): string {
  return `workspace:${workspaceId}:workers:${kind}`;
}

function reportWorkerFailure(
  workspaceId: string,
  kind: WorkerAlertKind,
  message: string,
  level: "warning" | "error" = "error",
): void {
  pushWorkspaceAlert(level, message, {
    id: workerAlertId(workspaceId, kind),
    title: kind === "workdirs"
      ? "Worker Workdirs unavailable"
      : "Worker updates unavailable",
  });
}

function clearWorkerFailure(workspaceId: string, kind: WorkerAlertKind): void {
  dismissWorkspaceAlert(workerAlertId(workspaceId, kind));
}

function clearWorkerFailures(workspaceId: string): void {
  clearWorkerFailure(workspaceId, "subscription");
  clearWorkerFailure(workspaceId, "workdirs");
}

export function disposeWorkspaceWorkersStore(workspaceId: string): void {
  const entry = stores.get(workspaceId);
  if (!entry) {
    clearWorkerFailures(workspaceId);
    return;
  }
  stores.delete(workspaceId);
  entry.dispose();
}

export function workspaceWorkersStore(
  workspaceId: string,
): Readable<WorkspaceWorkersState> {
  const cached = stores.get(workspaceId);
  if (cached) return cached.store;

  let stopActive: (() => void) | null = null;
  const store = readable<WorkspaceWorkersState>(
    { loading: true, workers: [] },
    (set) => {
      if (!workspaceId) {
        set({ loading: false, workers: [] });
        return;
      }
      const projection = createWorkspaceWorkersProjection();
      const workdirRequest = new AbortController();
      let subscription: WorkspaceMultiplexerSubscription | null = null;
      let workdirs = new Map<string, WorkingDirectorySummary>();
      let workers: SidebarWorker[] = [];
      let loading = true;
      let disposed = false;
      const publish = (): boolean => {
        if (disposed) return false;
        try {
          workers = [...projection.workers.values()]
            .map((worker) => projectWorker(worker, workdirs))
            .sort(compareWorkersForSidebar);
          set({ loading, workers });
          return true;
        } catch (cause) {
          reportWorkerFailure(
            workspaceId,
            "subscription",
            cause instanceof Error
              ? cause.message
              : "Invalid Worker subscription frame",
          );
          set({ loading: false, workers });
          return false;
        }
      };
      const loadWorkdirs = async () => {
        const result = await loadJson(
          fetch,
          workspaceApiPath(workspaceId, "/working-directories"),
          { signal: workdirRequest.signal },
          parseWorkingDirectoryListResponse,
        );
        if (disposed) return;
        if (!result.data) {
          reportWorkerFailure(
            workspaceId,
            "workdirs",
            result.error ?? "Worker Workdirs could not be loaded",
            "warning",
          );
          return;
        }
        workdirs = new Map(
          result.data.items.map((
            workdir,
          ) => [workdir.working_directory_id, workdir]),
        );
        clearWorkerFailure(workspaceId, "workdirs");
        publish();
      };
      subscription = workspaceMultiplexer(workspaceId).subscribe(
        { topic: "workspace_workers" },
        {
          onFrame: (frame) => {
            if (disposed) return;
            if (
              frame.frame === "event" &&
              frame.message.event === "subscription_closed"
            ) {
              loading = projection.workers.size === 0;
              if (
                frame.message.data.code === "resource_gone" ||
                frame.message.data.code === "unauthorized"
              ) {
                reportWorkerFailure(
                  workspaceId,
                  "subscription",
                  frame.message.data.message,
                );
              }
              publish();
              return;
            }
            if (
              frame.frame === "response" &&
              frame.message.result === "subscription_rejected"
            ) {
              loading = false;
              reportWorkerFailure(
                workspaceId,
                "subscription",
                frame.message.payload.message,
              );
              publish();
              return;
            }
            try {
              applyWorkspaceWorkersFrame(projection, frame);
              loading = false;
              if (publish()) clearWorkerFailure(workspaceId, "subscription");
            } catch (cause) {
              loading = false;
              reportWorkerFailure(
                workspaceId,
                "subscription",
                cause instanceof Error
                  ? cause.message
                  : "Invalid Worker subscription frame",
              );
              publish();
            }
          },
          onStatus: (status, failure?: WorkspaceMultiplexerFailure) => {
            if (disposed) return;
            if (status === "connecting") {
              loading = projection.workers.size === 0;
              publish();
            }
            if (status === "closed" && failure) {
              loading = projection.workers.size === 0;
              reportWorkerFailure(
                workspaceId,
                "subscription",
                failure.message,
                failure.kind === "transport" ? "warning" : "error",
              );
              publish();
            }
          },
        },
      );
      void loadWorkdirs();

      const stop = () => {
        if (disposed) return;
        disposed = true;
        workdirRequest.abort();
        subscription?.close();
        clearWorkerFailures(workspaceId);
        if (stopActive === stop) stopActive = null;
      };
      stopActive = stop;
      return stop;
    },
  );
  const entry: WorkspaceWorkersStoreEntry = {
    store,
    dispose: () => {
      stopActive?.();
      clearWorkerFailures(workspaceId);
    },
  };
  stores.set(workspaceId, entry);
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
    job: worker.job ?? null,
    tags: [],
    workspace: {
      visibility: "workspace",
      identity: "runtime_subscription_worker",
    },
    availability: worker.availability,
    state: liveWorkerState(worker),
    lifecycleState: worker.state,
    worker_state: worker.worker_state,
    pinned: false,
    retention_state: "transient",
    implementation: {
      kind: "runtime_subscription_worker",
      display_hint: "Workspace-authorized Runtime Worker",
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
