import type { WorkerStateSnapshot } from "$lib/generated/protocol";

export function liveWorkerState(worker: {
  state: string;
  availability?: "observed" | "unavailable";
  worker_state?: WorkerStateSnapshot | null;
}): string {
  if (worker.availability === "unavailable") return "unknown";
  const state = worker.worker_state?.state;
  if (!state) {
    if (worker.state === "missing" || worker.state === "stopped") {
      return worker.state;
    }
    return "unknown";
  }
  if (state.kind === "idle") return "idle";
  if (state.state.kind === "maintenance") return "running";
  return state.state.state === "paused" ? "paused" : "running";
}
