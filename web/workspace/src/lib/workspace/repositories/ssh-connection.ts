import type { WorkspaceRuntimeResource } from "$lib/generated/workspace-api";

export type RepositorySshProbeSelection<T> = Readonly<{
  changed: boolean;
  runtimeId: string;
  probe: T | null;
  selectedHostKey: string;
}>;

export function changeRepositorySshProbeRuntime<T>(
  currentRuntimeId: string,
  nextRuntimeId: string,
  probe: T | null,
  selectedHostKey: string,
): RepositorySshProbeSelection<T> {
  if (currentRuntimeId === nextRuntimeId) {
    return {
      changed: false,
      runtimeId: currentRuntimeId,
      probe,
      selectedHostKey,
    };
  }
  return {
    changed: true,
    runtimeId: nextRuntimeId,
    probe: null,
    selectedHostKey: "",
  };
}

export type RepositorySshProbeOperation = Readonly<{
  runtimeId: string;
  generation: number;
}>;

export class RepositorySshProbeFence {
  #runtimeId: string | null = null;
  #generation = 0;

  enter(runtimeId: string): number {
    if (this.#runtimeId !== runtimeId) {
      this.#runtimeId = runtimeId;
      this.#generation += 1;
    }
    return this.#generation;
  }

  capture(runtimeId: string): RepositorySshProbeOperation {
    return { runtimeId, generation: this.enter(runtimeId) };
  }

  isCurrent(
    operation: RepositorySshProbeOperation,
    runtimeId: string,
  ): boolean {
    return operation.runtimeId === runtimeId &&
      operation.generation === this.#generation &&
      this.#runtimeId === runtimeId;
  }
}

export function repositorySshProbeRuntimes(
  runtimes: readonly WorkspaceRuntimeResource[],
): WorkspaceRuntimeResource[] {
  return runtimes.filter((runtime) =>
    runtime.kind === "remote_worker_runtime" &&
    runtime.management.endpoint_configured &&
    runtime.management.binding !== undefined &&
    runtime.management.binding !== null &&
    runtime.management.binding.state !== "revoked"
  );
}
