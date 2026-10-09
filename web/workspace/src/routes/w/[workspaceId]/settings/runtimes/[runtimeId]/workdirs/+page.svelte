<script lang="ts">
  import { loadJson, workspaceApiPath } from '#lib/workspace/api/http.ts';
  import { parseRuntimeCleanupPlan } from '#lib/workspace/api/runtime-workers.ts';
  import { parseWorkingDirectoryListResponse } from '#lib/workspace/api/workdirs.ts';
  import { canRemoveWorkdir, removalCause, removalGuard, removeWorkdir, WorkdirRemovalError } from '#lib/workspace/settings/workdir-removal.ts';
  import { formatCurrentWorkdirRevision } from '#lib/workspace/settings/workdir-revision.ts';
  import type { CleanupWorkdirCandidate, RuntimeCleanupPlanResponse, WorkingDirectorySummary } from '#lib/workspace/sidebar/types.ts';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let cleanupBusyTarget = $state<string | null>(null);
  let refreshing = $state(false);
  let refreshError = $state<string | null>(null);
  let cleanupPlan = $state<RuntimeCleanupPlanResponse | null>(null);
  let workdirs = $state<WorkingDirectorySummary[]>([]);
  let inventoryLoaded = $state(false);
  let feedback = $state<{ id: string; message: string; retryable: boolean; removed: boolean } | null>(null);
  let runtimeLabel = $derived(data.runtimes?.items.find((runtime) => runtime.runtime_id === data.runtimeId)?.label ?? data.runtimeId);
  let canManage = $derived(data.workspace?.permissions.manage_runtimes === true);
  let cleanupCandidates = $derived(cleanupPlan?.workdirs ?? []);

  $effect(() => {
    cleanupPlan = data.cleanupPlan ?? null;
    workdirs = data.workdirs?.items ?? [];
    inventoryLoaded = Boolean(data.workdirs);
    refreshError = data.workdirsError || data.cleanupPlanError ? 'Inventory or removal eligibility is unavailable. Refresh before deletion.' : null;
    feedback = null;
  });

  function repositoryKey(workdir: WorkingDirectorySummary): string | null {
    return workdir.source.kind === 'repository' ? workdir.source.repository_key : null;
  }
  function currentRevision(workdir: WorkingDirectorySummary): string {
    const key = repositoryKey(workdir);
    const provider = key ? data.repositories?.items.find((repository) => repository.repository_key === key)?.provider ?? null : null;
    return formatCurrentWorkdirRevision(workdir, provider);
  }
  function cleanupCandidate(workdir: WorkingDirectorySummary): CleanupWorkdirCandidate | undefined {
    return cleanupCandidates.find((candidate) => candidate.workdir_id === workdir.working_directory_id);
  }
  function isDeleteDisabled(workdir: WorkingDirectorySummary): boolean {
    return !canManage || Boolean(refreshError) || refreshing || cleanupBusyTarget !== null || !canRemoveWorkdir(workdir, cleanupCandidate(workdir));
  }

  async function refreshInventory(): Promise<void> {
    refreshing = true;
    // Capture route authority before awaiting; a departed route must not receive this result.
    const workspaceId = data.workspaceId;
    const runtimeId = data.runtimeId;
    try {
      const base = `/runtimes/${encodeURIComponent(runtimeId)}`;
      const [inventory, plan] = await Promise.all([
        loadJson(fetch, workspaceApiPath(workspaceId, `${base}/working-directories`), undefined, parseWorkingDirectoryListResponse),
        loadJson(fetch, workspaceApiPath(workspaceId, `${base}/cleanup-plan`), undefined, parseRuntimeCleanupPlan),
      ]);
      if (workspaceId !== data.workspaceId || runtimeId !== data.runtimeId) return;
      // Keep the last observed list on failure, but never authorize another deletion with stale eligibility.
      if (inventory.data) { workdirs = inventory.data.items; inventoryLoaded = true; }
      cleanupPlan = plan.data;
      refreshError = inventory.error || plan.error ? 'Inventory or removal eligibility could not be refreshed. Refresh before another deletion.' : null;
    } finally {
      refreshing = false;
    }
  }

  async function deleteWorkdir(workdir: WorkingDirectorySummary): Promise<void> {
    if (isDeleteDisabled(workdir)) return;
    const workspaceId = data.workspaceId;
    const runtimeId = data.runtimeId;
    cleanupBusyTarget = workdir.working_directory_id;
    feedback = null;
    try {
      const result = await removeWorkdir(fetch, workspaceId, runtimeId, workdir.working_directory_id);
      if (workspaceId !== data.workspaceId || runtimeId !== data.runtimeId) return;
      feedback = {
        id: workdir.working_directory_id,
        message: result.disposition === 'removed' ? 'Deletion confirmed.' : removalCause(result.failure_category),
        retryable: result.retryable && result.disposition !== 'removed',
        removed: result.disposition === 'removed',
      };
    } catch (error) {
      if (workspaceId !== data.workspaceId || runtimeId !== data.runtimeId) return;
      feedback = { id: workdir.working_directory_id, message: error instanceof WorkdirRemovalError ? error.message : removalCause(null), retryable: false, removed: false };
    } finally {
      // Even a lost/error response can follow a real provider side effect. Re-read inventory AND guards.
      if (workspaceId === data.workspaceId && runtimeId === data.runtimeId) await refreshInventory();
      cleanupBusyTarget = null;
    }
  }
</script>

<svelte:head>
  <title>Workdirs · {runtimeLabel} · Yoi Workspace</title>
  <meta name="description" content="Runtime workdirs" />
</svelte:head>

<section class="workdirs-page" aria-labelledby="workdirs-heading" aria-busy={refreshing}>
  <header class="page-header-row">
    <div>
      <p class="breadcrumb"><a href={`/w/${data.workspaceId}/settings/runtimes`}>Runtimes</a> / {runtimeLabel}</p>
      <h1 id="workdirs-heading">Workdirs</h1>
      <p>Workdirs owned by <code>{data.runtimeId}</code>.</p>
    </div>
    <button class="workdir-action" type="button" disabled={refreshing || cleanupBusyTarget !== null} onclick={refreshInventory}>{refreshing ? 'Refreshing…' : 'Refresh'}</button>
  </header>

  {#if feedback}
    <div class="removal-feedback" class:error={!feedback.removed} role={feedback.removed ? 'status' : 'alert'}>
      <p><strong>{feedback.removed ? 'Workdir deleted' : 'Workdir not deleted'}</strong> · <code>{feedback.id}</code></p>
      <p>{feedback.message}</p>
      {#if feedback.retryable}<p>After resolving this cause, retry Delete. Current safety conditions will be rechecked.</p>{/if}
    </div>
  {/if}
  {#if refreshError}<p class="section-state error" role="alert">{refreshError}</p>{/if}

  {#if !inventoryLoaded}
    <p class="section-state">{data.workdirsError ? 'Workdir inventory is unavailable.' : 'Loading workdirs…'}</p>
  {:else if workdirs.length === 0}
    <p class="section-state">No workdirs are visible for this Runtime.</p>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (Horizontal inventory scroll must be keyboard-focusable.) -->
    <div class="table-wrap" role="region" aria-label="Workdir inventory" tabindex="0">
      <table class="workdirs-table">
        <thead><tr><th>Workdir</th><th>Repository</th><th>Revision</th><th>Status</th><th>Cleanliness</th><th>Occupied by</th></tr></thead>
        <tbody>
          {#each workdirs as workdir (workdir.working_directory_id)}
            {@const cleanup = cleanupCandidate(workdir)}
            <tr>
              <td class="workdir-identity">
                <span>{workdir.display_name ?? '—'}</span>
                <small><code>{workdir.working_directory_id}</code></small>
                {#if canManage}
                  <button class="workdir-action danger" type="button" disabled={isDeleteDisabled(workdir)}
                    aria-label={`${feedback?.id === workdir.working_directory_id && feedback.retryable ? 'Retry Delete' : 'Delete'} ${workdir.working_directory_id}`}
                    onclick={() => deleteWorkdir(workdir)}>
                    {cleanupBusyTarget === workdir.working_directory_id ? 'Deleting…' : feedback?.id === workdir.working_directory_id && feedback.retryable ? 'Retry Delete' : 'Delete'}
                  </button>
                  {#if !canRemoveWorkdir(workdir, cleanup)}<p class="removal-guard">{removalGuard(workdir, cleanup)}</p>{/if}
                {/if}
              </td>
              <td>{repositoryKey(workdir) ?? 'External'}</td>
              <td><code>{currentRevision(workdir)}</code></td>
              <td>{workdir.status}</td>
              <td>{workdir.cleanliness ?? 'unknown'}</td>
              <td>{#if workdir.occupied_by}<span>{workdir.occupied_by.display_name}</span><small>{workdir.occupied_by.runtime_id}:{workdir.occupied_by.worker_id}</small>{:else}<span class="muted">—</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .workdirs-page { min-width: 0; }
  .page-header-row { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: start; gap: var(--space-3); }
  .table-wrap { max-width: 100%; overflow-x: auto; }
  .workdirs-table { width: 100%; min-width: 54rem; border-collapse: collapse; }
  th, td { padding: var(--space-3); text-align: left; vertical-align: top; border-bottom: 1px solid var(--line); }
  th { color: var(--text-muted); font-weight: 500; }
  .workdir-identity { width: 18rem; min-width: 18rem; overflow-wrap: anywhere; }
  td small { display: block; color: var(--text-muted); margin-block: var(--space-1); }
  .workdir-action { display: inline-flex; align-items: center; justify-content: center; padding: var(--space-2) var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text); cursor: pointer; white-space: nowrap; }
  .workdir-action:hover:not(:disabled) { background: var(--interactive-hover); }
  .workdir-action.danger { color: var(--danger); }
  .workdir-action:disabled { cursor: not-allowed; opacity: 0.45; }
  .removal-feedback { margin-block: var(--space-4); overflow-wrap: anywhere; }
  .removal-feedback p { margin-block: var(--space-2); }
  .removal-feedback.error strong { color: var(--danger); }
  .removal-guard { margin-block: var(--space-2) 0; color: var(--text-muted); }
</style>
