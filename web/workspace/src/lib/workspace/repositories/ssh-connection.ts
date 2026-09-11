import type { WorkspaceRuntimeResource } from "$lib/generated/workspace-api";

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
