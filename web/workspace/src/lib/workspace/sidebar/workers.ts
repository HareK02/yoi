import type { Worker } from "./types";

export function canShowWorkerInSidebar(worker: Worker): boolean {
  return worker.implementation.kind !== "backend_worker_registry";
}

export function canOpenWorkerConsole(worker: Worker): boolean {
  return canShowWorkerInSidebar(worker);
}
