import type { RuntimeCleanupExecutionResult } from "#lib/generated/runtime-api.ts";
import type {
  WorkerRemovalCause,
  WorkerRemovalObservation,
  WorkerRemovalPhase,
} from "#lib/generated/worker-launch-api.ts";

export const removalPhaseLabels: Record<WorkerRemovalPhase, string> = {
  awaiting_quiescence: "Awaiting quiescence",
  input_handoff: "Preparing Job input",
  awaiting_effects_end: "Awaiting execution end",
  released: "Writer released; deletion pending",
  completed: "Input handoff complete; deletion unconfirmed",
};

export const removalCauseLabels: Record<WorkerRemovalCause, string> = {
  recording_disabled: "Journal recording disabled",
  source_unavailable: "Source unavailable",
  ownership_unknown: "Ownership unknown",
  job_pending: "Journal job pending",
  job_failed: "Journal job failed",
  execution_unconfirmed: "Execution end unconfirmed",
  storage_unavailable: "Storage unavailable",
};

export function removalStateLabel(removal: WorkerRemovalObservation): string {
  return removal.state === "held" ? "Removal held" : "Removal waiting";
}

export function removalDescription(removal: WorkerRemovalObservation): string {
  return `${removalStateLabel(removal)} · ${removalPhaseLabels[removal.phase]}${
    removal.cause ? ` · ${removalCauseLabels[removal.cause]}` : ""
  }. Worker retained, not deleted.`;
}

export type WorkerDeletionOutcome =
  | { removed: true }
  | { removed: false; removal: WorkerRemovalObservation };

/** Cleanup status and the saved removal observation must both confirm deletion. */
export function workerDeletionOutcome(
  result: RuntimeCleanupExecutionResult | undefined,
): WorkerDeletionOutcome {
  if (result?.removal) return { removed: false, removal: result.removal };
  if (result?.status === "deleted") return { removed: true };
  throw new Error(result?.message || "Worker deletion was not completed");
}

export function deletionNotice(outcome: WorkerDeletionOutcome): {
  level: "info" | "warning";
  title: string;
  message: string;
} {
  if (outcome.removed) {
    return {
      level: "info",
      title: "Worker deleted",
      message: "Worker deleted",
    };
  }
  return {
    level: outcome.removal.state === "held" ? "warning" : "info",
    title: "Worker removal accepted",
    message: `${removalDescription(outcome.removal)} View Removal operation for the saved intent.`,
  };
}
