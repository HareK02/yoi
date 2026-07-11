import type { Worker } from "./types";

export function canOpenWorkerConsole(worker: Worker): boolean {
  return worker.state !== "archived" &&
    worker.implementation.kind !== "backend_worker_registry";
}
