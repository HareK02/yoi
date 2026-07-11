<script lang="ts">
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import type {
    CleanupWorkdirCandidate,
    RuntimeCleanupExecutionResponse,
    RuntimeCleanupPlanResponse,
    WorkingDirectorySummary,
  } from '$lib/workspace-sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let cleanupStatus = $state<string | null>(null);
  let cleanupBusyTarget = $state<string | null>(null);
  let cleanupPlan = $state<RuntimeCleanupPlanResponse | null>(null);
  let runtimeLabel = $derived(
    data.runtimes?.items.find((runtime) => runtime.runtime_id === data.runtimeId)?.label ?? data.runtimeId,
  );
  let cleanupCandidates = $derived(cleanupPlan?.workdirs ?? []);

  $effect(() => {
    cleanupPlan = data.cleanupPlan ?? null;
  });

  function commitLabel(workdir: WorkingDirectorySummary): string {
    return workdir.resolved_commit ? workdir.resolved_commit.slice(0, 12) : '—';
  }

  function selectorLabel(workdir: WorkingDirectorySummary): string {
    return workdir.requested_selector ?? 'HEAD';
  }

  function cleanupLabel(candidate: CleanupWorkdirCandidate): string {
    if (candidate.action === 'workdir_dirty_discard') {
      return candidate.cleanliness === 'dirty' ? 'Discard' : 'Discard unknown';
    }
    if (candidate.action === 'workdir_record_delete') return 'Delete record';
    return 'Clean up';
  }

  function cleanupCandidate(workdir: WorkingDirectorySummary): CleanupWorkdirCandidate | undefined {
    return cleanupCandidates.find((candidate) => candidate.workdir_id === workdir.working_directory_id);
  }

  async function executeWorkdirCleanup(candidate: CleanupWorkdirCandidate): Promise<void> {
    if (!cleanupPlan) return;
    if (candidate.action === 'workdir_dirty_discard') {
      const confirmed = window.confirm(`${cleanupLabel(candidate)} ${candidate.workdir_id}? This explicitly discards the Workdir contents.`);
      if (!confirmed) return;
    }
    cleanupBusyTarget = candidate.target_id;
    cleanupStatus = null;
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
            confirm_dirty_discard_target_ids: candidate.action === 'workdir_dirty_discard' ? [candidate.target_id] : [],
          }),
        },
      );
      const payload = (await response.json().catch(() => null)) as RuntimeCleanupExecutionResponse | { message?: string; error?: string } | null;
      if (!response.ok) throw new Error(payload && 'message' in payload ? (payload.message ?? payload.error) : response.statusText);
      if (payload && 'plan_after' in payload) cleanupPlan = payload.plan_after;
      cleanupStatus = `Executed cleanup for ${candidate.workdir_id}. Refresh to see the latest Workdir list.`;
    } catch (error) {
      cleanupStatus = error instanceof Error ? error.message : 'Workdir cleanup failed';
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
      <p class="breadcrumb"><a href={`/w/${data.workspaceId}/runtimes`}>Runtimes</a> / {runtimeLabel}</p>
      <h1 id="workdirs-heading">Workdirs</h1>
      <p>Workdirs owned by <code>{data.runtimeId}</code>.</p>
      {#if data.cleanupPlanError}<p class="section-state error">{data.cleanupPlanError}</p>{/if}
      {#if cleanupStatus}<p>{cleanupStatus}</p>{/if}
    </div>
  </header>

  {#if data.workdirsError}
    <p class="section-state error">{data.workdirsError}</p>
  {:else if !data.workdirs}
    <p class="section-state">Loading workdirs…</p>
  {:else if data.workdirs.items.length === 0}
    <p class="section-state">No workdirs are visible for this Runtime.</p>
  {:else}
    <div class="table-wrap">
      <table class="workdirs-table">
        <thead>
          <tr>
            <th>Workdir</th>
            <th>Repository</th>
            <th>Selector</th>
            <th>Commit</th>
            <th>Status</th>
            <th>Policy</th>
            <th>Action</th>
          </tr>
        </thead>
        <tbody>
          {#each data.workdirs.items as workdir}
            {@const cleanup = cleanupCandidate(workdir)}
            <tr>
              <td><code>{workdir.working_directory_id}</code></td>
              <td>{workdir.repository_id}</td>
              <td>{selectorLabel(workdir)}</td>
              <td><code>{commitLabel(workdir)}</code></td>
              <td>{workdir.status}</td>
              <td>
                <span>{workdir.dirty_state_policy}</span>
                <small>{workdir.cleanup_policy}</small>
              </td>
              <td>
                {#if cleanup}
                  <button
                    type="button"
                    disabled={!!cleanup.blocking_reason || cleanupBusyTarget === cleanup.target_id}
                    title={cleanup.blocking_reason ?? cleanup.reason}
                    onclick={() => executeWorkdirCleanup(cleanup)}
                  >
                    {cleanupBusyTarget === cleanup.target_id ? 'Executing…' : cleanupLabel(cleanup)}
                  </button>
                  {#if cleanup.blocking_reason}<small class="error">{cleanup.blocking_reason}</small>{/if}
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
