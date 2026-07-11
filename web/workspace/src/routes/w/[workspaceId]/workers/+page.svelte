<script lang="ts">
  import { workspaceApiPath } from '$lib/workspace-api/http';
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import { canOpenWorkerConsole } from '$lib/workspace-sidebar/workers';
  import type { CleanupWorkerCandidate, RuntimeCleanupExecutionResponse, RuntimeCleanupPlanResponse, Worker } from '$lib/workspace-sidebar/types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let statusMessage = $state<string | null>(null);
  let cleanupPlans = $state<Record<string, RuntimeCleanupPlanResponse>>({});
  let busyCleanupTarget = $state<string | null>(null);

  $effect(() => {
    cleanupPlans = data.cleanupPlans;
  });

  async function setPinned(worker: Worker, pinned: boolean): Promise<void> {
    statusMessage = null;
    const response = await fetch(
      workspaceApiPath(
        data.workspaceId,
        `/runtimes/${encodeURIComponent(worker.runtime_id)}/workers/${encodeURIComponent(worker.worker_id)}/pin`,
      ),
      { method: pinned ? 'PUT' : 'DELETE' },
    );
    const payload = await response.json().catch(() => null);
    if (!response.ok) {
      statusMessage = payload?.message ?? payload?.error ?? response.statusText;
      return;
    }
    worker.pinned = Boolean(payload?.pinned);
    worker.retention_state = payload?.retention_state ?? (worker.pinned ? 'pinned' : 'normal');
    statusMessage = `${worker.label} ${worker.pinned ? 'pinned' : 'unpinned'}.`;
  }

  function cleanupCandidate(worker: Worker): CleanupWorkerCandidate | undefined {
    return cleanupPlans?.[worker.runtime_id]?.workers.find(
      (candidate) => candidate.runtime_id === worker.runtime_id && candidate.runtime_worker_id === worker.worker_id,
    );
  }

  async function deleteWorkerRegistryRow(worker: Worker, candidate: CleanupWorkerCandidate): Promise<void> {
    if (!cleanupPlans?.[worker.runtime_id]) return;
    statusMessage = null;
    busyCleanupTarget = candidate.target_id;
    try {
      const plan = cleanupPlans[worker.runtime_id];
      const response = await fetch(
        workspaceApiPath(data.workspaceId, `/runtimes/${encodeURIComponent(worker.runtime_id)}/cleanup-executions`),
        {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: [candidate.target_id],
            workdir_target_ids: [],
            confirm_dirty_discard_target_ids: [],
          }),
        },
      );
      const payload = (await response.json().catch(() => null)) as RuntimeCleanupExecutionResponse | { message?: string; error?: string } | null;
      if (!response.ok) throw new Error(payload && 'message' in payload ? (payload.message ?? payload.error) : response.statusText);
      if (payload && 'plan_after' in payload) {
        cleanupPlans = { ...cleanupPlans, [worker.runtime_id]: payload.plan_after };
      }
      if (data.workers) {
        data.workers.items = data.workers.items.filter(
          (item) => !(item.runtime_id === worker.runtime_id && item.worker_id === worker.worker_id),
        );
      }
      statusMessage = `Deleted Worker registry row for ${worker.label}.`;
    } catch (error) {
      statusMessage = error instanceof Error ? error.message : 'Worker cleanup failed';
    } finally {
      busyCleanupTarget = null;
    }
  }

  function workerStatus(worker: Worker): string {
    return worker.state;
  }

  function workerProfile(worker: Worker): string {
    return worker.profile ?? worker.role ?? 'unknown';
  }

  function workerDirectory(worker: Worker): string {
    const directory = worker.working_directory;
    if (!directory) return '—';
    const selector = directory.requested_selector ?? 'HEAD';
    const commit = directory.resolved_commit ? directory.resolved_commit.slice(0, 12) : null;
    return commit ? `${directory.repository_id} · ${selector} · ${commit}` : `${directory.repository_id} · ${selector}`;
  }
</script>

<svelte:head>
  <title>Workers · Yoi Workspace</title>
  <meta name="description" content="Workspace Workers" />
</svelte:head>

<section class="workers-page" aria-labelledby="workers-heading">
  <header class="workers-page-header">
    <div>
      <h1 id="workers-heading">Workers</h1>
      <p>Workers running or persisted for this workspace. Pinning only updates Backend retention.</p>
      {#if statusMessage}<p>{statusMessage}</p>{/if}
    </div>
    <a class="section-action" href={`/w/${data.workspaceId}/workers/new`}>New Worker</a>
  </header>

  {#if data.workersError}
    <p class="section-state error">{data.workersError}</p>
  {:else if !data.workers}
    <p class="section-state">Loading Workers…</p>
  {:else if data.workers.items.length === 0}
    <p class="section-state">No Workers are visible.</p>
  {:else}
    <div class="table-wrap workers-table-wrap">
      <table class="workers-table">
        <thead>
          <tr>
            <th>Worker</th>
            <th>Runtime</th>
            <th>Profile</th>
            <th>Status</th>
            <th>Retention</th>
            <th>Workdir</th>
            <th>Action</th>
          </tr>
        </thead>
        <tbody>
          {#each data.workers.items as worker}
            {@const cleanup = cleanupCandidate(worker)}
            <tr>
              <td>
                <strong>{worker.label}</strong>
                <small><code>{worker.worker_id}</code></small>
              </td>
              <td><code>{worker.runtime_id}</code></td>
              <td>{workerProfile(worker)}</td>
              <td>{workerStatus(worker)}</td>
              <td><span class="pill {worker.pinned ? 'success' : 'muted'}">{worker.retention_state ?? 'normal'}</span></td>
              <td>{workerDirectory(worker)}</td>
              <td>
                {#if canOpenWorkerConsole(worker)}
                  <a class="inline-link" href={workerConsoleHref(worker, data.workspaceId)}>Open Console</a>
                {:else}
                  <span class="muted" aria-disabled="true">Archived</span>
                {/if}
                <button type="button" onclick={() => setPinned(worker, !worker.pinned)}>
                  {worker.pinned ? 'Unpin' : 'Pin'}
                </button>
                {#if cleanup}
                  <button
                    type="button"
                    disabled={!!cleanup.blocking_reason || busyCleanupTarget === cleanup.target_id}
                    title={cleanup.blocking_reason ?? cleanup.reason}
                    onclick={() => deleteWorkerRegistryRow(worker, cleanup)}
                  >
                    {busyCleanupTarget === cleanup.target_id ? 'Deleting…' : 'Delete row'}
                  </button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
