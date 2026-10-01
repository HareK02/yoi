import type { SessionSnapshot } from "#lib/generated/protocol.ts";

export type WorkerSessionObservation =
  | { availability: "live_protocol" }
  | { availability: "retained_snapshot"; snapshot: SessionSnapshot }
  | { availability: "unavailable"; message: string };

export type WorkerSessionAction =
  | { kind: "subscribe_live" }
  | { kind: "apply_retained"; snapshot: SessionSnapshot }
  | { kind: "show_unavailable"; message: string };

export interface WorkerSessionTarget {
  workspaceId: string;
  runtimeId: string;
  workerId: string;
}

export interface WorkerSessionRequestIdentity extends WorkerSessionTarget {
  token: number;
}

export function resolveWorkerSessionTarget(
  workspaceId: string,
  runtimeId: string | null | undefined,
  workerId: string | null | undefined,
): WorkerSessionTarget | null {
  if (
    workspaceId.trim().length === 0 ||
    runtimeId?.trim().length === 0 ||
    workerId?.trim().length === 0 ||
    runtimeId == null ||
    workerId == null
  ) {
    return null;
  }
  return { workspaceId, runtimeId, workerId };
}

export function workerSessionRequestInit(signal: AbortSignal): RequestInit {
  return {
    signal,
    credentials: "same-origin",
    headers: { accept: "application/json" },
  };
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
