import type { WorkerStateSnapshot } from "$lib/generated/protocol";

export function liveWorkerState(worker: {
  state: string;
  worker_state?: WorkerStateSnapshot | null;
}): string {
  const state = worker.worker_state?.state;
  if (!state) return worker.state === "stopped" ? "stopped" : "unknown";
  if (state.kind === "idle") return "idle";
  if (state.state.kind === "maintenance") return "running";
  return state.state.state === "paused" ? "paused" : "running";
}
