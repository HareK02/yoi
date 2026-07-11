<script lang="ts">
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import type { CleanupWorkdirCandidate, WorkingDirectorySummary } from '$lib/workspace-sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let selectedCleanupTargets = $state(new Set<string>());
  let selectedWorkerCleanupTargets = $state(new Set<string>());
  let confirmedDirtyTargets = $state(new Set<string>());
  let cleanupStatus = $state<string | null>(null);
  let cleanupBusy = $state(false);
  let cleanupCandidates = $derived(data.cleanupPlan?.workdirs ?? []);
  let workerCleanupCandidates = $derived(data.cleanupPlan?.workers ?? []);
  let runtimeLabel = $derived(
    data.runtimes?.items.find((runtime) => runtime.runtime_id === data.runtimeId)?.label ?? data.runtimeId,
  );

  function commitLabel(workdir: WorkingDirectorySummary): string {
    return workdir.resolved_commit ? workdir.resolved_commit.slice(0, 12) : '—';
  }

  function selectorLabel(workdir: WorkingDirectorySummary): string {
    return workdir.requested_selector ?? 'HEAD';
  }

  function cleanupLabel(candidate: CleanupWorkdirCandidate): string {
    if (candidate.action === 'workdir_dirty_discard') {
      return candidate.cleanliness === 'dirty' ? 'Discard dirty workdir' : 'Discard unknown-state workdir';
    }
    if (candidate.action === 'workdir_record_delete') return 'Delete missing/removed record';
    return 'Clean up verified-clean workdir';
  }

  function toggleSelected(targetId: string): void {
    const next = new Set(selectedCleanupTargets);
    if (next.has(targetId)) next.delete(targetId);
    else next.add(targetId);
    selectedCleanupTargets = next;
  }

  function toggleWorkerSelected(targetId: string): void {
    const next = new Set(selectedWorkerCleanupTargets);
    if (next.has(targetId)) next.delete(targetId);
    else next.add(targetId);
    selectedWorkerCleanupTargets = next;
  }

  function toggleDirtyConfirmation(targetId: string): void {
    const next = new Set(confirmedDirtyTargets);
    if (next.has(targetId)) next.delete(targetId);
    else next.add(targetId);
    confirmedDirtyTargets = next;
  }

  async function executeCleanup(): Promise<void> {
    if (!data.cleanupPlan || (selectedCleanupTargets.size === 0 && selectedWorkerCleanupTargets.size === 0)) return;
    cleanupBusy = true;
    cleanupStatus = null;
    try {
      const response = await fetch(
        workspaceApiPath(data.workspaceId, `/runtimes/${encodeURIComponent(data.runtimeId)}/cleanup-executions`),
        {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            expected_plan_revision: data.cleanupPlan.revision,
            expected_plan_digest: data.cleanupPlan.digest,
            worker_target_ids: Array.from(selectedWorkerCleanupTargets),
            workdir_target_ids: Array.from(selectedCleanupTargets),
            confirm_dirty_discard_target_ids: Array.from(confirmedDirtyTargets),
          }),
        },
      );
      const payload = await response.json().catch(() => null);
      if (!response.ok) throw new Error(payload?.message ?? payload?.error ?? response.statusText);
      cleanupStatus = `Executed ${payload?.results?.length ?? 0} cleanup action(s). Refresh to see the latest plan.`;
      selectedCleanupTargets = new Set();
      selectedWorkerCleanupTargets = new Set();
      confirmedDirtyTargets = new Set();
    } catch (error) {
      cleanupStatus = error instanceof Error ? error.message : 'Cleanup failed';
    } finally {
      cleanupBusy = false;
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
    </div>
  </header>

  {#if data.workdirsError}
    <p class="section-state error">{data.workdirsError}</p>
  {:else if !data.workdirs}
    <p class="section-state">Loading workdirs…</p>
  {:else if data.workdirs.items.length === 0}
    <p class="section-state">No workdirs are visible for this Runtime.</p>
  {:else}
    <section class="cleanup-panel">
      <div>
        <h2>Manual cleanup preview</h2>
        <p class="muted">Select explicit Workdir targets. Raw Runtime paths are intentionally not shown.</p>
      </div>
      {#if data.cleanupPlanError}
        <p class="section-state error">{data.cleanupPlanError}</p>
      {:else if cleanupCandidates.length === 0 && workerCleanupCandidates.length === 0}
        <p>No cleanup candidates.</p>
      {:else}
        <div class="cleanup-list">
          {#each workerCleanupCandidates as candidate (candidate.target_id)}
            <label class:blocked={!!candidate.blocking_reason}>
              <input
                type="checkbox"
                checked={selectedWorkerCleanupTargets.has(candidate.target_id)}
                disabled={!!candidate.blocking_reason}
                onchange={() => toggleWorkerSelected(candidate.target_id)}
              />
              <span>
                <strong>Delete Worker registry row:</strong> <code>{candidate.runtime_worker_id}</code>
                <small>{candidate.retention_state}; linked Workdirs {candidate.linked_workdir_ids.length}</small>
                {#if candidate.blocking_reason}<small class="error">Blocked: {candidate.blocking_reason}</small>{/if}
              </span>
            </label>
          {/each}
          {#each cleanupCandidates as candidate (candidate.target_id)}
            <label class:blocked={!!candidate.blocking_reason} class:dirty={candidate.action === 'workdir_dirty_discard'}>
              <input
                type="checkbox"
                checked={selectedCleanupTargets.has(candidate.target_id)}
                disabled={!!candidate.blocking_reason}
                onchange={() => toggleSelected(candidate.target_id)}
              />
              <span>
                <strong>{cleanupLabel(candidate)}:</strong> <code>{candidate.workdir_id}</code>
                <small>
                  file {candidate.file_status}; {candidate.cleanliness}; linked Workers {candidate.linked_worker_ids.length}
                </small>
                {#if candidate.blocking_reason}
                  <small class="error">Blocked: {candidate.blocking_reason}</small>
                {:else if candidate.action === 'workdir_dirty_discard'}
                  <label class="confirm-dirty">
                    <input
                      type="checkbox"
                      checked={confirmedDirtyTargets.has(candidate.target_id)}
                      onchange={() => toggleDirtyConfirmation(candidate.target_id)}
                    />
                    Confirm discard
                  </label>
                {/if}
              </span>
            </label>
          {/each}
        </div>
        <button type="button" onclick={executeCleanup} disabled={cleanupBusy || (selectedCleanupTargets.size === 0 && selectedWorkerCleanupTargets.size === 0)}>
          {cleanupBusy ? 'Executing…' : 'Execute selected cleanup'}
        </button>
        {#if cleanupStatus}<p>{cleanupStatus}</p>{/if}
      {/if}
    </section>

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
          </tr>
        </thead>
        <tbody>
          {#each data.workdirs.items as workdir}
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
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
