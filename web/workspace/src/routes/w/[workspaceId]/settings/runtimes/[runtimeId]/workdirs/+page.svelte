<script lang="ts">
  import { pushWorkspaceAlert } from '$lib/workspace/alerts/store';
  import { workspaceApiPath } from '$lib/workspace/api/http';
  import { formatCurrentWorkdirRevision } from '$lib/workspace/settings/workdir-revision';
  import type {
    CleanupWorkdirCandidate,
    RuntimeCleanupExecutionResponse,
    RuntimeCleanupPlanResponse,
    WorkingDirectorySummary,
  } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let cleanupBusyTarget = $state<string | null>(null);
  let cleanupPlan = $state<RuntimeCleanupPlanResponse | null>(null);
  let workdirs = $state<WorkingDirectorySummary[]>([]);
  let runtimeLabel = $derived(
    data.runtimes?.items.find((runtime) => runtime.runtime_id === data.runtimeId)?.label ?? data.runtimeId,
  );
  let cleanupCandidates = $derived(cleanupPlan?.workdirs ?? []);

  $effect(() => {
    cleanupPlan = data.cleanupPlan ?? null;
    workdirs = data.workdirs?.items ?? [];
  });

  function repositoryProvider(workdir: WorkingDirectorySummary): string | null {
    return data.repositories?.items.find((repository) => repository.repository_key === workdir.repository_key)
      ?.provider ?? null;
  }

  function currentRevision(workdir: WorkingDirectorySummary): string {
    return formatCurrentWorkdirRevision(workdir, repositoryProvider(workdir));
  }

  function cleanupCandidate(workdir: WorkingDirectorySummary): CleanupWorkdirCandidate | undefined {
    return cleanupCandidates.find((candidate) => candidate.workdir_id === workdir.working_directory_id);
  }

  function isDeleteDisabled(candidate: CleanupWorkdirCandidate): boolean {
    return Boolean(candidate.blocking_reason) || candidate.action === 'workdir_dirty_discard' || cleanupBusyTarget !== null;
  }

  function errorMessage(payload: unknown, fallback: string): string {
    if (payload && typeof payload === 'object') {
      if ('message' in payload && typeof payload.message === 'string') return payload.message;
      if ('error' in payload) {
        const error = payload.error;
        if (typeof error === 'string') return error;
        if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
      }
      if ('diagnostics' in payload && Array.isArray(payload.diagnostics)) {
        const diagnostic = payload.diagnostics.find(
          (entry): entry is { message: string } => Boolean(entry) && typeof entry === 'object' && 'message' in entry && typeof entry.message === 'string',
        );
        if (diagnostic) return diagnostic.message;
      }
    }
    return fallback;
  }

  async function deleteWorkdir(workdir: WorkingDirectorySummary, candidate: CleanupWorkdirCandidate): Promise<void> {
    if (!cleanupPlan || isDeleteDisabled(candidate)) return;
    cleanupBusyTarget = candidate.target_id;
    try {
      const response = await fetch(
        workspaceApiPath(data.workspaceId, `/runtimes/${encodeURIComponent(data.runtimeId)}/cleanup-executions`),
        {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            expected_plan_revision: cleanupPlan.revision,
            expected_plan_digest: cleanupPlan.digest,
            worker_target_ids: [],
            workdir_target_ids: [candidate.target_id],
            confirm_dirty_discard_target_ids: [],
          }),
        },
      );
      const payload = (await response.json().catch(() => null)) as RuntimeCleanupExecutionResponse | unknown;
      if (!response.ok) throw new Error(errorMessage(payload, response.statusText));
      if (payload && typeof payload === 'object' && 'plan_after' in payload) {
        cleanupPlan = (payload as RuntimeCleanupExecutionResponse).plan_after;
      }
      const result = payload && typeof payload === 'object' && 'results' in payload
        ? (payload as RuntimeCleanupExecutionResponse).results.find((entry) => entry.target_id === candidate.target_id)
        : undefined;
      if (!result || result.status !== 'deleted') {
        throw new Error(result?.message ?? 'Runtime did not delete the selected Workdir');
      }
      workdirs = workdirs.filter((item) => item.working_directory_id !== workdir.working_directory_id);
    } catch (error) {
      pushWorkspaceAlert('error', error instanceof Error ? error.message : 'Workdir deletion failed', {
        title: 'Workdir deletion failed',
      });
    } finally {
      cleanupBusyTarget = null;
    }
  }
</script>

<svelte:head>
  <title>Workdirs · {runtimeLabel} · Yoi Workspace</title>
  <meta name="description" content="Runtime workdirs" />
</svelte:head>

<section class="workdirs-page" aria-labelledby="workdirs-heading">
  <header class="page-header-row">
    <div>
      <p class="breadcrumb"><a href={`/w/${data.workspaceId}/settings/runtimes`}>Runtimes</a> / {runtimeLabel}</p>
      <h1 id="workdirs-heading">Workdirs</h1>
      <p>Workdirs owned by <code>{data.runtimeId}</code>.</p>
      {#if data.cleanupPlanError}<p class="section-state error">{data.cleanupPlanError}</p>{/if}
    </div>
  </header>

  {#if data.workdirsError}
    <p class="section-state error">{data.workdirsError}</p>
  {:else if !data.workdirs}
    <p class="section-state">Loading workdirs…</p>
  {:else if workdirs.length === 0}
    <p class="section-state">No workdirs are visible for this Runtime.</p>
  {:else}
    <div class="table-wrap">
      <table class="workdirs-table">
        <thead>
          <tr>
            <th>Workdir</th>
            <th>Repository</th>
            <th>Revision</th>
            <th>Status</th>
            <th>Cleanliness</th>
            <th>Occupied by</th>
            <th>Action</th>
          </tr>
        </thead>
        <tbody>
          {#each workdirs as workdir}
            {@const cleanup = cleanupCandidate(workdir)}
            <tr>
              <td><code>{workdir.working_directory_id}</code></td>
              <td>{workdir.repository_key}</td>
              <td><code>{currentRevision(workdir)}</code></td>
              <td>{workdir.status}</td>
              <td>{workdir.cleanliness ?? 'unknown'}</td>
              <td>
                {#if workdir.occupied_by}
                  <span>{workdir.occupied_by.display_name}</span>
                  <small>{workdir.occupied_by.runtime_id}:{workdir.occupied_by.worker_id}</small>
                {:else}
                  <span class="muted">—</span>
                {/if}
              </td>
              <td>
                {#if cleanup}
                  <button
                    class="icon-action danger"
                    type="button"
                    disabled={isDeleteDisabled(cleanup)}
                    aria-label={`Delete ${workdir.working_directory_id}`}
                    title={cleanup.action === 'workdir_dirty_discard' ? 'Dirty Workdirs must be cleaned before deletion' : (cleanup.blocking_reason ?? cleanup.reason)}
                    onclick={() => deleteWorkdir(workdir, cleanup)}
                  >
                    {#if cleanupBusyTarget === cleanup.target_id}
                      <span class="spinner" aria-hidden="true"></span>
                    {:else}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path d="M3 6h18" /><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>
                    {/if}
                  </button>
                {:else}
                  <span class="muted">—</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .icon-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }

  .icon-action.danger:hover:not(:disabled),
  .icon-action.danger:focus-visible:not(:disabled) {
    border-color: var(--danger, oklch(60% 0.18 30));
    color: var(--danger, oklch(60% 0.18 30));
  }

  .icon-action:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .action-icon {
    width: 1rem;
    height: 1rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 999px;
    animation: workdir-action-spin 0.8s linear infinite;
  }

  @keyframes workdir-action-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
