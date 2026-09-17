import {
  isCurrentWorkerSessionRequest,
  workerSessionAction,
  type WorkerSessionRequestIdentity,
  workerSessionRequestInit,
} from "./session-observation";
import type { SessionSnapshot } from "$lib/generated/protocol";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

const snapshot: SessionSnapshot = {
  pending_submissions: {
    revision: 0,
    notification_count: 0,
    head_id: null,
    submissions: [],
  },
  entries: [],
};

Deno.test("worker session request includes same-origin credentials", () => {
  const controller = new AbortController();
  const init = workerSessionRequestInit(controller.signal);
  assert(
    init.credentials === "same-origin",
    "session observation must use the Workspace auth cookie boundary",
  );
  assert(
    init.signal === controller.signal,
    "request must retain its abort fence",
  );
});

Deno.test("worker session observation selects live, retained, and unavailable behavior", () => {
  assert(
    workerSessionAction({ availability: "live_protocol" }).kind ===
      "subscribe_live",
    "live observation must subscribe",
  );
  const retained = workerSessionAction({
    availability: "retained_snapshot",
    snapshot,
  });
  assert(
    retained.kind === "apply_retained" && retained.snapshot === snapshot,
    "retained observation must apply its snapshot",
  );
  const unavailable = workerSessionAction({
    availability: "unavailable",
    message: "retention expired",
  });
  assert(
    unavailable.kind === "show_unavailable" &&
      unavailable.message === "retention expired",
    "unavailable observation must stay recoverable and visible",
  );
});

Deno.test("late worker session response cannot overwrite a newer selection", () => {
  const request: WorkerSessionRequestIdentity = {
    token: 7,
    workspaceId: "workspace-a",
    runtimeId: "runtime-a",
    workerId: "worker-a",
  };
  assert(
    isCurrentWorkerSessionRequest(request, request),
    "matching request identity must remain current",
  );
  assert(
    !isCurrentWorkerSessionRequest(request, {
      ...request,
      token: 8,
      workerId: "worker-b",
    }),
    "late response for an older token/worker must be fenced",
  );
});
