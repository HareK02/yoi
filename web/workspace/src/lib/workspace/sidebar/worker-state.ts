import type { SubscriptionWorker } from "#lib/generated/protocol.ts";

export function liveWorkerState(worker: Pick<
  SubscriptionWorker,
  "availability" | "lifecycle" | "worker_state"
> & { state?: string }): string {
  if (worker.availability !== "observed") return "unknown";
  switch (worker.lifecycle) {
    case "not_started":
    case "stopped":
    case "awaiting_restore":
    case "creating":
    case "restoring":
    case "stopping":
    case "recovery_required":
      return worker.lifecycle;
    case "active":
      break;
    default:
      return "unknown";
  }
  const state = worker.worker_state?.state;
  if (!state) return "unknown";
  if (state.kind === "idle") return "idle";
  if (state.state.kind === "maintenance") return "running";
  return state.state.state === "paused" ? "paused" : "running";
}
