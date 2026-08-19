<script lang="ts">
  import { pushWorkspaceAlert } from '$lib/workspace/alerts/store';
  import { workspaceApiPath } from '$lib/workspace/api/http';
  import { workerHref } from '$lib/workspace/resource-links';
  import { formatCurrentWorkdirRevision } from '$lib/workspace/settings/workdir-revision';
  import { canOpenWorkerConsole } from '$lib/workspace/sidebar/workers';
  import type { CleanupWorkerCandidate, RuntimeCleanupExecutionResponse, RuntimeCleanupPlanResponse, Worker } from '$lib/workspace/sidebar/types';
  import type { PageProps } from './$types';

  type WorkerActionKind = 'pin' | 'delete';

  let { data }: PageProps = $props();
  let cleanupPlans = $state<Record<string, RuntimeCleanupPlanResponse>>({});
  let workers = $state<Worker[]>([]);
  let busyAction = $state<{ workerKey: string; kind: WorkerActionKind } | null>(null);

  $effect(() => {
    cleanupPlans = data.cleanupPlans;
    workers = data.workers?.items ?? [];
  });

  function workerKey(worker: Worker): string {
    return `${worker.runtime_id}/${worker.worker_id}`;
  }

  function isActionBusy(worker: Worker, kind: WorkerActionKind): boolean {
    return busyAction?.workerKey === workerKey(worker) && busyAction.kind === kind;
  }

  function actionsDisabled(): boolean {
    return busyAction !== null;
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

  async function refreshCleanupPlan(runtimeId: string): Promise<void> {
    const response = await fetch(
      workspaceApiPath(data.workspaceId, `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`),
    );
    if (!response.ok) return;
    const plan = (await response.json()) as RuntimeCleanupPlanResponse;
    cleanupPlans = { ...cleanupPlans, [runtimeId]: plan };
  }

  async function setPinned(worker: Worker, pinned: boolean): Promise<void> {
    if (busyAction) return;
    busyAction = { workerKey: workerKey(worker), kind: 'pin' };
    try {
      const response = await fetch(
        workspaceApiPath(
          data.workspaceId,
          `/runtimes/${encodeURIComponent(worker.runtime_id)}/workers/${encodeURIComponent(worker.worker_id)}/pin`,
        ),
        { method: pinned ? 'PUT' : 'DELETE' },
      );
      const payload = await response.json().catch(() => null);
      if (!response.ok) {
        pushWorkspaceAlert('error', errorMessage(payload, response.statusText), { title: 'Worker pin failed' });
        return;
      }
      worker.pinned = Boolean(payload?.pinned);
      worker.retention_state = payload?.retention_state ?? (worker.pinned ? 'pinned' : 'normal');
      await refreshCleanupPlan(worker.runtime_id);
    } catch (error) {
      pushWorkspaceAlert('error', error instanceof Error ? error.message : 'Worker pin failed', {
        title: 'Worker pin failed',
      });
    } finally {
      busyAction = null;
    }
  }

  function cleanupCandidate(worker: Worker): CleanupWorkerCandidate | undefined {
    return cleanupPlans?.[worker.runtime_id]?.workers.find(
      (candidate) => candidate.runtime_id === worker.runtime_id && candidate.runtime_worker_id === worker.worker_id,
    );
  }

  async function deleteWorker(worker: Worker, candidate: CleanupWorkerCandidate): Promise<void> {
    if (!cleanupPlans?.[worker.runtime_id] || busyAction) return;
    busyAction = { workerKey: workerKey(worker), kind: 'delete' };
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
      const payload = (await response.json().catch(() => null)) as RuntimeCleanupExecutionResponse | unknown;
      if (!response.ok) throw new Error(errorMessage(payload, response.statusText));
      if (payload && typeof payload === 'object' && 'plan_after' in payload) {
        const execution = payload as RuntimeCleanupExecutionResponse;
        cleanupPlans = { ...cleanupPlans, [worker.runtime_id]: execution.plan_after };
      }
      const result = payload && typeof payload === 'object' && 'results' in payload
        ? (payload as RuntimeCleanupExecutionResponse).results.find((entry) => entry.target_id === candidate.target_id)
        : undefined;
      if (!result || result.status !== 'deleted') {
        throw new Error(result?.message ?? 'Runtime did not delete the selected Worker');
      }
      workers = workers.filter(
        (item) => !(item.runtime_id === worker.runtime_id && item.worker_id === worker.worker_id),
      );
    } catch (error) {
      pushWorkspaceAlert('error', error instanceof Error ? error.message : 'Worker deletion failed', {
        title: 'Worker deletion failed',
      });
    } finally {
      busyAction = null;
    }
  }

  function workerStatus(worker: Worker): string {
    return worker.state;
  }

  function workerProfile(worker: Worker): string {
    return worker.profile ?? 'unknown';
  }

  function workerDirectory(worker: Worker): string {
    const directory = worker.working_directory;
    if (!directory) return '—';
    const provider = data.repositories?.items.find((repository) => repository.id === directory.repository_id)
      ?.provider;
    return `${directory.repository_id} · ${formatCurrentWorkdirRevision(directory, provider)}`;
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
      <p>Workers running or persisted for this workspace. Pinning updates Backend retention.</p>
    </div>
    <a class="section-action" href={`/w/${data.workspaceId}/workers/new`}>New Worker</a>
  </header>

  {#if data.workersError}
    <p class="section-state error">{data.workersError}</p>
  {:else if !data.workers}
    <p class="section-state">Loading Workers…</p>
  {:else if workers.length === 0}
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
          {#each workers as worker}
            {@const cleanup = cleanupCandidate(worker)}
            {@const canDelete = cleanup && !cleanup.blocking_reason}
            {@const anyActionDisabled = actionsDisabled()}
            {@const workerDisplayName = worker.display_name || worker.label}
            <tr>
              <td>
                {#if canOpenWorkerConsole(worker)}
                  <a class="worker-title-link" href={workerHref(data.workspaceId, worker)}><strong>{workerDisplayName}</strong></a>
                {:else}
                  <strong>{workerDisplayName}</strong>
                {/if}
                <small>worker <code>{worker.human_key ?? worker.worker_id}</code></small>
              </td>
              <td><code>{worker.runtime_id}</code></td>
              <td>{workerProfile(worker)}</td>
              <td>{workerStatus(worker)}</td>
              <td><span class="pill {worker.pinned ? 'success' : 'muted'}">{worker.retention_state ?? 'normal'}</span></td>
              <td>{workerDirectory(worker)}</td>
              <td>
                <div class="worker-actions" aria-label={`Actions for ${workerDisplayName}`}>
                  <button
                    class="icon-action"
                    type="button"
                    disabled={anyActionDisabled}
                    aria-label={worker.pinned ? `Unpin ${workerDisplayName}` : `Pin ${workerDisplayName}`}
                    title={worker.pinned ? 'Unpin' : 'Pin'}
                    onclick={() => setPinned(worker, !worker.pinned)}
                  >
                    {#if isActionBusy(worker, 'pin')}
                      <span class="spinner" aria-hidden="true"></span>
                    {:else if worker.pinned}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 17v5" /><path d="M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89" /><path d="m2 2 20 20" /><path d="M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11" /></svg>
                    {:else}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 17v5" /><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" /></svg>
                    {/if}
                  </button>
                  {#if cleanup}
                    <button
                      class="icon-action danger"
                      type="button"
                      disabled={!canDelete || anyActionDisabled}
                      aria-label={`Delete ${workerDisplayName}`}
                      title={cleanup.blocking_reason ?? cleanup.reason}
                      onclick={() => deleteWorker(worker, cleanup)}
                    >
                      {#if isActionBusy(worker, 'delete')}
                        <span class="spinner" aria-hidden="true"></span>
                      {:else}
                        <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path d="M3 6h18" /><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>
                      {/if}
                    </button>
                  {/if}
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .worker-title-link {
    color: inherit;
    text-decoration: none;
  }

  .worker-title-link:hover,
  .worker-title-link:focus-visible {
    color: var(--accent);
    text-decoration: underline;
  }

  .worker-actions {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

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

  .icon-action:hover:not(:disabled),
  .icon-action:focus-visible:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
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
    animation: worker-action-spin 0.8s linear infinite;
  }

  @keyframes worker-action-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
