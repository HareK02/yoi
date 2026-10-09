// One synthetic authoritative Worker catalog. Both REST and protocol projections
// derive from these same records; no grant-only Worker list or production override.
import type { WorkerListResponse } from "../../../web/workspace/src/lib/generated/worker-launch-api.ts";
import type { SubscriptionWorker } from "../../../web/workspace/src/lib/generated/protocol.ts";
import type { RuntimeCleanupPlanResponse } from "../../../web/workspace/src/lib/generated/runtime-api.ts";
export function workerCatalog(workspaceId: string): WorkerListResponse {
  const representative = ["home-owner", "home-member", "home-long", "home-error"].includes(
    workspaceId,
  );
  return {
    workspace_id: workspaceId,
    limit: 100,
    source: "fixture",
    diagnostics: [],
    items: representative
      ? ["a", "b"].map((suffix, index) => ({
        runtime_id: workspaceId === "home-long"
          ? `fixture-runtime-${"r".repeat(100)}`
          : "fixture-runtime",
        worker_id: workspaceId === "home-long"
          ? `fixture-worker-${suffix}-${"w".repeat(100)}`
          : `fixture-worker-${suffix}`,
        resource_key: `W-${801 + index}`,
        host_id: "fixture-host",
        display_name: workspaceId === "home-long"
          ? "Documentation and distributed development Worker — 国際化されたドキュメントを整理する長い表示名 — "
            .repeat(2)
          : index === 0
          ? "Documentation Worker"
          : "画像・資料を整理する Worker — 長い名前",
        label: `Fixture Worker ${suffix}`,
        profile: "default",
        workspace: { workspace_id: workspaceId, visibility: "workspace", identity: workspaceId },
        availability: "observed",
        state: "idle",
        worker_state: { last_command_id: 0, state: { kind: "idle" } },
        pinned: false,
        retention_state: "active",
        implementation: { kind: "builtin", display_hint: "fixture" },
        tags: [],
        diagnostics: [],
        workdir_attachments: [],
        last_seen_at: "2026-01-02T00:00:00Z",
      }))
      : [],
  };
}
export function workerSnapshot(workspaceId: string): SubscriptionWorker[] {
  return workerCatalog(workspaceId).items.map((worker) => ({
    worker_id: worker.worker_id,
    runtime_id: worker.runtime_id,
    resource_key: worker.resource_key,
    availability: worker.availability,
    state: "idle",
    worker_state: worker.worker_state,
    workspace_id: workspaceId,
    display_name: worker.display_name,
    profile: worker.profile,
    has_running_internal_workers: false,
    workdir_attachments: [],
  }));
}
export function cleanupPlan(workspaceId: string, runtimeId: string): RuntimeCleanupPlanResponse {
  return {
    workspace_id: workspaceId,
    runtime_id: runtimeId,
    diagnostics: [],
    digest: "fixture-cleanup",
    generated_at: "2026-01-02T00:00:00Z",
    revision: "1",
    workers: [],
    workdirs: [],
  };
}
