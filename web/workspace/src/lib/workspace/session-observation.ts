import type { SessionSnapshot } from "$lib/generated/protocol";

export type WorkerSessionObservation =
  | { availability: "live_protocol" }
  | { availability: "retained_snapshot"; snapshot: SessionSnapshot }
  | { availability: "unavailable"; message: string };

export type WorkerSessionAction =
  | { kind: "subscribe_live" }
  | { kind: "apply_retained"; snapshot: SessionSnapshot }
  | { kind: "show_unavailable"; message: string };

export interface WorkerSessionRequestIdentity {
  token: number;
  workspaceId: string;
  runtimeId: string;
  workerId: string;
}

export function workerSessionAction(
  observation: WorkerSessionObservation,
): WorkerSessionAction {
  switch (observation.availability) {
    case "live_protocol":
      return { kind: "subscribe_live" };
    case "retained_snapshot":
      return { kind: "apply_retained", snapshot: observation.snapshot };
    case "unavailable":
      return {
        kind: "show_unavailable",
        message: observation.message.slice(0, 512),
      };
  }
}

export function isCurrentWorkerSessionRequest(
  request: WorkerSessionRequestIdentity,
  current: WorkerSessionRequestIdentity,
): boolean {
  return (
    request.token === current.token &&
    request.workspaceId === current.workspaceId &&
    request.runtimeId === current.runtimeId &&
    request.workerId === current.workerId
  );
}
